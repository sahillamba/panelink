# P3 authenticated transport evidence

Date: 2026-10-18. Scope: src/transport.rs only. This implements a bounded
same-user connection boundary and private pathname listener; it does not add
server dispatch, capability authorization, a durable generation issuer, a
client context, or PTY/runtime behavior.

## Delivered

- AuthenticatedStream::new checks the kernel-reported peer UID through the
  reviewed peer_identity::require_same_user wrapper before setting the stream
  nonblocking. The stream is private and cannot be cloned or read before this
  check.
- read_frame first reads exactly the 24-byte wire header into stack storage.
  It validates the header before allocating at most 65,536 payload bytes. Only
  Hello, Welcome, Request, Response, and Error are accepted at this initial
  boundary. Snapshot and Event are rejected until a future runtime explicitly
  enables them.
- write_frame validates the initial message-kind and payload limits before
  I/O, then streams the existing codec's header and borrowed payload through a
  nonblocking deadline writer without building another whole-frame buffer.
- Read, write, and accept loops check absolute Instant deadlines on each
  iteration. Interrupted retries recheck the deadline; WouldBlock waits for
  at most one millisecond. Partial frames, EOF, malformed headers, I/O errors,
  and timeouts poison the stream. The connection cannot restart a frame after
  an incomplete write.
- PrivateListener::bind validates absolute unambiguous path syntax, walks
  directory components with no-follow descriptor opens, enforces trusted
  ancestor and exact private-directory ownership/modes, and creates the final
  directory exclusively at mode 0700 when absent. It refuses any existing
  control.sock entry. It records directory and socket device/inode identities,
  checks the directory pathname before and after binding, sets the socket to
  mode 0600, verifies type/owner/mode, and exposes only a nonblocking listener.
- Drop removes the socket only when the held and pathname directory identities
  still match and the endpoint still has the recorded socket inode. It never
  removes the directory or an unrelated/replaced endpoint.

Same-effective-UID authentication does not isolate one agent from another
process running as that user. Pathname bind resolves an absolute path after
descriptor-based validation; same-user processes remain within the documented
trust boundary and can race pathname changes. If the directory identity changes,
cleanup deliberately preserves the endpoint because ownership can no longer be
proved.

## Graph and dependency review

The existing Users-sahillamba-panelink graph was ready at 832 nodes and 3,759
edges. Search confirmed require_same_user, decode_header, Frame, encode_frame,
and the existing wire limits. No stale transport node existed. After the source
change, a fast nonpersistent refresh indexed 1,957 nodes and 6,112 edges;
target, docs, third_party, .git, and vendor were excluded.

The implementation uses existing std Unix sockets and the already selected
Rustix 1.1.5 filesystem/process APIs. It adds no package, feature, build target,
or manifest/lock change. rustix::fs::chmodat uses the macOS fchmodat path with
AT_SYMLINK_NOFOLLOW; the installed SDK declares that flag. No raw descriptor or
unauthenticated reader accessor was added.

## Actual verification

All commands ran on Rust/Cargo 1.99.1, target aarch64-apple-darwin, using a
temporary Cargo harness under /private/tmp/panelink-transport-harness that
imports peer_identity.rs, protocol_wire.rs, and transport.rs by path. This
avoids editing parent-owned module exports or Cargo files.

CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --offline --manifest-path /private/tmp/panelink-transport-harness/Cargo.toml -- --nocapture

Result: 31 passed, 0 failed, 2 ignored. The ignores are the existing peer
pathname fixture and the new listener lifecycle fixture. Passing transport
coverage includes authentication before reading queued bytes, frame decoding
and payload Debug redaction, malformed and unsupported headers before body
allocation, exact maximum control payload, partial header/body timeouts, EOF,
expired deadlines with data already queued, non-replayable stalled writes,
path syntax and ancestor checks, exclusive directory creation, and refusal to
unlink preexisting files or symlinks.

CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo clippy --offline --manifest-path /private/tmp/panelink-transport-harness/Cargo.toml --all-targets -- -D warnings

Result: exit 0.

rustfmt --check --edition 2021 src/transport.rs and
git diff --check -- src/transport.rs

Result: both exit 0.

The ignored pathname lifecycle test was also explicitly attempted:

CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --offline --manifest-path /private/tmp/panelink-transport-harness/Cargo.toml transport::tests::pathname_listener_authenticates_and_removes_only_its_socket -- --ignored --exact --nocapture

Actual result: exit 101 at UnixListener::bind, OS error 1,
PermissionDenied / Operation not permitted (EPERM). Bind, accept, pathname
permissions, drop cleanup, and replacement preservation remain pending external
verification; this attempt is not counted as a pass. Rerun the command above on
Apple Silicon macOS in an environment that permits pathname Unix socket binds.

## Integration boundary

The parent owns the lib.rs export and full-crate verification. The parent must
integrate and independently review this source before relying on the API. No
Phase 3 completion or pathname-authentication claim follows from this
standalone harness. File ownership for src/transport.rs and this evidence
note is released.

## Parent integration update — 2026-10-18

Parent reviewed and exported the source, added `prepare_directory` with
ancestor validation before mutation and descriptor-based chmod, and added a
bounded nonblocking pathname connect using the reviewed existing Rustix net
feature. See [P3-NET-REVIEW.md](P3-NET-REVIEW.md). The worker handoff above is
historical; integrated results are in [INTEGRATION.md](INTEGRATION.md).
The standalone temporary harness need not exist when resuming. External
listener lifecycle verification can use the repository directly:

```sh
CARGO_HOME=/private/tmp/panelink-p2-cargo-home cargo test --locked --offline --lib transport::tests::pathname_listener_authenticates_and_removes_only_its_socket -- --ignored --exact --nocapture
```
