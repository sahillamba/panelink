# Presentation spec: Terminal multiplexers and GUI agent-control environments for developer productivity

Status: **Draft v0.1, superseded in focus.** The talk now centres on the Panelink + RN Metro logs use case. See [script-stop-being-your-agents-clipboard.md](script-stop-being-your-agents-clipboard.md). This file is kept as a broader backlog of ideas.
Workflow: we review one screen at a time. The user decides the content of each screen, and this file is updated to match.

---

## 0. Talk metadata (to confirm)

| Field | Draft assumption | Confirm? |
| --- | --- | --- |
| Working title | "From Tabs to Teams: Running Many Coding Agents with Multiplexers and Agent Control Environments" | ☐ |
| Alt titles | "Your Terminal Is Now a Control Room" / "One Developer, Many Agents" | ☐ |
| Audience | Internal engineers (mobile, web, backend), mixed familiarity with AI coding agents | ☐ |
| Length | 30 min talk + 10 min Q&A (about 14 screens) | ☐ |
| Format | Slides plus 1–2 live or recorded demos | ☐ |
| Presenter | Sahil Lamba | ☐ |
| "White-label fork of T3 Code" | An internally branded and configured fork of T3 Code (for example with company model gateway, approved skills and MCPs). Exact name and scope are TBD. | ☐ |
| Can name internal tools? | (tescode, Panelink, internal skill store) TBD | ☐ |

## 1. Core message

**One-sentence thesis:** Agents made the bottleneck *parallel supervision*, not typing code. Terminal multiplexers keep agent work alive and observable. GUI agent-control environments make it reviewable, delegable and safe. Together they turn one developer into an effective lead of several agents.

**Three takeaways the audience should remember:**
1. Persistent sessions (multiplexers) stop you from losing agent work and context.
2. A control plane (GUI environment) lets you run, watch, review and merge parallel agent work without context-switch chaos.
3. Productivity comes from workflow habits such as isolation, small tasks and review gates. Tools alone are not enough.

## 2. Narrative arc

1. **Hook / pain**: the developer juggling 6 terminal tabs, lost sessions and agents waiting on input.
2. **Why now**: agents run for minutes to hours, in parallel, and need supervision.
3. **Layer 1 – Terminal multiplexers**: what they are, why they matter for agents.
4. **Layer 2 – GUI agent-control environments**: what T3 Code-style tools add.
5. **Our setup**: a white-label fork and how it fits the company environment.
6. **Demo**: an end-to-end parallel task.
7. **Results / evidence**: what changed (measured or anecdotal, clearly labelled).
8. **Pitfalls and guardrails**.
9. **How to start on Monday**.
10. **Q&A**.

---

## 3. Screen-by-screen spec

Each screen lists: **Purpose**, **Key message**, **Content (draft)**, **Speaker notes (draft)**, **Visual/demo placeholder** (description only, no design), and **Open questions**.

### Screen 1 – Title
- **Purpose:** Set the topic and presenter.
- **Key message:** —
- **Content:** Title, subtitle ("Multiplexers + agent control environments for developer productivity"), name, team, date.
- **Speaker notes:** One-line intro and why I care about this topic (daily driver for X months).
- **Visual placeholder:** None.
- **Open questions:** Final title? Team/logo?

### Screen 2 – The hook: "a normal day with agents"
- **Purpose:** Make the pain feel familiar.
- **Key message:** Running agents in plain terminal tabs does not scale past one or two at a time.
- **Content:**
  - 5+ terminal tabs, each with an agent or dev server
  - Closed a window and lost a session
  - An agent was waiting for approval for 20 minutes without anyone noticing
  - Two agents edited the same files
- **Speaker notes:** Short story from personal experience. Ask the audience: "How many of you have more than 3 agent sessions open right now?"
- **Visual placeholder:** Screenshot of a chaotic tab bar (real, anonymised).
- **Open questions:** Which real anecdote should we use?

### Screen 3 – What changed: from autocomplete to autonomous agents
- **Purpose:** Explain why the tooling question is new.
- **Key message:** Agents are long-running, parallel and stateful. Our tooling was built for short, single, interactive commands.
- **Content:** Timeline: autocomplete → chat → in-IDE agent → CLI agents (OpenCode, Claude Code, Codex) → multi-agent orchestration.
- **Speaker notes:** The developer's role shifts from writing code to specifying, supervising and reviewing it.
- **Visual placeholder:** Simple timeline.
- **Open questions:** Which agents do we name, given what is approved internally?

### Screen 4 – The new bottleneck
- **Purpose:** Frame the problem precisely.
- **Key message:** The constraints are now attention, context and review. Typing speed no longer is.
- **Content:** Four problems:
  1. **Persistence**: sessions die with windows or SSH drops.
  2. **Visibility**: you cannot see which agent is blocked.
  3. **Isolation**: parallel agents collide in one checkout.
  4. **Review**: diffs pile up without structure.
- **Speaker notes:** These four problems map to the two tool layers in the rest of the talk.
- **Visual placeholder:** 2×2 grid. Each cell is later tagged "multiplexer" or "GUI env".
- **Open questions:** —

### Screen 5 – Layer 1: terminal multiplexers, the 60-second primer
- **Purpose:** Bring everyone to the same baseline.
- **Key message:** A multiplexer keeps shells alive in a server process, independent of your terminal window.
- **Content:**
  - Definition: sessions → windows/tabs → panes; detach and reattach
  - Examples: tmux, Zellij, (screen), Panelink (our experiment), Herdr (agent-oriented)
  - Core concepts: server-owned PTYs, detach (Ctrl-B d / Ctrl-X), splits, scrollback
- **Speaker notes:** Keep it short. Experienced users can glance through it.
- **Visual placeholder:** Diagram: Terminal window → client → multiplexer server → PTYs → shells/agents.
- **Open questions:** Should Panelink be featured or only mentioned?

### Screen 6 – Why multiplexers matter for agents
- **Purpose:** Connect multiplexers to agent workflows.
- **Key message:** Multiplexers make agent sessions durable, observable and scriptable.
- **Content:**
  - **Durable**: close the laptop lid or the window, and the agent keeps running
  - **Observable**: a grid of panes shows agents, tests, servers and logs at once
  - **Scriptable**: agents can read other panes, send input and wait for output (for example `panelink`/`herdr`/`tmux send-keys`, `capture-pane`)
  - **Agent drives agent**: one agent starts and supervises another in a pane
- **Speaker notes:** Scriptability is the underrated part: the multiplexer becomes an API agents can use.
- **Visual placeholder:** Pane layout: agent A | agent B / tests | dev server.
- **Open questions:** Include a short multiplexer-only demo here?

### Screen 7 – Limits of the terminal-only approach
- **Purpose:** Motivate Layer 2.
- **Key message:** Terminals are good for running work and weak at review, notification and orchestration.
- **Content:** Hard to: review diffs side by side, track many threads, get notified on "waiting for input", manage worktrees and PRs, use on mobile.
- **Speaker notes:** This is not a case for replacing terminals. It is a case for adding a layer on top.
- **Visual placeholder:** —
- **Open questions:** —

### Screen 8 – Layer 2: GUI agent-control environments
- **Purpose:** Introduce the category.
- **Key message:** A control plane for agents: threads, delegation, worktrees, review and PRs in one place.
- **Content:**
  - What they are: a GUI over one or more agent providers/harnesses
  - Typical capabilities: thread list with status, per-thread git worktrees, diff review, delegating subagents, model choice per task, PR linking/watching, scheduled tasks, browser/device preview, approvals
  - Examples: T3 Code (open source), other market examples (TBD)
- **Speaker notes:** Think of it as "mission control" while the terminal stays the engine room.
- **Visual placeholder:** Architecture: GUI → orchestrator → providers (models) → worktrees/terminals.
- **Open questions:** Which competitors to list, if any?

### Screen 9 – Our white-label fork
- **Purpose:** Show what we built or configured and why.
- **Key message:** We forked T3 Code to fit company constraints: approved models, skills, security and branding.
- **Content (placeholders):**
  - Why fork rather than use upstream: model gateway, SSO, approved skill/MCP catalog, data policy
  - What changed: branding, defaults, integrations (Jira/Confluence/New Relic skills?), guardrails
  - What stayed upstream-compatible
- **Speaker notes:** Be explicit about what is production-ready and what is experimental.
- **Visual placeholder:** "Upstream vs our fork" comparison table.
- **Open questions:** Name of the fork? What can be shown publicly or internally? Who maintains it?

### Screen 10 – How the two layers fit together
- **Purpose:** Show the combined workflow.
- **Key message:** The GUI orchestrates while multiplexers and terminals execute and persist.
- **Content:** Flow: Ticket → plan thread → delegate N tasks (each in its own worktree) → agents run in panes/terminals → GUI shows status/waiting → review diffs → PR → CI watch → merge.
- **Speaker notes:** Map back to the four problems on Screen 4: each one is now covered.
- **Visual placeholder:** Flow diagram with the four problems ticked off.
- **Open questions:** —

### Screen 11 – Demo
- **Purpose:** Prove it works.
- **Key message:** One developer, three parallel tasks, about 10 minutes.
- **Content (demo script draft):**
  1. Open a real (or sandbox) repo in the GUI environment
  2. Create a plan thread from a ticket
  3. Delegate 2–3 subtasks to separate worktrees (for example: a bug fix, tests, a docs update)
  4. Show a multiplexer view with agents running and a dev server/test watcher
  5. Detach or close the window, then reattach: nothing is lost
  6. Handle an "agent waiting for approval" notification
  7. Review a diff, open a PR, and watch CI
- **Speaker notes:** Have a recorded backup. Pre-warm the environment.
- **Visual placeholder:** Live demo or recording.
- **Open questions:** Live or recorded? Which repo and tasks?

### Screen 12 – Results and evidence
- **Purpose:** Back the claims with numbers.
- **Key message:** Measurable gains, labelled honestly.
- **Content (placeholders; never invent numbers):**
  - Parallel tasks per day (before → after)
  - Time from ticket to PR for small tasks
  - Sessions lost per week (before → after)
  - Cost/tokens per task (for example from usage reports)
  - Qualitative quotes from pilot users
- **Speaker notes:** Separate measured data, anecdotes and expectations.
- **Visual placeholder:** Before/after bar chart.
- **Open questions:** What data do we actually have? Pilot size?

### Screen 13 – Pitfalls and guardrails
- **Purpose:** Build credibility and set safe usage.
- **Key message:** Parallelism amplifies mistakes too, so add gates.
- **Content:**
  - Review fatigue → small tasks, review delegates, tests as gates
  - Merge conflicts → one worktree per agent, clear file ownership
  - Cost creep → choose a cheaper model for mechanical tasks
  - Security → no secrets in prompts/logs, approval modes, approved MCPs only
  - Context/overload → limit concurrent agents (for example 3–5)
  - Over-trust → humans own the merge
- **Speaker notes:** Share one real failure story.
- **Visual placeholder:** Pitfall → guardrail table.
- **Open questions:** Which company policies to reference?

### Screen 14 – Getting started on Monday
- **Purpose:** Make it actionable.
- **Key message:** Start small: one multiplexer, one parallel task, one review habit.
- **Content:**
  1. Install a multiplexer and learn 5 keys (split, switch, detach, reattach, scroll)
  2. Run your agent inside it
  3. Try the GUI environment with one delegated task in a worktree
  4. Adopt rules: small tasks, worktree per agent, always review
  5. Links: setup guide, skill catalog, support channel (TBD)
- **Speaker notes:** Offer a follow-up office hour.
- **Visual placeholder:** Checklist.
- **Open questions:** Internal links and channel?

### Screen 15 – Q&A / thank you
- **Purpose:** Close.
- **Content:** Recap of the three takeaways, contact details, links.
- **Open questions:** —

---

## 4. Optional or backup screens
- B1: Comparison of tmux, Zellij, Herdr and Panelink (features relevant to agents)
- B2: Terminal-only vs GUI environment: when to use which
- B3: Cost model: model selection per task type
- B4: FAQ (security, data, licensing of the fork)

## 5. Open decisions log
| # | Decision | Status |
| --- | --- | --- |
| 1 | Final title and length | Open |
| 2 | Exact name and scope of the white-label fork | Open |
| 3 | Live or recorded demo | Open |
| 4 | Which data/metrics are available for Screen 12 | Open |
| 5 | Which internal tools can be named | Open |

## 6. Change log
- v0.1 – Initial skeleton draft (15 screens + backups).
