# External pathname/CLI acceptance fixture — 2026-10-18

Exclusive next owner: tests/p3_pathname.rs, docs/progress/P3-PATHNAME.md. Parent
owns shared docs/exports; CLI owner edits p3_cli.rs. Read current ROADMAP/AGENTS/
P3-CLI-TASK and product prerequisites. No production edits/dependencies/commits.

Add one explicitly ignored macOS/aarch64 integration scenario for a real
pathname listener and actual binary frontend (env CARGO_BIN_EXE_panelink).
Reason: sandbox has demonstrably denied pathname bind with EPERM. Explicitly
attempt the ignored test here and record the actual failure, never auto-skip or
call it passed. It should be directly runnable externally with one Cargo command.

Create private fixture directory0700 under /private/tmp, compile checked-in
terminfo with installed /usr/bin/tic into own fixture using finite child polling.
Launch actual binary `server-probe --directory <empty runtime dir> --terminfo
<compiled dir> --initialize --allow-input -- /bin/sh -c <controlled loop>`.
All argv/env/cwd explicit for fixture, only benign synthetic data; no global
config/home edits. Child script emits a PID/ready marker and one application GOT
per submitted line. Record stdout/stderr in bounded owned fixture streams;
readiness requires real started state/process survival, not a sleep alone.

Invoke short-lived actual `control --directory ... --context <private parent>
init`, describe, claim explicit returned TARGET/owner revision, Submit unique
token, Read current state, Detach and another Describe/Read. Verify context lease
and generation reused across invocations, owner revision explicit, child remains
same, application handled token once. Outputs are machine JSON; no new serde
dependency. A tiny parser for known fixture numeric/target fields is sufficient;
do not pretend it is a general JSON implementation. Parent socketpair test already
covers lostack; don't add hidden CLI mutation-retry/debugflags.

Check directory0700/socket0600 and owner UID, existing endpoint not unlinked by
a second server attempt. Explicitly stop the foreground server (SIGTERM flag)
with finite wait and verify endpoint cleanup. Use a guard so failures don't leave
normal test processes behind; signal only own spawned processes, no broad pgrep
or name-based kill. Record behavior if a native operation cannot be bounded; no
indefinite subprocess output/wait. Do not claim hostterminal/provider/performance
acceptance. Borrow root modules/CLI source as read-only to adapt exact output names.

Worker fmt-owned file, actual command/output evidence and honest blocked result.
Parent independently reviews and reruns required checks. No whole-phase DONE.
