//! Private, same-user UI/CLI endpoint for explicit provider-recovery intent.
//!
//! Requests carry a full managed target and project scope. They are not
//! provider proof, authorization to act, or evidence of selected route state.

#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use crate::control_protocol::TargetRef;
use crate::peer_identity::require_same_user;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const MAGIC: &[u8; 8] = b"PLRECV01";
const VERSION: u16 = 1;
const HEADER_BYTES: usize = 16;
const MAX_PACKET_BYTES: usize = 4_608;
const MAX_PROJECT_BYTES: usize = 4_096;
const MAX_SESSION_ID_BYTES: usize = 128;
const MAX_SOCKET_PATH_BYTES: usize = 103;
const OP_PREPARE: u16 = 1;
const OP_RESUME: u16 = 2;
const OP_STATUS: u16 = 3;
const RESPONSE_BIT: u16 = 0x8000;
const SERVER_OPERATION_LIMIT: Duration = Duration::from_secs(10);
const INITIAL_PACKET_LIMIT: Duration = Duration::from_millis(300);
const CLIENT_POLL: Duration = Duration::from_millis(10);
const ACCEPT_GATE_WAIT: Duration = Duration::from_millis(10);

/// Exact user intent submitted to the runtime owner.
pub enum RecoveryCommand {
    /// Validate and durably prepare this caller-provided exact session ID.
    Prepare { session_id: String },
    /// Spend and issue the previously prepared one-shot navigation request.
    Resume { ticket: [u8; 16], revision: u64 },
    /// Read the owner's current fixed recovery status.
    Status,
}

impl fmt::Debug for RecoveryCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Prepare { .. } => "Prepare { <redacted> }",
            Self::Resume { .. } => "Resume { <redacted> }",
            Self::Status => "Status",
        })
    }
}

/// Fixed manual-recovery reason safe for presentation and diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ManualReason {
    NoLastConfirmed = 1,
    InvalidChoice = 2,
    MissingSession = 3,
    DeletedSession = 4,
    ForeignProject = 5,
    ChildSession = 6,
    UnsupportedProvider = 7,
    UncertainProvider = 8,
    ProjectMismatch = 9,
    InterruptedOperation = 10,
}

/// Fixed endpoint response. It never carries provider IDs or raw errors.
#[derive(Clone, Eq, PartialEq)]
pub enum RecoveryReply {
    Prepared { ticket: [u8; 16], revision: u64 },
    Confirmed { revision: u64 },
    Pending { revision: u64 },
    ManualRequired { reason: ManualReason },
    Unavailable,
    Busy,
    Uncertain,
}

impl fmt::Debug for RecoveryReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Prepared { .. } => "Prepared { <redacted> }",
            Self::Confirmed { .. } => "Confirmed { <fixed> }",
            Self::Pending { .. } => "Pending { <fixed> }",
            Self::ManualRequired { .. } => "ManualRequired { <fixed> }",
            Self::Unavailable => "Unavailable",
            Self::Busy => "Busy",
            Self::Uncertain => "Uncertain",
        })
    }
}

/// One decoded request waiting for an owner decision.
pub struct RecoveryRequest {
    target: TargetRef,
    project: Vec<u8>,
    command: RecoveryCommand,
    cancelled: Arc<AtomicBool>,
    responded: AtomicBool,
    reply_tx: SyncSender<RecoveryReply>,
}

impl RecoveryRequest {
    pub const fn target(&self) -> TargetRef {
        self.target
    }

    pub fn project(&self) -> &[u8] {
        &self.project
    }

    pub fn command(&self) -> &RecoveryCommand {
        &self.command
    }

    /// Cancellation can race with owner work; check before SDK/navigation and
    /// again after marking a durable one-shot operation spent.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Share cancellation with an owner worker performing cancellable recovery
    /// work. The worker must still check it before navigation and after spend.
    pub fn cancellation_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }

    /// Send one fixed reply to the waiting client without blocking the owner.
    pub fn respond(&self, reply: RecoveryReply) -> Result<(), RespondError> {
        if self.is_cancelled() {
            return Err(RespondError::Cancelled);
        }
        if !reply_valid_for_command(&self.command, &reply) || encode_reply(&reply).is_err() {
            return Err(RespondError::InvalidReply);
        }
        if self
            .responded
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(RespondError::AlreadyResponded);
        }
        self.reply_tx.try_send(reply).map_err(|error| match error {
            TrySendError::Full(_) => RespondError::Busy,
            TrySendError::Disconnected(_) => RespondError::Cancelled,
        })
    }
}

impl fmt::Debug for RecoveryRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryRequest { <redacted> }")
    }
}

/// Fixed response submission failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RespondError {
    Cancelled,
    InvalidReply,
    AlreadyResponded,
    Busy,
}

/// Bound owner endpoint. Dropping it stops the accept worker and removes only
/// the pathname whose socket inode this instance created.
pub struct RecoveryEndpoint {
    requests: Receiver<RecoveryRequest>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    socket_path: PathBuf,
    socket_device: u64,
    socket_inode: u64,
}

impl fmt::Debug for RecoveryEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryEndpoint { <redacted> }")
    }
}

impl RecoveryEndpoint {
    /// Bind one endpoint below an existing owner-only canonical directory.
    pub fn bind(path: impl AsRef<Path>, target: TargetRef, project: &[u8]) -> io::Result<Self> {
        validate_scope(target, project)?;
        let socket_path = canonical_socket_path(path.as_ref())?;
        if socket_path.as_os_str().as_encoded_bytes().len() > MAX_SOCKET_PATH_BYTES {
            return Err(invalid_input("recovery endpoint path is too long"));
        }
        if fs::symlink_metadata(&socket_path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "recovery endpoint already exists",
            ));
        }
        // Coordinate listener FD creation with Panelink-managed exec so an
        // incompletely initialized descriptor cannot leak into a child.
        let listener = {
            let _guard = crate::spawn_gate::acquire()?;
            UnixListener::bind(&socket_path)?
        };
        let initial = fs::symlink_metadata(&socket_path)?;
        if !initial.file_type().is_socket() || initial.uid() != rustix::process::geteuid().as_raw()
        {
            return Err(permission_denied("recovery endpoint socket was replaced"));
        }
        let initial_identity = Some((initial.dev(), initial.ino()));
        if let Err(error) = fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600)) {
            remove_socket_if_owned(&socket_path, initial_identity);
            return Err(error);
        }
        let metadata = match validate_socket(&socket_path) {
            Ok(metadata) => metadata,
            Err(error) => {
                remove_socket_if_owned(&socket_path, initial_identity);
                return Err(error);
            }
        };
        if let Err(error) = listener.set_nonblocking(true) {
            remove_socket_if_owned(&socket_path, initial_identity);
            return Err(error);
        }
        let socket_device = metadata.dev();
        let socket_inode = metadata.ino();
        if let Err(error) = listener.set_nonblocking(true) {
            remove_socket_if_owned(&socket_path, Some((socket_device, socket_inode)));
            return Err(error);
        }
        let (request_tx, requests) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let project_copy = project.to_vec();
        let worker = match thread::Builder::new()
            .name("panelink-recovery-endpoint".to_owned())
            .spawn(move || endpoint_worker(listener, request_tx, worker_stop, target, project_copy))
        {
            Ok(worker) => worker,
            Err(error) => {
                remove_socket_if_owned(&socket_path, Some((socket_device, socket_inode)));
                return Err(error);
            }
        };
        Ok(Self {
            requests,
            stop,
            worker: Some(worker),
            socket_path,
            socket_device,
            socket_inode,
        })
    }

    /// Poll without blocking; this is safe to call on the runtime owner loop.
    pub fn poll(&self) -> Option<RecoveryRequest> {
        self.requests.try_recv().ok()
    }
}

impl Drop for RecoveryEndpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Cancellation is bounded on the worker; never join it on the PTY owner.
        self.worker.take();
        remove_socket_if_owned(
            &self.socket_path,
            Some((self.socket_device, self.socket_inode)),
        );
    }
}

/// Make one same-user request. There is no reconnect or retry; a lost reply
/// after resume is an uncertain operation and must stay manual.
pub fn exchange(
    path: impl AsRef<Path>,
    target: TargetRef,
    project: &[u8],
    command: RecoveryCommand,
    deadline: Instant,
) -> io::Result<RecoveryReply> {
    validate_scope(target, project)?;
    validate_command(&command)?;
    let deadline = bounded_deadline(deadline, SERVER_OPERATION_LIMIT)?;
    let socket_path = canonical_socket_path(path.as_ref())?;
    validate_socket(&socket_path)?;
    if Instant::now() >= deadline {
        return Err(timeout());
    }
    let mut stream = crate::transport::connect_unix_with_deadline(&socket_path, deadline)?;
    if Instant::now() >= deadline {
        return Err(timeout());
    }
    require_same_user(&stream)?;
    validate_socket(&socket_path)?;
    stream.set_nonblocking(true)?;
    let (kind, payload) = encode_request(target, project, &command)?;
    write_packet(&mut stream, kind, &payload, deadline)?;
    let (response_kind, body) = read_packet(&mut stream, deadline)?;
    if response_kind != (kind | RESPONSE_BIT) {
        return Err(invalid_data("recovery reply kind is invalid"));
    }
    decode_reply(&command, &body)
}

fn endpoint_worker(
    listener: UnixListener,
    requests: SyncSender<RecoveryRequest>,
    stop: Arc<AtomicBool>,
    target: TargetRef,
    project: Vec<u8>,
) {
    while !stop.load(Ordering::Acquire) {
        // The listener is nonblocking, so the protected accept syscall cannot
        // hold the cooperative exec gate while waiting for a client. Keep gate
        // acquisition itself bounded so endpoint shutdown remains responsive.
        let deadline = Instant::now() + ACCEPT_GATE_WAIT;
        let accepted = match crate::spawn_gate::acquire_until(deadline) {
            Ok(_guard) => listener.accept(),
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                pause(&stop);
                continue;
            }
            Err(_) => return,
        };
        match accepted {
            Ok((stream, _)) => handle_client(stream, &requests, &stop, target, &project),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => pause(&stop),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => pause(&stop),
        }
    }
}

fn handle_client(
    mut stream: UnixStream,
    requests: &SyncSender<RecoveryRequest>,
    stop: &AtomicBool,
    expected_target: TargetRef,
    expected_project: &[u8],
) {
    if require_same_user(&stream).is_err() || stream.set_nonblocking(true).is_err() {
        return;
    }
    let deadline = Instant::now() + SERVER_OPERATION_LIMIT;
    let read_deadline = deadline.min(Instant::now() + INITIAL_PACKET_LIMIT);
    let parsed =
        read_packet_control(&mut stream, read_deadline, Some(stop)).and_then(|(kind, body)| {
            let request = decode_request(kind, &body)?;
            if request.0 != expected_target || request.1 != expected_project {
                return Err(permission_denied("recovery request scope is invalid"));
            }
            Ok((kind, request.2))
        });
    let (kind, command) = match parsed {
        Ok(parsed) if peer_open_without_extra(&stream).unwrap_or(false) => parsed,
        _ => return,
    };
    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let request = RecoveryRequest {
        target: expected_target,
        project: expected_project.to_vec(),
        command,
        cancelled: Arc::clone(&cancelled),
        responded: AtomicBool::new(false),
        reply_tx,
    };
    if stop.load(Ordering::Acquire) || !peer_open_without_extra(&stream).unwrap_or(false) {
        cancelled.store(true, Ordering::Release);
        return;
    }
    match requests.try_send(request) {
        Ok(()) => {}
        Err(TrySendError::Full(request)) => {
            cancelled.store(true, Ordering::Release);
            let _ = write_reply(&mut stream, kind, &RecoveryReply::Busy, deadline);
            drop(request);
            return;
        }
        Err(TrySendError::Disconnected(_)) => return,
    }
    let reply = loop {
        if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
            cancelled.store(true, Ordering::Release);
            return;
        }
        if !peer_open_without_extra(&stream).unwrap_or(false) {
            cancelled.store(true, Ordering::Release);
            return;
        }
        match reply_rx.recv_timeout(CLIENT_POLL) {
            Ok(reply) => break reply,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                cancelled.store(true, Ordering::Release);
                return;
            }
        }
    };
    if peer_open_without_extra(&stream).unwrap_or(false) {
        let _ = write_reply(&mut stream, kind, &reply, deadline);
    } else {
        cancelled.store(true, Ordering::Release);
    }
}

fn encode_request(
    target: TargetRef,
    project: &[u8],
    command: &RecoveryCommand,
) -> io::Result<(u16, Vec<u8>)> {
    let mut body = Vec::with_capacity(42 + project.len() + MAX_SESSION_ID_BYTES);
    append_target(&mut body, target);
    body.extend_from_slice(&(project.len() as u16).to_be_bytes());
    body.extend_from_slice(project);
    let kind = match command {
        RecoveryCommand::Prepare { session_id } => {
            let id = session_id.as_bytes();
            body.extend_from_slice(&(id.len() as u16).to_be_bytes());
            body.extend_from_slice(id);
            OP_PREPARE
        }
        RecoveryCommand::Resume { ticket, revision } => {
            body.extend_from_slice(ticket);
            body.extend_from_slice(&revision.to_be_bytes());
            OP_RESUME
        }
        RecoveryCommand::Status => OP_STATUS,
    };
    if body.len() + HEADER_BYTES > MAX_PACKET_BYTES {
        return Err(invalid_input("recovery request is too large"));
    }
    Ok((kind, body))
}

fn decode_request(kind: u16, body: &[u8]) -> io::Result<(TargetRef, Vec<u8>, RecoveryCommand)> {
    if body.len() < 42 {
        return Err(invalid_data("recovery request is truncated"));
    }
    let target = decode_target(&body[..40])?;
    let project_len = u16::from_be_bytes([body[40], body[41]]) as usize;
    let command_at = 42usize
        .checked_add(project_len)
        .ok_or_else(|| invalid_data("recovery request length is invalid"))?;
    if project_len == 0 || project_len > MAX_PROJECT_BYTES || body.len() < command_at {
        return Err(invalid_data("recovery request project is invalid"));
    }
    let project = body[42..command_at].to_vec();
    if project.contains(&0) {
        return Err(invalid_data("recovery request project is invalid"));
    }
    let command_body = &body[command_at..];
    let command = match kind {
        OP_PREPARE if command_body.len() >= 3 => {
            let id_len = u16::from_be_bytes([command_body[0], command_body[1]]) as usize;
            if id_len == 0 || id_len > MAX_SESSION_ID_BYTES || command_body.len() != id_len + 2 {
                return Err(invalid_data("recovery session ID is invalid"));
            }
            let id = std::str::from_utf8(&command_body[2..])
                .map_err(|_| invalid_data("recovery session ID is invalid"))?
                .to_owned();
            validate_session_id(&id)?;
            RecoveryCommand::Prepare { session_id: id }
        }
        OP_RESUME if command_body.len() == 24 => {
            let ticket: [u8; 16] = command_body[..16]
                .try_into()
                .map_err(|_| invalid_data("recovery ticket is invalid"))?;
            let revision = u64::from_be_bytes(
                command_body[16..24]
                    .try_into()
                    .map_err(|_| invalid_data("recovery revision is invalid"))?,
            );
            if ticket.iter().all(|byte| *byte == 0) || revision == 0 {
                return Err(invalid_data("recovery resume command is invalid"));
            }
            RecoveryCommand::Resume { ticket, revision }
        }
        OP_STATUS if command_body.is_empty() => RecoveryCommand::Status,
        _ => return Err(invalid_data("recovery command is invalid")),
    };
    validate_scope(target, &project)?;
    Ok((target, project, command))
}

fn encode_reply(reply: &RecoveryReply) -> io::Result<[u8; 26]> {
    let mut bytes = [0; 26];
    match reply {
        RecoveryReply::Prepared { ticket, revision }
            if *revision != 0 && ticket.iter().any(|b| *b != 0) =>
        {
            bytes[0] = 1;
            bytes[2..10].copy_from_slice(&revision.to_be_bytes());
            bytes[10..26].copy_from_slice(ticket);
        }
        RecoveryReply::Confirmed { revision } if *revision != 0 => {
            bytes[0] = 2;
            bytes[2..10].copy_from_slice(&revision.to_be_bytes());
        }
        RecoveryReply::Pending { revision } if *revision != 0 => {
            bytes[0] = 3;
            bytes[2..10].copy_from_slice(&revision.to_be_bytes());
        }
        RecoveryReply::ManualRequired { reason } => {
            bytes[0] = 4;
            bytes[1] = *reason as u8;
        }
        RecoveryReply::Unavailable => bytes[0] = 5,
        RecoveryReply::Busy => bytes[0] = 6,
        RecoveryReply::Uncertain => bytes[0] = 7,
        _ => return Err(invalid_input("recovery reply is invalid")),
    }
    Ok(bytes)
}

fn decode_reply(command: &RecoveryCommand, body: &[u8]) -> io::Result<RecoveryReply> {
    if body.len() != 26 || body[10..26].iter().all(|byte| *byte == 0) != (body[0] != 1) {
        return Err(invalid_data("recovery reply is invalid"));
    }
    let revision = u64::from_be_bytes(
        body[2..10]
            .try_into()
            .map_err(|_| invalid_data("recovery reply is invalid"))?,
    );
    let ticket: [u8; 16] = body[10..26]
        .try_into()
        .map_err(|_| invalid_data("recovery reply is invalid"))?;
    let reply = match body[0] {
        1 if body[1] == 0 && revision != 0 => RecoveryReply::Prepared { ticket, revision },
        2 if body[1] == 0 && revision != 0 => RecoveryReply::Confirmed { revision },
        3 if body[1] == 0 && revision != 0 => RecoveryReply::Pending { revision },
        4 if body[1] != 0 && revision == 0 => RecoveryReply::ManualRequired {
            reason: decode_reason(body[1])?,
        },
        5 if body[1] == 0 && revision == 0 => RecoveryReply::Unavailable,
        6 if body[1] == 0 && revision == 0 => RecoveryReply::Busy,
        7 if body[1] == 0 && revision == 0 => RecoveryReply::Uncertain,
        _ => return Err(invalid_data("recovery reply is invalid")),
    };
    if !reply_valid_for_command(command, &reply) {
        return Err(invalid_data("recovery reply does not match request"));
    }
    Ok(reply)
}

fn decode_reason(reason: u8) -> io::Result<ManualReason> {
    match reason {
        1 => Ok(ManualReason::NoLastConfirmed),
        2 => Ok(ManualReason::InvalidChoice),
        3 => Ok(ManualReason::MissingSession),
        4 => Ok(ManualReason::DeletedSession),
        5 => Ok(ManualReason::ForeignProject),
        6 => Ok(ManualReason::ChildSession),
        7 => Ok(ManualReason::UnsupportedProvider),
        8 => Ok(ManualReason::UncertainProvider),
        9 => Ok(ManualReason::ProjectMismatch),
        10 => Ok(ManualReason::InterruptedOperation),
        _ => Err(invalid_data("recovery reply reason is invalid")),
    }
}

fn reply_valid_for_command(command: &RecoveryCommand, reply: &RecoveryReply) -> bool {
    match command {
        RecoveryCommand::Prepare { .. } => matches!(
            reply,
            RecoveryReply::Prepared { .. }
                | RecoveryReply::Pending { .. }
                | RecoveryReply::ManualRequired { .. }
                | RecoveryReply::Unavailable
                | RecoveryReply::Busy
                | RecoveryReply::Uncertain
        ),
        RecoveryCommand::Resume { .. } => matches!(
            reply,
            RecoveryReply::Confirmed { .. }
                | RecoveryReply::Pending { .. }
                | RecoveryReply::ManualRequired { .. }
                | RecoveryReply::Unavailable
                | RecoveryReply::Busy
                | RecoveryReply::Uncertain
        ),
        RecoveryCommand::Status => true,
    }
}

fn validate_command(command: &RecoveryCommand) -> io::Result<()> {
    match command {
        RecoveryCommand::Prepare { session_id } => validate_session_id(session_id),
        RecoveryCommand::Resume { ticket, revision }
            if *revision > 0 && ticket.iter().any(|byte| *byte != 0) =>
        {
            Ok(())
        }
        RecoveryCommand::Resume { .. } => Err(invalid_input("recovery resume command is invalid")),
        RecoveryCommand::Status => Ok(()),
    }
}

fn validate_session_id(session_id: &str) -> io::Result<()> {
    let bytes = session_id.as_bytes();
    if bytes.is_empty()
        || bytes.len() > MAX_SESSION_ID_BYTES
        || !bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    {
        Err(invalid_input("recovery session ID is invalid"))
    } else {
        Ok(())
    }
}

fn validate_scope(target: TargetRef, project: &[u8]) -> io::Result<()> {
    if target.generation == 0
        || target.workspace == 0
        || target.pane == 0
        || target.session == 0
        || target.occupant_revision == 0
        || project.is_empty()
        || project.len() > MAX_PROJECT_BYTES
        || project.contains(&0)
    {
        Err(invalid_input("recovery endpoint scope is invalid"))
    } else {
        Ok(())
    }
}

fn append_target(out: &mut Vec<u8>, target: TargetRef) {
    for value in [
        target.generation,
        target.workspace,
        target.pane,
        target.session,
        target.occupant_revision,
    ] {
        out.extend_from_slice(&value.to_be_bytes());
    }
}

fn decode_target(bytes: &[u8]) -> io::Result<TargetRef> {
    if bytes.len() != 40 {
        return Err(invalid_data("recovery target is invalid"));
    }
    let read = |at: usize| -> io::Result<u64> {
        Ok(u64::from_be_bytes(
            bytes[at..at + 8]
                .try_into()
                .map_err(|_| invalid_data("recovery target is invalid"))?,
        ))
    };
    Ok(TargetRef {
        generation: read(0)?,
        workspace: read(8)?,
        pane: read(16)?,
        session: read(24)?,
        occupant_revision: read(32)?,
    })
}

fn canonical_socket_path(path: &Path) -> io::Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| invalid_input("recovery endpoint name is invalid"))?;
    let canonical_parent = fs::canonicalize(parent)?;
    let metadata = fs::symlink_metadata(&canonical_parent)?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(permission_denied("recovery endpoint parent is not private"));
    }
    Ok(canonical_parent.join(name))
}

fn validate_socket(path: &Path) -> io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(permission_denied("recovery endpoint socket is not private"));
    }
    Ok(metadata)
}

fn remove_socket_if_owned(path: &Path, identity: Option<(u64, u64)>) {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_socket()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && identity.is_none_or(|(dev, ino)| metadata.dev() == dev && metadata.ino() == ino)
        {
            let _ = fs::remove_file(path);
        }
    }
}

fn write_reply(
    stream: &mut UnixStream,
    request_kind: u16,
    reply: &RecoveryReply,
    deadline: Instant,
) -> io::Result<()> {
    let body = encode_reply(reply)?;
    write_packet(stream, request_kind | RESPONSE_BIT, &body, deadline)
}

fn write_packet(
    stream: &mut UnixStream,
    kind: u16,
    body: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    if body.len() + HEADER_BYTES > MAX_PACKET_BYTES || kind == 0 {
        return Err(invalid_input("recovery packet is invalid"));
    }
    let mut header = [0; HEADER_BYTES];
    header[..8].copy_from_slice(MAGIC);
    header[8..10].copy_from_slice(&VERSION.to_be_bytes());
    header[10..12].copy_from_slice(&kind.to_be_bytes());
    header[12..16].copy_from_slice(&(body.len() as u32).to_be_bytes());
    write_all_until(stream, &header, deadline)?;
    write_all_until(stream, body, deadline)
}

fn read_packet(stream: &mut UnixStream, deadline: Instant) -> io::Result<(u16, Vec<u8>)> {
    read_packet_control(stream, deadline, None)
}

fn read_packet_control(
    stream: &mut UnixStream,
    deadline: Instant,
    stop: Option<&AtomicBool>,
) -> io::Result<(u16, Vec<u8>)> {
    let mut header = [0; HEADER_BYTES];
    read_exact_until_control(stream, &mut header, deadline, stop)?;
    if &header[..8] != MAGIC || u16::from_be_bytes([header[8], header[9]]) != VERSION {
        return Err(invalid_data("recovery packet header is invalid"));
    }
    let kind = u16::from_be_bytes([header[10], header[11]]);
    let len = u32::from_be_bytes(
        header[12..16]
            .try_into()
            .map_err(|_| invalid_data("recovery packet header is invalid"))?,
    ) as usize;
    if !(kind == OP_PREPARE
        || kind == OP_RESUME
        || kind == OP_STATUS
        || kind == (RESPONSE_BIT | OP_PREPARE)
        || kind == (RESPONSE_BIT | OP_RESUME)
        || kind == (RESPONSE_BIT | OP_STATUS))
        || len + HEADER_BYTES > MAX_PACKET_BYTES
    {
        return Err(invalid_data("recovery packet header is invalid"));
    }
    let mut body = vec![0; len];
    read_exact_until_control(stream, &mut body, deadline, stop)?;
    Ok((kind, body))
}

fn peer_open_without_extra(stream: &UnixStream) -> io::Result<bool> {
    loop {
        let mut byte = [0u8; 1];
        // SAFETY: stream keeps the borrowed descriptor live; byte is writable
        // and MSG_PEEK leaves extra protocol data queued for rejection.
        let result = unsafe {
            libc::recv(
                stream.as_raw_fd(),
                byte.as_mut_ptr().cast(),
                1,
                libc::MSG_PEEK | libc::MSG_DONTWAIT,
            )
        };
        if result > 0 {
            return Ok(false);
        }
        if result == 0 {
            return Ok(false);
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            return Ok(true);
        }
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(error);
    }
}

fn read_exact_until_control(
    stream: &mut UnixStream,
    output: &mut [u8],
    deadline: Instant,
    stop: Option<&AtomicBool>,
) -> io::Result<()> {
    let mut offset = 0;
    while offset < output.len() {
        if stop.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "recovery endpoint stopped",
            ));
        }
        if Instant::now() >= deadline {
            return Err(timeout());
        }
        match stream.read(&mut output[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "recovery client closed",
                ))
            }
            Ok(count) => offset += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(CLIENT_POLL.min(deadline.saturating_duration_since(Instant::now())))
            }
            Err(error) => return Err(error),
        }
    }
    if Instant::now() >= deadline {
        Err(timeout())
    } else {
        Ok(())
    }
}

fn write_all_until(stream: &mut UnixStream, bytes: &[u8], deadline: Instant) -> io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        if Instant::now() >= deadline {
            return Err(timeout());
        }
        match stream.write(&bytes[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "recovery write was empty",
                ))
            }
            Ok(count) => offset += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(CLIENT_POLL.min(deadline.saturating_duration_since(Instant::now())))
            }
            Err(error) => return Err(error),
        }
    }
    if Instant::now() >= deadline {
        Err(timeout())
    } else {
        Ok(())
    }
}

fn bounded_deadline(deadline: Instant, cap: Duration) -> io::Result<Instant> {
    if Instant::now() >= deadline {
        Err(timeout())
    } else {
        Ok(deadline.min(Instant::now() + cap))
    }
}

fn pause(stop: &AtomicBool) {
    if !stop.load(Ordering::Acquire) {
        thread::sleep(CLIENT_POLL);
    }
}

fn timeout() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "recovery endpoint deadline expired",
    )
}
fn invalid_input(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn permission_denied(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = PathBuf::from(format!(
                "/private/tmp/pl-recv-{}-{stamp}",
                std::process::id()
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
        fn socket(&self) -> PathBuf {
            self.0.join("recovery.sock")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn target() -> TargetRef {
        TargetRef {
            generation: 1,
            workspace: 2,
            pane: 3,
            session: 4,
            occupant_revision: 5,
        }
    }
    fn wait_request(endpoint: &RecoveryEndpoint, deadline: Instant) -> RecoveryRequest {
        loop {
            if let Some(request) = endpoint.poll() {
                return request;
            }
            assert!(Instant::now() < deadline, "request did not arrive");
            thread::sleep(CLIENT_POLL);
        }
    }

    #[test]
    fn same_user_private_endpoint_roundtrips_one_fixed_reply() {
        let fixture = Fixture::new();
        let endpoint = RecoveryEndpoint::bind(fixture.socket(), target(), b"/project").unwrap();
        let socket_meta = fs::symlink_metadata(fixture.socket()).unwrap();
        assert_eq!(socket_meta.mode() & 0o777, 0o600);
        assert_eq!(socket_meta.uid(), rustix::process::geteuid().as_raw());
        assert!(endpoint.poll().is_none());
        let path = fixture.socket();
        let client = thread::spawn(move || {
            exchange(
                path,
                target(),
                b"/project",
                RecoveryCommand::Prepare {
                    session_id: "ses_exact".to_owned(),
                },
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap()
        });
        let request = wait_request(&endpoint, Instant::now() + Duration::from_secs(2));
        assert_eq!(request.target(), target());
        assert_eq!(request.project(), b"/project");
        assert!(
            matches!(request.command(), RecoveryCommand::Prepare { session_id } if session_id == "ses_exact")
        );
        request
            .respond(RecoveryReply::Prepared {
                ticket: [9; 16],
                revision: 12,
            })
            .unwrap();
        assert!(matches!(
            client.join().unwrap(),
            RecoveryReply::Prepared { revision: 12, .. }
        ));
        assert_eq!(
            request.respond(RecoveryReply::Unavailable),
            Err(RespondError::AlreadyResponded)
        );
        drop(endpoint);
        assert!(!fixture.socket().exists());
    }

    #[test]
    fn wrong_scope_and_oversize_packet_never_reach_owner() {
        let fixture = Fixture::new();
        let endpoint = RecoveryEndpoint::bind(fixture.socket(), target(), b"/project").unwrap();
        let wrong = TargetRef {
            pane: 99,
            ..target()
        };
        assert!(exchange(
            fixture.socket(),
            wrong,
            b"/project",
            RecoveryCommand::Status,
            Instant::now() + Duration::from_secs(1)
        )
        .is_err());
        assert!(endpoint.poll().is_none());
        let mut stream = UnixStream::connect(fixture.socket()).unwrap();
        let mut header = [0; HEADER_BYTES];
        header[..8].copy_from_slice(MAGIC);
        header[8..10].copy_from_slice(&VERSION.to_be_bytes());
        header[10..12].copy_from_slice(&OP_STATUS.to_be_bytes());
        header[12..16].copy_from_slice(&(MAX_PACKET_BYTES as u32).to_be_bytes());
        stream.write_all(&header).unwrap();
        drop(stream);
        thread::sleep(Duration::from_millis(40));
        assert!(endpoint.poll().is_none());
    }

    #[test]
    fn client_disconnect_marks_polled_request_cancelled() {
        let fixture = Fixture::new();
        let endpoint = RecoveryEndpoint::bind(fixture.socket(), target(), b"/project").unwrap();
        let (kind, body) = encode_request(
            target(),
            b"/project",
            &RecoveryCommand::Resume {
                ticket: [3; 16],
                revision: 8,
            },
        )
        .unwrap();
        let mut stream = UnixStream::connect(fixture.socket()).unwrap();
        stream.set_nonblocking(true).unwrap();
        write_packet(
            &mut stream,
            kind,
            &body,
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        let request = wait_request(&endpoint, Instant::now() + Duration::from_secs(1));
        drop(stream);
        let until = Instant::now() + Duration::from_secs(1);
        while !request.is_cancelled() && Instant::now() < until {
            thread::sleep(CLIENT_POLL);
        }
        assert!(request.is_cancelled());
        assert_eq!(
            request.respond(RecoveryReply::Uncertain),
            Err(RespondError::Cancelled)
        );
    }

    #[test]
    fn whole_client_deadline_cancels_without_retry() {
        let fixture = Fixture::new();
        let endpoint = RecoveryEndpoint::bind(fixture.socket(), target(), b"/project").unwrap();
        let started = Instant::now();
        let result = exchange(
            fixture.socket(),
            target(),
            b"/project",
            RecoveryCommand::Status,
            started + Duration::from_millis(50),
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_millis(500));
        let request = wait_request(&endpoint, Instant::now() + Duration::from_secs(1));
        let until = Instant::now() + Duration::from_secs(1);
        while !request.is_cancelled() && Instant::now() < until {
            thread::sleep(CLIENT_POLL);
        }
        assert!(request.is_cancelled());
    }

    #[test]
    fn dropping_endpoint_during_owner_stall_is_nonblocking_and_cancels_client() {
        let fixture = Fixture::new();
        let endpoint = RecoveryEndpoint::bind(fixture.socket(), target(), b"/project").unwrap();
        let path = fixture.socket();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let client = thread::spawn(move || {
            let result = exchange(
                path,
                target(),
                b"/project",
                RecoveryCommand::Status,
                Instant::now() + Duration::from_secs(2),
            );
            let _ = reply_tx.send(result);
        });
        let request = wait_request(&endpoint, Instant::now() + Duration::from_secs(1));

        let started = Instant::now();
        drop(endpoint);
        assert!(started.elapsed() < Duration::from_millis(250));
        assert!(!fixture.socket().exists());
        let cancel_deadline = Instant::now() + Duration::from_secs(1);
        while !request.is_cancelled() && Instant::now() < cancel_deadline {
            thread::sleep(CLIENT_POLL);
        }
        assert!(request.is_cancelled());
        assert!(reply_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("client unblocked when endpoint stopped")
            .is_err());
        client.join().expect("reap client thread");
    }

    #[test]
    fn bind_requires_private_parent_and_drop_stops_worker() {
        let fixture = Fixture::new();
        let public = fixture.0.join("public");
        fs::create_dir(&public).unwrap();
        fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(RecoveryEndpoint::bind(public.join("socket"), target(), b"/project").is_err());
        let endpoint = RecoveryEndpoint::bind(fixture.socket(), target(), b"/project").unwrap();
        let started = Instant::now();
        drop(endpoint);
        assert!(started.elapsed() < Duration::from_millis(250));
    }
}
