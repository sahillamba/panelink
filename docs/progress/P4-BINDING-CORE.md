# P4 provider binding core — 2026-10-18

Status: the pure provider-binding state machine and focused tests are complete.
This is source-level ordering logic only; no provider adapter, runtime identity
claim, filesystem persistence, process launch, or resume behavior is enabled.

## Contract

`src/provider_binding.rs` configures one immutable complete `TargetRef`, an
opaque caller-canonicalized project identity (1–4,096 non-NUL bytes), exact
supported-version token (1–64 ASCII bytes from `[A-Za-z0-9.+-]`), and nonzero
active bridge instance. It accepts explicit observations for a selected top-level session
only. Child activity, mismatched target/project/version/bridge, malformed
fields, older revisions, and changed fields at an already seen revision are
rejected without mutating state. Session IDs are 1–128 ASCII alphanumeric,
underscore, dot, or hyphen bytes. Revisions, verification time, and bridge
instance are nonzero.

The state machine distinguishes `unbound`, `pending`, `verified`, `stale`,
`ambiguous`, and `unavailable`. Newer observations become pending; exact
same-revision duplicates are idempotent. `confirm_pending` calls a supplied
immutable-binding writer and moves to verified only on success. Writer failure
returns a fixed `StorageFailure` and preserves the previous confirmation and
current pending value. A mismatched acknowledgment never invokes the writer.
Invalidation removes the public pending view while retaining the latest valid
observation as a bounded revision/conflict watermark. At most one such latest
observation and one last-confirmed binding are held.

Restoration accepts a validated `LastConfirmedRecord` and always starts stale.
The record must match the configured target, project, and version. A bridge
replacement does not reset revision order: the first fresh observation must be
newer than the restored last-confirmed revision. The outer adapter must issue
ordered revisions across bridge instances. The wrapper validates record shape
but cannot prove that it came from durable storage; the writer callback's
success is likewise the caller's storage contract, not an independent
durability proof. The state machine makes no current-at-crash claim and never
initiates process or conversation actions.

All identity-bearing values and errors use redacted `Debug`; errors contain
fixed text only. Project identities are opaque bytes here: canonicalization,
source authenticity, provider-version support, and storage provenance remain
the adapter/storage caller's responsibility. The source enum retains only
`TuiRouteSnapshot` or `OrderedSelection`; this module proves neither source is
available nor ordered by a real provider.

## Verification

| Command | Result |
| --- | --- |
| `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline provider_binding` | Exit 0; 9 focused tests passed. |
| `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --locked --offline --lib --tests -- -D warnings` | Exit 0; strict library and test Clippy passed. |
| `CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo fmt --check` | Exit 0. |
| `python3 scripts/check_docs.py` | Exit 0; 106 Markdown files checked, zero findings. |
| `git diff --check` | Exit 0; owned-file whitespace and final-newline check also passed. |

Development corrections: the first compile caught an invalid format-string
expression in a redaction assertion. The first restore-order test also used a
different source field for its duplicate fixture; the test now compares every
field exactly. Clippy identified an unnecessary lazy `bool::then`; it was
replaced with an explicit state branch. The parent reviewed the bridge
replacement rule: revisions remain monotonic for the target across bridges.
The supported-version validator also rejects NUL, control, DEL, whitespace,
and non-ASCII characters while allowing prerelease/build token punctuation.

No dependencies or runtime integration were added. `src/lib.rs` export is
parent-owned; the parent added `pub mod provider_binding;` for focused tests.
