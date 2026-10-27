#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Private-runtime integration coverage for process-bound recognized agents.

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
            "panelink-recognized-agents-{}-{}-{}",
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
            .env("PS1", "RECOGNIZED$ ")
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
        let server = self
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
        self.server = Some(server);
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

    fn submit(&self, text: &str) -> Output {
        self.run(&["pane", "submit", "1", text, "--json"])
    }

    fn launch_fake_codex(&self, mode: &str) {
        // The generated path contains only fixed ASCII components, PID, and digits.
        let executable = self.root.join("bin/codex");
        let command = format!("exec {} {}", executable.display(), mode);
        let output = self.submit(&command);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let observed = self.run(&["agent", "get", "1", "--json"]);
            let text = output_text(&observed);
            let expected = format!("\"state\":\"{}\"", mode);
            if observed.status.success() && text.contains(&expected) {
                break;
            }
            assert!(Instant::now() < deadline, "fake agent readiness: {text}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn compile_fake_codex(&self) {
        fs::create_dir(self.root.join("bin")).unwrap();
        let source = self.root.join("fake_codex.c");
        fs::write(
            &source,
            r#"#include <stdio.h>
#include <string.h>
#include <time.h>

static void pause_briefly(void) {
    struct timespec delay = { .tv_sec = 0, .tv_nsec = 900000000 };
    nanosleep(&delay, NULL);
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "idle";
    if (strcmp(mode, "blocked") == 0) {
        puts("Do you want to run this command?"); fflush(stdout);
    } else if (strcmp(mode, "unknown") == 0) {
        puts("Loading workspace state"); fflush(stdout);
    } else {
        printf("\033[2J\033[23;1H› "); fflush(stdout);
    }
    char input[4096];
    while (fgets(input, sizeof input, stdin)) {
        input[strcspn(input, "\r\n")] = 0;
        if (strcmp(input, "QUIT") == 0) return 0;
        if (strcmp(mode, "idle") == 0) {
            printf("\033[2J\033[23;1HWorking…"); fflush(stdout);
            pause_briefly();
            printf("\033[2J\033[23;1H› "); fflush(stdout);
        }
    }
    return 0;
}
"#,
        )
        .unwrap();
        let status = Command::new("/usr/bin/cc")
            .args(["-Wall", "-Wextra", "-Werror"])
            .arg(&source)
            .arg("-o")
            .arg(self.root.join("bin/codex"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .status()
            .expect("compile deterministic fake codex");
        assert!(status.success(), "compile fake codex");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // QUIT is consumed only by the private fake process launched in this fixture.
        if self
            .server
            .as_mut()
            .is_some_and(|server| server.try_wait().ok().flatten().is_none())
        {
            let _ = self.submit("QUIT");
        }
        if let Some(server) = self.server.as_mut() {
            if server.try_wait().ok().flatten().is_none() {
                let _ = server.kill();
                let _ = server.wait();
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn output_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn json_string(output: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\":\"");
    let value = output.split_once(&marker)?.1;
    let end = value.find('"')?;
    Some(value[..end].to_owned())
}

#[test]
fn recognized_agent_list_get_read_prompt_and_wait_use_process_bound_identity() {
    let mut fixture = Fixture::new();
    fixture.compile_fake_codex();
    fixture.start();
    fixture.launch_fake_codex("idle");

    let listed = fixture.run(&["agent", "list", "--json"]);
    assert!(listed.status.success(), "{}", output_text(&listed));
    let listing = output_text(&listed);
    assert!(listing.contains("\"action\":\"agent_list\""), "{listing}");
    assert!(listing.contains("\"kind\":\"codex\""), "{listing}");
    assert!(listing.contains("\"state\":\"idle\""), "{listing}");
    assert!(
        listing.contains("\"identity_source\":\"pty_foreground_process\""),
        "{listing}"
    );
    assert!(
        listing.contains("\"conversation_identity_verified\":false"),
        "{listing}"
    );
    let agent_ref = json_string(&listing, "agent_ref").expect("discovered process-bound ref");
    assert!(
        agent_ref.contains('@') && agent_ref.ends_with(":codex"),
        "{agent_ref}"
    );
    let identity: Vec<_> = agent_ref.split('@').nth(1).unwrap().split(':').collect();
    let pid: u64 = identity[0].parse().expect("foreground PID in agent ref");
    let birth_seconds: u64 = identity[1]
        .parse()
        .expect("process birth seconds in agent ref");
    let birth_micros: u64 = identity[2]
        .parse()
        .expect("process birth microseconds in agent ref");
    assert!(
        pid > 0 && birth_seconds > 0 && birth_micros < 1_000_000,
        "{agent_ref}"
    );

    // A different process birth must never be accepted even at the same target/PID.
    let (target_ref, process_ref) = agent_ref.split_once('@').unwrap();
    let mut wrong_birth: Vec<String> = process_ref.split(':').map(str::to_owned).collect();
    wrong_birth[1] = (birth_seconds + 1).to_string();
    let stale_ref = format!("{target_ref}@{}", wrong_birth.join(":"));
    let stale = fixture.run(&["agent", "prompt", &stale_ref, "MUST_NOT_BE_SENT", "--json"]);
    assert!(!stale.status.success(), "{}", output_text(&stale));
    assert!(output_text(&stale).contains("agent_changed"));

    let got = fixture.run(&["agent", "get", &agent_ref, "--json"]);
    assert!(got.status.success(), "{}", output_text(&got));
    assert!(output_text(&got).contains("\"action\":\"agent_get\""));
    assert!(output_text(&got).contains(&agent_ref));

    let read = fixture.run(&["agent", "read", &agent_ref, "--lines", "1024", "--json"]);
    assert!(read.status.success(), "{}", output_text(&read));
    let read_text = output_text(&read);
    assert!(
        read_text.contains("\"action\":\"agent_read\""),
        "{read_text}"
    );
    assert!(read_text.contains("›"), "{read_text}");

    let waited = fixture.run(&[
        "agent",
        "wait",
        &agent_ref,
        "--until",
        "idle",
        "--timeout",
        "2",
        "--json",
    ]);
    assert!(waited.status.success(), "{}", output_text(&waited));
    let wait_text = output_text(&waited);
    assert!(
        wait_text.contains("\"action\":\"agent_wait\""),
        "{wait_text}"
    );
    assert!(wait_text.contains("\"state\":\"idle\""), "{wait_text}");
    assert!(
        !wait_text.contains("completed"),
        "observation must not imply turn completion: {wait_text}"
    );

    let prompted = fixture.run(&[
        "agent",
        "prompt",
        &agent_ref,
        "deterministic",
        "probe",
        "--wait",
        "--until",
        "idle",
        "--timeout",
        "3",
        "--json",
    ]);
    assert!(prompted.status.success(), "{}", output_text(&prompted));
    let prompt_text = output_text(&prompted);
    assert!(
        prompt_text.contains("\"delivery\":\"pty_written\""),
        "{prompt_text}"
    );
    assert!(
        prompt_text.contains("\"control_release\":\"confirmed\""),
        "{prompt_text}"
    );
    assert!(
        prompt_text.contains("\"claim_operation_key\":{"),
        "{prompt_text}"
    );
    assert!(prompt_text.contains("\"operation_key\":{"), "{prompt_text}");
    assert!(
        prompt_text.contains("\"release_operation_key\":{"),
        "{prompt_text}"
    );
    assert!(
        prompt_text.contains("\"action\":\"agent_wait\""),
        "{prompt_text}"
    );
    assert!(!prompt_text.contains("\"completed\":true"), "{prompt_text}");
}

#[test]
fn blocked_and_unknown_codex_screens_refuse_prompt_without_input() {
    for (mode, expected_state, expected_error) in [
        ("blocked", "blocked", "agent_blocked"),
        ("unknown", "unknown", "agent_not_ready"),
    ] {
        let mut fixture = Fixture::new();
        fixture.compile_fake_codex();
        fixture.start();
        fixture.launch_fake_codex(mode);

        let listed = fixture.run(&["agent", "list", "--json"]);
        assert!(listed.status.success(), "{}", output_text(&listed));
        let listing = output_text(&listed);
        assert!(
            listing.contains(&format!("\"state\":\"{expected_state}\"")),
            "{listing}"
        );
        let agent_ref = json_string(&listing, "agent_ref").expect("recognized codex process ref");
        let timeout = fixture.run(&[
            "agent",
            "wait",
            &agent_ref,
            "--until",
            "idle",
            "--timeout",
            "1",
            "--json",
        ]);
        assert!(!timeout.status.success(), "{}", output_text(&timeout));
        assert!(
            output_text(&timeout).contains("observation_timeout"),
            "{}",
            output_text(&timeout)
        );

        let refused = fixture.run(&["agent", "prompt", &agent_ref, "MUST_NOT_BE_SENT", "--json"]);
        assert!(!refused.status.success(), "{}", output_text(&refused));
        assert!(
            output_text(&refused).contains(expected_error),
            "{}",
            output_text(&refused)
        );
        let after = fixture.run(&["agent", "read", &agent_ref, "--lines", "1024", "--json"]);
        assert!(after.status.success(), "{}", output_text(&after));
        assert!(output_text(&after).contains(&format!("\"state\":\"{expected_state}\"")));
        assert!(
            !output_text(&after).contains("MUST_NOT_BE_SENT"),
            "{}",
            output_text(&after)
        );
    }
}
