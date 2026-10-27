//! Conservative provider state classification from a bounded terminal screen.
//!
//! Callers must supply a server-verified process kind. Text markers alone do
//! not establish process or conversation identity and never authorize input.

use crate::agent_protocol::{AgentKind, AgentState};
use crate::terminal::ScreenSnapshot;

const MAX_ROWS: usize = 80;
const MAX_COLUMNS: usize = 240;

/// Classify a known provider's recent screen text using conservative markers.
///
/// Only the last 80 rows and first 240 Unicode scalar values per row are
/// inspected. Unknown providers and ambiguous or unsupported screens remain
/// Unknown.
pub fn classify(kind: AgentKind, text: &str) -> AgentState {
    if kind == AgentKind::Unknown {
        return AgentState::Unknown;
    }

    let rows: Vec<String> = text
        .lines()
        .rev()
        .take(MAX_ROWS)
        .map(|line| line.chars().take(MAX_COLUMNS).collect())
        .collect();
    classify_rows(kind, rows, false, true)
}

/// Classify a provider's visible terminal snapshot, using cell styling when
/// recognizing the one observed Codex composer placeholder.
pub fn classify_snapshot(kind: AgentKind, snapshot: &ScreenSnapshot) -> AgentState {
    if kind == AgentKind::Unknown {
        return AgentState::Unknown;
    }

    let mut styled_empty = false;
    let mut rows = Vec::with_capacity(usize::from(snapshot.rows()).min(MAX_ROWS));
    for row_index in (0..snapshot.rows()).rev() {
        let mut text = String::new();
        let mut styled = Vec::new();
        for column in 0..snapshot.columns().min(MAX_COLUMNS as u16) {
            let Some(cell) = snapshot.cell(row_index, column) else {
                continue;
            };
            for character in cell.text().chars() {
                text.push(character);
                styled.push((character, cell.dim()));
            }
        }
        let trimmed = text.trim_end();
        if rows.is_empty() && trimmed.is_empty() {
            // PTYs without an alternate screen can leave unused rows below
            // the composer. They are not part of the bottom status window.
            continue;
        }
        if kind == AgentKind::Codex
            && trimmed.trim() == "›"
            && rows.iter().all(|row: &String| row.trim().is_empty())
        {
            if let Some(column) = (0..snapshot.columns()).find(|column| {
                snapshot
                    .cell(row_index, *column)
                    .is_some_and(|cell| cell.text() == "›")
            }) {
                styled_empty |= !snapshot.cursor_hidden()
                    && snapshot.cursor_row() == row_index
                    && snapshot.cursor_column() > column
                    && snapshot.cursor_column() <= column + 2;
            }
        }
        if kind == AgentKind::Codex && is_dim_codex_placeholder(trimmed, &styled) {
            // The observed placeholder is rendered only for an empty composer.
            // Require its visible input cursor too; a copied/old placeholder
            // elsewhere on screen cannot establish readiness.
            let marker_column = (0..snapshot.columns()).find(|column| {
                snapshot
                    .cell(row_index, *column)
                    .is_some_and(|cell| cell.text() == "›")
            });
            if let Some(column) = marker_column {
                let (cursor_row, cursor_column) = (snapshot.cursor_row(), snapshot.cursor_column());
                styled_empty |= !snapshot.cursor_hidden()
                    && cursor_row == row_index
                    && cursor_column <= column + 2
                    && cursor_column > column;
            }
        }
        rows.push(trimmed.to_owned());
        if rows.len() == MAX_ROWS {
            break;
        }
    }
    classify_rows(kind, rows, styled_empty, false)
}

fn is_dim_codex_placeholder(row: &str, styled: &[(char, bool)]) -> bool {
    if row.trim() != "› Ask Codex to do anything" {
        return false;
    }
    let Some(marker_index) = styled.iter().position(|(character, _)| *character == '›') else {
        return false;
    };
    if styled[..marker_index]
        .iter()
        .any(|(character, _)| !character.is_whitespace())
    {
        return false;
    }
    styled[marker_index + 1..]
        .iter()
        .all(|(character, dim)| character.is_whitespace() || *dim)
}

fn classify_rows(
    kind: AgentKind,
    rows: Vec<String>,
    styled_empty: bool,
    allow_bare: bool,
) -> AgentState {
    if rows.is_empty() {
        return AgentState::Unknown;
    }

    // Keep the original conservative precedence across the bounded screen.
    // Historical markers may cause false refusals; they must not hide a
    // possible approval request or a working provider behind an empty prompt.
    if rows.iter().any(|row| has_approval_marker(kind, row)) {
        return AgentState::Blocked;
    }
    if rows.iter().any(|row| has_working_marker(kind, row)) {
        return AgentState::Working;
    }

    match kind {
        AgentKind::Unknown => AgentState::Unknown,
        AgentKind::Codex if styled_empty || (allow_bare && codex_idle_prompt(&rows)) => {
            AgentState::Idle
        }
        AgentKind::OpenCode if opencode_idle_prompt(&rows) => AgentState::Idle,
        AgentKind::Codex | AgentKind::OpenCode => AgentState::Unknown,
    }
}

fn has_approval_marker(kind: AgentKind, row: &str) -> bool {
    let row = row.trim().to_ascii_lowercase();
    let markers: &[&str] = match kind {
        AgentKind::Codex => &[
            "approval required",
            "approve this command",
            "allow this command",
            "do you want to run",
            "run this command?",
            "press y to approve",
            "requires approval",
            "permission required",
        ],
        AgentKind::OpenCode => &[
            "permission required",
            "approval required",
            "allow this tool",
            "allow this command",
            "do you want to allow",
            "approve this action",
            "grant permission",
        ],
        AgentKind::Unknown => &[],
    };
    markers.iter().any(|marker| row.contains(marker))
}

fn has_working_marker(kind: AgentKind, row: &str) -> bool {
    let row = row.trim().to_ascii_lowercase();
    let markers: &[&str] = match kind {
        AgentKind::Codex => &[
            "esc to interrupt",
            "working…",
            "working...",
            "thinking…",
            "thinking...",
            "searching…",
            "searching...",
            "running command",
            "generating…",
            "generating...",
        ],
        AgentKind::OpenCode => &[
            "esc to interrupt",
            "ctrl+c to interrupt",
            "working…",
            "working...",
            "thinking…",
            "thinking...",
            "running tool",
            "running command",
            "generating…",
            "generating...",
        ],
        AgentKind::Unknown => &[],
    };
    markers.iter().any(|marker| row.contains(marker))
}

fn codex_idle_prompt(rows_from_bottom: &[String]) -> bool {
    rows_from_bottom
        .iter()
        .take(5)
        .enumerate()
        .any(|(index, row)| {
            row.trim() == "›"
                && rows_from_bottom[..index]
                    .iter()
                    .all(|below| below.trim().is_empty())
        })
}

fn opencode_idle_prompt(rows_from_bottom: &[String]) -> bool {
    // Require the OpenCode prompt affordance and interactive key footer near
    // the bottom. A bare greater-than marker can be ordinary output.
    let bottom = rows_from_bottom.iter().take(5);
    let mut has_prompt = false;
    let mut has_footer = false;
    for row in bottom {
        let trimmed = row.trim();
        if trimmed == "❯" || trimmed == "> " || trimmed == ">" {
            has_prompt = true;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.contains("ctrl+c") && (lower.contains("commands") || lower.contains("exit")) {
            has_footer = true;
        }
        if lower.contains("tab") && lower.contains("agents") && lower.contains("ctrl+c") {
            has_footer = true;
        }
    }
    has_prompt && has_footer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::{Environment, ResolvedLaunch};
    use crate::terminal::{TerminalSession, TerminalSize};
    use std::ffi::OsString;
    use std::time::{Duration, Instant};

    fn parsed_snapshot(output: &str) -> ScreenSnapshot {
        let launch = ResolvedLaunch {
            requested_path: "/bin/sh".into(),
            executable_path: "/bin/sh".into(),
            working_directory: std::env::temp_dir(),
        };
        let prompt_row = output
            .split("\r\n")
            .position(|row| row.contains('›'))
            .unwrap_or(0);
        let output = format!("{output}\x1b[{};3H", prompt_row + 1);
        let arguments = [
            OsString::from("-c"),
            OsString::from("printf '%s' \"$1\""),
            OsString::from("agent-detection-test"),
            OsString::from(output),
        ];
        let environment: Environment = [(OsString::from("TERM"), OsString::from("panelink-probe"))]
            .into_iter()
            .collect();
        let mut session = TerminalSession::spawn(
            &launch,
            &arguments,
            &environment,
            TerminalSize::new(12, 80).unwrap(),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            session.pump().unwrap();
            let snapshot = session.snapshot();
            let mut visible_text = String::new();
            for cell in snapshot.cells() {
                visible_text.push_str(cell.text());
            }
            let contains_output = visible_text.contains("Ask Codex to do anything")
                || visible_text.contains("Type a draft");
            if contains_output {
                let mut copy = ScreenSnapshot::try_new(TerminalSize::new(12, 80).unwrap()).unwrap();
                copy.copy_from(snapshot);
                return copy;
            }
            assert!(Instant::now() < deadline, "test PTY output was not parsed");
            std::thread::yield_now();
        }
    }

    #[test]
    fn codex_idle_requires_empty_provider_prompt() {
        assert_eq!(
            classify(AgentKind::Codex, "answer text\n›"),
            AgentState::Idle
        );
        assert_eq!(
            classify(AgentKind::Codex, "› draft text"),
            AgentState::Unknown
        );
        assert_eq!(classify(AgentKind::Codex, "›"), AgentState::Idle);
    }

    #[test]
    fn text_classification_keeps_nonempty_placeholder_unknown() {
        let screen = "Old answer\n\n› Ask Codex to do anything\n\n       ? for shortcuts       ";
        assert_eq!(classify(AgentKind::Codex, screen), AgentState::Unknown);
    }

    #[test]
    fn codex_placeholder_with_a_draft_or_without_footer_stays_unknown() {
        assert_eq!(
            classify(
                AgentKind::Codex,
                "› Ask Codex to do anything plus this draft\n? for shortcuts"
            ),
            AgentState::Unknown
        );
        assert_eq!(
            classify(AgentKind::Codex, "› Type your message"),
            AgentState::Unknown
        );
        assert_eq!(
            classify(
                AgentKind::Codex,
                "› Explain this failing test\n? for shortcuts"
            ),
            AgentState::Unknown
        );
    }

    #[test]
    fn ambiguous_activity_above_prompt_keeps_conservative_priority() {
        let screen = "Working…\napproval required in an older answer\n›";
        assert_eq!(classify(AgentKind::Codex, screen), AgentState::Blocked);
    }

    #[test]
    fn codex_working_and_blocked_markers_take_priority_over_prompt() {
        assert_eq!(
            classify(AgentKind::Codex, "›\nWorking…"),
            AgentState::Working
        );
        assert_eq!(
            classify(AgentKind::Codex, "›\nDo you want to run this command?"),
            AgentState::Blocked
        );
        assert_eq!(
            classify(AgentKind::Codex, "›\nanswer: approval required"),
            AgentState::Blocked
        );
    }

    #[test]
    fn snapshot_accepts_only_the_observed_dim_placeholder_and_trims_blank_rows() {
        let placeholder = parsed_snapshot(
            "\x1b[1m›\x1b[22m \x1b[2mAsk Codex to do anything\x1b[0m\r\n\r\nfixture model",
        );
        assert_eq!(
            classify_snapshot(AgentKind::Codex, &placeholder),
            AgentState::Idle
        );

        let typed = parsed_snapshot("\x1b[1m›\x1b[22m Type a draft\x1b[0m\r\nfixture model");
        assert_eq!(
            classify_snapshot(AgentKind::Codex, &typed),
            AgentState::Unknown
        );

        let non_dim =
            parsed_snapshot("\x1b[1m›\x1b[22m Ask Codex to do anything\x1b[0m\r\nfixture model");
        assert_eq!(
            classify_snapshot(AgentKind::Codex, &non_dim),
            AgentState::Unknown
        );
        assert_eq!(
            classify(
                AgentKind::Codex,
                "› Ask Codex to do anything\nfixture model"
            ),
            AgentState::Unknown
        );
    }

    #[test]
    fn snapshot_working_and_blocked_markers_above_prompt_keep_priority() {
        let working = parsed_snapshot(
            "Working…\r\n\x1b[1m›\x1b[22m \x1b[2mAsk Codex to do anything\x1b[0m\r\nfixture model",
        );
        assert_eq!(
            classify_snapshot(AgentKind::Codex, &working),
            AgentState::Working
        );

        let blocked = parsed_snapshot(
            "Approval required\r\n\x1b[1m›\x1b[22m \x1b[2mAsk Codex to do anything\x1b[0m\r\nfixture model",
        );
        assert_eq!(
            classify_snapshot(AgentKind::Codex, &blocked),
            AgentState::Blocked
        );
    }

    #[test]
    fn opencode_requires_supported_footer_and_empty_prompt() {
        assert_eq!(
            classify(AgentKind::OpenCode, "❯\nctrl+c exit"),
            AgentState::Idle
        );
        assert_eq!(classify(AgentKind::OpenCode, "❯"), AgentState::Unknown);
        assert_eq!(
            classify(AgentKind::OpenCode, "> draft\nctrl+c exit"),
            AgentState::Unknown
        );
        assert_eq!(
            classify(AgentKind::OpenCode, "❯\nctrl+c exit\nRunning tool"),
            AgentState::Working
        );
        assert_eq!(
            classify(AgentKind::OpenCode, "❯\nctrl+c exit\nPermission required"),
            AgentState::Blocked
        );
    }

    #[test]
    fn unknown_or_arbitrary_shell_text_never_looks_idle() {
        assert_eq!(classify(AgentKind::Unknown, "›"), AgentState::Unknown);
        assert_eq!(classify(AgentKind::Codex, "$ "), AgentState::Unknown);
        assert_eq!(
            classify(
                AgentKind::Codex,
                "The task is complete; here is the answer."
            ),
            AgentState::Unknown
        );
        assert_eq!(classify(AgentKind::OpenCode, "❯"), AgentState::Unknown);
        assert_eq!(
            classify(AgentKind::Codex, "Window title: Codex\n› text"),
            AgentState::Unknown
        );
    }

    #[test]
    fn searches_only_bounded_recent_rows_and_columns() {
        let old_marker = format!("{}\n›", "irrelevant\n".repeat(MAX_ROWS));
        assert_eq!(classify(AgentKind::Codex, &old_marker), AgentState::Idle);

        let far_right = format!("{}Working…", " ".repeat(MAX_COLUMNS));
        assert_eq!(classify(AgentKind::Codex, &far_right), AgentState::Unknown);
    }
    #[test]
    fn bare_prompt_above_multiline_draft_never_looks_idle() {
        assert_eq!(
            classify(AgentKind::Codex, "›\nunfinished draft"),
            AgentState::Unknown
        );
        let screen = parsed_snapshot("›\r\nType a draft\r\nfooter");
        assert_eq!(
            classify_snapshot(AgentKind::Codex, &screen),
            AgentState::Unknown
        );
    }
}
