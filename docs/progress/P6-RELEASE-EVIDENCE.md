# Phase 6 local release and installation evidence — 2026-10-22

Status: bounded release/installer implementation and lifecycle exercise for
the user-selected local build + install-script, no-signing path. This does not
close Phase 6 or establish firm approval, signing, broader macOS compatibility,
or real ten-agent acceptance.

## Proposed criteria

- Refuse a dirty checkout unless explicitly allowed; record source/toolchain/
  input/artifact metadata and an independent-build hash comparison.
- Install into a temporary prefix, leave the existing dev symlink alone, run
  the installed version, upgrade with rollback, roll back, then uninstall
  without state deletion.
- Run the release tests and Markdown hygiene check.

## Manifest build (executed)

Command:

```text
python3 scripts/release_manifest.py --allow-dirty
```

Actual output:

```text
release_manifest: commit=ebdb84e5a6850fccd4ae068d1c764f2c7cf81101 dirty=true binary_bytes=2033968 sha256=52e77f09fda35e0b256d998602cdb37309d6315d6b8d76e98f74a912027d3489 independent_sha256_identical=true
release/MANIFEST.json
release/MANIFEST.md
```

Recorded values: commit `ebdb84e5a6850fccd4ae068d1c764f2c7cf81101`, tag
`null`, Rust/Cargo `1.99.1`, macOS `26.6.1`, architecture `arm64`, Cargo.lock
SHA-256 `7d281bb10eec384b2632f0d8118288051093d25be9f958b64aa05a33a9975b16`,
notices SHA-256 `41584cd72018cde0458d96d423cd75eb94fa4067a5b98b1bed0f01e1a63c5298`,
binary 2,033,968 bytes and SHA-256
`52e77f09fda35e0b256d998602cdb37309d6315d6b8d76e98f74a912027d3489`.
The complete vendor tree hash, all 1,008 file paths/hashes, build target
directories, complete toolchain records and `otool -L` output are in
`release/MANIFEST.json`; the tree hash includes observed vendor-local build
artifacts. Builds in distinct private target directories took 6.571 s and
6.642 s and produced equal binary hashes. Linked libraries: `/usr/lib/libiconv.2.dylib`
and `/usr/lib/libSystem.B.dylib`.

The worktree was dirty, and `--allow-dirty` was passed; the manifest records
`source.dirty: true` and `source.dirty_override_used: true`. Hash equality is
only an observation of these two builds, not a general reproducibility claim.

## Direct install lifecycle (executed)

The following command used a temporary `PREFIX`, `HOME`, and `PANELINK_HOME`
with the built artifact. It did not access the real default state root.

```text
test_root=$(mktemp -d "${TMPDIR:-/tmp}/panelink-release-check.XXXXXX"); mkdir -p "$test_root/home"; PREFIX="$test_root/prefix" HOME="$test_root/home" PANELINK_HOME="$test_root/state" sh scripts/install.sh install; HOME="$test_root/home" PANELINK_HOME="$test_root/state" "$test_root/prefix/bin/panelink-release" --version; PREFIX="$test_root/prefix" HOME="$test_root/home" PANELINK_HOME="$test_root/state" sh scripts/install.sh upgrade; HOME="$test_root/home" PANELINK_HOME="$test_root/state" "$test_root/prefix/bin/panelink-release" --version; PREFIX="$test_root/prefix" HOME="$test_root/home" PANELINK_HOME="$test_root/state" sh scripts/install.sh rollback; HOME="$test_root/home" PANELINK_HOME="$test_root/state" "$test_root/prefix/bin/panelink-release" --version; PREFIX="$test_root/prefix" HOME="$test_root/home" PANELINK_HOME="$test_root/state" sh scripts/install.sh uninstall; test ! -e "$test_root/state"
panelink installer: installed /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.0PqErj/prefix/bin/panelink-release
panelink 0.1.0
panelink installer: upgraded /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.0PqErj/prefix/bin/panelink-release; previous binary saved as /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.0PqErj/prefix/bin/panelink-release.previous
panelink 0.1.0
panelink installer: rolled back /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.0PqErj/prefix/bin/panelink-release; prior binary retained as /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.0PqErj/prefix/bin/panelink-release.previous
panelink 0.1.0
panelink installer: uninstalled; Panelink state was preserved
```

An additional `--replace-dev` smoke test used a synthetic development
symlink in another temporary prefix and asserted it was restored on
uninstallation:

```text
test_root=$(mktemp -d "${TMPDIR:-/tmp}/panelink-release-check.XXXXXX"); mkdir -p "$test_root/home"; PREFIX="$test_root/prefix" HOME="$test_root/home" PANELINK_HOME="$test_root/state" sh scripts/install.sh install; HOME="$test_root/home" PANELINK_HOME="$test_root/state" "$test_root/prefix/bin/panelink-release" --version; PREFIX="$test_root/prefix" HOME="$test_root/home" PANELINK_HOME="$test_root/state" sh scripts/install.sh upgrade; HOME="$test_root/home" PANELINK_HOME="$test_root/state" "$test_root/prefix/bin/panelink-release" --version; PREFIX="$test_root/prefix" HOME="$test_root/home" PANELINK_HOME="$test_root/state" sh scripts/install.sh rollback; HOME="$test_root/home" PANELINK_HOME="$test_root/state" "$test_root/prefix/bin/panelink-release" --version; PREFIX="$test_root/prefix" HOME="$test_root/home" PANELINK_HOME="$test_root/state" sh scripts/install.sh uninstall; test ! -e "$test_root/state"
panelink installer: installed /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.lKMyYZ/prefix/bin/panelink-release
panelink 0.1.0
panelink installer: upgraded /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.lKMyYZ/prefix/bin/panelink-release; previous binary saved as /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.lKMyYZ/prefix/bin/panelink-release.previous
panelink 0.1.0
panelink installer: rolled back /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.lKMyYZ/prefix/bin/panelink-release; prior binary retained as /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.lKMyYZ/prefix/bin/panelink-release.previous
panelink 0.1.0
panelink installer: uninstalled; Panelink state was preserved
```

```text
test_root=$(mktemp -d "${TMPDIR:-/tmp}/panelink-release-check.XXXXXX"); mkdir -p "$test_root/home"; printf '#!/bin/sh\nexit 0\n' > "$test_root/dev-panelink"; chmod 755 "$test_root/dev-panelink"; mkdir -p "$test_root/prefix/bin"; ln -s "$test_root/dev-panelink" "$test_root/prefix/bin/panelink"; PREFIX="$test_root/prefix" HOME="$test_root/home" PANELINK_HOME="$test_root/state" sh scripts/install.sh install --replace-dev; test "$(readlink "$test_root/prefix/bin/panelink")" = panelink-release; test -L "$test_root/prefix/bin/panelink.previous"; PREFIX="$test_root/prefix" HOME="$test_root/home" PANELINK_HOME="$test_root/state" sh scripts/install.sh upgrade; PREFIX="$test_root/prefix" HOME="$test_root/home" PANELINK_HOME="$test_root/state" sh scripts/install.sh rollback; PREFIX="$test_root/prefix" HOME="$test_root/home" PANELINK_HOME="$test_root/state" sh scripts/install.sh uninstall; test -L "$test_root/prefix/bin/panelink"; test "$(readlink "$test_root/prefix/bin/panelink")" = "$test_root/dev-panelink"
panelink installer: installed /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.X0oMwJ/prefix/bin/panelink-release
panelink installer: upgraded /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.X0oMwJ/prefix/bin/panelink-release; previous binary saved as /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.X0oMwJ/prefix/bin/panelink-release.previous
panelink installer: rolled back /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.X0oMwJ/prefix/bin/panelink-release; prior binary retained as /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.X0oMwJ/prefix/bin/panelink-release.previous
panelink installer: uninstalled; Panelink state was preserved
```

An explicit purge smoke test used a temporary state path, with both required
confirmation flags; the final absent-path assertion succeeded:

```text
test_root=$(mktemp -d "${TMPDIR:-/tmp}/panelink-release-check.XXXXXX"); mkdir -p "$test_root/home"; PREFIX="$test_root/prefix" HOME="$test_root/home" PANELINK_HOME="$test_root/state" sh scripts/install.sh install; PREFIX="$test_root/prefix" HOME="$test_root/home" PANELINK_HOME="$test_root/state" sh scripts/install.sh uninstall --purge-state --confirm-purge-state; test ! -e "$test_root/state"
panelink installer: installed /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.7kQra0/prefix/bin/panelink-release
panelink installer: uninstalled; confirmed state purge completed for /var/folders/zk/280x24r52md1tjr11y6h1wm00000gp/T//panelink-release-check.7kQra0/state
```

## Automated checks (executed)

`python3 -m unittest tests.test_release_scripts -v`:

```text
test_GIVEN_modified_artifact_WHEN_install_requested_THEN_manifest_mismatch_prevents_copy (tests.test_release_scripts.InstallerLifecycleTests.test_GIVEN_modified_artifact_WHEN_install_requested_THEN_manifest_mismatch_prevents_copy) ... ok
test_GIVEN_purge_without_confirmation_WHEN_uninstall_requested_THEN_fails_without_state_change (tests.test_release_scripts.InstallerLifecycleTests.test_GIVEN_purge_without_confirmation_WHEN_uninstall_requested_THEN_fails_without_state_change) ... ok
test_GIVEN_release_fixture_WHEN_installed_upgraded_rolled_back_and_uninstalled_THEN_only_tracked_files_change (tests.test_release_scripts.InstallerLifecycleTests.test_GIVEN_release_fixture_WHEN_installed_upgraded_rolled_back_and_uninstalled_THEN_only_tracked_files_change) ... ok
test_GIVEN_replace_dev_option_WHEN_installed_then_uninstalled_THEN_original_symlink_is_restored (tests.test_release_scripts.InstallerLifecycleTests.test_GIVEN_replace_dev_option_WHEN_installed_then_uninstalled_THEN_original_symlink_is_restored) ... ok
test_GIVEN_workspace_running_WHEN_upgrade_requested_THEN_refuses_with_stop_hint (tests.test_release_scripts.InstallerLifecycleTests.test_GIVEN_workspace_running_WHEN_upgrade_requested_THEN_refuses_with_stop_hint) ... ok
test_GIVEN_file_and_sorted_vendor_tree_WHEN_hashed_THEN_stable_content_digests_are_returned (tests.test_release_scripts.ManifestHelperTests.test_GIVEN_file_and_sorted_vendor_tree_WHEN_hashed_THEN_stable_content_digests_are_returned) ... ok
test_GIVEN_malformed_or_wrong_schema_manifest_WHEN_loaded_THEN_rejected (tests.test_release_scripts.ManifestHelperTests.test_GIVEN_malformed_or_wrong_schema_manifest_WHEN_loaded_THEN_rejected) ... ok
test_GIVEN_manifest_json_WHEN_loaded_THEN_binary_and_notice_hashes_are_validated (tests.test_release_scripts.ManifestHelperTests.test_GIVEN_manifest_json_WHEN_loaded_THEN_binary_and_notice_hashes_are_validated) ... ok
test_GIVEN_successful_empty_git_status_WHEN_read_THEN_is_clean_not_unavailable (tests.test_release_scripts.ManifestHelperTests.test_GIVEN_successful_empty_git_status_WHEN_read_THEN_is_clean_not_unavailable) ... ok
test_GIVEN_dirty_checkout_without_override_WHEN_manifest_requested_THEN_build_is_refused (tests.test_release_scripts.ManifestHelperTests.test_GIVEN_dirty_checkout_without_override_WHEN_manifest_requested_THEN_build_is_refused) ... ok

----------------------------------------------------------------------
Ran 10 tests in 2.512s

OK
```

`sh -n scripts/install.sh && python3 -m py_compile scripts/release_manifest.py tests/test_release_scripts.py` exited 0 with no output. `git diff --check -- scripts/release_manifest.py scripts/install.sh tests/test_release_scripts.py docs/RELEASE.md docs/progress/P6-RELEASE-EVIDENCE.md release/.gitignore` exited 0 with no output.

`python3 scripts/check_docs.py`:

```text
Checked 205 Markdown file(s); 0 finding(s).
```

M6 remains open for real ten-agent acceptance, firm review and approved
publication/update/disclosure policy.
