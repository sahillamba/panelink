#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use panelink::launch::{Environment, ResolvedLaunch};
use panelink::terminal::{ScreenSnapshot, TerminalSession, TerminalSize};

const DEADLINE: Duration = Duration::from_secs(4);
static DIR_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct PrivateDir(PathBuf);

impl PrivateDir {
    fn new() -> Self {
        for _ in 0..100 {
            let sequence = DIR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = Path::new("/private/tmp").join(format!(
                "panelink-p5-snapshot-wrap-{}-{sequence}",
                std::process::id()
            ));
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create private PTY working directory: {error}"),
            }
        }
        panic!("could not allocate private PTY working directory")
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn shell(script: &str, cwd: &Path, size: TerminalSize) -> TerminalSession {
    let launch = ResolvedLaunch {
        requested_path: PathBuf::from("/bin/sh"),
        executable_path: PathBuf::from("/bin/sh"),
        working_directory: cwd.to_path_buf(),
    };
    let environment: Environment = [
        (OsString::from("TERM"), OsString::from("panelink-probe")),
        (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
    ]
    .into_iter()
    .collect();
    TerminalSession::spawn(
        &launch,
        &[OsString::from("-c"), OsString::from(script)],
        &environment,
        size,
    )
    .expect("spawn explicit shell in private PTY")
}

fn pump_to_exit(session: &mut TerminalSession) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let outcome = session.pump().expect("pump private PTY");
        if outcome.child_exited && outcome.pty_eof {
            return;
        }
        assert!(Instant::now() < deadline, "private PTY fixture timed out");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn live_snapshot_reports_autowrap_and_hard_newline_from_the_engine() {
    let dir = PrivateDir::new();
    let size = TerminalSize::new(8, 40).unwrap();

    let mut wrapped = shell(
        "printf 'ABCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHIJKLMNO'",
        &dir.0,
        size,
    );
    pump_to_exit(&mut wrapped);
    assert_eq!(wrapped.snapshot().row_wrapped(0), Some(true));
    assert_eq!(wrapped.snapshot().row_wrapped(1), Some(false));

    let mut hard_break = shell(
        "printf 'ABCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHIJKLM\\r\\nNO'",
        &dir.0,
        size,
    );
    pump_to_exit(&mut hard_break);
    assert_eq!(hard_break.snapshot().row_wrapped(0), Some(false));
}

#[test]
fn alternate_and_scrollback_snapshots_keep_their_own_visible_wrap_flags() {
    let dir = PrivateDir::new();
    let size = TerminalSize::new(8, 40).unwrap();
    let mut session = shell(
        "printf 'NORM1\\r\\nNORM2\\r\\nNORM3\\r\\nNORM4\\r\\nNORM5\\r\\nNORM6'; printf '\\033[?1049hABCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHIJKLMNO'",
        &dir.0,
        size,
    );
    pump_to_exit(&mut session);
    assert!(session.snapshot().alternate_screen());
    assert_eq!(session.snapshot().row_wrapped(0), Some(true));

    // Alternate screens have no normal-history viewport, regardless of offset.
    let (alt_view, actual) = session.history_snapshot(2).unwrap();
    assert_eq!(actual, 0);
    assert_eq!(alt_view.row_wrapped(0), session.snapshot().row_wrapped(0));

    let mut normal = shell(
        "printf '\\033[?1049lNORM1\\r\\nNORM2\\r\\nNORM3\\r\\nNORM4\\r\\nNORM5\\r\\nNORM6\\r\\nABCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHIJKLMNO\\r\\nFINAL'",
        &dir.0,
        size,
    );
    pump_to_exit(&mut normal);
    let live = normal.snapshot();
    let (older, actual) = normal.history_snapshot(1).unwrap();
    assert_eq!(actual, 1);
    assert!(older.row_wraps().is_some());
    assert_ne!(older.row_wraps(), live.row_wraps());
}

#[test]
fn checkpoint_style_snapshot_metadata_is_volatile_and_dimension_checked() {
    let size = TerminalSize::new(8, 40).unwrap();
    let mut unknown = ScreenSnapshot::try_new(size).unwrap();
    assert_eq!(unknown.row_wrapped(0), None);
    assert_eq!(unknown.row_wraps(), None);

    let wraps = [true, false, true, false, false, true, false, false];
    unknown.apply_row_wraps(&wraps).unwrap();
    assert_eq!(unknown.row_wrapped(0), Some(true));
    assert_eq!(unknown.row_wraps(), Some(&wraps[..]));

    let mut copied = ScreenSnapshot::try_new(size).unwrap();
    copied.copy_from(&unknown);
    assert_eq!(copied.row_wraps(), unknown.row_wraps());

    unknown.clear_row_wraps();
    copied.copy_from(&unknown);
    assert_eq!(copied.row_wrapped(0), None);
    assert!(unknown.apply_row_wraps(&wraps[..7]).is_err());
    assert_eq!(unknown.row_wrapped(0), None);
}
