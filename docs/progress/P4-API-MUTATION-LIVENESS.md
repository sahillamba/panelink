# Managed API-mutation liveness diagnostic

Status: bounded diagnostic implemented; four live runs show the API-mutation
window preserves exact A and managed ownership. The latest guarded picker
reached the exact new-root binding at revision 2 and the runtime reported a
matching `last_confirmed` revision. This is an isolation check, not a product
change or a replacement for managed transition acceptance.

The probe starts one opt-in managed Panelink/OpenCode target on fixture root A,
attaches a real PTY client, and waits for its exact last-confirmed binding. It
then creates the verified new root, imported fork root, and foreign-project
root through the fixture API on a background thread. The attach fixture's
lifetime drain thread consumes PTY output while the owner loop polls the serve
process and waits on the fixture work. The probe records fixed liveness
markers, bounded owner-poll timing, PTY byte counts, and SHA-256 identity
evidence. Raw output, titles, IDs, API bodies, and credentials are discarded.

After API fixture creation, the probe checks that the attach client and exact
managed pane remain live, the serve process still reports the original
last-confirmed revision, and the full-target binding remains on A. It then
uses the guarded native picker helper to select the API-created new root. The
test passes only if a later fresh source snapshot and exact durable binding
confirm that root at a higher revision. A pass would show that API fixture
mutation while the attach consumer drains does not trigger the earlier
observed client-exit/binding-loss pattern; it would not prove why the previous
ordering failed. If this controlled ordering still fails, the fixed report
distinguishes mutation completion, attach/pane/serve liveness, unchanged A,
picker progress, and durable selection without exposing session identifiers.

Run focused Python checks with:

```sh
python3 -m py_compile scripts/probe_panelink_api_mutation_liveness.py tests/test_panelink_api_mutation_liveness.py
python3 -m unittest -v tests.test_panelink_api_mutation_liveness
```

Run the live acceptance with the isolated Panelink binary:

```sh
python3 scripts/probe_panelink_api_mutation_liveness.py \
  --panelink-binary /private/tmp/panelink-p4-bridge-store-target/debug/panelink
```

The live runner requires native pinned OpenCode 2.0.32, a private disposable
fixture, and macOS's loopback-only sandbox with local Unix-socket access. It
submits no prompt or model request and performs complete child cleanup. The
separate
managed new/fork/Home/project UI acceptance remains governed by
[the managed bridge evidence](P4-MANAGED-BRIDGE.md); no user keymap change or
shipped terminal shortcut change is implied.

## Cleanup accounting correction — 2026-10-25

The final cleanup check now also requires the background fixture-mutation
thread to have terminated, on success, deadline and exception returns. The
join remains bounded; an outstanding finite API request is not claimed to be
cancelled, and its live worker makes cleanup incomplete. Serve cleanup is
attempted even when attach cleanup has already failed. A blocked/released
worker regression and all five diagnostic tests pass under parent rerun;
the combined diagnostic/fixture-helper suite passes ten tests. This corrects
the diagnostic evidence boundary without changing application behavior.

## Live results (2026-10-24 and 2026-10-25)

The pinned 2.0.32 run completed all background API fixture mutations while the
probe's foreground pump drained the PTY and polled the managed serve process.
The attach stayed alive, the exact pane and `client` input owner remained live,
and A stayed durably confirmed at revision 1 with the same in-memory hash.
The guarded picker showed one exact new-root row in attach and a fresh child
read, then sent Enter. No fresh selected binding was confirmed. That run's
schema did not capture final attach state, so it does not establish whether
that attach exited. A contemporaneous managed-transition run reported fixed
`terminal_output_stalled` after picker input. The first diagnostic reported a
746 ms foreground pump interval during fixture mutation, but its timer mixed
the foreground drain call with the interval and is not precise gap evidence.
Parent review identified the synchronous fresh child reads as a possible drain
gap. The probe was rerun with a continuous background PTY drainer; no product
timeout or API behavior was changed by this diagnostic.

The corrected run used the managed PTY's lifetime drain thread. Its fixed
report is `/private/tmp/panelink-p4-20261024/api-mutation-luna-final.json`.
All new-root, nonempty-fork, and foreign-project fixtures verified; the attach,
exact pane, and `client` input owner remained live through fixture creation;
A remained at revision 1 with the same SHA-256-derived fixture identity; and
the lifetime drainer stayed active while 59,190 PTY output bytes arrived.
The foreground owner completed 92 pump turns, with a maximum 25.137 ms gap
between turns. These are mutation-window observations, not a standalone proof
that foreground polling caused the prior stall.

The first final-guard run used the expanded seven-title exclusion set (A, B, new root,
fork root, imported fork parent, fork seed, and foreign root) and required two
identical fresh child reads at least 200 ms apart before Enter. During fixture
mutation, attach, pane, and serve stayed alive, input owner remained `client`,
and exact A revision 1 stayed unchanged. The owner loop completed 98 turns
with a maximum 33.263 ms interval; 70,022 PTY bytes were drained. The report
is again at `/private/tmp/panelink-p4-20261024/api-mutation-luna-final.json`.

After Enter, the exact API-created new-root lookup returned revision 2, while
the runtime was still `pending` at last-confirmed revision 1 in the first
snapshot read after row visibility. Review found the loop broke on that disk
row before polling for the matching runtime completion; the 7-second value is
only the maximum window, not elapsed wait. That result was inconclusive. The
harness was corrected to wait for both the exact newer row and a matching
`last_confirmed` revision before exiting.

The corrected rerun passed. The API-mutation window preserved exact A revision
1 while attach, pane, and serve remained live with `client` input ownership.
The seven-title exclusion set and two identical fresh child reads at least
200 ms apart guarded Enter. The exact API-created new-root record reached
revision 2, and runtime status was `last_confirmed` at revision 2. The attach,
pane, and serve stayed live after selection. The fixed-schema result is
`/private/tmp/panelink-p4-20261024/api-mutation-luna-final.json`; fixture
user-message checks passed, no prompts were submitted, and cleanup completed.
