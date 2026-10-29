# Scoped wheel bounded final review — 2026-10-29

No material actionable correctness or safety regression found in the reviewed
scoped-wheel changes. No blocking reproducer or source fix identified.

Reviewed the dirty `t3/improve-codex-mouse-scroll-speed` checkout at
`/Users/sahillamba/.t3/worktrees/panelink/t3-ca0a6ac8`, against the implementation
contract and current result report. Graph-first symbol lookup succeeded with
the explicit project name; conclusions use current source and diff.

Source checks covered:

- `src/server/multi_session.rs:2660`: both capabilities and exact target are
  required; duplicate/conflict lookup precedes target/mode/owner revalidation.
  Mode validation precedes vacancy refusal, including same-caller and due
  holders. Claim, pin, retained and released clones prove revision/time
  arithmetic before admission. Ledger completion is immediately followed by
  the prepared owner assignment. Non-Applied results retain the holder and
  clear only the completed pin; completion failure leaves the pin intact.
- Existing `InputOwner` expiry and `PtyEffects::mouse_wheel`/`drain_request`
  preserve completed-holder expiry, unresolved-pin retention, synchronous
  delivery and unwritten-suffix disposal. No new replay path was found.
- `src/ui_protocol.rs`, `src/handshake_protocol.rs`, `src/server.rs` and
  `src/multi_runtime.rs`: fixed typed request, optional negotiation, saved-grant
  intersection on resume, single-session exclusion, and matching-key
  Status/Rejected response filtering agree with the scoped contract.
- `src/ui_client/conn.rs:708` and `src/ui_client/net.rs:264`: the scoped sender
  uses existing keyed reserve/settlement and status-only reconciliation; it
  does not issue Claim/Release/Describe or alter local owner revision.
  Legacy wheel queries mode first and uses vacancy-only Claim CAS. Unsupported
  scoped requests permit only fresh exact-target readback. Stable no-mouse
  feedback bypasses ownership. Ordinary typing/paste retain their claim path.
- Inline codec, handler, classification and fake-peer tests cover the principal
  transaction boundaries, revision edge, non-Applied retention, duplicate and
  conflict safety, negotiation, wrong-key uncertainty and no-mouse paths.

This is source review, not an exhaustive audit or executed verification.
No builds, tests, installation, restart, live pane input, provider calls or
subagents were run. Only this report was written. Test totals and private
receiver timings in the result report remain parent-reported evidence; the
canonical runner was reported in progress and was not independently checked.
Neither the synthetic burst measurements nor this review establish sustained
60 Hz latency budgets, real Codex behavior or native host acceptance. Installed
0.1.7/live-server 0.1.4 identities were supplied by the task, not refreshed here.
