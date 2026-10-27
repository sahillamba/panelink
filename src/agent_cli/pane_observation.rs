//! Finite observation of one pane target for the `wait` command.
//!
//! This owns the target-pinned authenticated link for the complete wait and
//! serializes its initial snapshot, layout checks, event cursor and screen
//! reconciliations. It never reconnects after an exchange fails.
//! Geometry-only changes preserve the original index and full target identity;
//! a missing target, renumbered index or replaced occupant ends observation.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WaitOutcome {
    Matched,
    TimedOut,
}

pub(super) enum WaitCondition<'a> {
    Text { pattern: &'a str, regex: bool },
    Quiet(Duration),
    Exit,
    Change,
}

struct ObservationState {
    cursor: panelink::event_journal::EventCursor,
    initial_revision: u64,
    screen_revision: u64,
    quiet_since: Instant,
}

struct PaneObservation {
    link: WorkspaceLink,
    initial: AgentReconciliation,
    index: usize,
    target: TargetRef,
    deadline: Instant,
    state: ObservationState,
}

pub(super) fn wait(
    workspace: &Workspace,
    index: usize,
    target: TargetRef,
    condition: WaitCondition<'_>,
    timeout: u64,
) -> Result<WaitOutcome, CliError> {
    let mut observation = PaneObservation::open(workspace, index, target, timeout)?;
    if observation.matches(&condition, &observation.initial)? {
        return Ok(WaitOutcome::Matched);
    }

    loop {
        if Instant::now() >= observation.deadline {
            return Ok(WaitOutcome::TimedOut);
        }

        // The event link already carries a target-pinned authenticated stream.
        // Query layout on it rather than opening inventory.ctx every cycle.
        observation.check_index()?;
        let (events, end) = observation
            .link
            .watch_target(
                target,
                observation.state.cursor,
                MAX_WATCH_EVENTS,
                observation.remaining_watch_ms(),
            )
            .map_err(link_error)?;
        ensure_contiguous_events(end.status)?;
        observation.state.cursor = end.cursor;
        for event in events {
            if apply_event(&mut observation.state, &event.body, &condition) {
                return Ok(WaitOutcome::Matched);
            }
        }

        let current = observation
            .link
            .reconcile_target_full_screen(target)
            .map_err(link_error)?;
        if current.info.target != target {
            return Err(stale_target());
        }
        if matches!(condition, WaitCondition::Exit) && current.info.child_exited {
            return Ok(WaitOutcome::Matched);
        }
        if current.screen_revision != observation.state.screen_revision {
            observation.note_revision(current.screen_revision);
        }
        observation.check_index()?;
        if matches!(condition, WaitCondition::Change)
            && current.screen_revision != observation.state.initial_revision
        {
            return Ok(WaitOutcome::Matched);
        }
        if observation.matches(&condition, &current)? {
            return Ok(WaitOutcome::Matched);
        }
    }
}

impl PaneObservation {
    fn open(
        workspace: &Workspace,
        index: usize,
        target: TargetRef,
        timeout: u64,
    ) -> Result<Self, CliError> {
        let mut link = workspace.pane_events_link(target)?;
        // Text waits search the whole visible screen within protocol limits.
        let initial = link
            .reconcile_target_full_screen(target)
            .map_err(link_error)?;
        let _initial_truncated = initial.truncated;
        // Check index after reconciliation so target and snapshot use the same
        // authenticated connection and no inventory handshake is introduced.
        ensure_target(&link.panes().map_err(link_error)?, index, target)?;
        let now = Instant::now();
        let state = ObservationState {
            cursor: initial.cursor,
            initial_revision: initial.screen_revision,
            screen_revision: initial.screen_revision,
            quiet_since: now,
        };
        Ok(Self {
            link,
            initial,
            index,
            target,
            deadline: now + Duration::from_secs(timeout),
            state,
        })
    }

    fn check_index(&mut self) -> Result<(), CliError> {
        ensure_target(
            &self.link.panes().map_err(link_error)?,
            self.index,
            self.target,
        )
    }

    fn remaining_watch_ms(&self) -> u16 {
        let remaining = self
            .deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(u128::from(POLL_WAIT_MS)) as u16;
        remaining.min(MAX_WATCH_WAIT_MS)
    }

    fn note_revision(&mut self, revision: u64) {
        note_revision(&mut self.state, revision);
    }

    fn matches(
        &self,
        condition: &WaitCondition<'_>,
        state: &AgentReconciliation,
    ) -> Result<bool, CliError> {
        Ok(match condition {
            WaitCondition::Text { pattern, regex } => {
                if *regex {
                    regex_matches(pattern, &state.text)?
                } else {
                    state.text.contains(pattern)
                }
            }
            WaitCondition::Quiet(duration) => self.state.quiet_since.elapsed() >= *duration,
            WaitCondition::Exit => state.info.child_exited,
            WaitCondition::Change => false,
        })
    }
}

fn ensure_target(panes: &PaneReply, index: usize, target: TargetRef) -> Result<(), CliError> {
    ensure_target_identity(super::target_at_index(panes, index), target)
}

fn ensure_target_identity(current: Option<TargetRef>, target: TargetRef) -> Result<(), CliError> {
    if current == Some(target) {
        Ok(())
    } else {
        Err(stale_target())
    }
}

fn note_revision(state: &mut ObservationState, revision: u64) {
    state.screen_revision = revision;
    state.quiet_since = Instant::now();
}

fn apply_event(
    state: &mut ObservationState,
    body: &EventBody,
    condition: &WaitCondition<'_>,
) -> bool {
    match body {
        EventBody::ScreenChanged { revision } if *revision != state.screen_revision => {
            note_revision(state, *revision);
            false
        }
        EventBody::ChildExited if matches!(condition, WaitCondition::Exit) => true,
        _ => false,
    }
}

fn ensure_contiguous_events(status: WatchStatus) -> Result<(), CliError> {
    if status == WatchStatus::ResyncRequired {
        Err(CliError::new(
            "resync_required",
            "pane events expired; re-run the wait against a fresh pane state",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(pane: u64) -> TargetRef {
        TargetRef {
            generation: 7,
            workspace: 2,
            pane,
            session: pane + 10,
            occupant_revision: 1,
        }
    }

    #[test]
    fn geometry_revision_keeps_pinned_index_but_renumber_and_replacement_stale() {
        let first = target(1);
        let second = target(2);
        let replacement = target(9);

        // Geometry-only revisions do not alter the ordered target identity.
        assert!(ensure_target_identity(Some(first), first).is_ok());
        // Closing an earlier pane renumbers the target at the original index.
        assert_eq!(
            ensure_target_identity(Some(replacement), first)
                .unwrap_err()
                .code,
            "stale_target"
        );
        // A replacement occupant at the same index is also stale.
        assert_eq!(
            ensure_target_identity(Some(second), first)
                .unwrap_err()
                .code,
            "stale_target"
        );
        assert_eq!(
            ensure_target_identity(None, first).unwrap_err().code,
            "stale_target"
        );
    }

    #[test]
    fn each_observed_screen_revision_restarts_the_quiet_clock() {
        let mut state = ObservationState {
            cursor: panelink::event_journal::EventCursor {
                generation: 7,
                sequence: 0,
            },
            initial_revision: 3,
            screen_revision: 3,
            quiet_since: Instant::now() - Duration::from_secs(1),
        };
        assert_ne!(state.quiet_since.elapsed().as_secs(), 0);
        let before_reset = Instant::now();
        note_revision(&mut state, 4);
        assert_eq!(state.screen_revision, 4);
        assert!(state.quiet_since >= before_reset);
    }

    #[test]
    fn event_sequence_resets_quiet_for_screen_change_and_reports_gaps() {
        let mut state = ObservationState {
            cursor: panelink::event_journal::EventCursor {
                generation: 7,
                sequence: 2,
            },
            initial_revision: 3,
            screen_revision: 3,
            quiet_since: Instant::now() - Duration::from_secs(2),
        };
        assert!(!apply_event(
            &mut state,
            &EventBody::ScreenChanged { revision: 3 },
            &WaitCondition::Quiet(Duration::from_secs(1))
        ));
        assert!(state.quiet_since.elapsed() >= Duration::from_secs(1));
        let before_reset = Instant::now();
        assert!(!apply_event(
            &mut state,
            &EventBody::ScreenChanged { revision: 4 },
            &WaitCondition::Quiet(Duration::from_secs(1))
        ));
        assert_eq!(state.screen_revision, 4);
        assert!(state.quiet_since >= before_reset);
        assert_eq!(
            ensure_contiguous_events(WatchStatus::ResyncRequired)
                .unwrap_err()
                .code,
            "resync_required"
        );
        assert!(ensure_contiguous_events(WatchStatus::TimedOut).is_ok());
    }
}
