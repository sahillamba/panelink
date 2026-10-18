//! Component integration only: no runtime Events capability is enabled here.

use panelink::control_protocol::TargetRef;
use panelink::event_journal::{EventJournal, JournalError, MAX_RETAINED_ENCODED_BYTES};
use panelink::event_protocol::{decode_event, encode_event, EventBody, EVENT_PAYLOAD_BYTES};
use panelink::protocol_wire::{decode_frame, encode_frame, Frame, MessageKind, HEADER_BYTES};

#[test]
fn cursor_loss_requires_reconciliation_before_ordered_framed_notification_reads() {
    let target = TargetRef {
        generation: 17,
        workspace: 3,
        pane: 4,
        session: 5,
        occupant_revision: 6,
    };
    let mut journal = EventJournal::new(target.generation).unwrap();
    let disconnected_cursor = journal.current_cursor();
    journal
        .push(
            target,
            EventBody::InputOwnerChanged {
                revision: 1,
                owner_lease: 23,
            },
        )
        .unwrap();
    let reconciled_cursor = journal
        .push(target, EventBody::ScreenChanged { revision: 2 })
        .unwrap();
    for revision in 3..=66 {
        journal
            .push(target, EventBody::ScreenChanged { revision })
            .unwrap();
    }

    assert_eq!(
        journal.read_after(disconnected_cursor, 64),
        Err(JournalError::CursorExpired)
    );
    // A caller that obtained authoritative state at cursor2 can read every
    // subsequent retained fact, while cursor0 must never look current.
    let first_page = journal.read_after(reconciled_cursor, 7).unwrap();
    assert_eq!(first_page.len(), 7);
    let mut cursor = reconciled_cursor;
    let mut encoded_bytes = 0;
    let mut count = 0;
    loop {
        let page = journal.read_after(cursor, 7).unwrap();
        if page.is_empty() {
            break;
        }
        for event in page {
            assert_eq!(event.cursor, cursor.sequence + 1);
            assert_eq!(event.target, target);
            let payload = encode_event(&event).unwrap();
            let frame = Frame::new(MessageKind::Event, 0, &payload).unwrap();
            let mut wire = Vec::new();
            encode_frame(&frame, &mut wire).unwrap();
            assert_eq!(wire.len(), HEADER_BYTES + EVENT_PAYLOAD_BYTES);
            let decoded_frame = decode_frame(&wire).unwrap();
            assert_eq!(decoded_frame.header().kind(), MessageKind::Event);
            assert_eq!(decoded_frame.header().request_id(), 0);
            assert_eq!(decode_event(decoded_frame.payload()).unwrap(), event);
            encoded_bytes += wire.len();
            count += 1;
            cursor.sequence = event.cursor;
        }
    }
    assert_eq!(count, 64);
    assert_eq!(cursor, journal.current_cursor());
    assert_eq!(encoded_bytes, MAX_RETAINED_ENCODED_BYTES);
    assert_eq!(journal.retained_encoded_bytes(), encoded_bytes);
}
