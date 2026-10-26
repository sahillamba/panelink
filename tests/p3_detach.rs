#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::client_context::{ContextFile, ReconcileResult};
use panelink::control_protocol::{
    encode_request_payload, Command, ControlRequest, ReadSource, TargetRef,
};
use panelink::handshake_protocol::{Hello, CAPABILITY_INPUT, CAPABILITY_READ};
use panelink::launch::{Environment, ResolvedLaunch};
use panelink::protocol::{OperationKey, OperationStatus, Outcome};
use panelink::response_protocol::{decode_response_payload, RejectionCode, ResponseBody};
use panelink::server::{EffectSink, SessionReply, StateOwner};
use panelink::server_pty::{PtyEffects, MAX_READ_TEXT_BYTES};
use panelink::session_protocol::{encode_session_request, LeaseRef, SessionHello, SessionRequest};
use panelink::terminal::{TerminalSession, TerminalSize};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const GENERATION: u64 = 73;
const IO_DEADLINE: Duration = Duration::from_secs(3);
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = Path::new("/private/tmp").join(format!(
            "panelink-p3-detach-{}-{serial}",
            std::process::id()
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .expect("create private PTY fixture directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn context_path(&self) -> PathBuf {
        self.0.join("client.context")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn target() -> TargetRef {
    TargetRef {
        generation: GENERATION,
        workspace: 11,
        pane: 13,
        session: 17,
        occupant_revision: 19,
    }
}

fn environment() -> Environment {
    [(OsString::from("TERM"), OsString::from("panelink-probe"))]
        .into_iter()
        .collect()
}

fn launch(cwd: &Path) -> ResolvedLaunch {
    ResolvedLaunch {
        requested_path: PathBuf::from("/bin/sh"),
        executable_path: PathBuf::from("/bin/sh"),
        working_directory: cwd.to_path_buf(),
    }
}

fn shell(script: &str, cwd: &Path) -> TerminalSession {
    TerminalSession::spawn(
        &launch(cwd),
        &[OsString::from("-c"), OsString::from(script)],
        &environment(),
        TerminalSize::new(8, 80).unwrap(),
    )
    .expect("spawn one fixture shell on a private PTY")
}

fn hello(resume: Option<LeaseRef>, nonce: u64) -> SessionHello {
    SessionHello {
        hello: Hello {
            nonce,
            required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
            optional_capabilities: 0,
        },
        resume,
    }
}

fn read_sink_screen<E: EffectSink>(state: &mut StateOwner<E>) -> String {
    state
        .effects_mut()
        .read(ReadSource::Screen, 4_000, 8)
        .expect("read current sanitized engine cells")
        .text
}

/// Shell startup under heavy machine load can exceed the 3 s IO deadline;
/// waiting longer for a marker never weakens what the test proves.
const MARKER_DEADLINE: Duration = Duration::from_secs(10);

fn wait_for_screen<E: EffectSink>(
    state: &mut StateOwner<E>,
    now: impl Fn() -> u64,
    needle: &str,
) -> String {
    let deadline = Instant::now() + MARKER_DEADLINE;
    loop {
        state
            .poll(now())
            .expect("make one bounded PTY progress step");
        let text = read_sink_screen(state);
        if text.contains(needle) {
            return text;
        }
        assert!(Instant::now() < deadline, "PTY output marker timed out");
        thread::sleep(Duration::from_millis(5));
    }
}

fn reserve(context: &mut ContextFile) -> OperationKey {
    context.reserve().expect("durably reserve operation key")
}

fn status_from(payload: &[u8]) -> OperationStatus {
    let response = decode_response_payload(payload).expect("decode server response");
    match response.body {
        ResponseBody::Status(status) => status,
        _ => panic!("expected an operation status response"),
    }
}

fn encode_control(request: &ControlRequest<'_>) -> Vec<u8> {
    let mut payload = Vec::new();
    encode_request_payload(request, &mut payload).expect("encode typed control request");
    payload
}

fn elapsed_secs(start: Instant) -> u64 {
    100 + start.elapsed().as_secs()
}

#[test]
fn logical_detach_resume_and_lost_input_ack_never_relaunch_or_replay() {
    let directory = TempDir::new();
    let started = Instant::now();
    let script = concat!(
        "printf 'READY-P3-%s\\n' \"$$\"; ",
        "IFS= read -r first; ",
        "if [ \"$first\" = 'typed-token' ]; then printf 'GOT-P3-%s\\n' \"$$\"; fi; ",
        "sleep 0.2; printf 'AFTER-P3-%s\\n' \"$$\"; ",
        "IFS= read -r second; ",
        "if [ \"$second\" = 'typed-token' ]; then printf 'GOT-SECOND-P3-%s\\n' \"$$\"; fi; ",
        "sleep 10"
    );
    let effects = PtyEffects::new(shell(script, directory.path()));
    let mut state = StateOwner::new(GENERATION, target(), effects, true).unwrap();
    let (connection, _) = state
        .connect(&hello(None, 1), elapsed_secs(started))
        .unwrap();
    let mut context = ContextFile::create(
        &directory.context_path(),
        GENERATION,
        connection.lease(),
        Instant::now() + IO_DEADLINE,
    )
    .expect("create persistent metadata-only client context");

    wait_for_screen(&mut state, || elapsed_secs(started), "READY-P3-");

    let claim_key = reserve(&mut context);
    let claim_payload = encode_session_request(&SessionRequest::Claim {
        key: claim_key,
        target: target(),
        expected_owner_revision: 1,
        ttl_seconds: 30,
    })
    .unwrap();
    let claim_reply = state
        .handle_session(&connection, &claim_payload, elapsed_secs(started))
        .unwrap();
    let SessionReply::Payload(claim_status) = claim_reply else {
        panic!("claim returns a status payload");
    };
    assert_eq!(
        status_from(&claim_status),
        OperationStatus::Completed(Outcome::Applied)
    );
    context.ack(claim_key).unwrap();

    let initial_read_key = reserve(&mut context);
    let initial_read_payload = encode_control(&ControlRequest {
        key: initial_read_key,
        target: target(),
        command: Command::Read {
            source: ReadSource::Screen,
            max_bytes: 4_000,
            max_lines: 8,
        },
    });
    let initial_read = state
        .handle_control(&connection, &initial_read_payload, elapsed_secs(started))
        .unwrap();
    let initial_response = decode_response_payload(&initial_read).unwrap();
    let initial_revision = match initial_response.body {
        ResponseBody::Read {
            screen_revision,
            text,
            ..
        } => {
            assert!(text.contains("READY-P3-"));
            screen_revision
        }
        _ => panic!("screen read returns display text"),
    };
    context.ack(initial_read_key).unwrap();

    let submit_key = reserve(&mut context);
    let submit_payload = encode_control(&ControlRequest {
        key: submit_key,
        target: target(),
        command: Command::Submit {
            owner_revision: 2,
            text: "typed-token",
        },
    });
    let _lost_ack = state
        .handle_control(&connection, &submit_payload, elapsed_secs(started))
        .unwrap();
    // Simulate losing the acknowledgement: leave the durable uncertain marker
    // in place, then drop this invocation's lock without resending its payload.
    drop(_lost_ack);
    drop(context);

    let detach = encode_session_request(&SessionRequest::Detach).unwrap();
    assert!(matches!(
        state
            .handle_session(&connection, &detach, elapsed_secs(started))
            .unwrap(),
        SessionReply::Detached
    ));
    let detached_screen = wait_for_screen(&mut state, || elapsed_secs(started), "AFTER-P3-");
    assert!(detached_screen.contains("GOT-P3-"));
    let pid = detached_screen
        .lines()
        .find_map(|line| line.strip_prefix("READY-P3-"))
        .expect("initial process marker remains visible")
        .trim();
    assert!(detached_screen.contains(&format!("GOT-P3-{pid}")));
    assert!(detached_screen.contains(&format!("AFTER-P3-{pid}")));

    let (resumed, _) = state
        .connect(
            &hello(
                Some(LeaseRef {
                    generation: connection.generation(),
                    lease: connection.lease(),
                }),
                2,
            ),
            elapsed_secs(started),
        )
        .unwrap();
    assert_eq!(resumed.generation(), connection.generation());
    assert_eq!(resumed.lease(), connection.lease());

    let status_payload =
        encode_session_request(&SessionRequest::OperationStatus { key: submit_key }).unwrap();
    let observed_status = match state
        .handle_session(&resumed, &status_payload, elapsed_secs(started))
        .unwrap()
    {
        SessionReply::Payload(payload) => status_from(&payload),
        _ => panic!("operation status returns a payload"),
    };
    assert_eq!(
        observed_status,
        OperationStatus::Completed(Outcome::Applied)
    );

    let mut reopened_context = ContextFile::open(
        &directory.context_path(),
        GENERATION,
        resumed.lease(),
        Instant::now() + IO_DEADLINE,
    )
    .expect("reopen the exact generation/lease context after reconnect");
    assert_eq!(reopened_context.context().uncertain(), Some(submit_key));
    assert_eq!(
        reopened_context
            .reconcile(submit_key, observed_status)
            .unwrap(),
        ReconcileResult::Cleared
    );
    assert_eq!(reopened_context.context().uncertain(), None);

    // Inject a repeated wire payload to verify server-side deduplication. The
    // client path above reconciled by status and did not use this as a retry.
    let duplicate_status = state
        .handle_control(&resumed, &submit_payload, elapsed_secs(started))
        .unwrap();
    assert_eq!(
        status_from(&duplicate_status),
        OperationStatus::Completed(Outcome::Applied)
    );

    let stale_target_key = reserve(&mut reopened_context);
    let mut stale_target = target();
    stale_target.occupant_revision += 1;
    let stale_target_payload = encode_control(&ControlRequest {
        key: stale_target_key,
        target: stale_target,
        command: Command::Submit {
            owner_revision: 2,
            text: "typed-token",
        },
    });
    let rejected_target = state
        .handle_control(&resumed, &stale_target_payload, elapsed_secs(started))
        .unwrap();
    assert!(matches!(
        decode_response_payload(&rejected_target).unwrap().body,
        ResponseBody::Rejected(RejectionCode::StaleTarget)
    ));
    reopened_context.ack(stale_target_key).unwrap();

    let stale_owner_key = reserve(&mut reopened_context);
    let stale_owner_payload = encode_control(&ControlRequest {
        key: stale_owner_key,
        target: target(),
        command: Command::Submit {
            owner_revision: 1,
            text: "typed-token",
        },
    });
    let rejected_owner = state
        .handle_control(&resumed, &stale_owner_payload, elapsed_secs(started))
        .unwrap();
    assert!(matches!(
        decode_response_payload(&rejected_owner).unwrap().body,
        ResponseBody::Rejected(RejectionCode::StaleOwner)
    ));
    reopened_context.ack(stale_owner_key).unwrap();

    let new_read_key = reserve(&mut reopened_context);
    let new_read_payload = encode_control(&ControlRequest {
        key: new_read_key,
        target: target(),
        command: Command::Read {
            source: ReadSource::Screen,
            max_bytes: 4_000,
            max_lines: 8,
        },
    });
    let new_read = state
        .handle_control(&resumed, &new_read_payload, elapsed_secs(started))
        .unwrap();
    let response = decode_response_payload(&new_read).unwrap();
    let current_revision = match response.body {
        ResponseBody::Read {
            screen_revision,
            text,
            ..
        } => {
            assert!(text.contains("AFTER-P3-"));
            assert!(text.contains("GOT-P3-"));
            assert!(!text.contains("GOT-SECOND-P3-"));
            screen_revision
        }
        _ => panic!("reconnected read returns current display text"),
    };
    assert!(current_revision > initial_revision);
    reopened_context.ack(new_read_key).unwrap();

    let describe = encode_session_request(&SessionRequest::Describe).unwrap();
    let SessionReply::Info(info) = state
        .handle_session(&resumed, &describe, elapsed_secs(started))
        .unwrap()
    else {
        panic!("describe returns current session info");
    };
    assert_eq!(info.target, target());
    assert_eq!(info.owner_lease, resumed.lease());
    assert!(!info.child_exited);
}

#[test]
fn interrupt_delivers_sigint_after_a_ready_marker() {
    let directory = TempDir::new();
    let started = Instant::now();
    let script = concat!(
        "trap 'printf \\\"SIGNAL-GOT-P3-%s\\\\n\\\" \\\"$$\\\"; exit 0' INT; ",
        "printf 'READY-SIG-P3-%s\\n' \"$$\"; ",
        "while :; do sleep 30; done"
    );
    let effects = PtyEffects::new(shell(script, directory.path()));
    let mut state = StateOwner::new(GENERATION, target(), effects, true).unwrap();
    let (connection, _) = state
        .connect(&hello(None, 5), elapsed_secs(started))
        .unwrap();
    let mut context = ContextFile::create(
        &directory.context_path(),
        GENERATION,
        connection.lease(),
        Instant::now() + IO_DEADLINE,
    )
    .unwrap();
    wait_for_screen(&mut state, || elapsed_secs(started), "READY-SIG-P3-");

    let claim_key = reserve(&mut context);
    let claim = encode_session_request(&SessionRequest::Claim {
        key: claim_key,
        target: target(),
        expected_owner_revision: 1,
        ttl_seconds: 30,
    })
    .unwrap();
    let SessionReply::Payload(claim_status) = state
        .handle_session(&connection, &claim, elapsed_secs(started))
        .unwrap()
    else {
        panic!("claim returns a status payload");
    };
    assert_eq!(
        status_from(&claim_status),
        OperationStatus::Completed(Outcome::Applied)
    );
    context.ack(claim_key).unwrap();

    let interrupt_key = reserve(&mut context);
    let interrupt_payload = encode_control(&ControlRequest {
        key: interrupt_key,
        target: target(),
        command: Command::Interrupt { owner_revision: 2 },
    });
    let interrupted = state
        .handle_control(&connection, &interrupt_payload, elapsed_secs(started))
        .unwrap();
    assert_eq!(
        status_from(&interrupted),
        OperationStatus::Completed(Outcome::Applied),
        "Applied means the OS accepted SIGINT, not that the process already exited"
    );
    context.ack(interrupt_key).unwrap();

    let signal_output = wait_for_screen(&mut state, || elapsed_secs(started), "SIGNAL-GOT-P3-");
    assert!(signal_output.contains("READY-SIG-P3-"));
    assert!(signal_output.contains("SIGNAL-GOT-P3-"));
    let exit_deadline = Instant::now() + IO_DEADLINE;
    while !state.effects().child_exited() {
        state
            .poll(elapsed_secs(started))
            .expect("observe the signalled child's exit on a bounded poll");
        assert!(Instant::now() < exit_deadline, "SIGINT trap did not exit");
        thread::sleep(Duration::from_millis(5));
    }
    assert!(state.effects().child_exited());
}

#[test]
fn pty_reads_are_sanitized_utf8_bounded_and_live_scrollback_is_available() {
    let directory = TempDir::new();
    let script = concat!(
        "printf 'SAFE-P3-漢字-🧭\\033]52;c;CLIPBOARD-SECRET\\007\\n'; ",
        "sleep 10"
    );
    let mut effects = PtyEffects::new(shell(script, directory.path()));
    assert_eq!(MAX_READ_TEXT_BYTES, 65_446);
    assert_eq!(effects.revision(), 1);
    assert_eq!(
        effects.validate(Command::Read {
            source: ReadSource::LiveScrollback,
            max_bytes: 1_000,
            max_lines: 8,
        }),
        Ok(())
    );

    let too_long = "x".repeat(32_769);
    assert_eq!(
        effects.validate(Command::Paste {
            owner_revision: 1,
            text: &too_long,
        }),
        Err(RejectionCode::InvalidRequest)
    );

    let deadline = Instant::now() + IO_DEADLINE;
    let full = loop {
        effects.poll().expect("bounded PTY progress");
        let result = effects
            .read(ReadSource::Screen, MAX_READ_TEXT_BYTES, 8)
            .unwrap();
        if result.text.contains("SAFE-P3-") {
            break result;
        }
        assert!(
            Instant::now() < deadline,
            "safe marker did not reach screen"
        );
        thread::sleep(Duration::from_millis(5));
    };
    assert!(full.text.contains("漢字-🧭"));
    assert!(!full.text.contains('\u{1b}'));
    assert!(!full.text.contains("CLIPBOARD-SECRET"));
    assert!(full.text.len() <= MAX_READ_TEXT_BYTES);

    let bounded = effects.read(ReadSource::Screen, 10, 1).unwrap();
    assert_eq!(bounded.text, "SAFE-P3-");
    assert!(bounded.text.is_char_boundary(bounded.text.len()));
    assert!(bounded.truncated);
    assert!(!bounded.text.contains('\n'));
    assert_eq!(
        effects.read(ReadSource::Screen, MAX_READ_TEXT_BYTES + 1, 1),
        Err(Outcome::Failed)
    );
    assert_eq!(
        effects.read(ReadSource::LiveScrollback, MAX_READ_TEXT_BYTES + 1, 1),
        Err(Outcome::Failed)
    );
    assert_eq!(
        effects.read(ReadSource::LiveScrollback, MAX_READ_TEXT_BYTES, 0),
        Err(Outcome::Failed)
    );
}

#[test]
fn live_scrollback_reads_recent_normal_rows_while_alt_screen_is_active() {
    let directory = TempDir::new();
    let script = concat!(
        "printf 'HIST-00\\nHIST-01\\nHIST-02\\nHIST-03\\nHIST-04\\nHIST-05\\n",
        "HIST-06\\nHIST-07\\nHIST-08\\nHIST-09\\nHIST-10\\nHIST-11\\n",
        "\\033[?1049hALT-ONLY\\n'; ",
        "sleep 10"
    );
    let mut effects = PtyEffects::new(shell(script, directory.path()));
    let deadline = Instant::now() + IO_DEADLINE;
    loop {
        effects.poll().expect("make bounded PTY progress");
        let visible = effects
            .read(ReadSource::Screen, MAX_READ_TEXT_BYTES, 8)
            .expect("read the visible alternate screen");
        if visible.text.contains("ALT-ONLY") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "alternate-screen marker timed out"
        );
        thread::sleep(Duration::from_millis(5));
    }

    let history = effects
        .read(ReadSource::LiveScrollback, MAX_READ_TEXT_BYTES, 3)
        .expect("read the current normal-history tail through the live PTY effect");
    let rows: Vec<_> = history.text.split('\n').map(str::trim_end).collect();
    assert_eq!(rows, ["HIST-02", "HIST-03", "HIST-04"]);
    assert_eq!(history.text.matches('\n').count(), 2);
    assert!(history.truncated, "older retained rows exceeded max_lines");
    assert!(!history.text.contains("ALT-ONLY"));
}
