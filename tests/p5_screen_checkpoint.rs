#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use panelink::control_protocol::TargetRef;
use panelink::launch::{Environment, ResolvedLaunch};
use panelink::screen_checkpoint::{
    self, CheckpointColor, CheckpointMetadata, ScreenBuffer, ScreenCheckpoint,
};
use panelink::terminal::{ScreenSnapshot, TerminalSession, TerminalSize};

const PTY_DEADLINE: Duration = Duration::from_secs(4);
static PRIVATE_DIR_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct PrivateDir(PathBuf);

impl PrivateDir {
    fn new() -> std::io::Result<Self> {
        for _ in 0..100 {
            let sequence = PRIVATE_DIR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = Path::new("/private/tmp").join(format!(
                "panelink-p5-checkpoint-{}-{sequence}",
                std::process::id()
            ));
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&path) {
                Ok(()) => {
                    return Ok(Self(path));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
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

fn pump_until(session: &mut TerminalSession, predicate: impl Fn(&ScreenSnapshot) -> bool) {
    let deadline = Instant::now() + PTY_DEADLINE;
    loop {
        session.pump().expect("bounded private-PTY pump");
        if predicate(session.snapshot()) {
            return;
        }
        assert!(Instant::now() < deadline, "private PTY fixture timed out");
        thread::sleep(Duration::from_millis(5));
    }
}

fn finish(session: &mut TerminalSession) {
    let deadline = Instant::now() + PTY_DEADLINE;
    loop {
        let outcome = session.pump().expect("bounded PTY pump during child exit");
        if outcome.pty_eof && outcome.child_exited {
            return;
        }
        assert!(Instant::now() < deadline, "private PTY child did not exit");
        thread::sleep(Duration::from_millis(5));
    }
}

fn checkpoint_metadata(output_sequence: u64, screen_revision: u64) -> CheckpointMetadata {
    CheckpointMetadata::new(
        TargetRef {
            generation: 17,
            workspace: 23,
            pane: 29,
            session: 31,
            occupant_revision: 37,
        },
        output_sequence,
        1_792_288_800_123,
        screen_revision,
    )
    .unwrap()
}

#[test]
fn hup_final_output_is_captured_or_reported_incomplete_before_pty_close() {
    let directory = PrivateDir::new().unwrap();
    let size = TerminalSize::new(8, 60).unwrap();
    let mut session = launch_shell(
        // Keep the shell itself blocked after the readiness marker. A foreground
        // sleep shares the process group and makes the HUP/trap ordering depend
        // on scheduling the extra child before the cleanup escalation window.
        "trap 'printf P5-FINAL-ON-HUP\\n; exit 0' HUP; printf P5-HUP-READY\\n; while :; do IFS= read -r line; done",
        directory.path(),
        size,
    );
    pump_until(&mut session, |snapshot| {
        screen_text(snapshot).contains("P5-HUP-READY")
    });
    let eof = session.shutdown_with_final_output().unwrap();
    if eof {
        assert!(screen_text(session.snapshot()).contains("P5-FINAL-ON-HUP"));
    } else {
        // A caller must persist an explicit gap rather than claiming that
        // cleanup output could be fully recovered without a PTY EOF.
        assert!(!session.pty_eof());
    }
    assert!(session.exit_status().is_some());
    assert!(session.pump().is_err(), "shutdown keeps the PTY closed");
}

fn cell_containing<'a>(
    checkpoint: &'a ScreenCheckpoint<'_>,
    text: &str,
) -> impl Iterator<Item = panelink::screen_checkpoint::CheckpointCell<'a>> + 'a {
    let text = text.to_owned();
    checkpoint
        .cells()
        .filter(move |cell| cell.text().contains(&text))
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

fn assert_no_control_stream(checkpoint: &ScreenCheckpoint<'_>, record: &[u8]) {
    for cell in checkpoint.cells() {
        assert!(
            cell.text().chars().all(|character| !character.is_control()),
            "checkpoint cell retained a control character"
        );
    }
    assert!(!record
        .windows(b"U0VDUkVULVNDUkVFTi1DQU5BUlk=".len())
        .any(|bytes| bytes == b"U0VDUkVULVNDUkVFTi1DQU5BUlk="));
}

#[test]
fn private_pty_normal_and_alternate_snapshots_encode_only_safe_cells() {
    let directory = PrivateDir::new().unwrap();
    let size = TerminalSize::new(8, 60).unwrap();
    let script = r"printf '\033[2J\033[H\033[1;3;4;7;38;5;196;48;2;1;2;3mNORMAL-READY λ界\033[0m\n'; printf '\033]52;c;U0VDUkVULVNDUkVFTi1DQU5BUlk=\a'; IFS= read -r gate; printf '\033[?1049h\033[2J\033[H\033[2;38;2;11;22;33;48;5;201mALT-READY Ω界\033[0m'; printf '\033]52;c;U0VDUkVULVNDUkVFTi1DQU5BUlk=\a'; IFS= read -r gate; printf '\033[?1049l\033[2J\033[HAFTER-READY\n'";
    let mut session = launch_shell(script, directory.path(), size);

    // Hold the child in its normal screen until that exact visible snapshot is
    // encoded and decoded; then explicitly release it into the alternate buffer.
    pump_until(&mut session, |snapshot| {
        !snapshot.alternate_screen() && screen_text(snapshot).contains("NORMAL-READY")
    });
    assert_eq!(session.pending_input_bytes(), 0);
    let normal_metadata = checkpoint_metadata(11, 1);
    let normal_record = screen_checkpoint::encode(session.snapshot(), &normal_metadata).unwrap();
    let normal = screen_checkpoint::decode(&normal_record).unwrap();
    assert_eq!(normal.buffer(), ScreenBuffer::Normal);
    assert_eq!((normal.rows(), normal.columns()), (8, 60));
    assert_eq!(normal.metadata(), normal_metadata);
    assert!(checkpoint_text(&normal).contains("NORMAL-READY"));
    assert!(cell_containing(&normal, "λ").next().is_some());
    let normal_styled = normal.cell(0, 0).unwrap();
    assert_eq!(normal_styled.text(), "N");
    assert!(normal_styled.bold());
    assert!(normal_styled.italic());
    assert!(normal_styled.underline());
    assert!(normal_styled.inverse());
    assert_eq!(
        normal_styled.foreground(),
        Some(CheckpointColor::Indexed(196))
    );
    assert_eq!(
        normal_styled.background(),
        Some(CheckpointColor::Rgb(1, 2, 3))
    );
    assert!(cell_containing(&normal, "界")
        .next()
        .is_some_and(|lead| lead.is_wide()));
    assert_no_control_stream(&normal, &normal_record);

    session.text("release-to-alt", true).unwrap();
    pump_until(&mut session, |snapshot| {
        snapshot.alternate_screen() && screen_text(snapshot).contains("ALT-READY")
    });
    assert_eq!(session.pending_input_bytes(), 0);
    let alternate_metadata = checkpoint_metadata(12, 2);
    let alternate_record =
        screen_checkpoint::encode(session.snapshot(), &alternate_metadata).unwrap();
    let alternate = screen_checkpoint::decode(&alternate_record).unwrap();
    assert_eq!(alternate.buffer(), ScreenBuffer::Alternate);
    assert_eq!(alternate.metadata(), alternate_metadata);
    assert!(checkpoint_text(&alternate).contains("ALT-READY"));
    assert!(cell_containing(&alternate, "Ω").next().is_some());
    let alternate_styled = alternate.cell(0, 0).unwrap();
    assert_eq!(alternate_styled.text(), "A");
    assert!(alternate_styled.dim());
    assert_eq!(
        alternate_styled.foreground(),
        Some(CheckpointColor::Rgb(11, 22, 33))
    );
    assert_eq!(
        alternate_styled.background(),
        Some(CheckpointColor::Indexed(201))
    );
    assert!(cell_containing(&alternate, "界")
        .next()
        .is_some_and(|lead| lead.is_wide()));
    assert_no_control_stream(&alternate, &alternate_record);

    // Decoding is read-only: the shell stays at its barrier until this explicit
    // typed input advances it back to the normal buffer.
    assert!(session.snapshot().alternate_screen());
    session.text("release-to-normal", true).unwrap();
    pump_until(&mut session, |snapshot| {
        !snapshot.alternate_screen() && screen_text(snapshot).contains("AFTER-READY")
    });
    finish(&mut session);
}

#[test]
fn private_pty_maximum_viewport_encodes_with_the_declared_bound() {
    let directory = PrivateDir::new().unwrap();
    let size = TerminalSize::new(80, 240).unwrap();
    let mut session = launch_shell(
        "printf 'MAX-VIEWPORT-READY\\n'; IFS= read -r gate",
        directory.path(),
        size,
    );
    pump_until(&mut session, |snapshot| {
        screen_text(snapshot).contains("MAX-VIEWPORT-READY")
    });
    let record = screen_checkpoint::encode(session.snapshot(), &checkpoint_metadata(0, 1)).unwrap();
    assert_eq!(record.len(), screen_checkpoint::MAX_SCREEN_CHECKPOINT_BYTES);
    let decoded = screen_checkpoint::decode(&record).unwrap();
    assert_eq!((decoded.rows(), decoded.columns()), (80, 240));
    assert_eq!(decoded.cells().len(), 80 * 240);
    session.text("release", true).unwrap();
    finish(&mut session);
}
