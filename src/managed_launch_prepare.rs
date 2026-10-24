//! Bounded, explicit preparation of a validated managed-launch request.
//!
//! This adapter captures request bytes without UTF-8 conversion, builds only
//! from the client context and the prototype-owned environment, resolves the
//! executable through the shared launch policy, and consumes itself to spawn.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::control_protocol::TargetRef;
use crate::launch::{
    build_environment, resolve_launch, Environment, LaunchContext, ResolvedLaunch,
};
use crate::managed_launch_protocol::LaunchRequestView;
use crate::terminal::{TerminalSession, TerminalSize};

const MAX_PATH_BYTES: usize = 4096;

/// A launch-preparation failure with a fixed, redacted public description.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrepareError {
    /// The selected target contained a zero identity field.
    InvalidTarget,
    /// The selected target belongs to another server generation.
    GenerationMismatch,
    /// The explicit terminfo directory was invalid or unavailable.
    InvalidTerminfoDirectory,
    /// A canonical path exceeded the supported byte limit.
    PathTooLong,
    /// The captured client context could not be used for launch.
    InvalidClientContext,
    /// The request environment violated the launch environment policy.
    InvalidEnvironment,
    /// The executable or requested working directory could not be resolved.
    ResolutionFailed,
    /// The requested terminal dimensions were invalid.
    InvalidDimensions,
    /// The existing terminal spawn operation failed.
    SpawnFailed,
}

impl fmt::Display for PrepareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidTarget => "managed launch target is invalid",
            Self::GenerationMismatch => "managed launch target generation is invalid",
            Self::InvalidTerminfoDirectory => "managed launch terminfo directory is invalid",
            Self::PathTooLong => "managed launch path exceeds the supported limit",
            Self::InvalidClientContext => "managed launch client context is invalid",
            Self::InvalidEnvironment => "managed launch environment is invalid",
            Self::ResolutionFailed => "managed launch target could not be resolved",
            Self::InvalidDimensions => "managed launch dimensions are invalid",
            Self::SpawnFailed => "managed launch could not start",
        };
        f.write_str(message)
    }
}

impl std::error::Error for PrepareError {}

/// An owned launch descriptor that can only be used once to create a session.
pub struct PreparedLaunch {
    launch: ResolvedLaunch,
    arguments: Vec<OsString>,
    environment: Environment,
    size: TerminalSize,
}

impl PreparedLaunch {
    /// The canonical executable and working directory chosen by the resolver.
    pub const fn resolved_launch(&self) -> &ResolvedLaunch {
        &self.launch
    }

    /// Losslessly captured argument bytes represented as platform strings.
    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    /// The bounded, explicit child environment.
    pub const fn environment(&self) -> &Environment {
        &self.environment
    }

    /// Add only the two launch-scoped provider bridge activation values after
    /// the bounded schema-6 request has passed its normal validation path.
    pub(crate) fn with_provider_bridge_env(
        mut self,
        bridge_environment: &Environment,
    ) -> Result<Self, PrepareError> {
        let config_name = OsStr::new("OPENCODE_CONFIG_DIR");
        let socket_name = OsStr::new("PANELINK_BRIDGE_SOCKET");
        if bridge_environment.len() != 2
            || !bridge_environment.contains_key(config_name)
            || !bridge_environment.contains_key(socket_name)
        {
            return Err(PrepareError::InvalidEnvironment);
        }
        for name in [config_name, socket_name] {
            let value = bridge_environment
                .get(name)
                .ok_or(PrepareError::InvalidEnvironment)?;
            if value.is_empty()
                || value.as_os_str().as_bytes().contains(&0)
                || value.as_os_str().as_bytes().len() > MAX_PATH_BYTES
                || !Path::new(value).is_absolute()
            {
                return Err(PrepareError::InvalidEnvironment);
            }
            self.environment.insert(name.to_owned(), value.clone());
        }
        Ok(self)
    }

    /// Validated terminal dimensions.
    pub const fn size(&self) -> &TerminalSize {
        &self.size
    }

    /// Select the checked content size before the initial workspace child is
    /// spawned. Its startup request describes the outer layout rectangle.
    pub(crate) fn with_initial_size(mut self, size: TerminalSize) -> Self {
        self.size = size;
        self
    }

    /// Consume this descriptor and pass it to the reviewed PTY spawn path.
    pub fn spawn(self) -> Result<TerminalSession, PrepareError> {
        TerminalSession::spawn(&self.launch, &self.arguments, &self.environment, self.size)
            .map_err(|_| PrepareError::SpawnFailed)
    }
}

impl fmt::Debug for PreparedLaunch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PreparedLaunch(<redacted>)")
    }
}

/// Prepare a request using only its captured client context and explicit
/// server-owned values. Callers must authorize the target and persist the
/// launch Intent before consuming the result with [`PreparedLaunch::spawn`].
pub fn prepare_launch(
    request: LaunchRequestView<'_>,
    target: TargetRef,
    terminfo_directory: &Path,
) -> Result<PreparedLaunch, PrepareError> {
    if [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
    ]
    .contains(&0)
    {
        return Err(PrepareError::InvalidTarget);
    }
    if request.key().generation != target.generation {
        return Err(PrepareError::GenerationMismatch);
    }

    let terminfo_directory = canonical_terminfo_directory(terminfo_directory)?;
    let requested_executable = os_string(request.requested_executable());
    let requested_working_directory = os_string(request.requested_working_directory());
    let client_working_directory = os_string(request.client_working_directory());
    if !Path::new(&client_working_directory).is_absolute() {
        return Err(PrepareError::InvalidClientContext);
    }

    let mut client_environment = Environment::new();
    for entry in request.environment() {
        client_environment.insert(os_string(entry.name), os_string(entry.value));
    }
    let context = LaunchContext {
        working_directory: PathBuf::from(client_working_directory),
        environment: client_environment,
    };

    let owned = owned_environment(target, &terminfo_directory);
    let environment = build_environment(&Environment::new(), &context, &[], &owned)
        .map_err(|_| PrepareError::InvalidEnvironment)?;
    let launch = resolve_launch(
        &requested_executable,
        Path::new(&requested_working_directory),
        &context,
        &environment,
    )
    .map_err(|_| PrepareError::ResolutionFailed)?;
    ensure_path_bound(&launch.executable_path)?;
    ensure_path_bound(&launch.working_directory)?;

    let arguments = request.arguments().map(os_string).collect::<Vec<_>>();
    let (rows, columns) = request.dimensions();
    let size = TerminalSize::new(rows, columns).map_err(|_| PrepareError::InvalidDimensions)?;

    Ok(PreparedLaunch {
        launch,
        arguments,
        environment,
        size,
    })
}

fn canonical_terminfo_directory(path: &Path) -> Result<PathBuf, PrepareError> {
    ensure_path_bound(path)?;
    if !path.is_absolute() {
        return Err(PrepareError::InvalidTerminfoDirectory);
    }
    let canonical = fs::canonicalize(path).map_err(|_| PrepareError::InvalidTerminfoDirectory)?;
    ensure_path_bound(&canonical)?;
    if !canonical.is_dir() {
        return Err(PrepareError::InvalidTerminfoDirectory);
    }
    Ok(canonical)
}

fn ensure_path_bound(path: &Path) -> Result<(), PrepareError> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.contains(&0) {
        return Err(PrepareError::InvalidTerminfoDirectory);
    }
    if bytes.len() > MAX_PATH_BYTES {
        return Err(PrepareError::PathTooLong);
    }
    Ok(())
}

fn owned_environment(target: TargetRef, terminfo_directory: &Path) -> Environment {
    let mut environment = BTreeMap::new();
    environment.insert(OsString::from("TERM"), OsString::from("panelink-probe"));
    environment.insert(
        OsString::from("TERMINFO"),
        terminfo_directory.as_os_str().to_owned(),
    );
    environment.insert(
        OsString::from("PANELINK_GENERATION"),
        OsString::from(target.generation.to_string()),
    );
    environment.insert(
        OsString::from("PANELINK_SESSION"),
        OsString::from(target.session.to_string()),
    );
    environment
}

#[cfg(unix)]
fn os_string(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes.to_vec())
}

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managed_launch_protocol::{
        decode_launch_request, encode_launch_request, EnvironmentEntry, LaunchRequest,
    };
    use crate::protocol::OperationKey;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::DirBuilderExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            for _ in 0..100 {
                let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "panelink-launch-prepare-{}-{sequence}",
                    std::process::id()
                ));
                match fs::DirBuilder::new().mode(0o700).create(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create private launch fixture: {error}"),
                }
            }
            panic!("could not allocate a private launch fixture")
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn target(generation: u64, session: u64) -> TargetRef {
        TargetRef {
            generation,
            workspace: 1,
            pane: 2,
            session,
            occupant_revision: 1,
        }
    }

    struct RequestInput<'a> {
        generation: u64,
        executable: &'a [u8],
        working_directory: &'a [u8],
        client_working_directory: &'a [u8],
        arguments: &'a [&'a [u8]],
        environment: &'a [EnvironmentEntry<'a>],
        rows: u16,
        columns: u16,
    }

    fn request_payload(input: RequestInput<'_>) -> Vec<u8> {
        let request = LaunchRequest {
            key: OperationKey {
                generation: input.generation,
                lease: 1,
                sequence: 1,
            },
            rows: input.rows,
            columns: input.columns,
            requested_executable: input.executable,
            requested_working_directory: input.working_directory,
            client_working_directory: input.client_working_directory,
            arguments: input.arguments,
            environment: input.environment,
        };
        let mut payload = Vec::new();
        encode_launch_request(&request, &mut payload).expect("encode launch request");
        payload
    }

    fn prepare(
        payload: &[u8],
        target: TargetRef,
        terminfo: &Path,
    ) -> Result<PreparedLaunch, PrepareError> {
        let request = decode_launch_request(payload).expect("decode launch request");
        prepare_launch(request, target, terminfo)
    }

    fn valid_environment<'a>(path: &'a [u8]) -> [EnvironmentEntry<'a>; 2] {
        [
            EnvironmentEntry {
                name: b"PATH",
                value: path,
            },
            EnvironmentEntry {
                name: b"CLIENT_ONLY",
                value: b"captured",
            },
        ]
    }

    #[test]
    fn captures_client_context_resolves_path_and_owns_server_values() {
        let fixture = Fixture::new();
        let cwd = fixture.path().join("client");
        fs::create_dir(&cwd).unwrap();
        let search = fixture.path().join("search");
        fs::create_dir(&search).unwrap();
        let executable = search.join("probe");
        fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let path = search.as_os_str().as_bytes();
        let environment = [
            EnvironmentEntry {
                name: b"PATH",
                value: path,
            },
            EnvironmentEntry {
                name: b"CLIENT_ONLY",
                value: b"captured",
            },
            EnvironmentEntry {
                name: b"TERM",
                value: b"caller-term",
            },
            EnvironmentEntry {
                name: b"TERMINFO",
                value: b"caller-terminfo",
            },
        ];
        let arguments: [&[u8]; 2] = [b"-c", b"exit 0"];
        let payload = request_payload(RequestInput {
            generation: 7,
            executable: b"probe",
            working_directory: b".",
            client_working_directory: cwd.as_os_str().as_bytes(),
            arguments: &arguments,
            environment: &environment,
            rows: 10,
            columns: 100,
        });
        let prepared = prepare(&payload, target(7, 44), fixture.path()).unwrap();

        assert_eq!(
            prepared.resolved_launch().executable_path,
            fs::canonicalize(executable).unwrap()
        );
        assert_eq!(
            prepared.resolved_launch().working_directory,
            fs::canonicalize(cwd).unwrap()
        );
        assert_eq!(
            prepared.arguments(),
            &[OsString::from("-c"), OsString::from("exit 0")]
        );
        assert_eq!(prepared.size().rows(), 10);
        assert_eq!(prepared.size().columns(), 100);
        assert_eq!(
            prepared.environment().get(OsStr::new("CLIENT_ONLY")),
            Some(&OsString::from("captured"))
        );
        assert_eq!(
            prepared.environment().get(OsStr::new("TERM")),
            Some(&OsString::from("panelink-probe"))
        );
        let canonical_terminfo = fs::canonicalize(fixture.path()).unwrap().into_os_string();
        assert_eq!(
            prepared.environment().get(OsStr::new("TERMINFO")),
            Some(&canonical_terminfo)
        );
        assert_eq!(
            prepared
                .environment()
                .get(OsStr::new("PANELINK_GENERATION")),
            Some(&OsString::from("7"))
        );
        assert_eq!(
            prepared.environment().get(OsStr::new("PANELINK_SESSION")),
            Some(&OsString::from("44"))
        );
        assert_eq!(prepared.environment().len(), 6);
        assert!(!prepared.environment().contains_key(OsStr::new("HOME")));
        assert!(!prepared.environment().contains_key(OsStr::new("USER")));
    }

    #[test]
    fn provider_bridge_environment_accepts_only_the_two_private_launch_paths() {
        let fixture = Fixture::new();
        let cwd = fixture.path().join("client");
        fs::create_dir(&cwd).unwrap();
        let environment = valid_environment(fixture.path().as_os_str().as_bytes());
        let payload = request_payload(RequestInput {
            generation: 17,
            executable: b"/bin/sh",
            working_directory: b".",
            client_working_directory: cwd.as_os_str().as_bytes(),
            arguments: &[],
            environment: &environment,
            rows: 10,
            columns: 100,
        });
        let bridge = Environment::from([
            (
                OsString::from("OPENCODE_CONFIG_DIR"),
                fixture.path().join("config").into_os_string(),
            ),
            (
                OsString::from("PANELINK_BRIDGE_SOCKET"),
                fixture.path().join("bridge.sock").into_os_string(),
            ),
        ]);
        let prepared = prepare(&payload, target(17, 44), fixture.path())
            .unwrap()
            .with_provider_bridge_env(&bridge)
            .unwrap();
        assert_eq!(
            prepared
                .environment()
                .get(OsStr::new("OPENCODE_CONFIG_DIR")),
            bridge.get(OsStr::new("OPENCODE_CONFIG_DIR"))
        );
        assert_eq!(
            prepared
                .environment()
                .get(OsStr::new("PANELINK_BRIDGE_SOCKET")),
            bridge.get(OsStr::new("PANELINK_BRIDGE_SOCKET"))
        );

        let mut extra = bridge.clone();
        extra.insert(OsString::from("EXTRA"), OsString::from("rejected"));
        assert_eq!(
            prepare(&payload, target(17, 44), fixture.path())
                .unwrap()
                .with_provider_bridge_env(&extra)
                .unwrap_err(),
            PrepareError::InvalidEnvironment
        );
        let mut relative = bridge;
        relative.insert(
            OsString::from("PANELINK_BRIDGE_SOCKET"),
            OsString::from("bridge.sock"),
        );
        assert_eq!(
            prepare(&payload, target(17, 44), fixture.path())
                .unwrap()
                .with_provider_bridge_env(&relative)
                .unwrap_err(),
            PrepareError::InvalidEnvironment
        );
    }

    #[test]
    fn preserves_non_utf8_argument_and_environment_bytes() {
        let fixture = Fixture::new();
        let cwd = fixture.path().as_os_str().as_bytes();
        let argument = b"arg-\xff";
        let value = b"value-\xfe";
        let environment = [
            EnvironmentEntry {
                name: b"PATH",
                value: b"/bin:/usr/bin",
            },
            EnvironmentEntry {
                name: b"ODD_VALUE",
                value,
            },
        ];
        let arguments = [argument.as_slice()];
        let payload = request_payload(RequestInput {
            generation: 8,
            executable: b"/bin/sh",
            working_directory: cwd,
            client_working_directory: cwd,
            arguments: &arguments,
            environment: &environment,
            rows: 10,
            columns: 100,
        });
        let prepared = prepare(&payload, target(8, 45), fixture.path()).unwrap();
        assert_eq!(prepared.arguments()[0].as_os_str().as_bytes(), argument);
        assert_eq!(
            prepared.environment()[OsStr::new("ODD_VALUE")]
                .as_os_str()
                .as_bytes(),
            value
        );
    }

    #[test]
    fn rejects_targets_and_generation_mismatch_before_path_work() {
        let fixture = Fixture::new();
        let cwd = fixture.path().as_os_str().as_bytes();
        let environment = valid_environment(b"/bin:/usr/bin");
        let arguments: [&[u8]; 0] = [];
        let payload = request_payload(RequestInput {
            generation: 8,
            executable: b"/bin/sh",
            working_directory: cwd,
            client_working_directory: cwd,
            arguments: &arguments,
            environment: &environment,
            rows: 10,
            columns: 100,
        });
        for invalid in [
            TargetRef {
                generation: 0,
                ..target(8, 45)
            },
            TargetRef {
                workspace: 0,
                ..target(8, 45)
            },
            TargetRef {
                pane: 0,
                ..target(8, 45)
            },
            TargetRef {
                session: 0,
                ..target(8, 45)
            },
            TargetRef {
                occupant_revision: 0,
                ..target(8, 45)
            },
        ] {
            assert_eq!(
                prepare(&payload, invalid, Path::new("relative")).unwrap_err(),
                PrepareError::InvalidTarget
            );
        }
        assert_eq!(
            prepare(&payload, target(9, 45), Path::new("relative")).unwrap_err(),
            PrepareError::GenerationMismatch
        );
    }

    #[test]
    fn rejects_invalid_terminfo_and_enforces_path_byte_boundaries() {
        let fixture = Fixture::new();
        let cwd = fixture.path().as_os_str().as_bytes();
        let environment = valid_environment(b"/bin:/usr/bin");
        let arguments: [&[u8]; 0] = [];
        let payload = request_payload(RequestInput {
            generation: 8,
            executable: b"/bin/sh",
            working_directory: cwd,
            client_working_directory: cwd,
            arguments: &arguments,
            environment: &environment,
            rows: 10,
            columns: 100,
        });
        assert_eq!(
            prepare(&payload, target(8, 45), Path::new("relative")).unwrap_err(),
            PrepareError::InvalidTerminfoDirectory
        );
        assert_eq!(
            prepare(&payload, target(8, 45), &fixture.path().join("missing")).unwrap_err(),
            PrepareError::InvalidTerminfoDirectory
        );
        let file = fixture.path().join("not-directory");
        fs::write(&file, b"x").unwrap();
        assert_eq!(
            prepare(&payload, target(8, 45), &file).unwrap_err(),
            PrepareError::InvalidTerminfoDirectory
        );

        let accepted = fixture.path().as_os_str().as_bytes();
        assert!(
            ensure_path_bound(Path::new(OsStr::from_bytes(&vec![b'a'; MAX_PATH_BYTES]))).is_ok()
        );
        assert_eq!(
            ensure_path_bound(Path::new(OsStr::from_bytes(&vec![
                b'a';
                MAX_PATH_BYTES + 1
            ]))),
            Err(PrepareError::PathTooLong)
        );
        assert!(!accepted.is_empty());
        let oversized = vec![b'a'; MAX_PATH_BYTES + 1];
        assert_eq!(
            prepare(
                &payload,
                target(8, 45),
                Path::new(OsStr::from_bytes(&oversized))
            )
            .unwrap_err(),
            PrepareError::PathTooLong
        );
        assert_eq!(
            prepare(
                &payload,
                target(8, 45),
                Path::new(OsStr::from_bytes(b"/invalid\0terminfo"))
            )
            .unwrap_err(),
            PrepareError::InvalidTerminfoDirectory
        );
    }

    #[test]
    fn fixed_errors_and_debug_redact_request_data() {
        let fixture = Fixture::new();
        let cwd = fixture.path().as_os_str().as_bytes();
        let environment = [
            EnvironmentEntry {
                name: b"PATH",
                value: b"/bin:/usr/bin",
            },
            EnvironmentEntry {
                name: b"TERM",
                value: b"caller-term",
            },
        ];
        let arguments: [&[u8]; 1] = [b"secret-argument"];
        let payload = request_payload(RequestInput {
            generation: 8,
            executable: b"/bin/sh",
            working_directory: cwd,
            client_working_directory: cwd,
            arguments: &arguments,
            environment: &environment,
            rows: 10,
            columns: 100,
        });
        let prepared = prepare(&payload, target(8, 45), fixture.path()).unwrap();
        assert_eq!(format!("{prepared:?}"), "PreparedLaunch(<redacted>)");
        assert_eq!(
            format!("{}", PrepareError::ResolutionFailed),
            "managed launch target could not be resolved"
        );
        assert!(!format!("{prepared:?}").contains("secret-argument"));
        assert_eq!(
            prepared.environment()[OsStr::new("TERM")],
            OsStr::new("panelink-probe")
        );
    }

    #[test]
    fn codec_rejects_caller_panelink_names_before_preparation() {
        let fixture = Fixture::new();
        let cwd = fixture.path().as_os_str().as_bytes();
        let environment = [EnvironmentEntry {
            name: b"PANELINK_SESSION",
            value: b"999",
        }];
        let request = LaunchRequest {
            key: OperationKey {
                generation: 8,
                lease: 1,
                sequence: 1,
            },
            rows: 10,
            columns: 100,
            requested_executable: b"/bin/sh",
            requested_working_directory: cwd,
            client_working_directory: cwd,
            arguments: &[],
            environment: &environment,
        };
        let mut payload = Vec::new();
        assert!(encode_launch_request(&request, &mut payload).is_err());
    }

    #[test]
    fn rejects_missing_path_and_empty_or_relative_path_components() {
        let fixture = Fixture::new();
        let cwd = fixture.path().as_os_str().as_bytes();
        for path in [b"".as_slice(), b"/bin:", b"relative:/bin"] {
            let environment = [EnvironmentEntry {
                name: b"PATH",
                value: path,
            }];
            let arguments: [&[u8]; 0] = [];
            let payload = request_payload(RequestInput {
                generation: 8,
                executable: b"sh",
                working_directory: cwd,
                client_working_directory: cwd,
                arguments: &arguments,
                environment: &environment,
                rows: 10,
                columns: 100,
            });
            assert_eq!(
                prepare(&payload, target(8, 45), fixture.path()).unwrap_err(),
                PrepareError::ResolutionFailed
            );
        }
        let environment = [EnvironmentEntry {
            name: b"CLIENT_ONLY",
            value: b"captured",
        }];
        let arguments: [&[u8]; 0] = [];
        let payload = request_payload(RequestInput {
            generation: 8,
            executable: b"sh",
            working_directory: cwd,
            client_working_directory: cwd,
            arguments: &arguments,
            environment: &environment,
            rows: 10,
            columns: 100,
        });
        assert_eq!(
            prepare(&payload, target(8, 45), fixture.path()).unwrap_err(),
            PrepareError::ResolutionFailed
        );
    }

    #[test]
    fn rejects_non_utf8_missing_working_path_with_fixed_redacted_error() {
        let fixture = Fixture::new();
        let cwd = fixture.path().as_os_str().as_bytes();
        let environment = valid_environment(b"/bin:/usr/bin");
        let mut missing_path =
            format!("/private/tmp/panelink-missing-path-{}-", std::process::id()).into_bytes();
        missing_path.push(0xff);
        let arguments: [&[u8]; 0] = [];
        let payload = request_payload(RequestInput {
            generation: 8,
            executable: b"/bin/sh",
            working_directory: &missing_path,
            client_working_directory: cwd,
            arguments: &arguments,
            environment: &environment,
            rows: 10,
            columns: 100,
        });

        let error = prepare(&payload, target(8, 45), fixture.path()).unwrap_err();
        assert_eq!(error, PrepareError::ResolutionFailed);
        assert_eq!(
            error.to_string(),
            "managed launch target could not be resolved"
        );
        assert!(!format!("{error:?}").contains("panelink-missing-path"));
    }
}
