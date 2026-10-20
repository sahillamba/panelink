//! Zero-configuration `panelink`: open (or reattach to) the user's workspace.
//!
//! Running `panelink` with no arguments resolves one private per-user state
//! root, compiles the bundled terminfo entry once, starts the background
//! multi-session server if none is running, and attaches the interactive
//! client. Ctrl-X detaches; the shells keep running in the server, and the next
//! `panelink` reattaches. `panelink stop` ends the workspace.
//!
//! Lifecycle rules:
//! - Every opener and `stop` first takes an exclusive `flock` on
//!   `lifecycle.lock` in the state root. Liveness checks, stale cleanup,
//!   storage initialization and server spawn all happen under it, so two
//!   invocations can never both initialize, both spawn, or race a removal.
//! - A running server holds `server/server.lock` for its whole lifetime.
//!   A control socket is removed only while holding the lifecycle lock, when
//!   the server lock is free, and the entry is a socket owned by this user.
//! - The spawned server's identity (pid plus kernel process start time) is
//!   recorded in `server.pid`. `stop` signals only a process whose pid,
//!   start time and owner all still match, so pid reuse cannot redirect it.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use panelink::transport::AuthenticatedStream;

/// Terminfo source embedded at build time so the binary is self-contained.
const TERMINFO_SOURCE: &str = include_str!("../docs/terminfo/panelink-probe.terminfo");
const SOCKET_NAME: &str = "control.sock";
const LOG_NAME: &str = "server.log";
const PID_NAME: &str = "server.pid";
const MAX_LOG_BYTES: u64 = 1024 * 1024;
const START_TIMEOUT: Duration = Duration::from_secs(8);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_STEP: Duration = Duration::from_millis(25);
/// macOS `sun_path` capacity, including the terminating NUL.
const MAX_SOCKET_PATH_BYTES: usize = 103;

/// Fixed user-facing failure labels; never contain paths or environment data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OpenError {
    HomeUnavailable,
    StateRootUnsafe,
    StateRootUnavailable,
    SocketPathTooLong,
    TerminfoCompileFailed,
    ServerStartFailed,
    ServerStartTimedOut,
    StaleEndpointUnsafe,
    StopFailed,
}

impl OpenError {
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::HomeUnavailable => "cannot find your home directory (set PANELINK_HOME)",
            Self::StateRootUnsafe => {
                "panelink state directory must be owned by you and not group/world writable"
            }
            Self::StateRootUnavailable => "cannot create the panelink state directory",
            Self::SocketPathTooLong => "panelink state path is too long for a socket",
            Self::TerminfoCompileFailed => "could not compile the panelink terminfo entry",
            Self::ServerStartFailed => "could not start the panelink server (see server.log)",
            Self::ServerStartTimedOut => "panelink server did not become ready (see server.log)",
            Self::StaleEndpointUnsafe => "unexpected file at the panelink socket path",
            Self::StopFailed => "could not stop the panelink server",
        }
    }
}

/// Resolved private per-user layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StateRoot {
    pub(crate) root: PathBuf,
    pub(crate) server: PathBuf,
    pub(crate) terminfo: PathBuf,
    pub(crate) contexts: PathBuf,
}

impl StateRoot {
    fn from_root(root: PathBuf) -> Self {
        Self {
            server: root.join("server"),
            terminfo: root.join("terminfo"),
            contexts: root.join("contexts"),
            root,
        }
    }

    fn socket_path(&self) -> PathBuf {
        self.server.join(SOCKET_NAME)
    }
}

/// Select the state root: `$PANELINK_HOME` (absolute) or
/// `~/Library/Application Support/Panelink`.
pub(crate) fn resolve_root(
    panelink_home: Option<OsString>,
    home: Option<OsString>,
) -> Result<StateRoot, OpenError> {
    let root = match panelink_home.filter(|value| !value.is_empty()) {
        Some(value) => PathBuf::from(value),
        None => {
            let home = home
                .filter(|value| !value.is_empty())
                .ok_or(OpenError::HomeUnavailable)?;
            PathBuf::from(home).join("Library/Application Support/Panelink")
        }
    };
    if !root.is_absolute() {
        return Err(OpenError::StateRootUnavailable);
    }
    let state = StateRoot::from_root(root);
    let socket_len = state.socket_path().as_os_str().len();
    if socket_len > MAX_SOCKET_PATH_BYTES {
        return Err(OpenError::SocketPathTooLong);
    }
    Ok(state)
}

/// Create (0700) or validate one private directory owned by this user.
fn ensure_private_dir(path: &Path) -> Result<(), OpenError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)
                .map_err(|_| OpenError::StateRootUnavailable)?;
        }
        Err(_) => return Err(OpenError::StateRootUnavailable),
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| OpenError::StateRootUnavailable)?;
    private_dir_ok(&metadata, current_uid())
}

fn private_dir_ok(metadata: &fs::Metadata, uid: u32) -> Result<(), OpenError> {
    if !metadata.file_type().is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(OpenError::StateRootUnsafe);
    }
    Ok(())
}

/// Every existing ancestor must be a real directory owned by root or this
/// user and not writable by others (sticky root-owned dirs such as /tmp are
/// accepted), so no other account can swap a path component.
fn ancestors_ok(path: &Path, uid: u32) -> Result<(), OpenError> {
    let mut current = PathBuf::from("/");
    let components: Vec<_> = path.components().skip(1).collect();
    for component in &components[..components.len().saturating_sub(1)] {
        current.push(component);
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(OpenError::StateRootUnsafe),
        };
        let file_type = metadata.file_type();
        // `/tmp` and `/var` are root-owned symlinks into /private on macOS.
        if file_type.is_symlink() {
            if metadata.uid() != 0 {
                return Err(OpenError::StateRootUnsafe);
            }
            continue;
        }
        let owner_ok = metadata.uid() == 0 || metadata.uid() == uid;
        let sticky_root = metadata.uid() == 0 && metadata.mode() & 0o1000 != 0;
        if !file_type.is_dir() || !owner_ok || (metadata.mode() & 0o022 != 0 && !sticky_root) {
            return Err(OpenError::StateRootUnsafe);
        }
    }
    Ok(())
}

fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

fn prepare_root(state: &StateRoot) -> Result<(), OpenError> {
    ancestors_ok(&state.root, current_uid())?;
    ensure_private_dir(&state.root)?;
    for directory in [&state.server, &state.terminfo, &state.contexts] {
        ensure_private_dir(directory)?;
    }
    Ok(())
}

/// FNV-1a over the embedded source; identifies the compiled terminfo build.
fn terminfo_fingerprint(source: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in source.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}\n")
}

fn terminfo_entry_present(directory: &Path) -> bool {
    ["p", "70"]
        .iter()
        .any(|prefix| directory.join(prefix).join("panelink-probe").is_file())
}

/// Whether the compiled entry must be (re)built for `source`.
pub(crate) fn terminfo_needs_compile(directory: &Path, source: &str) -> bool {
    if !terminfo_entry_present(directory) {
        return true;
    }
    fs::read_to_string(directory.join("source.fingerprint"))
        .map_or(true, |stored| stored != terminfo_fingerprint(source))
}

/// Compile into a private per-process staging directory, then publish the
/// compiled entry and fingerprint with atomic renames so concurrent first
/// opens never observe or produce a half-written entry.
fn ensure_terminfo(state: &StateRoot) -> Result<(), OpenError> {
    if !terminfo_needs_compile(&state.terminfo, TERMINFO_SOURCE) {
        return Ok(());
    }
    let staging = state
        .root
        .join(format!("terminfo.staging.{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    ensure_private_dir(&staging)?;
    let result = compile_and_publish(state, &staging);
    let _ = fs::remove_dir_all(&staging);
    result
}

fn compile_and_publish(state: &StateRoot, staging: &Path) -> Result<(), OpenError> {
    let source_path = staging.join("panelink-probe.src");
    write_private(&source_path, TERMINFO_SOURCE.as_bytes())?;
    let status = Command::new("/usr/bin/tic")
        .arg("-x")
        .arg("-o")
        .arg(staging)
        .arg(&source_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| OpenError::TerminfoCompileFailed)?;
    if !status.success() {
        return Err(OpenError::TerminfoCompileFailed);
    }
    let mut published = false;
    for prefix in ["p", "70"] {
        let compiled = staging.join(prefix).join("panelink-probe");
        if !compiled.is_file() {
            continue;
        }
        let target_dir = state.terminfo.join(prefix);
        ensure_private_dir(&target_dir)?;
        fs::rename(&compiled, target_dir.join("panelink-probe"))
            .map_err(|_| OpenError::TerminfoCompileFailed)?;
        published = true;
    }
    if !published {
        return Err(OpenError::TerminfoCompileFailed);
    }
    let fingerprint = staging.join("source.fingerprint");
    write_private(
        &fingerprint,
        terminfo_fingerprint(TERMINFO_SOURCE).as_bytes(),
    )?;
    fs::rename(&fingerprint, state.terminfo.join("source.fingerprint"))
        .map_err(|_| OpenError::TerminfoCompileFailed)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), OpenError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| OpenError::StateRootUnavailable)?;
    file.write_all(bytes)
        .map_err(|_| OpenError::StateRootUnavailable)
}

/// What exists at the socket path and whether a server holds the lock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EndpointState {
    /// No socket entry.
    Absent,
    /// A socket entry owned by this user.
    Socket,
    /// Anything else (regular file, symlink, other owner).
    Foreign,
}

/// Decision for one observed endpoint/lock combination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EndpointAction {
    /// A server is alive: try to attach (retrying while it finishes startup).
    UseRunning,
    /// No server and no socket: start one.
    Start,
    /// No server but a leftover socket: remove it, then start.
    RemoveStaleThenStart,
    /// Refuse to touch an unexpected entry.
    Refuse,
}

pub(crate) fn decide_endpoint(endpoint: EndpointState, lock_held: bool) -> EndpointAction {
    match (endpoint, lock_held) {
        (EndpointState::Foreign, _) => EndpointAction::Refuse,
        (_, true) => EndpointAction::UseRunning,
        (EndpointState::Absent, false) => EndpointAction::Start,
        (EndpointState::Socket, false) => EndpointAction::RemoveStaleThenStart,
    }
}

fn endpoint_state(path: &Path) -> EndpointState {
    match fs::symlink_metadata(path) {
        Err(_) => EndpointState::Absent,
        Ok(metadata) if metadata.file_type().is_socket() && metadata.uid() == current_uid() => {
            EndpointState::Socket
        }
        Ok(_) => EndpointState::Foreign,
    }
}

/// Whether a live server holds the storage lock. A missing lock file means
/// storage was never initialized, so no server can be running.
fn server_lock_held(server_dir: &Path) -> bool {
    let Ok(file) = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(server_dir.join("server.lock"))
    else {
        return false;
    };
    let fd = std::os::fd::AsRawFd::as_raw_fd(&file);
    // SAFETY: fd is a valid open descriptor owned by `file`; a successful
    // probe lock is released by the following unlock/close.
    let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        // SAFETY: as above.
        unsafe { libc::flock(fd, libc::LOCK_UN) };
        false
    } else {
        true
    }
}

/// Exclusive per-root lifecycle lock (released on drop/close).
struct LifecycleLock {
    _file: File,
}

fn lifecycle_lock(state: &StateRoot, timeout: Duration) -> Result<LifecycleLock, OpenError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(state.root.join("lifecycle.lock"))
        .map_err(|_| OpenError::StateRootUnavailable)?;
    let metadata = file
        .metadata()
        .map_err(|_| OpenError::StateRootUnavailable)?;
    if !metadata.is_file() || metadata.uid() != current_uid() {
        return Err(OpenError::StateRootUnsafe);
    }
    let fd = std::os::fd::AsRawFd::as_raw_fd(&file);
    let deadline = Instant::now() + timeout;
    loop {
        // SAFETY: fd is a valid descriptor owned by `file`.
        if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(LifecycleLock { _file: file });
        }
        if Instant::now() >= deadline {
            return Err(OpenError::ServerStartTimedOut);
        }
        thread::sleep(POLL_STEP);
    }
}

fn storage_initialized(server_dir: &Path) -> bool {
    server_dir.join("server.lock").exists() && server_dir.join("generation").exists()
}

fn server_ready(state: &StateRoot) -> bool {
    AuthenticatedStream::connect(&state.server, Instant::now() + Duration::from_millis(500)).is_ok()
}

/// Rotate the bounded server log before a new server appends to it.
fn open_log(state: &StateRoot) -> Result<File, OpenError> {
    let path = state.root.join(LOG_NAME);
    if fs::metadata(&path).is_ok_and(|metadata| metadata.len() > MAX_LOG_BYTES) {
        let _ = fs::rename(&path, state.root.join("server.log.1"));
    }
    OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|_| OpenError::ServerStartFailed)
}

/// Shell for new workspaces: absolute executable `$SHELL`, else `/bin/zsh`.
pub(crate) fn default_shell(shell: Option<OsString>) -> PathBuf {
    shell
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && is_executable_file(path))
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"))
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.mode() & 0o111 != 0)
}

/// Start `panelink serve` detached in its own session. The server inherits
/// this invocation's environment and working directory as the explicit
/// launch context for its first shell (existing `serve` behavior).
fn spawn_server(state: &StateRoot) -> Result<std::process::Child, OpenError> {
    let executable = std::env::current_exe().map_err(|_| OpenError::ServerStartFailed)?;
    let log = open_log(state)?;
    let log_err = log.try_clone().map_err(|_| OpenError::ServerStartFailed)?;
    let shell = default_shell(std::env::var_os("SHELL"));
    let mut command = Command::new(executable);
    command
        .arg("serve")
        .arg("--directory")
        .arg(&state.server)
        .arg("--terminfo")
        .arg(&state.terminfo);
    if !storage_initialized(&state.server) {
        command.arg("--initialize");
    }
    command
        .arg("--allow-input")
        .arg("--")
        .arg(shell)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    // SAFETY: setsid is async-signal-safe and only detaches the child from
    // this terminal's session so closing the window cannot hang it up.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn().map_err(|_| OpenError::ServerStartFailed)?;
    // Spawned under the lifecycle lock, so this is the only starter. Record
    // pid + kernel start time; `stop` re-verifies both before signalling.
    let pid = i32::try_from(child.id()).map_err(|_| OpenError::ServerStartFailed)?;
    let start = process_start_time(pid).ok_or(OpenError::ServerStartFailed)?;
    write_private(
        &state.root.join(PID_NAME),
        format!("{pid} {start}\n").as_bytes(),
    )?;
    Ok(child)
}

/// Kernel start time (microseconds) of a live process owned by this user.
fn process_start_time(pid: i32) -> Option<u64> {
    // SAFETY: proc_bsdinfo is plain data; zeroed is a valid initial value.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: the buffer pointer/size describe `info` exactly.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            std::ptr::addr_of_mut!(info).cast(),
            i32::try_from(size).ok()?,
        )
    };
    if usize::try_from(written).ok()? != size || info.pbi_uid != current_uid() {
        return None;
    }
    // A zombie (exited, unreaped) is not a live server.
    if info.pbi_status == libc::SZOMB {
        return None;
    }
    Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

/// Ensure a server is running for `state`, starting one if needed.
///
/// The whole inspect/cleanup/spawn/wait sequence runs under the lifecycle
/// lock, so a concurrent opener simply waits and then finds the server.
fn ensure_server(state: &StateRoot) -> Result<(), OpenError> {
    let _lifecycle = lifecycle_lock(state, START_TIMEOUT + STOP_TIMEOUT)?;
    let endpoint = endpoint_state(&state.socket_path());
    let lock_held = server_lock_held(&state.server);
    match decide_endpoint(endpoint, lock_held) {
        EndpointAction::Refuse => return Err(OpenError::StaleEndpointUnsafe),
        EndpointAction::UseRunning => return wait_ready(state, None),
        EndpointAction::RemoveStaleThenStart => {
            // Still under the lifecycle lock: nobody can start a server
            // between the lock check above and this removal.
            fs::remove_file(state.socket_path()).map_err(|_| OpenError::StaleEndpointUnsafe)?;
        }
        EndpointAction::Start => {}
    }
    let child = spawn_server(state)?;
    wait_ready(state, Some(child))
}

/// Wait until the server accepts an authenticated connection. When we just
/// spawned it, fail fast if it exits instead of becoming ready.
fn wait_ready(state: &StateRoot, mut child: Option<std::process::Child>) -> Result<(), OpenError> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if endpoint_state(&state.socket_path()) == EndpointState::Socket && server_ready(state) {
            // The child keeps running after we exit; it is not waited on.
            drop(child);
            return Ok(());
        }
        if let Some(process) = child.as_mut() {
            if process.try_wait().ok().flatten().is_some() {
                return Err(OpenError::ServerStartFailed);
            }
        }
        if Instant::now() >= deadline {
            return Err(OpenError::ServerStartTimedOut);
        }
        thread::sleep(POLL_STEP);
    }
}

fn prepare() -> Result<StateRoot, OpenError> {
    let state = resolve_root(std::env::var_os("PANELINK_HOME"), std::env::var_os("HOME"))?;
    prepare_root(&state)?;
    Ok(state)
}

/// `panelink` with no arguments.
pub(crate) fn open() -> i32 {
    let result = prepare().and_then(|state| {
        ensure_terminfo(&state)?;
        ensure_server(&state)?;
        Ok(state)
    });
    match result {
        Ok(state) => crate::ui_client::run(state.server, state.contexts),
        Err(error) => {
            eprintln!("panelink: {}", error.message());
            1
        }
    }
}

/// Parse `PID START` from the private identity record.
pub(crate) fn parse_identity(text: &str) -> Option<(i32, u64)> {
    let mut parts = text.split_whitespace();
    let pid = parts.next()?.parse::<i32>().ok().filter(|pid| *pid > 1)?;
    let start = parts
        .next()?
        .parse::<u64>()
        .ok()
        .filter(|start| *start > 0)?;
    parts.next().is_none().then_some((pid, start))
}

/// The recorded server pid, only if that exact process (same start time and
/// owner) is still alive.
fn live_server_pid(state: &StateRoot) -> Option<i32> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(state.root.join(PID_NAME))
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.uid() != current_uid() || metadata.mode() & 0o077 != 0 {
        return None;
    }
    let mut text = String::new();
    file.take(64).read_to_string(&mut text).ok()?;
    let (pid, start) = parse_identity(&text)?;
    (process_start_time(pid) == Some(start)).then_some(pid)
}

/// `panelink stop`: end the background workspace (server and its shells).
pub(crate) fn stop() -> i32 {
    match stop_inner() {
        Ok(true) => {
            println!("panelink: workspace stopped");
            0
        }
        Ok(false) => {
            println!("panelink: no workspace is running");
            0
        }
        Err(error) => {
            eprintln!("panelink: {}", error.message());
            1
        }
    }
}

fn stop_inner() -> Result<bool, OpenError> {
    let state = prepare()?;
    let _lifecycle = lifecycle_lock(&state, STOP_TIMEOUT)?;
    if !server_lock_held(&state.server) {
        if endpoint_state(&state.socket_path()) == EndpointState::Socket {
            let _ = fs::remove_file(state.socket_path());
        }
        let _ = fs::remove_file(state.root.join(PID_NAME));
        return Ok(false);
    }
    let pid = live_server_pid(&state).ok_or(OpenError::StopFailed)?;
    // SAFETY: kill with a verified pid (same owner and kernel start time as
    // the server this root spawned) and SIGTERM, which serve handles cleanly.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        return Err(OpenError::StopFailed);
    }
    let deadline = Instant::now() + STOP_TIMEOUT;
    while server_lock_held(&state.server) {
        if Instant::now() >= deadline {
            return Err(OpenError::StopFailed);
        }
        thread::sleep(POLL_STEP);
    }
    let _ = fs::remove_file(state.root.join(PID_NAME));
    Ok(true)
}

/// Private context file reused by `panelink panes` and `panelink close`.
///
/// It lives in the private state root (not `contexts/`, which belongs to the
/// attach client), so both commands resume one lease instead of issuing a new
/// one per run.
const CLI_CONTEXT_NAME: &str = "cli.ctx";

const NO_WORKSPACE: &str = "no workspace is running";

/// The default workspace's state root when its server is running; located
/// exactly as `panelink stop` does, and never starts a server.
fn running_workspace() -> Result<Option<StateRoot>, OpenError> {
    let state = prepare()?;
    if !server_lock_held(&state.server)
        || endpoint_state(&state.socket_path()) != EndpointState::Socket
    {
        return Ok(None);
    }
    Ok(Some(state))
}

fn workspace_link(state: &StateRoot) -> Result<crate::p3_cli::WorkspaceLink, &'static str> {
    let link =
        crate::p3_cli::WorkspaceLink::open(&state.server, &state.root.join(CLI_CONTEXT_NAME))?;
    if link.discarded_uncertain {
        eprintln!("panelink: note: an earlier close from a previous workspace was never confirmed");
    }
    Ok(link)
}

/// Human-readable one-line description of one pane.
pub(crate) fn pane_line(index: usize, pane: &panelink::multi_runtime::PaneEntry) -> String {
    let target = pane.target;
    format!(
        "  {index}  {:<7} {}x{} at col {}, row {}  (target {}:{}:{}:{}:{})",
        if pane.live { "running" } else { "exited" },
        pane.rect.rows,
        pane.rect.columns,
        pane.rect.x,
        pane.rect.y,
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision
    )
}

/// `panelink panes`: list the default workspace's panes in layout order.
pub(crate) fn panes() -> i32 {
    let state = match running_workspace() {
        Ok(Some(state)) => state,
        Ok(None) => {
            eprintln!("panelink: {NO_WORKSPACE}");
            return 1;
        }
        Err(error) => {
            eprintln!("panelink: {}", error.message());
            return 1;
        }
    };
    let reply = match workspace_link(&state).and_then(|mut link| link.panes()) {
        Ok(reply) => reply,
        Err(_) => {
            eprintln!("panelink: could not read the workspace panes");
            return 1;
        }
    };
    let count = reply.panes.iter().flatten().count();
    println!(
        "{count} pane{} (layout revision {}, {}x{}):",
        if count == 1 { "" } else { "s" },
        reply.layout_revision,
        reply.rows,
        reply.columns
    );
    for (index, pane) in reply.panes.iter().flatten().enumerate() {
        println!("{}", pane_line(index + 1, pane));
    }
    0
}

/// Parse a 1-based pane index argument; `None` is a usage error.
pub(crate) fn parse_pane_index(value: &std::ffi::OsStr) -> Option<usize> {
    let text = value.to_str()?;
    if text.is_empty() || text.len() > 4 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<usize>().ok().filter(|index| *index != 0)
}

/// User-facing result of one `panelink close` (message, exit code).
pub(crate) fn close_message(
    index: usize,
    last_pane: bool,
    result: Result<panelink::close_protocol::CloseResult, crate::p3_cli::CloseFailure>,
) -> (String, i32) {
    use crate::p3_cli::CloseFailure;
    use panelink::close_protocol::{CloseOutcome, CloseRejection, StopOutcome};
    let result = match result {
        Ok(result) => result,
        Err(CloseFailure::Refused) => {
            return (
                "close refused by the server; nothing was stopped".to_owned(),
                1,
            )
        }
        Err(CloseFailure::Unknown) => {
            return (
                format!("close status unknown; pane {index} may still be running"),
                1,
            )
        }
        Err(CloseFailure::NotSent(_)) => {
            return ("close not sent; nothing was stopped".to_owned(), 1)
        }
    };
    let how = match result.outcome {
        CloseOutcome::Stopped(StopOutcome::Exited) => "exited",
        CloseOutcome::Stopped(StopOutcome::Escalated) => "escalated: SIGKILL",
        CloseOutcome::Stopped(StopOutcome::Unknown) => {
            return (
                format!("close status unknown; pane {index} may still be running"),
                1,
            )
        }
        CloseOutcome::Rejected(reason) => {
            let why = match reason {
                CloseRejection::StaleLayout => "the layout changed",
                CloseRejection::StaleTarget => "the pane changed",
                CloseRejection::AlreadyClosed => "the pane is already closed",
                CloseRejection::NoLayout => "the workspace has no panes",
                CloseRejection::NotAdmitted => "the server did not admit it",
            };
            return (
                format!("close rejected: {why}; run 'panelink panes' and retry"),
                1,
            );
        }
    };
    let mut message = format!("closed pane {index} ({how})");
    if last_pane {
        message.push_str("; the workspace has ended");
    } else if result.survivor_resize_pending {
        message.push_str("; the remaining pane's resize is pending");
    }
    (message, 0)
}

/// `panelink close INDEX`: explicitly stop one pane of the default workspace.
pub(crate) fn close(index: usize) -> i32 {
    let state = match running_workspace() {
        Ok(Some(state)) => state,
        Ok(None) => {
            eprintln!("panelink: {NO_WORKSPACE}");
            return 1;
        }
        Err(error) => {
            eprintln!("panelink: {}", error.message());
            return 1;
        }
    };
    let mut link = match workspace_link(&state) {
        Ok(link) => link,
        Err(_) => {
            eprintln!("panelink: could not connect to the workspace");
            return 1;
        }
    };
    let reply = match link.panes() {
        Ok(reply) => reply,
        Err(_) => {
            eprintln!("panelink: could not read the workspace panes");
            return 1;
        }
    };
    let count = reply.panes.iter().flatten().count();
    let Some(pane) = reply.panes.iter().flatten().nth(index - 1) else {
        eprintln!("panelink: no pane {index}; run 'panelink panes'");
        return 1;
    };
    let result = link.close(&state.server, pane.target, reply.layout_revision);
    let (message, code) = close_message(index, count == 1, result);
    if code == 0 {
        println!("panelink: {message}");
    } else {
        eprintln!("panelink: {message}");
    }
    code
}

/// Whether storage at `server_dir` can be opened (diagnostic helper for tests).
#[cfg(test)]
fn storage_openable(server_dir: &Path) -> bool {
    panelink::server_storage::ServerStorage::open(server_dir).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let base = std::env::temp_dir().canonicalize().unwrap();
        let path = base.join(format!(
            "pl-open-{name}-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        path
    }

    #[test]
    fn root_prefers_panelink_home_then_library() {
        let state = resolve_root(Some("/tmp/x".into()), Some("/Users/u".into())).unwrap();
        assert_eq!(state.root, PathBuf::from("/tmp/x"));
        assert_eq!(state.server, PathBuf::from("/tmp/x/server"));
        let state = resolve_root(None, Some("/Users/u".into())).unwrap();
        assert_eq!(
            state.root,
            PathBuf::from("/Users/u/Library/Application Support/Panelink")
        );
        assert_eq!(resolve_root(None, None), Err(OpenError::HomeUnavailable));
        assert_eq!(
            resolve_root(Some("relative".into()), None),
            Err(OpenError::StateRootUnavailable)
        );
    }

    #[test]
    fn overlong_socket_path_is_rejected() {
        let long = format!("/{}", "a".repeat(100));
        assert_eq!(
            resolve_root(Some(long.into()), None),
            Err(OpenError::SocketPathTooLong)
        );
    }

    #[test]
    fn private_directory_permissions_are_enforced() {
        let dir = temp_dir("perm");
        let sub = dir.join("root");
        ensure_private_dir(&sub).unwrap();
        assert_eq!(fs::metadata(&sub).unwrap().mode() & 0o777, 0o700);
        fs::set_permissions(&sub, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        assert_eq!(ensure_private_dir(&sub), Err(OpenError::StateRootUnsafe));
        fs::set_permissions(&sub, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&sub, &link).unwrap();
        assert_eq!(ensure_private_dir(&link), Err(OpenError::StateRootUnsafe));
        let file = dir.join("file");
        fs::write(&file, b"x").unwrap();
        assert_eq!(ensure_private_dir(&file), Err(OpenError::StateRootUnsafe));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn endpoint_decisions_never_touch_a_live_server_or_foreign_entry() {
        use EndpointAction::*;
        use EndpointState::*;
        assert_eq!(decide_endpoint(Socket, true), UseRunning);
        assert_eq!(decide_endpoint(Absent, true), UseRunning);
        assert_eq!(decide_endpoint(Absent, false), Start);
        assert_eq!(decide_endpoint(Socket, false), RemoveStaleThenStart);
        assert_eq!(decide_endpoint(Foreign, false), Refuse);
        assert_eq!(decide_endpoint(Foreign, true), Refuse);
    }

    #[test]
    fn endpoint_state_classifies_files_and_sockets() {
        let dir = temp_dir("ep");
        assert_eq!(endpoint_state(&dir.join("none")), EndpointState::Absent);
        fs::write(dir.join("file"), b"x").unwrap();
        assert_eq!(endpoint_state(&dir.join("file")), EndpointState::Foreign);
        let socket = dir.join("s.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert_eq!(endpoint_state(&socket), EndpointState::Socket);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lock_probe_detects_a_held_storage_lock() {
        let dir = temp_dir("lock");
        let server = dir.join("server");
        assert!(!server_lock_held(&server));
        ensure_private_dir(&server).unwrap();
        let storage = panelink::server_storage::ServerStorage::initialize(&server).unwrap();
        assert!(server_lock_held(&server));
        assert!(!storage_openable(&server));
        drop(storage);
        assert!(!server_lock_held(&server));
        assert!(storage_openable(&server));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn terminfo_compile_decision_tracks_source_fingerprint() {
        let dir = temp_dir("ti");
        assert!(terminfo_needs_compile(&dir, "a"));
        fs::create_dir_all(dir.join("p")).unwrap();
        fs::write(dir.join("p/panelink-probe"), b"x").unwrap();
        assert!(terminfo_needs_compile(&dir, "a"));
        fs::write(dir.join("source.fingerprint"), terminfo_fingerprint("a")).unwrap();
        assert!(!terminfo_needs_compile(&dir, "a"));
        assert!(terminfo_needs_compile(&dir, "b"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn embedded_terminfo_compiles() {
        let dir = temp_dir("tic");
        let state = StateRoot::from_root(dir.clone());
        prepare_root(&state).unwrap();
        ensure_terminfo(&state).unwrap();
        assert!(!terminfo_needs_compile(&state.terminfo, TERMINFO_SOURCE));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn concurrent_terminfo_compiles_both_succeed() {
        let dir = temp_dir("tic-race");
        let state = StateRoot::from_root(dir.clone());
        prepare_root(&state).unwrap();
        // Distinct staging per thread mirrors distinct processes.
        let results: Vec<_> = (0..4)
            .map(|index| {
                let state = state.clone();
                thread::spawn(move || {
                    let staging = state.root.join(format!("terminfo.staging.t{index}"));
                    ensure_private_dir(&staging).unwrap();
                    let result = compile_and_publish(&state, &staging);
                    let _ = fs::remove_dir_all(&staging);
                    result
                })
            })
            .map(|handle| handle.join().unwrap())
            .collect();
        assert!(results.iter().all(Result::is_ok), "{results:?}");
        assert!(!terminfo_needs_compile(&state.terminfo, TERMINFO_SOURCE));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn identity_record_parsing_is_strict() {
        assert_eq!(parse_identity("123 456\n"), Some((123, 456)));
        assert_eq!(parse_identity("1 456"), None);
        assert_eq!(parse_identity("123"), None);
        assert_eq!(parse_identity("123 0"), None);
        assert_eq!(parse_identity("123 456 7"), None);
        assert_eq!(parse_identity("-5 456"), None);
    }

    #[test]
    fn process_identity_matches_only_the_same_live_process() {
        let me = i32::try_from(std::process::id()).unwrap();
        let start = process_start_time(me).expect("own start time");
        assert_eq!(process_start_time(me), Some(start));
        let mut child = Command::new("/usr/bin/true").spawn().unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        child.wait().unwrap();
        assert_eq!(
            process_start_time(pid),
            None,
            "reaped process has no identity"
        );
        // launchd (pid 1) belongs to root, not this user.
        assert_eq!(process_start_time(1), None);
    }

    #[test]
    fn ancestors_must_not_be_writable_by_others() {
        let dir = temp_dir("anc");
        let open = dir.join("open");
        fs::create_dir(&open).unwrap();
        fs::set_permissions(&open, std::os::unix::fs::PermissionsExt::from_mode(0o777)).unwrap();
        assert_eq!(
            ancestors_ok(&open.join("root"), current_uid()),
            Err(OpenError::StateRootUnsafe)
        );
        assert_eq!(ancestors_ok(&dir.join("root"), current_uid()), Ok(()));
        assert_eq!(
            ancestors_ok(Path::new("/tmp/x/root"), current_uid()),
            Ok(())
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lifecycle_lock_is_exclusive_until_dropped() {
        let dir = temp_dir("life");
        let state = StateRoot::from_root(dir.clone());
        prepare_root(&state).unwrap();
        let held = lifecycle_lock(&state, Duration::from_millis(50)).unwrap();
        assert!(lifecycle_lock(&state, Duration::from_millis(100)).is_err());
        drop(held);
        assert!(lifecycle_lock(&state, Duration::from_millis(100)).is_ok());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pane_index_accepts_only_small_positive_decimals() {
        for (text, expected) in [("1", Some(1)), ("2", Some(2)), ("0042", Some(42))] {
            assert_eq!(parse_pane_index(std::ffi::OsStr::new(text)), expected);
        }
        for text in ["", "0", "-1", "+1", "1.0", "x", "12345", " 1"] {
            assert_eq!(
                parse_pane_index(std::ffi::OsStr::new(text)),
                None,
                "{text:?}"
            );
        }
    }

    #[test]
    fn close_messages_and_exit_codes_cover_every_outcome() {
        use crate::p3_cli::CloseFailure;
        use panelink::close_protocol::{CloseOutcome, CloseRejection, CloseResult, StopOutcome};
        use panelink::control_protocol::TargetRef;
        use panelink::protocol::OperationKey;
        let result = |outcome, pending| {
            Ok(CloseResult {
                key: OperationKey {
                    generation: 1,
                    lease: 2,
                    sequence: 3,
                },
                outcome,
                target: Some(TargetRef {
                    generation: 1,
                    workspace: 1,
                    pane: 2,
                    session: 2,
                    occupant_revision: 1,
                }),
                layout_revision: 4,
                survivor_resize_pending: pending,
            })
        };
        let exited = CloseOutcome::Stopped(StopOutcome::Exited);
        let escalated = CloseOutcome::Stopped(StopOutcome::Escalated);
        assert_eq!(
            close_message(2, false, result(exited, false)),
            ("closed pane 2 (exited)".to_owned(), 0)
        );
        assert_eq!(
            close_message(2, false, result(escalated, false)),
            ("closed pane 2 (escalated: SIGKILL)".to_owned(), 0)
        );
        assert_eq!(
            close_message(1, true, result(exited, false)),
            (
                "closed pane 1 (exited); the workspace has ended".to_owned(),
                0
            )
        );
        assert_eq!(close_message(1, false, result(exited, true)).1, 0);
        let (message, code) = close_message(
            2,
            false,
            result(CloseOutcome::Stopped(StopOutcome::Unknown), false),
        );
        assert!(message.starts_with("close status unknown"), "{message}");
        assert_eq!(code, 1);
        let (message, code) = close_message(
            2,
            false,
            result(CloseOutcome::Rejected(CloseRejection::StaleLayout), false),
        );
        assert!(message.starts_with("close rejected"), "{message}");
        assert_eq!(code, 1);
        for failure in [
            CloseFailure::Refused,
            CloseFailure::Unknown,
            CloseFailure::NotSent("transport_failed"),
        ] {
            let (message, code) = close_message(2, false, Err(failure));
            assert_eq!(code, 1);
            assert!(!message.contains("transport_failed"), "{message}");
        }
        assert!(close_message(2, false, Err(CloseFailure::Unknown))
            .0
            .starts_with("close status unknown"));
    }

    #[test]
    fn default_shell_requires_absolute_executable() {
        assert_eq!(default_shell(None), PathBuf::from("/bin/zsh"));
        assert_eq!(default_shell(Some("zsh".into())), PathBuf::from("/bin/zsh"));
        assert_eq!(
            default_shell(Some("/nonexistent/sh".into())),
            PathBuf::from("/bin/zsh")
        );
        assert_eq!(
            default_shell(Some("/bin/sh".into())),
            PathBuf::from("/bin/sh")
        );
    }
}
