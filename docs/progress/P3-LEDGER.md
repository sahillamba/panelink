# P3-B1 global ledger bounds and expiry reclamation

Status: implemented in the assigned files and locally verified; parent integration review remains pending. This is a bounded in-memory protocol ledger change only. It does not start or complete the Phase 3 server work, and it does not close the Phase 2 manual Ctrl-X or clipboard acceptance gates.

## Implemented behavior

`src/protocol.rs` keeps the original per-operation payload limit (65,536 bytes), per-lease unreceipted-operation limit (1,024), and TTL (1,800 seconds), and adds these provisional per-generation limits:

| Resource | Limit |
| --- | ---: |
| Permanent lease records, including expired IDs | 4,096 |
| Leases not yet marked expired by maintenance | 64 |
| Retained operation records across all leases | 4,096 |
| Logical retained payload bytes across all operations | 8 MiB |

`LedgerUsage` exposes stored numeric counts without consulting a clock. Payload bytes count logical payload lengths for duplicate comparison; they are not allocator-capacity or RSS measurements. A lease remains counted as active after its timestamp is due until `expire_leases` marks it expired. Callers must provide monotonic logical times and schedule maintenance explicitly; admission and other request methods do not run reclamation automatically.

`issue_lease` starts at 1 and returns an ID above every successfully opened ID, including IDs supplied through legacy `open_lease`. Failed issuance or opening leaves the ID watermark and usage unchanged. IDs and expired lease records are never removed, so the generation can eventually reach its 4,096-record ceiling. The issuer is volatile and does not provide durable generation allocation or authentication.

Admission validates generation, lease, expiry, sequence, and per-operation payload size; checks a retained duplicate or conflict before global capacity checks; then checks operation and logical-byte capacity before copying a new payload and commits the counts before returning `Admission::Execute`. Empty payloads still consume an operation slot. Rejections leave counters and the sequence high-water mark unchanged. Receipt validates the full range before removal, releases exact record and byte counts, and preserves the retired sequence boundary. Repeated receipts do not release records twice.

`expire_leases` scans at most the bounded lease and operation records. It marks each due lease once, releases payload vectors by replacing and dropping them, removes terminal operation metadata, and retains pending operation identities with empty payload vectors. Pending work can complete after expiry. A terminal result completed after expiry stays idempotent while its record remains; the next maintenance call removes it, after which `complete` returns `ResultExpired`. `admit`, `status`, `renew_lease`, `receipt`, and reopening reject an expired lease, even if a caller later supplies an earlier timestamp. `complete` remains available only for a retained operation record. No payload zeroization guarantee is made.

The `Ledger` debug output and new capacity error labels expose only numeric metadata or fixed text. Tests independently traverse private lease/operation records to compare stored usage after mixed transitions; production code does not expose payload contents.

## Verification

The initial formatting check found rustfmt-only changes in the newly added assertions. `cargo fmt -- src/protocol.rs` applied those changes; the final required checks below passed.

Command:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --lib protocol
```

Actual output:

```text
   Compiling panelink v0.1.0 (/Users/sahillamba/panelink)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.32s
     Running unittests src/lib.rs (target/debug/deps/panelink-87dcbc1ebcc07c4c)

running 27 tests
test protocol::tests::lease_issuance_follows_explicit_ids_and_handles_ttl_and_id_exhaustion ... ok
test protocol::tests::new_capacity_errors_have_fixed_redacted_labels ... ok
test protocol::tests::expiry_rejects_duplicates_but_does_not_cancel_completion ... ok
test protocol::tests::active_lease_limit_does_not_consume_issued_ids_on_failure ... ok
test protocol::tests::expiry_reclaims_payloads_and_terminal_records_but_preserves_pending_identity ... ok
test protocol::tests::duplicates_share_record_and_changed_payload_conflicts ... ok
test protocol::tests::ledger_debug_does_not_reveal_payload_bytes ... ok
test protocol::tests::receipts_wait_for_completion_then_retire_keys ... ok
test protocol::tests::global_payload_limit_precedes_copy_and_reuses_capacity_after_receipt ... ok
test protocol::tests::retry_classes_match_side_effect_boundaries ... ok
test protocol::tests::stored_usage_matches_independent_traversal_after_mixed_transitions ... ok
test protocol::tests::rejects_wrong_generation_and_out_of_order_new_sequences ... ok
test protocol::tests::zero_ids_and_nonterminal_completion_are_rejected ... ok
test protocol_wire::tests::all_message_kinds_round_trip_binary_payloads_and_zero_request_id ... ok
test protocol_wire::tests::decoded_payload_borrows_the_input_buffer ... ok
test protocol_wire::tests::encoder_rejects_invalid_internal_frame_before_first_write ... ok
test protocol_wire::tests::every_kind_obeys_its_exact_payload_boundary ... ok
test protocol_wire::tests::frame_debug_and_errors_do_not_disclose_payload_canary ... ok
test protocol_wire::tests::golden_header_bytes_are_stable ... ok
test protocol_wire::tests::rejects_short_headers_and_truncated_bodies ... ok
test protocol_wire::tests::rejects_trailing_bytes_and_invalid_header_fields ... ok
test protocol_wire::tests::u32_max_length_is_rejected_without_a_body_allocation ... ok
test protocol_wire::tests::writer_failure_preserves_only_the_accepted_prefix ... ok
test protocol::tests::payload_and_unreceipted_limits_reject_without_mutation ... ok
test protocol::tests::global_operation_limit_charges_empty_payloads_and_receipts_free_slots ... ok
test protocol::tests::expired_pending_metadata_keeps_global_slots_until_completion_is_reclaimed ... ok
test protocol::tests::permanent_lease_record_limit_includes_expired_records ... ok

test result: ok. 27 passed; 0 failed; 0 ignored; 0 measured; 40 filtered out; finished in 0.03s
```

The `protocol` name filter also selected the existing `protocol_wire` tests.

Command:

```text
cargo fmt --check
```

Actual output: no output; exit status 0.

Command:

```text
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --all-targets -- -D warnings
```

Actual output:

```text
    Checking panelink v0.1.0 (/Users/sahillamba/panelink)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.22s
```

## Scope and ownership

No dependency, manifest, export, socket, server, clock, I/O, PTY, or persistence work was added. These checks establish local ledger behavior only; they make no runtime, reconnect, execution, recovery, memory-RSS, or Apple Terminal claims. Parent review and integration remain outstanding. Implementation ownership is released to the parent for review.

## Parent integration

Parent reviewed the implementation and independently ran `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline -q`: exit0, **103passed /9ignored** (67library +21binary +4host fixture/preflight +11core PTY;0doctests). Parent then strengthened the existing full-global-operation test with same-key duplicate/conflict checks at the count ceiling; the focused test passed. No production code was changed by that test strengthening. Final fmt, strict locked/offline Clippy and diff whitespace checks all exited0. Scope remains the pure volatile ledger; no dependencies/lockfile/manifest changes, commits or pushes. Ownership released.
