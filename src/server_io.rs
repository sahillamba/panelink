//! Bounded worker threads for authenticated server connections.
//!
//! Each admitted connection owns one authenticated stream and is the only
//! writer for that stream. Workers validate protocol framing and typed request
//! payloads, then hand one request at a time to the server's state-owner loop.
//! They never access process effects or cancel work already accepted by the
//! owner loop when a connection goes away.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use crate::control_protocol::decode_request_payload;
use crate::event_protocol::{encode_event, Event};
use crate::handshake_protocol::{
    encode_welcome, validate_welcome, Welcome, CAPABILITY_EVENTS, CAPABILITY_READ,
};
use crate::protocol_wire::{Frame, MessageKind};
use crate::response_protocol::decode_response_payload;
use crate::server::Connection;
use crate::session_protocol::{
    decode_session_hello, decode_session_info, decode_session_request, SessionHello, SessionRequest,
};
use crate::transport::{AuthenticatedStream, OwnedFrame};
use crate::watch_protocol::{
    decode_reconciliation, decode_watch_request, encode_watch_end, WatchEnd, WatchRequest,
    WatchStatus, MAX_WATCH_EVENTS,
};
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Maximum number of live connection workers.
pub const MAX_CONNECTIONS: usize = 8;

/// Bounded number of owner envelopes waiting for the state-owner loop.
pub const OWNER_QUEUE_CAPACITY: usize = 8;

/// Maximum time allowed for one complete incoming frame.
pub const READ_DEADLINE: Duration = Duration::from_secs(1);

/// Maximum time allowed for one complete outgoing frame.
pub const WRITE_DEADLINE: Duration = Duration::from_millis(250);

/// Maximum total time to wait for one owner reply.
pub const OWNER_REPLY_DEADLINE: Duration = Duration::from_secs(2);

/// Maximum interval between stop checks while waiting for an owner reply.
pub const STOP_POLL_INTERVAL: Duration = Duration::from_millis(20);

const MAX_ERROR_LABEL_BYTES: usize = 4_096;
const DETACHED_ACK: [u8; 4] = [0, 2, 0, 2];

/// One request for the server's single-threaded state owner.
pub enum OwnerRequest {
    /// Initial authenticated session handshake.
    Hello(SessionHello),
    /// One validated, opaque typed request from an established connection.
    Request {
        /// Connection capabilities and lease created by StateOwner::connect.
        connection: Connection,
        /// Exact schema-1 control or schema-2 session payload bytes.
        payload: Vec<u8>,
    },
}

impl fmt::Debug for OwnerRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hello(hello) => f.debug_tuple("Hello").field(hello).finish(),
            Self::Request { payload, .. } => f
                .debug_struct("Request")
                .field("payload_len", &payload.len())
                .field("payload", &"<redacted>")
                .finish(),
        }
    }
}

/// One bounded reply from the state-owner loop.
pub enum OwnerReply {
    /// Successful initial connection and negotiated Welcome payload.
    Welcome {
        /// The exact server-created connection returned by StateOwner::connect.
        connection: Connection,
        /// Welcome fields paired with that connection.
        welcome: Welcome,
    },
    /// Already-encoded schema-1 response or schema-2 session-info payload.
    Response(Vec<u8>),
    /// One bounded metadata-event page followed by its terminal watch result.
    Watch {
        /// Ordered, complete metadata events; never terminal output or input.
        events: Vec<Event>,
        /// Final status and cursor for this finite watch request.
        end: WatchEnd,
    },
    /// Fixed, nonempty lower-snake-case error label; never caller content.
    Error(&'static str),
    /// Acknowledge a validated detach request and close only this connection.
    Detached,
}

impl fmt::Debug for OwnerReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Welcome { .. } => f.write_str("Welcome { .. }"),
            Self::Response(payload) => f
                .debug_struct("Response")
                .field("payload_len", &payload.len())
                .field("payload", &"<redacted>")
                .finish(),
            Self::Watch { events, .. } => f
                .debug_struct("Watch")
                .field("event_count", &events.len())
                .field("events", &"<redacted>")
                .field("end", &"<redacted>")
                .finish(),
            Self::Error(_) => f.write_str("Error(<redacted label>)"),
            Self::Detached => f.write_str("Detached"),
        }
    }
}

/// One owner request with a one-slot reply channel.
pub struct OwnerEnvelope {
    /// Typed owner request. Payload bytes remain redacted in Debug.
    pub request: OwnerRequest,
    reply: SyncSender<OwnerReply>,
}

impl OwnerEnvelope {
    /// Try to send the single reply for this envelope without blocking the
    /// state-owner thread.
    pub fn try_reply(&self, reply: OwnerReply) -> Result<(), TrySendError<OwnerReply>> {
        self.reply.try_send(reply)
    }
}

impl fmt::Debug for OwnerEnvelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnerEnvelope")
            .field("request", &self.request)
            .field("reply", &"<bounded>")
            .finish()
    }
}

/// Why an authenticated stream was not admitted to the worker pool.
#[derive(Debug)]
pub enum StartError {
    /// All eight connection slots are occupied.
    AtCapacity,
    /// Shutdown has started; no further connection is admitted.
    Stopped,
    /// The operating system refused to create a worker thread.
    Spawn(io::Error),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AtCapacity => f.write_str("connection worker capacity is full"),
            Self::Stopped => f.write_str("connection worker hub is stopping"),
            Self::Spawn(_) => f.write_str("connection worker could not be started"),
        }
    }
}

impl std::error::Error for StartError {}

/// Bounded owner queue and connection-worker lifecycle.
///
/// The parent event loop reads envelopes from [`owner_receiver`](Self::owner_receiver)
/// and processes them on the one state-owner thread. Only this hub's owning
/// thread mutates its worker list; worker threads update only the bounded
/// active-count atomic and publish through the capacity-eight channel.
pub struct WorkerHub {
    owner_sender: SyncSender<OwnerEnvelope>,
    owner_receiver: Receiver<OwnerEnvelope>,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    workers: Vec<JoinHandle<()>>,
}

impl WorkerHub {
    /// Create a hub with a capacity-eight owner queue and no workers.
    pub fn new() -> Self {
        let (owner_sender, owner_receiver) = mpsc::sync_channel(OWNER_QUEUE_CAPACITY);
        Self {
            owner_sender,
            owner_receiver,
            stop: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicUsize::new(0)),
            workers: Vec::with_capacity(MAX_CONNECTIONS),
        }
    }

    /// Borrow the bounded receiver for the parent state-owner event loop.
    pub fn owner_receiver(&self) -> &Receiver<OwnerEnvelope> {
        &self.owner_receiver
    }

    /// Return the number of workers that have reserved a live connection slot.
    pub fn active_connections(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    /// Start one worker for an already-authenticated stream.
    ///
    /// The ninth live connection is rejected before a thread is spawned. The
    /// caller must obtain the stream through `AuthenticatedStream::new` or
    /// `PrivateListener::accept`; this API accepts no raw UnixStream.
    pub fn start(&mut self, stream: AuthenticatedStream) -> Result<(), StartError> {
        self.reap_finished();
        if self.stop.load(Ordering::Acquire) {
            return Err(StartError::Stopped);
        }
        if self.workers.len() >= MAX_CONNECTIONS {
            return Err(StartError::AtCapacity);
        }
        reserve_connection(&self.active)?;

        let active_guard = ActiveGuard(Arc::clone(&self.active));
        let stop = Arc::clone(&self.stop);
        let sender = self.owner_sender.clone();
        let result = thread::Builder::new()
            .name("panelink-connection".to_owned())
            .spawn(move || {
                let _active_guard = active_guard;
                run_worker(stream, sender, stop);
            });

        match result {
            Ok(worker) => {
                self.workers.push(worker);
                Ok(())
            }
            Err(error) => Err(StartError::Spawn(error)),
        }
    }

    /// Join only finished workers during the normal server loop.
    pub fn reap_finished(&mut self) {
        let mut index = 0;
        while index < self.workers.len() {
            if self.workers[index].is_finished() {
                let worker = self.workers.swap_remove(index);
                let _ = worker.join();
            } else {
                index += 1;
            }
        }
    }

    /// Stop connection I/O and join all workers. This does not signal, cancel,
    /// or drop the server's process-effect sink. Accepted owner envelopes are
    /// independent work and the parent loop remains responsible for finishing
    /// them even if the corresponding reply receiver has gone away.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        let current = thread::current().id();
        for worker in self.workers.drain(..) {
            if worker.thread().id() != current {
                let _ = worker.join();
            }
        }
    }
}

impl Default for WorkerHub {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for WorkerHub {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct ActiveGuard(Arc<AtomicUsize>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn reserve_connection(active: &AtomicUsize) -> Result<(), StartError> {
    let mut current = active.load(Ordering::Acquire);
    loop {
        if current >= MAX_CONNECTIONS {
            return Err(StartError::AtCapacity);
        }
        match active.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return Ok(()),
            Err(observed) => current = observed,
        }
    }
}

fn run_worker(
    mut stream: AuthenticatedStream,
    owner_sender: SyncSender<OwnerEnvelope>,
    stop: Arc<AtomicBool>,
) {
    if stop.load(Ordering::Acquire) {
        return;
    }
    let Some(frame) = read_frame(&mut stream, &stop) else {
        return;
    };
    if frame.kind() != MessageKind::Hello || frame.request_id() != 0 {
        return;
    }
    let Ok(hello) = decode_session_hello(frame.payload()) else {
        return;
    };
    drop(frame);

    let Some(reply) = submit_owner(&owner_sender, &stop, OwnerRequest::Hello(hello)) else {
        return;
    };
    let (connection, welcome) = match reply {
        OwnerReply::Welcome {
            connection,
            welcome,
        } => (connection, welcome),
        OwnerReply::Error(label) => {
            let _ = write_error(&mut stream, 0, label, &stop);
            return;
        }
        OwnerReply::Response(_) | OwnerReply::Watch { .. } | OwnerReply::Detached => return,
    };
    if connection.generation() != welcome.generation
        || connection.lease() != welcome.lease
        || connection.selected_capabilities() != welcome.selected_capabilities
        || validate_welcome(&hello.hello, &welcome).is_err()
    {
        return;
    }
    if welcome.selected_capabilities & CAPABILITY_EVENTS != 0
        && stream.enable_metadata_events().is_err()
    {
        return;
    }
    let Ok(welcome_payload) = encode_welcome(&welcome) else {
        return;
    };
    if write_frame(
        &mut stream,
        MessageKind::Welcome,
        0,
        &welcome_payload,
        &stop,
    )
    .is_err()
    {
        return;
    }

    let mut last_request_id = 0;
    while !stop.load(Ordering::Acquire) {
        let Some((request_id, schema, payload)) = read_request(&mut stream, &stop, last_request_id)
        else {
            return;
        };
        last_request_id = request_id;

        if matches!(schema, RequestSchema::Watch(_))
            && connection.selected_capabilities() & (CAPABILITY_READ | CAPABILITY_EVENTS)
                != (CAPABILITY_READ | CAPABILITY_EVENTS)
        {
            if write_error(&mut stream, request_id, "capability_denied", &stop).is_err() {
                return;
            }
            continue;
        }

        let Some(reply) = submit_owner(
            &owner_sender,
            &stop,
            OwnerRequest::Request {
                connection,
                payload,
            },
        ) else {
            return;
        };

        match reply {
            OwnerReply::Response(payload) => {
                if !response_matches(schema, &payload) {
                    return;
                }
                if write_frame(
                    &mut stream,
                    MessageKind::Response,
                    request_id,
                    &payload,
                    &stop,
                )
                .is_err()
                {
                    return;
                }
            }
            OwnerReply::Watch { events, end } => {
                let RequestSchema::Watch(WatchRequest::Watch {
                    target,
                    cursor,
                    limit,
                    ..
                }) = schema
                else {
                    return;
                };
                if write_watch_reply(
                    &mut stream,
                    WatchReplyContext {
                        request_id,
                        target,
                        requested_cursor: cursor,
                        limit,
                    },
                    &events,
                    &end,
                    &stop,
                )
                .is_err()
                {
                    return;
                }
            }
            OwnerReply::Error(label) => {
                if write_error(&mut stream, request_id, label, &stop).is_err() {
                    return;
                }
            }
            OwnerReply::Detached => {
                if !matches!(schema, RequestSchema::Session(SessionRequest::Detach)) {
                    return;
                }
                let _ = write_frame(
                    &mut stream,
                    MessageKind::Response,
                    request_id,
                    &DETACHED_ACK,
                    &stop,
                );
                return;
            }
            OwnerReply::Welcome { .. } => return,
        }
    }
}

#[derive(Clone, Copy)]
enum RequestSchema {
    Control,
    Session(SessionRequest),
    Watch(WatchRequest),
}

fn validate_request_payload(payload: &[u8]) -> Result<RequestSchema, ()> {
    match payload.get(..2) {
        Some([0, 1]) => decode_request_payload(payload)
            .map(|_| RequestSchema::Control)
            .map_err(|_| ()),
        Some([0, 2]) => decode_session_request(payload)
            .map(RequestSchema::Session)
            .map_err(|_| ()),
        Some([0, 4]) => decode_watch_request(payload)
            .map(RequestSchema::Watch)
            .map_err(|_| ()),
        _ => Err(()),
    }
}

fn read_request(
    stream: &mut AuthenticatedStream,
    stop: &AtomicBool,
    last_request_id: u64,
) -> Option<(u64, RequestSchema, Vec<u8>)> {
    let frame = read_frame(stream, stop)?;
    if frame.kind() != MessageKind::Request || frame.request_id() == 0 {
        return None;
    }
    let request_id = frame.request_id();
    if request_id <= last_request_id {
        return None;
    }
    let schema = validate_request_payload(frame.payload()).ok()?;
    // Transfer its one bounded allocation; do not duplicate input/paste bytes.
    Some((request_id, schema, frame.into_payload()))
}

fn response_matches(schema: RequestSchema, payload: &[u8]) -> bool {
    match schema {
        RequestSchema::Control => decode_response_payload(payload).is_ok(),
        RequestSchema::Session(SessionRequest::Describe) => decode_session_info(payload).is_ok(),
        RequestSchema::Session(SessionRequest::Detach) => false,
        RequestSchema::Session(_) => decode_response_payload(payload).is_ok(),
        RequestSchema::Watch(WatchRequest::Reconcile {
            target,
            max_bytes,
            max_lines,
        }) => {
            if payload.len() > max_bytes as usize {
                return false;
            }
            decode_reconciliation(payload).is_ok_and(|reconciliation| {
                reconciliation.info.target == target
                    && display_line_count(reconciliation.text) <= usize::from(max_lines)
            })
        }
        RequestSchema::Watch(WatchRequest::Watch { .. }) => false,
    }
}

fn display_line_count(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        1 + text.bytes().filter(|byte| *byte == b'\n').count()
    }
}

#[derive(Clone, Copy)]
struct WatchReplyContext {
    request_id: u64,
    target: crate::control_protocol::TargetRef,
    requested_cursor: crate::event_journal::EventCursor,
    limit: u16,
}

fn write_watch_reply(
    stream: &mut AuthenticatedStream,
    context: WatchReplyContext,
    events: &[Event],
    end: &WatchEnd,
    stop: &AtomicBool,
) -> io::Result<()> {
    let deadline = Instant::now()
        .checked_add(WRITE_DEADLINE)
        .ok_or_else(stopped_error)?;
    let encoded_end = validate_watch_reply(
        context.target,
        context.requested_cursor,
        context.limit,
        events,
        end,
    )?;
    let _response = Frame::new(MessageKind::Response, context.request_id, &encoded_end)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    for event in events {
        let payload =
            encode_event(event).map_err(|_| invalid_data("owner watch event is invalid"))?;
        write_frame_until(stream, MessageKind::Event, 0, &payload, deadline, stop)?;
    }
    write_frame_until(
        stream,
        MessageKind::Response,
        context.request_id,
        &encoded_end,
        deadline,
        stop,
    )
}

fn validate_watch_reply(
    target: crate::control_protocol::TargetRef,
    requested_cursor: crate::event_journal::EventCursor,
    limit: u16,
    events: &[Event],
    end: &WatchEnd,
) -> io::Result<[u8; crate::watch_protocol::WATCH_END_BYTES]> {
    let event_count =
        u16::try_from(events.len()).map_err(|_| invalid_data("owner watch batch is malformed"))?;
    if events.len() > usize::from(MAX_WATCH_EVENTS)
        || event_count > limit
        || end.event_count != event_count
        || end.cursor.generation != target.generation
        || requested_cursor.generation != target.generation
    {
        return Err(invalid_data("owner watch batch is malformed"));
    }

    match end.status {
        WatchStatus::Changed => {
            let mut expected = requested_cursor
                .sequence
                .checked_add(1)
                .ok_or_else(|| invalid_data("owner watch batch is malformed"))?;
            if events.is_empty() {
                return Err(invalid_data("owner watch batch is malformed"));
            }
            for (index, event) in events.iter().enumerate() {
                if event.validate().is_err()
                    || event.target != target
                    || event.target.generation != end.cursor.generation
                    || event.cursor != expected
                {
                    return Err(invalid_data("owner watch batch is malformed"));
                }
                if index + 1 < events.len() {
                    expected = expected
                        .checked_add(1)
                        .ok_or_else(|| invalid_data("owner watch batch is malformed"))?;
                }
            }
            if events.last().map(|event| event.cursor) != Some(end.cursor.sequence) {
                return Err(invalid_data("owner watch batch is malformed"));
            }
        }
        WatchStatus::TimedOut => {
            if !events.is_empty() || end.cursor.sequence != requested_cursor.sequence {
                return Err(invalid_data("owner watch batch is malformed"));
            }
        }
        WatchStatus::ResyncRequired => {
            if !events.is_empty() || end.cursor.sequence < requested_cursor.sequence {
                return Err(invalid_data("owner watch batch is malformed"));
            }
        }
    }

    encode_watch_end(end).map_err(|_| invalid_data("owner watch end is invalid"))
}

fn submit_owner(
    owner_sender: &SyncSender<OwnerEnvelope>,
    stop: &AtomicBool,
    request: OwnerRequest,
) -> Option<OwnerReply> {
    if stop.load(Ordering::Acquire) {
        return None;
    }
    let (reply_sender, reply_receiver) = mpsc::sync_channel(1);
    let envelope = OwnerEnvelope {
        request,
        reply: reply_sender,
    };
    match owner_sender.try_send(envelope) {
        Ok(()) => receive_owner_reply(&reply_receiver, stop),
        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => None,
    }
}

fn receive_owner_reply(receiver: &Receiver<OwnerReply>, stop: &AtomicBool) -> Option<OwnerReply> {
    let deadline = Instant::now().checked_add(OWNER_REPLY_DEADLINE)?;
    loop {
        if stop.load(Ordering::Acquire) {
            return None;
        }
        let remaining = deadline.checked_duration_since(Instant::now())?;
        let wait = remaining.min(STOP_POLL_INTERVAL);
        match receiver.recv_timeout(wait) {
            Ok(reply) => return Some(reply),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

fn read_frame(stream: &mut AuthenticatedStream, stop: &AtomicBool) -> Option<OwnedFrame> {
    if stop.load(Ordering::Acquire) {
        return None;
    }
    let deadline = Instant::now().checked_add(READ_DEADLINE)?;
    stream.read_frame(deadline).ok()
}

fn write_frame(
    stream: &mut AuthenticatedStream,
    kind: MessageKind,
    request_id: u64,
    payload: &[u8],
    stop: &AtomicBool,
) -> io::Result<()> {
    let deadline = Instant::now()
        .checked_add(WRITE_DEADLINE)
        .ok_or_else(stopped_error)?;
    write_frame_until(stream, kind, request_id, payload, deadline, stop)
}

fn write_frame_until(
    stream: &mut AuthenticatedStream,
    kind: MessageKind,
    request_id: u64,
    payload: &[u8],
    deadline: Instant,
    stop: &AtomicBool,
) -> io::Result<()> {
    if stop.load(Ordering::Acquire) {
        return Err(stopped_error());
    }
    let frame = Frame::new(kind, request_id, payload)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    stream.write_frame(&frame, deadline)
}

fn write_error(
    stream: &mut AuthenticatedStream,
    request_id: u64,
    label: &'static str,
    stop: &AtomicBool,
) -> io::Result<()> {
    if !valid_error_label(label) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "owner returned an invalid error label",
        ));
    }
    write_frame(
        stream,
        MessageKind::Error,
        request_id,
        label.as_bytes(),
        stop,
    )
}

fn valid_error_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= MAX_ERROR_LABEL_BYTES
        && label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn stopped_error() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "connection worker is stopping")
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod watch_delivery_tests {
    use super::*;
    use crate::control_protocol::TargetRef;
    use crate::event_journal::EventCursor;
    use crate::event_protocol::{decode_event, EventBody};
    use crate::watch_protocol::{decode_watch_end, WatchStatus};
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;

    fn target() -> TargetRef {
        TargetRef {
            generation: 3,
            workspace: 4,
            pane: 5,
            session: 6,
            occupant_revision: 7,
        }
    }

    fn event(sequence: u64) -> Event {
        Event {
            cursor: sequence,
            target: target(),
            body: EventBody::ChildExited,
        }
    }

    fn cursor(sequence: u64) -> EventCursor {
        EventCursor {
            generation: target().generation,
            sequence,
        }
    }

    #[test]
    fn socketpair_watch_writes_ordered_events_then_terminal_response() {
        let (server_socket, client_socket) = UnixStream::pair().expect("socket pair");
        let mut server = AuthenticatedStream::new(server_socket).expect("server stream");
        let mut client = AuthenticatedStream::new(client_socket).expect("client stream");
        server.enable_metadata_events().unwrap();
        client.enable_metadata_events().unwrap();
        let events = [event(4), event(5)];
        let end = WatchEnd {
            status: WatchStatus::Changed,
            cursor: cursor(5),
            event_count: 2,
        };
        let stop = AtomicBool::new(false);

        write_watch_reply(
            &mut server,
            WatchReplyContext {
                request_id: 91,
                target: target(),
                requested_cursor: cursor(3),
                limit: 2,
            },
            &events,
            &end,
            &stop,
        )
        .expect("write complete watch result");

        for expected in &events {
            let frame = client
                .read_frame(Instant::now() + Duration::from_secs(1))
                .expect("read event frame");
            assert_eq!(frame.kind(), MessageKind::Event);
            assert_eq!(frame.request_id(), 0);
            assert_eq!(decode_event(frame.payload()).unwrap(), *expected);
        }
        let terminal = client
            .read_frame(Instant::now() + Duration::from_secs(1))
            .expect("read terminal response");
        assert_eq!(terminal.kind(), MessageKind::Response);
        assert_eq!(terminal.request_id(), 91);
        assert_eq!(decode_watch_end(terminal.payload()).unwrap(), end);
    }

    #[test]
    fn malformed_owner_batch_is_rejected_before_any_event_is_written() {
        let (server_socket, mut client_socket) = UnixStream::pair().expect("socket pair");
        let mut server = AuthenticatedStream::new(server_socket).expect("server stream");
        server.enable_metadata_events().unwrap();
        client_socket.set_nonblocking(true).unwrap();
        let events = [event(4), event(6)];
        let end = WatchEnd {
            status: WatchStatus::Changed,
            cursor: cursor(6),
            event_count: 2,
        };
        let stop = AtomicBool::new(false);

        let error = write_watch_reply(
            &mut server,
            WatchReplyContext {
                request_id: 92,
                target: target(),
                requested_cursor: cursor(3),
                limit: 2,
            },
            &events,
            &end,
            &stop,
        )
        .expect_err("cursor gap must invalidate the whole owner batch");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!server.is_poisoned());
        assert_eq!(
            client_socket.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn enabled_client_event_is_not_accepted_as_a_request_role() {
        let (server_socket, client_socket) = UnixStream::pair().expect("socket pair");
        let mut server = AuthenticatedStream::new(server_socket).expect("server stream");
        let mut client = AuthenticatedStream::new(client_socket).expect("client stream");
        server.enable_metadata_events().unwrap();
        client.enable_metadata_events().unwrap();
        let payload = encode_event(&event(1)).unwrap();
        let frame = Frame::new(MessageKind::Event, 0, &payload).unwrap();
        client
            .write_frame(&frame, Instant::now() + Duration::from_secs(1))
            .unwrap();

        assert!(read_request(&mut server, &AtomicBool::new(false), 0).is_none());
    }

    #[test]
    fn slow_watch_reader_shares_one_absolute_write_deadline_across_the_batch() {
        let (server_socket, client_socket) = UnixStream::pair().expect("socket pair");
        let send_buffer: libc::c_int = 1_024;
        // SAFETY: the descriptor is a live Unix stream, and the size and
        // pointer match setsockopt(SO_SNDBUF)'s integer input contract.
        let result = unsafe {
            libc::setsockopt(
                server_socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&send_buffer as *const libc::c_int).cast(),
                std::mem::size_of_val(&send_buffer) as libc::socklen_t,
            )
        };
        assert_eq!(result, 0, "shrink watch socket send buffer");

        let mut server = AuthenticatedStream::new(server_socket).expect("server stream");
        let mut client = AuthenticatedStream::new(client_socket).expect("client stream");
        server.enable_metadata_events().unwrap();
        client.enable_metadata_events().unwrap();
        let events = (1..=64).map(event).collect::<Vec<_>>();
        let end = WatchEnd {
            status: WatchStatus::Changed,
            cursor: cursor(64),
            event_count: 64,
        };
        let stop = AtomicBool::new(false);

        let writer = thread::spawn(move || {
            let started = Instant::now();
            let result = write_watch_reply(
                &mut server,
                WatchReplyContext {
                    request_id: 93,
                    target: target(),
                    requested_cursor: cursor(0),
                    limit: 64,
                },
                &events,
                &end,
                &stop,
            );
            (result, started.elapsed())
        });
        let reader = thread::spawn(move || {
            let mut event_count = 0;
            let mut saw_terminal = false;
            while let Ok(frame) = client.read_frame(Instant::now() + Duration::from_secs(2)) {
                if frame.kind() == MessageKind::Response {
                    saw_terminal = true;
                    break;
                }
                assert_eq!(frame.kind(), MessageKind::Event);
                event_count += 1;
                thread::sleep(Duration::from_millis(40));
            }
            (event_count, saw_terminal)
        });

        let (write_result, elapsed) = writer.join().unwrap();
        let (received_events, saw_terminal) = reader.join().unwrap();
        assert_eq!(write_result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(elapsed < Duration::from_millis(600));
        assert!(received_events > 0);
        assert!(received_events < 64);
        assert!(!saw_terminal);
    }
}
