use std::fmt;

use crate::event::{Event, InternalEvent};
use crate::parse;

const ORDINARY_SEQUENCE_LIMIT: usize = 16_384;
const PASTE_SEQUENCE_LIMIT: usize = 65_548;
const PASTE_OPENER: &[u8] = b"\x1b[200~";

/// A fixed-storage stream decoder around Crossterm's Unix input parser.
///
/// The caller must enable raw mode before feeding bytes and must stop input and
/// restore the terminal after any error. This type performs no host IO.
pub struct Decoder {
    buffer: Box<[u8; PASTE_SEQUENCE_LIMIT]>,
    pending: usize,
    failed: bool,
}

/// A malformed or over-limit input sequence. The error carries no input bytes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DecodeError;

impl fmt::Debug for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DecodeError")
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("input decoding failed")
    }
}

impl std::error::Error for DecodeError {}

impl fmt::Debug for Decoder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Decoder")
            .field("pending_bytes", &self.pending)
            .field("failed", &self.failed)
            .finish()
    }
}

impl Decoder {
    /// Allocate the decoder's fixed maximum input buffer.
    pub fn new() -> Self {
        Self {
            buffer: Box::new([0; PASTE_SEQUENCE_LIMIT]),
            pending: 0,
            failed: false,
        }
    }

    /// Feed one raw input byte. At most one user-facing event is returned.
    ///
    /// A parser error permanently poisons this decoder. The caller must stop
    /// the probe and restore the terminal rather than feeding any suffix.
    pub fn advance(
        &mut self,
        byte: u8,
        input_available: bool,
    ) -> Result<Option<Event>, DecodeError> {
        if self.failed {
            return Err(DecodeError);
        }

        let limit = self.sequence_limit();
        if self.pending == limit {
            return Err(self.poison());
        }

        self.buffer[self.pending] = byte;
        self.pending += 1;

        match parse::parse_event(&self.buffer[..self.pending], input_available) {
            Ok(Some(InternalEvent::Event(event))) => {
                self.clear_pending();
                Ok(Some(event))
            }
            Ok(Some(_)) => {
                self.clear_pending();
                Ok(None)
            }
            Ok(None) => Ok(None),
            Err(_) => Err(self.poison()),
        }
    }

    /// Resolve a pending single ESC after the caller's escape timeout.
    ///
    /// Incomplete CSI and paste sequences remain pending and are not flushed.
    pub fn finish_escape(&mut self) -> Result<Option<Event>, DecodeError> {
        if self.failed {
            return Err(DecodeError);
        }
        if self.pending != 1 || self.buffer[0] != b'\x1b' {
            return Ok(None);
        }

        match parse::parse_event(&self.buffer[..self.pending], false) {
            Ok(Some(InternalEvent::Event(event))) => {
                self.clear_pending();
                Ok(Some(event))
            }
            Ok(Some(_)) | Ok(None) => {
                self.clear_pending();
                Ok(None)
            }
            Err(_) => Err(self.poison()),
        }
    }

    /// Number of buffered, incomplete input bytes, without exposing their contents.
    pub fn pending_bytes(&self) -> usize {
        self.pending
    }

    /// Whether malformed or over-limit input has permanently poisoned this decoder.
    pub fn is_failed(&self) -> bool {
        self.failed
    }

    fn sequence_limit(&self) -> usize {
        if self.buffer[..self.pending].starts_with(PASTE_OPENER) {
            PASTE_SEQUENCE_LIMIT
        } else {
            ORDINARY_SEQUENCE_LIMIT
        }
    }

    fn poison(&mut self) -> DecodeError {
        self.clear_pending();
        self.failed = true;
        DecodeError
    }

    fn clear_pending(&mut self) {
        self.buffer[..self.pending].fill(0);
        self.pending = 0;
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    fn feed(decoder: &mut Decoder, bytes: &[u8]) -> Result<Vec<Event>, DecodeError> {
        let mut events = Vec::new();
        for &byte in bytes {
            if let Some(event) = decoder.advance(byte, true)? {
                events.push(event);
            }
        }
        Ok(events)
    }

    #[test]
    fn decodes_utf8_and_arrow_across_byte_boundaries_once() {
        let mut decoder = Decoder::new();
        assert!(
            feed(&mut decoder, "Ž".as_bytes()).unwrap().as_slice()
                == [Event::Key(KeyEvent::new(
                    KeyCode::Char('Ž'),
                    KeyModifiers::SHIFT,
                ))]
        );
        assert_eq!(feed(&mut decoder, b"\x1b[").unwrap(), Vec::<Event>::new());
        assert_eq!(
            feed(&mut decoder, b"A").unwrap(),
            [Event::Key(KeyCode::Up.into())]
        );
        assert_eq!(decoder.pending_bytes(), 0);
    }

    #[test]
    fn finish_escape_resolves_only_single_escape() {
        let mut decoder = Decoder::new();
        assert_eq!(decoder.advance(b'\x1b', true), Ok(None));
        assert_eq!(
            decoder.finish_escape(),
            Ok(Some(Event::Key(KeyCode::Esc.into())))
        );
        assert_eq!(decoder.pending_bytes(), 0);

        assert_eq!(decoder.advance(b'\x1b', true), Ok(None));
        assert_eq!(decoder.advance(b'[', true), Ok(None));
        assert_eq!(decoder.advance(b'1', true), Ok(None));
        let pending = decoder.pending_bytes();
        assert_eq!(decoder.finish_escape(), Ok(None));
        assert_eq!(decoder.pending_bytes(), pending);
        assert!(!decoder.is_failed());
    }

    #[test]
    fn paste_delimiters_may_be_split_at_every_boundary() {
        let opener = b"\x1b[200~";
        let closer = b"\x1b[201~";
        for opener_split in 0..=opener.len() {
            for closer_split in 0..=closer.len() {
                let mut decoder = Decoder::new();
                let mut events = Vec::new();
                for chunk in [
                    &opener[..opener_split],
                    &opener[opener_split..],
                    b"hello",
                    &closer[..closer_split],
                    &closer[closer_split..],
                ] {
                    events.extend(feed(&mut decoder, chunk).unwrap());
                }
                assert_eq!(events, [Event::Paste("hello".to_owned())]);
                assert_eq!(decoder.pending_bytes(), 0);
                assert_eq!(
                    decoder.advance(b'x', true),
                    Ok(Some(Event::Key(KeyCode::Char('x').into())))
                );
            }
        }
    }

    #[test]
    fn accepts_maximum_paste_payload_and_rejects_first_excess_byte() {
        let mut decoder = Decoder::new();
        assert!(feed(&mut decoder, b"\x1b[200~").unwrap().is_empty());
        assert!(feed(&mut decoder, &vec![b'x'; 65_536]).unwrap().is_empty());
        let events = feed(&mut decoder, b"\x1b[201~").unwrap();
        assert_eq!(events.len(), 1);
        let Event::Paste(paste) = &events[0] else {
            panic!("expected one paste event");
        };
        assert_eq!(paste.len(), 65_536);
        assert!(paste.bytes().all(|byte| byte == b'x'));
        assert_eq!(decoder.pending_bytes(), 0);

        let mut decoder = Decoder::new();
        assert!(feed(&mut decoder, b"\x1b[200~").unwrap().is_empty());
        assert!(feed(&mut decoder, &vec![b'x'; 65_537]).unwrap().is_empty());
        assert!(feed(&mut decoder, b"\x1b[201~"[..5].as_ref())
            .unwrap()
            .is_empty());
        assert_eq!(decoder.pending_bytes(), 65_548);
        assert_eq!(decoder.advance(b'~', true), Err(DecodeError));
        assert_eq!(decoder.pending_bytes(), 0);
        assert!(decoder.is_failed());
        for byte in b"\x1b[A" {
            assert_eq!(decoder.advance(*byte, true), Err(DecodeError));
        }
    }

    #[test]
    fn ordinary_sequence_ceiling_is_checked_before_store() {
        let mut decoder = Decoder::new();
        assert!(feed(&mut decoder, b"\x1b[").unwrap().is_empty());
        assert!(feed(&mut decoder, &vec![b'1'; 16_382]).unwrap().is_empty());
        assert_eq!(decoder.pending_bytes(), 16_384);
        assert_eq!(decoder.advance(b'1', true), Err(DecodeError));
        assert_eq!(decoder.pending_bytes(), 0);
        assert!(decoder.is_failed());
    }

    #[test]
    fn invalid_utf8_and_malformed_control_poison_without_suffix_events() {
        let mut decoder = Decoder::new();
        assert!(feed(&mut decoder, b"\x1b[200~").unwrap().is_empty());
        assert!(feed(&mut decoder, &[0xff]).unwrap().is_empty());
        assert_eq!(decoder.advance(b'\x1b', true), Ok(None));
        assert_eq!(decoder.advance(b'[', true), Ok(None));
        assert_eq!(decoder.advance(b'2', true), Ok(None));
        assert_eq!(decoder.advance(b'0', true), Ok(None));
        assert_eq!(decoder.advance(b'1', true), Ok(None));
        assert_eq!(decoder.advance(b'~', true), Err(DecodeError));
        assert_eq!(decoder.advance(b'\x1b', true), Err(DecodeError));

        let mut malformed = Decoder::new();
        assert_eq!(malformed.advance(b'\x1b', true), Ok(None));
        assert_eq!(malformed.advance(b'[', true), Ok(None));
        assert_eq!(malformed.advance(b'!', true), Err(DecodeError));
        assert_eq!(malformed.advance(b'a', true), Err(DecodeError));
    }

    #[test]
    fn zero_terminal_coordinates_poison_without_panicking() {
        for malformed in [
            b"\x1b[0;1R".as_slice(),
            b"\x1b[32;0;1;M",
            b"\x1b[M  !",
            b"\x1b[<0;0;1M",
        ] {
            let mut decoder = Decoder::new();
            for &byte in &malformed[..malformed.len() - 1] {
                assert_eq!(decoder.advance(byte, true), Ok(None));
            }
            assert_eq!(
                decoder.advance(*malformed.last().unwrap(), true),
                Err(DecodeError)
            );
            assert!(decoder.is_failed());
            assert_eq!(decoder.advance(b'a', true), Err(DecodeError));
        }
    }

    #[test]
    fn internal_query_responses_are_dropped_without_queueing() {
        let mut decoder = Decoder::new();
        for response in [b"\x1b[20;10R".as_slice(), b"\x1b[?1u", b"\x1b[?1c"] {
            assert!(feed(&mut decoder, response).unwrap().is_empty());
            assert_eq!(decoder.pending_bytes(), 0);
        }
        assert_eq!(
            decoder.advance(b'a', true),
            Ok(Some(Event::Key(KeyCode::Char('a').into())))
        );
        assert_eq!(
            decoder.advance(b'b', true),
            Ok(Some(Event::Key(KeyCode::Char('b').into())))
        );
    }

    #[test]
    fn debug_output_redacts_event_payloads_and_pending_bytes() {
        let secret = "distinctive-user-secret";
        let key = Event::Key(KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::NONE));
        let paste = Event::Paste(secret.to_owned());
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 123,
            row: 456,
            modifiers: KeyModifiers::NONE,
        };
        let mouse_event = Event::Mouse(mouse);
        assert!(!format!("{key:?}").contains('Q'));
        assert!(!format!("{:?}", key.as_key_event().unwrap().code).contains('Q'));
        assert!(!format!("{:?}", key.as_key_event().unwrap()).contains('Q'));
        assert!(!format!("{paste:?}").contains(secret));
        assert!(!format!("{mouse_event:?}").contains("123"));
        assert!(!format!("{mouse_event:?}").contains("456"));
        assert!(!format!("{mouse:?}").contains("123"));
        assert!(!format!("{mouse:?}").contains("456"));

        let mut decoder = Decoder::new();
        feed(&mut decoder, b"\x1b[200~distinctive-user-secret").unwrap();
        assert!(!format!("{decoder:?}").contains(secret));
        let mut failed = Decoder::new();
        assert_eq!(failed.advance(b'\x1b', true), Ok(None));
        assert_eq!(failed.advance(b'[', true), Ok(None));
        assert_eq!(failed.advance(b'!', true), Err(DecodeError));
        assert!(!format!("{failed:?}").contains(secret));
        let error = DecodeError;
        assert!(!format!("{error:?} {error}").contains(secret));
    }
}
