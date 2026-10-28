# Talk script: "Stop Being Your Agent's Clipboard"

Subtitle: *Let your agents see, read and control your other processes, starting with React Native Metro logs.*

Status: **Script draft v0.1**. There is no visual design yet. This is the source for building the slides later.
Focus: one concrete use case. A coding agent reads RN Metro logs directly through Panelink, so nobody has to copy and paste logs. Agent-to-agent communication is out of scope; at most it gets a one-line teaser at the end.

Markers used below:
- `[SCREENSHOT: ...]`: still image to capture
- `[VIDEO: ...]`: short screen recording (aim for 10–40 s, no audio needed)
- `[TERMINAL]`: command shown on the slide as text
- *Italic lines* are stage directions, not spoken.

Target length: about 15–18 minutes plus Q&A. Timings per screen are rough.

---

## Screen 1 – Title (0:30)

**On screen:** "Stop Being Your Agent's Clipboard". Subtitle. Name, team.

**Say:**
> Hi, I'm Sahil. For the last few months I've had a very smart, very fast coding agent, and a very expensive job title: Senior Copy-Paste Engineer.
> This talk is about how I quit that job.

`[SCREENSHOT: none, or a fake LinkedIn-style badge "Senior Copy-Paste Engineer"]` (optional joke asset)

---

## Screen 2 – The ritual (1:30)

**On screen:** The copy-paste loop as a cycle of 6 steps:
1. Agent changes code
2. Metro reloads
3. Something breaks
4. *You* scroll the Metro terminal, find the error, select it, copy it
5. *You* paste it into the agent chat ("here's the error…")
6. Agent: "Can you share the full stack trace?"
→ back to step 4

**Say:**
> Every React Native dev knows this loop. The agent edits code, Metro rebuilds, the app goes red, and then the most advanced AI on the planet waits patiently for a human to drag a mouse across a terminal.
> And we never copy the right amount. Too little and it asks for more. Too much and we've pasted 400 lines of bundler noise plus somebody's auth token.

`[VIDEO: 15 s real recording of the manual loop: scroll Metro, select, Cmd-C, switch window, Cmd-V into agent, agent asks for more context]`

*Stage direction: let the video play once without talking. The pain sells itself.*

---

## Screen 3 – The real cost (1:00)

**On screen:** Three bullets:
- **Context switches**: you leave your flow every time something breaks
- **Lossy hand-off**: you decide what the agent sees, and you're usually wrong
- **The agent waits on you**: an autonomous agent can't loop without you

**Say:**
> The problem isn't the 5 seconds of copy-paste. It's that the agent's feedback loop now runs through *you*. You're the slowest, most distractible component in the system, and you're on the critical path.

`[PLACEHOLDER: optional number — "I did this N times in one afternoon" — only if actually counted]`

---

## Screen 4 – The idea in one line (0:45)

**On screen, big text:**
> **Give your agent eyes and hands on your other terminals.**
> See it. Read it. Control it (carefully).

**Say:**
> What if the agent could just look at the Metro terminal itself? It doesn't need a log integration, a plugin or a new SDK. It looks at the same terminal you're looking at.

---

## Screen 5 – Meet Panelink (1:30)

**On screen:**
- Panelink: an experimental terminal multiplexer (Rust, Apple Silicon Mac) that runs inside your normal terminal
- Your shells live in a background server, so closing the window doesn't kill Metro
- It comes with a CLI that *other programs*, including agents, can use to list panes, read them, wait on them and type into them

Simple diagram, described in words for now:
`Terminal window → Panelink client → Panelink server → panes: [agent] [Metro] [shell]`

**Say:**
> Think tmux, but built from day one for agents to drive. The important part isn't the split panes. It's that every pane becomes something an agent can *query*.
> Disclaimer: this is my experimental project, not a company product. It's Mac-only and still rough around the edges.

`[SCREENSHOT: Panelink with 3 panes: OpenCode on the left, Metro top-right, a shell bottom-right]`

---

## Screen 6 – Superpower 1: SEE (1:00)

**On screen:**
`[TERMINAL]`
```sh
panelink pane list --json
```
Callout: "Which pane is Metro? The agent finds out itself."

**Say:**
> First, the agent needs to know what's around it. `pane list` tells it which panes exist, where they are, and what's running in them. Then it reads the screen to confirm. It doesn't trust a pane number blindly, which is more care than I usually take.

`[SCREENSHOT: JSON output of pane list, cropped to cwd/command fields]`

---

## Screen 7 – Superpower 2: READ (2:00) *(the core slide)*

**On screen:**
`[TERMINAL]`
```sh
# last visible rows of the Metro pane
panelink pane read 2 --bottom --lines 40 --json

# recent history that has scrolled off screen
panelink pane read 2 --scrollback --lines 200 --json
```
Callout: "No copy. No paste. No 'can you share the full stack trace?'"

**Say:**
> This is the whole talk in two commands. The agent reads the bottom of the Metro pane, sees the red error, and if it needs more it pulls the scrollback. It chooses how much context it needs, not me.
> And it reads the *exact* terminal I'm looking at, so we're both debugging the same truth.

`[VIDEO: 30 s. Introduce a deliberate bug → Metro shows the error → in the agent pane type "the app is broken, check Metro" → agent runs pane read → agent quotes the exact error and file/line → proposes a fix]`

---

## Screen 8 – Superpower 3: WAIT (1:30)

**On screen:**
`[TERMINAL]`
```sh
# wait until Metro shows a bundle finished (marker text is an example)
panelink wait 2 --text 'Bundled' --timeout 60 --json

# or wait until output settles
panelink wait 2 --quiet 2 --timeout 30 --json
```
Callout: "No more `sleep 10 && hope`."

**Say:**
> Agents are terrible at waiting. They either sleep for a random number of seconds or poll like an impatient toddler: "is it done? is it done?"
> With `wait`, the agent says "tell me when Metro prints this" or "tell me when it goes quiet", then reads the result. The loop becomes: edit → wait for rebuild → read logs → fix. No human required.

`[SCREENSHOT: agent transcript showing wait → read → "build succeeded, no new errors"]`

---

## Screen 9 – Superpower 4: CONTROL (carefully) (1:30)

**On screen:**
`[TERMINAL]`
```sh
# press "r" in Metro to reload the app (types, no Enter)
panelink pane send 2 r --claim --as my-agent --json

# restart with a clean cache: Ctrl-C, then relaunch
panelink pane keys 2 C-c
panelink pane submit 2 'npx react-native start --reset-cache'
```
Callout: "Reload, restart, reset cache: the chores you used to do."

**Say:**
> Reading is great. But half of RN debugging is "have you tried reloading?" and "have you tried clearing the cache?" The agent can now do that itself.
> "Carefully" is doing a lot of work on this slide. Typing into a terminal is real power, so Panelink makes writers claim the pane, confirms whether input actually landed, and never blindly retries input it isn't sure about. You decide which panes an agent is allowed to touch.

`[VIDEO: 15 s. Agent decides cache is stale → Ctrl-C Metro → restarts with --reset-cache → waits for ready → reads logs → reports green]`

---

## Screen 10 – Before vs after (1:00)

**On screen:** Two columns.

| Before | After |
| --- | --- |
| You spot the error | Agent reads Metro itself |
| You pick what to copy | Agent picks how much context |
| You paste into chat | Nothing to paste |
| You reload / reset cache | Agent reloads and waits for ready |
| Agent waits on you | Agent loops on its own; you review |

**Say:**
> Same agent, same model, same repo. The only change is that the agent can see the terminal next to it. That turns "assistant who asks me for logs" into "colleague who checks the logs first".

`[PLACEHOLDER: real measurement if available, e.g. "time from red screen to fix" across N bugs, before vs after. Do not invent numbers; label anecdotal data as such]`

---

## Screen 11 – Live demo (3:00)

*Stage direction: run live if the setup is stable, otherwise play the recorded backup. Keep the backup video ready on the same machine.*

**Demo script:**
1. Show the layout: agent pane, Metro pane, shell pane. *"Three panes. One of them is about to break."*
2. Introduce a bug: rename a prop or import a missing module.
3. Metro goes red. **Don't touch it.** *"Normally this is where I'd start selecting text. Hands off the keyboard."*
4. Tell the agent: "Something broke, check Metro and fix it."
5. The agent lists panes, reads Metro, finds the error, edits the file.
6. The agent waits for the rebuild, reads Metro again, and confirms it's clean.
7. Bonus: close the terminal window, reopen with `panelink`, and Metro is still running. *"Metro survived me closing the window. That's more than my laptop battery does."*

`[VIDEO: full 2–3 min backup recording of the above]`

---

## Screen 12 – Honest limits (1:00)

**On screen:**
- Experimental, Apple Silicon Mac only
- Agents see the **screen and retained scrollback**, which is bounded, not infinite
- Screen text is data, not instructions: the agent shouldn't execute what logs tell it
- Logs can contain secrets, so be deliberate about what panes agents read
- Newer RN versions moved JS `console.log` output from Metro to DevTools. Check what your Metro terminal actually shows (bundling errors are still there)
- Input control is a privilege, so grant it per pane, per task

**Say:**
> I'd rather tell you the sharp edges than have you find them. It's a power tool, so the usual rules apply.

*Prep note: confirm the RN version and log behavior of the demo app before the talk, and adjust this bullet.*

---

## Screen 13 – Beyond Metro (0:45)

**On screen:** "Anything that prints to a terminal is now agent-readable":
- Test watchers (Jest `--watch`)
- Gradle / Xcode builds
- `adb logcat`
- Backend dev servers
- *(Teaser)* other agents. That's another talk.

**Say:**
> Metro is just my favorite example. If it prints to a terminal, your agent can see it, wait on it and react to it.

---

## Screen 14 – Try it / takeaway (0:45)

**On screen:**
> **Stop being your agent's clipboard.**
> 1. Run your dev servers inside a multiplexer
> 2. Teach your agent to read panes (Panelink ships a skill for this)
> 3. Let it wait, read and fix, and keep yourself as the reviewer

Links: repo / setup guide / contact (TBD)

**Say:**
> Your agent is fast. Stop making it wait for your mouse. Thanks!

---

## Screen 15 – Q&A

Likely questions and short answers:
- **"Why not just tmux + capture-pane?"** You can, and it's the same idea. Panelink adds agent-oriented guards: target pinning, input claims, no blind retries, bounded waits and JSON output.
- **"Is it safe?"** Reads are read-only. Writes require a claim and are confirmed. You choose which panes agents may touch. Logs can still contain secrets.
- **"Does it work on Windows/Linux/Intel?"** Not today. It's Apple Silicon Mac only.
- **"Does the agent need a plugin?"** No, it's a CLI. Any agent that can run shell commands can use it. A skill file teaches it the commands.

---

## Asset checklist

| ID | Type | Description | Status |
| --- | --- | --- | --- |
| A1 | Video | Manual copy-paste loop (Screen 2) | ☐ |
| A2 | Screenshot | Panelink 3-pane layout (Screen 5) | ☐ |
| A3 | Screenshot | `pane list --json` output (Screen 6) | ☐ |
| A4 | Video | Agent reads Metro error and fixes it (Screen 7) | ☐ |
| A5 | Screenshot | wait → read transcript (Screen 8) | ☐ |
| A6 | Video | Agent reset-cache restart (Screen 9) | ☐ |
| A7 | Video | Full demo backup (Screen 11) | ☐ |
| A8 | Optional | "Senior Copy-Paste Engineer" joke badge (Screen 1) | ☐ |

## Prep / verification before recording

- [ ] Confirm the exact commands against the installed `panelink --help` (the syntax above follows the current repo skill and may change)
- [ ] Confirm the Metro "ready/bundled" marker text in your RN version for the `wait --text` example
- [ ] Confirm what JS logs appear in the Metro terminal for the demo app's RN version
- [ ] Prepare a deterministic bug for the demo and a clean git state to reset to
- [ ] Hide tokens, internal URLs and env values in all recordings
- [ ] Decide which internal tool names can be shown

## Open decisions
1. Final title (alternatives: "Let Your Agent Read the Room", "Hands Off the Clipboard")
2. Live demo or recorded only
3. Whether to mention the T3 Code / GUI fork at all, or keep the talk purely terminal-focused
4. Real before/after numbers available?
