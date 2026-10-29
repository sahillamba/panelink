# "Stop Being Your Agent's Clipboard": 3-minute version

Four slides, about 2:45 total. Condensed from [the full script](script-stop-being-your-agents-clipboard.md).
Slides file: `stop-being-your-agents-clipboard.pptx`. Import it with Google Slides → File → Import slides, or upload it to Drive and open it with Google Slides. Speaker notes are included.

---

## Slide 1 – The copy-paste loop (0:40)

**On slide:** Title "Stop Being Your Agent's Clipboard".
Loop: Agent edits → Metro reloads → app goes red → **you** copy logs → **you** paste → "can you share the full stack trace?" ↺
`[VIDEO: 10–15 s of the manual copy-paste loop]`

**Say:**
> For months my official job title has been Senior Copy-Paste Engineer. My agent edits code, Metro goes red, and the most advanced AI on the planet waits for me to drag a mouse across a terminal. Then it asks for more of the stack trace. The agent's feedback loop runs through me, and I'm the slowest part of it.

## Slide 2 – Give your agent eyes and hands (0:50)

**On slide:** "Let your agents **see, read, wait on and control** your other terminals." Panelink is a terminal multiplexer whose panes agents can query.
```sh
panelink pane list                       # SEE: which pane is Metro?
panelink pane read 2 --bottom --lines 40 # READ: the error, no copy-paste
panelink wait 2 --text 'Bundled'         # WAIT: rebuild done, no sleep 10
panelink pane send 2 r --claim           # CONTROL: reload Metro
```
`[SCREENSHOT: Panelink with agent pane + Metro pane side by side]`

**Say:**
> Panelink is my experimental terminal multiplexer. Metro runs in one pane and my agent in another, and the agent can query the panes around it. It lists panes to find Metro, reads the error itself, waits for the rebuild instead of guessing with sleep, and can even press "r" to reload. There's no plugin and no log SDK. It's the same terminal I'm looking at.

## Slide 3 – Demo: red screen to green, hands off (1:00)

**On slide:** "Something broke. Check Metro and fix it." (that's the whole prompt)
`[VIDEO: 45–60 s. Bug introduced → Metro red → agent reads pane → fixes file → waits for rebuild → reads again → green]`

**Say (over the video):**
> I break an import. Metro goes red, and I don't touch it. I tell the agent "check Metro and fix it". It finds the pane, reads the exact error, fixes the file, waits for the rebuild and checks the logs again. Green. My only job was reviewing the diff.

## Slide 4 – Takeaway (0:15)

**On slide:** **Stop being your agent's clipboard.**
Run dev servers in a multiplexer → let the agent read and wait → you review.
Small print: experimental, Apple Silicon Mac only; logs can contain secrets; on newer RN versions JS console logs go to DevTools, not Metro.

**Say:**
> Your agent is fast. Stop making it wait for your mouse. Thanks!

---

## Prep before recording
- [ ] Confirm the commands against the installed `panelink --help`
- [ ] Confirm the Metro "bundled/ready" text for your RN version
- [ ] Check that the demo error actually shows in the Metro terminal
- [ ] Hide tokens and internal URLs in recordings
