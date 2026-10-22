//! Independent wire fixtures: historical records must stay canonical and inert.
//!
//! The current record is the 280-byte four-pane form. The 160-byte two-pane
//! form written before the four-pane slice must still decode unchanged; it is
//! re-encoded in the current form (migration), never rewritten in place.
use panelink::saved_layout::{
    decode, encode, LayoutAxis, LEGACY_SAVED_LAYOUT_BYTES, SAVED_LAYOUT_BYTES,
};

fn fill_header_and_first_target(bytes: &mut [u8]) {
    bytes[..8].copy_from_slice(b"PLLAY001");
    // generation, workspace, sequence, revision, time; one target in that scope.
    for (offset, value) in [
        (8, 1u64),
        (16, 2),
        (24, 3),
        (32, 4),
        (40, 5),
        (64, 1),
        (72, 2),
        (80, 6),
        (88, 7),
        (96, 8),
    ] {
        bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
    }
    bytes[49] = 5;
    bytes[51] = 20;
    bytes[52] = 1;
}

fn legacy_fixture() -> [u8; LEGACY_SAVED_LAYOUT_BYTES] {
    let mut bytes = [0; LEGACY_SAVED_LAYOUT_BYTES];
    fill_header_and_first_target(&mut bytes);
    bytes
}

fn fixture() -> [u8; SAVED_LAYOUT_BYTES] {
    let mut bytes = [0; SAVED_LAYOUT_BYTES];
    fill_header_and_first_target(&mut bytes);
    // First pane rect at 224..232: x, y, rows, columns.
    bytes[229] = 5;
    bytes[231] = 20;
    bytes
}

fn assert_single_pane(bytes: &[u8]) -> panelink::saved_layout::SavedLayout {
    let layout = decode(bytes).expect("valid fixed historical record");
    assert_eq!(layout.axis(), LayoutAxis::Single);
    assert_eq!(layout.metadata().snapshot_sequence(), 3);
    assert_eq!(layout.metadata().layout_revision(), 4);
    let pane = layout.geometry()[0].expect("one pane");
    assert_eq!(
        (pane.rect.x, pane.rect.y, pane.rect.rows, pane.rect.columns),
        (0, 0, 5, 20)
    );
    assert_eq!(
        (
            pane.target.generation,
            pane.target.workspace,
            pane.target.pane,
            pane.target.session,
            pane.target.occupant_revision
        ),
        (1, 2, 6, 7, 8)
    );
    layout
}

#[test]
fn independently_written_wire_fixture_decodes_without_normalization() {
    let bytes = fixture();
    let layout = assert_single_pane(&bytes);
    assert_eq!(encode(&layout), bytes);
}

#[test]
fn legacy_two_pane_form_decodes_and_migrates_to_the_current_form() {
    let layout = assert_single_pane(&legacy_fixture());
    assert_eq!(encode(&layout), fixture());
}

#[test]
fn every_accepted_single_byte_mutation_remains_exactly_canonical() {
    let original = fixture();
    for offset in 0..SAVED_LAYOUT_BYTES {
        for value in 0..=u8::MAX {
            let mut bytes = original;
            bytes[offset] = value;
            if let Ok(layout) = decode(&bytes) {
                assert_eq!(encode(&layout), bytes, "noncanonical byte at {offset}");
                for pane in layout.geometry().into_iter().flatten() {
                    assert!((5..=80).contains(&pane.rect.rows));
                    assert!((20..=240).contains(&pane.rect.columns));
                    assert!(pane.rect.x + pane.rect.columns <= layout.columns());
                    assert!(pane.rect.y + pane.rect.rows <= layout.rows());
                }
            }
        }
    }
}

#[test]
fn every_accepted_legacy_single_byte_mutation_stays_inert() {
    let original = legacy_fixture();
    for offset in 0..LEGACY_SAVED_LAYOUT_BYTES {
        for value in 0..=u8::MAX {
            let mut bytes = original;
            bytes[offset] = value;
            if let Ok(layout) = decode(&bytes) {
                // Re-encoding yields a current record that decodes to the same value.
                let current = encode(&layout);
                assert_eq!(decode(&current).unwrap(), layout, "offset {offset}");
                for pane in layout.geometry().into_iter().flatten() {
                    assert!((5..=80).contains(&pane.rect.rows));
                    assert!((20..=240).contains(&pane.rect.columns));
                    assert!(pane.rect.x + pane.rect.columns <= layout.columns());
                    assert!(pane.rect.y + pane.rect.rows <= layout.rows());
                }
            }
        }
    }
}
