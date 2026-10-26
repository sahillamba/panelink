# OpenCode 2.0.32 project isolation slice

The `project-isolation` TUI scenario adds one private project beneath the
fixture root, creates and verifies one top-level session through the exact
owned OpenCode server, and attaches a third private TUI with the existing
fixture plugin configuration. It submits no prompt. The helper reads that
session's user-message endpoint directly and requires an empty `data` array;
the bootstrap helper for the original two or three sessions is not used for
this additional session.

The probe requires a fresh plugin observation whose session hash, top-level
status, and project hash all match the second project. It then applies the
same identity rows to the original project scope and requires rejection. Both
original TUI streams must advance beyond the hook watermark while retaining
their original identities; every pump drains both originals and the extra TUI.
The probe bounds this observation to ten seconds and checks exact listener
ownership before every authenticated call. The extra session list is capped at
20 rows and its canonical project path is validated defensively. The extra TUI
is reaped and closed in `finally`, including failure paths, and the empty-user
message read is repeated after cleanup; a failed final read downgrades the
result. Results expose only booleans, counts, and HTTP status codes; they omit
IDs, paths, credentials, response bodies, and terminal text.

This is a separate-project attachment and identity-scope proof. It does not
exercise a project switch inside one client. That keyboard path and any
same-client project-switch behavior remain open. Parent live acceptance passes3/3 fresh pinned2.0.32 runs; see
[P4-V2-ISOLATION-RESULT.md](P4-V2-ISOLATION-RESULT.md).
