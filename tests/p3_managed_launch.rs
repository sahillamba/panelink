#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::control_protocol::Command;
use panelink::managed_launch::{LaunchJournalPort, ManagedLaunchCoordinator, ManagedLaunchError};
use panelink::managed_launch_prepare::{prepare_launch, PreparedLaunch};
use panelink::managed_launch_protocol::{encode_launch_request, LaunchRequest, LaunchResult};
use panelink::protocol::{Ledger, OperationKey, OperationStatus, Outcome};
use panelink::server::{EffectSink, ReadResult};
use panelink::server_pty::PtyEffects;
use panelink::server_storage::{LaunchJournal, LaunchRecord, LaunchState, ServerStorage};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const LEASE: u64 = 83;
const NOW: u64 = 10;
const CHILD_SCRIPT: &[u8] =
    b"printf 'P3ML-READY-%s-PID-%s-END\\n' \"$1\" \"$$\"; exec /bin/sleep 60";
const ACK_SCRIPT: &[u8] = b"printf 'P3ML-READY-%s-PID-%s-END\\n' \"$1\" \"$$\"; IFS= read -r token; printf 'P3ML-ACK-%s-PID-%s-END\\n' \"$token\" \"$$\"; exec /bin/sleep 60";
const CHILD_DEADLINE: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(2);
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct PrivateFixture(PathBuf);

impl PrivateFixture {
    fn new() -> io::Result<Self> {
        for _ in 0..100 {
            let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let path = Path::new("/private/tmp").join(format!(
                "panelink-p3-managed-launch-{}-{serial}",
                std::process::id()
            ));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::from(io::ErrorKind::AlreadyExists))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PrivateFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn request_payload(
    generation: u64,
    sequence: u64,
    cwd: &Path,
    marker: &str,
    script: &[u8],
) -> Vec<u8> {
    let key = OperationKey {
        generation,
        lease: LEASE,
        sequence,
    };
    let cwd = cwd.as_os_str().as_bytes();
    let arguments: [&[u8]; 4] = [
        b"-c",
        script,
        b"panelink-managed-launch-test",
        marker.as_bytes(),
    ];
    let environment = [
        panelink::managed_launch_protocol::EnvironmentEntry {
            name: b"TERM",
            value: b"panelink-probe",
        },
        panelink::managed_launch_protocol::EnvironmentEntry {
            name: b"PATH",
            value: b"/usr/bin:/bin",
        },
    ];
    let request = LaunchRequest {
        key,
        rows: 10,
        columns: 100,
        requested_executable: b"/bin/sh",
        requested_working_directory: cwd,
        client_working_directory: cwd,
        arguments: &arguments,
        environment: &environment,
    };
    let mut payload = Vec::new();
    encode_launch_request(&request, &mut payload).expect("encode exact launch fixture");
    payload
}

fn prepare(
    request: panelink::managed_launch_protocol::LaunchRequestView<'_>,
    target: panelink::control_protocol::TargetRef,
    terminfo_directory: &Path,
) -> Result<PreparedLaunch, ()> {
    prepare_launch(request, target, terminfo_directory).map_err(|_| ())
}

fn spawn(prepared: PreparedLaunch) -> Result<PtyEffects, ()> {
    prepared.spawn().map(PtyEffects::new).map_err(|_| ())
}

fn screen(effects: &mut dyn EffectSink) -> ReadResult {
    effects.poll().expect("bounded child poll remains healthy");
    effects
        .read(panelink::control_protocol::ReadSource::Screen, 3_000, 10)
        .expect("read the retained bounded terminal screen")
}

fn wait_for_marker<E: EffectSink>(
    owner: &mut ManagedLaunchCoordinator<E>,
    target: panelink::control_protocol::TargetRef,
    marker: &str,
) -> (String, i32) {
    let deadline = Instant::now() + CHILD_DEADLINE;
    loop {
        let read = screen(
            owner
                .effects_mut(target)
                .expect("target owns child effects"),
        );
        if let Some(pid) = extract_pid(&read.text, marker) {
            return (read.text, pid);
        }
        assert!(
            Instant::now() < deadline,
            "child marker {marker:?} did not arrive before deadline; screen was {:?}",
            read.text
        );
        thread::sleep(POLL_INTERVAL);
    }
}

fn extract_pid(text: &str, marker: &str) -> Option<i32> {
    for (index, _) in text.match_indices(marker) {
        let suffix = &text[index + marker.len()..];
        let Some(pid) = suffix.strip_prefix("-PID-") else {
            continue;
        };
        let Some((digits, _)) = pid.split_once("-END") else {
            continue;
        };
        if !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit()) {
            return digits.parse().ok();
        }
    }
    None
}

fn count_marker(text: &str, marker: &str) -> usize {
    text.match_indices(marker).count()
}

struct FailingJournal<'a> {
    inner: LaunchJournal<'a>,
    fail_state: LaunchState,
}

impl LaunchJournalPort for FailingJournal<'_> {
    fn preflight_launch(&self, key: OperationKey, session: u64) -> io::Result<()> {
        self.inner.preflight_launch(key, session)
    }

    fn record(&mut self, record: LaunchRecord) -> io::Result<()> {
        if record.state == self.fail_state {
            Err(io::Error::other("injected post-spawn journal fault"))
        } else {
            self.inner.record(record)
        }
    }
}

fn assert_process_reaped(pid: i32) {
    // SAFETY: signal zero only checks whether this PID exists and is visible;
    // it does not deliver a signal or change child state.
    let result = unsafe { libc::kill(pid, 0) };
    assert_eq!(result, -1, "child PID should be gone after owner cleanup");
    assert_eq!(
        io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH),
        "signal-zero must report ESRCH, not another process or permission error"
    );
}

#[test]
fn private_pty_launch_deduplicates_exact_identity_and_retains_two_children() {
    let fixture = PrivateFixture::new().expect("create private PTY/journal fixture");
    let mut storage = ServerStorage::initialize(fixture.path()).expect("initialize real journal");
    let generation = storage.generation();
    let mut ledger = Ledger::new(generation).unwrap();
    ledger.open_lease(LEASE, NOW).unwrap();
    let mut owner = ManagedLaunchCoordinator::<PtyEffects>::new(generation).unwrap();

    let first_marker = "first-agent";
    let first_payload = request_payload(generation, 1, fixture.path(), first_marker, CHILD_SCRIPT);
    let first = {
        let mut journal = storage.journal().unwrap();
        owner
            .execute(
                &first_payload,
                NOW,
                &mut ledger,
                &mut journal,
                |request, target| prepare(request, target, fixture.path()),
                spawn,
            )
            .expect("launch first real private-PTY child")
    };
    assert_eq!(first.outcome, Outcome::Applied);
    let first_target = first.target.unwrap();
    let (first_screen, first_pid) = wait_for_marker(&mut owner, first_target, first_marker);
    assert_eq!(count_marker(&first_screen, first_marker), 1);
    assert!(!owner.effects(first_target).unwrap().child_exited());

    // Treat the original result as lost, then retry the exact same bytes.
    let repeated = {
        let mut journal = storage.journal().unwrap();
        owner
            .execute(
                &first_payload,
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| -> Result<PreparedLaunch, ()> { panic!("duplicate must not prepare") },
                |_| -> Result<PtyEffects, ()> { panic!("duplicate must not spawn") },
            )
            .unwrap()
    };
    assert_eq!(repeated, first);
    let (repeated_screen, repeated_pid) = wait_for_marker(&mut owner, first_target, first_marker);
    assert_eq!(repeated_pid, first_pid);
    assert_eq!(count_marker(&repeated_screen, first_marker), 1);

    let changed = request_payload(
        generation,
        1,
        fixture.path(),
        first_marker,
        b"printf changed",
    );
    let changed_error = {
        let mut journal = storage.journal().unwrap();
        owner.execute(
            &changed,
            NOW,
            &mut ledger,
            &mut journal,
            |_, _| -> Result<PreparedLaunch, ()> { panic!("changed key must conflict first") },
            |_| -> Result<PtyEffects, ()> { panic!("changed key must not spawn") },
        )
    }
    .unwrap_err();
    assert_eq!(
        changed_error.ledger_error(),
        Some(&panelink::protocol::ProtocolError::OperationConflict)
    );

    let second_marker = "second-agent";
    let second_payload =
        request_payload(generation, 2, fixture.path(), second_marker, CHILD_SCRIPT);
    let second = {
        let mut journal = storage.journal().unwrap();
        owner
            .execute(
                &second_payload,
                NOW,
                &mut ledger,
                &mut journal,
                |request, target| prepare(request, target, fixture.path()),
                spawn,
            )
            .expect("launch second real private-PTY child")
    };
    assert_eq!(second.outcome, Outcome::Applied);
    let second_target = second.target.unwrap();
    let (_second_screen, second_pid) = wait_for_marker(&mut owner, second_target, second_marker);
    assert_ne!(first_target, second_target);
    assert_ne!(first_pid, second_pid);
    assert!(!owner.effects(first_target).unwrap().child_exited());
    assert!(!owner.effects(second_target).unwrap().child_exited());
    assert_eq!(owner.records().count(), 2);

    // Fill the remaining capacity (SESSION_CAPACITY = MAX_PANES) with real
    // children; the next new identity must then be refused before prepare.
    let mut extra_pids = Vec::new();
    let capacity = panelink::session_registry::SESSION_CAPACITY as u64;
    for sequence in 3..=capacity {
        let marker = format!("extra-agent-{sequence}");
        let payload = request_payload(generation, sequence, fixture.path(), &marker, CHILD_SCRIPT);
        let result = {
            let mut journal = storage.journal().unwrap();
            owner
                .execute(
                    &payload,
                    NOW,
                    &mut ledger,
                    &mut journal,
                    |request, target| prepare(request, target, fixture.path()),
                    spawn,
                )
                .expect("launch real private-PTY child within capacity")
        };
        assert_eq!(result.outcome, Outcome::Applied);
        let (_, pid) = wait_for_marker(&mut owner, result.target.unwrap(), &marker);
        extra_pids.push(pid);
    }
    assert_eq!(owner.records().count(), capacity as usize);

    let refused_sequence = capacity + 1;
    let third = request_payload(
        generation,
        refused_sequence,
        fixture.path(),
        "over-capacity-agent",
        CHILD_SCRIPT,
    );
    let third_result = {
        let mut journal = storage.journal().unwrap();
        owner.execute(
            &third,
            NOW,
            &mut ledger,
            &mut journal,
            |_, _| -> Result<PreparedLaunch, ()> {
                panic!("third identity must fail before prepare")
            },
            |_| -> Result<PtyEffects, ()> { panic!("third identity must not spawn") },
        )
    };
    assert!(matches!(third_result, Err(ManagedLaunchError::Registry(_))));
    assert_eq!(
        ledger.status(
            OperationKey {
                generation,
                lease: LEASE,
                sequence: refused_sequence
            },
            NOW
        ),
        Ok(OperationStatus::NotAdmitted)
    );
    assert!(owner.effects(first_target).is_ok());
    assert!(owner.effects(second_target).is_ok());
    // Dropping the owner invokes bounded TerminalSession cleanup for both live
    // children. The storage lock outlives the coordinator in this scope.
    drop(owner);
    assert_process_reaped(first_pid);
    assert_process_reaped(second_pid);
    for pid in extra_pids {
        assert_process_reaped(pid);
    }
}

#[test]
fn real_children_remain_usable_after_spawned_and_applied_journal_faults() {
    for (index, fail_state) in [LaunchState::Spawned, LaunchState::Applied]
        .into_iter()
        .enumerate()
    {
        let fixture = PrivateFixture::new().expect("create private PTY/journal fixture");
        let mut storage =
            ServerStorage::initialize(fixture.path()).expect("initialize real journal");
        let generation = storage.generation();
        let mut ledger = Ledger::new(generation).unwrap();
        ledger.open_lease(LEASE, NOW).unwrap();
        let mut owner = ManagedLaunchCoordinator::<PtyEffects>::new(generation).unwrap();
        let sequence = index as u64 + 1;
        let identity = format!("post-spawn-{sequence}");
        let payload = request_payload(generation, sequence, fixture.path(), &identity, ACK_SCRIPT);
        let result: LaunchResult = {
            let journal = storage.journal().unwrap();
            let mut failing = FailingJournal {
                inner: journal,
                fail_state,
            };
            owner
                .execute(
                    &payload,
                    NOW,
                    &mut ledger,
                    &mut failing,
                    |request, target| prepare(request, target, fixture.path()),
                    spawn,
                )
                .expect("post-spawn uncertainty is a launch result")
        };
        assert_eq!(result.outcome, Outcome::Unknown);
        let target = result.target.unwrap();
        assert_eq!(
            owner.state(target),
            Ok(panelink::session_registry::SessionState::Unknown)
        );
        let ready_marker = format!("P3ML-READY-{identity}");
        let (ready_screen, pid) = wait_for_marker(&mut owner, target, &ready_marker);
        assert_eq!(count_marker(&ready_screen, &ready_marker), 1);

        // The shell is waiting on its PTY input. Submit a fresh unique token
        // after the journal fault and require the still-owned child to echo it.
        let token = format!("P3ML-UNIQUE-ACK-{sequence}");
        assert_eq!(
            owner.effects_mut(target).unwrap().input(Command::Submit {
                owner_revision: 1,
                text: &token,
            }),
            Outcome::Applied
        );
        let ack_marker = format!("P3ML-ACK-{token}");
        let (ack_screen, ack_pid) = wait_for_marker(&mut owner, target, &ack_marker);
        assert_eq!(ack_pid, pid);
        assert_eq!(count_marker(&ack_screen, &ack_marker), 1);
        assert!(ack_screen.contains(&format!("P3ML-ACK-{token}-PID-{pid}-END")));
        assert!(!owner.effects(target).unwrap().child_exited());
        assert_eq!(
            ledger.status(result.key, NOW),
            Ok(OperationStatus::Completed(Outcome::Unknown))
        );

        // Storage write failure latches this coordinator unhealthy. Existing
        // exact results remain observable, but a new key cannot run callbacks.
        assert!(!owner.is_healthy());
        let duplicate = {
            let mut journal = storage.journal().unwrap();
            owner
                .execute(
                    &payload,
                    NOW,
                    &mut ledger,
                    &mut journal,
                    |_, _| -> Result<PreparedLaunch, ()> {
                        panic!("exact Unknown remains observable")
                    },
                    |_| -> Result<PtyEffects, ()> { panic!("exact Unknown must not respawn") },
                )
                .unwrap()
        };
        assert_eq!(duplicate, result);
        let new_sequence = sequence + 1;
        let new_payload = request_payload(
            generation,
            new_sequence,
            fixture.path(),
            "new-child",
            CHILD_SCRIPT,
        );
        let new_result = {
            let mut journal = storage.journal().unwrap();
            owner.execute(
                &new_payload,
                NOW,
                &mut ledger,
                &mut journal,
                |_, _| -> Result<PreparedLaunch, ()> {
                    panic!("health latch rejects before prepare")
                },
                |_| -> Result<PtyEffects, ()> { panic!("health latch rejects before spawn") },
            )
        };
        assert!(matches!(new_result, Err(ManagedLaunchError::Unhealthy)));
        assert_eq!(
            ledger.status(
                OperationKey {
                    generation,
                    lease: LEASE,
                    sequence: new_sequence
                },
                NOW
            ),
            Ok(OperationStatus::NotAdmitted)
        );

        // The on-disk journal intentionally ends at Intent for the first case
        // and Spawned for the second. Both recover as Unknown.
        {
            let journal = storage.journal().unwrap();
            assert_eq!(
                journal.recovered(result.key).unwrap().state,
                LaunchState::Unknown
            );
        }
        drop(owner);
        assert_process_reaped(pid);
    }
}
