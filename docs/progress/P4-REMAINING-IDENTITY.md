# Phase 4 remaining OpenCode identity fixture

This fixture adds three acceptance attempts to the existing pinned OpenCode
2.0.32 private-server and passive TUI identity probe. It is an evidence tool,
not product integration, and it does not close Phase 4 by itself.

The gates are intentionally end-to-end:

1. Create a new root through the server API, prove its exact project scope and
   top-level status from the public session listing, then select its exact title
   through Ctrl-O in an already-running TUI. The route observer must report the
   new session ID hash and project hash in that same client. The new root must
   still contain no user prompt messages.
2. Create a private seed root and use its export as the exact session-info
   template. The pinned bundle defines `SessionTransfer.Data` as `info` plus an
   array of `Session.Message.Info`; this fixture uses the bundled
   `Session.Message.Synthetic` member with its schema-defined `id`, `type`,
   `text`, optional `description`, and `time.created` fields. It imports that
   fixed inert record directly; it does not enqueue synthetic input or run a
   model. Delete the seed and require its identifier to be absent before
   importing it. Export the imported session again and require an identical
   nonempty canonical message hash before using it. Fork the full history with
   an empty fork body, require a
   different child ID, exact parent link, exact nonempty message hash, child
   project scope, and presence in the top-level listing. OpenCode 2.0.32 models
   a fork as a top-level session with a separate fork-origin link, not as a
   `parentID` child. Its projector assigns child message IDs from the fork
   session ID and sequence, so history equality hashes every message field
   except `id`, while separately requiring a complete unique child-ID set that
   is disjoint from parent message IDs. Select parent, child, then parent
   through the existing session picker in the same TUI.
3. Create a second private project and a root session there. Use Ctrl-O from
   the already-running first TUI to select the exact foreign session, prove the
   same observer-instance nonce across the project-hash change, then use Ctrl-O
   to return to the exact original project/root. The passive second TUI must
   keep observing its original project/session and advance during the actions.

The fixture makes zero prompt submissions and zero model/provider calls. The
project directories, HOME/XDG data, server, clients, plugin records,
and control files live in a fresh mode-0700 temporary root. The worker is run
under the existing loopback-only macOS sandbox, checks the pinned executable's
version and SHA-256 before starting the server, and reaps both client and server
process groups before returning. The JSON output contains identity hashes and
allowlisted statuses, not raw session IDs, transcript text, credentials, or
project paths.

Run on the pinned native macOS host with:

```sh
python3 scripts/probe_opencode_v2_remaining_identity.py
python3 -m unittest tests.test_opencode_v2_remaining_identity -v
```

The script exits 0 only when the baseline TUI identity gate, all three new
gates, loopback-only sandbox, exact binary pin, and process cleanup pass. It
returns a nonzero status and a specific failure reason when an API route,
import/fork capability, UI picker, or observer contract is unsupported or
unproven. Such an outcome is a failed acceptance gate, not a pass or a manual
fallback. A clean run is required before recording any live acceptance result;
unit tests alone do not establish provider compatibility or Phase 4 completion.

## Evidence status

Live private-host fixture result, 2026-10-24 12:21 UTC:

- Launcher: `opencode` at `/Users/sahillamba/.opencode/bin/opencode`.
- Version/hash pin: OpenCode `2.0.32`; expected SHA-256
  `e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`
  matched before launch.
- Baseline TUI identity and the three remaining gates: `proven`.
- New root: exact project/top-level scope and same-TUI selection proven;
  user-message read returned HTTP 200 with an empty list.
- Nonempty transcript import and full-history fork: imported transcript
  round-trip preserved; fork child history matched after excluding only
  remapped message IDs; distinct message IDs, fork-origin parent/boundary,
  top-level listing, and parent→child→parent picker selection proven.
- Same-client project switch: Ctrl-O selected the exact foreign-project root
  and then returned to the original root. The observer reported one TUI
  instance across the project-hash transition; the passive TUI remained fresh
  on its original project/session.
- Safety/cleanup: enforced loopback-only sandbox; zero prompt submissions and
  model/provider calls; both TUI process groups were reaped and PTYs closed;
  owned server cleanup completed.
- Validation: `python3 -m unittest tests.test_opencode_v2_remaining_identity
  -v` passed (10 tests); `python3 -m py_compile
  scripts/probe_opencode_v2_remaining_identity.py` passed.

This is fixture evidence for the pinned native OpenCode build. It does not
claim general provider compatibility or Phase 4 completion; canonical Phase 4
gates remain unchanged.
