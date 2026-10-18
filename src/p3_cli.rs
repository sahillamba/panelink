//! Experimental foreground server and reusable control command frontend.
//!
//! This is a narrow binary adapter over the reviewed launch, transport,
//! context, session, response, and server-owner APIs. It does not persist
//! request bodies, infer pane selection, retry operations, or own a host TTY.

use super::{capture_child_arguments, os_str_byte_len, prepare_probe, ProbeRequest};
use panelink::client_context::{ContextError, ContextFile, ReconcileResult};
use panelink::control_protocol::{
    encode_request_payload, Command as ControlCommand, ControlRequest, ReadSource, TargetRef,
    MAX_INPUT_TEXT_BYTES,
};
use panelink::event_journal::EventCursor;
use panelink::event_protocol::{decode_event, Event, EventBody};
use panelink::handshake_protocol::{
    validate_welcome, Hello, Welcome, CAPABILITY_EVENTS, CAPABILITY_INPUT, CAPABILITY_READ,
};
use panelink::protocol::{OperationKey, OperationStatus, Outcome};
use panelink::protocol_wire::{Frame, MessageKind};
use panelink::response_protocol::{decode_response_payload, RejectionCode, ResponseBody};
use panelink::server_startup::StartedServer;
use panelink::server_storage::ServerStorage;
use panelink::session_protocol::{
    decode_session_info, encode_session_hello, encode_session_request, LeaseRef, SessionHello,
    SessionInfo, SessionRequest,
};
use panelink::transport::{AuthenticatedStream, PrivateListener};
use panelink::watch_protocol::{
    decode_reconciliation, decode_watch_end, encode_watch_request, Reconciliation, WatchEnd,
    WatchRequest, WatchStatus, MAX_WATCH_EVENTS, MAX_WATCH_WAIT_MS,
};
use std::ffi::{OsStr, OsString};
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{self, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CLIENT_DEADLINE: Duration = Duration::from_secs(2);
const SERVER_ACCEPT_SLICE: Duration = Duration::from_millis(1);
const SERVER_READ_BYTES: u32 = 4_096;
const SERVER_READ_LINES: u16 = 24;
const OWNER_TTL_SECONDS: u32 = 30;
const MAX_CONTROL_ARGUMENT_BYTES: usize = 65_536;

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);
static NONCE_SEED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

enum CliCommand {
    Help,
    ServerProbe(ServerProbe),
    Control(Control),
}

struct ServerProbe {
    directory: PathBuf,
    terminfo: PathBuf,
    initialize: bool,
    allow_input: bool,
    executable: OsString,
    arguments: Vec<OsString>,
}

struct Control {
    directory: PathBuf,
    context_path: PathBuf,
    action: Action,
}

enum Action {
    Init,
    Show,
    Describe,
    Resync,
    Detach,
    Read(TargetRef),
    Reconcile(TargetRef),
    Watch {
        target: TargetRef,
        cursor: EventCursor,
        limit: u16,
        wait_ms: u16,
    },
    Claim {
        target: TargetRef,
        owner_revision: u64,
    },
    Release {
        target: TargetRef,
        owner_revision: u64,
    },
    Handoff {
        target: TargetRef,
        owner_revision: u64,
        recipient_lease: u64,
    },
    Text {
        target: TargetRef,
        owner_revision: u64,
        text: String,
    },
    Paste {
        target: TargetRef,
        owner_revision: u64,
        text: String,
    },
    Submit {
        target: TargetRef,
        owner_revision: u64,
        text: String,
    },
    Interrupt {
        target: TargetRef,
        owner_revision: u64,
    },
    Status(u64),
    Receipt(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReceiptReply {
    Status(OperationStatus),
    Rejected(RejectionCode),
}

pub(super) fn run(args: Vec<OsString>) -> i32 {
    let command = match parse(args) {
        Ok(command) => command,
        Err(()) => return emit_error("invalid_arguments", 2),
    };
    match command {
        CliCommand::Help => {
            print_help();
            0
        }
        CliCommand::ServerProbe(request) => run_server_probe(request),
        CliCommand::Control(request) => run_control(request),
    }
}

fn parse(args: Vec<OsString>) -> Result<CliCommand, ()> {
    if args.len() > 268 {
        return Err(());
    }
    if (args.len() == 1 && matches_os(&args[0], &["--help", "-h"]))
        || (args.len() == 2
            && matches_os(&args[0], &["server-probe", "control"])
            && matches_os(&args[1], &["--help", "-h"]))
    {
        return Ok(CliCommand::Help);
    }
    let Some(first) = args.first() else {
        return Err(());
    };
    if first == OsStr::new("server-probe") {
        parse_server_probe(&args[1..]).map(CliCommand::ServerProbe)
    } else if first == OsStr::new("control") {
        parse_control(&args[1..]).map(CliCommand::Control)
    } else {
        Err(())
    }
}

fn parse_server_probe(args: &[OsString]) -> Result<ServerProbe, ()> {
    let mut index = 0;
    expect_arg(args, &mut index, "--directory")?;
    let directory = next_path(args, &mut index)?;
    expect_arg(args, &mut index, "--terminfo")?;
    let terminfo = next_path(args, &mut index)?;
    let mut initialize = false;
    let mut allow_input = false;
    while args.get(index).is_some_and(|arg| arg != OsStr::new("--")) {
        match args[index].to_str() {
            Some("--initialize") if !initialize => initialize = true,
            Some("--allow-input") if !allow_input => allow_input = true,
            _ => return Err(()),
        }
        index += 1;
    }
    expect_arg(args, &mut index, "--")?;
    let executable = args.get(index).ok_or(())?.clone();
    index += 1;
    if executable.is_empty() {
        return Err(());
    }
    let arguments =
        capture_child_arguments(&executable, args[index..].iter().cloned()).map_err(|_| ())?;
    if !arguments_within_limit(args, MAX_CONTROL_ARGUMENT_BYTES) {
        return Err(());
    }
    Ok(ServerProbe {
        directory,
        terminfo,
        initialize,
        allow_input,
        executable,
        arguments,
    })
}

fn parse_control(args: &[OsString]) -> Result<Control, ()> {
    if args.len() > 9 || !arguments_within_limit(args, MAX_CONTROL_ARGUMENT_BYTES) {
        return Err(());
    }
    let mut index = 0;
    expect_arg(args, &mut index, "--directory")?;
    let directory = next_path(args, &mut index)?;
    expect_arg(args, &mut index, "--context")?;
    let context_path = next_path(args, &mut index)?;
    let action_name = next_utf8(args, &mut index)?;
    let action = match action_name {
        "init" => Action::Init,
        "show" => Action::Show,
        "describe" => Action::Describe,
        "resync" => Action::Resync,
        "detach" => Action::Detach,
        "read" => Action::Read(parse_target(next_utf8(args, &mut index)?)?),
        "reconcile" => Action::Reconcile(parse_target(next_utf8(args, &mut index)?)?),
        "watch" => {
            let target = parse_target(next_utf8(args, &mut index)?)?;
            let cursor = parse_cursor(next_utf8(args, &mut index)?)?;
            let limit = u16::try_from(parse_nonzero_decimal(next_utf8(args, &mut index)?)?)
                .map_err(|_| ())?;
            let wait_ms =
                u16::try_from(parse_decimal(next_utf8(args, &mut index)?)?).map_err(|_| ())?;
            if target.generation != cursor.generation
                || limit > MAX_WATCH_EVENTS
                || wait_ms > MAX_WATCH_WAIT_MS
            {
                return Err(());
            }
            Action::Watch {
                target,
                cursor,
                limit,
                wait_ms,
            }
        }
        "claim" => Action::Claim {
            target: parse_target(next_utf8(args, &mut index)?)?,
            owner_revision: parse_nonzero_decimal(next_utf8(args, &mut index)?)?,
        },
        "release" => Action::Release {
            target: parse_target(next_utf8(args, &mut index)?)?,
            owner_revision: parse_nonzero_decimal(next_utf8(args, &mut index)?)?,
        },
        "handoff" => Action::Handoff {
            target: parse_target(next_utf8(args, &mut index)?)?,
            owner_revision: parse_nonzero_decimal(next_utf8(args, &mut index)?)?,
            recipient_lease: parse_nonzero_decimal(next_utf8(args, &mut index)?)?,
        },
        "text" | "paste" | "submit" => {
            let target = parse_target(next_utf8(args, &mut index)?)?;
            let owner_revision = parse_nonzero_decimal(next_utf8(args, &mut index)?)?;
            let text = next_utf8(args, &mut index)?.to_owned();
            if text.len() > MAX_INPUT_TEXT_BYTES {
                return Err(());
            }
            match action_name {
                "text" => Action::Text {
                    target,
                    owner_revision,
                    text,
                },
                "paste" => Action::Paste {
                    target,
                    owner_revision,
                    text,
                },
                _ => Action::Submit {
                    target,
                    owner_revision,
                    text,
                },
            }
        }
        "interrupt" => Action::Interrupt {
            target: parse_target(next_utf8(args, &mut index)?)?,
            owner_revision: parse_nonzero_decimal(next_utf8(args, &mut index)?)?,
        },
        "status" => Action::Status(parse_nonzero_decimal(next_utf8(args, &mut index)?)?),
        "receipt" => Action::Receipt(parse_nonzero_decimal(next_utf8(args, &mut index)?)?),
        _ => return Err(()),
    };
    if index != args.len() {
        return Err(());
    }
    Ok(Control {
        directory,
        context_path,
        action,
    })
}

fn expect_arg(args: &[OsString], index: &mut usize, expected: &str) -> Result<(), ()> {
    if args
        .get(*index)
        .is_some_and(|arg| arg == OsStr::new(expected))
    {
        *index += 1;
        Ok(())
    } else {
        Err(())
    }
}

fn next_path(args: &[OsString], index: &mut usize) -> Result<PathBuf, ()> {
    let path = PathBuf::from(args.get(*index).ok_or(())?);
    *index += 1;
    if !path.is_absolute() || os_str_byte_len(path.as_os_str()) > 1_024 {
        return Err(());
    }
    Ok(path)
}

fn next_utf8<'a>(args: &'a [OsString], index: &mut usize) -> Result<&'a str, ()> {
    let value = args.get(*index).ok_or(())?.to_str().ok_or(())?;
    *index += 1;
    Ok(value)
}

fn parse_nonzero_decimal(value: &str) -> Result<u64, ()> {
    let value = parse_decimal(value)?;
    if value == 0 {
        Err(())
    } else {
        Ok(value)
    }
}

fn parse_decimal(value: &str) -> Result<u64, ()> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(());
    }
    value.parse::<u64>().map_err(|_| ())
}

fn parse_cursor(value: &str) -> Result<EventCursor, ()> {
    let (generation, sequence) = value.split_once(':').ok_or(())?;
    Ok(EventCursor {
        generation: parse_nonzero_decimal(generation)?,
        sequence: parse_decimal(sequence)?,
    })
}

fn parse_target(value: &str) -> Result<TargetRef, ()> {
    let mut fields = value.split(':');
    let target = TargetRef {
        generation: parse_nonzero_decimal(fields.next().ok_or(())?)?,
        workspace: parse_nonzero_decimal(fields.next().ok_or(())?)?,
        pane: parse_nonzero_decimal(fields.next().ok_or(())?)?,
        session: parse_nonzero_decimal(fields.next().ok_or(())?)?,
        occupant_revision: parse_nonzero_decimal(fields.next().ok_or(())?)?,
    };
    if fields.next().is_some() {
        return Err(());
    }
    Ok(target)
}

fn matches_os(value: &OsStr, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| value == OsStr::new(candidate))
}

fn arguments_within_limit(args: &[OsString], maximum: usize) -> bool {
    args.iter()
        .try_fold(0_usize, |total, argument| {
            total
                .checked_add(os_str_byte_len(argument))
                .filter(|next| *next <= maximum)
        })
        .is_some()
}

fn run_control(control: Control) -> i32 {
    match control.action {
        Action::Init => run_init(&control.directory, &control.context_path),
        Action::Show => run_show(&control.context_path),
        action => run_existing_control(control.directory, control.context_path, action),
    }
}

fn run_show(path: &Path) -> i32 {
    let context = match ContextFile::open_existing(path, deadline_after(CLIENT_DEADLINE)) {
        Ok(context) => context,
        Err(error) => return emit_failure(context_error_label(&error)),
    };
    emit_context(&context, "show")
}

fn run_init(directory: &Path, context_path: &Path) -> i32 {
    match fs::symlink_metadata(context_path) {
        Ok(_) => return emit_failure("context_exists"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return emit_failure("context_unavailable"),
    }
    let mut stream = match connect_stream(directory) {
        Ok(stream) => stream,
        Err(error) => return emit_failure(error),
    };
    let welcome = match handshake(&mut stream, None) {
        Ok(welcome) => welcome,
        Err(error) => return emit_failure(error),
    };
    let context = match ContextFile::create(
        context_path,
        welcome.generation,
        welcome.lease,
        deadline_after(CLIENT_DEADLINE),
    ) {
        Ok(context) => context,
        Err(error) => return emit_failure(context_error_label(&error)),
    };
    emit_context(&context, "init")
}

fn run_existing_control(directory: PathBuf, path: PathBuf, action: Action) -> i32 {
    let mut context = match ContextFile::open_existing(&path, deadline_after(CLIENT_DEADLINE)) {
        Ok(context) => context,
        Err(error) => return emit_failure(context_error_label(&error)),
    };

    if matches!(&action, Action::Reconcile(_) | Action::Watch { .. }) {
        let mut stream = match connect_stream(&directory) {
            Ok(stream) => stream,
            Err(error) => return emit_failure(error),
        };
        if let Err(error) = handshake_mode(&mut stream, Some(context.context()), true) {
            return emit_failure(error);
        }
        return match action {
            Action::Reconcile(target) => reconcile_command(&mut stream, target),
            Action::Watch {
                target,
                cursor,
                limit,
                wait_ms,
            } => {
                let request = WatchRequest::Watch {
                    target,
                    cursor,
                    limit,
                    wait_ms,
                };
                match exchange_watch(&mut stream, request) {
                    Ok((events, end)) => emit_watch(&events, end),
                    Err(error) => emit_failure(error),
                }
            }
            _ => unreachable!("observation action was matched"),
        };
    }

    if matches!(&action, Action::Resync) {
        let Some(key) = context.context().uncertain() else {
            return emit_resync_clear(None, None, None);
        };
        let mut stream = match connect_stream(&directory) {
            Ok(stream) => stream,
            Err(error) => return emit_failure(error),
        };
        if let Err(error) = handshake(&mut stream, Some(context.context())) {
            return emit_failure(error);
        }
        let payload = match encode_session_request(&SessionRequest::OperationStatus { key }) {
            Ok(payload) => payload,
            Err(_) => return emit_failure("request_invalid"),
        };
        let bytes = match exchange(&mut stream, &payload) {
            Ok(bytes) => bytes,
            Err(error) => return emit_failure(error),
        };
        let response = match decode_response_payload(&bytes) {
            Ok(response) if response.key == key => response,
            _ => return emit_failure("response_invalid"),
        };
        let ResponseBody::Status(status) = response.body else {
            return emit_failure("response_invalid");
        };
        let result = match context.reconcile(key, status) {
            Ok(result) => result,
            Err(error) => return emit_failure(context_error_label(&error)),
        };
        return emit_resync_clear(Some(key), Some(status), Some(result));
    }

    if let Action::Status(sequence) = &action {
        let state = context.context();
        let key = OperationKey {
            generation: state.generation(),
            lease: state.lease(),
            sequence: *sequence,
        };
        let mut stream = match connect_stream(&directory) {
            Ok(stream) => stream,
            Err(error) => return emit_failure(error),
        };
        if let Err(error) = handshake(&mut stream, Some(state)) {
            return emit_failure(error);
        }
        let payload = match encode_session_request(&SessionRequest::OperationStatus { key }) {
            Ok(payload) => payload,
            Err(_) => return emit_failure("request_invalid"),
        };
        let bytes = match exchange(&mut stream, &payload) {
            Ok(bytes) => bytes,
            Err(error) => return emit_failure(error),
        };
        let response = match decode_response_payload(&bytes) {
            Ok(response) if response.key == key => response,
            _ => return emit_failure("response_invalid"),
        };
        return match response.body {
            ResponseBody::Status(status) => emit_status("status", key, status, None),
            ResponseBody::Rejected(code) => emit_rejected("status", key, code, false),
            ResponseBody::Read { .. } => emit_failure("response_invalid"),
        };
    }

    if let Action::Receipt(through_sequence) = &action {
        let (key, mut stream) =
            match connect_for_receipt(&context, *through_sequence, || connect_stream(&directory)) {
                Ok(receipt) => receipt,
                Err(error) => return emit_failure(error),
            };
        if let Err(error) = handshake(&mut stream, Some(context.context())) {
            return emit_failure(error);
        }
        return emit_receipt(&context, &mut stream, key);
    }

    let mut stream = match connect_stream(&directory) {
        Ok(stream) => stream,
        Err(error) => return emit_failure(error),
    };
    if let Err(error) = handshake(&mut stream, Some(context.context())) {
        return emit_failure(error);
    }

    match action {
        Action::Describe => describe(&mut stream),
        Action::Detach => detach(&mut stream),
        Action::Read(target) => control_read(&mut context, &mut stream, target),
        Action::Claim {
            target,
            owner_revision,
        } => session_mutation(
            &mut context,
            &mut stream,
            "claim",
            target,
            owner_revision,
            Mutation::Claim,
        ),
        Action::Release {
            target,
            owner_revision,
        } => session_mutation(
            &mut context,
            &mut stream,
            "release",
            target,
            owner_revision,
            Mutation::Release,
        ),
        Action::Handoff {
            target,
            owner_revision,
            recipient_lease,
        } => session_mutation(
            &mut context,
            &mut stream,
            "handoff",
            target,
            owner_revision,
            Mutation::Handoff(recipient_lease),
        ),
        Action::Text {
            target,
            owner_revision,
            text,
        } => control_input(
            &mut context,
            &mut stream,
            "text",
            target,
            owner_revision,
            text,
            InputKind::Text,
        ),
        Action::Paste {
            target,
            owner_revision,
            text,
        } => control_input(
            &mut context,
            &mut stream,
            "paste",
            target,
            owner_revision,
            text,
            InputKind::Paste,
        ),
        Action::Submit {
            target,
            owner_revision,
            text,
        } => control_input(
            &mut context,
            &mut stream,
            "submit",
            target,
            owner_revision,
            text,
            InputKind::Submit,
        ),
        Action::Interrupt {
            target,
            owner_revision,
        } => control_interrupt(&mut context, &mut stream, target, owner_revision),
        Action::Init
        | Action::Show
        | Action::Resync
        | Action::Status(_)
        | Action::Receipt(_)
        | Action::Reconcile(_)
        | Action::Watch { .. } => emit_failure("invalid_arguments"),
    }
}

fn receipt_key(
    context: panelink::client_context::ClientContext,
    through_sequence: u64,
) -> Result<OperationKey, &'static str> {
    if context.uncertain().is_some() {
        return Err("operation_uncertain");
    }
    if through_sequence == 0 || through_sequence >= context.next_sequence() {
        return Err("receipt_out_of_range");
    }
    Ok(OperationKey {
        generation: context.generation(),
        lease: context.lease(),
        sequence: through_sequence,
    })
}

fn connect_for_receipt(
    context: &ContextFile,
    through_sequence: u64,
    connect: impl FnOnce() -> Result<AuthenticatedStream, &'static str>,
) -> Result<(OperationKey, AuthenticatedStream), &'static str> {
    let key = receipt_key(context.context(), through_sequence)?;
    let stream = connect()?;
    Ok((key, stream))
}

fn request_receipt(
    context: &ContextFile,
    stream: &mut AuthenticatedStream,
    key: OperationKey,
) -> Result<ReceiptReply, &'static str> {
    if receipt_key(context.context(), key.sequence)? != key {
        return Err("context_key_mismatch");
    }
    let payload = encode_session_request(&SessionRequest::Receipt {
        generation: key.generation,
        lease: key.lease,
        through_sequence: key.sequence,
    })
    .map_err(|_| "request_invalid")?;
    let bytes = exchange(stream, &payload)?;
    let response = decode_response_payload(&bytes).map_err(|_| "response_invalid")?;
    if response.key != key {
        return Err("response_invalid");
    }
    match response.body {
        ResponseBody::Status(status) => Ok(ReceiptReply::Status(status)),
        ResponseBody::Rejected(code) => Ok(ReceiptReply::Rejected(code)),
        ResponseBody::Read { .. } => Err("response_invalid"),
    }
}

fn emit_receipt(context: &ContextFile, stream: &mut AuthenticatedStream, key: OperationKey) -> i32 {
    match request_receipt(context, stream, key) {
        Ok(ReceiptReply::Status(status)) => emit_status("receipt", key, status, None),
        Ok(ReceiptReply::Rejected(code)) => emit_rejected("receipt", key, code, false),
        Err(error) => emit_failure(error),
    }
}

fn connect_stream(directory: &Path) -> Result<AuthenticatedStream, &'static str> {
    AuthenticatedStream::connect(directory, deadline_after(CLIENT_DEADLINE))
        .map_err(|_| "transport_unavailable")
}

fn handshake(
    stream: &mut AuthenticatedStream,
    resume: Option<panelink::client_context::ClientContext>,
) -> Result<Welcome, &'static str> {
    handshake_mode(stream, resume, false)
}

fn handshake_mode(
    stream: &mut AuthenticatedStream,
    resume: Option<panelink::client_context::ClientContext>,
    require_events: bool,
) -> Result<Welcome, &'static str> {
    let hello = Hello {
        nonce: fresh_nonce(),
        required_capabilities: CAPABILITY_READ | if require_events { CAPABILITY_EVENTS } else { 0 },
        optional_capabilities: CAPABILITY_INPUT
            | if require_events { 0 } else { CAPABILITY_EVENTS },
    };
    let session_hello = SessionHello {
        hello,
        resume: resume.map(|context| LeaseRef {
            generation: context.generation(),
            lease: context.lease(),
        }),
    };
    let payload = encode_session_hello(&session_hello).map_err(|_| "handshake_invalid")?;
    write_frame(stream, MessageKind::Hello, 0, &payload)?;
    let frame = read_frame(stream)?;
    if frame.kind() == MessageKind::Error && frame.request_id() == 0 {
        return Err("server_rejected");
    }
    if frame.kind() != MessageKind::Welcome || frame.request_id() != 0 {
        return Err("handshake_invalid");
    }
    let welcome = panelink::handshake_protocol::decode_welcome(frame.payload())
        .map_err(|_| "handshake_invalid")?;
    validate_welcome(&hello, &welcome).map_err(|_| "handshake_invalid")?;
    if let Some(context) = resume {
        if welcome.generation != context.generation() || welcome.lease != context.lease() {
            return Err("context_mismatch");
        }
    }
    if welcome.selected_capabilities & CAPABILITY_EVENTS != 0 {
        stream
            .enable_metadata_events()
            .map_err(|_| "transport_failed")?;
    }
    Ok(welcome)
}

fn exchange(stream: &mut AuthenticatedStream, payload: &[u8]) -> Result<Vec<u8>, &'static str> {
    write_frame(stream, MessageKind::Request, 1, payload)?;
    let frame = read_frame(stream)?;
    if frame.request_id() != 1 {
        return Err("response_invalid");
    }
    match frame.kind() {
        MessageKind::Response => Ok(frame.payload().to_vec()),
        MessageKind::Error => Err("server_rejected"),
        _ => Err("response_invalid"),
    }
}

fn reconcile_command(stream: &mut AuthenticatedStream, target: TargetRef) -> i32 {
    let request = WatchRequest::Reconcile {
        target,
        max_bytes: SERVER_READ_BYTES,
        max_lines: SERVER_READ_LINES,
    };
    let result = (|| {
        let payload = encode_watch_request(&request).map_err(|_| "request_invalid")?;
        let bytes = exchange(stream, &payload)?;
        let state = decode_reconciliation(&bytes).map_err(|_| "response_invalid")?;
        if state.info.target != target
            || state.cursor.generation != target.generation
            || bytes.len() > SERVER_READ_BYTES as usize
            || display_lines(state.text) > usize::from(SERVER_READ_LINES)
        {
            return Err("response_invalid");
        }
        Ok(reconciliation_json(&state))
    })();
    match result {
        Ok(json) => emit_json(&json, 0),
        Err(error) => emit_failure(error),
    }
}

fn display_lines(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        1 + text.bytes().filter(|byte| *byte == b'\n').count()
    }
}

fn reconciliation_json(state: &Reconciliation<'_>) -> String {
    let mut json = String::from("{\"ok\":true,\"action\":\"reconcile\",\"target\":");
    push_target(&mut json, state.info.target);
    let _ = write!(json,
        ",\"cursor\":\"{}:{}\",\"screen_revision\":{},\"owner_revision\":{},\"owner_lease\":{},\"child_exited\":{},\"truncated\":{},\"text\":",
        state.cursor.generation, state.cursor.sequence, state.screen_revision,
        state.info.owner_revision, state.info.owner_lease, state.info.child_exited,
        state.truncated);
    push_json_string(&mut json, state.text);
    json.push('}');
    json
}

fn exchange_watch(
    stream: &mut AuthenticatedStream,
    request: WatchRequest,
) -> Result<(Vec<Event>, WatchEnd), &'static str> {
    let WatchRequest::Watch {
        target,
        cursor,
        limit,
        ..
    } = request
    else {
        return Err("request_invalid");
    };
    let payload = encode_watch_request(&request).map_err(|_| "request_invalid")?;
    let deadline = deadline_after(CLIENT_DEADLINE);
    let frame = Frame::new(MessageKind::Request, 1, &payload).map_err(|_| "request_invalid")?;
    stream
        .write_frame(&frame, deadline)
        .map_err(|_| "transport_failed")?;
    let mut events = Vec::with_capacity(usize::from(limit));
    let mut last = cursor;
    // At most limit events plus exactly one terminal response. Nothing is
    // printed until the entire finite batch has validated.
    for _ in 0..=limit {
        let frame = stream
            .read_frame(deadline)
            .map_err(|_| "transport_failed")?;
        match (frame.kind(), frame.request_id()) {
            (MessageKind::Event, 0) => {
                if events.len() == usize::from(limit) {
                    return Err("response_invalid");
                }
                let event = decode_event(frame.payload()).map_err(|_| "response_invalid")?;
                if event.target != target || Some(event.cursor) != last.sequence.checked_add(1) {
                    return Err("response_invalid");
                }
                last.sequence = event.cursor;
                events.push(event);
            }
            (MessageKind::Response, 1) => {
                let end = decode_watch_end(frame.payload()).map_err(|_| "response_invalid")?;
                if usize::from(end.event_count) != events.len()
                    || end.cursor.generation != cursor.generation
                {
                    return Err("response_invalid");
                }
                match end.status {
                    WatchStatus::Changed if !events.is_empty() && end.cursor == last => {}
                    WatchStatus::TimedOut if events.is_empty() && end.cursor == cursor => {}
                    WatchStatus::ResyncRequired
                        if events.is_empty() && end.cursor.sequence >= cursor.sequence => {}
                    _ => return Err("response_invalid"),
                }
                return Ok((events, end));
            }
            (MessageKind::Error, 1) => return Err("server_rejected"),
            _ => return Err("response_invalid"),
        }
    }
    Err("response_invalid")
}

fn emit_watch(events: &[Event], end: WatchEnd) -> i32 {
    let changed = end.status == WatchStatus::Changed;
    let status = match end.status {
        WatchStatus::Changed => "changed",
        WatchStatus::TimedOut => "timed_out",
        WatchStatus::ResyncRequired => "resync_required",
    };
    let mut json = format!(
        "{{\"ok\":{changed},\"action\":\"watch\",\"status\":\"{status}\",\"cursor\":\"{}:{}\",\"events\":[",
        end.cursor.generation, end.cursor.sequence);
    for (index, event) in events.iter().enumerate() {
        if index != 0 {
            json.push(',');
        }
        let _ = write!(
            json,
            "{{\"cursor\":\"{}:{}\",\"target\":",
            event.target.generation, event.cursor
        );
        push_target(&mut json, event.target);
        match event.body {
            EventBody::ScreenChanged { revision } => {
                let _ = write!(json, ",\"kind\":\"screen_changed\",\"revision\":{revision}");
            }
            EventBody::InputOwnerChanged {
                revision,
                owner_lease,
            } => {
                let _ = write!(json, ",\"kind\":\"input_owner_changed\",\"revision\":{revision},\"owner_lease\":{owner_lease}");
            }
            EventBody::ChildExited => json.push_str(",\"kind\":\"child_exited\""),
        }
        json.push('}');
    }
    json.push_str("]}");
    emit_json(&json, if changed { 0 } else { 1 })
}

fn write_frame(
    stream: &mut AuthenticatedStream,
    kind: MessageKind,
    request_id: u64,
    payload: &[u8],
) -> Result<(), &'static str> {
    let frame = Frame::new(kind, request_id, payload).map_err(|_| "request_invalid")?;
    stream
        .write_frame(&frame, deadline_after(CLIENT_DEADLINE))
        .map_err(|_| "transport_failed")
}

fn read_frame(
    stream: &mut AuthenticatedStream,
) -> Result<panelink::transport::OwnedFrame, &'static str> {
    stream
        .read_frame(deadline_after(CLIENT_DEADLINE))
        .map_err(|_| "transport_failed")
}

fn describe(stream: &mut AuthenticatedStream) -> i32 {
    let payload = match encode_session_request(&SessionRequest::Describe) {
        Ok(payload) => payload,
        Err(_) => return emit_failure("request_invalid"),
    };
    let bytes = match exchange(stream, &payload) {
        Ok(bytes) => bytes,
        Err(error) => return emit_failure(error),
    };
    let info = match decode_session_info(&bytes) {
        Ok(info) => info,
        Err(_) => return emit_failure("response_invalid"),
    };
    emit_session_info(&info)
}

fn detach(stream: &mut AuthenticatedStream) -> i32 {
    let payload = match encode_session_request(&SessionRequest::Detach) {
        Ok(payload) => payload,
        Err(_) => return emit_failure("request_invalid"),
    };
    match exchange(stream, &payload) {
        Ok(bytes) if bytes == [0, 2, 0, 2] => emit_json("{\"ok\":true,\"action\":\"detach\"}", 0),
        Ok(_) => emit_failure("response_invalid"),
        Err(error) => emit_failure(error),
    }
}

fn control_read(
    context: &mut ContextFile,
    stream: &mut AuthenticatedStream,
    target: TargetRef,
) -> i32 {
    let state = context.context();
    let key = candidate_key(state);
    let request = ControlRequest {
        key,
        target,
        command: ControlCommand::Read {
            source: ReadSource::Screen,
            max_bytes: SERVER_READ_BYTES,
            max_lines: SERVER_READ_LINES,
        },
    };
    let bytes = match encode_control(&request) {
        Ok(bytes) => bytes,
        Err(error) => return emit_failure(error),
    };
    let reserved = match context.reserve() {
        Ok(key) if key == request.key => key,
        Ok(_) => return emit_failure("context_key_mismatch"),
        Err(error) => return emit_failure(context_error_label(&error)),
    };
    let response_bytes = match exchange(stream, &bytes) {
        Ok(bytes) => bytes,
        Err(error) => return emit_failure(error),
    };
    handle_reserved_response(context, reserved, &response_bytes, "read", Some(target))
}

fn control_input(
    context: &mut ContextFile,
    stream: &mut AuthenticatedStream,
    action: &'static str,
    target: TargetRef,
    owner_revision: u64,
    text: String,
    kind: InputKind,
) -> i32 {
    let state = context.context();
    let key = candidate_key(state);
    let command = match kind {
        InputKind::Text => ControlCommand::Text {
            owner_revision,
            text: &text,
        },
        InputKind::Paste => ControlCommand::Paste {
            owner_revision,
            text: &text,
        },
        InputKind::Submit => ControlCommand::Submit {
            owner_revision,
            text: &text,
        },
    };
    let request = ControlRequest {
        key,
        target,
        command,
    };
    let bytes = match encode_control(&request) {
        Ok(bytes) => bytes,
        Err(error) => return emit_failure(error),
    };
    let reserved = match context.reserve() {
        Ok(key) if key == request.key => key,
        Ok(_) => return emit_failure("context_key_mismatch"),
        Err(error) => return emit_failure(context_error_label(&error)),
    };
    let response_bytes = match exchange(stream, &bytes) {
        Ok(bytes) => bytes,
        Err(error) => return emit_failure(error),
    };
    handle_reserved_response(context, reserved, &response_bytes, action, None)
}

fn control_interrupt(
    context: &mut ContextFile,
    stream: &mut AuthenticatedStream,
    target: TargetRef,
    owner_revision: u64,
) -> i32 {
    let state = context.context();
    let request = ControlRequest {
        key: candidate_key(state),
        target,
        command: ControlCommand::Interrupt { owner_revision },
    };
    let bytes = match encode_control(&request) {
        Ok(bytes) => bytes,
        Err(error) => return emit_failure(error),
    };
    let reserved = match context.reserve() {
        Ok(key) if key == request.key => key,
        Ok(_) => return emit_failure("context_key_mismatch"),
        Err(error) => return emit_failure(context_error_label(&error)),
    };
    let response_bytes = match exchange(stream, &bytes) {
        Ok(bytes) => bytes,
        Err(error) => return emit_failure(error),
    };
    handle_reserved_response(context, reserved, &response_bytes, "interrupt", None)
}

fn run_server_probe(request: ServerProbe) -> i32 {
    let prepared = match prepare_probe(ProbeRequest {
        terminfo_directory: request.terminfo,
        executable: request.executable,
        arguments: request.arguments,
    }) {
        Ok(prepared) => prepared,
        Err(_) => return emit_failure("launch_context_invalid"),
    };
    if ensure_server_directory(&request.directory).is_err() {
        return emit_failure("server_directory_invalid");
    }
    let storage = match if request.initialize {
        ServerStorage::initialize(&request.directory)
    } else {
        ServerStorage::open(&request.directory)
    } {
        Ok(storage) => storage,
        Err(_) => return emit_failure("server_storage_unavailable"),
    };
    let listener = match PrivateListener::bind(&request.directory) {
        Ok(listener) => listener,
        Err(_) => return emit_failure("listener_unavailable"),
    };

    let stop = Arc::new(AtomicBool::new(false));
    let _signal_ids = match register_stop_signals(Arc::clone(&stop)) {
        Ok(ids) => ids,
        Err(_) => return emit_failure("signal_registration_failed"),
    };
    let mut server = match StartedServer::start(
        storage,
        &prepared.launch,
        &prepared.arguments,
        &prepared.environment,
        request.allow_input,
    ) {
        Ok(server) => server,
        Err(_) => return emit_failure("server_start_failed"),
    };

    let generation = server.generation();
    let target = server.driver_mut().state().target();
    let startup_outcome = server.startup_outcome();
    let mut startup = String::from(
        "{\"ok\":true,\"action\":\"server-probe\",\"state\":\"running\",\"generation\":",
    );
    let _ = write!(startup, "{generation},\"target\":");
    push_target(&mut startup, target);
    let _ = write!(
        startup,
        ",\"startup_outcome\":\"{}\",\"allow_input\":{}}}",
        outcome_name(startup_outcome),
        request.allow_input
    );
    if !write_json(&startup) {
        let clock = monotonic_clock(Instant::now());
        server.driver_mut().stop_connections(&clock);
        drop(server);
        drop(listener);
        return 1;
    }

    let started_at = Instant::now();
    let clock = monotonic_clock(started_at);
    let mut failed = false;
    while !stop.load(Ordering::Relaxed) {
        if server.verify_storage().is_err() {
            failed = true;
            break;
        }
        if server.driver_mut().tick(&clock).is_err() {
            failed = true;
            break;
        }
        match listener.accept(deadline_after(SERVER_ACCEPT_SLICE)) {
            Ok(stream) => {
                // Capacity and thread-spawn failures reject this attachment.
                // They never stop the process-owning server or retry a request.
                let _ = server.driver_mut().attach(stream);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(_) => {
                failed = true;
                break;
            }
        }
    }
    server.driver_mut().stop_connections(&clock);
    drop(server);
    drop(listener);
    if failed {
        emit_failure("server_runtime_failed")
    } else {
        emit_json(
            &format!(
                "{{\"ok\":true,\"action\":\"server-probe\",\"state\":\"stopped\",\"generation\":{generation}}}"
            ),
            if startup_outcome == Outcome::Applied { 0 } else { 1 },
        )
    }
}

fn ensure_server_directory(path: &Path) -> io::Result<()> {
    panelink::transport::prepare_directory(path)
}

fn register_stop_signals(stop: Arc<AtomicBool>) -> io::Result<Vec<signal_hook::SigId>> {
    let mut registrations = Vec::with_capacity(3);
    for signal in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        registrations.push(signal_hook::flag::register(signal, Arc::clone(&stop))?);
    }
    Ok(registrations)
}

fn monotonic_clock(started_at: Instant) -> impl Fn() -> u64 {
    move || started_at.elapsed().as_secs()
}

fn emit_context(context: &ContextFile, action: &'static str) -> i32 {
    let state = context.context();
    let mut json = String::from("{\"ok\":true,\"action\":");
    push_json_string(&mut json, action);
    let _ = write!(
        json,
        ",\"generation\":{},\"lease\":{},\"next_sequence\":{},\"uncertain\":",
        state.generation(),
        state.lease(),
        state.next_sequence()
    );
    match state.uncertain() {
        Some(key) => push_key(&mut json, key),
        None => json.push_str("null"),
    }
    json.push('}');
    emit_json(&json, 0)
}

fn emit_session_info(info: &SessionInfo) -> i32 {
    let mut json = String::from("{\"ok\":true,\"action\":\"describe\",\"target\":");
    push_target(&mut json, info.target);
    let _ = write!(
        json,
        ",\"owner_revision\":{},\"owner_lease\":{},\"owner_expires_secs\":{},\"child_exited\":{}}}",
        info.owner_revision, info.owner_lease, info.owner_expires_secs, info.child_exited
    );
    emit_json(&json, 0)
}

fn emit_read(
    action: &'static str,
    key: OperationKey,
    target: TargetRef,
    screen_revision: u64,
    truncated: bool,
    text: &str,
) -> i32 {
    let mut json = String::with_capacity(text.len().saturating_mul(2).saturating_add(256));
    json.push_str("{\"ok\":true,\"action\":");
    push_json_string(&mut json, action);
    json.push_str(",\"target\":");
    push_target(&mut json, target);
    let _ = write!(
        json,
        ",\"generation\":{},\"lease\":{},\"sequence\":{},\"source\":\"screen\",\"screen_revision\":{screen_revision},\"truncated\":{truncated},\"text\":" ,
        key.generation,
        key.lease,
        key.sequence
    );
    push_json_string(&mut json, text);
    json.push('}');
    emit_json(&json, 0)
}

fn emit_status(
    action: &'static str,
    key: OperationKey,
    status: OperationStatus,
    context_cleared: Option<bool>,
) -> i32 {
    let mut json = String::from("{\"ok\":");
    json.push_str(if status_success(status) {
        "true"
    } else {
        "false"
    });
    json.push_str(",\"action\":");
    push_json_string(&mut json, action);
    let _ = write!(
        json,
        ",\"generation\":{},\"lease\":{},\"sequence\":{},\"status\":\"{}\"",
        key.generation,
        key.lease,
        key.sequence,
        status_name(status)
    );
    if let OperationStatus::Completed(outcome) = status {
        let _ = write!(json, ",\"outcome\":\"{}\"", outcome_name(outcome));
    }
    if matches!(status, OperationStatus::InProgress) {
        json.push_str(",\"uncertain\":true");
    }
    if let Some(cleared) = context_cleared {
        let _ = write!(json, ",\"context_cleared\":{cleared}");
    }
    json.push('}');
    emit_json(&json, if status_success(status) { 0 } else { 1 })
}

fn emit_resync_clear(
    key: Option<OperationKey>,
    status: Option<OperationStatus>,
    result: Option<ReconcileResult>,
) -> i32 {
    let Some(status) = status else {
        return emit_json(
            "{\"ok\":true,\"action\":\"resync\",\"uncertain\":false,\"context_cleared\":false}",
            0,
        );
    };
    let cleared = matches!(result, Some(ReconcileResult::Cleared));
    let Some(key) = key else {
        return emit_failure("response_invalid");
    };
    let mut json = String::from("{\"ok\":");
    json.push_str(if status_success(status) && cleared {
        "true"
    } else {
        "false"
    });
    let _ = write!(
        json,
        ",\"action\":\"resync\",\"generation\":{},\"lease\":{},\"sequence\":{},\"status\":\"{}\"",
        key.generation,
        key.lease,
        key.sequence,
        status_name(status)
    );
    if let OperationStatus::Completed(outcome) = status {
        let _ = write!(json, ",\"outcome\":\"{}\"", outcome_name(outcome));
    }
    let _ = write!(
        json,
        ",\"uncertain\":{},\"context_cleared\":{}",
        !cleared, cleared
    );
    json.push('}');
    emit_json(
        &json,
        if status_success(status) && cleared {
            0
        } else {
            1
        },
    )
}

fn emit_rejected(
    action: &'static str,
    key: OperationKey,
    code: RejectionCode,
    context_acked: bool,
) -> i32 {
    let mut json = String::from("{\"ok\":false,\"action\":");
    push_json_string(&mut json, action);
    let _ = write!(
        json,
        ",\"generation\":{},\"lease\":{},\"sequence\":{},\"status\":\"rejected\",\"reason\":\"{}\",\"context_acked\":{context_acked}}}",
        key.generation,
        key.lease,
        key.sequence,
        rejection_name(code)
    );
    emit_json(&json, 1)
}

fn status_name(status: OperationStatus) -> &'static str {
    match status {
        OperationStatus::NotAdmitted => "not_admitted",
        OperationStatus::InProgress => "in_progress",
        OperationStatus::Completed(_) => "completed",
    }
}

fn status_success(status: OperationStatus) -> bool {
    matches!(status, OperationStatus::Completed(Outcome::Applied))
}

fn outcome_name(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Accepted => "accepted",
        Outcome::Applied => "applied",
        Outcome::Failed => "failed",
        Outcome::Partial => "partial",
        Outcome::Unknown => "unknown",
    }
}

fn rejection_name(code: RejectionCode) -> &'static str {
    match code {
        RejectionCode::Unauthorized => "unauthorized",
        RejectionCode::StaleTarget => "stale_target",
        RejectionCode::InputBusy => "input_busy",
        RejectionCode::StaleOwner => "stale_owner",
        RejectionCode::ExpiredOwner => "expired_owner",
        RejectionCode::Capacity => "capacity",
        RejectionCode::Unsupported => "unsupported",
        RejectionCode::InvalidRequest => "invalid_request",
        RejectionCode::UnknownContext => "unknown_context",
    }
}

fn push_key(json: &mut String, key: OperationKey) {
    let _ = write!(
        json,
        "{{\"generation\":{},\"lease\":{},\"sequence\":{}}}",
        key.generation, key.lease, key.sequence
    );
}

fn push_target(json: &mut String, target: TargetRef) {
    let _ = write!(
        json,
        "\"{}:{}:{}:{}:{}\"",
        target.generation, target.workspace, target.pane, target.session, target.occupant_revision
    );
}

fn push_json_string(json: &mut String, value: &str) {
    json.push('"');
    for scalar in value.chars() {
        match scalar {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            '\n' => json.push_str("\\n"),
            '\r' => json.push_str("\\r"),
            '\t' => json.push_str("\\t"),
            '\u{08}' => json.push_str("\\b"),
            '\u{0c}' => json.push_str("\\f"),
            '\u{2028}' | '\u{2029}' => {
                let _ = write!(json, "\\u{:04x}", scalar as u32);
            }
            scalar if scalar.is_control() => {
                let _ = write!(json, "\\u{:04x}", scalar as u32);
            }
            scalar => json.push(scalar),
        }
    }
    json.push('"');
}

fn emit_failure(label: &'static str) -> i32 {
    emit_error(label, 1)
}

fn emit_error(label: &'static str, exit_code: i32) -> i32 {
    let mut json = String::from("{\"ok\":false,\"error\":");
    push_json_string(&mut json, label);
    json.push('}');
    emit_json(&json, exit_code)
}

fn emit_json(value: &str, exit_code: i32) -> i32 {
    if write_json(value) {
        exit_code
    } else {
        1
    }
}

fn write_json(value: &str) -> bool {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    stdout.write_all(value.as_bytes()).is_ok() && stdout.write_all(b"\n").is_ok()
}

fn context_error_label(error: &ContextError) -> &'static str {
    match error {
        ContextError::InvalidIdentity | ContextError::InvalidRecord => "context_invalid",
        ContextError::UnsafeParent | ContextError::UnsafeFile => "context_unsafe",
        ContextError::IdentityMismatch => "context_mismatch",
        ContextError::LockTimeout => "context_lock_timeout",
        ContextError::SequenceExhausted | ContextError::RevisionExhausted => {
            "context_sequence_exhausted"
        }
        ContextError::UncertainOperation => "operation_uncertain",
        ContextError::NoUncertainOperation => "no_uncertain_operation",
        ContextError::OperationKeyMismatch => "operation_key_mismatch",
        ContextError::Poisoned | ContextError::Io(_) => "context_storage_failed",
    }
}

fn deadline_after(duration: Duration) -> Instant {
    Instant::now() + duration
}

fn fresh_nonce() -> u64 {
    let seed = *NONCE_SEED.get_or_init(|| {
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0);
        let seed = time ^ (u64::from(std::process::id()) << 32);
        if seed == 0 {
            u64::MAX
        } else {
            seed
        }
    });
    loop {
        let counter = NONCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nonce = seed.wrapping_add(counter);
        if nonce != 0 {
            return nonce;
        }
    }
}

fn print_help() {
    let _ = writeln!(
        io::stdout().lock(),
        "Usage:\n  panelink server-probe --directory DIR --terminfo TERMINFO [--initialize] [--allow-input] -- EXE [ARG...]\n  panelink control --directory DIR --context FILE init|show|describe|resync|detach\n  panelink control --directory DIR --context FILE read TARGET\n  panelink control --directory DIR --context FILE reconcile TARGET\n  panelink control --directory DIR --context FILE watch TARGET GEN:SEQ LIMIT WAIT_MS\n  panelink control --directory DIR --context FILE claim|release TARGET OWNER_REV\n  panelink control --directory DIR --context FILE handoff TARGET OWNER_REV RECIPIENT_LEASE\n  panelink control --directory DIR --context FILE text|paste|submit TARGET OWNER_REV TEXT\n  panelink control --directory DIR --context FILE interrupt TARGET OWNER_REV\n  panelink control --directory DIR --context FILE status SEQUENCE\n  panelink control --directory DIR --context FILE receipt THROUGH_SEQUENCE\nTARGET is generation:workspace:pane:session:occupant_revision. Use --initialize only for a new empty server directory."
    );
}

fn encode_control(request: &ControlRequest<'_>) -> Result<Vec<u8>, &'static str> {
    request.encoded_len().map_err(|_| "request_invalid")?;
    let mut bytes = Vec::with_capacity(request.encoded_len().map_err(|_| "request_invalid")?);
    encode_request_payload(request, &mut bytes).map_err(|_| "request_invalid")?;
    if bytes.len() > panelink::protocol_wire::MAX_CONTROL_PAYLOAD_BYTES {
        return Err("request_invalid");
    }
    Ok(bytes)
}

fn candidate_key(context: panelink::client_context::ClientContext) -> OperationKey {
    OperationKey {
        generation: context.generation(),
        lease: context.lease(),
        sequence: context.next_sequence(),
    }
}

fn handle_reserved_response(
    context: &mut ContextFile,
    key: OperationKey,
    bytes: &[u8],
    action: &'static str,
    expected_target: Option<TargetRef>,
) -> i32 {
    let response = match decode_response_payload(bytes) {
        Ok(response) if response.key == key => response,
        _ => return emit_failure("response_invalid"),
    };
    match response.body {
        ResponseBody::Status(status) => {
            if matches!(status, OperationStatus::InProgress) {
                return emit_status(action, key, status, Some(false));
            }
            if let Err(error) = context.ack(key) {
                return emit_failure(context_error_label(&error));
            }
            emit_status(action, key, status, Some(true))
        }
        ResponseBody::Rejected(code) => {
            if let Err(error) = context.ack(key) {
                return emit_failure(context_error_label(&error));
            }
            emit_rejected(action, key, code, true)
        }
        ResponseBody::Read {
            target,
            source,
            screen_revision,
            truncated,
            text,
        } => {
            let lines = if text.is_empty() {
                0
            } else {
                1 + text.bytes().filter(|b| *b == b'\n').count()
            };
            if expected_target != Some(target)
                || source != ReadSource::Screen
                || bytes.len() > SERVER_READ_BYTES as usize
                || lines > usize::from(SERVER_READ_LINES)
            {
                return emit_failure("response_invalid");
            }
            if let Err(error) = context.ack(key) {
                return emit_failure(context_error_label(&error));
            }
            emit_read(action, key, target, screen_revision, truncated, text)
        }
    }
}

enum Mutation {
    Claim,
    Release,
    Handoff(u64),
}

fn session_mutation(
    context: &mut ContextFile,
    stream: &mut AuthenticatedStream,
    action: &'static str,
    target: TargetRef,
    owner_revision: u64,
    mutation: Mutation,
) -> i32 {
    let key = candidate_key(context.context());
    let request = match mutation {
        Mutation::Claim => SessionRequest::Claim {
            key,
            target,
            expected_owner_revision: owner_revision,
            ttl_seconds: OWNER_TTL_SECONDS,
        },
        Mutation::Release => SessionRequest::Release {
            key,
            target,
            expected_owner_revision: owner_revision,
        },
        Mutation::Handoff(recipient_lease) => SessionRequest::Handoff {
            key,
            target,
            expected_owner_revision: owner_revision,
            recipient_lease,
            ttl_seconds: OWNER_TTL_SECONDS,
        },
    };
    let bytes = match encode_session_request(&request) {
        Ok(bytes) => bytes,
        Err(_) => return emit_failure("request_invalid"),
    };
    let reserved = match context.reserve() {
        Ok(reserved) if reserved == key => reserved,
        Ok(_) => return emit_failure("context_key_mismatch"),
        Err(error) => return emit_failure(context_error_label(&error)),
    };
    let response_bytes = match exchange(stream, &bytes) {
        Ok(bytes) => bytes,
        Err(error) => return emit_failure(error),
    };
    handle_reserved_response(context, reserved, &response_bytes, action, None)
}

enum InputKind {
    Text,
    Paste,
    Submit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use panelink::protocol_wire::{decode_header, encode_frame, FrameHeader, HEADER_BYTES};
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    use std::thread;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn test_target() -> TargetRef {
        TargetRef {
            generation: 7,
            workspace: 2,
            pane: 3,
            session: 4,
            occupant_revision: 5,
        }
    }

    fn read_test_frame(stream: &mut UnixStream) -> (FrameHeader, Vec<u8>) {
        let mut header_bytes = [0_u8; HEADER_BYTES];
        stream.read_exact(&mut header_bytes).unwrap();
        let header = decode_header(&header_bytes).unwrap();
        let mut payload = vec![0_u8; header.payload_len()];
        stream.read_exact(&mut payload).unwrap();
        (header, payload)
    }

    fn write_test_frame(
        stream: &mut UnixStream,
        kind: MessageKind,
        request_id: u64,
        payload: &[u8],
    ) {
        let frame = Frame::new(kind, request_id, payload).unwrap();
        encode_frame(&frame, stream).unwrap();
    }

    fn welcome_for(payload: &[u8], generation: u64, lease: u64) -> Vec<u8> {
        let hello = panelink::session_protocol::decode_session_hello(payload).unwrap();
        panelink::handshake_protocol::encode_welcome(&Welcome {
            nonce: hello.hello.nonce,
            generation,
            lease,
            selected_capabilities: CAPABILITY_READ,
            ttl_seconds: panelink::handshake_protocol::HANDSHAKE_LEASE_TTL_SECONDS,
        })
        .unwrap()
        .to_vec()
    }

    #[test]
    fn parser_help_and_maximum_child_arguments_remain_reachable() {
        for command in ["control", "server-probe"] {
            assert!(matches!(
                parse(args(&[command, "--help"])),
                Ok(CliCommand::Help)
            ));
        }
        let mut values = args(&[
            "server-probe",
            "--directory",
            "/private/tmp/pl-server",
            "--terminfo",
            "/private/tmp/terminfo",
            "--initialize",
            "--allow-input",
            "--",
            "/bin/sh",
        ]);
        values.extend((0..256).map(|_| OsString::from("argument")));
        let Ok(CliCommand::ServerProbe(probe)) = parse(values.clone()) else {
            panic!("maximum child argument count rejected");
        };
        assert_eq!(probe.arguments.len(), 256);
        values.push(OsString::from("extra"));
        assert!(parse(values).is_err());
    }

    #[test]
    fn parser_accepts_exact_probe_and_control_forms_without_implicit_selection() {
        let parsed = parse(args(&[
            "server-probe",
            "--directory",
            "/private/tmp/pl-server",
            "--terminfo",
            "/private/tmp/terminfo",
            "--allow-input",
            "--initialize",
            "--",
            "/bin/sh",
            "-c",
            "printf ok",
        ]))
        .unwrap();
        let CliCommand::ServerProbe(probe) = parsed else {
            panic!("expected server probe");
        };
        assert!(probe.initialize);
        assert!(probe.allow_input);
        assert_eq!(probe.executable, OsString::from("/bin/sh"));
        assert_eq!(probe.arguments, args(&["-c", "printf ok"]));

        let parsed = parse(args(&[
            "control",
            "--directory",
            "/private/tmp/pl-server",
            "--context",
            "/private/tmp/client.ctx",
            "read",
            "7:2:3:4:5",
        ]))
        .unwrap();
        let CliCommand::Control(control) = parsed else {
            panic!("expected control command");
        };
        let Action::Read(target) = control.action else {
            panic!("expected explicit read target");
        };
        assert_eq!(target, test_target());

        let parsed = parse(args(&[
            "control",
            "--directory",
            "/private/tmp/pl-server",
            "--context",
            "/private/tmp/client.ctx",
            "receipt",
            "4",
        ]))
        .unwrap();
        let CliCommand::Control(control) = parsed else {
            panic!("expected control command");
        };
        assert!(matches!(control.action, Action::Receipt(4)));
    }

    #[test]
    fn parser_rejects_noncanonical_or_incomplete_targets_and_extra_arguments() {
        for target in [
            "0:2:3:4:5",
            "7:2:3:4",
            "7:2:3:4:5:6",
            "+7:2:3:4:5",
            "7:x:3:4:5",
        ] {
            assert!(parse_target(target).is_err(), "accepted {target}");
        }
        for malformed in [
            vec![
                "control",
                "--directory",
                "relative",
                "--context",
                "/tmp/c",
                "show",
            ],
            vec![
                "control",
                "--directory",
                "/tmp/s",
                "--context",
                "/tmp/c",
                "read",
            ],
            vec![
                "control",
                "--directory",
                "/tmp/s",
                "--context",
                "/tmp/c",
                "read",
                "7:2:3:4:5",
                "extra",
            ],
            vec![
                "control",
                "--directory",
                "/tmp/s",
                "--context",
                "/tmp/c",
                "claim",
                "7:2:3:4:5",
                "0",
            ],
            vec![
                "server-probe",
                "--directory",
                "/tmp/s",
                "--terminfo",
                "/tmp/t",
                "--initialize",
                "--initialize",
                "--",
                "/bin/sh",
            ],
            vec![
                "control",
                "--directory",
                "/tmp/s",
                "--context",
                "/tmp/c",
                "receipt",
                "0",
            ],
            vec![
                "control",
                "--directory",
                "/tmp/s",
                "--context",
                "/tmp/c",
                "receipt",
            ],
            vec![
                "control",
                "--directory",
                "/tmp/s",
                "--context",
                "/tmp/c",
                "receipt",
                "1",
                "extra",
            ],
        ] {
            assert!(parse(args(&malformed)).is_err(), "accepted {malformed:?}");
        }
    }

    #[test]
    fn json_strings_escape_terminal_controls_quotes_and_line_separators() {
        let mut encoded = String::new();
        push_json_string(
            &mut encoded,
            "quote\" slash\\ newline\n ESC\u{1b} C1\u{0085}\u{2028}",
        );
        assert_eq!(
            encoded,
            r#""quote\" slash\\ newline\n ESC\u001b C1\u0085\u2028""#
        );
        assert!(!encoded.contains('\u{1b}'));
        assert!(!encoded.contains('\u{0085}'));
    }

    #[test]
    fn socketpair_handshake_checks_welcome_identity_and_frame_role() {
        let (client_socket, mut server_socket) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            let (header, payload) = read_test_frame(&mut server_socket);
            assert_eq!(header.kind(), MessageKind::Hello);
            assert_eq!(header.request_id(), 0);
            let payload = welcome_for(&payload, 7, 9);
            write_test_frame(&mut server_socket, MessageKind::Welcome, 0, &payload);
        });
        let mut client = AuthenticatedStream::new(client_socket).unwrap();
        let welcome = handshake(&mut client, None).unwrap();
        assert_eq!((welcome.generation, welcome.lease), (7, 9));
        server.join().unwrap();

        let (client_socket, mut server_socket) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            let (header, payload) = read_test_frame(&mut server_socket);
            assert_eq!(header.kind(), MessageKind::Hello);
            assert_eq!(header.request_id(), 0);
            let payload = welcome_for(&payload, 8, 9);
            write_test_frame(&mut server_socket, MessageKind::Welcome, 0, &payload);
        });
        let mut client = AuthenticatedStream::new(client_socket).unwrap();
        let prior = panelink::client_context::ClientContext::new(7, 9).unwrap();
        assert_eq!(handshake(&mut client, Some(prior)), Err("context_mismatch"));
        server.join().unwrap();

        let (client_socket, mut server_socket) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            let (header, _) = read_test_frame(&mut server_socket);
            assert_eq!(header.kind(), MessageKind::Hello);
            write_test_frame(&mut server_socket, MessageKind::Response, 0, &[]);
        });
        let mut client = AuthenticatedStream::new(client_socket).unwrap();
        assert_eq!(handshake(&mut client, None), Err("handshake_invalid"));
        server.join().unwrap();
    }

    #[test]
    fn socketpair_request_uses_id_one_and_decodes_typed_session_info() {
        let (client_socket, mut server_socket) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            let (header, payload) = read_test_frame(&mut server_socket);
            assert_eq!(header.kind(), MessageKind::Hello);
            let welcome = welcome_for(&payload, 7, 9);
            write_test_frame(&mut server_socket, MessageKind::Welcome, 0, &welcome);

            let (header, payload) = read_test_frame(&mut server_socket);
            assert_eq!(header.kind(), MessageKind::Request);
            assert_eq!(header.request_id(), 1);
            assert_eq!(
                panelink::session_protocol::decode_session_request(&payload).unwrap(),
                SessionRequest::Describe
            );
            let info = SessionInfo {
                target: test_target(),
                owner_revision: 1,
                owner_lease: 0,
                owner_expires_secs: 0,
                child_exited: false,
            };
            let info = panelink::session_protocol::encode_session_info(&info).unwrap();
            write_test_frame(&mut server_socket, MessageKind::Response, 1, &info);
        });
        let mut client = AuthenticatedStream::new(client_socket).unwrap();
        handshake(&mut client, None).unwrap();
        let request = encode_session_request(&SessionRequest::Describe).unwrap();
        let response = exchange(&mut client, &request).unwrap();
        let info = decode_session_info(&response).unwrap();
        assert_eq!(info.target, test_target());
        server.join().unwrap();
    }

    #[test]
    fn socketpair_rejects_response_with_a_different_request_id() {
        let (client_socket, mut server_socket) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            let (header, payload) = read_test_frame(&mut server_socket);
            assert_eq!(header.kind(), MessageKind::Hello);
            let welcome = welcome_for(&payload, 7, 9);
            write_test_frame(&mut server_socket, MessageKind::Welcome, 0, &welcome);

            let (header, _) = read_test_frame(&mut server_socket);
            assert_eq!(header.kind(), MessageKind::Request);
            assert_eq!(header.request_id(), 1);
            write_test_frame(&mut server_socket, MessageKind::Response, 2, &[0, 2, 0, 2]);
        });
        let mut client = AuthenticatedStream::new(client_socket).unwrap();
        handshake(&mut client, None).unwrap();
        let request = encode_session_request(&SessionRequest::Detach).unwrap();
        assert_eq!(exchange(&mut client, &request), Err("response_invalid"));
        server.join().unwrap();
    }

    static TEST_CONTEXT_COUNTER: AtomicU64 = AtomicU64::new(1);

    fn test_context(uncertain: bool) -> (PathBuf, PathBuf, ContextFile) {
        use std::os::unix::fs::PermissionsExt;

        let directory = std::env::temp_dir().join(format!(
            "panelink-p3-cli-{}-{}",
            std::process::id(),
            TEST_CONTEXT_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.join("caller.ctx");
        let mut context =
            ContextFile::create(&path, 7, 9, deadline_after(CLIENT_DEADLINE)).unwrap();
        if uncertain {
            context.reserve().unwrap();
        }
        (directory, path, context)
    }

    fn receipt_peer(
        context: panelink::client_context::ClientContext,
        response_key: OperationKey,
        response_body: ResponseBody<'static>,
    ) -> (AuthenticatedStream, thread::JoinHandle<SessionRequest>) {
        let response = panelink::response_protocol::Response {
            key: response_key,
            body: response_body,
        };
        let mut response_bytes = Vec::new();
        panelink::response_protocol::encode_response_payload(&response, &mut response_bytes)
            .unwrap();
        receipt_peer_frame(context, MessageKind::Response, response_bytes)
    }

    fn receipt_peer_frame(
        context: panelink::client_context::ClientContext,
        response_kind: MessageKind,
        response_payload: Vec<u8>,
    ) -> (AuthenticatedStream, thread::JoinHandle<SessionRequest>) {
        let (client_socket, mut server_socket) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            let (header, payload) = read_test_frame(&mut server_socket);
            assert_eq!(header.kind(), MessageKind::Hello);
            assert_eq!(header.request_id(), 0);
            let welcome = welcome_for(&payload, context.generation(), context.lease());
            write_test_frame(&mut server_socket, MessageKind::Welcome, 0, &welcome);

            let (header, payload) = read_test_frame(&mut server_socket);
            assert_eq!(header.kind(), MessageKind::Request);
            assert_eq!(header.request_id(), 1);
            let request = panelink::session_protocol::decode_session_request(&payload).unwrap();
            write_test_frame(&mut server_socket, response_kind, 1, &response_payload);
            request
        });
        let mut client = AuthenticatedStream::new(client_socket).unwrap();
        handshake(&mut client, Some(context)).unwrap();
        (client, server)
    }

    fn acknowledged_test_context() -> (PathBuf, PathBuf, ContextFile) {
        let (directory, path, mut context) = test_context(false);
        let key = context.reserve().unwrap();
        context.ack(key).unwrap();
        (directory, path, context)
    }

    #[test]
    fn receipt_sends_exact_session_body_and_accepts_only_matching_applied_status() {
        let (directory, path, context) = acknowledged_test_context();
        let context_state = context.context();
        let before = std::fs::read(&path).unwrap();
        let key = receipt_key(context_state, 1).unwrap();
        let (mut client, server) = receipt_peer(
            context_state,
            key,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied)),
        );

        assert_eq!(
            request_receipt(&context, &mut client, key),
            Ok(ReceiptReply::Status(OperationStatus::Completed(
                Outcome::Applied
            )))
        );
        assert_eq!(
            server.join().unwrap(),
            SessionRequest::Receipt {
                generation: 7,
                lease: 9,
                through_sequence: 1,
            }
        );
        assert!(status_success(OperationStatus::Completed(Outcome::Applied)));
        assert_eq!(context.context(), context_state);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(context);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn receipt_rejects_wrong_response_identity_and_non_applied_status() {
        let (directory, path, context) = acknowledged_test_context();
        let state = context.context();
        let before = std::fs::read(&path).unwrap();
        let key = receipt_key(state, 1).unwrap();
        let wrong_key = OperationKey { sequence: 2, ..key };
        let (mut client, server) = receipt_peer(
            state,
            wrong_key,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied)),
        );
        assert_eq!(
            request_receipt(&context, &mut client, key),
            Err("response_invalid")
        );
        assert!(matches!(
            server.join().unwrap(),
            SessionRequest::Receipt { .. }
        ));

        let (mut client, server) = receipt_peer(
            state,
            key,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Failed)),
        );
        assert_eq!(
            request_receipt(&context, &mut client, key),
            Ok(ReceiptReply::Status(OperationStatus::Completed(
                Outcome::Failed
            )))
        );
        assert!(!status_success(OperationStatus::Completed(Outcome::Failed)));
        assert!(matches!(
            server.join().unwrap(),
            SessionRequest::Receipt { .. }
        ));
        assert_eq!(context.context(), state);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(context);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn receipt_rejects_wrong_body_role_malformed_payload_and_fixed_server_error() {
        let (directory, path, context) = acknowledged_test_context();
        let state = context.context();
        let before = std::fs::read(&path).unwrap();
        let key = receipt_key(state, 1).unwrap();

        let (mut client, server) = receipt_peer(
            state,
            key,
            ResponseBody::Read {
                target: test_target(),
                source: ReadSource::Screen,
                screen_revision: 1,
                truncated: false,
                text: "unexpected read body",
            },
        );
        assert_eq!(
            request_receipt(&context, &mut client, key),
            Err("response_invalid")
        );
        assert!(matches!(
            server.join().unwrap(),
            SessionRequest::Receipt { .. }
        ));

        let (mut client, server) = receipt_peer_frame(state, MessageKind::Response, vec![0, 1, 2]);
        assert_eq!(
            request_receipt(&context, &mut client, key),
            Err("response_invalid")
        );
        assert!(matches!(
            server.join().unwrap(),
            SessionRequest::Receipt { .. }
        ));

        let (mut client, server) = receipt_peer_frame(state, MessageKind::Error, vec![1, 2, 3]);
        assert_eq!(
            request_receipt(&context, &mut client, key),
            Err("server_rejected")
        );
        assert!(matches!(
            server.join().unwrap(),
            SessionRequest::Receipt { .. }
        ));
        assert_eq!(context.context(), state);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(context);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn locked_context_blocks_uncertain_and_out_of_range_receipts_before_connecting() {
        let (directory, path, uncertain) = test_context(true);
        let before = std::fs::read(&path).unwrap();
        let mut connected = false;
        assert!(matches!(
            connect_for_receipt(&uncertain, 1, || {
                connected = true;
                Err("transport_unavailable")
            }),
            Err("operation_uncertain")
        ));
        assert!(!connected);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(uncertain);
        std::fs::remove_dir_all(directory).unwrap();

        let (directory, path, current) = test_context(false);
        let before = std::fs::read(&path).unwrap();
        let mut connected = false;
        assert!(matches!(
            connect_for_receipt(&current, 1, || {
                connected = true;
                Err("transport_unavailable")
            }),
            Err("receipt_out_of_range")
        ));
        assert!(!connected);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        drop(current);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn observation_parser_accepts_zero_cursor_and_enforces_watch_bounds() {
        let base = [
            "control",
            "--directory",
            "/private/tmp/runtime",
            "--context",
            "/private/tmp/caller",
        ];
        let mut values = args(&base);
        values.extend(args(&["reconcile", "7:2:3:4:5"]));
        assert!(matches!(
            parse(values),
            Ok(CliCommand::Control(Control {
                action: Action::Reconcile(_),
                ..
            }))
        ));
        for (cursor, limit, wait, valid) in [
            ("7:0", "64", "1000", true),
            ("7:1", "1", "0", true),
            ("8:0", "1", "0", false),
            ("0:0", "1", "0", false),
            ("7:1:2", "1", "0", false),
            ("7:0", "0", "0", false),
            ("7:0", "65", "0", false),
            ("7:0", "1", "1001", false),
        ] {
            let mut values = args(&base);
            values.extend(args(&["watch", "7:2:3:4:5", cursor, limit, wait]));
            assert_eq!(parse(values).is_ok(), valid, "{cursor} {limit} {wait}");
        }
    }

    fn watch_peer(
        frames: Vec<(MessageKind, u64, Vec<u8>)>,
    ) -> (AuthenticatedStream, thread::JoinHandle<WatchRequest>) {
        let (client, mut server) = UnixStream::pair().unwrap();
        server.set_read_timeout(Some(CLIENT_DEADLINE)).unwrap();
        server.set_write_timeout(Some(CLIENT_DEADLINE)).unwrap();
        let thread = thread::spawn(move || {
            let (header, payload) = read_test_frame(&mut server);
            assert_eq!(header.kind(), MessageKind::Request);
            assert_eq!(header.request_id(), 1);
            let request = panelink::watch_protocol::decode_watch_request(&payload).unwrap();
            for (kind, id, payload) in frames {
                write_test_frame(&mut server, kind, id, &payload);
            }
            request
        });
        let mut client = AuthenticatedStream::new(client).unwrap();
        client.enable_metadata_events().unwrap();
        (client, thread)
    }

    fn watch_request() -> WatchRequest {
        WatchRequest::Watch {
            target: test_target(),
            cursor: EventCursor {
                generation: 7,
                sequence: 0,
            },
            limit: 2,
            wait_ms: 0,
        }
    }

    fn watch_frames(event: Event, end: WatchEnd) -> Vec<(MessageKind, u64, Vec<u8>)> {
        vec![
            (
                MessageKind::Event,
                0,
                panelink::event_protocol::encode_event(&event)
                    .unwrap()
                    .to_vec(),
            ),
            (
                MessageKind::Response,
                1,
                panelink::watch_protocol::encode_watch_end(&end)
                    .unwrap()
                    .to_vec(),
            ),
        ]
    }

    #[test]
    fn watch_accepts_complete_ordered_batch_and_explicit_empty_outcomes() {
        let event = Event {
            cursor: 1,
            target: test_target(),
            body: EventBody::ScreenChanged { revision: 3 },
        };
        let end = WatchEnd {
            status: WatchStatus::Changed,
            cursor: EventCursor {
                generation: 7,
                sequence: 1,
            },
            event_count: 1,
        };
        let (mut client, peer) = watch_peer(watch_frames(event, end));
        assert_eq!(
            exchange_watch(&mut client, watch_request()),
            Ok((vec![event], end))
        );
        assert_eq!(peer.join().unwrap(), watch_request());
        for (status, sequence) in [
            (WatchStatus::TimedOut, 0),
            (WatchStatus::ResyncRequired, 65),
        ] {
            let end = WatchEnd {
                status,
                cursor: EventCursor {
                    generation: 7,
                    sequence,
                },
                event_count: 0,
            };
            let frames = vec![(
                MessageKind::Response,
                1,
                panelink::watch_protocol::encode_watch_end(&end)
                    .unwrap()
                    .to_vec(),
            )];
            let (mut client, peer) = watch_peer(frames);
            assert_eq!(
                exchange_watch(&mut client, watch_request()),
                Ok((vec![], end))
            );
            peer.join().unwrap();
        }
    }

    #[test]
    fn watch_rejects_incomplete_or_mismatched_batches_before_emitting_output() {
        let event = Event {
            cursor: 1,
            target: test_target(),
            body: EventBody::ScreenChanged { revision: 3 },
        };
        let end = WatchEnd {
            status: WatchStatus::Changed,
            cursor: EventCursor {
                generation: 7,
                sequence: 1,
            },
            event_count: 1,
        };
        for (event, end) in [
            (Event { cursor: 2, ..event }, end),
            (
                Event {
                    target: TargetRef {
                        occupant_revision: 6,
                        ..test_target()
                    },
                    ..event
                },
                end,
            ),
            (
                event,
                WatchEnd {
                    event_count: 2,
                    ..end
                },
            ),
            (
                event,
                WatchEnd {
                    cursor: EventCursor {
                        generation: 7,
                        sequence: 2,
                    },
                    ..end
                },
            ),
        ] {
            let (mut client, peer) = watch_peer(watch_frames(event, end));
            assert_eq!(
                exchange_watch(&mut client, watch_request()),
                Err("response_invalid")
            );
            peer.join().unwrap();
        }
        let (mut client, peer) = watch_peer(vec![(
            MessageKind::Response,
            2,
            panelink::watch_protocol::encode_watch_end(&WatchEnd {
                status: WatchStatus::TimedOut,
                cursor: EventCursor {
                    generation: 7,
                    sequence: 0,
                },
                event_count: 0,
            })
            .unwrap()
            .to_vec(),
        )]);
        assert_eq!(
            exchange_watch(&mut client, watch_request()),
            Err("response_invalid")
        );
        peer.join().unwrap();
    }

    #[test]
    fn reconcile_json_keeps_cursor_metadata_and_escapes_display_controls() {
        let state = Reconciliation {
            info: SessionInfo {
                target: test_target(),
                owner_revision: 4,
                owner_lease: 9,
                owner_expires_secs: 30,
                child_exited: false,
            },
            cursor: EventCursor {
                generation: 7,
                sequence: 12,
            },
            screen_revision: 8,
            truncated: true,
            text: "quote\"\nnext",
        };
        let json = reconciliation_json(&state);
        assert!(json.contains("\"cursor\":\"7:12\""));
        assert!(json.contains("\"screen_revision\":8"));
        assert!(json.contains("\"owner_revision\":4"));
        assert!(json.contains("quote\\\"\\nnext"));
        assert!(!json.contains('\n'));
    }
}
