//! Bounded, volatile metadata event history scoped to one server generation.
//!
//! The journal assigns monotonically increasing cursors and retains only the
//! newest [`MAX_RETAINED_EVENTS`] validated events. It does not dispatch,
//! persist, coalesce, or resynchronize events.

use crate::control_protocol::TargetRef;
use crate::event_protocol::{Event, EventBody, EVENT_PAYLOAD_BYTES};
use std::collections::VecDeque;
use std::error::Error;
use std::fmt;

/// Maximum number of events retained by one journal.
pub const MAX_RETAINED_EVENTS: usize = 64;

const EVENT_FRAME_HEADER_BYTES: usize = 24;
const EVENT_FRAME_BYTES: usize = EVENT_PAYLOAD_BYTES + EVENT_FRAME_HEADER_BYTES;

/// Maximum logical encoded size of retained events, including one frame
/// header for each event payload.
pub const MAX_RETAINED_ENCODED_BYTES: usize = MAX_RETAINED_EVENTS * EVENT_FRAME_BYTES;

/// A cursor within one server-start generation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct EventCursor {
    /// Server-start generation to which this cursor belongs.
    pub generation: u64,
    /// Event sequence within the generation. Zero is the initial cursor.
    pub sequence: u64,
}

impl fmt::Debug for EventCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventCursor").finish_non_exhaustive()
    }
}

/// Fixed, redacted failures returned by the metadata event journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalError {
    /// A journal generation must be nonzero.
    ZeroGeneration,
    /// A target belongs to a different server generation.
    GenerationMismatch,
    /// The cursor predates the last event evicted from the journal.
    CursorExpired,
    /// The cursor is ahead of the journal's current sequence.
    FutureCursor,
    /// A read limit must be between one and [`MAX_RETAINED_EVENTS`].
    InvalidLimit,
    /// The event sequence cannot be incremented without wrapping.
    SequenceExhausted,
    /// The event body or target failed the N1 event contract.
    InvalidEvent,
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ZeroGeneration => "event journal generation must be nonzero",
            Self::GenerationMismatch => "event journal generation does not match",
            Self::CursorExpired => "event cursor has expired",
            Self::FutureCursor => "event cursor is ahead of the journal",
            Self::InvalidLimit => "event read limit is outside its supported range",
            Self::SequenceExhausted => "event sequence is exhausted",
            Self::InvalidEvent => "event is invalid",
        };
        f.write_str(message)
    }
}

impl Error for JournalError {}

/// A bounded journal of validated metadata events for one generation.
pub struct EventJournal {
    generation: u64,
    sequence: u64,
    evicted_through: u64,
    events: VecDeque<Event>,
}

impl EventJournal {
    /// Create an empty journal at sequence zero for a nonzero generation.
    pub fn new(generation: u64) -> Result<Self, JournalError> {
        if generation == 0 {
            return Err(JournalError::ZeroGeneration);
        }

        Ok(Self {
            generation,
            sequence: 0,
            evicted_through: 0,
            events: VecDeque::with_capacity(MAX_RETAINED_EVENTS),
        })
    }

    /// Return the latest fact cursor. This cursor does not imply delivery or
    /// completion of any event.
    pub const fn current_cursor(&self) -> EventCursor {
        EventCursor {
            generation: self.generation,
            sequence: self.sequence,
        }
    }

    /// Validate and append one event, assigning its next sequence.
    ///
    /// Validation and checked sequence allocation happen before the journal is
    /// mutated. Once full, the oldest event is evicted before the new event is
    /// appended, so the retained count never exceeds the configured bound.
    pub fn push(
        &mut self,
        target: TargetRef,
        body: EventBody,
    ) -> Result<EventCursor, JournalError> {
        if target.generation != self.generation {
            return Err(JournalError::GenerationMismatch);
        }

        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(JournalError::SequenceExhausted)?;
        let event = Event {
            cursor: sequence,
            target,
            body,
        };
        event.validate().map_err(|_| JournalError::InvalidEvent)?;

        if self.events.len() == MAX_RETAINED_EVENTS {
            if let Some(evicted) = self.events.pop_front() {
                self.evicted_through = evicted.cursor;
            }
        }
        self.events.push_back(event);
        self.sequence = sequence;

        Ok(EventCursor {
            generation: self.generation,
            sequence,
        })
    }

    /// Read up to `limit` earliest retained events strictly after `cursor`.
    ///
    /// A cursor equal to the last evicted sequence remains valid because the
    /// following sequence is still the first event available. A cursor older
    /// than that boundary fails closed with [`JournalError::CursorExpired`];
    /// callers must reconcile authoritative state before resuming. The returned
    /// vector is only a page and does not represent the journal's current state.
    pub fn read_after(
        &self,
        cursor: EventCursor,
        limit: usize,
    ) -> Result<Vec<Event>, JournalError> {
        if cursor.generation != self.generation {
            return Err(JournalError::GenerationMismatch);
        }
        if cursor.sequence > self.sequence {
            return Err(JournalError::FutureCursor);
        }
        if !(1..=MAX_RETAINED_EVENTS).contains(&limit) {
            return Err(JournalError::InvalidLimit);
        }
        if cursor.sequence < self.evicted_through {
            return Err(JournalError::CursorExpired);
        }

        Ok(self
            .events
            .iter()
            .filter(|event| event.cursor > cursor.sequence)
            .take(limit)
            .copied()
            .collect())
    }

    /// Return the number of retained events.
    pub fn retained_len(&self) -> usize {
        self.events.len()
    }

    /// Return retained logical encoded bytes, including one frame header per
    /// event. This is accounting for the wire representation, not an RSS bound.
    pub fn retained_encoded_bytes(&self) -> usize {
        self.events.len() * EVENT_FRAME_BYTES
    }

    #[cfg(test)]
    pub(crate) fn exhaust_sequence_for_test(&mut self) {
        self.sequence = u64::MAX;
    }
}

impl fmt::Debug for EventJournal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventJournal")
            .field("generation", &"[redacted]")
            .field("cursor", &"[redacted]")
            .field("retained_len", &self.events.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(generation: u64, workspace: u64) -> TargetRef {
        TargetRef {
            generation,
            workspace,
            pane: 2,
            session: 3,
            occupant_revision: 4,
        }
    }

    fn body(revision: u64) -> EventBody {
        EventBody::ScreenChanged { revision }
    }

    fn push(journal: &mut EventJournal, sequence: u64) -> EventCursor {
        journal
            .push(target(journal.generation, sequence + 1), body(sequence + 1))
            .expect("valid event")
    }

    #[test]
    fn new_requires_nonzero_generation_and_starts_at_sequence_zero() {
        assert_eq!(
            EventJournal::new(0).unwrap_err(),
            JournalError::ZeroGeneration
        );

        let journal = EventJournal::new(9).expect("nonzero generation");
        assert_eq!(
            journal.current_cursor(),
            EventCursor {
                generation: 9,
                sequence: 0,
            }
        );
        assert_eq!(journal.retained_len(), 0);
        assert_eq!(journal.retained_encoded_bytes(), 0);
    }

    #[test]
    fn retains_exactly_sixty_four_and_evicts_oldest_before_the_sixty_fifth() {
        let mut journal = EventJournal::new(9).expect("nonzero generation");
        for sequence in 1..=MAX_RETAINED_EVENTS as u64 {
            assert_eq!(push(&mut journal, sequence - 1).sequence, sequence);
        }
        assert_eq!(journal.retained_len(), 64);
        assert_eq!(journal.retained_encoded_bytes(), 64 * (72 + 24));
        assert_eq!(journal.retained_encoded_bytes(), MAX_RETAINED_ENCODED_BYTES);

        let sixty_fifth = push(&mut journal, 64);
        assert_eq!(sixty_fifth.sequence, 65);
        assert_eq!(journal.retained_len(), 64);
        assert_eq!(journal.retained_encoded_bytes(), MAX_RETAINED_ENCODED_BYTES);

        let retained = journal
            .read_after(
                EventCursor {
                    generation: 9,
                    sequence: 1,
                },
                MAX_RETAINED_EVENTS,
            )
            .expect("last-evicted cursor remains valid");
        assert_eq!(retained.len(), 64);
        assert_eq!(retained.first().expect("first retained").cursor, 2);
        assert_eq!(retained.last().expect("last retained").cursor, 65);
    }

    #[test]
    fn cursor_expiry_boundary_is_the_last_evicted_sequence() {
        let mut journal = EventJournal::new(9).expect("nonzero generation");
        for sequence in 1..=65 {
            push(&mut journal, sequence - 1);
        }

        assert_eq!(
            journal.read_after(
                EventCursor {
                    generation: 9,
                    sequence: 0,
                },
                1,
            ),
            Err(JournalError::CursorExpired)
        );
        assert_eq!(
            journal
                .read_after(
                    EventCursor {
                        generation: 9,
                        sequence: 1,
                    },
                    1,
                )
                .expect("boundary is valid")[0]
                .cursor,
            2
        );
    }

    #[test]
    fn reads_are_ordered_paged_and_empty_after_the_latest_cursor() {
        let mut journal = EventJournal::new(9).expect("nonzero generation");
        for sequence in 1..=5 {
            push(&mut journal, sequence - 1);
        }

        let first = journal
            .read_after(
                EventCursor {
                    generation: 9,
                    sequence: 0,
                },
                2,
            )
            .expect("first page");
        assert_eq!(
            first.iter().map(|event| event.cursor).collect::<Vec<_>>(),
            [1, 2]
        );
        let second = journal
            .read_after(
                EventCursor {
                    generation: 9,
                    sequence: first.last().expect("page item").cursor,
                },
                2,
            )
            .expect("second page");
        assert_eq!(
            second.iter().map(|event| event.cursor).collect::<Vec<_>>(),
            [3, 4]
        );
        let last = journal
            .read_after(
                EventCursor {
                    generation: 9,
                    sequence: second.last().expect("page item").cursor,
                },
                2,
            )
            .expect("last page");
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].cursor, 5);
        assert!(journal
            .read_after(journal.current_cursor(), 1)
            .expect("empty current page")
            .is_empty());
    }

    #[test]
    fn rejects_wrong_generation_future_cursor_and_invalid_limits() {
        let journal = EventJournal::new(9).expect("nonzero generation");
        assert_eq!(
            journal.read_after(
                EventCursor {
                    generation: 10,
                    sequence: 0,
                },
                1,
            ),
            Err(JournalError::GenerationMismatch)
        );
        assert_eq!(
            journal.read_after(
                EventCursor {
                    generation: 9,
                    sequence: 1,
                },
                1,
            ),
            Err(JournalError::FutureCursor)
        );
        for limit in [0, MAX_RETAINED_EVENTS + 1] {
            assert_eq!(
                journal.read_after(journal.current_cursor(), limit),
                Err(JournalError::InvalidLimit)
            );
        }
    }

    #[test]
    fn invalid_pushes_do_not_mutate_journal() {
        let mut journal = EventJournal::new(9).expect("nonzero generation");
        for sequence in 1..=MAX_RETAINED_EVENTS as u64 {
            push(&mut journal, sequence - 1);
        }
        let before = journal.current_cursor();

        assert_eq!(
            journal.push(target(10, 1), body(1)),
            Err(JournalError::GenerationMismatch)
        );
        assert_eq!(
            journal.push(target(9, 0), body(1)),
            Err(JournalError::InvalidEvent)
        );
        assert_eq!(
            journal.push(target(9, 1), body(0)),
            Err(JournalError::InvalidEvent)
        );
        assert_eq!(journal.current_cursor(), before);
        assert_eq!(journal.retained_len(), MAX_RETAINED_EVENTS);
        assert_eq!(journal.retained_encoded_bytes(), MAX_RETAINED_ENCODED_BYTES);
        assert_eq!(
            journal
                .read_after(
                    EventCursor {
                        generation: 9,
                        sequence: 0,
                    },
                    MAX_RETAINED_EVENTS,
                )
                .expect("invalid push left the cursor boundary intact")[0]
                .cursor,
            1
        );
    }

    #[test]
    fn sequence_exhaustion_does_not_wrap_or_mutate() {
        let mut journal = EventJournal::new(9).expect("nonzero generation");
        journal.sequence = u64::MAX;
        let before = journal.current_cursor();

        assert_eq!(
            journal.push(target(9, 1), body(1)),
            Err(JournalError::SequenceExhausted)
        );
        assert_eq!(journal.current_cursor(), before);
        assert_eq!(journal.retained_len(), 0);
        assert_eq!(journal.retained_encoded_bytes(), 0);
    }

    #[test]
    fn debug_output_does_not_expose_cursor_or_event_identifiers() {
        let cursor = EventCursor {
            generation: 91_337,
            sequence: 81_229,
        };
        assert!(!format!("{cursor:?}").contains("91337"));
        assert!(!format!("{cursor:?}").contains("81229"));

        let mut journal = EventJournal::new(91_337).expect("nonzero generation");
        journal
            .push(target(91_337, 71_111), body(61_003))
            .expect("valid event");
        let debug = format!("{journal:?}");
        for canary in ["91337", "71111", "61003"] {
            assert!(!debug.contains(canary));
        }
    }
}
