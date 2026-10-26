# P5 pane operations runtime slice

Date: 2026-10-25
Status: implementation authored; no Cargo, build, or test verification run by
this owner. Capability offering and integrated verification remain parent work.

## Runtime path

Schema 13 is classified with the exported strict
`crate::pane_operations_protocol::decode_request` codec before any request is
forwarded to the owner queue. Truncated, trailing, reserved-byte, invalid
boolean, and other malformed shapes therefore receive the fixed
`invalid_request` worker error without owner admission.

The connection worker requires capability 64 plus Read for `ViewQuery`, and
capability 64 plus Input for Swap, DividerSet, and ZoomSet. The serialized
owner dispatcher repeats this authorization check before calling any owner
API. Pane operations continue through the existing single owner request queue;
connection workers do not access PTYs or layout state.

View queries call `pane_operations_view` and encode the exact 504-byte
schema-13 view. Mutations call `handle_pane_operation` with the original bytes
for operation-ledger payload identity and the existing `PtyEffects::resize`
adapter. Their existing status/rejection payload is forwarded only if its
decoded operation key matches the request key. A view response must pass the
strict schema-13 view decoder. Legacy schema-8 dispatch and schema-9 response
handling were left unchanged.

## Owner API assumptions

The runtime calls the in-flight owner interfaces specified for this slice:

```text
pane_operations_view(&self, connection: &Connection, now_secs: u64)
    -> Result<Option<PaneOperationsView>, ServerError>
handle_pane_operation<RE, Resize>(
    &mut self, connection: &Connection, request: Request, payload: &[u8],
    now_secs: u64, resize: Resize,
) -> Result<Vec<u8>, ServerError>
where Resize: FnMut(&mut PtyEffects, u16, u16) -> Result<(), RE>
```

`now` is the existing runtime logical timestamp and is passed unchanged as
`now_secs`. The mutation resize adapter passes rows and columns to
`PtyEffects::resize`. No other server capability offer, connection code, or
layout implementation was changed here.

## Focused runtime tests authored

The owned runtime module now includes tests for strict schema-13 classification
and trailing-byte rejection, Query versus DurableMutation capability classes,
keyed mutation status matching, and rejection of a noncanonical schema-13 view
shape. The exact test names are
`pane_operations_classifier_and_capability_classes_are_strict`,
`pane_operation_mutation_response_must_match_original_key`, and
`pane_operations_view_response_rejects_noncanonical_shape`. None of these
three unit tests requires capability 64 to be offered; the classifier and
response tests are local codec/guard checks, and the capability test exercises
the worker/dispatcher predicate. The shared owner fixture was not suitable for
an unnegotiated dispatch/no-effects test, so that test was not authored. Any
live negotiated request/response integration test requires the parent to offer
capability 64 after combining these runtime changes with the owner APIs.

Parent serial test filter after integration:

```text
cargo test --locked --offline --lib multi_runtime::tests::pane_operations_
```

Run that filter after the owner APIs are present so the module compiles. Passing
the unit filter does not depend on the server offer and does not prove
negotiated availability or end-to-end support. No test result is claimed by
this slice.
