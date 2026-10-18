use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::io::IsTerminal;

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
use panelink::launch::{resolve_launch, Environment, LaunchContext, LaunchError, ResolvedLaunch};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod p2_probe;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod p3_cli;

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
const MAX_ENVIRONMENT_VARIABLES: usize = 4_096;
#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
const MAX_ENVIRONMENT_BYTES: usize = 1024 * 1024;
const MAX_CHILD_ARGUMENTS: usize = 256;
const MAX_EXECUTABLE_AND_ARGUMENT_BYTES: usize = 64 * 1024;

enum Command {
    Help,
    Version,
    Doctor,
    PtyProbe(ProbeRequest),
}

struct ProbeRequest {
    terminfo_directory: PathBuf,
    executable: OsString,
    arguments: Vec<OsString>,
}

#[derive(Debug)]
enum CliError {
    InvalidArguments,
    ArgumentLimitExceeded,
    #[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
    EnvironmentLimitExceeded,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    InvalidTerminfoDirectory,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    TerminfoEntryMissing,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    CurrentDirectoryUnavailable,
    #[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
    Launch(LaunchError),
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArguments => {
                formatter.write_str("invalid command syntax; try 'panelink --help'")
            }
            Self::ArgumentLimitExceeded => formatter.write_str("child argv exceeds probe limits"),
            #[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
            Self::EnvironmentLimitExceeded => {
                formatter.write_str("client environment exceeds probe limits")
            }
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Self::InvalidTerminfoDirectory => {
                formatter.write_str("terminfo directory is unavailable or invalid")
            }
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Self::TerminfoEntryMissing => {
                formatter.write_str("compiled panelink-probe terminfo entry was not found")
            }
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Self::CurrentDirectoryUnavailable => {
                formatter.write_str("could not capture the client working directory")
            }
            #[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
            Self::Launch(error) => write!(formatter, "launch resolution failed: {error}"),
        }
    }
}

fn main() {
    std::process::exit(run(std::env::args_os().skip(1)));
}

fn run<I>(arguments: I) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let mut arguments = arguments.into_iter().peekable();
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    if arguments
        .peek()
        .is_some_and(|argument| argument == "server-probe" || argument == "control")
    {
        let mut captured = Vec::new();
        let mut bytes = 0_usize;
        for argument in arguments {
            bytes = match bytes.checked_add(os_str_byte_len(&argument)) {
                Some(total) if total <= MAX_EXECUTABLE_AND_ARGUMENT_BYTES => total,
                _ => return report_cli_error(CliError::ArgumentLimitExceeded),
            };
            if captured.len() >= MAX_CHILD_ARGUMENTS + 12 {
                return report_cli_error(CliError::ArgumentLimitExceeded);
            }
            captured.push(argument);
        }
        return p3_cli::run(captured);
    }
    let command = match parse_command(arguments) {
        Ok(command) => command,
        Err(error) => return report_cli_error(error),
    };

    match command {
        Command::Help => {
            print_help();
            0
        }
        Command::Version => {
            println!("panelink {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Command::Doctor => {
            println!("Panelink runtime diagnostics");
            println!(
                "Target: {}/{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            );
            println!("Experimental PTY probe: one-shot child launch on macOS/aarch64.");
            println!("Experimental local server/control probe: available on macOS/aarch64.");
            println!("Persistent multiplexer: unsupported; host and pathname acceptance pending.");
            1
        }
        Command::PtyProbe(request) => execute_probe(request),
    }
}

fn parse_command<I>(arguments: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut arguments = arguments.into_iter();
    let Some(first) = arguments.next() else {
        return Ok(Command::Help);
    };

    if first == OsStr::new("--help") || first == OsStr::new("-h") {
        return if arguments.next().is_none() {
            Ok(Command::Help)
        } else {
            Err(CliError::InvalidArguments)
        };
    }
    if first == OsStr::new("--version") {
        return if arguments.next().is_none() {
            Ok(Command::Version)
        } else {
            Err(CliError::InvalidArguments)
        };
    }
    if first == OsStr::new("doctor") {
        return if arguments.next().is_none() {
            Ok(Command::Doctor)
        } else {
            Err(CliError::InvalidArguments)
        };
    }
    if first != OsStr::new("pty-probe") {
        return Err(CliError::InvalidArguments);
    }

    let Some(terminfo_flag) = arguments.next() else {
        return Err(CliError::InvalidArguments);
    };
    if terminfo_flag != OsStr::new("--terminfo") {
        return Err(CliError::InvalidArguments);
    }
    let Some(terminfo_argument) = arguments.next() else {
        return Err(CliError::InvalidArguments);
    };
    let terminfo_directory = PathBuf::from(terminfo_argument);
    if !terminfo_directory.is_absolute() {
        return Err(CliError::InvalidArguments);
    }
    let Some(separator) = arguments.next() else {
        return Err(CliError::InvalidArguments);
    };
    if separator != OsStr::new("--") {
        return Err(CliError::InvalidArguments);
    }
    let Some(executable) = arguments.next() else {
        return Err(CliError::InvalidArguments);
    };
    if executable.is_empty() {
        return Err(CliError::InvalidArguments);
    }
    let child_arguments = capture_child_arguments(&executable, arguments)?;

    Ok(Command::PtyProbe(ProbeRequest {
        terminfo_directory,
        executable,
        arguments: child_arguments,
    }))
}

fn capture_child_arguments(
    executable: &OsStr,
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Vec<OsString>, CliError> {
    let mut total_bytes = os_str_byte_len(executable);
    if total_bytes > MAX_EXECUTABLE_AND_ARGUMENT_BYTES {
        return Err(CliError::ArgumentLimitExceeded);
    }

    let mut captured = Vec::new();
    for argument in arguments {
        if captured.len() == MAX_CHILD_ARGUMENTS {
            return Err(CliError::ArgumentLimitExceeded);
        }
        let Some(next_total) = total_bytes.checked_add(os_str_byte_len(&argument)) else {
            return Err(CliError::ArgumentLimitExceeded);
        };
        if next_total > MAX_EXECUTABLE_AND_ARGUMENT_BYTES {
            return Err(CliError::ArgumentLimitExceeded);
        }
        captured.push(argument);
        total_bytes = next_total;
    }
    Ok(captured)
}

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
fn capture_client_environment(
    entries: impl IntoIterator<Item = (OsString, OsString)>,
) -> Result<Environment, CliError> {
    let mut environment = Environment::new();
    let mut total_bytes = 0_usize;

    for (count, (name, value)) in entries.into_iter().enumerate() {
        if count == MAX_ENVIRONMENT_VARIABLES {
            return Err(CliError::EnvironmentLimitExceeded);
        }
        let Some(entry_bytes) = os_str_byte_len(&name).checked_add(os_str_byte_len(&value)) else {
            return Err(CliError::EnvironmentLimitExceeded);
        };
        let Some(next_total) = total_bytes.checked_add(entry_bytes) else {
            return Err(CliError::EnvironmentLimitExceeded);
        };
        if next_total > MAX_ENVIRONMENT_BYTES {
            return Err(CliError::EnvironmentLimitExceeded);
        }
        total_bytes = next_total;
        environment.insert(name, value);
    }

    Ok(environment)
}

#[cfg(unix)]
fn os_str_byte_len(value: &OsStr) -> usize {
    use std::os::unix::ffi::OsStrExt;

    value.as_bytes().len()
}

#[cfg(not(unix))]
fn os_str_byte_len(value: &OsStr) -> usize {
    value.to_string_lossy().len()
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
struct PreparedProbe {
    launch: ResolvedLaunch,
    arguments: Vec<OsString>,
    environment: Environment,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn execute_probe(request: ProbeRequest) -> i32 {
    let prepared = match prepare_probe(request) {
        Ok(prepared) => prepared,
        Err(error) => return report_cli_error(error),
    };

    match p2_probe::run(&prepared.launch, &prepared.arguments, &prepared.environment) {
        Ok(exit) => probe_exit_code(exit),
        Err(error) => {
            if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                p2_probe::report_error(&error);
            } else {
                eprintln!("error: {error}");
            }
            1
        }
    }
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn execute_probe(request: ProbeRequest) -> i32 {
    let ProbeRequest {
        terminfo_directory,
        executable,
        arguments,
    } = request;
    drop((terminfo_directory, executable, arguments));
    eprintln!("error: pty-probe is supported only on macOS/aarch64");
    1
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn prepare_probe(request: ProbeRequest) -> Result<PreparedProbe, CliError> {
    let terminfo_directory = canonical_terminfo_directory(&request.terminfo_directory)?;
    let working_directory =
        std::env::current_dir().map_err(|_| CliError::CurrentDirectoryUnavailable)?;
    let client_environment = capture_client_environment(std::env::vars_os())?;
    let context = LaunchContext {
        working_directory,
        environment: client_environment,
    };
    let (launch, environment) =
        resolve_probe_launch(&terminfo_directory, &request.executable, &context)?;
    drop(context);

    Ok(PreparedProbe {
        launch,
        arguments: request.arguments,
        environment,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn canonical_terminfo_directory(directory: &Path) -> Result<PathBuf, CliError> {
    if !directory.is_absolute() {
        return Err(CliError::InvalidTerminfoDirectory);
    }
    let directory =
        std::fs::canonicalize(directory).map_err(|_| CliError::InvalidTerminfoDirectory)?;
    let metadata = std::fs::metadata(&directory).map_err(|_| CliError::InvalidTerminfoDirectory)?;
    if !metadata.is_dir() {
        return Err(CliError::InvalidTerminfoDirectory);
    }

    let has_compiled_entry = ["p", "70"].into_iter().any(|prefix| {
        std::fs::metadata(directory.join(prefix).join("panelink-probe"))
            .is_ok_and(|entry| entry.is_file())
    });
    if !has_compiled_entry {
        return Err(CliError::TerminfoEntryMissing);
    }
    Ok(directory)
}

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
fn resolve_probe_launch(
    terminfo_directory: &Path,
    executable: &OsStr,
    context: &LaunchContext,
) -> Result<(ResolvedLaunch, Environment), CliError> {
    validate_final_environment_profile(&context.environment, terminfo_directory.as_os_str())?;
    let mut owned = Environment::new();
    owned.insert(OsString::from("TERM"), OsString::from("panelink-probe"));
    owned.insert(
        OsString::from("TERMINFO"),
        terminfo_directory.as_os_str().to_owned(),
    );
    let defaults = Environment::new();
    let environment = panelink::launch::build_environment(&defaults, context, &[], &owned)
        .map_err(CliError::Launch)?;
    let launch = resolve_launch(executable, Path::new("."), context, &environment)
        .map_err(CliError::Launch)?;
    Ok((launch, environment))
}

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
fn validate_final_environment_profile(
    client_environment: &Environment,
    terminfo_directory: &OsStr,
) -> Result<(), CliError> {
    let term_name = OsStr::new("TERM");
    let terminfo_name = OsStr::new("TERMINFO");
    let additional_variables = client_environment
        .keys()
        .filter(|name| *name != term_name && *name != terminfo_name)
        .count();
    if additional_variables > MAX_ENVIRONMENT_VARIABLES - 2 {
        return Err(CliError::EnvironmentLimitExceeded);
    }
    let mut total_bytes = os_str_byte_len(term_name)
        .checked_add(os_str_byte_len(OsStr::new("panelink-probe")))
        .and_then(|bytes| bytes.checked_add(os_str_byte_len(terminfo_name)))
        .and_then(|bytes| bytes.checked_add(os_str_byte_len(terminfo_directory)))
        .ok_or(CliError::EnvironmentLimitExceeded)?;

    for (name, value) in client_environment {
        if name == term_name || name == terminfo_name {
            continue;
        }
        let entry_bytes = os_str_byte_len(name)
            .checked_add(os_str_byte_len(value))
            .ok_or(CliError::EnvironmentLimitExceeded)?;
        total_bytes = total_bytes
            .checked_add(entry_bytes)
            .ok_or(CliError::EnvironmentLimitExceeded)?;
        if total_bytes > MAX_ENVIRONMENT_BYTES {
            return Err(CliError::EnvironmentLimitExceeded);
        }
    }

    if total_bytes > MAX_ENVIRONMENT_BYTES {
        return Err(CliError::EnvironmentLimitExceeded);
    }
    Ok(())
}

fn report_cli_error(error: CliError) -> i32 {
    let status = match &error {
        CliError::InvalidArguments | CliError::ArgumentLimitExceeded => 2,
        #[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
        CliError::EnvironmentLimitExceeded => 1,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        CliError::InvalidTerminfoDirectory
        | CliError::TerminfoEntryMissing
        | CliError::CurrentDirectoryUnavailable => 1,
        #[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
        CliError::Launch(_) => 1,
    };
    eprintln!("error: {error}");
    status
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn probe_exit_code(exit: p2_probe::ProbeExit) -> i32 {
    match exit {
        p2_probe::ProbeExit::Child(status) => child_exit_code(status),
        p2_probe::ProbeExit::Quit => 0,
        p2_probe::ProbeExit::Interrupted => 130,
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn child_exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;

    status
        .code()
        .or_else(|| status.signal().map(|signal| 128_i32.saturating_add(signal)))
        .unwrap_or(1)
}

fn print_help() {
    println!(
        "Panelink {} - experimental terminal probe",
        env!("CARGO_PKG_VERSION")
    );
    println!("Usage:");
    println!("  panelink");
    println!("  panelink --help");
    println!("  panelink -h");
    println!("  panelink --version");
    println!("  panelink doctor");
    println!("  panelink pty-probe --terminfo <absolute-dir> -- <executable> [arguments...]");
    println!("  panelink server-probe --directory <private-dir> --terminfo <absolute-dir> [--initialize] [--allow-input] -- <executable> [arguments...]");
    println!("  panelink control --directory <private-dir> --context <absolute-file> <command> [arguments...]");
    println!();
    println!("pty-probe runs one disposable child on macOS/aarch64.");
    println!("server-probe runs a foreground one-child server; control exchanges typed commands.");
    println!("A persistent multiplexer UI and provider recovery are not implemented.");
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod supported_target_tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn maps_child_exit_statuses_and_probe_control_results() {
        assert_eq!(
            child_exit_code(std::process::ExitStatus::from_raw(23 << 8)),
            23
        );
        assert_eq!(
            child_exit_code(std::process::ExitStatus::from_raw(15)),
            128 + 15
        );
        assert_eq!(probe_exit_code(p2_probe::ProbeExit::Quit), 0);
        assert_eq!(probe_exit_code(p2_probe::ProbeExit::Interrupted), 130);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn help_version_doctor_and_unknown_commands_keep_their_exit_contract() {
        assert_eq!(run(Vec::new()), 0);
        assert_eq!(run(arguments(&["--help"])), 0);
        assert_eq!(run(arguments(&["--version"])), 0);
        assert_eq!(run(arguments(&["doctor"])), 1);
        assert_eq!(run(arguments(&["--unknown-option"])), 2);
        assert_eq!(report_cli_error(CliError::InvalidArguments), 2);
        assert_eq!(report_cli_error(CliError::ArgumentLimitExceeded), 2);
    }

    #[test]
    fn malformed_probe_syntax_is_rejected_without_host_or_environment_access() {
        for malformed in [
            vec!["pty-probe"],
            vec!["pty-probe", "--wrong", "/tmp/ti", "--", "/bin/sh"],
            vec!["pty-probe", "--terminfo", "relative", "--", "/bin/sh"],
            vec!["pty-probe", "--terminfo", "/tmp/ti", "/bin/sh"],
            vec!["pty-probe", "--terminfo", "/tmp/ti", "--"],
            vec!["pty-probe", "--terminfo", "/tmp/ti", "--", ""],
            vec!["doctor", "ARGUMENT-CANARY"],
            vec!["--help", "ARGUMENT-CANARY"],
        ] {
            assert!(matches!(
                parse_command(arguments(&malformed)),
                Err(CliError::InvalidArguments)
            ));
        }

        assert!(!CliError::InvalidArguments
            .to_string()
            .contains("ARGUMENT-CANARY"));
    }

    #[test]
    fn exact_probe_prefix_preserves_executable_and_argument_order() {
        let command = parse_command(arguments(&[
            "pty-probe",
            "--terminfo",
            "/private/tmp/compiled-terminfo",
            "--",
            "/bin/sh",
            "-c",
            "exit 7",
        ]))
        .unwrap();
        match command {
            Command::PtyProbe(request) => {
                assert_eq!(
                    request.terminfo_directory,
                    PathBuf::from("/private/tmp/compiled-terminfo")
                );
                assert_eq!(request.executable, OsString::from("/bin/sh"));
                assert_eq!(request.arguments, arguments(&["-c", "exit 7"]));
            }
            Command::Help | Command::Version | Command::Doctor => {
                panic!("expected the pty-probe command")
            }
        }
    }

    #[test]
    fn argument_capture_enforces_count_and_combined_byte_limits() {
        let executable = OsString::from("x");
        let at_count_limit = (0..MAX_CHILD_ARGUMENTS)
            .map(|_| OsString::from("a"))
            .collect::<Vec<_>>();
        assert_eq!(
            capture_child_arguments(&executable, at_count_limit.clone())
                .unwrap()
                .len(),
            MAX_CHILD_ARGUMENTS
        );
        let over_count = at_count_limit
            .into_iter()
            .chain(std::iter::once(OsString::from("extra")));
        assert!(matches!(
            capture_child_arguments(&executable, over_count),
            Err(CliError::ArgumentLimitExceeded)
        ));

        let exact_bytes = OsString::from("x".repeat(MAX_EXECUTABLE_AND_ARGUMENT_BYTES));
        assert!(capture_child_arguments(&exact_bytes, [])
            .unwrap()
            .is_empty());
        let over_bytes = OsString::from("x".repeat(MAX_EXECUTABLE_AND_ARGUMENT_BYTES + 1));
        assert!(matches!(
            capture_child_arguments(&over_bytes, []),
            Err(CliError::ArgumentLimitExceeded)
        ));
    }

    #[test]
    fn environment_capture_enforces_variable_and_name_value_byte_limits() {
        let at_count_limit = (0..MAX_ENVIRONMENT_VARIABLES)
            .map(|index| (OsString::from(format!("P{index}")), OsString::new()))
            .collect::<Vec<_>>();
        assert_eq!(
            capture_client_environment(at_count_limit.clone())
                .unwrap()
                .len(),
            MAX_ENVIRONMENT_VARIABLES
        );
        let over_count = at_count_limit
            .into_iter()
            .chain(std::iter::once((OsString::from("OVER"), OsString::new())));
        assert!(matches!(
            capture_client_environment(over_count),
            Err(CliError::EnvironmentLimitExceeded)
        ));

        let exactly_maximum_bytes = [(
            OsString::from("K"),
            OsString::from("x".repeat(MAX_ENVIRONMENT_BYTES - 1)),
        )];
        assert_eq!(
            capture_client_environment(exactly_maximum_bytes)
                .unwrap()
                .get(OsStr::new("K"))
                .unwrap()
                .as_os_str()
                .len(),
            MAX_ENVIRONMENT_BYTES - 1
        );
        let over_bytes = [(
            OsString::from("K"),
            OsString::from("x".repeat(MAX_ENVIRONMENT_BYTES)),
        )];
        assert!(matches!(
            capture_client_environment(over_bytes),
            Err(CliError::EnvironmentLimitExceeded)
        ));
    }

    #[test]
    fn final_environment_caps_include_owned_term_and_terminfo_values() {
        let at_count_limit = (0..MAX_ENVIRONMENT_VARIABLES - 2)
            .map(|index| (OsString::from(format!("P{index}")), OsString::new()))
            .collect::<Vec<_>>();
        let environment = capture_client_environment(at_count_limit).unwrap();
        validate_final_environment_profile(&environment, OsStr::new("/ti")).unwrap();

        let over_count = (0..MAX_ENVIRONMENT_VARIABLES - 1)
            .map(|index| (OsString::from(format!("P{index}")), OsString::new()))
            .collect::<Vec<_>>();
        let environment = capture_client_environment(over_count).unwrap();
        assert!(matches!(
            validate_final_environment_profile(&environment, OsStr::new("/ti")),
            Err(CliError::EnvironmentLimitExceeded)
        ));

        let owned_bytes = os_str_byte_len(OsStr::new("TERM"))
            + os_str_byte_len(OsStr::new("panelink-probe"))
            + os_str_byte_len(OsStr::new("TERMINFO"))
            + os_str_byte_len(OsStr::new("/ti"));
        let exact_value_bytes = MAX_ENVIRONMENT_BYTES - owned_bytes - 1;
        let environment = capture_client_environment([(
            OsString::from("K"),
            OsString::from("x".repeat(exact_value_bytes)),
        )])
        .unwrap();
        validate_final_environment_profile(&environment, OsStr::new("/ti")).unwrap();

        let environment = capture_client_environment([(
            OsString::from("K"),
            OsString::from("x".repeat(exact_value_bytes + 1)),
        )])
        .unwrap();
        assert!(matches!(
            validate_final_environment_profile(&environment, OsStr::new("/ti")),
            Err(CliError::EnvironmentLimitExceeded)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn launch_resolution_uses_explicit_cwd_and_owned_terminfo_without_spawning() {
        let context = LaunchContext {
            working_directory: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            environment: [
                (OsString::from("TERM"), OsString::from("untrusted-term")),
                (
                    OsString::from("TERMINFO"),
                    OsString::from("untrusted-terminfo"),
                ),
            ]
            .into_iter()
            .collect(),
        };
        let terminfo_directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/terminfo");
        let (launch, environment) =
            resolve_probe_launch(&terminfo_directory, OsStr::new("/usr/bin/env"), &context)
                .unwrap();

        assert_eq!(launch.working_directory, context.working_directory);
        assert_eq!(launch.requested_path, PathBuf::from("/usr/bin/env"));
        assert_eq!(
            environment.get(OsStr::new("TERM")),
            Some(&OsString::from("panelink-probe"))
        );
        assert_eq!(
            environment.get(OsStr::new("TERMINFO")),
            Some(&terminfo_directory.into_os_string())
        );
    }
}
