//! Pure decisions for the saved-layout startup chooser.

/// Minimal historical observation presented by the chooser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayoutSummary {
    /// Number of panes in the historical layout.
    pub pane_count: usize,
    /// Wall-clock time of the saved observation.
    pub observed_at_unix_ms: u64,
}

/// Available actions shown by the startup chooser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChooserOptions {
    /// A validated current layout exists.
    pub view_current: bool,
    /// A validated previous layout exists.
    pub view_previous: bool,
}

/// Chooser decision independent of IO and terminal state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChooserPlan {
    /// A live server already exists; preserve today's attach path.
    AttachExisting,
    /// Continue with the normal fresh-shell startup path.
    FreshShell,
    /// A saved layout exists and it is appropriate to ask interactively.
    Prompt { options: ChooserOptions },
}

/// User choices recognized by the line-mode prompt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChooserChoice {
    /// Start the normal fresh shell.
    Fresh,
    /// Open the read-only current-layout viewer.
    ViewCurrent,
    /// Open the read-only previous-layout viewer.
    ViewPrevious,
    /// Exit without starting or attaching a server.
    Quit,
}

/// Pick the startup action from read-only observations.
pub const fn plan(
    live_server: bool,
    interactive: bool,
    current: Option<LayoutSummary>,
    previous: Option<LayoutSummary>,
) -> ChooserPlan {
    if live_server {
        return ChooserPlan::AttachExisting;
    }
    if !interactive || current.is_none() {
        return ChooserPlan::FreshShell;
    }
    ChooserPlan::Prompt {
        options: ChooserOptions {
            view_current: true,
            view_previous: previous.is_some(),
        },
    }
}

/// Resolve one byte from the bounded prompt; all unrecognized input defaults fresh.
pub const fn choice(byte: u8) -> ChooserChoice {
    match byte {
        b'v' | b'V' => ChooserChoice::ViewCurrent,
        b'p' | b'P' => ChooserChoice::ViewPrevious,
        b'q' | b'Q' => ChooserChoice::Quit,
        _ => ChooserChoice::Fresh,
    }
}

/// Human-readable relative age without exposing identifiers or storage paths.
pub fn relative_age(summary: LayoutSummary, now_unix_ms: u64) -> String {
    let age_ms = now_unix_ms.saturating_sub(summary.observed_at_unix_ms);
    if age_ms < 1_000 {
        return "less than a second ago".to_owned();
    }
    let age_secs = age_ms / 1_000;
    if age_secs < 60 {
        return format!("{age_secs}s ago");
    }
    let age_minutes = age_secs / 60;
    if age_minutes < 60 {
        return format!("{age_minutes}m ago");
    }
    let age_hours = age_minutes / 60;
    if age_hours < 24 {
        return format!("{age_hours}h ago");
    }
    format!("{}d ago", age_hours / 24)
}

#[cfg(test)]
mod tests {
    use super::{
        choice, plan, relative_age, ChooserChoice, ChooserOptions, ChooserPlan, LayoutSummary,
    };

    const CURRENT: LayoutSummary = LayoutSummary {
        pane_count: 2,
        observed_at_unix_ms: 1_000,
    };
    const PREVIOUS: LayoutSummary = LayoutSummary {
        pane_count: 1,
        observed_at_unix_ms: 500,
    };

    #[test]
    fn plan_exhaustively_prioritizes_live_server_and_noninteractive_fresh_start() {
        for live_server in [false, true] {
            for interactive in [false, true] {
                for current in [None, Some(CURRENT)] {
                    for previous in [None, Some(PREVIOUS)] {
                        let result = plan(live_server, interactive, current, previous);
                        let expected = if live_server {
                            ChooserPlan::AttachExisting
                        } else if !interactive || current.is_none() {
                            ChooserPlan::FreshShell
                        } else {
                            ChooserPlan::Prompt {
                                options: ChooserOptions {
                                    view_current: true,
                                    view_previous: previous.is_some(),
                                },
                            }
                        };
                        assert_eq!(result, expected);
                    }
                }
            }
        }
    }

    #[test]
    fn every_byte_value_has_a_deterministic_choice() {
        for byte in u8::MIN..=u8::MAX {
            let expected = match byte {
                b'v' | b'V' => ChooserChoice::ViewCurrent,
                b'p' | b'P' => ChooserChoice::ViewPrevious,
                b'q' | b'Q' => ChooserChoice::Quit,
                _ => ChooserChoice::Fresh,
            };
            assert_eq!(choice(byte), expected, "byte {byte}");
        }
        assert_eq!(choice(0x1b), ChooserChoice::Fresh);
        assert_eq!(choice(b'\r'), ChooserChoice::Fresh);
        assert_eq!(choice(b'\n'), ChooserChoice::Fresh);
    }

    #[test]
    fn previous_layout_can_be_viewed_without_a_current_layout() {
        assert_eq!(
            plan(false, true, None, Some(PREVIOUS)),
            ChooserPlan::FreshShell
        );
    }

    #[test]
    fn relative_age_is_bounded_and_handles_future_timestamps() {
        let cases = [
            (1_000, "less than a second ago"),
            (2_999, "1s ago"),
            (61_000, "1m ago"),
            (3_601_000, "1h ago"),
            (86_401_000, "1d ago"),
        ];
        for (now, expected) in cases {
            assert_eq!(relative_age(CURRENT, now), expected);
        }
        assert_eq!(relative_age(PREVIOUS, 1), "less than a second ago");
    }
}
