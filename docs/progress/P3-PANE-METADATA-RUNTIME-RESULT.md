# P3 managed-child metadata runtime result

Runtime integration for the accepted bounded pane metadata slice is implemented
alongside the pure codec and native sampler work. The multi-session server alone
offers capability 16. A schema-12 request must name the complete target and its
lease must have both Read and pane-metadata authority for that same target.
Native sampling runs on one detached worker. The owner thread submits at most one
job, returns `timed_out` after 500 ms, and keeps the slot busy until late work is
drained. It rechecks lease, target, child liveness, and the captured process
identity before publishing paths. No query reads or modifies PTY input, screen,
launch metadata, or history.

`TerminalSession` captures the direct child's opaque process identity immediately
after spawn; capture failure does not fail launch. Sampling is outside the PTY
spawn lock. Live children without a captured identity report Unsupported;
exited children report Exited.

The multi-session owner also supports the workspace observer's same-generation
resume after its original pane has been retired. It permits only the same lease,
exact original target, original Read|Workspace Events grant, and an exact Read plus
Workspace Events request. It renews the 1800-second expiry while preserving the
grant and requires a healthy matching workspace journal. It does not create a
lease or retarget the observer.

Focused evidence: metadata capability/exact-target authorization and no-effect
test passes; retired-target observer renewal passes twice across TTL boundaries
and rejects expiry, generation mismatch, and capability widening. A gated
`MultiServerDriver` test forces an expired owner deadline without sleeping,
verifies Read, workspace watch, and claimed PTY input continue during a blocked
sampler, observes TimedOut then Busy, releases the late sample and confirms a
new query is accepted without publishing its late path. The driver's later
query returns the current sample. `cargo check --locked --offline`, strict
all-target Clippy, the focused runtime test, and `git diff --check` pass. The
parent's real CLI metadata acceptance test also passes.
