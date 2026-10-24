//! Historical inspection and explicit two-step provider recovery commands.

use panelink::control_protocol::TargetRef;
use panelink::managed_provider_store::ManagedProviderStore;
use panelink::provider_binding::LastConfirmedRecord;
use panelink::provider_binding_store::ProviderBindingStore;
use panelink::provider_bridge::SUPPORTED_PROVIDER_VERSION;
use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub fn run(args: Vec<OsString>) -> i32 {
    let result = if args
        .first()
        .is_some_and(|arg| matches!(arg.to_str(), Some("prepare" | "resume" | "status")))
    {
        recovery(&args)
    } else {
        inspect(&args).map(|line| (0, line))
    };
    match result {
        Ok((code, line)) => {
            println!("{line}");
            code
        }
        Err(code) => {
            println!("{{\"ok\":false,\"error\":\"{code}\",\"recovery\":\"manual_selection_required\",\"automatic_resume\":false}}");
            1
        }
    }
}

fn recovery(args: &[OsString]) -> Result<(i32, String), &'static str> {
    use panelink::provider_recovery_endpoint::{
        exchange, ManualReason, RecoveryCommand, RecoveryReply,
    };
    if args.len() < 7
        || args.len() > 11
        || args[1] != "--directory"
        || args[3] != "--target"
        || args[5] != "--project"
        || args.iter().any(|arg| arg.as_bytes().len() > 4096)
    {
        return Err("provider_recovery_syntax");
    }
    let target = parse_target(args[4].to_str().ok_or("invalid_target")?)?;
    let directory = std::fs::canonicalize(PathBuf::from(&args[2]))
        .map_err(|_| "provider_recovery_unavailable")?;
    let project =
        std::fs::canonicalize(PathBuf::from(&args[6])).map_err(|_| "project_unavailable")?;
    if !project.is_dir() {
        return Err("project_unavailable");
    }
    let command = match args[0].to_str() {
        Some("prepare") if args.len() == 9 && args[7] == "--session" => {
            let session_id = args[8].to_str().ok_or("invalid_provider_session")?;
            if session_id.is_empty()
                || session_id.len() > 128
                || !session_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
            {
                return Err("invalid_provider_session");
            }
            RecoveryCommand::Prepare {
                session_id: session_id.to_owned(),
            }
        }
        Some("resume") if args.len() == 11 && args[7] == "--ticket" && args[9] == "--revision" => {
            let ticket = parse_ticket(args[8].to_str().ok_or("invalid_recovery_ticket")?)?;
            let revision_text = args[10].to_str().ok_or("invalid_binding_revision")?;
            if revision_text.is_empty() || !revision_text.bytes().all(|b| b.is_ascii_digit()) {
                return Err("invalid_binding_revision");
            }
            let revision = revision_text
                .parse::<u64>()
                .map_err(|_| "invalid_binding_revision")?;
            if revision == 0 {
                return Err("invalid_binding_revision");
            }
            RecoveryCommand::Resume { ticket, revision }
        }
        Some("status") if args.len() == 7 => RecoveryCommand::Status,
        _ => return Err("provider_recovery_syntax"),
    };
    let socket = directory
        .join(format!("provider-bridge-{}", target.generation))
        .join("manual.sock");
    let result = exchange(
        &socket,
        target,
        project.as_os_str().as_bytes(),
        command,
        Instant::now() + Duration::from_secs(10),
    );
    let (code, fields) = match result {
        Ok(RecoveryReply::Prepared { ticket, revision }) => {
            let ticket = ticket.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            (0, format!("\"ok\":true,\"state\":\"prepared_choice_saved\",\"ticket\":\"{ticket}\",\"revision\":{revision},\"selected_confirmed\":false,\"next_action\":\"explicit_resume_required\""))
        }
        Ok(RecoveryReply::Confirmed { revision }) => (0, format!("\"ok\":true,\"state\":\"selected_and_saved\",\"revision\":{revision},\"selected_confirmed\":true")),
        Ok(RecoveryReply::Pending { revision }) => (2, format!("\"ok\":false,\"state\":\"pending\",\"revision\":{revision}")),
        Ok(RecoveryReply::ManualRequired { reason }) => {
            let error = match reason {
                ManualReason::NoLastConfirmed => "no_last_confirmed_conversation",
                ManualReason::InvalidChoice => "explicit_choice_not_prepared",
                ManualReason::MissingSession | ManualReason::DeletedSession => "provider_session_not_found",
                ManualReason::ForeignProject | ManualReason::ProjectMismatch => "provider_project_mismatch",
                ManualReason::ChildSession => "provider_session_is_child",
                ManualReason::UnsupportedProvider => "provider_version_unsupported",
                ManualReason::UncertainProvider => "provider_choice_unverified",
                ManualReason::InterruptedOperation => "recovery_interrupted_or_already_spent",
            };
            (1, format!("\"ok\":false,\"state\":\"manual_selection_required\",\"error\":\"{error}\""))
        }
        Ok(RecoveryReply::Unavailable) => (1, "\"ok\":false,\"error\":\"provider_recovery_unavailable\"".to_owned()),
        Ok(RecoveryReply::Busy) => (1, "\"ok\":false,\"error\":\"provider_recovery_busy\"".to_owned()),
        Ok(RecoveryReply::Uncertain) | Err(_) => (2, "\"ok\":false,\"state\":\"uncertain\",\"retry_allowed\":false,\"next_action\":\"query_status_or_use_provider_picker\"".to_owned()),
    };
    Ok((
        code,
        format!("{{{fields},\"automatic_resume\":false,\"current_at_crash\":false}}"),
    ))
}

fn parse_ticket(text: &str) -> Result<[u8; 16], &'static str> {
    if text.len() != 32 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid_recovery_ticket");
    }
    let mut bytes = [0; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .map_err(|_| "invalid_recovery_ticket")?;
    }
    if bytes.iter().all(|b| *b == 0) {
        return Err("invalid_recovery_ticket");
    }
    Ok(bytes)
}

fn inspect(args: &[OsString]) -> Result<String, &'static str> {
    if args.len() != 7
        || args[0] != "show"
        || args[1] != "--directory"
        || args[3] != "--target"
        || args[5] != "--project"
        || args.iter().any(|arg| arg.as_bytes().len() > 4096)
    {
        return Err("provider_binding_syntax");
    }
    let target = parse_target(args[4].to_str().ok_or("invalid_target")?)?;
    let directory = std::fs::canonicalize(PathBuf::from(&args[2]))
        .map_err(|_| "provider_binding_unavailable")?;
    let project =
        std::fs::canonicalize(PathBuf::from(&args[6])).map_err(|_| "project_unavailable")?;
    if !project.is_dir() {
        return Err("project_unavailable");
    }
    let path =
        panelink::provider_bridge_launch::BridgeLaunch::binding_directory(&directory, target);
    let (record, managed_linked) =
        if std::fs::symlink_metadata(path.join("managed-provider.record")).is_ok() {
            let record = ManagedProviderStore::inspect_historical(
                &path,
                target,
                project.as_os_str().as_bytes(),
                SUPPORTED_PROVIDER_VERSION,
                panelink::provider_bridge::SUPPORTED_PROVIDER_FINGERPRINT,
            )
            .map_err(|_| "provider_binding_unavailable")?
            .ok_or("no_last_confirmed_conversation")?;
            (
                LastConfirmedRecord::from_validated_binding(record.into_binding()),
                true,
            )
        } else {
            // Do not create a lock or invent a candidate for a target with no saved record.
            if std::fs::symlink_metadata(path.join("last-confirmed.binding")).is_err() {
                return Err("no_last_confirmed_conversation");
            }
            let store = ProviderBindingStore::open(
                &path,
                target,
                project.as_os_str().as_bytes(),
                SUPPORTED_PROVIDER_VERSION,
            )
            .map_err(|_| "provider_binding_unavailable")?;
            let record = store
                .load()
                .map_err(|_| "provider_binding_unavailable")?
                .ok_or("no_last_confirmed_conversation")?;
            (record, false)
        };
    let binding = record.binding();
    // Session IDs are validated bounded ASCII tokens by the binding codec.
    Ok(format!(
        "{{\"ok\":true,\"state\":\"last_confirmed_conversation\",\"target\":{{\"generation\":{},\"workspace\":{},\"pane\":{},\"session\":{},\"occupant_revision\":{}}},\"provider_session_id\":\"{}\",\"binding_revision\":{},\"verified_at_unix_ms\":{},\"provider_version\":\"{}\",\"managed_launch_linked\":{},\"current_at_crash\":false,\"automatic_resume\":false,\"next_action\":\"explicit_provider_session_selection_required\"}}",
        target.generation, target.workspace, target.pane, target.session,
        target.occupant_revision, binding.session_id(), binding.revision(),
        binding.verified_at_unix_ms(), binding.supported_version(), managed_linked,
    ))
}

fn parse_target(text: &str) -> Result<TargetRef, &'static str> {
    let mut values = [0u64; 5];
    let mut parts = text.split(':');
    for value in &mut values {
        let part = parts.next().ok_or("invalid_target")?;
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid_target");
        }
        *value = part.parse().map_err(|_| "invalid_target")?;
        if *value == 0 {
            return Err("invalid_target");
        }
    }
    if parts.next().is_some() {
        return Err("invalid_target");
    }
    Ok(TargetRef {
        generation: values[0],
        workspace: values[1],
        pane: values[2],
        session: values[3],
        occupant_revision: values[4],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_target_never_accepts_latest_or_partial_identity() {
        for input in [
            "latest",
            "1:2:3:4",
            "1:2:3:4:0",
            "1:2:3:4:5:6",
            "-1:2:3:4:5",
            "1:2:3:4:18446744073709551616",
        ] {
            assert!(parse_target(input).is_err());
        }
        assert_eq!(parse_target("1:2:3:4:5").unwrap().occupant_revision, 5);
    }
}
