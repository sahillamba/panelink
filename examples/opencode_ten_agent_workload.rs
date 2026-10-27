//! Idle, real-OpenCode Phase 6 workload. No model prompts are submitted.
#![allow(clippy::too_many_arguments, clippy::needless_range_loop)]

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod workload {
    use panelink::control_protocol::TargetRef;
    use panelink::handshake_protocol::{decode_welcome, Hello, CAPABILITY_INPUT, CAPABILITY_READ};
    use panelink::history_diagnostics::HistoryDiagnostics;
    use panelink::history_store::HistoryStore;
    use panelink::launch::{Environment, LaunchContext};
    use panelink::layout_store::{LayoutSlot, LayoutStore};
    use panelink::managed_launch_protocol::{EnvironmentEntry, LaunchRequest};
    use panelink::multi_runtime::MultiServerDriver;
    use panelink::multi_startup::{StartedMultiServer, StartupLaunch};
    use panelink::pane_layout::SplitDirection;
    use panelink::protocol::{OperationKey, OperationStatus, Outcome};
    use panelink::protocol_wire::{Frame, MessageKind};
    use panelink::response_protocol::{decode_response_payload, ResponseBody};
    use panelink::screen_checkpoint;
    use panelink::server_storage::ServerStorage;
    use panelink::session_protocol::{
        decode_session_info, encode_session_hello, encode_session_request, SessionHello,
        SessionRequest,
    };
    use panelink::split_protocol::{decode_split_result, encode_split_request, SplitRequest};
    use panelink::terminal::Key;
    use panelink::transport::{AuthenticatedStream, OwnedFrame};
    use panelink::ui_protocol::{
        decode_layout_reply, decode_screen_reply, encode_attach_target, encode_key_input,
        encode_layout_query, encode_screen_query, planned_split_size, KeyInput,
    };
    use std::ffi::OsString;
    use std::fs::{self, DirBuilder, OpenOptions};
    use std::io::{self, Write};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::process::{self, Command};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver, SyncSender};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    const WORKSPACES_AB: usize = 5;
    const WORKSPACES_C: usize = 6;
    const PANES: usize = 2;
    const MEASURE_SECONDS: u64 = 60;
    const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
    const KEEPALIVE_INTERVAL: Duration = Duration::from_millis(800);
    const INPUT_INTERVAL: Duration = Duration::from_secs(10);
    const IO_TIMEOUT: Duration = Duration::from_secs(8);
    /// Whole-probe bound: echo (8 s) + erase (8 s) + slack.
    const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
    const ROOT_ROWS: u16 = 53;
    const ROOT_COLUMNS: u16 = 102;
    const PID_WRAPPER: &str = "printf '%s\\n' \"$$\" > \"$1\"; shift; exec \"$@\"";
    const SHELL_SCRIPT: &str = r#"
printf '%s\n' "$$" > "$2"
printf 'PANELINK-SHELL-READY\r\n'
while [ ! -e "$3" ]; do sleep 0.05; done
(
  count=0
  while [ -e "$3" ]; do
    i=0
    while [ "$i" -lt 8 ]; do
      count=$((count + 1)); i=$((i + 1)); printf 'paced-shell-output-%s\r\n' "$count"
    done
    printf '%s\n' "$count" > "$4.pending" && mv "$4.pending" "$4"
    sleep 0.1
  done
) &
writer=$!
trap 'kill "$writer" 2>/dev/null; wait "$writer" 2>/dev/null; exit 0' HUP INT TERM
count=0
while IFS= read -r line; do
  if [ "$line" = q ]; then
    count=$((count + 1)); printf '%s\n' "$count" > "$1.pending" && mv "$1.pending" "$1"
  fi
done
"#;
    const BUILD_SHELL_SCRIPT: &str = r#"
printf '%s\n' "$$" > "$2"
printf 'PANELINK-SHELL-READY\r\n'
(
  while [ ! -e "$6" ]; do sleep 0.05; done
  count=0
  while [ -e "$6" ]; do
    if "$RUSTC" --edition=2021 "$3" -o "$4"; then
      count=$((count + 1))
      printf 'BUILD_OK %s\r\n' "$count"
      printf '%s\n' "$count" > "$1.pending" && mv "$1.pending" "$1"
    else
      printf 'BUILD_FAILED\r\n'
      printf '%s\n' failed > "$7.pending" && mv "$7.pending" "$7"
      exit 71
    fi
    sleep 1
  done
) &
writer=$!
trap 'kill "$writer" 2>/dev/null; wait "$writer" 2>/dev/null; exit 0' HUP INT TERM
count=0
while IFS= read -r line; do
  if [ "$line" = q ]; then
    count=$((count + 1)); printf '%s\n' "$count" > "$5.pending" && mv "$5.pending" "$5"
  fi
done
"#;
    type Result<T> = std::result::Result<T, &'static str>;

    static CLEANUP_FAILED: AtomicBool = AtomicBool::new(false);
    static CLEANUP_UNCERTAIN: AtomicBool = AtomicBool::new(false);
    static CLEANUP_IDENTITY_CHANGED: AtomicBool = AtomicBool::new(false);
    static FIXTURE_ROOT: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Variant {
        A,
        B,
        C,
    }

    impl Variant {
        fn parse(value: &str) -> Result<Self> {
            match value {
                "A" => Ok(Self::A),
                "B" => Ok(Self::B),
                "C" => Ok(Self::C),
                _ => Err("variant must be A, B, or C"),
            }
        }
        fn name(self) -> &'static str {
            match self {
                Self::A => "A",
                Self::B => "B",
                Self::C => "C",
            }
        }
        fn workspace_count(self) -> usize {
            if self == Self::C {
                WORKSPACES_C
            } else {
                WORKSPACES_AB
            }
        }
        fn kind(self, workspace: usize, pane: usize) -> PaneKind {
            match self {
                Self::A => {
                    if pane == 0 {
                        PaneKind::OpenCode
                    } else {
                        PaneKind::Shell
                    }
                }
                Self::B => PaneKind::OpenCode,
                Self::C if workspace < WORKSPACES_AB => PaneKind::OpenCode,
                Self::C if pane == 0 => PaneKind::Shell,
                Self::C => PaneKind::BuildShell,
            }
        }
        fn opencode_count(self) -> usize {
            match self {
                Self::A => WORKSPACES_AB,
                Self::B | Self::C => WORKSPACES_AB * PANES,
            }
        }
    }

    struct PrivateRoot(PathBuf);

    impl PrivateRoot {
        fn create() -> Result<Self> {
            let base =
                fs::canonicalize(std::env::temp_dir()).map_err(|_| "temporary root unavailable")?;
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            for attempt in 0..32_u32 {
                let path = base.join(format!(
                    "panelink-opencode-p6-{}-{nonce:x}-{attempt:x}",
                    process::id()
                ));
                match private_dir(&path) {
                    Ok(()) => {
                        let canonical = fs::canonicalize(&path)
                            .map_err(|_| "temporary root validation failed")?;
                        let metadata = fs::symlink_metadata(&canonical)
                            .map_err(|_| "temporary root validation failed")?;
                        if canonical != path || metadata.mode() & 0o7777 != 0o700 {
                            return Err("temporary root permissions invalid");
                        }
                        return Ok(Self(canonical));
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(_) => return Err("temporary root creation failed"),
                }
            }
            Err("temporary root name allocation exhausted")
        }
    }

    fn private_dir(path: &Path) -> io::Result<()> {
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        builder.create(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
    }

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum PaneKind {
        OpenCode,
        Shell,
        BuildShell,
    }

    impl PaneKind {
        fn label(self) -> &'static str {
            match self {
                Self::OpenCode => "opencode",
                Self::Shell => "shell_log",
                Self::BuildShell => "shell_build",
            }
        }
        fn is_shell(self) -> bool {
            self != Self::OpenCode
        }
        fn is_build(self) -> bool {
            self == Self::BuildShell
        }
    }

    struct Launcher {
        executable: PathBuf,
        label: &'static str,
        version: String,
    }

    impl Launcher {
        fn resolve() -> Result<Self> {
            let path = std::env::var_os("PATH").unwrap_or_default();
            let candidates =
                std::env::split_paths(&path).map(|directory| directory.join("opencode"));
            let direct = candidates.into_iter().find(|path| executable(path));
            let fallback = std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".opencode/bin/opencode"))
                .filter(|path| executable(path));
            let (executable, label) = if let Some(path) = direct {
                (path, "opencode on PATH")
            } else if let Some(path) = fallback {
                (path, "~/.opencode/bin/opencode")
            } else {
                let wrapper = std::env::split_paths(&path)
                    .map(|directory| directory.join("tescode"))
                    .find(|path| executable(path))
                    .ok_or("OpenCode and tescode launchers are unavailable")?;
                (wrapper, "tescode workflow custom")
            };
            let version = if label == "tescode workflow custom" {
                let output = Command::new(&executable)
                    .args(["version"])
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin")
                    .output()
                    .map_err(|_| "tescode version probe failed to start")?;
                if !output.status.success() {
                    return Err("tescode version probe failed");
                }
                format!(
                    "tescode {}",
                    String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .next()
                        .unwrap_or("version unavailable")
                        .trim()
                )
            } else {
                let root = PrivateRoot::create()?;
                let output_result = Command::new(&executable)
                    .arg("--version")
                    .env_clear()
                    .env("HOME", &root.0)
                    .env("PATH", "/usr/bin:/bin")
                    .env("OPENCODE_DISABLE_MODELS_FETCH", "1")
                    .env("OPENCODE_DISABLE_AUTOUPDATE", "1")
                    .env("OPENCODE_DISABLE_UPDATE_CHECK", "1")
                    .output();
                // output() has reaped the version process. This probe root is
                // distinct from the workload fixture retained on run failure.
                fs::remove_dir_all(&root.0).map_err(|_| "version probe fixture cleanup failed")?;
                if let Ok(mut recorded) = FIXTURE_ROOT.lock() {
                    if recorded.as_deref() == Some(root.0.as_path()) {
                        *recorded = None;
                    }
                }
                let output = output_result.map_err(|_| "OpenCode version probe failed to start")?;
                if !output.status.success() {
                    return Err("OpenCode version probe failed");
                }
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or("version unavailable")
                    .trim()
                    .to_owned()
            };
            Ok(Self {
                executable,
                label,
                version,
            })
        }
        fn command(&self, project: &Path) -> Vec<OsString> {
            if self.label == "tescode workflow custom" {
                vec![
                    "workflow".into(),
                    "custom".into(),
                    project.as_os_str().to_owned(),
                ]
            } else {
                let mut arguments = Vec::new();
                // This installed/pinned v2 CLI otherwise starts one managed
                // service at a fixed shared port despite separate HOME/XDG.
                // Its locally verified --standalone mode owns a private server.
                if self.version == "opencode v2.0.32" {
                    arguments.push("--standalone".into());
                }
                arguments.push(project.as_os_str().to_owned());
                arguments
            }
        }
    }

    fn executable(path: &Path) -> bool {
        fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }

    fn resolve_rustc() -> Result<PathBuf> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let output = Command::new("rustup")
            .args(["which", "rustc"])
            .env("PATH", &path)
            .output();
        if let Ok(output) = output {
            if output.status.success() {
                let candidate = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
                if candidate.is_absolute() && executable(&candidate) {
                    return fs::canonicalize(candidate)
                        .map_err(|_| "rustc path canonicalization failed");
                }
            }
        }
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join("rustc");
            if executable(&candidate) {
                return fs::canonicalize(candidate)
                    .map_err(|_| "rustc path canonicalization failed");
            }
        }
        Err("local rustc executable unavailable")
    }

    struct Pane {
        kind: PaneKind,
        executable: PathBuf,
        arguments: Vec<OsString>,
        context: LaunchContext,
        pid_file: PathBuf,
        ack_file: PathBuf,
        build_file: PathBuf,
        build_failure_file: PathBuf,
        start_file: PathBuf,
        pid: Option<u32>,
        identity: Option<ProcessIdentity>,
        startup_started: Instant,
        startup_us: Option<u64>,
        marker: Option<&'static str>,
        prompt_ready: bool,
    }

    impl Pane {
        fn new(
            root: &Path,
            terminfo: &Path,
            pane: usize,
            kind: PaneKind,
            launcher: &Launcher,
            rustc: &Path,
        ) -> Result<Self> {
            let private = root.join(format!("pane-{pane}"));
            let project = private.join("project");
            let home = private.join("home");
            private_dir(&private).map_err(|_| "pane directory setup failed")?;
            private_dir(&project).map_err(|_| "project directory setup failed")?;
            private_dir(&home).map_err(|_| "isolated HOME setup failed")?;
            let mut env = Environment::new();
            for (name, folder) in [
                ("XDG_CONFIG_HOME", "config"),
                ("XDG_DATA_HOME", "data"),
                ("XDG_STATE_HOME", "state"),
                ("XDG_CACHE_HOME", "cache"),
            ] {
                let directory = home.join(folder);
                private_dir(&directory).map_err(|_| "isolated XDG setup failed")?;
                env.insert(name.into(), directory.into_os_string());
            }
            let temp = private.join("tmp");
            private_dir(&temp).map_err(|_| "isolated TMPDIR setup failed")?;
            env.extend([
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("HOME".into(), home.into_os_string()),
                ("TMPDIR".into(), temp.into_os_string()),
                ("TERMINFO".into(), terminfo.as_os_str().to_owned()),
                ("TERM".into(), "panelink-probe".into()),
                ("LANG".into(), "C.UTF-8".into()),
                ("LC_ALL".into(), "C.UTF-8".into()),
                ("OPENCODE_DISABLE_MODELS_FETCH".into(), "1".into()),
                ("OPENCODE_DISABLE_AUTOUPDATE".into(), "1".into()),
                ("OPENCODE_DISABLE_UPDATE_CHECK".into(), "1".into()),
                ("DO_NOT_TRACK".into(), "1".into()),
            ]);
            let pid_file = private.join("child.pid");
            let ack_file = private.join("child.ack");
            let build_file = private.join("build-count");
            let build_failure_file = private.join("build-failed");
            let start_file = private.join("start-workload");
            let (executable, arguments) = match kind {
                PaneKind::OpenCode => {
                    let command = launcher.command(&project);
                    let mut args = vec![
                        "-c".into(),
                        PID_WRAPPER.into(),
                        "panelink-opencode".into(),
                        pid_file.as_os_str().to_owned(),
                        launcher.executable.as_os_str().to_owned(),
                    ];
                    args.extend(command);
                    (PathBuf::from("/bin/sh"), args)
                }
                PaneKind::Shell => (
                    PathBuf::from("/bin/sh"),
                    vec![
                        "-c".into(),
                        SHELL_SCRIPT.into(),
                        "panelink-shell".into(),
                        ack_file.as_os_str().to_owned(),
                        pid_file.as_os_str().to_owned(),
                        private.join("start-workload").into_os_string(),
                        build_file.as_os_str().to_owned(),
                    ],
                ),
                PaneKind::BuildShell => {
                    let src = private.join("build-project/src");
                    private_dir(&private.join("build-project"))
                        .map_err(|_| "build fixture setup failed")?;
                    private_dir(&src).map_err(|_| "build source directory setup failed")?;
                    let source = src.join("main.rs");
                    fs::write(
                        &source,
                        b"fn main() { let n: u64 = (1..=1000).sum(); println!(\"{n}\"); }\n",
                    )
                    .map_err(|_| "build fixture source setup failed")?;
                    let output = private.join("build-project/fixture-bin");
                    env.insert("RUSTC".into(), rustc.as_os_str().to_owned());
                    (
                        PathBuf::from("/bin/sh"),
                        vec![
                            "-c".into(),
                            BUILD_SHELL_SCRIPT.into(),
                            "panelink-build".into(),
                            build_file.as_os_str().to_owned(),
                            pid_file.as_os_str().to_owned(),
                            source.into_os_string(),
                            output.into_os_string(),
                            ack_file.as_os_str().to_owned(),
                            private.join("start-workload").into_os_string(),
                            build_failure_file.as_os_str().to_owned(),
                        ],
                    )
                }
            };
            Ok(Self {
                kind,
                executable,
                arguments,
                context: LaunchContext {
                    working_directory: project,
                    environment: env,
                },
                pid_file,
                ack_file,
                build_file,
                build_failure_file,
                start_file,
                pid: None,
                identity: None,
                startup_started: Instant::now(),
                startup_us: None,
                marker: None,
                prompt_ready: false,
            })
        }
        fn record_pid(&mut self) -> Result<()> {
            let text =
                fs::read_to_string(&self.pid_file).map_err(|_| "child PID marker missing")?;
            self.pid = text.trim().parse::<u32>().ok().filter(|pid| *pid != 0);
            if self.pid.is_none() {
                return Err("child PID marker invalid");
            }
            let pid = self.pid.ok_or("child PID marker invalid")?;
            self.identity = Some(ProcessIdentity::capture(pid)?);
            Ok(())
        }
    }

    enum ClientCommand {
        Keepalive,
        Screen(SyncSender<Result<Screen>>),
        Probe {
            index: usize,
            ack: PathBuf,
            previous: u64,
            reply: SyncSender<Result<InputResult>>,
        },
        Stop,
    }

    struct Screen {
        text: String,
        marker: Option<&'static str>,
        prompt: bool,
    }
    struct InputResult {
        kind: PaneKind,
        probe: usize,
        echo_us: Option<u64>,
        round_trip_us: u64,
        erased: bool,
    }

    struct Client {
        tx: SyncSender<ClientCommand>,
        ready: Receiver<Result<()>>,
        ready_done: bool,
        join: Option<JoinHandle<()>>,
    }

    impl Client {
        fn start(
            driver: &mut MultiServerDriver,
            target: TargetRef,
            pane: usize,
            kind: PaneKind,
            ack: PathBuf,
        ) -> Result<Self> {
            let (server, socket) = UnixStream::pair().map_err(|_| "control socketpair failed")?;
            driver
                .attach(
                    AuthenticatedStream::new(server)
                        .map_err(|_| "server peer authentication failed")?,
                )
                .map_err(|_| "runtime refused workload client")?;
            let (tx, rx) = mpsc::sync_channel(1);
            let (ready_tx, ready) = mpsc::sync_channel(1);
            let join = thread::Builder::new()
                .name(format!("opencode-p6-{pane}"))
                .spawn(move || client_loop(socket, target, pane, kind, ack, rx, ready_tx))
                .map_err(|_| "control client thread could not start")?;
            Ok(Self {
                tx,
                ready,
                ready_done: false,
                join: Some(join),
            })
        }
        fn poll_ready(&mut self) -> Result<bool> {
            if self.ready_done {
                return Ok(true);
            }
            match self.ready.try_recv() {
                Ok(result) => result.map(|()| true),
                Err(mpsc::TryRecvError::Empty) => Ok(false),
                Err(_) => Err("client handshake disconnected"),
            }
        }
        fn send(&self, command: ClientCommand) -> Result<()> {
            self.tx.send(command).map_err(|_| "control client stopped")
        }
        fn screen(&self) -> Result<Receiver<Result<Screen>>> {
            let (tx, rx) = mpsc::sync_channel(1);
            self.send(ClientCommand::Screen(tx))?;
            Ok(rx)
        }
        fn probe(
            &self,
            index: usize,
            ack: PathBuf,
            previous: u64,
        ) -> Result<Receiver<Result<InputResult>>> {
            let (tx, rx) = mpsc::sync_channel(1);
            self.send(ClientCommand::Probe {
                index,
                ack,
                previous,
                reply: tx,
            })?;
            Ok(rx)
        }
    }

    impl Drop for Client {
        fn drop(&mut self) {
            let _ = self.tx.try_send(ClientCommand::Stop);
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    struct Workspace {
        index: usize,
        history: PathBuf,
        layout: PathBuf,
        driver: Option<MultiServerDriver>,
        targets: [TargetRef; PANES],
        panes: [Pane; PANES],
        clients: Vec<Client>,
        ticks: Vec<u64>,
        inputs: Vec<InputResult>,
        pending: Vec<(Receiver<Result<InputResult>>, Instant)>,
        next_keepalive: Instant,
        next_input: Instant,
        input_index: usize,
        history_ok: bool,
        history_capture_failed: bool,
        history_diagnostics: HistoryDiagnostics,
        history_targets_json: String,
        history_diagnostic_json: String,
        history_inspection_complete: bool,
        build_cycles: u64,
        build_failed: bool,
        layout_ok: bool,
        shutdown_ms: Option<u64>,
        groups_stopped: bool,
    }

    struct CreateCleanup {
        pid_files: Vec<PathBuf>,
        identities: Vec<Option<ProcessIdentity>>,
        started: Vec<bool>,
        armed: bool,
    }
    impl Drop for CreateCleanup {
        fn drop(&mut self) {
            if self.armed {
                for (index, path) in self.pid_files.iter().enumerate() {
                    if !self.started.get(index).copied().unwrap_or(false) {
                        continue;
                    }
                    match fs::read_to_string(path)
                        .ok()
                        .and_then(|text| text.trim().parse::<u32>().ok())
                    {
                        Some(pid) => record_cleanup_result(stop_group(
                            pid,
                            self.identities.get(index).copied().flatten(),
                        )),
                        None => {
                            CLEANUP_FAILED.store(true, Ordering::SeqCst);
                            CLEANUP_UNCERTAIN.store(true, Ordering::SeqCst);
                        }
                    }
                }
            }
        }
    }

    impl Workspace {
        fn create(
            index: usize,
            path: PathBuf,
            variant: Variant,
            launcher: &Launcher,
            rustc: &Path,
        ) -> Result<Self> {
            private_dir(&path).map_err(|_| "workspace directory setup failed")?;
            let server = path.join("server");
            let history = server.join("history");
            let layout = server.join("layout");
            let terminfo = path.join("terminfo");
            private_dir(&server).map_err(|_| "server directory setup failed")?;
            private_dir(&terminfo).map_err(|_| "terminfo directory setup failed")?;
            compile_terminfo(&terminfo)?;
            let kinds = [variant.kind(index, 0), variant.kind(index, 1)];
            let mut panes = Vec::new();
            for (pane, kind) in kinds.into_iter().enumerate() {
                panes.push(Pane::new(&path, &terminfo, pane, kind, launcher, rustc)?);
            }
            let mut panes: [Pane; PANES] = panes.try_into().map_err(|_| "pane setup failed")?;
            let mut cleanup = CreateCleanup {
                pid_files: panes.iter().map(|pane| pane.pid_file.clone()).collect(),
                identities: vec![None; PANES],
                started: vec![false; PANES],
                armed: true,
            };
            let storage = ServerStorage::initialize(&server)
                .map_err(|_| "server storage initialization failed")?;
            let args = panes[0].arguments.clone();
            panes[0].startup_started = Instant::now();
            cleanup.started[0] = true;
            let started = StartedMultiServer::start(
                storage,
                &terminfo,
                &StartupLaunch {
                    executable: panes[0].executable.as_os_str(),
                    arguments: &args,
                    context: &panes[0].context,
                    rows: ROOT_ROWS,
                    columns: ROOT_COLUMNS,
                },
                true,
            )
            .map_err(|_| "OpenCode pane startup failed")?;
            let target0 = started
                .startup()
                .target
                .ok_or("first pane target missing")?;
            let mut driver = started.into_driver();
            private_dir(&history).map_err(|_| "history directory setup failed")?;
            driver
                .enable_layout(
                    LayoutStore::prepare(&server).map_err(|_| "layout storage setup failed")?,
                )
                .map_err(|_| "layout capture unavailable")?;
            driver.enable_history(
                HistoryStore::open(&history).map_err(|_| "history storage setup failed")?,
            );
            if !driver.history_available() {
                return Err("history capture unavailable");
            }
            wait_pid(&panes[0].pid_file, &mut driver)?;
            panes[0].record_pid()?;
            cleanup.identities[0] = panes[0].identity;
            panes[1].startup_started = Instant::now();
            cleanup.started[1] = true;
            let target1 = split_pane(&mut driver, target0, &mut panes[1], &terminfo)?;
            cleanup.identities[1] = panes[1].identity;
            wait_layout(&mut driver, &server, target0, target1)?;
            let diagnostics = driver.history_diagnostics();
            let workspace = Self {
                index,
                history,
                layout,
                driver: Some(driver),
                targets: [target0, target1],
                panes,
                clients: Vec::new(),
                ticks: Vec::new(),
                inputs: Vec::new(),
                pending: Vec::new(),
                next_keepalive: Instant::now(),
                next_input: Instant::now(),
                input_index: 0,
                history_ok: false,
                history_capture_failed: false,
                history_diagnostics: diagnostics,
                history_targets_json: String::new(),
                history_diagnostic_json: String::new(),
                history_inspection_complete: false,
                build_cycles: 0,
                build_failed: false,
                layout_ok: false,
                shutdown_ms: None,
                groups_stopped: false,
            };
            cleanup.armed = false;
            Ok(workspace)
        }
        fn connect(&mut self) -> Result<()> {
            for pane in 0..PANES {
                self.clients.push(Client::start(
                    self.driver.as_mut().ok_or("driver missing")?,
                    self.targets[pane],
                    pane,
                    self.panes[pane].kind,
                    self.panes[pane].ack_file.clone(),
                )?);
            }
            Ok(())
        }
        fn ready(&mut self) -> Result<bool> {
            let mut ready = true;
            for client in &mut self.clients {
                let is_ready = client.poll_ready()?;
                if is_ready {
                    client.ready_done = true;
                }
                ready &= is_ready;
            }
            Ok(ready)
        }
        fn tick(&mut self, measure: bool) -> Result<()> {
            let started = Instant::now();
            self.driver
                .as_mut()
                .ok_or("driver missing")?
                .tick(seconds)
                .map_err(|_| "owner tick failed")?;
            if measure {
                self.ticks.push(micros(started.elapsed()));
            }
            Ok(())
        }
        fn start_probes(&mut self, index: usize) -> Result<()> {
            for pane in 0..PANES {
                let previous = ack_value(&self.panes[pane].ack_file)?;
                self.pending.push((
                    self.clients[pane].probe(index, self.panes[pane].ack_file.clone(), previous)?,
                    Instant::now(),
                ));
            }
            Ok(())
        }
        fn advance(&mut self, now: Instant, stop: Instant) -> Result<()> {
            self.tick(true)?;
            if now >= self.next_keepalive {
                for client in &self.clients {
                    client.send(ClientCommand::Keepalive)?;
                }
                self.next_keepalive = now + KEEPALIVE_INTERVAL;
            }
            if now >= self.next_input && now < stop && self.pending.is_empty() {
                self.input_index += 1;
                self.start_probes(self.input_index)?;
                self.next_input += INPUT_INTERVAL;
            }
            let mut index = 0;
            while index < self.pending.len() {
                match self.pending[index].0.try_recv() {
                    Ok(Ok(value)) => {
                        self.pending.swap_remove(index);
                        self.inputs.push(value);
                    }
                    Ok(Err(_)) => return Err("input round trip failed"),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err("input client disconnected")
                    }
                    Err(mpsc::TryRecvError::Empty)
                        if now.duration_since(self.pending[index].1) >= PROBE_TIMEOUT =>
                    {
                        return Err("input round trip timed out")
                    }
                    Err(mpsc::TryRecvError::Empty) => index += 1,
                }
            }
            Ok(())
        }
        fn shutdown(&mut self) -> Result<()> {
            let deadline = Instant::now() + PROBE_TIMEOUT;
            while !self.pending.is_empty() && Instant::now() < deadline {
                self.tick(false)?;
                self.drain()?;
                thread::sleep(Duration::from_millis(2));
            }
            if !self.pending.is_empty() {
                return Err("input round trip pending at shutdown");
            }
            self.clients.clear();
            let started = Instant::now();
            let mut clean = true;
            for pane in &self.panes {
                if let (Some(pid), identity) = (pane.pid, pane.identity) {
                    let stopped = stop_group(pid, identity)?;
                    clean &= stopped;
                }
            }
            let history_failure_signal = self
                .driver
                .as_ref()
                .map(|driver| driver.history_failure_signal());
            drop(self.driver.take());
            self.shutdown_ms = Some(micros(started.elapsed()) / 1000);
            self.groups_stopped = clean;
            if !clean {
                return Err("child process group leaked");
            }
            let server_dir = self.history.parent().ok_or("history parent missing")?;
            let targets = self.targets;
            match lock_history_for_inspection(server_dir) {
                Ok(lock) => {
                    if let Ok(history) = HistoryStore::open(&self.history) {
                        let mut target_rows = Vec::new();
                        self.history_ok = true;
                        self.history_inspection_complete = true;
                        for target in targets {
                            let rows = history.load_rows(target);
                            let screens = history.load_screens(target);
                            let (saved, rows_readable) = match rows {
                                Ok(saved) => (saved, true),
                                Err(_) => (None, false),
                            };
                            let (screens, screens_readable) = match screens {
                                Ok(screens) => (screens, true),
                                Err(_) => ([None, None], false),
                            };
                            let rows_present = saved.is_some();
                            let (
                                retained_rows,
                                retained_bytes,
                                gap,
                                ended_at,
                                normal,
                                alternate,
                                capture_disabled,
                            ) = if let Some(saved) = saved {
                                (
                                    saved.records.len(),
                                    saved.records.iter().map(Vec::len).sum::<usize>(),
                                    saved.gap,
                                    saved.ended_at_unix_ms,
                                    screens[0].is_some(),
                                    screens[1].is_some(),
                                    saved.capture_disabled,
                                )
                            } else {
                                (
                                    0,
                                    0,
                                    false,
                                    0,
                                    screens[0].is_some(),
                                    screens[1].is_some(),
                                    false,
                                )
                            };
                            self.history_ok &=
                                rows_readable && screens_readable && rows_present && normal;
                            self.history_inspection_complete &= rows_readable && screens_readable;
                            target_rows.push(format!("{{\"target\":{{\"generation\":{},\"workspace\":{},\"pane\":{},\"session\":{},\"occupant_revision\":{}}},\"rows_readable\":{},\"rows_present\":{},\"screens_readable\":{},\"retained_rows\":{},\"retained_bytes\":{},\"gap\":{},\"ended\":{},\"ended_at_unix_ms\":{},\"normal_screen\":{},\"alternate_screen\":{},\"capture_disabled\":{}}}", target.generation, target.workspace, target.pane, target.session, target.occupant_revision, rows_readable, rows_present, screens_readable, retained_rows, retained_bytes, gap, ended_at != 0, ended_at, normal, alternate, capture_disabled));
                        }
                        self.history_targets_json = target_rows.join(",");
                    } else {
                        self.history_ok = false;
                    }
                    drop(lock);
                }
                Err(_) => {
                    self.history_ok = false;
                }
            }
            // Settlement prevents late writer results from being omitted.
            self.history_capture_failed = history_failure_signal
                .as_ref()
                .is_some_and(|signal| signal.load(Ordering::SeqCst));
            let diagnostic_snapshot = self.history_diagnostics.snapshot();
            let reason_names = diagnostic_snapshot
                .reasons()
                .map(|reason| json_string(reason.as_str()))
                .collect::<Vec<_>>()
                .join(",");
            self.history_diagnostic_json = format!(
                "{{\"sticky_failure\":{},\"reasons\":[{}],\"writer_drain_outcome\":{}}}",
                self.history_capture_failed,
                reason_names,
                json_string(diagnostic_snapshot.writer_drain_outcome().as_str())
            );
            for pane in &self.panes {
                if pane.kind.is_build() {
                    self.build_cycles = fs::read_to_string(&pane.build_file)
                        .ok()
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    self.build_failed = pane.build_failure_file.exists();
                }
            }
            let layout = LayoutStore::open(&self.layout).map_err(|_| "layout reopen failed")?;
            self.layout_ok = layout
                .load(LayoutSlot::Current)
                .ok()
                .flatten()
                .is_some_and(|saved| {
                    saved.pane_count() == 2 && saved.panes()[..2] == self.targets.map(Some)
                });
            Ok(())
        }
        fn drain(&mut self) -> Result<()> {
            let now = Instant::now();
            let mut index = 0;
            while index < self.pending.len() {
                match self.pending[index].0.try_recv() {
                    Ok(Ok(value)) => {
                        self.pending.swap_remove(index);
                        self.inputs.push(value);
                    }
                    Ok(Err(_)) => return Err("input round trip failed"),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err("input client disconnected")
                    }
                    Err(mpsc::TryRecvError::Empty)
                        if now.duration_since(self.pending[index].1) >= IO_TIMEOUT =>
                    {
                        return Err("input round trip timed out")
                    }
                    Err(mpsc::TryRecvError::Empty) => index += 1,
                }
            }
            Ok(())
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            self.clients.clear();
            for pane in &self.panes {
                if let Some(pid) = pane.pid {
                    record_cleanup_result(stop_group(pid, pane.identity));
                }
            }
            drop(self.driver.take());
        }
    }

    fn compile_terminfo(directory: &Path) -> Result<()> {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/terminfo/panelink-probe.terminfo");
        let status = Command::new("/usr/bin/tic")
            .args(["-x", "-o"])
            .arg(directory)
            .arg(source)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .status()
            .map_err(|_| "tic could not start")?;
        if status.success() {
            Ok(())
        } else {
            Err("private terminfo compilation failed")
        }
    }

    fn wait_pid(path: &Path, driver: &mut MultiServerDriver) -> Result<()> {
        let deadline = Instant::now() + IO_TIMEOUT;
        while !path.exists() {
            driver
                .tick(seconds)
                .map_err(|_| "owner tick waiting for pane PID failed")?;
            if Instant::now() >= deadline {
                return Err("pane PID marker timed out");
            }
            thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }

    struct SplitLaunch {
        pane: Pane,
        terminfo: PathBuf,
    }

    fn split_pane(
        driver: &mut MultiServerDriver,
        target: TargetRef,
        pane: &mut Pane,
        terminfo: &Path,
    ) -> Result<TargetRef> {
        let (server, socket) = UnixStream::pair().map_err(|_| "split socketpair failed")?;
        driver
            .attach(AuthenticatedStream::new(server).map_err(|_| "split server peer invalid")?)
            .map_err(|_| "split client refused")?;
        let launch = SplitLaunch {
            pane: Pane {
                kind: pane.kind,
                executable: pane.executable.clone(),
                arguments: pane.arguments.clone(),
                context: LaunchContext {
                    working_directory: pane.context.working_directory.clone(),
                    environment: pane.context.environment.clone(),
                },
                pid_file: pane.pid_file.clone(),
                ack_file: pane.ack_file.clone(),
                build_file: pane.build_file.clone(),
                build_failure_file: pane.build_failure_file.clone(),
                start_file: pane.start_file.clone(),
                pid: None,
                identity: None,
                startup_started: pane.startup_started,
                startup_us: None,
                marker: None,
                prompt_ready: false,
            },
            terminfo: terminfo.to_path_buf(),
        };
        let (tx, rx) = mpsc::sync_channel(1);
        let join = thread::spawn(move || {
            let _ = tx.send(split_client(socket, target, &launch));
        });
        let deadline = Instant::now() + IO_TIMEOUT;
        loop {
            match rx.try_recv() {
                Ok(result) => {
                    join.join().map_err(|_| "split client panicked")?;
                    let new_target = result?;
                    wait_pid(&pane.pid_file, driver)?;
                    pane.record_pid()?;
                    return Ok(new_target);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("split client disconnected before result")
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if Instant::now() >= deadline {
                return Err("split client timed out");
            }
            driver
                .tick(seconds)
                .map_err(|_| "owner tick during split failed")?;
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn split_client(
        socket: UnixStream,
        target0: TargetRef,
        launch: &SplitLaunch,
    ) -> Result<TargetRef> {
        let mut stream = AuthenticatedStream::new(socket).map_err(|_| "split peer invalid")?;
        let mut hello = encode_session_hello(&SessionHello {
            hello: Hello {
                nonce: 71,
                required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
                optional_capabilities: 0,
            },
            resume: None,
        })
        .map_err(|_| "split hello invalid")?;
        hello
            .extend_from_slice(&encode_attach_target(target0).map_err(|_| "split target invalid")?);
        stream
            .write_frame(
                &Frame::new(MessageKind::Hello, 0, &hello)
                    .map_err(|_| "split hello frame invalid")?,
                Instant::now() + IO_TIMEOUT,
            )
            .map_err(|_| "split hello write failed")?;
        let frame = stream
            .read_frame(Instant::now() + IO_TIMEOUT)
            .map_err(|_| "split welcome missing")?;
        if frame.kind() != MessageKind::Welcome {
            return Err("split welcome kind invalid");
        }
        let welcome = decode_welcome(frame.payload()).map_err(|_| "split welcome invalid")?;
        let mut request_id = 0;
        let describe = encode_session_request(&SessionRequest::Describe)
            .map_err(|_| "split describe invalid")?;
        let info = decode_session_info(request(&mut stream, &mut request_id, &describe)?.payload())
            .map_err(|_| "split session info invalid")?;
        let claim_key = OperationKey {
            generation: welcome.generation,
            lease: welcome.lease,
            sequence: 1,
        };
        let claim = encode_session_request(&SessionRequest::Claim {
            key: claim_key,
            target: target0,
            expected_owner_revision: info.owner_revision,
            ttl_seconds: 30,
        })
        .map_err(|_| "split claim invalid")?;
        let claimed = request(&mut stream, &mut request_id, &claim)?;
        if !matches!(
            decode_response_payload(claimed.payload())
                .map_err(|_| "split claim response invalid")?
                .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        ) {
            return Err("split owner claim failed");
        }
        let owner =
            decode_session_info(request(&mut stream, &mut request_id, &describe)?.payload())
                .map_err(|_| "split owner confirmation invalid")?;
        let key = OperationKey {
            generation: welcome.generation,
            lease: welcome.lease,
            sequence: 2,
        };
        let args = launch
            .pane
            .arguments
            .iter()
            .map(|arg| arg.as_bytes())
            .collect::<Vec<_>>();
        let env = environment_entries(&launch.pane.context.environment);
        let layout_reply = request(&mut stream, &mut request_id, &encode_layout_query())?;
        if layout_reply.kind() != MessageKind::Response {
            return Err("split layout query response invalid");
        }
        let decoded_layout = decode_layout_reply(layout_reply.payload())
            .map_err(|_| "split layout query decode failed")?;
        let (planned_rows, planned_columns) =
            planned_split_size(&decoded_layout, target0, SplitDirection::Down)
                .map_err(|_| "split launch size unavailable")?;
        let child = LaunchRequest {
            key,
            rows: planned_rows,
            columns: planned_columns,
            requested_executable: launch.pane.executable.as_os_str().as_bytes(),
            requested_working_directory: launch
                .pane
                .context
                .working_directory
                .as_os_str()
                .as_bytes(),
            client_working_directory: launch.pane.context.working_directory.as_os_str().as_bytes(),
            arguments: &args,
            environment: &env,
        };
        let split = SplitRequest {
            key,
            target: target0,
            expected_layout_revision: 1,
            direction: SplitDirection::Down,
            launch: &child,
        };
        let mut payload = Vec::new();
        encode_split_request(&split, &mut payload).map_err(|_| "split request invalid")?;
        let result =
            decode_split_result(request(&mut stream, &mut request_id, &payload)?.payload())
                .map_err(|_| "split response invalid")?;
        if result.outcome != Outcome::Applied {
            return Err("split operation was not applied");
        }
        let target1 = result.new_target.ok_or("split target missing")?;
        let release = encode_session_request(&SessionRequest::Release {
            key: OperationKey {
                generation: welcome.generation,
                lease: welcome.lease,
                sequence: 3,
            },
            target: target0,
            expected_owner_revision: owner.owner_revision,
        })
        .map_err(|_| "temporary owner release invalid")?;
        if !matches!(
            decode_response_payload(request(&mut stream, &mut request_id, &release)?.payload())
                .map_err(|_| "temporary release response invalid")?
                .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        ) {
            return Err("temporary owner release failed");
        }
        let _ = (
            &launch.terminfo,
            launch.pane.kind,
            launch.pane.ack_file.as_os_str(),
        );
        Ok(target1)
    }

    fn environment_entries(env: &Environment) -> Vec<EnvironmentEntry<'_>> {
        env.iter()
            .map(|(name, value)| EnvironmentEntry {
                name: name.as_bytes(),
                value: value.as_bytes(),
            })
            .collect()
    }

    fn wait_layout(
        driver: &mut MultiServerDriver,
        server: &Path,
        first: TargetRef,
        second: TargetRef,
    ) -> Result<()> {
        let deadline = Instant::now() + IO_TIMEOUT;
        loop {
            driver
                .tick(seconds)
                .map_err(|_| "owner tick waiting for layout failed")?;
            let durable = LayoutStore::open(&server.join("layout"))
                .ok()
                .and_then(|store| store.load(LayoutSlot::Current).ok().flatten())
                .is_some_and(|layout| {
                    layout.pane_count() == 2 && layout.panes()[..2] == [Some(first), Some(second)]
                });
            if durable {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("two-pane saved layout timed out");
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn client_loop(
        socket: UnixStream,
        target: TargetRef,
        pane: usize,
        kind: PaneKind,
        ack: PathBuf,
        commands: Receiver<ClientCommand>,
        ready: SyncSender<Result<()>>,
    ) {
        if let Err(error) = client_service(socket, target, pane, kind, ack, commands, ready) {
            eprintln!("workload control connection failed: {error}");
        }
    }

    fn client_service(
        socket: UnixStream,
        target: TargetRef,
        pane: usize,
        kind: PaneKind,
        _ack: PathBuf,
        commands: Receiver<ClientCommand>,
        ready: SyncSender<Result<()>>,
    ) -> Result<()> {
        let mut stream = AuthenticatedStream::new(socket).map_err(|_| "client peer invalid")?;
        let mut hello = encode_session_hello(&SessionHello {
            hello: Hello {
                nonce: 500 + pane as u64,
                required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
                optional_capabilities: 0,
            },
            resume: None,
        })
        .map_err(|_| "client hello invalid")?;
        hello
            .extend_from_slice(&encode_attach_target(target).map_err(|_| "client target invalid")?);
        stream
            .write_frame(
                &Frame::new(MessageKind::Hello, 0, &hello)
                    .map_err(|_| "client hello frame invalid")?,
                Instant::now() + IO_TIMEOUT,
            )
            .map_err(|_| "client hello write failed")?;
        let welcome_frame = stream
            .read_frame(Instant::now() + IO_TIMEOUT)
            .map_err(|_| "client welcome missing")?;
        if welcome_frame.kind() != MessageKind::Welcome {
            return Err("client welcome kind invalid");
        }
        let welcome =
            decode_welcome(welcome_frame.payload()).map_err(|_| "client welcome invalid")?;
        stream
            .enable_snapshot_reads()
            .map_err(|_| "snapshot read mode unavailable")?;
        let mut request_id = 0;
        let describe = encode_session_request(&SessionRequest::Describe)
            .map_err(|_| "client describe invalid")?;
        let info = decode_session_info(request(&mut stream, &mut request_id, &describe)?.payload())
            .map_err(|_| "client session info invalid")?;
        if info.target != target {
            return Err("client target mismatch");
        }
        let claim = encode_session_request(&SessionRequest::Claim {
            key: OperationKey {
                generation: welcome.generation,
                lease: welcome.lease,
                sequence: 1,
            },
            target,
            expected_owner_revision: info.owner_revision,
            ttl_seconds: 30,
        })
        .map_err(|_| "client input claim invalid")?;
        let response = request(&mut stream, &mut request_id, &claim)?;
        if !matches!(
            decode_response_payload(response.payload())
                .map_err(|_| "input claim response invalid")?
                .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        ) {
            return Err("input claim refused");
        }
        let owner =
            decode_session_info(request(&mut stream, &mut request_id, &describe)?.payload())
                .map_err(|_| "input owner confirmation invalid")?;
        if owner.owner_lease != welcome.lease || owner.target != target {
            return Err("input ownership not confirmed");
        }
        query_layout(&mut stream, &mut request_id)?;
        let _ = ready.send(Ok(()));
        let mut sequence = 2;
        loop {
            match commands.recv() {
                Ok(ClientCommand::Stop) | Err(_) => return Ok(()),
                Ok(ClientCommand::Keepalive) => query_layout(&mut stream, &mut request_id)?,
                Ok(ClientCommand::Screen(reply)) => {
                    let _ = reply.send(query_screen(&mut stream, &mut request_id, target));
                }
                Ok(ClientCommand::Probe {
                    index,
                    ack,
                    previous,
                    reply,
                }) => {
                    // Parent fix: the 30 s owner lease (MAX_OWNER_TTL_SECS)
                    // can expire during slow OpenCode startup, after which
                    // the server rejects input as StaleOwner. Re-claim with a
                    // fresh key before each probe (a claim is idempotent
                    // ownership, not replayed input).
                    let current = decode_session_info(
                        request(&mut stream, &mut request_id, &describe)?.payload(),
                    )
                    .map_err(|_| "owner refresh invalid")?;
                    let owner_revision = if current.owner_lease == welcome.lease {
                        current.owner_revision
                    } else {
                        let reclaim = encode_session_request(&SessionRequest::Claim {
                            key: OperationKey {
                                generation: welcome.generation,
                                lease: welcome.lease,
                                sequence,
                            },
                            target,
                            expected_owner_revision: current.owner_revision,
                            ttl_seconds: 30,
                        })
                        .map_err(|_| "input reclaim invalid")?;
                        sequence = sequence.checked_add(1).ok_or("sequence exhausted")?;
                        let reply = request(&mut stream, &mut request_id, &reclaim)?;
                        if !matches!(
                            decode_response_payload(reply.payload())
                                .map_err(|_| "input reclaim response invalid")?
                                .body,
                            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
                        ) {
                            return Err("input reclaim refused");
                        }
                        let refreshed = decode_session_info(
                            request(&mut stream, &mut request_id, &describe)?.payload(),
                        )
                        .map_err(|_| "owner refresh invalid")?;
                        if refreshed.owner_lease != welcome.lease {
                            return Err("input reclaim not confirmed");
                        }
                        refreshed.owner_revision
                    };
                    let mut result = match kind {
                        PaneKind::OpenCode => opencode_input(
                            &mut stream,
                            &mut request_id,
                            target,
                            welcome.generation,
                            welcome.lease,
                            owner_revision,
                            &mut sequence,
                            index,
                        ),
                        PaneKind::Shell | PaneKind::BuildShell => shell_input(
                            &mut stream,
                            &mut request_id,
                            target,
                            welcome.generation,
                            welcome.lease,
                            owner_revision,
                            &mut sequence,
                            index,
                            &ack,
                            previous,
                        ),
                    };
                    if let Ok(value) = &mut result {
                        value.kind = kind;
                    }
                    let failed = result.is_err();
                    if let Err(reason) = &result {
                        eprintln!(
                            "workload probe failure: pane={pane} kind={} reason={reason}",
                            kind.label()
                        );
                    }
                    let _ = reply.send(result);
                    if failed {
                        return Err("input probe failed");
                    }
                }
            }
        }
    }

    fn query_layout(stream: &mut AuthenticatedStream, request_id: &mut u64) -> Result<()> {
        let reply = request(stream, request_id, &encode_layout_query())?;
        if reply.kind() != MessageKind::Response || decode_layout_reply(reply.payload()).is_err() {
            return Err("layout keepalive response invalid");
        }
        Ok(())
    }

    fn query_screen(
        stream: &mut AuthenticatedStream,
        request_id: &mut u64,
        target: TargetRef,
    ) -> Result<Screen> {
        let reply = request(
            stream,
            request_id,
            &encode_screen_query(target).map_err(|_| "screen query encoding failed")?,
        )?;
        if reply.kind() != MessageKind::Snapshot {
            let label = match reply.payload() {
                b"invalid_screen" => "invalid_screen",
                b"unknown_context" => "unknown_context",
                b"lease_expired" => "lease_expired",
                b"unauthorized" => "unauthorized",
                b"target_stale" => "target_stale",
                b"capability_denied" => "capability_denied",
                b"internal_error" => "internal_error",
                b"invalid_request" => "invalid_request",
                _ => "unclassified_fixed_error",
            };
            eprintln!(
                "workload: screen reply kind {:?}, label {label}, fixed response {:?}",
                reply.kind(),
                decode_response_payload(reply.payload()).map(|reply| reply.body)
            );
            return Err("screen query response kind invalid");
        }
        let view =
            decode_screen_reply(reply.payload()).map_err(|_| "screen query decode failed")?;
        if view.target() != target {
            return Err("screen query target mismatch");
        }
        let screen = screen_checkpoint::decode(view.checkpoint())
            .map_err(|_| "screen checkpoint decode failed")?;
        if screen.rows() != 24 || screen.columns() != 100 {
            return Err("screen content geometry does not match workload");
        }
        let mut text = String::new();
        for row in 0..screen.rows() {
            for col in 0..screen.columns() {
                if let Some(cell) = screen.cell(row, col) {
                    text.push_str(cell.text());
                }
            }
            text.push('\n');
        }
        let lower = text.to_ascii_lowercase();
        let marker = if lower.contains("opencode") {
            Some("opencode")
        } else if lower.contains("ask anything") {
            Some("ask anything")
        } else {
            None
        };
        let prompt = ["ask anything", "type a message", "send a message"]
            .iter()
            .any(|needle| lower.contains(needle));
        Ok(Screen {
            text,
            marker,
            prompt,
        })
    }

    fn input_keys(
        stream: &mut AuthenticatedStream,
        request_id: &mut u64,
        target: TargetRef,
        generation: u64,
        lease: u64,
        revision: u64,
        sequence: &mut u64,
        keys: &[Key],
    ) -> Result<()> {
        let key = OperationKey {
            generation,
            lease,
            sequence: *sequence,
        };
        *sequence = sequence.checked_add(1).ok_or("input sequence exhausted")?;
        let payload = encode_key_input(&KeyInput {
            key,
            target,
            expected_owner_revision: revision,
            keys,
        })
        .map_err(|_| "typed input encoding failed")?;
        // Input is nonreplayable. Any failed request ends this probe; it is never resent.
        let reply = request(stream, request_id, &payload)?;
        if reply.kind() != MessageKind::Response {
            return Err("typed input was not acknowledged");
        }
        let body = decode_response_payload(reply.payload())
            .map_err(|_| "input response decode failed")?
            .body;
        if !matches!(
            body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        ) {
            return Err("typed input was not acknowledged");
        }
        Ok(())
    }

    fn opencode_input(
        stream: &mut AuthenticatedStream,
        request_id: &mut u64,
        target: TargetRef,
        generation: u64,
        lease: u64,
        revision: u64,
        sequence: &mut u64,
        probe: usize,
    ) -> Result<InputResult> {
        let before = query_screen(stream, request_id, target)?;
        if !before.prompt {
            return Err("OpenCode composer marker absent; withheld input");
        }
        let count = before.text.matches('z').count();
        let start = Instant::now();
        input_keys(
            stream,
            request_id,
            target,
            generation,
            lease,
            revision,
            sequence,
            &[Key::Char('z')],
        )?;
        let mut echo = None;
        // Ten real TUIs on one host can delay a redraw beyond 3 s.
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if query_screen(stream, request_id, target)?
                .text
                .matches('z')
                .count()
                > count
            {
                echo = Some(micros(start.elapsed()));
                break;
            }
            thread::sleep(Duration::from_millis(40));
        }
        // The only TUI input is one printable character and Backspace. Never Enter.
        input_keys(
            stream,
            request_id,
            target,
            generation,
            lease,
            revision,
            sequence,
            &[Key::Backspace],
        )?;
        let erase_deadline = Instant::now() + Duration::from_secs(8);
        let mut erased = false;
        while Instant::now() < erase_deadline {
            if echo.is_some()
                && query_screen(stream, request_id, target)?
                    .text
                    .matches('z')
                    .count()
                    == count
            {
                erased = true;
                break;
            }
            thread::sleep(Duration::from_millis(40));
        }
        if !erased {
            return Err("OpenCode typed character did not echo and erase");
        }
        Ok(InputResult {
            kind: PaneKind::OpenCode,
            probe,
            echo_us: echo,
            round_trip_us: micros(start.elapsed()),
            erased,
        })
    }

    fn shell_input(
        stream: &mut AuthenticatedStream,
        request_id: &mut u64,
        target: TargetRef,
        generation: u64,
        lease: u64,
        revision: u64,
        sequence: &mut u64,
        probe: usize,
        ack: &Path,
        previous: u64,
    ) -> Result<InputResult> {
        let start = Instant::now();
        input_keys(
            stream,
            request_id,
            target,
            generation,
            lease,
            revision,
            sequence,
            &[Key::Char('q'), Key::Enter],
        )?;
        let deadline = Instant::now() + IO_TIMEOUT;
        while Instant::now() < deadline {
            if ack_value(ack).is_ok_and(|value| value > previous) {
                return Ok(InputResult {
                    kind: PaneKind::Shell,
                    probe,
                    echo_us: None,
                    round_trip_us: micros(start.elapsed()),
                    erased: true,
                });
            }
            thread::sleep(Duration::from_millis(10));
        }
        Err("shell input acknowledgment timed out")
    }

    fn request(
        stream: &mut AuthenticatedStream,
        request_id: &mut u64,
        payload: &[u8],
    ) -> Result<OwnedFrame> {
        *request_id = request_id
            .checked_add(1)
            .ok_or("control request ID exhausted")?;
        let frame = Frame::new(MessageKind::Request, *request_id, payload)
            .map_err(|_| "control request frame invalid")?;
        stream
            .write_frame(&frame, Instant::now() + IO_TIMEOUT)
            .map_err(|_| "control request write failed")?;
        let reply = stream
            .read_frame(Instant::now() + IO_TIMEOUT)
            .map_err(|_| "control response missing")?;
        if reply.request_id() != *request_id {
            return Err("control response ID mismatch");
        }
        Ok(reply)
    }

    fn counter_value(path: &Path) -> Option<u64> {
        fs::read_to_string(path)
            .ok()
            .and_then(|value| value.trim().parse().ok())
    }

    fn lock_history_for_inspection(
        path: &Path,
    ) -> io::Result<panelink::server_storage::MaintenanceLock> {
        let deadline = Instant::now() + IO_TIMEOUT;
        loop {
            match ServerStorage::lock_for_maintenance(path) {
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                result => return result,
            }
        }
    }

    fn ack_value(path: &Path) -> Result<u64> {
        match fs::read_to_string(path) {
            Ok(text) => text
                .trim()
                .parse()
                .map_err(|_| "shell acknowledgment marker invalid"),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
            Err(_) => Err("shell acknowledgment marker unreadable"),
        }
    }

    #[derive(Clone, Copy, Default)]
    struct Usage {
        rss: u64,
        cpu: f64,
        count: usize,
    }
    struct Sample {
        second: u64,
        panelink: Usage,
        opencode: Usage,
        shells: Usage,
        builds: Usage,
        log_lines: u64,
        build_cycles: u64,
    }

    fn ps_rows() -> Result<Vec<(u32, u32, u64, f64)>> {
        let output = Command::new("/bin/ps")
            .args(["-axo", "pid=,pgid=,rss=,%cpu="])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output()
            .map_err(|_| "ps failed to start")?;
        if !output.status.success() {
            return Err("ps returned failure");
        }
        String::from_utf8(output.stdout)
            .map_err(|_| "ps output not UTF-8")?
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let mut f = line.split_whitespace();
                let pid = f
                    .next()
                    .ok_or("ps PID missing")?
                    .parse()
                    .map_err(|_| "ps PID invalid")?;
                let pgid = f
                    .next()
                    .ok_or("ps PGID missing")?
                    .parse()
                    .map_err(|_| "ps PGID invalid")?;
                let rss = f
                    .next()
                    .ok_or("ps RSS missing")?
                    .parse()
                    .map_err(|_| "ps RSS invalid")?;
                let cpu = f
                    .next()
                    .ok_or("ps CPU missing")?
                    .parse()
                    .map_err(|_| "ps CPU invalid")?;
                Ok((pid, pgid, rss, cpu))
            })
            .collect()
    }

    fn sample(
        groups_opencode: &[u32],
        groups_shell: &[u32],
        groups_build: &[u32],
        second: u64,
        log_counter: Option<&Path>,
        build_counter: Option<&Path>,
    ) -> Result<Sample> {
        let mut panelink = Usage::default();
        let mut opencode = Usage::default();
        let mut shells = Usage::default();
        let mut builds = Usage::default();
        for (pid, pgid, rss, cpu) in ps_rows()? {
            let usage = if pid == process::id() {
                Some(&mut panelink)
            } else if groups_opencode.contains(&pgid) {
                Some(&mut opencode)
            } else if groups_build.contains(&pgid) {
                Some(&mut builds)
            } else if groups_shell.contains(&pgid) {
                Some(&mut shells)
            } else {
                None
            };
            if let Some(usage) = usage {
                usage.rss = usage.rss.saturating_add(rss.saturating_mul(1024));
                usage.cpu += cpu;
                usage.count += 1;
            }
        }
        if panelink.count != 1 {
            return Err("ps did not include workload process");
        }
        Ok(Sample {
            second,
            panelink,
            opencode,
            shells,
            builds,
            log_lines: log_counter.and_then(counter_value).unwrap_or(0),
            build_cycles: build_counter.and_then(counter_value).unwrap_or(0),
        })
    }

    fn group_alive(group: u32) -> Result<bool> {
        // Parent fix: a killed child stays a zombie (still listed by ps with
        // its pgid) until its parent, the PTY effect owned by the driver,
        // reaps it. Count only processes that are not zombies.
        let output = Command::new("/bin/ps")
            .args(["-axo", "pgid=,stat="])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output()
            .map_err(|_| "ps failed")?;
        if !output.status.success() {
            return Err("process-group inspection failed");
        }
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(text.lines().any(|line| {
            let mut fields = line.split_whitespace();
            let pgid = fields.next().and_then(|v| v.parse::<u32>().ok());
            let stat = fields.next().unwrap_or("");
            // 'Z' zombie and 'E' exiting (already signalled, awaiting
            // reap by the PTY owner) are not live leaks.
            pgid == Some(group) && !stat.contains('Z') && !stat.contains('E')
        }))
    }

    #[derive(Clone, Copy)]
    struct ProcessIdentity {
        pid: u32,
        birth_micros: u64,
        uid: u32,
    }

    impl ProcessIdentity {
        fn capture(pid: u32) -> Result<Self> {
            let pid_i32 = i32::try_from(pid).map_err(|_| "child PID is out of range")?;
            let info = process_info(pid_i32).ok_or("child process identity unavailable")?;
            if info.pbi_status == libc::SZOMB {
                return Err("child exited before identity capture");
            }
            Ok(Self {
                pid,
                birth_micros: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
                uid: info.pbi_uid,
            })
        }

        fn matches_current(self) -> Option<bool> {
            let info = process_info(i32::try_from(self.pid).ok()?)?;
            Some(
                info.pbi_uid == self.uid
                    && info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec
                        == self.birth_micros,
            )
        }
    }

    fn process_info(pid: i32) -> Option<libc::proc_bsdinfo> {
        // SAFETY: proc_bsdinfo is plain data and zero is a valid buffer state.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>();
        // SAFETY: the pointer and byte count exactly describe the writable result.
        let count = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                std::ptr::addr_of_mut!(info).cast(),
                i32::try_from(size).ok()?,
            )
        };
        (usize::try_from(count).ok() == Some(size)).then_some(info)
    }

    fn record_cleanup_result(result: Result<bool>) {
        match result {
            Ok(true) => {}
            Ok(false) => CLEANUP_FAILED.store(true, Ordering::SeqCst),
            Err(_) => {
                CLEANUP_FAILED.store(true, Ordering::SeqCst);
                CLEANUP_UNCERTAIN.store(true, Ordering::SeqCst);
            }
        }
    }

    fn stop_group(group: u32, identity: Option<ProcessIdentity>) -> Result<bool> {
        if !group_alive(group)? {
            return Ok(true);
        }
        let Some(identity) = identity else {
            CLEANUP_FAILED.store(true, Ordering::SeqCst);
            CLEANUP_UNCERTAIN.store(true, Ordering::SeqCst);
            return Ok(false);
        };
        for signal in ["TERM", "KILL"] {
            if !group_alive(group)? {
                return Ok(true);
            }
            match identity.matches_current() {
                Some(true) => {}
                Some(false) => {
                    CLEANUP_FAILED.store(true, Ordering::SeqCst);
                    CLEANUP_IDENTITY_CHANGED.store(true, Ordering::SeqCst);
                    return Ok(false);
                }
                None => {
                    CLEANUP_FAILED.store(true, Ordering::SeqCst);
                    CLEANUP_UNCERTAIN.store(true, Ordering::SeqCst);
                    return Ok(false);
                }
            }
            let signal_arg = format!("-{signal}");
            let group_arg = group.to_string();
            let _ = Command::new("/usr/bin/pkill")
                .args([signal_arg.as_str(), "-g", group_arg.as_str()])
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .status();
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if !group_alive(group)? {
                    return Ok(true);
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
        let stopped = !group_alive(group)?;
        if !stopped {
            CLEANUP_FAILED.store(true, Ordering::SeqCst);
        }
        Ok(stopped)
    }

    fn micros(duration: Duration) -> u64 {
        u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
    }
    fn seconds() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
    fn pct_u64(samples: &[u64], fraction: f64) -> Option<u64> {
        let mut v = samples.to_vec();
        if v.is_empty() {
            return None;
        }
        v.sort_unstable();
        Some(
            v[((fraction * v.len() as f64).ceil() as usize)
                .max(1)
                .min(v.len())
                - 1],
        )
    }
    fn pct_f64(samples: &[f64], fraction: f64) -> Option<f64> {
        let mut v = samples.to_vec();
        if v.is_empty() {
            return None;
        }
        v.sort_by(f64::total_cmp);
        Some(
            v[((fraction * v.len() as f64).ceil() as usize)
                .max(1)
                .min(v.len())
                - 1],
        )
    }
    fn num(v: Option<u64>) -> String {
        v.map_or_else(|| "null".into(), |n| n.to_string())
    }
    fn float(v: Option<f64>) -> String {
        v.map_or_else(|| "null".into(), |n| format!("{n:.3}"))
    }
    fn json_string(value: &str) -> String {
        format!(
            "\"{}\"",
            value
                .chars()
                .flat_map(char::escape_default)
                .collect::<String>()
        )
    }

    fn usage_summary(samples: &[Sample], get: fn(&Sample) -> Usage) -> String {
        let rss = samples.iter().map(|s| get(s).rss).collect::<Vec<_>>();
        let cpu = samples.iter().map(|s| get(s).cpu).collect::<Vec<_>>();
        format!("{{\"samples\":{},\"rss_min_bytes\":{},\"rss_p50_bytes\":{},\"rss_p95_bytes\":{},\"rss_max_bytes\":{},\"cpu_mean_percent\":{},\"cpu_p50_percent\":{},\"cpu_p95_percent\":{},\"cpu_max_percent\":{}}}", samples.len(), rss.iter().min().map_or_else(|| "null".into(), u64::to_string), num(pct_u64(&rss, 0.5)), num(pct_u64(&rss, 0.95)), rss.iter().max().map_or_else(|| "null".into(), u64::to_string), float(if cpu.is_empty() { None } else { Some(cpu.iter().sum::<f64>() / cpu.len() as f64) }), float(pct_f64(&cpu, 0.5)), float(pct_f64(&cpu, 0.95)), float(cpu.iter().copied().reduce(f64::max)))
    }

    fn workspace_json(w: &Workspace) -> String {
        let ticks = &w.ticks;
        let mut panes = Vec::new();
        for pane in &w.panes {
            panes.push(format!("{{\"kind\":{},\"pid\":{},\"startup_us\":{},\"stable_ui_marker\":{},\"prompt_ready\":{}}}", json_string(pane.kind.label()), pane.pid.map_or_else(|| "null".into(), |pid| pid.to_string()), num(pane.startup_us), pane.marker.map_or_else(|| "null".into(), json_string), pane.prompt_ready));
        }
        let opencode = w
            .inputs
            .iter()
            .filter(|i| i.kind == PaneKind::OpenCode)
            .collect::<Vec<_>>();
        let shell = w
            .inputs
            .iter()
            .filter(|i| i.kind.is_shell())
            .collect::<Vec<_>>();
        let echoes = opencode
            .iter()
            .filter_map(|i| i.echo_us)
            .collect::<Vec<_>>();
        let rts = opencode.iter().map(|i| i.round_trip_us).collect::<Vec<_>>();
        let shell_rts = shell.iter().map(|i| i.round_trip_us).collect::<Vec<_>>();
        let inputs = w.inputs.iter().map(|input| format!("{{\"probe\":{},\"kind\":{},\"echo_us\":{},\"round_trip_us\":{},\"erased_or_acknowledged\":{}}}", input.probe, json_string(input.kind.label()), num(input.echo_us), input.round_trip_us, input.erased)).collect::<Vec<_>>().join(",");
        format!("{{\"workspace\":{},\"owner_ticks\":{{\"count\":{},\"p50_us\":{},\"p95_us\":{},\"max_us\":{}}},\"panes\":[{}],\"input_samples\":[{}],\"opencode_input\":{{\"count\":{},\"echo_p50_us\":{},\"echo_p95_us\":{},\"echo_max_us\":{},\"round_trip_p50_us\":{},\"round_trip_p95_us\":{},\"round_trip_max_us\":{}}},\"shell_input_ack\":{{\"count\":{},\"p50_us\":{},\"p95_us\":{},\"max_us\":{}}},\"history_ok\":{},\"history_capture_failed\":{},\"history_inspection_complete\":{},\"history_targets\":[{}],\"history_diagnostics\":{},\"build_cycles_completed\":{},\"build_child_acknowledgements\":{},\"build_failed\":{},\"layout_ok\":{},\"shutdown_ms\":{},\"process_groups_stopped\":{}}}", w.index, ticks.len(), num(pct_u64(ticks, 0.5)), num(pct_u64(ticks, 0.95)), num(ticks.iter().copied().max()), panes.join(","), inputs, opencode.len(), num(pct_u64(&echoes, 0.5)), num(pct_u64(&echoes, 0.95)), num(echoes.iter().copied().max()), num(pct_u64(&rts, 0.5)), num(pct_u64(&rts, 0.95)), num(rts.iter().copied().max()), shell.len(), num(pct_u64(&shell_rts, 0.5)), num(pct_u64(&shell_rts, 0.95)), num(shell_rts.iter().copied().max()), w.history_ok, w.history_capture_failed, w.history_inspection_complete, w.history_targets_json, w.history_diagnostic_json, w.build_cycles, w.build_cycles, w.build_failed, w.layout_ok, w.shutdown_ms.map_or_else(|| "null".into(), |n| n.to_string()), w.groups_stopped)
    }

    fn private_json(path: &Path, bytes: &[u8]) -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut file = options
            .open(path)
            .map_err(|_| "raw JSON output could not be created")?;
        file.write_all(bytes)
            .map_err(|_| "raw JSON output write failed")?;
        file.sync_all().map_err(|_| "raw JSON output sync failed")
    }

    fn run(variant: Variant, run_number: u8, output: &Path) -> Result<()> {
        eprintln!("workload: resolving launcher");
        let launcher = Launcher::resolve()?;
        eprintln!("workload: using {} ({})", launcher.label, launcher.version);
        let root = PrivateRoot::create()?;
        if let Ok(mut fixture_root) = FIXTURE_ROOT.lock() {
            *fixture_root = Some(root.0.clone());
        }
        let rustc = if variant == Variant::C {
            Some(resolve_rustc()?)
        } else {
            None
        };
        let mut workspaces = Vec::with_capacity(variant.workspace_count());
        for index in 0..variant.workspace_count() {
            eprintln!(
                "workload: starting workspace {}/{}",
                index + 1,
                variant.workspace_count()
            );
            workspaces.push(Workspace::create(
                index,
                root.0.join(format!("workspace-{index}")),
                variant,
                &launcher,
                rustc.as_deref().unwrap_or(Path::new("/usr/bin/rustc")),
            )?);
        }
        for workspace in &mut workspaces {
            workspace.connect()?;
        }
        eprintln!("workload: waiting for input claims");
        let ready_deadline = Instant::now() + IO_TIMEOUT;
        loop {
            let mut all_ready = true;
            for workspace in &mut workspaces {
                workspace.tick(false)?;
                all_ready &= workspace.ready()?;
            }
            if all_ready {
                break;
            }
            if Instant::now() >= ready_deadline {
                return Err("input claim handshakes timed out");
            }
            thread::sleep(Duration::from_millis(2));
        }
        // Require the visible OpenCode brand plus a composer label before input.
        let expected_agents = variant.opencode_count();
        let screen_deadline = Instant::now() + Duration::from_secs(60);
        let mut shell_ready = 0;
        // Parent fix: at most ONE outstanding screen query per pane. Issuing a
        // fresh blocking query every loop while the previous one was still
        // queued filled the size-1 command channel and deadlocked the owner
        // loop (observed: 11+ minutes at 0% CPU before output).
        let mut in_flight: Vec<Vec<Option<Receiver<Result<Screen>>>>> = (0..workspaces.len())
            .map(|_| (0..PANES).map(|_| None).collect())
            .collect();
        loop {
            let mut any_not_ready = false;
            for (workspace, item) in workspaces.iter().enumerate() {
                for pane in 0..PANES {
                    let launch = &item.panes[pane];
                    let not_ready = if launch.kind == PaneKind::OpenCode {
                        launch.marker.is_none() || !launch.prompt_ready
                    } else {
                        launch.startup_us.is_none()
                    };
                    if not_ready {
                        any_not_ready = true;
                        if in_flight[workspace][pane].is_none() {
                            in_flight[workspace][pane] = Some(item.clients[pane].screen()?);
                        }
                    }
                }
            }
            if !any_not_ready {
                break;
            }
            for workspace in &mut workspaces {
                workspace.tick(false)?;
                // Parent fix: keep idle connections alive during the up-to-60 s
                // UI-readiness wait (server drops idle clients after ~1 s).
                let now = Instant::now();
                if now >= workspace.next_keepalive {
                    for client in &workspace.clients {
                        client.send(ClientCommand::Keepalive)?;
                    }
                    workspace.next_keepalive = now + KEEPALIVE_INTERVAL;
                }
            }
            let mut pending = Vec::new();
            for (workspace, row) in in_flight.iter_mut().enumerate() {
                for (pane, slot) in row.iter_mut().enumerate() {
                    if let Some(receiver) = slot.take() {
                        pending.push((workspace, pane, receiver));
                    }
                }
            }
            for (workspace, pane, receiver) in pending {
                match receiver.try_recv() {
                    Ok(Ok(screen)) => {
                        let launch = &mut workspaces[workspace].panes[pane];
                        if launch.kind == PaneKind::OpenCode {
                            if screen.marker.is_some() {
                                launch.marker = screen.marker;
                            }
                            launch.prompt_ready |= screen.prompt;
                            if launch.marker.is_some()
                                && launch.prompt_ready
                                && launch.startup_us.is_none()
                            {
                                launch.startup_us = Some(micros(launch.startup_started.elapsed()));
                            }
                        } else if screen.text.contains("PANELINK-SHELL-READY")
                            && launch.startup_us.is_none()
                        {
                            launch.startup_us = Some(micros(launch.startup_started.elapsed()));
                            shell_ready += 1;
                        }
                    }
                    Ok(Err(reason)) => {
                        eprintln!("workload: startup screen query failed for workspace {workspace} pane {pane}: {reason}");
                        return Err("startup screen query failed");
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err("startup screen query disconnected")
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        in_flight[workspace][pane] = Some(receiver);
                    }
                }
            }
            if Instant::now() >= screen_deadline {
                return Err("stable UI startup marker timed out");
            }
            thread::sleep(Duration::from_millis(50));
        }
        let ready_agents = workspaces
            .iter()
            .flat_map(|w| w.panes.iter())
            .filter(|p| p.kind == PaneKind::OpenCode && p.marker.is_some() && p.prompt_ready)
            .count();
        if ready_agents != expected_agents {
            return Err("OpenCode marker/composer not detected on every agent");
        }
        if shell_ready != variant.workspace_count() * PANES - expected_agents {
            return Err("shell startup marker missing");
        }
        eprintln!("workload: UI ready ({ready_agents} OpenCode, {shell_ready} shells); workload gate prepared");
        let mut samples = Vec::with_capacity(MEASURE_SECONDS as usize);
        let agent_groups = workspaces
            .iter()
            .flat_map(|w| w.panes.iter())
            .filter(|p| p.kind == PaneKind::OpenCode)
            .filter_map(|p| p.pid)
            .collect::<Vec<_>>();
        let shell_groups = workspaces
            .iter()
            .flat_map(|w| w.panes.iter())
            .filter(|p| p.kind == PaneKind::Shell)
            .filter_map(|p| p.pid)
            .collect::<Vec<_>>();
        let build_groups = workspaces
            .iter()
            .flat_map(|w| w.panes.iter())
            .filter(|p| p.kind.is_build())
            .filter_map(|p| p.pid)
            .collect::<Vec<_>>();
        let log_counter = workspaces
            .iter()
            .flat_map(|workspace| workspace.panes.iter())
            .find(|pane| pane.kind == PaneKind::Shell)
            .map(|pane| pane.build_file.clone());
        let build_counter = workspaces
            .iter()
            .flat_map(|workspace| workspace.panes.iter())
            .find(|pane| pane.kind.is_build())
            .map(|pane| pane.build_file.clone());
        let start = Instant::now();
        let stop = start + Duration::from_secs(MEASURE_SECONDS);
        for pane in workspaces
            .iter()
            .flat_map(|workspace| workspace.panes.iter())
            .filter(|pane| pane.kind.is_shell())
        {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&pane.start_file)
                .map_err(|_| "workload gate could not start shell producers")?;
        }
        eprintln!("workload: measurement gate opened for {MEASURE_SECONDS}s");
        for workspace in &mut workspaces {
            // Parent fix: do not postpone the next keepalive by a full interval
            // after the readiness phase; the server drops idle clients ~1 s.
            workspace.next_keepalive = start;
            workspace.next_input = start + Duration::from_secs(5);
        }
        let mut next_sample = start;
        while Instant::now() < stop {
            let now = Instant::now();
            for workspace in &mut workspaces {
                workspace.advance(now, stop)?;
            }
            if now >= next_sample {
                samples.push(sample(
                    &agent_groups,
                    &shell_groups,
                    &build_groups,
                    micros(now.duration_since(start)) / 1000,
                    log_counter.as_deref(),
                    build_counter.as_deref(),
                )?);
                next_sample += SAMPLE_INTERVAL;
            }
            thread::sleep(Duration::from_millis(2));
        }
        let measured_log_lines = log_counter.as_deref().and_then(counter_value).unwrap_or(0);
        let measured_build_cycles = build_counter
            .as_deref()
            .and_then(counter_value)
            .unwrap_or(0);
        for pane in workspaces
            .iter()
            .flat_map(|workspace| workspace.panes.iter())
            .filter(|pane| pane.kind.is_shell())
        {
            fs::remove_file(&pane.start_file)
                .map_err(|_| "workload output gate could not close")?;
        }
        for workspace in &mut workspaces {
            workspace.shutdown()?;
        }
        eprintln!("workload: child process groups stopped; verifying storage results");
        let groups_stopped = workspaces.iter().all(|w| w.groups_stopped);
        if !groups_stopped {
            return Err("child process group remained after shutdown");
        }
        let open_code_count = expected_agents;
        let shell_count = variant.workspace_count() * PANES - open_code_count;
        let build_cycles = workspaces
            .iter()
            .map(|workspace| workspace.build_cycles)
            .sum::<u64>();
        let private_pane_directories_distinct = {
            let paths = workspaces
                .iter()
                .flat_map(|workspace| workspace.panes.iter())
                .map(|pane| pane.pid_file.clone())
                .collect::<Vec<_>>();
            paths.iter().all(|path| path.exists())
                && paths
                    .iter()
                    .map(|path| path.parent().map(Path::to_path_buf))
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    == paths.len()
        };
        let history_ok = workspaces.iter().all(|workspace| workspace.history_ok);
        let history_sticky_failure = workspaces
            .iter()
            .any(|workspace| workspace.history_capture_failed);
        let layout_ok = workspaces.iter().all(|workspace| workspace.layout_ok);
        let build_failed = workspaces.iter().any(|workspace| workspace.build_failed);
        let profile = match variant { Variant::A => "five workspaces: five idle OpenCode plus five paced shell", Variant::B => "five workspaces: ten idle OpenCode", Variant::C => "five workspaces: ten idle OpenCode; sixth workspace: paced log shell plus dependency-free rustc build shell" };
        let result = format!(
            "{{\"schema_version\":2,\"variant\":{},\"run\":{},\"synthetic_shell_output\":{},\"real_opencode\":true,\"spec_r03_complete\":false,\"acceptance_budget_status\":\"unset\",\"profile\":{},\"launcher_used\":{},\"launcher_executable\":{},\"opencode_version\":{},\"credential_policy\":\"fresh isolated 0700 HOME/XDG/tmp/project per pane; no auth/config copied\",\"prompt_submissions\":0,\"model_calls_requested\":0,\"opencode_sessions\":{},\"shell_processes\":{},\"workspace_count\":{},\"pane_count\":{},\"measurement_seconds\":60,\"root_rows\":53,\"root_columns\":102,\"ready_opencode_tuis\":{},\"probe_output_overlap_instrumented\":false,\"paced_log_lines_at_measurement_end\":{},\"build_cycles_at_measurement_end\":{},\"build_cycles_completed\":{},\"build_child_acknowledgements\":{},\"private_pane_directories_distinct\":{},\"correctness_checks\":{{\"history_rows_and_normal_screens\":{},\"history_sticky_failure\":{},\"saved_layout_matches_targets\":{},\"build_cycle_observed\":{},\"build_failed\":{},\"private_pane_directories_distinct\":{},\"process_groups_stopped\":{}}},\"resource_summary\":{{\"panelink_process\":{},\"opencode_children\":{},\"shell_output_process_groups\":{},\"shell_build_process_groups\":{}}},\"process_samples\":[{}],\"process_groups_stopped\":{},\"results\":[{}]}}",
            json_string(variant.name()), run_number, shell_count != 0, json_string(profile), json_string(launcher.label), json_string(&launcher.executable.to_string_lossy()), json_string(&launcher.version), open_code_count, shell_count, workspaces.len(), workspaces.len() * PANES, ready_agents, measured_log_lines, measured_build_cycles, build_cycles, build_cycles, private_pane_directories_distinct, history_ok, history_sticky_failure, layout_ok, variant != Variant::C || measured_build_cycles > 0, build_failed, private_pane_directories_distinct, groups_stopped,
            usage_summary(&samples, |s| s.panelink), usage_summary(&samples, |s| s.opencode), usage_summary(&samples, |s| s.shells), usage_summary(&samples, |s| s.builds),
            samples.iter().map(|s| format!("{{\"elapsed_ms\":{},\"panelink_rss_bytes\":{},\"panelink_cpu_percent\":{:.2},\"panelink_process_count\":{},\"opencode_rss_bytes\":{},\"opencode_cpu_percent\":{:.2},\"opencode_process_count\":{},\"shell_output_rss_bytes\":{},\"shell_output_cpu_percent\":{:.2},\"shell_output_process_count\":{},\"shell_build_rss_bytes\":{},\"shell_build_cpu_percent\":{:.2},\"shell_build_process_count\":{},\"paced_log_lines\":{},\"build_cycles_completed\":{}}}", s.second, s.panelink.rss, s.panelink.cpu, s.panelink.count, s.opencode.rss, s.opencode.cpu, s.opencode.count, s.shells.rss, s.shells.cpu, s.shells.count, s.builds.rss, s.builds.cpu, s.builds.count, s.log_lines, s.build_cycles)).collect::<Vec<_>>().join(","), groups_stopped, workspaces.iter().map(workspace_json).collect::<Vec<_>>().join(",")
        );
        private_json(output, result.as_bytes())?;
        eprintln!("workload: raw result written to {}", output.display());
        println!("{result}");
        if !groups_stopped {
            return Err("child process group remained; fixture retained");
        }
        fs::remove_dir_all(&root.0).map_err(|_| "fixture cleanup failed")?;
        if let Ok(mut fixture_root) = FIXTURE_ROOT.lock() {
            *fixture_root = None;
        }
        if !history_ok
            || history_sticky_failure
            || !layout_ok
            || (variant == Variant::C && (measured_build_cycles == 0 || build_failed))
            || !private_pane_directories_distinct
        {
            return Err("correctness check failed; schema-2 report retained");
        }
        Ok(())
    }

    pub fn main() -> Result<()> {
        let args = std::env::args().skip(1).collect::<Vec<_>>();
        if args.len() != 6 || args[0] != "--variant" || args[2] != "--run" || args[4] != "--output"
        {
            return Err("usage: --variant A|B|C --run N --output PATH");
        }
        let variant = Variant::parse(&args[1])?;
        let run_number = args[3]
            .parse::<u8>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or("run number must be positive")?;
        let output = PathBuf::from(&args[5]);
        if let Err(reason) = run(variant, run_number, &output) {
            let cleanup_failed = CLEANUP_FAILED.load(Ordering::SeqCst);
            let cleanup_status = if CLEANUP_UNCERTAIN.load(Ordering::SeqCst) {
                "uncertain_identity_no_signal"
            } else if CLEANUP_IDENTITY_CHANGED.load(Ordering::SeqCst) {
                "leader_identity_mismatch_group_skipped"
            } else if cleanup_failed {
                "cleanup_failed"
            } else {
                "no_cleanup_failure"
            };
            let retained_root = FIXTURE_ROOT
                .lock()
                .ok()
                .and_then(|root| {
                    root.as_ref()
                        .map(|path| json_string(&path.to_string_lossy()))
                })
                .unwrap_or_else(|| "null".into());
            let diagnostic = format!(
                "{{\"variant\":{},\"run\":{},\"status\":\"blocked\",\"reason\":{},\"cleanup_failed\":{},\"cleanup_status\":{},\"fixture_retained\":{},\"retained_root\":{}}}",
                json_string(variant.name()),
                run_number,
                json_string(reason),
                cleanup_failed,
                json_string(cleanup_status),
                retained_root != "null",
                retained_root
            );
            if !output.exists() {
                let _ = private_json(&output, diagnostic.as_bytes());
            }
            return Err(reason);
        }
        Ok(())
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn main() {
    if let Err(error) = workload::main() {
        eprintln!("OpenCode workload failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn main() {
    eprintln!("OpenCode workload requires macOS aarch64");
    std::process::exit(2);
}
