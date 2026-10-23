#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Real-CLI coverage for server-issued pane owner identity. Each test uses a
//! private server, state root, terminfo tree, and bounded process cleanup.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::DirBuilderExt;
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
            "panelink-owner-identity-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock after epoch")
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
            .env("PS1", "OWNER-IDENTITY$ ")
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

    fn start(&mut self) -> String {
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
            ]);
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start private workspace server");
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
        String::from_utf8(startup).expect("serve startup JSON line")
    }

    fn list(&self, caller: &str) -> String {
        success(
            self.run(["pane", "list", "--json", "--as", caller]),
            "pane list",
        )
    }

    fn claim(&self, caller: &str) -> Output {
        self.run(["pane", "claim", "1", "--json", "--as", caller])
    }

    fn release(&self, caller: &str) -> Output {
        self.run(["pane", "release", "1", "--json", "--as", caller])
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Stop closes all server-owned PTYs before isolated state is removed.
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

#[derive(Debug, PartialEq, Eq)]
struct OwnerInfo {
    kind: String,
    id: Option<String>,
    is_caller: bool,
    revision: u64,
    expires_at_server_secs: Option<u64>,
}

fn owner_info(listing: &str, target: &str) -> OwnerInfo {
    let pane = pane_object(listing, target);
    let raw = value(pane, "input_owner_info").expect("input_owner_info field");
    assert!(raw.starts_with('{'), "owner info object: {raw}");
    OwnerInfo {
        kind: string_value(raw, "kind").expect("owner kind"),
        id: optional_string(raw, "id").expect("owner id"),
        is_caller: bool_value(raw, "is_caller").expect("caller flag"),
        revision: number_value(raw, "revision").expect("owner revision"),
        expires_at_server_secs: optional_number(raw, "expires_at_server_secs")
            .expect("server owner deadline"),
    }
}

fn pane_owner_label(listing: &str, target: &str) -> String {
    string_value(pane_object(listing, target), "input_owner").expect("legacy owner label")
}

fn pane_object<'a>(listing: &'a str, target: &str) -> &'a str {
    listing
        .split("{\"index\":")
        .find(|entry| entry.contains(&format!("\"target\":\"{target}\"")))
        .unwrap_or_else(|| panic!("target {target} missing from pane listing: {listing}"))
}

fn value<'a>(json: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("\"{name}\":");
    let start = json.find(&marker)? + marker.len();
    let rest = json[start..].trim_start();
    let end = if rest.starts_with('"') {
        let mut escaped = false;
        let mut end = None;
        for (index, character) in rest.char_indices().skip(1) {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                end = Some(index + character.len_utf8());
                break;
            }
        }
        end?
    } else if rest.starts_with('{') || rest.starts_with('[') {
        let (open, close) = if rest.starts_with('{') {
            ('{', '}')
        } else {
            ('[', ']')
        };
        let mut depth = 0_u32;
        let mut in_string = false;
        let mut escaped = false;
        let mut end = None;
        for (index, character) in rest.char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    in_string = false;
                }
                continue;
            }
            match character {
                '"' => in_string = true,
                value if value == open => depth += 1,
                value if value == close => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(index + character.len_utf8());
                        break;
                    }
                }
                _ => {}
            }
        }
        end?
    } else {
        rest.find([',', '}', ']']).unwrap_or(rest.len())
    };
    Some(&rest[..end])
}

fn string_value(json: &str, name: &str) -> Option<String> {
    let raw = value(json, name)?;
    raw.strip_prefix('"')?.strip_suffix('"').map(str::to_owned)
}

fn optional_string(json: &str, name: &str) -> Option<Option<String>> {
    let raw = value(json, name)?;
    if raw == "null" {
        Some(None)
    } else {
        Some(Some(string_value(json, name)?))
    }
}

fn bool_value(json: &str, name: &str) -> Option<bool> {
    match value(json, name)? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn number_value(json: &str, name: &str) -> Option<u64> {
    value(json, name)?.parse().ok()
}

fn optional_number(json: &str, name: &str) -> Option<Option<u64>> {
    let raw = value(json, name)?;
    if raw == "null" {
        Some(None)
    } else {
        Some(Some(raw.parse().ok()?))
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

#[test]
fn owner_identity_is_server_issued_generation_scoped_and_cas_authorized() {
    let mut fixture = Fixture::new();
    let startup = fixture.start();
    assert_eq!(string_value(&startup, "action").as_deref(), Some("serve"));
    assert_eq!(string_value(&startup, "state").as_deref(), Some("running"));
    let target = string_value(&startup, "target").expect("server-issued full target");
    let generation = target
        .split(':')
        .next()
        .expect("target generation")
        .parse::<u64>()
        .expect("numeric target generation");

    const CALLER_A: &str = "owner-a-self-label";
    const CALLER_B: &str = "owner-b-self-label";
    let initial_a = fixture.list(CALLER_A);
    let initial_b = fixture.list(CALLER_B);
    let none_a = owner_info(&initial_a, &target);
    let none_b = owner_info(&initial_b, &target);
    assert_eq!(none_a.kind, "none");
    assert_eq!(none_a.id, None);
    assert!(!none_a.is_caller);
    assert_eq!(none_a.expires_at_server_secs, None);
    assert_eq!(pane_owner_label(&initial_a, &target), "none");
    assert_eq!(none_a, none_b, "unowned pane observations agree");

    let claim_a = fixture.claim(CALLER_A);
    let claim_a_stdout = String::from_utf8_lossy(&claim_a.stdout);
    assert!(
        claim_a.status.success(),
        "caller A claim: stdout={claim_a_stdout:?} stderr={:?}",
        String::from_utf8_lossy(&claim_a.stderr)
    );

    // Separate CLI processes reopen caller A's private context; the issued
    // owner identity and server deadline remain stable across reconnects.
    let owned_a = fixture.list(CALLER_A);
    let owned_a_again = fixture.list(CALLER_A);
    let owned_by_a = owner_info(&owned_a, &target);
    assert_eq!(owned_by_a.kind, "client");
    assert!(owned_by_a.is_caller);
    assert_eq!(pane_owner_label(&owned_a, &target), "you");
    assert_eq!(owner_info(&owned_a_again, &target), owned_by_a);
    let owner_id = owned_by_a.id.as_deref().expect("server owner identity");
    assert!(owned_by_a.expires_at_server_secs.is_some());
    assert!(owned_by_a.expires_at_server_secs.unwrap() > 0);
    let (id_generation, id_lease) = owner_id.split_once(':').expect("generation:lease ID");
    assert_eq!(id_generation.parse::<u64>().unwrap(), generation);
    let lease = id_lease.parse::<u64>().expect("numeric owner lease");
    assert_ne!(lease, 0);
    assert_ne!(format!("{}:{lease}", generation + 1), owner_id);

    let owned_b = fixture.list(CALLER_B);
    let observed_by_b = owner_info(&owned_b, &target);
    assert_eq!(observed_by_b.kind, "client");
    assert!(!observed_by_b.is_caller);
    assert_eq!(observed_by_b.id.as_deref(), Some(owner_id));
    assert_eq!(observed_by_b.revision, owned_by_a.revision);
    assert_eq!(
        observed_by_b.expires_at_server_secs,
        owned_by_a.expires_at_server_secs
    );
    assert_eq!(
        pane_owner_label(&owned_b, &target),
        format!("client:{lease}")
    );
    assert!(!owned_b.contains(CALLER_A));
    assert!(!owned_b.contains(CALLER_B));
    assert!(!owned_b.contains("agent:"));

    let other_claim = fixture.claim(CALLER_B);
    let other_claim_stdout = String::from_utf8_lossy(&other_claim.stdout);
    assert!(
        !other_claim.status.success(),
        "B claim while A owns the pane unexpectedly succeeded: {other_claim_stdout:?}"
    );
    assert_eq!(
        owner_info(&fixture.list(CALLER_A), &target),
        owned_by_a,
        "rejected B claim must preserve A's exact owner state"
    );
    let other_release = fixture.release(CALLER_B);
    let other_release_stdout = String::from_utf8_lossy(&other_release.stdout);
    assert!(
        !other_release.status.success(),
        "B release of A's lease unexpectedly succeeded: {other_release_stdout:?}"
    );
    assert_eq!(
        owner_info(&fixture.list(CALLER_A), &target),
        owned_by_a,
        "rejected B release must preserve A's exact owner state"
    );

    let denied_send = fixture.run([
        "pane",
        "send",
        "1",
        "printf 'OWNER_IDENTITY_MUST_NOT_SEND'",
        "--enter",
        "--json",
        "--as",
        CALLER_B,
    ]);
    assert!(
        !denied_send.status.success(),
        "B send unexpectedly succeeded"
    );
    assert_eq!(
        string_value(&String::from_utf8_lossy(&denied_send.stdout), "error").as_deref(),
        Some("input_not_claimed")
    );
    let denied_claim = fixture.run([
        "pane",
        "send",
        "1",
        "printf 'OWNER_IDENTITY_MUST_NOT_SEND'",
        "--enter",
        "--claim",
        "--json",
        "--as",
        CALLER_B,
    ]);
    assert!(
        !denied_claim.status.success(),
        "B implicit claim/send unexpectedly succeeded"
    );

    let after_denial = fixture.list(CALLER_A);
    assert_eq!(owner_info(&after_denial, &target), owned_by_a);
    let screen = fixture.run(["pane", "read", "1", "--json", "--as", CALLER_A]);
    let screen = success(screen, "pane read after B denial");
    assert!(
        !screen.contains("OWNER_IDENTITY_MUST_NOT_SEND"),
        "denied input reached PTY: {screen}"
    );

    let release = fixture.release(CALLER_A);
    let release_stdout = String::from_utf8_lossy(&release.stdout);
    assert!(
        release.status.success(),
        "A release: stdout={release_stdout:?} stderr={:?}",
        String::from_utf8_lossy(&release.stderr)
    );
    let released_a = owner_info(&fixture.list(CALLER_A), &target);
    let released_b = owner_info(&fixture.list(CALLER_B), &target);
    assert_eq!(released_a.kind, "none");
    assert_eq!(released_a.id, None);
    assert_eq!(released_a.expires_at_server_secs, None);
    assert!(!released_a.is_caller);
    assert!(released_a.revision > owned_by_a.revision);
    assert_eq!(released_a, released_b);
    assert_eq!(pane_owner_label(&fixture.list(CALLER_B), &target), "none");

    let claim_b = fixture.claim(CALLER_B);
    let claim_b_stdout = String::from_utf8_lossy(&claim_b.stdout);
    assert!(
        claim_b.status.success(),
        "B claim after release: stdout={claim_b_stdout:?} stderr={:?}",
        String::from_utf8_lossy(&claim_b.stderr)
    );
    let owned_by_b = owner_info(&fixture.list(CALLER_B), &target);
    assert_eq!(owned_by_b.kind, "client");
    assert!(owned_by_b.is_caller);
    assert!(owned_by_b.revision > released_a.revision);
    assert_ne!(owned_by_b.id.as_deref(), Some(owner_id));
    assert_eq!(pane_owner_label(&fixture.list(CALLER_B), &target), "you");
    let observed_b_by_a = owner_info(&fixture.list(CALLER_A), &target);
    assert_eq!(observed_b_by_a.kind, "client");
    assert!(!observed_b_by_a.is_caller);
    assert_eq!(observed_b_by_a.id, owned_by_b.id);
    assert_eq!(observed_b_by_a.revision, owned_by_b.revision);

    let text = success(
        fixture.run(["pane", "list", "--as", CALLER_A]),
        "text pane list",
    );
    let new_id = owned_by_b.id.as_deref().unwrap();
    assert!(
        text.contains(&format!("input owner client:{new_id}")),
        "{text}"
    );
    assert!(
        text.contains(&format!("revision {}", owned_by_b.revision)),
        "{text}"
    );
    let deadline = owned_by_b
        .expires_at_server_secs
        .expect("new owner deadline");
    assert!(
        text.contains(&format!("expires at server {deadline}")),
        "text listing omitted server deadline: {text}"
    );
    assert!(!text.contains(CALLER_A));
    assert!(!text.contains(CALLER_B));
    assert!(!text.contains("agent:"));
}
