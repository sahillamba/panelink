#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Parent acceptance through the public multi-server startup/driver API.
//! No pathname listener, user configuration, provider or host TTY is used.

use panelink::control_protocol::TargetRef;
use panelink::history_rows;
use panelink::history_store::HistoryStore;
use panelink::launch::LaunchContext;
use panelink::multi_runtime::MultiServerDriver;
use panelink::multi_startup::{StartedMultiServer, StartupLaunch};
use panelink::screen_checkpoint::{self, ScreenBuffer};
use panelink::server_storage::ServerStorage;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    server: PathBuf,
    history: PathBuf,
    terminfo: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(format!(
            "/private/tmp/pl-live-driver-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let fixture = Self {
            server: root.join("server"),
            history: root.join("server/history"),
            terminfo: root.join("terminfo"),
            root,
        };
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&fixture.server)
            .unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&fixture.terminfo)
            .unwrap();
        fixture
    }

    fn start(&self, script: &str) -> (MultiServerDriver, TargetRef) {
        let storage = ServerStorage::initialize(&self.server).unwrap();
        let context = LaunchContext {
            working_directory: self.root.clone(),
            environment: [
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (OsString::from("HOME"), self.root.as_os_str().to_owned()),
            ]
            .into_iter()
            .collect(),
        };
        let arguments = vec![
            OsString::from("-c"),
            OsString::from(script),
            OsString::from("panelink-live-history-test"),
            self.root.as_os_str().to_owned(),
        ];
        let executable = OsString::from("/bin/sh");
        let started = StartedMultiServer::start(
            storage,
            &self.terminfo,
            &StartupLaunch {
                executable: &executable,
                arguments: &arguments,
                context: &context,
                // Startup's one-cell content inset keeps the PTY at the
                // original 8x60 test size.
                rows: 10,
                columns: 62,
            },
            false,
        )
        .unwrap();
        let target = started.startup().target.unwrap();
        let mut driver = started.into_driver();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&self.history)
            .unwrap();
        driver.enable_history(HistoryStore::open(&self.history).unwrap());
        assert!(driver.history_available());
        (driver, target)
    }

    fn store(&self) -> HistoryStore {
        HistoryStore::open(&self.history).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn checkpoint_text(store: &HistoryStore, target: TargetRef, buffer: ScreenBuffer) -> String {
    store
        .load(target, buffer)
        .unwrap()
        .map(|bytes| {
            let decoded = screen_checkpoint::decode(&bytes).unwrap();
            (0..decoded.rows())
                .map(|row| {
                    (0..decoded.columns())
                        .map(|column| decoded.cell(row, column).unwrap().text().to_owned())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn pump_until(driver: &mut MultiServerDriver, phase: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let _ = driver.tick(|| 1);
        // Exercise the force-capture boundary without waiting five seconds.
        driver.capture_now();
        if ready() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "public driver capture did not settle during {phase}; failed={}, diagnostics={:?}",
            driver.history_capture_failed(),
            driver.history_diagnostics().snapshot()
        );
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn live_driver_saves_rows_and_both_buffers_before_natural_exit() {
    let fixture = Fixture::new();
    let (mut driver, target) = fixture.start(
        "stty size > \"$1/initial-size\"; \
         for n in 1 2 3 4 5 6 7 8 9 10 11 12; do printf 'ROW-%s\\r\\n' \"$n\"; done; \
         printf 'NORMAL-HOLD\\r\\n'; \
         while [ ! -f \"$1/enter-alt\" ]; do /bin/sleep 0.01; done; \
         printf '\\033[?1049hALT-HOLD'; \
         while [ ! -f \"$1/leave-alt\" ]; do /bin/sleep 0.01; done; \
         printf '\\033[?1049l\\r\\nNORMAL-FINAL\\r\\n'",
    );
    let store = fixture.store();
    pump_until(&mut driver, "normal screen", || {
        checkpoint_text(&store, target, ScreenBuffer::Normal).contains("NORMAL-HOLD")
    });
    assert_eq!(
        fs::read_to_string(fixture.root.join("initial-size"))
            .unwrap()
            .trim(),
        "8 60",
        "the child must start directly at its content size"
    );
    fs::write(fixture.root.join("enter-alt"), b"go").unwrap();
    pump_until(&mut driver, "alternate screen", || {
        checkpoint_text(&store, target, ScreenBuffer::Alternate).contains("ALT-HOLD")
    });
    fs::write(fixture.root.join("leave-alt"), b"go").unwrap();
    pump_until(&mut driver, "final rows", || {
        store
            .load_rows(target)
            .unwrap()
            .is_some_and(|saved| saved.ended_at_unix_ms != 0)
    });
    assert!(!driver.history_capture_failed());
    drop(driver);
    let saved = fixture.store().load_rows(target).unwrap().unwrap();
    let text = saved
        .records
        .iter()
        .map(|bytes| {
            let row = history_rows::decode(bytes).unwrap();
            (0..row.width)
                .map(|col| row.cell_text(col).unwrap())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.lines().any(|line| line.trim() == "ROW-1"));
    assert!(checkpoint_text(&store, target, ScreenBuffer::Normal).contains("NORMAL-FINAL"));
    assert!(checkpoint_text(&store, target, ScreenBuffer::Alternate).contains("ALT-HOLD"));
    assert!(!checkpoint_text(&store, target, ScreenBuffer::Normal).contains("ALT-HOLD"));
}

#[test]
fn driver_drop_stops_live_child_then_persists_final_history() {
    let fixture = Fixture::new();
    let (mut driver, target) = fixture.start(
        "trap 'printf \"FINAL-ON-HUP\\r\\n\"; exit 0' HUP; \
         printf 'DROP-READY\\r\\n'; while :; do /bin/sleep 1; done",
    );
    let store = fixture.store();
    pump_until(&mut driver, "drop screen", || {
        checkpoint_text(&store, target, ScreenBuffer::Normal).contains("DROP-READY")
    });
    let failed = driver.history_failure_signal();
    drop(driver);
    assert!(!failed.load(Ordering::Relaxed));
    let saved = store.load_rows(target).unwrap().unwrap();
    assert_ne!(saved.ended_at_unix_ms, 0);
    // Bounded final-output cleanup can explicitly report an incomplete tail.
    assert!(
        checkpoint_text(&store, target, ScreenBuffer::Normal).contains("FINAL-ON-HUP") || saved.gap
    );
    drop(ServerStorage::lock_for_maintenance(&fixture.server).unwrap());
}
