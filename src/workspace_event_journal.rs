//! Bounded volatile workspace layout invalidations for one generation.
//!
//! This journal is independent of target-scoped pane events. It records only
//! layout revision and terminal-empty metadata, never layout contents or IO.

use crate::workspace_watch_protocol::{LayoutEvent, WorkspaceCursor};
use std::collections::VecDeque;
use std::error::Error;
use std::fmt;

/// Maximum number of workspace events retained by one journal.
pub const MAX_RETAINED_EVENTS: usize = 64;
/// Maximum logical bytes retained by this journal (24 bytes per event).
pub const MAX_RETAINED_ENCODED_BYTES: usize = MAX_RETAINED_EVENTS * 24;
const EVENT_BYTES: usize = 24;

/// Fixed failures returned by the workspace event journal.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum JournalError {
    /// Generation or workspace identity is zero.
    ZeroIdentifier,
    /// Cursor generation or workspace differs from this journal.
    ScopeMismatch,
    /// Cursor is older than the last evicted event.
    CursorExpired,
    /// Cursor sequence is ahead of the current journal sequence.
    FutureCursor,
    /// Read limit is outside 1 through 64.
    InvalidLimit,
    /// Sequence cannot advance without wrapping.
    SequenceExhausted,
    /// Layout revision must be nonzero.
    InvalidRevision,
}

impl fmt::Debug for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ZeroIdentifier => "ZeroIdentifier",
            Self::ScopeMismatch => "ScopeMismatch",
            Self::CursorExpired => "CursorExpired",
            Self::FutureCursor => "FutureCursor",
            Self::InvalidLimit => "InvalidLimit",
            Self::SequenceExhausted => "SequenceExhausted",
            Self::InvalidRevision => "InvalidRevision",
        })
    }
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("workspace event journal operation is invalid")
    }
}

impl Error for JournalError {}

/// A fixed-capacity workspace event history scoped to one generation/workspace.
pub struct WorkspaceEventJournal {
    generation: u64,
    workspace: u64,
    sequence: u64,
    evicted_through: u64,
    events: VecDeque<LayoutEvent>,
}

impl WorkspaceEventJournal {
    /// Create an empty journal at sequence zero for a nonzero scope.
    pub fn new(generation: u64, workspace: u64) -> Result<Self, JournalError> {
        if generation == 0 || workspace == 0 {
            return Err(JournalError::ZeroIdentifier);
        }
        Ok(Self {
            generation,
            workspace,
            sequence: 0,
            evicted_through: 0,
            events: VecDeque::with_capacity(MAX_RETAINED_EVENTS),
        })
    }

    /// Return the latest fact cursor; it does not imply delivery.
    pub const fn current_cursor(&self) -> WorkspaceCursor {
        WorkspaceCursor {
            generation: self.generation,
            workspace: self.workspace,
            sequence: self.sequence,
        }
    }

    /// Append one committed layout revision or terminal-empty transition.
    pub fn push(&mut self, revision: u64, empty: bool) -> Result<WorkspaceCursor, JournalError> {
        if revision == 0 {
            return Err(JournalError::InvalidRevision);
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(JournalError::SequenceExhausted)?;
        let event = LayoutEvent {
            sequence,
            revision,
            empty,
        };
        if self.events.len() == MAX_RETAINED_EVENTS {
            if let Some(evicted) = self.events.pop_front() {
                self.evicted_through = evicted.sequence;
            }
        }
        self.events.push_back(event);
        self.sequence = sequence;
        debug_assert_eq!(
            self.events.len() * EVENT_BYTES,
            self.retained_encoded_bytes()
        );
        Ok(self.current_cursor())
    }

    /// Read ordered events strictly after a cursor, up to `limit` records.
    pub fn read_after(
        &self,
        cursor: WorkspaceCursor,
        limit: u16,
    ) -> Result<Vec<LayoutEvent>, JournalError> {
        if cursor.generation != self.generation || cursor.workspace != self.workspace {
            return Err(JournalError::ScopeMismatch);
        }
        if cursor.sequence > self.sequence {
            return Err(JournalError::FutureCursor);
        }
        if !(1..=MAX_RETAINED_EVENTS as u16).contains(&limit) {
            return Err(JournalError::InvalidLimit);
        }
        if cursor.sequence < self.evicted_through {
            return Err(JournalError::CursorExpired);
        }
        Ok(self
            .events
            .iter()
            .filter(|event| event.sequence > cursor.sequence)
            .take(usize::from(limit))
            .copied()
            .collect())
    }

    /// Number of retained records.
    pub fn retained_len(&self) -> usize {
        self.events.len()
    }

    /// Logical bytes retained by the fixed 24-byte event representation.
    pub fn retained_encoded_bytes(&self) -> usize {
        self.events.len() * EVENT_BYTES
    }

    #[cfg(test)]
    fn exhaust_sequence_for_test(&mut self) {
        self.sequence = u64::MAX;
    }
}

impl fmt::Debug for WorkspaceEventJournal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkspaceEventJournal")
            .field("scope", &"<redacted>")
            .field("cursor", &"<redacted>")
            .field("retained_len", &self.events.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor(sequence: u64) -> WorkspaceCursor {
        WorkspaceCursor {
            generation: 2,
            workspace: 3,
            sequence,
        }
    }

    #[test]
    fn new_push_and_paged_reads_are_scoped_and_ordered() {
        let mut journal = WorkspaceEventJournal::new(2, 3).unwrap();
        assert_eq!(journal.current_cursor(), cursor(0));
        assert_eq!(journal.push(4, false), Ok(cursor(1)));
        assert_eq!(journal.push(5, true), Ok(cursor(2)));
        assert_eq!(
            journal.read_after(cursor(0), 1),
            Ok(vec![LayoutEvent {
                sequence: 1,
                revision: 4,
                empty: false
            }])
        );
        assert_eq!(
            journal.read_after(cursor(1), 64),
            Ok(vec![LayoutEvent {
                sequence: 2,
                revision: 5,
                empty: true
            }])
        );
        assert_eq!(journal.read_after(cursor(2), 1), Ok(vec![]));
        assert_eq!(journal.retained_len(), 2);
        assert_eq!(journal.retained_encoded_bytes(), 48);
    }

    #[test]
    fn rejects_zero_scope_bad_revision_wrong_scope_future_and_invalid_limit() {
        assert_eq!(
            WorkspaceEventJournal::new(0, 3).err(),
            Some(JournalError::ZeroIdentifier)
        );
        assert_eq!(
            WorkspaceEventJournal::new(2, 0).err(),
            Some(JournalError::ZeroIdentifier)
        );
        let mut journal = WorkspaceEventJournal::new(2, 3).unwrap();
        assert_eq!(journal.push(0, false), Err(JournalError::InvalidRevision));
        assert_eq!(journal.current_cursor(), cursor(0));
        assert_eq!(
            journal.read_after(
                WorkspaceCursor {
                    workspace: 8,
                    ..cursor(0)
                },
                1
            ),
            Err(JournalError::ScopeMismatch)
        );
        assert_eq!(
            journal.read_after(
                WorkspaceCursor {
                    generation: 8,
                    ..cursor(0)
                },
                1
            ),
            Err(JournalError::ScopeMismatch)
        );
        assert_eq!(
            journal.read_after(cursor(1), 1),
            Err(JournalError::FutureCursor)
        );
        assert_eq!(
            journal.read_after(cursor(0), 0),
            Err(JournalError::InvalidLimit)
        );
        assert_eq!(
            journal.read_after(cursor(0), 65),
            Err(JournalError::InvalidLimit)
        );
    }

    #[test]
    fn rollover_expires_only_cursors_before_last_evicted_sequence() {
        let mut journal = WorkspaceEventJournal::new(2, 3).unwrap();
        for revision in 1..=65 {
            journal.push(revision, false).unwrap();
        }
        assert_eq!(journal.retained_len(), MAX_RETAINED_EVENTS);
        assert_eq!(journal.retained_encoded_bytes(), MAX_RETAINED_ENCODED_BYTES);
        assert_eq!(journal.current_cursor(), cursor(65));
        assert_eq!(
            journal.read_after(cursor(0), 64),
            Err(JournalError::CursorExpired)
        );
        let page = journal.read_after(cursor(1), 64).unwrap();
        assert_eq!(page.len(), 64);
        assert_eq!(page[0].sequence, 2);
        assert_eq!(page[63].sequence, 65);
    }

    #[test]
    fn sequence_exhaustion_does_not_wrap_or_mutate_records() {
        let mut journal = WorkspaceEventJournal::new(2, 3).unwrap();
        journal.push(1, false).unwrap();
        journal.exhaust_sequence_for_test();
        let before = journal.retained_len();
        assert_eq!(journal.push(2, true), Err(JournalError::SequenceExhausted));
        assert_eq!(journal.retained_len(), before);
        assert_eq!(journal.current_cursor().sequence, u64::MAX);
    }

    #[test]
    fn debug_redacts_scope_cursor_and_event_metadata() {
        let mut journal = WorkspaceEventJournal::new(213, 321).unwrap();
        journal.push(456, true).unwrap();
        let text = format!("{:?} {:?}", journal, journal.current_cursor());
        assert!(!text.contains("213"));
        assert!(!text.contains("321"));
        assert!(!text.contains("456"));
    }
}
