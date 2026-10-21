//! Experimental foreground server and reusable control command frontend.
//!
//! This is a narrow binary adapter over the reviewed launch, transport,
//! context, session, response, and server-owner APIs. It does not persist
//! request bodies, infer pane selection, retry operations, or own a host TTY.

use super::{
    capture_child_arguments, os_str_byte_len, preflight_serve_launch, prepare_probe, ProbeRequest,
};
use panelink::client_context::{ContextError, ContextFile, ReconcileResult};
use panelink::close_protocol::{
    decode_close_result, encode_close_request, encode_close_status_query, CloseOutcome,
    CloseRejection, CloseRequest, CloseResult, CloseStatusQuery, StopOutcome,
};
use panelink::control_protocol::{
    encode_request_payload, Command as ControlCommand, ControlRequest, ReadSource, TargetRef,
    MAX_INPUT_TEXT_BYTES,
};
use panelink::event_journal::EventCursor;
use panelink::event_protocol::{decode_event, Event, EventBody};
use panelink::handshake_protocol::{
    validate_welcome, Hello, Welcome, CAPABILITY_EVENTS, CAPABILITY_INPUT, CAPABILITY_READ,
};
use panelink::launch::{Environment, LaunchContext};
use panelink::managed_launch_protocol::LaunchRequest;
use panelink::multi_runtime::{decode_pane_reply, encode_pane_query, PaneReply};
use panelink::multi_startup::{StartedMultiServer, StartupLaunch};
use panelink::pane_layout::SplitDirection;
use panelink::protocol::{OperationKey, OperationStatus, Outcome};
use panelink::protocol_wire::{Frame, MessageKind};
use panelink::response_protocol::{decode_response_payload, RejectionCode, ResponseBody};
use panelink::server_startup::StartedServer;
use panelink::server_storage::ServerStorage;
use panelink::session_protocol::{
    decode_session_info, encode_session_hello, encode_session_request, LeaseRef, SessionHello,
    SessionInfo, SessionRequest,
};
use panelink::session_registry::SessionState;
use panelink::split_protocol::{decode_split_result, encode_split_request, SplitRequest};
use panelink::terminal::Key;
use panelink::transport::{AuthenticatedStream, PrivateListener};
use panelink::ui_protocol::KeyInput;
use panelink::watch_protocol::{
    decode_reconciliation, decode_watch_end, encode_watch_request, Reconciliation, WatchEnd,
    WatchRequest, WatchStatus, MAX_WATCH_EVENTS, MAX_WATCH_WAIT_MS,
};
use std::ffi::{OsStr, OsString};
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{self, Write as IoWrite};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CLIENT_DEADLINE: Duration = Duration::from_secs(2);
const SERVER_ACCEPT_SLICE: Duration = Duration::from_millis(1);
const SERVER_READ_BYTES: u32 = 4_096;
const SERVER_READ_LINES: u16 = 24;
const OWNER_TTL_SECONDS: u32 = 30;
const MAX_CONTROL_ARGUMENT_BYTES: usize = 65_536;
/// Bounded wait for queued replies after the last session ended.
const LAST_REPLY_LINGER: Duration = Duration::from_millis(250);

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(1);
static NONCE_SEED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

enum CliCommand {
    Help,
    ServerProbe(ServerProbe),
    Serve(ServerProbe),
    Control(Control),
    Attach(Attach),
}

/// `attach --directory SERVER_DIR --context-dir DIR`.
struct Attach {
    directory: PathBuf,
    context_dir: PathBuf,
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
    Init(Option<TargetRef>),
    Layout,
    Split {
        target: TargetRef,
        layout_revision: u64,
        direction: SplitDirection,
        cwd: Option<PathBuf>,
    },
    Keys {
        target: TargetRef,
        owner_revision: u64,
        text: String,
    },
    Panes,
    Close {
        target: TargetRef,
        layout_revision: u64,
    },
    CloseStatus(u64),
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
        CliCommand::Serve(request) => run_serve(request),
        CliCommand::Control(request) => run_control(request),
        CliCommand::Attach(request) => {
            crate::ui_client::run(request.directory, request.context_dir)
        }
    }
}

fn parse(args: Vec<OsString>) -> Result<CliCommand, ()> {
    if args.len() > 268 {
        return Err(());
    }
    if (args.len() == 1 && matches_os(&args[0], &["--help", "-h"]))
        || (args.len() == 2
            && matches_os(&args[0], &["server-probe", "serve", "control", "attach"])
            && matches_os(&args[1], &["--help", "-h"]))
    {
        return Ok(CliCommand::Help);
    }
    let Some(first) = args.first() else {
        return Err(());
    };
    if first == OsStr::new("server-probe") {
        parse_server_probe(&args[1..]).map(CliCommand::ServerProbe)
    } else if first == OsStr::new("serve") {
        parse_server_probe(&args[1..]).map(CliCommand::Serve)
    } else if first == OsStr::new("control") {
        parse_control(&args[1..]).map(CliCommand::Control)
    } else if first == OsStr::new("attach") {
        parse_attach(&args[1..]).map(CliCommand::Attach)
    } else {
        Err(())
    }
}

fn parse_attach(args: &[OsString]) -> Result<Attach, ()> {
    let mut index = 0;
    expect_arg(args, &mut index, "--directory")?;
    let directory = next_path(args, &mut index)?;
    expect_arg(args, &mut index, "--context-dir")?;
    let context_dir = next_path(args, &mut index)?;
    if index != args.len() {
        return Err(());
    }
    Ok(Attach {
        directory,
        context_dir,
    })
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
    if args.len() > 10 || !arguments_within_limit(args, MAX_CONTROL_ARGUMENT_BYTES) {
        return Err(());
    }
    let mut index = 0;
    expect_arg(args, &mut index, "--directory")?;
    let directory = next_path(args, &mut index)?;
    expect_arg(args, &mut index, "--context")?;
    let context_path = next_path(args, &mut index)?;
    let action_name = next_utf8(args, &mut index)?;
    let action = match action_name {
        "init" => Action::Init(match args.get(index) {
            Some(_) => Some(parse_target(next_utf8(args, &mut index)?)?),
            None => None,
        }),
        "layout" => Action::Layout,
        "split" => {
            let target = parse_target(next_utf8(args, &mut index)?)?;
            let layout_revision = parse_nonzero_decimal(next_utf8(args, &mut index)?)?;
            let direction = match next_utf8(args, &mut index)? {
                "left" => SplitDirection::Left,
                "right" => SplitDirection::Right,
                "up" => SplitDirection::Up,
                "down" => SplitDirection::Down,
                _ => return Err(()),
            };
            let cwd = if args.get(index).is_some() {
                expect_arg(args, &mut index, "--cwd")?;
                Some(next_path(args, &mut index)?)
            } else {
                None
            };
            Action::Split {
                target,
                layout_revision,
                direction,
                cwd,
            }
        }
        "keys" => {
            let target = parse_target(next_utf8(args, &mut index)?)?;
            let owner_revision = parse_nonzero_decimal(next_utf8(args, &mut index)?)?;
            let text = next_utf8(args, &mut index)?.to_owned();
            // One Char key per scalar plus Enter, within the schema-8 cap.
            if text.chars().count() + 1 > panelink::ui_protocol::MAX_KEYS
                || text.chars().any(char::is_control)
            {
                return Err(());
            }
            Action::Keys {
                target,
                owner_revision,
                text,
            }
        }
        "panes" => Action::Panes,
        "close" => Action::Close {
            target: parse_target(next_utf8(args, &mut index)?)?,
            layout_revision: parse_nonzero_decimal(next_utf8(args, &mut index)?)?,
        },
        "close-status" => Action::CloseStatus(parse_nonzero_decimal(next_utf8(args, &mut index)?)?),
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
        Action::Init(target) => run_init(&control.directory, &control.context_path, target),
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

fn run_init(directory: &Path, context_path: &Path, target: Option<TargetRef>) -> i32 {
    match fs::symlink_metadata(context_path) {
        Ok(_) => return emit_failure("context_exists"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return emit_failure("context_unavailable"),
    }
    let mut stream = match connect_stream(directory) {
        Ok(stream) => stream,
        Err(error) => return emit_failure(error),
    };
    let welcome = match handshake_attach(&mut stream, None, target) {
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
        Action::Layout => ui_layout(&mut stream),
        Action::Split {
            target,
            layout_revision,
            direction,
            cwd,
        } => ui_split(
            &mut context,
            &mut stream,
            target,
            layout_revision,
            direction,
            cwd,
        ),
        Action::Keys {
            target,
            owner_revision,
            text,
        } => ui_keys(&mut context, &mut stream, target, owner_revision, &text),
        Action::Panes => match request_panes(&mut stream, 1) {
            Ok(panes) => emit_json(&panes_json(&panes), 0),
            Err(error) => emit_failure(error),
        },
        Action::Close {
            target,
            layout_revision,
        } => control_close(&mut context, &mut stream, target, layout_revision),
        Action::CloseStatus(sequence) => control_close_status(&context, &mut stream, sequence),
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
        Action::Init(_)
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
    handshake_full(stream, resume, require_events, None)
}

/// Fresh or resumed Hello with an optional schema-8 AttachTarget trailer.
///
/// Only the multi-session `serve` runtime understands the trailer; it pins a
/// new lease to that exact pane.
fn handshake_attach(
    stream: &mut AuthenticatedStream,
    resume: Option<panelink::client_context::ClientContext>,
    target: Option<TargetRef>,
) -> Result<Welcome, &'static str> {
    handshake_full(stream, resume, false, target)
}

fn handshake_full(
    stream: &mut AuthenticatedStream,
    resume: Option<panelink::client_context::ClientContext>,
    require_events: bool,
    target: Option<TargetRef>,
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
    let mut payload = encode_session_hello(&session_hello).map_err(|_| "handshake_invalid")?;
    if let Some(target) = target {
        let trailer =
            panelink::ui_protocol::encode_attach_target(target).map_err(|_| "request_invalid")?;
        payload.extend_from_slice(&trailer);
    }
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
    exchange_with_id(stream, 1, payload)
}

/// One request/response with an explicit, strictly increasing request ID.
fn exchange_with_id(
    stream: &mut AuthenticatedStream,
    request_id: u64,
    payload: &[u8],
) -> Result<Vec<u8>, &'static str> {
    write_frame(stream, MessageKind::Request, request_id, payload)?;
    let frame = read_frame(stream)?;
    if frame.request_id() != request_id {
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

/// Called with the server storage lock held, before any capture writer exists.
fn prepare_history_store(
    history_path: &Path,
    now_unix_ms: Option<u64>,
) -> io::Result<panelink::history_store::HistoryStore> {
    let store = panelink::history_store::HistoryStore::open(history_path)?;
    store.cleanup_temporary_files()?;
    if let Some(now) = now_unix_ms {
        let _ = store.prune_expired(now);
    }
    Ok(store)
}

/// Only fixed diagnostic labels are rendered; no storage error text or data.
fn history_diagnostic_line(
    snapshot: panelink::history_diagnostics::HistoryDiagnosticSnapshot,
    shutdown: bool,
) -> String {
    let prefix = if shutdown {
        "panelink: history capture incomplete during shutdown"
    } else {
        "panelink: screen history capture failed; shells remain running"
    };
    let mut line = format!("{prefix}; reasons=");
    let mut any = false;
    for reason in snapshot.reasons() {
        if any {
            line.push(',');
        }
        line.push_str(reason.as_str());
        any = true;
    }
    if !any {
        line.push_str("none");
    }
    let _ = write!(
        line,
        "; writer_drain={}",
        snapshot.writer_drain_outcome().as_str()
    );
    line
}

/// Foreground multi-session server: same storage/endpoint semantics as
/// `server-probe`, but the first child is a managed schema-6 launch and the
/// runtime routes schemas 1, 2, 5, 6 (status), 7 (split) and 8 (pane UI).
fn run_serve(request: ServerProbe) -> i32 {
    // Capture the filtered context once (inherited PANELINK_* removed, so a
    // serve started inside a Panelink pane works), then validate terminfo and
    // resolve the executable against that same context before storage.
    let context = match capture_launch_context(None) {
        Ok(context) => context,
        Err(error) => return emit_failure(error),
    };
    if preflight_serve_launch(&request.terminfo, &request.executable, &context).is_err() {
        return emit_failure("launch_context_invalid");
    }
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
        // EPERM/EACCES is reported distinctly so callers can tell an
        // environment-denied pathname bind from any other bind failure.
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            return emit_failure("listener_permission_denied")
        }
        Err(_) => return emit_failure("listener_unavailable"),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let _signal_ids = match register_stop_signals(Arc::clone(&stop)) {
        Ok(ids) => ids,
        Err(_) => return emit_failure("signal_registration_failed"),
    };
    let (rows, columns) = host_size();
    let started = StartedMultiServer::start(
        storage,
        &request.terminfo,
        &StartupLaunch {
            executable: &request.executable,
            arguments: &request.arguments,
            context: &context,
            rows,
            columns,
        },
        request.allow_input,
    );
    drop(context);
    let started = match started {
        Ok(started) => started,
        Err(_) => return emit_failure("server_start_failed"),
    };
    let startup = started.startup();
    let mut driver = started.into_driver();
    // Capture is best-effort: a history filesystem failure never prevents
    // running shells or their authenticated control socket from starting.
    let history_path = request.directory.join("history");
    let history_ready = match fs::DirBuilder::new().mode(0o700).create(&history_path) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => true,
        Err(_) => false,
    };
    if history_ready {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|time| u64::try_from(time.as_millis()).ok());
        match prepare_history_store(&history_path, now) {
            Ok(store) => driver.enable_history(store),
            Err(_) => {
                eprintln!("panelink: history setup failed; history capture unavailable");
            }
        }
    }
    // Saved-layout snapshots are best-effort historical metadata in their own
    // layout/ stream; failure never prevents shells or history. Startup may
    // create/validate the directory and clean only recognized layout temps
    // under the held singleton before the writer starts.
    let layout_ready = panelink::layout_store::LayoutStore::prepare(&request.directory)
        .and_then(|store| store.cleanup_temporary_files().map(|_| store))
        .and_then(|store| driver.enable_layout(store));
    if layout_ready.is_err() {
        eprintln!("panelink: saved layout unavailable; shells remain running");
    }
    let generation = driver.generation();
    let mut line =
        String::from("{\"ok\":true,\"action\":\"serve\",\"state\":\"running\",\"generation\":");
    let _ = write!(line, "{generation},\"target\":");
    match startup.target {
        Some(target) => push_target(&mut line, target),
        None => line.push_str("null"),
    }
    let _ = write!(
        line,
        ",\"startup_outcome\":\"{}\",\"rows\":{rows},\"columns\":{columns},\"allow_input\":{},\"history_capture\":\"{}\"}}",
        outcome_name(startup.outcome),
        request.allow_input,
        if driver.history_available() && !driver.history_capture_failed() { "enabled" } else { "unavailable" }
    );
    let clock = monotonic_clock(Instant::now());
    let history_failure = driver.history_failure_signal();
    let history_diagnostics = driver.history_diagnostics();
    if !write_json(&line) {
        driver.stop_connections(&clock);
        drop(driver);
        drop(listener);
        if history_failure.load(Ordering::Relaxed) {
            eprintln!(
                "{}",
                history_diagnostic_line(history_diagnostics.snapshot(), true)
            );
        }
        return 1;
    }
    let mut failed = false;
    let mut reported_capture_failure = false;
    while !stop.load(Ordering::Relaxed) {
        if driver.verify_storage().is_err() {
            failed = true;
            break;
        }
        // A per-session poll error (for example one exited child) does not
        // stop the server or its sibling session.
        let _ = driver.tick(&clock);
        if driver.history_capture_failed() && !reported_capture_failure {
            eprintln!(
                "{}",
                history_diagnostic_line(history_diagnostics.snapshot(), false)
            );
            reported_capture_failure = true;
        } else if !driver.history_capture_failed() {
            reported_capture_failure = false;
        }
        // When every shell has exited there is nothing left to serve: stop
        // so the next `panelink` starts a fresh workspace.
        if driver.owner().all_sessions_ended() {
            // The tick that ended the last session may have queued a reply
            // (for example the result of closing the last pane). Give its
            // worker a short, bounded moment to write it before workers stop.
            thread::sleep(LAST_REPLY_LINGER);
            break;
        }
        match listener.accept(deadline_after(SERVER_ACCEPT_SLICE)) {
            Ok(stream) => {
                // Capacity/spawn failures reject only this attachment.
                let _ = driver.attach(stream);
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
    driver.stop_connections(&clock);
    drop(driver);
    drop(listener);
    if history_failure.load(Ordering::Relaxed) {
        // A final summary includes reasons/outcome raised after the first log.
        eprintln!(
            "{}",
            history_diagnostic_line(history_diagnostics.snapshot(), true)
        );
    }
    if failed {
        emit_failure("server_runtime_failed")
    } else {
        emit_json(
            &format!(
                "{{\"ok\":true,\"action\":\"serve\",\"state\":\"stopped\",\"generation\":{generation}}}"
            ),
            if startup.outcome == Outcome::Applied { 0 } else { 1 },
        )
    }
}

/// Initial root size: the terminal size when stdout is a TTY, clamped to pane
/// bounds; otherwise 24x80.
fn host_size() -> (u16, u16) {
    use panelink::pane_layout::{MAX_PANE_COLUMNS, MAX_PANE_ROWS, MIN_PANE_COLUMNS, MIN_PANE_ROWS};
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ writes one winsize into the valid pointer supplied.
    let result = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) };
    if result != 0 || size.ws_row == 0 || size.ws_col == 0 {
        return (24, 80);
    }
    (
        size.ws_row.clamp(MIN_PANE_ROWS, MAX_PANE_ROWS),
        size.ws_col.clamp(MIN_PANE_COLUMNS, MAX_PANE_COLUMNS),
    )
}

/// Capture this CLI process's own cwd (or an explicit override) and
/// environment as one bounded launch context, minus reserved names.
fn capture_launch_context(cwd: Option<PathBuf>) -> Result<LaunchContext, &'static str> {
    let working_directory = match cwd {
        Some(path) => path,
        None => std::env::current_dir().map_err(|_| "launch_context_invalid")?,
    };
    if !working_directory.is_absolute() {
        return Err("launch_context_invalid");
    }
    filtered_launch_context(working_directory, std::env::vars_os())
}

/// Bound and filter one captured environment. Inherited `PANELINK_*` names
/// (for example when started from inside a Panelink pane) are dropped; the
/// server sets its own owned values for each child.
fn filtered_launch_context(
    working_directory: PathBuf,
    variables: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<LaunchContext, &'static str> {
    let mut environment = Environment::new();
    let mut total = 0_usize;
    for (name, value) in variables {
        // Panelink-owned names are set by the server, never forwarded.
        if name.as_bytes().starts_with(b"PANELINK_") {
            continue;
        }
        total = total
            .checked_add(name.len() + value.len() + 6)
            .filter(|total| *total <= 48 * 1024)
            .ok_or("launch_context_too_large")?;
        if environment.len() >= 1_024 {
            return Err("launch_context_too_large");
        }
        environment.insert(name, value);
    }
    Ok(LaunchContext {
        working_directory,
        environment,
    })
}

fn ui_layout_reply(
    stream: &mut AuthenticatedStream,
) -> Result<panelink::ui_protocol::LayoutReply, &'static str> {
    let bytes = exchange(stream, &panelink::ui_protocol::encode_layout_query())?;
    panelink::ui_protocol::decode_layout_reply(&bytes).map_err(|_| "response_invalid")
}

fn ui_layout(stream: &mut AuthenticatedStream) -> i32 {
    match ui_layout_reply(stream) {
        Ok(layout) => emit_json(&layout_json(&layout), 0),
        Err(error) => emit_failure(error),
    }
}

fn layout_json(layout: &panelink::ui_protocol::LayoutReply) -> String {
    let mut json = String::from("{\"ok\":true,\"action\":\"layout\"");
    let _ = write!(
        json,
        ",\"revision\":{},\"rows\":{},\"columns\":{},\"pane_count\":{},\"panes\":[",
        layout.revision,
        layout.rows,
        layout.columns,
        layout.panes.iter().flatten().count()
    );
    for (index, pane) in layout.panes.iter().flatten().enumerate() {
        if index != 0 {
            json.push(',');
        }
        json.push_str("{\"target\":");
        push_target(&mut json, pane.target);
        let _ = write!(
            json,
            ",\"x\":{},\"y\":{},\"rows\":{},\"columns\":{},\"resize_pending\":{}}}",
            pane.rect.x,
            pane.rect.y,
            pane.rect.rows,
            pane.rect.columns,
            pane.pending_confirmed.is_some()
        );
    }
    json.push(']');
    match layout.divider {
        Some(divider) => {
            let _ = write!(
                json,
                ",\"divider\":{{\"x\":{},\"y\":{},\"rows\":{},\"columns\":{}}}}}",
                divider.x, divider.y, divider.rows, divider.columns
            );
        }
        None => json.push_str(",\"divider\":null}"),
    }
    json
}

/// Split from the caller-captured target/revision. The new zsh child's
/// launch context is this CLI process's own cwd (or `--cwd`) and environment,
/// never the server's; its size is the locally previewed planned rectangle.
fn ui_split(
    context: &mut ContextFile,
    stream: &mut AuthenticatedStream,
    target: TargetRef,
    layout_revision: u64,
    direction: SplitDirection,
    cwd: Option<PathBuf>,
) -> i32 {
    let layout = match ui_layout_reply(stream) {
        Ok(layout) => layout,
        Err(error) => return emit_failure(error),
    };
    // The server independently checks target, revision and size first.
    let (rows, columns) =
        match panelink::ui_protocol::planned_split_size(&layout, target, direction) {
            Ok(size) => size,
            Err(_) => return emit_failure("split_unavailable"),
        };
    let launch_context = match capture_launch_context(cwd) {
        Ok(context) => context,
        Err(error) => return emit_failure(error),
    };
    let key = candidate_key(context.context());
    let mut payload = Vec::new();
    {
        let cwd = launch_context.working_directory.as_os_str().as_bytes();
        let environment = panelink::multi_startup::environment_entries(&launch_context.environment);
        let launch = LaunchRequest {
            key,
            rows,
            columns,
            requested_executable: b"/bin/zsh",
            requested_working_directory: cwd,
            client_working_directory: cwd,
            arguments: &[],
            environment: &environment,
        };
        if encode_split_request(
            &SplitRequest {
                key,
                target,
                expected_layout_revision: layout_revision,
                direction,
                launch: &launch,
            },
            &mut payload,
        )
        .is_err()
        {
            return emit_failure("request_invalid");
        }
    }
    drop(launch_context);
    let reserved = match context.reserve() {
        Ok(reserved) if reserved == key => reserved,
        Ok(_) => return emit_failure("context_key_mismatch"),
        Err(error) => return emit_failure(context_error_label(&error)),
    };
    // Request ID 1 was the layout query on this connection.
    let bytes = exchange_with_id(stream, 2, &payload);
    // The payload held environment values; release it before any output.
    drop(payload);
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(error) => return emit_failure(error),
    };
    let result = match decode_split_result(&bytes) {
        Ok(result) if result.key == reserved => result,
        _ => return emit_failure("response_invalid"),
    };
    if let Err(error) = context.ack(reserved) {
        return emit_failure(context_error_label(&error));
    }
    let applied = result.outcome == Outcome::Applied;
    let mut json = String::from("{\"ok\":");
    json.push_str(if applied { "true" } else { "false" });
    let _ = write!(
        json,
        ",\"action\":\"split\",\"sequence\":{},\"outcome\":\"{}\",\"layout_revision\":{},\"existing_target\":",
        reserved.sequence,
        outcome_name(result.outcome),
        result.committed_layout_revision
    );
    push_target(&mut json, result.existing_target);
    json.push_str(",\"new_target\":");
    match result.new_target {
        Some(target) => push_target(&mut json, target),
        None => json.push_str("null"),
    }
    json.push('}');
    emit_json(&json, if applied { 0 } else { 1 })
}

/// Client wait for one close reply: the server's bounded stop (HUP, 1 s, KILL,
/// 1 s) runs before it replies, and its worker waits up to 3.5 s.
const CLOSE_CLIENT_DEADLINE: Duration = Duration::from_millis(4_500);

/// One schema-9 pane status exchange.
fn request_panes(
    stream: &mut AuthenticatedStream,
    request_id: u64,
) -> Result<PaneReply, &'static str> {
    let bytes = exchange_with_id(stream, request_id, &encode_pane_query())?;
    decode_pane_reply(&bytes).map_err(|_| "response_invalid")
}

fn session_state_name(state: SessionState) -> &'static str {
    match state {
        SessionState::Reserved => "reserved",
        SessionState::Intent => "intent",
        SessionState::Spawned => "spawned",
        SessionState::Applied => "applied",
        SessionState::Failed => "failed",
        SessionState::Unknown => "unknown",
        SessionState::Exited => "exited",
    }
}

fn panes_json(panes: &PaneReply) -> String {
    let mut json = String::from("{\"ok\":true,\"action\":\"panes\"");
    let _ = write!(
        json,
        ",\"layout_revision\":{},\"rows\":{},\"columns\":{},\"pane_count\":{},\"panes\":[",
        panes.layout_revision,
        panes.rows,
        panes.columns,
        panes.panes.iter().flatten().count()
    );
    for (index, pane) in panes.panes.iter().flatten().enumerate() {
        if index != 0 {
            json.push(',');
        }
        let _ = write!(json, "{{\"index\":{},\"target\":", index + 1);
        push_target(&mut json, pane.target);
        let _ = write!(
            json,
            ",\"state\":\"{}\",\"live\":{},\"x\":{},\"y\":{},\"rows\":{},\"columns\":{},\"resize_pending\":{}}}",
            session_state_name(pane.state),
            pane.live,
            pane.rect.x,
            pane.rect.y,
            pane.rect.rows,
            pane.rect.columns,
            pane.resize_pending
        );
    }
    json.push_str("]}");
    json
}

/// Fixed name of a close outcome and, for a rejection, its reason.
pub(super) fn close_outcome_names(outcome: CloseOutcome) -> (&'static str, Option<&'static str>) {
    match outcome {
        CloseOutcome::Stopped(StopOutcome::Exited) => ("exited", None),
        CloseOutcome::Stopped(StopOutcome::Escalated) => ("escalated", None),
        CloseOutcome::Stopped(StopOutcome::Unknown) => ("unknown", None),
        CloseOutcome::Rejected(reason) => (
            "rejected",
            Some(match reason {
                CloseRejection::NoLayout => "no_layout",
                CloseRejection::StaleTarget => "stale_target",
                CloseRejection::StaleLayout => "stale_layout",
                CloseRejection::AlreadyClosed => "already_closed",
                CloseRejection::NotAdmitted => "not_admitted",
            }),
        ),
    }
}

/// Whether a close stopped the pane's child and removed the pane.
pub(super) const fn close_stopped(outcome: CloseOutcome) -> bool {
    matches!(
        outcome,
        CloseOutcome::Stopped(StopOutcome::Exited | StopOutcome::Escalated)
    )
}

fn close_json(action: &'static str, key: OperationKey, result: &CloseResult) -> String {
    let (outcome, reason) = close_outcome_names(result.outcome);
    let mut json = String::from("{\"ok\":");
    json.push_str(if close_stopped(result.outcome) {
        "true"
    } else {
        "false"
    });
    json.push_str(",\"action\":");
    push_json_string(&mut json, action);
    let _ = write!(
        json,
        ",\"sequence\":{},\"outcome\":\"{outcome}\",\"reason\":",
        key.sequence
    );
    match reason {
        Some(reason) => {
            let _ = write!(json, "\"{reason}\"");
        }
        None => json.push_str("null"),
    }
    let _ = write!(
        json,
        ",\"layout_revision\":{},\"survivor_resize_pending\":{},\"target\":",
        result.layout_revision, result.survivor_resize_pending
    );
    match result.target {
        Some(target) => push_target(&mut json, target),
        None => json.push_str("null"),
    }
    json.push('}');
    json
}

/// Send one schema-9 close and read its reply within the close deadline.
///
/// Returns the decoded result for exactly `key`; any other reply is invalid.
fn exchange_close(
    stream: &mut AuthenticatedStream,
    request_id: u64,
    key: OperationKey,
    payload: &[u8],
) -> Result<CloseResult, &'static str> {
    let deadline = deadline_after(CLOSE_CLIENT_DEADLINE);
    let frame =
        Frame::new(MessageKind::Request, request_id, payload).map_err(|_| "request_invalid")?;
    stream
        .write_frame(&frame, deadline)
        .map_err(|_| "transport_failed")?;
    let frame = stream
        .read_frame(deadline)
        .map_err(|_| "transport_failed")?;
    if frame.request_id() != request_id {
        return Err("response_invalid");
    }
    match frame.kind() {
        MessageKind::Response => match decode_close_result(frame.payload()) {
            Ok(result) if result.key == key => Ok(result),
            _ => Err("response_invalid"),
        },
        MessageKind::Error => Err("server_rejected"),
        _ => Err("response_invalid"),
    }
}

fn close_request_payload(
    key: OperationKey,
    target: TargetRef,
    layout_revision: u64,
) -> Result<Vec<u8>, &'static str> {
    let mut payload = Vec::new();
    encode_close_request(
        &CloseRequest {
            key,
            target,
            expected_layout_revision: layout_revision,
        },
        &mut payload,
    )
    .map_err(|_| "request_invalid")?;
    Ok(payload)
}

/// Reserve one durable key, send exactly one close for it and acknowledge
/// the validated result. On any failure the key stays uncertain in the
/// context and is never resent; status settles it later.
fn reserved_close(
    context: &mut ContextFile,
    stream: &mut AuthenticatedStream,
    request_id: u64,
    target: TargetRef,
    layout_revision: u64,
) -> Result<(OperationKey, CloseResult), &'static str> {
    let key = candidate_key(context.context());
    let payload = close_request_payload(key, target, layout_revision)?;
    let reserved = context
        .reserve()
        .map_err(|error| context_error_label(&error))?;
    if reserved != key {
        return Err("context_key_mismatch");
    }
    let result = exchange_close(stream, request_id, key, &payload)?;
    context
        .ack(key)
        .map_err(|error| context_error_label(&error))?;
    Ok((key, result))
}

/// `control ... close TARGET LAYOUT_REV`: explicit stop of one pane session.
fn control_close(
    context: &mut ContextFile,
    stream: &mut AuthenticatedStream,
    target: TargetRef,
    layout_revision: u64,
) -> i32 {
    match reserved_close(context, stream, 1, target, layout_revision) {
        Ok((key, result)) => emit_json(
            &close_json("close", key, &result),
            if close_stopped(result.outcome) { 0 } else { 1 },
        ),
        Err(error) => emit_failure(error),
    }
}

/// `control ... close-status SEQUENCE`: observe a retained close result for
/// one of this context's keys without changing the context.
fn control_close_status(
    context: &ContextFile,
    stream: &mut AuthenticatedStream,
    sequence: u64,
) -> i32 {
    let state = context.context();
    let key = OperationKey {
        generation: state.generation(),
        lease: state.lease(),
        sequence,
    };
    match close_status_exchange(stream, 1, key) {
        Ok(result) => emit_json(
            &close_json("close-status", key, &result),
            if close_stopped(result.outcome) { 0 } else { 1 },
        ),
        Err(error) => emit_failure(error),
    }
}

fn close_status_exchange(
    stream: &mut AuthenticatedStream,
    request_id: u64,
    key: OperationKey,
) -> Result<CloseResult, &'static str> {
    let mut payload = Vec::new();
    encode_close_status_query(CloseStatusQuery { key }, &mut payload)
        .map_err(|_| "request_invalid")?;
    exchange_close(stream, request_id, key, &payload)
}

/// Why a [`WorkspaceLink`] close produced no result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CloseFailure {
    /// The server refused the request before admission; nothing was stopped.
    Refused,
    /// The request may have been admitted but no result was observed.
    Unknown,
    /// The request could not be sent (local or transport failure).
    NotSent(&'static str),
}

/// One short-lived connection for the user-facing `panelink panes` and
/// `panelink close`, on a reusable private context file.
///
/// A fresh Hello issues a new 30-minute lease, and live leases are capped, so
/// these commands resume one CLI lease instead of opening a new one per run.
/// The context's exclusive lock is held for the whole command, serializing
/// concurrent invocations. A context the server no longer recognizes (a new
/// workspace generation, or its pinned pane was closed) is replaced; an
/// operation left uncertain on such a lease is unobservable and is reported,
/// never resent.
pub(super) struct WorkspaceLink {
    context: ContextFile,
    stream: AuthenticatedStream,
    next_request: u64,
    /// An earlier close on a now-unrecognized lease never settled.
    pub(super) discarded_uncertain: bool,
}

impl WorkspaceLink {
    /// Resume (or create) the CLI context at `context_path` against the server
    /// in `directory`.
    pub(super) fn open(directory: &Path, context_path: &Path) -> Result<Self, &'static str> {
        let mut discarded_uncertain = false;
        if fs::symlink_metadata(context_path).is_ok() {
            let context = ContextFile::open_existing(
                context_path,
                deadline_after(CLOSE_CLIENT_DEADLINE + CLIENT_DEADLINE),
            )
            .map_err(|error| context_error_label(&error))?;
            let mut stream = connect_stream(directory)?;
            match handshake(&mut stream, Some(context.context())) {
                Ok(_) => {
                    let mut link = Self {
                        context,
                        stream,
                        next_request: 1,
                        discarded_uncertain,
                    };
                    link.settle_uncertain()?;
                    return Ok(link);
                }
                Err("server_rejected" | "context_mismatch") => {
                    // The lease is unknown to this server, so no result for
                    // it can ever be observed; replace the record while it
                    // is still locked.
                    discarded_uncertain = context.context().uncertain().is_some();
                    fs::remove_file(context_path).map_err(|_| "context_storage_failed")?;
                    drop(context);
                }
                Err(error) => return Err(error),
            }
        }
        let mut stream = connect_stream(directory)?;
        let welcome = handshake(&mut stream, None)?;
        let context = ContextFile::create(
            context_path,
            welcome.generation,
            welcome.lease,
            deadline_after(CLIENT_DEADLINE),
        )
        .map_err(|error| context_error_label(&error))?;
        Ok(Self {
            context,
            stream,
            next_request: 1,
            discarded_uncertain,
        })
    }

    fn request_id(&mut self) -> u64 {
        let id = self.next_request;
        self.next_request += 1;
        id
    }

    /// Settle an earlier uncertain key by schema-2 status; never resend it.
    /// Returns the observed status when a key was uncertain.
    fn settle_uncertain(&mut self) -> Result<Option<OperationStatus>, &'static str> {
        let Some(key) = self.context.context().uncertain() else {
            return Ok(None);
        };
        let payload = encode_session_request(&SessionRequest::OperationStatus { key })
            .map_err(|_| "request_invalid")?;
        let id = self.request_id();
        let bytes = exchange_with_id(&mut self.stream, id, &payload)?;
        let response = decode_response_payload(&bytes).map_err(|_| "response_invalid")?;
        if response.key != key {
            return Err("response_invalid");
        }
        let ResponseBody::Status(status) = response.body else {
            return Err("response_invalid");
        };
        match self
            .context
            .reconcile(key, status)
            .map_err(|error| context_error_label(&error))?
        {
            ReconcileResult::Cleared => Ok(Some(status)),
            ReconcileResult::StillInProgress => Err("operation_uncertain"),
        }
    }

    /// Current layout panes in spatial order.
    pub(super) fn panes(&mut self) -> Result<PaneReply, &'static str> {
        let id = self.request_id();
        request_panes(&mut self.stream, id)
    }

    /// Send exactly one close for `target` at `layout_revision`.
    ///
    /// A lost reply is settled once by a close status query on a resumed
    /// connection (observation only); the close itself is never resent.
    pub(super) fn close(
        &mut self,
        directory: &Path,
        target: TargetRef,
        layout_revision: u64,
    ) -> Result<CloseResult, CloseFailure> {
        let key = candidate_key(self.context.context());
        let payload =
            close_request_payload(key, target, layout_revision).map_err(CloseFailure::NotSent)?;
        let reserved = self
            .context
            .reserve()
            .map_err(|error| CloseFailure::NotSent(context_error_label(&error)))?;
        if reserved != key {
            return Err(CloseFailure::NotSent("context_key_mismatch"));
        }
        let id = self.request_id();
        let outcome = exchange_close(&mut self.stream, id, key, &payload);
        drop(payload);
        let result = match outcome {
            Ok(result) => result,
            Err("server_rejected") => {
                // An Error frame: settle the key by status on this stream.
                // Only a key the ledger never admitted is a clean refusal.
                return match self.settle_uncertain() {
                    Ok(Some(OperationStatus::NotAdmitted)) => Err(CloseFailure::Refused),
                    _ => Err(CloseFailure::Unknown),
                };
            }
            Err(_) => self.observe_lost_close(directory, key)?,
        };
        let _ = self.context.ack(key);
        Ok(result)
    }

    fn observe_lost_close(
        &mut self,
        directory: &Path,
        key: OperationKey,
    ) -> Result<CloseResult, CloseFailure> {
        let mut stream = connect_stream(directory).map_err(|_| CloseFailure::Unknown)?;
        handshake(&mut stream, Some(self.context.context())).map_err(|_| CloseFailure::Unknown)?;
        self.stream = stream;
        self.next_request = 1;
        let id = self.request_id();
        match close_status_exchange(&mut self.stream, id, key) {
            Ok(result) if result.outcome != CloseOutcome::Rejected(CloseRejection::NotAdmitted) => {
                Ok(result)
            }
            _ => Err(CloseFailure::Unknown),
        }
    }
}

/// Type printable text plus Enter as typed keys (never replayed).
fn ui_keys(
    context: &mut ContextFile,
    stream: &mut AuthenticatedStream,
    target: TargetRef,
    owner_revision: u64,
    text: &str,
) -> i32 {
    let key = candidate_key(context.context());
    let mut keys: Vec<Key> = text.chars().map(Key::Char).collect();
    keys.push(Key::Enter);
    let payload = match panelink::ui_protocol::encode_key_input(&KeyInput {
        key,
        target,
        expected_owner_revision: owner_revision,
        keys: &keys,
    }) {
        Ok(payload) => payload,
        Err(_) => return emit_failure("request_invalid"),
    };
    let reserved = match context.reserve() {
        Ok(reserved) if reserved == key => reserved,
        Ok(_) => return emit_failure("context_key_mismatch"),
        Err(error) => return emit_failure(context_error_label(&error)),
    };
    let response = match exchange(stream, &payload) {
        Ok(bytes) => bytes,
        Err(error) => return emit_failure(error),
    };
    handle_reserved_response(context, reserved, &response, "keys", None)
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
        "Usage:\n  panelink server-probe --directory DIR --terminfo TERMINFO [--initialize] [--allow-input] -- EXE [ARG...]\n  panelink serve --directory DIR --terminfo TERMINFO [--initialize] [--allow-input] -- EXE [ARG...]\n  panelink control --directory DIR --context FILE init [TARGET]\n  panelink control --directory DIR --context FILE show|describe|resync|detach\n  panelink control --directory DIR --context FILE layout\n  panelink control --directory DIR --context FILE split TARGET LAYOUT_REV left|right|up|down [--cwd DIR]\n  panelink control --directory DIR --context FILE keys TARGET OWNER_REV TEXT\n  panelink control --directory DIR --context FILE panes\n  panelink control --directory DIR --context FILE close TARGET LAYOUT_REV\n  panelink control --directory DIR --context FILE close-status SEQUENCE\n  panelink control --directory DIR --context FILE read TARGET\n  panelink control --directory DIR --context FILE reconcile TARGET\n  panelink control --directory DIR --context FILE watch TARGET GEN:SEQ LIMIT WAIT_MS\n  panelink control --directory DIR --context FILE claim|release TARGET OWNER_REV\n  panelink control --directory DIR --context FILE handoff TARGET OWNER_REV RECIPIENT_LEASE\n  panelink control --directory DIR --context FILE text|paste|submit TARGET OWNER_REV TEXT\n  panelink control --directory DIR --context FILE interrupt TARGET OWNER_REV\n  panelink control --directory DIR --context FILE status SEQUENCE\n  panelink control --directory DIR --context FILE receipt THROUGH_SEQUENCE\nTARGET is generation:workspace:pane:session:occupant_revision. Use --initialize only for a new empty server directory.\nserve is an experimental two-pane server (at most one split). split launches /bin/zsh with this CLI process's own cwd (or --cwd) and environment and needs a serve started with --allow-input. init TARGET pins a new context to one serve pane; layout, split, keys, panes and close work only against serve. close stops one pane's shell (HUP, then KILL after 1 s) and exits 0 only when it exited or was escalated.\n  panelink attach --directory DIR --context-dir CONTEXT_DIR\nattach is an experimental full-screen client for serve (run it inside a terminal): one connection and one private context file per pane under CONTEXT_DIR (created 0700 if missing). Keys go to the focused pane; left-click focuses; right-click or Ctrl-B then s opens the split menu; Ctrl-X detaches (server-owned shells keep running)."
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

    #[test]
    fn history_diagnostic_lines_keep_serving_context_and_fixed_labels() {
        let snapshot = panelink::history_diagnostics::HistoryDiagnostics::default().snapshot();
        assert_eq!(
            history_diagnostic_line(snapshot, false),
            "panelink: screen history capture failed; shells remain running; reasons=none; writer_drain=not_requested"
        );
        assert_eq!(
            history_diagnostic_line(snapshot, true),
            "panelink: history capture incomplete during shutdown; reasons=none; writer_drain=not_requested"
        );
    }

    #[test]
    fn history_setup_discards_orphans_before_capture_and_refuses_unsafe_temps() {
        struct PrivateDirectory(PathBuf);
        impl Drop for PrivateDirectory {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        use std::os::unix::fs::PermissionsExt;

        let root = PrivateDirectory(std::env::temp_dir().canonicalize().unwrap().join(format!(
            "pl-history-startup-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        )));
        fs::DirBuilder::new().mode(0o700).create(&root.0).unwrap();
        // The runtime holds this lock before preparing history, too.
        let _storage = ServerStorage::initialize(&root.0).unwrap();
        let history = root.0.join("history");
        fs::DirBuilder::new().mode(0o700).create(&history).unwrap();
        let store = panelink::history_store::HistoryStore::open(&history).unwrap();
        let target = test_target();
        store.begin_history(target, 1, false).unwrap();
        let final_path = fs::read_dir(&history)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let final_bytes = fs::read(&final_path).unwrap();
        let final_name = final_path.file_name().unwrap().to_str().unwrap();
        let temporary = history.join(format!(".{final_name}-123-1.tmp"));
        fs::write(&temporary, b"partial").unwrap();
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).unwrap();
        drop(store);

        let ready = prepare_history_store(&history, None).unwrap();
        assert!(!temporary.exists());
        assert_eq!(fs::read(&final_path).unwrap(), final_bytes);
        assert!(ready.load_rows(target).unwrap().is_some());
        drop(ready);

        let sentinel = root.0.join("sentinel");
        fs::write(&sentinel, b"keep").unwrap();
        let unsafe_temp = history.join(format!(".{final_name}-123-2.tmp"));
        std::os::unix::fs::symlink(&sentinel, &unsafe_temp).unwrap();
        assert!(prepare_history_store(&history, None).is_err());
        assert_eq!(fs::read(&final_path).unwrap(), final_bytes);
        assert_eq!(fs::read(sentinel).unwrap(), b"keep");
    }

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
    fn parser_accepts_serve_and_pane_ui_control_forms() {
        let parsed = parse(args(&[
            "serve",
            "--directory",
            "/private/tmp/pl-serve",
            "--terminfo",
            "/private/tmp/terminfo",
            "--allow-input",
            "--",
            "/bin/zsh",
            "-f",
        ]))
        .unwrap();
        let CliCommand::Serve(serve) = parsed else {
            panic!("expected serve");
        };
        assert!(serve.allow_input && !serve.initialize);
        assert_eq!(serve.arguments, args(&["-f"]));
        assert!(matches!(
            parse(args(&["serve", "--help"])),
            Ok(CliCommand::Help)
        ));
        let control = |extra: &[&str]| {
            let mut values = vec![
                "control",
                "--directory",
                "/private/tmp/pl-serve",
                "--context",
                "/private/tmp/c.ctx",
            ];
            values.extend_from_slice(extra);
            parse(args(&values))
        };
        let Ok(CliCommand::Control(parsed)) = control(&["init", "7:2:3:4:5"]) else {
            panic!("init TARGET");
        };
        assert!(matches!(parsed.action, Action::Init(Some(target)) if target == test_target()));
        let Ok(CliCommand::Control(parsed)) = control(&["init"]) else {
            panic!("init");
        };
        assert!(matches!(parsed.action, Action::Init(None)));
        let Ok(CliCommand::Control(parsed)) = control(&["layout"]) else {
            panic!("layout");
        };
        assert!(matches!(parsed.action, Action::Layout));
        for (word, direction) in [
            ("left", SplitDirection::Left),
            ("right", SplitDirection::Right),
            ("up", SplitDirection::Up),
            ("down", SplitDirection::Down),
        ] {
            let Ok(CliCommand::Control(parsed)) = control(&["split", "7:2:3:4:5", "3", word])
            else {
                panic!("split {word}");
            };
            assert!(matches!(
                parsed.action,
                Action::Split { layout_revision: 3, direction: d, cwd: None, .. } if d == direction
            ));
        }
        let Ok(CliCommand::Control(parsed)) =
            control(&["split", "7:2:3:4:5", "1", "down", "--cwd", "/private/tmp"])
        else {
            panic!("split --cwd");
        };
        assert!(matches!(parsed.action, Action::Split { cwd: Some(_), .. }));
        let Ok(CliCommand::Control(parsed)) = control(&["keys", "7:2:3:4:5", "2", "echo hi"])
        else {
            panic!("keys");
        };
        assert!(matches!(
            parsed.action,
            Action::Keys {
                owner_revision: 2,
                ..
            }
        ));
        let long = "x".repeat(panelink::ui_protocol::MAX_KEYS);
        for malformed in [
            vec!["init", "7:2:3:4:5", "extra"],
            vec!["layout", "extra"],
            vec!["split", "7:2:3:4:5", "0", "left"],
            vec!["split", "7:2:3:4:5", "1", "sideways"],
            vec!["split", "7:2:3:4:5", "1", "left", "--cwd", "relative"],
            vec!["split", "7:2:3:4:5", "1", "left", "--other", "/tmp"],
            vec!["keys", "7:2:3:4:5", "1", "tab\there"],
            vec!["keys", "7:2:3:4:5", "1", long.as_str()],
        ] {
            assert!(control(&malformed).is_err(), "accepted {malformed:?}");
        }
    }

    fn control_args(extra: &[&str]) -> Result<CliCommand, ()> {
        let mut values = vec![
            "control",
            "--directory",
            "/private/tmp/pl-serve",
            "--context",
            "/private/tmp/c.ctx",
        ];
        values.extend_from_slice(extra);
        parse(args(&values))
    }

    #[test]
    fn parser_accepts_exact_panes_close_and_close_status_forms() {
        let Ok(CliCommand::Control(parsed)) = control_args(&["panes"]) else {
            panic!("panes");
        };
        assert!(matches!(parsed.action, Action::Panes));
        let Ok(CliCommand::Control(parsed)) = control_args(&["close", "7:2:3:4:5", "4"]) else {
            panic!("close");
        };
        assert!(matches!(
            parsed.action,
            Action::Close { target, layout_revision: 4 } if target == test_target()
        ));
        let Ok(CliCommand::Control(parsed)) = control_args(&["close-status", "9"]) else {
            panic!("close-status");
        };
        assert!(matches!(parsed.action, Action::CloseStatus(9)));
        for malformed in [
            vec!["panes", "extra"],
            vec!["close"],
            vec!["close", "7:2:3:4:5"],
            vec!["close", "7:2:3:4:5", "0"],
            vec!["close", "7:2:3:4", "1"],
            vec!["close", "7:2:3:4:5", "1", "extra"],
            vec!["close", "7:2:3:4:5", "-1"],
            vec!["close-status"],
            vec!["close-status", "0"],
            vec!["close-status", "1", "2"],
        ] {
            assert!(control_args(&malformed).is_err(), "accepted {malformed:?}");
        }
    }

    fn close_result(outcome: CloseOutcome, pending: bool) -> CloseResult {
        CloseResult {
            key: OperationKey {
                generation: 7,
                lease: 3,
                sequence: 2,
            },
            outcome,
            target: Some(test_target()),
            layout_revision: 5,
            survivor_resize_pending: pending,
        }
    }

    #[test]
    fn close_json_reports_every_outcome_with_fixed_names() {
        let key = OperationKey {
            generation: 7,
            lease: 3,
            sequence: 2,
        };
        let json = close_json(
            "close",
            key,
            &close_result(CloseOutcome::Stopped(StopOutcome::Exited), false),
        );
        assert_eq!(
            json,
            "{\"ok\":true,\"action\":\"close\",\"sequence\":2,\"outcome\":\"exited\",\"reason\":null,\"layout_revision\":5,\"survivor_resize_pending\":false,\"target\":\"7:2:3:4:5\"}"
        );
        let json = close_json(
            "close",
            key,
            &close_result(CloseOutcome::Stopped(StopOutcome::Escalated), true),
        );
        assert!(json.starts_with("{\"ok\":true,") && json.contains("\"outcome\":\"escalated\""));
        assert!(json.contains("\"survivor_resize_pending\":true"));
        let json = close_json(
            "close",
            key,
            &close_result(CloseOutcome::Stopped(StopOutcome::Unknown), false),
        );
        assert!(json.starts_with("{\"ok\":false,") && json.contains("\"outcome\":\"unknown\""));
        for (reason, name) in [
            (CloseRejection::NoLayout, "no_layout"),
            (CloseRejection::StaleTarget, "stale_target"),
            (CloseRejection::StaleLayout, "stale_layout"),
            (CloseRejection::AlreadyClosed, "already_closed"),
        ] {
            let json = close_json(
                "close",
                key,
                &close_result(CloseOutcome::Rejected(reason), false),
            );
            assert!(json.starts_with("{\"ok\":false,"));
            assert!(json.contains(&format!("\"reason\":\"{name}\"")), "{json}");
        }
        let not_admitted = CloseResult {
            target: None,
            layout_revision: 0,
            ..close_result(CloseOutcome::Rejected(CloseRejection::NotAdmitted), false)
        };
        let json = close_json("close-status", key, &not_admitted);
        assert!(json.contains("\"reason\":\"not_admitted\"") && json.ends_with("\"target\":null}"));
    }

    #[test]
    fn panes_json_lists_one_based_indexes_in_layout_order() {
        use panelink::multi_runtime::PaneEntry;
        use panelink::pane_layout::Rect;
        let second = TargetRef {
            pane: 6,
            session: 6,
            ..test_target()
        };
        let reply = PaneReply {
            layout_revision: 3,
            rows: 24,
            columns: 81,
            panes: [
                Some(PaneEntry {
                    target: test_target(),
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows: 24,
                        columns: 40,
                    },
                    state: SessionState::Applied,
                    live: true,
                    resize_pending: false,
                }),
                Some(PaneEntry {
                    target: second,
                    rect: Rect {
                        x: 41,
                        y: 0,
                        rows: 24,
                        columns: 40,
                    },
                    state: SessionState::Exited,
                    live: false,
                    resize_pending: true,
                }),
            ],
        };
        assert_eq!(
            panes_json(&reply),
            "{\"ok\":true,\"action\":\"panes\",\"layout_revision\":3,\"rows\":24,\"columns\":81,\"pane_count\":2,\"panes\":[{\"index\":1,\"target\":\"7:2:3:4:5\",\"state\":\"applied\",\"live\":true,\"x\":0,\"y\":0,\"rows\":24,\"columns\":40,\"resize_pending\":false},{\"index\":2,\"target\":\"7:2:6:6:5\",\"state\":\"exited\",\"live\":false,\"x\":41,\"y\":0,\"rows\":24,\"columns\":40,\"resize_pending\":true}]}"
        );
    }

    #[test]
    fn launch_context_drops_inherited_panelink_names_and_stays_bounded() {
        let context = filtered_launch_context(
            PathBuf::from("/private/tmp"),
            [
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("PANELINK_SESSION".into(), "INHERITED".into()),
                ("PANELINK_GENERATION".into(), "INHERITED".into()),
            ],
        )
        .unwrap();
        assert_eq!(context.environment.len(), 1);
        assert!(context
            .environment
            .keys()
            .all(|name| !name.as_bytes().starts_with(b"PANELINK_")));
        let huge = (0..2_000).map(|index| (OsString::from(format!("V{index}")), OsString::new()));
        assert_eq!(
            filtered_launch_context(PathBuf::from("/"), huge).unwrap_err(),
            "launch_context_too_large"
        );
    }

    #[test]
    fn attach_parser_accepts_exact_form_only() {
        let Ok(CliCommand::Attach(attach)) = parse(args(&[
            "attach",
            "--directory",
            "/private/tmp/pl-serve",
            "--context-dir",
            "/private/tmp/pl-ctx",
        ])) else {
            panic!("attach");
        };
        assert_eq!(attach.directory, PathBuf::from("/private/tmp/pl-serve"));
        assert_eq!(attach.context_dir, PathBuf::from("/private/tmp/pl-ctx"));
        assert!(matches!(
            parse(args(&["attach", "--help"])),
            Ok(CliCommand::Help)
        ));
        for malformed in [
            vec!["attach"],
            vec!["attach", "--directory", "/private/tmp/s"],
            vec![
                "attach",
                "--directory",
                "relative",
                "--context-dir",
                "/tmp/c",
            ],
            vec![
                "attach",
                "--directory",
                "/tmp/s",
                "--context-dir",
                "relative",
            ],
            vec!["attach", "--context-dir", "/tmp/c", "--directory", "/tmp/s"],
            vec![
                "attach",
                "--directory",
                "/tmp/s",
                "--context-dir",
                "/tmp/c",
                "x",
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
