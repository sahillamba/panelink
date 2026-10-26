//! Explicit multi-session server startup through the managed launch path.
//!
//! The first child is launched by [`MultiSessionOwner::execute_launch`] with a
//! schema-6 request built from the explicit server-start launch context (the
//! invoking client's captured working directory and environment), prepared by
//! [`crate::managed_launch_prepare`] and journaled intent-before-spawn. The
//! storage lock is moved into the returned driver and held for the owner's
//! lifetime. No saved journal row is ever re-executed.

use crate::launch::{Environment, LaunchContext};
use crate::managed_launch_prepare::prepare_launch;
use crate::managed_launch_protocol::{
    encode_launch_request, EnvironmentEntry, LaunchRequest, LaunchResult,
};
use crate::multi_runtime::MultiServerDriver;
use crate::protocol::{Ledger, OperationKey, Outcome};
use crate::server::multi_session::MultiSessionOwner;
use crate::server_pty::PtyEffects;
use crate::server_storage::ServerStorage;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// Fixed, redacted startup failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MultiStartError {
    /// The explicit launch context could not be represented as a schema-6
    /// request (size, reserved `PANELINK_` names, path shape, dimensions).
    InvalidLaunchContext,
    /// The ledger or owner could not be assembled.
    OwnerUnavailable,
    /// The launch journal could not be opened.
    JournalUnavailable,
    /// The first launch was rejected or failed; no child is running.
    LaunchFailed,
}

impl fmt::Display for MultiStartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidLaunchContext => "startup launch context is invalid",
            Self::OwnerUnavailable => "multi-session owner could not be assembled",
            Self::JournalUnavailable => "launch journal is unavailable",
            Self::LaunchFailed => "first managed launch failed",
        })
    }
}

impl std::error::Error for MultiStartError {}

impl From<MultiStartError> for io::Error {
    fn from(value: MultiStartError) -> Self {
        io::Error::other(value)
    }
}

/// The explicit server-start launch: one executable, its arguments, and the
/// invoking client's captured context. Environment values stay volatile.
pub struct StartupLaunch<'a> {
    /// Absolute path or bare executable name (resolved against `PATH` in the
    /// effective environment).
    pub executable: &'a OsStr,
    /// Child arguments after the executable.
    pub arguments: &'a [OsString],
    /// Captured client working directory and environment.
    pub context: &'a LaunchContext,
    /// Initial root rows.
    pub rows: u16,
    /// Initial root columns.
    pub columns: u16,
}

impl fmt::Debug for StartupLaunch<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StartupLaunch")
            .field("argument_count", &self.arguments.len())
            .field("rows", &self.rows)
            .field("columns", &self.columns)
            .finish_non_exhaustive()
    }
}

/// A started multi-session server and its first launch result.
pub struct StartedMultiServer {
    driver: MultiServerDriver,
    startup: LaunchResult,
}

impl fmt::Debug for StartedMultiServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StartedMultiServer")
            .field("startup_outcome", &self.startup.outcome)
            .finish_non_exhaustive()
    }
}

impl StartedMultiServer {
    /// Launch the first child and assemble the driver.
    ///
    /// The first operation uses an internal lease opened before any client
    /// can connect; client leases are issued later from the same ledger and
    /// cannot resume the internal lease (it has no grant). A `Failed` first
    /// launch returns an error after the journal recorded it; an `Unknown`
    /// result keeps the retained child and is reported, never respawned.
    pub fn start(
        storage: ServerStorage,
        terminfo: &Path,
        launch: &StartupLaunch<'_>,
        allow_input: bool,
    ) -> Result<Self, MultiStartError> {
        Self::start_inner(storage, terminfo, launch, allow_input, None)
    }

    /// Start with exactly the bridge-owned config and socket assignments.
    /// The bridge values are applied only inside the preparation closure,
    /// after `execute_launch` has validated the bounded schema-6 request.
    pub fn start_with_provider_bridge(
        storage: ServerStorage,
        terminfo: &Path,
        launch: &StartupLaunch<'_>,
        allow_input: bool,
        bridge_environment: &Environment,
    ) -> Result<Self, MultiStartError> {
        Self::start_inner(
            storage,
            terminfo,
            launch,
            allow_input,
            Some(bridge_environment),
        )
    }

    fn start_inner(
        mut storage: ServerStorage,
        terminfo: &Path,
        launch: &StartupLaunch<'_>,
        allow_input: bool,
        bridge_environment: Option<&Environment>,
    ) -> Result<Self, MultiStartError> {
        let generation = storage.generation();
        let mut ledger = Ledger::new(generation).map_err(|_| MultiStartError::OwnerUnavailable)?;
        let lease = ledger
            .issue_lease(0)
            .map_err(|_| MultiStartError::OwnerUnavailable)?;
        let key = OperationKey {
            generation,
            lease,
            sequence: 1,
        };
        let payload = startup_request(key, launch)?;
        let content = crate::pane_layout::content_rect(crate::pane_layout::Rect {
            x: 0,
            y: 0,
            rows: launch.rows,
            columns: launch.columns,
        })
        .ok_or(MultiStartError::LaunchFailed)?;
        let content_size = crate::terminal::TerminalSize::new(content.rows, content.columns)
            .map_err(|_| MultiStartError::LaunchFailed)?;
        let mut owner = MultiSessionOwner::<PtyEffects>::with_ledger(ledger, allow_input)
            .map_err(|_| MultiStartError::OwnerUnavailable)?;
        let terminfo = terminfo.to_path_buf();
        let result = {
            let mut journal = storage
                .journal()
                .map_err(|_| MultiStartError::JournalUnavailable)?;
            owner.execute_launch(
                &payload,
                0,
                &mut journal,
                |request, target| {
                    let prepared = prepare_launch(request, target, &terminfo).map_err(|_| ())?;
                    let prepared = match bridge_environment {
                        Some(environment) => prepared
                            .with_provider_bridge_env(environment)
                            .map_err(|_| ())?,
                        None => prepared,
                    };
                    Ok::<_, ()>(prepared.with_initial_size(content_size))
                },
                |prepared| prepared.spawn().map(PtyEffects::new).map_err(|_| ()),
            )
        };
        // The request carried environment values; release them now.
        drop(payload);
        let result = result.map_err(|_| MultiStartError::LaunchFailed)?;
        if result.outcome == Outcome::Failed || result.target.is_none() {
            return Err(MultiStartError::LaunchFailed);
        }
        // The launch request sizes the outer root rectangle. The attached UI
        // leaves a one-cell border around the child PTY. The child already
        // starts at that content size, so this records its confirmed size
        // without pumping early output through a shrinking screen.
        let _ = owner.resize_layout(1, launch.rows, launch.columns, |effects, rows, columns| {
            effects.resize(rows, columns)
        });
        Ok(Self {
            driver: MultiServerDriver::new(owner, storage, PathBuf::from(&terminfo)),
            startup: result,
        })
    }

    /// First launch result (`Applied` or `Unknown`, with its target).
    pub const fn startup(&self) -> LaunchResult {
        self.startup
    }

    /// Borrow the driver.
    pub fn driver_mut(&mut self) -> &mut MultiServerDriver {
        &mut self.driver
    }

    /// Consume into the driver, which keeps the storage lock.
    pub fn into_driver(self) -> MultiServerDriver {
        self.driver
    }
}

/// Encode the explicit startup context as one bounded schema-6 request.
fn startup_request(
    key: OperationKey,
    launch: &StartupLaunch<'_>,
) -> Result<Vec<u8>, MultiStartError> {
    let cwd = launch.context.working_directory.as_os_str().as_bytes();
    let arguments: Vec<&[u8]> = launch
        .arguments
        .iter()
        .map(|argument| argument.as_bytes())
        .collect();
    let environment = environment_entries(&launch.context.environment);
    let request = LaunchRequest {
        key,
        rows: launch.rows,
        columns: launch.columns,
        requested_executable: launch.executable.as_bytes(),
        requested_working_directory: cwd,
        client_working_directory: cwd,
        arguments: &arguments,
        environment: &environment,
    };
    let length = request
        .encoded_len()
        .map_err(|_| MultiStartError::InvalidLaunchContext)?;
    let mut bytes = Vec::with_capacity(length);
    encode_launch_request(&request, &mut bytes)
        .map_err(|_| MultiStartError::InvalidLaunchContext)?;
    Ok(bytes)
}

/// Borrow captured environment entries in their stable map order.
pub fn environment_entries(environment: &Environment) -> Vec<EnvironmentEntry<'_>> {
    environment
        .iter()
        .map(|(name, value)| EnvironmentEntry {
            name: name.as_bytes(),
            value: value.as_bytes(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = PathBuf::from(format!(
                "/private/tmp/pl-multi-start-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Reopen after release. A sibling test's concurrent fork can briefly
    /// share the flock'd descriptor until its exec closes it (close-on-exec),
    /// so allow a bounded wait rather than asserting the very first attempt.
    fn reopen_within_bound(path: &Path) -> io::Result<ServerStorage> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            match ServerStorage::open(path) {
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                result => return result,
            }
        }
    }

    fn context(cwd: &Path) -> LaunchContext {
        LaunchContext {
            working_directory: cwd.to_path_buf(),
            environment: Environment::from([
                ("PATH".into(), "/usr/bin:/bin".into()),
                (
                    "STARTUP_SECRET".into(),
                    "DO_NOT_PERSIST_MULTI_CANARY".into(),
                ),
            ]),
        }
    }

    #[test]
    fn first_launch_establishes_layout_journal_and_holds_the_lock() {
        let storage_dir = Fixture::new();
        let cwd = Fixture::new();
        let storage = ServerStorage::initialize_for_test(&storage_dir.0).unwrap();
        let context = context(&cwd.0);
        let arguments = [OsString::from("-c"), OsString::from("read line")];
        let launch = StartupLaunch {
            executable: OsStr::new("/bin/sh"),
            arguments: &arguments,
            context: &context,
            rows: 24,
            columns: 80,
        };
        let started = StartedMultiServer::start(storage, &cwd.0, &launch, true).unwrap();
        assert_eq!(started.startup().outcome, Outcome::Applied);
        let layout = started.driver.owner().layout_snapshot().unwrap();
        assert_eq!((layout.revision, layout.rows, layout.columns), (1, 24, 80));
        assert_eq!(
            layout.panes[0].unwrap().target,
            started.startup().target.unwrap()
        );
        assert_eq!(
            ServerStorage::open(&storage_dir.0).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        let journal = fs::read(storage_dir.0.join("launch.journal")).unwrap();
        assert_eq!(journal.len(), 3 * 56, "Intent, Spawned, Applied");
        assert!(!journal.windows(9).any(|part| part == b"DO_NOT_PE"));
        assert!(!journal.windows(9).any(|part| part == b"read line"));
        assert!(!format!("{started:?}").contains("DO_NOT"));
        drop(started);
        assert_eq!(reopen_within_bound(&storage_dir.0).unwrap().generation(), 2);
    }

    #[test]
    fn opt_in_bridge_environment_reaches_only_the_started_child() {
        let storage_dir = Fixture::new();
        let cwd = Fixture::new();
        let output = cwd.0.join("bridge-env.txt");
        let script = format!(
            "printf '%s\\n%s\\n' \"$OPENCODE_CONFIG_DIR\" \"$PANELINK_BRIDGE_SOCKET\" > '{}'",
            output.display()
        );
        let arguments = [OsString::from("-c"), OsString::from(script)];
        let context = context(&cwd.0);
        let launch = StartupLaunch {
            executable: OsStr::new("/bin/sh"),
            arguments: &arguments,
            context: &context,
            rows: 24,
            columns: 80,
        };
        let bridge_environment = Environment::from([
            (
                "OPENCODE_CONFIG_DIR".into(),
                cwd.0.join("private-config").into_os_string(),
            ),
            (
                "PANELINK_BRIDGE_SOCKET".into(),
                cwd.0.join("bridge.sock").into_os_string(),
            ),
        ]);
        let parent_socket = std::env::var_os("PANELINK_BRIDGE_SOCKET");
        let parent_config = std::env::var_os("OPENCODE_CONFIG_DIR");
        let storage = ServerStorage::initialize_for_test(&storage_dir.0).unwrap();
        let started = StartedMultiServer::start_with_provider_bridge(
            storage,
            &cwd.0,
            &launch,
            true,
            &bridge_environment,
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !output.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            fs::read_to_string(&output).unwrap(),
            format!(
                "{}\n{}\n",
                cwd.0.join("private-config").display(),
                cwd.0.join("bridge.sock").display()
            )
        );
        assert_eq!(std::env::var_os("PANELINK_BRIDGE_SOCKET"), parent_socket);
        assert_eq!(std::env::var_os("OPENCODE_CONFIG_DIR"), parent_config);
        let journal = fs::read(storage_dir.0.join("launch.journal")).unwrap();
        assert!(!journal
            .windows(b"private-config".len())
            .any(|part| part == b"private-config"));
        drop(started);
    }

    #[test]
    fn invalid_context_or_failed_launch_spawns_nothing_and_releases_the_lock() {
        let storage_dir = Fixture::new();
        let cwd = Fixture::new();
        let mut reserved = context(&cwd.0);
        reserved
            .environment
            .insert("PANELINK_SESSION".into(), "9".into());
        let launch = StartupLaunch {
            executable: OsStr::new("/bin/sh"),
            arguments: &[],
            context: &reserved,
            rows: 24,
            columns: 80,
        };
        let storage = ServerStorage::initialize_for_test(&storage_dir.0).unwrap();
        assert_eq!(
            StartedMultiServer::start(storage, &cwd.0, &launch, true).unwrap_err(),
            MultiStartError::InvalidLaunchContext
        );
        let context = context(&cwd.0);
        let missing = StartupLaunch {
            executable: OsStr::new("/nonexistent/panelink-missing"),
            arguments: &[],
            context: &context,
            rows: 24,
            columns: 80,
        };
        let storage = reopen_within_bound(&storage_dir.0).unwrap();
        assert_eq!(
            StartedMultiServer::start(storage, &cwd.0, &missing, true).unwrap_err(),
            MultiStartError::LaunchFailed
        );
        // Preparation failed before Intent: nothing journaled, lock released.
        assert!(fs::read(storage_dir.0.join("launch.journal"))
            .unwrap()
            .is_empty());
        assert!(reopen_within_bound(&storage_dir.0).is_ok());
    }
}
