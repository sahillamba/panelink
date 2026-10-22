# U2 in-app help result — 2026-10-22

Implemented the Ctrl-B then `?` help overlay in the attach client's existing
compositor. The fixed eight-row text describes the currently available menu,
split, focus, close, detach, and reattach actions. It clips to the current host
viewport and remains above transient status notices. NAV now advertises `?`.

The help modal persists independently of NAV expiration. Esc or `?` dismisses
it; Ctrl-X detaches. Other keys, paste and mouse events are consumed without
changing focus, opening a menu, closing/splitting panes, or sending input.
Opening help flushes already batched keys once through their pinned target.
Host resizing remains observed through the existing host loop and redraws the
clipped overlay. No key map, protocol, dependency or module changes were made.

## Verification

`rustfmt --check --edition 2021 src/ui_client/state.rs src/ui_client/state_tests.rs` — passed.

`cargo test --locked --offline --bin panelink ui_client::state -- --test-threads=1` — passed: 27 passed, 0 failed, 0 ignored; 72 filtered out.

The recording-fake tests cover NAV discovery; one-time pending-key flush;
modal key/paste/mouse isolation; focus preservation; Esc/`?` dismissal; Ctrl-X
detach; help persistence beyond NAV timeout; status priority; resize and tiny
viewport clipping; incremental overlay repaint; and no output for an unchanged
committed frame.

During parallel integration, one attempt could not compile because U1's
in-progress `keys.rs` tests constructed `KeyEventState` using `Default`, which
that type does not implement. U1 corrected and released its owned files; the
final scoped command above then passed. An earlier compositor assertion also
initially measured the uncommitted first frame; the test now commits its
baseline before checking the help repaint. No external/manual host acceptance
is claimed by this client-state test.
