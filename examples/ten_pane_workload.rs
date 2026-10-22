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
    use std::fs::{self, DirBuilder};
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
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
    const OWNER_POLL: Duration = Duration::from_millis(2);
    const IO_TIMEOUT: Duration = Duration::from_secs(4);
    const ROOT_ROWS: u16 = 49;
    const ROOT_COLUMNS: u16 = 80;

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
        ready_ack: [PathBuf; PANES],
        next_probe: Instant,
        probe_round: usize,
        pending_probes: Vec<(Receiver<WorkResult<()>>, Instant)>,
        owner_ticks: Vec<u64>,
        probe_latencies: Vec<u64>,
        rss_bytes: Vec<u64>,
        cpu_percent: Vec<f64>,
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
            let executable = PathBuf::from("/bin/sh");
            let root_args = shell_args(&path, 0);
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
                ready_ack: [path.join("pane-0.ack"), path.join("pane-1.ack")],
                next_probe: Instant::now() + Duration::from_secs(3),
                probe_round: 0,
                pending_probes: Vec::with_capacity(PANES),
                owner_ticks: Vec::new(),
                probe_latencies: Vec::new(),
                rss_bytes: Vec::new(),
                cpu_percent: Vec::new(),
                history_ok: false,
                layout_ok: false,
                shutdown_ms: None,
            };
            workspace.wait_ready()?;
            workspace.children = [
                read_marker_pid(&workspace.root.join("pane-0.ready"))?,
                read_marker_pid(&workspace.root.join("pane-1.ready"))?,
            ];
            for (pane, target) in workspace.targets.iter().copied().enumerate() {
                let worker = ClientWorker::start(
                    workspace.driver.as_mut().ok_or("driver missing")?,
                    target,
                    pane,
                    1000 + (index as u64 * 10) + pane as u64,
                    1,
                )?;
                workspace.clients.push(worker);
            }
            let mut ready = workspace
                .clients
                .iter()
                .map(ClientWorker::connected)
                .collect::<WorkResult<Vec<_>>>()?;
            let deadline = Instant::now() + IO_TIMEOUT;
            while !ready.is_empty() {
                workspace.tick(false)?;
                let mut index = 0;
                while index < ready.len() {
                    match ready[index].try_recv() {
                        Ok(result) => {
                            result?;
                            ready.swap_remove(index);
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            eprintln!("client readiness disconnected before reply");
                            return Err("client handshake worker disconnected");
                        }
                        Err(mpsc::TryRecvError::Empty) => index += 1,
                    }
                }
                if Instant::now() >= deadline {
                    return Err("client handshake timed out");
                }
                thread::sleep(OWNER_POLL);
            }
            workspace.clients[0].query(encode_layout_query().to_vec())?;
            let deadline = Instant::now() + IO_TIMEOUT;
            loop {
                workspace.tick(false)?;
                if LayoutStore::open(&server.join("layout"))
                    .ok()
                    .and_then(|store| store.load(LayoutSlot::Current).ok().flatten())
                    .is_some_and(|saved| {
                        saved.pane_count() == 2
                            && saved.panes()
                                == [Some(workspace.targets[0]), Some(workspace.targets[1])]
                    })
                {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err("two-pane layout not confirmed durable before probes");
                }
                thread::sleep(OWNER_POLL);
            }
            Ok(workspace)
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
                .tick(|| 1)
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
            if now >= self.next_probe && now < stop_at {
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
                        return Err("probe client disconnected")
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

        fn sample_children(&mut self) -> WorkResult<()> {
            let mut pids = Vec::with_capacity(PANES + 1);
            pids.push(process::id());
            pids.extend(self.children);
            let (rss, cpu) = sample_processes(&pids)?;
            self.rss_bytes.push(rss);
            self.cpu_percent.push(cpu);
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

    fn shell_args(root: &Path, pane: usize) -> Vec<OsString> {
        vec![
            OsString::from("-c"),
            OsString::from(
                "root=$1; pane=$2; ready=\"$root/pane-$pane.ready\"; ack=\"$root/pane-$pane.ack\"; printf 'SYNTHETIC-PANE-%s-READY\\r\\n' \"$pane\"; printf '%s\\n' \"$$\" > \"$ready\"; (i=0; while :; do i=$((i+1)); case $((i % 3)) in 0) text='plain mixed text' ;; 1) text='quoted \"text\"' ;; *) text='unicode café λ' ;; esac; printf 'pane=%s tick=%s %s\\r\\n' \"$pane\" \"$i\" \"$text\"; sleep 0.25; done) & producer=$!; trap 'kill \"$producer\" 2>/dev/null; exit 0' HUP TERM INT; count=0; while IFS= read -r token; do count=$((count+1)); printf '%s\\n' \"$count\" > \"$ack\"; done; kill \"$producer\" 2>/dev/null; wait \"$producer\" 2>/dev/null; exit 0",
            ),
            OsString::from("panelink-ten-pane"),
            root.as_os_str().to_owned(),
            OsString::from(pane.to_string()),
        ]
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
                    return Err("split client stopped before result")
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
        let arguments = shell_args(root, 1);
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
        pane: usize,
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
        *sequence += 1;
        let claim_frame = request(&mut stream, &mut request_id, &claim)
            .map_err(|_| "client claim request failed")?;
        let response = decode_response_payload(claim_frame.payload())
            .map_err(|_| "client claim response invalid")?;
        if !matches!(
            response.body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        ) {
            eprintln!("synthetic claim rejected: {:?}", response.body);
            return Err("client claim refused");
        }
        *sequence += 1;
        let owner_frame = request(&mut stream, &mut request_id, &describe)
            .map_err(|_| "client owner refresh request failed")?;
        let owner = decode_session_info(owner_frame.payload())
            .map_err(|_| "client owner refresh invalid")?;
        loop {
            match commands.recv() {
                Ok(ClientCommand::Stop) | Err(_) => return Ok(()),
                Ok(ClientCommand::Ready { reply }) => {
                    let _ = reply.send(Ok(()));
                }
                Ok(ClientCommand::Query { payload }) => {
                    request(&mut stream, &mut request_id, &payload)
                        .map_err(|_| "client layout query failed")?;
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
                        pane,
                        welcome.generation,
                        welcome.lease,
                        owner.owner_revision,
                        sequence,
                        probe,
                    );
                    if result.is_ok() {
                        *sequence += 1;
                    }
                    let deadline = Instant::now() + IO_TIMEOUT;
                    let result = result.and_then(|()| loop {
                        if marker_number(&ack_path).is_ok_and(|value| value > previous_ack) {
                            return Ok(());
                        }
                        if Instant::now() >= deadline {
                            return Err("child acknowledgment timed out");
                        }
                        thread::sleep(OWNER_POLL);
                    });
                    if result.is_err() {
                        let _ = reply.send(result);
                        return Err("child probe failed");
                    }
                    let _ = reply.send(result);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn send_probe(
        stream: &mut AuthenticatedStream,
        request_id: &mut u64,
        target: TargetRef,
        pane: usize,
        generation: u64,
        lease: u64,
        owner_revision: u64,
        sequence: &mut u64,
        probe: usize,
    ) -> WorkResult<()> {
        let text = format!("p{probe}n{pane}");
        let mut keys = text.chars().map(Key::Char).collect::<Vec<_>>();
        keys.push(Key::Enter);
        let payload = encode_key_input(&KeyInput {
            key: OperationKey {
                generation,
                lease,
                sequence: *sequence,
            },
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
            Ok(())
        } else {
            eprintln!("client input rejected: {:?}", decoded.body);
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

    fn percentile_float(samples: &[f64], fraction: f64) -> Option<f64> {
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let rank = ((fraction * sorted.len() as f64).ceil() as usize).max(1);
        sorted
            .get(rank.saturating_sub(1).min(sorted.len() - 1))
            .copied()
    }

    fn json_num(value: Option<u64>) -> String {
        value.map_or_else(|| "null".to_owned(), |value| value.to_string())
    }
    fn json_float(value: Option<f64>) -> String {
        value.map_or_else(|| "null".to_owned(), |value| format!("{value:.2}"))
    }

    fn print_workspace(workspace: &Workspace) {
        print!("{{\"workspace\":{},\"owner_ticks\":{{\"count\":{},\"p50_us\":{},\"p95_us\":{},\"max_us\":{}}},\"child_ack\":{{\"count\":{},\"p50_us\":{},\"p95_us\":{},\"max_us\":{}}},\"process_samples\":{},\"children_rss_bytes\":{{\"p50\":{},\"p95\":{},\"max\":{}}},\"children_cpu_percent\":{{\"p50\":{},\"p95\":{},\"max\":{}}},\"history_ok\":{},\"layout_ok\":{},\"shutdown_ms\":{}}}",
            workspace.index, workspace.owner_ticks.len(), json_num(percentile(&workspace.owner_ticks, 0.50)), json_num(percentile(&workspace.owner_ticks, 0.95)), json_num(workspace.owner_ticks.iter().copied().max()),
            workspace.probe_latencies.len(), json_num(percentile(&workspace.probe_latencies, 0.50)), json_num(percentile(&workspace.probe_latencies, 0.95)), json_num(workspace.probe_latencies.iter().copied().max()),
            workspace.rss_bytes.len(), json_num(percentile(&workspace.rss_bytes, 0.50)), json_num(percentile(&workspace.rss_bytes, 0.95)), json_num(workspace.rss_bytes.iter().copied().max()),
            json_float(percentile_float(&workspace.cpu_percent, 0.50)), json_float(percentile_float(&workspace.cpu_percent, 0.95)), json_float(workspace.cpu_percent.iter().copied().reduce(f64::max)),
            workspace.history_ok, workspace.layout_ok, workspace.shutdown_ms.map_or_else(|| "null".to_owned(), |value| value.to_string()));
    }

    pub fn main() -> WorkResult<()> {
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
        let start = Instant::now();
        for workspace in &mut workspaces {
            workspace.next_probe = start + Duration::from_secs(3);
        }
        for workspace in &workspaces {
            fs::write(workspace.root.join("workload.start"), b"go\n")
                .map_err(|_| "synthetic producer start gate failed")?;
        }
        let stop_at = start + Duration::from_secs(SECONDS);
        let mut next_sample = start + SAMPLE_EVERY;
        while Instant::now() < stop_at {
            let now = Instant::now();
            for workspace in &mut workspaces {
                workspace.advance(now, stop_at)?;
            }
            if now >= next_sample {
                for workspace in &mut workspaces {
                    workspace.sample_children()?;
                }
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
        println!("{{\"schema_version\":1,\"synthetic\":true,\"spec_ten_agent_acceptance\":false,\"server_limit_model\":\"five independent two-pane workspaces and storage domains in one process\",\"workspaces\":{WORKSPACES},\"panes\":{},\"workload_seconds\":{SECONDS},\"cleanup_processes_verified_stopped\":{},\"results\":[", WORKSPACES * PANES, cleanup_verified);
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
