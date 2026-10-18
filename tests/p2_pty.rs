#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use panelink::launch::{Environment, ResolvedLaunch};
use panelink::terminal::{Key, ScreenSnapshot, TerminalError, TerminalSession, TerminalSize};

const IO_DEADLINE: Duration = Duration::from_secs(3);
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> std::io::Result<Self> {
        for _ in 0..100 {
            let n = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path =
                Path::new("/private/tmp").join(format!("panelink-p2-{}-{n}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
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

fn env(entries: &[(&str, &str)]) -> Environment {
    entries
        .iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect()
}

fn launch(executable: &str, cwd: &Path) -> ResolvedLaunch {
    ResolvedLaunch {
        requested_path: PathBuf::from(executable),
        executable_path: PathBuf::from(executable),
        working_directory: cwd.to_path_buf(),
    }
}

fn shell(script: &str, cwd: &Path, size: TerminalSize) -> TerminalSession {
    TerminalSession::spawn(
        &launch("/bin/sh", cwd),
        &[OsString::from("-c"), OsString::from(script)],
        &env(&[
            ("TERM", "panelink-probe"),
            ("P2_MARKER", "harmless-core-fixture"),
        ]),
        size,
    )
    .expect("spawn shell in PTY")
}

fn snapshot_text(snapshot: &ScreenSnapshot) -> String {
    let mut text = String::new();
    for row in 0..snapshot.rows() {
        for column in 0..snapshot.columns() {
            if let Some(cell) = snapshot.cell(row, column) {
                text.push_str(cell.text());
            }
        }
        text.push('\n');
    }
    text
}

fn pump_until(
    session: &mut TerminalSession,
    timeout: Duration,
    predicate: impl Fn(&ScreenSnapshot) -> bool,
) {
    let deadline = Instant::now() + timeout;
    loop {
        session.pump().expect("bounded PTY pump");
        if predicate(session.snapshot()) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "PTY fixture exceeded its I/O deadline"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn finish(session: &mut TerminalSession) {
    let deadline = Instant::now() + IO_DEADLINE;
    loop {
        let outcome = session.pump().expect("bounded PTY pump while child exits");
        if outcome.pty_eof && outcome.child_exited {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "child did not reach PTY EOF and exit before deadline"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn stdio_is_a_tty_and_child_gets_only_the_explicit_environment_and_cwd() {
    let directory = TempDir::new().unwrap();
    let size = TerminalSize::new(12, 80).unwrap();
    let script = "test -t 0 && test -t 1 && test -t 2 && printf 'TTY-OK\\n'; pwd; printf 'TERM=%s\\nMARK=%s\\n' \"$TERM\" \"$P2_MARKER\"";
    let mut child = shell(script, directory.path(), size);
    finish(&mut child);
    let screen = snapshot_text(child.snapshot());
    assert!(screen.contains("TTY-OK"));
    assert!(screen.contains(directory.path().to_str().unwrap()));
    assert!(screen.contains("TERM=panelink-probe"));
    assert!(screen.contains("MARK=harmless-core-fixture"));
    assert!(child.exit_status().unwrap().success());

    let explicit = env(&[("TERM", "panelink-probe"), ("P2_MARKER", "only-these-two")]);
    let mut env_child = TerminalSession::spawn(
        &launch("/usr/bin/env", directory.path()),
        &[],
        &explicit,
        size,
    )
    .unwrap();
    finish(&mut env_child);
    let output = snapshot_text(env_child.snapshot());
    assert!(output.contains("TERM=panelink-probe"));
    assert!(output.contains("P2_MARKER=only-these-two"));
    assert!(
        !output.contains("PATH="),
        "the process PATH must not be inherited implicitly"
    );
    assert!(
        !output.contains("SHELL="),
        "SHELL must not be synthesized by the core"
    );
    assert_eq!(output.lines().filter(|line| !line.is_empty()).count(), 2);
}

#[test]
fn forged_launch_environment_arguments_and_dimensions_are_rejected_before_spawn() {
    let directory = TempDir::new().unwrap();
    assert!(TerminalSize::new(4, 80).is_err());
    assert!(TerminalSize::new(24, 19).is_err());
    assert!(TerminalSize::new(81, 240).is_err());
    let size = TerminalSize::new(24, 80).unwrap();
    let good_launch = launch("/bin/sh", directory.path());
    let good_environment = env(&[("TERM", "panelink-probe")]);

    let mut bad = good_launch.clone();
    bad.working_directory = directory.path().join("missing");
    assert!(matches!(
        TerminalSession::spawn(&bad, &[], &good_environment, size),
        Err(TerminalError::Io {
            operation: "validate working directory",
            ..
        })
    ));

    let bad_exec = launch("/private/tmp/panelink-no-such-executable", directory.path());
    assert!(matches!(
        TerminalSession::spawn(&bad_exec, &[], &good_environment, size),
        Err(TerminalError::Io {
            operation: "validate executable",
            ..
        }) | Err(TerminalError::InvalidLaunch)
    ));

    let wrong_term = env(&[("TERM", "xterm-256color")]);
    assert!(matches!(
        TerminalSession::spawn(&good_launch, &[], &wrong_term, size),
        Err(TerminalError::InvalidEnvironment)
    ));
    let missing_term = env(&[("P2_MARKER", "harmless")]);
    assert!(matches!(
        TerminalSession::spawn(&good_launch, &[], &missing_term, size),
        Err(TerminalError::InvalidEnvironment)
    ));

    let nul_arg = [OsString::from_vec(b"bad\0arg".to_vec())];
    assert!(matches!(
        TerminalSession::spawn(&good_launch, &nul_arg, &good_environment, size),
        Err(TerminalError::InvalidArgument)
    ));
}

#[test]
fn child_output_populates_separate_normal_and_alternate_screens() {
    let directory = TempDir::new().unwrap();
    let size = TerminalSize::new(10, 60).unwrap();
    let script = "printf 'NORMAL-MARK\\n'; printf '\\033[?1049hALT-MARK'; IFS= read -r ignored; printf '\\033[?1049lRESTORED-MARK\\n'; IFS= read -r ignored";
    let mut child = shell(script, directory.path(), size);
    pump_until(&mut child, IO_DEADLINE, |snapshot| {
        snapshot.alternate_screen() && snapshot_text(snapshot).contains("ALT-MARK")
    });
    child.key(Key::Char('x')).unwrap();
    child.key(Key::Enter).unwrap();
    pump_until(&mut child, IO_DEADLINE, |snapshot| {
        !snapshot.alternate_screen() && snapshot_text(snapshot).contains("RESTORED-MARK")
    });
    let normal = snapshot_text(child.snapshot());
    assert!(normal.contains("NORMAL-MARK"));
    assert!(
        !normal.contains("ALT-MARK"),
        "alternate contents must not overwrite normal buffer"
    );
    child.key(Key::Enter).unwrap();
    finish(&mut child);
}

#[test]
fn paste_framing_follows_engine_bracketed_mode() {
    let directory = TempDir::new().unwrap();
    for (mode, count, expected) in [
        ("", 5, "68656c6c6f"),
        ("\\033[?2004h", 17, "1b5b3230307e68656c6c6f1b5b3230317e"),
    ] {
        let script = format!("/bin/stty raw -echo; printf '{mode}PASTE-READY'; /bin/dd bs=1 count={count} 2>/dev/null | /usr/bin/od -An -tx1");
        let mut child = shell(
            &script,
            directory.path(),
            TerminalSize::new(10, 80).unwrap(),
        );
        pump_until(&mut child, IO_DEADLINE, |snapshot| {
            snapshot_text(snapshot).contains("PASTE-READY")
        });
        assert_eq!(child.snapshot().bracketed_paste(), count == 17);
        assert_eq!(
            child.paste(&"x".repeat(65_537)),
            Err(TerminalError::InputQueueFull)
        );
        assert_eq!(child.pending_input_bytes(), 0);
        child.paste("hello").unwrap();
        finish(&mut child);
        let output = snapshot_text(child.snapshot()).replace(char::is_whitespace, "");
        assert_eq!(output, format!("PASTE-READY{expected}"));
        assert!(child.exit_status().unwrap().success());
    }
}

#[test]
fn four_allowlisted_queries_are_written_back_to_the_child_pty() {
    let directory = TempDir::new().unwrap();
    let script = "/bin/stty raw -echo; printf '\\033[c\\033[5n\\033[6n\\033[18t'; /bin/dd bs=1 count=24 2>/dev/null | /usr/bin/od -An -tx1";
    let mut child = shell(script, directory.path(), TerminalSize::new(8, 80).unwrap());
    finish(&mut child);
    let raw_output = snapshot_text(child.snapshot());
    let output = raw_output.replace(char::is_whitespace, "");
    assert!(
        output.contains("1b5b3f31631b5b306e1b5b313b31521b5b383b383b383074"),
        "query replies must reach the child PTY as exact bytes, got {raw_output:?}"
    );
    assert!(child.exit_status().unwrap().success());
}

#[test]
fn clipboard_denial_does_not_inject_reply_bytes_before_the_next_shell_input() {
    let directory = TempDir::new().unwrap();
    let script = "/bin/stty -echo; printf '\\033]52;c;YWJj\\007\\033]52;c;\\007\\033]52;c;?\\007\\033]52;c;?\\033\\\\'; printf 'CLIP-READY\\n'; IFS= read -r received; printf '%s' \"$received\" | /usr/bin/od -An -tx1";
    let mut child = shell(script, directory.path(), TerminalSize::new(8, 80).unwrap());
    let deadline = Instant::now() + IO_DEADLINE;
    loop {
        let progress = child.pump().unwrap();
        assert_eq!(
            progress.bytes_written, 0,
            "clipboard denial injected child input"
        );
        assert_eq!(child.pending_input_bytes(), 0);
        if snapshot_text(child.snapshot()).contains("CLIP-READY") {
            break;
        }
        assert!(Instant::now() < deadline, "clipboard fixture not ready");
        thread::sleep(Duration::from_millis(5));
    }
    child.text("NEXT-LINE", true).unwrap();
    finish(&mut child);
    let raw_output = snapshot_text(child.snapshot());
    let output = raw_output.replace(char::is_whitespace, "");
    assert_eq!(
        output, "CLIP-READY4e4558542d4c494e45",
        "the shell must receive only the explicit next input: {raw_output:?}"
    );
    assert!(child.exit_status().unwrap().success());
}

#[test]
fn resize_coalesces_and_commits_only_the_newest_successful_size() {
    let directory = TempDir::new().unwrap();
    let script = "IFS= read -r ignored; /bin/stty size";
    let mut child = shell(script, directory.path(), TerminalSize::new(8, 20).unwrap());
    child
        .request_resize(TerminalSize::new(15, 40).unwrap())
        .unwrap();
    child
        .request_resize(TerminalSize::new(12, 35).unwrap())
        .unwrap();
    let report = child.pump().unwrap();
    assert!(report.bytes_written <= 16 * 1024);
    assert_eq!(child.size(), TerminalSize::new(12, 35).unwrap());
    child.key(Key::Enter).unwrap();
    finish(&mut child);
    assert!(snapshot_text(child.snapshot()).contains("12 35"));
    assert_eq!(child.snapshot().rows(), 12);
    assert_eq!(child.snapshot().columns(), 35);
}

#[test]
fn pending_input_is_discarded_once_child_exit_is_observed() {
    let directory = TempDir::new().unwrap();
    let mut child = shell(
        "exit 0",
        directory.path(),
        TerminalSize::new(8, 20).unwrap(),
    );
    child.paste(&"x".repeat(65_536)).unwrap();
    let deadline = Instant::now() + IO_DEADLINE;
    loop {
        let report = match child.pump() {
            Ok(report) => report,
            Err(TerminalError::Io {
                operation: "write PTY",
                ..
            }) => {
                // Closing the slave can race the status poll in pump. The
                // hard-write path must discard input and reap, not replay it.
                assert_eq!(child.pending_input_bytes(), 0);
                assert!(child.exit_status().is_some());
                break;
            }
            Err(error) => panic!("unexpected pump failure: {error}"),
        };
        if report.child_exited {
            assert_eq!(report.bytes_written, 0);
            assert_eq!(child.pending_input_bytes(), 0);
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(child.key(Key::Enter), Err(TerminalError::InputDisabled));
}

#[test]
fn hup_ignoring_child_is_killed_reaped_and_cleanup_is_cached() {
    let directory = TempDir::new().unwrap();
    let mut child = shell(
        "trap '' HUP; printf 'HUP-READY'; while :; do /bin/sleep 10; done",
        directory.path(),
        TerminalSize::new(8, 20).unwrap(),
    );
    pump_until(&mut child, IO_DEADLINE, |snapshot| {
        snapshot_text(snapshot).contains("HUP-READY")
    });
    let started = Instant::now();
    child.shutdown().unwrap();
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert!(
        started.elapsed() < Duration::from_millis(2_500),
        "cleanup must stay within HUP plus KILL windows"
    );
    assert_eq!(
        child.exit_status().unwrap().signal(),
        Some(rustix::process::Signal::KILL.as_raw())
    );
    let after_first = Instant::now();
    child.shutdown().unwrap();
    assert!(
        after_first.elapsed() < Duration::from_millis(50),
        "cached cleanup must prevent a second wait window"
    );
    assert_eq!(child.pending_input_bytes(), 0);
    assert_eq!(child.pump(), Err(TerminalError::SessionClosed));
    assert_eq!(
        child.request_resize(TerminalSize::new(10, 30).unwrap()),
        Err(TerminalError::SessionClosed)
    );
    assert_eq!(child.paste("closed"), Err(TerminalError::InputDisabled));
}

#[test]
fn final_output_larger_than_one_chunk_is_drained_after_exit() {
    let directory = TempDir::new().unwrap();
    let mut child = shell(
        "printf '%040000d' 0; printf '\\r\\nFINAL-DRAIN-MARK'",
        directory.path(),
        TerminalSize::new(8, 80).unwrap(),
    );
    let deadline = Instant::now() + IO_DEADLINE;
    let mut read = 0;
    loop {
        let report = child.pump().unwrap();
        read += report.bytes_read;
        if report.child_exited {
            assert_eq!(child.pending_input_bytes(), 0);
            assert_eq!(report.bytes_written, 0);
        }
        if report.child_exited && report.pty_eof {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(read > 40_000);
    assert!(snapshot_text(child.snapshot()).contains("FINAL-DRAIN-MARK"));
    assert!(child.exit_status().unwrap().success());
}

#[test]
fn output_flood_keeps_engine_and_snapshot_within_visible_bounds() {
    let directory = TempDir::new().unwrap();
    let size = TerminalSize::new(8, 20).unwrap();
    let combining = "\u{301}".repeat(5_000);
    let script = format!(
        "printf 'X{combining}'; printf '\\033]52;c;?'; printf '%020000d' 0; printf '\\007'"
    );
    let mut child = TerminalSession::spawn(
        &launch("/bin/sh", directory.path()),
        &[OsString::from("-c"), OsString::from(script)],
        &env(&[("TERM", "panelink-probe")]),
        size,
    )
    .unwrap();
    let deadline = Instant::now() + IO_DEADLINE;
    loop {
        let report = child.pump().unwrap();
        assert!(report.bytes_read <= 16 * 1024);
        assert_eq!(
            child.snapshot().cells().len(),
            usize::from(size.rows()) * usize::from(size.columns())
        );
        assert!(child.snapshot().cell(0, 0).unwrap().text().chars().count() <= 9);
        if report.pty_eof && report.child_exited {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "flood fixture exceeded its I/O deadline"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

use std::os::unix::ffi::OsStringExt;
