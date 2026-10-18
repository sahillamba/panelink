#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::ffi::OsString;
use std::fs::{self, DirBuilder, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
use rustix::process::{ioctl_tiocsctty, kill_process, kill_process_group, setsid, Pid, Signal};
use rustix::termios::{tcgetattr, tcgetsid, tcsetwinsize, LocalModes, Termios, Winsize};
use rustix_openpty::openpty;

const IO_DEADLINE: Duration = Duration::from_secs(6);
const MAX_CAPTURE_BYTES: usize = 8 * 1024 * 1024;
const CONTROL_FILE_BYTES: usize = 32;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "panelink-p2-{label}-{}-{serial}",
            std::process::id()
        ));
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(&path)
            .expect("create private test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn compiled_terminfo() -> TempDir {
    let output = TempDir::new("terminfo");
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/terminfo/panelink-probe.terminfo");
    let mut command = Command::new("/usr/bin/tic");
    command
        .arg("-x")
        .arg("-o")
        .arg(output.path())
        .arg(source)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", output.path());
    let mut child = command
        .spawn()
        .expect("run tic for the private test fixture");
    let status =
        match bounded_wait(&mut child, Duration::from_secs(3)).expect("poll terminfo compiler") {
            Some(status) => status,
            None => {
                let _ = child.kill();
                let _ = bounded_wait(&mut child, Duration::from_secs(1));
                panic!("terminfo compiler exceeded its fixture deadline");
            }
        };
    assert!(status.success(), "tic rejected the checked-in test profile");
    assert!(
        output.path().join("p/panelink-probe").is_file()
            || output.path().join("70/panelink-probe").is_file()
    );
    output
}

struct ProbeHarness {
    supervisor: Child,
    supervisor_status: Option<ExitStatus>,
    probe_pid: Option<Pid>,
    probe_status: Option<ExitStatus>,
    _control_directory: TempDir,
    probe_pid_path: PathBuf,
    probe_status_path: PathBuf,
    release_path: PathBuf,
    master: Option<fs::File>,
    slave: OwnedFd,
    initial_flags: OFlags,
    initial_termios: Termios,
    release_drained_bytes: usize,
}

impl ProbeHarness {
    fn spawn(terminfo: &TempDir, executable: &str, args: &[&str]) -> Self {
        let mut command = vec![
            OsString::from(env!("CARGO_BIN_EXE_panelink")),
            OsString::from("pty-probe"),
            OsString::from("--terminfo"),
            terminfo.path().as_os_str().to_owned(),
            OsString::from("--"),
            OsString::from(executable),
        ];
        command.extend(args.iter().map(OsString::from));
        Self::spawn_command(&command)
    }

    fn spawn_command(command_arguments: &[OsString]) -> Self {
        assert!(
            !command_arguments.is_empty(),
            "supervised command is nonempty"
        );
        let winsize = Winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pty = openpty(None, Some(&winsize)).expect("open outer test PTY");
        let master = fs::File::from(pty.controller);
        let slave = pty.user;
        let initial_termios = tcgetattr(&slave).expect("save outer terminal before spawning probe");
        let flags = fcntl_getfl(&slave).expect("read outer slave descriptor flags");
        let master_flags = fcntl_getfl(&master).expect("read outer master flags");
        fcntl_setfl(&master, master_flags | OFlags::NONBLOCK)
            .expect("make only the outer test controller nonblocking");
        let control_directory = TempDir::new("supervisor");
        let probe_pid_path = control_directory.path().join("probe.pid");
        let probe_status_path = control_directory.path().join("probe.status");
        let release_path = control_directory.path().join("release");
        let controlling = slave
            .as_fd()
            .try_clone_to_owned()
            .expect("clone outer controlling TTY");
        let stdin = fs::File::from(slave.as_fd().try_clone_to_owned().unwrap());
        let stdout = fs::File::from(slave.as_fd().try_clone_to_owned().unwrap());
        let stderr = fs::File::from(slave.as_fd().try_clone_to_owned().unwrap());
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(SUPERVISOR_SCRIPT)
            .arg("panelink-p2-supervisor")
            .arg(&probe_pid_path)
            .arg(&probe_status_path)
            .arg(&release_path)
            .args(command_arguments)
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", control_directory.path());
        // Safety: the fixture hook only establishes a fresh session and sets
        // the inherited PTY as its controlling terminal through reviewed Rustix
        // wrappers. It does not allocate, lock, log, or inspect the environment.
        unsafe {
            command.pre_exec(move || {
                setsid().map_err(io::Error::from)?;
                ioctl_tiocsctty(&controlling).map_err(io::Error::from)?;
                tcgetsid(&controlling).map_err(io::Error::from)?;
                Ok(())
            });
        }
        let supervisor = command.spawn().expect("start outer-PTY supervisor");
        drop(command);
        let mut harness = Self {
            supervisor,
            supervisor_status: None,
            probe_pid: None,
            probe_status: None,
            _control_directory: control_directory,
            probe_pid_path,
            probe_status_path,
            release_path,
            master: Some(master),
            slave,
            initial_flags: flags,
            initial_termios,
            release_drained_bytes: 0,
        };
        harness
            .wait_for_probe_start(Duration::from_secs(2))
            .expect("supervisor did not publish a bounded probe PID or exit status");
        harness
    }

    fn poll_supervisor_status(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.supervisor_status {
            return Ok(Some(status));
        }
        let status = self.supervisor.try_wait()?;
        self.supervisor_status = status;
        Ok(status)
    }

    fn poll_probe_status(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.probe_status {
            return Ok(Some(status));
        }
        match read_control_file(&self.probe_status_path)? {
            Some(bytes) => {
                let code = parse_control_number(&bytes, "probe status")?;
                if code > u32::from(u8::MAX) {
                    return Err(invalid_control(
                        "probe status is outside the exit-code range",
                    ));
                }
                let status = ExitStatus::from_raw((code as i32) << 8);
                self.probe_status = Some(status);
                self.probe_pid = None;
                Ok(Some(status))
            }
            None => {
                if let Some(status) = self.poll_supervisor_status()? {
                    return Err(invalid_control(&format!(
                        "supervisor exited before publishing probe status ({status})"
                    )));
                }
                Ok(None)
            }
        }
    }

    fn read_probe_pid(&self) -> io::Result<Option<Pid>> {
        let Some(bytes) = read_control_file(&self.probe_pid_path)? else {
            return Ok(None);
        };
        let value = parse_control_number(&bytes, "probe PID")?;
        let raw = i32::try_from(value).map_err(|_| invalid_control("probe PID is too large"))?;
        Pid::from_raw(raw)
            .map(Some)
            .ok_or_else(|| invalid_control("probe PID is invalid"))
    }

    fn wait_for_probe_start(&mut self, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.poll_probe_status()?.is_some() {
                return Ok(());
            }
            if let Some(pid) = self.read_probe_pid()? {
                if self.poll_probe_status()?.is_some() {
                    return Ok(());
                }
                self.probe_pid = Some(pid);
                return Ok(());
            }
            if let Some(status) = self.poll_supervisor_status()? {
                return Err(invalid_control(&format!(
                    "supervisor exited before publishing probe PID ({status})"
                )));
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "supervisor did not publish probe PID or status before deadline",
                ));
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn probe_pid_for_signal(&mut self) -> Pid {
        assert!(
            self.poll_probe_status()
                .expect("read probe status before signaling")
                .is_none(),
            "do not signal a probe after its exit is known"
        );
        self.probe_pid
            .expect("supervisor published the running probe PID")
    }

    fn wait_for_marker(&mut self, path: &Path, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            if path.is_file() {
                return;
            }
            if let Some(status) = self
                .poll_probe_status()
                .expect("read probe status while waiting for fixture readiness")
            {
                panic!("probe exited before child fixture became ready: {status}");
            }
            assert!(
                Instant::now() < deadline,
                "child fixture readiness timed out"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn wait_for_raw_mode(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut capture = Vec::new();
        loop {
            let termios = match tcgetattr(&self.slave) {
                Ok(termios) => termios,
                Err(error) => {
                    self.drain_once(&mut capture);
                    let status = self.poll_probe_status().expect("poll failed probe status");
                    panic!(
                        "query outer slave termios failed ({error:?}), probe status {status:?}, captured {:?}",
                        String::from_utf8_lossy(&capture)
                    );
                }
            };
            if !termios.local_modes.contains(LocalModes::ICANON) {
                return;
            }
            self.drain_once(&mut capture);
            if let Some(status) = self.poll_probe_status().expect("poll probe status") {
                panic!("probe exited before raw mode: {status}");
            }
            assert!(Instant::now() < deadline, "probe never entered raw mode");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn write_host(&mut self, bytes: &[u8]) {
        assert_eq!(self.write_until_closed(bytes), bytes.len());
    }

    fn write_until_closed(&mut self, bytes: &[u8]) -> usize {
        let deadline = Instant::now() + IO_DEADLINE;
        let mut cursor = 0;
        let mut captured = Vec::new();
        while cursor < bytes.len() {
            let Some(master) = self.master.as_mut() else {
                return cursor;
            };
            match master.write(&bytes[cursor..]) {
                Ok(0) => return cursor,
                Ok(count) => cursor += count,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        || error.kind() == io::ErrorKind::Interrupted =>
                {
                    self.drain_once(&mut captured);
                    if Instant::now() >= deadline {
                        return cursor;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(_) => return cursor,
            }
        }
        cursor
    }

    fn write_single_short(&mut self, bytes: &[u8]) -> usize {
        let deadline = Instant::now() + IO_DEADLINE;
        let mut captured = Vec::new();
        loop {
            let Some(master) = self.master.as_mut() else {
                return 0;
            };
            match master.write(bytes) {
                Ok(count) => return count,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        || error.kind() == io::ErrorKind::Interrupted =>
                {
                    self.drain_once(&mut captured);
                    if Instant::now() >= deadline {
                        return 0;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(_) => return 0,
            }
        }
    }

    fn resize(&self, rows: u16, columns: u16) {
        let master = self.master.as_ref().expect("outer test PTY remains open");
        tcsetwinsize(
            master,
            Winsize {
                ws_row: rows,
                ws_col: columns,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )
        .expect("resize outer test PTY");
    }

    fn drain_once(&mut self, capture: &mut Vec<u8>) -> usize {
        let Some(master) = self.master.as_mut() else {
            return 0;
        };
        let mut buffer = [0_u8; 8192];
        let mut drained = 0;
        // A continuously writing child must not keep the harness from checking
        // its caller's deadline or polling process status.
        for _ in 0..8 {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    drained += count;
                    let available = MAX_CAPTURE_BYTES.saturating_sub(capture.len());
                    capture.extend_from_slice(&buffer[..count.min(available)]);
                }
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        || error.kind() == io::ErrorKind::Interrupted =>
                {
                    break;
                }
                Err(_) => break,
            }
        }
        drained
    }

    fn wait_exit(&mut self, timeout: Duration) -> (ExitStatus, Vec<u8>) {
        let deadline = Instant::now() + timeout;
        let mut capture = Vec::new();
        loop {
            self.drain_once(&mut capture);
            if let Some(status) = self.poll_probe_status().expect("poll probe status") {
                return (status, capture);
            }
            assert!(Instant::now() < deadline, "probe exceeded test deadline");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn wait_exit_without_draining(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.poll_probe_status().expect("poll probe status") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "probe exceeded no-drain output-stall deadline"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn wait_for_probe_status(&mut self, timeout: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(Some(status)) = self.poll_probe_status() {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn release_and_reap_supervisor(&mut self) -> io::Result<ExitStatus> {
        // A cached Child status means try_wait already reaped this process.
        // Never use its former PID as a process-group ID after that point.
        if let Some(status) = self.supervisor_status {
            return Ok(status);
        }
        fs::write(&self.release_path, b"release\n")?;
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut discarded = Vec::new();
        loop {
            self.release_drained_bytes = self
                .release_drained_bytes
                .saturating_add(self.drain_once(&mut discarded));
            if let Some(status) = self.supervisor.try_wait()? {
                self.supervisor_status = Some(status);
                return Ok(status);
            }
            if Instant::now() >= deadline {
                // Closing the controller also releases any kernel-held PTY
                // output state before the bounded forced reap below.
                self.master.take();
                if let Some(group) = Pid::from_raw(self.supervisor.id() as i32) {
                    let _ = kill_process_group(group, Signal::KILL);
                }
                let status = bounded_wait(&mut self.supervisor, Duration::from_secs(1))?
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::TimedOut,
                            "supervisor did not reap before cleanup deadline",
                        )
                    })?;
                self.supervisor_status = Some(status);
                return Ok(status);
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn saved_terminal_matches(&self, expected: &Termios) {
        let actual = tcgetattr(&self.slave).expect("query restored outer terminal");
        assert_eq!(actual.input_modes, expected.input_modes);
        assert_eq!(actual.output_modes, expected.output_modes);
        assert_eq!(actual.control_modes, expected.control_modes);
        assert_eq!(actual.local_modes, expected.local_modes);
        assert_eq!(actual.input_speed(), expected.input_speed());
        assert_eq!(actual.output_speed(), expected.output_speed());
        assert_eq!(
            format!("{:?}", actual.special_codes),
            format!("{:?}", expected.special_codes)
        );
        assert_eq!(fcntl_getfl(&self.slave).unwrap(), self.initial_flags);
    }
}

impl Drop for ProbeHarness {
    fn drop(&mut self) {
        let probe_finished = self.poll_probe_status().ok().flatten().is_some();
        let supervisor_alive = matches!(self.poll_supervisor_status(), Ok(None));
        if !probe_finished && supervisor_alive {
            if let Some(group) = Pid::from_raw(self.supervisor.id() as i32) {
                // The owned, unreaped session leader pins this process-group ID.
                // TERM the group rather than signaling a raw probe PID that may
                // have exited just before its status file became visible.
                let _ = kill_process_group(group, Signal::TERM);
                let _ = self.wait_for_probe_status(Duration::from_millis(2_500));
            }
        }
        if self.release_and_reap_supervisor().is_err()
            && self.supervisor_status.is_none()
            && matches!(self.poll_supervisor_status(), Ok(None))
        {
            if let Some(group) = Pid::from_raw(self.supervisor.id() as i32) {
                let _ = kill_process_group(group, Signal::KILL);
            }
            let _ = bounded_wait(&mut self.supervisor, Duration::from_secs(1));
        }
        self.master.take();
    }
}

const SUPERVISOR_SCRIPT: &str = r#"
pid_path=$1
status_path=$2
release_path=$3
shift 3
trap ':' HUP INT TERM
"$@" <&0 &
probe_pid=$!
printf '%s\n' "$probe_pid" > "$pid_path.tmp" || exit 125
/bin/mv "$pid_path.tmp" "$pid_path" || exit 125
have_wait_status=0
last_wait_status=0
while :; do
    wait "$probe_pid"
    probe_status=$?
    if ! kill -0 "$probe_pid" 2>/dev/null; then
        if [ "$probe_status" -eq 127 ] && [ "$have_wait_status" -eq 1 ]; then
            probe_status=$last_wait_status
        fi
        break
    fi
    if [ "$probe_status" -ne 127 ]; then
        last_wait_status=$probe_status
        have_wait_status=1
    fi
done
printf '%s\n' "$probe_status" > "$status_path.tmp" || exit 125
/bin/mv "$status_path.tmp" "$status_path" || exit 125
while [ ! -f "$release_path" ]; do /bin/sleep 0.05; done
exit 0
"#;

fn invalid_control(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

fn read_control_file(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(invalid_control(
            "supervisor control path is not a regular file",
        ));
    }
    let mut file = file.take((CONTROL_FILE_BYTES + 1) as u64);
    let mut bytes = [0_u8; CONTROL_FILE_BYTES + 1];
    let mut length = 0;
    while length < bytes.len() {
        let count = file.read(&mut bytes[length..])?;
        if count == 0 {
            break;
        }
        length += count;
    }
    if length > CONTROL_FILE_BYTES {
        return Err(invalid_control("supervisor control file exceeds 32 bytes"));
    }
    Ok(Some(bytes[..length].to_vec()))
}

fn parse_control_number(bytes: &[u8], name: &str) -> io::Result<u32> {
    if bytes.is_empty() || bytes.last() != Some(&b'\n') {
        return Err(invalid_control(&format!("partial {name} control file")));
    }
    let digits = &bytes[..bytes.len() - 1];
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err(invalid_control(&format!("invalid {name} control file")));
    }
    std::str::from_utf8(digits)
        .map_err(|_| invalid_control(&format!("invalid {name} encoding")))?
        .parse::<u32>()
        .map_err(|_| invalid_control(&format!("invalid {name} number")))
}

fn bounded_wait(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn marker_path(directory: &TempDir, name: &str) -> String {
    directory.path().join(name).to_str().unwrap().to_owned()
}

#[test]
fn outer_pty_fixture_attaches_a_controlling_session() {
    let pty = openpty(None, None).expect("open PTY for the fixture control check");
    let controlling = pty.user.as_fd().try_clone_to_owned().unwrap();
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("exit 0")
        .stdin(Stdio::from(fs::File::from(
            pty.user.as_fd().try_clone_to_owned().unwrap(),
        )))
        .stdout(Stdio::from(fs::File::from(
            pty.user.as_fd().try_clone_to_owned().unwrap(),
        )))
        .stderr(Stdio::from(fs::File::from(
            pty.user.as_fd().try_clone_to_owned().unwrap(),
        )))
        .env_clear()
        .env("PATH", "/usr/bin:/bin");
    unsafe {
        command.pre_exec(move || {
            setsid().map_err(io::Error::from)?;
            ioctl_tiocsctty(&controlling).map_err(io::Error::from)?;
            let terminal_session = tcgetsid(&controlling).map_err(io::Error::from)?;
            let process_session = rustix::process::getsid(None).map_err(io::Error::from)?;
            if terminal_session != process_session {
                return Err(io::Error::from(io::ErrorKind::NotFound));
            }
            Ok(())
        });
    }
    let mut child = command.spawn().expect("spawn controlling-PTY fixture");
    drop(command);
    drop(pty.user);
    let status = match bounded_wait(&mut child, Duration::from_secs(2)) {
        Ok(Some(status)) => status,
        Ok(None) | Err(_) => {
            let _ = child.kill();
            let _ = bounded_wait(&mut child, Duration::from_secs(1));
            panic!("controlling-PTY fixture child did not exit promptly");
        }
    };
    assert!(status.success());
}

#[test]
fn retained_slave_remains_queryable_after_supervised_child_exit() {
    let command = [OsString::from("/usr/bin/true")];
    let mut harness = ProbeHarness::spawn_command(&command);
    let saved = harness.initial_termios.clone();
    let (status, capture) = harness.wait_exit(Duration::from_secs(2));
    assert!(
        status.success(),
        "supervised /usr/bin/true status: {status:?}"
    );
    assert!(capture.is_empty());
    harness.saved_terminal_matches(&saved);

    let supervisor_status = harness
        .release_and_reap_supervisor()
        .expect("release and reap the controlling-session supervisor");
    assert!(supervisor_status.success());
}

#[test]
fn release_drains_suppressed_child_output_and_reaps_supervisor() {
    let command = [
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from("printf '%512s' x"),
    ];
    let mut harness = ProbeHarness::spawn_command(&command);
    let probe_status = harness.wait_exit_without_draining(Duration::from_secs(2));
    assert!(probe_status.success());
    assert_eq!(harness.release_drained_bytes, 0);

    let supervisor_status = harness
        .release_and_reap_supervisor()
        .expect("release and reap supervisor after suppressed-output check");
    assert!(supervisor_status.success());
    assert!(
        harness.release_drained_bytes >= 512,
        "release cleanup drained only {} bytes",
        harness.release_drained_bytes
    );
}

#[test]
#[ignore = "operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access"]
fn normal_child_exit_restores_termios_and_never_forwards_osc_controls() {
    let terminfo = compiled_terminfo();
    let script = "printf '\\033]52;c;not-a-host-clipboard\\007'; printf '\\033]0;not-a-host-title\\007'; printf done; exit 7";
    let mut probe = ProbeHarness::spawn(&terminfo, "/bin/sh", &["-c", script]);
    let saved = probe.initial_termios.clone();
    let (status, capture) = probe.wait_exit(Duration::from_secs(5));
    assert_eq!(status.code(), Some(7));
    probe.saved_terminal_matches(&saved);
    assert!(!capture.windows(5).any(|window| window == b"\x1b]52;"));
    assert!(!capture.windows(4).any(|window| window == b"\x1b]0;"));
    assert!(!capture.is_empty());
}

#[test]
#[ignore = "operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access"]
fn typed_keys_and_one_decoded_paste_reach_the_child_once() {
    let terminfo = compiled_terminfo();
    let fixture = TempDir::new("paste");
    let output = marker_path(&fixture, "child-input");
    let script = "IFS= read -r line; printf '%s' \"$line\" > \"$1\"";
    let mut probe = ProbeHarness::spawn(&terminfo, "/bin/sh", &["-c", script, "sh", &output]);
    probe.wait_for_raw_mode();
    probe.write_host(b"typed\x1b[200~paste\x1b[201~\r");
    let (status, _) = probe.wait_exit(Duration::from_secs(5));
    assert!(status.success());
    assert_eq!(fs::read_to_string(output).unwrap(), "typedpaste");
}

#[test]
#[ignore = "operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access"]
fn ctrl_q_restores_the_outer_terminal_and_stops_the_child() {
    let terminfo = compiled_terminfo();
    let mut probe = ProbeHarness::spawn(
        &terminfo,
        "/bin/sh",
        &["-c", "trap '' HUP; while :; do sleep 1; done"],
    );
    let saved = probe.initial_termios.clone();
    probe.wait_for_raw_mode();
    probe.write_host(b"\x11");
    let (status, _) = probe.wait_exit(Duration::from_secs(3));
    assert_eq!(status.code(), Some(0));
    probe.saved_terminal_matches(&saved);
}

#[test]
#[ignore = "operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access"]
fn local_ctrl_q_remains_available_while_child_exit_output_is_draining() {
    let terminfo = compiled_terminfo();
    let mut probe = ProbeHarness::spawn(&terminfo, "/bin/sh", &["-c", "(sleep 2) & exit 0"]);
    probe.wait_for_raw_mode();
    // Let the shell exit while its inherited PTY slave remains open in the
    // harmless background fixture. The probe must keep polling host input.
    thread::sleep(Duration::from_millis(150));
    probe.write_host(b"\x11");
    let (status, _) = probe.wait_exit(Duration::from_secs(3));
    assert_eq!(status.code(), Some(0));
}

#[test]
#[ignore = "operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access"]
fn external_sigterm_is_polled_and_hangup_ignoring_child_is_killed_within_budget() {
    let terminfo = compiled_terminfo();
    let fixture = TempDir::new("hup-ignore");
    let ready = marker_path(&fixture, "child-ready");
    let mut probe = ProbeHarness::spawn(
        &terminfo,
        "/bin/sh",
        &[
            "-c",
            "trap '' HUP; : > \"$1\"; while :; do :; done",
            "sh",
            &ready,
        ],
    );
    probe.wait_for_raw_mode();
    probe.wait_for_marker(Path::new(&ready), Duration::from_secs(2));
    let started = Instant::now();
    let pid = probe.probe_pid_for_signal();
    kill_process(pid, Signal::TERM).expect("send SIGTERM to isolated probe");
    let (status, _) = probe.wait_exit(Duration::from_secs(3));
    assert_eq!(status.code(), Some(130));
    assert!(started.elapsed() < Duration::from_millis(2_500));
}

#[test]
#[ignore = "operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access"]
fn output_stall_diagnostic_and_child_cleanup_are_bounded_without_outer_drain() {
    let terminfo = compiled_terminfo();
    let mut probe = ProbeHarness::spawn(
        &terminfo,
        "/bin/sh",
        &[
            "-c",
            "trap '' HUP; while :; do printf '\\n'; sleep 0.01; done",
        ],
    );
    probe.wait_for_raw_mode();
    let saved = probe.initial_termios.clone();
    let started = Instant::now();
    let status = probe.wait_exit_without_draining(Duration::from_secs(5));
    assert_eq!(status.code(), Some(1));
    assert!(started.elapsed() < Duration::from_millis(4_500));
    probe.saved_terminal_matches(&saved);
}

#[test]
#[ignore = "operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access"]
fn resize_is_applied_to_the_child_and_an_undersized_resize_restores_the_host() {
    let terminfo = compiled_terminfo();
    let fixture = TempDir::new("resize");
    let output = marker_path(&fixture, "child-size");
    let script = "IFS= read -r line; stty size > \"$1\"";
    let mut probe = ProbeHarness::spawn(&terminfo, "/bin/sh", &["-c", script, "sh", &output]);
    probe.wait_for_raw_mode();
    probe.resize(30, 90);
    thread::sleep(Duration::from_millis(180));
    probe.write_host(b"\r");
    let (status, _) = probe.wait_exit(Duration::from_secs(5));
    assert!(status.success());
    assert_eq!(fs::read_to_string(output).unwrap(), "30 90\n");

    let mut probe = ProbeHarness::spawn(&terminfo, "/bin/sh", &["-c", "sleep 20"]);
    let saved = probe.initial_termios.clone();
    probe.wait_for_raw_mode();
    probe.resize(4, 19);
    let (status, _) = probe.wait_exit(Duration::from_secs(4));
    assert_eq!(status.code(), Some(1));
    probe.saved_terminal_matches(&saved);
    // The fixture PTY is private, but return it to its starting size before drop.
    probe.resize(24, 80);
}

#[test]
#[ignore = "operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access"]
fn oversized_paste_drops_suffix_and_flushes_already_queued_outer_input() {
    let terminfo = compiled_terminfo();
    let fixture = TempDir::new("paste-overflow");
    let output = marker_path(&fixture, "child-line");
    let script = "IFS= read -r line; printf '%s' \"$line\" > \"$1\"";
    let mut probe = ProbeHarness::spawn(&terminfo, "/bin/sh", &["-c", script, "sh", &output]);
    probe.wait_for_raw_mode();

    let mut valid_pending = Vec::with_capacity(6 + 65_536);
    valid_pending.extend_from_slice(b"\x1b[200~");
    valid_pending.resize(6 + 65_536, b'x');
    assert_eq!(
        probe.write_until_closed(&valid_pending),
        valid_pending.len()
    );

    let rejected_tail = b"x\x1b[201~\rnot-a-command\r";
    assert!(rejected_tail.len() < 64);
    assert_eq!(
        probe.write_single_short(rejected_tail),
        rejected_tail.len(),
        "the complete overflow trigger and queued tail must enter in one short write"
    );
    let (status, _) = probe.wait_exit(Duration::from_secs(5));
    assert_eq!(status.code(), Some(1));
    assert!(!Path::new(&output).exists());

    // A completed line would be readable here if raw input survived cleanup.
    let mut pending = [0_u8; 128];
    let mut pending_file = fs::File::from(
        probe
            .slave
            .as_fd()
            .try_clone_to_owned()
            .expect("clone restored outer slave"),
    );
    let old_flags = fcntl_getfl(&probe.slave).unwrap();
    fcntl_setfl(&probe.slave, old_flags | OFlags::NONBLOCK).unwrap();
    let pending_result = pending_file.read(&mut pending);
    fcntl_setfl(&probe.slave, old_flags).expect("restore shared PTY status flags");
    match pending_result {
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
        Ok(0) => {}
        Ok(count) => panic!("host input suffix remained queued after cleanup ({count} bytes)"),
        Err(error) => panic!("could not inspect restored PTY input queue: {error:?}"),
    }
}

#[test]
#[ignore = "operator-only: restricted command sandbox denies /dev/tty open with EPERM; rerun with outer-PTY device access"]
fn host_hangup_exits_within_deadline() {
    let terminfo = compiled_terminfo();
    let mut probe = ProbeHarness::spawn(&terminfo, "/bin/sh", &["-c", "sleep 20"]);
    probe.wait_for_raw_mode();
    probe.master.take();
    let (status, _) = probe.wait_exit(Duration::from_secs(4));
    assert!(!status.success());
}

#[test]
fn non_tty_rejection_happens_before_child_side_effects() {
    let terminfo = compiled_terminfo();
    let fixture = TempDir::new("non-tty");
    let marker = fixture.path().join("spawned");
    let mut command = Command::new(env!("CARGO_BIN_EXE_panelink"));
    command
        .arg("pty-probe")
        .arg("--terminfo")
        .arg(terminfo.path())
        .arg("--")
        .arg("/usr/bin/touch")
        .arg(&marker)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", fixture.path());
    let mut child = command.spawn().expect("run non-TTY rejection case");
    drop(command);
    let status = match bounded_wait(&mut child, Duration::from_secs(3)) {
        Ok(Some(status)) => status,
        Ok(None) | Err(_) => {
            let _ = child.kill();
            let _ = bounded_wait(&mut child, Duration::from_secs(1));
            panic!("non-TTY preflight child did not exit promptly");
        }
    };
    assert_eq!(status.code(), Some(1));
    assert!(!marker.exists());
}
