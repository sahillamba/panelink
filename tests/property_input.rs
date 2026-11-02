//! Property-based tests for the bounded terminal-input decoder.
//!
//! Packet 2 of `docs/design/TRACING-PROPTEST-PLAN.md`. These tests call the
//! shipped `crossterm-input` crate from the root integration target, so they
//! exercise exactly the component the client uses. They never open a terminal,
//! sleep, or drive a PTY: every case is a pure in-memory byte schedule.
//!
//! Covered contracts:
//! * exact pending-byte accounting up to both documented sequence ceilings;
//! * sticky poison with no events or state change afterwards;
//! * bracketed-paste UTF-8 validation and exact content preservation;
//! * ESC availability/timeout semantics, compared only where they are
//!   equivalent, with the ESC/Alt ambiguity tested explicitly.
//!
//! Failures persist to `proptest-regressions/property_input.txt` (a reviewed,
//! versioned file). Re-running this test target replays every persisted case
//! before generating new ones. `PROPTEST_CASES` may raise the case count for an
//! opt-in local stress run; the committed default stays at 256.

use crossterm_input::{
    DecodeError, Decoder, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

/// Mirrors the decoder's documented paste ceiling (`PASTE_SEQUENCE_LIMIT`).
const PASTE_SEQUENCE_LIMIT: usize = 65_548;
/// Mirrors the decoder's documented ordinary-sequence ceiling.
const ORDINARY_SEQUENCE_LIMIT: usize = 16_384;
/// Longest random buffered prefix used by a trial.
const MAX_RANDOM_PREFIX: usize = 256;
/// Longest random token stream used by a trial.
const MAX_RANDOM_TOKENS: usize = 8;

fn config() -> ProptestConfig {
    ProptestConfig {
        // Default is 256; honor PROPTEST_CASES for opt-in larger runs.
        max_shrink_iters: 4_096,
        // Failures persist to a reviewed, versioned file under the crate-root
        // `proptest-regressions/` directory. Replay: re-run the same test target;
        // proptest re-executes every persisted case before new random cases.
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "proptest-regressions/property_input.txt",
        ))),
        ..ProptestConfig::default()
    }
}

fn esc_event() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
}

fn feed(decoder: &mut Decoder, bytes: &[u8], available: bool) -> Result<Vec<Event>, DecodeError> {
    let mut events = Vec::new();
    for &byte in bytes {
        if let Some(event) = decoder.advance(byte, available)? {
            events.push(event);
        }
    }
    Ok(events)
}

/// Paste bodies may contain any scalar except ESC, which would be ambiguous
/// with the closing delimiter. Everything else, including controls, must be
/// preserved byte for byte.
fn arb_paste_text() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..=32).prop_map(|characters| {
        characters
            .into_iter()
            .filter(|character| *character != '\u{1b}')
            .collect()
    })
}

/// One well-formed input token with its byte encoding and expected event.
#[derive(Clone, Debug)]
enum Token {
    Char(char),
    Up,
    Home,
    Mouse { column: u16, row: u16 },
    Paste(String),
}

fn arb_token() -> impl Strategy<Value = Token> {
    prop_oneof![
        (0x20u8..=0x7e).prop_map(|byte| Token::Char(byte as char)),
        Just(Token::Up),
        Just(Token::Home),
        (1u16..=200, 1u16..=200).prop_map(|(column, row)| Token::Mouse { column, row }),
        arb_paste_text().prop_map(Token::Paste),
    ]
}

impl Token {
    fn bytes(&self) -> Vec<u8> {
        match self {
            Token::Char(character) => {
                let mut buffer = [0_u8; 4];
                character.encode_utf8(&mut buffer).as_bytes().to_vec()
            }
            Token::Up => b"\x1b[A".to_vec(),
            Token::Home => b"\x1b[H".to_vec(),
            Token::Mouse { column, row } => format!("\x1b[<0;{column};{row}M").into_bytes(),
            Token::Paste(text) => {
                let mut bytes = b"\x1b[200~".to_vec();
                bytes.extend_from_slice(text.as_bytes());
                bytes.extend_from_slice(b"\x1b[201~");
                bytes
            }
        }
    }

    fn event(&self) -> Event {
        match self {
            Token::Char(character) => Event::Key(KeyEvent::new(
                KeyCode::Char(*character),
                if character.is_uppercase() {
                    KeyModifiers::SHIFT
                } else {
                    KeyModifiers::NONE
                },
            )),
            Token::Up => Event::Key(KeyCode::Up.into()),
            Token::Home => Event::Key(KeyCode::Home.into()),
            Token::Mouse { column, row } => Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: column - 1,
                row: row - 1,
                modifiers: KeyModifiers::NONE,
            }),
            Token::Paste(text) => Event::Paste(text.clone()),
        }
    }
}

fn arb_token_stream() -> impl Strategy<Value = Vec<Token>> {
    prop::collection::vec(arb_token(), 0..=MAX_RANDOM_TOKENS)
}

/// Malformed inputs that must permanently poison the decoder. Each ends with
/// the byte that triggers the failure.
const POISONING_INPUTS: &[&[u8]] = &[
    // Invalid CSI intermediate byte.
    b"\x1b[!",
    // Zero terminal coordinates in several mouse/cursor report encodings.
    b"\x1b[0;1R",
    b"\x1b[32;0;1;M",
    b"\x1b[M  !",
    b"\x1b[<0;0;1M",
    // Invalid UTF-8 closed paste payload.
    b"\x1b[200~\xff\x1b[201~",
    // Unsupported SS3 final byte.
    b"\x1bOX",
];

proptest! {
    #![proptest_config(config())]

    /// Printable ASCII decodes to exactly one key event per byte, with no
    /// residue and no failure.
    #[test]
    fn printable_ascii_decodes_one_key_event_per_byte(text in "[ -~]{0,48}") {
        let mut decoder = Decoder::new();
        let events = feed(&mut decoder, text.as_bytes(), true).expect("printable ASCII decodes");
        prop_assert_eq!(events.len(), text.chars().count());
        prop_assert_eq!(decoder.pending_bytes(), 0);
        prop_assert!(!decoder.is_failed());
        for (event, character) in events.iter().zip(text.chars()) {
            let key = event.as_key_event().expect("printable ASCII yields key events");
            prop_assert_eq!(key.code, KeyCode::Char(character));
            prop_assert_eq!(
                key.modifiers,
                if character.is_uppercase() {
                    KeyModifiers::SHIFT
                } else {
                    KeyModifiers::NONE
                }
            );
        }
    }

    /// Paste content is preserved exactly, including control scalars and
    /// multi-byte UTF-8, and the pending buffer empties afterwards.
    #[test]
    fn paste_content_round_trips_exactly(text in arb_paste_text()) {
        let mut decoder = Decoder::new();
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        let events = feed(&mut decoder, &bytes, true).expect("valid paste decodes");
        prop_assert_eq!(events, vec![Event::Paste(text)]);
        prop_assert_eq!(decoder.pending_bytes(), 0);
        prop_assert!(!decoder.is_failed());
    }

    /// While a paste body is incomplete, every buffered byte is counted
    /// exactly and the timeout resolver never flushes a partial sequence.
    #[test]
    fn incomplete_paste_pending_bytes_are_exact(
        filler in prop::collection::vec(0x20u8..=0x7e, 0..=MAX_RANDOM_PREFIX)
    ) {
        let mut decoder = Decoder::new();
        prop_assert_eq!(feed(&mut decoder, b"\x1b[200~", true), Ok(Vec::new()));
        prop_assert_eq!(decoder.pending_bytes(), 6);
        for (offset, byte) in filler.iter().enumerate() {
            prop_assert_eq!(decoder.advance(*byte, true), Ok(None));
            prop_assert_eq!(decoder.pending_bytes(), 6 + offset + 1);
            prop_assert!(!decoder.is_failed());
        }
        prop_assert_eq!(decoder.finish_escape(), Ok(None));
        prop_assert_eq!(decoder.pending_bytes(), 6 + filler.len());
        prop_assert_eq!(decoder.finish_escape(), Ok(None));
        prop_assert!(!decoder.is_failed());
        prop_assert_eq!(
            decoder.advance(b'x', true),
            Ok(None),
            "the sequence stays pending until its closer arrives"
        );
    }

    /// Ordinary CSI prefixes are counted exactly and stay pending.
    #[test]
    fn incomplete_ordinary_sequence_pending_bytes_are_exact(
        digits in prop::collection::vec(0x30u8..=0x39, 0..=MAX_RANDOM_PREFIX)
    ) {
        let mut decoder = Decoder::new();
        prop_assert_eq!(decoder.advance(b'\x1b', true), Ok(None));
        prop_assert_eq!(decoder.pending_bytes(), 1);
        prop_assert_eq!(decoder.advance(b'[', true), Ok(None));
        for (offset, byte) in digits.iter().enumerate() {
            prop_assert_eq!(decoder.advance(*byte, true), Ok(None));
            prop_assert_eq!(decoder.pending_bytes(), 2 + offset + 1);
        }
        prop_assert!(!decoder.is_failed());
        prop_assert_eq!(decoder.finish_escape(), Ok(None));
        prop_assert_eq!(decoder.pending_bytes(), 2 + digits.len());
        prop_assert!(!decoder.is_failed());
    }

    /// ESC followed by an ordinary byte is an Alt chord, not Escape, and the
    /// same two bytes never produce two events.
    #[test]
    fn escape_prefix_forms_an_alt_chord(byte in 0x20u8..=0x7e) {
        // ESC '[' and ESC 'O' are prefix bytes of compound sequences instead.
        prop_assume!(byte != b'[' && byte != b'O');
        let mut decoder = Decoder::new();
        prop_assert_eq!(decoder.advance(b'\x1b', true), Ok(None));
        prop_assert_eq!(decoder.pending_bytes(), 1);
        let event = decoder
            .advance(byte, true)
            .expect("alt chord decodes")
            .expect("alt chord produces one event");
        prop_assert_eq!(decoder.pending_bytes(), 0);
        let key = event.as_key_event().expect("alt chord is a key event");
        prop_assert_eq!(key.code, KeyCode::Char(byte as char));
        prop_assert!(key.modifiers.contains(KeyModifiers::ALT));
        prop_assert_eq!(decoder.advance(byte, true), Ok(Some(Event::Key(
            KeyEvent::new(
                KeyCode::Char(byte as char),
                if (byte as char).is_uppercase() {
                    KeyModifiers::SHIFT
                } else {
                    KeyModifiers::NONE
                },
            )
        ))));
    }

    /// One poisoning input permanently fails the decoder: no further byte can
    /// produce an event or change the (empty) pending buffer.
    #[test]
    fn poisoning_is_sticky_and_produces_no_further_events(
        input_index in 0usize..POISONING_INPUTS.len(),
        suffix in prop::collection::vec(any::<u8>(), 0..=32),
    ) {
        let input = POISONING_INPUTS[input_index];
        let mut decoder = Decoder::new();
        for (offset, byte) in input.iter().enumerate() {
            let result = decoder.advance(*byte, true);
            if offset + 1 == input.len() {
                prop_assert_eq!(result, Err(DecodeError));
                prop_assert!(decoder.is_failed());
                prop_assert_eq!(decoder.pending_bytes(), 0);
            } else {
                prop_assert_eq!(result, Ok(None));
                prop_assert!(!decoder.is_failed());
            }
        }
        prop_assert_eq!(decoder.finish_escape(), Err(DecodeError));

        for byte in &suffix {
            prop_assert_eq!(decoder.advance(*byte, true), Err(DecodeError));
            prop_assert_eq!(decoder.advance(*byte, false), Err(DecodeError));
            prop_assert_eq!(decoder.pending_bytes(), 0);
            prop_assert!(decoder.is_failed());
        }
        prop_assert_eq!(decoder.finish_escape(), Err(DecodeError));
    }

    /// Arbitrary byte schedules stay within the fixed buffer bound, never emit
    /// an event after a failure, and keep the poison sticky.
    #[test]
    fn arbitrary_input_is_bounded_and_fails_closed(
        bytes in prop::collection::vec(any::<u8>(), 0..=MAX_RANDOM_PREFIX),
        availability in prop::collection::vec(any::<bool>(), 0..=MAX_RANDOM_PREFIX),
    ) {
        let mut decoder = Decoder::new();
        for (offset, byte) in bytes.iter().enumerate() {
            let available = availability.get(offset).copied().unwrap_or(true);
            let result = decoder.advance(*byte, available);
            prop_assert!(decoder.pending_bytes() <= PASTE_SEQUENCE_LIMIT);
            match result {
                Ok(_) => prop_assert!(!decoder.is_failed()),
                Err(error) => {
                    prop_assert_eq!(error, DecodeError);
                    prop_assert!(decoder.is_failed());
                    prop_assert_eq!(decoder.pending_bytes(), 0);
                }
            }
            if decoder.is_failed() {
                break;
            }
        }
        if decoder.is_failed() {
            for byte in bytes.iter().take(8) {
                prop_assert_eq!(decoder.advance(*byte, true), Err(DecodeError));
                prop_assert_eq!(decoder.advance(*byte, false), Err(DecodeError));
            }
            prop_assert_eq!(decoder.finish_escape(), Err(DecodeError));
        }
    }

    /// Complete token streams decode identically whether or not the final byte
    /// reported more input pending. Availability only changes the lone-ESC case.
    #[test]
    fn complete_token_streams_ignore_final_availability(tokens in arb_token_stream()) {
        let mut bytes = Vec::new();
        let mut expected = Vec::new();
        for token in &tokens {
            bytes.extend_from_slice(&token.bytes());
            expected.push(token.event());
        }
        if bytes.is_empty() {
            return Ok(());
        }

        for final_available in [true, false] {
            let mut decoder = Decoder::new();
            let mut events = feed(&mut decoder, &bytes[..bytes.len() - 1], true)
                .expect("token prefix decodes");
            events.extend(
                feed(&mut decoder, &bytes[bytes.len() - 1..], final_available)
                    .expect("token stream decodes"),
            );
            prop_assert_eq!(&events, &expected);
            prop_assert_eq!(decoder.pending_bytes(), 0);
            prop_assert!(!decoder.is_failed());
        }
    }
}

// ---------------------------------------------------------------------------
// Deterministic companions
// ---------------------------------------------------------------------------

/// ESC has three distinct meanings that depend on availability and timeout.
#[test]
fn escape_availability_and_timeout_semantics_are_distinct() {
    // Availability says "more input may arrive": ESC stays pending.
    let mut deferred = Decoder::new();
    assert_eq!(deferred.advance(b'\x1b', true), Ok(None));
    assert_eq!(deferred.pending_bytes(), 1);
    assert!(!deferred.is_failed());
    assert_eq!(deferred.finish_escape(), Ok(Some(esc_event())));
    assert_eq!(deferred.pending_bytes(), 0);
    // Resolving twice is harmless.
    assert_eq!(deferred.finish_escape(), Ok(None));

    // Availability says "the input is exhausted": ESC resolves immediately.
    let mut immediate = Decoder::new();
    assert_eq!(immediate.advance(b'\x1b', false), Ok(Some(esc_event())));
    assert_eq!(immediate.pending_bytes(), 0);

    // Two ESCs consume the first as a prefix of the second: exactly one Escape.
    let mut doubled = Decoder::new();
    assert_eq!(doubled.advance(b'\x1b', true), Ok(None));
    assert_eq!(doubled.advance(b'\x1b', true), Ok(Some(esc_event())));
    assert_eq!(doubled.pending_bytes(), 0);

    // ESC 'a' is Alt+a, not Escape followed by 'a'.
    let mut alt = Decoder::new();
    assert_eq!(alt.advance(b'\x1b', true), Ok(None));
    let event = alt.advance(b'a', true).expect("alt chord");
    let key = event.expect("one event").as_key_event().expect("key event");
    assert_eq!(key.code, KeyCode::Char('a'));
    assert_eq!(key.modifiers, KeyModifiers::ALT);

    // A partial CSI is never flushed by the timeout resolver.
    let mut partial_csi = Decoder::new();
    assert_eq!(feed(&mut partial_csi, b"\x1b[1;5", true), Ok(Vec::new()));
    let pending = partial_csi.pending_bytes();
    assert_eq!(pending, 5);
    assert_eq!(partial_csi.finish_escape(), Ok(None));
    assert_eq!(partial_csi.pending_bytes(), pending);
    assert!(!partial_csi.is_failed());
    // The completed sequence still decodes.
    assert_eq!(
        feed(&mut partial_csi, b"C", true),
        Ok(vec![Event::Key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::CONTROL
        ))])
    );
}

/// Ordinary and paste ceilings are exact byte counts, checked through the
/// shipped crate boundary rather than the vendor-internal unit tests.
#[test]
fn pending_sequence_ceilings_are_exact() {
    // Ordinary ceiling: ESC '[' plus filler reaches exactly the limit, and the
    // next byte poisons instead of overwriting the fixed buffer.
    let mut ordinary = Decoder::new();
    assert_eq!(feed(&mut ordinary, b"\x1b[", true), Ok(Vec::new()));
    let filler = ORDINARY_SEQUENCE_LIMIT - 2;
    assert_eq!(
        feed(&mut ordinary, &vec![b'1'; filler], true),
        Ok(Vec::new())
    );
    assert_eq!(ordinary.pending_bytes(), ORDINARY_SEQUENCE_LIMIT);
    assert!(!ordinary.is_failed());
    assert_eq!(ordinary.finish_escape(), Ok(None));
    assert_eq!(ordinary.pending_bytes(), ORDINARY_SEQUENCE_LIMIT);
    assert_eq!(ordinary.advance(b'1', true), Err(DecodeError));
    assert!(ordinary.is_failed());
    assert_eq!(ordinary.pending_bytes(), 0);

    // Paste ceiling: 6-byte opener + 65,536 bytes + 6-byte closer is allowed.
    let mut allowed = Decoder::new();
    assert_eq!(feed(&mut allowed, b"\x1b[200~", true), Ok(Vec::new()));
    assert_eq!(
        feed(&mut allowed, &vec![b'x'; 65_536], true),
        Ok(Vec::new())
    );
    let events = feed(&mut allowed, b"\x1b[201~", true).expect("maximum paste decodes");
    assert_eq!(events.len(), 1);
    match &events[0] {
        Event::Paste(text) => {
            assert_eq!(text.len(), 65_536);
            assert!(text.bytes().all(|byte| byte == b'x'));
        }
        other => panic!("expected a paste event, got {other:?}"),
    }
    assert_eq!(allowed.pending_bytes(), 0);
    assert!(!allowed.is_failed());

    // One byte over the paste content limit poisons at the final closer byte.
    let mut over = Decoder::new();
    assert_eq!(feed(&mut over, b"\x1b[200~", true), Ok(Vec::new()));
    assert_eq!(feed(&mut over, &vec![b'x'; 65_537], true), Ok(Vec::new()));
    assert_eq!(
        feed(&mut over, b"\x1b[201~", true),
        Err(DecodeError),
        "the sixth closer byte exceeds the fixed paste buffer"
    );
    assert!(over.is_failed());
    assert_eq!(over.pending_bytes(), 0);
}

/// A paste body that is not UTF-8 poisons only when the closer completes the
/// sequence; buffered bytes before that are not events.
#[test]
fn paste_utf8_validation_waits_for_the_closer() {
    let mut decoder = Decoder::new();
    assert_eq!(feed(&mut decoder, b"\x1b[200~", true), Ok(Vec::new()));
    assert_eq!(decoder.advance(0xff, true), Ok(None));
    assert_eq!(decoder.advance(0xfe, true), Ok(None));
    assert_eq!(decoder.pending_bytes(), 8);
    assert!(!decoder.is_failed());
    assert_eq!(
        feed(&mut decoder, b"\x1b[201~"[..5].as_ref(), true),
        Ok(Vec::new())
    );
    assert_eq!(
        decoder.advance(b'~', true),
        Err(DecodeError),
        "invalid UTF-8 is rejected when the paste closes"
    );
    assert!(decoder.is_failed());
    assert_eq!(decoder.pending_bytes(), 0);

    // A valid multi-byte payload of the same length still decodes.
    let mut valid = Decoder::new();
    let mut bytes = b"\x1b[200~".to_vec();
    bytes.extend_from_slice("héllo\nwörld".as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    assert_eq!(
        feed(&mut valid, &bytes, true),
        Ok(vec![Event::Paste(String::from("héllo\nwörld"))])
    );
}
