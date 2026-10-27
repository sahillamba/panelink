//! Foreground-process-bound agent commands. Screen states are observations,
//! not provider conversation identity or correlated turn-completion proofs.
use super::*;
use panelink::agent_protocol::{AgentIdentity, AgentKind, AgentState, Observation};

pub(super) fn run(args: &[&str], options: Options<'_>) -> Result<i32, CliError> {
    let action = *args.first().ok_or_else(CliError::usage)?;
    let workspace = workspace(options.caller)?;
    match action {
        "list" => list(&workspace, &args[1..], options),
        "get" | "read" => get(&workspace, &args[1..], options, action == "read"),
        "prompt" => prompt(&workspace, &args[1..], options),
        "wait" => wait(&workspace, &args[1..], options),
        _ => Err(CliError::usage()),
    }
}

fn observation_error(label: &'static str) -> CliError {
    match label {
        "agent_observation_unsupported" => CliError::new(
            label,
            "this server lacks foreground-agent observations; use pane reads or start a workspace with the matching newer server",
        ),
        _ => link_error(label),
    }
}

fn reference(observation: &Observation) -> String {
    format!(
        "{}@{}:{}:{}:{}",
        target_string(observation.target),
        observation.identity.pid,
        observation.identity.start_seconds,
        observation.identity.start_microseconds,
        observation.identity.kind.as_str()
    )
}

fn parse_reference(value: &str) -> Result<(TargetRef, AgentIdentity), CliError> {
    let (target, identity) = value.split_once('@').ok_or_else(CliError::usage)?;
    let fields: Vec<_> = target.split(':').collect();
    let ids: Vec<u64> = fields
        .iter()
        .map(|field| field.parse().map_err(|_| CliError::usage()))
        .collect::<Result<_, _>>()?;
    if ids.len() != 5 || ids.contains(&0) {
        return Err(CliError::usage());
    }
    let fields: Vec<_> = identity.split(':').collect();
    if fields.len() != 4 {
        return Err(CliError::usage());
    }
    let pid: u64 = fields[0].parse().map_err(|_| CliError::usage())?;
    let start_seconds: u64 = fields[1].parse().map_err(|_| CliError::usage())?;
    let start_microseconds: u64 = fields[2].parse().map_err(|_| CliError::usage())?;
    let kind = match fields[3] {
        "codex" => AgentKind::Codex,
        "opencode" => AgentKind::OpenCode,
        _ => return Err(CliError::usage()),
    };
    if pid == 0 || pid > i32::MAX as u64 || start_seconds == 0 || start_microseconds >= 1_000_000 {
        return Err(CliError::usage());
    }
    Ok((
        TargetRef {
            generation: ids[0],
            workspace: ids[1],
            pane: ids[2],
            session: ids[3],
            occupant_revision: ids[4],
        },
        AgentIdentity {
            pid,
            start_seconds,
            start_microseconds,
            kind,
        },
    ))
}

fn query(workspace: &Workspace, target: TargetRef) -> Result<Observation, CliError> {
    let mut link = workspace.pane_agent_link(target)?;
    let observation = link.observe_agent(target).map_err(observation_error)?;
    if observation.target != target {
        return Err(stale_target());
    }
    Ok(observation)
}

fn resolve(workspace: &Workspace, value: &str) -> Result<(usize, Observation), CliError> {
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        let index = parse_index(value)?;
        let (target, _, _) = resolve_index(workspace, index)?;
        return Ok((index, query(workspace, target)?));
    }
    let (target, expected) = parse_reference(value)?;
    let panes = all_panes(workspace)?;
    let mut found = None;
    for index in 1..=panes.panes.iter().flatten().count() {
        if resolve_index(workspace, index)?.0 == target {
            found = Some(index);
            break;
        }
    }
    let index = found.ok_or_else(|| {
        CliError::new(
            "agent_not_running",
            "the referenced pane occupant is no longer present",
        )
    })?;
    let observation = query(workspace, target)?;
    if observation.identity != expected {
        return Err(CliError::new(
            "agent_changed",
            "the foreground agent process changed; no input was sent",
        ));
    }
    Ok((index, observation))
}

fn require_recognized(observation: &Observation) -> Result<(), CliError> {
    if observation.identity.kind == AgentKind::Unknown || observation.identity.pid == 0 {
        return Err(CliError::new("agent_not_recognized", "no supported foreground agent is verified for this pane; use raw pane commands deliberately"));
    }
    Ok(())
}

fn push_observation(json: &mut String, index: usize, observation: &Observation) {
    let _ = write!(
        json,
        "{{\"index\":{index},\"recognized\":{},\"agent_ref\":",
        observation.identity.kind != AgentKind::Unknown
    );
    if observation.identity.kind == AgentKind::Unknown {
        json.push_str("null");
    } else {
        push_json_string(json, &reference(observation));
    }
    json.push_str(",\"target\":");
    push_json_string(json, &target_string(observation.target));
    json.push_str(",\"kind\":");
    push_json_string(json, observation.identity.kind.as_str());
    json.push_str(",\"state\":");
    push_json_string(json, observation.state.as_str());
    let _ = write!(json, ",\"screen_revision\":\"{}\",\"sampled_at_mono_ms\":\"{}\",\"identity_source\":\"pty_foreground_process\",\"state_source\":\"screen_markers\",\"conversation_identity_verified\":false}}", observation.screen_revision, observation.sampled_at_mono_ms);
}

fn emit_observation(action: &str, index: usize, observation: &Observation, options: Options<'_>) {
    if options.json {
        let mut json = format!("{{\"ok\":true,\"action\":\"{action}\",\"agent\":");
        push_observation(&mut json, index, observation);
        json.push('}');
        write_line(&json);
    } else {
        println!(
            "{} {} {} {}",
            index,
            observation.identity.kind.as_str(),
            observation.state.as_str(),
            reference(observation)
        );
    }
}

fn list(workspace: &Workspace, args: &[&str], options: Options<'_>) -> Result<i32, CliError> {
    if args.iter().any(|arg| *arg != "--json") {
        return Err(CliError::usage());
    }
    let panes = all_panes(workspace)?;
    // Collect before printing, so a routing/protocol failure cannot become a
    // truncated success envelope. The server advertises at most four panes.
    let mut observations = Vec::new();
    for index in 1..=panes.panes.iter().flatten().count() {
        let (target, _, _) = resolve_index(workspace, index)?;
        observations.push((index, query(workspace, target)?));
    }
    if options.json {
        let mut json = String::from(
            "{\"ok\":true,\"action\":\"agent_list\",\"route\":\"Default/Main\",\"agents\":[",
        );
        for (position, (index, observation)) in observations.iter().enumerate() {
            if position != 0 {
                json.push(',');
            }
            push_observation(&mut json, *index, observation);
        }
        json.push_str("]}");
        write_line(&json);
    } else {
        for (index, observation) in &observations {
            emit_observation("agent_get", *index, observation, options);
        }
    }
    Ok(0)
}

fn get(
    workspace: &Workspace,
    args: &[&str],
    options: Options<'_>,
    read: bool,
) -> Result<i32, CliError> {
    let value = *args.first().ok_or_else(CliError::usage)?;
    let mut lines = 80u16;
    let mut bottom = false;
    let mut cursor = 1;
    while cursor < args.len() {
        match args[cursor] {
            "--json" => cursor += 1,
            "--bottom" if read => {
                bottom = true;
                cursor += 1;
            }
            "--lines" if read => {
                cursor += 1;
                lines = args
                    .get(cursor)
                    .ok_or_else(CliError::usage)?
                    .parse()
                    .map_err(|_| CliError::usage())?;
                cursor += 1;
            }
            _ => return Err(CliError::usage()),
        }
    }
    if lines == 0 || lines > 1024 {
        return Err(CliError::usage());
    }
    let (index, observation) = resolve(workspace, value)?;
    require_recognized(&observation)?;
    if !read {
        emit_observation("agent_get", index, &observation, options);
        return Ok(0);
    }
    let mut link = workspace.pane_agent_link(observation.target)?;
    let mut screen = link
        .read_screen(
            observation.target,
            ReadSource::Screen,
            65_446,
            if bottom { 1024 } else { lines },
        )
        .map_err(link_error)?;
    if bottom {
        select_bottom(&mut screen, lines)?;
    }
    let after = link
        .observe_agent(observation.target)
        .map_err(observation_error)?;
    if after.identity != observation.identity {
        return Err(CliError::new(
            "agent_changed",
            "the foreground process changed during the read",
        ));
    }
    if options.json {
        let mut json = String::from("{\"ok\":true,\"action\":\"agent_read\",\"agent\":");
        push_observation(&mut json, index, &after);
        let _ = write!(
            json,
            ",\"row_selection\":\"{}\",\"text\":",
            if bottom { "bottom" } else { "top" }
        );
        push_json_string(&mut json, &screen.text);
        let _ = write!(json, ",\"truncated\":{}}}", screen.truncated);
        write_line(&json);
    } else {
        print!("{}", screen.text);
        let _ = io::stdout().flush();
    }
    Ok(0)
}

fn state(value: &str) -> Result<AgentState, CliError> {
    match value {
        "idle" => Ok(AgentState::Idle),
        "working" => Ok(AgentState::Working),
        "blocked" => Ok(AgentState::Blocked),
        "unknown" => Ok(AgentState::Unknown),
        _ => Err(CliError::usage()),
    }
}

fn wait_options(args: &[&str]) -> Result<(Vec<AgentState>, u64), CliError> {
    let mut states = Vec::new();
    let mut timeout = 30;
    let mut seen_timeout = false;
    let mut cursor = 0;
    while cursor < args.len() {
        match args[cursor] {
            "--json" => cursor += 1,
            "--until" => {
                cursor += 1;
                states.push(state(args.get(cursor).ok_or_else(CliError::usage)?)?);
                cursor += 1;
            }
            "--timeout" if !seen_timeout => {
                cursor += 1;
                timeout = args
                    .get(cursor)
                    .ok_or_else(CliError::usage)?
                    .parse()
                    .map_err(|_| CliError::usage())?;
                seen_timeout = true;
                cursor += 1;
            }
            _ => return Err(CliError::usage()),
        }
    }
    if timeout == 0 || timeout > 300 {
        return Err(CliError::usage());
    }
    if states.is_empty() {
        states = vec![AgentState::Idle, AgentState::Blocked];
    }
    Ok((states, timeout))
}

fn wait(workspace: &Workspace, args: &[&str], options: Options<'_>) -> Result<i32, CliError> {
    let value = *args.first().ok_or_else(CliError::usage)?;
    let (states, timeout) = wait_options(&args[1..])?;
    let (index, observation) = resolve(workspace, value)?;
    require_recognized(&observation)?;
    wait_for(
        workspace,
        index,
        observation,
        &states,
        timeout,
        false,
        options,
    )
}

fn wait_for(
    workspace: &Workspace,
    index: usize,
    initial: Observation,
    states: &[AgentState],
    timeout: u64,
    require_activity: bool,
    options: Options<'_>,
) -> Result<i32, CliError> {
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let startup_deadline = (Instant::now() + Duration::from_secs(5)).min(deadline);
    let mut activity = !require_activity;
    // One finite read-only wait keeps its authenticated connection instead of
    // reopening a socket/lease handshake for every 50 ms observation. Each
    // query still validates the exact process identity; failures are reported,
    // never hidden by reconnecting or replaying a prompt.
    let mut link = workspace.pane_agent_link(initial.target)?;
    loop {
        let current = link
            .observe_agent(initial.target)
            .map_err(observation_error)?;
        if current.identity != initial.identity {
            return Err(CliError::new(
                "agent_changed",
                "the referenced foreground agent changed while waiting",
            ));
        }
        activity |= current.state == AgentState::Working || current.state == AgentState::Blocked;
        if activity && states.contains(&current.state) {
            emit_observation("agent_wait", index, &current, options);
            return Ok(0);
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(CliError::new(
                "observation_timeout",
                "agent state wait expired; this does not mean earlier input was undelivered",
            ));
        }
        if !activity && now >= startup_deadline {
            return Err(CliError::new("agent_prompt_stalled", "prompt delivery was confirmed but working/blocked activity was not observed; do not resend blindly"));
        }
        thread::sleep(Duration::from_millis(50).min(deadline.saturating_duration_since(now)));
    }
}

fn prompt(workspace: &Workspace, args: &[&str], options: Options<'_>) -> Result<i32, CliError> {
    let value = *args.first().ok_or_else(CliError::usage)?;
    // Mutations accept only a process-bound ref returned by discovery.
    parse_reference(value)?;
    let mut wait_args = Vec::new();
    let mut words = Vec::new();
    let mut wait = false;
    let mut cursor = 1;
    while cursor < args.len() {
        match args[cursor] {
            "--wait" if !wait => {
                wait = true;
                cursor += 1;
            }
            "--json" => cursor += 1,
            "--timeout" | "--until" => {
                wait_args.push(args[cursor]);
                cursor += 1;
                wait_args.push(*args.get(cursor).ok_or_else(CliError::usage)?);
                cursor += 1;
            }
            value if value.starts_with('-') => return Err(CliError::usage()),
            value => {
                words.push(value);
                cursor += 1;
            }
        }
    }
    if words.is_empty() || (!wait && !wait_args.is_empty()) {
        return Err(CliError::usage());
    }
    let (states, timeout) = wait_options(&wait_args)?;
    let (index, observation) = resolve(workspace, value)?;
    require_recognized(&observation)?;
    if observation.state == AgentState::Blocked {
        return Err(CliError::new(
            "agent_blocked",
            "the agent shows an approval/question screen; no prompt was sent",
        ));
    }
    if observation.state != AgentState::Idle {
        return Err(CliError::new(
            "agent_not_ready",
            "the agent is not observably idle; no prompt was sent",
        ));
    }
    let result = scoped::submit(
        workspace,
        index,
        options,
        &words.join(" "),
        Some(&observation),
    )?;
    if result != 0 || !wait {
        return Ok(result);
    }
    // Delivery and release are already reported. A subsequent wait result is
    // a separate observation and can never turn delivery into 'nothing sent'.
    wait_for(
        workspace,
        index,
        observation,
        &states,
        timeout,
        true,
        options,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_bound_refs_round_trip_u64_without_name_resolution() {
        let target = TargetRef {
            generation: u64::MAX,
            workspace: 1,
            pane: 2,
            session: 3,
            occupant_revision: 4,
        };
        let identity = AgentIdentity {
            pid: 42,
            start_seconds: u64::MAX,
            start_microseconds: 999_999,
            kind: AgentKind::Codex,
        };
        let observation = Observation {
            target,
            identity,
            state: AgentState::Idle,
            screen_revision: u64::MAX,
            sampled_at_mono_ms: 1,
        };
        assert_eq!(
            parse_reference(&reference(&observation)).unwrap(),
            (target, identity)
        );
        assert!(parse_reference("1:1:1:1:1@0:1:0:codex").is_err());
        assert!(parse_reference("1:1:1:1:1@1:1:1000000:codex").is_err());
        assert!(parse_reference("1:1:1:1:1@1:1:0:unknown").is_err());
    }
    #[test]
    fn wait_defaults_and_limits_are_bounded() {
        assert_eq!(wait_options(&[]).unwrap().1, 30);
        assert!(wait_options(&["--timeout", "0"]).is_err());
        assert!(wait_options(&["--timeout", "301"]).is_err());
        assert!(wait_options(&["--until", "done"]).is_err());
        assert!(wait_options(&["--timeout", "2", "--timeout", "3"]).is_err());
    }
}
