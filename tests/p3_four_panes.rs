#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Real-PTY contract for four independent panes, nonreplayed input, and
//! arbitrary-leaf close/collapse. Every process is isolated under a private
//! PANELINK_HOME and uses the real `serve` workspace runtime.

use std::fs;
use std::io::Read;
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
            "panelink-four-panes-{}-{}-{}",
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
            .env("PS1", "FOUR$ ")
            .env("PANELINK_HOME", &self.state)
            .current_dir(&self.home)
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().expect("run panelink")
    }

    fn start(&mut self) -> String {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/terminfo/panelink-probe.terminfo");
        let status = Command::new("/usr/bin/tic")
            .args(["-x", "-o"])
            .arg(&self.terminfo)
            .arg(source)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .status()
            .expect("compile private terminfo");
        assert!(status.success());
        let mut child = self
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
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start default workspace server");
        let mut stdout = child.stdout.take().unwrap();
        let mut startup = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut byte = [0_u8; 1];
            match stdout.read(&mut byte) {
                Ok(1) if byte[0] == b'\n' => break,
                Ok(1) => startup.push(byte[0]),
                _ => {
                    assert!(Instant::now() < deadline, "serve did not start");
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
        self.server = Some(child);
        String::from_utf8(startup).expect("serve JSON line")
    }

    /// Split `target` (pinned in its own private control context) at the
    /// current layout revision and return the new pane's full target.
    fn split(&self, target: &str, direction: &str) -> String {
        let revision = self.layout_revision();
        let context = self.root.join(format!("split-{}.ctx", pane_id(target)));
        if !context.exists() {
            ok(&self.control(&context, &["init", target]), "control init");
        }
        let output = self.control(&context, &["split", target, &revision, direction]);
        let response = ok(&output, &format!("split {direction}"));
        assert_eq!(field(&response, "outcome"), Some("applied"), "{response}");
        field(&response, "new_target").unwrap().to_owned()
    }

    fn control(&self, context: &Path, args: &[&str]) -> Output {
        self.command()
            .arg("control")
            .arg("--directory")
            .arg(self.state.join("server"))
            .arg("--context")
            .arg(context)
            .args(args)
            .output()
            .expect("run control CLI")
    }

    fn list(&self) -> String {
        ok(&self.run(&["pane", "list", "--json"]), "pane list")
    }

    fn layout_revision(&self) -> String {
        field(&self.list(), "layout_revision").unwrap().to_owned()
    }

    /// 1-based spatial index currently occupied by `target`.
    fn index_of(&self, target: &str) -> usize {
        let listing = self.list();
        listing
            .split("{\"index\":")
            .skip(1)
            .find(|entry| entry.contains(&format!("\"target\":\"{target}\"")))
            .and_then(|entry| entry.split(',').next()?.parse().ok())
            .unwrap_or_else(|| panic!("{target} not listed: {listing}"))
    }

    fn send(&self, target: &str, text: &str) {
        let index = self.index_of(target).to_string();
        let sent = ok(
            &self.run(&["pane", "send", &index, text, "--enter", "--claim", "--json"]),
            "pane send",
        );
        assert_eq!(field(&sent, "status"), Some("applied"), "{sent}");
    }

    fn screen(&self, target: &str) -> String {
        let index = self.index_of(target).to_string();
        ok(
            &self.run(&["pane", "read", &index, "--lines", "40", "--json"]),
            "pane read",
        )
    }

    fn wait_for(&self, target: &str, needle: &str) {
        let index = self.index_of(target).to_string();
        let matched = ok(
            &self.run(&[
                "wait",
                &index,
                "--text",
                needle,
                "--timeout",
                "10",
                "--json",
            ]),
            "wait text",
        );
        assert_eq!(field(&matched, "matched"), Some("true"), "{matched}");
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

fn ok(output: &Output, what: &str) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{what}: stdout={stdout:?} stderr={:?}",
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

fn pane_id(target: &str) -> &str {
    target.split(':').nth(2).unwrap()
}

#[test]
#[ignore = "intermittent control_unavailable during rapid CLI connections; see PR #4 run 40499048646"]
fn real_ptys_split_to_four_keep_input_isolated_and_collapse_any_closed_pane() {
    let mut fixture = Fixture::new();
    let startup = fixture.start();
    let first = field(&startup, "target").unwrap().to_owned();

    let second = fixture.split(&first, "right");
    let third = fixture.split(&first, "down");
    // 80x24 default: the right half (40x24) can still split down; a 39-column
    // leaf is below the 41-column left/right minimum.
    let fourth = fixture.split(&second, "down");
    let listing = fixture.list();
    assert_eq!(field(&listing, "pane_count"), Some("4"), "{listing}");

    // A fifth split is refused at the four-pane capacity without effects.
    let revision = fixture.layout_revision();
    let context = fixture.root.join("split-cap.ctx");
    ok(&fixture.control(&context, &["init", &fourth]), "init cap");
    let refused = fixture.control(&context, &["split", &fourth, &revision, "down"]);
    let refused_text = String::from_utf8_lossy(&refused.stdout);
    assert_ne!(
        field(&refused_text, "outcome"),
        Some("applied"),
        "{refused_text}"
    );
    assert_eq!(field(&fixture.list(), "pane_count"), Some("4"));

    let targets = [&first, &second, &third, &fourth];
    for (index, target) in targets.iter().enumerate() {
        fixture.send(target, &format!("echo FOUR:MARKER-{index}"));
    }
    for (index, target) in targets.iter().enumerate() {
        fixture.wait_for(target, &format!("FOUR:MARKER-{index}"));
        let screen = fixture.screen(target);
        for other in 0..targets.len() {
            if index != other {
                assert!(
                    !screen.contains(&format!("FOUR:MARKER-{other}")),
                    "pane {index} saw marker {other}: {screen}"
                );
            }
        }
    }

    // Close an interior leaf (not the last-created one); its sibling subtree
    // collapses into the freed space and the other panes keep running.
    let third_index = fixture.index_of(&third).to_string();
    let closed = ok(
        &fixture.run(&["pane", "close", &third_index, "--json"]),
        "pane close",
    );
    assert_eq!(field(&closed, "action"), Some("pane_close"), "{closed}");
    let listing = fixture.list();
    assert_eq!(field(&listing, "pane_count"), Some("3"), "{listing}");
    assert!(!listing.contains(&third), "{listing}");
    for survivor in [&first, &second, &fourth] {
        assert!(listing.contains(survivor.as_str()), "{listing}");
    }
    fixture.send(&first, "echo FOUR:AFTER-CLOSE");
    fixture.wait_for(&first, "FOUR:AFTER-CLOSE");
    fixture.send(&fourth, "echo FOUR:FOURTH-ALIVE");
    fixture.wait_for(&fourth, "FOUR:FOURTH-ALIVE");
}
