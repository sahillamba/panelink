# Panelink: product discovery and spec handoff

Status: first user answers and security review objective recorded; focused follow-ups pending. Updated 2026-10-27.

## Confirmed requirements

- UI/UX planning priority (2026-10-27): pane layout, menus and mouse interaction;
  use Herdr as inspiration with independently designed Panelink behavior and
  presentation. See [the proposed outcome and implementation plan](UI-UX-PLAN.md).
  Six interview rounds additionally confirm smooth anchored history, continuous
  scrollbar dragging, focus-preserving scrolling/resizing, contrasted focused
  chrome, copy feedback, mouse/keyboard tab create/switch/rename/close, managed
  new shell tabs, button confirmations and last-tab empty state. Four panes are
  enough; whole-pane drag, zoom expansion and permanent sidebar are excluded.
  See [the interview record](design/UI-UX-INTERVIEW.md). The consolidated scope was
  confirmed on 2026-10-27 and implementation started; these remain requirements,
  not claims that the redesigned interactions are implemented.

- Name: Panelink.
- Run inside existing terminal apps. Panelink is a terminal multiplexer/workspace runtime, with Herdr and tmux as behavior references.
- Rust, targeting Apple Silicon Macs; high performance with manageable development/build time.
- Support 5–10 agents plus ordinary processes.
- Original application code, without directly copying code from other projects.
- Panelink's own code is intended to be MIT licensed.
- Research comes first, followed by clarification and a specification that another or future agent can implement.
- Dependencies are exceptions: established components must save substantial time and have a documented reason for inclusion. Original Panelink logic remains independently implemented.
- The main reason for dependency caution is obtaining the firm's security audit approval and making Panelink suitable for others in similarly controlled workplaces. The firm's actual review policy and OpenCode approval status are not yet known.
- OpenCode is the required agent. Tabnine is an optional feasibility item. Preserve the agent's own terminal interface with status and notifications.
- Provide an easy scriptable/agent command API to discover, read and control other managed agents/processes across panes (requested2026-10-17). Exact access scope/defaults and command contracts are proposed in [AGENT-CONTROL.md](AGENT-CONTROL.md); this does not request autonomous scheduling.
- Keyboard navigation in the style of tmux, with Herdr-like workspace and mouse interactions.
- Simpler pointer and keyboard operation (2026-10-27): draggable pane dividers,
  quick close controls, clickable/draggable scrollbars and a visible actions
  menu. Touch operates through host-translated mouse events; native touch and
  a fixed sidebar layout are not selected by this request. See the
  [delivery plan](progress/P5-POINTER-CONTROLS-2026-10-27.md).
- Right-click context menus and automatic clipboard copy after text selection. The user explicitly requested Split Left, Right, Up and Down options (2026-10-17), and on2026-10-18 prioritized a new phase to open a fresh terminal session on the new side before the next planned work. See [Phase3A](progress/P3A-RIGHT-CLICK-SPLITS.md) and the existing [pane-layout design](PANE-LAYOUT.md); the confirmed hierarchy is Project → workspace tabs → split panes.
- Reboot recovery restores layouts and offers supported agent conversation resume.
- Local layouts, settings, and saved terminal history.
- Automatically expose background agents/subagents from OpenCode or compatible OpenCode wrappers in a tab named Subagents for optional observation (requested2026-10-21). See [Phase SA plan](progress/SUBAGENTS-PLAN.md). This selects the dedicated tab, while the project-sidebar presentation and proposed observation UX remain open. Wrapper compatibility must be proved per supported version.
- Personal use first, public MIT release later, with private Git source hosting for commits and branches.

The earlier desktop UI recommendation is superseded. Ratatui/Crossterm with a terminal engine and independent session server is an engineering candidate, not a selected or installed dependency set. The user's dependency concern is security audit acceptance. Do not treat any named library as accepted or claim the architecture will pass a firm's audit. The dependency-free foundation has been implemented and verified; [ROADMAP.md](../ROADMAP.md) tracks remaining implementation.

## Current interpretation and recommendations

Hierarchy decision (2026-10-25): the user selected **Project → workspace tabs →
split panes**. Tabs represent workspaces within a project; each workspace owns
its split-pane layout. This supersedes the earlier pending hierarchy options.
It does not select a project sidebar, remote execution, or automatic worktrees.

Keyboard copy mode is implemented as a bounded attached-client prototype for
the currently visible live/history view. Its `Ctrl-B c` entry key is an
implementation assumption, not a confirmed long-term shortcut choice. The
frozen view, local input handling, and cancellation boundaries are documented
in the [specification](SPEC.md#bounded-keyboard-copy-mode-prototype). Focused
client, compositor, clipboard-free PTY and full serial checks pass; human Apple
Terminal and real clipboard acceptance remain open. This does not close Phase 5.

Pane-location swap, ancestor-divider adjustment and zoom are also implemented
as bounded M2 prototypes. Their current `Ctrl-B m`, `Ctrl-B e`, and `Ctrl-B z`
bindings are not final shortcut decisions. Zoom resizes the selected PTY while
hidden processes continue; unzoom restores the underlying split geometry for
the current root. Swap changes exact pane locations within one tab without
moving processes. Divider preview commits once on Enter and cancels on Esc.
Stale captured targets or revisions cancel locally, and uncertain async results
are not replayed. Runtime ratios/zoom do not claim physical-reboot restoration;
the saved-observation codec is unchanged. Full serial Rust1399/0/15ignored,
final client123/0 and outer-terminal3/0 checks pass, with a separate real4PTY
protocol scenario1/0. Human Apple Terminal, real clipboard, physical reboot
and representative workload acceptance remain open. Repeat handling follows the host
key events and is not a final shortcut contract. M2 and Phases 3/5 remain open; see
the [bounded pane-operations contract](SPEC.md#bounded-pane-operations-prototype).

The user broadly accepted the first-release bundle but did not unambiguously choose among the feature bundles. Interpret the current direction as an agent workspace: project workspaces, tabs/splits, OpenCode launching, session persistence, status/notifications, saved history/search, and mouse support. Confirm the remaining scope through the questions below rather than adding autonomous delegation by implication.

A launcher means a saved executable, argument list, and working directory. It can start OpenCode, a shell, or a development server. It does not require another provider SDK.

Recommend Herdr-style lifetime: detach or quit the host terminal while processes continue in the server; use explicit stop actions to end them. The user asked what Herdr does, so this remains a recommendation pending their answer.

## Focused follow-ups presented as options

1. Primary host terminal: Apple Terminal selected for the first prototype. Expand terminal compatibility after that baseline works.
2. Agent scope update: agent-to-pane/process read and control API is now requested. Exact launch/create capabilities and access defaults are under design; autonomous scheduling is not implied.
3. Lifetime: persistent server with explicit stop (recommended) / prompt on explicit detach / stop with client.
4. Project presentation: the bounded opt-in `workspace open` UI currently uses a row-zero project/tab strip and chooser; a persistent project sidebar remains an unselected presentation option.
5. Remote scope: local Mac with ordinary ssh commands (recommended) / persistent remote sessions / existing tmux attachment.
6. Private hosting destination: confirmed as [sahillamba/panelink](https://github.com/sahillamba/panelink) on GitHub; the user requested a private repository and an initial commit of the current files.

Answer 1 is confirmed. Agent read/control in answer2 is now requested; its detailed scope/defaults and answers3–5 remain pending. No preselected option is an accepted user decision. Implementation was authorized on 2026-10-17 using cheaper models where appropriate, with durable roadmap/progress files; prototype assumptions are recorded in `ROADMAP.md`.

## Security review follow-ups presented as options

1. Is OpenCode approved, under review, not approved, or of unknown approval status at the firm?
2. Are open-source dependencies allowed after review, restricted to an internal approved registry/list, prohibited, or governed by an unknown policy?
3. Are persistent terminal history and automatic clipboard copying permitted, restricted, or not yet assessed?
4. Does the team require dependency inventory/SBOM and vulnerability results, source/build/signing evidence, or a broader application checklist? A nonconfidential summary is sufficient for planning.

These answers are pending. The earlier requirements for saved history and copy-on-selection remain recorded; policy controls are proposals until the firm's constraints are known. Audit approval and agent/provider authorization are separate decisions.

## Contract review incorporated into spec revision 0.3

All five submitted findings were confirmed as design gaps. The spec now defines operation deduplication/acknowledgments and nonreplayable input; early authoritative OpenCode identity capture with durable mapping updates and explicit fallback; retained scrollback versus screen checkpoints versus provider conversations; child environment precedence and secret persistence limits; and denial of programmatic clipboard reads/writes separately from user Copy/Paste. It adds acceptance scenarios and moves identity feasibility into M1.

These are engineering contracts for the draft, not evidence of completed implementation or a demonstrated OpenCode adapter. Provider support for observing every in-TUI session switch and an ordering barrier remains to be proved; only last-confirmed/manual recovery may be advertised until then. Full recording of overwritten alternate-screen content is outside the proposed first release.

## Workspace observations and source hosting

- Current shell reports macOS 26.5.2; this is not yet the minimum supported version.
- Neither OpenCode nor Tabnine was found on the active shell PATH. Do not infer global absence or install either without the relevant setup task.
- The earlier Git initialization restriction was resolved through an approved operation; the local repository now uses `main` and tracks `origin/main`.
- GitHub CLI authentication was verified as `sahillamba` after the user completed `gh auth login`. The earlier sandbox-only authentication check was misleading because network access was restricted.
- The private [sahillamba/panelink](https://github.com/sahillamba/panelink) repository was created on 2026-10-17 for the current specifications and future implementation. Git history is the source of truth for commits and pushes.

## Follow-up decisions for the eventual specification

Ask only for choices still unresolved after the first round; do not require the user to design low-level protocols.

- Workspace-tab naming, switching and persistence details under the confirmed
  Project → workspace tabs → split panes hierarchy; pane views remain distinct
  from managed process/session identities.
- First-launch flow, dependency discovery, shell environment, agent configuration, missing CLI errors.
- Terminal compatibility target: required keyboard protocols, colors, links, images, shell integration, history retention, and export.
- Shortcut conflicts and focus behavior; pane close versus session stop; multi-window and multiple-viewer policy.
- Provider support/version matrix, status evidence, cancellation, pending approvals, conversation resume, auth constraints.
- Worktree lifecycle, branch naming, dirty changes, cleanup ownership, and what needs explicit action.
- Remote execution boundary and persistence location if remote work is included.
- Storage location, retention bounds, diagnostics, deletion/export, and telemetry defaults.
- Signing identity, release hosting, updater policy, helper service lifecycle, and uninstall behavior.
- Numeric quality budgets on specified hardware and the observable scenarios used to evaluate them.
- Copyright holder for the future MIT license and contributor policy for original code.

## Proposed final specification package

The user requested a spec document. Prefer one canonical `docs/SPEC.md` that links supporting material only where it improves clarity. It should include:

1. Product purpose, intended users, primary workflows, and agreed release scope.
2. Confirmed requirements with stable identifiers, priorities, and observable acceptance criteria.
3. UX flow and workspace/pane/session concepts, including shortcuts and failure states.
4. Chosen stack with exact dependency/revision policy, alternatives, license inventory, and original-code boundaries.
5. Server/client ownership, terminal engine and cell rendering boundary, concurrency model, and background lifetime.
6. Domain entities, state machines, storage schema/versioning, and migration expectations.
7. IPC operations/events/errors, schemas, IDs, ordering, flow control, snapshots, reconnect, and version negotiation.
8. Agent support matrix, transport/capability contracts, approvals, cancellation, status evidence, and resume limits.
9. Host-terminal compatibility, macOS clipboard integration, supported hardware/OS, CLI distribution, updates, and server lifecycle.
10. Privacy/access model and explicit limitations, including restoration versus live persistence.
11. Phased implementation tasks with dependencies, deliverables, and exit criteria; unresolved feasibility work remains visible.
12. Validation plan, performance budgets, release criteria, and handoff instructions.

Supporting artifacts, if warranted: `docs/adr/` for decisions, versioned protocol schemas, `docs/IMPLEMENTATION-PLAN.md` for task tracking, and a short root `AGENTS.md` explaining how to continue. Create these once decisions warrant them; do not duplicate the canonical requirements across many files.

## Guidance for the next agent

Read this document and [PANELINK-RESEARCH.md](PANELINK-RESEARCH.md) first. If user answers arrive later, record them as confirmed decisions before turning recommendations into mandatory requirements. Distinguish assumptions, accepted choices, deferred features, and prototype gates. Recheck fast-changing dependency and provider documentation before pinning versions. Do not infer that the whole application has been specified or built from the presence of these research documents.
