#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! End-to-end pathname-socket proof of `panelink serve` plus the control CLI:
//! one zsh child at start, a CLI-driven Split Right creating a second zsh
//! pane, independent typed input in each pane, and cleanup on SIGTERM.
//!
//! Any startup failure fails the test. The only exception: when
//! `PANELINK_ALLOW_PATHNAME_SKIP=1` is set and serve reports
//! `listener_permission_denied` (bind EPERM/EACCES, an environment sandbox),
//! the test prints a SKIPPED line and returns without exercising the
//! scenario. A skip is never evidence that the pathname scenario passed.

use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const STEP_LIMIT: Duration = Duration::from_secs(10);
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    runtime: PathBuf,
    terminfo: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/pl-p3a-serve-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let builder = |path: &Path| {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .expect("create private fixture directory");
        };
        builder(&root);
        let fixture = Self {
            runtime: root.join("runtime"),
            terminfo: root.join("terminfo"),
            home: root.join("home"),
            root,
        };
        builder(&fixture.runtime);
        builder(&fixture.terminfo);
        builder(&fixture.home);
        fixture
    }

    fn context(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Explicit, minimal environment for every Panelink process; the split
    /// child inherits exactly the CLI's environment, so it is minimal too.
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_panelink"));
        // Every process (serve and control) inherits PANELINK_* as if run
        // inside an existing Panelink pane; serve/split must drop them.
        command
            .env_clear()
            .env("PANELINK_SESSION", "INHERITED-SESSION-CANARY")
            .env("PANELINK_GENERATION", "INHERITED-GENERATION-CANARY")
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("ZDOTDIR", &self.home)
            .env("PS1", "P3AS$ ")
            .current_dir(&self.home);
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Server {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Server {
    fn next_line(&self, limit: Duration) -> Option<String> {
        self.lines.recv_timeout(limit).ok()
    }

    fn sigterm_and_wait(&mut self) -> std::process::ExitStatus {
        // SAFETY: signals only this test's own still-unreaped child PID.
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGTERM);
        }
        let deadline = Instant::now() + STEP_LIMIT;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll server") {
                return status;
            }
            assert!(Instant::now() < deadline, "server stops after SIGTERM");
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn line_reader(stdout: ChildStdout) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::sync_channel(16);
    thread::spawn(move || {
        let mut stdout = stdout.take(64 * 1024);
        let mut buffer = Vec::new();
        let mut byte = [0_u8; 1];
        while let Ok(1) = stdout.read(&mut byte) {
            if byte[0] == b'\n' {
                let line = String::from_utf8_lossy(&buffer).into_owned();
                buffer.clear();
                if sender.send(line).is_err() {
                    return;
                }
            } else {
                buffer.push(byte[0]);
            }
        }
    });
    receiver
}

fn control(fixture: &Fixture, context: &str, args: &[&str]) -> (i32, String) {
    let mut command = fixture.command();
    command
        .arg("control")
        .arg("--directory")
        .arg(&fixture.runtime)
        .arg("--context")
        .arg(fixture.context(context))
        .args(args)
        .stdin(Stdio::null());
    let output = command.output().expect("run control CLI");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

fn field<'a>(json: &'a str, name: &str) -> Option<&'a str> {
    let pattern = format!("\"{name}\":");
    let start = json.find(&pattern)? + pattern.len();
    let rest = &json[start..];
    let end = rest.find([',', '}', ']']).unwrap_or(rest.len());
    Some(rest[..end].trim_matches('"'))
}

fn text_contains_after(
    fixture: &Fixture,
    context: &str,
    target: &str,
    marker: &str,
    forbidden: &str,
) -> String {
    let deadline = Instant::now() + STEP_LIMIT;
    loop {
        let (code, output) = control(fixture, context, &["read", target]);
        assert_eq!(code, 0, "read {target}: {output}");
        assert!(
            !output.contains(forbidden),
            "{forbidden} leaked into {target}: {output}"
        );
        if output.contains(marker) {
            return output;
        }
        assert!(
            Instant::now() < deadline,
            "{marker} never appeared: {output}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn claim_and_type(fixture: &Fixture, context: &str, target: &str, text: &str) {
    let (code, described) = control(fixture, context, &["describe"]);
    assert_eq!(code, 0, "describe: {described}");
    assert_eq!(field(&described, "target"), Some(target));
    let revision = field(&described, "owner_revision").unwrap().to_owned();
    let (code, claimed) = control(fixture, context, &["claim", target, &revision]);
    assert_eq!(code, 0, "claim: {claimed}");
    let (_, described) = control(fixture, context, &["describe"]);
    let revision = field(&described, "owner_revision").unwrap().to_owned();
    let (code, typed) = control(fixture, context, &["keys", target, &revision, text]);
    assert_eq!(code, 0, "keys: {typed}");
    assert_eq!(field(&typed, "outcome"), Some("applied"));
}

/// Type into a pane whose context already holds input ownership.
fn claim_and_type_again(fixture: &Fixture, context: &str, target: &str, text: &str) {
    let (_, described) = control(fixture, context, &["describe"]);
    let revision = field(&described, "owner_revision").unwrap().to_owned();
    let (code, typed) = control(fixture, context, &["keys", target, &revision, text]);
    assert_eq!(code, 0, "keys: {typed}");
}

#[test]
fn serve_binary_splits_over_the_pathname_socket_and_stops_on_sigterm() {
    let fixture = Fixture::new();
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/terminfo/panelink-probe.terminfo");
    let tic = Command::new("/usr/bin/tic")
        .args([OsStr::new("-x"), OsStr::new("-o")])
        .arg(&fixture.terminfo)
        .arg(&source)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .status()
        .expect("run tic");
    assert!(tic.success());

    let mut command = fixture.command();
    command
        .arg("serve")
        .arg("--directory")
        .arg(&fixture.runtime)
        .arg("--terminfo")
        .arg(&fixture.terminfo)
        .args([
            "--initialize",
            "--allow-input",
            "--",
            "/bin/zsh",
            "-f",
            "-i",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().expect("spawn panelink serve");
    let lines = line_reader(child.stdout.take().unwrap());
    let mut server = Server { child, lines };
    let startup = server
        .next_line(STEP_LIMIT)
        .expect("serve prints its startup line");
    if field(&startup, "error") == Some("listener_permission_denied")
        && std::env::var_os("PANELINK_ALLOW_PATHNAME_SKIP").as_deref() == Some(OsStr::new("1"))
    {
        println!(
            "SKIPPED (NOT A PASS): pathname bind denied by the environment (EPERM) and \
             PANELINK_ALLOW_PATHNAME_SKIP=1 is set: {startup}"
        );
        return;
    }
    assert_eq!(field(&startup, "action"), Some("serve"), "{startup}");
    assert_eq!(field(&startup, "state"), Some("running"), "{startup}");
    assert_eq!(field(&startup, "startup_outcome"), Some("applied"));
    let first = field(&startup, "target").unwrap().to_owned();
    assert!(fs::symlink_metadata(fixture.runtime.join("control.sock"))
        .unwrap()
        .file_type()
        .is_socket());

    // Singleton: a second serve on the same directory refuses to start.
    let mut second = fixture.command();
    let second = second
        .arg("serve")
        .arg("--directory")
        .arg(&fixture.runtime)
        .arg("--terminfo")
        .arg(&fixture.terminfo)
        .args(["--", "/bin/zsh", "-f"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&second.stdout).contains("server_storage_unavailable"));

    let (code, init) = control(&fixture, "one.ctx", &["init"]);
    assert_eq!(code, 0, "init: {init}");
    let (code, layout) = control(&fixture, "one.ctx", &["layout"]);
    assert_eq!(code, 0, "layout: {layout}");
    assert_eq!(field(&layout, "pane_count"), Some("1"));
    assert_eq!(field(&layout, "revision"), Some("1"));

    let (code, split) = control(&fixture, "one.ctx", &["split", &first, "1", "right"]);
    assert_eq!(code, 0, "split: {split}");
    assert_eq!(field(&split, "outcome"), Some("applied"));
    assert_eq!(field(&split, "layout_revision"), Some("2"));
    let second_target = field(&split, "new_target").unwrap().to_owned();
    assert_ne!(second_target, first);
    let (code, layout) = control(&fixture, "one.ctx", &["layout"]);
    assert_eq!(code, 0);
    assert_eq!(field(&layout, "pane_count"), Some("2"), "{layout}");
    assert!(layout.contains(&second_target));

    // At capacity the CLI's local preview refuses before sending anything;
    // the server-side capacity/stale rejection is covered by p3a_runtime.
    let (code, full) = control(&fixture, "one.ctx", &["split", &first, "2", "down"]);
    assert_eq!(code, 1, "{full}");
    assert_eq!(field(&full, "error"), Some("split_unavailable"));

    // Schema-4 reconcile/watch from the existing control CLI work against
    // serve (mapped onto the pinned target).
    let (code, reconciled) = control(&fixture, "one.ctx", &["reconcile", &first]);
    assert_eq!(code, 0, "reconcile: {reconciled}");
    assert_eq!(field(&reconciled, "action"), Some("reconcile"));
    assert_eq!(field(&reconciled, "target"), Some(first.as_str()));
    let cursor = field(&reconciled, "cursor").unwrap().to_owned();
    let (code, timed_out) = control(&fixture, "one.ctx", &["watch", &first, &cursor, "4", "0"]);
    assert_eq!(code, 1, "immediate watch with no change: {timed_out}");
    assert_eq!(field(&timed_out, "status"), Some("timed_out"));
    // Pane 2 is outside one.ctx's pinned scope.
    let (code, denied) = control(&fixture, "one.ctx", &["reconcile", &second_target]);
    assert_eq!(code, 1, "{denied}");

    claim_and_type(&fixture, "one.ctx", &first, "echo PANE1-$((40+2))");
    text_contains_after(&fixture, "one.ctx", &first, "PANE1-42", "PANE2-");
    let (code, changed) = control(&fixture, "one.ctx", &["watch", &first, &cursor, "8", "500"]);
    assert_eq!(code, 0, "watch after output: {changed}");
    assert_eq!(field(&changed, "status"), Some("changed"));
    assert!(changed.contains("screen_changed") || changed.contains("input_owner_changed"));

    // Both children see only server-owned PANELINK_* values, never the
    // inherited canaries.
    claim_and_type_again(
        &fixture,
        "one.ctx",
        &first,
        "echo ENV1-$PANELINK_SESSION-$((1+1))-$PANELINK_GENERATION-END",
    );
    let generation = field(&startup, "generation").unwrap().to_owned();
    text_contains_after(
        &fixture,
        "one.ctx",
        &first,
        &format!("ENV1-1-2-{generation}-END"),
        "CANARY",
    );

    let (code, init) = control(&fixture, "two.ctx", &["init", &second_target]);
    assert_eq!(code, 0, "init pane 2: {init}");
    claim_and_type(&fixture, "two.ctx", &second_target, "echo PANE2-$((40+3))");
    text_contains_after(&fixture, "two.ctx", &second_target, "PANE2-43", "PANE1-");
    claim_and_type_again(
        &fixture,
        "two.ctx",
        &second_target,
        "echo ENV2-$PANELINK_SESSION-$((2+2))",
    );
    text_contains_after(&fixture, "two.ctx", &second_target, "ENV2-2-4", "CANARY");
    let (_, first_screen) = control(&fixture, "one.ctx", &["read", &first]);
    assert!(!first_screen.contains("PANE2-"));

    let status = server.sigterm_and_wait();
    assert_eq!(status.code(), Some(0));
    let stopped = server.next_line(STEP_LIMIT).expect("stopped line");
    assert_eq!(field(&stopped, "state"), Some("stopped"), "{stopped}");
    assert!(
        !fixture.runtime.join("control.sock").exists(),
        "socket removed"
    );
    // The storage lock is released: a new serve could open the directory.
    let (code, _) = control(&fixture, "one.ctx", &["layout"]);
    assert_ne!(code, 0, "no server after stop");
}
