#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Real outer-PTY scrolling regressions for `panelink attach`.

use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
use rustix::process::{ioctl_tiocsctty, setsid};
use rustix::termios::Winsize;
use rustix_openpty::openpty;

const ROWS: u16 = 24;
const COLUMNS: u16 = 100;
const WAIT: Duration = Duration::from_secs(12);
const MAX_CAPTURE: usize = 8 * 1024 * 1024;
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    runtime: PathBuf,
    terminfo: PathBuf,
    home: PathBuf,
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/pl-p3-scroll-attach-{}-{}",
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
            runtime: root.join("runtime"),
            terminfo: root.join("terminfo"),
            home: root.join("home"),
            root,
        };
        make(&fixture.runtime);
        make(&fixture.terminfo);
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
            .env("TERM", "panelink-probe")
            .current_dir(&self.home);
        command
    }

    fn serve(&self, script: &str) -> Server {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/terminfo/panelink-probe.terminfo");
        let status = Command::new("/usr/bin/tic")
            .args(["-x", "-o"])
            .arg(&self.terminfo)
            .arg(source)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .status()
            .expect("run tic");
        assert!(status.success());
        Server(
            self.command()
                .arg("serve")
                .arg("--directory")
                .arg(&self.runtime)
                .arg("--terminfo")
                .arg(&self.terminfo)
                .args([
                    "--initialize",
                    "--allow-input",
                    "--",
                    "/bin/sh",
                    "-c",
                    script,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn fixture-owned panelink server"),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Attach {
    child: Child,
    master: fs::File,
    capture: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    drain: Option<thread::JoinHandle<()>>,
}

impl Attach {
    fn spawn(fixture: &Fixture) -> Self {
        let winsize = Winsize {
            ws_row: ROWS,
            ws_col: COLUMNS,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pty = openpty(None, Some(&winsize)).expect("open outer PTY");
        let master = fs::File::from(pty.controller);
        let flags = fcntl_getfl(&master).unwrap();
        fcntl_setfl(&master, flags | OFlags::NONBLOCK).unwrap();
        let slave = pty.user;
        let controlling = slave.as_fd().try_clone_to_owned().unwrap();
        let clone_slave =
            || Stdio::from(fs::File::from(slave.as_fd().try_clone_to_owned().unwrap()));
        let mut command = fixture.command();
        command
            .arg("attach")
            .arg("--directory")
            .arg(&fixture.runtime)
            .arg("--context-dir")
            .arg(fixture.root.join("contexts"));
        command
            .stdin(clone_slave())
            .stdout(clone_slave())
            .stderr(clone_slave());
        // SAFETY: the child starts a new session and installs this fixture's
        // private outer PTY as its controlling terminal.
        unsafe {
            command.pre_exec(move || {
                setsid().map_err(io::Error::from)?;
                ioctl_tiocsctty(&controlling).map_err(io::Error::from)?;
                Ok(())
            });
        }
        let child = command.spawn().expect("spawn fixture-owned attach client");
        drop(command);
        let capture = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let mut reader = master.try_clone().unwrap();
        let drain = {
            let capture = Arc::clone(&capture);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                let mut buffer = [0_u8; 8192];
                while !stop.load(Ordering::Relaxed) {
                    match reader.read(&mut buffer) {
                        Ok(0) => thread::sleep(Duration::from_millis(2)),
                        Ok(count) => {
                            let mut capture = capture.lock().unwrap();
                            if capture.len() + count <= MAX_CAPTURE {
                                capture.extend_from_slice(&buffer[..count]);
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(2)),
                    }
                }
            })
        };
        Self {
            child,
            master,
            capture,
            stop,
            drain: Some(drain),
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        let end = Instant::now() + WAIT;
        let mut offset = 0;
        while offset < bytes.len() {
            match self.master.write(&bytes[offset..]) {
                Ok(n) => offset += n,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < end, "outer PTY input stalled");
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("write outer PTY: {error}"),
            }
        }
    }

    fn captured(&self) -> Vec<u8> {
        self.capture.lock().unwrap().clone()
    }

    fn screen(&self) -> String {
        let mut parser = vt100::Parser::new(ROWS, COLUMNS, 0);
        parser.process(&self.captured());
        parser.screen().contents()
    }

    fn wait_for(&mut self, description: &str, predicate: impl Fn(&str) -> bool) -> String {
        let end = Instant::now() + WAIT;
        loop {
            let screen = self.screen();
            if predicate(&screen) {
                return screen;
            }
            if let Some(status) = self.child.try_wait().expect("query attach status") {
                panic!("attach exited ({status}) waiting for {description}: {screen}");
            }
            assert!(
                Instant::now() < end,
                "timed out waiting for {description}:\n{screen}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Attach {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(drain) = self.drain.take() {
            let _ = drain.join();
        }
    }
}

fn wait_server_ready(fixture: &Fixture, server: &mut Server) {
    let end = Instant::now() + WAIT;
    loop {
        let socket = fixture.runtime.join("control.sock");
        if socket.exists() {
            return;
        }
        if let Some(status) = server.0.try_wait().expect("query server status") {
            let mut stdout = String::new();
            let mut stderr = String::new();
            server
                .0
                .stdout
                .take()
                .unwrap()
                .read_to_string(&mut stdout)
                .expect("read server stdout");
            server
                .0
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut stderr)
                .expect("read server stderr");
            panic!("fixture server exited ({status}): stdout={stdout} stderr={stderr}");
        }
        assert!(Instant::now() < end, "server socket was not created");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn attached_sgr_wheel_scrolls_normal_history_and_wheel_down_or_typing_returns_live() {
    let fixture = Fixture::new();
    let script = "i=0; while [ $i -lt 80 ]; do printf 'P3HISTORY-LINE-%02d\\n' $i; i=$((i+1)); done; exec /bin/sh -i";
    let mut server = fixture.serve(script);
    wait_server_ready(&fixture, &mut server);

    let mut attach = Attach::spawn(&fixture);
    attach.wait_for("newest normal-screen output", |screen| {
        screen.contains("P3HISTORY-LINE-79")
    });
    // Outer coordinate (3,3) is two cells inside the pane's padded content.
    attach.send(b"\x1b[<64;3;3M");
    let historical = attach.wait_for("older retained output after wheel-up", |screen| {
        !screen.contains("P3HISTORY-LINE-79")
            && (screen.contains("P3HISTORY-LINE-78") || screen.contains("P3HISTORY-LINE-77"))
    });
    assert!(
        !historical.contains("P3HISTORY-LINE-79"),
        "history view moved: {historical}"
    );

    attach.send(b"\x1b[<65;3;3M");
    attach.wait_for("live output after wheel-down", |screen| {
        screen.contains("P3HISTORY-LINE-79")
    });
    attach.send(b"\x1b[<64;3;3M");
    attach.wait_for("older output before typing", |screen| {
        !screen.contains("P3HISTORY-LINE-79") && screen.contains("P3HISTORY-LINE-77")
    });
    attach.send(b"printf 'P3LIVE-AFTER-%s\\n' TYPE\r");
    attach.wait_for("live output after typing", |screen| {
        screen.contains("P3LIVE-AFTER-TYPE")
    });

    let _ = unsafe { libc::kill(server.0.id() as i32, libc::SIGTERM) };
    let _ = server.0.wait();
}

#[test]
fn attached_sgr_wheel_reaches_alternate_screen_mouse_app_in_content_coordinates() {
    let fixture = Fixture::new();
    // 10-byte SGR wheel events are captured directly from the app's raw PTY.
    // Outer one-based (3,3) maps past the one-cell inset to content-local
    // zero-based (1,1), then returns to the child as one-based SGR (2,2).
    let script = "stty raw -echo; printf '\\033[?1049h\\033[?1000h\\033[?1006hP3MOUSE-READY\\r\\n'; for n in 1 2; do dd bs=10 count=1 2>/dev/null | od -An -tx1; printf '\\r\\n'; done; exec /bin/sleep 60";
    let mut server = fixture.serve(script);
    wait_server_ready(&fixture, &mut server);

    let mut attach = Attach::spawn(&fixture);
    attach.wait_for("alternate-screen mouse app", |screen| {
        screen.contains("P3MOUSE-READY")
    });
    attach.send(b"\x1b[<64;3;3M");
    let up = attach.wait_for("captured up app wheel bytes", |screen| {
        screen
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("1b 5b 3c 36 34 3b 32 3b 32 4d")
    });
    assert!(
        up.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("1b 5b 3c 36 34 3b 32 3b 32 4d"),
        "up bytes: {up}"
    );
    attach.send(b"\x1b[<65;3;3M");
    let down = attach.wait_for("captured down app wheel bytes", |screen| {
        screen
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("1b 5b 3c 36 35 3b 32 3b 32 4d")
    });
    assert!(
        down.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .contains("1b 5b 3c 36 35 3b 32 3b 32 4d"),
        "down bytes: {down}"
    );

    let _ = unsafe { libc::kill(server.0.id() as i32, libc::SIGTERM) };
    let _ = server.0.wait();
}
