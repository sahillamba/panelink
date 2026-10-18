# External OpenCode MiMo worker: mouse/menu handoff

User request2026-10-18: run a second worker through OpenCode using only
MiMo-V2.6-Flash Free. This is separate from the Luna layout implementation.

## Exclusive task

Read-only source analysis and implementation handoff. Do not edit files,
install dependencies, launch other agents, change Git, inspect credentials or
environment/auth files, or run mutating commands. Do not change the layout
worker's src/pane_layout.rs or P3A-LAYOUT.md. Return a bounded report to the
parent; the parent reviews it before any source adoption.

FIRST read AGENTS.md, ROADMAP.md, P3A-RIGHT-CLICK-SPLITS.md, P3A-LAYOUT-TASK.md,
and docs/PANE-LAYOUT.md. Use graph-first discovery when available; otherwise
state the limitation and inspect only relevant source. Identify actual mouse
event support and missing hooks in the bounded host input/parser/dispatch and
renderer adapters. Produce exact symbols/files for a minimal right-click menu
model, viewport-clamped overlay, disabled geometry/capacity reasons, captured
full target and layout revision, all four split actions, keyboard fallback,
and menu-owned input suppression. List specific automated assertions and a
staged patch plan. Distinguish pure menu work from runtime launch integration.

Preserve current capacity two, original session identity, server-owned PTYs,
global exact-byte deduplication, client-captured explicit launch environment,
incremental rendering, and all launch/host gates. Do not choose workspace/tab
hierarchy, add unrelated menu items, claim tests not executed, or use a paid
model fallback.

## Current attempt and honest status

OpenCode is now installed at /Users/sahillamba/.opencode/bin/opencode;
its actual --version output is opencode v2.0.32. The previous missing-CLI
observation is superseded; live provider/session identity remains unproved.
The official [model catalog](https://dev.opencode.ai/v2/docs/console/models)
lists model ID mimo-v2.6-flash-free. Its availability in this project's actual
provider configuration has not been verified by a successful catalog request.

The default CLI could not open its home log under the current filesystem
permissions. A private temporary XDG data/cache/state root allowed --version
and --help to run without modifying the user's configuration. A separate Bash
PTY was used for models --standalone, but the private server failed readiness.
Direct serve --stdio and the documented ACP path failed at the same private
server bootstrap. A direct 127.0.0.1 port0 bind independently returned
PermissionError errno1. No MiMo prompt or model task was sent, and no external
worker result is claimed. Do not repeat port guesses or bypass restrictions.

A temporary ACP controller is prepared at
/private/tmp/panelink-opencode.W8dvE8/mimo_worker.py. From a normal Mac Terminal,
outside the restricted agent execution environment, launch it with:

```sh
cd /Users/sahillamba/panelink
python3 /private/tmp/panelink-opencode.W8dvE8/mimo_worker.py
```

The controller selects read-only plan mode, discovers an exact available
provider/model ending in /mimo-v2.6-flash-free, and requires server confirmation
of that selection before sending this bounded task. It never chooses a paid
fallback. If successful, the report is written to the same temporary root as
mimo-menu-report.md for parent review. This temporary launcher is not a shipped
Panelink capability or an authentication/identity proof. Temporary files may
need recreation after cleanup/reboot; this document preserves the durable task.

The ACP route follows the official [OpenCode v2 ACP documentation](https://dev.opencode.ai/v2/docs/cli/acp/).
CLI startup and official catalog evidence do not establish successful inference.
