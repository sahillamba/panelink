# Global request preflight seam — parent contract, 2026-10-18

Adopt a bounded prerequisite for routing before target lookup. Exclusive files:
src/server.rs and P3-GLOBAL-PREFLIGHT.md. No runtime/CLI/protocol changes,
dependencies, exports or commits. Preserve all current behavior and the parent's
latest scope-error propagation and denied-Submit assertions.

Introduce a private fixed decision enum, e.g. RequestPreflight::{Continue,
Rejected(RejectionCode), Known(OperationStatus)}. Continue is an observation,
never execution authority. Only Ledger::admit can reserve an operation. Decisions
contain no target or request text.

Move the existing control prefix to
GlobalAuthority::preflight_control(&self, connection, &ControlRequest, exact
payload, now). Preserve order: require_live_connection; key lease equals
connection; required capability by command; exact existing_admission; then return
Continue. Existing admission maps InProgress/Completed to Known; Execute is an
Internal error. Unauthorized cases return Rejected. Do not inspect a TargetState,
lookup a registry, admit/complete an operation, clone a ledger or run a sink.

SessionHandler::handle_control_inner still decodes the request, calls that
preflight, renders Rejected/Known exactly as today, and otherwise delegates to
private handle_control_preflighted(request,payload,now). The latter holds only
current full-target staleness, scope, command validation/owner preflight and actual
read/input admission/effect. It must not silently skip Ledger::admit. The outer
handle_control retains its existing metadata synchronization order.

Move the existing owner-mutation prefix similarly to
GlobalAuthority::preflight_owner_mutation(&self, connection,key,payload,now):
require live context, input capability, key lease AND generation, exact duplicate,
then Continue. The existing session wrapper already checked live context; the
helper's extra immutable check is bounded/idempotent and permits safe future use
before registry lookup. No lease renewal or other mutation occurs. A private
handle_owner_mutation_preflighted keeps target staleness/scope, candidate owner
logic, handoff recipient scope, actual admission/completion and reply. The
wrapper renders Rejected/Known as SessionReply::Payload. Preserve all input and
owner dedup ordering; never return cached process data under another key.

Extract the pure body of SessionHandler::encode_response into one private free
function so future owner routing can render preflight replies without an E or
TargetState. Keep the handler wrapper if useful. A private preflight_reply(key,
decision) -> Result<Option<Vec<u8>>,ServerError> can render current rejected/status
forms and return None for Continue. Exact wire/error/redaction and byte budgets
stay unchanged. Do not add global Describe/status/receipt routing in this step.

Existing 32 owner tests and real integration tests must remain intact. Add a
focused preflight contract test, if needed, proving a new-key decision does not
reserve or change global usage, exact cached results and changed-byte conflicts
work without reading/selecting a TargetState, and missing authority still rejects
before admission. This is a global-before-routing contract test, not a mirror of
field layout. No ignored tests or weakened assertions.

Verify exact owner filter, p3_socket_detach, p3_managed_launch, strict all-target
Clippy, package fmt, docs/diff with locked/offline writable Cargo cache. Parent
reviews source and runs integrated verifier before multi-owner assembly.
