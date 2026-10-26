//! Managed child launch from the agent CLI.
//!
//! Parsing stays on raw OS arguments until the `--` delimiter so command
//! names, arguments, and working-directory paths can contain non-UTF8 bytes.

use super::{
    ensure_current_index, link_error, parse_index, push_json_string, resolve_index, target_name,
    target_string, workspace, write_line, CliError, Workspace, JSON_ERRORS,
};
use crate::p3_cli::{RunPaneFailure, RunPaneObservation, WorkspaceLink};
use panelink::control_protocol::TargetRef;
use panelink::pane_layout::SplitDirection;
use panelink::protocol::{OperationKey, Outcome};
use panelink::split_protocol::SplitResult;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as FmtWrite;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

const MAX_ENTRYPOINT_TOKENS: usize = 80;
const MAX_ARGV_BYTES: usize = 65_536;
const MAX_PANES: usize = 4;

#[derive(Debug)]
struct ParsedRun {
    pane_index: usize,
    direction: SplitDirection,
    cwd: Option<PathBuf>,
    caller: String,
    json: bool,
    executable: OsString,
    arguments: Vec<OsString>,
}

/// Run `panelink pane run ...`; `args` includes the `pane run` words.
pub(super) fn run(args: Vec<OsString>) -> Result<i32, CliError> {
    // Set JSON error mode before syntax validation when the flag occurs in the
    // option section. Tokens after `--` are opaque child arguments.
    let json = json_option_before_delimiter(&args);
    JSON_ERRORS.with(|enabled| enabled.set(json));

    let mut parsed = parse_args(args)?;
    parsed.cwd = resolve_cwd(parsed.cwd)?;

    // All syntax, token-count, and byte-count checks complete before opening
    // caller state or any reusable operation context.
    let workspace = workspace(&parsed.caller)?;
    let (target, layout_revision, _panes) = resolve_index(&workspace, parsed.pane_index)?;
    ensure_current_index(&workspace, parsed.pane_index, target, layout_revision)?;
    let mut link = run_link(&workspace, target)?;
    let result: Result<SplitResult, RunPaneFailure> = link.run_pane(
        target,
        layout_revision,
        parsed.direction,
        parsed.cwd,
        &parsed.executable,
        &parsed.arguments,
    );
    let result = match result {
        Ok(result) => result,
        Err(failure) => match failure.operation {
            None => return Err(run_error(failure.code)),
            Some(key) => {
                return Ok(emit_operation_failure(
                    parsed.json,
                    target,
                    key,
                    failure.code,
                    failure.observed,
                ));
            }
        },
    };

    Ok(emit_result(parsed.json, result))
}

fn json_option_before_delimiter(args: &[OsString]) -> bool {
    for arg in args.iter().skip(2) {
        if arg == OsStr::new("--") {
            return false;
        }
        if arg == OsStr::new("--json") {
            return true;
        }
    }
    false
}

fn parse_args(args: Vec<OsString>) -> Result<ParsedRun, CliError> {
    if args.len() < 2
        || args[0] != OsStr::new("pane")
        || args[1] != OsStr::new("run")
        || args.len() > MAX_ENTRYPOINT_TOKENS
    {
        return Err(CliError::usage());
    }

    let mut pane_index = 1;
    let mut direction = SplitDirection::Right;
    let mut cwd: Option<PathBuf> = None;
    let mut caller: Option<String> = None;
    let mut json = false;
    let mut seen_pane = false;
    let mut seen_direction = false;
    let mut seen_cwd = false;
    let mut seen_as = false;
    let mut seen_json = false;
    let mut cursor = 2;

    while cursor < args.len() && args[cursor] != OsStr::new("--") {
        let option = args[cursor].to_str().ok_or_else(CliError::usage)?;
        match option {
            "--pane" => {
                if seen_pane {
                    return Err(CliError::usage());
                }
                seen_pane = true;
                let value = args.get(cursor + 1).ok_or_else(CliError::usage)?;
                let value = value.to_str().ok_or_else(CliError::usage)?;
                pane_index = parse_index(value)?;
                if pane_index > MAX_PANES {
                    return Err(CliError::usage());
                }
                cursor += 2;
            }
            "--direction" => {
                if seen_direction {
                    return Err(CliError::usage());
                }
                seen_direction = true;
                let value = args.get(cursor + 1).ok_or_else(CliError::usage)?;
                direction = match value.to_str().ok_or_else(CliError::usage)? {
                    "left" => SplitDirection::Left,
                    "right" => SplitDirection::Right,
                    "up" => SplitDirection::Up,
                    "down" => SplitDirection::Down,
                    _ => return Err(CliError::usage()),
                };
                cursor += 2;
            }
            "--cwd" => {
                if seen_cwd {
                    return Err(CliError::usage());
                }
                seen_cwd = true;
                let value = args.get(cursor + 1).ok_or_else(CliError::usage)?;
                if value.is_empty() {
                    return Err(CliError::usage());
                }
                cwd = Some(PathBuf::from(value));
                cursor += 2;
            }
            "--as" => {
                if seen_as {
                    return Err(CliError::usage());
                }
                seen_as = true;
                let value = args.get(cursor + 1).ok_or_else(CliError::usage)?;
                caller = Some(value.to_str().ok_or_else(CliError::usage)?.to_owned());
                cursor += 2;
            }
            "--json" => {
                if seen_json {
                    return Err(CliError::usage());
                }
                seen_json = true;
                json = true;
                cursor += 1;
            }
            _ => return Err(CliError::usage()),
        }
    }

    if cursor >= args.len() || args[cursor] != OsStr::new("--") {
        return Err(CliError::usage());
    }
    let command_start = cursor + 1;
    let executable = args.get(command_start).ok_or_else(CliError::usage)?;
    if executable.is_empty() {
        return Err(CliError::usage());
    }

    let argv_bytes = args.iter().try_fold(0usize, |total, value| {
        total.checked_add(value.as_bytes().len())
    });
    if argv_bytes.is_none_or(|size| size > MAX_ARGV_BYTES) {
        return Err(CliError::usage());
    }

    let caller = caller
        .or_else(|| std::env::var("PANELINK_AGENT").ok())
        .unwrap_or_else(|| "agent".to_owned());
    if !valid_caller(&caller) {
        return Err(CliError::usage());
    }

    Ok(ParsedRun {
        pane_index,
        direction,
        cwd,
        caller,
        json,
        executable: executable.clone(),
        arguments: args[command_start + 1..].to_vec(),
    })
}

fn valid_caller(caller: &str) -> bool {
    !caller.is_empty()
        && caller.len() <= 64
        && caller
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn resolve_cwd(cwd: Option<PathBuf>) -> Result<Option<PathBuf>, CliError> {
    let Some(cwd) = cwd else {
        return Ok(None);
    };
    if cwd.is_absolute() {
        return Ok(Some(cwd));
    }
    let invocation_dir = std::env::current_dir().map_err(|_| {
        CliError::new(
            "cwd_unavailable",
            "could not resolve the invoking directory",
        )
    })?;
    Ok(Some(invocation_dir.join(cwd)))
}

fn run_link(workspace: &Workspace, target: TargetRef) -> Result<WorkspaceLink, CliError> {
    let path = workspace
        .caller_dir
        .join(format!("{}-run.ctx", target_name(target)));
    WorkspaceLink::open_for_target(&workspace.state.server, &path, target).map_err(link_error)
}

fn run_error(error: &'static str) -> CliError {
    match error {
        "previous_operation_settled" => CliError::new(
            "previous_operation_settled",
            "a prior pane launch was settled; this invocation did not issue a new launch",
        ),
        "previous_operation_unobservable" => CliError::new(
            "outcome_unknown",
            "a prior pane launch could not be observed; this invocation issued no new launch",
        ),
        "operation_uncertain" | "transport_failed" | "response_invalid" => CliError::new(
            "outcome_unknown",
            "the pane launch may have completed; do not repeat it blindly",
        ),
        "operation_refused" => CliError::new(
            "operation_refused",
            "the pane launch was refused before it was admitted",
        ),
        "input_capability_unavailable" | "input_not_authorized" => CliError::new(
            "input_unavailable",
            "pane launch is not authorized for this workspace connection",
        ),
        "split_unavailable" => CliError::new(
            "split_unavailable",
            "pane launch is unavailable for the current layout or capacity",
        ),
        "stale_layout" | "stale_target" => CliError::new(
            "stale_target",
            "stale pane target or layout; no replacement pane was selected",
        ),
        "launch_context_invalid" => CliError::new(
            "launch_context_invalid",
            "could not prepare the invoking process launch context",
        ),
        "request_invalid" => CliError::new(
            "invalid_request",
            "pane launch request exceeds supported limits",
        ),
        "context_lock_timeout" => CliError::new(
            "context_lock_timeout",
            "could not acquire the pane launch context; no new launch was issued",
        ),
        "context_key_mismatch" | "context_sequence_exhausted" => CliError::new(
            error,
            "could not reserve a safe pane launch operation; no new launch was issued",
        ),
        "context_unsafe" | "context_mismatch" => {
            CliError::new(error, "pane launch context failed its safety check")
        }
        "context_storage_failed" | "context_invalid" => CliError::new(
            "outcome_unknown",
            "pane launch context could not be safely acknowledged; do not repeat it blindly",
        ),
        _ => link_error(error),
    }
}

fn emit_result(json: bool, result: SplitResult) -> i32 {
    let code = i32::from(result.outcome != Outcome::Applied);
    if json {
        let mut output = String::from("{\"ok\":");
        output.push_str(if code == 0 { "true" } else { "false" });
        output.push_str(",\"action\":\"pane_run\",\"outcome\":");
        push_json_string(&mut output, outcome_name(result.outcome));
        let _ = write!(
            output,
            ",\"operation\":\"{}:{}:{}\",\"existing_target\":",
            result.key.generation, result.key.lease, result.key.sequence
        );
        push_json_string(&mut output, &target_string(result.existing_target));
        output.push_str(",\"target\":");
        match result.new_target {
            Some(target) => push_json_string(&mut output, &target_string(target)),
            None => output.push_str("null"),
        }
        let _ = write!(
            output,
            ",\"layout_revision\":{}}}",
            result.committed_layout_revision
        );
        write_line(&output);
    } else {
        let label = outcome_name(result.outcome);
        if let Some(target) = result.new_target {
            println!("pane run {label}: {}", target_string(target));
        } else {
            println!("pane run {label}");
        }
    }
    code
}

fn operation_failure_json(
    target: TargetRef,
    key: OperationKey,
    code: &'static str,
    observed: Option<RunPaneObservation>,
) -> String {
    let error = run_error(code);
    let outcome = if code == "operation_refused" {
        "failed"
    } else {
        "unknown"
    };
    let mut output = String::from("{\"ok\":false,\"action\":\"pane_run\",\"outcome\":");
    push_json_string(&mut output, outcome);
    let _ = write!(
        output,
        ",\"operation\":\"{}:{}:{}\",\"existing_target\":",
        key.generation, key.lease, key.sequence
    );
    push_json_string(&mut output, &target_string(target));
    output.push_str(",\"target\":");
    match observed.and_then(|result| result.new_target) {
        Some(target) => push_json_string(&mut output, &target_string(target)),
        None => output.push_str("null"),
    }
    let revision = observed.map_or(0, |result| result.committed_layout_revision);
    let _ = write!(output, ",\"layout_revision\":{revision},\"error\":");
    push_json_string(&mut output, error.code);
    output.push_str(",\"message\":");
    push_json_string(&mut output, error.message);
    output.push('}');
    output
}

fn emit_operation_failure(
    json: bool,
    target: TargetRef,
    key: OperationKey,
    code: &'static str,
    observed: Option<RunPaneObservation>,
) -> i32 {
    if json {
        write_line(&operation_failure_json(target, key, code, observed));
    } else {
        let error = run_error(code);
        let outcome = if code == "operation_refused" {
            "failed"
        } else {
            "unknown"
        };
        if let Some(child) = observed.and_then(|result| result.new_target) {
            println!("observed pane target: {}", target_string(child));
        }
        println!(
            "pane run {outcome} for operation {}:{}:{}: {}",
            key.generation, key.lease, key.sequence, error.message
        );
    }
    1
}

fn outcome_name(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Accepted => "unknown",
        Outcome::Applied => "applied",
        Outcome::Failed => "failed",
        Outcome::Partial => "partial",
        Outcome::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    fn os(value: &str) -> OsString {
        OsString::from(value)
    }

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(|word| os(word)).collect()
    }

    #[test]
    fn defaults_and_literal_command_arguments_follow_delimiter() {
        let parsed = parse_args(args(&[
            "pane", "run", "--", "printf", "--as", "literal", "--json", "--cwd", "x",
        ]))
        .unwrap();
        assert_eq!(parsed.pane_index, 1);
        assert_eq!(parsed.direction, SplitDirection::Right);
        assert!(!parsed.json);
        assert_eq!(parsed.executable, os("printf"));
        assert_eq!(
            parsed.arguments,
            args(&["--as", "literal", "--json", "--cwd", "x"])
        );
    }

    #[test]
    fn parses_options_before_delimiter_and_caller_name() {
        let parsed = parse_args(args(&[
            "pane",
            "run",
            "--pane",
            "4",
            "--direction",
            "up",
            "--cwd",
            "../tmp",
            "--as",
            "worker_1",
            "--json",
            "--",
            "tool",
            "arg",
        ]))
        .unwrap();
        assert_eq!(parsed.pane_index, 4);
        assert_eq!(parsed.direction, SplitDirection::Up);
        assert_eq!(parsed.cwd, Some(PathBuf::from("../tmp")));
        assert_eq!(parsed.caller, "worker_1");
        assert!(parsed.json);
        assert_eq!(parsed.arguments, args(&["arg"]));
    }

    #[test]
    fn rejects_duplicate_unknown_invalid_and_missing_command_syntax() {
        for words in [
            vec!["pane", "run", "--json", "--json", "--", "x"],
            vec!["pane", "run", "--pane", "1", "--pane", "2", "--", "x"],
            vec!["pane", "run", "--direction", "diagonal", "--", "x"],
            vec!["pane", "run", "--pane", "5", "--", "x"],
            vec!["pane", "run", "--unknown", "x", "--", "x"],
            vec!["pane", "run", "--", ""],
            vec!["pane", "run", "--"],
            vec!["pane", "run", "x"],
        ] {
            assert_eq!(
                parse_args(args(&words)).unwrap_err().code,
                "invalid_arguments"
            );
        }
    }

    #[test]
    fn preserves_non_utf8_paths_and_command_bytes() {
        let raw = |bytes: &[u8]| OsString::from_vec(bytes.to_vec());
        let parsed = parse_args(vec![
            os("pane"),
            os("run"),
            os("--cwd"),
            raw(b"/tmp/cwd-\xff"),
            os("--"),
            raw(b"/tmp/exe-\xfe"),
            raw(b"arg-\xfd"),
        ])
        .unwrap();
        assert_eq!(parsed.cwd.unwrap().as_os_str().as_bytes(), b"/tmp/cwd-\xff");
        assert_eq!(parsed.executable.as_bytes(), b"/tmp/exe-\xfe");
        assert_eq!(parsed.arguments[0].as_bytes(), b"arg-\xfd");
    }

    #[test]
    fn enforces_token_and_aggregate_byte_bounds() {
        let mut too_many = vec![os("pane"), os("run"), os("--")];
        too_many.push(os("cmd"));
        too_many.extend((0..77).map(|_| os("x")));
        assert_eq!(too_many.len(), MAX_ENTRYPOINT_TOKENS + 1);
        assert_eq!(parse_args(too_many).unwrap_err().code, "invalid_arguments");

        let too_large = vec![
            os("pane"),
            os("run"),
            os("--"),
            OsString::from_vec(vec![b'x'; MAX_ARGV_BYTES]),
        ];
        assert_eq!(parse_args(too_large).unwrap_err().code, "invalid_arguments");
    }

    #[test]
    fn json_mode_ignores_literal_command_flags_after_delimiter() {
        assert!(!json_option_before_delimiter(&args(&[
            "pane", "run", "--", "cmd", "--json",
        ])));
        assert!(json_option_before_delimiter(&args(&[
            "pane", "run", "--json", "--", "cmd",
        ])));
    }

    #[test]
    fn reserved_failure_json_keeps_key_anchor_and_static_error_only() {
        let anchor = TargetRef {
            generation: 11,
            workspace: 22,
            pane: 33,
            session: 44,
            occupant_revision: 55,
        };
        let key = OperationKey {
            generation: 11,
            lease: 66,
            sequence: 77,
        };
        let output = operation_failure_json(anchor, key, "transport_failed", None);
        assert!(output.contains("\"action\":\"pane_run\""));
        assert!(output.contains("\"outcome\":\"unknown\""));
        assert!(output.contains("\"operation\":\"11:66:77\""));
        assert!(output.contains("\"existing_target\":\"11:22:33:44:55\""));
        assert!(output.contains("\"target\":null,\"layout_revision\":0"));
        assert!(output.contains("\"error\":\"outcome_unknown\""));
        assert!(!output.contains("secret-command"));
        assert!(!output.contains("secret-argument"));
        assert!(!output.contains("/private/secret-cwd"));
        assert!(!output.contains("SECRET_ENV=value"));
    }

    #[test]
    fn explicit_not_admitted_failure_is_failed_not_unknown() {
        let anchor = TargetRef {
            generation: 1,
            workspace: 2,
            pane: 3,
            session: 4,
            occupant_revision: 5,
        };
        let key = OperationKey {
            generation: 1,
            lease: 6,
            sequence: 7,
        };
        let output = operation_failure_json(anchor, key, "operation_refused", None);
        assert!(output.contains("\"outcome\":\"failed\""));
        assert!(output.contains("\"error\":\"operation_refused\""));
    }
    #[test]
    fn ack_failure_json_preserves_the_observed_target_and_revision() {
        let anchor = TargetRef {
            generation: 1,
            workspace: 2,
            pane: 3,
            session: 4,
            occupant_revision: 5,
        };
        let child = TargetRef {
            pane: 6,
            session: 7,
            ..anchor
        };
        let key = OperationKey {
            generation: 1,
            lease: 8,
            sequence: 9,
        };
        let observed = SplitResult {
            key,
            outcome: Outcome::Applied,
            committed_layout_revision: 10,
            existing_target: anchor,
            new_target: Some(child),
        };
        let output = operation_failure_json(anchor, key, "context_unsafe", Some(observed.into()));
        assert!(output.contains("\"outcome\":\"unknown\""));
        assert!(output.contains("\"target\":\"1:2:6:7:5\""));
        assert!(output.contains("\"layout_revision\":10"));
        assert!(!output.contains("\"ok\":true"));
    }
}
