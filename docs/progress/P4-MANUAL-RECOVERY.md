# P4 explicit provider choice and last-confirmed recovery

Status: the opt-in managed adapter now wires native SDK validation, durable
prepared choice, separate explicit resume, and fresh selected-route confirmation.
Pinned native acceptance passes for missing/deleted, foreign-project, linked
child, and imported nonempty fork-root choices; explicit resume, fresh
selected-and-saved confirmation, replay refusal, and historical inspection are
also documented in [live evidence](P4-NATIVE-MANUAL-RECOVERY.md). The earlier
pure decision model below remains separate component evidence and is not the
runtime controller. Managed normal-UI foreign-project refusal and exact return
now pass separately; see [scoped Phase4 completion](P4-INTEGRATED-RESULT.md).

## Operator flow

The ordinary launcher does not enable this experimental adapter. A dedicated
`serve --opencode-bridge` launch must use the exact pinned OpenCode executable
and a canonical project directory. Use a short private server directory to fit
macOS Unix socket path limits. The bridge applies to that original managed
occupant; later ordinary shell splits do not inherit provider capability.

Read the full target tuple from the serve/control response. It is
`generation:workspace:pane:session:occupant_revision`; pane indices alone are
insufficient. Supply the same server directory, target and canonical project to
every command, in this option order:

```sh
panelink provider-binding prepare \
  --directory /path/to/server-state \
  --target 12:3:7:19:4 \
  --project /path/to/project \
  --session EXACT_PROVIDER_SESSION_ID
```

`prepared_choice_saved` means the exact top-level session was validated and a
one-shot choice was saved with its managed launch scope. It does not navigate
or claim that the session is selected. Retain the returned ticket and revision.
After reviewing the choice, explicitly request resume:

```sh
panelink provider-binding resume \
  --directory /path/to/server-state \
  --target 12:3:7:19:4 \
  --project /path/to/project \
  --ticket RETURNED_32_HEX_TICKET --revision RETURNED_REVISION
```

The owner revalidates the choice, durably marks the ticket spent, and issues at
most one navigation action. `selected_and_saved` is returned only after a fresh
selected-route snapshot and an atomic joint managed/provider record save. A
lost response is uncertain: query `provider-binding status` with the same base
options, or use the provider picker. Do not retry under another operation key.
One prepared choice is supported per managed launch; its record is not reset
or overwritten to authorize repeated recovery.

Missing/deleted, foreign-project, linked-child, unsupported or unverified choices
return fixed manual-selection errors without navigation. Use the provider's own
picker when verification is unavailable. No newest-session scan, `--continue`,
replacement conversation or automatic resume is used. Layout and saved Panelink
history remain independent from provider conversations.

After orderly shutdown releases the writer lock, inspect the exact saved record:

```sh
panelink provider-binding show \
  --directory /path/to/server-state \
  --target 12:3:7:19:4 \
  --project /path/to/project
```

`last_confirmed_conversation` labels historical metadata. The command fails
while the lifetime writer lock is held; use `status` for the live explicit
operation. All results keep `current_at_crash:false` and
`automatic_resume:false`. Reopened records never prove present provider state.
A prepared/spent record from another bridge instance cannot authorize a new
launch; interrupted operations require manual provider selection.

## Current implementation and evidence

`provider_recovery_runtime.rs` controls the real owner workflow;
`provider_recovery_endpoint.rs` accepts scoped same-user intent;
`provider_recovery_prepared.rs` saves one-shot state off the owner loop;
`managed_provider_store.rs` atomically links confirmed identity and managed
launch metadata. The TUI companion validates through the pinned provider SDK
and navigates through its public router. Binding confirmation uses a later
challenge-bound snapshot, not the navigation return value.

Actual private IPC/disk tests cover explicit prepare/resume and lost-response
ordering. SIGKILL/reopen tests cover joint mapping and Prepared/Spent boundaries;
they do not claim power-loss durability. Native evidence and remaining gates are
recorded separately in the linked live note. The native manual-recovery and
bridge-failure results pass for the pinned client; see
[manual recovery](P4-NATIVE-MANUAL-RECOVERY.md) and
[bridge failure](P4-NATIVE-BRIDGE-FAILURE.md). They do not establish normal
managed-picker foreign-project refusal, power-loss durability, current-at-crash
identity, or automatic resume. The historical component model's seven tests
below remain component evidence only.

## Independent decision-model slice

`src/provider_recovery.rs` retains the earlier pure recovery-plan model. Its
sealed proof has no production constructor and the live controller does not
call that model. Seven component tests exercise matching, rejected missing or
foreign/child/unsupported/uncertain inputs, revision overflow and a valid
proposal. Those tests do not verify native provider behavior. The live
controller's typed SDK response and private durable completion receipts enforce
the implemented sequence above.

No dependency was added. Codebase-memory was unavailable (`Transport closed`)
for the integration review; direct source reads were used.
