# Panelink prototype readiness

Updated: 2026-10-18. This is a readiness note, not a replacement for `SPEC.md`.

Pause handoff (2026-10-21): the original readiness narrative below predates
the completed two-pane, scoped stop and saved-history slices. For current
setup use [SETUP.md](SETUP.md); for status, commits, verification and open gates
use [ROADMAP.md](../ROADMAP.md) and [SESSION.md](progress/SESSION.md).
Work resumes only on a new user instruction.

## Current state

Phase 3 integration now includes an experimental foreground `server-probe` and
`control` CLI for one child process. The child is server-owned while the
foreground server runs; short-lived clients reuse a durable metadata-only
context and can explicitly resync an uncertain operation. The CLI supports
numeric five-field targets, owner revisions, bounded screen reads, and optional
input with `--allow-input`. A new empty private runtime directory requires
explicit `--initialize`; later startups open its existing identity storage and
do not recreate missing files. This remains a prototype: the corrected pathname CLI scenario is
operator-confirmed passing outside the sandbox, Phase 2 Apple Terminal
acceptance is incomplete, and Phase 3 has not been declared complete. No
provider recovery, indefinite subscriptions, pane layout, or multiple-session UI
is implemented. Bounded metadata Events and atomic reconciliation are now
implemented; actual CLI pathname coverage for these new commands remains external. See [the CLI evidence](progress/P3-CLI.md), [socket gate](progress/P3-SOCKET-GATE.md),
and [current integrated verification](progress/INTEGRATION.md).

## Recommended next step

Implementation was authorized on 2026-10-17. Follow [ROADMAP.md](../ROADMAP.md); the dependency-free foundation and scoped replacement core are implemented and parent-verified. Cargo access is restored; the [source-gate evidence](progress/P2-SOURCE-GATE.md) remains the rejection record for Alacritty/portable-pty unchanged. The replacement uses a locally corrected VT100 engine and Rustix/openpty; a bounded parser-only input component, pure snapshot renderer, and one-shot host IO/restoration/CLI probe are implemented. The user reports that most short manual terminal checks worked, while the full Apple Terminal acceptance list remains incomplete. Automatic private-PTY cleanup-order observations pass; see the [core result](progress/P2-CORE-RESULT.md), [host-input gate](progress/P2-HOST-INPUT.md), and [experiment plan](progress/P2-EXPERIMENT.md). Phase 3 now has an integrated one-child server/control path, with its corrected pathname CLI scenario operator-confirmed passing. Continue to prove independent server/PTY lifetime, durable context resync without input replay, terminal clipboard denials, and authoritative OpenCode identity through session changes. A complete UI and every later feature need not be designed before those gates.

Use representative synthetic projects/output for initial checks. Firm deployment depends on the organization's actual approval process; that process is still unknown. A personal prototype does not establish approval for work data or provider use.

## Decisions and setup to finish

| Item | Current state | Next concrete action |
| --- | --- | --- |
| First host terminal and minimum macOS | Apple Terminal selected as the first compatibility baseline; installed versions observed as macOS 26.5.2 / Terminal 2.15 (470.2), with subsequent partial qualitative manual feedback; minimum OS remains pending. | Record the versions used for M1. Broaden compatibility after the baseline works. |
| Experimental dependency set | VT100 local correction + Rustix/openpty and parser-only crossterm-input selected; signal-hook0.4.5 flag-only selected for the one-shot host; broader UI/storage dependencies unselected; mimalloc deferred. | See replacement review, preserved notices and exact graph; see P2-INPUT-REVIEW.md and finish runtime verification under P2-PROBE.md before Apple Terminal validation. |
| Source control | Private GitHub repo exists and local `main` tracks `origin/main`. | Keep Git history as the source of truth; commit planning updates with the next authorized source-control action. |
| Toolchain and build baseline | Rust/Cargo 1.99.1 tested and recorded on Apple Silicon macOS. Current integrated test totals, ignored cases, and known failures are maintained in `progress/INTEGRATION.md`. | See `progress/INTEGRATION.md`; establish minimum supported OS and representative build/runtime measurements later. |
| M1 experiment plan | Acceptance cases exist in the spec. | Turn them into a short ordered task list with pass/fail evidence, starting with one PTY and same-project OpenCode identity switching. Record capability failures without claiming recovery works. |
| Firm review constraints | Unknown. | Obtain a nonconfidential checklist, dependency policy, OpenCode approval status, and history/clipboard constraints before the corporate pilot. |

These are scoped preparation tasks. There is no need to adopt a large framework, agent plugin marketplace, cloud backend, or autonomous scheduler to run M1.

## Rust toolchain and build baseline

Rust/Cargo 1.99.1 is tested and recorded on Apple Silicon macOS; current integrated checks are in `progress/INTEGRATION.md`. On another machine use the [official Rust installation instructions](https://rust-lang.org/tools/install/) or the firm's approved method, then follow the roadmap. Cargo registry access is restored; use a writable `CARGO_HOME` under the current filesystem restrictions. See `progress/P2-SOURCE-GATE.md` for acquisition evidence and source-level blockers.

## Requested ms-rust skill

Source: [LX Industries / ms-rust-skill](https://gitlab.com/lx-industries/ms-rust-skill). It is a community skill wrapping Microsoft's Pragmatic Rust Guidelines; it is not evidence of Microsoft endorsement of Panelink or security certification.

The [upstream README](https://gitlab.com/lx-industries/ms-rust-skill/-/raw/main/README.md) says generated guideline files are committed and usable after cloning. Python/uv are needed for its refresh generator, not for reading the committed skill. Do not run the generator as an automatic install step.

Read-only inspection covered the README, SKILL.md, application guidelines, universal guidelines, library index, and Microsoft's current guideline introduction. A full repository, generator, and dependency audit has not been completed. The fetched skill identifies a 2026-02-21 snapshot; Microsoft's [current guideline introduction](https://microsoft.github.io/rust-guidelines/guidelines/index.html) reports version 2026.6, generated 2026-09-25. Do not claim the skill is synchronized to that newer version without a reviewed refresh. Its library index is a heading-only file in the fetched snapshot; detailed library topics are separate files and must be checked when relevant.

The [application guidance](https://gitlab.com/lx-industries/ms-rust-skill/-/raw/main/02_application_guidelines.md) recommends mimalloc on the grounds that an allocation-heavy hot path can improve; it cites results as high as 25% in its benchmarks. That is not a general performance promise. The Rust `mimalloc` crate is a thin global-allocator integration around Microsoft's native allocator, but brings native C build code and changes allocation behavior for the whole Panelink process. The underlying allocator is mature and broadly deployed; that does not establish a speedup, lower memory use, or a simpler audit for Panelink. Its own documentation tells users to compare against the standard allocator and memory tracking tools.

For Panelink, defer mimalloc for M1. The main CPU/memory load will often be the child agents and processes, which Panelink does not allocate memory for. Panelink's own emulator/parser and event loop could be allocation-sensitive, but we have no profile yet. Start with Rust's standard system allocator, then benchmark both allocators using representative terminal output, pane counts, reconnect, and idle operation on Apple Silicon. Adopt mimalloc only if it produces a material repeatable gain without unacceptable memory, build, or review cost. The guideline's application guidance also permits anyhow/eyre; neither that nor mimalloc overrides the selective dependency policy. The entrypoint's requested source compliance comment is a style marker, not audit proof. Apply relevant guidance with documented project exceptions, as Microsoft's own introduction allows.

### Installation status and manual step

Not installed. The local shell cannot resolve `gitlab.com`; the global Codex skill directory is outside the writable workspace. The bundled skill-installer helper accepts GitHub sources and does not directly handle this GitLab URL. No upstream generator or remote installer was executed; no third-party skill files were copied into Panelink.

From a terminal with network and home-directory write access, install to an unused destination:

```sh
git clone https://gitlab.com/lx-industries/ms-rust-skill.git "$HOME/.codex/skills/ms-rust"
git -C "$HOME/.codex/skills/ms-rust" rev-parse HEAD
```

The clone command fails rather than replacing a preexisting populated destination. Record the resulting commit in the development-tool inventory before using it as the reviewed baseline. This command fetches the then-current default branch, which may differ from the web-fetched snapshot inspected here; inspect the installed entrypoint and relevant guideline files at that exact commit before adopting them. It does not refresh the generated guidelines or run project code.

After installation, verify that the next agent turn can discover `ms-rust` and read its entrypoint. If a different agent is used, configure its skill discovery separately. Keep development-tool provenance separate from the eventual binary's runtime SBOM.

## What is already ready

The working spec defines command retry/acknowledgment behavior, OpenCode identity durability and fallback, history meanings, launch environment precedence, clipboard authority, and acceptance scenarios. Root `AGENTS.md` points future agents to the roadmap. The foundation, bounded PTY core, renderer, host probe, and experimental one-child server/control CLI are implemented to their recorded scopes. Current integrated test totals, formatting/Clippy results, ignored cases, and blockers are in `docs/progress/INTEGRATION.md`; no aggregate count is repeated here. No representative performance benchmarks, completed Apple Terminal acceptance, provider recovery, or release artifacts exist yet.
