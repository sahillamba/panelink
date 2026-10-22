use super::*;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod macos_tests {
    use super::*;
    use crate::handshake_protocol::{decode_welcome, Hello, CAPABILITY_INPUT, CAPABILITY_READ};
    use crate::launch::LaunchContext;
    use crate::managed_launch_protocol::{EnvironmentEntry, LaunchRequest};
    use crate::multi_startup::{StartedMultiServer, StartupLaunch};
    use crate::pane_layout::SplitDirection;
    use crate::protocol::{OperationKey, OperationStatus, Outcome};
    use crate::protocol_wire::{Frame, MessageKind};
    use crate::response_protocol::{decode_response_payload, ResponseBody};
    use crate::screen_checkpoint::decode as decode_checkpoint;
    use crate::session_protocol::{
        decode_session_info, encode_session_hello, encode_session_request, LeaseRef, SessionHello,
        SessionRequest,
    };
    use crate::split_protocol::{decode_split_result, encode_split_request, SplitRequest};
    use crate::terminal::Key;
    use crate::transport::{AuthenticatedStream, OwnedFrame};
    use crate::ui_protocol::{
        decode_layout_reply, decode_screen_reply, encode_key_input, encode_layout_query,
        encode_screen_query, planned_split_size, KeyInput,
    };
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::sync::{mpsc, Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct Gate {
        state: Mutex<(bool, bool)>,
        changed: Condvar,
    }

    impl Gate {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new((false, false)),
                changed: Condvar::new(),
            })
        }

        fn hook(
            self: &Arc<Self>,
            blocked_stage: crate::launch_worker::LaunchStage,
        ) -> Arc<dyn Fn(crate::launch_worker::LaunchStage) + Send + Sync> {
            let gate = Arc::clone(self);
            Arc::new(move |stage| {
                if stage != blocked_stage {
                    return;
                }
                let mut state = gate.state.lock().unwrap();
                state.0 = true;
                gate.changed.notify_all();
                while !state.1 {
                    state = gate.changed.wait(state).unwrap();
                }
            })
        }

        fn is_entered(&self) -> bool {
            self.state.lock().unwrap().0
        }

        fn wait_entered(&self, driver: &mut MultiServerDriver) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !self.is_entered() {
                driver.tick(|| 10).unwrap();
                assert!(
                    Instant::now() < deadline,
                    "launch worker did not reach the gate"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn release(&self) {
            let mut state = self.state.lock().unwrap();
            state.1 = true;
            self.changed.notify_all();
        }
    }

    impl Drop for Gate {
        fn drop(&mut self) {
            if let Ok(mut state) = self.state.lock() {
                state.1 = true;
                self.changed.notify_all();
            }
        }
    }

    struct ReleaseGate(Arc<Gate>);

    impl Drop for ReleaseGate {
        fn drop(&mut self) {
            self.0.release();
        }
    }

    struct Client {
        stream: AuthenticatedStream,
        next_id: u64,
        generation: u64,
        lease: u64,
    }

    impl Client {
        fn connect(driver: &mut MultiServerDriver, resume: Option<LeaseRef>) -> Self {
            let (server, client) = UnixStream::pair().unwrap();
            driver
                .attach(AuthenticatedStream::new(server).unwrap())
                .unwrap();
            let mut stream = AuthenticatedStream::new(client).unwrap();
            let hello = encode_session_hello(&SessionHello {
                hello: Hello {
                    nonce: 37,
                    required_capabilities: CAPABILITY_READ | CAPABILITY_INPUT,
                    optional_capabilities: 0,
                },
                resume,
            })
            .unwrap();
            let welcome = exchange(driver, &mut stream, MessageKind::Hello, 0, &hello);
            assert_eq!(welcome.kind(), MessageKind::Welcome);
            let welcome = decode_welcome(welcome.payload()).unwrap();
            stream.enable_snapshot_reads().unwrap();
            Self {
                stream,
                next_id: 0,
                generation: welcome.generation,
                lease: welcome.lease,
            }
        }

        fn send(&mut self, driver: &mut MultiServerDriver, payload: &[u8]) -> OwnedFrame {
            self.next_id += 1;
            exchange(
                driver,
                &mut self.stream,
                MessageKind::Request,
                self.next_id,
                payload,
            )
        }
    }

    fn exchange(
        driver: &mut MultiServerDriver,
        stream: &mut AuthenticatedStream,
        kind: MessageKind,
        request_id: u64,
        payload: &[u8],
    ) -> OwnedFrame {
        let (send, receive) = mpsc::sync_channel(1);
        let deadline = Instant::now() + Duration::from_secs(8);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let result = stream
                    .write_frame(&Frame::new(kind, request_id, payload).unwrap(), deadline)
                    .and_then(|_| stream.read_frame(deadline));
                let _ = send.send(result);
            });
            loop {
                driver.tick(|| 10).unwrap();
                match receive.try_recv() {
                    Ok(result) => return result.unwrap(),
                    Err(mpsc::TryRecvError::Disconnected) => panic!("socket exchange ended"),
                    Err(mpsc::TryRecvError::Empty) => {
                        assert!(Instant::now() < deadline, "socket exchange timed out");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            }
        })
    }

    fn fixture() -> (
        Fixture,
        MultiServerDriver,
        crate::control_protocol::TargetRef,
    ) {
        let root = PathBuf::from("/private/tmp").join(format!(
            "panelink-async-launch-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let fixture = Fixture(root.clone());
        let server_dir = root.join("server");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&server_dir)
            .unwrap();
        let terminfo = root.join("terminfo");
        fs::DirBuilder::new().mode(0o700).create(&terminfo).unwrap();
        let storage = crate::server_storage::ServerStorage::initialize(&server_dir).unwrap();
        let context = LaunchContext {
            working_directory: root.clone(),
            environment: [
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (OsString::from("HOME"), root.as_os_str().to_owned()),
            ]
            .into_iter()
            .collect(),
        };
        let arguments = [
            OsString::from("-c"),
            OsString::from(
                "printf 'ASYNC-ROOT-READY\\r\\n'; IFS= read -r line; printf 'ASYNC-ROOT-INPUT:%s\\r\\n' \"$line\"; while :; do /bin/sleep 1; done",
            ),
        ];
        let executable = OsString::from("/bin/sh");
        let started = StartedMultiServer::start(
            storage,
            &terminfo,
            &StartupLaunch {
                executable: &executable,
                arguments: &arguments,
                context: &context,
                rows: 8,
                columns: 60,
            },
            true,
        )
        .unwrap();
        let target = started.startup().target.unwrap();
        (fixture, started.into_driver(), target)
    }

    fn split_payload(
        key: OperationKey,
        target: crate::control_protocol::TargetRef,
        revision: u64,
        rows: u16,
        columns: u16,
        cwd: &[u8],
        marker: &[u8],
    ) -> Vec<u8> {
        let script = [
            b"printf x >> '",
            marker,
            b"'; printf 'ASYNC-SPLIT-READY\\r\\n'; while :; do /bin/sleep 1; done",
        ]
        .concat();
        let arguments: [&[u8]; 2] = [b"-c", &script];
        let environment = [
            EnvironmentEntry {
                name: b"PATH",
                value: b"/usr/bin:/bin",
            },
            EnvironmentEntry {
                name: b"HOME",
                value: cwd,
            },
        ];
        let launch = LaunchRequest {
            key,
            rows,
            columns,
            requested_executable: b"/bin/sh",
            requested_working_directory: cwd,
            client_working_directory: cwd,
            arguments: &arguments,
            environment: &environment,
        };
        let mut payload = Vec::new();
        encode_split_request(
            &SplitRequest {
                key,
                target,
                expected_layout_revision: revision,
                direction: SplitDirection::Right,
                launch: &launch,
            },
            &mut payload,
        )
        .unwrap();
        payload
    }

    fn screen_text(
        driver: &mut MultiServerDriver,
        client: &mut Client,
        target: crate::control_protocol::TargetRef,
    ) -> String {
        let payload = encode_screen_query(target).unwrap();
        let frame = client.send(driver, &payload);
        assert_eq!(frame.kind(), MessageKind::Snapshot);
        let screen = decode_screen_reply(frame.payload()).unwrap();
        let checkpoint = decode_checkpoint(screen.checkpoint()).unwrap();
        (0..checkpoint.rows())
            .map(|row| {
                (0..checkpoint.columns())
                    .map(|column| checkpoint.cell(row, column).unwrap().text())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn blocked_split_keeps_sibling_io_and_reads_live_then_adopts_once() {
        for stage in [
            crate::launch_worker::LaunchStage::BeforeIntent,
            crate::launch_worker::LaunchStage::BeforeFinalize,
        ] {
            blocked_split_stage(stage);
        }
    }

    #[test]
    fn serialized_socket_splits_reach_four_panes_and_reject_fifth_without_launch() {
        let (fixture, mut driver, original_target) = fixture();
        let initial = driver.owner.layout_snapshot().unwrap();
        driver
            .owner
            .resize_layout(initial.revision, 8, 240, |effects, rows, columns| {
                effects.resize(rows, columns)
            })
            .unwrap();

        let mut client = Client::connect(&mut driver, None);
        let describe = encode_session_request(&SessionRequest::Describe).unwrap();
        let info = decode_session_info(client.send(&mut driver, &describe).payload()).unwrap();
        let claim_key = OperationKey {
            generation: client.generation,
            lease: client.lease,
            sequence: 1,
        };
        let claim = encode_session_request(&SessionRequest::Claim {
            key: claim_key,
            target: original_target,
            expected_owner_revision: info.owner_revision,
            ttl_seconds: 30,
        })
        .unwrap();
        assert!(matches!(
            decode_response_payload(client.send(&mut driver, &claim).payload())
                .unwrap()
                .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        ));
        // Split the original pane three times. The 240-column root leaves
        // enough width for each split and the final four-pane layout. Each request travels through the
        // authenticated socket path and settles before the next is issued.
        for index in 0..3 {
            let split_target = original_target;
            let layout =
                decode_layout_reply(client.send(&mut driver, &encode_layout_query()).payload())
                    .unwrap();
            let marker = fixture.0.join(format!("four-pane-{index}.marker"));
            let key = OperationKey {
                generation: client.generation,
                lease: client.lease,
                sequence: index as u64 + 2,
            };
            let (rows, columns) =
                planned_split_size(&layout, split_target, SplitDirection::Right).unwrap();
            let request = split_payload(
                key,
                split_target,
                layout.revision,
                rows,
                columns,
                fixture.0.as_os_str().as_bytes(),
                marker.as_os_str().as_bytes(),
            );
            let reply = client.send(&mut driver, &request);
            assert_eq!(
                reply.kind(),
                MessageKind::Response,
                "split reply: {}",
                String::from_utf8_lossy(reply.payload())
            );
            let result = decode_split_result(reply.payload()).unwrap();
            assert_eq!(result.outcome, Outcome::Applied, "split {index} failed");
            assert!(result.new_target.is_some(), "successful split has a target");

            let after =
                decode_layout_reply(client.send(&mut driver, &encode_layout_query()).payload())
                    .unwrap();
            assert_eq!(after.panes.iter().flatten().count(), index + 2);

            let deadline = Instant::now() + Duration::from_secs(3);
            while !marker.exists() {
                assert!(
                    Instant::now() < deadline,
                    "split child did not reach marker"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(fs::read(&marker).unwrap().len(), 1);
        }

        let full = decode_layout_reply(client.send(&mut driver, &encode_layout_query()).payload())
            .unwrap();
        assert_eq!(
            full.panes.iter().flatten().count(),
            crate::pane_layout::MAX_PANES
        );

        let marker = fixture.0.join("fifth-pane.marker");
        let key = OperationKey {
            generation: client.generation,
            lease: client.lease,
            sequence: 5,
        };
        let request = split_payload(
            key,
            original_target,
            full.revision,
            8,
            20,
            fixture.0.as_os_str().as_bytes(),
            marker.as_os_str().as_bytes(),
        );
        let reply = client.send(&mut driver, &request);
        assert_eq!(reply.kind(), MessageKind::Response);
        let result = decode_split_result(reply.payload()).unwrap();
        assert_eq!(result.outcome, Outcome::Failed);
        assert!(result.new_target.is_none());

        let retry = decode_split_result(client.send(&mut driver, &request).payload()).unwrap();
        assert_eq!(retry, result, "retry must return the retained failure");

        let after = decode_layout_reply(client.send(&mut driver, &encode_layout_query()).payload())
            .unwrap();
        assert_eq!(
            after.panes.iter().flatten().count(),
            crate::pane_layout::MAX_PANES
        );

        // Give any incorrectly prepared asynchronous fifth launch a bounded
        // window to reveal itself through its marker.
        let deadline = Instant::now() + Duration::from_millis(250);
        while Instant::now() < deadline {
            driver.tick(|| 10).unwrap();
            assert!(
                !marker.exists(),
                "capacity rejection launched a fifth child"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        for index in 0..3 {
            assert_eq!(
                fs::read(fixture.0.join(format!("four-pane-{index}.marker")))
                    .unwrap()
                    .len(),
                1
            );
        }
    }

    fn blocked_split_stage(stage: crate::launch_worker::LaunchStage) {
        let (fixture, mut driver, target) = fixture();
        let gate = Gate::new();
        let _release_gate = ReleaseGate(Arc::clone(&gate));
        let mut owner_client = Client::connect(&mut driver, None);
        let describe = encode_session_request(&SessionRequest::Describe).unwrap();
        let info =
            decode_session_info(owner_client.send(&mut driver, &describe).payload()).unwrap();
        let claim_key = OperationKey {
            generation: owner_client.generation,
            lease: owner_client.lease,
            sequence: 1,
        };
        let claim = encode_session_request(&SessionRequest::Claim {
            key: claim_key,
            target,
            expected_owner_revision: info.owner_revision,
            ttl_seconds: 30,
        })
        .unwrap();
        let claim_reply = owner_client.send(&mut driver, &claim);
        assert!(matches!(
            decode_response_payload(claim_reply.payload()).unwrap().body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        ));

        let mut observer = Client::connect(
            &mut driver,
            Some(LeaseRef {
                generation: owner_client.generation,
                lease: owner_client.lease,
            }),
        );
        let split_key = OperationKey {
            generation: observer.generation,
            lease: observer.lease,
            sequence: 2,
        };
        let layout_frame = observer.send(&mut driver, &encode_layout_query());
        let layout = decode_layout_reply(layout_frame.payload()).unwrap();
        let marker = fixture.0.join("split.marker");
        let (rows, columns) = planned_split_size(&layout, target, SplitDirection::Right).unwrap();
        let bytes = split_payload(
            split_key,
            target,
            layout.revision,
            rows,
            columns,
            fixture.0.as_os_str().as_bytes(),
            marker.as_os_str().as_bytes(),
        );
        driver
            .launch_worker
            .as_ref()
            .unwrap()
            .set_test_hook(gate.hook(stage));

        let (reply_send, reply_recv) = mpsc::sync_channel(1);
        std::thread::scope(|scope| {
            let mut split_client = observer;
            let request_bytes = bytes.clone();
            scope.spawn(move || {
                split_client.next_id += 1;
                let deadline = Instant::now() + Duration::from_secs(8);
                let result = split_client
                    .stream
                    .write_frame(
                        &Frame::new(MessageKind::Request, split_client.next_id, &request_bytes)
                            .unwrap(),
                        deadline,
                    )
                    .and_then(|_| split_client.stream.read_frame(deadline));
                let _ = reply_send.send((split_client, result));
            });
            gate.wait_entered(&mut driver);
            assert!(driver.owner.async_split_active());
            assert_eq!(driver.owner.async_split_key(), Some(split_key));

            let exact = driver.owner.begin_async_split(&bytes, 10, true).unwrap();
            assert!(matches!(
                exact,
                crate::server::multi_session::AsyncSplitAdmission::Pending
            ));
            let mut changed = bytes.clone();
            *changed.last_mut().unwrap() ^= 1;
            assert!(driver.owner.begin_async_split(&changed, 10, true).is_err());

            let describe = encode_session_request(&SessionRequest::Describe).unwrap();
            let read_info =
                decode_session_info(owner_client.send(&mut driver, &describe).payload()).unwrap();
            assert!(read_info.owner_revision >= info.owner_revision);
            let input_key = OperationKey {
                sequence: 3,
                ..claim_key
            };
            let keys = [Key::Char('O'), Key::Char('K'), Key::Enter];
            let input = encode_key_input(&KeyInput {
                key: input_key,
                target,
                expected_owner_revision: read_info.owner_revision,
                keys: &keys,
            })
            .unwrap();
            let input_reply = owner_client.send(&mut driver, &input);
            assert!(matches!(
                decode_response_payload(input_reply.payload()).unwrap().body,
                ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
            ));
            let deadline = Instant::now() + Duration::from_secs(4);
            loop {
                let text = screen_text(&mut driver, &mut owner_client, target);
                if text.contains("ASYNC-ROOT-INPUT:OK") {
                    break;
                }
                assert!(Instant::now() < deadline, "sibling PTY input was stalled");
            }
            let layout_frame = owner_client.send(&mut driver, &encode_layout_query());
            let layout = decode_layout_reply(layout_frame.payload()).unwrap();
            assert_eq!(
                layout.panes.iter().flatten().count(),
                1,
                "split is not committed before finalization"
            );
            assert!(
                reply_recv.try_recv().is_err(),
                "split must remain pending at the gate"
            );

            gate.release();
            let deadline = Instant::now() + Duration::from_secs(8);
            let (mut split_client, result_frame) = loop {
                driver.tick(|| 10).unwrap();
                match reply_recv.try_recv() {
                    Ok(value) => break value,
                    Err(mpsc::TryRecvError::Disconnected) => panic!("split reply worker ended"),
                    Err(mpsc::TryRecvError::Empty) => {
                        assert!(Instant::now() < deadline, "split did not settle");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            };
            let frame = result_frame.unwrap();
            assert_eq!(frame.kind(), MessageKind::Response);
            let result = decode_split_result(frame.payload()).unwrap();
            assert_eq!(result.outcome, Outcome::Applied);
            assert!(result.new_target.is_some());
            assert!(!driver.owner.async_split_active());
            let layout_frame = owner_client.send(&mut driver, &encode_layout_query());
            assert_eq!(
                decode_layout_reply(layout_frame.payload())
                    .unwrap()
                    .panes
                    .iter()
                    .flatten()
                    .count(),
                2
            );

            let duplicate = driver.owner.begin_async_split(&bytes, 10, true).unwrap();
            assert!(matches!(
                duplicate,
                crate::server::multi_session::AsyncSplitAdmission::Terminal(done)
                    if done == result
            ));
            let duplicate_reply = split_client.send(&mut driver, &bytes);
            let duplicate_result = decode_split_result(duplicate_reply.payload()).unwrap();
            assert_eq!(duplicate_result, result);
            let deadline = Instant::now() + Duration::from_secs(3);
            while !marker.exists() {
                assert!(Instant::now() < deadline, "new child did not reach marker");
                std::thread::sleep(Duration::from_millis(1));
            }
            let marker_count = fs::read(&marker).unwrap().len();
            std::thread::sleep(Duration::from_millis(30));
            assert_eq!(fs::read(&marker).unwrap().len(), marker_count);
            assert_eq!(marker_count, 1, "exact retry must not spawn a second child");
        });
    }

    #[test]
    fn disconnected_split_waiter_does_not_cancel_or_duplicate_committed_launch() {
        let (fixture, mut driver, target) = fixture();
        let gate = Gate::new();
        let _release_gate = ReleaseGate(Arc::clone(&gate));
        let mut client = Client::connect(&mut driver, None);
        let lease = LeaseRef {
            generation: client.generation,
            lease: client.lease,
        };
        let describe = encode_session_request(&SessionRequest::Describe).unwrap();
        let info = decode_session_info(client.send(&mut driver, &describe).payload()).unwrap();
        let claim_key = OperationKey {
            generation: client.generation,
            lease: client.lease,
            sequence: 1,
        };
        let claim = encode_session_request(&SessionRequest::Claim {
            key: claim_key,
            target,
            expected_owner_revision: info.owner_revision,
            ttl_seconds: 30,
        })
        .unwrap();
        client.send(&mut driver, &claim);
        let layout =
            decode_layout_reply(client.send(&mut driver, &encode_layout_query()).payload())
                .unwrap();
        let marker = fixture.0.join("disconnect.marker");
        let split_key = OperationKey {
            sequence: 2,
            ..claim_key
        };
        let (rows, columns) = planned_split_size(&layout, target, SplitDirection::Right).unwrap();
        let bytes = split_payload(
            split_key,
            target,
            layout.revision,
            rows,
            columns,
            fixture.0.as_os_str().as_bytes(),
            marker.as_os_str().as_bytes(),
        );
        driver
            .launch_worker
            .as_ref()
            .unwrap()
            .set_test_hook(gate.hook(crate::launch_worker::LaunchStage::BeforeFinalize));

        let mut split_client = Client::connect(&mut driver, Some(lease));
        let (sent, sent_rx) = mpsc::sync_channel(1);

        std::thread::scope(|scope| {
            scope.spawn(|| {
                split_client.next_id += 1;
                let deadline = Instant::now() + Duration::from_secs(8);
                let result = split_client.stream.write_frame(
                    &Frame::new(MessageKind::Request, split_client.next_id, &bytes).unwrap(),
                    deadline,
                );
                let _ = sent.send(result.is_ok());
                drop(split_client);
            });
            loop {
                driver.tick(|| 10).unwrap();
                match sent_rx.try_recv() {
                    Ok(true) => break,
                    Ok(false) => panic!("split request write failed"),
                    Err(mpsc::TryRecvError::Disconnected) => panic!("split sender ended"),
                    Err(mpsc::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(1)),
                }
            }
            gate.wait_entered(&mut driver);
            assert!(driver.owner.async_split_active());
            gate.release();

            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                driver.tick(|| 10).unwrap();
                if !driver.owner.async_split_active() {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "disconnected split did not commit"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        });

        let mut retry = Client::connect(&mut driver, Some(lease));
        let retry_reply = retry.send(&mut driver, &bytes);
        let result = decode_split_result(retry_reply.payload()).unwrap();
        assert_eq!(result.outcome, Outcome::Applied);
        assert!(result.new_target.is_some());
        let layout =
            decode_layout_reply(retry.send(&mut driver, &encode_layout_query()).payload()).unwrap();
        assert_eq!(layout.panes.iter().flatten().count(), 2);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !marker.exists() {
            assert!(Instant::now() < deadline, "new child did not reach marker");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(fs::read(&marker).unwrap().len(), 1);
    }

    #[test]
    fn dropping_driver_while_before_intent_is_gated_keeps_singleton_until_cancel() {
        let (fixture, mut driver, target) = fixture();
        let gate = Gate::new();
        let _release_gate = ReleaseGate(Arc::clone(&gate));
        let mut client = Client::connect(&mut driver, None);
        let describe = encode_session_request(&SessionRequest::Describe).unwrap();
        let info = decode_session_info(client.send(&mut driver, &describe).payload()).unwrap();
        let key = OperationKey {
            generation: client.generation,
            lease: client.lease,
            sequence: 1,
        };
        let claim = encode_session_request(&SessionRequest::Claim {
            key,
            target,
            expected_owner_revision: info.owner_revision,
            ttl_seconds: 30,
        })
        .unwrap();
        client.send(&mut driver, &claim);
        let layout =
            decode_layout_reply(client.send(&mut driver, &encode_layout_query()).payload())
                .unwrap();
        let marker = fixture.0.join("shutdown.marker");
        let split_key = OperationKey { sequence: 2, ..key };
        let (rows, columns) = planned_split_size(&layout, target, SplitDirection::Right).unwrap();
        let bytes = split_payload(
            split_key,
            target,
            layout.revision,
            rows,
            columns,
            fixture.0.as_os_str().as_bytes(),
            marker.as_os_str().as_bytes(),
        );
        driver
            .launch_worker
            .as_ref()
            .unwrap()
            .set_test_hook(gate.hook(crate::launch_worker::LaunchStage::BeforeIntent));
        let (reply_send, _reply_recv) = mpsc::sync_channel(1);
        let server_dir = fixture.0.join("server");
        std::thread::scope(|scope| {
            scope.spawn(move || {
                client.next_id += 1;
                let deadline = Instant::now() + Duration::from_secs(10);
                let _ = client
                    .stream
                    .write_frame(
                        &Frame::new(MessageKind::Request, client.next_id, &bytes).unwrap(),
                        deadline,
                    )
                    .and_then(|_| client.stream.read_frame(deadline));
                let _ = reply_send.send(());
            });
            gate.wait_entered(&mut driver);
            drop(driver);
            let lock_error =
                crate::server_storage::ServerStorage::lock_for_maintenance(&server_dir)
                    .unwrap_err();
            assert_eq!(
                lock_error.kind(),
                std::io::ErrorKind::WouldBlock,
                "the detached launch worker must retain singleton exclusion"
            );
            gate.release();
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if crate::server_storage::ServerStorage::lock_for_maintenance(&server_dir).is_ok() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "launch worker did not observe cancellation"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            !marker.exists(),
            "cancellation before Intent must prevent child spawn"
        );
    }
}
