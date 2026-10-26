#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

//! Real CLI routing acceptance for independent workspace server namespaces.

use std::fs::{self, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT: AtomicUsize = AtomicUsize::new(0);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_TIMEOUT: Duration = Duration::from_secs(9);

struct Fixture {
    root: PathBuf,
    owned_routes: Vec<(u64, u64)>,
}

impl Fixture {
    fn new() -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = Path::new("/private/tmp").join(format!("pwr-{}-{n}", std::process::id()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .expect("create private routing fixture");
        Self {
            root,
            owned_routes: Vec::new(),
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_panelink"))
            .args(args)
            .env_clear()
            .env("PANELINK_HOME", &self.root)
            .env("HOME", &self.root)
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", "/bin/sh")
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .expect("start panelink fixture command");
        let deadline = Instant::now() + COMMAND_TIMEOUT;
        loop {
            if child.try_wait().expect("poll panelink command").is_some() {
                return child.wait_with_output().expect("collect panelink output");
            }
            if Instant::now() >= deadline {
                // SAFETY: this process group belongs only to the test command.
                unsafe { libc::kill(-i32::try_from(child.id()).unwrap(), libc::SIGKILL) };
                let output = child.wait_with_output().expect("reap timed-out command");
                panic!("panelink command exceeded {COMMAND_TIMEOUT:?}: {args:?}; {output:?}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn create_project(&self) {
        let output = self.run(&["workspace", "project", "create", "Editors", "Main"]);
        assert!(output.status.success(), "create project: {output:?}");
    }

    fn create_tab(&self, name: &str) {
        let output = self.run(&["workspace", "tab", "create", "2", name]);
        assert!(output.status.success(), "create tab: {output:?}");
    }

    fn route_root(&self, tab: u64) -> PathBuf {
        self.root
            .join("projects/2/workspaces")
            .join(tab.to_string())
    }

    fn start_route(&mut self, tab: u64) -> Output {
        // Register before invoking open so Drop also owns cleanup after a
        // panic during startup or attach assertions.
        if !self.owned_routes.contains(&(2, tab)) {
            self.owned_routes.push((2, tab));
        }
        let id = tab.to_string();
        self.run(&["workspace", "open", "2", &id])
    }

    fn stop_owned_route(&self, project: u64, tab: u64) {
        let route_root = self.route_root(tab);
        let server = route_root.join("server");
        let project = project.to_string();
        let tab = tab.to_string();
        if let Ok(mut child) = Command::new(env!("CARGO_BIN_EXE_panelink"))
            .args(["workspace", "stop", project.as_str(), tab.as_str()])
            .env_clear()
            .env("PANELINK_HOME", &self.root)
            .env("HOME", &self.root)
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", "/bin/sh")
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
        {
            let deadline = Instant::now() + STOP_TIMEOUT;
            loop {
                if child.try_wait().ok().flatten().is_some() {
                    break;
                }
                if Instant::now() >= deadline {
                    // The CLI stop path has its own five second bound. Kill
                    // only that fixture cleanup command if it exceeds this cap.
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
        if !server_lock_held(&server) {
            return;
        }

        // If the normal stop command could not complete, signal only the
        // exact process identity recorded by this fixture route.
        if let Some(pid) = verified_server_pid(&route_root) {
            // SAFETY: the PID and kernel start time match the route's record.
            unsafe { libc::kill(pid, libc::SIGTERM) };
            if wait_server_exit(&server, Duration::from_secs(5)) {
                return;
            }
            if verified_server_pid(&route_root) == Some(pid) {
                // SAFETY: rechecked the same route-owned process identity.
                unsafe { libc::kill(pid, libc::SIGKILL) };
                let _ = wait_server_exit(&server, Duration::from_secs(2));
            }
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for &(project, tab) in self.owned_routes.iter().rev() {
            self.stop_owned_route(project, tab);
        }
        if self
            .owned_routes
            .iter()
            .all(|&(_, tab)| !server_lock_held(&self.route_root(tab).join("server")))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn server_lock_held(server: &Path) -> bool {
    use std::os::fd::AsRawFd;

    let lock = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(server.join("server.lock"))
    {
        Ok(lock) => lock,
        Err(_) => return false,
    };
    // SAFETY: this test owns a descriptor for the route's lock file.
    let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        // SAFETY: release the lock acquired above.
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) };
    } else {
        return true;
    }
    false
}

fn wait_server_exit(server: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while server_lock_held(server) {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    true
}

fn verified_server_pid(route_root: &Path) -> Option<i32> {
    let record = fs::read_to_string(route_root.join("server.pid")).ok()?;
    let mut fields = record.split_whitespace();
    let pid = fields.next()?.parse::<i32>().ok()?;
    let start = fields.next()?.parse::<u64>().ok()?;
    if fields.next().is_some() || pid <= 0 {
        return None;
    }
    // SAFETY: proc_pidinfo writes the documented BSD process info record.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: the output buffer and size describe `info` exactly.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            std::ptr::addr_of_mut!(info).cast(),
            i32::try_from(size).ok()?,
        )
    };
    (usize::try_from(written).ok()? == size
        && info.pbi_uid == rustix::process::getuid().as_raw()
        && info.pbi_status != libc::SZOMB
        && info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec == start)
        .then_some(pid)
}

fn assert_server_lock_held(server: &Path, expected: bool) {
    assert_eq!(
        server_lock_held(server),
        expected,
        "unexpected server lock state at {}",
        server.display()
    );
}

fn create_private_route_dirs(route_root: &Path) {
    let projects = route_root
        .ancestors()
        .nth(3)
        .expect("projects directory")
        .to_path_buf();
    let project = projects.join("2");
    let workspaces = project.join("workspaces");
    let server = route_root.join("server");
    for path in [
        projects,
        project,
        workspaces,
        route_root.to_path_buf(),
        server,
    ] {
        fs::create_dir_all(&path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn list_without_registry_creates_no_files() {
    let f = Fixture::new();
    assert_eq!(fs::read_dir(&f.root).unwrap().count(), 0);
    let output = f.run(&["workspace", "list"]);
    assert!(output.status.success(), "list: {output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("legacy Default/Main available"));
    assert_eq!(fs::read_dir(&f.root).unwrap().count(), 0);
}

#[test]
fn two_nondefault_routes_open_exact_servers_and_stop_independently() {
    let mut f = Fixture::new();
    f.create_project();
    f.create_tab("Review");
    assert!(f.run(&["workspace", "select", "2", "4"]).status.success());

    let first = f.start_route(3);
    assert_eq!(first.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&first.stderr).contains("attach needs an interactive terminal"));
    let first_root = f.route_root(3);
    let first_server = first_root.join("server");
    let first_generation = fs::read(first_server.join("generation")).expect("first generation");
    assert!(first_root.join("contexts").is_dir());
    assert!(first_root.join("server.pid").is_file());
    assert!(first_root.join("server.log").is_file());
    assert!(f.root.join("terminfo").is_dir());
    assert_server_lock_held(&first_server, true);

    let first_context = first_root.join("contexts/route-marker");
    fs::write(&first_context, b"route-three").unwrap();
    let second = f.start_route(4);
    assert_eq!(second.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("attach needs an interactive terminal")
    );
    let second_root = f.route_root(4);
    let second_server = second_root.join("server");
    let second_generation = fs::read(second_server.join("generation")).expect("second generation");
    assert!(second_root.join("contexts").is_dir());
    assert!(second_root.join("server.pid").is_file());
    assert!(second_root.join("server.log").is_file());
    assert_ne!(first_root.join("contexts"), second_root.join("contexts"));
    assert_ne!(
        first_server.join("server.lock"),
        second_server.join("server.lock")
    );
    assert_ne!(
        first_root.join("lifecycle.lock"),
        second_root.join("lifecycle.lock")
    );
    assert_ne!(
        first_generation, second_generation,
        "server generations must be isolated"
    );
    assert_eq!(fs::read(&first_context).unwrap(), b"route-three");
    assert_server_lock_held(&first_server, true);
    assert_server_lock_held(&second_server, true);
    assert!(
        !f.root.join("server/server.lock").exists(),
        "no fallback to Default/Main"
    );

    // Legacy no-selector commands keep their default route and cannot stop
    // either selected nondefault server.
    assert_ne!(f.run(&["panes"]).status.code(), Some(0));
    assert!(f.run(&["stop"]).status.success());
    assert_server_lock_held(&first_server, true);
    assert_server_lock_held(&second_server, true);
    assert_ne!(f.run(&["stop", "extra"]).status.code(), Some(0));

    let stop_first = f.run(&["workspace", "stop", "2", "3"]);
    assert!(
        stop_first.status.success(),
        "stop exact first route: {stop_first:?}"
    );
    assert_server_lock_held(&first_server, false);
    assert_server_lock_held(&second_server, true);

    let stop_second = f.run(&["workspace", "stop", "2", "4"]);
    assert!(
        stop_second.status.success(),
        "stop exact second route: {stop_second:?}"
    );
    assert_server_lock_held(&second_server, false);
    assert!(!f.root.join("server/server.lock").exists());
    let listed = String::from_utf8(f.run(&["workspace", "list"]).stdout).unwrap();
    assert!(listed.contains("Tab 4 Review *"));
    assert!(listed.contains("Tab 3 Main"));
}

#[test]
fn removal_requires_stopped_unambiguous_route_and_allows_clean_stopped_route() {
    use std::os::fd::AsRawFd;

    let mut f = Fixture::new();
    f.create_project();
    for name in ["HeldLock", "UncertainPid", "Socket", "Removable"] {
        f.create_tab(name);
    }

    // A fixture-owned live route cannot be removed, including immediately
    // after the deliberately rejected non-TTY attach.
    let live = f.start_route(3);
    assert_eq!(live.status.code(), Some(1));
    assert_ne!(
        f.run(&["workspace", "remove", "2", "3"]).status.code(),
        Some(0)
    );
    assert_server_lock_held(&f.route_root(3).join("server"), true);

    // A separate held server lock is enough to refuse metadata removal.
    let held_root = f.route_root(4);
    create_private_route_dirs(&held_root);
    let held_server = held_root.join("server");
    let lock = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(held_server.join("server.lock"))
        .unwrap();
    // SAFETY: lock descriptor refers to this test's isolated lock file.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    assert_ne!(
        f.run(&["workspace", "remove", "2", "4"]).status.code(),
        Some(0)
    );
    drop(lock);

    // A leftover PID identity is ambiguous even when no process can be
    // established, and a socket endpoint is never silently removed.
    let pid_root = f.route_root(5);
    create_private_route_dirs(&pid_root);
    fs::write(pid_root.join("server.pid"), b"2147483647 1\n").unwrap();
    assert_ne!(
        f.run(&["workspace", "remove", "2", "5"]).status.code(),
        Some(0)
    );

    let socket_root = f.route_root(6);
    create_private_route_dirs(&socket_root);
    let _listener = UnixListener::bind(socket_root.join("server/control.sock")).unwrap();
    assert_ne!(
        f.run(&["workspace", "remove", "2", "6"]).status.code(),
        Some(0)
    );

    let listed = String::from_utf8(f.run(&["workspace", "list"]).stdout).unwrap();
    for tab in [3, 4, 5, 6] {
        assert!(
            listed.contains(&format!("Tab {tab} ")),
            "refused tab {tab} disappeared"
        );
    }

    // This tab never had a server, PID, or socket and can be removed.
    let removed = f.run(&["workspace", "remove", "2", "7"]);
    assert!(
        removed.status.success(),
        "remove stopped route: {removed:?}"
    );
    let listed = String::from_utf8(f.run(&["workspace", "list"]).stdout).unwrap();
    assert!(!listed.contains("Tab 7 Removable"));
}

#[test]
fn removing_an_unopened_route_creates_its_serialization_lock_before_unregistering() {
    let f = Fixture::new();
    f.create_project();
    let route = f.route_root(3);
    assert!(!route.exists());
    let removed = f.run(&["workspace", "remove", "2", "3"]);
    assert!(
        removed.status.success(),
        "remove unopened route: {removed:?}"
    );
    assert!(route.join("lifecycle.lock").is_file());
    assert!(!route.join("server/generation").exists());
    assert!(!f.run(&["workspace", "open", "2", "3"]).status.success());
    assert!(!route.join("server/generation").exists());
}

#[test]
fn inactive_legacy_default_remains_registered_for_legacy_commands() {
    let f = Fixture::new();
    f.create_project();
    assert!(f.run(&["workspace", "select", "2", "3"]).status.success());
    let registry = fs::read(f.root.join("workspace-index.v1")).unwrap();
    let removed = f.run(&["workspace", "remove", "1", "1"]);
    assert!(!removed.status.success());
    assert!(String::from_utf8_lossy(&removed.stderr).contains("cannot be removed"));
    assert_eq!(
        fs::read(f.root.join("workspace-index.v1")).unwrap(),
        registry
    );
    assert!(!f.root.join("server").exists());
    let listed = String::from_utf8(f.run(&["workspace", "list"]).stdout).unwrap();
    assert!(listed.contains("Project 1 Default"));
    assert!(listed.contains("Tab 1 Main"));
}

#[test]
fn corrupt_or_unsafe_route_refuses_open_without_default_fallback() {
    let f = Fixture::new();
    f.create_project();
    fs::write(f.root.join("workspace-index.v1"), b"corrupt registry").unwrap();
    let corrupt = f.run(&["workspace", "open"]);
    assert_ne!(corrupt.status.code(), Some(0));
    assert!(!f.root.join("server").exists());

    // Repair only the fixture registry by making a fresh fixture for the
    // namespace symlink case; the production CLI remains the only writer.
    drop(f);
    let f = Fixture::new();
    f.create_project();
    let outside = f.root.join("outside");
    fs::DirBuilder::new().mode(0o700).create(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, f.root.join("projects")).unwrap();
    let unsafe_route = f.run(&["workspace", "open", "2", "3"]);
    assert_ne!(unsafe_route.status.code(), Some(0));
    assert!(
        !f.root.join("server").exists(),
        "unsafe route fell back to Default/Main"
    );
    assert!(!outside.join("2/workspaces/3/server").exists());
    let _ = fs::remove_dir_all(outside);
}
