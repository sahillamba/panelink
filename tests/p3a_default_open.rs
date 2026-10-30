#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Outer-PTY end-to-end proof of zero-configuration `panelink`: no
//! arguments starts (or reattaches to) the per-user workspace server under
//! `PANELINK_HOME`, Ctrl-X detaches, `panelink stop` ends it, the last shell
//! exiting closes it, and a killed server's stale socket is recovered.
//!
//! The client runs as a session leader on a private outer PTY (like
//! `tests/p2_host.rs`), so its `/dev/tty` is that PTY. The test injects real
//! host bytes (keys, an SGR right-click, Ctrl-B s, Esc, Ctrl-X) and checks
//! both the rendered client output (through a vt100 parser) and the server
//! state (through the separate `panelink control` CLI).

use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const STEP: Duration = Duration::from_secs(10);
const ROWS: u16 = 24;
const COLUMNS: u16 = 100;
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    state: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/pl-p3a-open-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let make = |path: &Path| {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .expect("create private fixture directory")
        };
        make(&root);
        let fixture = Self {
            state: root.join("state"),
            home: root.join("home"),
            root,
        };
        make(&fixture.home);
        fixture
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_panelink"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("ZDOTDIR", &self.home)
            .env("SHELL", "/bin/zsh")
            .env("PANELINK_HOME", &self.state)
            .current_dir(&self.home)
            .stdin(Stdio::null());
        command
    }

    fn server_dir(&self) -> PathBuf {
        self.state.join("server")
    }

    fn socket(&self) -> PathBuf {
        self.server_dir().join("control.sock")
    }

    fn control(&self, context: &str, args: &[&str]) -> (i32, String) {
        let output = self
            .command()
            .arg("control")
            .arg("--directory")
            .arg(self.server_dir())
            .arg("--context")
            .arg(self.root.join(context))
            .args(args)
            .output()
            .expect("run control CLI");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    }

    fn stop(&self) -> (i32, String) {
        let output = self.command().arg("stop").output().expect("panelink stop");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    }

    fn server_pid(&self) -> Option<i32> {
        fs::read_to_string(self.state.join("server.pid"))
            .ok()?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    }

    fn server_alive(&self) -> bool {
        // SAFETY: signal 0 only checks existence of our own child server.
        self.server_pid()
            .is_some_and(|pid| unsafe { libc::kill(pid, 0) } == 0)
    }

    fn verified_server_pid(&self) -> Option<i32> {
        let record = fs::read_to_string(self.state.join("server.pid")).ok()?;
        let mut fields = record.split_whitespace();
        let pid: i32 = fields.next()?.parse().ok()?;
        let recorded_start: u64 = fields.next()?.parse().ok()?;
        if pid <= 0 || fields.next().is_some() {
            return None;
        }
        let info = process_info(pid)?;
        // SAFETY: geteuid has no arguments or memory effects.
        if info.pbi_uid != unsafe { libc::geteuid() } || info.pbi_status == libc::SZOMB {
            return None;
        }
        let start = info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec;
        (start == recorded_start).then_some(pid)
    }

    fn wait_server_gone(&self) {
        let deadline = Instant::now() + STEP;
        while self.socket().exists() || self.server_pid().is_some_and(|pid| !process_exited(pid)) {
            assert!(Instant::now() < deadline, "server did not exit");
            thread::sleep(Duration::from_millis(20));
        }
    }
}

#[path = "support/outer_pty.rs"]
mod outer_pty;
use outer_pty::{Client, SUPERVISOR};

impl Fixture {
    fn client(&self, stderr: Option<std::os::fd::OwnedFd>, rows: u16, columns: u16) -> Client {
        Client::spawn(
            &self.root,
            rows,
            columns,
            stderr,
            |status, release, stdin, stdout, stderr| {
                let mut command = Command::new("/bin/sh");
                command
                    .arg("-c")
                    .arg(SUPERVISOR)
                    .arg("panelink-default-open-supervisor")
                    .arg(status)
                    .arg(release)
                    .arg(env!("CARGO_BIN_EXE_panelink"))
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin")
                    .env("HOME", &self.home)
                    .env("ZDOTDIR", &self.home)
                    .env("SHELL", "/bin/zsh")
                    .env("PANELINK_HOME", &self.state)
                    .env("PS1", "P3AD$ ")
                    .current_dir(&self.home)
                    .stdin(stdin)
                    .stdout(stdout)
                    .stderr(stderr);
                command
            },
        )
    }

    fn real_environment_client(&self) -> Client {
        Client::spawn(
            &self.root,
            ROWS,
            COLUMNS,
            None,
            |status, release, stdin, stdout, stderr| {
                let mut command = Command::new("/bin/sh");
                command
                    .arg("-c")
                    .arg(SUPERVISOR)
                    .arg("panelink-real-environment-supervisor")
                    .arg(status)
                    .arg(release)
                    .arg(env!("CARGO_BIN_EXE_panelink"))
                    .env("PANELINK_HOME", &self.state)
                    .stdin(stdin)
                    .stdout(stdout)
                    .stderr(stderr);
                command
            },
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.stop();
        if let Some(pid) = self.verified_server_pid() {
            // SAFETY: best-effort cleanup of this test's own server.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn field<'a>(json: &'a str, name: &str) -> Option<&'a str> {
    let pattern = format!("\"{name}\":");
    let start = json.find(&pattern)? + pattern.len();
    let rest = &json[start..];
    let end = rest.find([',', '}', ']']).unwrap_or(rest.len());
    Some(rest[..end].trim_matches('"'))
}

/// A zombie has exited even if its parent has not reaped it yet.
fn process_info(pid: i32) -> Option<libc::proc_bsdinfo> {
    // SAFETY: proc_bsdinfo is plain data; zeroed is a valid initial value.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: the output pointer and byte count describe `info` exactly.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            std::ptr::addr_of_mut!(info).cast(),
            i32::try_from(size).expect("proc_bsdinfo size fits i32"),
        )
    };
    (usize::try_from(written).ok() == Some(size)).then_some(info)
}

fn process_exited(pid: i32) -> bool {
    if let Some(info) = process_info(pid) {
        return info.pbi_status == libc::SZOMB;
    }

    // A missing PID is exited; inaccessible or otherwise uncertain identity
    // remains live for this bounded check rather than being treated as done.
    // SAFETY: signal 0 only checks existence and does not signal the process.
    let result = unsafe { libc::kill(pid, 0) };
    result == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

fn wait_process_exit(pid: i32, what: &str) {
    let deadline = Instant::now() + STEP;
    while !process_exited(pid) {
        assert!(Instant::now() < deadline, "{what} did not exit");
        thread::sleep(Duration::from_millis(20));
    }
}

fn first_target(json: &str) -> Option<&str> {
    let start = json.find("\"target\":\"")? + "\"target\":\"".len();
    let rest = &json[start..];
    Some(&rest[..rest.find('"')?])
}

fn target_generation(target: &str) -> u64 {
    assert_eq!(target.split(':').count(), 5, "full target: {target}");
    target
        .split(':')
        .next()
        .expect("target generation")
        .parse()
        .expect("numeric target generation")
}

fn wait_control(what: &str, mut check: impl FnMut() -> Option<String>) -> String {
    let deadline = Instant::now() + STEP;
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

/// Whether `needle` appears in the left (`left == true`) or right half of
/// the rendered screen, using exact cell columns.
fn in_half(client: &Client, needle: &str, left: bool) -> bool {
    let mut parser = vt100::Parser::new(ROWS, COLUMNS, 0);
    parser.process(&client.capture());
    let half = COLUMNS / 2;
    let (start, end) = if left { (0, half) } else { (half, COLUMNS) };
    (0..ROWS).any(|row| {
        parser
            .screen()
            .contents_between(row, start, row, end)
            .contains(needle)
    })
}

fn open_client(fixture: &Fixture) -> Client {
    let mut client = fixture.client(None, ROWS, COLUMNS);
    client.wait_raw_mode();
    // A stopped workspace leaves a saved layout, so plain `panelink` now asks
    // the recovery chooser first (user decision 2026-10-21). Enter keeps
    // today's fresh-shell path.
    let deadline = Instant::now() + STEP;
    loop {
        let screen = client.screen();
        if screen.contains("saved layout from") {
            client.write(b"\r");
            break;
        }
        if screen.contains('%') || screen.contains('$') || Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    client.wait_raw_mode();
    client.wait_screen("shell prompt", |screen| {
        screen.contains("%") || screen.contains("$")
    });
    client
}

fn detach(client: &mut Client) {
    client.write(b"\x18");
    assert_eq!(client.wait_attach_exit(), 0);
    client.termios_restored();
    // Process exit does not synchronize with the outer-PTY drain thread.
    // Wait for the actual restore bytes, including after attach has exited.
    client.wait_until("mouse disabled", |client| {
        client.capture().windows(8).any(|w| w == b"\x1b[?1000l")
    });
}

#[test]
fn wide_terminal_starts_and_reattaches_without_losing_the_shell() {
    let fixture = Fixture::new();
    let mut original_target = None;
    for _ in 0..2 {
        let mut client = fixture.client(None, 64, 269);
        client.wait_raw_mode();
        // Exercise macOS timer coalescing only in our verified private server.
        // Keep the test's outer-PTY drain and UI at normal priority so the
        // regression isolates socket progress rather than terminal-reader lag.
        let pid = fixture.verified_server_pid().expect("fixture-owned server");
        assert!(Command::new("/usr/sbin/taskpolicy")
            .args(["-b", "-p", &pid.to_string()])
            .status()
            .expect("background fixture server")
            .success());
        client.wait_until("wide terminal pane controls and shell prompt", |client| {
            let mut parser = vt100::Parser::new(64, 269, 0);
            parser.process(&client.capture());
            let screen = parser.screen().contents();
            screen.contains(" Menu ") && (screen.contains('%') || screen.contains('$'))
        });
        if original_target.is_none() {
            let (code, output) = fixture.control("wide-reader", &["init"]);
            assert_eq!(code, 0, "initialize layout reader: {output}");
        }
        let layout = wait_control("bounded wide-terminal layout", || {
            let (code, text) = fixture.control("wide-reader", &["layout"]);
            (code == 0 && text.contains("\"columns\":240")).then_some(text)
        });
        let target = first_target(&layout).expect("live pane").to_owned();
        if let Some(original) = &original_target {
            assert_eq!(&target, original, "reattach preserves the exact live shell");
        } else {
            original_target = Some(target);
        }
        detach(&mut client);
        assert!(fixture.server_alive());
    }
    assert_eq!(fixture.stop().0, 0);
}

#[test]
fn bare_panelink_starts_splits_detaches_reattaches_and_stops() {
    let fixture = Fixture::new();

    // First run: no server exists, so `panelink` starts one and attaches.
    let mut client = open_client(&fixture);
    assert!(fixture.socket().exists(), "server socket created");
    client.type_slowly(b"echo ACK$((40+1))\r");
    client.wait_root_marker("ACK41");

    // Right-click opens the split menu; `r` splits right.
    client.write(b"\x1b[<2;10;5M\x1b[<2;10;5m");
    client.wait_screen("split menu", |screen| screen.contains("Split Right"));
    client.write(b"r");
    let (code, init) = fixture.control("probe.ctx", &["init"]);
    assert_eq!(code, 0, "control init: {init}");
    wait_control("two panes", || {
        let (_, layout) = fixture.control("probe.ctx", &["layout"]);
        (field(&layout, "pane_count") == Some("2")).then_some(layout)
    });
    thread::sleep(Duration::from_millis(400));
    client.type_slowly(b"echo ACK$((40+2))\r");
    client.wait_screen("ACK42", |screen| screen.contains("ACK42"));

    // Ctrl-X detaches; the workspace keeps running.
    detach(&mut client);
    drop(client);
    assert!(fixture.server_alive(), "server survives detach");

    // Second run reattaches to both panes with their output intact.
    let mut client = open_client(&fixture);
    client.wait_screen("both panes restored", |screen| {
        screen.contains("ACK41") && screen.contains("ACK42")
    });
    detach(&mut client);
    drop(client);

    // `panelink stop` ends the workspace and its shells.
    let (code, output) = fixture.stop();
    assert_eq!(code, 0, "stop: {output}");
    fixture.wait_server_gone();
    let (code, output) = fixture.stop();
    assert_eq!(code, 0);
    assert!(output.contains("no workspace"), "{output}");

    // A fresh `panelink` starts a new single-pane workspace.
    let mut client = open_client(&fixture);
    let (code, init) = fixture.control("fresh.ctx", &["init"]);
    assert_eq!(code, 0, "init: {init}");
    let (_, layout) = fixture.control("fresh.ctx", &["layout"]);
    assert_eq!(field(&layout, "pane_count"), Some("1"), "{layout}");
    client.type_slowly(b"echo FRESH$((1+1))\r");
    client.wait_screen("FRESH2", |screen| screen.contains("FRESH2"));
    detach(&mut client);
}

#[test]
fn ctrl_b_navigation_mode_banner_split_and_focus_keys() {
    let fixture = Fixture::new();
    let mut client = open_client(&fixture);

    // Ctrl-B shows the navigation banner; Esc hides it and sends nothing.
    client.write(b"\x02");
    client.wait_screen("NAV banner", |screen| screen.contains(" NAV "));
    client.write(b"\x1b");
    client.wait_screen("banner gone", |screen| !screen.contains(" NAV "));

    // Ctrl-B r splits right directly; the new pane takes typing.
    client.write(b"\x02r");
    let (code, init) = fixture.control("nav.ctx", &["init"]);
    assert_eq!(code, 0, "init: {init}");
    wait_control("two panes", || {
        let (_, layout) = fixture.control("nav.ctx", &["layout"]);
        (field(&layout, "pane_count") == Some("2")).then_some(layout)
    });
    thread::sleep(Duration::from_millis(400));
    client.type_slowly(b"echo RIGHT$((1+1))\r");
    client.wait_until("RIGHT2 in right pane", |client| {
        in_half(client, "RIGHT2", false)
    });

    // Ctrl-B Left focuses the left pane; typing lands there.
    client.write(b"\x02\x1b[D\x1b");
    thread::sleep(Duration::from_millis(200));
    client.type_slowly(b"echo LEFT$((1+2))\r");
    client.wait_until("LEFT3 in left pane", |client| {
        in_half(client, "LEFT3", true)
    });
    detach(&mut client);
}

#[test]
fn last_shell_exit_closes_the_workspace_and_the_client() {
    let fixture = Fixture::new();
    let mut client = open_client(&fixture);
    client.type_slowly(b"exit\r");
    let code = client.wait_attach_exit();
    assert_eq!(code, 0, "client exits cleanly when the workspace closes");
    client.termios_restored();
    fixture.wait_server_gone();
    let screen = String::from_utf8_lossy(&client.capture()).into_owned();
    assert!(screen.contains("workspace closed"), "notice shown");
}

#[test]
fn killed_server_leaves_a_stale_socket_that_the_next_open_recovers() {
    let fixture = Fixture::new();
    let mut client = open_client(&fixture);

    // Bind the old control context and full target to this server generation.
    let (code, old_context) = fixture.control("old.ctx", &["init"]);
    assert_eq!(code, 0, "old context init: {old_context}");
    let old_generation: u64 = field(&old_context, "generation")
        .expect("old context generation")
        .parse()
        .expect("numeric old context generation");
    let (code, old_panes) = fixture.control("old.ctx", &["panes"]);
    assert_eq!(code, 0, "old panes: {old_panes}");
    let old_target = first_target(&old_panes)
        .expect("old full target")
        .to_owned();
    assert_eq!(target_generation(&old_target), old_generation);
    let old_context_bytes = fs::read(fixture.root.join("old.ctx")).unwrap();

    let child_pid_path = fixture.root.join("cooperative-child.pid");
    let side_effect_path = fixture.root.join("old-side-effect.log");
    let fixture_name = fixture.root.file_name().unwrap().to_string_lossy();
    let old_marker = format!("P3A_OLD_LIVE_{fixture_name}");
    let side_effect = format!("P3A_SIDE_EFFECT_{fixture_name}");
    let expected_side_effect = format!("{side_effect}\n");
    let child_pid_arg = child_pid_path.to_string_lossy();
    let side_effect_arg = side_effect_path.to_string_lossy();
    // The output marker is assembled by two printf calls so the echoed input
    // cannot satisfy the live-output observation.
    let command = format!(
        "trap 'exit 0' HUP; print -r -- $$ > '{child_pid_arg}'; printf '{side_effect}\\n' >> '{side_effect_arg}'; printf 'P3A_OLD_'; printf 'LIVE_{fixture_name}\\n'"
    );
    client.type_slowly(command.as_bytes());
    client.type_slowly(b"\r");
    client.wait_screen("cooperative old live output", |screen| {
        screen.contains(&old_marker)
    });
    let shell_pid: i32 = fs::read_to_string(&child_pid_path)
        .expect("fixture shell wrote its pid")
        .trim()
        .parse()
        .expect("numeric fixture shell pid");
    assert!(shell_pid > 0);
    assert_ne!(shell_pid, fixture.server_pid().unwrap());
    assert!(
        !process_exited(shell_pid),
        "fixture shell is live before death"
    );
    assert_eq!(
        fs::read(&side_effect_path).unwrap(),
        expected_side_effect.as_bytes()
    );

    detach(&mut client);
    drop(client);
    let pid = fixture
        .verified_server_pid()
        .expect("server PID, owner and kernel start time match private record");
    assert!(pid > 0);
    assert!(
        !process_exited(pid),
        "fixture server is live before SIGKILL"
    );
    // SAFETY: SIGKILL to this test's own background server.
    let signal_result = unsafe { libc::kill(pid, libc::SIGKILL) };
    assert_eq!(
        signal_result,
        0,
        "SIGKILL fixture-owned server: {}",
        io::Error::last_os_error()
    );
    wait_process_exit(pid, "SIGKILLed fixture server");
    assert!(
        fixture.socket().exists(),
        "SIGKILL leaves the socket behind"
    );
    wait_process_exit(shell_pid, "cooperative HUP-trapped fixture shell");
    assert_eq!(
        fs::read(&side_effect_path).unwrap(),
        expected_side_effect.as_bytes()
    );

    let mut client = open_client(&fixture);
    let replacement_pid = fixture
        .server_pid()
        .expect("replacement server pid recorded");
    assert_ne!(replacement_pid, pid, "a new server was started");

    // The old generation-bound context must be refused against the replacement.
    let (code, stale_output) = fixture.control("old.ctx", &["layout"]);
    assert_ne!(code, 0, "old context unexpectedly worked: {stale_output}");
    assert!(stale_output.contains("\"ok\":false"), "{stale_output}");
    assert_eq!(
        fs::read(fixture.root.join("old.ctx")).unwrap(),
        old_context_bytes,
        "rejected context is preserved rather than silently initialized"
    );

    let (code, fresh_context) = fixture.control("fresh.ctx", &["init"]);
    assert_eq!(code, 0, "fresh context init: {fresh_context}");
    let fresh_generation: u64 = field(&fresh_context, "generation")
        .expect("fresh context generation")
        .parse()
        .expect("numeric fresh context generation");
    assert!(
        fresh_generation > old_generation,
        "old={old_context} fresh={fresh_context}"
    );
    let (code, fresh_panes) = fixture.control("fresh.ctx", &["panes"]);
    assert_eq!(code, 0, "fresh panes: {fresh_panes}");
    let fresh_target = first_target(&fresh_panes).expect("fresh full target");
    assert_eq!(target_generation(fresh_target), fresh_generation);
    assert_ne!(
        fresh_target, old_target,
        "fresh target has a new generation"
    );
    let (code, fresh_read) = fixture.control("fresh.ctx", &["read", fresh_target]);
    assert_eq!(code, 0, "fresh read: {fresh_read}");
    assert!(
        !fresh_read.contains(&old_marker),
        "old output absent from fresh read"
    );
    assert_eq!(
        fs::read(&side_effect_path).unwrap(),
        expected_side_effect.as_bytes()
    );
    assert!(
        !client.screen().contains(&old_marker),
        "dead shell output was not replayed into the fresh shell"
    );

    let new_marker = format!("P3A_NEW_LIVE_{fixture_name}");
    let command = format!("printf 'P3A_NEW_'; printf 'LIVE_{fixture_name}\\n'");
    client.type_slowly(command.as_bytes());
    client.type_slowly(b"\r");
    client.wait_screen("replacement live output", |screen| {
        screen.contains(&new_marker)
    });
    let (code, new_read) = fixture.control("fresh.ctx", &["read", fresh_target]);
    assert_eq!(code, 0, "replacement read: {new_read}");
    assert_eq!(new_read.matches(&new_marker).count(), 1, "{new_read}");
    detach(&mut client);
    assert_eq!(
        fs::read(&side_effect_path).unwrap(),
        expected_side_effect.as_bytes()
    );
    drop(client);
    let (code, output) = fixture.stop();
    assert_eq!(code, 0, "stop replacement: {output}");
    fixture.wait_server_gone();
}

#[test]
fn attached_client_exits_cleanly_when_the_workspace_is_stopped_and_restarted() {
    let fixture = Fixture::new();
    let mut client = open_client(&fixture);
    let (code, output) = fixture.stop();
    assert_eq!(code, 0, "stop: {output}");
    // Start a replacement workspace before the old client notices.
    let mut other = open_client(&fixture);
    assert_eq!(client.wait_attach_exit(), 0, "old client leaves cleanly");
    client.termios_restored();
    let notice = String::from_utf8_lossy(&client.capture()).into_owned();
    assert!(notice.contains("workspace closed"), "notice shown");
    other.type_slowly(b"echo NEW$((3+4))\r");
    other.wait_screen("NEW7", |screen| screen.contains("NEW7"));
    detach(&mut other);
}

#[test]
fn concurrent_opens_share_one_server() {
    let fixture = Fixture::new();
    let mut first = fixture.client(None, ROWS, COLUMNS);
    let mut second = fixture.client(None, ROWS, COLUMNS);
    first.wait_raw_mode();
    second.wait_raw_mode();
    first.wait_screen("prompt 1", |screen| {
        screen.contains("%") || screen.contains("$")
    });
    second.wait_screen("prompt 2", |screen| {
        screen.contains("%") || screen.contains("$")
    });
    let (code, init) = fixture.control("c.ctx", &["init"]);
    assert_eq!(code, 0, "init: {init}");
    let (_, layout) = fixture.control("c.ctx", &["layout"]);
    assert_eq!(
        field(&layout, "pane_count"),
        Some("1"),
        "one workspace: {layout}"
    );
    first.write(b"\x18");
    second.write(b"\x18");
    assert_eq!(first.wait_attach_exit(), 0);
    assert_eq!(second.wait_attach_exit(), 0);
}

/// Smoke check with the operator's real environment, shell and dotfiles in
/// an isolated state root. Run explicitly:
/// `cargo test --test p3a_default_open real_environment_smoke -- --ignored --nocapture`
#[test]
#[ignore = "uses the invoking user's real shell configuration"]
fn real_environment_smoke() {
    let fixture = Fixture::new();
    let mut client = fixture.real_environment_client();
    client.wait_raw_mode();
    client.wait_screen("real-environment prompt", |screen| {
        screen.contains('%') || screen.contains('$') || screen.contains('#')
    });
    println!("--- after open ---\n{}", client.screen());
    client.write(b"echo SMOKE$((2+3))\r");
    client.wait_root_marker("SMOKE5");
    println!("--- after echo ---\n{}", client.screen());
    client.write(b"\x1b[<2;10;5M\x1b[<2;10;5m");
    client.wait_screen("real-environment split menu", |screen| {
        screen.contains("Split Right")
    });
    println!("--- menu ---\n{}", client.screen());
    client.write(b"r");
    client.wait_screen("real-environment split", |screen| {
        screen.contains("split applied; new pane focused")
    });
    client.write(b"echo SECOND$((1+1))\r");
    client.wait_screen("second pane smoke output", |screen| {
        screen.contains("SECOND2")
    });
    let after = client.screen();
    println!("--- after split ---\n{after}");
    client.write(b"\x18");
    assert_eq!(client.wait_attach_exit(), 0);
    client.wait_capture_contains(b"\x1b[?1000l", STEP);
    client.termios_restored();
    assert!(after.contains("SMOKE5"), "first pane output");
    assert!(after.contains("SECOND2"), "second pane output");
}
