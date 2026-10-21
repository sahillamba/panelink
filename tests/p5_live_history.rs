#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Public live-capture acceptance through the multi-server driver.

use panelink::control_protocol::TargetRef;
use panelink::history_rows;
use panelink::history_store::HistoryStore;
use panelink::launch::LaunchContext;
use panelink::multi_runtime::MultiServerDriver;
use panelink::multi_startup::{StartedMultiServer, StartupLaunch};
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
        let root = PathBuf::from("/private/tmp").join(format!(
            "panelink-p5-live-history-{}-{}",
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

    fn start(&self) -> (MultiServerDriver, TargetRef) {
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
        let executable = OsString::from("/bin/sh");
        let arguments = vec![
            OsString::from("-c"),
            OsString::from(
                "for n in 1 2 3 4 5 6 7 8 9 10 11 12; do printf 'LIVE-ROW-%s\\r\\n' \"$n\"; done",
            ),
        ];
        let started = StartedMultiServer::start(
            storage,
            &self.terminfo,
            &StartupLaunch {
                executable: &executable,
                arguments: &arguments,
                context: &context,
                rows: 8,
                columns: 60,
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

#[test]
fn live_driver_persists_scrollback_and_end_after_natural_child_exit() {
    let fixture = Fixture::new();
    let (mut driver, target) = fixture.start();
    let store = fixture.store();
    let deadline = Instant::now() + Duration::from_secs(8);
    let saved = loop {
        driver.tick(|| 1).unwrap();
        driver.capture_now();
        if let Some(saved) = store.load_rows(target).unwrap() {
            if saved.ended_at_unix_ms != 0 {
                break saved;
            }
        }
        assert!(
            Instant::now() < deadline,
            "live history did not reach its end receipt"
        );
        thread::sleep(Duration::from_millis(2));
    };

    let text = saved
        .records
        .iter()
        .map(|record| {
            let row = history_rows::decode(record).unwrap();
            (0..row.width)
                .map(|column| row.cell_text(column).unwrap())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("LIVE-ROW-1"));
    assert!(!text.contains("LIVE-ROW-12"));

    let screens = store.load_screens(target).unwrap();
    let normal = screens[0].as_deref().expect("normal screen checkpoint");
    let checkpoint = panelink::screen_checkpoint::decode(normal).unwrap();
    let screen_text = (0..checkpoint.rows())
        .map(|row| {
            (0..checkpoint.columns())
                .map(|column| checkpoint.cell(row, column).unwrap().text())
                .collect::<String>()
        })
        .collect::<String>();
    assert!(screen_text.contains("LIVE-ROW-12"));
    assert!(!driver.history_capture_failed());
    drop(driver);
}
