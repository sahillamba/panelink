# Local alpha release and installation

This is a local build-and-install path for an experimental Apple Silicon
macOS application. It is unsigned and not notarized. No binary download,
self-update, privileged helper, network call, `sudo`, or shell startup-file
edit is part of the procedure. Personal alpha installation is not firm review
or production approval.

## Build and verify

Requirements: Apple Silicon macOS, Rust/Cargo stable, Python 3, and the macOS
`sw_vers` and `otool` utilities. The build is locked and offline; a populated
Cargo cache is required. From a source checkout:

```sh
python3 scripts/release_manifest.py
```

The script refuses a non-clean Git working tree. The explicit evidence path
`python3 scripts/release_manifest.py --allow-dirty` records `dirty: true` and
`dirty_override_used: true`; it does not make that build a reviewed release.
It runs `cargo build --locked --offline --release` twice in separate fresh
private target directories under `${CARGO_TARGET_DIR:-${TMPDIR:-/tmp}}`, then
copies the first binary to `release/panelink-release`. `release/MANIFEST.json`
and `release/MANIFEST.md` record commit and exact tag (if present), dirty state,
`rustc -vV`, Cargo version, macOS version and architecture, lockfile/vendor/
notices hashes, artifact path/size/SHA-256, `otool -L` libraries, build command
and duration, and the second build's binary hash/duration. The vendor digest is
derived from sorted relative paths and SHA-256 values for all files in `vendor/`
at the time of the build, including files in generated vendor-local `target/`
directories if present. The two-build hash
comparison is an observation for these exact inputs, not a general
reproducibility guarantee.

Before installing, inspect both manifests and verify the artifact independently:

```sh
shasum -a 256 release/panelink-release THIRD_PARTY_NOTICES.md
```

`scripts/install.sh` validates the JSON schema and checks the binary and notices
SHA-256 values against `release/MANIFEST.json` before copying. A changed binary,
notices file, malformed manifest, or missing manifest fails closed. The manifest
does not cryptographically sign or authenticate the source checkout.

## Install

The default prefix is `$HOME/.local`; use an absolute `PREFIX` for another
user-writable location. Install the versioned command without changing an
existing development launcher:

```sh
scripts/install.sh install
```

The result is `$PREFIX/bin/panelink-release` and
`$PREFIX/share/panelink/THIRD_PARTY_NOTICES.md`. It does not add `$PREFIX/bin`
to `PATH`. The existing `$HOME/.local/bin/panelink` development symlink is
left alone. To explicitly replace/create that command as a symlink to the
release binary, request:

```sh
scripts/install.sh install --replace-dev
```

Only a symlink is eligible for replacement. The installer tracks exactly the
paths it installed in `$PREFIX/share/panelink/INSTALLED` and refuses unsafe,
unrecorded file replacements. It needs write permission to the selected prefix;
there is no `sudo` path. With `--replace-dev`, an existing development symlink
is preserved as `$PREFIX/bin/panelink.previous`; uninstall restores it.

## Upgrade, state compatibility, and rollback

Build and verify the newer checkout, then run:

```sh
scripts/install.sh upgrade
```

Upgrade checks the currently installed `panelink-release` for a reachable
workspace first. It refuses unless `panelink panes` positively reports no
workspace, and prints the `panelink stop` hint. If it cannot establish that
state (including inaccessible state/socket paths), it refuses. Stop the
workspace explicitly and retry. A successful upgrade saves the prior
`panelink-release` as `panelink-release.previous`, installs the verified binary
and notices, and keeps the previous binary available for rollback. It does not
launch, stop, or otherwise migrate a workspace.

This version changes no saved history or layout formats; the upgrade preserves
state and is state-compatible with the existing format. Layout snapshots,
history, running shells and the state root are not installation files.
Compatibility across future format changes is not implied.

If the workspace is stopped, return to the previous binary with:

```sh
scripts/install.sh rollback
```

Rollback swaps the current and previous binaries so either can be restored by
the next rollback. `panelink-release.previous` is removed on uninstall only
when listed in the installer's record.

## Uninstall and state root

Normal uninstall removes only paths listed in `INSTALLED`; it does not remove
untracked files in the prefix and never removes Panelink state:

```sh
scripts/install.sh uninstall
```

The state root defaults to
`~/Library/Application Support/Panelink`; `PANELINK_HOME` overrides it with an
absolute path. Panelink prepares this root and its `server`, `terminfo`, and
`contexts` directories with user-private mode `0700`; files are created with
private permissions. It contains runtime metadata, history and saved layouts.
The installer deliberately leaves it intact.

State deletion requires both explicit flags:

```sh
scripts/install.sh uninstall --purge-state --confirm-purge-state
```

This is the installer's explicit confirmation. It removes the selected state
root recursively after uninstalling tracked files. Check `PANELINK_HOME` before
using it. The flag pair was smoke-tested only against an isolated temporary
state path.

## Support, limits, and vulnerability/update process (SEC-08 stub)

- **Support target:** Apple Silicon (`arm64`) macOS only. Minimum macOS version
  is not yet selected or established by this packaging task.
- **Distribution:** unsigned local builds only; no signing, notarization,
  package-manager publication, hosted binaries, automatic update, or
  self-update. Installer and manifests make no authenticity guarantee.
- **Maturity:** experimental personal alpha. M6 still lacks real ten-agent
  acceptance and firm security review; this document does not close Phase 6.
- **Supported versions:** latest repository tag only. There is no promise for
  older tags or independently modified builds.
- **Owner/contact:** repository owner (the owner of `sahillamba/panelink`); use
  the repository's private issue/reporting channel for vulnerability reports.
  A public disclosure/contact policy has not been established.
- **Assessment/remediation:** before preparing a new tag, the repository owner
  should inspect the complete lockfile graph, run the native Cargo audit, triage
  reachable advisories, and update this process/evidence. Cargo advisory results
  are time-dependent and do not establish safety.
- **Rerun audit:** install/use the approved `cargo-audit` tool in the review
  environment and run `cargo audit --json` from the repository
  root. Record tool/database versions, date, output counts, dispositions, and
  any blocked database refresh. The advisory database is not shipped with this
  repository and may need an approved refresh; never report stale or unavailable
  results as a clean current audit.
- **Update path:** users obtain source from the repository owner and build
  locally using the procedure above. No automatic feed or binary update service
  exists. Organization-specific exceptions and firm review criteria remain
  unresolved.

The release manifest, installation exercise and audit stub satisfy only a
bounded part of proposed SEC-02/SEC-03/SEC-08 evidence. They are not a signed
release, a security audit, or approval to use the application with corporate
data.
