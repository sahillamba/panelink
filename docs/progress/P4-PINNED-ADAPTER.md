# Pinned OpenCode adapter contract

Status: implemented for the opt-in managed launch and pinned provider below.
Managed normal-UI A-B-A/new/fork/Home/project transitions, foreign manual
refusal and exact return pass Luna and independent parent native acceptance.
The [integrated result](P4-INTEGRATED-RESULT.md) records scoped Phase4 completion.

## Supported launch and scope

The adapter admits only native OpenCode **2.0.32**, pinned to SHA-256
`e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`. It is
opt-in through `serve --opencode-bridge`; ordinary launches and later generic
shell splits do not gain provider identity. The source belongs to the original
managed occupant and its complete target tuple, canonical project directory,
provider version, and fresh nonzero bridge instance. It does not follow a pane
replacement or another project.

Panelink writes a private per-launch configuration and plugin files, then
provisions one accepted same-user Unix connection with a one-use scoped grant.
The binary pin is checked before launch. Kernel same-UID credentials plus the
grant protect the channel; they are not independent attestation of which
same-user executable connected. The adapter adds no global listener, automatic
provider discovery, or executable authority to the bridge.

## Fresh observations and failure policy

The initial source accept has a **15 second** timeout.
After the source handshake and store load, the owner restores any saved record
as historical, advances a reconciliation epoch, and admits only a fresh
challenge-bound snapshot in that epoch. Snapshot requests run every **500 ms**;
each IO operation has a **300 ms** deadline, and a received frame expires after
**3,000 ms**. The lease starts at host receipt time. A queued frame cannot
extend it. An older in-flight epoch may preserve source sequence accounting but
cannot satisfy a newer reconciliation request.

There is one source connection and one bounded frame slot. Queue overflow,
malformed or timed-out IO and source loss report `Unavailable`; lease expiry
revokes eligibility as `Stale`, and storage failure reports `StorageFailed`.
All disable verified recovery. The source does not reconnect or receive a
replacement grant. A new owner reconciliation can follow a normal Home/Child/Unknown route
observation. Expired identity requires fresh explicit reconciliation; ordinary
heartbeats cannot revive it. Reconciliation does not reconnect a failed source
or clear storage ambiguity.

After the managed child starts, runtime storage, source, or provider identity
failure leaves its PTY and shell alive while disabling provider recovery. A
failure to prepare the opt-in private launch configuration or verify the pinned
binary prevents that opt-in launch from starting. Fixed operator outcomes
include `provider_recovery_unavailable`,
`provider_recovery_busy`, `provider_session_not_found`,
`provider_project_mismatch`, `provider_session_is_child`,
`provider_version_unsupported`, `provider_choice_unverified`, and
`recovery_interrupted_or_already_spent`. Uncertain outcomes do not invite an
automatic retry; query status or use the provider's own picker.

## Persistence and explicit recovery

The writer atomically publishes one joint managed-launch/provider record for
the exact target, project, pinned version/fingerprint, launch operation key, and
binding. The owner acknowledges only the exact worker completion and exact
current pending binding. A stale completion cannot confirm a newer pending
frame; the owner schedules that frame for its own save. Reopened bindings are
historical and restore stale. Inspection reports
`current_at_crash:false` and `automatic_resume:false`.

Manual recovery takes an explicit session ID. The pinned SDK checks the exact
ID, provider version, current canonical project, session project, and top-level
status. Missing/deleted, foreign-project, child, unsupported, and uncertain
results do not navigate. A successful prepare writes a separate one-shot
`Prepared` record and returns a ticket; it does not change the selected binding
or navigate. The operator must explicitly invoke resume. Resume revalidates the
same ID, durably records `Spent` before issuing at most one public-router
navigation action, then requires a later fresh selected snapshot and joint
binding save before returning `selected_and_saved`. Lost acknowledgments,
interrupted work, and spent tickets are never replayed automatically. Client
cancellation is checked before dispatch and again by the source worker before
navigation; if cancellation races with or follows a durable spend, that ticket
stays spent and cannot be submitted again.

No newest-session heuristic, `--continue`, automatic resume, conversation
replay, or current-at-crash guarantee is part of this adapter. Panelink history
and saved terminal state remain separate from provider conversations.

## Acceptance boundary and references

The contract and implementation are described in
[managed bridge runtime](P4-BRIDGE-RUNTIME.md),
[explicit recovery flow](P4-MANUAL-RECOVERY.md),
[native manual-recovery result](P4-NATIVE-MANUAL-RECOVERY.md),
[joint-store crash result](P4-MANAGED-CRASH.md), and
[native bridge-failure result](P4-NATIVE-BRIDGE-FAILURE.md). Existing focused
tests cover exact scope, challenge/epoch ordering, expiry, durable completion,
one-shot Prepared/Spent state, and process-kill/reopen boundaries. These do not
prove machine power-loss behavior.

Actual managed normal-UI acceptance passes for the pinned version, including
foreign-project selection, public structured manual refusal with the original
A record preserved, and same-instance UI return with a matching newer runtime/
disk revision. Exact nonce/sequence observer evidence establishes identity;
rendered titles are diagnostics. The disposable fixture maps local/global
pickers to noncolliding keys and uses the supported exact session-ID picker
lookup for return. No user-owned keymap change is shipped. The native default
Ctrl-X leader still conflicts with attach detach; the private acceptance
keymap does not claim general shortcut compatibility. Broader product/host,
power-loss and reboot gates remain separate.
