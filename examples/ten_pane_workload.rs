//! Synthetic ten-pane workload proxy: five independent two-pane servers.
//!
//! Not ten OpenCode agents or the SPEC R-03 ten-agent acceptance workload.
//! Results are observations only; no budget is asserted.

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod workload {
    use panelink::control_protocol::TargetRef;
    use panelink::handshake_protocol::{decode_welcome, Hello, CAPABILITY_INPUT, CAPABILITY_READ};
    use panelink::history_store::HistoryStore;
    use panelink::launch::{Environment, LaunchContext};
    use panelink::layout_store::{LayoutSlot, LayoutStore};
    use panelink::managed_launch_protocol::{EnvironmentEntry, LaunchRequest};
    use panelink::multi_runtime::MultiServerDriver;
    use panelink::multi_startup::{StartedMultiServer, StartupLaunch};
    use panelink::pane_layout::SplitDirection;
    use panelink::protocol::{OperationKey, OperationStatus, Outcome};
    use panelink::protocol_wire::{Frame, MessageKind};
    use panelink::response_protocol::{decode_response_payload, ResponseBody};
    use panelink::server_storage::ServerStorage;
    use panelink::session_protocol::{
        decode_session_info, encode_session_hello, encode_session_request, SessionHello,
        SessionRequest,
    };
    use panelink::split_protocol::{decode_split_result, encode_split_request, SplitRequest};
    use panelink::terminal::Key;
    use panelink::transport::{AuthenticatedStream, OwnedFrame};
    use panelink::ui_protocol::{
        encode_attach_target, encode_key_input, encode_layout_query, KeyInput,
    };
    use std::ffi::{OsStr, OsString};
    use std::fs::{self, DirBuilder, OpenOptions};
    use std::io::{self, Read, Write};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::process::{self, Command};
    use std::sync::mpsc::{self, Receiver, SyncSender};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    const WORKSPACES: usize = 5;
    const PANES: usize = 2;
    const SECONDS: u64 = 30;
    const SAMPLE_EVERY: Duration = Duration::from_secs(1);
    const KEEPALIVE_EVERY: Duration = Duration::from_millis(800);
    const OWNER_POLL: Duration = Duration::from_millis(2);
    const IO_TIMEOUT: Duration = Duration::from_secs(8);
    const ROOT_ROWS: u16 = 49;
    const ROOT_COLUMNS: u16 = 80;
    const CHILD_MODE: &str = "--panelink-ten-pane-child";

    type WorkResult<T> = Result<T, &'static str>;

    struct PrivateRoot(PathBuf);

    impl PrivateRoot {
        fn create() -> WorkResult<Self> {
            let base = fs::canonicalize(std::env::temp_dir())
                .map_err(|_| "private temp root unavailable")?;
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            for attempt in 0..32_u32 {
                let path = base.join(format!(
                    "panelink-p6-{}-{nonce:x}-{attempt:x}",
                    process::id()
                ));
                let mut builder = DirBuilder::new();
                builder.mode(0o700);
                match builder.create(&path) {
                    Ok(()) => {
                        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                            .map_err(|_| "private root permission setup failed")?;
                        let canonical = fs::canonicalize(&path)
                            .map_err(|_| "private root validation failed")?;
                        let metadata = fs::symlink_metadata(&canonical)
                            .map_err(|_| "private root validation failed")?;
                        if canonical != path || metadata.mode() & 0o7777 != 0o700 {
                            return Err("private root validation failed");
                        }
                        return Ok(Self(canonical));
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(_) => return Err("private root creation failed"),
                }
            }
            Err("private root name allocation exhausted")
        }
    }

    impl Drop for PrivateRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    enum ClientCommand {
        Ready {
            reply: SyncSender<WorkResult<()>>,
        },
        Query {
            payload: Vec<u8>,
        },
        Probe {
            probe: usize,
            ack_path: PathBuf,
            previous_ack: u64,
            reply: SyncSender<WorkResult<()>>,
        },
        Stop,
    }

    struct ClientWorker {
        command: SyncSender<ClientCommand>,
        join: Option<JoinHandle<()>>,
    }

    impl ClientWorker {
        fn start(
            driver: &mut MultiServerDriver,
            target: TargetRef,
            pane: usize,
            nonce: u64,
            sequence: u64,
        ) -> WorkResult<Self> {
            let (server_socket, client_socket) =
                UnixStream::pair().map_err(|_| "control socketpair failed")?;
            driver
                .attach(
                    AuthenticatedStream::new(server_socket)
                        .map_err(|_| "server peer authentication failed")?,
                )
                .map_err(|_| "runtime refused workload client")?;
            let (command_tx, command_rx) = mpsc::sync_channel(1);
            let join = thread::Builder::new()
                .name(format!("p6-client-{nonce}"))
                .spawn(move || {
                    client_loop(client_socket, target, pane, nonce, sequence, command_rx)
                })
                .map_err(|_| "client worker creation failed")?;
            Ok(Self {
                command: command_tx,
                join: Some(join),
            })
        }

        fn probe(
            &self,
            probe: usize,
            ack_path: PathBuf,
            previous_ack: u64,
        ) -> WorkResult<Receiver<WorkResult<()>>> {
            let (reply_tx, reply_rx) = mpsc::sync_channel(1);
            self.command
                .send(ClientCommand::Probe {
                    probe,
                    ack_path,
                    previous_ack,
                    reply: reply_tx,
                })
                .map_err(|_| "client worker stopped")?;
            Ok(reply_rx)
        }

        fn connected(&self) -> WorkResult<Receiver<WorkResult<()>>> {
            let (reply_tx, reply_rx) = mpsc::sync_channel(1);
            self.command
                .send(ClientCommand::Ready { reply: reply_tx })
                .map_err(|_| "client worker stopped")?;
            Ok(reply_rx)
        }

        fn keepalive(&self) -> WorkResult<()> {
            self.query(encode_layout_query().to_vec())
        }

        fn query(&self, payload: Vec<u8>) -> WorkResult<()> {
            self.command
                .send(ClientCommand::Query { payload })
                .map_err(|_| "client worker stopped")
        }
    }

    impl Drop for ClientWorker {
        fn drop(&mut self) {
            let _ = self.command.try_send(ClientCommand::Stop);
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    struct Workspace {
        index: usize,
        root: PathBuf,
        history: PathBuf,
        layout: PathBuf,
        driver: Option<MultiServerDriver>,
        targets: [TargetRef; PANES],
        children: [u32; PANES],
        clients: Vec<ClientWorker>,
        pending_ready: Vec<Receiver<WorkResult<()>>>,
        ready_ack: [PathBuf; PANES],
        next_probe: Instant,
        next_keepalive: Instant,
        probe_round: usize,
        pending_probes: Vec<(Receiver<WorkResult<()>>, Instant)>,
        owner_ticks: Vec<u64>,
        probe_latencies: Vec<u64>,
        history_ok: bool,
        layout_ok: bool,
        shutdown_ms: Option<u64>,
    }

    impl Workspace {
        fn create(index: usize, path: PathBuf) -> WorkResult<Self> {
            let server = path.join("server");
            let history = server.join("history");
            let layout = server.join("layout");
            let terminfo = path.join("terminfo");
            for dir in [&path, &server, &terminfo] {
                let mut builder = DirBuilder::new();
                builder.mode(0o700);
                builder
                    .create(dir)
                    .map_err(|_| "workspace private directories failed")?;
            }
            compile_terminfo(&terminfo)?;

            let storage = ServerStorage::initialize(&server)
                .map_err(|_| "server storage initialization failed")?;
            let context = launch_context(&path, &terminfo);
            let executable =
                std::env::current_exe().map_err(|_| "workload executable lookup failed")?;
            let root_args = child_args(&path, 0);
            let started = StartedMultiServer::start(
                storage,
                &terminfo,
                &StartupLaunch {
                    executable: executable.as_os_str(),
                    arguments: &root_args,
                    context: &context,
                    rows: ROOT_ROWS,
                    columns: ROOT_COLUMNS,
                },
                true,
            )
            .map_err(|_| "first pane startup failed")?;
            let mut history_builder = DirBuilder::new();
            history_builder.mode(0o700);
            history_builder
                .create(&history)
                .map_err(|_| "history directory setup failed")?;
            let target0 = started.startup().target.ok_or("root target missing")?;
            let mut driver = started.into_driver();
            fs::set_permissions(&history, fs::Permissions::from_mode(0o700))
                .map_err(|_| "history directory permission setup failed")?;
            driver
                .enable_layout(
                    LayoutStore::prepare(&server).map_err(|_| "layout store setup failed")?,
                )
                .map_err(|_| "layout capture startup failed")?;
            driver.enable_history(
                HistoryStore::open(&history).map_err(|_| "history store setup failed")?,
            );
            if !driver.history_available() {
                return Err("history capture startup failed");
            }
            let target1 = split_second_pane(
                &mut driver,
                target0,
                &context,
                &terminfo,
                &executable,
                &path,
            )?;
            wait_layout_durable(&mut driver, &server, target0, target1)?;

            let mut workspace = Self {
                index,
                root: path.clone(),
                history,
                layout,
                driver: Some(driver),
                targets: [target0, target1],
                children: [0, 0],
                clients: Vec::with_capacity(PANES),
                pending_ready: Vec::with_capacity(PANES),
                ready_ack: [path.join("pane-0.ack"), path.join("pane-1.ack")],
                next_probe: Instant::now() + Duration::from_secs(1),
                next_keepalive: Instant::now() + KEEPALIVE_EVERY,
                probe_round: 0,
                pending_probes: Vec::with_capacity(PANES),
                owner_ticks: Vec::new(),
                probe_latencies: Vec::new(),
                history_ok: false,
                layout_ok: false,
                shutdown_ms: None,
            };
            workspace.wait_ready()?;
            workspace.children = [
                read_marker_pid(&workspace.root.join("pane-0.ready"))?,
                read_marker_pid(&workspace.root.join("pane-1.ready"))?,
            ];
            Ok(workspace)
        }

        fn connect_clients(&mut self) -> WorkResult<()> {
            for (pane, target) in self.targets.iter().copied().enumerate() {
                let worker = ClientWorker::start(
                    self.driver.as_mut().ok_or("driver missing")?,
                    target,
                    pane,
                    1000 + (self.index as u64 * 10) + pane as u64,
                    1,
                )?;
                self.clients.push(worker);
            }
            Ok(())
        }

        fn announce_ready(&mut self) -> WorkResult<()> {
            self.pending_ready = self
                .clients
                .iter()
                .map(ClientWorker::connected)
                .collect::<WorkResult<Vec<_>>>()?;
            Ok(())
        }

        fn poll_ready(&mut self) -> WorkResult<bool> {
            let mut index = 0;
            while index < self.pending_ready.len() {
                match self.pending_ready[index].try_recv() {
                    Ok(result) => {
                        result?;
                        self.pending_ready.swap_remove(index);
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err("client handshake worker disconnected");
                    }
                    Err(mpsc::TryRecvError::Empty) => index += 1,
                }
            }
            Ok(self.pending_ready.is_empty())
        }

        fn wait_ready(&mut self) -> WorkResult<()> {
            let deadline = Instant::now() + IO_TIMEOUT;
            while !(self.root.join("pane-0.ready").exists()
                && self.root.join("pane-1.ready").exists())
            {
                self.tick(false)?;
                if Instant::now() >= deadline {
                    return Err("synthetic children not ready");
                }
                thread::sleep(OWNER_POLL);
            }
            Ok(())
        }

        fn tick(&mut self, measured: bool) -> WorkResult<()> {
            let started = Instant::now();
            self.driver
                .as_mut()
                .ok_or("workspace driver missing")?
                .tick(|| 1 + elapsed_whole_seconds())
                .map_err(|_| "owner tick failed")?;
            if measured && self.clients.is_empty() {
                return Err("no connected control clients");
            }
            if measured {
                self.owner_ticks.push(elapsed_us(started.elapsed()));
            }
            Ok(())
        }

        fn advance(&mut self, now: Instant, stop_at: Instant) -> WorkResult<()> {
            self.tick(true)?;
            if now >= self.next_keepalive {
                for client in &self.clients {
                    client.keepalive()?;
                }
                self.next_keepalive = now + KEEPALIVE_EVERY;
            }
            if self.probe_round < CHILD_PROBES && now >= self.next_probe && now < stop_at {
                self.probe_round += 1;
                for (pane, client) in self.clients.iter().enumerate() {
                    let previous = marker_number(&self.ready_ack[pane])?;
                    self.pending_probes.push((
                        client.probe(self.probe_round, self.ready_ack[pane].clone(), previous)?,
                        now,
                    ));
                }
                self.next_probe += Duration::from_secs(5);
            }
            self.observe_probes(now)
        }

        fn observe_probes(&mut self, now: Instant) -> WorkResult<()> {
            let mut index = 0;
            while index < self.pending_probes.len() {
                match self.pending_probes[index].0.try_recv() {
                    Ok(Ok(())) => {
                        let (_, started_at) = self.pending_probes.swap_remove(index);
                        self.probe_latencies
                            .push(elapsed_us(now.saturating_duration_since(started_at)));
                    }
                    Ok(Err(_)) => return Err("child input acknowledgment failed"),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err("probe client disconnected");
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        if now.saturating_duration_since(self.pending_probes[index].1) >= IO_TIMEOUT
                        {
                            return Err("child input acknowledgment timed out");
                        }
                        index += 1;
                    }
                }
            }
            Ok(())
        }

        fn shutdown(&mut self) -> WorkResult<()> {
            let drain_deadline = Instant::now() + IO_TIMEOUT;
            while !self.pending_probes.is_empty() && Instant::now() < drain_deadline {
                self.tick(true)?;
                self.observe_probes(Instant::now())?;
                thread::sleep(OWNER_POLL);
            }
            if !self.pending_probes.is_empty() {
                return Err("pending child input acknowledgment at shutdown");
            }
            self.clients.clear();
            let started = Instant::now();
            drop(self.driver.take());
            self.shutdown_ms = Some(elapsed_us(started.elapsed()) / 1000);
            let history = HistoryStore::open(&self.history).map_err(|_| "history reopen failed")?;
            self.history_ok = self.targets.iter().all(|target| {
                history.load_rows(*target).ok().flatten().is_some()
                    && history
                        .load_screens(*target)
                        .is_ok_and(|screens| screens[0].is_some())
            });
            let layout = LayoutStore::open(&self.layout).map_err(|_| "layout reopen failed")?;
            self.layout_ok = layout
                .load(LayoutSlot::Current)
                .ok()
                .flatten()
                .is_some_and(|saved| {
                    saved.pane_count() == 2 && saved.panes() == self.targets.map(Some)
                });
            Ok(())
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            self.clients.clear();
            drop(self.driver.take());
        }
    }

    fn launch_context(root: &Path, terminfo: &Path) -> LaunchContext {
        LaunchContext {
            working_directory: root.to_path_buf(),
            environment: Environment::from([
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (OsString::from("HOME"), root.as_os_str().to_owned()),
                (OsString::from("TERMINFO"), terminfo.as_os_str().to_owned()),
                (OsString::from("TERM"), OsString::from("panelink-probe")),
            ]),
        }
    }

    fn compile_terminfo(directory: &Path) -> WorkResult<()> {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/terminfo/panelink-probe.terminfo");
        let status = Command::new("/usr/bin/tic")
            .args([OsStr::new("-x"), OsStr::new("-o")])
            .arg(directory)
            .arg(source)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .status()
            .map_err(|_| "tic failed to start")?;
        if status.success() {
            Ok(())
        } else {
            Err("tic failed to compile private terminfo")
        }
    }

    fn child_args(root: &Path, pane: usize) -> Vec<OsString> {
        vec![
            OsString::from(CHILD_MODE),
            root.as_os_str().to_owned(),
            OsString::from(pane.to_string()),
        ]
    }

    const CHILD_PROBES: usize = 6;

    pub fn child_mode() -> Option<WorkResult<()>> {
        let arguments = std::env::args_os().collect::<Vec<_>>();
        if arguments.get(1).map(OsStr::new) != Some(OsStr::new(CHILD_MODE)) {
            return None;
        }
        Some((|| {
            if arguments.len() != 4 {
                return Err("synthetic child arguments invalid");
            }
            let root = PathBuf::from(&arguments[2]);
            let pane = arguments[3]
                .to_str()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|pane| *pane < PANES)
                .ok_or("synthetic child pane invalid")?;
            run_synthetic_child(&root, pane)
        })())
    }

    fn run_synthetic_child(root: &Path, pane: usize) -> WorkResult<()> {
        let ready = root.join(format!("pane-{pane}.ready"));
        let ack = root.join(format!("pane-{pane}.ack"));
        let reader_ack = ack.clone();
        let reader = thread::Builder::new()
            .name(format!("ten-pane-input-{pane}"))
            .spawn(move || read_child_inputs(&reader_ack, CHILD_PROBES))
            .map_err(|_| "synthetic child input reader could not start")?;
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(format!("SYNTHETIC-PANE-{pane}-READY\r\n").as_bytes())
            .and_then(|()| stdout.flush())
            .map_err(|_| "synthetic child readiness output failed")?;
        fs::write(&ready, format!("{}\n", process::id()))
            .map_err(|_| "synthetic child readiness marker failed")?;
        let mut tick = 0_u64;
        loop {
            tick += 1;
            let text = match tick % 3 {
                0 => "plain mixed text",
                1 => "quoted \"text\"",
                _ => "unicode café λ",
            };
            if writeln!(stdout, "pane={pane} tick={tick} {text}")
                .and_then(|()| stdout.flush())
                .is_err()
            {
                break;
            }
            thread::sleep(Duration::from_millis(250));
        }
        drop(stdout);
        reader
            .join()
            .map_err(|_| "synthetic child input reader panicked")??;
        Ok(())
    }

    fn read_child_inputs(path: &Path, count: usize) -> WorkResult<()> {
        let mut stdin = io::stdin().lock();
        for completed in 1..=count {
            let mut input = [0_u8; 2];
            stdin
                .read_exact(&mut input)
                .map_err(|_| "synthetic child input was not acknowledged")?;
            if input != *b"q\n" && input != *b"q\r" {
                return Err("synthetic child received unexpected input");
            }
            atomic_write(path, format!("{completed}\n").as_bytes())
                .map_err(|_| "synthetic child acknowledgment write failed")?;
        }
        Ok(())
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

    fn split_second_pane(
        driver: &mut MultiServerDriver,
        target: TargetRef,
        context: &LaunchContext,
        terminfo: &Path,
        executable: &Path,
        root: &Path,
    ) -> WorkResult<TargetRef> {
        let (server_socket, client_socket) =
            UnixStream::pair().map_err(|_| "split socketpair failed")?;
        driver
            .attach(
                AuthenticatedStream::new(server_socket).map_err(|_| "split server peer invalid")?,
            )
            .map_err(|_| "split client refused")?;
        let root = root.to_path_buf();
        let context = LaunchContext {
            working_directory: context.working_directory.clone(),
            environment: context.environment.clone(),
        };
        let terminfo = terminfo.to_path_buf();
        let executable = executable.to_path_buf();
        let (result_tx, result_rx) = mpsc::sync_channel(1);
        let client = thread::spawn(move || {
            let _ = result_tx.send(split_client(
                client_socket,
                target,
                &root,
                &context,
                &terminfo,
                &executable,
            ));
        });
        let deadline = Instant::now() + IO_TIMEOUT;
        loop {
            match result_rx.try_recv() {
                Ok(result) => {
                    client.join().map_err(|_| "split client panicked")?;
                    return result;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("split client stopped before result");
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if Instant::now() >= deadline {
                return Err("split client timed out");
            }
            driver
                .tick(|| 1)
                .map_err(|_| "owner tick failed during split")?;
            thread::sleep(OWNER_POLL);
        }
    }

    fn wait_layout_durable(
        driver: &mut MultiServerDriver,
        server: &Path,
        target0: TargetRef,
        target1: TargetRef,
    ) -> WorkResult<()> {
        let deadline = Instant::now() + IO_TIMEOUT;
        loop {
            driver
                .tick(|| 1)
                .map_err(|_| "owner tick failed waiting for saved layout")?;
            if let Ok(store) = LayoutStore::open(&server.join("layout")) {
                if store
                    .load(LayoutSlot::Current)
                    .ok()
                    .flatten()
                    .is_some_and(|saved| {
                        saved.pane_count() == 2 && saved.panes() == [Some(target0), Some(target1)]
                    })
                {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                return Err("two-pane layout was not durably saved before workload");
            }
            thread::sleep(OWNER_POLL);
        }
    }

    fn split_client(
        socket: UnixStream,
        target0: TargetRef,
        root: &Path,
        context: &LaunchContext,
        terminfo: &Path,
        executable: &Path,
    ) -> WorkResult<TargetRef> {
        let mut stream =
            AuthenticatedStream::new(socket).map_err(|_| "split client peer invalid")?;
        let mut hello = encode_session_hello(&SessionHello {
            hello: Hello {
                nonce: 71,
                required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
                optional_capabilities: 0,
            },
            resume: None,
        })
        .map_err(|_| "split hello invalid")?;
        hello
            .extend_from_slice(&encode_attach_target(target0).map_err(|_| "split target invalid")?);
        stream
            .write_frame(
                &Frame::new(MessageKind::Hello, 0, &hello)
                    .map_err(|_| "split hello frame invalid")?,
                Instant::now() + IO_TIMEOUT,
            )
            .map_err(|_| "split hello write failed")?;
        let welcome_frame = stream
            .read_frame(Instant::now() + IO_TIMEOUT)
            .map_err(|_| "split welcome missing")?;
        if welcome_frame.kind() != MessageKind::Welcome {
            return Err("split welcome kind invalid");
        }
        let welcome =
            decode_welcome(welcome_frame.payload()).map_err(|_| "split welcome invalid")?;
        let mut request_id = 0;
        let mut sequence = 1;
        let describe = encode_session_request(&SessionRequest::Describe)
            .map_err(|_| "split describe invalid")?;
        let info = decode_session_info(request(&mut stream, &mut request_id, &describe)?.payload())
            .map_err(|_| "split session info invalid")?;
        let claim_key = OperationKey {
            generation: welcome.generation,
            lease: welcome.lease,
            sequence,
        };
        sequence += 1;
        let claim = encode_session_request(&SessionRequest::Claim {
            key: claim_key,
            target: target0,
            expected_owner_revision: info.owner_revision,
            ttl_seconds: 30,
        })
        .map_err(|_| "split claim invalid")?;
        let claim_frame = request(&mut stream, &mut request_id, &claim)?;
        let claim = decode_response_payload(claim_frame.payload())
            .map_err(|_| "split claim reply invalid")?;
        if !matches!(
            claim.body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        ) {
            eprintln!("split claim rejected: {:?}", claim.body);
            return Err("split input ownership claim failed");
        }
        let info = decode_session_info(request(&mut stream, &mut request_id, &describe)?.payload())
            .map_err(|_| "split revised info invalid")?;
        let key = OperationKey {
            generation: welcome.generation,
            lease: welcome.lease,
            sequence,
        };
        let arguments = child_args(root, 1);
        let argument_refs = arguments
            .iter()
            .map(|argument| argument.as_bytes())
            .collect::<Vec<_>>();
        let env = [
            EnvironmentEntry {
                name: b"PATH",
                value: b"/usr/bin:/bin",
            },
            EnvironmentEntry {
                name: b"HOME",
                value: root.as_os_str().as_bytes(),
            },
            EnvironmentEntry {
                name: b"TERMINFO",
                value: terminfo.as_os_str().as_bytes(),
            },
            EnvironmentEntry {
                name: b"TERM",
                value: b"panelink-probe",
            },
        ];
        let launch = LaunchRequest {
            key,
            rows: 24,
            columns: ROOT_COLUMNS,
            requested_executable: executable.as_os_str().as_bytes(),
            requested_working_directory: root.as_os_str().as_bytes(),
            client_working_directory: context.working_directory.as_os_str().as_bytes(),
            arguments: &argument_refs,
            environment: &env,
        };
        let split = SplitRequest {
            key,
            target: target0,
            expected_layout_revision: 1,
            direction: SplitDirection::Down,
            launch: &launch,
        };
        let mut payload = Vec::new();
        encode_split_request(&split, &mut payload).map_err(|_| "split request invalid")?;
        let split = decode_split_result(request(&mut stream, &mut request_id, &payload)?.payload())
            .map_err(|_| "split reply invalid")?;
        if split.outcome != Outcome::Applied {
            return Err("split operation was not applied");
        }
        let target = split.new_target.ok_or("split result target missing")?;
        let release = encode_session_request(&SessionRequest::Release {
            key: OperationKey {
                generation: welcome.generation,
                lease: welcome.lease,
                sequence: sequence + 1,
            },
            target: target0,
            expected_owner_revision: info.owner_revision,
        })
        .map_err(|_| "split owner release invalid")?;
        let release_frame = request(&mut stream, &mut request_id, &release)?;
        let released = decode_response_payload(release_frame.payload())
            .map_err(|_| "split owner release response invalid")?;
        if !matches!(
            released.body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        ) {
            return Err("split temporary input ownership release failed");
        }
        Ok(target)
    }

    fn client_loop(
        socket: UnixStream,
        target: TargetRef,
        pane: usize,
        nonce: u64,
        mut sequence: u64,
        commands: Receiver<ClientCommand>,
    ) {
        if let Err(error) = service_client(socket, target, pane, nonce, &mut sequence, commands) {
            eprintln!("synthetic control client pane={pane}: {error}");
        } else {
            eprintln!("synthetic control client pane={pane} stopped cleanly");
        }
    }

    fn service_client(
        socket: UnixStream,
        target: TargetRef,
        _pane: usize,
        nonce: u64,
        sequence: &mut u64,
        commands: Receiver<ClientCommand>,
    ) -> WorkResult<()> {
        let mut stream = AuthenticatedStream::new(socket).map_err(|_| "client peer invalid")?;
        let mut hello = encode_session_hello(&SessionHello {
            hello: Hello {
                nonce,
                required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
                optional_capabilities: 0,
            },
            resume: None,
        })
        .map_err(|_| "client hello invalid")?;
        hello
            .extend_from_slice(&encode_attach_target(target).map_err(|_| "client target invalid")?);
        stream
            .write_frame(
                &Frame::new(MessageKind::Hello, 0, &hello)
                    .map_err(|_| "client hello frame invalid")?,
                Instant::now() + IO_TIMEOUT,
            )
            .map_err(|_| "client hello write failed")?;
        let welcome_frame = stream
            .read_frame(Instant::now() + IO_TIMEOUT)
            .map_err(|_| "client welcome missing")?;
        if welcome_frame.kind() != MessageKind::Welcome {
            return Err("client welcome kind invalid");
        }
        let welcome =
            decode_welcome(welcome_frame.payload()).map_err(|_| "client welcome invalid")?;
        let mut request_id = 0;
        let describe = encode_session_request(&SessionRequest::Describe)
            .map_err(|_| "client describe invalid")?;
        let info_frame = request(&mut stream, &mut request_id, &describe)
            .map_err(|_| "client describe request failed")?;
        let info =
            decode_session_info(info_frame.payload()).map_err(|_| "client session info invalid")?;
        if info.target != target {
            return Err("client session target did not match attachment");
        }
        let claim = encode_session_request(&SessionRequest::Claim {
            key: OperationKey {
                generation: welcome.generation,
                lease: welcome.lease,
                sequence: *sequence,
            },
            target,
            expected_owner_revision: info.owner_revision,
            ttl_seconds: 30,
        })
        .map_err(|_| "client input claim invalid")?;
        let claim_frame = request(&mut stream, &mut request_id, &claim)
            .map_err(|_| "client claim request failed")?;
        let response = decode_response_payload(claim_frame.payload())
            .map_err(|_| "client claim response invalid")?;
        if !matches!(
            response.body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        ) {
            return Err("client claim refused");
        }
        *sequence = sequence
            .checked_add(1)
            .ok_or("client operation sequence exhausted")?;
        let owner_frame = request(&mut stream, &mut request_id, &describe)
            .map_err(|_| "client owner refresh request failed")?;
        let owner = decode_session_info(owner_frame.payload())
            .map_err(|_| "client owner refresh invalid")?;
        if owner.target != target || owner.owner_lease != welcome.lease {
            return Err("client input ownership was not confirmed");
        }
        loop {
            match commands.recv() {
                Ok(ClientCommand::Stop) | Err(_) => return Ok(()),
                Ok(ClientCommand::Ready { reply }) => {
                    let _ = reply.send(Ok(()));
                }
                Ok(ClientCommand::Query { payload }) => {
                    if request(&mut stream, &mut request_id, &payload).is_err() {
                        return Err("client layout query failed");
                    }
                }
                Ok(ClientCommand::Probe {
                    probe,
                    ack_path,
                    previous_ack,
                    reply,
                }) => {
                    let result = send_probe(
                        &mut stream,
                        &mut request_id,
                        target,
                        welcome.generation,
                        welcome.lease,
                        owner.owner_revision,
                        sequence,
                    );
                    let result = result.and_then(|sequence_used| {
                        *sequence = sequence_used
                            .checked_add(1)
                            .ok_or("client operation sequence exhausted")?;
                        let deadline = Instant::now() + IO_TIMEOUT;
                        loop {
                            if marker_number(&ack_path)
                                .is_ok_and(|value| value >= probe as u64 && value > previous_ack)
                            {
                                return Ok(());
                            }
                            if Instant::now() >= deadline {
                                return Err("child acknowledgment timed out");
                            }
                            thread::sleep(OWNER_POLL);
                        }
                    });
                    match result {
                        Ok(()) => {
                            let _ = reply.send(Ok(()));
                        }
                        Err(error) => {
                            let _ = reply.send(Err(error));
                            return Err("child probe failed");
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn send_probe(
        stream: &mut AuthenticatedStream,
        request_id: &mut u64,
        target: TargetRef,
        generation: u64,
        lease: u64,
        owner_revision: u64,
        sequence: &mut u64,
    ) -> WorkResult<u64> {
        let keys = [Key::Char('q'), Key::Enter];
        let key = OperationKey {
            generation,
            lease,
            sequence: *sequence,
        };
        let payload = encode_key_input(&KeyInput {
            key,
            target,
            expected_owner_revision: owner_revision,
            keys: &keys,
        })
        .map_err(|_| "client input encoding failed")?;
        *request_id = request_id.checked_add(1).ok_or("request id exhausted")?;
        let frame = Frame::new(MessageKind::Request, *request_id, &payload)
            .map_err(|_| "client input frame invalid")?;
        // Input is nonreplayable: a failed write is reported, never resent
        // under a new request ID or operation key.
        stream
            .write_frame(&frame, Instant::now() + IO_TIMEOUT)
            .map_err(|_| "client input write failed")?;
        let response = stream
            .read_frame(Instant::now() + IO_TIMEOUT)
            .map_err(|_| "client input reply missing")?;
        if response.request_id() != *request_id || response.kind() != MessageKind::Response {
            eprintln!(
                "client input reply mismatch: id={} expected={} kind={:?}",
                response.request_id(),
                request_id,
                response.kind()
            );
            return Err("client input reply mismatched");
        }
        let decoded = decode_response_payload(response.payload())
            .map_err(|_| "client input response invalid")?;
        if matches!(
            decoded.body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        ) {
            Ok(key.sequence)
        } else {
            Err("client input was not applied")
        }
    }

    fn request(
        stream: &mut AuthenticatedStream,
        request_id: &mut u64,
        payload: &[u8],
    ) -> WorkResult<OwnedFrame> {
        *request_id += 1;
        let frame = Frame::new(MessageKind::Request, *request_id, payload)
            .map_err(|_| "request frame invalid")?;
        stream
            .write_frame(&frame, Instant::now() + IO_TIMEOUT)
            .map_err(|_| "request write failed")?;
        let response = stream
            .read_frame(Instant::now() + IO_TIMEOUT)
            .map_err(|_| "request response missing")?;
        if response.request_id() != *request_id || response.kind() != MessageKind::Response {
            return Err("request response mismatch");
        }
        Ok(response)
    }

    fn read_marker_pid(path: &Path) -> WorkResult<u32> {
        let text = fs::read_to_string(path).map_err(|_| "child readiness file unavailable")?;
        text.trim().parse().map_err(|_| "child PID invalid")
    }

    fn marker_number(path: &Path) -> WorkResult<u64> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(_) => return Err("child acknowledgment marker read failed"),
        };
        text.trim()
            .parse()
            .map_err(|_| "child acknowledgment marker invalid")
    }

    fn sample_processes(pids: &[u32]) -> WorkResult<(u64, f64)> {
        let list = pids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let output = Command::new("/bin/ps")
            .args(["-o", "rss=", "-o", "%cpu=", "-p", &list])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output()
            .map_err(|_| "ps sampling could not start")?;
        if !output.status.success() {
            return Err("ps sampling failed");
        }
        let text = std::str::from_utf8(&output.stdout).map_err(|_| "ps output invalid")?;
        let mut rss_kib = 0_u64;
        let mut cpu = 0_f64;
        let mut count = 0;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let mut fields = line.split_whitespace();
            rss_kib = rss_kib.saturating_add(
                fields
                    .next()
                    .ok_or("ps RSS missing")?
                    .parse()
                    .map_err(|_| "ps RSS invalid")?,
            );
            cpu += fields
                .next()
                .ok_or("ps CPU missing")?
                .parse::<f64>()
                .map_err(|_| "ps CPU invalid")?;
            count += 1;
        }
        if count != pids.len() {
            return Err("ps did not return every child process");
        }
        Ok((rss_kib.saturating_mul(1024), cpu))
    }

    fn elapsed_us(duration: Duration) -> u64 {
        u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
    }

    fn elapsed_whole_seconds() -> u64 {
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn percentile(samples: &[u64], fraction: f64) -> Option<u64> {
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let rank = ((fraction * sorted.len() as f64).ceil() as usize).max(1);
        sorted
            .get(rank.saturating_sub(1).min(sorted.len() - 1))
            .copied()
    }

    fn json_num(value: Option<u64>) -> String {
        value.map_or_else(|| "null".to_owned(), |value| value.to_string())
    }
    fn print_workspace(workspace: &Workspace) {
        print!(
            "{{\"workspace\":{},\"owner_ticks\":{{\"count\":{},\"p50_us\":{},\"p95_us\":{},\"max_us\":{}}},\"child_ack\":{{\"count\":{},\"p50_us\":{},\"p95_us\":{},\"max_us\":{}}},\"history_ok\":{},\"layout_ok\":{},\"shutdown_ms\":{}}}",
            workspace.index,
            workspace.owner_ticks.len(),
            json_num(percentile(&workspace.owner_ticks, 0.50)),
            json_num(percentile(&workspace.owner_ticks, 0.95)),
            json_num(workspace.owner_ticks.iter().copied().max()),
            workspace.probe_latencies.len(),
            json_num(percentile(&workspace.probe_latencies, 0.50)),
            json_num(percentile(&workspace.probe_latencies, 0.95)),
            json_num(workspace.probe_latencies.iter().copied().max()),
            workspace.history_ok,
            workspace.layout_ok,
            workspace
                .shutdown_ms
                .map_or_else(|| "null".to_owned(), |value| value.to_string())
        );
    }

    fn print_resource_samples(samples: &[(u64, f64)]) {
        print!("\"process_samples\":[");
        for (index, (rss_bytes, cpu_percent)) in samples.iter().copied().enumerate() {
            if index > 0 {
                print!(",");
            }
            print!("{{\"rss_bytes\":{rss_bytes},\"cpu_percent\":{cpu_percent:.2}}}");
        }
        print!("],");
    }

    pub fn main() -> WorkResult<()> {
        if let Some(result) = child_mode() {
            return result;
        }
        let arguments = std::env::args_os().collect::<Vec<_>>();
        if arguments.len() != 1 {
            return Err("ten-pane workload takes no arguments");
        }
        let root = PrivateRoot::create()?;
        let mut workspaces = Vec::with_capacity(WORKSPACES);
        for index in 0..WORKSPACES {
            workspaces.push(Workspace::create(
                index,
                root.0.join(format!("workspace-{index}")),
            )?);
        }
        for workspace in &mut workspaces {
            workspace.connect_clients()?;
        }
        for workspace in &mut workspaces {
            workspace.wait_ready()?;
            workspace.children = [
                read_marker_pid(&workspace.root.join("pane-0.ready"))?,
                read_marker_pid(&workspace.root.join("pane-1.ready"))?,
            ];
        }
        for workspace in &mut workspaces {
            workspace.announce_ready()?;
        }
        let ready_deadline = Instant::now() + IO_TIMEOUT;
        loop {
            let mut all_ready = true;
            for workspace in &mut workspaces {
                workspace.tick(false)?;
                all_ready &= workspace.poll_ready()?;
            }
            if all_ready {
                break;
            }
            if Instant::now() >= ready_deadline {
                return Err("client handshake timed out");
            }
            thread::sleep(OWNER_POLL);
        }
        for workspace in &mut workspaces {
            for client in &workspace.clients {
                client.query(encode_layout_query().to_vec())?;
            }
            workspace.tick(false)?;
        }
        let layout_deadline = Instant::now() + IO_TIMEOUT;
        loop {
            let mut all_durable = true;
            for workspace in &mut workspaces {
                workspace.tick(false)?;
                let server = workspace.root.join("server");
                all_durable &= LayoutStore::open(&server.join("layout"))
                    .ok()
                    .and_then(|store| store.load(LayoutSlot::Current).ok().flatten())
                    .is_some_and(|saved| {
                        saved.pane_count() == 2
                            && saved.panes()
                                == [Some(workspace.targets[0]), Some(workspace.targets[1])]
                    });
            }
            if all_durable {
                break;
            }
            if Instant::now() >= layout_deadline {
                return Err("two-pane layout not confirmed durable before probes");
            }
            thread::sleep(OWNER_POLL);
        }
        let start = Instant::now();
        for workspace in &mut workspaces {
            workspace.next_probe = start + Duration::from_secs(5);
            workspace.next_keepalive = start + KEEPALIVE_EVERY;
        }
        for workspace in &workspaces {
            fs::write(workspace.root.join("workload.start"), b"go\n")
                .map_err(|_| "synthetic producer start gate failed")?;
        }
        let stop_at = start + Duration::from_secs(SECONDS);
        let mut next_sample = start + SAMPLE_EVERY;
        let mut process_samples = Vec::new();
        while Instant::now() < stop_at {
            let now = Instant::now();
            for workspace in &mut workspaces {
                workspace.advance(now, stop_at)?;
            }
            if now >= next_sample {
                let mut pids = Vec::with_capacity(1 + WORKSPACES * PANES);
                pids.push(process::id());
                pids.extend(workspaces.iter().flat_map(|workspace| workspace.children));
                process_samples.push(sample_processes(&pids)?);
                next_sample += SAMPLE_EVERY;
            }
            thread::sleep(OWNER_POLL);
        }
        for workspace in &mut workspaces {
            workspace.shutdown()?;
        }
        let pids = workspaces
            .iter()
            .flat_map(|workspace| workspace.children)
            .collect::<Vec<_>>();
        let cleanup_verified = verify_processes_stopped(&pids)?;
        print!(
            "{{\"schema_version\":1,\"synthetic\":true,\"spec_ten_agent_acceptance\":false,\"server_limit_model\":\"five independent two-pane workspaces and storage domains in one process\",\"workspaces\":{WORKSPACES},\"panes\":{},\"workload_seconds\":{SECONDS},\"cleanup_processes_verified_stopped\":{},",
            WORKSPACES * PANES,
            cleanup_verified
        );
        print_resource_samples(&process_samples);
        print!("\"results\":[");
        for (index, workspace) in workspaces.iter().enumerate() {
            if index > 0 {
                print!(",");
            }
            print_workspace(workspace);
        }
        println!("]}}");
        Ok(())
    }

    fn verify_processes_stopped(pids: &[u32]) -> WorkResult<bool> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let mut alive = false;
            for pid in pids {
                let output = Command::new("/bin/ps")
                    .args(["-o", "pid=", "-p", &pid.to_string()])
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin")
                    .output()
                    .map_err(|_| "cleanup process verification failed")?;
                if !output.stdout.is_empty() {
                    alive = true;
                    break;
                }
            }
            if !alive {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Err(
                    "one or more synthetic producer children remained after server shutdown",
                );
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn main() {
    if let Err(error) = workload::main() {
        eprintln!("ten-pane workload failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn main() {
    eprintln!("ten-pane workload requires macOS aarch64");
    std::process::exit(2);
}
