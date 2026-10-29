#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Opt-in private-PTY baseline for alternate-screen wheel delivery.

use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
use rustix::process::{ioctl_tiocsctty, setsid};
use rustix::termios::Winsize;
use rustix_openpty::openpty;
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
            "/private/tmp/pl-codex-wheel-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let make = |p: &Path| {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(p)
                .expect("create private fixture directory")
        };
        make(&root);
        let f = Self {
            runtime: root.join("runtime"),
            terminfo: root.join("terminfo"),
            home: root.join("home"),
            root,
        };
        make(&f.runtime);
        make(&f.terminfo);
        make(&f.home);
        f
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_panelink"));
        c.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("ZDOTDIR", &self.home)
            .env("PANELINK_HOME", &self.home)
            .env("TERM", "panelink-probe")
            .current_dir(&self.home);
        c
    }
    fn server_binary(&self) -> PathBuf {
        let path = std::env::var_os("PANELINK_WHEEL_BENCH_SERVER")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_panelink")));
        assert!(
            path.is_absolute() && path.is_file(),
            "benchmark server must be an existing absolute file: {}",
            path.display()
        );
        path
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
        let mut c = Command::new(self.server_binary());
        c.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("ZDOTDIR", &self.home)
            .env("PANELINK_HOME", &self.home)
            .env("TERM", "panelink-probe")
            .current_dir(&self.home);
        Server(
            c.arg("serve")
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
                .expect("spawn fixture-owned server"),
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
    fn spawn(f: &Fixture) -> Self {
        let ws = Winsize {
            ws_row: ROWS,
            ws_col: COLUMNS,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pty = openpty(None, Some(&ws)).expect("open outer PTY");
        let master = fs::File::from(pty.controller);
        let flags = fcntl_getfl(&master).unwrap();
        fcntl_setfl(&master, flags | OFlags::NONBLOCK).unwrap();
        let slave = pty.user;
        let controlling = slave.as_fd().try_clone_to_owned().unwrap();
        let clone_slave =
            || Stdio::from(fs::File::from(slave.as_fd().try_clone_to_owned().unwrap()));
        let mut c = f.command();
        c.arg("attach")
            .arg("--directory")
            .arg(&f.runtime)
            .arg("--context-dir")
            .arg(f.root.join("contexts"))
            .stdin(clone_slave())
            .stdout(clone_slave())
            .stderr(clone_slave());
        // SAFETY: child installs this fixture's PTY as its controlling terminal.
        unsafe {
            c.pre_exec(move || {
                setsid().map_err(io::Error::from)?;
                ioctl_tiocsctty(&controlling).map_err(io::Error::from)?;
                Ok(())
            });
        }
        let child = c.spawn().expect("spawn fixture-owned attach");
        drop(c);
        let capture = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let mut reader = master.try_clone().unwrap();
        let drain = {
            let capture = Arc::clone(&capture);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                let mut b = [0; 8192];
                while !stop.load(Ordering::Relaxed) {
                    match reader.read(&mut b) {
                        Ok(0) => thread::sleep(Duration::from_millis(2)),
                        Ok(n) => {
                            let mut g = capture.lock().unwrap();
                            if g.len() + n <= MAX_CAPTURE {
                                g.extend_from_slice(&b[..n]);
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2))
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
    fn send(&mut self, b: &[u8]) {
        let end = Instant::now() + WAIT;
        let mut off = 0;
        while off < b.len() {
            match self.master.write(&b[off..]) {
                Ok(n) => off += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < end, "outer PTY input stalled");
                    thread::sleep(Duration::from_millis(2));
                }
                Err(e) => panic!("write outer PTY: {e}"),
            }
        }
    }
    fn screen(&self) -> String {
        let mut p = vt100::Parser::new(ROWS, COLUMNS, 0);
        p.process(&self.capture.lock().unwrap());
        p.screen().contents()
    }
    fn wait_for(&mut self, what: &str, pred: impl Fn(&str) -> bool) -> String {
        let end = Instant::now() + WAIT;
        loop {
            let s = self.screen();
            if pred(&s) {
                return s;
            }
            if let Some(st) = self.child.try_wait().expect("query attach status") {
                panic!("attach exited ({st}) waiting for {what}: {s}");
            }
            assert!(Instant::now() < end, "timeout waiting for {what}:\n{s}");
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
        if let Some(j) = self.drain.take() {
            let _ = j.join();
        }
    }
}
fn wait_server_ready(f: &Fixture, s: &mut Server) {
    let end = Instant::now() + WAIT;
    loop {
        if f.runtime.join("control.sock").exists() {
            return;
        }
        if let Some(st) = s.0.try_wait().expect("query server status") {
            let mut out = String::new();
            let mut err = String::new();
            let _ = s.0.stdout.take().unwrap().read_to_string(&mut out);
            let _ = s.0.stderr.take().unwrap().read_to_string(&mut err);
            panic!("server exited ({st}): stdout={out} stderr={err}");
        }
        assert!(Instant::now() < end, "server socket not created");
        thread::sleep(Duration::from_millis(10));
    }
}
fn visible_count(screen: &str) -> usize {
    screen
        .lines()
        .find_map(|l| {
            l.split("COUNT-")
                .nth(1)?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
        .unwrap_or(0)
}
fn fixture_case(paced: bool) -> (usize, Duration, Duration, Duration, usize) {
    let fixture = Fixture::new();
    let log = fixture.root.join("reports.log");
    let quote = |path: &Path| format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"));
    let script = format!(
        "stty raw -echo; PANELINK_WHEEL_RECEIVER_LOG={} exec {} --exact wheel_receiver_child --ignored --nocapture",
        quote(&log), quote(&std::env::current_exe().unwrap())
    );
    let mut server = fixture.serve(&script);
    wait_server_ready(&fixture, &mut server);
    let mut attach = Attach::spawn(&fixture);
    attach.wait_for("READY from attached child", |s| s.contains("READY"));
    let start = Instant::now();
    let event = b"\x1b[<64;3;3M";
    let mut first = None;
    let mut last_visible = None;
    if paced {
        for i in 1..=20 {
            attach.send(event);
            let s = attach.wait_for("paced visible count", |s| visible_count(s) >= i);
            let _ = s;
            if first.is_none() {
                first = Some(start.elapsed());
            }
            last_visible = Some(start.elapsed());
        }
    } else {
        let burst: Vec<u8> = event.repeat(20);
        attach.send(&burst);
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut last = 0;
        let mut stable = Instant::now();
        while Instant::now() < deadline {
            let n = visible_count(&attach.screen());
            if n > 0 && n != last {
                last = n;
                stable = Instant::now();
                if first.is_none() {
                    first = Some(start.elapsed());
                }
                last_visible = Some(start.elapsed());
            }
            if n > 0 && stable.elapsed() >= Duration::from_secs(1) {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
    let duration = start.elapsed();
    let log_deadline = Instant::now() + Duration::from_secs(1);
    while !log.exists() && Instant::now() < log_deadline {
        thread::sleep(Duration::from_millis(2));
    }
    let text = fs::read_to_string(&log).expect("receiver sample block");
    let reports = text.lines().filter(|l| l.parse::<u128>().is_ok()).count();
    let final_count = visible_count(&attach.screen());
    (
        reports,
        first.expect("child report must become visible"),
        last_visible.expect("last child response must become visible"),
        duration,
        final_count,
    )
}
#[test]
#[ignore = "private PTY baseline; run explicitly"]
fn attached_alternate_screen_wheel_latency_baseline() {
    for (label, paced) in [("paced", true), ("burst", false)] {
        let (n, first, last, observe, final_count) = fixture_case(paced);
        println!(
            "{label}: child_report_events={n}/20 first_visible_response_ms={:.1} last_visible_response_ms={:.1} final_visible_count={final_count} observation_duration_ms={:.1}",
            first.as_secs_f64() * 1000.0,
            last.as_secs_f64() * 1000.0,
            observe.as_secs_f64() * 1000.0
        );
        assert_eq!(n, 20, "healthy {label} gesture should deliver every report");
        assert_eq!(final_count, 20, "all {label} reports should become visible");
    }
}

#[test]
#[ignore = "private fixture receiver child"]
fn wheel_receiver_child() {
    let Some(path) = std::env::var_os("PANELINK_WHEEL_RECEIVER_LOG") else {
        return;
    };
    let mut input = io::stdin();
    let mut output = io::stdout();
    output
        .write_all(b"\x1b[?1049h\x1b[?1000h\x1b[?1006hREADY\r\n")
        .unwrap();
    output.flush().unwrap();
    let flags = fcntl_getfl(&input).unwrap();
    fcntl_setfl(&input, flags | OFlags::NONBLOCK).unwrap();
    let start = Instant::now();
    let deadline = start + Duration::from_secs(10);
    let mut pending = Vec::with_capacity(32);
    let mut samples = Vec::with_capacity(20);
    let mut buffer = [0u8; 4096];
    while samples.len() < 20 && Instant::now() < deadline {
        match input.read(&mut buffer) {
            Ok(n) if n > 0 => {
                pending.extend_from_slice(&buffer[..n]);
                while let Some(end) = pending.iter().position(|b| *b == b'M') {
                    let report: Vec<_> = pending.drain(..=end).collect();
                    if report == b"\x1b[<64;2;2M" {
                        samples.push(start.elapsed().as_micros());
                        output
                            .write_all(
                                format!("\x1b[5;1HCOUNT-{:02}\x1b[K", samples.len()).as_bytes(),
                            )
                            .unwrap();
                        output.flush().unwrap();
                    } else {
                        panic!("unexpected SGR report in private fixture");
                    }
                }
                assert!(pending.len() <= 32, "oversized partial SGR input");
            }
            Ok(_) => thread::sleep(Duration::from_millis(2)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2))
            }
            Err(e) => panic!("receiver stdin: {e}"),
        }
    }
    assert_eq!(samples.len(), 20, "receiver timed out before 20 reports");
    fs::write(
        path,
        samples
            .iter()
            .map(u128::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();
    thread::sleep(Duration::from_secs(2));
    output
        .write_all(b"\x1b[?1006l\x1b[?1000l\x1b[?1049l")
        .unwrap();
    output.flush().unwrap();
}
