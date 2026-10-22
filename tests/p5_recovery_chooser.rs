#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::control_protocol::TargetRef;
use panelink::layout_store::LayoutStore;
use panelink::pane_layout::{LayoutSnapshot, PaneGeometry, Rect};
use panelink::recovery_chooser::{self, ChooserChoice, ChooserOptions, ChooserPlan, LayoutSummary};
use panelink::saved_layout::{LayoutMetadata, SavedLayout};
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/panelink-recovery-chooser-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join("server"))
            .unwrap();
        let marker = root.join("shell-ran");
        let shell = root.join("marker-shell");
        fs::write(
            &shell,
            format!(
                "#!/bin/sh\nprintf ran > '{}'\nexec /bin/sleep 30\n",
                marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
        Self { root }
    }

    fn server(&self) -> PathBuf {
        self.root.join("server")
    }

    fn marker(&self) -> PathBuf {
        self.root.join("shell-ran")
    }

    fn assert_no_runtime_effects(&self) {
        assert!(!self.server().join("server.lock").exists());
        assert!(!self.server().join("control.sock").exists());
        assert!(!self.server().join("server.pid").exists());
        assert!(!self.root.join("terminfo").exists());
        assert!(!self.root.join("contexts").exists());
        assert!(!self.root.join("lifecycle.lock").exists());
        assert!(!self.marker().exists());
    }

    fn write_current(&self) {
        let target = TargetRef {
            generation: 1,
            workspace: 2,
            pane: 3,
            session: 4,
            occupant_revision: 5,
        };
        let metadata = LayoutMetadata::new(1, 2, 3, 4, 1_000_000).unwrap();
        let snapshot = LayoutSnapshot {
            revision: 4,
            rows: 10,
            columns: 30,
            panes: [
                Some(PaneGeometry {
                    target,
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows: 10,
                        columns: 30,
                    },
                }),
                None,
                None,
                None,
            ],
            divider: None,
            dividers: [None; panelink::pane_layout::MAX_PANES - 1],
        };
        let layout = SavedLayout::from_snapshot(metadata, &snapshot).unwrap();
        let store = LayoutStore::prepare(&self.server()).unwrap();
        store.save(1, 2, &layout).unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_panelink"));
        command
            .env_clear()
            .env("PANELINK_HOME", &self.root)
            .env("HOME", &self.root)
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", self.root.join("marker-shell"))
            .env("TERM", "xterm-256color")
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn run_noninteractive(&self) -> std::process::Output {
        let mut child = self.command().spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("plain panelink exceeded the 12 second fixture deadline");
            }
            thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_panelink"))
            .arg("stop")
            .env_clear()
            .env("PANELINK_HOME", &self.root)
            .env("HOME", &self.root)
            .env("PATH", "/usr/bin:/bin")
            .output();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn current_summary() -> LayoutSummary {
    LayoutSummary {
        pane_count: 1,
        observed_at_unix_ms: 1_000,
    }
}

fn previous_summary() -> LayoutSummary {
    LayoutSummary {
        pane_count: 2,
        observed_at_unix_ms: 2_000,
    }
}

#[test]
fn chooser_plan_exhaustively_covers_live_interactive_and_saved_slot_presence() {
    for live in [false, true] {
        for interactive in [false, true] {
            for current in [None, Some(current_summary())] {
                for previous in [None, Some(previous_summary())] {
                    let actual = recovery_chooser::plan(live, interactive, current, previous);
                    let expected = if live {
                        ChooserPlan::AttachExisting
                    } else if !interactive || current.is_none() {
                        ChooserPlan::FreshShell
                    } else {
                        ChooserPlan::Prompt {
                            options: ChooserOptions {
                                view_current: true,
                                view_previous: previous.is_some(),
                            },
                        }
                    };
                    assert_eq!(actual, expected);
                }
            }
        }
    }
}

#[test]
fn every_input_byte_maps_to_a_choice_and_unknowns_default_fresh() {
    for byte in u8::MIN..=u8::MAX {
        let expected = match byte {
            b'v' | b'V' => ChooserChoice::ViewCurrent,
            b'p' | b'P' => ChooserChoice::ViewPrevious,
            b'q' | b'Q' => ChooserChoice::Quit,
            _ => ChooserChoice::Fresh,
        };
        assert_eq!(recovery_chooser::choice(byte), expected);
    }
}

#[test]
fn previous_layout_without_current_does_not_offer_the_chooser() {
    assert_eq!(
        recovery_chooser::plan(false, true, None, Some(previous_summary())),
        ChooserPlan::FreshShell
    );
}

#[test]
fn relative_age_uses_only_the_supplied_summary_and_now() {
    assert_eq!(
        recovery_chooser::relative_age(current_summary(), 1_000),
        "less than a second ago"
    );
    assert_eq!(
        recovery_chooser::relative_age(current_summary(), 61_000),
        "1m ago"
    );
    assert_eq!(
        recovery_chooser::relative_age(current_summary(), 3_601_000),
        "1h ago"
    );
    assert_eq!(
        recovery_chooser::relative_age(current_summary(), 86_401_000),
        "1d ago"
    );
    assert_eq!(
        recovery_chooser::relative_age(previous_summary(), 1),
        "less than a second ago"
    );
}

#[test]
fn saved_layout_with_piped_streams_keeps_the_noninteractive_fresh_shell_path() {
    // Parent correction: plain `panelink` always needs a terminal for attach,
    // so a piped run exits nonzero after starting the server (unchanged
    // baseline behavior). The decision under test is only that NO chooser or
    // viewer runs and the normal fresh-shell startup path is taken.
    let fixture = Fixture::new();
    fixture.write_current();
    let output = fixture.run_noninteractive();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("saved layout from"));
    assert!(!stderr.contains("saved layout from"));
    assert!(!stdout.contains("recovery viewer"));
    assert!(!stderr.contains("recovery viewer"));
    // Fresh-shell path reached: lifecycle/runtime paths were prepared.
    assert!(fixture.root.join("lifecycle.lock").exists());
}

#[test]
fn absent_saved_layout_with_piped_streams_keeps_the_noninteractive_fresh_shell_path() {
    let fixture = Fixture::new();
    let output = fixture.run_noninteractive();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("saved layout from"));
    assert!(!stderr.contains("saved layout from"));
    assert!(!stdout.contains("recovery viewer"));
    assert!(!stderr.contains("recovery viewer"));
    assert!(fixture.root.join("lifecycle.lock").exists());
}

#[test]
fn private_pty_quit_exits_without_starting_a_server_or_shell() {
    let fixture = Fixture::new();
    fixture.write_current();
    let script = r##"
import ast, os, pty, select, signal, sys, termios, time
binary, root = sys.argv[1:]
read_original, write_original = os.pipe()
pid, fd = pty.fork()
if pid == 0:
    original = termios.tcgetattr(0)
    os.write(write_original, repr(original).encode())
    os.close(write_original)
    os.close(read_original)
    os.environ.clear()
    os.environ.update({"PANELINK_HOME": root, "HOME": root, "PATH": "/usr/bin:/bin", "SHELL": root + "/marker-shell", "TERM": "xterm-256color"})
    os.execv(binary, [binary])
os.close(write_original)
original_termios = ast.literal_eval(os.read(read_original, 4096).decode())
os.close(read_original)
deadline = time.monotonic() + 12
captured = bytearray()
sent = False
status = None
while time.monotonic() < deadline:
    ready, _, _ = select.select([fd], [], [], 0.05)
    if ready:
        try:
            chunk = os.read(fd, 4096)
        except OSError:
            chunk = b""
        captured.extend(chunk)
        if not sent and b"[q] quit" in captured:
            time.sleep(0.05)
            os.write(fd, b"q")
            sent = True
    waited, status = os.waitpid(pid, os.WNOHANG)
    if waited == pid:
        break
if status is None:
    os.kill(pid, signal.SIGKILL)
    os.waitpid(pid, 0)
    raise SystemExit("BLOCKED: interactive chooser did not exit within 12 seconds")
if not sent or not os.WIFEXITED(status) or os.WEXITSTATUS(status) != 0:
    raise SystemExit("interactive chooser did not display its prompt and exit cleanly: " + repr(bytes(captured[-2048:])))
if termios.tcgetattr(fd) != original_termios:
    raise SystemExit("interactive chooser did not restore the PTY termios state")
text = bytes(captured)
if b"saved layout from" not in text or b"[Enter]" not in text or b"[v]" not in text:
    raise SystemExit("chooser prompt was incomplete: " + repr(text[-2048:]))
"##;
    let result = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(env!("CARGO_BIN_EXE_panelink"))
        .arg(&fixture.root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/opt/homebrew/bin")
        .output();
    let output = match result {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            panic!("BLOCKED: python3 unavailable; private-PTY chooser scenario not executed");
        }
        Err(error) => panic!("could not start the private-PTY helper: {error}"),
    };
    assert!(
        output.status.success(),
        "private-PTY helper failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    fixture.assert_no_runtime_effects();
}
