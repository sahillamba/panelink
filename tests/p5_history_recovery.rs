#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Bounded restart-read acceptance for the saved, read-only history path.
//! The child runs on a private PTY and the real history CLI opens the resulting
//! private store in fresh processes; no application server or user workspace
//! is started.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Output, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use panelink::close_protocol::StopOutcome;
use panelink::control_protocol::{Command, TargetRef};
use panelink::history_rows;
use panelink::history_store::HistoryStore;
use panelink::launch::{Environment, ResolvedLaunch};
use panelink::protocol::Outcome;
use panelink::screen_checkpoint::{self, ScreenCheckpoint};
use panelink::server::EffectSink;
use panelink::server_pty::PtyEffects;
use panelink::terminal::{ScreenSnapshot, TerminalSession, TerminalSize};

const DEADLINE: Duration = Duration::from_secs(6);
const OSC52_CANARY: &str = "U0VDUkVULVNDUkVFTi1DQU5BUlk=";
const ALTERNATE_A: &str = "ALT-A-UNRECORDED";
const ALTERNATE_B: &str = "ALT-B-LATEST";
const NORMAL_ROW: &str = "NORMAL-ROW-03";
const FINAL_NORMAL: &str = "FINAL-NORMAL-LATEST";

static PRIVATE_DIR_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct PrivateFixture {
    root: PathBuf,
    state: PathBuf,
    home: PathBuf,
    server: PathBuf,
    history: PathBuf,
    shell_probe: PathBuf,
    child_marker: PathBuf,
}

impl PrivateFixture {
    fn new() -> std::io::Result<Self> {
        for _ in 0..100 {
            let sequence = PRIVATE_DIR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = Path::new("/private/tmp").join(format!(
                "panelink-p5-history-recovery-{}-{sequence}",
                std::process::id()
            ));
            match create_private_dir(&root) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }

            let fixture = Self {
                state: root.join("state"),
                home: root.join("home"),
                server: root.join("state/server"),
                history: root.join("state/server/history"),
                shell_probe: root.join("forbidden-shell"),
                child_marker: root.join("forbidden-child-started"),
                root,
            };
            create_private_dir(&fixture.state)?;
            create_private_dir(&fixture.home)?;
            create_private_dir(&fixture.server)?;
            create_private_dir(&fixture.history)?;

            // If a history query accidentally takes the normal workspace-open
            // route, the selected shell records execution and exits at once.
            fs::write(
                &fixture.shell_probe,
                format!(
                    "#!/bin/sh\nprintf 'started\\n' > '{}'\nexit 0\n",
                    fixture.child_marker.display()
                ),
            )?;
            fs::set_permissions(&fixture.shell_probe, fs::Permissions::from_mode(0o700))?;
            return Ok(fixture);
        }
        Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
    }

    fn command(&self) -> ProcessCommand {
        let mut command = ProcessCommand::new(env!("CARGO_BIN_EXE_panelink"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("ZDOTDIR", &self.home)
            .env("SHELL", &self.shell_probe)
            .env("PANELINK_HOME", &self.state)
            .current_dir(&self.home)
            .stdin(Stdio::null());
        command
    }

    fn panelink(&self, args: &[&str]) -> Output {
        let mut command = self.command();
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = command.spawn().expect("spawn fresh panelink CLI process");
        let process_group = i32::try_from(child.id()).expect("CLI PID fits pid_t");
        let deadline = Instant::now() + DEADLINE;
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll fresh CLI process") {
                break status;
            }
            if Instant::now() >= deadline {
                // Kill the isolated process group before collecting its small
                // pipe output, so a mistakenly started child cannot keep the
                // output descriptors open or escape fixture teardown.
                // SAFETY: this negative PID names only this test-owned group.
                unsafe {
                    libc::kill(-process_group, libc::SIGKILL);
                }
                let _ = child.kill();
                let output = child
                    .wait_with_output()
                    .expect("reap timed-out history CLI process");
                panic!(
                    "panelink {:?} exceeded its {DEADLINE:?} deadline (status {:?})",
                    args, output.status
                );
            }
            thread::sleep(Duration::from_millis(5));
        };

        // A read-only history command must not leave a server or child running
        // after it returns. Its private process group lets the fixture clean
        // up any unexpected descendant before waiting for pipe EOF.
        // SAFETY: this negative PID names only this test-owned group.
        unsafe {
            libc::kill(-process_group, libc::SIGKILL);
        }
        let output = child
            .wait_with_output()
            .expect("collect bounded panelink CLI output");
        assert_eq!(
            output.status, status,
            "CLI exit status changed while collecting output"
        );
        output
    }
}

impl Drop for PrivateFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn create_private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(path)
}

fn explicit_environment() -> Environment {
    [
        (OsString::from("TERM"), OsString::from("panelink-probe")),
        (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
    ]
    .into_iter()
    .collect()
}

fn launch_shell(script: &str, cwd: &Path, size: TerminalSize) -> TerminalSession {
    let launch = ResolvedLaunch {
        requested_path: PathBuf::from("/bin/sh"),
        executable_path: PathBuf::from("/bin/sh"),
        working_directory: cwd.to_path_buf(),
    };
    TerminalSession::spawn(
        &launch,
        &[OsString::from("-c"), OsString::from(script)],
        &explicit_environment(),
        size,
    )
    .expect("spawn explicit shell in private PTY")
}

fn screen_text(snapshot: &ScreenSnapshot) -> String {
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

fn pump_terminal_until(
    session: &mut TerminalSession,
    description: &str,
    predicate: impl Fn(&ScreenSnapshot) -> bool,
) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        session.pump().expect("bounded private-PTY pump");
        if predicate(session.snapshot()) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn poll_effects_until(
    effects: &mut PtyEffects,
    description: &str,
    predicate: impl Fn(&ScreenSnapshot) -> bool,
) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        EffectSink::poll(effects).expect("bounded server-side PTY poll");
        if predicate(effects.screen()) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {description}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn target() -> TargetRef {
    TargetRef {
        generation: 71,
        workspace: 3,
        pane: 5,
        session: 11,
        occupant_revision: 2,
    }
}

fn target_argument(target: TargetRef) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        target.generation, target.workspace, target.pane, target.session, target.occupant_revision
    )
}

fn checkpoint_text(checkpoint: &ScreenCheckpoint<'_>) -> String {
    let mut text = String::new();
    for row in 0..checkpoint.rows() {
        for column in 0..checkpoint.columns() {
            if let Some(cell) = checkpoint.cell(row, column) {
                text.push_str(cell.text());
            }
        }
        text.push('\n');
    }
    text
}

#[derive(Debug, Eq, PartialEq)]
struct TreeEntry {
    directory: bool,
    length: u64,
    modified: SystemTime,
    contents: Vec<u8>,
}

fn storage_snapshot(root: &Path) -> std::io::Result<BTreeMap<PathBuf, TreeEntry>> {
    fn visit(
        root: &Path,
        path: &Path,
        entries: &mut BTreeMap<PathBuf, TreeEntry>,
    ) -> std::io::Result<()> {
        for item in fs::read_dir(path)? {
            let item = item?;
            let child = item.path();
            let metadata = fs::symlink_metadata(&child)?;
            assert!(
                metadata.is_dir() || metadata.is_file(),
                "unexpected non-file in fixture"
            );
            let relative = child
                .strip_prefix(root)
                .expect("fixture entry has its private root prefix")
                .to_path_buf();
            let contents = if metadata.is_file() {
                fs::read(&child)?
            } else {
                Vec::new()
            };
            entries.insert(
                relative,
                TreeEntry {
                    directory: metadata.is_dir(),
                    length: metadata.len(),
                    modified: metadata.modified().unwrap_or(UNIX_EPOCH),
                    contents,
                },
            );
            if metadata.is_dir() {
                visit(root, &child, entries)?;
            }
        }
        Ok(())
    }

    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries)?;
    Ok(entries)
}

fn assert_cli_output_is_sanitized(output: &Output) {
    for stream in [&output.stdout, &output.stderr] {
        assert!(!stream.contains(&0x1b), "CLI output contains ESC");
        assert!(
            !stream.windows(b"]52;".len()).any(|bytes| bytes == b"]52;"),
            "CLI output contains an OSC 52 introducer"
        );
        assert!(
            !stream
                .windows(OSC52_CANARY.len())
                .any(|bytes| bytes == OSC52_CANARY.as_bytes()),
            "CLI output contains the OSC 52 canary payload"
        );
    }
}

#[test]
fn fresh_history_cli_reads_only_committed_rows_and_latest_checkpoints() {
    let fixture = PrivateFixture::new().expect("create private PANELINK_HOME fixture");
    let history =
        Arc::new(HistoryStore::open(&fixture.history).expect("open private history store"));
    let size = TerminalSize::new(8, 72).unwrap();
    let script = format!(
        r#"for n in 01 02 03 04 05 06 07 08 09 10 11 12 13 14 15 16 17 18; do printf 'NORMAL-ROW-%s\r\n' "$n"; done
printf 'NORMAL-GATE\r\n'
IFS= read -r gate
printf '\033[?1049h\033[2J\033[HALT-A-UNRECORDED\r\n'
IFS= read -r gate
printf '\033[2J\033[HALT-B-LATEST\r\n'
printf '\033]52;c;{OSC52_CANARY}\a'
printf 'ALT-B-GATE\r\n'
IFS= read -r gate
printf '\033[?1049l\033[2J\033[HFINAL-NORMAL-LATEST\r\n'
IFS= read -r gate
exit 0"#
    );
    let mut session = launch_shell(&script, &fixture.home, size);

    pump_terminal_until(&mut session, "normal scrollback gate", |screen| {
        !screen.alternate_screen() && screen_text(screen).contains("NORMAL-GATE")
    });
    session
        .text("enter-alt-a", true)
        .expect("advance normal gate");
    pump_terminal_until(&mut session, "alternate A gate", |screen| {
        screen.alternate_screen() && screen_text(screen).contains(ALTERNATE_A)
    });
    assert!(screen_text(session.snapshot()).contains(ALTERNATE_A));

    // The session remains a raw TerminalSession here. Reaching the B gate
    // proves A was parsed and overwritten before any PtyEffects capture call.
    session
        .text("replace-with-b", true)
        .expect("advance alternate A gate");
    pump_terminal_until(&mut session, "alternate B gate after OSC 52", |screen| {
        screen.alternate_screen() && screen_text(screen).contains("ALT-B-GATE")
    });
    let alternate_b = screen_text(session.snapshot());
    assert!(alternate_b.contains(ALTERNATE_B));
    assert!(!alternate_b.contains(ALTERNATE_A));

    let mut effects = PtyEffects::new(session);
    effects.enable_capture(history.clone(), Arc::new(AtomicBool::new(false)), target());
    effects.capture_now();
    assert!(
        !effects.capture_failed(),
        "initial B/scrollback capture failed"
    );

    let saved_rows = history
        .load_rows(target())
        .expect("read persisted normal rows")
        .expect("normal scrollback segment exists");
    let row_text = saved_rows
        .records
        .iter()
        .map(|record| history_rows::decode(record).expect("decode saved row"))
        .map(|row| {
            (0..row.width)
                .filter_map(|column| row.cell_text(column))
                .collect::<String>()
        })
        .collect::<String>();
    assert!(
        row_text.contains(NORMAL_ROW),
        "retained normal row was not saved"
    );

    let initial_screens = history
        .load_screens(target())
        .expect("read initial checkpoints");
    assert!(
        initial_screens[0].is_none(),
        "normal checkpoint was not captured yet"
    );
    let encoded_b = initial_screens[1]
        .as_deref()
        .expect("alternate B checkpoint was captured");
    let checkpoint_b = screen_checkpoint::decode(encoded_b).expect("decode alternate B");
    let text_b = checkpoint_text(&checkpoint_b);
    assert!(text_b.contains(ALTERNATE_B));
    assert!(!text_b.contains(ALTERNATE_A));

    assert_eq!(
        EffectSink::input(
            &mut effects,
            Command::Submit {
                owner_revision: 1,
                text: "return-to-normal",
            }
        ),
        Outcome::Applied
    );
    poll_effects_until(&mut effects, "final normal checkpoint gate", |screen| {
        !screen.alternate_screen() && screen_text(screen).contains(FINAL_NORMAL)
    });
    effects.capture_now();
    assert!(!effects.capture_failed(), "final normal capture failed");

    assert_eq!(
        EffectSink::input(
            &mut effects,
            Command::Submit {
                owner_revision: 1,
                text: "finish",
            }
        ),
        Outcome::Applied
    );
    let deadline = Instant::now() + DEADLINE;
    while !EffectSink::child_exited(&effects) {
        EffectSink::poll(&mut effects).expect("drain bounded PTY output before stop");
        assert!(Instant::now() < deadline, "private PTY child did not exit");
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(EffectSink::stop(&mut effects), StopOutcome::Exited);
    drop(effects);
    drop(history);

    let target_arg = target_argument(target());
    let before_cli = storage_snapshot(&fixture.state).expect("snapshot committed private store");
    let shown = fixture.panelink(&["history", "show", &target_arg]);
    assert!(shown.status.success(), "history show failed: {shown:?}");
    let show_text = String::from_utf8_lossy(&shown.stdout);
    assert!(
        show_text.contains(NORMAL_ROW),
        "saved normal row missing from CLI"
    );
    assert!(
        show_text.contains("checkpoint normal") && show_text.contains(FINAL_NORMAL),
        "latest normal checkpoint is not separately labeled: {show_text}"
    );
    assert!(
        show_text.contains("checkpoint alternate") && show_text.contains(ALTERNATE_B),
        "latest alternate checkpoint B is not separately labeled: {show_text}"
    );
    assert!(
        !show_text.contains(ALTERNATE_A),
        "overwritten alternate A was recovered"
    );
    assert_cli_output_is_sanitized(&shown);
    assert_eq!(
        storage_snapshot(&fixture.state).expect("snapshot after show"),
        before_cli,
        "read-only history show changed the stored tree"
    );

    let searched = fixture.panelink(&["history", "search", &target_arg, ALTERNATE_B]);
    assert!(
        searched.status.success(),
        "history search failed: {searched:?}"
    );
    let search_text = String::from_utf8_lossy(&searched.stdout);
    assert!(
        search_text.contains("checkpoint alternate") && search_text.contains(ALTERNATE_B),
        "search result did not identify the alternate checkpoint: {search_text}"
    );
    assert!(!search_text.contains(ALTERNATE_A));
    assert_cli_output_is_sanitized(&searched);
    assert_eq!(
        storage_snapshot(&fixture.state).expect("snapshot after search"),
        before_cli,
        "read-only history search changed the stored tree"
    );

    assert!(
        !fixture.child_marker.exists(),
        "history query executed the configured shell child probe"
    );
    for runtime_path in [
        fixture.state.join("server.pid"),
        fixture.server.join("control.sock"),
        fixture.server.join("server.lock"),
        fixture.server.join("generation"),
        fixture.server.join("server.log"),
        fixture.state.join("server.log"),
        fixture.state.join("cli.ctx"),
    ] {
        assert!(
            !runtime_path.exists(),
            "history query created server runtime state at {}",
            runtime_path.display()
        );
    }
    assert_eq!(
        fs::read_dir(&fixture.state)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>(),
        [OsString::from("server")],
        "history query created a workspace runtime entry"
    );
    assert_eq!(
        fs::read_dir(&fixture.server)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>(),
        [OsString::from("history")],
        "history query created server runtime files"
    );
}
