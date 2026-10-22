# Phase 6 security-review pack evidence — 2026-10-22

Scope: documentation-only mapping of SEC-01..SEC-08 in
[`SPEC.md`](../SPEC.md#proposed-security-review-requirements) to current
repository evidence. This records commands actually run in this worker session.
It does not certify controls or substitute for organization review. The named
Rust/integration tests below were located as evidence references; this worker
did not execute those application tests.

## Source and test evidence locations

Source locations and behaviors were explored from the indexed Panelink source
before drafting [`SECURITY-REVIEW.md`](../SECURITY-REVIEW.md). Important current
references include:

- `src/transport.rs:31-33, 321-355`: fixed `control.sock`, private directory,
  existing socket refusal, chmod `0600`.
- `src/peer_identity.rs:41-48, 50-71`: kernel peer UID check before framed
  application input; `src/transport.rs:161-173` invokes it at construction.
- `src/default_open.rs:85-103, 110-202, 286-297, 408-419, 434-478`:
  state-root paths, ancestor/directory validation, private metadata/log writes,
  detached server launch and lifecycle PID metadata.
- `src/server_storage.rs:36-98, 129-138, 202-303`: `0700` server directory,
  `0600` single-link files, generation and metadata-only launch journal.
- `src/client_context.rs:22-27, 192-239`: `0700` context parent and `0600`
  context file, containing lease/sequence/uncertain-operation metadata.
- `src/history_store.rs:17-22, 69-79, 300-340, 752-773` and
  `src/layout_store.rs:21-24, 96-106, 151-201, 443-509`: history/layout bounds,
  file/directory permissions, validation and target-scoped clear/atomic saves.
  `src/multi_runtime.rs:526-567` wires history capture; current CLI/runtime
  setup enables capture rather than exposing an end-user off setting.
- `src/recovery_links.rs:15-25, 99-147, 287-315`: bounded 30-day links and
  protected link store.
- `src/launch.rs:182-235`, `src/managed_launch_prepare.rs:106-169`, and
  `src/terminal/mod.rs:834-913`: explicit child launch environment and PTY
  spawn path.
- `src/terminal/mod.rs:698-787`, `vendor/vt100/src/callbacks.rs:24-36`,
  `src/protocol_wire.rs:11-28, 216-274`, and
  `vendor/vt100/src/cell.rs:5-66`: clipboard callback handling, bounded wire
  frame validation, fixed-size OSC engine and combining-scalar cap.
- `vendor/crossterm-input/src/decoder.rs:6-7` and
  `vendor/crossterm-input/src/parse.rs:5`: fixed host decoder limits (16,384
  ordinary bytes, 65,548 paste frame bytes, 65,536 paste payload bytes).
- `src/ui_client/conn.rs:270-315, 318-343`, `src/input_owner.rs:284-334`, and
  `tests/p3_detach.rs:156-398`: status reconciliation and nonreplayed input.

Named test references in the pack (located by source search, not executed here):

- `every_kind_obeys_its_exact_payload_boundary`
  (`src/protocol_wire.rs:491`)
- `u32_max_length_is_rejected_without_a_body_allocation`
  (`src/protocol_wire.rs:564`)
- `payload_ceiling_trailing_bytes_and_control_classification_are_exact`
  (`src/control_protocol.rs:1001`)
- `clipboard_reads_writes_and_clears_never_queue_child_input`
  (`src/terminal/unit_tests.rs:75`)
- `clipboard_denial_does_not_inject_reply_bytes_before_the_next_shell_input`
  (`tests/p2_pty.rs:274`)
- `fixed_osc_storage_survives_chunked_oversize_and_combining_is_capped`
  (`src/terminal/unit_tests.rs:203`)
- `logical_detach_resume_and_lost_input_ack_never_relaunch_or_replay`
  (`tests/p3_detach.rs:157`)
- `expiry_and_clear_are_scoped_to_ended_targets`
  (`tests/p5_history_store.rs:108`)
- `retention_prunes_links_older_than_thirty_days`
  (`tests/p5_recovery_links.rs:199`)
- `debug_output_redacts_environment_values_and_override_values`
  (`src/launch.rs:622`)
- `fixed_errors_and_debug_redact_request_data`
  (`src/managed_launch_prepare.rs:565`)
- Provider identity limitation and observations:
  [`P4-OPENCODE-IDENTITY-PROBE.md`](P4-OPENCODE-IDENTITY-PROBE.md#boundaries-and-observations).
- Dependency inventory and audit result:
  [`P6-DEPENDENCY-INVENTORY.md`](P6-DEPENDENCY-INVENTORY.md#advisory-scan).

## Executed repository scans

Commands below were run from the repository root. For `rg`, exit code 1 means
no match; wrappers printed the explicit marker shown.

### TCP / HTTP clients and URL literals

Command:

```sh
rg -n -e '\bTcpListener\b' -e '\bTcpStream\b' -e '\breqwest\b' -e 'https?://' src Cargo.toml; rc=$?; if [ "$rc" -eq 1 ]; then printf '[no matches; exit=1]\n'; elif [ "$rc" -ne 0 ]; then printf '[rg exit=%s]\n' "$rc"; fi
```

Actual output:

```text
[no matches; exit=1]
```

Scope is only `src/` and root `Cargo.toml`; this does not search dependency
source, documentation links, or child-agent behavior.

### Telemetry, automatic/self-update and privileged-helper terms

Command:

```sh
rg -n -i -e 'telemetry' -e 'self.?update' -e 'auto.?update' -e 'update.?download' -e 'privileged helper' src Cargo.toml; rc=$?; if [ "$rc" -eq 1 ]; then printf '[no matches; exit=1]\n'; elif [ "$rc" -ne 0 ]; then printf '[rg exit=%s]\n' "$rc"; fi
```

Actual output:

```text
[no matches; exit=1]
```

This is a text scan, not a formal whole-graph capability audit.

### Clipboard, parser-bound, retention and nonreplayable-input test names

Command:

```sh
rg -n 'clipboard_reads_writes_and_clears_never_queue_child_input|clipboard_denial_does_not_inject_reply_bytes_before_the_next_shell_input|logical_detach_resume_and_lost_input_ack_never_relaunch_or_replay|fixed_osc_storage_survives_chunked_oversize_and_combining_is_capped|expiry_and_clear_are_scoped_to_ended_targets|retention_prunes_links_older_than_thirty_days|debug_output_redacts_environment_values_and_override_values|fixed_errors_and_debug_redact_request_data|every_kind_obeys_its_exact_payload_boundary|u32_max_length_is_rejected_without_a_body_allocation|payload_ceiling_trailing_bytes_and_control_classification_are_exact' src/terminal/unit_tests.rs tests/p2_pty.rs tests/p3_detach.rs tests/p5_history_store.rs tests/p5_recovery_links.rs src/launch.rs src/managed_launch_prepare.rs src/protocol_wire.rs src/control_protocol.rs
```

Actual output (11 matches):

```text
src/control_protocol.rs:1001:    fn payload_ceiling_trailing_bytes_and_control_classification_are_exact() {
src/managed_launch_prepare.rs:565:    fn fixed_errors_and_debug_redact_request_data() {
src/protocol_wire.rs:491:    fn every_kind_obeys_its_exact_payload_boundary() {
src/protocol_wire.rs:564:    fn u32_max_length_is_rejected_without_a_body_allocation() {
src/launch.rs:622:    fn debug_output_redacts_environment_values_and_override_values() {
src/terminal/unit_tests.rs:75:fn clipboard_reads_writes_and_clears_never_queue_child_input() {
src/terminal/unit_tests.rs:203:fn fixed_osc_storage_survives_chunked_oversize_and_combining_is_capped() {
tests/p5_recovery_links.rs:199:fn retention_prunes_links_older_than_thirty_days() {
tests/p3_detach.rs:157:fn logical_detach_resume_and_lost_input_ack_never_relaunch_or_replay() {
tests/p5_history_store.rs:108:fn expiry_and_clear_are_scoped_to_ended_targets() {
tests/p2_pty.rs:274:fn clipboard_denial_does_not_inject_reply_bytes_before_the_next_shell_input() {
```

### Unsafe source inventory

Command:

```sh
rg -n '\bunsafe\s*\{' src --glob '*.rs'
```

Actual output (19 `unsafe { ... }` occurrences):

```text
src/default_open.rs:192:    unsafe { libc::getuid() }
src/default_open.rs:355:    let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
src/default_open.rs:358:        unsafe { libc::flock(fd, libc::LOCK_UN) };
src/default_open.rs:389:        if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
src/default_open.rs:461:    unsafe {
src/default_open.rs:484:    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
src/default_open.rs:487:    let written = unsafe {
src/default_open.rs:822:    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
src/peer_identity.rs:56:    let result = unsafe { libc::getpeereid(fd.as_raw_fd(), &mut uid, &mut gid) };
src/p3_cli.rs:1595:    let result = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) };
src/server_io.rs:888:        let result = unsafe {
src/transport.rs:1544:        let result = unsafe {
src/terminal/pty.rs:22:    let raw_controller = unsafe { libc::posix_openpt(flags) };
src/terminal/pty.rs:29:    let controller = unsafe { OwnedFd::from_raw_fd(raw_controller) };
src/terminal/pty.rs:51:    if unsafe { libc::grantpt(controller) } != 0 {
src/terminal/pty.rs:60:    if unsafe { libc::unlockpt(controller) } != 0 {
src/terminal/pty.rs:70:    let result = unsafe {
src/terminal/mod.rs:881:        unsafe {
src/terminal/mod.rs:1513:        let (pid, session_id, tty_session_id, process_group, foreground_group, is_tty) = unsafe {
```

The separate review script counted the same 19 and printed a nearby
`SAFETY:`/`Safety:` comment for every occurrence. All 19 are in application
`src/`; vendor source had no matching block in this scoped scan. This inventory
does not inspect macro-expanded unsafe or establish that each rationale is
sufficient.

Command used to count and print one nearby rationale for each source match:

```sh
python3 -c 'from pathlib import Path; import re
files=sorted(Path("src").rglob("*.rs")); pat=re.compile(r"\bunsafe\s*\{"); uses=[]
for path in files:
 lines=path.read_text(errors="replace").splitlines()
 for i,line in enumerate(lines):
  if pat.search(line):
   safety=next((lines[j].strip() for j in range(i-1,max(-1,i-7),-1) if "SAFETY:" in lines[j] or "Safety:" in lines[j]), "NO SAFETY/Safety comment in prior 6 lines")
   uses.append((path,i+1,safety))
print(f"unsafe block expressions in src/: {len(uses)}")
for path,line,safety in uses: print(f"{path}:{line}: {safety}")'
```

Actual output was 19 lines (one count plus 19 locations and nearby first
`SAFETY:`/`Safety:` comment lines); those locations/comments are transcribed in
[`SECURITY-REVIEW.md`](../SECURITY-REVIEW.md#unsafe-blocks-counted-in-src-19).

## Existing dependency/release observations (not rerun here)

- [`P6-DEPENDENCY-INVENTORY.md`](P6-DEPENDENCY-INVENTORY.md) records the exact
  `cargo audit --json` observation: advisory database commit
  `4b587f00996ebe5c05a73eaef105ef241e876414`, 1,348 advisories,
  `vulnerabilities: found=false, count=0`, no unmaintained/unsound/notice
  warnings. It also records offline metadata command exit 101 and target-filtered
  tree exit 0.
- [`P6-MEASUREMENTS.md`](P6-MEASUREMENTS.md) records Rust 1.99.1, host/OS,
  release builds, sizes, and explicitly synthetic proxy workload results.
- `docs/RELEASE.md` was added concurrently after the earlier presence check and
  was read for this review. It documents an unsigned local release procedure,
  controlled-source/build manifest fields, latest-tag support target, owner
  contact stub and proposed audit/update process. It expressly states no
  signature/authenticity guarantee, no signing/notarization, no hosted binaries
  or automatic update; this pack links it but did not edit it.
- The user-designated release evidence file appeared in the working tree during
  the concurrent documentation pass. Presence check:

  ```text
  $ python3 -c 'from pathlib import Path
  for value in ("docs/RELEASE.md", "docs/progress/P6-RELEASE-EVIDENCE.md"):
   path=Path(value)
   print(f"{value}: {path.is_file()}")'
  docs/RELEASE.md: True
  docs/progress/P6-RELEASE-EVIDENCE.md: True
  ```

  Its execution command/output and manifest-value sections are blank in the
  version read by this worker, so those results remain unverified and are not
  counted as completed release acceptance here.

## Documentation checker

Required final command and actual result on the completed pack:

```text
$ python3 scripts/check_docs.py
Checked 205 Markdown file(s); 0 finding(s).
exit code: 0
```
