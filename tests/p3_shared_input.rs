#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Private-runtime coverage for scoped pane submission and retained claims.

use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static NEXT: AtomicU64 = AtomicU64::new(1);

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
            "panelink-shared-input-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let fixture = Self {
            state: root.join("state"),
            home: root.join("home"),
            terminfo: root.join("terminfo"),
            root,
            server: None,
        };
        for path in [&fixture.state, &fixture.home, &fixture.terminfo] {
            fs::DirBuilder::new().mode(0o700).create(path).unwrap();
        }
        fs::DirBuilder::new()
            .mode(0o700)
            .create(fixture.state.join("server"))
            .unwrap();
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
            .env("PS1", "SHARED$ ")
            .env("PANELINK_HOME", &self.state)
            .current_dir(&self.home)
            .stdin(Stdio::null());
        command
    }

    fn start(&mut self) {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/terminfo/panelink-probe.terminfo");
        assert!(Command::new("/usr/bin/tic")
            .args(["-x", "-o"])
            .arg(&self.terminfo)
            .arg(source)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .status()
            .expect("compile private terminfo")
            .success());
        let child = self
            .command()
            .args(["serve", "--directory"])
            .arg(self.state.join("server"))
            .arg("--terminfo")
            .arg(&self.terminfo)
            .args([
                "--initialize",
                "--allow-input",
                "--",
                "/bin/zsh",
                "-f",
                "-i",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start private fixture server");
        self.server = Some(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.state.join("server/control.sock").exists() {
            assert!(Instant::now() < deadline, "fixture server did not start");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = self.command();
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("start fixture CLI command");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if child
                .try_wait()
                .expect("poll fixture CLI command")
                .is_some()
            {
                return child
                    .wait_with_output()
                    .expect("collect fixture CLI output");
            }
            assert!(Instant::now() < deadline, "fixture CLI command timed out");
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(server) = self.server.as_mut() {
            if server.try_wait().ok().flatten().is_none() {
                let _ = server.kill();
                let _ = server.wait();
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn pane_submit_releases_after_applied_and_refuses_to_reuse_a_retained_claim() {
    let mut fixture = Fixture::new();
    fixture.start();

    let identity = fixture.run(&["server-info", "--json"]);
    assert!(
        identity.status.success(),
        "{}",
        String::from_utf8_lossy(&identity.stdout)
    );
    let identity = String::from_utf8_lossy(&identity.stdout);
    assert!(identity.contains("\"server\":{"));
    assert!(identity.contains("\"build_identity\""));
    assert!(identity.contains("\"source_fingerprint\":"));

    let unclaimed = fixture.run(&["pane", "keys", "1", "Enter", "--json"]);
    assert!(!unclaimed.status.success());
    let unclaimed = String::from_utf8_lossy(&unclaimed.stdout);
    assert!(
        unclaimed.contains("\"error\":\"input_not_claimed\""),
        "{unclaimed}"
    );
    assert!(
        unclaimed.contains("\"input_owner_info\":{\"kind\":\"none\""),
        "{unclaimed}"
    );
    assert!(
        unclaimed.contains("\"owner_observation\":\"before_operation\""),
        "{unclaimed}"
    );

    let bottom = fixture.run(&["pane", "read", "1", "--bottom", "--lines", "3", "--json"]);
    assert!(
        bottom.status.success(),
        "{}",
        String::from_utf8_lossy(&bottom.stdout)
    );
    assert!(String::from_utf8_lossy(&bottom.stdout).contains("\"row_selection\":\"bottom\""));

    let submitted = fixture.run(&[
        "pane",
        "submit",
        "1",
        "printf",
        "SHARED_SUBMIT_OK",
        "--json",
    ]);
    assert!(
        submitted.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&submitted.stdout),
        String::from_utf8_lossy(&submitted.stderr)
    );
    let result = String::from_utf8_lossy(&submitted.stdout);
    assert!(result.contains("\"delivery\":\"pty_written\""), "{result}");
    assert!(
        result.contains("\"control_release\":\"confirmed\""),
        "{result}"
    );
    assert!(result.contains("\"operation_key\":{"), "{result}");
    let operation_key = json_key(&result, "operation_key");
    let claim_key = json_key(&result, "claim_operation_key");
    let release_key = json_key(&result, "release_operation_key");
    assert!(operation_key.is_some(), "{result}");
    assert!(claim_key.is_some(), "{result}");
    assert!(release_key.is_some(), "{result}");
    assert_ne!(operation_key, claim_key, "{result}");
    assert_ne!(operation_key, release_key, "{result}");
    assert_ne!(claim_key, release_key, "{result}");

    let claimed = fixture.run(&["pane", "claim", "1", "--json"]);
    assert!(
        claimed.status.success(),
        "{}",
        String::from_utf8_lossy(&claimed.stdout)
    );
    let refused = fixture.run(&["pane", "submit", "1", "printf", "MUST_NOT_RUN", "--json"]);
    assert!(!refused.status.success());
    let refusal = String::from_utf8_lossy(&refused.stdout);
    assert!(refusal.contains("input_already_claimed"), "{refusal}");
    assert!(refusal.contains("release"), "{refusal}");
    let released = fixture.run(&["pane", "release", "1", "--json"]);
    assert!(
        released.status.success(),
        "{}",
        String::from_utf8_lossy(&released.stdout)
    );
}

fn json_key(output: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\":");
    let value = output.split_once(&marker)?.1;
    let value = value.strip_prefix('{')?;
    let end = value.find('}')?;
    Some(value[..=end].to_owned())
}

// Each attachment owns only its outer terminal. Neither may reserve idle pane
// input; independent scoped input must remain possible while both are alive.
#[test]
fn two_idle_attachments_allow_independent_scoped_input() {
    use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
    use rustix::process::{ioctl_tiocsctty, setsid};
    use rustix::termios::Winsize;
    use std::io::{Read, Write};
    use std::os::fd::AsFd;
    use std::os::unix::process::CommandExt;

    struct Attached {
        child: Child,
        master: fs::File,
        parser: vt100::Parser,
    }
    impl Attached {
        fn drain(&mut self) {
            let mut bytes = [0; 16384];
            loop {
                match self.master.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(n) => self.parser.process(&bytes[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => panic!("outer terminal read: {e}"),
                }
            }
        }
    }
    impl Drop for Attached {
        fn drop(&mut self) {
            let _ = self.master.write_all(&[0x18]);
            let deadline = Instant::now() + Duration::from_secs(3);
            while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                self.drain();
                thread::sleep(Duration::from_millis(5));
            }
            if self.child.try_wait().ok().flatten().is_none() {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
        }
    }
    let mut fixture = Fixture::new();
    fixture.start();
    let mut clients = Vec::new();
    for number in 0..2 {
        let size = Winsize {
            ws_row: 24,
            ws_col: 100,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pty = rustix_openpty::openpty(None, Some(&size)).unwrap();
        let master = fs::File::from(pty.controller);
        fcntl_setfl(&master, fcntl_getfl(&master).unwrap() | OFlags::NONBLOCK).unwrap();
        let controlling = pty.user.as_fd().try_clone_to_owned().unwrap();
        let stdio = || {
            Stdio::from(fs::File::from(
                pty.user.as_fd().try_clone_to_owned().unwrap(),
            ))
        };
        let mut command = fixture.command();
        command
            .args(["attach", "--directory"])
            .arg(fixture.state.join("server"))
            .arg("--context-dir")
            .arg(fixture.root.join(format!("ui-{number}")))
            .env("TERM", "xterm-256color")
            .stdin(stdio())
            .stdout(stdio())
            .stderr(stdio());
        // SAFETY: only fixture-owned terminal descriptors and a new session.
        unsafe {
            command.pre_exec(move || {
                setsid().map_err(std::io::Error::from)?;
                ioctl_tiocsctty(&controlling).map_err(std::io::Error::from)?;
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        drop(command);
        clients.push(Attached {
            child,
            master,
            parser: vt100::Parser::new(24, 100, 0),
        });
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        for client in &mut clients {
            client.drain();
            assert!(client.child.try_wait().unwrap().is_none());
        }
        if clients
            .iter()
            .all(|c| c.parser.screen().contents().contains("SHARED$"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "both attachments must show shell readiness"
        );
        thread::sleep(Duration::from_millis(5));
    }
    // A human key batch must release too, allowing the next independent writer.
    clients[0]
        .master
        .write_all(b"printf 'HUMAN_%s\\n' BATCH_OK\r")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        for client in &mut clients {
            client.drain();
        }
        if clients
            .iter()
            .all(|c| c.parser.screen().contents().contains("HUMAN_BATCH_OK"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "human input must reach both attachments"
        );
        thread::sleep(Duration::from_millis(5));
    }
    let submitted = fixture.run(&[
        "pane",
        "submit",
        "1",
        "printf 'ATTACHED_%s\\n' SHARED_OK",
        "--json",
    ]);
    assert!(
        submitted.status.success(),
        "{}",
        String::from_utf8_lossy(&submitted.stdout)
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        for client in &mut clients {
            client.drain();
        }
        if clients
            .iter()
            .all(|c| c.parser.screen().contents().contains("ATTACHED_SHARED_OK"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "independent marker must reach both live attachments"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
