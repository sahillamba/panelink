# Phase 3 owner identity implementation review — 2026-10-23

Read-only review of the current `src/agent_cli.rs` additions against
[P3-OWNER-IDENTITY-TASK.md](P3-OWNER-IDENTITY-TASK.md). No production changes
or tests were made or run as part of this review.

**Result: no actionable finding in the reviewed scope.** The implementation
exposes the server's existing owner lease and revision as neutral client
identity. It does not authenticate an agent role, accept a caller-supplied
identity, or change claim/input authority.

| Review point | Evidence and assessment |
|---|---|
| Neutral role | `PaneOwner` labels an unowned pane `none`, the caller's lease `you`, and another owner `client`. JSON `kind` is `none` or `client`; no `agent:` role or `--as` label is surfaced. `--as` selects the local context, while the displayed identity comes from `SessionInfo`. |
| Generation scope | Structured IDs and text labels use `target.generation:owner_lease`. The caller comparison uses the lease from the `WorkspaceLink` for this target; that link is established for the target's server generation. Tests cover equal numeric lease values in different generations as distinct IDs. The legacy `input_owner` value remains the task's intentionally retained `client:<lease>` form. |
| Clock semantics | `owner_expires_secs` is copied unchanged to `expires_at_server_secs` and shown with an explicit server-clock label. No wall-clock conversion, client countdown, local expiry decision, or authorization check was added. |
| Pending expiry | A nonzero owner lease remains observable with its server deadline even if the deadline has elapsed; this preserves the server's current owner state while pending input may retain it. Displaying that lease does not confer authority. Claims and sends still pass through the existing revision/owner CAS paths, which remain server-authoritative. The logical-clock pending-expiry behavior is covered by existing core owner tests; the new CLI fixture deliberately does not wait for TTL. |
| Self comparison | `is_caller` is false for no owner and otherwise compares the nonzero server-issued owner lease to this pane link's lease. The link is target/generation-scoped, so the comparison is in the correct namespace. |
| Same observation | `pane_list` stores the full `SessionInfo` and caller lease in one `PaneOwner` per pane. `push_pane_json` derives both `input_owner` and `input_owner_info` from that same value; text output uses the same object. Revision, deadline, identity and self status therefore do not come from separate owner queries. The list is still a point-in-time sample per pane, not an atomic owner snapshot across the workspace; its existing layout freshness check does not claim one. |
| Authority boundary | The read path uses `describe_info`; owner-changing operations continue through `change_owner` and server validation. No new grant, wire field, persisted record, registration path, or dispatch path was introduced in this slice. |

The accompanying `tests/p3_owner_identity.rs` scenario checks none, caller and
other-client observations, stable identity/deadline across separate CLI
reconnects, neutral output, generation scoping, release/reclaim revision
changes, and refusal of another client's claim/release/send. Its presence is
source inspection only here; the parent owns final verification. This review
does not claim the full Phase 3 is complete or that human host acceptance has
passed.
