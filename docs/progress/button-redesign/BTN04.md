# BTN04 — workspace controls and tab strip

Implementation sub-agent, shared checkout `t3-baecf477`, branch
`t3/terminal-button-design-research`, HEAD
`fdd8664d3085958430f1fbd91ec066d868c33050` plus preserved parent dirty changes.
No subagents, commits, push, installation, user panes or runtime restart.

## Behavior

Project selector remains left; workspace tabs occupy the middle; New tab and
All tabs are pinned right, in that order. Optional previous/next controls are
omitted below 60 columns. Names shrink before controls disappear. Each visible
tab has an independent padded `x` target separated by a plain gap. Selection and
close have disjoint semantic bounds. Active-tab click and right-click menu
compatibility remain; Rename stays in Tab actions.

`TabAction::Close(route)` goes through the existing safe close admission. It
never switches tabs. Only the attached runtime generation is known: clicking an
inactive close reports `open this tab first to close it`. This does not implement
inactive runtime lifecycle lookup or silently stop unknown runtimes.

Header fills use shared semantic styles. Rest, selected, pressed and pending
have separate fills; selected uses the Primary role, now bright blue rather than
the pressed state's blue. Filled/pending HeaderCells are bounded data; transient
press/pending presentation is applied to render copies, preserving TabsLayout
capture equality. Existing compositor commit/cache/write-failure rules remain.
Pane control styling and the parent's pre-output span admission are preserved.

Workspace actions, rename Save/Cancel, Close/Cancel, picker route rows/Clear and
empty-body New tab/Detach now use OverlayControl spans derived directly from
workspace_controls or empty action geometry. Each independent control gets its
own role/state across its exact padded hit bounds. Titles, filter/status text
and gaps remain plain. Focus comes from local editor/dialog selection; validated
press is independent of keyboard focus. Pending Save/Close retain Pending and
Cancel Disabled; metadata guards remain authoritative. Pending creation fills
New tab while outcome-unknown creation retains explicit reconciliation access.
Removed workspace `[pressed]`, empty pressed glyph substitution and uppercase
Save/Cancel feedback. Existing editing, focus traversal, request identity,
revision/catalog/UI/runtime validation and no-child-input guards remain.

## Checks and evidence

All Cargo checks use `CARGO_TARGET_DIR=/tmp/panelink-btn00-baseline-target` and
`--locked --offline`. Full stdout/stderr logs are under `logs/btn04-*`.

- State suite `cargo test --bin panelink ui_client::state::tests`: 169 passed.
- Workspace suite `cargo test --lib workspace_tabs::`: 11 passed.
- Terminal suite `cargo test --lib terminal::`: 95 passed.
- Full serial `cargo test --test p5_workspace_tabs -- --test-threads=1`: 5 passed, 0 failed (39.92s).
- Strict all-target Clippy, `cargo fmt --check`, `git diff --check`: passed.

`logs/btn04-cells.log` contains actual decoded renderer text and per-control
backgrounds for independent same-row Save/Cancel (1 test passed).
`logs/btn04-capture-cells.log` contains decoded frames from the complete existing
workspace capture invalidation regression (1 passed). Its assertion now checks
pressed fill across every cell of each control's actual bounds. Pending and
empty-picker tests retain mutation and routing assertions. New geometry coverage
checks active tab, separate close, New and All tabs at every width 22–240,
nonoverlapping exact hits and Unicode topology. Existing Unicode coverage spans
0–240, clamping and minimum-pane coverage remain. New state coverage proves
inactive-close does not switch or submit metadata and independent Save/Cancel
focus fills across full bounds with no child effects.

Initial logs retain appearance failures: old inverse/bracketed tab expectations,
old control order/navigation assumptions and provisional pressed text/glyphs.
The first full PTY run failed all five at the old header readiness predicate.
Updated that predicate to require padded selected-name text and the real selected
fill; process/readiness/cancel assertions remain. The create fixture checks New
before rightmost All tabs. A second run reached 4/5: the pane-close label helper
matched the newly added strip x, so it tested tab cancellation instead. Scoped
that pane-label lookup below row zero; preserved the full 4/1 log as
`logs/btn04-p5-pane-selector.log`. Process/cancel assertions remain unchanged. Intermediate compiler errors (test-only close helper,
missing Availability in the added test) were corrected; their overwritten output
is not retained evidence. No zero-test result counted and no process assertions
removed or ignored.

## Parent follow-up

Ready for independent bounded parent source review; final logs verified. Combined canonical,
provider, host, clipboard, touch and theme acceptance remain pending. BTN05 owns
consistency/theme audit and canonical skill/docs synchronization. This packet
makes no installed-runtime or full design acceptance claim. ms-rust unavailable.
The single graph attempt could not resolve this checkout (project not indexed);
used current source per task fallback without retrying. Memory registry supplied
no relevant button facts. Parent planning/roadmap/skill files were not edited.
