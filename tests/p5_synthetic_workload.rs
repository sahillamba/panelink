#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::ffi::OsString;
use std::fs;
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use panelink::launch::{Environment, ResolvedLaunch};
use panelink::terminal::{TerminalSession, TerminalSize};

const CHILD_COUNT: usize = 10;
const LOAD_LINES_PER_CHILD: usize = 100;
const ROWS: u16 = 24;
const COLUMNS: u16 = 80;
const MAX_ASCII_CELL_TEXT_BYTES: usize = 1;
const IO_DEADLINE: Duration = Duration::from_secs(15);
const MAX_PUMP_ROUNDS: usize = 7_500;
const POLL_INTERVAL: Duration = Duration::from_millis(2);
const TERM_NAME: &str = "panelink-probe";

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> io::Result<Self> {
        let root = std::env::temp_dir();
        for _ in 0..100 {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!(
                "panelink-p5-workload-{}-{sequence}",
                std::process::id()
            ));
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique workload directory",
        ))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Explicit test guard: all still-live PTY children get TerminalSession's
/// bounded HUP/KILL/reap cleanup during normal return and panic unwinding.
struct ChildBatch(Vec<TerminalSession>);

impl Drop for ChildBatch {
    fn drop(&mut self) {
        for session in &mut self.0 {
            let _ = session.shutdown();
        }
    }
}

#[derive(Default)]
struct PumpMetrics {
    rounds: usize,
    calls: usize,
    bytes_read_total: u64,
    bytes_read_by_child: [u64; CHILD_COUNT],
}

#[test]
fn ten_synthetic_children_keep_isolated_ptys_under_bounded_output_load() {
    let test_started = Instant::now();
    let deadline = test_started + IO_DEADLINE;
    let rss_start_bytes = peak_rss_bytes().expect("read test-process start peak RSS");
    let working_directory = TempDir::new().expect("create private workload cwd");
    let launch = ResolvedLaunch {
        requested_path: PathBuf::from("/bin/sh"),
        executable_path: PathBuf::from("/bin/sh"),
        working_directory: working_directory.path().to_path_buf(),
    };
    let environment: Environment = [(OsString::from("TERM"), OsString::from(TERM_NAME))]
        .into_iter()
        .collect();
    let size = TerminalSize::new(ROWS, COLUMNS).expect("fixed prototype size is valid");
    let mut children = ChildBatch(Vec::with_capacity(CHILD_COUNT));

    for child in 0..CHILD_COUNT {
        let script = child_script(child);
        let session = TerminalSession::spawn(
            &launch,
            &[OsString::from("-c"), OsString::from(script)],
            &environment,
            size,
        )
        .unwrap_or_else(|error| panic!("spawn synthetic PTY child {child:02}: {error}"));
        children.0.push(session);
    }
    assert_eq!(children.0.len(), CHILD_COUNT);

    let mut metrics = PumpMetrics::default();
    pump_round_robin_until(
        &mut children,
        deadline,
        &mut metrics,
        "all ten children reached READY",
        |sessions| {
            sessions
                .iter()
                .enumerate()
                .all(|(child, session)| snapshot_text(session).contains(&ready_marker(child)))
        },
    );

    for (child, session) in children.0.iter_mut().enumerate() {
        assert!(
            session.exit_status().is_none(),
            "child {child:02} exited before the input handshake"
        );
    }

    for (child, session) in children.0.iter_mut().enumerate() {
        session
            .text(&input_token(child), true)
            .unwrap_or_else(|error| panic!("send input token to child {child:02}: {error}"));
    }

    pump_round_robin_until(
        &mut children,
        deadline,
        &mut metrics,
        "all ten children drained and reaped",
        |sessions| {
            sessions
                .iter()
                .all(|session| session.pty_eof() && session.exit_status().is_some())
        },
    );

    for (child, session) in children.0.iter_mut().enumerate() {
        let output = snapshot_text(session);
        let expected = [
            ready_marker(child),
            load_count_marker(child),
            last_load_marker(child),
            ack_marker(child),
            final_marker(child),
        ];
        for marker in expected {
            assert!(
                output.contains(&marker),
                "child {child:02} is missing its own output marker {marker:?}"
            );
        }
        assert!(
            session.exit_status().is_some_and(|status| status.success()),
            "child {child:02} did not exit normally"
        );
        assert!(session.pty_eof(), "child {child:02} PTY did not reach EOF");

        for sibling in 0..CHILD_COUNT {
            if sibling == child {
                continue;
            }
            for marker in [
                ready_marker(sibling),
                load_count_marker(sibling),
                load_prefix(sibling),
                input_token(sibling),
                ack_prefix(sibling),
                final_marker(sibling),
            ] {
                assert!(
                    !output.contains(&marker),
                    "child {child:02} output contains sibling {sibling:02} marker {marker:?}"
                );
            }
        }
        assert!(
            metrics.bytes_read_by_child[child] > 0,
            "child {child:02} contributed no observed PTY output"
        );
        session
            .shutdown()
            .unwrap_or_else(|error| panic!("close/reap child {child:02}: {error}"));
    }

    assert_eq!(metrics.rounds, metrics.calls.div_ceil(CHILD_COUNT));
    let rss_end_bytes = peak_rss_bytes().expect("read test-process end peak RSS");
    assert!(rss_start_bytes > 0);
    assert!(rss_end_bytes >= rss_start_bytes);

    eprintln!(
        "P5_SYNTHETIC_WORKLOAD children={CHILD_COUNT} load_lines_per_child={LOAD_LINES_PER_CHILD} poll_rounds={} pump_calls={} pty_bytes_read={} elapsed_ms={} test_process_peak_rss_start_bytes={rss_start_bytes} test_process_peak_rss_end_bytes={rss_end_bytes}",
        metrics.rounds,
        metrics.calls,
        metrics.bytes_read_total,
        test_started.elapsed().as_millis(),
    );
}

fn child_script(child: usize) -> String {
    format!(
        "i=1; while [ \"$i\" -le {lines} ]; do printf 'P5-LOAD-{child:02}-%03d\\n' \"$i\"; i=$((i + 1)); done; i=$((i - 1)); printf 'P5-LOAD-COUNT-{child:02}:%s\\n' \"$i\"; printf 'P5-READY-{child:02}\\n'; IFS= read -r token || exit 20; [ \"$token\" = '{token}' ] || exit 21; printf 'P5-ACK-{child:02}:%s\\n' \"$token\"; printf 'P5-FINAL-{child:02}\\n'",
        lines = LOAD_LINES_PER_CHILD,
        child = child,
        token = input_token(child),
    )
}

fn pump_round_robin_until(
    children: &mut ChildBatch,
    deadline: Instant,
    metrics: &mut PumpMetrics,
    phase: &str,
    predicate: impl Fn(&[TerminalSession]) -> bool,
) {
    for _ in 0..MAX_PUMP_ROUNDS {
        for (child, session) in children.0.iter_mut().enumerate() {
            let outcome = session
                .pump()
                .unwrap_or_else(|error| panic!("bounded PTY pump child {child:02}: {error}"));
            metrics.calls += 1;
            let bytes = u64::try_from(outcome.bytes_read).expect("read count fits u64");
            metrics.bytes_read_total = metrics
                .bytes_read_total
                .checked_add(bytes)
                .expect("bounded read counter does not overflow");
            metrics.bytes_read_by_child[child] = metrics.bytes_read_by_child[child]
                .checked_add(bytes)
                .expect("bounded per-child read counter does not overflow");
        }
        metrics.rounds += 1;
        if predicate(&children.0) {
            assert!(Instant::now() < deadline, "{phase} exceeded 15 seconds");
            return;
        }
        assert!(Instant::now() < deadline, "{phase} exceeded 15 seconds");
        thread::sleep(POLL_INTERVAL);
    }
    panic!("{phase} exceeded {MAX_PUMP_ROUNDS} bounded poll rounds");
}

fn snapshot_text(session: &TerminalSession) -> String {
    let snapshot = session.snapshot();
    let rows = usize::from(snapshot.rows());
    let columns = usize::from(snapshot.columns());
    let max_bytes = rows
        .checked_mul(
            columns
                .checked_mul(MAX_ASCII_CELL_TEXT_BYTES)
                .expect("snapshot column bound fits usize")
                + 1,
        )
        .expect("snapshot byte bound fits usize");
    let mut text = String::with_capacity(max_bytes);
    for row in 0..snapshot.rows() {
        for column in 0..snapshot.columns() {
            if let Some(cell) = snapshot.cell(row, column) {
                let cell_text = cell.text();
                assert!(
                    cell_text.is_ascii() && cell_text.len() <= MAX_ASCII_CELL_TEXT_BYTES,
                    "synthetic workload snapshot must remain ASCII-only"
                );
                assert!(
                    text.len() + cell_text.len() <= max_bytes,
                    "fixture snapshot exceeded its fixed text collection bound"
                );
                text.push_str(cell_text);
            }
        }
        text.push('\n');
    }
    text
}

fn ready_marker(child: usize) -> String {
    format!("P5-READY-{child:02}")
}

fn load_prefix(child: usize) -> String {
    format!("P5-LOAD-{child:02}-")
}

fn load_count_marker(child: usize) -> String {
    format!("P5-LOAD-COUNT-{child:02}:{LOAD_LINES_PER_CHILD}")
}

fn last_load_marker(child: usize) -> String {
    format!("{}{:03}", load_prefix(child), LOAD_LINES_PER_CHILD)
}

fn input_token(child: usize) -> String {
    format!("P5-TOKEN-{child:02}")
}

fn ack_prefix(child: usize) -> String {
    format!("P5-ACK-{child:02}:")
}

fn ack_marker(child: usize) -> String {
    format!("{}{token}", ack_prefix(child), token = input_token(child))
}

fn final_marker(child: usize) -> String {
    format!("P5-FINAL-{child:02}")
}

fn peak_rss_bytes() -> io::Result<u64> {
    let mut usage = MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: RUSAGE_SELF is the current test process and `usage` points to a
    // writable libc::rusage of the SDK-declared layout; success initializes it.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful getrusage initialized the complete structure.
    let max_rss = unsafe { usage.assume_init() }.ru_maxrss;
    u64::try_from(max_rss)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "negative peak RSS value"))
}
