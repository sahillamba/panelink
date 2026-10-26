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
Python unit tests, and documentation checks. Each command has a 300-second
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

The first hosted baseline is pending until the workflow runs successfully on
GitHub. After that baseline, configure branch protection to require the exact
check name **macOS arm64 verification**. Branch protection is a repository
setting and is not changed by this workflow.

The initial setup adds validation and dependency-update proposals only. It does
not publish releases, sign binaries, run provider sessions, or add an automatic
agent repair loop. Future release automation should reuse [RELEASE.md](RELEASE.md).
