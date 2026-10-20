use super::*;

const KEY: OperationKey = OperationKey {
    generation: 1,
    lease: 2,
    sequence: 3,
};

const TARGET: TargetRef = TargetRef {
    generation: 1,
    workspace: 4,
    pane: 5,
    session: 6,
    occupant_revision: 7,
};

fn request() -> CloseRequest {
    CloseRequest {
        key: KEY,
        target: TARGET,
        expected_layout_revision: 9,
    }
}

fn encode(request: &CloseRequest) -> Vec<u8> {
    let mut bytes = Vec::new();
    assert_eq!(
        encode_close_request(request, &mut bytes).expect("request encodes"),
        CLOSE_REQUEST_BYTES
    );
    bytes
}

fn result(outcome: CloseOutcome) -> CloseResult {
    CloseResult {
        key: KEY,
        outcome,
        target: Some(TARGET),
        layout_revision: 10,
        survivor_resize_pending: false,
    }
}

fn encode_result(result: &CloseResult) -> Vec<u8> {
    let mut bytes = Vec::new();
    assert_eq!(
        encode_close_result(result, &mut bytes).expect("result encodes"),
        CLOSE_RESULT_BYTES
    );
    bytes
}

fn set_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn set_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}

struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "writer-private-marker",
        ))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn request_matches_golden_bytes_and_round_trips() {
    let request = request();
    assert_eq!(request.operation_class(), OperationClass::DurableMutation);
    let bytes = encode(&request);
    #[rustfmt::skip]
    let golden: [u8; CLOSE_REQUEST_BYTES] = [
        0, 9, 0, 1,
        0, 0, 0, 0, 0, 0, 0, 1,
        0, 0, 0, 0, 0, 0, 0, 2,
        0, 0, 0, 0, 0, 0, 0, 3,
        0, 0, 0, 0, 0, 0, 0, 1,
        0, 0, 0, 0, 0, 0, 0, 4,
        0, 0, 0, 0, 0, 0, 0, 5,
        0, 0, 0, 0, 0, 0, 0, 6,
        0, 0, 0, 0, 0, 0, 0, 7,
        0, 0, 0, 0, 0, 0, 0, 9,
        0, 0, 0, 0,
    ];
    assert_eq!(bytes, golden);
    assert_eq!(decode_close_request(&bytes), Ok(request));
    // Each covered field changes the exact dedup bytes.
    let variants = [
        CloseRequest {
            expected_layout_revision: 10,
            ..request
        },
        CloseRequest {
            target: TargetRef {
                occupant_revision: 8,
                ..TARGET
            },
            ..request
        },
        CloseRequest {
            key: OperationKey { sequence: 4, ..KEY },
            ..request
        },
    ];
    for variant in variants {
        assert_ne!(encode(&variant), bytes);
    }
}

#[test]
fn request_truncation_trailing_header_and_reserved_are_rejected() {
    let bytes = encode(&request());
    for end in 0..CLOSE_REQUEST_BYTES {
        assert_eq!(
            decode_close_request(&bytes[..end]),
            Err(CloseCodecError::Truncated)
        );
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert_eq!(
        decode_close_request(&trailing),
        Err(CloseCodecError::TrailingBytes)
    );
    for schema in [0, 7, 8, 10] {
        let mut wrong = bytes.clone();
        set_u16(&mut wrong, 0, schema);
        assert_eq!(
            decode_close_request(&wrong),
            Err(CloseCodecError::UnsupportedSchema)
        );
    }
    for kind in [0, 2, KIND_RESULT] {
        let mut wrong = bytes.clone();
        set_u16(&mut wrong, 2, kind);
        assert_eq!(
            decode_close_request(&wrong),
            Err(CloseCodecError::UnknownKind)
        );
    }
    for offset in 76..CLOSE_REQUEST_BYTES {
        let mut reserved = bytes.clone();
        reserved[offset] = 1;
        assert_eq!(
            decode_close_request(&reserved),
            Err(CloseCodecError::NonzeroReserved)
        );
    }
}

#[test]
fn zero_identity_revision_and_generation_mismatch_are_rejected() {
    let bytes = encode(&request());
    for offset in [4, 12, 20] {
        let mut zero = bytes.clone();
        set_u64(&mut zero, offset, 0);
        assert_eq!(
            decode_close_request(&zero),
            Err(CloseCodecError::ZeroKeyComponent)
        );
    }
    for offset in [28, 36, 44, 52, 60] {
        let mut zero = bytes.clone();
        set_u64(&mut zero, offset, 0);
        assert_eq!(
            decode_close_request(&zero),
            Err(CloseCodecError::InvalidTarget)
        );
    }
    let mut revision = bytes.clone();
    set_u64(&mut revision, 68, 0);
    assert_eq!(
        decode_close_request(&revision),
        Err(CloseCodecError::ZeroLayoutRevision)
    );
    let mut generation = bytes;
    set_u64(&mut generation, 28, 2);
    assert_eq!(
        decode_close_request(&generation),
        Err(CloseCodecError::GenerationMismatch)
    );

    let invalid = [
        (
            CloseRequest {
                key: OperationKey { lease: 0, ..KEY },
                ..request()
            },
            CloseCodecError::ZeroKeyComponent,
        ),
        (
            CloseRequest {
                target: TargetRef { pane: 0, ..TARGET },
                ..request()
            },
            CloseCodecError::InvalidTarget,
        ),
        (
            CloseRequest {
                expected_layout_revision: 0,
                ..request()
            },
            CloseCodecError::ZeroLayoutRevision,
        ),
    ];
    for (request, expected) in invalid {
        let mut written = Vec::new();
        assert!(matches!(
            encode_close_request(&request, &mut written),
            Err(CloseEncodeError::Invalid(error)) if error == expected
        ));
        assert!(
            written.is_empty(),
            "nothing is written on validation failure"
        );
    }
}

#[test]
fn status_query_matches_golden_bytes_and_rejects_malformed_input() {
    let query = CloseStatusQuery { key: KEY };
    assert_eq!(query.operation_class(), OperationClass::Query);
    let mut bytes = Vec::new();
    assert_eq!(
        encode_close_status_query(query, &mut bytes).unwrap(),
        CLOSE_STATUS_QUERY_BYTES
    );
    #[rustfmt::skip]
    let golden: [u8; 32] = [
        0, 9, 0, 2,
        0, 0, 0, 0, 0, 0, 0, 1,
        0, 0, 0, 0, 0, 0, 0, 2,
        0, 0, 0, 0, 0, 0, 0, 3,
        0, 0, 0, 0,
    ];
    assert_eq!(bytes, golden);
    assert_eq!(decode_close_status_query(&bytes), Ok(query));
    assert_eq!(
        decode_close_status_query(&bytes[..31]),
        Err(CloseCodecError::Truncated)
    );
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert_eq!(
        decode_close_status_query(&trailing),
        Err(CloseCodecError::TrailingBytes)
    );
    let mut kind = bytes.clone();
    set_u16(&mut kind, 2, 1);
    assert_eq!(
        decode_close_status_query(&kind),
        Err(CloseCodecError::UnknownKind)
    );
    for offset in 28..32 {
        let mut reserved = bytes.clone();
        reserved[offset] = 1;
        assert_eq!(
            decode_close_status_query(&reserved),
            Err(CloseCodecError::NonzeroReserved)
        );
    }
    let mut zero = bytes;
    set_u64(&mut zero, 12, 0);
    assert_eq!(
        decode_close_status_query(&zero),
        Err(CloseCodecError::ZeroKeyComponent)
    );
}

#[test]
fn result_matches_golden_bytes_and_every_outcome_round_trips() {
    let escalated = CloseResult {
        survivor_resize_pending: true,
        ..result(CloseOutcome::Stopped(StopOutcome::Escalated))
    };
    let bytes = encode_result(&escalated);
    #[rustfmt::skip]
    let golden: [u8; CLOSE_RESULT_BYTES] = [
        0, 9, 0x80, 0x01,
        0, 0, 0, 0, 0, 0, 0, 1,
        0, 0, 0, 0, 0, 0, 0, 2,
        0, 0, 0, 0, 0, 0, 0, 3,
        // Escalated, no rejection.
        0, 2, 0, 0,
        // Layout revision 10.
        0, 0, 0, 0, 0, 0, 0, 10,
        // Flags: target present + resize pending; reserved.
        0, 3, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 1,
        0, 0, 0, 0, 0, 0, 0, 4,
        0, 0, 0, 0, 0, 0, 0, 5,
        0, 0, 0, 0, 0, 0, 0, 6,
        0, 0, 0, 0, 0, 0, 0, 7,
        0, 0, 0, 0, 0, 0, 0, 0,
    ];
    assert_eq!(bytes, golden);
    assert_eq!(decode_close_result(&bytes), Ok(escalated));
    assert_eq!(escalated.ledger_outcome(), Outcome::Partial);

    let valid = [
        (
            result(CloseOutcome::Stopped(StopOutcome::Exited)),
            Outcome::Applied,
        ),
        (
            result(CloseOutcome::Stopped(StopOutcome::Escalated)),
            Outcome::Applied,
        ),
        (
            result(CloseOutcome::Stopped(StopOutcome::Unknown)),
            Outcome::Unknown,
        ),
        (
            result(CloseOutcome::Rejected(CloseRejection::StaleTarget)),
            Outcome::Failed,
        ),
        (
            result(CloseOutcome::Rejected(CloseRejection::StaleLayout)),
            Outcome::Failed,
        ),
        (
            result(CloseOutcome::Rejected(CloseRejection::AlreadyClosed)),
            Outcome::Failed,
        ),
        (
            CloseResult {
                layout_revision: 0,
                ..result(CloseOutcome::Rejected(CloseRejection::NoLayout))
            },
            Outcome::Failed,
        ),
        (
            CloseResult {
                target: None,
                layout_revision: 0,
                ..result(CloseOutcome::Rejected(CloseRejection::NotAdmitted))
            },
            Outcome::Failed,
        ),
    ];
    for (value, ledger) in valid {
        assert_eq!(value.ledger_outcome(), ledger);
        assert_eq!(decode_close_result(&encode_result(&value)), Ok(value));
    }
}

#[test]
fn result_contract_and_malformed_wire_fields_are_rejected() {
    let mismatches = [
        CloseResult {
            target: None,
            ..result(CloseOutcome::Stopped(StopOutcome::Exited))
        },
        CloseResult {
            layout_revision: 0,
            ..result(CloseOutcome::Stopped(StopOutcome::Exited))
        },
        CloseResult {
            survivor_resize_pending: true,
            ..result(CloseOutcome::Stopped(StopOutcome::Unknown))
        },
        CloseResult {
            survivor_resize_pending: true,
            ..result(CloseOutcome::Rejected(CloseRejection::StaleLayout))
        },
        result(CloseOutcome::Rejected(CloseRejection::NoLayout)),
        result(CloseOutcome::Rejected(CloseRejection::NotAdmitted)),
    ];
    for value in mismatches {
        assert_eq!(value.encoded_len(), Err(CloseCodecError::OutcomeMismatch));
    }

    let original = encode_result(&result(CloseOutcome::Stopped(StopOutcome::Exited)));
    assert_eq!(
        decode_close_result(&original[..95]),
        Err(CloseCodecError::Truncated)
    );
    let mut trailing = original.clone();
    trailing.push(0);
    assert_eq!(
        decode_close_result(&trailing),
        Err(CloseCodecError::TrailingBytes)
    );
    let mut schema = original.clone();
    set_u16(&mut schema, 0, 7);
    assert_eq!(
        decode_close_result(&schema),
        Err(CloseCodecError::UnsupportedSchema)
    );
    for code in [0, 5] {
        let mut outcome = original.clone();
        set_u16(&mut outcome, 28, code);
        assert_eq!(
            decode_close_result(&outcome),
            Err(CloseCodecError::UnknownOutcome)
        );
    }
    let mut stopped_with_reason = original.clone();
    set_u16(&mut stopped_with_reason, 30, 1);
    assert_eq!(
        decode_close_result(&stopped_with_reason),
        Err(CloseCodecError::UnknownRejection)
    );
    for reason in [0, 6] {
        let mut rejected = original.clone();
        set_u16(&mut rejected, 28, 4);
        set_u16(&mut rejected, 30, reason);
        assert_eq!(
            decode_close_result(&rejected),
            Err(CloseCodecError::UnknownRejection)
        );
    }
    let mut flags = original.clone();
    set_u16(&mut flags, 40, 4 | 1);
    assert_eq!(
        decode_close_result(&flags),
        Err(CloseCodecError::InvalidFlags)
    );
    let mut absent = original.clone();
    set_u16(&mut absent, 40, 0);
    assert_eq!(
        decode_close_result(&absent),
        Err(CloseCodecError::NonzeroReserved)
    );
    for offset in (42..48).chain(88..96) {
        let mut reserved = original.clone();
        reserved[offset] = 1;
        assert_eq!(
            decode_close_result(&reserved),
            Err(CloseCodecError::NonzeroReserved)
        );
    }
    let mut zero_key = original.clone();
    set_u64(&mut zero_key, 20, 0);
    assert_eq!(
        decode_close_result(&zero_key),
        Err(CloseCodecError::ZeroKeyComponent)
    );
    let mut zero_target = original;
    set_u64(&mut zero_target, 72, 0);
    assert_eq!(
        decode_close_result(&zero_target),
        Err(CloseCodecError::InvalidTarget)
    );
}

#[test]
fn debug_and_display_are_redacted_and_writer_errors_hidden() {
    let marker = TargetRef {
        generation: 9_876_543_210_123,
        workspace: 9_876_543_210_124,
        pane: 9_876_543_210_125,
        session: 9_876_543_210_126,
        occupant_revision: 9_876_543_210_127,
    };
    let key = OperationKey {
        generation: marker.generation,
        lease: 9_876_543_210_128,
        sequence: 9_876_543_210_129,
    };
    let request = CloseRequest {
        key,
        target: marker,
        expected_layout_revision: 9_876_543_210_130,
    };
    let value = CloseResult {
        key,
        outcome: CloseOutcome::Stopped(StopOutcome::Exited),
        target: Some(marker),
        layout_revision: 9_876_543_210_131,
        survivor_resize_pending: false,
    };
    let rendered = format!("{request:?} {value:?} {:?}", CloseStatusQuery { key });
    assert!(
        !rendered.contains("98765432101"),
        "debug leaked an identity"
    );

    let error = encode_close_request(&request, &mut FailingWriter).unwrap_err();
    assert!(
        matches!(&error, CloseEncodeError::Writer(inner) if inner.kind() == io::ErrorKind::BrokenPipe)
    );
    assert!(!format!("{error} {error:?}").contains("writer-private-marker"));
    assert_eq!(
        CloseCodecError::Truncated.to_string(),
        "pane-close payload is truncated"
    );
}
