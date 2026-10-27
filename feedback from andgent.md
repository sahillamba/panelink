# Feedback from an agent inside Panelink

Date: 2026-10-27

## What worked

- Pane discovery returned geometry and exact targets, allowing me to identify the left shell and this Codex session on the right.
- Bounded screen reads worked and let me inspect the other pane without sending input.
- Starting Codex in the left pane worked; its version and project directory appeared on screen.
- A separately sent Enter eventually submitted the coordination task. Codex answered, and a bounded text wait matched its unique completion marker.
- Successful input and release responses exposed useful delivery and ownership status.

## Problems observed

1. **PTY delivery did not establish application submission.** `pane submit` reported confirmed text-plus-Enter delivery, but Codex continued showing the prompt as an unfinished draft. A later explicit Enter worked. This observation does not establish the underlying cause; the text was not resent.
2. **Control failures interrupted otherwise working access.** A completion wait and an Enter attempt returned `control_unavailable`, while subsequent screen reads succeeded. The generic error did not reveal whether connection, query, or reconciliation failed.
3. **Ownership was hard to follow after failure.** A claim reported success, a key operation failed, and a later key attempt reported `input_not_claimed`. A fresh claim, Enter, and release then succeeded. Clearer reconciliation information would help an agent understand this transition.
4. **Agent state recognition was inconclusive.** The left Codex process was recognized, but its screen state was `unknown` while its input prompt was visible. State alone could not support the coordination test.
5. **Short screen reads can miss the actionable area.** Reading fewer rows showed the top of the pane while the prompt was near the bottom. Reading the full 64-row screen exposed the draft and later answer.
6. **Build identity needs to be explicit.** The installed 0.1.3 executable resolves to another worktree. Tests in `/Users/sahillamba/panelink` exercised `main` at `bd84625cef37cf037e2045ec0cb6a31390ff7db0`, package 0.1.0. Those results do not certify the installed binary.

## Recommended improvements

- Investigate text-plus-Enter handling against Codex, including paste framing and application readiness. Preserve the distinction between a confirmed PTY write and an accepted prompt.
- Report connection/query failure stages and outstanding operation reconciliation state without exposing input text or credentials.
- Include useful ownership state when input is refused, so agents can determine whether a claim remains valid without guessing or changing caller identities.
- Improve recognition of supported Codex idle screens; retain `unknown` when evidence is insufficient.
- Document screen row selection clearly, and consider a bounded bottom-of-screen read option for terminal agents.
- Expose build revision and server capability information alongside the package version.
- For future agent coordination, consider task IDs and explicit result notifications. Terminal state heuristics should remain observations rather than proof of task completion.

## Coordination pattern demonstrated

Give a worker a bounded task and a unique completion marker that does not appear verbatim in the echoed prompt. Wait for that marker with a deadline, read the response, validate the result, and acknowledge the task after review. A timeout means completion has not been observed; it does not authorize replaying input. Screen silence and an `idle` classification alone do not prove success.

The live test produced `PANELINK_COORD_DONE_7Q9` in Codex's final response, and `panelink wait` returned `matched: true`. This demonstrates one successful screen-based handshake, not reliable asynchronous task notifications or provider conversation identity.

## Important-feature verification

The fresh source verification passed 675 library tests, 39 decoder tests, build, strict Clippy, formatting, component fingerprints, and documentation checks. Focused PTY, detach, managed-launch, history-storage, corruption-handling, saved-layout lifecycle, and recovery-reading tests also passed.

The canonical verifier still failed overall: one binary test encountered socket-bind EPERM, and one of 77 Python tests failed at loopback listener creation. An independent loopback bind reproduced `Operation not permitted`. Isolated four-pane, agent CLI, close, and recovery-relaunch scenarios failed at startup or socket-related boundaries. One explicitly attempted ignored terminal cleanup test failed with child exit 1 instead of 7; its exact cause was not established by that run.

These failures remain failures or blocked acceptance, not passes. Actual-host mouse and clipboard behavior, physical reboot, provider recovery, the full integration sweep, and isolated acceptance of the installed build remain unverified in this test pass. No application guards were weakened and the existing live panes were preserved.

Detailed counts and commands: [Important-feature verification](docs/progress/IMPORTANT-FEATURE-VERIFICATION-2026-10-27.md).
