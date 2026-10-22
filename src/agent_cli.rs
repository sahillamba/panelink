//! Agent-facing pane operations over the default workspace's existing control
//! client. This module does not open a server, own a PTY, or persist input.

use crate::default_open::{self, StateRoot};
use crate::p3_cli::{AgentReconciliation, WorkspaceLink};
use panelink::close_protocol::{CloseOutcome, StopOutcome};
use panelink::control_protocol::ReadSource;
use panelink::control_protocol::TargetRef;
use panelink::event_protocol::{Event, EventBody};
use panelink::multi_runtime::{PaneEntry, PaneReply};
use panelink::protocol::{OperationStatus, Outcome};
use panelink::session_protocol::SessionInfo;
use panelink::terminal::Key;
use panelink::watch_protocol::{WatchStatus, MAX_WATCH_EVENTS, MAX_WATCH_WAIT_MS};
use std::ffi::OsString;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{self, Write as IoWrite};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_VISIBLE_LINES: u16 = 1_024;
const MAX_WAIT_SECONDS: u64 = 86_400;
const POLL_WAIT_MS: u16 = 200;

#[derive(Debug)]
struct CliError {
    code: &'static str,
    message: &'static str,
    usage: bool,
}

impl CliError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            usage: false,
        }
    }

    fn usage() -> Self {
        Self {
            code: "invalid_arguments",
            message: "invalid pane command syntax; run 'panelink --help'",
            usage: true,
        }
    }
}

#[derive(Clone, Copy)]
struct Options<'a> {
    json: bool,
    caller: &'a str,
}

struct Workspace {
    state: StateRoot,
    caller_dir: PathBuf,
}

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let result = run_inner(args);
    match result {
        Ok(code) => code,
        Err(error) => {
            let json_mode = JSON_ERRORS.with(std::cell::Cell::get);
            if error.usage && !json_mode {
                eprintln!("{}", error.message);
            }
            let code = if error.usage { 2 } else { 1 };
            if json_mode {
                let mut json = String::from("{\"ok\":false,\"error\":");
                push_json_string(&mut json, error.code);
                json.push_str(",\"message\":");
                push_json_string(&mut json, error.message);
                json.push('}');
                write_line(&json);
            } else if !error.usage {
                eprintln!("panelink: {}", error.message);
            }
            code
        }
    }
}

thread_local! {
    static JSON_ERRORS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn run_inner(args: Vec<OsString>) -> Result<i32, CliError> {
    if args.is_empty() || args.len() > 80 {
        return Err(CliError::usage());
    }
    let raw_words = args
        .iter()
        .map(|value| value.to_str().ok_or_else(CliError::usage))
        .collect::<Result<Vec<_>, _>>()?;
    let json = raw_words.contains(&"--json");
    JSON_ERRORS.with(|enabled| enabled.set(json));
    let caller = selected_caller(&raw_words)?;
    let words = strip_agent_selector(&raw_words);
    let options = Options {
        json,
        caller: &caller,
    };

    match words[0] {
        "pane" => run_pane(&words[1..], options),
        "wait" => run_wait(&words[1..], options),
        "events" => run_events(&words[1..], options),
        "--help" | "-h" => {
            print_help();
            Ok(0)
        }
        _ => Err(CliError::usage()),
    }
}

fn selected_caller(words: &[&str]) -> Result<String, CliError> {
    let explicit = words
        .windows(2)
        .find(|pair| pair[0] == "--as")
        .map(|pair| pair[1]);
    let environment = std::env::var("PANELINK_AGENT").ok();
    let caller = explicit
        .or(environment.as_deref())
        .unwrap_or("agent")
        .to_owned();
    if caller.is_empty()
        || caller.len() > 64
        || !caller
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(CliError::usage());
    }
    Ok(caller)
}

fn strip_agent_selector<'a>(words: &'a [&'a str]) -> Vec<&'a str> {
    let mut filtered = Vec::with_capacity(words.len());
    let mut skip_next = false;
    for word in words {
        if skip_next {
            skip_next = false;
        } else if *word == "--as" {
            skip_next = true;
        } else if !word.starts_with("--as=") {
            filtered.push(*word);
        }
    }
    filtered
}

fn workspace(caller: &str) -> Result<Workspace, CliError> {
    let state = resolved_state()?;
    for directory in [&state.root, &state.server] {
        let metadata = fs::symlink_metadata(directory).map_err(|_| {
            CliError::new(
                "no_workspace",
                "no running default workspace; run 'panelink' first",
            )
        })?;
        if !metadata.is_dir()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o077 != 0
        {
            return Err(CliError::new(
                "state_directory_unsafe",
                "Panelink state directories must be private directories owned by this user",
            ));
        }
    }
    match fs::symlink_metadata(state.server.join("control.sock")) {
        Ok(metadata) if metadata.file_type().is_socket() => {}
        _ => {
            return Err(CliError::new(
                "no_workspace",
                "no running default workspace; run 'panelink' first",
            ));
        }
    }
    let agent_root = state.root.join("agent-contexts");
    ensure_private_directory(&agent_root)?;
    let caller_dir = agent_root.join(caller);
    ensure_private_directory(&caller_dir)?;
    Ok(Workspace { state, caller_dir })
}

fn resolved_state() -> Result<StateRoot, CliError> {
    default_open::resolve_root(std::env::var_os("PANELINK_HOME"), std::env::var_os("HOME")).map_err(
        |_| {
            CliError::new(
                "state_unavailable",
                "could not resolve the Panelink state root",
            )
        },
    )
}

fn ensure_private_directory(path: &Path) -> Result<(), CliError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir()
                || metadata.uid() != rustix::process::geteuid().as_raw()
                || metadata.mode() & 0o077 != 0
            {
                return Err(CliError::new(
                    "context_directory_unsafe",
                    "agent context directory must be a private directory owned by this user",
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .map_err(|_| {
                    CliError::new(
                        "context_directory_unavailable",
                        "could not create agent context directory",
                    )
                })?;
            let metadata = fs::symlink_metadata(path).map_err(|_| {
                CliError::new(
                    "context_directory_unsafe",
                    "agent context directory is unavailable",
                )
            })?;
            if !metadata.is_dir()
                || metadata.uid() != rustix::process::geteuid().as_raw()
                || metadata.mode() & 0o077 != 0
            {
                return Err(CliError::new(
                    "context_directory_unsafe",
                    "agent context directory must be a private directory owned by this user",
                ));
            }
        }
        Err(_) => {
            return Err(CliError::new(
                "context_directory_unavailable",
                "could not inspect agent context directory",
            ));
        }
    }
    Ok(())
}

impl Workspace {
    fn inventory(&self) -> Result<WorkspaceLink, CliError> {
        let path = self.caller_dir.join("inventory.ctx");
        WorkspaceLink::open(&self.state.server, &path).map_err(link_error)
    }

    fn pane_link(&self, target: TargetRef) -> Result<WorkspaceLink, CliError> {
        let name = target_name(target);
        let path = self.caller_dir.join(format!("{name}.ctx"));
        WorkspaceLink::open_for_target(&self.state.server, &path, target).map_err(link_error)
    }

    fn pane_events_link(&self, target: TargetRef) -> Result<WorkspaceLink, CliError> {
        let name = format!("{}-watch", target_name(target));
        let path = self.caller_dir.join(format!("{name}.ctx"));
        WorkspaceLink::open_for_target_events(&self.state.server, &path, target).map_err(link_error)
    }
}

fn target_name(target: TargetRef) -> String {
    format!(
        "{}-{}-{}-{}-{}",
        target.generation, target.workspace, target.pane, target.session, target.occupant_revision
    )
}

fn link_error(error: &'static str) -> CliError {
    match error {
        "context_target_mismatch" => CliError::new(
            "stale_target",
            "stale pane target; re-run 'panelink pane list'",
        ),
        "previous_operation_unobservable" => CliError::new(
            "outcome_unknown",
            "an earlier operation belongs to an unrecognized workspace; it was not resent",
        ),
        "context_storage_failed" | "context_unavailable" | "context_invalid" => CliError::new(
            "context_unavailable",
            "could not safely open the private agent context",
        ),
        _ => CliError::new(
            "control_unavailable",
            "could not connect to or query the workspace",
        ),
    }
}

fn run_pane(args: &[&str], options: Options<'_>) -> Result<i32, CliError> {
    let action = args.first().ok_or_else(CliError::usage)?;
    let workspace = workspace(options.caller)?;
    match *action {
        "list" => pane_list(&workspace, &args[1..], options),
        "read" => pane_read(&workspace, &args[1..], options),
        "claim" => pane_owner(&workspace, &args[1..], options, true),
        "release" => pane_owner(&workspace, &args[1..], options, false),
        "send" => pane_send(&workspace, &args[1..], options),
        "keys" => pane_keys(&workspace, &args[1..], options),
        "interrupt" => pane_interrupt(&workspace, &args[1..], options),
        "close" => pane_close(&workspace, &args[1..], options),
        _ => Err(CliError::usage()),
    }
}

fn pane_list(workspace: &Workspace, args: &[&str], options: Options<'_>) -> Result<i32, CliError> {
    if args.iter().any(|value| *value != "--json") {
        return Err(CliError::usage());
    }
    let panes = all_panes(workspace)?;
    let expected_revision = panes.layout_revision;
    let entries = panes.panes.iter().flatten().copied().collect::<Vec<_>>();
    let mut owners = Vec::with_capacity(entries.len());
    for pane in &entries {
        let mut link = workspace.pane_link(pane.target)?;
        let info = link.describe_info().map_err(link_error)?;
        if info.target != pane.target {
            return Err(stale_target());
        }
        owners.push(owner_label(info, link.lease()));
    }
    if all_panes(workspace)?.layout_revision != expected_revision {
        return Err(stale_target());
    }
    if options.json {
        let mut json = String::from("{\"ok\":true,\"action\":\"pane_list\"");
        let _ = write!(
            json,
            ",\"layout_revision\":{},\"pane_count\":{},\"panes\":[",
            panes.layout_revision,
            entries.len()
        );
        for (index, (pane, owner)) in entries.iter().zip(owners.iter()).enumerate() {
            if index != 0 {
                json.push(',');
            }
            push_pane_json(&mut json, index + 1, pane, owner);
        }
        json.push_str("]}");
        write_line(&json);
    } else {
        println!(
            "{} pane(s), layout revision {}:",
            entries.len(),
            panes.layout_revision
        );
        for (index, (pane, owner)) in entries.iter().zip(owners.iter()).enumerate() {
            println!(
                "{}  {}  {}x{} at col {}, row {}  target {}  input owner {}",
                index + 1,
                if pane.live { "alive" } else { "exited" },
                pane.rect.rows,
                pane.rect.columns,
                pane.rect.x,
                pane.rect.y,
                target_string(pane.target),
                owner
            );
        }
    }
    Ok(0)
}

fn push_pane_json(json: &mut String, index: usize, pane: &PaneEntry, owner: &str) {
    let _ = write!(json, "{{\"index\":{index},\"target\":");
    push_json_string(json, &target_string(pane.target));
    let _ = write!(
        json,
        ",\"cwd\":null,\"command\":null,\"size\":{{\"rows\":{},\"columns\":{},\"x\":{},\"y\":{}}},\"alive\":{},\"exited\":{}",
        pane.rect.rows, pane.rect.columns, pane.rect.x, pane.rect.y, pane.live, !pane.live
    );
    json.push_str(",\"exit_status\":");
    push_exit_json(json, pane.exit);
    json.push_str(",\"input_owner\":");
    push_json_string(json, owner);
    json.push('}');
}

fn push_exit_json(json: &mut String, exit: Option<panelink::multi_runtime::PaneExit>) {
    match exit {
        Some(panelink::multi_runtime::PaneExit::Code(code)) => {
            let _ = write!(json, "{{\"code\":{code}}}");
        }
        Some(panelink::multi_runtime::PaneExit::Signal(signal)) => {
            let _ = write!(json, "{{\"signal\":{signal}}}");
        }
        None => json.push_str("null"),
    }
}

fn owner_label(info: SessionInfo, caller_lease: u64) -> String {
    match info.owner_lease {
        0 => "none".to_owned(),
        lease if lease == caller_lease => "you".to_owned(),
        lease => format!("agent:{lease}"),
    }
}

fn pane_read(workspace: &Workspace, args: &[&str], options: Options<'_>) -> Result<i32, CliError> {
    let (index, lines, scrollback) = parse_read_args(args)?;
    let (target, revision, _) = resolve_index(workspace, index)?;
    ensure_current_index(workspace, index, target, revision)?;
    let mut link = workspace.pane_link(target)?;
    if link.discarded_uncertain {
        return Err(CliError::new(
            "outcome_unknown",
            "an earlier operation belongs to an unrecognized workspace; it was not resent",
        ));
    }
    if let Some(status) = link.settled_uncertain {
        return Err(CliError::new(
            "previous_operation_settled",
            match status {
                OperationStatus::NotAdmitted => "an earlier operation was not admitted; no replacement operation was issued",
                OperationStatus::Completed(_) => "an earlier operation was settled after an uncertain acknowledgment; no replacement operation was issued",
                OperationStatus::InProgress => "an earlier operation is still in progress; no replacement operation was issued",
            },
        ));
    }
    let source = if scrollback {
        ReadSource::LiveScrollback
    } else {
        ReadSource::Screen
    };
    let screen = link
        .read_screen(target, source, 65_536, lines)
        .map_err(|error| {
            if error == "live_scrollback_unavailable" {
                CliError::new(
                    "live_scrollback_unavailable",
                    "live scrollback is not available in this server",
                )
            } else if error == "stale_target" {
                stale_target()
            } else {
                CliError::new(
                    "read_failed",
                    "pane read failed; the target was not retargeted",
                )
            }
        })?;
    let (current, current_revision, _) = resolve_index(workspace, index)?;
    if current != target || current_revision != revision {
        return Err(stale_target());
    }
    if options.json {
        let mut json = String::from("{\"ok\":true,\"action\":\"pane_read\",\"index\":");
        let _ = write!(json, "{index},\"target\":");
        push_json_string(&mut json, &target_string(screen.target));
        let _ = write!(
            json,
            ",\"source\":\"{}\",\"screen_revision\":{},\"truncated\":{},\"text\":",
            if scrollback {
                "live_scrollback"
            } else {
                "screen"
            },
            screen.screen_revision,
            screen.truncated
        );
        push_json_string(&mut json, &screen.text);
        json.push('}');
        write_line(&json);
    } else {
        print!("{}", screen.text);
        let _ = io::stdout().flush();
    }
    Ok(0)
}

fn pane_owner(
    workspace: &Workspace,
    args: &[&str],
    options: Options<'_>,
    claim: bool,
) -> Result<i32, CliError> {
    let index = parse_index(args.first().copied().ok_or_else(CliError::usage)?)?;
    if args[1..].iter().any(|argument| !is_common_flag(argument)) {
        return Err(CliError::usage());
    }
    let (target, revision, _) = resolve_index(workspace, index)?;
    ensure_current_index(workspace, index, target, revision)?;
    let mut link = workspace.pane_link(target)?;
    if link.discarded_uncertain {
        return Err(CliError::new(
            "outcome_unknown",
            "an earlier operation belongs to an unrecognized workspace; it was not resent",
        ));
    }
    if let Some(status) = link.settled_uncertain {
        return Err(CliError::new(
            "previous_operation_settled",
            match status {
                OperationStatus::NotAdmitted => "an earlier operation was not admitted; no replacement input was sent",
                OperationStatus::Completed(_) => "an earlier operation was settled after an uncertain acknowledgment; no replacement input was sent",
                OperationStatus::InProgress => "an earlier operation is still in progress; no replacement input was sent",
            },
        ));
    }
    let before = link.describe_info().map_err(link_error)?;
    if before.target != target {
        return Err(stale_target());
    }
    let status = link
        .change_owner(target, before.owner_revision, claim)
        .map_err(owner_error)?;
    let after = link.describe_info().map_err(link_error)?;
    if after.target != target || after.owner_lease != if claim { link.lease() } else { 0 } {
        return Err(CliError::new(
            "input_ownership_lost",
            "input ownership changed before the operation was confirmed",
        ));
    }
    let (current, current_revision, _) = resolve_index(workspace, index)?;
    if current != target || current_revision != revision {
        return Err(stale_target());
    }
    emit_status_result(
        options,
        if claim { "pane_claim" } else { "pane_release" },
        index,
        target,
        status,
        after.owner_revision,
        if claim { "claimed" } else { "released" },
    );
    Ok(0)
}

fn pane_send(workspace: &Workspace, args: &[&str], options: Options<'_>) -> Result<i32, CliError> {
    let index = parse_index(args.first().copied().ok_or_else(CliError::usage)?)?;
    let mut enter = false;
    let mut claim = false;
    let mut text = Vec::new();
    for word in &args[1..] {
        match *word {
            "--enter" => enter = true,
            "--claim" => claim = true,
            "--json" => {}
            value if value.starts_with("--as=") => {}
            value if value.starts_with('-') => return Err(CliError::usage()),
            value => text.push(value),
        }
    }
    if text.is_empty() {
        return Err(CliError::usage());
    }
    let text = text.join(" ");
    if text.len() > panelink::control_protocol::MAX_INPUT_TEXT_BYTES
        || text.chars().any(char::is_control)
    {
        return Err(CliError::usage());
    }
    let (target, revision, _) = resolve_index(workspace, index)?;
    ensure_current_index(workspace, index, target, revision)?;
    let mut link = workspace.pane_link(target)?;
    if link.discarded_uncertain {
        return Err(CliError::new(
            "outcome_unknown",
            "an earlier operation belongs to an unrecognized workspace; it was not resent",
        ));
    }
    let before = link.describe_info().map_err(link_error)?;
    if before.target != target {
        return Err(stale_target());
    }
    let owner_revision = if claim {
        let status = link
            .change_owner(target, before.owner_revision, true)
            .map_err(owner_error)?;
        if status != OperationStatus::Completed(Outcome::Applied) {
            return Err(CliError::new("claim_failed", "input claim was not applied"));
        }
        let after_claim = link.describe_info().map_err(link_error)?;
        if after_claim.target != target || after_claim.owner_lease != link.lease() {
            return Err(CliError::new(
                "input_ownership_lost",
                "input ownership was taken back before send",
            ));
        }
        ensure_current_index(workspace, index, target, revision)?;
        after_claim.owner_revision
    } else {
        if before.owner_lease != link.lease() {
            return Err(CliError::new(
                "input_not_claimed",
                "claim this pane before sending input (or pass --claim)",
            ));
        }
        before.owner_revision
    };
    // The request carries the complete target, so the server applied it to
    // this exact occupant or rejected it. A later layout change (for example
    // the child exiting because of this very input) must not turn an applied
    // send into a reported failure.
    let status = link
        .send_text(target, owner_revision, &text, enter)
        .map_err(owner_error)?;
    emit_status_result(
        options,
        "pane_send",
        index,
        target,
        status,
        owner_revision,
        if enter { "submitted" } else { "sent" },
    );
    Ok(0)
}

fn pane_keys(workspace: &Workspace, args: &[&str], options: Options<'_>) -> Result<i32, CliError> {
    let index = parse_index(args.first().copied().ok_or_else(CliError::usage)?)?;
    let keys = args[1..]
        .iter()
        .filter(|word| !is_common_flag(word))
        .map(|word| parse_key(word))
        .collect::<Result<Vec<_>, _>>()?;
    if keys.is_empty() {
        return Err(CliError::usage());
    }
    if keys.len() > panelink::ui_protocol::MAX_KEYS {
        return Err(CliError::usage());
    }
    let (target, revision, _) = resolve_index(workspace, index)?;
    ensure_current_index(workspace, index, target, revision)?;
    let mut link = workspace.pane_link(target)?;
    if link.discarded_uncertain {
        return Err(CliError::new(
            "outcome_unknown",
            "an earlier operation belongs to an unrecognized workspace; it was not resent",
        ));
    }
    let info = link.describe_info().map_err(link_error)?;
    if info.owner_lease != link.lease() {
        return Err(CliError::new(
            "input_not_claimed",
            "claim this pane before sending keys",
        ));
    }
    let status = link
        .send_keys(target, info.owner_revision, &keys)
        .map_err(owner_error)?;
    let (current, current_revision, _) = resolve_index(workspace, index)?;
    if current != target || current_revision != revision {
        return Err(stale_target());
    }
    emit_status_result(
        options,
        "pane_keys",
        index,
        target,
        status,
        info.owner_revision,
        "sent",
    );
    Ok(0)
}

fn pane_interrupt(
    workspace: &Workspace,
    args: &[&str],
    options: Options<'_>,
) -> Result<i32, CliError> {
    let index = parse_index(args.first().copied().ok_or_else(CliError::usage)?)?;
    if args[1..].iter().any(|argument| !is_common_flag(argument)) {
        return Err(CliError::usage());
    }
    let (target, revision, _) = resolve_index(workspace, index)?;
    ensure_current_index(workspace, index, target, revision)?;
    let mut link = workspace.pane_link(target)?;
    if link.discarded_uncertain {
        return Err(CliError::new(
            "outcome_unknown",
            "an earlier operation belongs to an unrecognized workspace; it was not resent",
        ));
    }
    let info = link.describe_info().map_err(link_error)?;
    if info.owner_lease != link.lease() {
        return Err(CliError::new(
            "input_not_claimed",
            "claim this pane before interrupting it",
        ));
    }
    let status = link
        .interrupt_target(target, info.owner_revision)
        .map_err(owner_error)?;
    let (current, current_revision, _) = resolve_index(workspace, index)?;
    if current != target || current_revision != revision {
        return Err(stale_target());
    }
    emit_status_result(
        options,
        "pane_interrupt",
        index,
        target,
        status,
        info.owner_revision,
        "interrupted",
    );
    Ok(0)
}

fn pane_close(workspace: &Workspace, args: &[&str], options: Options<'_>) -> Result<i32, CliError> {
    let index = parse_index(args.first().copied().ok_or_else(CliError::usage)?)?;
    if args[1..].iter().any(|argument| !is_common_flag(argument)) {
        return Err(CliError::usage());
    }
    let (target, revision, panes) = resolve_index(workspace, index)?;
    ensure_current_index(workspace, index, target, revision)?;
    let last_pane = panes.panes.iter().flatten().count() == 1;
    let mut link = workspace.pane_link(target)?;
    let current = link.panes().map_err(link_error)?;
    if current.layout_revision != revision
        || current
            .panes
            .iter()
            .flatten()
            .nth(index - 1)
            .is_none_or(|pane| pane.target != target)
    {
        return Err(stale_target());
    }
    let result = link
        .close_target(&workspace.state.server, target, revision)
        .map_err(|error| {
            if error == "close_refused" || error == "stale_target" {
                stale_target()
            } else {
                CliError::new(
                    "close_unknown",
                    "close result is unknown; the pane may still be running",
                )
            }
        })?;
    let (outcome, code) = match result.outcome {
        CloseOutcome::Stopped(StopOutcome::Exited) => ("exited", 0),
        CloseOutcome::Stopped(StopOutcome::Escalated) => ("escalated", 0),
        CloseOutcome::Stopped(StopOutcome::Unknown) => ("unknown", 1),
        CloseOutcome::Rejected(_) => ("rejected", 1),
    };
    if code != 0 {
        return Err(CliError::new(
            "close_unknown",
            "close result is unknown; the pane may still be running",
        ));
    }
    if options.json {
        let mut json = String::from("{\"ok\":true,\"action\":\"pane_close\",\"index\":");
        let _ = write!(json, "{index},\"target\":");
        push_json_string(&mut json, &target_string(target));
        let _ = write!(
            json,
            ",\"outcome\":\"{outcome}\",\"workspace_ended\":{last_pane}}}"
        );
        write_line(&json);
    } else {
        println!(
            "panelink: closed pane {index} ({outcome}){}",
            if last_pane {
                "; the workspace has ended"
            } else {
                ""
            }
        );
    }
    Ok(0)
}

fn emit_status_result(
    options: Options<'_>,
    action: &str,
    index: usize,
    target: TargetRef,
    status: OperationStatus,
    owner_revision: u64,
    message: &str,
) {
    if options.json {
        let mut json = String::from("{\"ok\":true,\"action\":");
        push_json_string(&mut json, action);
        let _ = write!(json, ",\"index\":{index},\"target\":");
        push_json_string(&mut json, &target_string(target));
        let _ = write!(
            json,
            ",\"status\":\"{}\",\"owner_revision\":{owner_revision},\"message\":",
            status_name(status)
        );
        push_json_string(&mut json, message);
        json.push('}');
        write_line(&json);
    } else {
        println!("panelink: {message} pane {index}; owner revision {owner_revision}");
    }
}

fn owner_error(error: &'static str) -> CliError {
    match error {
        "input_ownership_lost" | "input_not_claimed" | "stale_owner" | "expired_owner" => CliError::new(
            "input_ownership_lost",
            "input ownership was lost (possibly taken back by the human UI); no input was retried",
        ),
        "stale_target" | "context_target_mismatch" => stale_target(),
        "previous_operation_settled" => CliError::new(
            "previous_operation_settled",
            "an earlier operation was reconciled; no replacement input was sent",
        ),
        "operation_acknowledgment_lost" | "operation_uncertain" | "outcome_unknown" => CliError::new(
            "outcome_unknown",
            "operation outcome is unknown; non-replayable input was not resent",
        ),
        "operation_refused" | "input_busy" => CliError::new("input_busy", "input claim was refused because another owner holds the pane"),
        "live_scrollback_unavailable" => CliError::new("live_scrollback_unavailable", "live scrollback is not available in this server"),
        _ => CliError::new("input_failed", "pane input or ownership operation failed; no input was retried"),
    }
}

fn resolve_index(
    workspace: &Workspace,
    index: usize,
) -> Result<(TargetRef, u64, PaneReply), CliError> {
    let mut link = workspace.inventory()?;
    let panes = link.panes().map_err(link_error)?;
    let pane = panes.panes.iter().flatten().nth(index - 1).ok_or_else(|| {
        CliError::new(
            "pane_not_found",
            "pane index is not present; run 'panelink pane list'",
        )
    })?;
    Ok((pane.target, panes.layout_revision, panes))
}

fn all_panes(workspace: &Workspace) -> Result<PaneReply, CliError> {
    workspace.inventory()?.panes().map_err(link_error)
}

fn ensure_current_index(
    workspace: &Workspace,
    index: usize,
    expected_target: TargetRef,
    expected_revision: u64,
) -> Result<(), CliError> {
    let (target, revision, _) = resolve_index(workspace, index)?;
    if target != expected_target || revision != expected_revision {
        Err(stale_target())
    } else {
        Ok(())
    }
}

fn stale_target() -> CliError {
    CliError::new(
        "stale_target",
        "stale pane index/target; layout changed, so no replacement pane was selected",
    )
}

fn parse_index(value: &str) -> Result<usize, CliError> {
    if value.is_empty() || value.len() > 4 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CliError::usage());
    }
    value
        .parse()
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(CliError::usage)
}

fn parse_read_args(args: &[&str]) -> Result<(usize, u16, bool), CliError> {
    let index = parse_index(args.first().copied().ok_or_else(CliError::usage)?)?;
    let mut lines = DEFAULT_VISIBLE_LINES;
    let mut scrollback = false;
    let mut cursor = 1;
    while cursor < args.len() {
        match args[cursor] {
            "--scrollback" => {
                scrollback = true;
                cursor += 1;
            }
            "--lines" => {
                cursor += 1;
                let value = args.get(cursor).ok_or_else(CliError::usage)?;
                let parsed = parse_index(value)?;
                lines = u16::try_from(parsed)
                    .ok()
                    .filter(|value| *value <= panelink::control_protocol::MAX_READ_LINES)
                    .ok_or_else(CliError::usage)?;
                cursor += 1;
            }
            "--json" => cursor += 1,
            "--as" => cursor += 2,
            value if value.starts_with("--as=") => cursor += 1,
            _ => return Err(CliError::usage()),
        }
    }
    Ok((index, lines, scrollback))
}

fn parse_key(value: &str) -> Result<Key, CliError> {
    Ok(match value {
        "Enter" => Key::Enter,
        "Tab" => Key::Tab,
        "Esc" => Key::Escape,
        "C-c" => Key::Interrupt,
        "Up" => Key::Up,
        "Down" => Key::Down,
        "Left" => Key::Left,
        "Right" => Key::Right,
        "Backspace" => Key::Backspace,
        _ => return Err(CliError::usage()),
    })
}

fn is_common_flag(value: &str) -> bool {
    matches!(value, "--json" | "--as") || value.starts_with("--as=")
}

fn run_wait(args: &[&str], options: Options<'_>) -> Result<i32, CliError> {
    let index = parse_index(args.first().copied().ok_or_else(CliError::usage)?)?;
    let mut condition = None;
    let mut text = None;
    let mut regex = false;
    let mut timeout = None;
    let mut quiet = None;
    let mut cursor = 1;
    while cursor < args.len() {
        match args[cursor] {
            "--text" => {
                if condition.replace("text").is_some() {
                    return Err(CliError::usage());
                }
                cursor += 1;
                text = Some(args.get(cursor).copied().ok_or_else(CliError::usage)?);
                cursor += 1;
            }
            "--regex" => {
                regex = true;
                cursor += 1;
            }
            "--quiet" => {
                if condition.replace("quiet").is_some() {
                    return Err(CliError::usage());
                }
                cursor += 1;
                quiet = Some(parse_seconds(
                    args.get(cursor).copied().ok_or_else(CliError::usage)?,
                )?);
                cursor += 1;
            }
            "--exit" | "--change" => {
                if condition
                    .replace(args[cursor].trim_start_matches("--"))
                    .is_some()
                {
                    return Err(CliError::usage());
                }
                cursor += 1;
            }
            "--timeout" => {
                cursor += 1;
                timeout = Some(parse_seconds(
                    args.get(cursor).copied().ok_or_else(CliError::usage)?,
                )?);
                cursor += 1;
            }
            "--json" => cursor += 1,
            "--as" => cursor += 2,
            value if value.starts_with("--as=") => cursor += 1,
            _ => return Err(CliError::usage()),
        }
    }
    let condition = condition.ok_or_else(CliError::usage)?;
    let timeout = timeout.ok_or_else(CliError::usage)?;
    if timeout == 0
        || (condition == "text" && text.is_none())
        || (condition != "text" && text.is_some())
        || (condition == "quiet" && quiet.is_none())
        || (condition != "quiet" && quiet.is_some())
        || (regex && condition != "text")
    {
        return Err(CliError::usage());
    }
    let workspace = workspace(options.caller)?;
    let (target, revision, _) = resolve_index(&workspace, index)?;
    let result = wait_loop(
        &workspace, options, index, target, revision, condition, text, regex, quiet, timeout,
    );
    match result {
        // An exited child's pane collapses, so the exit can surface as a
        // layout change or a vanished target before the exit event is read.
        // The pinned target leaving the layout entirely means its session
        // ended; never match a replacement occupant.
        Err(error)
            if condition == "exit"
                && matches!(error.code, "stale_target" | "control_unavailable")
                && matches!(target_present(&workspace, target), Ok(false)) =>
        {
            finish_wait(&workspace, options, index, target, condition, true, false)
        }
        other => other,
    }
}

fn target_present(workspace: &Workspace, target: TargetRef) -> Result<bool, CliError> {
    Ok(all_panes(workspace)?
        .panes
        .iter()
        .flatten()
        .any(|entry| entry.target == target))
}

#[allow(clippy::too_many_arguments)]
fn wait_loop(
    workspace: &Workspace,
    options: Options<'_>,
    index: usize,
    target: TargetRef,
    revision: u64,
    condition: &str,
    text: Option<&str>,
    regex: bool,
    quiet: Option<u64>,
    timeout: u64,
) -> Result<i32, CliError> {
    let mut link = workspace.pane_events_link(target)?;
    let initial = link.reconcile_target(target).map_err(link_error)?;
    let _initial_truncated = initial.truncated;
    if link.panes().map_err(link_error)?.layout_revision != revision {
        return Err(stale_target());
    }
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let mut cursor = initial.cursor;
    let initial_revision = initial.screen_revision;
    let mut screen_revision = initial.screen_revision;
    let mut quiet_since = Instant::now();
    if condition_matches(
        condition,
        text,
        regex,
        quiet,
        &initial,
        screen_revision,
        quiet_since,
    )? {
        return finish_wait(workspace, options, index, target, condition, true, false);
    }
    loop {
        let now = Instant::now();
        if now >= deadline {
            return finish_wait(workspace, options, index, target, condition, false, true);
        }
        let before = all_panes(workspace)?;
        let before_revision = before.layout_revision;
        let before_target = before
            .panes
            .iter()
            .flatten()
            .nth(index - 1)
            .map(|entry| entry.target);
        if before_revision != revision || before_target != Some(target) {
            return Err(stale_target());
        }
        let wait_ms = deadline
            .saturating_duration_since(now)
            .as_millis()
            .min(u128::from(POLL_WAIT_MS)) as u16;
        let (events, end) = link
            .watch_target(
                target,
                cursor,
                MAX_WATCH_EVENTS,
                wait_ms.min(MAX_WATCH_WAIT_MS),
            )
            .map_err(link_error)?;
        if end.status == WatchStatus::ResyncRequired {
            return Err(CliError::new(
                "resync_required",
                "pane events expired; re-run the wait against a fresh pane state",
            ));
        }
        cursor = end.cursor;
        for event in events {
            match event.body {
                EventBody::ScreenChanged { revision: observed } if observed != screen_revision => {
                    screen_revision = observed;
                    quiet_since = Instant::now();
                }
                EventBody::ChildExited if condition == "exit" => {
                    return finish_wait(workspace, options, index, target, condition, true, false);
                }
                _ => {}
            }
        }
        let current = link.reconcile_target(target).map_err(link_error)?;
        if current.info.target != target {
            return Err(stale_target());
        }
        if condition == "exit" && current.info.child_exited {
            return finish_wait(workspace, options, index, target, condition, true, false);
        }
        if current.screen_revision != screen_revision {
            screen_revision = current.screen_revision;
            quiet_since = Instant::now();
        }
        let after = all_panes(workspace)?;
        let after_target = after
            .panes
            .iter()
            .flatten()
            .nth(index - 1)
            .map(|entry| entry.target);
        if after.layout_revision != revision || after_target != Some(target) {
            return Err(stale_target());
        }
        if condition == "change" && current.screen_revision != initial_revision {
            return finish_wait(workspace, options, index, target, condition, true, false);
        }
        if condition_matches(
            condition,
            text,
            regex,
            quiet,
            &current,
            screen_revision,
            quiet_since,
        )? {
            return finish_wait(workspace, options, index, target, condition, true, false);
        }
    }
}

fn condition_matches(
    condition: &str,
    text: Option<&str>,
    regex: bool,
    quiet: Option<u64>,
    state: &AgentReconciliation,
    screen_revision: u64,
    quiet_since: Instant,
) -> Result<bool, CliError> {
    Ok(match condition {
        "text" => {
            let needle = text.ok_or_else(CliError::usage)?;
            if regex {
                regex_matches(needle, &state.text)?
            } else {
                state.text.contains(needle)
            }
        }
        "quiet" => {
            let _ = screen_revision;
            quiet_since.elapsed() >= Duration::from_secs(quiet.ok_or_else(CliError::usage)?)
        }
        "exit" => state.info.child_exited,
        "change" => false,
        _ => false,
    })
}

fn finish_wait(
    workspace: &Workspace,
    options: Options<'_>,
    index: usize,
    target: TargetRef,
    condition: &str,
    matched: bool,
    timed_out: bool,
) -> Result<i32, CliError> {
    if !matched {
        let message = if timed_out {
            "wait condition timed out"
        } else {
            "wait condition did not match"
        };
        if options.json {
            let mut json = String::from("{\"ok\":false,\"action\":\"wait\",\"matched\":false,\"status\":\"timed_out\",\"condition\":");
            push_json_string(&mut json, condition);
            let _ = write!(json, ",\"index\":{index},\"target\":");
            push_json_string(&mut json, &target_string(target));
            json.push('}');
            write_line(&json);
        } else {
            eprintln!("panelink: {message}");
        }
        return Ok(1);
    }
    let exit = condition == "exit";
    let exit_status = if exit {
        Some(observed_exit_status(workspace, target)?)
    } else {
        None
    };
    if options.json {
        let mut json = String::from("{\"ok\":true,\"action\":\"wait\",\"matched\":true,\"status\":\"matched\",\"condition\":");
        push_json_string(&mut json, condition);
        let _ = write!(json, ",\"index\":{index},\"target\":");
        push_json_string(&mut json, &target_string(target));
        if exit {
            json.push_str(",\"exit_status\":");
            push_exit_json(&mut json, exit_status.flatten());
            json.push_str(",\"message\":");
            push_json_string(&mut json, &exit_message(exit_status.flatten()));
        }
        json.push('}');
        write_line(&json);
    } else if exit {
        println!(
            "panelink: pane {index} {}",
            exit_message(exit_status.flatten())
        );
    } else {
        println!("panelink: wait condition matched on pane {index}");
    }
    Ok(0)
}

/// `None` means the exited pane disappeared before its retained PTY status
/// could be read. A still-present pane must have a confirmed status.
fn observed_exit_status(
    workspace: &Workspace,
    target: TargetRef,
) -> Result<Option<panelink::multi_runtime::PaneExit>, CliError> {
    let panes = all_panes(workspace)?;
    let Some(pane) = panes
        .panes
        .iter()
        .flatten()
        .find(|pane| pane.target == target)
    else {
        return Ok(None);
    };
    pane.exit.map(Some).ok_or_else(|| {
        CliError::new(
            "exit_status_unavailable",
            "pane exit was observed but its retained PTY status is not confirmed",
        )
    })
}

fn exit_message(exit: Option<panelink::multi_runtime::PaneExit>) -> String {
    match exit {
        Some(panelink::multi_runtime::PaneExit::Code(code)) => {
            format!("exited (code {code})")
        }
        Some(panelink::multi_runtime::PaneExit::Signal(signal)) => {
            format!("exited (signal {signal})")
        }
        None => "exited (status unavailable)".to_owned(),
    }
}

fn run_events(args: &[&str], options: Options<'_>) -> Result<i32, CliError> {
    if !options.json {
        return Err(CliError::new("json_required", "events requires --json"));
    }
    let mut pane = None;
    let mut timeout = None;
    let mut cursor = 0;
    while cursor < args.len() {
        match args[cursor] {
            "--pane" => {
                cursor += 1;
                pane = Some(parse_index(
                    args.get(cursor).copied().ok_or_else(CliError::usage)?,
                )?);
                cursor += 1;
            }
            "--timeout" => {
                cursor += 1;
                timeout = Some(parse_seconds(
                    args.get(cursor).copied().ok_or_else(CliError::usage)?,
                )?);
                cursor += 1;
            }
            "--json" => cursor += 1,
            "--as" => cursor += 2,
            value if value.starts_with("--as=") => cursor += 1,
            _ => return Err(CliError::usage()),
        }
    }
    let workspace = workspace(options.caller).or_else(|error| {
        if error.code == "no_workspace" {
            Ok(Workspace {
                state: resolved_state().map_err(|_| error)?,
                caller_dir: PathBuf::new(),
            })
        } else {
            Err(error)
        }
    })?;
    if !workspace.state.server.join("control.sock").exists() {
        return Ok(0);
    }
    let initial = all_panes(&workspace)?;
    let mut known = initial
        .panes
        .iter()
        .flatten()
        .enumerate()
        .map(|(index, entry)| (entry.target, index + 1, entry.live))
        .collect::<Vec<_>>();
    let mut all_known = known.clone();
    if let Some(index) = pane {
        if index > known.len() {
            return Err(CliError::new(
                "pane_not_found",
                "pane index is not present; run 'panelink pane list'",
            ));
        }
        let target = known[index - 1].0;
        known = vec![(target, index, known[index - 1].2)];
    }
    let mut cursors = Vec::new();
    for (target, index, live) in &known {
        if !live {
            continue;
        }
        let mut link = workspace.pane_events_link(*target)?;
        let state = link.reconcile_target(*target).map_err(link_error)?;
        cursors.push((*target, *index, state.cursor));
    }
    if timeout == Some(0) {
        return Err(CliError::usage());
    }
    let started = Instant::now();
    let deadline = timeout.map(|seconds| started + Duration::from_secs(seconds));
    let mut exited_targets = Vec::new();
    loop {
        let current = match all_panes(&workspace) {
            Ok(current) => current,
            Err(_) if !exited_targets.is_empty() => {
                for (index, target) in exited_targets {
                    emit_event(options, "pane_closed", index, target, None);
                }
                return Ok(0);
            }
            Err(_) if !workspace.state.server.join("control.sock").exists() => {
                for (target, index, live) in &known {
                    if *live {
                        emit_event(options, "pane_closed", *index, *target, None);
                    }
                }
                return Ok(0);
            }
            Err(error) => return Err(error),
        };
        let all_current_targets = current
            .panes
            .iter()
            .flatten()
            .enumerate()
            .map(|(index, entry)| (entry.target, index + 1, entry.live))
            .collect::<Vec<_>>();
        for (target, index, live) in &all_current_targets {
            let matches_filter = pane.is_none_or(|filter| filter == *index);
            if matches_filter && *live && !all_known.iter().any(|(old, _, _)| old == target) {
                emit_event(options, "pane_opened", *index, *target, None);
                let mut link = workspace.pane_events_link(*target)?;
                let state = link.reconcile_target(*target).map_err(link_error)?;
                cursors.push((*target, *index, state.cursor));
            }
        }
        for (target, index, was_live) in &all_known {
            if pane.is_none_or(|filter| filter == *index)
                && (*was_live || !all_current_targets.iter().any(|(now, _, _)| now == target))
                && !all_current_targets
                    .iter()
                    .any(|(now, _, live)| now == target && *live)
            {
                emit_event(options, "pane_closed", *index, *target, None);
            }
        }
        all_known.retain(|(target, _, _)| {
            all_current_targets
                .iter()
                .any(|(current, _, _)| current == target)
        });
        known = all_current_targets
            .into_iter()
            .filter(|(_, index, _)| pane.is_none_or(|filter| filter == *index))
            .collect();
        cursors.retain(|(target, _, _)| {
            known
                .iter()
                .any(|(current, _, live)| *live && current == target)
        });
        for (target, index, live) in &known {
            if *live && !cursors.iter().any(|(current, _, _)| current == target) {
                let mut link = workspace.pane_events_link(*target)?;
                let state = link.reconcile_target(*target).map_err(link_error)?;
                cursors.push((*target, *index, state.cursor));
            }
        }
        let mut did_watch = false;
        for (target, index, cursor_value) in &mut cursors {
            let mut link = match workspace.pane_events_link(*target) {
                Ok(link) => link,
                Err(_) => continue,
            };
            if let Ok((events, end)) =
                link.watch_target(*target, *cursor_value, MAX_WATCH_EVENTS, POLL_WAIT_MS)
            {
                did_watch = true;
                *cursor_value = end.cursor;
                for event in events {
                    if matches!(event.body, EventBody::ChildExited)
                        && !exited_targets
                            .iter()
                            .any(|(_, target)| *target == event.target)
                    {
                        exited_targets.push((*index, event.target));
                    }
                    emit_metadata_event(options, *index, event);
                }
            }
        }
        all_known = known.clone();
        if deadline.is_some_and(|end| Instant::now() >= end) {
            emit_event(
                options,
                "quiet",
                pane.unwrap_or(0),
                empty_target(),
                Some(started.elapsed().as_secs()),
            );
            return Ok(0);
        }
        if !did_watch {
            thread::sleep(Duration::from_millis(POLL_WAIT_MS.into()));
        }
    }
}

fn emit_metadata_event(options: Options<'_>, index: usize, event: Event) {
    let kind = match event.body {
        EventBody::ScreenChanged { .. } => "output",
        EventBody::InputOwnerChanged { .. } => "output",
        EventBody::ChildExited => "exited",
    };
    emit_event(options, kind, index, event.target, Some(event.cursor));
}

fn emit_event(
    options: Options<'_>,
    kind: &str,
    index: usize,
    target: TargetRef,
    cursor: Option<u64>,
) {
    if options.json {
        let mut json = String::from("{\"event\":");
        push_json_string(&mut json, kind);
        let _ = write!(json, ",\"index\":{index},\"target\":");
        if target.generation == 0 {
            json.push_str("null");
        } else {
            push_json_string(&mut json, &target_string(target));
        }
        if let Some(cursor) = cursor {
            let _ = write!(json, ",\"cursor\":{cursor}");
        }
        if kind == "quiet" {
            if let Some(duration) = cursor {
                let _ = write!(json, ",\"quiet_seconds\":{duration}");
            }
        }
        json.push('}');
        write_line(&json);
    } else {
        println!("{kind} pane {index}");
    }
}

fn empty_target() -> TargetRef {
    TargetRef {
        generation: 0,
        workspace: 0,
        pane: 0,
        session: 0,
        occupant_revision: 0,
    }
}

fn regex_matches(pattern: &str, text: &str) -> Result<bool, CliError> {
    if pattern.is_empty() || pattern.len() > 256 {
        return Err(CliError::new(
            "invalid_regex",
            "regex must contain 1 to 256 bytes",
        ));
    }
    let tokens = parse_regex(pattern)?;
    let (anchored_start, anchored_end, tokens) = tokens;
    let input = text.chars().collect::<Vec<_>>();
    let accept = tokens.len();
    let mut active = vec![false; accept + 1];
    active[0] = true;
    epsilon_closure(&tokens, &mut active);
    if active[accept] && (!anchored_end || input.is_empty()) {
        return Ok(true);
    }
    for (position, scalar) in input.iter().enumerate() {
        let mut next = vec![false; accept + 1];
        for (index, token) in tokens.iter().enumerate() {
            if !active[index] || !token.atom.matches(*scalar) {
                continue;
            }
            match token.quantifier {
                Quantifier::ZeroOrMore | Quantifier::OneOrMore => {
                    next[index] = true;
                    next[index + 1] = true;
                }
                Quantifier::One | Quantifier::ZeroOrOne => next[index + 1] = true,
            }
        }
        if !anchored_start {
            next[0] = true;
        }
        epsilon_closure(&tokens, &mut next);
        active = next;
        if active[accept] && (!anchored_end || position + 1 == input.len()) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Quantifier {
    One,
    ZeroOrMore,
    OneOrMore,
    ZeroOrOne,
}

enum RegexAtom {
    Any,
    Literal(char),
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
}

impl RegexAtom {
    fn matches(&self, scalar: char) -> bool {
        match self {
            Self::Any => true,
            Self::Literal(expected) => *expected == scalar,
            Self::Class { negated, ranges } => {
                ranges
                    .iter()
                    .any(|(low, high)| (*low..=*high).contains(&scalar))
                    != *negated
            }
        }
    }
}

struct RegexToken {
    atom: RegexAtom,
    quantifier: Quantifier,
}

/// Parse the bounded regex subset: literals, `\` escapes, `.`, `[...]`/`[^...]`
/// classes with ranges, `*`/`+`/`?` quantifiers and `^`/`$` anchors.
fn parse_regex(pattern: &str) -> Result<(bool, bool, Vec<RegexToken>), CliError> {
    let invalid = |message| CliError::new("invalid_regex", message);
    let mut chars = pattern.chars().peekable();
    let anchored_start = chars.peek() == Some(&'^');
    if anchored_start {
        chars.next();
    }
    let mut tokens = Vec::new();
    let mut anchored_end = false;
    while let Some(scalar) = chars.next() {
        let atom = match scalar {
            '$' if chars.peek().is_none() => {
                anchored_end = true;
                break;
            }
            '\\' => RegexAtom::Literal(
                chars
                    .next()
                    .ok_or_else(|| invalid("regex ends with an escape"))?,
            ),
            '.' => RegexAtom::Any,
            '*' | '+' | '?' => return Err(invalid("regex quantifier has no preceding atom")),
            '[' => {
                let negated = chars.peek() == Some(&'^');
                if negated {
                    chars.next();
                }
                let mut ranges = Vec::new();
                loop {
                    let low = match chars.next() {
                        None => return Err(invalid("regex class is not closed")),
                        Some(']') if !ranges.is_empty() => break,
                        Some('\\') => chars
                            .next()
                            .ok_or_else(|| invalid("regex ends with an escape"))?,
                        Some(low) => low,
                    };
                    let mut high = low;
                    if chars.peek() == Some(&'-') {
                        chars.next();
                        match chars.next() {
                            None => return Err(invalid("regex class is not closed")),
                            Some(']') => {
                                ranges.push((low, low));
                                ranges.push(('-', '-'));
                                break;
                            }
                            Some('\\') => {
                                high = chars
                                    .next()
                                    .ok_or_else(|| invalid("regex ends with an escape"))?;
                            }
                            Some(value) => high = value,
                        }
                        if high < low {
                            return Err(invalid("regex class range is reversed"));
                        }
                    }
                    ranges.push((low, high));
                    if ranges.len() > 64 {
                        return Err(invalid("regex class exceeds the supported size"));
                    }
                }
                RegexAtom::Class { negated, ranges }
            }
            literal => RegexAtom::Literal(literal),
        };
        let quantifier = match chars.peek() {
            Some('*') => Quantifier::ZeroOrMore,
            Some('+') => Quantifier::OneOrMore,
            Some('?') => Quantifier::ZeroOrOne,
            _ => Quantifier::One,
        };
        if quantifier != Quantifier::One {
            chars.next();
        }
        tokens.push(RegexToken { atom, quantifier });
        if tokens.len() > 256 {
            return Err(invalid("regex exceeds the supported size"));
        }
    }
    Ok((anchored_start, anchored_end, tokens))
}

fn epsilon_closure(tokens: &[RegexToken], states: &mut [bool]) {
    for index in 0..tokens.len() {
        if states[index]
            && matches!(
                tokens[index].quantifier,
                Quantifier::ZeroOrMore | Quantifier::ZeroOrOne
            )
        {
            states[index + 1] = true;
        }
    }
}

fn parse_seconds(value: &str) -> Result<u64, CliError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CliError::usage());
    }
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value <= MAX_WAIT_SECONDS)
        .ok_or_else(CliError::usage)
}

fn target_string(target: TargetRef) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        target.generation, target.workspace, target.pane, target.session, target.occupant_revision
    )
}

fn status_name(status: OperationStatus) -> &'static str {
    match status {
        OperationStatus::NotAdmitted => "not_admitted",
        OperationStatus::InProgress => "in_progress",
        OperationStatus::Completed(Outcome::Accepted) => "accepted",
        OperationStatus::Completed(Outcome::Applied) => "applied",
        OperationStatus::Completed(Outcome::Failed) => "failed",
        OperationStatus::Completed(Outcome::Partial) => "partial",
        OperationStatus::Completed(Outcome::Unknown) => "unknown",
    }
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
            scalar if scalar.is_control() => {
                let _ = write!(json, "\\u{:04x}", scalar as u32);
            }
            scalar => json.push(scalar),
        }
    }
    json.push('"');
}

fn write_line(value: &str) {
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "{value}");
}

fn print_help() {
    println!("Agent pane commands:\n  panelink pane list [--as NAME] [--json]\n  panelink pane read N [--lines K] [--scrollback] [--json]\n  panelink pane claim|release N [--json]\n  panelink pane send N TEXT [--enter] [--claim] [--json]\n  panelink pane keys N Enter|Tab|Esc|C-c|Up|Down|Left|Right|Backspace... [--json]\n  panelink pane interrupt|close N [--json]\n  panelink wait N (--text STRING [--regex] | --quiet SECS | --exit | --change) --timeout SECS [--json]\n  panelink events [--pane N] [--timeout SECS] --json");
}

#[cfg(test)]
mod parser_tests {
    use super::*;

    #[test]
    fn common_dispatch_options_and_regex_limits_are_explicit() {
        assert_eq!(parse_seconds("0").unwrap(), 0);
        assert_eq!(parse_index("0").unwrap_err().code, "invalid_arguments");
        assert_eq!(parse_key("C-c").unwrap(), Key::Interrupt);
        assert_eq!(parse_key("PageUp").unwrap_err().code, "invalid_arguments");
        assert!(regex_matches("^AGENT_[A-Z]+$", "AGENT_READY").unwrap());
        assert!(regex_matches("A.*Z", "prefix A-middle-Z suffix").unwrap());
        assert!(!regex_matches("^A?Z$", "prefix AZ").unwrap());
        assert!(regex_matches("^A?Z$", "Z").unwrap());
        assert!(regex_matches("A\\+B", "A+B").unwrap());
        assert!(regex_matches("^ab+c$", "abbbc").unwrap());
        assert!(!regex_matches("^ab+c$", "ac").unwrap());
        assert!(regex_matches("a\\.b", "a.b").unwrap());
        assert!(!regex_matches("a\\.b", "axb").unwrap());
        assert!(regex_matches("[^0-9]$", "x1y").unwrap());
        assert!(!regex_matches("^[^0-9]+$", "x1y").unwrap());
        assert!(regex_matches("ready", "").is_ok_and(|hit| !hit));
        assert!(regex_matches("x*", "").unwrap());
        assert!(regex_matches("^$", "").unwrap());
        assert!(!regex_matches("^$", "a").unwrap());
        assert_eq!(
            regex_matches("[a-", "text").unwrap_err().code,
            "invalid_regex"
        );
        assert_eq!(
            regex_matches("[z-a]", "text").unwrap_err().code,
            "invalid_regex"
        );
        assert_eq!(
            regex_matches("*bad", "text").unwrap_err().code,
            "invalid_regex"
        );
    }
}
