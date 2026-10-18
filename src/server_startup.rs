//! Explicit one-child prototype startup with an intent-before-spawn journal.
//!
//! This does not expose API launch/relaunch or automatically execute any saved
//! journal row. Every startup requires a fresh explicit launch context.

use crate::control_protocol::TargetRef;
use crate::launch::{Environment, ResolvedLaunch};
use crate::protocol::{Admission, Ledger, OperationKey, Outcome, MAX_OPERATION_PAYLOAD_BYTES};
use crate::server::StateOwner;
use crate::server_pty::PtyEffects;
use crate::server_runtime::ServerDriver;
use crate::server_storage::{LaunchJournal, LaunchRecord, LaunchState, ServerStorage};
use crate::terminal::{TerminalSession, TerminalSize};
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt;

fn failed(label: &'static str) -> io::Error {
    io::Error::other(label)
}

/// Holds process state and the singleton generation lock for one foreground run.
pub struct StartedServer {
    // Driver drops first, stopping workers before reaping the child; only then
    // may storage release the singleton lock for the next generation.
    driver: ServerDriver<PtyEffects>,
    storage: ServerStorage,
    startup_outcome: Outcome,
}

impl StartedServer {
    /// Admit one explicit launch, flush intent, spawn, commit registry and result.
    ///
    /// A result-sync failure retains the owned child but exposes Unknown; it
    /// never retries spawn. A failed intent write prevents spawn entirely.
    pub fn start(
        storage: ServerStorage,
        launch: &ResolvedLaunch,
        arguments: &[OsString],
        environment: &Environment,
        allow_input: bool,
    ) -> io::Result<Self> {
        Self::start_with_writer(
            storage,
            launch,
            arguments,
            environment,
            allow_input,
            |journal, record| journal.record(record),
        )
    }

    fn start_with_writer(
        mut storage: ServerStorage,
        launch: &ResolvedLaunch,
        arguments: &[OsString],
        environment: &Environment,
        allow_input: bool,
        mut write_result: impl FnMut(&mut LaunchJournal<'_>, LaunchRecord) -> io::Result<()>,
    ) -> io::Result<Self> {
        let payload = startup_payload(launch, arguments, environment)?;
        let generation = storage.generation();
        let mut ledger = Ledger::new(generation).map_err(|_| failed("startup_ledger_failed"))?;
        let lease = ledger
            .issue_lease(0)
            .map_err(|_| failed("startup_lease_failed"))?;
        let key = OperationKey {
            generation,
            lease,
            sequence: 1,
        };
        if ledger
            .admit(key, &payload, 0)
            .map_err(|_| failed("startup_admission_failed"))?
            != Admission::Execute
        {
            return Err(failed("startup_not_executable"));
        }
        drop(payload);
        let record = LaunchRecord {
            key,
            session: 1,
            state: LaunchState::Intent,
        };
        let mut journal = storage.journal()?;
        journal.record(record)?;
        let size = TerminalSize::new(24, 80).map_err(|_| failed("startup_size_failed"))?;
        let session = match TerminalSession::spawn(launch, arguments, environment, size) {
            Ok(session) => session,
            Err(_) => {
                let _ = journal.record(LaunchRecord {
                    state: LaunchState::Failed,
                    ..record
                });
                let _ = ledger.complete(key, Outcome::Failed);
                return Err(failed("startup_spawn_failed"));
            }
        };
        let spawned_recorded = write_result(
            &mut journal,
            LaunchRecord {
                state: LaunchState::Spawned,
                ..record
            },
        )
        .is_ok();
        let target = TargetRef {
            generation,
            workspace: 1,
            pane: 1,
            session: 1,
            occupant_revision: 1,
        };
        let mut state =
            StateOwner::with_ledger(ledger, target, PtyEffects::new(session), allow_input)
                .map_err(|_| failed("startup_registry_failed"))?;
        let startup_outcome = if spawned_recorded
            && write_result(
                &mut journal,
                LaunchRecord {
                    state: LaunchState::Applied,
                    ..record
                },
            )
            .is_ok()
        {
            Outcome::Applied
        } else {
            Outcome::Unknown
        };
        state
            .complete_startup(key, startup_outcome)
            .map_err(|_| failed("startup_completion_failed"))?;
        drop(journal);
        Ok(Self {
            driver: ServerDriver::new(state),
            storage,
            startup_outcome,
        })
    }

    /// The startup generation already advanced/flushed before server use.
    pub const fn generation(&self) -> u64 {
        self.storage.generation()
    }

    /// Applied means startup registration committed, never application success.
    pub const fn startup_outcome(&self) -> Outcome {
        self.startup_outcome
    }

    /// Fail closed if runtime paths no longer identify the held singleton lock.
    pub fn verify_storage(&self) -> io::Result<()> {
        self.storage.verify_identity()
    }

    /// Access the driver while retaining the singleton lock beside it.
    pub fn driver_mut(&mut self) -> &mut ServerDriver<PtyEffects> {
        &mut self.driver
    }
}

fn startup_payload(
    launch: &ResolvedLaunch,
    arguments: &[OsString],
    environment: &Environment,
) -> io::Result<Vec<u8>> {
    if arguments.len() > 256 || environment.len() > 4096 {
        return Err(failed("startup_payload_too_large"));
    }
    let mut bytes = Vec::with_capacity(MAX_OPERATION_PAYLOAD_BYTES);
    bytes.extend_from_slice(b"PLBOOT01");
    for path in [
        &launch.requested_path,
        &launch.executable_path,
        &launch.working_directory,
    ] {
        append_blob(&mut bytes, path.as_os_str())?;
    }
    append_blob(&mut bytes, OsStr::new("argv"))?;
    if bytes.len() + 4 > MAX_OPERATION_PAYLOAD_BYTES {
        return Err(failed("startup_payload_too_large"));
    }
    bytes.extend_from_slice(&(arguments.len() as u32).to_be_bytes());
    for argument in arguments {
        append_blob(&mut bytes, argument)?;
    }
    append_blob(&mut bytes, OsStr::new("environment"))?;
    if bytes.len() + 4 > MAX_OPERATION_PAYLOAD_BYTES {
        return Err(failed("startup_payload_too_large"));
    }
    bytes.extend_from_slice(&(environment.len() as u32).to_be_bytes());
    for (name, value) in environment {
        append_blob(&mut bytes, name)?;
        append_blob(&mut bytes, value)?;
    }
    Ok(bytes)
}

fn append_blob(bytes: &mut Vec<u8>, value: &OsStr) -> io::Result<()> {
    let data = value.as_bytes();
    if data.len()
        > MAX_OPERATION_PAYLOAD_BYTES
            .saturating_sub(bytes.len())
            .saturating_sub(4)
        || bytes.len() + 4 > MAX_OPERATION_PAYLOAD_BYTES
    {
        return Err(failed("startup_payload_too_large"));
    }
    bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
    bytes.extend_from_slice(data);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::EffectSink;
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = PathBuf::from(format!(
                "/private/tmp/pl-start-{}-{stamp}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .unwrap();
            Self(directory)
        }
        fn launch(&self) -> ResolvedLaunch {
            ResolvedLaunch {
                requested_path: "/bin/sh".into(),
                executable_path: "/bin/sh".into(),
                working_directory: self.0.clone(),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn environment() -> Environment {
        Environment::from([
            ("TERM".into(), "panelink-probe".into()),
            (
                "STARTUP_SECRET".into(),
                "DO_NOT_PERSIST_STARTUP_ENV_CANARY".into(),
            ),
        ])
    }

    #[test]
    fn explicit_startup_commits_metadata_and_keeps_singleton_while_child_lives() {
        let fixture = Fixture::new();
        let storage = ServerStorage::initialize_for_test(&fixture.0).unwrap();
        let mut started = StartedServer::start(
            storage,
            &fixture.launch(),
            &["-c".into(), "printf 'BOOT-READY'; read line".into()],
            &environment(),
            true,
        )
        .unwrap();
        assert_eq!(started.startup_outcome(), Outcome::Applied);
        assert_eq!(started.generation(), 1);
        assert_eq!(
            ServerStorage::open(&fixture.0).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(!started.driver.state().effects().child_exited());
        let key = OperationKey {
            generation: 1,
            lease: 1,
            sequence: 1,
        };
        assert_eq!(
            started
                .storage
                .journal()
                .unwrap()
                .recovered(key)
                .unwrap()
                .state,
            LaunchState::Applied
        );
        let bytes = fs::read(fixture.0.join("launch.journal")).unwrap();
        assert_eq!(bytes.len(), 3 * 56);
        assert!(!bytes.windows(10).any(|part| part == b"DO_NOT_PER"));
        drop(started);
        assert_eq!(ServerStorage::open(&fixture.0).unwrap().generation(), 2);
    }

    #[test]
    fn result_commit_failure_keeps_child_but_reports_unknown_without_respawn() {
        for failed_state in [LaunchState::Spawned, LaunchState::Applied] {
            let fixture = Fixture::new();
            let storage = ServerStorage::initialize_for_test(&fixture.0).unwrap();
            let mut calls = 0;
            let mut started = StartedServer::start_with_writer(
                storage,
                &fixture.launch(),
                &["-c".into(), "printf 'BOOT-READY'; read line".into()],
                &environment(),
                true,
                |journal, record| {
                    calls += 1;
                    if record.state == failed_state {
                        Err(failed("injected_result_sync_failure"))
                    } else {
                        journal.record(record)
                    }
                },
            )
            .unwrap();
            assert_eq!(started.startup_outcome(), Outcome::Unknown);
            assert!(!started.driver.state().effects().child_exited());
            let key = OperationKey {
                generation: 1,
                lease: 1,
                sequence: 1,
            };
            assert_eq!(
                started
                    .storage
                    .journal()
                    .unwrap()
                    .recovered(key)
                    .unwrap()
                    .state,
                LaunchState::Unknown
            );
            assert_eq!(
                calls,
                if failed_state == LaunchState::Spawned {
                    1
                } else {
                    2
                }
            );
            assert_eq!(
                ServerStorage::open(&fixture.0).unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
        }
    }

    #[test]
    fn full_valid_journal_prevents_first_spawn() {
        let fixture = Fixture::new();
        drop(ServerStorage::initialize_for_test(&fixture.0).unwrap());
        // Repeated identical metadata records are valid but cannot grow beyond
        // the bounded append budget. New startup belongs to generation 2.
        let mut record = [0; 56];
        record[..8].copy_from_slice(b"PLJNL001");
        for offset in [8, 16, 24, 32] {
            record[offset..offset + 8].copy_from_slice(&1_u64.to_be_bytes());
        }
        record[40] = 1;
        record[48..56].copy_from_slice(&(!1_u64).to_be_bytes());
        let bytes = record.repeat(crate::server_storage::MAX_JOURNAL_BYTES as usize / 56);
        let path = fixture.0.join("launch.journal");
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let mut env = environment();
        let marker = fixture.0.join("spawned");
        env.insert("SPAWN_MARKER".into(), marker.as_os_str().to_owned());
        let storage = ServerStorage::open(&fixture.0).unwrap();
        let result = StartedServer::start(
            storage,
            &fixture.launch(),
            &["-c".into(), "printf spawned > \"$SPAWN_MARKER\"".into()],
            &env,
            true,
        );
        assert!(result.is_err());
        assert!(!marker.exists(), "no spawn before successful intent append");
    }

    #[test]
    fn startup_payload_checks_complete_metadata_budget_and_exact_argument_bytes() {
        let fixture = Fixture::new();
        let launch = fixture.launch();
        let a = startup_payload(&launch, &["x".into(), "yz".into()], &environment()).unwrap();
        let b = startup_payload(&launch, &["xy".into(), "z".into()], &environment()).unwrap();
        assert_ne!(a, b);
        assert!(a.len() <= MAX_OPERATION_PAYLOAD_BYTES);
        let mut oversized = environment();
        oversized.insert(
            "LARGE".into(),
            "x".repeat(MAX_OPERATION_PAYLOAD_BYTES).into(),
        );
        assert!(startup_payload(&launch, &[], &oversized).is_err());
    }
}
