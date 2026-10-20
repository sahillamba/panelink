use panelink::control_protocol::TargetRef;
use panelink::history_rows::{decode, encode};
use vt100::Parser;

fn target() -> TargetRef {
    TargetRef {
        generation: 7,
        workspace: 1,
        pane: 2,
        session: 3,
        occupant_revision: 1,
    }
}

#[test]
fn serialized_scrollback_is_sanitized_bounded_and_read_only() {
    let mut parser = Parser::new(2, 20, 4);
    let initial = parser.screen().history_cursor().unwrap();
    parser.process(b"\x1b[31msecret\x1b[0m\x1b]52;c;CANARY\x07\r\nnext\r\nlast");
    let record = encode(
        target(),
        initial.epoch,
        parser
            .screen()
            .history_after(initial)
            .unwrap()
            .next()
            .unwrap(),
        123,
    )
    .unwrap();
    let decoded = decode(&record).unwrap();
    assert_eq!((decoded.epoch, decoded.ordinal, decoded.width), (1, 1, 20));
    assert_eq!(decoded.cell_text(0), Some("s"));
    assert!(!record.windows(6).any(|window| window == b"CANARY"));
    assert!(!record.contains(&0x1b));
    assert!(!format!("{decoded:?}").contains("secret"));
    for length in 0..record.len() {
        assert!(
            decode(&record[..length]).is_err(),
            "accepted truncated length {length}"
        );
    }
    let mut corrupt = record;
    corrupt[80 + 10] = 0x1b;
    assert!(decode(&corrupt).is_err());
}
