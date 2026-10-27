#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Real-pathname-server crash recovery acceptance. The server is killed with
//! SIGKILL only after its committed layout/history files have been decoded.
//! This is process-death evidence, not a physical reboot or power-loss test.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use panelink::control_protocol::TargetRef;
use panelink::history_rows;
use panelink::history_store::HistoryStore;
use panelink::saved_layout;
use panelink::screen_checkpoint;

const DEADLINE: Duration = Duration::from_secs(20);
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    state: PathBuf,
    server: PathBuf,
    terminfo: PathBuf,
    env_dir: PathBuf,
    home: PathBuf,
    cwd: PathBuf,
    relaunch_shell: PathBuf,
    starts: PathBuf,
    input_effect: PathBuf,
    replay_effect: PathBuf,
    replay_probe_done: PathBuf,
    input_canary: String,
    normal_row: String,
    alternate_a: String,
    alternate_b: String,
    final_normal: String,
    replacement: Cell<Option<ProcessIdentity>>,
}

impl Fixture {
    fn new() -> io::Result<Self> {
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = Path::new("/private/tmp").join(format!(
            "panelink-p5-crash-{}-{sequence}",
            std::process::id()
        ));
        private_dir(&root)?;
        let fixture = Self {
            state: root.join("state"),
            server: root.join("state/server"),
            terminfo: root.join("terminfo"),
            env_dir: root.join("env"),
            home: root.join("home"),
            cwd: root.join("cwd"),
            relaunch_shell: root.join("relaunch-shell"),
            starts: root.join("shell-starts"),
            input_effect: root.join("input-effects"),
            replay_effect: root.join("replayed-input"),
            replay_probe_done: root.join("replay-probe-done"),
            input_canary: format!("P5-CRASH-INPUT-{}-{sequence}", std::process::id()),
            normal_row: format!("P5-CRASH-NORMAL-ROW-{}-{sequence}", std::process::id()),
            alternate_a: format!("P5-CRASH-ALT-A-{}-{sequence}", std::process::id()),
            alternate_b: format!("P5-CRASH-ALT-B-{}-{sequence}", std::process::id()),
            final_normal: format!("P5-CRASH-FINAL-NORMAL-{}-{sequence}", std::process::id()),
            replacement: Cell::new(None),
            root,
        };
        for path in [
            &fixture.state,
            &fixture.home,
            &fixture.cwd,
            &fixture.terminfo,
            &fixture.env_dir,
            &fixture.server,
        ] {
            private_dir(path)?;
        }
        let shell = format!(
            "#!/bin/sh\nprintf '%s|%s|%s|%s\\n' \"$PWD\" \"$HOME\" \"$PANELINK_GENERATION\" \"$P5_CRASH_TEST_CANARY\" >> '{}'\nIFS= read -r -t 2 replayed\nif [ \"$replayed\" = '{}' ]; then printf 'replayed\\n' >> '{}'; fi\nprintf 'done\\n' >> '{}'\nexec /bin/sleep 60\n",
            fixture.starts.display(),
            fixture.input_canary,
            fixture.replay_effect.display(),
            fixture.replay_probe_done.display()
        );
        fs::write(&fixture.relaunch_shell, shell)?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fixture.relaunch_shell, fs::Permissions::from_mode(0o700))?;
        Ok(fixture)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_panelink"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("ZDOTDIR", &self.home)
            .env("SHELL", &self.relaunch_shell)
            .env("TERM", "panelink-probe")
            .env("TERMINFO", &self.terminfo)
            .env("TMPDIR", &self.env_dir)
            .env("XDG_CONFIG_HOME", &self.env_dir)
            .env("PANELINK_HOME", &self.state)
            .env("P5_CRASH_TEST_CANARY", "private-environment")
            .current_dir(&self.cwd)
            .stdin(Stdio::null());
        command
    }

    fn compile_terminfo(&self) {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/terminfo/panelink-probe.terminfo");
        let status = Command::new("/usr/bin/tic")
            .args([OsStr::new("-x"), OsStr::new("-o")])
            .arg(&self.terminfo)
            .arg(source)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&self.cwd)
            .status()
            .expect("compile fixture terminfo with /usr/bin/tic");
        assert!(status.success(), "private terminfo compilation failed");
        assert!(
            self.terminfo.join("p/panelink-probe").is_file()
                || self.terminfo.join("70/panelink-probe").is_file()
        );
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = self.command();
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        run_bounded(command, DEADLINE)
            .unwrap_or_else(|| panic!("panelink {args:?} exceeded {DEADLINE:?}"))
    }

    fn control(&self, args: &[&str]) -> (i32, String) {
        let mut command = self.command();
        command
            .args(["control", "--directory"])
            .arg(&self.server)
            .arg("--context")
            .arg(self.root.join("control.ctx"))
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = run_bounded(command, DEADLINE)
            .unwrap_or_else(|| panic!("control {args:?} exceeded {DEADLINE:?}"));
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    }

    fn replacement_identity(&self) -> Option<ProcessIdentity> {
        let record = fs::read_to_string(self.state.join("server.pid")).ok()?;
        let mut fields = record.split_whitespace();
        let pid: i32 = fields.next()?.parse().ok()?;
        let start: u64 = fields.next()?.parse().ok()?;
        if pid <= 0 || fields.next().is_some() {
            return None;
        }
        let identity = ProcessIdentity { pid, start };
        identity.matches_live().then_some(identity)
    }

    fn stop_recovery_server(&self) -> bool {
        // Also capture a server started by a relaunch CLI that failed before
        // returning its result. This record is private to this fresh fixture;
        // its PID, kernel birth time and UID must all agree before signalling.
        let identity = self
            .replacement
            .get()
            .or_else(|| self.replacement_identity());
        let Some(identity) = identity else {
            return !self.state.join("server.pid").exists();
        };
        if identity.has_exited() {
            return true;
        }
        let mut command = self.command();
        command
            .arg("stop")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let _ = run_bounded(command, Duration::from_secs(5));
        if wait_for_exit(identity, Duration::from_secs(2)) {
            return true;
        }
        if identity.matches_live() {
            // SAFETY: identity was checked against UID and kernel birth time.
            unsafe { libc::kill(identity.pid, libc::SIGKILL) };
        }
        wait_for_exit(identity, Duration::from_secs(2))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.stop_recovery_server() {
            let _ = fs::remove_dir_all(&self.root);
        } else {
            eprintln!("crash-recovery fixture retained: server exit could not be confirmed");
        }
    }
}

#[derive(Clone, Copy)]
struct ProcessIdentity {
    pid: i32,
    start: u64,
}

impl ProcessIdentity {
    fn matches_live(self) -> bool {
        process_info(self.pid).is_some_and(|info| {
            // SAFETY: geteuid reads the current effective UID.
            info.pbi_uid == unsafe { libc::geteuid() }
                && info.pbi_status != libc::SZOMB
                && info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec == self.start
        })
    }

    fn has_exited(self) -> bool {
        if let Some(info) = process_info(self.pid) {
            return info.pbi_status == libc::SZOMB
                || info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec != self.start;
        }
        // Probe failure is not exit evidence unless the kernel reports ESRCH.
        // SAFETY: signal zero only probes this previously captured identity.
        (unsafe { libc::kill(self.pid, 0) }) == -1
            && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
}

fn wait_for_exit(identity: ProcessIdentity, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if identity.has_exited() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

struct Server {
    child: Child,
    lines: mpsc::Receiver<String>,
    start_time: u64,
}

impl Server {
    fn start(fixture: &Fixture, script: &str) -> Self {
        let mut command = fixture.command();
        command
            .args(["serve", "--directory"])
            .arg(&fixture.server)
            .args(["--terminfo"])
            .arg(&fixture.terminfo)
            .args([
                "--initialize",
                "--allow-input",
                "--",
                "/bin/sh",
                "-c",
                script,
            ])
            .arg("panelink-p5-crash-fixture")
            .arg(&fixture.input_canary)
            .arg(&fixture.input_effect)
            .arg(&fixture.normal_row)
            .arg(&fixture.alternate_a)
            .arg(&fixture.alternate_b)
            .arg(&fixture.final_normal)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().expect("spawn private pathname server");
        let pid = i32::try_from(child.id()).expect("owned server PID fits pid_t");
        let info = process_info(pid).expect("capture owned server process identity");
        // SAFETY: geteuid reads the current effective UID.
        assert_eq!(info.pbi_uid, unsafe { libc::geteuid() });
        let start_time = info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec;
        let stdout = child.stdout.take().expect("server startup output pipe");
        Self {
            child,
            lines: line_reader(stdout),
            start_time,
        }
    }

    fn startup(&self) -> String {
        self.lines
            .recv_timeout(DEADLINE)
            .expect("pathname server startup line before deadline")
    }

    fn verified_pid(&self) -> i32 {
        let pid = i32::try_from(self.child.id()).expect("owned server PID fits pid_t");
        assert_eq!(pid, i32::try_from(self.child.id()).unwrap());
        let info = process_info(pid).expect("live server kernel process identity");
        // SAFETY: geteuid reads the current effective UID.
        assert_eq!(info.pbi_uid, unsafe { libc::geteuid() });
        assert_ne!(info.pbi_status, libc::SZOMB);
        let actual_start = info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec;
        assert_eq!(
            self.start_time, actual_start,
            "owned server PID start time must match"
        );
        pid
    }

    fn kill_sigkill_and_reap(&mut self) -> i32 {
        let pid = self.verified_pid();
        // SAFETY: pid was matched to this test's unreaped Child, UID and
        // kernel start time immediately before signalling.
        let result = unsafe { libc::kill(pid, libc::SIGKILL) };
        assert_eq!(
            result,
            0,
            "SIGKILL exact fixture server: {}",
            io::Error::last_os_error()
        );
        let deadline = Instant::now() + DEADLINE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => assert!(Instant::now() < deadline, "SIGKILLed server did not exit"),
                Err(error) => panic!("poll SIGKILLed server: {error}"),
            }
            thread::sleep(Duration::from_millis(10));
        }
        let status = self.child.wait().expect("reap SIGKILLed server");
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        pid
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            // The direct Child is the owned pathname server. No arbitrary
            // descendant PID is inferred or signalled by fixture cleanup.
            // SAFETY: best-effort cleanup of this test-owned server process.
            unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
            let deadline = Instant::now() + Duration::from_secs(2);
            while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            if self.child.try_wait().ok().flatten().is_none() {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
        }
    }
}

fn private_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(path)
}

fn line_reader(stdout: ChildStdout) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::sync_channel(8);
    thread::spawn(move || {
        let mut stdout = stdout.take(64 * 1024);
        let mut line = Vec::new();
        let mut byte = [0_u8; 1];
        while let Ok(1) = stdout.read(&mut byte) {
            if byte[0] == b'\n' {
                if sender
                    .send(String::from_utf8_lossy(&line).into_owned())
                    .is_err()
                {
                    return;
                }
                line.clear();
            } else if line.len() < 4096 {
                line.push(byte[0]);
            } else {
                return;
            }
        }
    });
    receiver
}

fn run_bounded(mut command: Command, timeout: Duration) -> Option<Output> {
    command.process_group(0);
    let mut child = command.spawn().ok()?;
    let group = i32::try_from(child.id()).ok()?;
    let stdout = child.stdout.take()?;
    let stderr = child.stderr.take()?;
    let (sender, receiver) = mpsc::channel();
    let out_sender = sender.clone();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(1024 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = out_sender.send((true, result));
    });
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stderr
            .take(1024 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send((false, result));
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            _ => {
                // SAFETY: process_group is the isolated CLI command group.
                unsafe { libc::kill(-group, libc::SIGKILL) };
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    for _ in 0..2 {
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((true, Ok(bytes))) => out = bytes,
            Ok((false, Ok(bytes))) => err = bytes,
            _ => {
                // SAFETY: process_group is the isolated CLI command group.
                unsafe { libc::kill(-group, libc::SIGKILL) };
                return None;
            }
        }
    }
    Some(Output {
        status,
        stdout: out,
        stderr: err,
    })
}

fn field<'a>(json: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("\"{name}\":");
    let start = json.find(&key)? + key.len();
    let rest = &json[start..];
    let end = rest.find([',', '}', ']']).unwrap_or(rest.len());
    Some(rest[..end].trim_matches('"'))
}

fn parse_target(value: &str) -> TargetRef {
    let fields = value
        .split(':')
        .map(|part| part.parse::<u64>().expect("numeric full target component"))
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 5, "full target reference: {value}");
    TargetRef {
        generation: fields[0],
        workspace: fields[1],
        pane: fields[2],
        session: fields[3],
        occupant_revision: fields[4],
    }
}

fn target_arg(target: TargetRef) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        target.generation, target.workspace, target.pane, target.session, target.occupant_revision
    )
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

fn read_rows(store: &HistoryStore, target: TargetRef) -> Option<String> {
    let saved = store.load_rows(target).ok()??;
    Some(
        saved
            .records
            .iter()
            .map(|record| history_rows::decode(record).expect("decode committed saved row"))
            .flat_map(|row| {
                (0..row.width).filter_map(move |column| row.cell_text(column).map(str::to_owned))
            })
            .collect(),
    )
}

fn committed_screens(store: &HistoryStore, target: TargetRef) -> Option<(String, String)> {
    let screens = store.load_screens(target).ok()?;
    let normal = screens[0].as_deref()?;
    let alternate = screens[1].as_deref()?;
    let normal = screen_checkpoint::decode(normal).ok()?;
    let alternate = screen_checkpoint::decode(alternate).ok()?;
    Some((checkpoint_text(&normal), checkpoint_text(&alternate)))
}

fn checkpoint_text(screen: &screen_checkpoint::ScreenCheckpoint<'_>) -> String {
    screen.cells().map(|cell| cell.text().to_owned()).collect()
}

fn wait_until<T>(label: &str, mut inspect: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(value) = inspect() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn tree_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, at: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(at) else {
            return;
        };
        for entry in entries {
            let entry = entry.expect("read private storage entry");
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).expect("stat private storage entry");
            if metadata.file_type().is_dir() {
                visit(root, &path, out);
            } else if metadata.file_type().is_file() {
                out.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

fn assert_sanitized(output: &Output) {
    let bytes = [output.stdout.as_slice(), output.stderr.as_slice()].concat();
    assert!(!bytes.contains(&0x1b), "read-only output contains ESC");
    assert!(
        !String::from_utf8_lossy(&bytes).contains("]52;"),
        "read-only output contains an OSC 52 introducer"
    );
}

fn submit(fixture: &Fixture, target: &str, text: &str) {
    let (code, described) = fixture.control(&["describe"]);
    assert_eq!(code, 0, "describe: {described}");
    assert_eq!(field(&described, "target"), Some(target));
    let revision = field(&described, "owner_revision")
        .expect("owner revision")
        .to_owned();
    let (code, claimed) = fixture.control(&["claim", target, &revision]);
    assert_eq!(code, 0, "claim: {claimed}");
    let (code, described) = fixture.control(&["describe"]);
    assert_eq!(code, 0, "describe after claim: {described}");
    let revision = field(&described, "owner_revision").expect("claimed owner revision");
    let (code, result) = fixture.control(&["submit", target, revision, text]);
    assert_eq!(code, 0, "submit: {result}");
    assert_eq!(field(&result, "outcome"), Some("applied"), "{result}");
}

#[test]
fn committed_live_pathname_state_survives_abrupt_server_death() {
    let fixture = Fixture::new().expect("create isolated crash-recovery fixture");
    fixture.compile_terminfo();

    let script = "stty -echo; printf '%s\\r\\n' \"$3\"; i=1; while [ \"$i\" -le 40 ]; do printf 'P5-CRASH-FILL-%s\\r\\n' \"$i\"; i=$((i+1)); done; IFS= read -r gate; if [ \"$gate\" = \"$1\" ]; then printf '%s\\n' \"$gate\" >> \"$2\"; fi; printf '\\033[?1049h\\033[2J\\033[H%s\\r\\n' \"$4\"; printf '\\033[2J\\033[H%s\\r\\n\\033]52;c;UFI1LUNSQVNILU9TQzUy\\a' \"$5\"; printf 'P5-CRASH-ALT-GATE\\r\\n'; IFS= read -r release; printf '\\033[?1049l\\033[2J\\033[H%s\\r\\n' \"$6\"; while :; do /bin/sleep 1; done";
    let mut server = Server::start(&fixture, script);
    let startup = server.startup();
    assert_eq!(field(&startup, "action"), Some("serve"), "{startup}");
    assert_eq!(field(&startup, "state"), Some("running"), "{startup}");
    assert_eq!(
        field(&startup, "startup_outcome"),
        Some("applied"),
        "{startup}"
    );
    assert_eq!(
        field(&startup, "history_capture"),
        Some("enabled"),
        "live history must be enabled: {startup}"
    );
    assert!(
        fs::symlink_metadata(fixture.server.join("control.sock"))
            .expect("pathname listener exists")
            .file_type()
            .is_socket(),
        "real executable pathname server must bind a Unix socket"
    );
    let target_text = field(&startup, "target")
        .expect("startup full target")
        .to_owned();
    let target = parse_target(&target_text);
    let generation = field(&startup, "generation")
        .expect("startup generation")
        .parse::<u64>()
        .expect("numeric generation");
    assert_eq!(target.generation, generation);

    let (code, initialized) = fixture.control(&["init"]);
    assert_eq!(code, 0, "initialize exact control context: {initialized}");
    wait_until("normal PTY output ready for input", || {
        let (code, screen) = fixture.control(&["read", &target_text]);
        (code == 0 && screen.contains("P5-CRASH-FILL-40")).then_some(screen)
    });
    submit(&fixture, &target_text, &fixture.input_canary);
    wait_until("input side effect exactly once", || {
        fs::read_to_string(&fixture.input_effect)
            .ok()
            .filter(|text| text == &format!("{}\n", fixture.input_canary))
    });
    wait_until("alternate B visible after A overwrite", || {
        let (code, screen) = fixture.control(&["read", &target_text]);
        (code == 0
            && screen.contains(&fixture.alternate_b)
            && !screen.contains(&fixture.alternate_a)
            && screen.contains("P5-CRASH-ALT-GATE"))
        .then_some(screen)
    });

    // Independently decode committed disk records while the real server and
    // its live history writer are still running. Screen output is not used as
    // persistence evidence.
    let history = fixture.server.join("history");
    let store = HistoryStore::open(&history).expect("open live committed history store");
    let rows = wait_until("committed normal history row", || read_rows(&store, target));
    assert!(rows.contains(&fixture.normal_row), "saved rows: {rows}");
    let alternate_checkpoint = wait_until("committed alternate B checkpoint", || {
        let screens = store.load_screens(target).ok()?;
        let bytes = screens[1].as_deref()?;
        let decoded = screen_checkpoint::decode(bytes).ok()?;
        let text = checkpoint_text(&decoded);
        (text.contains(&fixture.alternate_b) && !text.contains(&fixture.alternate_a))
            .then_some(text)
    });
    assert!(alternate_checkpoint.contains(&fixture.alternate_b));
    assert!(
        !alternate_checkpoint.contains(&fixture.alternate_a),
        "the committed latest alternate checkpoint must reflect B after A was overwritten"
    );
    drop(store);

    let layout_path = fixture.server.join("layout/current.layout");
    let saved_layout = wait_until("committed one-pane layout", || {
        let bytes = fs::read(&layout_path).ok()?;
        let decoded = saved_layout::decode(&bytes).ok()?;
        (decoded.pane_count() == 1 && decoded.panes().iter().flatten().next() == Some(&target))
            .then_some((bytes, decoded))
    });
    assert_eq!(saved_layout.1.pane_count(), 1);
    assert_eq!(
        saved_layout.1.panes().iter().flatten().next(),
        Some(&target)
    );
    let layout_bytes = saved_layout.0;

    // Return the live fixture to normal screen only after B is independently
    // committed, then wait for the committed final normal checkpoint.
    submit(&fixture, &target_text, "return-to-normal");
    let store = HistoryStore::open(&history).expect("reopen live history store");
    let (normal_checkpoint, alternate_checkpoint) =
        wait_until("committed final normal screen checkpoint", || {
            committed_screens(&store, target).filter(|(normal, alternate)| {
                normal.contains(&fixture.final_normal)
                    && alternate.contains(&fixture.alternate_b)
                    && !alternate.contains(&fixture.alternate_a)
            })
        });
    assert!(normal_checkpoint.contains(&fixture.final_normal));
    assert!(alternate_checkpoint.contains(&fixture.alternate_b));
    drop(store);
    assert_eq!(fs::read(&layout_path).unwrap(), layout_bytes);
    assert_eq!(
        fs::read_to_string(&fixture.input_effect).unwrap(),
        format!("{}\n", fixture.input_canary)
    );

    let old_pid = server.kill_sigkill_and_reap();
    assert!(
        fixture.server.join("control.sock").exists(),
        "SIGKILL leaves the stale socket"
    );
    let historical = target_arg(target);
    let history_bytes_before_read = tree_bytes(&history);
    let storage_before_read = tree_bytes(&fixture.server);

    let view = fixture.run(&["recovery", "view"]);
    assert!(
        view.status.success(),
        "offline recovery view failed: {view:?}"
    );
    let view_text = String::from_utf8_lossy(&view.stdout);
    assert!(
        view_text.contains(&format!(
            "pane 1: target {historical}; rows available; normal available; alternate available"
        )),
        "{view_text}"
    );
    assert!(!view_text.contains(&fixture.input_canary));
    assert_sanitized(&view);
    assert!(
        !fixture.starts.exists(),
        "read-only recovery view started a shell"
    );
    assert_eq!(tree_bytes(&fixture.server), storage_before_read);

    let show = fixture.run(&["history", "show", &historical]);
    assert!(show.status.success(), "historical show failed: {show:?}");
    let show_text = String::from_utf8_lossy(&show.stdout);
    assert!(show_text.contains(&fixture.normal_row), "{show_text}");
    assert!(show_text.contains(&fixture.final_normal), "{show_text}");
    assert!(show_text.contains(&fixture.alternate_b), "{show_text}");
    assert!(!show_text.contains(&fixture.input_canary));
    assert_sanitized(&show);
    assert!(!fixture.starts.exists(), "historical show started a shell");
    assert_eq!(tree_bytes(&fixture.server), storage_before_read);

    let search = fixture.run(&["history", "search", &historical, &fixture.normal_row]);
    assert!(
        search.status.success(),
        "historical search failed: {search:?}"
    );
    assert!(String::from_utf8_lossy(&search.stdout).contains(&fixture.normal_row));
    assert!(!String::from_utf8_lossy(&search.stdout).contains(&fixture.input_canary));
    assert_sanitized(&search);
    assert!(
        !fixture.starts.exists(),
        "historical search started a shell"
    );
    assert_eq!(tree_bytes(&fixture.server), storage_before_read);
    assert_eq!(tree_bytes(&history), history_bytes_before_read);

    // Explicit recovery is the documented stale-socket route. It must start
    // one fresh target at the saved one-pane topology and a new generation.
    let relaunch = fixture.run(&["recovery", "relaunch"]);
    fixture.replacement.set(fixture.replacement_identity());
    assert!(
        relaunch.status.success(),
        "explicit recovery relaunch failed: {relaunch:?}"
    );
    let relaunch_text = String::from_utf8_lossy(&relaunch.stdout);
    assert!(
        relaunch_text.contains(&format!("old {historical} -> new ")),
        "{relaunch_text}"
    );
    assert_eq!(
        relaunch_text.matches("(applied)").count(),
        1,
        "{relaunch_text}"
    );
    wait_until("fresh relaunch shell start marker", || {
        fs::read_to_string(&fixture.starts)
            .ok()
            .filter(|text| !text.is_empty())
    });
    wait_until("fresh shell replay probe completion", || {
        fixture.replay_probe_done.is_file().then_some(())
    });
    assert!(
        !fixture.replay_effect.exists(),
        "historical input canary was replayed into the fresh shell"
    );
    assert_eq!(
        fs::read_to_string(&fixture.input_effect).unwrap(),
        format!("{}\n", fixture.input_canary),
        "input side effect remains exactly once"
    );

    let panes = fixture.run(&["panes"]);
    assert!(
        panes.status.success(),
        "fresh panes query failed: {panes:?}"
    );
    let panes_text = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(panes_text.matches("target ").count(), 1, "{panes_text}");
    assert!(!panes_text.contains(&historical));
    let new_target_text = panes_text
        .split("target ")
        .nth(1)
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|value| value.strip_suffix(')'))
        .expect("new full target");
    let new_target = parse_target(new_target_text);
    assert_ne!(new_target, target);
    assert_ne!(new_target.generation, generation);

    let start = fs::read_to_string(&fixture.starts).unwrap();
    let fields = start.lines().next().unwrap().split('|').collect::<Vec<_>>();
    assert_eq!(
        fields.len(),
        4,
        "fresh shell cwd/home/env/generation: {start:?}"
    );
    assert_eq!(fields[0], fixture.cwd.to_string_lossy());
    assert_eq!(fields[1], fixture.home.to_string_lossy());
    assert_eq!(fields[2].parse::<u64>().unwrap(), new_target.generation);
    assert_eq!(fields[3], "private-environment");

    // Recovery metadata may add links and fresh layout records. The old
    // history files themselves remain byte-identical and addressable by the
    // historical full target after relaunch.
    let old_history_after_relaunch = fixture.run(&["history", "show", &historical]);
    assert!(old_history_after_relaunch.status.success());
    assert!(
        String::from_utf8_lossy(&old_history_after_relaunch.stdout).contains(&fixture.alternate_b)
    );
    assert_sanitized(&old_history_after_relaunch);
    for (path, bytes) in history_bytes_before_read {
        assert_eq!(fs::read(history.join(path)).unwrap(), bytes);
    }
    let replacement = fixture
        .replacement
        .get()
        .expect("verified replacement server identity");
    assert_ne!(old_pid, replacement.pid);
    let stopped = fixture.run(&["stop"]);
    assert!(
        stopped.status.success(),
        "replacement stop failed: {stopped:?}"
    );
    assert!(
        wait_for_exit(replacement, DEADLINE),
        "replacement server did not exit"
    );
    assert!(!fixture.server.join("control.sock").exists());
}
