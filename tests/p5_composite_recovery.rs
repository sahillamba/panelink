#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Composite process-restart acceptance for separately committed saved layout
//! and terminal history. This uses a private PTY and private PANELINK_HOME; it
//! is not physical reboot or filesystem power-loss evidence.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use panelink::close_protocol::StopOutcome;
use panelink::control_protocol::{Command, TargetRef};
use panelink::history_rows;
use panelink::history_store::HistoryStore;
use panelink::launch::{Environment, ResolvedLaunch};
use panelink::pane_layout::{LayoutSnapshot, PaneGeometry, Rect};
use panelink::protocol::Outcome;
use panelink::recovery_links::{LinkOutcome, LinkStore};
use panelink::saved_layout::{encode, LayoutMetadata, SavedLayout};
use panelink::screen_checkpoint::{self, ScreenCheckpoint};
use panelink::server::EffectSink;
use panelink::server_pty::PtyEffects;
use panelink::server_storage::ServerStorage;
use panelink::terminal::{ScreenSnapshot, TerminalSession, TerminalSize};

const DEADLINE: Duration = Duration::from_secs(8);
const OLD_GENERATION: u64 = 91;
const NORMAL_ROW: &str = "COMPOSITE-NORMAL-ROW-03";
const FINAL_NORMAL: &str = "COMPOSITE-FINAL-NORMAL";
const ALTERNATE_A: &str = "COMPOSITE-ALT-A-UNRECORDED";
const ALTERNATE_B: &str = "COMPOSITE-ALT-B-LATEST";
const OSC52_CANARY: &str = "U0VDUkVULUNPTVBPU0lURS1PU0M1Mg==";
const INPUT_CANARY: &str = "COMPOSITE-CONTROL-EFFECT-CANARY";
const SHELL_CANARY: &str = "COMPOSITE-FRESH-SHELL-ENV-CANARY";

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
    state: PathBuf,
    home: PathBuf,
    cwd: PathBuf,
    server: PathBuf,
    history: PathBuf,
    marker: PathBuf,
    input_effect: PathBuf,
    replay_effect: PathBuf,
    shell: PathBuf,
}

impl Fixture {
    fn new() -> std::io::Result<Self> {
        let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = Path::new("/private/tmp").join(format!(
            "panelink-p5-composite-{}-{sequence}",
            std::process::id()
        ));
        create_private_dir(&root)?;
        let fixture = Self {
            state: root.join("state"),
            home: root.join("home"),
            cwd: root.join("fresh-cwd"),
            server: root.join("state/server"),
            history: root.join("state/server/history"),
            marker: root.join("shell-starts"),
            input_effect: root.join("control-effect-ran"),
            replay_effect: root.join("control-effect-replayed"),
            shell: root.join("fresh-shell"),
            root,
        };

        for path in [&fixture.state, &fixture.home, &fixture.cwd, &fixture.server] {
            create_private_dir(path)?;
        }
        let server_storage = ServerStorage::initialize(&fs::canonicalize(&fixture.server)?)
            .expect("initialize private server storage");
        drop(server_storage);
        create_private_dir(&fixture.history)?;

        let script = format!(
            "#!/bin/sh\nIFS= read -r -t 1 replayed\nif [ \"$replayed\" = '{}' ]; then printf 'replayed\\n' > '{}'; fi\nprintf '%s|%s|%s|%s\\n' \"$PWD\" \"$HOME\" \"$RELAUNCH_TEST_CANARY\" \"$PANELINK_GENERATION\" >> '{}'\nexec /bin/sleep 60\n",
            INPUT_CANARY,
            fixture.replay_effect.display(),
            fixture.marker.display()
        );
        fs::write(&fixture.shell, script)?;
        fs::set_permissions(&fixture.shell, fs::Permissions::from_mode(0o700))?;
        Ok(fixture)
    }

    fn command(&self, arguments: &[&str]) -> ProcessCommand {
        let mut command = ProcessCommand::new(env!("CARGO_BIN_EXE_panelink"));
        command
            .args(arguments)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("ZDOTDIR", &self.home)
            .env("SHELL", &self.shell)
            .env("PANELINK_HOME", &self.state)
            .env("RELAUNCH_TEST_CANARY", SHELL_CANARY)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn run(&self, arguments: &[&str], timeout: Duration) -> Output {
        run_bounded(self.command(arguments), timeout)
            .unwrap_or_else(|| panic!("panelink {arguments:?} exceeded {timeout:?}"))
    }

    fn write_layout(&self, layout: &SavedLayout) {
        let directory = self.server.join("layout");
        if !directory.exists() {
            create_private_dir(&directory).unwrap();
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(directory.join("current.layout"))
            .unwrap();
        let record = encode(layout);
        assert_eq!(panelink::saved_layout::decode(&record), Ok(*layout));
        file.write_all(&record).unwrap();
        file.sync_all().unwrap();
    }

    fn assert_no_child_spawn(&self) {
        assert!(
            !self.marker.exists(),
            "inspection launched the configured shell"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Relaunch starts a detached server. Ask that private server to stop
        // before removing its root, including when an assertion unwinds.
        let _ = run_bounded(self.command(&["stop"]), Duration::from_secs(5));
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct OwnedChild {
    child: Child,
    process_group: i32,
}

impl OwnedChild {
    fn spawn(mut command: ProcessCommand) -> std::io::Result<Self> {
        command.process_group(0);
        let child = command.spawn()?;
        let process_group = i32::try_from(child.id()).expect("child PID fits pid_t");
        Ok(Self {
            child,
            process_group,
        })
    }

    fn run(mut self, timeout: Duration) -> Option<Output> {
        let mut stdout = self.child.stdout.take()?;
        let mut stderr = self.child.stderr.take()?;
        let (sender, receiver) = mpsc::channel();
        let stdout_sender = sender.clone();
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout.read_to_end(&mut bytes).map(|_| bytes);
            let _ = stdout_sender.send((true, result));
        });
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stderr.read_to_end(&mut bytes).map(|_| bytes);
            let _ = sender.send((false, result));
        });
        let deadline = Instant::now() + timeout;
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                Ok(None) => {
                    self.kill_group();
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return None;
                }
                Err(_) => {
                    self.kill_group();
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return None;
                }
            }
        };
        let mut output = Vec::new();
        let mut error = Vec::new();
        for _ in 0..2 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(remaining) {
                Ok((true, Ok(bytes))) => output = bytes,
                Ok((false, Ok(bytes))) => error = bytes,
                _ => {
                    self.kill_group();
                    return None;
                }
            }
        }
        Some(Output {
            status,
            stdout: output,
            stderr: error,
        })
    }

    fn kill_group(&self) {
        // SAFETY: process_group is the private process group created for this
        // exact fixture subprocess by CommandExt::process_group(0).
        unsafe {
            libc::kill(-self.process_group, libc::SIGKILL);
        }
    }
}

fn run_bounded(command: ProcessCommand, timeout: Duration) -> Option<Output> {
    OwnedChild::spawn(command).ok()?.run(timeout)
}

fn create_private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(path)
}

fn old_target(pane: u64, session: u64) -> TargetRef {
    TargetRef {
        generation: OLD_GENERATION,
        workspace: 1,
        pane,
        session,
        occupant_revision: 1,
    }
}

fn layout() -> SavedLayout {
    let rows = 24;
    let columns = 82;
    SavedLayout::from_snapshot(
        LayoutMetadata::new(OLD_GENERATION, 1, 17, 1, 10_017).unwrap(),
        &LayoutSnapshot {
            revision: 1,
            rows,
            columns,
            panes: [
                Some(PaneGeometry {
                    target: old_target(7, 9),
                    rect: Rect {
                        x: 0,
                        y: 0,
                        rows,
                        columns: 40,
                    },
                }),
                Some(PaneGeometry {
                    target: old_target(8, 10),
                    rect: Rect {
                        x: 41,
                        y: 0,
                        rows,
                        columns: 41,
                    },
                }),
                None,
                None,
            ],
            divider: Some(Rect {
                x: 40,
                y: 0,
                rows,
                columns: 1,
            }),
            dividers: [
                Some(Rect {
                    x: 40,
                    y: 0,
                    rows,
                    columns: 1,
                }),
                None,
                None,
            ],
        },
    )
    .unwrap()
}

fn explicit_environment() -> Environment {
    [
        (OsString::from("TERM"), OsString::from("panelink-probe")),
        (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
    ]
    .into_iter()
    .collect()
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

fn pump_until(
    session: &mut TerminalSession,
    what: &str,
    predicate: impl Fn(&ScreenSnapshot) -> bool,
) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        session.pump().expect("pump private PTY");
        if predicate(session.snapshot()) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

fn poll_effects_until(
    effects: &mut PtyEffects,
    what: &str,
    predicate: impl Fn(&ScreenSnapshot) -> bool,
) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        EffectSink::poll(effects).expect("poll private PTY effects");
        if predicate(effects.screen()) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
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

fn storage_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).expect("read private storage directory") {
            let entry = entry.expect("read private storage entry");
            let child = entry.path();
            let metadata = fs::symlink_metadata(&child).expect("inspect private storage entry");
            assert!(metadata.is_dir() || metadata.is_file());
            let relative = child.strip_prefix(root).unwrap().to_path_buf();
            if metadata.is_dir() {
                entries.insert(relative, Vec::new());
                visit(root, &child, entries);
            } else {
                entries.insert(
                    relative,
                    fs::read(&child).expect("read private storage file"),
                );
            }
        }
    }

    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

fn target_arg(target: TargetRef) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        target.generation, target.workspace, target.pane, target.session, target.occupant_revision
    )
}

fn assert_sanitized(output: &Output) {
    for stream in [&output.stdout, &output.stderr] {
        assert!(!stream.contains(&0x1b), "CLI output contains ESC");
        assert!(!stream.windows(4).any(|window| window == b"]52;"));
        assert!(!stream
            .windows(OSC52_CANARY.len())
            .any(|window| window == OSC52_CANARY.as_bytes()));
    }
}

#[test]
fn fresh_process_composes_saved_layout_history_and_explicit_relaunch() {
    let fixture = Fixture::new().expect("create isolated composite recovery fixture");
    let layout = layout();
    fixture.write_layout(&layout);

    // Seed real PTY output for pane 1. The input canary has one intentional
    // private side effect; the snapshot and fresh shells prove it is not
    // replayed. Alternate A is overwritten before the first checkpoint.
    let first = old_target(7, 9);
    let second = old_target(8, 10);
    let history = Arc::new(HistoryStore::open(&fixture.history).unwrap());
    let script = format!(
        r#"for n in 01 02 03 04 05 06 07 08 09 10 11 12; do printf 'COMPOSITE-NORMAL-ROW-%s\r\n' "$n"; done
printf 'NORMAL-GATE\r\n'
IFS= read -r gate
if [ "$gate" = "{INPUT_CANARY}" ]; then printf 'ran\n' > '{}'; fi
printf '\033[?1049h\033[2J\033[H{ALTERNATE_A}\r\n'
IFS= read -r gate
printf '\033[2J\033[H{ALTERNATE_B}\r\n'
printf '\033]52;c;{OSC52_CANARY}\a'
printf 'ALT-B-GATE\r\n'
IFS= read -r gate
printf '\033[?1049l\033[2J\033[H{FINAL_NORMAL}\r\n'
IFS= read -r gate
exit 0"#,
        fixture.input_effect.display()
    );
    let launch = ResolvedLaunch {
        requested_path: PathBuf::from("/bin/sh"),
        executable_path: PathBuf::from("/bin/sh"),
        working_directory: fixture.home.clone(),
    };
    let size = TerminalSize::new(8, 72).unwrap();
    let mut session = TerminalSession::spawn(
        &launch,
        &[OsString::from("-c"), OsString::from(script)],
        &explicit_environment(),
        size,
    )
    .expect("spawn history seeding shell in a private PTY");

    pump_until(&mut session, "normal output gate", |screen| {
        !screen.alternate_screen() && screen_text(screen).contains("NORMAL-GATE")
    });
    session
        .text(INPUT_CANARY, true)
        .expect("submit one deliberate control canary");
    pump_until(&mut session, "alternate A before overwrite", |screen| {
        screen.alternate_screen() && screen_text(screen).contains(ALTERNATE_A)
    });
    assert!(
        fixture.input_effect.exists(),
        "input canary did not execute once"
    );
    session
        .text("advance-to-b", true)
        .expect("advance from A to B");
    pump_until(&mut session, "alternate B after A overwrite", |screen| {
        screen.alternate_screen() && screen_text(screen).contains("ALT-B-GATE")
    });
    assert!(screen_text(session.snapshot()).contains(ALTERNATE_B));
    assert!(!screen_text(session.snapshot()).contains(ALTERNATE_A));

    let mut effects = PtyEffects::new(session);
    effects.enable_capture(history.clone(), Arc::new(AtomicBool::new(false)), first);
    effects.capture_now();
    assert!(
        !effects.capture_failed(),
        "capture of normal rows and alternate B failed"
    );

    let saved_rows = history
        .load_rows(first)
        .unwrap()
        .expect("normal rows committed");
    let rows_text = saved_rows
        .records
        .iter()
        .map(|record| history_rows::decode(record).unwrap())
        .flat_map(|row| {
            (0..row.width).filter_map(move |column| row.cell_text(column).map(str::to_owned))
        })
        .collect::<String>();
    assert!(
        rows_text.contains(NORMAL_ROW),
        "real PTY normal row was not saved"
    );
    let saved_screens = history.load_screens(first).unwrap();
    assert!(
        saved_screens[0].is_none(),
        "normal checkpoint should not exist yet"
    );
    let alternate = screen_checkpoint::decode(saved_screens[1].as_deref().unwrap()).unwrap();
    let alternate_text = checkpoint_text(&alternate);
    assert!(alternate_text.contains(ALTERNATE_B));
    assert!(!alternate_text.contains(ALTERNATE_A));

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
    poll_effects_until(&mut effects, "final normal checkpoint", |screen| {
        !screen.alternate_screen() && screen_text(screen).contains(FINAL_NORMAL)
    });
    effects.capture_now();
    assert!(
        !effects.capture_failed(),
        "capture of final normal checkpoint failed"
    );

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
        EffectSink::poll(&mut effects).expect("drain private PTY before orderly stop");
        assert!(
            Instant::now() < deadline,
            "history seeding PTY did not exit"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(EffectSink::stop(&mut effects), StopOutcome::Exited);
    drop(effects);
    drop(history);

    // Close the last storage writer and re-open only through fresh CLI
    // processes. Pane 2 deliberately has no history, despite the valid layout.
    let unchanged_before_read = storage_bytes(&fixture.state);
    let view = fixture.run(&["recovery", "view"], DEADLINE);
    assert!(view.status.success(), "recovery view failed: {view:?}");
    let view_text = String::from_utf8_lossy(&view.stdout);
    assert!(view_text.contains("orientation: SideBySide"), "{view_text}");
    assert!(
        view_text.contains(
            "pane 1: target 91:1:7:9:1; rows available; normal available; alternate available"
        ),
        "{view_text}"
    );
    assert!(
        view_text.contains(
            "pane 2: target 91:1:8:10:1; rows missing; normal missing; alternate missing"
        ),
        "{view_text}"
    );
    fixture.assert_no_child_spawn();
    assert_eq!(storage_bytes(&fixture.state), unchanged_before_read);

    let first_arg = target_arg(first);
    let shown = fixture.run(&["history", "show", &first_arg], DEADLINE);
    assert!(shown.status.success(), "history show failed: {shown:?}");
    let shown_text = String::from_utf8_lossy(&shown.stdout);
    assert!(shown_text.contains(NORMAL_ROW), "{shown_text}");
    assert!(shown_text.contains("checkpoint normal") && shown_text.contains(FINAL_NORMAL));
    assert!(shown_text.contains("checkpoint alternate") && shown_text.contains(ALTERNATE_B));
    assert!(
        !shown_text.contains(ALTERNATE_A),
        "overwritten A was recovered"
    );
    assert_sanitized(&shown);
    fixture.assert_no_child_spawn();
    assert_eq!(storage_bytes(&fixture.state), unchanged_before_read);

    // Relaunch is explicit. The stored input/OSC 52/display data must remain
    // historical; new shells receive only fresh cwd/environment values.
    let relaunch = fixture.run(&["recovery", "relaunch"], Duration::from_secs(20));
    assert!(
        relaunch.status.success(),
        "recovery relaunch failed: {relaunch:?}"
    );
    let summary = String::from_utf8_lossy(&relaunch.stdout);
    assert!(
        summary.contains("pane 1: old 91:1:7:9:1 -> new "),
        "{summary}"
    );
    assert!(
        summary.contains("pane 2: old 91:1:8:10:1 -> new "),
        "{summary}"
    );
    let first_pane_position = summary.find("pane 1: old 91:1:7:9:1").unwrap();
    let second_pane_position = summary.find("pane 2: old 91:1:8:10:1").unwrap();
    assert!(
        first_pane_position < second_pane_position,
        "relaunch summary did not retain saved spatial order: {summary}"
    );
    assert_eq!(summary.matches("(applied)").count(), 2, "{summary}");
    assert!(!summary.contains(ALTERNATE_B));
    assert!(!summary.contains(OSC52_CANARY));

    let deadline = Instant::now() + DEADLINE;
    loop {
        if fs::read_to_string(&fixture.marker)
            .map(|contents| contents.lines().count() == 2)
            .unwrap_or(false)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fresh relaunch shells did not start"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let starts = fs::read_to_string(&fixture.marker).unwrap();
    let start_lines = starts.lines().collect::<Vec<_>>();
    assert_eq!(start_lines.len(), 2, "{starts:?}");
    assert!(
        start_lines.iter().all(|line| {
            let fields = line.split('|').collect::<Vec<_>>();
            fields.len() == 4
                && fields[0] == fixture.cwd.to_string_lossy()
                && fields[1] == fixture.home.to_string_lossy()
                && fields[2] == SHELL_CANARY
                && fields[3]
                    .parse::<u64>()
                    .is_ok_and(|generation| generation != OLD_GENERATION)
        }),
        "fresh cwd/environment or generation mismatch: {starts:?}"
    );
    assert!(
        !fixture.replay_effect.exists(),
        "historical input canary was replayed into a fresh shell"
    );
    assert_eq!(fs::read_to_string(&fixture.input_effect).unwrap(), "ran\n");

    let panes = fixture.run(&["panes"], DEADLINE);
    assert!(
        panes.status.success(),
        "panes failed after relaunch: {panes:?}"
    );
    let panes_text = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(panes_text.matches("target ").count(), 2, "{panes_text}");
    assert!(!panes_text.contains("91:1:7:9:1"));
    assert!(!panes_text.contains("91:1:8:10:1"));
    assert!(panes_text.contains(" at col 40, row 0"), "{panes_text}");

    let links = LinkStore::open(&fixture.server.join("links")).unwrap();
    let mut linked_generation = None;
    for historical in [first, second] {
        let records = links.links_for_old(historical).unwrap();
        assert_eq!(records.len(), 1, "old target {historical:?}");
        assert_eq!(records[0].outcome, LinkOutcome::Applied);
        assert_ne!(records[0].new_target.generation, OLD_GENERATION);
        if let Some(expected) = linked_generation {
            assert_eq!(records[0].new_target.generation, expected);
        } else {
            linked_generation = Some(records[0].new_target.generation);
        }
        assert!(
            panes_text.contains(&format!("target {}", target_arg(records[0].new_target))),
            "linked historical target is not a live pane: {panes_text}"
        );
    }

    // Recovery links and live panes are new identities. The saved history is
    // still keyed by the old ID and read-only after relaunch.
    let history_after_relaunch = storage_bytes(&fixture.history);
    let old_history = fixture.run(&["history", "show", &first_arg], DEADLINE);
    assert!(
        old_history.status.success(),
        "historical lookup failed: {old_history:?}"
    );
    let old_history_text = String::from_utf8_lossy(&old_history.stdout);
    assert!(old_history_text.contains(ALTERNATE_B));
    assert!(!old_history_text.contains(ALTERNATE_A));
    assert_sanitized(&old_history);
    assert_eq!(storage_bytes(&fixture.history), history_after_relaunch);
    assert_eq!(fs::read_to_string(&fixture.input_effect).unwrap(), "ran\n");
}
