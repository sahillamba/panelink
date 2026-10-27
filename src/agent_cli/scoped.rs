//! One-shot pane submissions with a durable, target-pinned ownership scope.

use super::{
    ensure_current_index, parse_index, push_json_string, resolve_index, stale_target,
    target_string, write_line, CliError, Options, Workspace,
};
use crate::p3_cli::WorkspaceLink;
use panelink::agent_protocol::Observation;
use panelink::control_protocol::{TargetRef, MAX_INPUT_TEXT_BYTES};
use panelink::protocol::{OperationKey, OperationStatus, Outcome};
use std::fmt::Write as FmtWrite;

thread_local! {
    static DETAILS: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}
fn record(
    link: &WorkspaceLink,
    stage: &'static str,
    cause: Option<&'static str>,
    owner: Option<panelink::session_protocol::SessionInfo>,
) {
    let mut details = String::new();
    details.push_str(",\"stage\":");
    push_json_string(&mut details, stage);
    details.push_str(",\"cause\":");
    if let Some(cause) = cause {
        push_json_string(&mut details, cause);
    } else {
        details.push_str("null");
    }
    details.push_str(",\"transport_stage\":");
    push_json_string(&mut details, crate::p3_cli::control_diagnostic().transport);
    details.push_str(",\"pending_operation_key\":");
    push_key(&mut details, link.pending_operation_key());
    details.push_str(",\"reconciliation\":");
    push_json_string(
        &mut details,
        if link.pending_operation_key().is_some() {
            "pending"
        } else if link.discarded_uncertain {
            "unobservable"
        } else if link.settled_uncertain.is_some() {
            "settled_previous"
        } else {
            "clear"
        },
    );
    details.push_str(",\"current_ownership\":\"unverified\"");
    if let Some(info) = owner {
        details.push_str(",\"owner_observation\":\"before_operation\",\"input_owner_info\":");
        super::push_owner_info_json(
            &mut details,
            super::PaneOwner {
                info,
                caller_lease: link.lease(),
            }
            .structured(),
        );
    }
    DETAILS.with(|value| *value.borrow_mut() = details);
}

pub(super) fn command(
    workspace: &Workspace,
    args: &[&str],
    options: Options<'_>,
) -> Result<i32, CliError> {
    let index = parse_index(args.first().copied().ok_or_else(CliError::usage)?)?;
    let mut words = Vec::new();
    let mut cursor = 1;
    while cursor < args.len() {
        match args[cursor] {
            "--json" => cursor += 1,
            "--as" => {
                if args.get(cursor + 1).is_none() {
                    return Err(CliError::usage());
                }
                cursor += 2;
            }
            value if value.starts_with("--as=") => cursor += 1,
            value if value.starts_with('-') => return Err(CliError::usage()),
            value => {
                words.push(value);
                cursor += 1;
            }
        }
    }
    if words.is_empty() {
        return Err(CliError::usage());
    }
    let text = words.join(" ");
    if text.len() > MAX_INPUT_TEXT_BYTES || text.chars().any(char::is_control) {
        return Err(CliError::usage());
    }
    submit(workspace, index, options, &text, None)
}

/// Submit complete text plus Enter after a separately keyed claim, then
/// release only after the PTY write is confirmed Applied.
pub(super) fn submit(
    workspace: &Workspace,
    index: usize,
    options: Options<'_>,
    text: &str,
    expected_agent: Option<&Observation>,
) -> Result<i32, CliError> {
    if text.is_empty() || text.len() > MAX_INPUT_TEXT_BYTES || text.chars().any(char::is_control) {
        return Err(CliError::usage());
    }
    DETAILS.with(|value| value.borrow_mut().clear());
    let (numbered_target, layout_revision, _) = resolve_index(workspace, index)?;
    if expected_agent.is_some_and(|observation| observation.target != numbered_target) {
        return Err(stale_target());
    }
    let target = expected_agent.map_or(numbered_target, |observation| observation.target);
    ensure_current_index(workspace, index, target, layout_revision)?;
    let mut caller_link = workspace.pane_link(target)?;
    record(&caller_link, "preflight", None, None);
    if emit_blocked(&caller_link, options, index, target) {
        return Ok(1);
    }
    let caller_before = caller_link.describe_info().map_err(super::link_error)?;
    if caller_before.target != target {
        return Err(stale_target());
    }
    if caller_before.owner_lease == caller_link.lease() {
        return Err(CliError::new(
            "input_already_claimed",
            "this caller already holds a retained pane claim; use pane send/claim/release explicitly before scoped submit",
        ));
    }
    let mut link = if expected_agent.is_some() {
        workspace.pane_agent_link(target)?
    } else {
        caller_link
    };
    record(&link, "preflight", None, None);
    if emit_blocked(&link, options, index, target) {
        return Ok(1);
    }

    let before = link.describe_info().map_err(super::link_error)?;
    if before.target != target {
        return Err(stale_target());
    }
    if before.owner_lease == link.lease() {
        return Err(CliError::new(
            "input_already_claimed",
            "this caller already holds a retained pane claim; use pane send/claim/release explicitly before scoped submit",
        ));
    }

    let prior_claim_key = link.last_operation_key();
    let claim_result = link.change_owner(target, before.owner_revision, true);
    record(
        &link,
        "claim",
        claim_result.as_ref().err().copied(),
        Some(before),
    );
    let claim_key = link
        .last_operation_key()
        .filter(|key| Some(*key) != prior_claim_key);
    let claim_status = match claim_key
        .and_then(|_| link.last_operation_status())
        .or_else(|| claim_result.ok())
    {
        Some(status) => status,
        None => {
            emit(
                options,
                index,
                target,
                claim_key,
                claim_key,
                None,
                None,
                "not_written",
                "not_attempted",
                None,
                false,
            );
            return Ok(1);
        }
    };
    if claim_status != OperationStatus::Completed(Outcome::Applied) {
        emit(
            options,
            index,
            target,
            claim_key,
            claim_key,
            None,
            Some(claim_status),
            "not_written",
            "not_attempted",
            None,
            false,
        );
        return Ok(1);
    }

    let after_claim = match link.describe_info() {
        Ok(info) => info,
        Err(error) => {
            record(&link, "owner_query_after_claim", Some(error), Some(before));
            emit(
                options,
                index,
                target,
                claim_key,
                claim_key,
                None,
                Some(claim_status),
                "not_written",
                "not_attempted",
                None,
                false,
            );
            return Ok(1);
        }
    };
    if after_claim.target != target || after_claim.owner_lease != link.lease() {
        emit(
            options,
            index,
            target,
            claim_key,
            claim_key,
            None,
            Some(claim_status),
            "not_written",
            "not_attempted",
            None,
            false,
        );
        return Ok(1);
    }
    if ensure_current_index(workspace, index, target, layout_revision).is_err() {
        // No input key has been reserved. Release the exact original target's
        // scoped claim even though the displayed index no longer selects it.
        let prior = link.last_operation_key();
        let release_result = link.change_owner(target, after_claim.owner_revision, false);
        record(
            &link,
            "cleanup_unused_claim",
            release_result.as_ref().err().copied(),
            Some(after_claim),
        );
        let release_key = link.last_operation_key().filter(|key| Some(*key) != prior);
        let release_status = release_key
            .and_then(|_| link.last_operation_status())
            .or_else(|| release_result.ok());
        let released = release_status == Some(OperationStatus::Completed(Outcome::Applied));
        emit(
            options,
            index,
            target,
            None,
            claim_key,
            release_key,
            Some(OperationStatus::NotAdmitted),
            "not_written",
            if released { "confirmed" } else { "unconfirmed" },
            release_status,
            false,
        );
        return Ok(1);
    }

    let prior_key = link.last_operation_key();
    let submit_result = if let Some(observation) = expected_agent {
        link.guarded_submit(target, after_claim.owner_revision, observation, text)
    } else {
        link.send_text(target, after_claim.owner_revision, text, true)
    };
    record(
        &link,
        "submit",
        submit_result.as_ref().err().copied(),
        Some(after_claim),
    );
    let current_key = link.last_operation_key();
    let submit_key = current_key.filter(|key| Some(*key) != prior_key);
    let submit_status = submit_key
        .and_then(|_| link.last_operation_status())
        .or_else(|| submit_result.ok());
    if submit_status != Some(OperationStatus::Completed(Outcome::Applied)) {
        // A local preflight failure may happen before a submit key is reserved
        // (for example, a context-lane uncertainty guard). In that case no
        // input was attempted; do not label the result as possibly delivered.
        let delivery = if submit_key.is_some() {
            effect_name(submit_status)
        } else {
            "not_written"
        };
        emit(
            options,
            index,
            target,
            submit_key,
            claim_key,
            None,
            submit_status,
            delivery,
            "not_attempted",
            None,
            false,
        );
        return Ok(1);
    }

    // The complete submission is known Applied. Keep its original key in the
    // result even though the subsequent release reserves another key.
    let prior_release_key = link.last_operation_key();
    let release_result = link.change_owner(target, after_claim.owner_revision, false);
    record(
        &link,
        "release",
        release_result.as_ref().err().copied(),
        Some(after_claim),
    );
    let release_key = link
        .last_operation_key()
        .filter(|key| Some(*key) != prior_release_key);
    let release_status = release_key
        .and_then(|_| link.last_operation_status())
        .or_else(|| release_result.ok());
    let released = release_status == Some(OperationStatus::Completed(Outcome::Applied));
    emit(
        options,
        index,
        target,
        submit_key,
        claim_key,
        release_key,
        Some(OperationStatus::Completed(Outcome::Applied)),
        "pty_written",
        if released { "confirmed" } else { "unconfirmed" },
        release_status,
        released,
    );
    Ok(if released { 0 } else { 1 })
}

fn emit_blocked(
    link: &WorkspaceLink,
    options: Options<'_>,
    index: usize,
    target: TargetRef,
) -> bool {
    if link.discarded_uncertain {
        emit(
            options,
            index,
            target,
            link.last_operation_key(),
            None,
            None,
            None,
            "possible",
            "not_attempted",
            None,
            false,
        );
        return true;
    }
    if let Some(status) = link.settled_uncertain {
        emit(
            options,
            index,
            target,
            link.last_operation_key(),
            None,
            None,
            Some(status),
            effect_name(Some(status)),
            "not_attempted",
            None,
            false,
        );
        return true;
    }
    false
}

fn effect_name(status: Option<OperationStatus>) -> &'static str {
    match status {
        Some(OperationStatus::NotAdmitted) => "not_written",
        Some(OperationStatus::Completed(Outcome::Failed)) => "not_written",
        Some(OperationStatus::Completed(Outcome::Applied)) => "pty_written",
        Some(OperationStatus::Completed(
            Outcome::Partial | Outcome::Unknown | Outcome::Accepted,
        ))
        | Some(OperationStatus::InProgress)
        | None => "possible",
    }
}

fn effect_confidence(status: Option<OperationStatus>) -> &'static str {
    match status {
        Some(OperationStatus::NotAdmitted) | Some(OperationStatus::Completed(Outcome::Failed)) => {
            "none"
        }
        Some(OperationStatus::Completed(Outcome::Applied)) => "confirmed",
        Some(OperationStatus::Completed(
            Outcome::Partial | Outcome::Unknown | Outcome::Accepted,
        ))
        | Some(OperationStatus::InProgress)
        | None => "possible",
    }
}

// Keep each lifecycle result explicit so delivery cannot be overwritten by release.
#[allow(clippy::too_many_arguments)]
fn emit(
    options: Options<'_>,
    index: usize,
    target: TargetRef,
    operation_key: Option<OperationKey>,
    claim_key: Option<OperationKey>,
    release_key: Option<OperationKey>,
    status: Option<OperationStatus>,
    delivery: &str,
    control_release: &str,
    release_status: Option<OperationStatus>,
    ok: bool,
) {
    let effect = if delivery == "not_written" {
        "none"
    } else {
        effect_confidence(status)
    };
    if options.json {
        let mut json = String::from("{\"ok\":");
        json.push_str(if ok { "true" } else { "false" });
        json.push_str(",\"action\":\"pane_submit\"");
        let _ = write!(json, ",\"index\":{index},\"target\":");
        push_json_string(&mut json, &target_string(target));
        json.push_str(",\"operation_key\":");
        push_key(&mut json, operation_key);
        json.push_str(",\"claim_operation_key\":");
        push_key(&mut json, claim_key);
        json.push_str(",\"release_operation_key\":");
        push_key(&mut json, release_key);
        json.push_str(",\"status\":");
        if let Some(status) = status {
            push_json_string(&mut json, super::status_name(status));
        } else {
            push_json_string(&mut json, "unknown");
        }
        json.push_str(",\"delivery\":");
        push_json_string(&mut json, delivery);
        json.push_str(",\"effect\":");
        push_json_string(&mut json, effect);
        json.push_str(",\"application_submission\":\"unverified\"");
        json.push_str(",\"control_release\":");
        push_json_string(&mut json, control_release);
        json.push_str(",\"control_release_status\":");
        if let Some(status) = release_status {
            push_json_string(&mut json, super::status_name(status));
        } else {
            push_json_string(&mut json, "not_attempted");
        }
        DETAILS.with(|value| json.push_str(&value.borrow()));
        json.push('}');
        write_line(&json);
    } else {
        let operation = operation_key
            .map(key_string)
            .unwrap_or_else(|| "unknown".to_owned());
        println!(
            "panelink: pane submit {index}; operation {operation}; status {}; delivery {delivery}; application submission unverified; effect {}; control release {control_release} ({})",
            status.map(super::status_name).unwrap_or("unknown"),
            effect,
            release_status
                .map(super::status_name)
                .unwrap_or("not_attempted")
        );
    }
}

pub(super) fn push_key(json: &mut String, key: Option<OperationKey>) {
    if let Some(key) = key {
        let _ = write!(
            json,
            "{{\"generation\":{},\"lease\":{},\"sequence\":{}}}",
            key.generation, key.lease, key.sequence
        );
    } else {
        json.push_str("null");
    }
}

fn key_string(key: OperationKey) -> String {
    format!("{}:{}:{}", key.generation, key.lease, key.sequence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_effect_distinguishes_confirmed_partial_and_nonadmitted() {
        assert_eq!(
            effect_name(Some(OperationStatus::Completed(Outcome::Applied))),
            "pty_written"
        );
        assert_eq!(
            effect_confidence(Some(OperationStatus::Completed(Outcome::Applied))),
            "confirmed"
        );
        assert_eq!(
            effect_name(Some(OperationStatus::Completed(Outcome::Partial))),
            "possible"
        );
        assert_eq!(
            effect_confidence(Some(OperationStatus::Completed(Outcome::Partial))),
            "possible"
        );
        assert_eq!(
            effect_name(Some(OperationStatus::NotAdmitted)),
            "not_written"
        );
        assert_eq!(
            effect_confidence(Some(OperationStatus::NotAdmitted)),
            "none"
        );
    }
}
