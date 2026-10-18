# Explicit launch preparation adapter — parent contract, 2026-10-18

Adopted bounded prerequisite for milestone 2; no new endpoint or authority.
Exclusive files: new src/managed_launch_prepare.rs, existing
 tests/p3_managed_launch.rs, and P3-LAUNCH-PREPARATION.md. Parent owns export.
Current macOS/aarch64 only; no dependencies or other/shared file edits.

Implement `prepare_launch(request: LaunchRequestView<'_>, target: TargetRef,
terminfo_directory: &Path) -> Result<PreparedLaunch, PrepareError>`. Require all
five target fields nonzero and target generation equals request key generation.
Do not constrain the hierarchy beyond that; registry supplies its prototype IDs.

Capture the request's raw Unix argument/path/environment bytes losslessly into
owned OsStrings. Create LaunchContext from the explicit absolute client cwd
and captured client environment. For this prototype account defaults are empty,
there are no profile/override layers, and no daemon process environment is read.
Call existing build_environment with owned TERM=panelink-probe, TERMINFO equal
to the explicitly supplied canonical absolute terminfo directory, and owned
PANELINK_GENERATION/PANELINK_SESSION containing the selected numeric IDs.
Owned values replace any captured TERM/TERMINFO. Caller PANELINK_* was rejected
by the codec and must remain rejected by environment validation. This is a
prototype adapter policy, not a choice of named account profiles/defaults.

The supplied terminfo path must be absolute, NUL-free, nonempty, <=4096 raw
bytes, canonicalize to an existing directory, and its canonical path must also
be <=4096 bytes. No ambient cwd resolution. This checks the directory only;
compiled-entry availability is a separate runtime/host prerequisite. Invoke
existing resolve_launch with captured context and the resolved environment;
then bound canonical executable and cwd paths individually to4096 raw bytes.
Use TerminalSize::new for the already-validated dimensions. No shell expansion,
global env mutation, credentials logging or persistence. No filesystem mutation.

PreparedLaunch owns ResolvedLaunch, arguments, Environment and TerminalSize.
Expose immutable getters for focused assertions plus a consuming spawn method
that only invokes TerminalSession::spawn through its existing gate. This is a
trusted adapter: caller authorization and durable Intent must precede spawn.
Its spawn error must not leave an unowned child; the current TerminalSession
constructor allocates/validates before spawn and has no fallible step after
acquiring its Child. Debug/errors use fixed redacted text and do not expose
paths, arguments, env, target IDs or original filesystem errors.

Add meaningful unit tests with controlled private directories/executable
fixtures: explicit captured client cwd/PATH resolution, raw non-UTF8 argv/env,
owned overrides, reserved-name denial (codec or environment boundary), absent
PATH for basename, relative/empty PATH components rejection through resolver,
invalid/mismatched targets, relative/missing/non-directory terminfo, path byte
bounds and redaction. Compare exact effective environment keys to prove no
ambient daemon entries enter it. No parent env mutation or dependency downloads.

Replace the test-only hand-built preparation helper in p3_managed_launch with
this real adapter; pass the coordinator's selected target and explicit fixture
terminfo directory. Preserve all deduplication/capacity/PID/post-fault ACK and
cleanup assertions. Use the consuming spawn adapter and existing PtyEffects.
The fixture directory need not contain a compiled entry for its simple /bin/sh
children; explicitly record that this is not full-screen host acceptance.

Run focused `managed_launch_prepare::tests::`, p3_managed_launch, strict
all-target Clippy, package fmt, docs/diff checks with locked/offline Cargo and
writable cache. Parent reviews and reruns integrated verifier. No commits.
