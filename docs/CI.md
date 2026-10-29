# Continuous integration

GitHub Actions runs the non-UI verification gate for changes pushed to `main`,
pull requests, and manual `workflow_dispatch` runs. The workflow is named **CI**
and exposes one check, **macOS arm64 verification**. It uses GitHub's
standard `macos-15` arm64 runner with the repository's pinned Rust toolchain
(1.99.1) and Python 3.11.

The job fetches the locked dependencies for both the root crate and
`vendor/crossterm-input`, then runs the canonical local command:

```sh
RUST_TEST_THREADS=1 python3 scripts/verify_non_ui.py
```

The script runs eight bounded checks: root Rust tests, vendored decoder tests,
component-source verification, formatting, strict all-target Clippy, a build,
Python unit tests, and documentation checks. Each command has a 420-second
limit, allowing the serialized root Rust suite to finish on slower runners. The
workflow prepares the Rust test build separately so compilation on a
cold hosted runner does not consume that per-command limit. The whole job is
limited to 40 minutes. Cargo download and build caches are keyed by runner
architecture, toolchain, and lockfiles; build caches also include vendored
sources. Build output is kept outside `vendor/`, whose contents are included
in release manifests.

The job needs only read access to repository contents. It does not use secrets,
deployments, or release credentials, and it does not persist the checkout
token. Pull requests from forks run with the same restricted permissions.
Superseded runs for the same change may be cancelled. On completion, the job
publishes a concise check table in the GitHub Actions summary and uploads logs
and the extracted verification summary for 14 days, including on failure.
Cancelled runs may not finish publishing evidence. Logs are visible to readers
of this public repository, so tests must use synthetic data and avoid secrets.

Standard GitHub-hosted compute is free for public repositories. Artifact and
cache storage have separate allowances; the short artifact retention limits
stored evidence. This workflow does not use paid larger runners or Blacksmith.

Dependabot proposes weekly GitHub Actions and root Cargo updates, with at most
three open PRs per ecosystem. There is no automatic merge. Cargo changes still
need the project's dependency/license/source review. Dependabot does not
maintain Panelink's local component patches or their fingerprint manifests.

## Running the same gate locally

Use the pinned Rust toolchain and Python 3.11, fetch both dependency graphs,
then invoke the same script:

```sh
cargo fetch --locked
cargo fetch --locked --manifest-path vendor/crossterm-input/Cargo.toml
RUST_TEST_THREADS=1 python3 scripts/verify_non_ui.py
```

The script's Cargo checks run offline after fetching. Fix the first failing
check and rerun the command; do not skip checks or retry failures automatically
to obtain a green result.

## What a green check establishes

A pass records the scripted component checks for that commit on the hosted
macOS arm64 runner. It does not complete product milestones or establish Apple
Terminal behavior, ignored host-specific tests, reboot recovery, or provider
acceptance. Those remain separate gates in the roadmap and their evidence.

The hosted baseline passed on PR #2 head `1060668` on 2026-10-26, as reported
by the PR watcher. Configure branch protection to require the exact
check name **macOS arm64 verification**. Branch protection is a repository
setting and is not changed by this workflow.

The initial setup adds validation and dependency-update proposals only. It does
not publish releases, sign binaries, run provider sessions, or add an automatic
agent repair loop. Future release automation should reuse [RELEASE.md](RELEASE.md).

## PR 3 wide-terminal failure

The first PR 3 hosted run at `ec09eb0` passed seven checks but failed the
wide-terminal attach test with connection loss. Local green evidence did not
establish a hosted pass. The scoped repair replaces timer-based socket waiting
with descriptor readiness while retaining existing deadlines; see
[the diagnosis, counterfactual and verification](progress/PR3-CI-SOCKET-READINESS-2026-10-27.md).

## Final-pane event observation

Run [40621974603](https://github.com/sahillamba/panelink/actions/runs/40621974603)
failed because the broad agent CLI test required a polling event observer to
report the last pane closing while that same close shut down the server.
The observer could be between requests when the server retired; its next
workspace-watch write then failed. A fixed startup sleep cannot synchronize
that shutdown boundary.

The test now checks the successful close receipt, server exit/socket removal,
and subsequent `no_workspace` result. The separate
`close_event_and_stale_index_remain_scoped_to_current_layout` test retains
close-event delivery coverage with a sibling pane keeping the server alive.
No test is ignored or deleted and no transport error is converted into success.

The local repair verifier also exposed a provider fixture race in
`failed_control_reply_latches_source_unavailable_without_reconnect`: receiving
the failure reply precedes the worker's completion notification. Its immediate
channel probe now uses a two-second bounded receive, preserving the failure,
unavailable-state and no-reconnect assertions. Production behavior is unchanged.
