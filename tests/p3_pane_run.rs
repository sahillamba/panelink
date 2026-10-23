#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Real CLI coverage for managed pane runs. Each test uses a private server,
//! state root, contexts, PTYs, and bounded process cleanup.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static NEXT: AtomicU64 = AtomicU64::new(1);
const PROCESS_DEADLINE: Duration = Duration::from_secs(10);

struct Fixture {
    root: PathBuf,
    state: PathBuf,
    home: PathBuf,
    terminfo: PathBuf,
    server: Option<Child>,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from("/private/tmp").join(format!(
            "panelink-pane-run-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        private_dir(&root);
        let fixture = Self {
            state: root.join("state"),
            home: root.join("home"),
            terminfo: root.join("terminfo"),
            root,
            server: None,
        };
        for path in [&fixture.state, &fixture.home, &fixture.terminfo] {
            private_dir(path);
        }
        private_dir(&fixture.state.join("server"));
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
            .env("PS1", "PANE-RUN$ ")
            .env("PANELINK_HOME", &self.state)
            .current_dir(&self.home)
            .stdin(Stdio::null());
        command
    }

    fn run<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command();
        command.args(args);
        bounded_output(command, PROCESS_DEADLINE)
    }

    fn start(&mut self, allow_input: bool, server_path: &str) -> String {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/terminfo/panelink-probe.terminfo");
        let status = bounded_status(
            {
                let mut command = Command::new("/usr/bin/tic");
                command
                    .args(["-x", "-o"])
                    .arg(&self.terminfo)
                    .arg(source)
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin");
                command
            },
            PROCESS_DEADLINE,
        );
        assert!(status.success(), "compile private terminfo: {status}");

        let mut command = self.command();
        command
            .env("PATH", server_path)
            .args(["serve", "--directory"])
            .arg(self.state.join("server"))
            .arg("--terminfo")
            .arg(&self.terminfo)
            .arg("--initialize");
        if allow_input {
            command.arg("--allow-input");
        }
        let mut child = command
            .args(["--", "/bin/zsh", "-f", "-i"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start default workspace server");
        let stdout = child.stdout.take().expect("server stdout");
        let (tx, rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut stdout = stdout;
            let mut line = Vec::new();
            let result = loop {
                let mut byte = [0_u8; 1];
                match stdout.read(&mut byte) {
                    Ok(1) if byte[0] == b'\n' => break Ok(line),
                    Ok(1) => line.push(byte[0]),
                    Ok(0) => break Err("server stdout closed before startup line"),
                    Ok(_) => break Err("unexpected server startup read size"),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break Err("could not read server startup line"),
                }
            };
            let _ = tx.send(result);
        });
        let startup = match rx.recv_timeout(PROCESS_DEADLINE) {
            Ok(Ok(line)) => line,
            Ok(Err(message)) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{message}");
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("serve did not produce a startup line within {PROCESS_DEADLINE:?}");
            }
        };
        self.server = Some(child);
        String::from_utf8(startup).expect("serve JSON line")
    }

    fn list(&self) -> String {
        success(self.run(["pane", "list", "--json"]), "pane list")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // `stop` shuts down every server-owned PTY before the isolated state
        // directory is removed. Keep a forced server fallback for failed tests.
        let mut stop = self.command();
        stop.arg("stop");
        let _ = best_effort_output(stop, Duration::from_secs(5));
        if let Some(server) = self.server.as_mut() {
            let deadline = Instant::now() + Duration::from_secs(3);
            while server.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            if server.try_wait().ok().flatten().is_none() {
                let _ = server.kill();
                let _ = server.wait();
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn private_dir(path: &Path) {
    fs::DirBuilder::new().mode(0o700).create(path).unwrap();
}

fn bounded_output(mut command: Command, timeout: Duration) -> Output {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn CLI process");
    wait_child(&mut child, timeout);
    child.wait_with_output().expect("collect CLI output")
}

fn bounded_status(mut command: Command, timeout: Duration) -> std::process::ExitStatus {
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = command.spawn().expect("spawn helper process");
    wait_child(&mut child, timeout);
    child.wait().expect("wait for helper process")
}

fn best_effort_output(mut command: Command, timeout: Duration) -> Option<Output> {
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = command.spawn().ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().ok()?.is_some() {
            return child.wait_with_output().ok();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_child(child: &mut Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().expect("poll child process").is_some() {
            return;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child process exceeded {timeout:?} deadline");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn success(output: Output, what: &str) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{what}: status={} stdout={stdout:?} stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

fn field<'a>(json: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("\"{name}\":");
    let start = json.find(&marker)? + marker.len();
    let rest = &json[start..];
    let end = rest.find([',', '}', ']']).unwrap_or(rest.len());
    Some(rest[..end].trim_matches('"'))
}

fn target_of_run(output: &Output) -> String {
    let json = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "pane run: status={} stdout={json:?} stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(field(&json, "action"), Some("pane_run"), "{json}");
    assert_eq!(field(&json, "outcome"), Some("applied"), "{json}");
    let target = field(&json, "target").expect("returned target");
    assert_ne!(target, "null", "{json}");
    target.to_owned()
}

fn pane_object<'a>(listing: &'a str, target: &str) -> &'a str {
    listing
        .split("{\"index\":")
        .find(|entry| entry.contains(&format!("\"target\":\"{target}\"")))
        .unwrap_or_else(|| panic!("target {target} missing from pane list: {listing}"))
}

fn geometry(listing: &str, target: &str) -> (u16, u16, u16, u16) {
    let object = pane_object(listing, target);
    let size = object.split_once("\"size\":{").expect("pane size object").1;
    let number = |name: &str| -> u16 {
        let marker = format!("\"{name}\":");
        let rest = &size[size.find(&marker).expect("geometry field") + marker.len()..];
        rest.split([',', '}'])
            .next()
            .unwrap()
            .parse()
            .expect("geometry number")
    };
    (number("x"), number("y"), number("columns"), number("rows"))
}

fn create_executable(path: &Path, source: &str) {
    fs::write(path, source).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn state_contains(root: &Path, needle: &[u8]) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            state_contains(&path, needle)
        } else {
            fs::read(path)
                .map(|bytes| bytes.windows(needle.len()).any(|window| window == needle))
                .unwrap_or(false)
        }
    })
}

#[test]
fn pane_run_uses_invocation_context_literal_argv_and_returns_quick_exit_identity() {
    let mut fixture = Fixture::new();
    let server_path = "/usr/bin:/bin";
    let startup = fixture.start(true, server_path);
    let original = field(&startup, "target").unwrap().to_owned();

    let invocation_path = fixture.root.join("client-bin");
    private_dir(&invocation_path);
    let capture = fixture.root.join("captured-child.bin");
    let executable = invocation_path.join("from-client-path");
    create_executable(
        &executable,
        "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$P3_RUN_ENV_SENTINEL\" > \"$P3_RUN_CAPTURE\"\nprintf '%s\\n' \"$@\" >> \"$P3_RUN_CAPTURE\"\n",
    );
    let cwd = fixture.root.join("explicit-cwd");
    private_dir(&cwd);
    let sentinel = b"PANE_RUN_PRIVATE_ARG_SENTINEL";
    let mut non_utf8 = b"byte-".to_vec();
    non_utf8.push(0xff);
    let expected = [
        cwd.as_os_str().as_bytes().to_vec(),
        b"INVOCATION_ENV_SENTINEL".to_vec(),
        b"--as".to_vec(),
        b"--json".to_vec(),
        b"--cwd".to_vec(),
        b"literal -- value".to_vec(),
        sentinel.to_vec(),
        non_utf8.clone(),
    ];
    let args: Vec<OsString> = vec![
        "pane".into(),
        "run".into(),
        "--as".into(),
        "fixture-agent".into(),
        "--json".into(),
        "--cwd".into(),
        OsString::from("../explicit-cwd"),
        "--".into(),
        "from-client-path".into(),
        "--as".into(),
        "--json".into(),
        "--cwd".into(),
        "literal -- value".into(),
        OsString::from_vec(sentinel.to_vec()),
        OsString::from_vec(non_utf8),
    ];
    let mut command = fixture.command();
    command
        .env("PATH", &invocation_path)
        .env("P3_RUN_ENV_SENTINEL", "INVOCATION_ENV_SENTINEL")
        .env("P3_RUN_CAPTURE", &capture)
        .args(args);
    let output = bounded_output(command, PROCESS_DEADLINE);
    let target = target_of_run(&output);

    let capture_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(captured) = fs::read(&capture) {
            let actual: Vec<Vec<u8>> = captured
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .map(<[u8]>::to_vec)
                .collect();
            if actual == expected {
                break;
            }
        }
        assert!(
            Instant::now() < capture_deadline,
            "quick child did not finish writing its capture"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        target.split(':').count(),
        5,
        "target is not a full identity: {target}"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let listing = fixture.list();
        if field(&listing, "pane_count") == Some("1") && !listing.contains(&target) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "quick-exit child did not settle: {listing}"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        fixture.list().contains(&original),
        "original sibling disappeared"
    );
    assert!(
        !fixture.list().contains(&target),
        "quick-exit target still appears live"
    );
    assert!(!state_contains(&fixture.state, sentinel));
    assert!(!state_contains(&fixture.state, b"INVOCATION_ENV_SENTINEL"));
}

#[test]
fn pane_run_preserves_sibling_and_splits_in_each_requested_direction() {
    for direction in ["right", "left", "down", "up"] {
        let mut fixture = Fixture::new();
        let startup = fixture.start(true, "/usr/bin:/bin");
        let anchor = field(&startup, "target").unwrap().to_owned();
        let output = fixture.run([
            "pane",
            "run",
            "--pane",
            "1",
            "--direction",
            direction,
            "--json",
            "--",
            "/bin/sleep",
            "30",
        ]);
        let target = target_of_run(&output);
        let listing = fixture.list();
        assert!(
            pane_object(&listing, &anchor).contains("\"alive\":true"),
            "{listing}"
        );
        let after = geometry(&listing, &anchor);
        let created = geometry(&listing, &target);
        assert!(
            listing.contains(&anchor),
            "anchor sibling disappeared: {listing}"
        );
        assert_eq!(field(&listing, "pane_count"), Some("2"), "{listing}");
        match direction {
            "right" => assert!(
                created.0 >= after.0 + after.2,
                "{direction}: {after:?} {created:?}"
            ),
            "left" => assert!(
                created.0 + created.2 <= after.0,
                "{direction}: {after:?} {created:?}"
            ),
            "down" => assert!(
                created.1 >= after.1 + after.3,
                "{direction}: {after:?} {created:?}"
            ),
            "up" => assert!(
                created.1 + created.3 <= after.1,
                "{direction}: {after:?} {created:?}"
            ),
            _ => unreachable!(),
        }
    }
}

#[test]
fn pane_run_refuses_readonly_capacity_and_invalid_pre_spawn_launches() {
    let mut readonly = Fixture::new();
    readonly.start(false, "/usr/bin:/bin");
    let denied = readonly.run(["pane", "run", "--json", "--", "/bin/sleep", "30"]);
    assert!(
        !denied.status.success(),
        "readonly run unexpectedly succeeded"
    );
    assert_eq!(field(&readonly.list(), "pane_count"), Some("1"));

    let mut fixture = Fixture::new();
    fixture.start(true, "/usr/bin:/bin");
    for invalid in [
        vec![
            "pane",
            "run",
            "--json",
            "--",
            "definitely-no-such-pane-run-executable",
        ],
        vec![
            "pane",
            "run",
            "--cwd",
            "/definitely/no/such/pane-run-dir",
            "--json",
            "--",
            "/bin/sleep",
            "30",
        ],
    ] {
        let failed = fixture.run(invalid);
        assert!(
            !failed.status.success(),
            "invalid launch unexpectedly succeeded"
        );
        let response = String::from_utf8_lossy(&failed.stdout);
        assert_eq!(field(&response, "outcome"), Some("failed"), "{response}");
        assert_eq!(field(&fixture.list(), "pane_count"), Some("1"));
    }

    for (pane, direction) in [("1", "right"), ("1", "down"), ("2", "down")] {
        let output = fixture.run([
            "pane",
            "run",
            "--pane",
            pane,
            "--direction",
            direction,
            "--json",
            "--",
            "/bin/sleep",
            "30",
        ]);
        target_of_run(&output);
    }
    assert_eq!(field(&fixture.list(), "pane_count"), Some("4"));
    let capacity = fixture.run([
        "pane",
        "run",
        "--pane",
        "1",
        "--json",
        "--",
        "/bin/sleep",
        "30",
    ]);
    assert!(
        !capacity.status.success(),
        "fifth pane unexpectedly launched"
    );
    assert_eq!(field(&fixture.list(), "pane_count"), Some("4"));
}
