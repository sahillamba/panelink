# P4 native managed-bridge failure acceptance

This fixture checks failure handling for a live managed provider bridge using
the pinned OpenCode 2.0.32 client. It starts one private Panelink `serve`
process in a fresh HOME and loopback-only sandbox, waits for source session A
to be durably confirmed, then identifies its exact PTY child by both direct
parent PID and canonical executable path. It sends `SIGSTOP` only after those
checks pass.

The fixture waits for the owner's fixed `stale` or `unavailable` status, checks
that the owner and PTY child remain alive and the binding revision remains at
A, then asks to prepare a valid same-project session B. Preparation must be
refused while the TUI process is paused. It sends `SIGCONT` to the same verified
PID, checks the process path and parent again, and confirms that the source
remains unavailable/stale and the original A record has not advanced or been
replaced. The fixture makes no prompt submissions or model calls.

Only fixed statuses, booleans, and hashes are emitted. Process IDs, session
IDs, paths, command output, and provider payloads are not included. Cleanup
resumes the child if needed, closes the owner, and confirms the fixture-owned
child is reaped. Historical inspection after shutdown must retain the exact A
hash, original revision, provider version, and managed-launch link.

Run focused tests with:

```sh
python3 -m unittest tests.test_panelink_bridge_failure -v
```

Run the native acceptance with the isolated Panelink binary:

```sh
python3 scripts/probe_panelink_bridge_failure.py \
  --panelink-binary /private/tmp/panelink-p4-bridge-store-target/debug/panelink
```

## Native result (2026-10-24)

The focused helper tests passed **3/3**. The native fixture passed in the
enforced loopback-only sandbox with OpenCode **2.0.32** and pinned SHA-256
`e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf`; the
Panelink binary SHA-256 was
`1497ac732020573d633df85d38fdd594a92b31d7f1b8ee29d06e0087e52ec9ee`.

Source A was durable at revision 1 before the fault. `proc_listchildpids`
identified one direct child; the fixture then verified that PID's parent was
the private `serve` process and its canonical executable path matched the
pinned OpenCode binary before sending `SIGSTOP`. The owner reported
`unavailable` while the `serve` process remained alive, and the source revision
stayed at 1. Preparing a valid same-project B was refused with the fixed
`provider_recovery_unavailable` result.

The fixture sent `SIGCONT` to the same verified PID and confirmed that it was
running under the same parent and executable. The owner remained unavailable;
status returned `manual_selection_required` with `explicit_choice_not_prepared`,
and another prepare returned `provider_recovery_unavailable`. The source A
record remained exact both while the failure was active and after resume.
Historical inspection after shutdown confirmed A's hash, revision 1, pinned
provider version, and managed-launch link. The fixture verified zero user
messages, zero prompt submissions, and complete cleanup.

This demonstrates fail-closed handling for this pinned native process and
managed target. It is one provider failure gate and does not close Phase 4 by
itself.
