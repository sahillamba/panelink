//! Explicit child-process launch environment construction and executable lookup.
//!
//! This module is deliberately side-effect free: it never reads the server's
//! environment, persists launch data, expands shell syntax, or starts a process.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
#[cfg(unix)]
use std::path::Component;

/// An explicitly constructed environment for one child launch.
pub type Environment = BTreeMap<OsString, OsString>;

/// Client-scoped launch inputs supplied with an attachment or explicit profile.
///
/// `working_directory` must be absolute. The environment is volatile and stays
/// scoped to this context; constructing another context does not affect it.
#[derive(Clone, PartialEq, Eq)]
pub struct LaunchContext {
    /// The requesting client's declared absolute working directory.
    pub working_directory: PathBuf,
    /// The requesting client's environment captured for this launch context.
    pub environment: Environment,
}

impl fmt::Debug for LaunchContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaunchContext")
            .field("working_directory", &self.working_directory)
            .field("environment", &RedactedEnvironment(self.environment.len()))
            .finish()
    }
}

struct RedactedEnvironment(usize);

impl fmt::Debug for RedactedEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<redacted: {} variables>", self.0)
    }
}

/// A single explicit environment assignment or unset operation.
#[derive(Clone, PartialEq, Eq)]
pub struct EnvironmentOverride {
    /// Environment variable name.
    pub name: OsString,
    /// `Some(value)` sets the variable; `None` removes lower-priority values.
    pub value: Option<OsString>,
}

impl fmt::Debug for EnvironmentOverride {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnvironmentOverride")
            .field("name", &self.name)
            .field("value", &self.value.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// A fully resolved launch target. It contains no child environment values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedLaunch {
    /// Absolute path selected from the explicit executable or effective `PATH`.
    /// This preserves the selected path, including a symlink path if supplied.
    pub requested_path: PathBuf,
    /// Canonical absolute path of the selected executable target.
    pub executable_path: PathBuf,
    /// Canonical absolute working directory for the launch.
    pub working_directory: PathBuf,
}

/// Failures while constructing an explicit environment or resolving a launch.
///
/// Variants intentionally contain no environment values or caller-provided
/// path text, so displaying an error cannot disclose those inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchError {
    /// An environment name is empty, contains `=`, or contains NUL.
    InvalidEnvironmentName,
    /// An environment value contains NUL.
    InvalidEnvironmentValue,
    /// A caller supplied a reserved `PANELINK_*` identity variable.
    ReservedEnvironmentName,
    /// The Panelink-owned environment omitted `TERM`.
    MissingOwnedTerm,
    /// The Panelink-owned `TERM` value is empty.
    EmptyOwnedTerm,
    /// The requesting client's base working directory was not absolute.
    RelativeClientWorkingDirectory,
    /// A requested executable is empty or contains NUL.
    InvalidExecutable,
    /// A relative executable contains a path separator; only a basename is allowed.
    RelativeExecutableContainsSeparator,
    /// The effective environment does not contain `PATH`.
    MissingPath,
    /// `PATH` has an empty component.
    EmptyPathComponent,
    /// `PATH` contains a relative component.
    RelativePathComponent,
    /// The executable was not found as a regular executable file.
    ExecutableNotFound,
    /// An explicitly named executable does not resolve to a regular file.
    ExecutableNotRegularFile,
    /// An explicitly named file has no executable permission bit.
    ExecutableNotExecutable,
    /// The resolved working directory is not a directory.
    WorkingDirectoryNotDirectory,
    /// Unix executable-permission checks are unavailable on this platform.
    UnsupportedPlatform,
    /// An operating-system filesystem operation failed.
    Io {
        /// A stable operation label without caller-provided paths.
        operation: &'static str,
        /// The operating-system error category.
        kind: io::ErrorKind,
    },
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEnvironmentName => {
                f.write_str("environment contains an invalid variable name")
            }
            Self::InvalidEnvironmentValue => {
                f.write_str("environment contains an invalid variable value")
            }
            Self::ReservedEnvironmentName => {
                f.write_str("caller environment contains a reserved Panelink variable")
            }
            Self::MissingOwnedTerm => f.write_str("Panelink-owned environment must provide TERM"),
            Self::EmptyOwnedTerm => f.write_str("Panelink-owned TERM must not be empty"),
            Self::RelativeClientWorkingDirectory => {
                f.write_str("client working directory must be absolute")
            }
            Self::InvalidExecutable => f.write_str("executable name is invalid"),
            Self::RelativeExecutableContainsSeparator => {
                f.write_str("relative executable must be an unqualified basename")
            }
            Self::MissingPath => f.write_str("effective environment has no PATH"),
            Self::EmptyPathComponent => f.write_str("PATH contains an empty component"),
            Self::RelativePathComponent => f.write_str("PATH contains a relative component"),
            Self::ExecutableNotFound => {
                f.write_str("no executable file was found in the requested location")
            }
            Self::ExecutableNotRegularFile => {
                f.write_str("explicit executable does not resolve to a regular file")
            }
            Self::ExecutableNotExecutable => {
                f.write_str("explicit executable has no executable permission bit")
            }
            Self::WorkingDirectoryNotDirectory => {
                f.write_str("resolved working directory is not a directory")
            }
            Self::UnsupportedPlatform => {
                f.write_str("executable resolution is unsupported on this platform")
            }
            Self::Io { operation, kind } => {
                write!(f, "filesystem operation {operation} failed ({kind:?})")
            }
        }
    }
}

impl std::error::Error for LaunchError {}

/// Construct an explicit child environment using Panelink's precedence order.
///
/// Values are applied in this order: account defaults, the requesting client's
/// captured environment, each override/unset layer in order, and finally the
/// Panelink-owned values. Caller-supplied `PANELINK_*` names are rejected in
/// every layer; the owned layer may set them. `TERM` in the owned layer is
/// required and replaces any account, client, or user value. No process-global
/// environment is read.
pub fn build_environment(
    account_defaults: &Environment,
    context: &LaunchContext,
    layers: &[Vec<EnvironmentOverride>],
    owned: &Environment,
) -> Result<Environment, LaunchError> {
    validate_environment(account_defaults, false)?;
    validate_environment(&context.environment, false)?;
    for layer in layers {
        for item in layer {
            validate_name(&item.name, false)?;
            if let Some(value) = &item.value {
                validate_value(value)?;
            }
        }
    }
    validate_environment(owned, true)?;

    let owned_term = owned
        .get(OsStr::new("TERM"))
        .ok_or(LaunchError::MissingOwnedTerm)?;
    if owned_term.is_empty() {
        return Err(LaunchError::EmptyOwnedTerm);
    }

    let mut result = account_defaults.clone();
    result.extend(
        context
            .environment
            .iter()
            .map(|(k, v)| (k.clone(), v.clone())),
    );
    for layer in layers {
        for item in layer {
            if let Some(value) = &item.value {
                result.insert(item.name.clone(), value.clone());
            } else {
                result.remove(&item.name);
            }
        }
    }
    result.extend(owned.iter().map(|(k, v)| (k.clone(), v.clone())));
    Ok(result)
}

fn validate_environment(
    environment: &Environment,
    allow_panelink: bool,
) -> Result<(), LaunchError> {
    for (name, value) in environment {
        validate_name(name, allow_panelink)?;
        validate_value(value)?;
    }
    Ok(())
}

fn validate_name(name: &OsStr, allow_panelink: bool) -> Result<(), LaunchError> {
    if name.is_empty() || contains_nul(name) || contains_equals(name) {
        return Err(LaunchError::InvalidEnvironmentName);
    }
    if !allow_panelink && is_panelink_name(name) {
        return Err(LaunchError::ReservedEnvironmentName);
    }
    Ok(())
}

fn validate_value(value: &OsStr) -> Result<(), LaunchError> {
    if contains_nul(value) {
        return Err(LaunchError::InvalidEnvironmentValue);
    }
    Ok(())
}

#[cfg(unix)]
fn contains_nul(value: &OsStr) -> bool {
    value.as_bytes().contains(&0)
}

#[cfg(not(unix))]
fn contains_nul(value: &OsStr) -> bool {
    value.to_string_lossy().contains('\0')
}

#[cfg(unix)]
fn contains_equals(value: &OsStr) -> bool {
    value.as_bytes().contains(&b'=')
}

#[cfg(not(unix))]
fn contains_equals(value: &OsStr) -> bool {
    value.to_string_lossy().contains('=')
}

#[cfg(unix)]
fn is_panelink_name(name: &OsStr) -> bool {
    name.as_bytes().starts_with(b"PANELINK_")
}

#[cfg(not(unix))]
fn is_panelink_name(name: &OsStr) -> bool {
    name.to_string_lossy().starts_with("PANELINK_")
}

/// Resolve a launch executable and working directory without spawning it.
///
/// Relative working directories are based on the client's absolute working
/// directory. An absolute executable bypasses `PATH`; a relative executable
/// must be an unqualified basename and is searched against the explicit
/// effective environment. All `PATH` components are validated before lookup.
/// The returned paths are absolute, and the executable target is canonicalized.
/// This is filesystem inspection only; it does not prevent a target from being
/// replaced before any future caller attempts to spawn it.
pub fn resolve_launch(
    executable: &OsStr,
    working_directory: &Path,
    context: &LaunchContext,
    environment: &Environment,
) -> Result<ResolvedLaunch, LaunchError> {
    #[cfg(unix)]
    {
        resolve_launch_unix(executable, working_directory, context, environment)
    }
    #[cfg(not(unix))]
    {
        let _ = (executable, working_directory, context, environment);
        Err(LaunchError::UnsupportedPlatform)
    }
}

#[cfg(unix)]
fn resolve_launch_unix(
    executable: &OsStr,
    working_directory: &Path,
    context: &LaunchContext,
    environment: &Environment,
) -> Result<ResolvedLaunch, LaunchError> {
    if executable.is_empty() || contains_nul(executable) {
        return Err(LaunchError::InvalidExecutable);
    }
    if !context.working_directory.is_absolute() {
        return Err(LaunchError::RelativeClientWorkingDirectory);
    }

    let requested_working_directory = if working_directory.is_absolute() {
        working_directory.to_path_buf()
    } else {
        context.working_directory.join(working_directory)
    };
    let resolved_working_directory =
        fs_canonicalize(&requested_working_directory, "resolve working directory")?;
    let working_metadata = fs_metadata(&resolved_working_directory, "inspect working directory")?;
    if !working_metadata.is_dir() {
        return Err(LaunchError::WorkingDirectoryNotDirectory);
    }

    let (requested_path, executable_path) = if Path::new(executable).is_absolute() {
        let requested_path = PathBuf::from(executable);
        let canonical = fs_canonicalize(&requested_path, "resolve explicit executable")?;
        let metadata = fs_metadata(&canonical, "inspect explicit executable")?;
        if !metadata.is_file() {
            return Err(LaunchError::ExecutableNotRegularFile);
        }
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(LaunchError::ExecutableNotExecutable);
        }
        (requested_path, canonical)
    } else {
        if !is_unqualified_basename(executable) {
            return Err(LaunchError::RelativeExecutableContainsSeparator);
        }
        resolve_from_path(executable, environment)?
    };

    Ok(ResolvedLaunch {
        requested_path,
        executable_path,
        working_directory: resolved_working_directory,
    })
}

#[cfg(unix)]
fn is_unqualified_basename(executable: &OsStr) -> bool {
    if executable.as_bytes().contains(&b'/') {
        return false;
    }
    let mut components = Path::new(executable).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

#[cfg(unix)]
fn resolve_from_path(
    executable: &OsStr,
    environment: &Environment,
) -> Result<(PathBuf, PathBuf), LaunchError> {
    let path = environment
        .get(OsStr::new("PATH"))
        .ok_or(LaunchError::MissingPath)?;
    if contains_nul(path) {
        return Err(LaunchError::EmptyPathComponent);
    }

    let components: Vec<PathBuf> = std::env::split_paths(path).collect();
    if components.is_empty() {
        return Err(LaunchError::EmptyPathComponent);
    }
    for component in &components {
        if component.as_os_str().is_empty() {
            return Err(LaunchError::EmptyPathComponent);
        }
        if !component.is_absolute() {
            return Err(LaunchError::RelativePathComponent);
        }
    }

    for directory in components {
        let candidate = directory.join(executable);
        match inspect_path_candidate(&candidate)? {
            Some(canonical) => return Ok((candidate, canonical)),
            None => continue,
        }
    }
    Err(LaunchError::ExecutableNotFound)
}

#[cfg(unix)]
fn inspect_path_candidate(candidate: &Path) -> Result<Option<PathBuf>, LaunchError> {
    let canonical = match std::fs::canonicalize(candidate) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("search executable path", error)),
    };
    let metadata = match std::fs::metadata(&canonical) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("inspect PATH executable", error)),
    };
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Ok(None);
    }
    Ok(Some(canonical))
}

#[cfg(unix)]
fn fs_canonicalize(path: &Path, operation: &'static str) -> Result<PathBuf, LaunchError> {
    std::fs::canonicalize(path).map_err(|error| io_error(operation, error))
}

#[cfg(unix)]
fn fs_metadata(path: &Path, operation: &'static str) -> Result<std::fs::Metadata, LaunchError> {
    std::fs::metadata(path).map_err(|error| io_error(operation, error))
}

#[cfg(unix)]
fn io_error(operation: &'static str, error: io::Error) -> LaunchError {
    LaunchError::Io {
        operation,
        kind: error.kind(),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("panelink-launch-{}-{id}", std::process::id()));
            fs::create_dir(&path).expect("create unique fixture directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn executable(&self, directory: &str, name: &str) -> PathBuf {
            let directory = self.path().join(directory);
            fs::create_dir_all(&directory).expect("create executable directory");
            let path = directory.join(name);
            fs::write(&path, b"fixture executable\n").expect("write executable fixture");
            #[cfg(unix)]
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .expect("set executable mode");
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn env(entries: &[(&str, &str)]) -> Environment {
        entries
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    fn owned() -> Environment {
        env(&[
            ("TERM", "xterm-panelink"),
            ("PANELINK_SESSION", "session-1"),
        ])
    }

    #[test]
    fn applies_precedence_and_unsets_in_order() {
        let defaults = env(&[("LEVEL", "account"), ("REMOVE", "account")]);
        let context = LaunchContext {
            working_directory: PathBuf::from("/client"),
            environment: env(&[("LEVEL", "client"), ("TERM", "client-term")]),
        };
        let layers = vec![
            vec![
                EnvironmentOverride {
                    name: "LEVEL".into(),
                    value: Some("workspace".into()),
                },
                EnvironmentOverride {
                    name: "REMOVE".into(),
                    value: None,
                },
                EnvironmentOverride {
                    name: "TERM".into(),
                    value: Some("user-term".into()),
                },
            ],
            vec![EnvironmentOverride {
                name: "LEVEL".into(),
                value: Some("one-launch".into()),
            }],
        ];

        let result = build_environment(&defaults, &context, &layers, &owned()).unwrap();
        assert_eq!(
            result.get(OsStr::new("LEVEL")).unwrap(),
            &OsString::from("one-launch")
        );
        assert!(!result.contains_key(OsStr::new("REMOVE")));
        assert_eq!(
            result.get(OsStr::new("TERM")).unwrap(),
            &OsString::from("xterm-panelink")
        );
        assert_eq!(
            result.get(OsStr::new("PANELINK_SESSION")).unwrap(),
            &OsString::from("session-1")
        );
    }

    #[test]
    fn rejects_reserved_names_in_every_caller_layer() {
        let defaults = env(&[("PANELINK_ID", "account")]);
        let context = LaunchContext {
            working_directory: PathBuf::from("/client"),
            environment: Environment::new(),
        };
        assert_eq!(
            build_environment(&defaults, &context, &[], &owned()),
            Err(LaunchError::ReservedEnvironmentName)
        );

        let defaults = Environment::new();
        let context = LaunchContext {
            working_directory: PathBuf::from("/client"),
            environment: env(&[("PANELINK_ID", "client")]),
        };
        assert_eq!(
            build_environment(&defaults, &context, &[], &owned()),
            Err(LaunchError::ReservedEnvironmentName)
        );

        let context = LaunchContext {
            working_directory: PathBuf::from("/client"),
            environment: Environment::new(),
        };
        let layers = vec![vec![EnvironmentOverride {
            name: "PANELINK_ID".into(),
            value: None,
        }]];
        assert_eq!(
            build_environment(&defaults, &context, &layers, &owned()),
            Err(LaunchError::ReservedEnvironmentName)
        );
    }

    #[test]
    fn rejects_invalid_environment_names_and_values() {
        let context = LaunchContext {
            working_directory: PathBuf::from("/client"),
            environment: Environment::new(),
        };
        let mut bad_name = Environment::new();
        bad_name.insert("BAD=NAME".into(), "value".into());
        assert_eq!(
            build_environment(&bad_name, &context, &[], &owned()),
            Err(LaunchError::InvalidEnvironmentName)
        );

        let mut bad_value = Environment::new();
        bad_value.insert("TOKEN".into(), OsString::from("bad\0value"));
        assert_eq!(
            build_environment(&bad_value, &context, &[], &owned()),
            Err(LaunchError::InvalidEnvironmentValue)
        );

        let mut empty_name = Environment::new();
        empty_name.insert(OsString::new(), "value".into());
        assert_eq!(
            build_environment(&empty_name, &context, &[], &owned()),
            Err(LaunchError::InvalidEnvironmentName)
        );
    }

    #[test]
    fn owned_term_is_required_and_nonempty() {
        let context = LaunchContext {
            working_directory: PathBuf::from("/client"),
            environment: env(&[("TERM", "client-term")]),
        };
        assert_eq!(
            build_environment(&Environment::new(), &context, &[], &Environment::new()),
            Err(LaunchError::MissingOwnedTerm)
        );
        assert_eq!(
            build_environment(&Environment::new(), &context, &[], &env(&[("TERM", "")])),
            Err(LaunchError::EmptyOwnedTerm)
        );
    }

    #[test]
    fn debug_output_redacts_environment_values_and_override_values() {
        let secret = "fixture-secret-should-not-appear";
        let context = LaunchContext {
            working_directory: PathBuf::from("/client"),
            environment: env(&[("TOKEN", secret)]),
        };
        let override_value = EnvironmentOverride {
            name: "TOKEN".into(),
            value: Some(secret.into()),
        };
        assert!(!format!("{context:?}").contains(secret));
        assert!(!format!("{override_value:?}").contains(secret));
        assert!(format!("{override_value:?}").contains("<redacted>"));
    }

    #[test]
    fn contexts_do_not_contaminate_each_other() {
        let defaults = Environment::new();
        let first = LaunchContext {
            working_directory: PathBuf::from("/first"),
            environment: env(&[("AUTH_CONTEXT", "first-only")]),
        };
        let second = LaunchContext {
            working_directory: PathBuf::from("/second"),
            environment: env(&[("AUTH_CONTEXT", "second-only")]),
        };
        let first_result = build_environment(&defaults, &first, &[], &owned()).unwrap();
        let second_result = build_environment(&defaults, &second, &[], &owned()).unwrap();
        assert_eq!(
            first_result.get(OsStr::new("AUTH_CONTEXT")).unwrap(),
            &OsString::from("first-only")
        );
        assert_eq!(
            second_result.get(OsStr::new("AUTH_CONTEXT")).unwrap(),
            &OsString::from("second-only")
        );
    }

    #[test]
    fn resolves_absolute_executable_and_relative_working_directory() {
        let fixture = Fixture::new();
        let executable = fixture.executable("bin", "tool");
        let client_directory = fixture.path().join("client");
        fs::create_dir_all(client_directory.join("child")).unwrap();
        let context = LaunchContext {
            working_directory: client_directory.clone(),
            environment: Environment::new(),
        };

        let result = resolve_launch(
            executable.as_os_str(),
            Path::new("child"),
            &context,
            &Environment::new(),
        )
        .unwrap();
        assert_eq!(result.requested_path, executable);
        assert_eq!(
            result.executable_path,
            fs::canonicalize(&executable).unwrap()
        );
        assert_eq!(
            result.working_directory,
            fs::canonicalize(client_directory.join("child")).unwrap()
        );
    }

    #[test]
    fn resolves_existing_relative_working_directory() {
        let fixture = Fixture::new();
        let executable = fixture.executable("bin", "tool");
        let client_directory = fixture.path().join("client");
        fs::create_dir_all(client_directory.join("child")).unwrap();
        let context = LaunchContext {
            working_directory: client_directory.clone(),
            environment: Environment::new(),
        };
        let result = resolve_launch(
            executable.as_os_str(),
            Path::new("child"),
            &context,
            &Environment::new(),
        )
        .unwrap();
        assert_eq!(
            result.working_directory,
            fs::canonicalize(client_directory.join("child")).unwrap()
        );
    }

    #[test]
    fn searches_path_in_order_and_returns_canonical_target() {
        let fixture = Fixture::new();
        let first = fixture.executable("first", "tool");
        let second = fixture.executable("second", "tool");
        let cwd = fixture.path().join("client");
        fs::create_dir(&cwd).unwrap();
        let context = LaunchContext {
            working_directory: cwd,
            environment: Environment::new(),
        };
        let path = std::env::join_paths([
            first.parent().unwrap().as_os_str(),
            second.parent().unwrap().as_os_str(),
        ])
        .unwrap();
        let environment = [(OsString::from("PATH"), path)].into_iter().collect();

        let result =
            resolve_launch(OsStr::new("tool"), Path::new("."), &context, &environment).unwrap();
        assert_eq!(result.requested_path, first);
        assert_eq!(result.executable_path, fs::canonicalize(first).unwrap());
    }

    #[test]
    fn validates_all_path_components_before_search() {
        let fixture = Fixture::new();
        let executable = fixture.executable("bin", "tool");
        let cwd = fixture.path().join("client");
        fs::create_dir(&cwd).unwrap();
        let context = LaunchContext {
            working_directory: cwd,
            environment: Environment::new(),
        };
        let path =
            std::env::join_paths([executable.parent().unwrap(), Path::new("relative")]).unwrap();
        let environment = [(OsString::from("PATH"), path)].into_iter().collect();
        assert_eq!(
            resolve_launch(OsStr::new("tool"), Path::new("."), &context, &environment),
            Err(LaunchError::RelativePathComponent)
        );
    }

    #[test]
    fn rejects_missing_path_empty_and_separator_qualified_relative_executables() {
        let fixture = Fixture::new();
        let cwd = fixture.path().join("client");
        fs::create_dir(&cwd).unwrap();
        let context = LaunchContext {
            working_directory: cwd,
            environment: Environment::new(),
        };
        assert_eq!(
            resolve_launch(
                OsStr::new("tool"),
                Path::new("."),
                &context,
                &Environment::new()
            ),
            Err(LaunchError::MissingPath)
        );

        let empty_path = [(OsString::from("PATH"), OsString::new())]
            .into_iter()
            .collect();
        assert_eq!(
            resolve_launch(OsStr::new("tool"), Path::new("."), &context, &empty_path),
            Err(LaunchError::EmptyPathComponent)
        );
        for executable in ["./tool", "foo/bar", "tool/", "tool/."] {
            assert_eq!(
                resolve_launch(
                    OsStr::new(executable),
                    Path::new("."),
                    &context,
                    &empty_path
                ),
                Err(LaunchError::RelativeExecutableContainsSeparator),
                "relative path {executable:?} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_inaccessible_context_directory_and_displays_no_values() {
        let fixture = Fixture::new();
        let context = LaunchContext {
            working_directory: PathBuf::from("relative-client-dir"),
            environment: env(&[("TOKEN", "sensitive-value")]),
        };
        let error = resolve_launch(
            OsStr::new("tool"),
            Path::new("."),
            &context,
            &Environment::new(),
        )
        .unwrap_err();
        assert_eq!(error, LaunchError::RelativeClientWorkingDirectory);
        assert!(!error.to_string().contains("sensitive-value"));
        drop(fixture);
    }
}
