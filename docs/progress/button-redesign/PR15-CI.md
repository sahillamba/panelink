# PR 15 attach fixture synchronization

The PR head was `8246a33b711de5029453d9cede4347905e6620ed` on
`t3/terminal-button-design-research`; the checkout was clean and matched it.
GitHub returned no review threads, reviews or conversation comments.

Hosted run [41120243840](https://github.com/sahillamba/panelink/actions/runs/41120243840)
passed seven supporting checks but failed the root Rust suite in
`real_attach_routes_renders_closes_and_detaches_four_panes`, waiting for
`ATTACH4_MARKER_2`. The focused test passed locally before editing, so the
exact hosted failure was not reproduced locally.

Source inspection found that the fixture accepted a marker from the echoed
`echo` command before the shell executed Enter. That is insufficient evidence
that the previous pane's input has drained before changing focus. Both routing
loops now construct the full marker only in shell output using `printf`.
The new private-PTY regression demonstrates the old false positive without
Enter, proves the constructed marker is absent from the unexecuted draft,
and requires it after Enter. Existing isolation, rendering, close and detach
assertions remain, with unchanged timeouts and no input retries.

The focused integration target passed all three tests. The full canonical
`RUST_TEST_THREADS=1 python3 scripts/verify_non_ui.py` gate passed **8/8**,
including the repaired integration target, formatting, strict Clippy, build,
component fingerprints, Python tests (233 run, one skipped), and documentation
checks (466 files, zero findings). Full local output is available at
`/tmp/pr15-verification.log` in this session. This repair addresses a demonstrated synchronization
gap; only a new hosted run can establish whether it resolves the reported CI
failure. Application behavior and agent usage documentation are unchanged.
