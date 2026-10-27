#![cfg(unix)]

use panelink::control_protocol::TargetRef;
use panelink::launch::{Environment, ResolvedLaunch};
use panelink::protocol::OperationKey;
use panelink::terminal::{Key, TerminalSession, TerminalSize};
use panelink::ui_protocol::{decode_ui_request, encode_key_input, KeyInput, UiRequest};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        for _ in 0..100 {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = PathBuf::from(format!(
                "/private/tmp/panelink-usability-{}-{sequence}",
                std::process::id()
            ));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("private fixture directory: {error}"),
            }
        }
        panic!("private fixture name space exhausted");
    }

    fn spawn(&self, script: &str) -> TerminalSession {
        TerminalSession::spawn(
            &ResolvedLaunch {
                requested_path: PathBuf::from("/bin/sh"),
                executable_path: PathBuf::from("/bin/sh"),
                working_directory: self.0.clone(),
            },
            &[OsString::from("-c"), OsString::from(script)],
            &Environment::from([(OsString::from("TERM"), OsString::from("panelink-probe"))]),
            TerminalSize::new(8, 240).unwrap(),
        )
        .expect("spawn real PTY fixture")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn text(session: &TerminalSession) -> String {
    let screen = session.snapshot();
    let mut output = String::new();
    for row in 0..screen.rows() {
        for column in 0..screen.columns() {
            output.push_str(screen.cell(row, column).unwrap().text());
        }
        output.push('\n');
    }
    output
}

fn until(session: &mut TerminalSession, predicate: impl Fn(&TerminalSession) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        session.pump().expect("bounded PTY progress");
        if predicate(session) {
            return;
        }
        assert!(Instant::now() < deadline, "PTY fixture deadline exceeded");
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn extended_wire_keys_reach_a_raw_child_as_exact_terminal_bytes() {
    let keys = [
        Key::Control(1),
        Key::Control(4),
        Key::Control(23),
        Key::Control(26),
        Key::AltChar('é'),
        Key::Function(1),
        Key::Function(12),
        Key::Delete,
        Key::Insert,
        Key::PageUp,
        Key::PageDown,
        Key::BackTab,
    ];
    let expected = b"\x01\x04\x17\x1a\x1b\xc3\xa9\x1bOP\x1b[24~\x1b[3~\x1b[2~\x1b[5~\x1b[6~\x1b[Z";
    let target = TargetRef {
        generation: 7,
        workspace: 1,
        pane: 1,
        session: 1,
        occupant_revision: 1,
    };
    let payload = encode_key_input(&KeyInput {
        key: OperationKey {
            generation: 7,
            lease: 1,
            sequence: 1,
        },
        target,
        expected_owner_revision: 1,
        keys: &keys,
    })
    .unwrap();
    let UiRequest::KeyInput(view) = decode_ui_request(&payload).unwrap() else {
        panic!("key input request required");
    };
    let fixture = Fixture::new();
    // Raw input prevents the child line discipline from interpreting Ctrl-D/Z.
    let script = format!(
        "/bin/stty raw -echo || exit 1; printf 'READY\\r\\n'; /bin/dd bs=1 count={} 2>/dev/null | /usr/bin/od -An -tx1 | /usr/bin/tr -d ' \\n'; printf '\\r\\nDONE\\r\\n'",
        expected.len()
    );
    let mut session = fixture.spawn(&script);
    until(&mut session, |session| text(session).contains("READY"));
    for key in view.keys() {
        session.key(key).unwrap();
    }
    until(&mut session, |session| {
        session.exit_status().is_some() && session.pty_eof()
    });
    let expected_hex: String = expected.iter().map(|byte| format!("{byte:02x}")).collect();
    assert!(text(&session).contains(&expected_hex));
    assert!(text(&session).contains("DONE"));
    assert!(session.exit_status().unwrap().success());
    assert_eq!(session.pending_input_bytes(), 0);
}

#[test]
fn discard_shutdown_escalates_a_ready_hup_ignoring_continuous_writer() {
    let fixture = Fixture::new();
    let mut session = fixture.spawn(
        "trap '' HUP; printf ready > ready; while :; do printf 'continuous-output\\n'; done",
    );
    let marker = fixture.0.join("ready");
    until(&mut session, |_| marker.is_file());
    let started = Instant::now();
    session.shutdown().expect("bounded shutdown");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(session.kill_escalated());
    assert!(session.exit_status().is_some());
}

#[test]
fn submit_brackets_text_and_keeps_enter_outside_the_paste() {
    for bracketed in [false, true] {
        let expected: &[u8] = if bracketed {
            b"\x1b[200~hello\x1b[201~\r"
        } else {
            b"hello\r"
        };
        let fixture = Fixture::new();
        let mode = if bracketed { "\\033[?2004h" } else { "" };
        let script = format!("/bin/stty raw -echo || exit 1; printf '{mode}READY\\r\\n'; /bin/dd bs=1 count={} 2>/dev/null | /usr/bin/od -An -tx1 | /usr/bin/tr -d ' \\n'; printf '\\r\\nDONE\\r\\n'", expected.len());
        let mut session = fixture.spawn(&script);
        until(&mut session, |session| text(session).contains("READY"));
        assert_eq!(session.snapshot().bracketed_paste(), bracketed);
        session.text("hello", true).unwrap();
        until(&mut session, |session| {
            session.exit_status().is_some() && session.pty_eof()
        });
        let expected_hex: String = expected.iter().map(|byte| format!("{byte:02x}")).collect();
        assert!(text(&session).contains(&expected_hex), "{}", text(&session));
        assert!(session.exit_status().unwrap().success());
    }
}
