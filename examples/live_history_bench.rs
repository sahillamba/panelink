//! Synthetic sustained-output observations through the public live history path.
//!
//! The parent process prints one sanitized JSON object. Each case owns a fresh
//! private fixture; the child mode is only an internal PTY workload.

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod benchmark {
    use panelink::control_protocol::TargetRef;
    use panelink::handshake_protocol::{decode_welcome, Hello, CAPABILITY_INPUT, CAPABILITY_READ};
    use panelink::history_diagnostics::{HistoryDiagnosticSnapshot, HistoryDiagnostics};
    use panelink::history_rows;
    use panelink::history_store::HistoryStore;
    use panelink::launch::LaunchContext;
    use panelink::managed_launch_protocol::{EnvironmentEntry, LaunchRequest};
    use panelink::multi_runtime::MultiServerDriver;
    use panelink::multi_startup::{StartedMultiServer, StartupLaunch};
    use panelink::pane_layout::SplitDirection;
    use panelink::protocol::{OperationKey, OperationStatus, Outcome};
    use panelink::protocol_wire::{Frame, MessageKind};
    use panelink::response_protocol::{decode_response_payload, ResponseBody};
    use panelink::screen_checkpoint::{self, ScreenBuffer};
    use panelink::server_storage::{MaintenanceLock, ServerStorage};
    use panelink::session_protocol::{
        decode_session_info, encode_session_hello, encode_session_request, SessionHello,
        SessionRequest,
    };
    use panelink::split_protocol::{decode_split_result, encode_split_request, SplitRequest};
    use panelink::terminal::Key;
    use panelink::transport::AuthenticatedStream;
    use panelink::ui_protocol::{
        decode_layout_reply, decode_screen_reply, encode_attach_target, encode_key_input,
        encode_layout_query, encode_screen_query, planned_split_size, KeyInput,
    };
    use std::ffi::{OsStr, OsString};
    use std::fmt::Write as FmtWrite;
    use std::fs::{self, DirBuilder, File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::process;
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    const CHILD_MODE: &str = "--panelink-live-history-child";
    const CASE_TIMEOUT: Duration = Duration::from_secs(30);
    const IO_TIMEOUT: Duration = Duration::from_secs(3);
    const ACK_TIMEOUT: Duration = Duration::from_secs(6);
    const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);
    const PUMP_SLEEP: Duration = Duration::from_millis(1);
    const MAX_OWNER_TICKS: usize = 100_000;
    const MAX_RETAINED_ROWS: usize = 1_024;
    const MAX_SEGMENT_BYTES: u64 = 8 * 1024 * 1024;
    const PROBES_PER_PANE: usize = 8;

    type BenchResult<T> = Result<T, BenchError>;

    #[derive(Clone, Copy)]
    pub struct BenchError(&'static str);

    impl std::fmt::Display for BenchError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str(self.0)
        }
    }

    impl std::fmt::Debug for BenchError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str(self.0)
        }
    }

    fn checked<T, E>(result: Result<T, E>, context: &'static str) -> BenchResult<T> {
        result.map_err(|_| BenchError(context))
    }

    #[derive(Clone, Copy)]
    struct CaseSpec {
        name: &'static str,
        columns: u16,
        planned_rows: u64,
        batch_rows: usize,
        delay_ms: u64,
        shutdown: bool,
    }

    const CASES: [CaseSpec; 3] = [
        CaseSpec {
            name: "paced_80",
            columns: 80,
            planned_rows: 768,
            batch_rows: 8,
            delay_ms: 20,
            shutdown: false,
        },
        CaseSpec {
            name: "burst_240",
            columns: 240,
            planned_rows: 20_000,
            batch_rows: 64,
            delay_ms: 0,
            shutdown: false,
        },
        CaseSpec {
            name: "shutdown_240",
            columns: 240,
            planned_rows: 100_000,
            batch_rows: 32,
            delay_ms: 1,
            shutdown: true,
        },
    ];

    struct PrivateRoot {
        path: PathBuf,
    }

    impl PrivateRoot {
        fn create() -> BenchResult<Self> {
            let base = checked(
                fs::canonicalize(std::env::temp_dir()),
                "temporary root unavailable",
            )?;
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            for attempt in 0..64_u32 {
                let candidate = base.join(format!(
                    "panelink-live-history-{}-{stamp:x}-{attempt:x}",
                    process::id()
                ));
                let mut builder = DirBuilder::new();
                builder.mode(0o700);
                match builder.create(&candidate) {
                    Ok(()) => {
                        checked(
                            fs::set_permissions(&candidate, fs::Permissions::from_mode(0o700)),
                            "private temporary directory setup failed",
                        )?;
                        let canonical = checked(
                            fs::canonicalize(&candidate),
                            "private temporary directory setup failed",
                        )?;
                        let metadata = checked(
                            fs::symlink_metadata(&canonical),
                            "private temporary directory setup failed",
                        )?;
                        if canonical != candidate
                            || !metadata.is_dir()
                            || metadata.mode() & 0o7777 != 0o700
                        {
                            return Err(BenchError(
                                "private temporary directory validation failed",
                            ));
                        }
                        return Ok(Self { path: canonical });
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(_) => return Err(BenchError("private temporary directory setup failed")),
                }
            }
            Err(BenchError(
                "could not allocate a unique private temporary directory",
            ))
        }

        fn create_case(&self, name: &str) -> BenchResult<PathBuf> {
            let candidate = self.path.join(name);
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            checked(
                builder.create(&candidate),
                "private case directory setup failed",
            )?;
            checked(
                fs::set_permissions(&candidate, fs::Permissions::from_mode(0o700)),
                "private case directory setup failed",
            )?;
            let canonical = checked(
                fs::canonicalize(&candidate),
                "private case directory setup failed",
            )?;
            let metadata = checked(
                fs::symlink_metadata(&canonical),
                "private case directory setup failed",
            )?;
            if canonical != candidate || metadata.mode() & 0o7777 != 0o700 {
                return Err(BenchError("private case directory validation failed"));
            }
            Ok(canonical)
        }

        fn remove(&self) -> BenchResult<()> {
            checked(
                fs::remove_dir(&self.path),
                "private temporary root cleanup failed",
            )
        }
    }

    impl Drop for PrivateRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir(&self.path);
        }
    }

    struct CasePaths {
        root: PathBuf,
        server: PathBuf,
        history: PathBuf,
        terminfo: PathBuf,
        signals: PathBuf,
        start: PathBuf,
        ready: [PathBuf; 2],
        done: [PathBuf; 2],
        ack: [PathBuf; 2],
    }

    impl CasePaths {
        fn create(root: PathBuf) -> BenchResult<Self> {
            let server = root.join("server");
            let history = server.join("history");
            let terminfo = root.join("terminfo");
            let signals = root.join("signals");
            for path in [&server, &terminfo, &signals] {
                let mut builder = DirBuilder::new();
                builder.mode(0o700);
                checked(builder.create(path), "case storage setup failed")?;
                checked(
                    fs::set_permissions(path, fs::Permissions::from_mode(0o700)),
                    "case storage setup failed",
                )?;
            }
            let ready = [signals.join("pane-0.ready"), signals.join("pane-1.ready")];
            let done = [signals.join("pane-0.done"), signals.join("pane-1.done")];
            let ack = [signals.join("pane-0.ack"), signals.join("pane-1.ack")];
            Ok(Self {
                root,
                server,
                history,
                terminfo,
                signals: signals.clone(),
                start: signals.join("start"),
                ready,
                done,
                ack,
            })
        }
    }

    fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("invalid private marker name"))?;
        let mut temporary_name = name.to_os_string();
        temporary_name.push(".pending");
        let temporary = path.with_file_name(temporary_name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(bytes)?;
        drop(file);
        fs::rename(temporary, path)
    }

    fn marker_value(path: &Path) -> BenchResult<Option<u64>> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(BenchError("private workload marker read failed")),
        };
        let mut bytes = Vec::with_capacity(32);
        checked(
            file.take(33).read_to_end(&mut bytes),
            "private workload marker read failed",
        )?;
        if bytes.is_empty() || bytes.len() > 32 {
            return Err(BenchError("private workload marker is invalid"));
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| BenchError("private workload marker is invalid"))?;
        let digits = text.strip_suffix('\n').unwrap_or(text);
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(BenchError("private workload marker is invalid"));
        }
        let value = digits
            .parse::<u64>()
            .map_err(|_| BenchError("private workload marker is invalid"))?;
        Ok(Some(value))
    }

    fn child_mode(arguments: &[OsString]) -> BenchResult<()> {
        if arguments.len() != 7 || arguments[1] != OsStr::new(CHILD_MODE) {
            return Err(BenchError("internal child invocation is invalid"));
        }
        let root = PathBuf::from(&arguments[2]);
        let pane = parse_child_number(&arguments[3], 1)? as usize;
        let planned_rows = parse_child_number(&arguments[4], 100_000)? as usize;
        let batch_rows = parse_child_number(&arguments[5], 64)? as usize;
        let delay_ms = parse_child_number(&arguments[6], 20)?;
        if pane > 1 || planned_rows == 0 || batch_rows == 0 {
            return Err(BenchError("internal child invocation is invalid"));
        }
        run_child(&root, pane, planned_rows, batch_rows, delay_ms)
    }

    fn parse_child_number(value: &OsStr, maximum: u64) -> BenchResult<u64> {
        let text = value
            .to_str()
            .ok_or(BenchError("internal child invocation is invalid"))?;
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(BenchError("internal child invocation is invalid"));
        }
        let parsed = text
            .parse::<u64>()
            .map_err(|_| BenchError("internal child invocation is invalid"))?;
        if parsed > maximum {
            return Err(BenchError("internal child invocation is invalid"));
        }
        Ok(parsed)
    }

    fn run_child(
        root: &Path,
        pane: usize,
        rows: usize,
        batch: usize,
        delay_ms: u64,
    ) -> BenchResult<()> {
        let signals = root.join("signals");
        let ready = signals.join(format!("pane-{pane}.ready"));
        let done = signals.join(format!("pane-{pane}.done"));
        let ack = signals.join(format!("pane-{pane}.ack"));
        let start = signals.join("start");
        let reader_ack = ack.clone();
        let reader = thread::Builder::new()
            .name(format!("synthetic-input-{pane}"))
            .spawn(move || read_child_inputs(reader_ack, PROBES_PER_PANE))
            .map_err(|_| BenchError("synthetic child input reader could not start"))?;
        {
            let mut stdout = io::stdout().lock();
            stdout
                .write_all(b"PANELINK-SYNTHETIC-READY\r\n")
                .and_then(|()| stdout.flush())
                .map_err(|_| BenchError("synthetic child readiness write failed"))?;
        }
        checked(
            atomic_write(&ready, b"1\n"),
            "synthetic child readiness write failed",
        )?;
        let gate_deadline = Instant::now() + Duration::from_secs(20);
        while !start.exists() {
            if Instant::now() >= gate_deadline {
                return Err(BenchError("synthetic child start gate timed out"));
            }
            thread::sleep(PUMP_SLEEP);
        }
        let mut stdout = io::stdout().lock();
        let mut produced = 0_usize;
        while produced < rows {
            let limit = (produced + batch).min(rows);
            while produced < limit {
                writeln!(
                    stdout,
                    "P{pane}-ROW-{produced:06}-ABCDEFGHIJKLMNOPQRSTUVWXYZ"
                )
                .map_err(|_| BenchError("synthetic child output failed"))?;
                produced += 1;
            }
            stdout
                .flush()
                .map_err(|_| BenchError("synthetic child output failed"))?;
            if delay_ms != 0 && produced < rows {
                thread::sleep(Duration::from_millis(delay_ms));
            }
        }
        drop(stdout);
        checked(
            atomic_write(&done, format!("{produced}\n").as_bytes()),
            "synthetic child completion write failed",
        )?;
        reader
            .join()
            .map_err(|_| BenchError("synthetic child input reader failed"))??;
        Ok(())
    }

    fn read_child_inputs(path: PathBuf, count: usize) -> BenchResult<()> {
        let mut stdin = io::stdin().lock();
        for completed in 1..=count {
            let mut input = [0_u8; 2];
            stdin
                .read_exact(&mut input)
                .map_err(|_| BenchError("synthetic child input was not acknowledged"))?;
            if input != *b"q\n" && input != *b"q\r" {
                return Err(BenchError("synthetic child received unexpected input"));
            }
            checked(
                atomic_write(&path, format!("{completed}\n").as_bytes()),
                "synthetic child acknowledgment write failed",
            )?;
        }
        Ok(())
    }

    #[derive(Clone, Copy)]
    struct ProbeResult {
        pane: usize,
        control_rtt_us: u64,
        input_reply_us: u64,
        child_ack_us: u64,
        started_during_output: bool,
        ack_before_output_done: bool,
    }

    struct ProbeCommand {
        probe_index: usize,
        done_path: PathBuf,
        ack_path: PathBuf,
        reply: mpsc::Sender<BenchResult<ProbeResult>>,
    }

    enum ClientCommand {
        Probe(ProbeCommand),
        Stop,
    }

    #[derive(Clone, Copy)]
    struct ClientSetup {
        pane: usize,
        target: TargetRef,
        split_target: Option<TargetRef>,
    }

    struct ClientHandle {
        command: SyncSender<ClientCommand>,
        setup: Receiver<BenchResult<ClientSetup>>,
        worker_done: Receiver<()>,
        join: Option<JoinHandle<()>>,
    }

    fn spawn_root_client(
        driver: &mut MultiServerDriver,
        root_target: TargetRef,
        spec: CaseSpec,
        paths: CasePaths,
        current_exe: PathBuf,
    ) -> BenchResult<ClientHandle> {
        let (server_socket, client_socket) =
            checked(UnixStream::pair(), "control socketpair creation failed")?;
        checked(
            driver.attach(checked(
                AuthenticatedStream::new(server_socket),
                "control peer authentication failed",
            )?),
            "control client attachment failed",
        )?;
        spawn_client_worker(0, client_socket, move |stream| {
            setup_root_client(stream, root_target, spec, &paths, &current_exe)
        })
    }

    fn spawn_child_client(
        driver: &mut MultiServerDriver,
        target: TargetRef,
        columns: u16,
        _cwd: &Path,
    ) -> BenchResult<ClientHandle> {
        let (server_socket, client_socket) =
            checked(UnixStream::pair(), "control socketpair creation failed")?;
        checked(
            driver.attach(checked(
                AuthenticatedStream::new(server_socket),
                "control peer authentication failed",
            )?),
            "control client attachment failed",
        )?;
        spawn_client_worker(1, client_socket, move |stream| {
            setup_child_client(stream, target, columns)
        })
    }

    fn spawn_client_worker<F>(
        pane: usize,
        socket: UnixStream,
        setup_fn: F,
    ) -> BenchResult<ClientHandle>
    where
        F: FnOnce(AuthenticatedStream) -> BenchResult<(ClientSetup, ClientProtocol)>
            + Send
            + 'static,
    {
        let (command_tx, command_rx) = mpsc::sync_channel(1);
        let (setup_tx, setup_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name(format!("history-bench-client-{pane}"))
            .spawn(move || {
                let stream = match AuthenticatedStream::new(socket) {
                    Ok(stream) => stream,
                    Err(_) => {
                        let _ =
                            setup_tx.send(Err(BenchError("control peer authentication failed")));
                        let _ = done_tx.send(());
                        return;
                    }
                };
                let setup = setup_fn(stream);
                match setup {
                    Ok((info, protocol)) => {
                        if setup_tx.send(Ok(info)).is_ok() {
                            service_client(protocol, info, command_rx);
                        }
                    }
                    Err(error) => {
                        let _ = setup_tx.send(Err(error));
                    }
                }
                let _ = done_tx.send(());
            })
            .map_err(|_| BenchError("control client worker could not start"))?;
        Ok(ClientHandle {
            command: command_tx,
            setup: setup_rx,
            worker_done: done_rx,
            join: Some(join),
        })
    }

    fn setup_root_client(
        stream: AuthenticatedStream,
        root_target: TargetRef,
        spec: CaseSpec,
        paths: &CasePaths,
        current_exe: &Path,
    ) -> BenchResult<(ClientSetup, ClientProtocol)> {
        let mut client = begin_client(stream, 100 + spec.columns as u64)?;
        let info = describe(&mut client.stream, &mut client.request_id)?;
        if info.target != root_target {
            return Err(BenchError("root control target did not match startup"));
        }
        claim(&mut client, info.owner_revision, root_target, 1)?;
        let layout = query_layout(&mut client)?;
        verify_root_layout(&layout, spec.columns, root_target)?;
        let (rows, columns) = checked(
            planned_split_size(&layout, root_target, SplitDirection::Down),
            "root split dimensions are invalid",
        )?;
        if rows != 24 || columns != spec.columns {
            return Err(BenchError(
                "root split dimensions do not match the workload",
            ));
        }
        let split_sequence = 2;
        let split_key = OperationKey {
            generation: client.generation,
            lease: client.lease,
            sequence: split_sequence,
        };
        let root_path = paths.root.as_os_str().as_bytes();
        let current_exe_bytes = current_exe.as_os_str().as_bytes();
        let pane_arg = OsString::from("1");
        let rows_arg = OsString::from(spec.planned_rows.to_string());
        let batch_arg = OsString::from(spec.batch_rows.to_string());
        let delay_arg = OsString::from(spec.delay_ms.to_string());
        let arguments = [
            OsStr::new(CHILD_MODE).as_bytes(),
            root_path,
            pane_arg.as_bytes(),
            rows_arg.as_bytes(),
            batch_arg.as_bytes(),
            delay_arg.as_bytes(),
        ];
        let environment = [
            EnvironmentEntry {
                name: b"PATH",
                value: b"/usr/bin:/bin",
            },
            EnvironmentEntry {
                name: b"HOME",
                value: root_path,
            },
        ];
        let launch = LaunchRequest {
            key: split_key,
            rows,
            columns,
            requested_executable: current_exe_bytes,
            requested_working_directory: root_path,
            client_working_directory: root_path,
            arguments: &arguments,
            environment: &environment,
        };
        let mut split_payload = Vec::new();
        checked(
            encode_split_request(
                &SplitRequest {
                    key: split_key,
                    target: root_target,
                    expected_layout_revision: layout.revision,
                    direction: SplitDirection::Down,
                    launch: &launch,
                },
                &mut split_payload,
            ),
            "schema-7 split request encoding failed",
        )?;
        let split_reply = send_request(&mut client.stream, &mut client.request_id, &split_payload)?;
        if split_reply.kind() != MessageKind::Response {
            return Err(BenchError("schema-7 split response kind is invalid"));
        }
        let split = checked(
            decode_split_result(split_reply.payload()),
            "schema-7 split response is invalid",
        )?;
        if split.outcome != Outcome::Applied {
            return Err(BenchError("schema-7 split was not applied"));
        }
        let split_target = split
            .new_target
            .ok_or(BenchError("schema-7 split did not return a target"))?;
        client.next_sequence = split_sequence + 1;
        let final_layout = query_layout(&mut client)?;
        verify_layout(&final_layout, spec.columns)?;
        if !final_layout
            .panes
            .iter()
            .flatten()
            .any(|pane| pane.target == split_target && pane.rect.rows == 24)
        {
            return Err(BenchError("split target was absent from the live layout"));
        }
        query_screen(&mut client, root_target, 0, 24, spec.columns)?;
        Ok((
            ClientSetup {
                pane: 0,
                target: root_target,
                split_target: Some(split_target),
            },
            client,
        ))
    }

    fn setup_child_client(
        stream: AuthenticatedStream,
        target: TargetRef,
        columns: u16,
    ) -> BenchResult<(ClientSetup, ClientProtocol)> {
        let mut client = begin_client_for_target(stream, target, 202)?;
        let info = describe(&mut client.stream, &mut client.request_id)?;
        if info.target != target {
            return Err(BenchError("split control target did not match admission"));
        }
        claim(&mut client, info.owner_revision, target, 1)?;
        let layout = query_layout(&mut client)?;
        verify_layout(&layout, columns)?;
        if !layout
            .panes
            .iter()
            .flatten()
            .any(|pane| pane.target == target && pane.rect.rows == 24)
        {
            return Err(BenchError("split target dimensions are invalid"));
        }
        query_screen(&mut client, target, 1, 24, columns)?;
        Ok((
            ClientSetup {
                pane: 1,
                target,
                split_target: None,
            },
            client,
        ))
    }

    struct ClientProtocol {
        stream: AuthenticatedStream,
        generation: u64,
        lease: u64,
        target: TargetRef,
        owner_revision: u64,
        next_sequence: u64,
        request_id: u64,
    }

    fn begin_client(stream: AuthenticatedStream, nonce: u64) -> BenchResult<ClientProtocol> {
        begin_client_inner(stream, nonce, None)
    }

    fn begin_client_for_target(
        stream: AuthenticatedStream,
        target: TargetRef,
        nonce: u64,
    ) -> BenchResult<ClientProtocol> {
        begin_client_inner(stream, nonce, Some(target))
    }

    fn begin_client_inner(
        mut stream: AuthenticatedStream,
        nonce: u64,
        target: Option<TargetRef>,
    ) -> BenchResult<ClientProtocol> {
        let mut hello = checked(
            encode_session_hello(&SessionHello {
                hello: Hello {
                    nonce,
                    required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
                    optional_capabilities: 0,
                },
                resume: None,
            }),
            "control hello encoding failed",
        )?;
        if let Some(target) = target {
            hello.extend_from_slice(&checked(
                encode_attach_target(target),
                "target attachment encoding failed",
            )?);
        }
        checked(
            stream.write_frame(
                &checked(
                    Frame::new(MessageKind::Hello, 0, &hello),
                    "control hello framing failed",
                )?,
                Instant::now() + IO_TIMEOUT,
            ),
            "control hello write failed",
        )?;
        let welcome_frame = checked(
            stream.read_frame(Instant::now() + IO_TIMEOUT),
            "control welcome read failed",
        )?;
        if welcome_frame.kind() != MessageKind::Welcome {
            return Err(BenchError("control welcome kind is invalid"));
        }
        let welcome = checked(
            decode_welcome(welcome_frame.payload()),
            "control welcome is invalid",
        )?;
        checked(
            stream.enable_snapshot_reads(),
            "control snapshot reads could not be enabled",
        )?;
        Ok(ClientProtocol {
            stream,
            generation: welcome.generation,
            lease: welcome.lease,
            target: target.unwrap_or(TargetRef {
                generation: welcome.generation,
                workspace: 1,
                pane: 1,
                session: 1,
                occupant_revision: 1,
            }),
            owner_revision: 1,
            next_sequence: 1,
            request_id: 0,
        })
    }

    fn send_request(
        stream: &mut AuthenticatedStream,
        request_id: &mut u64,
        payload: &[u8],
    ) -> BenchResult<panelink::transport::OwnedFrame> {
        *request_id = request_id
            .checked_add(1)
            .ok_or(BenchError("control request sequence exhausted"))?;
        let frame = checked(
            Frame::new(MessageKind::Request, *request_id, payload),
            "control request framing failed",
        )?;
        checked(
            stream.write_frame(&frame, Instant::now() + IO_TIMEOUT),
            "control request write failed",
        )?;
        let reply = checked(
            stream.read_frame(Instant::now() + IO_TIMEOUT),
            "control response read failed",
        )?;
        if reply.request_id() != *request_id {
            return Err(BenchError("control response identifier did not match"));
        }
        Ok(reply)
    }

    fn describe(
        stream: &mut AuthenticatedStream,
        request_id: &mut u64,
    ) -> BenchResult<panelink::session_protocol::SessionInfo> {
        let payload = checked(
            encode_session_request(&SessionRequest::Describe),
            "session description encoding failed",
        )?;
        let reply = send_request(stream, request_id, &payload)?;
        if reply.kind() != MessageKind::Response {
            return Err(BenchError("session description response kind is invalid"));
        }
        checked(
            decode_session_info(reply.payload()),
            "session description is invalid",
        )
    }

    fn claim(
        client: &mut ClientProtocol,
        owner_revision: u64,
        target: TargetRef,
        sequence: u64,
    ) -> BenchResult<()> {
        let key = OperationKey {
            generation: client.generation,
            lease: client.lease,
            sequence,
        };
        let payload = checked(
            encode_session_request(&SessionRequest::Claim {
                key,
                target,
                expected_owner_revision: owner_revision,
                ttl_seconds: 30,
            }),
            "input lease request encoding failed",
        )?;
        let reply = send_request(&mut client.stream, &mut client.request_id, &payload)?;
        if reply.kind() != MessageKind::Response {
            return Err(BenchError("input lease response kind is invalid"));
        }
        let response = checked(
            decode_response_payload(reply.payload()),
            "input lease response is invalid",
        )?;
        if !matches!(
            response.body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        ) {
            return Err(BenchError("input lease was not applied"));
        }
        let info = describe(&mut client.stream, &mut client.request_id)?;
        if info.target != target || info.owner_lease != client.lease {
            return Err(BenchError("input lease was not confirmed"));
        }
        client.target = target;
        client.owner_revision = info.owner_revision;
        client.next_sequence = sequence + 1;
        Ok(())
    }

    fn query_layout(
        client: &mut ClientProtocol,
    ) -> BenchResult<panelink::ui_protocol::LayoutReply> {
        let payload = encode_layout_query();
        let reply = send_request(&mut client.stream, &mut client.request_id, &payload)?;
        if reply.kind() != MessageKind::Response {
            return Err(BenchError("layout response kind is invalid"));
        }
        checked(
            decode_layout_reply(reply.payload()),
            "layout response is invalid",
        )
    }

    fn query_screen(
        client: &mut ClientProtocol,
        target: TargetRef,
        pane: usize,
        expected_rows: u16,
        expected_columns: u16,
    ) -> BenchResult<()> {
        let payload = checked(encode_screen_query(target), "screen query encoding failed")?;
        let reply = send_request(&mut client.stream, &mut client.request_id, &payload)?;
        if reply.kind() != MessageKind::Snapshot {
            return Err(BenchError("screen query response kind is invalid"));
        }
        let view = checked(
            decode_screen_reply(reply.payload()),
            "screen query response is invalid",
        )?;
        if view.target() != target {
            return Err(BenchError("screen query target did not match"));
        }
        let checkpoint = checked(
            screen_checkpoint::decode(view.checkpoint()),
            "live screen checkpoint is invalid",
        )?;
        if checkpoint.metadata().target() != target
            || checkpoint.rows() != expected_rows
            || checkpoint.columns() != expected_columns
            || checkpoint_contains_sibling_prefix(&checkpoint, pane)
        {
            if checkpoint.rows() != expected_rows || checkpoint.columns() != expected_columns {
                eprintln!(
                    "live PTY screen geometry mismatch: pane {pane} is {}x{}, expected {expected_rows}x{expected_columns}",
                    checkpoint.rows(),
                    checkpoint.columns()
                );
            }
            return Err(BenchError(
                "live PTY screen geometry or target content is invalid",
            ));
        }
        Ok(())
    }

    fn sibling_row_prefix(pane: usize) -> &'static str {
        if pane == 0 {
            "P1-ROW-"
        } else {
            "P0-ROW-"
        }
    }

    fn checkpoint_contains_sibling_prefix(
        checkpoint: &screen_checkpoint::ScreenCheckpoint<'_>,
        pane: usize,
    ) -> bool {
        let prefix = sibling_row_prefix(pane);
        for row in 0..checkpoint.rows() {
            let mut text = String::new();
            for column in 0..checkpoint.columns() {
                if let Some(cell) = checkpoint.cell(row, column) {
                    text.push_str(cell.text());
                }
            }
            if text.contains(prefix) {
                return true;
            }
        }
        false
    }

    fn saved_row_contains_sibling_prefix(row: &history_rows::SavedRow<'_>, pane: usize) -> bool {
        let mut text = String::new();
        for column in 0..row.width {
            text.push_str(row.cell_text(column).unwrap_or(""));
        }
        text.contains(sibling_row_prefix(pane))
    }

    fn verify_layout(layout: &panelink::ui_protocol::LayoutReply, columns: u16) -> BenchResult<()> {
        if layout.rows != 49 || layout.columns != columns {
            return Err(BenchError("root layout dimensions are invalid"));
        }
        let panes: Vec<_> = layout.panes.iter().flatten().collect();
        if panes.len() != 2
            || panes
                .iter()
                .any(|pane| pane.rect.rows != 24 || pane.rect.columns != columns)
        {
            return Err(BenchError("two-pane layout dimensions are invalid"));
        }
        Ok(())
    }

    fn verify_root_layout(
        layout: &panelink::ui_protocol::LayoutReply,
        columns: u16,
        target: TargetRef,
    ) -> BenchResult<()> {
        if layout.rows != 49
            || layout.columns != columns
            || layout.panes.iter().flatten().count() != 1
            || !layout.panes.iter().flatten().any(|pane| {
                pane.target == target && pane.rect.rows == 49 && pane.rect.columns == columns
            })
        {
            return Err(BenchError("initial root layout dimensions are invalid"));
        }
        Ok(())
    }

    fn service_client(
        mut protocol: ClientProtocol,
        setup: ClientSetup,
        commands: Receiver<ClientCommand>,
    ) {
        loop {
            match commands.recv() {
                Ok(ClientCommand::Stop) | Err(_) => break,
                Ok(ClientCommand::Probe(command)) => {
                    let result = execute_probe(
                        &mut protocol,
                        setup.pane,
                        command.probe_index,
                        &command.done_path,
                        &command.ack_path,
                    );
                    let _ = command.reply.send(result);
                }
            }
        }
        drop(protocol.stream);
    }

    fn execute_probe(
        client: &mut ClientProtocol,
        pane: usize,
        probe_index: usize,
        done_path: &Path,
        ack_path: &Path,
    ) -> BenchResult<ProbeResult> {
        let control_started = Instant::now();
        let layout = query_layout(client)?;
        if !layout
            .panes
            .iter()
            .flatten()
            .any(|entry| entry.target == client.target)
        {
            return Err(BenchError("probe layout lost its target"));
        }
        let control_rtt_us = elapsed_us(control_started.elapsed());
        let started_during_output = marker_value(done_path)?.is_none();
        let input_key = OperationKey {
            generation: client.generation,
            lease: client.lease,
            sequence: client.next_sequence,
        };
        client.next_sequence = client
            .next_sequence
            .checked_add(1)
            .ok_or(BenchError("input operation sequence exhausted"))?;
        let keys = [Key::Char('q'), Key::Enter];
        let payload = checked(
            encode_key_input(&KeyInput {
                key: input_key,
                target: client.target,
                expected_owner_revision: client.owner_revision,
                keys: &keys,
            }),
            "synthetic input encoding failed",
        )?;
        let input_started = Instant::now();
        let reply = send_request(&mut client.stream, &mut client.request_id, &payload)?;
        let input_reply_us = elapsed_us(input_started.elapsed());
        if reply.kind() != MessageKind::Response {
            return Err(BenchError("synthetic input response kind is invalid"));
        }
        let response = checked(
            decode_response_payload(reply.payload()),
            "synthetic input response is invalid",
        )?;
        if !matches!(
            response.body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        ) {
            return Err(BenchError("synthetic input was not applied"));
        }
        let ack_deadline = input_started + ACK_TIMEOUT;
        loop {
            if marker_value(ack_path)?.is_some_and(|value| value >= probe_index as u64) {
                let ack_before_output_done = marker_value(done_path)?.is_none();
                return Ok(ProbeResult {
                    pane,
                    control_rtt_us,
                    input_reply_us,
                    child_ack_us: elapsed_us(input_started.elapsed()),
                    started_during_output,
                    ack_before_output_done,
                });
            }
            if Instant::now() >= ack_deadline {
                return Err(BenchError("synthetic child acknowledgment timed out"));
            }
            thread::sleep(PUMP_SLEEP);
        }
    }

    fn elapsed_us(duration: Duration) -> u64 {
        u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
    }

    #[derive(Clone, Copy)]
    struct TargetReport {
        pane: usize,
        retained_rows: usize,
        segment_bytes: u64,
        first_ordinal: Option<u64>,
        last_ordinal: Option<u64>,
        gap: bool,
        ended: bool,
        normal_checkpoint: bool,
        alternate_checkpoint: bool,
        producer_completed: bool,
        completed_rows: Option<u64>,
    }

    struct CaseReport {
        spec: CaseSpec,
        elapsed_us: u64,
        owner_tick_us: Vec<u64>,
        probes: Vec<ProbeResult>,
        driver_drop_us: u64,
        post_drop_lock_wait_us: u64,
        history_failure: bool,
        history_diagnostics: HistoryDiagnosticSnapshot,
        shutdown_during_output: bool,
        targets: [TargetReport; 2],
    }

    struct LoadResult {
        targets: [TargetRef; 2],
        owner_tick_us: Vec<u64>,
        probes: Vec<ProbeResult>,
        shutdown_during_output: bool,
    }

    struct CaseHarness {
        paths: CasePaths,
        driver: Option<MultiServerDriver>,
        workers: Vec<ClientHandle>,
        history_failure: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
        history_diagnostics: Option<HistoryDiagnostics>,
        driver_created: bool,
        case_started: Instant,
    }

    impl CaseHarness {
        fn new(root: &PrivateRoot, spec: CaseSpec) -> BenchResult<Self> {
            let case_root = root.create_case(spec.name)?;
            let paths = match CasePaths::create(case_root.clone()) {
                Ok(paths) => paths,
                Err(error) => {
                    let _ = fs::remove_dir_all(case_root);
                    return Err(error);
                }
            };
            Ok(Self {
                paths,
                driver: None,
                workers: Vec::new(),
                history_failure: None,
                history_diagnostics: None,
                driver_created: false,
                case_started: Instant::now(),
            })
        }

        fn run_load(&mut self, spec: CaseSpec) -> BenchResult<LoadResult> {
            let storage = checked(
                ServerStorage::initialize(&self.paths.server),
                "server storage initialization failed",
            )?;
            let mut history_builder = DirBuilder::new();
            history_builder.mode(0o700);
            checked(
                history_builder.create(&self.paths.history),
                "history directory setup failed",
            )?;
            checked(
                fs::set_permissions(&self.paths.history, fs::Permissions::from_mode(0o700)),
                "history directory setup failed",
            )?;
            let context = LaunchContext {
                working_directory: self.paths.root.clone(),
                environment: [
                    (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                    (
                        OsString::from("HOME"),
                        self.paths.root.as_os_str().to_owned(),
                    ),
                ]
                .into_iter()
                .collect(),
            };
            let exe = checked(
                std::env::current_exe(),
                "benchmark executable path lookup failed",
            )?;
            let root_arguments = child_arguments(&self.paths.root, 0, spec);
            let started = checked(
                StartedMultiServer::start(
                    storage,
                    &self.paths.terminfo,
                    &StartupLaunch {
                        executable: exe.as_os_str(),
                        arguments: &root_arguments,
                        context: &context,
                        rows: 49,
                        columns: spec.columns,
                    },
                    true,
                ),
                "root pane managed startup failed",
            )?;
            let root_target = started
                .startup()
                .target
                .ok_or(BenchError("root pane startup target is missing"))?;
            let driver = started.into_driver();
            self.history_diagnostics = Some(driver.history_diagnostics());
            self.driver_created = true;
            self.driver = Some(driver);
            let history = checked(
                HistoryStore::open(&self.paths.history),
                "history store open failed",
            )?;
            self.driver
                .as_mut()
                .expect("driver stored")
                .enable_history(history);
            if !self
                .driver
                .as_ref()
                .expect("driver stored")
                .history_available()
            {
                return Err(BenchError("live history writer was unavailable"));
            }
            self.history_failure = Some(
                self.driver
                    .as_ref()
                    .expect("driver stored")
                    .history_failure_signal(),
            );

            let current_exe = exe;
            let root_client = spawn_root_client(
                self.driver.as_mut().expect("driver stored"),
                root_target,
                spec,
                CasePaths {
                    root: self.paths.root.clone(),
                    server: self.paths.server.clone(),
                    history: self.paths.history.clone(),
                    terminfo: self.paths.terminfo.clone(),
                    signals: self.paths.signals.clone(),
                    start: self.paths.start.clone(),
                    ready: self.paths.ready.clone(),
                    done: self.paths.done.clone(),
                    ack: self.paths.ack.clone(),
                },
                current_exe,
            )?;
            self.workers.push(root_client);
            let deadline = self.case_started + CASE_TIMEOUT;
            let root_setup = self.wait_for_setup(0, deadline)?;
            let split_target = root_setup
                .split_target
                .ok_or(BenchError("schema-7 split target is missing"))?;
            let cwd = self.paths.root.clone();
            let child_client = spawn_child_client(
                self.driver.as_mut().expect("driver stored"),
                split_target,
                spec.columns,
                &cwd,
            )?;
            self.workers.push(child_client);
            let child_setup = self.wait_for_setup(1, deadline)?;
            if child_setup.target != split_target {
                return Err(BenchError("split client setup target did not match"));
            }
            self.wait_for_ready_markers(deadline)?;
            let start = Instant::now();
            checked(
                atomic_write(&self.paths.start, b"go\n"),
                "workload start gate failed",
            )?;
            let mut owner_tick_us = Vec::new();
            let mut probes = Vec::with_capacity(PROBES_PER_PANE * 2);
            for probe_index in 1..=PROBES_PER_PANE {
                let mut pending: Vec<(usize, Receiver<BenchResult<ProbeResult>>)> = Vec::new();
                for pane in 0..2 {
                    let (reply_tx, reply_rx) = mpsc::channel();
                    let command = ProbeCommand {
                        probe_index,
                        done_path: self.paths.done[pane].clone(),
                        ack_path: self.paths.ack[pane].clone(),
                        reply: reply_tx,
                    };
                    self.workers[pane]
                        .command
                        .send(ClientCommand::Probe(command))
                        .map_err(|_| BenchError("control probe worker stopped"))?;
                    pending.push((pane, reply_rx));
                }
                while !pending.is_empty() {
                    self.tick(true, &mut owner_tick_us)?;
                    let mut index = 0;
                    while index < pending.len() {
                        match pending[index].1.try_recv() {
                            Ok(result) => {
                                let (_, receiver) = pending.swap_remove(index);
                                drop(receiver);
                                probes.push(result?);
                            }
                            Err(mpsc::TryRecvError::Disconnected) => {
                                return Err(BenchError("control probe worker disconnected"));
                            }
                            Err(mpsc::TryRecvError::Empty) => index += 1,
                        }
                    }
                    self.check_deadline(deadline)?;
                    if !pending.is_empty() {
                        thread::sleep(PUMP_SLEEP);
                    }
                }
            }

            if spec.shutdown {
                while start.elapsed() < Duration::from_millis(500) {
                    self.tick(true, &mut owner_tick_us)?;
                    self.check_deadline(deadline)?;
                    thread::sleep(PUMP_SLEEP);
                }
                let shutdown_during_output = marker_value(&self.paths.done[0])?.is_none()
                    || marker_value(&self.paths.done[1])?.is_none();
                return Ok(LoadResult {
                    targets: [root_target, split_target],
                    owner_tick_us,
                    probes,
                    shutdown_during_output,
                });
            }

            loop {
                let done0 = marker_value(&self.paths.done[0])?;
                let done1 = marker_value(&self.paths.done[1])?;
                if done0 == Some(spec.planned_rows)
                    && done1 == Some(spec.planned_rows)
                    && self
                        .driver
                        .as_ref()
                        .is_some_and(|driver| driver.owner().all_sessions_ended())
                {
                    break;
                }
                self.tick(true, &mut owner_tick_us)?;
                self.check_deadline(deadline)?;
                thread::sleep(PUMP_SLEEP);
            }
            Ok(LoadResult {
                targets: [root_target, split_target],
                owner_tick_us,
                probes,
                shutdown_during_output: false,
            })
        }

        fn wait_for_setup(&mut self, index: usize, deadline: Instant) -> BenchResult<ClientSetup> {
            loop {
                self.tick(false, &mut Vec::new())?;
                match self.workers[index].setup.try_recv() {
                    Ok(setup) => return setup,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err(BenchError("control client setup worker disconnected"));
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                self.check_deadline(deadline)?;
                thread::sleep(PUMP_SLEEP);
            }
        }

        fn wait_for_ready_markers(&mut self, deadline: Instant) -> BenchResult<()> {
            loop {
                let all_ready = self.paths.ready.iter().all(|path| path.exists());
                if all_ready {
                    return Ok(());
                }
                self.tick(false, &mut Vec::new())?;
                self.check_deadline(deadline)?;
                thread::sleep(PUMP_SLEEP);
            }
        }

        fn tick(&mut self, measured: bool, samples: &mut Vec<u64>) -> BenchResult<()> {
            let driver = self
                .driver
                .as_mut()
                .ok_or(BenchError("live driver is unavailable"))?;
            let started = Instant::now();
            let logical_now = self.case_started.elapsed().as_secs().saturating_add(1);
            checked(driver.tick(|| logical_now), "owner tick failed")?;
            if measured {
                if samples.len() >= MAX_OWNER_TICKS {
                    return Err(BenchError("owner tick sample bound exceeded"));
                }
                samples.push(elapsed_us(started.elapsed()));
            }
            Ok(())
        }

        fn check_deadline(&self, deadline: Instant) -> BenchResult<()> {
            if Instant::now() >= deadline {
                Err(BenchError("case workload exceeded its bounded deadline"))
            } else {
                Ok(())
            }
        }

        fn finish(
            mut self,
            spec: CaseSpec,
            load: BenchResult<LoadResult>,
        ) -> BenchResult<CaseReport> {
            self.stop_workers();
            let drop_started = Instant::now();
            drop(self.driver.take());
            let driver_drop_us = elapsed_us(drop_started.elapsed());
            if !self.driver_created {
                let root = self.paths.root.clone();
                checked(
                    fs::remove_dir_all(&root),
                    "failed case fixture cleanup failed",
                )?;
                return match load {
                    Ok(_) => Err(BenchError("case driver was not initialized")),
                    Err(error) => Err(error),
                };
            }
            let settle_started = Instant::now();
            let settle_deadline =
                (self.case_started + CASE_TIMEOUT).min(settle_started + SETTLE_TIMEOUT);
            let lock = acquire_maintenance(&self.paths.server, settle_deadline)?;
            let post_drop_lock_wait_us = elapsed_us(settle_started.elapsed());
            let load = match load {
                Ok(load) => load,
                Err(error) => {
                    drop(lock);
                    checked(
                        fs::remove_dir_all(&self.paths.root),
                        "failed case fixture cleanup failed",
                    )?;
                    return Err(error);
                }
            };
            let validated = (|| {
                let store = checked(
                    HistoryStore::open(&self.paths.history),
                    "settled history store reopen failed",
                )?;
                let targets = inspect_targets(
                    &store,
                    &self.paths.history,
                    load.targets,
                    &self.paths.done,
                    spec,
                )?;
                let history_failure = self
                    .history_failure
                    .as_ref()
                    .is_some_and(|signal| signal.load(std::sync::atomic::Ordering::Acquire));
                let diagnostics = self
                    .history_diagnostics
                    .as_ref()
                    .ok_or(BenchError("history diagnostic observer missing"))?
                    .snapshot();
                Ok::<_, BenchError>((targets, history_failure, diagnostics))
            })();
            drop(lock);
            let cleanup = checked(
                fs::remove_dir_all(&self.paths.root),
                "private case cleanup failed",
            );
            let (targets, history_failure, history_diagnostics) = validated?;
            cleanup?;
            Ok(CaseReport {
                spec,
                elapsed_us: elapsed_us(self.case_started.elapsed()),
                owner_tick_us: load.owner_tick_us,
                probes: load.probes,
                driver_drop_us,
                post_drop_lock_wait_us,
                history_failure,
                history_diagnostics,
                shutdown_during_output: load.shutdown_during_output,
                targets,
            })
        }

        fn stop_workers(&mut self) {
            for worker in &self.workers {
                let _ = worker.command.try_send(ClientCommand::Stop);
            }
            let deadline = Instant::now() + IO_TIMEOUT + Duration::from_secs(1);
            for worker in &mut self.workers {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match worker.worker_done.recv_timeout(remaining) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                        if let Some(join) = worker.join.take() {
                            let _ = join.join();
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        // The stream has bounded I/O deadlines. Detaching avoids
                        // an unbounded join if a transport is already timing out.
                        worker.join.take();
                    }
                }
            }
        }
    }

    fn child_arguments(root: &Path, pane: usize, spec: CaseSpec) -> Vec<OsString> {
        vec![
            OsString::from(CHILD_MODE),
            root.as_os_str().to_owned(),
            OsString::from(pane.to_string()),
            OsString::from(spec.planned_rows.to_string()),
            OsString::from(spec.batch_rows.to_string()),
            OsString::from(spec.delay_ms.to_string()),
        ]
    }

    fn acquire_maintenance(path: &Path, deadline: Instant) -> BenchResult<MaintenanceLock> {
        loop {
            match ServerStorage::lock_for_maintenance(path) {
                Ok(lock) => return Ok(lock),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(BenchError(
                            "history writer lock did not settle before deadline",
                        ));
                    }
                    thread::sleep(PUMP_SLEEP);
                }
                Err(_) => return Err(BenchError("maintenance lock acquisition failed")),
            }
        }
    }

    fn inspect_targets(
        store: &HistoryStore,
        history_path: &Path,
        targets: [TargetRef; 2],
        done_paths: &[PathBuf; 2],
        spec: CaseSpec,
    ) -> BenchResult<[TargetReport; 2]> {
        let listed = checked(store.list_targets(), "saved target inventory is invalid")?;
        if listed.iter().any(|target| !targets.contains(target)) {
            return Err(BenchError("saved history target isolation failed"));
        }
        let mut reports = Vec::with_capacity(2);
        for pane in 0..2 {
            let target = targets[pane];
            let filename = format!(
                "{:016x}-{:016x}-{:016x}-{:016x}-{:016x}.rows",
                target.generation,
                target.workspace,
                target.pane,
                target.session,
                target.occupant_revision
            );
            let segment_path = history_path.join(filename);
            let (segment_bytes, saved) = match fs::symlink_metadata(&segment_path) {
                Ok(metadata) => {
                    if !metadata.is_file()
                        || metadata.mode() & 0o7777 != 0o600
                        || metadata.nlink() != 1
                        || metadata.len() > MAX_SEGMENT_BYTES
                    {
                        return Err(BenchError(
                            "saved history segment bound or safety check failed",
                        ));
                    }
                    (
                        metadata.len(),
                        checked(store.load_rows(target), "saved history row decode failed")?,
                    )
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => (0, None),
                Err(_) => return Err(BenchError("saved history segment metadata read failed")),
            };
            if saved
                .as_ref()
                .is_some_and(|saved| saved.records.len() > MAX_RETAINED_ROWS)
            {
                return Err(BenchError("saved history row retention bound failed"));
            }
            let mut first_ordinal = None;
            let mut last_ordinal = None;
            if let Some(saved) = saved.as_ref() {
                for record in &saved.records {
                    let row = checked(
                        history_rows::decode(record),
                        "saved history row codec failed",
                    )?;
                    if row.target != target {
                        return Err(BenchError("saved history row target did not match"));
                    }
                    if saved_row_contains_sibling_prefix(&row, pane) {
                        return Err(BenchError(
                            "saved history row contains sibling pane synthetic content",
                        ));
                    }
                    first_ordinal.get_or_insert(row.ordinal);
                    last_ordinal = Some(row.ordinal);
                }
            }
            let screens = checked(
                store.load_screens(target),
                "saved screen checkpoint read failed",
            )?;
            let mut normal_checkpoint = false;
            let mut alternate_checkpoint = false;
            for (index, record) in screens.into_iter().enumerate() {
                let Some(record) = record else { continue };
                let checkpoint = checked(
                    screen_checkpoint::decode(&record),
                    "saved screen checkpoint codec failed",
                )?;
                if checkpoint.metadata().target() != target
                    || checkpoint.buffer()
                        != if index == 0 {
                            ScreenBuffer::Normal
                        } else {
                            ScreenBuffer::Alternate
                        }
                {
                    return Err(BenchError(
                        "saved screen checkpoint target or buffer did not match",
                    ));
                }
                if checkpoint_contains_sibling_prefix(&checkpoint, pane) {
                    return Err(BenchError(
                        "saved screen checkpoint contains sibling pane synthetic content",
                    ));
                }
                if index == 0 {
                    normal_checkpoint = true;
                } else {
                    alternate_checkpoint = true;
                }
            }
            let completed_rows = marker_value(&done_paths[pane])?;
            let producer_completed = completed_rows == Some(spec.planned_rows);
            if completed_rows.is_some() && !producer_completed {
                return Err(BenchError(
                    "synthetic producer completion count did not match",
                ));
            }
            if !spec.shutdown && !producer_completed {
                return Err(BenchError("normal producer did not complete"));
            }
            reports.push(TargetReport {
                pane,
                retained_rows: first_ordinal
                    .map_or(0, |_| saved.as_ref().map_or(0, |saved| saved.records.len())),
                segment_bytes,
                first_ordinal,
                last_ordinal,
                gap: saved.as_ref().is_some_and(|saved| saved.gap),
                ended: saved
                    .as_ref()
                    .is_some_and(|saved| saved.ended_at_unix_ms != 0),
                normal_checkpoint,
                alternate_checkpoint,
                producer_completed,
                completed_rows: producer_completed.then_some(spec.planned_rows),
            });
        }
        reports
            .try_into()
            .map_err(|_| BenchError("saved target inventory count is invalid"))
    }

    fn run_case(root: &PrivateRoot, spec: CaseSpec) -> BenchResult<CaseReport> {
        let mut harness = CaseHarness::new(root, spec)?;
        let load = harness.run_load(spec);
        harness.finish(spec, load)
    }

    fn render_report(cases: &[CaseReport]) -> BenchResult<String> {
        if cases.len() != CASES.len() {
            return Err(BenchError("benchmark case count is invalid"));
        }
        let mut output = String::new();
        write!(
            output,
            "{{\"schema_version\":2,\"synthetic\":true,\"actual_history_store\":true,\"queue_occupancy_instrumented\":false,\"build_profile\":\"{}\",\"cases\":[",
            if cfg!(debug_assertions) { "debug" } else { "release" }
        )
        .map_err(|_| BenchError("benchmark report formatting failed"))?;
        for (case_index, case) in cases.iter().enumerate() {
            if case_index != 0 {
                output.push(',');
            }
            write!(
                output,
                "{{\"name\":\"{}\",\"columns\":{},\"rows_per_pane\":24,\"panes\":2,\"planned_rows_per_pane\":{},\"elapsed_us\":{},\"owner_tick_us\":[",
                case.spec.name,
                case.spec.columns,
                case.spec.planned_rows,
                case.elapsed_us
            )
            .map_err(|_| BenchError("benchmark report formatting failed"))?;
            for (index, sample) in case.owner_tick_us.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write!(output, "{sample}")
                    .map_err(|_| BenchError("benchmark report formatting failed"))?;
            }
            write!(output, "],\"probes\":[")
                .map_err(|_| BenchError("benchmark report formatting failed"))?;
            for (index, probe) in case.probes.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write!(
                    output,
                    "{{\"pane\":{},\"control_rtt_us\":{},\"input_reply_us\":{},\"child_ack_us\":{},\"started_during_output\":{},\"ack_before_output_done\":{}}}",
                    probe.pane,
                    probe.control_rtt_us,
                    probe.input_reply_us,
                    probe.child_ack_us,
                    probe.started_during_output,
                    probe.ack_before_output_done
                )
                .map_err(|_| BenchError("benchmark report formatting failed"))?;
            }
            write!(
                output,
                "],\"driver_drop_us\":{},\"post_drop_lock_wait_us\":{},\"history_failure\":{},\"shutdown_during_output\":{},\"history_diagnostics\":{{\"reasons\":[",
                case.driver_drop_us,
                case.post_drop_lock_wait_us,
                case.history_failure,
                case.shutdown_during_output
            )
            .map_err(|_| BenchError("benchmark report formatting failed"))?;
            for (index, reason) in case.history_diagnostics.reasons().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write!(output, "\"{}\"", reason.as_str())
                    .map_err(|_| BenchError("benchmark report formatting failed"))?;
            }
            write!(
                output,
                "],\"writer_drain_outcome\":\"{}\"}},\"targets\":[",
                case.history_diagnostics.writer_drain_outcome().as_str()
            )
            .map_err(|_| BenchError("benchmark report formatting failed"))?;
            for (index, target) in case.targets.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write!(
                    output,
                    "{{\"pane\":{},\"retained_rows\":{},\"segment_bytes\":{},\"first_ordinal\":{},\"last_ordinal\":{},\"gap\":{},\"ended\":{},\"normal_checkpoint\":{},\"alternate_checkpoint\":{},\"producer_completed\":{},\"completed_rows\":{}}}",
                    target.pane,
                    target.retained_rows,
                    target.segment_bytes,
                    option_json(target.first_ordinal),
                    option_json(target.last_ordinal),
                    target.gap,
                    target.ended,
                    target.normal_checkpoint,
                    target.alternate_checkpoint,
                    target.producer_completed,
                    option_json(target.completed_rows)
                )
                .map_err(|_| BenchError("benchmark report formatting failed"))?;
            }
            output.push_str("]}");
        }
        output.push_str("]}");
        Ok(output)
    }

    fn option_json(value: Option<u64>) -> String {
        value.map_or_else(|| "null".to_owned(), |value| value.to_string())
    }

    pub fn main() -> BenchResult<()> {
        let arguments: Vec<OsString> = std::env::args_os().collect();
        if arguments
            .get(1)
            .is_some_and(|arg| arg == OsStr::new(CHILD_MODE))
        {
            if let Err(error) = child_mode(&arguments) {
                eprintln!("synthetic child failed: {error}");
                process::exit(2);
            }
            return Ok(());
        }
        if arguments.len() != 1 {
            return Err(BenchError("benchmark accepts no command-line arguments"));
        }
        let root = PrivateRoot::create()?;
        let mut cases = Vec::with_capacity(CASES.len());
        for spec in CASES {
            cases.push(run_case(&root, spec)?);
        }
        let output = render_report(&cases)?;
        root.remove()?;
        println!("{output}");
        Ok(())
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn main() {
    if let Err(error) = benchmark::main() {
        eprintln!("live history benchmark failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn main() {
    eprintln!("live history benchmark requires Apple Silicon macOS");
    std::process::exit(1);
}
