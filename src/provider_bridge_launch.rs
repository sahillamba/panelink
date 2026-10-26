//! Private, opt-in OpenCode TUI plugin provisioning for one server generation.
//!
//! This module resolves and pins the native provider executable before making
//! filesystem changes, then owns an isolated configuration directory and
//! Unix listener for the lifetime of the launch coordinator.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use crate::control_protocol::TargetRef;
use crate::launch::{resolve_launch, Environment, LaunchContext};
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const PINNED_PROVIDER_SHA256: &str =
    "e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf";
const MAX_COMMAND_OUTPUT: usize = 256;
const MAX_PRIVATE_PATH: usize = 4_096;
const MAX_UNIX_SOCKET_PATH: usize = 103;
const TUI_PLUGIN: &str = include_str!("../assets/opencode-bridge/tui.ts");
const RECOVERY_PLUGIN: &str = include_str!("../assets/opencode-bridge/recovery.ts");

/// Private launch-scoped environment, listener and project identity.
pub struct BridgeLaunch {
    generation: u64,
    server_dir: PathBuf,
    launch_dir: PathBuf,
    socket_path: PathBuf,
    socket_identity: (u64, u64),
    listener: Option<UnixListener>,
    project: Vec<u8>,
    environment: Environment,
}

impl std::fmt::Debug for BridgeLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BridgeLaunch { <redacted> }")
    }
}

impl BridgeLaunch {
    /// Resolve, verify and provision one private launch generation.
    pub fn prepare(
        server_dir: &Path,
        generation: u64,
        context: &LaunchContext,
        executable: &OsStr,
    ) -> io::Result<Self> {
        Self::prepare_inner(server_dir, generation, context, executable, verify_provider)
    }

    #[cfg(test)]
    fn prepare_with_verifier<F>(
        server_dir: &Path,
        generation: u64,
        context: &LaunchContext,
        executable: &OsStr,
        verify: F,
    ) -> io::Result<Self>
    where
        F: FnOnce(&Path) -> io::Result<()>,
    {
        Self::prepare_inner(server_dir, generation, context, executable, verify)
    }

    fn prepare_inner<F>(
        server_dir: &Path,
        generation: u64,
        context: &LaunchContext,
        executable: &OsStr,
        verify: F,
    ) -> io::Result<Self>
    where
        F: FnOnce(&Path) -> io::Result<()>,
    {
        if generation == 0 || !context.working_directory.is_absolute() {
            return Err(invalid_input());
        }
        validate_server_dir(server_dir)?;
        let resolved = resolve_launch(
            executable,
            &context.working_directory,
            context,
            &context.environment,
        )
        .map_err(|_| invalid_input())?;
        verify(&resolved.executable_path)?;
        let project = resolved
            .working_directory
            .as_os_str()
            .as_encoded_bytes()
            .to_vec();
        if project.is_empty() || project.len() > MAX_PRIVATE_PATH || project.contains(&0) {
            return Err(invalid_input());
        }

        let launch_dir = server_dir.join(format!("provider-bridge-{generation}"));
        create_private_dir(&launch_dir)?;
        let config_dir = launch_dir.join("opencode-config");
        let plugin_dir = launch_dir.join("identity-plugin");
        create_private_dir(&config_dir)?;
        create_private_dir(&plugin_dir)?;
        write_private_file(&plugin_dir.join("tui.ts"), TUI_PLUGIN.as_bytes())?;
        write_private_file(&plugin_dir.join("recovery.ts"), RECOVERY_PLUGIN.as_bytes())?;
        let plugin_dir_text = plugin_dir.to_str().ok_or_else(invalid_input)?;
        let config = format!("{{\"plugins\":[{}]}}\n", json_string(plugin_dir_text));
        write_private_file(&config_dir.join("cli.json"), config.as_bytes())?;

        let socket_path = launch_dir.join("bridge.sock");
        if socket_path.as_os_str().as_encoded_bytes().len() > MAX_UNIX_SOCKET_PATH {
            return Err(invalid_input());
        }
        let listener = {
            let _guard = crate::spawn_gate::acquire()?;
            UnixListener::bind(&socket_path)?
        };
        let socket_metadata = fs::symlink_metadata(&socket_path)?;
        let socket_identity = (socket_metadata.dev(), socket_metadata.ino());
        let mut owner = Self {
            generation,
            server_dir: server_dir.to_path_buf(),
            launch_dir,
            socket_path: socket_path.clone(),
            socket_identity,
            listener: Some(listener),
            project,
            environment: Environment::new(),
        };
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
        validate_owned_socket(&socket_path, socket_identity)?;
        owner.environment.insert(
            OsString::from("OPENCODE_CONFIG_DIR"),
            config_dir.into_os_string(),
        );
        owner.environment.insert(
            OsString::from("PANELINK_BRIDGE_SOCKET"),
            socket_path.into_os_string(),
        );
        Ok(owner)
    }

    /// Only the two per-child bridge activation assignments.
    pub const fn environment(&self) -> &Environment {
        &self.environment
    }

    /// Canonical project directory bytes used to scope provider observations.
    pub fn project(&self) -> &[u8] {
        &self.project
    }

    /// Consume the already-bound listener for the provider bridge runtime.
    pub fn take_listener(&mut self) -> io::Result<UnixListener> {
        self.listener.take().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "provider bridge listener was taken",
            )
        })
    }

    /// Create a fresh private store directory for this exact managed target.
    pub fn binding_path(&self, target: TargetRef) -> io::Result<PathBuf> {
        validate_server_dir(&self.server_dir)?;
        if target.generation != self.generation
            || [
                target.generation,
                target.workspace,
                target.pane,
                target.session,
                target.occupant_revision,
            ]
            .contains(&0)
        {
            return Err(invalid_input());
        }
        let root = self.server_dir.join("provider-bindings");
        create_or_validate_private_dir(&root)?;
        let path = Self::binding_directory(&self.server_dir, target);
        create_private_dir(&path)?;
        Ok(path)
    }

    /// Deterministic exact-target store path for historical inspection.
    ///
    /// This mapping performs no filesystem access or recency scan. Callers
    /// opening the result must still validate its private-directory identity.
    pub fn binding_directory(server_dir: &Path, target: TargetRef) -> PathBuf {
        server_dir.join("provider-bindings").join(format!(
            "{}-{}-{}-{}-{}",
            target.generation,
            target.workspace,
            target.pane,
            target.session,
            target.occupant_revision
        ))
    }

    /// Launch-scoped directory, useful for diagnostics without exposing secrets.
    pub fn launch_directory(&self) -> &Path {
        &self.launch_dir
    }
}

impl Drop for BridgeLaunch {
    fn drop(&mut self) {
        drop(self.listener.take());
        if let Ok(metadata) = fs::symlink_metadata(&self.socket_path) {
            if metadata.file_type().is_socket()
                && metadata.uid() == rustix::process::geteuid().as_raw()
                && (metadata.dev(), metadata.ino()) == self.socket_identity
            {
                let _ = fs::remove_file(&self.socket_path);
            }
        }
    }
}

fn verify_provider(path: &Path) -> io::Result<()> {
    verify_provider_with(path, PINNED_PROVIDER_SHA256)
}

fn verify_provider_with(path: &Path, expected: &str) -> io::Result<()> {
    let mut child = Command::new("/usr/bin/shasum")
        .env_clear()
        .arg("-a")
        .arg("256")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdout = Vec::new();
    let read_result = child
        .stdout
        .take()
        .ok_or_else(invalid_data)?
        .take((MAX_COMMAND_OUTPUT + 1) as u64)
        .read_to_end(&mut stdout);
    if read_result.is_err() || stdout.len() > MAX_COMMAND_OUTPUT {
        let _ = child.kill();
        let _ = child.wait();
        return Err(invalid_data());
    }
    let status = child.wait()?;
    if !status.success() {
        return Err(invalid_data());
    }
    let digest = stdout
        .split(|byte| byte.is_ascii_whitespace())
        .next()
        .ok_or_else(invalid_data)?;
    if digest.len() != 64
        || !digest.iter().all(u8::is_ascii_hexdigit)
        || !digest.eq_ignore_ascii_case(expected.as_bytes())
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "provider executable is not approved",
        ));
    }
    Ok(())
}

fn validate_server_dir(path: &Path) -> io::Result<()> {
    if !path.is_absolute() || fs::canonicalize(path)? != path {
        return Err(unsafe_path());
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
    {
        return Err(unsafe_path());
    }
    Ok(())
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    DirBuilder::new().mode(0o700).create(path)?;
    validate_private_dir(path)
}

fn create_or_validate_private_dir(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => validate_private_dir(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => create_private_dir(path),
        Err(error) => Err(error),
    }
}

fn validate_private_dir(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o700
        || fs::canonicalize(path)? != path
    {
        return Err(unsafe_path());
    }
    Ok(())
}

fn write_private_file(path: &Path, content: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(unsafe_path());
    }
    use std::io::Write;
    file.write_all(content)?;
    file.sync_all()
}

fn validate_owned_socket(path: &Path, identity: (u64, u64)) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o600
        || (metadata.dev(), metadata.ino()) != identity
    {
        return Err(unsafe_path());
    }
    Ok(())
}

fn json_string(value: &str) -> String {
    let mut result = String::with_capacity(value.len() + 2);
    result.push('"');
    for ch in value.chars() {
        match ch {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            ch if ch <= '\u{1f}' => result.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => result.push(ch),
        }
    }
    result.push('"');
    result
}

fn invalid_input() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "provider bridge launch is invalid",
    )
}

fn invalid_data() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "provider executable verification failed",
    )
}

fn unsafe_path() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "provider bridge path is unsafe",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::DirBuilderExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = PathBuf::from(format!(
                "panelink-bridge-launch-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let path = Path::new("/private/tmp").join(path);
            DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn context(cwd: &Path) -> LaunchContext {
        LaunchContext {
            working_directory: cwd.to_path_buf(),
            environment: Environment::from([
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (
                    OsString::from("PANELINK_BRIDGE_CANARY"),
                    OsString::from("never-forward"),
                ),
            ]),
        }
    }

    fn target(generation: u64) -> TargetRef {
        TargetRef {
            generation,
            workspace: 2,
            pane: 3,
            session: 4,
            occupant_revision: 5,
        }
    }

    fn prepare_fixture(fixture: &Fixture, generation: u64) -> BridgeLaunch {
        let cwd = fs::canonicalize(".").unwrap();
        BridgeLaunch::prepare_with_verifier(
            &fixture.0,
            generation,
            &context(&cwd),
            OsStr::new("/bin/sh"),
            |_| Ok(()),
        )
        .unwrap()
    }

    #[test]
    fn creates_private_config_plugin_socket_and_target_binding_directory() {
        let fixture = Fixture::new();
        let before_socket = std::env::var_os("PANELINK_BRIDGE_SOCKET");
        let before_config = std::env::var_os("OPENCODE_CONFIG_DIR");
        let bridge = prepare_fixture(&fixture, 71);
        let config_dir = bridge
            .environment()
            .get(OsStr::new("OPENCODE_CONFIG_DIR"))
            .unwrap();
        let socket_path = bridge
            .environment()
            .get(OsStr::new("PANELINK_BRIDGE_SOCKET"))
            .unwrap();
        assert_eq!(bridge.environment().len(), 2);
        assert_eq!(std::env::var_os("PANELINK_BRIDGE_SOCKET"), before_socket);
        assert_eq!(std::env::var_os("OPENCODE_CONFIG_DIR"), before_config);
        assert!(!format!("{bridge:?}").contains("never-forward"));
        assert_eq!(
            bridge.project(),
            fs::canonicalize(".")
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
        );
        assert_eq!(
            fs::read(Path::new(config_dir).join("cli.json")).unwrap(),
            format!(
                "{{\"plugins\":[{}]}}\n",
                json_string(
                    bridge
                        .launch_directory()
                        .join("identity-plugin")
                        .to_str()
                        .unwrap()
                )
            )
            .as_bytes()
        );
        assert_eq!(
            fs::read(bridge.launch_directory().join("identity-plugin/tui.ts")).unwrap(),
            TUI_PLUGIN.as_bytes()
        );
        assert_eq!(
            fs::read(
                bridge
                    .launch_directory()
                    .join("identity-plugin/recovery.ts")
            )
            .unwrap(),
            RECOVERY_PLUGIN.as_bytes()
        );
        for dir in [
            bridge.launch_directory(),
            Path::new(config_dir),
            &bridge.launch_directory().join("identity-plugin"),
        ] {
            assert_eq!(fs::symlink_metadata(dir).unwrap().mode() & 0o7777, 0o700);
        }
        for asset in ["tui.ts", "recovery.ts"] {
            let metadata = fs::symlink_metadata(
                bridge
                    .launch_directory()
                    .join("identity-plugin")
                    .join(asset),
            )
            .unwrap();
            assert!(metadata.is_file());
            assert_eq!(metadata.mode() & 0o7777, 0o600);
            assert_eq!(metadata.nlink(), 1);
        }
        let socket = Path::new(socket_path);
        assert!(fs::symlink_metadata(socket)
            .unwrap()
            .file_type()
            .is_socket());
        assert_eq!(fs::symlink_metadata(socket).unwrap().mode() & 0o7777, 0o600);
        let path = bridge.binding_path(target(71)).unwrap();
        assert_eq!(path.file_name().unwrap(), "71-2-3-4-5");
        assert_eq!(
            BridgeLaunch::binding_directory(&fixture.0, target(71)),
            path
        );
        assert_eq!(fs::symlink_metadata(&path).unwrap().mode() & 0o7777, 0o700);
        assert_eq!(
            bridge.binding_path(target(72)).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn unsupported_executable_is_rejected_before_generation_effects() {
        let fixture = Fixture::new();
        let cwd = fs::canonicalize(".").unwrap();
        let error = BridgeLaunch::prepare(&fixture.0, 72, &context(&cwd), OsStr::new("/bin/sh"))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(!fixture.0.join("provider-bridge-72").exists());
    }

    #[test]
    fn preserves_existing_generation_and_socket_replacements() {
        let fixture = Fixture::new();
        let existing = fixture.0.join("provider-bridge-73");
        create_private_dir(&existing).unwrap();
        fs::write(existing.join("preserve"), b"stale").unwrap();
        let cwd = fs::canonicalize(".").unwrap();
        let error = BridgeLaunch::prepare_with_verifier(
            &fixture.0,
            73,
            &context(&cwd),
            OsStr::new("/bin/sh"),
            |_| Ok(()),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(existing.join("preserve")).unwrap(), b"stale");

        let mut bridge = prepare_fixture(&fixture, 74);
        let socket = bridge.socket_path.clone();
        drop(bridge.take_listener().unwrap());
        fs::remove_file(&socket).unwrap();
        fs::write(&socket, b"replacement").unwrap();
        drop(bridge);
        assert_eq!(fs::read(socket).unwrap(), b"replacement");
    }
}
