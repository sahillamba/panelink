# Panelink development instructions

## Start here

Read `README.md`, `docs/SPEC.md`, `docs/PANELINK-DISCOVERY.md`, and `docs/PREBUILD.md` before changing the application. The current product runs inside existing terminal applications on Apple Silicon Macs. The archived desktop GUI proposal is superseded.

Implementation is paused after scoped stop and saved-history checkpoints. Read `ROADMAP.md` and the current handoff at `docs/progress/SESSION.md` before resuming; the roadmap is the durable status entrypoint. Follow phase order and preserve unresolved product decisions; use documented prototype assumptions only within their scope.

Delegation update: the user subsequently instructed "stop putting agents on
anything do yourself"; do the next implementation directly unless the user
explicitly requests an agent (as they did for the independent reviewer).
Historical Luna assignments in older progress notes are completed work, not
an instruction to delegate on resume.

The user paused work on 2026-10-21. Resume only on a new instruction. On
resumption, retain the earlier automated-test/no-manual-handoff requirements:
run permitted checks and record blocked acceptance honestly rather than
counting skipped tests as passes.

## Rust guidance

The user requested the community `ms-rust` skill from https://gitlab.com/lx-industries/ms-rust-skill. Installation is pending under this session's network/filesystem restrictions. Do not claim it is installed or that a local skill revision has been pinned.

When the skill is installed and available, read its `SKILL.md` and only the guideline topics relevant to the task before Rust changes. Record the upstream commit used. Use the committed guideline snapshot until an explicit refresh task reviews an update; do not execute its guideline generator automatically.

User requirements and Panelink's dependency/security policy take precedence over generic guideline recommendations. In particular, a guideline recommending an allocator, error crate, logging crate, or telemetry library does not itself justify adding that dependency. Explain any project-specific guideline exceptions. Do not assert complete guideline compliance or use a compliance stamp as security-audit evidence without an actual scoped review and recorded revision.

## Implementation constraints

- Write Panelink's application logic independently. Preserve notices for any deliberately selected third-party components; development skills are distinct from shipped application dependencies.
- Evaluate every dependency against saved effort, full dependency graph, licensing, maintenance, build impact, and the firm's applicable review criteria. The existing shortlist is not an approved manifest.
- Keep GUI/client lifetime separate from server-owned PTYs. Preserve the spec's command deduplication, nonreplayable input, explicit launch environments, and clipboard-denial contracts.
- Prove OpenCode identity capture and session-switch behavior early. Provider lifecycle events alone do not justify guaranteed recovery. Use the specified last-confirmed/manual fallback when identity is uncertain.
- Keep saved scrollback, screen checkpoints, and provider conversations distinct. Never replay stored terminal side effects.
- Keep credentials, input/paste contents, and sensitive environment values out of persisted launch metadata and diagnostics.
- Record evidence honestly: proposed acceptance scenarios, completed checks, compatibility observations, and unresolved capabilities are different things.
- Update the canonical spec and affected planning notes when a design changes, without turning pending user options into confirmed requirements.

## Repository state

The private GitHub repository exists; local `main` tracks `origin/main`. Check current status before source-control work and use Git history as the source of truth.

User authorized phase-completion commits on 2026-10-17: after each phase is fully complete and its required checks pass, the parent agent must commit the phase's code and affected documentation/evidence. A later explicit user request may also authorize a checkpoint commit of accumulated work; label it as a checkpoint and do not mark unfinished phases complete. Worker subagents leave commits to the parent. Pushing requires a separate user request. This supersedes older blanket no-commit instructions in planning/task notes for the parent; workers' exclusive-file and no-commit constraints remain in force.
