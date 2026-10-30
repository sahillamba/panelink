//! Trusted bounded multi-session owner core.
//!
//! This library core does not authenticate peers or impose launch policy.
//! Callers must validate authority before launch, hold the server storage lock
//! for the owner's lifetime, and keep runtime routing disabled until the
//! parent separately adopts it. One caller-supplied ledger owns every lease,
//! launch, control result, status, and receipt in this generation.

mod async_split;

use self::async_split::ActiveAsyncSplit;
pub(crate) use self::async_split::AsyncSplitAdmission;

use super::{
    decode_request_payload, encode_response, is_capacity_error, owner_rejection, preflight_reply,
    terminal_outcome, Connection, EffectSink, EventPage, GlobalAuthority, OwnerMutation,
    OwnerObservation, RequestPreflight, ServerError, SessionHandler, SessionReply, TargetState,
};
use crate::close_protocol::{
    decode_close_request, CloseCodecError, CloseOutcome, CloseRejection, CloseRequest, CloseResult,
    StopOutcome,
};
use crate::control_protocol::{Command, TargetRef};
use crate::event_journal::{EventCursor, EventJournal};
use crate::event_protocol::Event;
use crate::handshake_protocol::{
    Welcome, CAPABILITY_EVENTS, CAPABILITY_HISTORY_CURSOR, CAPABILITY_INPUT,
    CAPABILITY_PANE_METADATA, CAPABILITY_READ, CAPABILITY_SCREEN_ROW_WRAPS,
    CAPABILITY_WORKSPACE_EVENTS,
};
use crate::input_owner::{InputOwner, Owner};
use crate::managed_launch::{LaunchJournalPort, ManagedLaunchCoordinator, ManagedLaunchError};
use crate::managed_launch_protocol::{decode_launch_request, LaunchRequestView, LaunchResult};
use crate::multi_runtime::PaneExit;
use crate::pane_layout::PaneGeometry;
use crate::pane_layout::{LayoutError, LayoutSnapshot, PaneLayout};
use crate::pane_metadata_protocol::{decode_query, PathStatus};
use crate::pane_operations_protocol::{
    self, PaneDividerSet, PaneOperationsView, PaneSwap, PaneZoomSet,
    Request as PaneOperationRequest,
};
use crate::process_metadata::ProcessIdentity;
use crate::protocol::{
    Admission, Ledger, OperationKey, OperationStatus, Outcome, ProtocolError, MAX_ACTIVE_LEASES,
};
use crate::response_protocol::{RejectionCode, Response, ResponseBody};
use crate::scoped_watch_protocol::SessionCursor;
use crate::server_pty::PtyEffects;
use crate::session_protocol::{decode_session_request, SessionHello, SessionRequest};
use crate::session_registry::{RegistryError, SessionState, SESSION_CAPACITY};
use crate::split_protocol::{decode_split_request, SplitCodecError, SplitResult};
use crate::workspace_event_journal::{
    JournalError as WorkspaceJournalError, WorkspaceEventJournal,
};
use crate::workspace_watch_protocol::{
    Reconciliation as WorkspaceReconciliation, WatchReply as WorkspaceWatchReply, WorkspaceCursor,
};
use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

/// One scoped authoritative screen and metadata snapshot.
pub struct ScopedReconcileState {
    /// Current target, ownership, and lifecycle metadata.
    pub info: crate::session_protocol::SessionInfo,
    /// Cursor captured with this target's snapshot.
    pub cursor: SessionCursor,
    /// Nonzero revision of the screen represented by `text`.
    pub screen_revision: u64,
    /// Whether the bounded screen was truncated.
    pub truncated: bool,
    /// Sanitized display text, not raw terminal bytes.
    pub text: String,
}

impl fmt::Debug for ScopedReconcileState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScopedReconcileState")
            .field("info", &"<redacted>")
            .field("cursor", &"<redacted>")
            .field("screen_revision", &"<redacted>")
            .field("text_len", &self.text.len())
            .field("truncated", &self.truncated)
            .finish()
    }
}

/// One bounded ordered event page scoped to a managed session.
pub struct ScopedEventPage {
    /// Metadata events retained by this target's journal.
    pub events: Vec<Event>,
    /// Target-scoped cursor after the returned page.
    pub cursor: SessionCursor,
    /// Whether the caller must reconcile after journal eviction.
    pub resync_required: bool,
}

impl fmt::Debug for ScopedEventPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScopedEventPage")
            .field("events_len", &self.events.len())
            .field("cursor", &"<redacted>")
            .field("resync_required", &self.resync_required)
            .finish()
    }
}

struct TargetStateSeed {
    target: TargetRef,
    input_owner: InputOwner,
    journal: EventJournal,
    last_owner_observation: Option<OwnerObservation>,
}

impl TargetStateSeed {
    fn prepare(generation: u64, target: TargetRef) -> Result<Self, ServerError> {
        if generation == 0 || !super::target_is_valid(target) || target.generation != generation {
            return Err(ServerError::InvalidConfiguration);
        }
        let input_owner = InputOwner::new();
        let last_owner_observation = super::owner_observation(input_owner.snapshot());
        let journal = EventJournal::new(generation).map_err(|_| ServerError::Internal)?;
        Ok(Self {
            target,
            input_owner,
            journal,
            last_owner_observation,
        })
    }

    fn attach<E: EffectSink>(self, effects: E) -> TargetState<E> {
        let last_screen_revision = effects.screen_revision().filter(|revision| *revision != 0);
        let last_child_exited = effects.child_exited();
        TargetState {
            target: self.target,
            input_owner: self.input_owner,
            effects,
            journal: self.journal,
            notifications_available: last_screen_revision.is_some()
                && self.last_owner_observation.is_some(),
            last_screen_revision,
            last_owner_observation: self.last_owner_observation,
            last_child_exited,
        }
    }
}

/// One trusted owner for a generation's shared authority and up to four retained
/// process sessions. This does not authenticate callers or select launch
/// policy; a runtime must perform those checks before using the owner.
pub struct MultiSessionOwner<E: EffectSink> {
    global: GlobalAuthority,
    launches: ManagedLaunchCoordinator<TargetState<E>>,
    /// Volatile layout established by the first Applied `execute_launch`.
    layout: Option<PaneLayout>,
    workspace_events: Option<WorkspaceEventJournal>,
    workspace_event_key: Option<(u64, bool)>,
    workspace_events_failed: bool,
    /// Every committed split result, retained for exact-duplicate retries.
    /// Bounded by the coordinator's retained launch results, because each
    /// committed split spends one launch identity.
    committed_splits: Vec<SplitResult>,
    /// Every recorded close result, retained for exact duplicates and status.
    close_results: Vec<CloseResult>,
    active_closes: Vec<ActiveClose>,
    /// Bounded per-pane size state keyed by full target: last confirmed
    /// absolute PTY size and, while a resize is pending, the desired size.
    pane_sizes: [Option<PaneSizeState>; crate::pane_layout::MAX_PANES],
    /// Display-only projection state; the underlying split tree remains intact.
    zoomed_target: Option<TargetRef>,
    /// One schema-7 split whose prepare/spawn/journal work is outside the
    /// owner thread. All authority and topology remain here.
    active_async_split: Option<Box<ActiveAsyncSplit>>,
    /// A child handle returned by a worker whose key/target could not be
    /// reconciled with the active reservation. Retained until owner teardown.
    async_emergency_effect: Option<Box<E>>,
}

#[derive(Clone, Copy)]
enum ActiveClosePhase {
    Hup { deadline: Instant },
    Kill { deadline: Instant },
}

struct ActiveClose {
    request: CloseRequest,
    phase: ActiveClosePhase,
}

/// State of a keyed close query while the owner advances stop work.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum CloseStatus {
    /// No close admission exists for this key.
    NotAdmitted,
    /// The close was admitted and remains owner work.
    InProgress,
    /// The operation has one retained terminal result.
    Terminal(CloseResult),
}

#[derive(Clone, Copy)]
struct PaneSizeState {
    target: TargetRef,
    confirmed: (u16, u16),
    desired: Option<(u16, u16)>,
}

/// Read-only view of one pane whose absolute PTY resize has not succeeded.
///
/// The layout rectangle is the desired size; the confirmed size is the last
/// size this owner launched or successfully resized the session to. A UI must
/// treat the pane as resize-pending until a later absolute resize succeeds.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PendingResize {
    /// Complete identity of the pane occupant.
    pub target: TargetRef,
    /// Desired `(rows, columns)` from the committed layout.
    pub desired: (u16, u16),
    /// Last confirmed `(rows, columns)` applied to the session.
    pub confirmed: (u16, u16),
}

impl fmt::Debug for PendingResize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingResize")
            .field("target", &"<redacted>")
            .field("desired", &self.desired)
            .field("confirmed", &self.confirmed)
            .finish()
    }
}

impl<E: EffectSink> fmt::Debug for MultiSessionOwner<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MultiSessionOwner")
            .field("generation", &"<redacted>")
            .field("active_leases", &self.global.grants.len())
            .field("launches", &self.launches)
            .field("layout_established", &self.layout.is_some())
            .field("committed_splits", &self.committed_splits.len())
            .field(
                "async_emergency_effect_retained",
                &self.async_emergency_effect.is_some(),
            )
            .field("close_results", &self.close_results.len())
            .field("active_closes", &self.active_closes.len())
            .field(
                "pending_resizes",
                &self
                    .pane_sizes
                    .iter()
                    .flatten()
                    .filter(|state| state.desired.is_some())
                    .count(),
            )
            .finish_non_exhaustive()
    }
}

/// Fixed reason a new split operation was recorded as `Failed`.
///
/// Every reason means no layout commit, no resize, and no child process. Only
/// [`SplitFailure::LaunchFailed`] spends a registry identity (a known Intent or
/// spawn failure after reservation); every other reason consumes none.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SplitFailure {
    /// No layout exists yet; no first launch reached Applied.
    NoLayout,
    /// The target is not a retained pane occupant in the current layout.
    StaleTarget,
    /// The expected layout revision is not the current revision.
    StaleLayout,
    /// The four-session workspace capacity is exhausted.
    Capacity,
    /// The launch coordinator latched unhealthy and accepts no new launches.
    Unhealthy,
    /// The registry could not allocate a checked identity.
    Registry(RegistryError),
    /// The layout rejected the split geometry or state.
    Layout(LayoutError),
    /// The embedded launch rows/columns differ from the planned new pane.
    LaunchSizeMismatch,
    /// Pure launch preparation failed before reservation.
    PreparationFailed,
    /// Launch journal preflight failed before reservation.
    JournalPreflightFailed,
    /// A known Intent or spawn failure after reservation; no child exists.
    LaunchFailed,
    /// An exact duplicate of a key already recorded `Failed`. The original
    /// reason is not retained; the recorded result is identical.
    PreviouslyFailed,
}

/// Fixed, redacted failures of the trusted split transaction.
pub enum SplitError {
    /// The schema-7 payload is malformed; nothing was admitted.
    InvalidPayload(SplitCodecError),
    /// The key generation is not this owner's generation.
    GenerationMismatch,
    /// Ledger validation, conflict, or capacity rejected the key before effects.
    Ledger(ProtocolError),
    /// The key is admitted but has no terminal retained result yet.
    InProgress,
    /// The new operation was admitted and recorded `Failed`; `result` is the
    /// exact value an identical retry returns.
    Failed {
        /// Why this first execution failed.
        reason: SplitFailure,
        /// Recorded result: no committed revision and no new target.
        result: Box<SplitResult>,
    },
    /// An internal invariant failed; the ledger records `Unknown` when admitted.
    InternalInvariant,
}

impl SplitError {
    /// Return the protocol classification without formatting payload data.
    pub fn ledger_error(&self) -> Option<&ProtocolError> {
        match self {
            Self::Ledger(error) => Some(error),
            _ => None,
        }
    }

    /// Return the recorded `Failed` result and its reason, when admitted.
    pub fn failure(&self) -> Option<(SplitFailure, SplitResult)> {
        match self {
            Self::Failed { reason, result } => Some((*reason, **result)),
            _ => None,
        }
    }
}

impl fmt::Debug for SplitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPayload(error) => f.debug_tuple("InvalidPayload").field(error).finish(),
            Self::GenerationMismatch => f.write_str("GenerationMismatch"),
            Self::Ledger(_) => f.write_str("Ledger(<redacted>)"),
            Self::InProgress => f.write_str("InProgress"),
            Self::Failed { reason, .. } => f
                .debug_struct("Failed")
                .field("reason", reason)
                .field("result", &"<redacted>")
                .finish(),
            Self::InternalInvariant => f.write_str("InternalInvariant"),
        }
    }
}

impl fmt::Display for SplitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidPayload(_) => "pane split payload is invalid",
            Self::GenerationMismatch => "pane split generation is invalid",
            Self::Ledger(ProtocolError::OperationConflict) => {
                "operation key was reused with a different pane split"
            }
            Self::Ledger(_) => "pane split ledger validation failed",
            Self::InProgress => "pane split result is not yet available",
            Self::Failed { reason, .. } => match reason {
                SplitFailure::NoLayout => "no pane layout exists",
                SplitFailure::StaleTarget => "pane split target is stale",
                SplitFailure::StaleLayout => "pane layout revision is stale",
                SplitFailure::Capacity => "pane or session capacity is exhausted",
                SplitFailure::Unhealthy => "managed launch coordinator is unhealthy",
                SplitFailure::Registry(_) => "managed session identity is unavailable",
                SplitFailure::Layout(_) => "pane layout rejected the split",
                SplitFailure::LaunchSizeMismatch => "launch size does not match the new pane",
                SplitFailure::PreparationFailed => "managed launch preparation failed",
                SplitFailure::JournalPreflightFailed => "launch journal preflight failed",
                SplitFailure::LaunchFailed => "managed process launch failed",
                SplitFailure::PreviouslyFailed => "pane split previously failed",
            },
            Self::InternalInvariant => "pane split invariant failed",
        })
    }
}

impl std::error::Error for SplitError {}

/// Failures of a layout-owned root resize.
#[derive(Debug)]
pub enum LayoutResizeError {
    /// No layout exists yet.
    NoLayout,
    /// A split launch is in progress; topology changes are temporarily held.
    InProgress,
    /// The layout rejected the request; nothing changed and no PTY was resized.
    Layout(LayoutError),
    /// The new layout is committed but some sessions were not resized.
    ///
    /// `failed[i]` corresponds to `snapshot.panes[i]`. Reconcile with another
    /// absolute resize; never a relative delta.
    ResizePending {
        /// Committed layout whose rectangles are the desired sizes.
        snapshot: Box<LayoutSnapshot>,
        /// Which pane sessions did not accept their absolute size.
        failed: [bool; crate::pane_layout::MAX_PANES],
    },
}

impl fmt::Display for LayoutResizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoLayout => "no pane layout exists",
            Self::InProgress => "pane split is in progress",
            Self::Layout(LayoutError::StaleRevision) => "pane layout revision is stale",
            Self::Layout(LayoutError::LayoutTooSmall | LayoutError::InvalidDimensions) => {
                "pane layout is too small or too large"
            }
            Self::Layout(_) => "pane layout rejected the resize",
            Self::ResizePending { .. } => "pane layout committed; session resize is pending",
        })
    }
}

impl std::error::Error for LayoutResizeError {}

impl<E: EffectSink> MultiSessionOwner<E> {
    /// Authorize a schema-12 query and return the captured direct-child
    /// identity, if the live child has one.
    pub(crate) fn authorize_metadata_query(
        &mut self,
        connection: &Connection,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<(TargetRef, Option<ProcessIdentity>, PathStatus), ServerError> {
        let target = decode_query(payload).map_err(|_| ServerError::TargetStale)?;
        self.global.require_live_connection(connection, now_secs)?;
        let required = CAPABILITY_READ | crate::pane_metadata_protocol::CAPABILITY_PANE_METADATA;
        if connection.selected_capabilities & required != required
            || self
                .global
                .grants
                .get(&connection.lease)
                .map(|grant| grant.target)
                != Some(target)
        {
            return Err(ServerError::Unauthorized);
        }
        let session = self
            .launches
            .session_mut(target)
            .map_err(map_connect_registry_error)?;
        if session.effects.child_exited() {
            return Ok((target, None, PathStatus::Exited));
        }
        let identity = session.effects.process_identity();
        let status = if identity.is_some() {
            PathStatus::Available
        } else {
            PathStatus::Unsupported
        };
        Ok((target, identity, status))
    }

    /// Revalidate authority, target, liveness, and captured identity before a
    /// native worker completion can be published.
    pub(crate) fn metadata_publish_allowed(
        &mut self,
        connection: &Connection,
        target: TargetRef,
        identity: ProcessIdentity,
        now_secs: u64,
    ) -> bool {
        let required = CAPABILITY_READ | crate::pane_metadata_protocol::CAPABILITY_PANE_METADATA;
        if self
            .global
            .require_live_connection(connection, now_secs)
            .is_err()
            || connection.selected_capabilities & required != required
            || self
                .global
                .grants
                .get(&connection.lease)
                .map(|grant| grant.target)
                != Some(target)
        {
            return false;
        }
        self.launches.session_mut(target).is_ok_and(|session| {
            !session.effects.child_exited() && session.effects.process_identity() == Some(identity)
        })
    }

    /// Adopt a pre-opened ledger and trusted internal launch lease.
    ///
    /// The caller must open the internal lease before handing over the ledger;
    /// this owner deliberately exposes neither lease creation nor mutable
    /// ledger access. The owner must remain protected by the server's storage
    /// lock for its complete lifetime.
    pub fn with_ledger(ledger: Ledger, allow_input: bool) -> Result<Self, ServerError> {
        let generation = ledger.generation();
        if generation == 0 {
            return Err(ServerError::InvalidConfiguration);
        }
        let launches = ManagedLaunchCoordinator::new(generation)
            .map_err(|_| ServerError::InvalidConfiguration)?;
        let grants = HashMap::with_capacity(MAX_ACTIVE_LEASES);
        Ok(Self {
            global: GlobalAuthority {
                generation,
                allow_input,
                ledger,
                grants,
            },
            launches,
            layout: None,
            workspace_events: None,
            workspace_event_key: None,
            workspace_events_failed: false,
            committed_splits: Vec::new(),
            close_results: Vec::new(),
            active_closes: Vec::new(),
            pane_sizes: [None; crate::pane_layout::MAX_PANES],
            zoomed_target: None,
            active_async_split: None,
            async_emergency_effect: None,
        })
    }

    /// Return whether at least one session exists and every retained session
    /// has exited or failed, so no live child remains.
    pub fn all_sessions_ended(&self) -> bool {
        // A worker-returned handle that could not be attached is still a
        // server-owned child. Never publish a terminal-owner result while it
        // is retained for emergency shutdown.
        if self.async_emergency_effect.is_some() || self.active_async_split.is_some() {
            return false;
        }
        let mut any = false;
        for record in self.launches.records() {
            any = true;
            if !matches!(record.state, SessionState::Exited | SessionState::Failed) {
                return false;
            }
        }
        any
    }

    /// Owner-serialized proof that no live pane remains: true only when a
    /// layout exists and EVERY target in it has a positively recorded
    /// `Exited` state. Unknown, failed lookup or a live state is not proof.
    pub fn layout_terminal_empty(&self) -> bool {
        if self.active_async_split.is_some() {
            return false;
        }
        let Some(layout) = self.layout_snapshot() else {
            return false;
        };
        let mut any = false;
        for pane in layout.panes.into_iter().flatten() {
            any = true;
            if self.launches.state(pane.target) != Ok(SessionState::Exited) {
                return false;
            }
        }
        any
    }

    /// Return a copy of the current pane layout, if one is established.
    pub fn layout_snapshot(&self) -> Option<LayoutSnapshot> {
        self.layout.as_ref().map(PaneLayout::snapshot)
    }

    /// Record committed topology on the owner thread. Notification failure
    /// never rolls back a layout or changes a child/process result.
    fn record_workspace_layout(&mut self) {
        if self.workspace_events_failed {
            return;
        }
        let Some(snapshot) = self.layout_snapshot() else {
            return;
        };
        let Some(first) = snapshot.panes.iter().flatten().next() else {
            self.workspace_events_failed = true;
            return;
        };
        let workspace = first.target.workspace;
        if snapshot.panes.iter().flatten().any(|pane| {
            pane.target.workspace != workspace || pane.target.generation != self.global.generation
        }) {
            self.workspace_events_failed = true;
            return;
        }
        if self.workspace_events.is_none() {
            self.workspace_events =
                WorkspaceEventJournal::new(self.global.generation, workspace).ok();
        }
        let empty = self.layout_terminal_empty();
        let key = (snapshot.revision, empty);
        if self.workspace_event_key == Some(key) {
            return;
        }
        let Some(journal) = self.workspace_events.as_mut() else {
            self.workspace_events_failed = true;
            return;
        };
        if journal.current_cursor().workspace != workspace
            || journal.push(snapshot.revision, empty).is_err()
        {
            self.workspace_events_failed = true;
            return;
        }
        self.workspace_event_key = Some(key);
    }

    fn require_workspace_authority(
        &self,
        connection: &Connection,
        generation: u64,
        workspace: u64,
        now_secs: u64,
    ) -> Result<(), ServerError> {
        self.global.require_live_connection(connection, now_secs)?;
        let required = CAPABILITY_READ | CAPABILITY_WORKSPACE_EVENTS;
        if connection.selected_capabilities & required != required {
            return Err(ServerError::Unauthorized);
        }
        if generation != self.global.generation {
            return Err(ServerError::WrongGeneration);
        }
        let grant = self
            .global
            .grants
            .get(&connection.lease)
            .ok_or(ServerError::UnknownContext)?;
        // The grant can outlive the pane it originally named. Its immutable
        // workspace authority does not grant access to any other workspace.
        if workspace == 0 || grant.target.workspace != workspace {
            return Err(ServerError::Unauthorized);
        }
        if self.workspace_events_failed {
            return Err(ServerError::NotificationsUnavailable);
        }
        let journal = self
            .workspace_events
            .as_ref()
            .ok_or(ServerError::NotificationsUnavailable)?;
        if journal.current_cursor().workspace != workspace {
            return Err(ServerError::Unauthorized);
        }
        Ok(())
    }

    /// Atomically copy the current layout and workspace event cursor. This
    /// reads only metadata and never reserves an operation key or polls a PTY.
    pub fn reconcile_workspace(
        &mut self,
        connection: &Connection,
        generation: u64,
        workspace: u64,
        now_secs: u64,
    ) -> Result<WorkspaceReconciliation, ServerError> {
        self.require_workspace_authority(connection, generation, workspace, now_secs)?;
        let snapshot = self.layout_snapshot().ok_or(ServerError::InvalidSnapshot)?;
        let cursor = self
            .workspace_events
            .as_ref()
            .ok_or(ServerError::NotificationsUnavailable)?
            .current_cursor();
        let layout = if self.layout_terminal_empty() {
            None
        } else {
            let pending = self.pending_resizes();
            Some(crate::ui_protocol::LayoutReply {
                revision: snapshot.revision,
                rows: snapshot.rows,
                columns: snapshot.columns,
                panes: snapshot.panes.map(|pane| {
                    pane.map(|pane| crate::ui_protocol::UiPane {
                        target: pane.target,
                        rect: pane.rect,
                        pending_confirmed: pending
                            .iter()
                            .flatten()
                            .find(|entry| entry.target == pane.target)
                            .map(|entry| entry.confirmed),
                    })
                }),
                divider: snapshot.divider,
                dividers: snapshot.dividers,
            })
        };
        let state = WorkspaceReconciliation {
            cursor,
            revision: snapshot.revision,
            layout,
        };
        crate::workspace_watch_protocol::encode_reconciliation(&state)
            .map_err(|_| ServerError::InvalidSnapshot)?;
        Ok(state)
    }

    /// Read a bounded workspace event page, independently of pane journals.
    /// Expired cursors return a resync hint; other invalid cursors are errors.
    pub fn read_workspace_events(
        &mut self,
        connection: &Connection,
        cursor: WorkspaceCursor,
        limit: u16,
        now_secs: u64,
    ) -> Result<WorkspaceWatchReply, ServerError> {
        self.require_workspace_authority(
            connection,
            cursor.generation,
            cursor.workspace,
            now_secs,
        )?;
        let journal = self
            .workspace_events
            .as_ref()
            .ok_or(ServerError::NotificationsUnavailable)?;
        match journal.read_after(cursor, limit) {
            Ok(events) => {
                let sequence = events
                    .last()
                    .map_or(cursor.sequence, |event| event.sequence);
                Ok(WorkspaceWatchReply {
                    status: if events.is_empty() {
                        crate::watch_protocol::WatchStatus::TimedOut
                    } else {
                        crate::watch_protocol::WatchStatus::Changed
                    },
                    cursor: WorkspaceCursor { sequence, ..cursor },
                    events,
                })
            }
            Err(WorkspaceJournalError::CursorExpired) => Ok(WorkspaceWatchReply {
                status: crate::watch_protocol::WatchStatus::ResyncRequired,
                cursor: journal.current_cursor(),
                events: Vec::new(),
            }),
            Err(WorkspaceJournalError::FutureCursor) => Err(ServerError::CursorAhead),
            Err(WorkspaceJournalError::ScopeMismatch) => Err(ServerError::Unauthorized),
            Err(_) => Err(ServerError::InvalidWatchRequest),
        }
    }

    /// Visit only currently retained process handles for server-owned
    /// maintenance (for example checkpoint capture). No target lookup or
    /// caller authority is inferred from this internal iteration.
    pub fn for_each_effects_mut(&mut self, mut visit: impl FnMut(TargetRef, &mut E)) {
        let targets: Vec<_> = self
            .launches
            .records()
            .map(|record| record.target)
            .collect();
        for target in targets {
            if let Ok(session) = self.launches.session_mut(target) {
                visit(target, &mut session.effects);
            }
        }
    }

    /// Return every layout pane whose absolute PTY resize is still pending.
    ///
    /// Entries are bounded by the four-pane workspace; unused entries are
    /// `None`. A pending pane is retried by [`Self::resize_layout`], including
    /// at the current root size, without changing the layout revision.
    pub fn pending_resizes(&self) -> [Option<PendingResize>; crate::pane_layout::MAX_PANES] {
        self.pane_sizes.map(|state| {
            state.and_then(|state| {
                state.desired.map(|desired| PendingResize {
                    target: state.target,
                    desired,
                    confirmed: state.confirmed,
                })
            })
        })
    }

    /// Whether the owner currently retains one accepted schema-7 split.
    pub(crate) fn async_split_active(&self) -> bool {
        self.active_async_split.is_some()
    }

    /// Key whose accepted split currently owns the launch-work slot.
    pub(crate) fn async_split_key(&self) -> Option<OperationKey> {
        self.active_async_split.as_ref().map(|split| split.key)
    }

    /// Read a retained terminal split result without reauthorizing or extending
    /// the request's lease. This also settles bounded pending reply waiters.
    pub(crate) fn async_split_result(&self, key: OperationKey) -> Option<SplitResult> {
        self.committed_splits
            .iter()
            .find(|result| result.key == key)
            .copied()
    }

    /// Admit and reserve one schema-7 split without preparation, storage IO or
    /// process creation. Only the exact duplicate path may observe the active
    /// operation while it is pending.
    pub(crate) fn begin_async_split(
        &mut self,
        payload: &[u8],
        now_secs: u64,
        can_start: bool,
    ) -> Result<AsyncSplitAdmission, SplitError> {
        let request = decode_split_request(payload).map_err(SplitError::InvalidPayload)?;
        let key = request.key();
        if key.generation != self.global.generation {
            return Err(SplitError::GenerationMismatch);
        }
        if let Some(admission) = self
            .global
            .ledger
            .existing_admission(key, payload, now_secs)
            .map_err(SplitError::Ledger)?
        {
            if self
                .active_async_split
                .as_ref()
                .is_some_and(|active| active.key == key)
                && matches!(admission, Admission::InProgress)
            {
                return Ok(AsyncSplitAdmission::Pending);
            }
            if let Some(result) = self.async_split_result(key) {
                return Ok(AsyncSplitAdmission::Terminal(result));
            }
            if matches!(admission, Admission::Completed(Outcome::Unknown))
                && self
                    .launches
                    .retained_result(key)
                    .is_some_and(|result| result.outcome == Outcome::Unknown)
            {
                return Ok(AsyncSplitAdmission::Uncertain);
            }
            return match self.split_duplicate(key, request.target(), admission) {
                Ok(result) => Ok(AsyncSplitAdmission::Terminal(result)),
                Err(SplitError::Failed { result, .. }) => {
                    Ok(AsyncSplitAdmission::Terminal(*result))
                }
                Err(error) => Err(error),
            };
        }
        if self.active_async_split.is_some()
            || self.async_emergency_effect.is_some()
            || !self.active_closes.is_empty()
            || !can_start
        {
            return Err(SplitError::InProgress);
        }
        match self
            .global
            .ledger
            .admit(key, payload, now_secs)
            .map_err(SplitError::Ledger)?
        {
            Admission::Execute => {}
            Admission::InProgress | Admission::Completed(_) => {
                return Err(SplitError::InternalInvariant);
            }
        }

        let failed = SplitResult {
            key,
            outcome: Outcome::Failed,
            committed_layout_revision: 0,
            existing_target: request.target(),
            new_target: None,
        };
        if self.launches.retained_result(key).is_some() {
            let _ = self.global.ledger.complete(key, Outcome::Failed);
            return Err(SplitError::InternalInvariant);
        }
        self.reclaim_failed_slots();
        let (preview, new_target) = match self.plan_split(&request) {
            Ok(plan) => plan,
            Err(_) => {
                self.global
                    .ledger
                    .complete(key, Outcome::Failed)
                    .map_err(|_| SplitError::InternalInvariant)?;
                return Ok(AsyncSplitAdmission::Terminal(failed));
            }
        };
        let planned = preview.planned_snapshot();
        let Some(existing_rect) = planned
            .panes
            .into_iter()
            .flatten()
            .find(|pane| pane.target == request.target())
            .map(|pane| pane.rect)
        else {
            self.global
                .ledger
                .complete(key, Outcome::Failed)
                .map_err(|_| SplitError::InternalInvariant)?;
            return Ok(AsyncSplitAdmission::Terminal(failed));
        };
        let seed = match TargetStateSeed::prepare(self.global.generation, new_target) {
            Ok(seed) => seed,
            Err(_) => {
                self.global
                    .ledger
                    .complete(key, Outcome::Failed)
                    .map_err(|_| SplitError::InternalInvariant)?;
                return Ok(AsyncSplitAdmission::Terminal(failed));
            }
        };
        if self.launches.reserve_async(key, new_target).is_err() {
            self.global
                .ledger
                .complete(key, Outcome::Failed)
                .map_err(|_| SplitError::InternalInvariant)?;
            return Ok(AsyncSplitAdmission::Terminal(failed));
        }
        self.active_async_split = Some(Box::new(ActiveAsyncSplit {
            key,
            existing_target: request.target(),
            new_target,
            preview,
            existing_rect,
            dimensions: request.launch().dimensions(),
            seed: Some(seed),
            child_attached: false,
        }));
        Ok(AsyncSplitAdmission::Start {
            key,
            target: new_target,
        })
    }

    /// Attach a child returned by the worker. The owner consumes its prebuilt
    /// target state before acknowledging the worker's final journal phase.
    pub(crate) fn attach_async_split(
        &mut self,
        key: OperationKey,
        target: TargetRef,
        effects: E,
    ) -> Result<(), SplitError> {
        let seed = match self
            .active_async_split
            .as_mut()
            .filter(|active| active.key == key && active.new_target == target)
            .and_then(|active| active.seed.take())
        {
            Some(seed) => seed,
            None => {
                debug_assert!(self.async_emergency_effect.is_none());
                self.async_emergency_effect = Some(Box::new(effects));
                self.launches.mark_unhealthy();
                return Err(SplitError::InternalInvariant);
            }
        };
        if let Some(active) = self.active_async_split.as_mut() {
            active.child_attached = true;
        }
        self.launches
            .attach_async(key, target, seed.attach(effects))
            .map_err(|_| SplitError::InternalInvariant)
    }

    /// Finish one attached async split after the worker's Spawned/Applied
    /// journal result arrives. Unknown still commits and retains the child.
    pub(crate) fn finish_async_split<RE, Resize>(
        &mut self,
        key: OperationKey,
        target: TargetRef,
        outcome: Outcome,
        unhealthy: bool,
        mut resize: Resize,
    ) -> Result<AsyncSplitAdmission, SplitError>
    where
        Resize: FnMut(&mut E, u16, u16) -> Result<(), RE>,
    {
        let Some(active) = self
            .active_async_split
            .as_ref()
            .filter(|active| active.key == key && active.new_target == target)
        else {
            return Err(SplitError::InternalInvariant);
        };
        if !active.child_attached {
            let terminal = if outcome == Outcome::Failed {
                Outcome::Failed
            } else {
                Outcome::Unknown
            };
            return self.fail_async_split(key, target, terminal, unhealthy);
        }
        let active = self
            .active_async_split
            .take()
            .expect("active split was checked above");
        let committed = match self
            .layout
            .as_mut()
            .map(|layout| layout.commit_split(active.preview))
        {
            Some(Ok(snapshot)) => snapshot,
            _ => {
                // The process exists, but the owner could not publish its
                // preview. Preserve the child while recording uncertainty.
                self.launches
                    .finish_async(key, target, Outcome::Unknown, true, true)
                    .map_err(|_| SplitError::InternalInvariant)?;
                self.global
                    .ledger
                    .complete(key, Outcome::Unknown)
                    .map_err(|_| SplitError::InternalInvariant)?;
                return Ok(AsyncSplitAdmission::Uncertain);
            }
        };
        self.record_workspace_layout();
        let launch = self
            .launches
            .finish_async(key, target, outcome, true, unhealthy)
            .map_err(|_| SplitError::InternalInvariant)?;
        self.record_pane_size(active.new_target, active.dimensions, None);
        let existing_exited =
            self.launches.state(active.existing_target) == Ok(SessionState::Exited);
        let resized = existing_exited
            || self.apply_pane_size(active.existing_target, active.existing_rect, &mut resize);
        let final_outcome = match (launch.outcome, resized) {
            (Outcome::Applied, true) => Outcome::Applied,
            (Outcome::Applied, false) => Outcome::Partial,
            _ => Outcome::Unknown,
        };
        let mut result = SplitResult {
            key,
            outcome: final_outcome,
            committed_layout_revision: committed.revision,
            existing_target: active.existing_target,
            new_target: Some(active.new_target),
        };
        if self.collapse_exited_panes().is_err() {
            result.outcome = Outcome::Unknown;
        }
        self.committed_splits.push(result);
        if self.global.ledger.complete(key, result.outcome).is_err() {
            result.outcome = Outcome::Unknown;
            if let Some(retained) = self.committed_splits.last_mut() {
                *retained = result;
            }
        }
        Ok(AsyncSplitAdmission::Terminal(result))
    }

    /// Complete a worker failure known to have acquired no child.
    pub(crate) fn fail_async_split(
        &mut self,
        key: OperationKey,
        target: TargetRef,
        outcome: Outcome,
        unhealthy: bool,
    ) -> Result<AsyncSplitAdmission, SplitError> {
        let active = self
            .active_async_split
            .as_ref()
            .filter(|active| active.key == key && active.new_target == target)
            .ok_or(SplitError::InternalInvariant)?;
        if active.child_attached {
            return Err(SplitError::InternalInvariant);
        }
        let launch = self
            .launches
            .finish_async(key, target, outcome, false, unhealthy)
            .map_err(|_| SplitError::InternalInvariant)?;
        let active = self
            .active_async_split
            .take()
            .expect("active split was checked above");
        let split_outcome = launch.outcome;
        if split_outcome == Outcome::Unknown {
            self.global
                .ledger
                .complete(key, Outcome::Unknown)
                .map_err(|_| SplitError::InternalInvariant)?;
            return Ok(AsyncSplitAdmission::Uncertain);
        }
        let result = SplitResult {
            key,
            outcome: Outcome::Failed,
            committed_layout_revision: 0,
            existing_target: active.existing_target,
            new_target: None,
        };
        self.global
            .ledger
            .complete(key, Outcome::Failed)
            .map_err(|_| SplitError::InternalInvariant)?;
        self.committed_splits.push(result);
        Ok(AsyncSplitAdmission::Terminal(result))
    }

    /// Execute or observe one trusted schema-6 managed launch.
    ///
    /// The caller must validate launch authority before invoking this method.
    /// `prepare` must remain pure; `spawn` must return an error only when no
    /// child handle was acquired. The returned result contains only the fixed
    /// existing operation outcome and target identity.
    ///
    /// The first launch that reaches `Applied` while no layout exists also
    /// establishes the owner's volatile layout: revision 1, its target as the
    /// single pane, and the launch rows/columns as the root. The launch result
    /// is never changed by layout establishment; if the root were invalid the
    /// layout would stay absent and splits would fail with `NoLayout`. Later
    /// launches through this method are retained sessions outside the layout.
    pub fn execute_launch<P, PE, SE, J, Prepare, Spawn>(
        &mut self,
        payload: &[u8],
        now_secs: u64,
        journal: &mut J,
        prepare: Prepare,
        spawn: Spawn,
    ) -> Result<LaunchResult, ManagedLaunchError>
    where
        J: LaunchJournalPort,
        Prepare: FnOnce(LaunchRequestView<'_>, TargetRef) -> Result<P, PE>,
        Spawn: FnOnce(P) -> Result<E, SE>,
    {
        if self.active_async_split.is_some() || self.async_emergency_effect.is_some() {
            if let Ok(request) = decode_launch_request(payload) {
                if self
                    .global
                    .ledger
                    .existing_admission(request.key(), payload, now_secs)
                    .map_err(ManagedLaunchError::Ledger)?
                    .is_none()
                {
                    return Err(ManagedLaunchError::InProgress);
                }
            }
        }
        let generation = self.global.generation;
        let result = self.launches.execute(
            payload,
            now_secs,
            &mut self.global.ledger,
            journal,
            |request, target| {
                let seed = TargetStateSeed::prepare(generation, target).map_err(|_| ())?;
                let prepared = prepare(request, target).map_err(|_| ())?;
                Ok::<_, ()>((prepared, seed))
            },
            |(prepared, seed)| {
                spawn(prepared)
                    .map(|effects| seed.attach(effects))
                    .map_err(|_| ())
            },
        )?;
        if self.layout.is_none() && result.outcome == Outcome::Applied {
            if let (Some(target), Ok(request)) = (result.target, decode_launch_request(payload)) {
                let (rows, columns) = request.dimensions();
                self.layout = PaneLayout::new(target, rows, columns).ok();
                if self.layout.is_some() {
                    self.pane_sizes = [
                        Some(PaneSizeState {
                            target,
                            confirmed: (rows, columns),
                            // The launch dimensions describe the outer root;
                            // the UI's content rectangle is smaller. Startup
                            // performs an immediate owner resize before the
                            // server is published, and this pending marker
                            // ensures that same-revision retry is not skipped.
                            desired: Some((rows, columns)),
                        }),
                        None,
                        None,
                        None,
                    ];
                }
            }
        }
        self.record_workspace_layout();
        Ok(result)
    }

    /// Execute or observe one trusted schema-7 pane split.
    ///
    /// The caller must validate split/launch authority first. Ordering:
    /// decode; exact-payload duplicate lookup in the one global ledger
    /// (returning the retained result before any layout, target or capacity
    /// check; changed bytes conflict); admission of the complete split bytes;
    /// then, for a new key only, target/revision/capacity/geometry and exact
    /// launch-size validation. A rejection there records `Failed` and spends
    /// no identity. Otherwise exactly one child is launched through the
    /// coordinator's journaled path under the split's own key (not admitted a
    /// second time). The layout commits only after that child is acquired;
    /// a known pre-acquisition failure records `Failed` with no commit. After
    /// commit, the existing session receives one absolute resize to its new
    /// rectangle through `resize`. Outcomes: `Applied`; `Partial` when that
    /// resize fails; `Unknown` when the child's journal/result boundary is
    /// uncertain (the child stays retained). No input is ever sent, no child
    /// is respawned or killed as rollback, and the split claims no input
    /// ownership.
    pub fn execute_split<P, PE, SE, RE, J, Prepare, Spawn, Resize>(
        &mut self,
        payload: &[u8],
        now_secs: u64,
        journal: &mut J,
        prepare: Prepare,
        spawn: Spawn,
        mut resize: Resize,
    ) -> Result<SplitResult, SplitError>
    where
        J: LaunchJournalPort,
        Prepare: FnOnce(LaunchRequestView<'_>, TargetRef) -> Result<P, PE>,
        Spawn: FnOnce(P) -> Result<E, SE>,
        Resize: FnMut(&mut E, u16, u16) -> Result<(), RE>,
    {
        let request = decode_split_request(payload).map_err(SplitError::InvalidPayload)?;
        let key = request.key();
        let generation = self.global.generation;
        if key.generation != generation {
            return Err(SplitError::GenerationMismatch);
        }
        if let Some(admission) = self
            .global
            .ledger
            .existing_admission(key, payload, now_secs)
            .map_err(SplitError::Ledger)?
        {
            return self.split_duplicate(key, request.target(), admission);
        }
        if self.active_async_split.is_some() || self.async_emergency_effect.is_some() {
            return Err(SplitError::InProgress);
        }
        match self
            .global
            .ledger
            .admit(key, payload, now_secs)
            .map_err(SplitError::Ledger)?
        {
            Admission::Execute => {}
            Admission::InProgress | Admission::Completed(_) => {
                return Err(SplitError::InternalInvariant);
            }
        }

        let failed = SplitResult {
            key,
            outcome: Outcome::Failed,
            committed_layout_revision: 0,
            existing_target: request.target(),
            new_target: None,
        };
        if self.launches.retained_result(key).is_some() {
            // Unreachable while the ledger owns key uniqueness; nothing ran.
            let _ = self.global.ledger.complete(key, Outcome::Failed);
            return Err(SplitError::InternalInvariant);
        }
        // Known-failed launch records outside the layout hold no child and
        // may be retired so their slot can host this split.
        self.reclaim_failed_slots();
        let (preview, new_target) = match self.plan_split(&request) {
            Ok(plan) => plan,
            Err(reason) => return self.finish_split_failed(failed, reason),
        };

        let launch = self.launches.execute_admitted(
            request.launch(),
            new_target,
            journal,
            |launch, target| {
                let seed = TargetStateSeed::prepare(generation, target).map_err(|_| ())?;
                let prepared = prepare(launch, target).map_err(|_| ())?;
                Ok::<_, ()>((prepared, seed))
            },
            |(prepared, seed)| {
                spawn(prepared)
                    .map(|effects| seed.attach(effects))
                    .map_err(|_| ())
            },
        );
        // The coordinator's retained result, not the call's return value, is
        // authoritative about whether a child may have been acquired.
        let launch_outcome = match self.launches.retained_result(key) {
            None
            | Some(LaunchResult {
                outcome: Outcome::Failed,
                ..
            }) => {
                let reason = match launch {
                    Err(ManagedLaunchError::PreparationFailed) => SplitFailure::PreparationFailed,
                    Err(ManagedLaunchError::JournalPreflightFailed) => {
                        SplitFailure::JournalPreflightFailed
                    }
                    Err(ManagedLaunchError::Unhealthy) => SplitFailure::Unhealthy,
                    Err(ManagedLaunchError::Registry(RegistryError::Capacity)) => {
                        SplitFailure::Capacity
                    }
                    Err(ManagedLaunchError::Registry(error)) => SplitFailure::Registry(error),
                    _ => SplitFailure::LaunchFailed,
                };
                return self.finish_split_failed(failed, reason);
            }
            Some(LaunchResult {
                outcome: Outcome::Applied,
                target: Some(target),
                ..
            }) if target == new_target => Outcome::Applied,
            // Unknown (or any non-terminal value) may own a child: keep it.
            Some(LaunchResult {
                target: Some(target),
                ..
            }) if target == new_target => Outcome::Unknown,
            Some(_) => {
                let _ = self.global.ledger.complete(key, Outcome::Unknown);
                return Err(SplitError::InternalInvariant);
            }
        };

        let committed = match self
            .layout
            .as_mut()
            .map(|layout| layout.commit_split(preview))
        {
            Some(Ok(snapshot)) => snapshot,
            _ => {
                let _ = self.global.ledger.complete(key, Outcome::Unknown);
                return Err(SplitError::InternalInvariant);
            }
        };
        self.record_workspace_layout();
        let existing_rect = committed
            .panes
            .into_iter()
            .flatten()
            .find(|pane| pane.target == request.target())
            .map(|pane| pane.rect);
        // The acquired child was spawned at exactly its planned rectangle.
        self.record_pane_size(new_target, request.launch().dimensions(), None);
        let resized = existing_rect
            .is_some_and(|rect| self.apply_pane_size(request.target(), rect, &mut resize));
        let outcome = match (launch_outcome, resized) {
            (Outcome::Applied, true) => Outcome::Applied,
            (Outcome::Applied, false) => Outcome::Partial,
            _ => Outcome::Unknown,
        };
        let result = SplitResult {
            key,
            outcome,
            committed_layout_revision: committed.revision,
            existing_target: request.target(),
            new_target: Some(new_target),
        };
        self.committed_splits.push(result);
        if self.global.ledger.complete(key, outcome).is_err() {
            let unknown = SplitResult {
                outcome: Outcome::Unknown,
                ..result
            };
            if let Some(entry) = self.committed_splits.last_mut() {
                *entry = unknown;
            }
            return Ok(unknown);
        }
        Ok(result)
    }

    /// Resize the layout root and apply each pane's absolute rectangle.
    ///
    /// An invalid, stale, or too-small request changes nothing and resizes no
    /// PTY. At the current size the revision is unchanged and only panes in
    /// [`Self::pending_resizes`] are retried, once each, to their current
    /// rectangle; with nothing pending this has no PTY effect. Otherwise the
    /// new revision is committed first and every pane session receives one
    /// absolute resize. Any failure returns
    /// [`LayoutResizeError::ResizePending`] with the committed snapshot,
    /// keeps that pane pending, and never reverts the layout.
    pub fn resize_layout<RE, Resize>(
        &mut self,
        expected_revision: u64,
        rows: u16,
        columns: u16,
        mut resize: Resize,
    ) -> Result<LayoutSnapshot, LayoutResizeError>
    where
        Resize: FnMut(&mut E, u16, u16) -> Result<(), RE>,
    {
        if self.active_async_split.is_some() || self.async_emergency_effect.is_some() {
            return Err(LayoutResizeError::InProgress);
        }
        let layout = self.layout.as_mut().ok_or(LayoutResizeError::NoLayout)?;
        let before = layout.snapshot().revision;
        let snapshot = layout
            .resize(expected_revision, rows, columns)
            .map_err(LayoutResizeError::Layout)?;
        self.record_workspace_layout();
        let retry_pending_only = snapshot.revision == before;
        if let Some(zoomed) = self.zoomed_target {
            let root =
                crate::pane_layout::content_rect(full_root_rect(snapshot.rows, snapshot.columns))
                    .ok_or(LayoutResizeError::Layout(LayoutError::InvalidDimensions))?;
            let zoom_size = (root.rows, root.columns);
            let mut failed = [false; crate::pane_layout::MAX_PANES];
            for (index, pane) in snapshot.panes.into_iter().enumerate() {
                let Some(pane) = pane else {
                    continue;
                };
                let Some(underlying) = crate::pane_layout::content_rect(pane.rect) else {
                    failed[index] = true;
                    continue;
                };
                let under_size = (underlying.rows, underlying.columns);
                if pane.target == zoomed {
                    let confirmed = self
                        .pane_state(zoomed)
                        .map_or(under_size, |state| state.confirmed);
                    let pending = self
                        .pane_state(zoomed)
                        .is_some_and(|state| state.desired == Some(zoom_size));
                    if !retry_pending_only || confirmed != zoom_size || pending {
                        failed[index] =
                            !self.apply_effective_size(zoomed, zoom_size, snapshot, &mut resize);
                    }
                } else {
                    let confirmed = self
                        .pane_state(pane.target)
                        .map_or(under_size, |state| state.confirmed);
                    self.record_pane_size(pane.target, confirmed, None);
                }
            }
            if failed.contains(&true) {
                return Err(LayoutResizeError::ResizePending {
                    snapshot: Box::new(snapshot),
                    failed,
                });
            }
            return Ok(snapshot);
        }
        let mut failed = [false; crate::pane_layout::MAX_PANES];
        for (index, pane) in snapshot.panes.into_iter().enumerate() {
            let Some(pane) = pane else {
                continue;
            };
            if retry_pending_only && !self.is_resize_pending(pane.target) {
                continue;
            }
            failed[index] = !self.apply_pane_size(pane.target, pane.rect, &mut resize);
        }
        if failed.contains(&true) {
            Err(LayoutResizeError::ResizePending {
                snapshot: Box::new(snapshot),
                failed,
            })
        } else {
            Ok(snapshot)
        }
    }

    fn split_duplicate(
        &self,
        key: OperationKey,
        target: TargetRef,
        admission: Admission,
    ) -> Result<SplitResult, SplitError> {
        if let Some(result) = self
            .committed_splits
            .iter()
            .find(|result| result.key == key)
        {
            return Ok(*result);
        }
        match admission {
            // Exact bytes match a split this owner recorded as Failed, so the
            // result is fully determined by the request: no commit, no IDs.
            Admission::Completed(Outcome::Failed) => Err(SplitError::Failed {
                reason: SplitFailure::PreviouslyFailed,
                result: Box::new(SplitResult {
                    key,
                    outcome: Outcome::Failed,
                    committed_layout_revision: 0,
                    existing_target: target,
                    new_target: None,
                }),
            }),
            Admission::InProgress => Err(SplitError::InProgress),
            Admission::Execute | Admission::Completed(_) => Err(SplitError::InternalInvariant),
        }
    }

    fn plan_split(
        &self,
        request: &crate::split_protocol::SplitRequestView<'_>,
    ) -> Result<(crate::pane_layout::SplitPreview, TargetRef), SplitFailure> {
        if self.zoomed_target.is_some() {
            return Err(SplitFailure::Layout(LayoutError::InvalidLayoutState));
        }
        let layout = self.layout.as_ref().ok_or(SplitFailure::NoLayout)?;
        let current = layout.snapshot();
        let target = request.target();
        if self.launches.effects(target).is_err()
            || !current
                .panes
                .into_iter()
                .flatten()
                .any(|pane| pane.target == target)
        {
            return Err(SplitFailure::StaleTarget);
        }
        if request.expected_layout_revision() != current.revision {
            return Err(SplitFailure::StaleLayout);
        }
        if !self.launches.is_healthy() {
            return Err(SplitFailure::Unhealthy);
        }
        let new_target = self.launches.next_target().map_err(|error| match error {
            RegistryError::Capacity => SplitFailure::Capacity,
            other => SplitFailure::Registry(other),
        })?;
        let preview = layout
            .preview_split(
                target,
                request.expected_layout_revision(),
                request.direction(),
                new_target,
            )
            .map_err(|error| match error {
                LayoutError::StaleTarget => SplitFailure::StaleTarget,
                LayoutError::StaleRevision => SplitFailure::StaleLayout,
                LayoutError::PaneLimitExceeded => SplitFailure::Capacity,
                other => SplitFailure::Layout(other),
            })?;
        let planned = preview
            .planned_snapshot()
            .panes
            .into_iter()
            .flatten()
            .find(|pane| pane.target == new_target)
            .ok_or(SplitFailure::Layout(LayoutError::InvalidLayoutState))?;
        let content = crate::pane_layout::content_rect(planned.rect)
            .ok_or(SplitFailure::LaunchSizeMismatch)?;
        if request.launch().dimensions() != (content.rows, content.columns) {
            return Err(SplitFailure::LaunchSizeMismatch);
        }
        Ok((preview, new_target))
    }

    fn finish_split_failed(
        &mut self,
        result: SplitResult,
        reason: SplitFailure,
    ) -> Result<SplitResult, SplitError> {
        if self
            .global
            .ledger
            .complete(result.key, Outcome::Failed)
            .is_err()
        {
            return Err(SplitError::InternalInvariant);
        }
        Err(SplitError::Failed {
            reason,
            result: Box::new(result),
        })
    }

    /// Apply one absolute size and record it as confirmed or pending.
    fn apply_pane_size<RE, Resize>(
        &mut self,
        target: TargetRef,
        rect: crate::pane_layout::Rect,
        resize: &mut Resize,
    ) -> bool
    where
        Resize: FnMut(&mut E, u16, u16) -> Result<(), RE>,
    {
        let Some(content) = crate::pane_layout::content_rect(rect) else {
            let confirmed = self
                .pane_state(target)
                .map_or((rect.rows, rect.columns), |state| state.confirmed);
            self.record_pane_size(target, confirmed, Some((rect.rows, rect.columns)));
            return false;
        };
        let desired = (content.rows, content.columns);
        if self
            .pane_state(target)
            .is_some_and(|state| state.confirmed == desired)
        {
            self.record_pane_size(target, desired, None);
            return true;
        }
        let applied = match self.launches.session_mut(target) {
            Ok(session) => resize(&mut session.effects, content.rows, content.columns).is_ok(),
            Err(_) => false,
        };
        if applied {
            self.record_pane_size(target, desired, None);
        } else {
            let Some(confirmed) = self.pane_state(target).map(|state| state.confirmed) else {
                return false;
            };
            self.record_pane_size(target, confirmed, Some(desired));
        }
        applied
    }

    fn pane_state(&self, target: TargetRef) -> Option<PaneSizeState> {
        self.pane_sizes
            .into_iter()
            .flatten()
            .find(|state| state.target == target)
    }

    fn is_resize_pending(&self, target: TargetRef) -> bool {
        self.pane_state(target)
            .is_some_and(|state| state.desired.is_some())
    }

    /// Update or insert one bounded pane-size entry keyed by full target.
    fn record_pane_size(
        &mut self,
        target: TargetRef,
        confirmed: (u16, u16),
        desired: Option<(u16, u16)>,
    ) {
        let entry = PaneSizeState {
            target,
            confirmed,
            desired,
        };
        if let Some(slot) = self
            .pane_sizes
            .iter_mut()
            .find(|slot| slot.is_some_and(|state| state.target == target))
        {
            *slot = Some(entry);
        } else if let Some(slot) = self.pane_sizes.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(entry);
        }
        // Every layout pane has one bounded entry, so a fifth distinct target
        // cannot reach this point.
    }

    /// Attach to an exact retained target and issue a fresh global lease or
    /// resume a lease already pinned to that full target identity.
    pub fn connect(
        &mut self,
        target: TargetRef,
        hello: &SessionHello,
        now_secs: u64,
    ) -> Result<(Connection, Welcome), ServerError> {
        hello.validate()?;
        self.record_workspace_layout();
        let workspace_capability =
            if self.workspace_events_failed || self.workspace_events.is_none() {
                0
            } else {
                CAPABILITY_WORKSPACE_EVENTS
            };
        let target_is_retired = self.launches.is_retired(target);
        let session = match self.launches.session_mut(target) {
            Ok(session) => session,
            Err(RegistryError::InvalidTarget | RegistryError::EffectsUnavailable)
                if target_is_retired
                    && !self.workspace_events_failed
                    && self.workspace_events.is_some()
                    && self.workspace_events.as_ref().is_some_and(|journal| {
                        journal.current_cursor().workspace == target.workspace
                    }) =>
            {
                return self
                    .global
                    .resume_workspace_observer(target, hello, now_secs);
            }
            Err(error) => return Err(map_connect_registry_error(error)),
        };
        self.global.connect_with_capability_mask(
            session,
            hello,
            now_secs,
            CAPABILITY_READ
                | CAPABILITY_INPUT
                | CAPABILITY_EVENTS
                | CAPABILITY_PANE_METADATA
                | CAPABILITY_SCREEN_ROW_WRAPS
                | CAPABILITY_HISTORY_CURSOR
                | crate::handshake_protocol::CAPABILITY_PANE_OPERATIONS
                | crate::handshake_protocol::CAPABILITY_AGENT_OBSERVATION
                | crate::handshake_protocol::CAPABILITY_BUILD_IDENTITY
                | if self.global.allow_input {
                    crate::handshake_protocol::CAPABILITY_SCOPED_MOUSE_WHEEL
                } else {
                    0
                }
                | workspace_capability,
        )
    }

    /// Return read-only server identity for a live connection that negotiated
    /// `CAPABILITY_BUILD_IDENTITY`. Runtime transports must not expose this
    /// helper before peer authentication and normal handshake completion.
    pub fn build_identity(
        &self,
        connection: &Connection,
        now_secs: u64,
    ) -> Result<crate::build_info::ServerIdentity, ServerError> {
        self.global.require_live_connection(connection, now_secs)?;
        let selected_capabilities = connection.selected_capabilities();
        if selected_capabilities & crate::handshake_protocol::CAPABILITY_BUILD_IDENTITY == 0 {
            return Err(ServerError::Unauthorized);
        }
        Ok(crate::build_info::ServerIdentity {
            build: crate::build_info::current(),
            generation: self.global.generation,
            selected_capabilities,
        })
    }

    /// Read one bounded screen and bind it to this target's metadata and journal.
    pub fn reconcile(
        &mut self,
        connection: &Connection,
        target: TargetRef,
        max_bytes: u32,
        max_lines: u16,
        now_secs: u64,
    ) -> Result<ScopedReconcileState, ServerError> {
        self.global.require_live_connection(connection, now_secs)?;
        if connection.selected_capabilities & (CAPABILITY_READ | CAPABILITY_EVENTS)
            != (CAPABILITY_READ | CAPABILITY_EVENTS)
        {
            return Err(ServerError::Unauthorized);
        }
        let generation = self.global.generation;
        let session = self
            .launches
            .session_mut(target)
            .map_err(map_watch_registry_error)?;
        let mut handler = SessionHandler::new(&mut self.global, session);
        handler.require_watch_target_scope(connection, target)?;
        let state = handler.reconcile_preflighted(max_bytes, max_lines)?;
        let scoped = ScopedReconcileState {
            info: state.info,
            cursor: SessionCursor {
                generation,
                session: target.session,
                sequence: state.cursor.sequence,
            },
            screen_revision: state.screen_revision,
            truncated: state.truncated,
            text: state.text,
        };
        let encoded = crate::scoped_watch_protocol::encode_reconciliation(
            &crate::scoped_watch_protocol::Reconciliation {
                info: scoped.info,
                cursor: scoped.cursor,
                screen_revision: scoped.screen_revision,
                truncated: scoped.truncated,
                text: &scoped.text,
            },
        )
        .map_err(|_| ServerError::InvalidSnapshot)?;
        if encoded.len() > max_bytes as usize {
            return Err(ServerError::InvalidSnapshot);
        }
        Ok(scoped)
    }

    /// Read one bounded event page from the requested target's journal.
    pub fn read_events(
        &mut self,
        connection: &Connection,
        target: TargetRef,
        cursor: SessionCursor,
        limit: u16,
        now_secs: u64,
    ) -> Result<ScopedEventPage, ServerError> {
        self.global.require_live_connection(connection, now_secs)?;
        if connection.selected_capabilities & (CAPABILITY_READ | CAPABILITY_EVENTS)
            != (CAPABILITY_READ | CAPABILITY_EVENTS)
        {
            return Err(ServerError::Unauthorized);
        }
        let generation = self.global.generation;
        let session = self
            .launches
            .session_mut(target)
            .map_err(map_watch_registry_error)?;
        let mut handler = SessionHandler::new(&mut self.global, session);
        handler.require_watch_target_scope(connection, target)?;
        if cursor.generation != generation {
            return Err(ServerError::WrongGeneration);
        }
        if cursor.session != target.session {
            return Err(ServerError::TargetStale);
        }
        if !(1..=super::MAX_WATCH_EVENTS).contains(&limit) {
            return Err(ServerError::InvalidWatchRequest);
        }
        let page = handler.read_events_preflighted(
            EventCursor {
                generation: cursor.generation,
                sequence: cursor.sequence,
            },
            limit,
        )?;
        Ok(scoped_event_page(target, page))
    }

    /// Dispatch one control request after global exact-key preflight.
    ///
    /// Rejected or known global results return before target lookup and do not
    /// synchronize session metadata. A new request selects only its exact full
    /// target; after selection, metadata synchronization occurs once even when
    /// target/scope checks reject the request.
    pub fn handle_control(
        &mut self,
        connection: &Connection,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<Vec<u8>, ServerError> {
        let request = decode_request_payload(payload)?;
        let decision = self
            .global
            .preflight_control(connection, &request, payload, now_secs)?;
        if let Some(reply) = preflight_reply(request.key, decision)? {
            return Ok(reply);
        }

        let session = match self.launches.session_mut(request.target) {
            Ok(session) => session,
            Err(RegistryError::InvalidTarget | RegistryError::EffectsUnavailable) => {
                return preflight_reply(
                    request.key,
                    RequestPreflight::Rejected(RejectionCode::StaleTarget),
                )?
                .ok_or(ServerError::Internal);
            }
            Err(_) => return Err(ServerError::Internal),
        };

        let mut handler = SessionHandler::new(&mut self.global, session);
        let result = handler.handle_control_preflighted(connection, request, payload, now_secs);
        handler.synchronize_metadata();
        result
    }

    /// Route one existing schema-2 session request by its pinned grant or,
    /// for mutations, by the request's exact full target identity.
    ///
    /// Global cached results and targetless requests never select a session.
    /// A newly selected session is synchronized once after its handler result,
    /// including denials after lookup. This trusted core neither authenticates
    /// peers nor stops children on Detach.
    pub fn handle_session(
        &mut self,
        connection: &Connection,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<SessionReply, ServerError> {
        let request = decode_session_request(payload)?;
        match request {
            SessionRequest::Describe => {
                self.global.require_live_connection(connection, now_secs)?;
                if connection.selected_capabilities & CAPABILITY_READ == 0 {
                    return Err(ServerError::Unauthorized);
                }
                let target = self
                    .global
                    .grants
                    .get(&connection.lease)
                    .map(|grant| grant.target)
                    .ok_or(ServerError::UnknownContext)?;
                let session = self
                    .launches
                    .session_mut(target)
                    .map_err(map_connect_registry_error)?;
                let mut handler = SessionHandler::new(&mut self.global, session);
                let result = handler.session_info().map(SessionReply::Info);
                handler.synchronize_metadata();
                result
            }
            SessionRequest::Claim {
                key,
                target,
                expected_owner_revision,
                ttl_seconds,
            } => self.route_owner_mutation(
                connection,
                key,
                target,
                OwnerMutation::Claim {
                    expected_revision: expected_owner_revision,
                    ttl_seconds,
                },
                payload,
                now_secs,
            ),
            SessionRequest::Release {
                key,
                target,
                expected_owner_revision,
            } => self.route_owner_mutation(
                connection,
                key,
                target,
                OwnerMutation::Release {
                    expected_revision: expected_owner_revision,
                },
                payload,
                now_secs,
            ),
            SessionRequest::Handoff {
                key,
                target,
                expected_owner_revision,
                recipient_lease,
                ttl_seconds,
            } => self.route_owner_mutation(
                connection,
                key,
                target,
                OwnerMutation::Handoff {
                    expected_revision: expected_owner_revision,
                    recipient_lease,
                    ttl_seconds,
                },
                payload,
                now_secs,
            ),
            SessionRequest::OperationStatus { key } => self
                .global
                .operation_status(connection, key, now_secs)
                .map(SessionReply::Payload),
            SessionRequest::Receipt {
                generation,
                lease,
                through_sequence,
            } => self
                .global
                .receipt(connection, generation, lease, through_sequence, now_secs)
                .map(SessionReply::Payload),
            SessionRequest::Detach => {
                self.global.require_live_connection(connection, now_secs)?;
                Ok(SessionReply::Detached)
            }
        }
    }

    fn route_owner_mutation(
        &mut self,
        connection: &Connection,
        key: OperationKey,
        target: TargetRef,
        mutation: OwnerMutation,
        payload: &[u8],
        now_secs: u64,
    ) -> Result<SessionReply, ServerError> {
        let decision = self
            .global
            .preflight_owner_mutation(connection, key, payload, now_secs)?;
        if let Some(reply) = preflight_reply(key, decision)? {
            return Ok(SessionReply::Payload(reply));
        }

        let session = match self.launches.session_mut(target) {
            Ok(session) => session,
            Err(RegistryError::InvalidTarget | RegistryError::EffectsUnavailable) => {
                let reply =
                    preflight_reply(key, RequestPreflight::Rejected(RejectionCode::StaleTarget))?
                        .ok_or(ServerError::Internal)?;
                return Ok(SessionReply::Payload(reply));
            }
            Err(_) => return Err(ServerError::Internal),
        };

        let mut handler = SessionHandler::new(&mut self.global, session);
        let result = handler.handle_owner_mutation_preflighted(
            connection, key, target, mutation, payload, now_secs,
        );
        handler.synchronize_metadata();
        result
    }

    /// Observe a status from the caller's own global lease without selecting a
    /// process target.
    pub fn operation_status(
        &self,
        connection: &Connection,
        key: OperationKey,
        now_secs: u64,
    ) -> Result<Vec<u8>, ServerError> {
        self.global.operation_status(connection, key, now_secs)
    }

    /// Retire completed results in the caller's own global lease, without
    /// selecting a process target.
    pub fn receipt(
        &mut self,
        connection: &Connection,
        generation: u64,
        lease: u64,
        through_sequence: u64,
        now_secs: u64,
    ) -> Result<Vec<u8>, ServerError> {
        self.global
            .receipt(connection, generation, lease, through_sequence, now_secs)
    }

    /// Expire global leases once and poll every retained session with effects.
    ///
    /// A session error does not prevent the other retained target from being
    /// visited. The first encountered session or registry error is returned.
    /// After polling, an exited pane with a live-or-present sibling is
    /// collapsed out of the layout and its slot retired; the survivor gets
    /// one absolute resize through [`EffectSink::apply_size`] (recorded as
    /// resize-pending on failure, retried by [`Self::resize_layout`]).
    pub fn poll(&mut self, now_secs: u64) -> Result<(), ServerError> {
        self.global.expire(now_secs);

        let records: Vec<_> = self.launches.records().take(SESSION_CAPACITY).collect();

        let mut first_error = None;
        for record in records {
            if !record.has_effects {
                continue;
            }

            let session = match self.launches.session_mut(record.target) {
                Ok(session) => session,
                Err(_) => {
                    first_error.get_or_insert(ServerError::Internal);
                    continue;
                }
            };
            let poll_result = SessionHandler::new(&mut self.global, session).poll_session(now_secs);
            let child_exited = session.effects.child_exited();
            if let Err(error) = poll_result {
                first_error.get_or_insert(error);
            }
            if child_exited
                && record.state != SessionState::Exited
                && self.launches.note_exited(record.target).is_err()
            {
                first_error.get_or_insert(ServerError::Internal);
            }
        }
        // A pane whose shell exited while its sibling lives is removed; the
        // sibling fills the root through its effects' absolute resize. The
        // last remaining pane keeps its tombstone so the workspace can close.
        if self.collapse_exited_panes().is_err() {
            first_error.get_or_insert(ServerError::Internal);
        }
        self.record_workspace_layout();
        first_error.map_or(Ok(()), Err)
    }
}

/// One layout snapshot with its per-pane resize-pending state.
pub type LayoutView = (
    LayoutSnapshot,
    [Option<PendingResize>; crate::pane_layout::MAX_PANES],
);

/// Result of an authorized layout-owned root resize request.
pub enum RootResizeReply {
    /// Encoded schema-1 rejection or known status for a duplicate key; no
    /// new effect ran.
    Response(Vec<u8>),
    /// The resize executed; `snapshot` is the committed layout and `pending`
    /// marks layout panes whose absolute PTY resize is still pending.
    Resized {
        /// Committed layout after this request.
        snapshot: Box<LayoutSnapshot>,
        /// Per-pane resize-pending flags, aligned with `snapshot.panes`.
        pending: [bool; crate::pane_layout::MAX_PANES],
    },
    /// The layout rejected the request; the key is recorded `Failed` and no
    /// PTY was resized.
    LayoutRejected(LayoutResizeError),
}

impl fmt::Debug for RootResizeReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Response(bytes) => f
                .debug_struct("Response")
                .field("bytes_len", &bytes.len())
                .finish(),
            Self::Resized { pending, .. } => f
                .debug_struct("Resized")
                .field("pending", pending)
                .finish_non_exhaustive(),
            Self::LayoutRejected(error) => f.debug_tuple("LayoutRejected").field(error).finish(),
        }
    }
}

impl<E: EffectSink> MultiSessionOwner<E> {
    /// Return the full target identity pinned to a live lease grant, if any.
    ///
    /// Runtime adapters use this only to select the attach target for a
    /// resume Hello; `connect` still validates the grant against the target.
    pub fn lease_target(&self, lease: u64) -> Option<TargetRef> {
        self.global.grants.get(&lease).map(|grant| grant.target)
    }

    /// Return the first layout pane, the default attach target for a fresh
    /// Hello that names no target.
    pub fn first_pane_target(&self) -> Option<TargetRef> {
        self.layout_snapshot()
            .and_then(|layout| layout.panes[0])
            .map(|pane| pane.target)
    }

    /// Observe the layout for a live Read-capable connection.
    ///
    /// The layout is shared geometry, not pane content, so it is visible to
    /// any connection regardless of its pinned target. `Ok(None)` means no
    /// layout has been established.
    pub fn layout_view(
        &self,
        connection: &Connection,
        now_secs: u64,
    ) -> Result<Option<LayoutView>, ServerError> {
        self.global.require_live_connection(connection, now_secs)?;
        if connection.selected_capabilities & CAPABILITY_READ == 0 {
            return Err(ServerError::Unauthorized);
        }
        Ok(self
            .layout_snapshot()
            .map(|layout| (layout, self.pending_resizes())))
    }

    /// Return the schema-13 pane-operation view without changing owner state.
    pub fn pane_operations_view(
        &self,
        connection: &Connection,
        now_secs: u64,
    ) -> Result<Option<PaneOperationsView>, ServerError> {
        self.global.require_live_connection(connection, now_secs)?;
        if connection.selected_capabilities
            & (CAPABILITY_READ | crate::handshake_protocol::CAPABILITY_PANE_OPERATIONS)
            != (CAPABILITY_READ | crate::handshake_protocol::CAPABILITY_PANE_OPERATIONS)
        {
            return Err(ServerError::Unauthorized);
        }
        self.pane_operations_view_unchecked()
    }

    fn pane_operations_view_unchecked(&self) -> Result<Option<PaneOperationsView>, ServerError> {
        let Some(snapshot) = self.layout_snapshot() else {
            return Ok(None);
        };
        let mut confirmed_sizes = [None; crate::pane_layout::MAX_PANES];
        let mut panes = [None; crate::pane_layout::MAX_PANES];
        for (index, pane) in snapshot.panes.into_iter().enumerate() {
            let Some(pane) = pane else {
                continue;
            };
            let content =
                crate::pane_layout::content_rect(pane.rect).ok_or(ServerError::InvalidSnapshot)?;
            let confirmed = self
                .pane_state(pane.target)
                .map_or((content.rows, content.columns), |state| state.confirmed);
            confirmed_sizes[index] = Some(confirmed);
            panes[index] = Some(crate::ui_protocol::UiPane {
                target: pane.target,
                rect: pane.rect,
                pending_confirmed: (confirmed != (content.rows, content.columns))
                    .then_some(confirmed),
            });
        }
        Ok(Some(PaneOperationsView {
            layout: crate::ui_protocol::LayoutReply {
                revision: snapshot.revision,
                rows: snapshot.rows,
                columns: snapshot.columns,
                panes,
                divider: snapshot.divider,
                dividers: snapshot.dividers,
            },
            zoomed_target: self.zoomed_target,
            confirmed_sizes,
        }))
    }

    /// Validate the typed request against its exact schema-13 payload, then
    /// admit and execute one serialized absolute pane mutation.
    pub fn handle_pane_operation<RE, Resize>(
        &mut self,
        connection: &Connection,
        request: PaneOperationRequest,
        payload: &[u8],
        now_secs: u64,
        mut resize: Resize,
    ) -> Result<Vec<u8>, ServerError>
    where
        Resize: FnMut(&mut E, u16, u16) -> Result<(), RE>,
    {
        let decoded = pane_operations_protocol::decode_request(payload)
            .map_err(|_| ServerError::InvalidSnapshot)?;
        if decoded != request {
            return Err(ServerError::InvalidSnapshot);
        }
        let (key, mutation) = match request {
            PaneOperationRequest::ViewQuery => return Err(ServerError::Unauthorized),
            PaneOperationRequest::Swap(value) => (value.key, PaneMutation::Swap(value)),
            PaneOperationRequest::DividerSet(value) => (value.key, PaneMutation::Divider(value)),
            PaneOperationRequest::ZoomSet(value) => (value.key, PaneMutation::Zoom(value)),
        };
        self.global.require_live_connection(connection, now_secs)?;
        if connection.selected_capabilities
            & (CAPABILITY_INPUT | crate::handshake_protocol::CAPABILITY_PANE_OPERATIONS)
            != (CAPABILITY_INPUT | crate::handshake_protocol::CAPABILITY_PANE_OPERATIONS)
            || key.lease != connection.lease
            || key.generation != self.global.generation
        {
            return rejected_reply(key, RejectionCode::Unauthorized);
        }
        if let Some(reply) = known_reply(&self.global, key, payload, now_secs)? {
            return Ok(reply);
        }
        if self.active_async_split.is_some() || self.async_emergency_effect.is_some() {
            return rejected_reply(key, RejectionCode::Capacity);
        }
        match self.global.ledger.admit(key, payload, now_secs) {
            Ok(Admission::Execute) => {}
            Ok(Admission::InProgress) => return status_reply(key, OperationStatus::InProgress),
            Ok(Admission::Completed(outcome)) => {
                return status_reply(key, OperationStatus::Completed(outcome));
            }
            Err(error) if is_capacity_error(&error) => {
                return rejected_reply(key, RejectionCode::Capacity);
            }
            Err(error) => return Err(ServerError::Protocol(error)),
        }

        let operation = self.execute_pane_mutation(mutation, &mut resize);
        let (outcome, response) = match operation {
            Ok(outcome) => (
                outcome,
                status_reply(key, OperationStatus::Completed(outcome))?,
            ),
            Err(code) => {
                self.global.ledger.complete(key, Outcome::Failed)?;
                return rejected_reply(key, code);
            }
        };
        self.global.ledger.complete(key, outcome)?;
        Ok(response)
    }

    fn execute_pane_mutation<RE, Resize>(
        &mut self,
        mutation: PaneMutation,
        resize: &mut Resize,
    ) -> Result<Outcome, RejectionCode>
    where
        Resize: FnMut(&mut E, u16, u16) -> Result<(), RE>,
    {
        let snapshot = self
            .layout_snapshot()
            .ok_or(RejectionCode::UnknownContext)?;
        let mutation_revision = mutation.revision();
        if mutation_revision != snapshot.revision {
            return Err(RejectionCode::StaleOwner);
        }
        let workspace = snapshot.panes[0]
            .ok_or(RejectionCode::UnknownContext)?
            .target
            .workspace;
        if snapshot.panes.iter().flatten().any(|pane| {
            pane.target.generation != self.global.generation || pane.target.workspace != workspace
        }) {
            return Err(RejectionCode::Unauthorized);
        }
        let targets = mutation.targets();
        if targets.iter().flatten().any(|target| {
            target.generation != self.global.generation
                || target.workspace != workspace
                || !snapshot
                    .panes
                    .iter()
                    .flatten()
                    .any(|pane| pane.target == *target)
                || self.launches.effects(*target).is_err()
        }) {
            return Err(RejectionCode::StaleTarget);
        }
        if !matches!(mutation, PaneMutation::Zoom(_)) && self.zoomed_target.is_some() {
            return Err(RejectionCode::InvalidRequest);
        }

        match mutation {
            PaneMutation::Swap(request) => {
                let layout = self.layout.as_mut().ok_or(RejectionCode::UnknownContext)?;
                let after = layout
                    .swap(
                        request.source,
                        request.destination,
                        request.expected_layout_revision,
                    )
                    .map_err(layout_rejection)?;
                self.record_workspace_layout();
                let mut failed = false;
                for pane in after.panes.iter().flatten().copied() {
                    let old_rect = snapshot
                        .panes
                        .iter()
                        .flatten()
                        .find(|old| old.target == pane.target)
                        .map(|old| old.rect);
                    if old_rect != Some(pane.rect) {
                        failed |= !self.apply_pane_size(pane.target, pane.rect, resize);
                    }
                }
                Ok(if failed {
                    Outcome::Partial
                } else {
                    Outcome::Applied
                })
            }
            PaneMutation::Divider(request) => {
                let layout = self.layout.as_mut().ok_or(RejectionCode::UnknownContext)?;
                let after = layout
                    .set_divider(
                        request.target,
                        request.expected_layout_revision,
                        request.divider_index,
                        request.first_cells,
                    )
                    .map_err(layout_rejection)?;
                if after.revision != snapshot.revision {
                    self.record_workspace_layout();
                }
                let mut failed = false;
                for pane in after.panes.iter().flatten().copied() {
                    let old_rect = snapshot
                        .panes
                        .iter()
                        .flatten()
                        .find(|old| old.target == pane.target)
                        .map(|old| old.rect);
                    if old_rect != Some(pane.rect)
                        || (after.revision == snapshot.revision
                            && self.is_resize_pending(pane.target))
                    {
                        failed |= !self.apply_pane_size(pane.target, pane.rect, resize);
                    }
                }
                Ok(if failed {
                    Outcome::Partial
                } else {
                    Outcome::Applied
                })
            }
            PaneMutation::Zoom(request) => self.apply_zoom(request, resize),
        }
    }

    fn apply_zoom<RE, Resize>(
        &mut self,
        request: PaneZoomSet,
        resize: &mut Resize,
    ) -> Result<Outcome, RejectionCode>
    where
        Resize: FnMut(&mut E, u16, u16) -> Result<(), RE>,
    {
        let snapshot = self
            .layout_snapshot()
            .ok_or(RejectionCode::UnknownContext)?;
        let target_is_current = snapshot
            .panes
            .iter()
            .flatten()
            .any(|pane| pane.target == request.target);
        let next_zoom = if request.zoomed {
            if !target_is_current
                || self
                    .zoomed_target
                    .is_some_and(|current| current != request.target)
            {
                return Err(RejectionCode::StaleTarget);
            }
            Some(request.target)
        } else {
            match self.zoomed_target {
                Some(current) if current == request.target => None,
                Some(_) => return Err(RejectionCode::StaleTarget),
                None if target_is_current => None,
                None => return Err(RejectionCode::StaleTarget),
            }
        };
        let changed = next_zoom != self.zoomed_target;
        if changed {
            self.layout
                .as_mut()
                .ok_or(RejectionCode::UnknownContext)?
                .advance_revision(request.expected_layout_revision)
                .map_err(layout_rejection)?;
            self.zoomed_target = next_zoom;
            self.record_workspace_layout();
        }

        let effective_root = full_root_rect(snapshot.rows, snapshot.columns);
        let mut failed = false;
        if let Some(target) = next_zoom {
            let rect = crate::pane_layout::content_rect(effective_root)
                .ok_or(RejectionCode::InvalidRequest)?;
            let desired = (rect.rows, rect.columns);
            if changed
                || self
                    .pane_state(target)
                    .is_some_and(|state| state.confirmed != desired)
            {
                failed |= !self.apply_effective_size(target, desired, snapshot, resize);
            }
        } else {
            for pane in snapshot.panes.iter().flatten().copied() {
                let Some(rect) = crate::pane_layout::content_rect(pane.rect) else {
                    failed = true;
                    continue;
                };
                let desired = (rect.rows, rect.columns);
                if self.pane_state(pane.target).is_none_or(|state| {
                    state.confirmed != desired || state.desired == Some(desired)
                }) {
                    failed |= !self.apply_effective_size(pane.target, desired, snapshot, resize);
                }
            }
        }
        Ok(if failed {
            Outcome::Partial
        } else {
            Outcome::Applied
        })
    }

    fn apply_effective_size<RE, Resize>(
        &mut self,
        target: TargetRef,
        desired: (u16, u16),
        _underlying: LayoutSnapshot,
        resize: &mut Resize,
    ) -> bool
    where
        Resize: FnMut(&mut E, u16, u16) -> Result<(), RE>,
    {
        if self
            .pane_state(target)
            .is_some_and(|state| state.confirmed == desired)
        {
            self.record_pane_size(target, desired, None);
            return true;
        }
        let applied = self
            .launches
            .session_mut(target)
            .is_ok_and(|session| resize(&mut session.effects, desired.0, desired.1).is_ok());
        let confirmed = if applied {
            desired
        } else if let Some(state) = self.pane_state(target) {
            state.confirmed
        } else {
            return false;
        };
        self.record_pane_size(target, confirmed, (!applied).then_some(desired));
        applied
    }

    /// Run one bounded read-only observation of the exact target pinned to a
    /// live Read-capable connection.
    ///
    /// Scope is checked before target lookup: another target is
    /// `Unauthorized`; a pinned target without effects is `TargetStale`.
    /// Metadata is synchronized once before `observe` runs.
    pub fn observe_target<R>(
        &mut self,
        connection: &Connection,
        target: TargetRef,
        now_secs: u64,
        observe: impl FnOnce(&E) -> R,
    ) -> Result<R, ServerError> {
        self.global.require_live_connection(connection, now_secs)?;
        if connection.selected_capabilities & CAPABILITY_READ == 0 {
            return Err(ServerError::Unauthorized);
        }
        self.global.require_target_scope(connection, target)?;
        let session = self
            .launches
            .session_mut(target)
            .map_err(map_watch_registry_error)?;
        let mut handler = SessionHandler::new(&mut self.global, session);
        handler.synchronize_metadata();
        Ok(observe(&handler.session.effects))
    }

    /// Deliver a process- and screen-pinned agent submission after exact-key
    /// duplicate lookup and existing input-owner checks.
    pub fn handle_guarded_agent_submit<V>(
        &mut self,
        connection: &Connection,
        request: &crate::agent_protocol::GuardedSubmit<'_>,
        payload: &[u8],
        now_secs: u64,
        validate_agent: V,
    ) -> Result<Vec<u8>, ServerError>
    where
        V: FnOnce(&E) -> Result<(), RejectionCode>,
    {
        let key = request.key;
        self.global.require_live_connection(connection, now_secs)?;
        let required = CAPABILITY_INPUT | crate::handshake_protocol::CAPABILITY_AGENT_OBSERVATION;
        if key.lease != connection.lease
            || key.generation != self.global.generation
            || connection.selected_capabilities & required != required
        {
            return rejected_reply(key, RejectionCode::Unauthorized);
        }
        // Exact original request equality comes before mutable target/process
        // observations. A duplicate never re-evaluates or resends its input.
        if let Some(reply) = known_reply(&self.global, key, payload, now_secs)? {
            return Ok(reply);
        }
        if self
            .global
            .require_target_scope(connection, request.target)
            .is_err()
        {
            return rejected_reply(key, RejectionCode::Unauthorized);
        }
        let session = match self.launches.session_mut(request.target) {
            Ok(session) => session,
            Err(RegistryError::InvalidTarget | RegistryError::EffectsUnavailable) => {
                return rejected_reply(key, RejectionCode::StaleTarget);
            }
            Err(_) => return Err(ServerError::Internal),
        };
        let mut handler = SessionHandler::new(&mut self.global, session);
        let result = (|| {
            let mut candidate = handler.session.input_owner.clone();
            if let Err(error) = candidate.begin_input(
                Owner::Client(key.lease),
                request.owner_revision,
                key,
                now_secs,
            ) {
                return rejected_reply(key, owner_rejection(error));
            }
            let command = Command::Submit {
                owner_revision: request.owner_revision,
                text: request.text,
            };
            if let Err(code) = handler
                .session
                .effects
                .validate(command)
                .and_then(|()| validate_agent(&handler.session.effects))
            {
                return rejected_reply(key, code);
            }
            match handler.global.ledger.admit(key, payload, now_secs) {
                Ok(Admission::Execute) => {}
                Ok(Admission::InProgress) => return status_reply(key, OperationStatus::InProgress),
                Ok(Admission::Completed(outcome)) => {
                    return status_reply(key, OperationStatus::Completed(outcome))
                }
                Err(error) if is_capacity_error(&error) => {
                    return rejected_reply(key, RejectionCode::Capacity)
                }
                Err(error) => return Err(ServerError::Protocol(error)),
            }
            handler.session.input_owner = candidate;
            let outcome = terminal_outcome(handler.session.effects.input(command));
            handler.global.ledger.complete(key, outcome)?;
            handler.session.input_owner.finish_input(key)?;
            status_reply(key, OperationStatus::Completed(outcome))
        })();
        handler.synchronize_metadata();
        result
    }

    /// Admit and synchronously deliver one non-replayable typed-key input.
    ///
    /// Ordering matches a schema-1 Text request: live connection, own lease
    /// and Input capability, then global exact-key lookup (duplicates return
    /// their status and never re-deliver), then stale-target and pinned-scope
    /// checks, then the input-owner pin for `expected_owner_revision`, then
    /// `validate`, admission, `apply`, and completion. Returns an encoded
    /// schema-1 status or rejection.
    pub fn handle_key_input<V, A>(
        &mut self,
        connection: &Connection,
        request: &crate::ui_protocol::KeyInputView<'_>,
        payload: &[u8],
        now_secs: u64,
        validate: V,
        apply: A,
    ) -> Result<Vec<u8>, ServerError>
    where
        V: FnOnce(&E) -> Result<(), RejectionCode>,
        A: FnOnce(&mut E) -> Outcome,
    {
        let key = request.key();
        self.global.require_live_connection(connection, now_secs)?;
        if key.lease != connection.lease
            || key.generation != self.global.generation
            || connection.selected_capabilities & CAPABILITY_INPUT == 0
        {
            return rejected_reply(key, RejectionCode::Unauthorized);
        }
        if let Some(reply) = known_reply(&self.global, key, payload, now_secs)? {
            return Ok(reply);
        }
        let target = request.target();
        if self
            .global
            .require_target_scope(connection, target)
            .is_err()
        {
            return rejected_reply(key, RejectionCode::Unauthorized);
        }
        let session = match self.launches.session_mut(target) {
            Ok(session) => session,
            Err(RegistryError::InvalidTarget | RegistryError::EffectsUnavailable) => {
                return rejected_reply(key, RejectionCode::StaleTarget);
            }
            Err(_) => return Err(ServerError::Internal),
        };
        let mut handler = SessionHandler::new(&mut self.global, session);
        let result = (|| {
            let mut candidate = handler.session.input_owner.clone();
            if let Err(error) = candidate.begin_input(
                Owner::Client(key.lease),
                request.expected_owner_revision(),
                key,
                now_secs,
            ) {
                return rejected_reply(key, owner_rejection(error));
            }
            if let Err(code) = validate(&handler.session.effects) {
                return rejected_reply(key, code);
            }
            match handler.global.ledger.admit(key, payload, now_secs) {
                Ok(Admission::Execute) => {}
                Ok(Admission::InProgress) => return status_reply(key, OperationStatus::InProgress),
                Ok(Admission::Completed(outcome)) => {
                    return status_reply(key, OperationStatus::Completed(outcome));
                }
                Err(error) if is_capacity_error(&error) => {
                    return rejected_reply(key, RejectionCode::Capacity);
                }
                Err(error) => return Err(ServerError::Protocol(error)),
            }
            // Commit the pin only after this call reserved the unique key.
            handler.session.input_owner = candidate;
            let outcome = terminal_outcome(apply(&mut handler.session.effects));
            handler.global.ledger.complete(key, outcome)?;
            handler.session.input_owner.finish_input(key)?;
            status_reply(key, OperationStatus::Completed(outcome))
        })();
        handler.synchronize_metadata();
        result
    }

    /// Admit and synchronously deliver one non-replayable mouse-wheel event.
    ///
    /// A disabled mouse mode is rejected by `validate` before ledger
    /// admission, allowing the attach client to fall back to a read-only
    /// history query. Admitted events are never retried or replayed.
    pub fn handle_scoped_mouse_wheel<V, A>(
        &mut self,
        connection: &Connection,
        request: &crate::ui_protocol::ScopedMouseWheelInput,
        payload: &[u8],
        now_secs: u64,
        validate: V,
        apply: A,
    ) -> Result<Vec<u8>, ServerError>
    where
        V: FnOnce(&E) -> Result<(), RejectionCode>,
        A: FnOnce(&mut E) -> Outcome,
    {
        let key = request.key;
        self.global.require_live_connection(connection, now_secs)?;
        let required = CAPABILITY_INPUT | crate::handshake_protocol::CAPABILITY_SCOPED_MOUSE_WHEEL;
        if key.lease != connection.lease
            || key.generation != self.global.generation
            || connection.selected_capabilities & required != required
        {
            return rejected_reply(key, RejectionCode::Unauthorized);
        }
        if let Some(reply) = known_reply(&self.global, key, payload, now_secs)? {
            return Ok(reply);
        }
        if self
            .global
            .require_target_scope(connection, request.target)
            .is_err()
        {
            return rejected_reply(key, RejectionCode::Unauthorized);
        }
        let session = match self.launches.session_mut(request.target) {
            Ok(session) => session,
            Err(RegistryError::InvalidTarget | RegistryError::EffectsUnavailable) => {
                return rejected_reply(key, RejectionCode::StaleTarget)
            }
            Err(_) => return Err(ServerError::Internal),
        };
        let mut handler = SessionHandler::new(&mut self.global, session);
        let result = (|| {
            // Validate mode before touching ownership, including when another
            // caller holds a no-mouse pane.
            if let Err(code) = validate(&handler.session.effects) {
                return rejected_reply(key, code);
            }
            let current = handler.session.input_owner.snapshot();
            if current.holder.is_some() || current.pending.is_some() {
                return rejected_reply(key, RejectionCode::InputBusy);
            }
            let mut pinned = handler.session.input_owner.clone();
            let claimed = match pinned.claim(
                Owner::Client(key.lease),
                current.revision,
                crate::input_owner::MAX_OWNER_TTL_SECS,
                now_secs,
            ) {
                Ok(value) => value,
                Err(error) => return rejected_reply(key, owner_rejection(error)),
            };
            if let Err(error) =
                pinned.begin_input(Owner::Client(key.lease), claimed.revision, key, now_secs)
            {
                return rejected_reply(key, owner_rejection(error));
            }
            let mut completed = pinned.clone();
            if let Err(error) = completed.finish_input(key) {
                return rejected_reply(key, owner_rejection(error));
            }
            let retained = completed.clone();
            let mut released = completed;
            if let Err(error) = released.release(Owner::Client(key.lease), claimed.revision) {
                return rejected_reply(key, owner_rejection(error));
            }
            let applied = status_reply(key, OperationStatus::Completed(Outcome::Applied))?;
            let failed = status_reply(key, OperationStatus::Completed(Outcome::Failed))?;
            let partial = status_reply(key, OperationStatus::Completed(Outcome::Partial))?;
            let unknown = status_reply(key, OperationStatus::Completed(Outcome::Unknown))?;
            match handler.global.ledger.admit(key, payload, now_secs) {
                Ok(Admission::Execute) => {}
                Ok(Admission::InProgress) => return status_reply(key, OperationStatus::InProgress),
                Ok(Admission::Completed(outcome)) => {
                    return status_reply(key, OperationStatus::Completed(outcome))
                }
                Err(error) if is_capacity_error(&error) => {
                    return rejected_reply(key, RejectionCode::Capacity)
                }
                Err(error) => return Err(ServerError::Protocol(error)),
            }
            handler.session.input_owner = pinned;
            let outcome = terminal_outcome(apply(&mut handler.session.effects));
            handler.global.ledger.complete(key, outcome)?;
            handler.session.input_owner = if outcome == Outcome::Applied {
                released
            } else {
                retained
            };
            Ok(match outcome {
                Outcome::Applied => applied,
                Outcome::Failed => failed,
                Outcome::Partial => partial,
                Outcome::Unknown | Outcome::Accepted => unknown,
            })
        })();
        handler.synchronize_metadata();
        result
    }

    pub fn handle_mouse_wheel<V, A>(
        &mut self,
        connection: &Connection,
        request: &crate::ui_protocol::MouseWheelInput,
        payload: &[u8],
        now_secs: u64,
        validate: V,
        apply: A,
    ) -> Result<Vec<u8>, ServerError>
    where
        V: FnOnce(&E) -> Result<(), RejectionCode>,
        A: FnOnce(&mut E) -> Outcome,
    {
        let key = request.key;
        self.global.require_live_connection(connection, now_secs)?;
        if key.lease != connection.lease
            || key.generation != self.global.generation
            || connection.selected_capabilities & CAPABILITY_INPUT == 0
        {
            return rejected_reply(key, RejectionCode::Unauthorized);
        }
        if let Some(reply) = known_reply(&self.global, key, payload, now_secs)? {
            return Ok(reply);
        }
        if self
            .global
            .require_target_scope(connection, request.target)
            .is_err()
        {
            return rejected_reply(key, RejectionCode::Unauthorized);
        }
        let session = match self.launches.session_mut(request.target) {
            Ok(session) => session,
            Err(RegistryError::InvalidTarget | RegistryError::EffectsUnavailable) => {
                return rejected_reply(key, RejectionCode::StaleTarget);
            }
            Err(_) => return Err(ServerError::Internal),
        };
        let mut handler = SessionHandler::new(&mut self.global, session);
        let result = (|| {
            let mut candidate = handler.session.input_owner.clone();
            if let Err(error) = candidate.begin_input(
                Owner::Client(key.lease),
                request.expected_owner_revision,
                key,
                now_secs,
            ) {
                return rejected_reply(key, owner_rejection(error));
            }
            if let Err(code) = validate(&handler.session.effects) {
                return rejected_reply(key, code);
            }
            match handler.global.ledger.admit(key, payload, now_secs) {
                Ok(Admission::Execute) => {}
                Ok(Admission::InProgress) => return status_reply(key, OperationStatus::InProgress),
                Ok(Admission::Completed(outcome)) => {
                    return status_reply(key, OperationStatus::Completed(outcome));
                }
                Err(error) if is_capacity_error(&error) => {
                    return rejected_reply(key, RejectionCode::Capacity);
                }
                Err(error) => return Err(ServerError::Protocol(error)),
            }
            handler.session.input_owner = candidate;
            let outcome = terminal_outcome(apply(&mut handler.session.effects));
            handler.global.ledger.complete(key, outcome)?;
            handler.session.input_owner.finish_input(key)?;
            status_reply(key, OperationStatus::Completed(outcome))
        })();
        handler.synchronize_metadata();
        result
    }

    /// Admit and execute one absolute layout-root resize.
    ///
    /// Requires a live connection with the Input capability and its own
    /// lease; input ownership is not required and not changed. A duplicate
    /// key returns its recorded status without a second resize. Same-size
    /// requests retry only resize-pending panes, as [`Self::resize_layout`].
    /// The ledger records `Applied`, `Partial` (some pane pending) or
    /// `Failed` (layout rejection, no PTY effect).
    pub fn handle_root_resize<RE, Resize>(
        &mut self,
        connection: &Connection,
        request: crate::ui_protocol::RootResize,
        payload: &[u8],
        now_secs: u64,
        resize: Resize,
    ) -> Result<RootResizeReply, ServerError>
    where
        Resize: FnMut(&mut E, u16, u16) -> Result<(), RE>,
    {
        let key = request.key;
        self.global.require_live_connection(connection, now_secs)?;
        if key.lease != connection.lease
            || key.generation != self.global.generation
            || connection.selected_capabilities & CAPABILITY_INPUT == 0
        {
            return rejected_reply(key, RejectionCode::Unauthorized).map(RootResizeReply::Response);
        }
        if let Some(reply) = known_reply(&self.global, key, payload, now_secs)? {
            return Ok(RootResizeReply::Response(reply));
        }
        if self.active_async_split.is_some() || self.async_emergency_effect.is_some() {
            return rejected_reply(key, RejectionCode::Capacity).map(RootResizeReply::Response);
        }
        match self.global.ledger.admit(key, payload, now_secs) {
            Ok(Admission::Execute) => {}
            Ok(Admission::InProgress) => {
                return status_reply(key, OperationStatus::InProgress)
                    .map(RootResizeReply::Response);
            }
            Ok(Admission::Completed(outcome)) => {
                return status_reply(key, OperationStatus::Completed(outcome))
                    .map(RootResizeReply::Response);
            }
            Err(error) if is_capacity_error(&error) => {
                return rejected_reply(key, RejectionCode::Capacity).map(RootResizeReply::Response);
            }
            Err(error) => return Err(ServerError::Protocol(error)),
        }
        let result = self.resize_layout(
            request.expected_layout_revision,
            request.rows,
            request.columns,
            resize,
        );
        let (outcome, reply) = match result {
            Ok(snapshot) => (
                Outcome::Applied,
                RootResizeReply::Resized {
                    pending: self.pending_flags(&snapshot),
                    snapshot: Box::new(snapshot),
                },
            ),
            Err(LayoutResizeError::ResizePending { snapshot, .. }) => (
                Outcome::Partial,
                RootResizeReply::Resized {
                    pending: self.pending_flags(&snapshot),
                    snapshot,
                },
            ),
            Err(error) => (Outcome::Failed, RootResizeReply::LayoutRejected(error)),
        };
        self.global.ledger.complete(key, outcome)?;
        Ok(reply)
    }

    /// Authorize one schema-7 split from a live connection before
    /// [`Self::execute_split`].
    ///
    /// The prototype rule: the connection must hold the Input capability
    /// (server started with input enabled), use its own lease in the split
    /// key, and be pinned to the exact split target. A split never claims or
    /// requires input ownership.
    pub fn authorize_split(
        &self,
        connection: &Connection,
        request: &crate::split_protocol::SplitRequestView<'_>,
        now_secs: u64,
    ) -> Result<(), ServerError> {
        self.global.require_live_connection(connection, now_secs)?;
        if connection.selected_capabilities & CAPABILITY_INPUT == 0
            || request.key().lease != connection.lease
        {
            return Err(ServerError::Unauthorized);
        }
        self.global
            .require_target_scope(connection, request.target())
    }

    /// Observe a retained launch result for one of the caller's own keys.
    pub fn launch_status(
        &self,
        connection: &Connection,
        key: OperationKey,
        now_secs: u64,
    ) -> Result<LaunchResult, ServerError> {
        self.global.require_live_connection(connection, now_secs)?;
        if connection.selected_capabilities & CAPABILITY_READ == 0
            || key.lease != connection.lease
            || key.generation != self.global.generation
        {
            return Err(ServerError::Unauthorized);
        }
        self.launches
            .status(key, now_secs, &self.global.ledger)
            .map_err(|error| match error {
                ManagedLaunchError::Ledger(error) => ServerError::Protocol(error),
                ManagedLaunchError::UnknownKey => ServerError::UnknownContext,
                _ => ServerError::Internal,
            })
    }

    fn pending_flags(&self, snapshot: &LayoutSnapshot) -> [bool; crate::pane_layout::MAX_PANES] {
        let mut flags = [false; crate::pane_layout::MAX_PANES];
        for (flag, pane) in flags.iter_mut().zip(snapshot.panes) {
            *flag = pane.is_some_and(|pane| self.is_resize_pending(pane.target));
        }
        flags
    }
}

/// Maximum close results one owner retains for exact duplicates and status.
pub const MAX_RETAINED_CLOSE_RESULTS: usize = 4_096;

/// Fixed, redacted failures of the trusted close transaction.
///
/// Admitted rejections (stale target/layout, no layout, already closed) are
/// not errors: they are returned as `Ok` [`CloseResult`]s with a
/// [`CloseOutcome::Rejected`] outcome and recorded `Failed` in the ledger.
pub enum CloseError {
    /// The schema-9 payload is malformed; nothing was admitted.
    InvalidPayload(CloseCodecError),
    /// The key generation is not this owner's generation.
    GenerationMismatch,
    /// Ledger validation, conflict, or capacity rejected the key before effects.
    Ledger(ProtocolError),
    /// The key is admitted but has no terminal retained result yet.
    InProgress,
    /// The bounded close-result store is full; nothing was admitted.
    Capacity,
    /// An internal invariant failed; the ledger records `Unknown` when admitted.
    InternalInvariant,
}

impl CloseError {
    /// Return the protocol classification without formatting payload data.
    pub fn ledger_error(&self) -> Option<&ProtocolError> {
        match self {
            Self::Ledger(error) => Some(error),
            _ => None,
        }
    }
}

impl fmt::Debug for CloseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPayload(error) => f.debug_tuple("InvalidPayload").field(error).finish(),
            Self::GenerationMismatch => f.write_str("GenerationMismatch"),
            Self::Ledger(_) => f.write_str("Ledger(<redacted>)"),
            Self::InProgress => f.write_str("InProgress"),
            Self::Capacity => f.write_str("Capacity"),
            Self::InternalInvariant => f.write_str("InternalInvariant"),
        }
    }
}

impl fmt::Display for CloseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidPayload(_) => "pane close payload is invalid",
            Self::GenerationMismatch => "pane close generation is invalid",
            Self::Ledger(ProtocolError::OperationConflict) => {
                "operation key was reused with a different pane close"
            }
            Self::Ledger(_) => "pane close ledger validation failed",
            Self::InProgress => "pane close result is not yet available",
            Self::Capacity => "pane close result capacity is exhausted",
            Self::InternalInvariant => "pane close invariant failed",
        })
    }
}

impl std::error::Error for CloseError {}

/// Read-only status of one current layout pane.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PaneStatus {
    /// Complete pane identity and its current layout rectangle.
    pub geometry: PaneGeometry,
    /// Lifecycle state of the pane's session record.
    pub state: SessionState,
    /// Whether the pane's child is still running as far as the owner knows.
    pub live: bool,
    /// Whether the pane's absolute PTY resize is still pending.
    pub resize_pending: bool,
    /// Confirmed child exit code or terminating signal, when available.
    pub exit: Option<PaneExit>,
}

impl fmt::Debug for PaneStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaneStatus")
            .field("geometry", &self.geometry)
            .field("state", &self.state)
            .field("live", &self.live)
            .field("resize_pending", &self.resize_pending)
            .field("exit", &self.exit)
            .finish()
    }
}

impl MultiSessionOwner<PtyEffects> {
    /// Return pane status including a confirmed PTY exit code or signal.
    pub fn pane_statuses_with_exit(&self) -> [Option<PaneStatus>; crate::pane_layout::MAX_PANES] {
        self.pane_statuses().map(|status| {
            status.map(|mut status| {
                status.exit = self
                    .launches
                    .effects(status.geometry.target)
                    .ok()
                    .and_then(|session| session.effects.exit_status());
                status
            })
        })
    }

    /// Observe pane lifecycle and confirmed exit status for a live Read-capable connection.
    pub fn pane_view_with_exit(
        &self,
        connection: &Connection,
        now_secs: u64,
    ) -> Result<[Option<PaneStatus>; crate::pane_layout::MAX_PANES], ServerError> {
        self.global.require_live_connection(connection, now_secs)?;
        if connection.selected_capabilities & CAPABILITY_READ == 0 {
            return Err(ServerError::Unauthorized);
        }
        Ok(self.pane_statuses_with_exit())
    }
}

impl<E: EffectSink> MultiSessionOwner<E> {
    /// Authorize one schema-9 close from a live connection before
    /// [`Self::execute_close`].
    ///
    /// User decision (2026-10-19): any authenticated same-UID client may
    /// explicitly stop an individual pane session; there is no separate stop
    /// grant, no Input capability requirement and no target pin. The key must
    /// still use the connection's own lease and this generation. This grants
    /// no launch authority and claims no input ownership.
    pub fn authorize_close(
        &self,
        connection: &Connection,
        request: &CloseRequest,
        now_secs: u64,
    ) -> Result<(), ServerError> {
        self.global.require_live_connection(connection, now_secs)?;
        if request.key.lease != connection.lease || request.key.generation != self.global.generation
        {
            return Err(ServerError::Unauthorized);
        }
        Ok(())
    }

    /// Execute or observe one trusted schema-9 pane close.
    ///
    /// The caller must authorize first ([`Self::authorize_close`]). Ordering:
    /// decode; exact-payload duplicate lookup (returning the retained result
    /// before any layout or target check; changed bytes conflict); bounded
    /// result capacity; admission of the exact bytes; then target/revision
    /// validation. A rejection there records `Failed` and has no effect.
    /// Otherwise the pane's child receives exactly one bounded stop through
    /// [`EffectSink::stop`] (HUP, wait, KILL, wait). On `Exited`/`Escalated`
    /// the session is recorded `Exited`; when a sibling pane exists the layout
    /// collapses (the sibling fills the root and receives one absolute resize
    /// through `resize`) and the closed session's slot is retired, so a later
    /// split receives a fresh, never-reused identity. Closing the only pane
    /// keeps its tombstone so [`Self::all_sessions_ended`] becomes true. On
    /// `Unknown` the layout and record are unchanged; a later observed exit
    /// collapses the pane through [`Self::poll`]. No input is sent or
    /// replayed and no process is respawned.
    pub fn execute_close<RE, Resize>(
        &mut self,
        payload: &[u8],
        now_secs: u64,
        mut resize: Resize,
    ) -> Result<CloseResult, CloseError>
    where
        Resize: FnMut(&mut E, u16, u16) -> Result<(), RE>,
    {
        let request = decode_close_request(payload).map_err(CloseError::InvalidPayload)?;
        let key = request.key;
        if key.generation != self.global.generation {
            return Err(CloseError::GenerationMismatch);
        }
        if let Some(admission) = self
            .global
            .ledger
            .existing_admission(key, payload, now_secs)
            .map_err(CloseError::Ledger)?
        {
            return match self.retained_close(key) {
                Some(result) => Ok(result),
                None if matches!(admission, Admission::InProgress) => Err(CloseError::InProgress),
                None => Err(CloseError::InternalInvariant),
            };
        }
        if self.active_async_split.is_some() || self.async_emergency_effect.is_some() {
            return Err(CloseError::InProgress);
        }
        if self.close_results.len() + self.active_closes.len() >= MAX_RETAINED_CLOSE_RESULTS {
            return Err(CloseError::Capacity);
        }
        match self
            .global
            .ledger
            .admit(key, payload, now_secs)
            .map_err(CloseError::Ledger)?
        {
            Admission::Execute => {}
            Admission::InProgress | Admission::Completed(_) => {
                return Err(CloseError::InternalInvariant);
            }
        }

        let target = request.target;
        let already_closing = self
            .active_closes
            .iter()
            .any(|close| close.request.target == target);
        let plan = if already_closing {
            Err(CloseRejection::AlreadyClosed)
        } else {
            self.plan_close(&request)
        };
        let result = match plan {
            Err(rejection) => CloseResult {
                key,
                outcome: CloseOutcome::Rejected(rejection),
                target: Some(target),
                layout_revision: self.layout_revision(),
                survivor_resize_pending: false,
            },
            Ok(()) => {
                let stop = match self.launches.session_mut(target) {
                    Ok(session) => session.effects.stop(),
                    Err(_) => {
                        let _ = self.global.ledger.complete(key, Outcome::Unknown);
                        return Err(CloseError::InternalInvariant);
                    }
                };
                let mut pending = false;
                let stop = match stop {
                    StopOutcome::Unknown => StopOutcome::Unknown,
                    exited => match self.finish_pane_exit(target, &mut resize) {
                        Ok(survivor_pending) => {
                            pending = survivor_pending;
                            exited
                        }
                        // The child reported exit but the owner could not
                        // record it; never claim the close completed.
                        Err(()) => StopOutcome::Unknown,
                    },
                };
                CloseResult {
                    key,
                    outcome: CloseOutcome::Stopped(stop),
                    target: Some(target),
                    layout_revision: self.layout_revision(),
                    survivor_resize_pending: pending,
                }
            }
        };
        self.close_results.push(result);
        if self
            .global
            .ledger
            .complete(key, result.ledger_outcome())
            .is_err()
        {
            return Err(CloseError::InternalInvariant);
        }
        Ok(result)
    }

    /// Admit one live-runtime close and send HUP without waiting for exit.
    pub fn begin_close(
        &mut self,
        payload: &[u8],
        now_secs: u64,
        now: Instant,
    ) -> Result<CloseStatus, CloseError> {
        let request = decode_close_request(payload).map_err(CloseError::InvalidPayload)?;
        let key = request.key;
        if key.generation != self.global.generation {
            return Err(CloseError::GenerationMismatch);
        }
        if let Some(admission) = self
            .global
            .ledger
            .existing_admission(key, payload, now_secs)
            .map_err(CloseError::Ledger)?
        {
            if let Some(result) = self.retained_close(key) {
                return Ok(CloseStatus::Terminal(result));
            }
            if matches!(admission, Admission::InProgress)
                && self
                    .active_closes
                    .iter()
                    .any(|close| close.request.key == key)
            {
                return Ok(CloseStatus::InProgress);
            }
            return Err(if matches!(admission, Admission::InProgress) {
                CloseError::InProgress
            } else {
                CloseError::InternalInvariant
            });
        }
        if self.active_async_split.is_some() || self.async_emergency_effect.is_some() {
            return Err(CloseError::InProgress);
        }
        if self.close_results.len() + self.active_closes.len() >= MAX_RETAINED_CLOSE_RESULTS {
            return Err(CloseError::Capacity);
        }
        match self
            .global
            .ledger
            .admit(key, payload, now_secs)
            .map_err(CloseError::Ledger)?
        {
            Admission::Execute => {}
            Admission::InProgress | Admission::Completed(_) => {
                return Err(CloseError::InternalInvariant);
            }
        }
        let rejection = if self
            .active_closes
            .iter()
            .any(|close| close.request.target == request.target)
        {
            Some(CloseRejection::AlreadyClosed)
        } else {
            self.plan_close(&request).err()
        };
        if let Some(rejection) = rejection {
            let result = CloseResult {
                key,
                outcome: CloseOutcome::Rejected(rejection),
                target: Some(request.target),
                layout_revision: self.layout_revision(),
                survivor_resize_pending: false,
            };
            self.retain_close_result(result)?;
            return Ok(CloseStatus::Terminal(result));
        }
        let stop = self
            .launches
            .session_mut(request.target)
            .map_err(|_| CloseError::InternalInvariant)?
            .effects
            .begin_stop();
        match stop {
            Ok(Some(outcome)) | Err(outcome) => {
                let result = self.finish_close_result(request, outcome)?;
                self.retain_close_result(result)?;
                Ok(CloseStatus::Terminal(result))
            }
            Ok(None) => {
                self.active_closes.push(ActiveClose {
                    request,
                    phase: ActiveClosePhase::Hup {
                        deadline: now + Duration::from_secs(1),
                    },
                });
                Ok(CloseStatus::InProgress)
            }
        }
    }

    /// Advance every active close once. This is owner work and is independent
    /// of any worker's finite reply wait or socket lifetime.
    pub fn advance_closes_at(&mut self, now: Instant) -> Result<(), CloseError> {
        let mut index = 0;
        while index < self.active_closes.len() {
            let transaction = &self.active_closes[index];
            let (kill, expired) = match transaction.phase {
                ActiveClosePhase::Hup { deadline } => (now >= deadline, false),
                ActiveClosePhase::Kill { deadline } => (false, now >= deadline),
            };
            let request = transaction.request;
            let progress = self
                .launches
                .session_mut(request.target)
                .map_err(|_| CloseError::InternalInvariant)?
                .effects
                .advance_stop(kill)
                .unwrap_or(Some(StopOutcome::Unknown));
            let outcome = match progress {
                Some(outcome) => Some(outcome),
                None if expired => Some(StopOutcome::Unknown),
                None if kill => {
                    self.active_closes[index].phase = ActiveClosePhase::Kill {
                        deadline: now + Duration::from_secs(1),
                    };
                    None
                }
                None => None,
            };
            let Some(outcome) = outcome else {
                index += 1;
                continue;
            };
            let transaction = self.active_closes.swap_remove(index);
            debug_assert_eq!(transaction.request, request);
            let result = self.finish_close_result(transaction.request, outcome)?;
            self.retain_close_result(result)?;
        }
        Ok(())
    }

    fn finish_close_result(
        &mut self,
        request: CloseRequest,
        mut outcome: StopOutcome,
    ) -> Result<CloseResult, CloseError> {
        let mut pending = false;
        if matches!(outcome, StopOutcome::Exited | StopOutcome::Escalated) {
            match self.finish_pane_exit(request.target, &mut |effects: &mut E, rows, columns| {
                effects.apply_size(rows, columns)
            }) {
                Ok(resize_pending) => pending = resize_pending,
                Err(()) => outcome = StopOutcome::Unknown,
            }
        }
        Ok(CloseResult {
            key: request.key,
            outcome: CloseOutcome::Stopped(outcome),
            target: Some(request.target),
            layout_revision: self.layout_revision(),
            survivor_resize_pending: pending,
        })
    }

    fn retain_close_result(&mut self, result: CloseResult) -> Result<(), CloseError> {
        self.close_results.push(result);
        self.global
            .ledger
            .complete(result.key, result.ledger_outcome())
            .map_err(|_| CloseError::InternalInvariant)
    }

    /// Observe keyed close state without encoding an in-progress wire result.
    pub fn close_status_state(
        &self,
        connection: &Connection,
        key: OperationKey,
        now_secs: u64,
    ) -> Result<CloseStatus, ServerError> {
        self.global.require_live_connection(connection, now_secs)?;
        if key.lease != connection.lease || key.generation != self.global.generation {
            return Err(ServerError::Unauthorized);
        }
        match self.global.ledger.status(key, now_secs)? {
            OperationStatus::NotAdmitted => Ok(CloseStatus::NotAdmitted),
            OperationStatus::InProgress => {
                if self
                    .active_closes
                    .iter()
                    .any(|close| close.request.key == key)
                {
                    Ok(CloseStatus::InProgress)
                } else {
                    Err(ServerError::UnknownContext)
                }
            }
            OperationStatus::Completed(_) => self
                .retained_close(key)
                .map(CloseStatus::Terminal)
                .ok_or(ServerError::UnknownContext),
        }
    }

    /// Observe a retained close result for one of the caller's own keys.
    ///
    /// A key never admitted in the live ledger returns a `NotAdmitted`
    /// rejection result; a key admitted for another operation kind returns
    /// `UnknownContext`.
    pub fn close_status(
        &self,
        connection: &Connection,
        key: OperationKey,
        now_secs: u64,
    ) -> Result<CloseResult, ServerError> {
        self.global.require_live_connection(connection, now_secs)?;
        if key.lease != connection.lease || key.generation != self.global.generation {
            return Err(ServerError::Unauthorized);
        }
        match self.global.ledger.status(key, now_secs)? {
            OperationStatus::NotAdmitted => Ok(CloseResult {
                key,
                outcome: CloseOutcome::Rejected(CloseRejection::NotAdmitted),
                target: None,
                layout_revision: 0,
                survivor_resize_pending: false,
            }),
            OperationStatus::InProgress | OperationStatus::Completed(_) => {
                self.retained_close(key).ok_or(ServerError::UnknownContext)
            }
        }
    }

    /// Return every current layout pane with its lifecycle and resize state.
    ///
    /// Entries follow the layout's spatial order; unused entries are `None`.
    /// A pane whose child exited but has not yet been collapsed by
    /// [`Self::poll`] is reported with `live == false`.
    pub fn pane_statuses(&self) -> [Option<PaneStatus>; crate::pane_layout::MAX_PANES] {
        let Some(layout) = self.layout_snapshot() else {
            return [None; crate::pane_layout::MAX_PANES];
        };
        layout.panes.map(|pane| {
            pane.map(|geometry| {
                let state = self
                    .launches
                    .state(geometry.target)
                    .unwrap_or(SessionState::Failed);
                let exited = self
                    .launches
                    .effects(geometry.target)
                    .map_or(true, |session| session.effects.child_exited());
                PaneStatus {
                    geometry,
                    state,
                    live: !exited && !matches!(state, SessionState::Exited | SessionState::Failed),
                    resize_pending: self.is_resize_pending(geometry.target),
                    exit: None,
                }
            })
        })
    }

    /// Observe [`Self::pane_statuses`] for a live Read-capable connection.
    ///
    /// Like [`Self::layout_view`], pane geometry and lifecycle are shared
    /// metadata visible to any Read connection regardless of its pinned target.
    pub fn pane_view(
        &self,
        connection: &Connection,
        now_secs: u64,
    ) -> Result<[Option<PaneStatus>; crate::pane_layout::MAX_PANES], ServerError> {
        self.global.require_live_connection(connection, now_secs)?;
        if connection.selected_capabilities & CAPABILITY_READ == 0 {
            return Err(ServerError::Unauthorized);
        }
        Ok(self.pane_statuses())
    }

    fn retained_close(&self, key: OperationKey) -> Option<CloseResult> {
        self.close_results
            .iter()
            .find(|result| result.key == key)
            .copied()
    }

    fn layout_revision(&self) -> u64 {
        self.layout
            .as_ref()
            .map_or(0, |layout| layout.snapshot().revision)
    }

    fn plan_close(&self, request: &CloseRequest) -> Result<(), CloseRejection> {
        if self.active_async_split.is_some()
            || self.async_emergency_effect.is_some()
            || self.zoomed_target.is_some()
        {
            return Err(CloseRejection::StaleLayout);
        }
        let layout = self.layout.as_ref().ok_or(CloseRejection::NoLayout)?;
        let current = layout.snapshot();
        let target = request.target;
        let in_layout = current
            .panes
            .into_iter()
            .flatten()
            .any(|pane| pane.target == target);
        if !in_layout {
            return Err(if self.launches.is_retired(target) {
                CloseRejection::AlreadyClosed
            } else {
                CloseRejection::StaleTarget
            });
        }
        if self.launches.effects(target).is_err() {
            return Err(CloseRejection::StaleTarget);
        }
        if current.pane_count() == 1
            && matches!(self.launches.state(target), Ok(SessionState::Exited))
        {
            // The last pane's tombstone is retained so the workspace closes.
            return Err(CloseRejection::AlreadyClosed);
        }
        if request.expected_layout_revision != current.revision {
            return Err(CloseRejection::StaleLayout);
        }
        Ok(())
    }

    /// Record an observed exit and remove the pane when a sibling remains.
    ///
    /// Returns whether the survivor's absolute resize is pending. The last
    /// remaining pane keeps its `Exited` tombstone record and layout entry.
    fn finish_pane_exit<RE, Resize>(
        &mut self,
        target: TargetRef,
        resize: &mut Resize,
    ) -> Result<bool, ()>
    where
        Resize: FnMut(&mut E, u16, u16) -> Result<(), RE>,
    {
        if self.launches.state(target) != Ok(SessionState::Exited) {
            self.launches.note_exited(target).map_err(|_| ())?;
        }
        let Some(layout) = self.layout.as_mut() else {
            return Ok(false);
        };
        let before = layout.snapshot();
        if before.pane_count() <= 1
            || !before
                .panes
                .into_iter()
                .flatten()
                .any(|pane| pane.target == target)
        {
            if before.pane_count() == 1 && self.zoomed_target == Some(target) {
                self.layout
                    .as_mut()
                    .ok_or(())?
                    .advance_revision(before.revision)
                    .map_err(|_| ())?;
                self.zoomed_target = None;
            }
            self.record_workspace_layout();
            return Ok(false);
        }
        let collapsed = layout.collapse(target, before.revision).map_err(|_| ())?;
        if self.zoomed_target == Some(target) {
            self.zoomed_target = None;
        }
        self.record_workspace_layout();
        for slot in &mut self.pane_sizes {
            if slot.is_some_and(|state| state.target == target) {
                *slot = None;
            }
        }
        // The retired handle's child already exited; dropping it releases
        // the PTY without another wait window.
        drop(self.launches.retire(target).map_err(|_| ())?);
        let mut resize_pending = false;
        let zoomed = self.zoomed_target;
        let root_content =
            crate::pane_layout::content_rect(full_root_rect(collapsed.rows, collapsed.columns));
        for pane in collapsed.panes.iter().flatten().copied() {
            let old_rect = before
                .panes
                .iter()
                .flatten()
                .find(|old| old.target == pane.target)
                .map(|old| old.rect);
            if old_rect == Some(pane.rect) {
                continue;
            }
            let exited = self
                .launches
                .effects(pane.target)
                .map_or(true, |session| session.effects.child_exited());
            if exited {
                continue;
            }
            let applied = if let Some(zoomed) = zoomed {
                if pane.target == zoomed {
                    root_content.is_some_and(|rect| {
                        let desired = (rect.rows, rect.columns);
                        if self
                            .pane_state(zoomed)
                            .is_some_and(|state| state.confirmed == desired)
                        {
                            true
                        } else {
                            self.apply_effective_size(zoomed, desired, collapsed, resize)
                        }
                    })
                } else {
                    let confirmed = self.pane_state(pane.target).map_or(
                        (
                            pane.rect.rows.saturating_sub(2),
                            pane.rect.columns.saturating_sub(2),
                        ),
                        |state| state.confirmed,
                    );
                    self.record_pane_size(pane.target, confirmed, None);
                    true
                }
            } else {
                self.apply_pane_size(pane.target, pane.rect, resize)
            };
            if !applied {
                resize_pending = true;
            }
        }
        Ok(resize_pending)
    }

    /// Collapse every exited pane that still has a sibling in the layout.
    fn collapse_exited_panes(&mut self) -> Result<(), ()> {
        if self.active_async_split.is_some() || self.async_emergency_effect.is_some() {
            return Ok(());
        }
        // A split can be pending while several existing children exit. Keep
        // each exit pending until its preview commits, then produce one
        // revision per individual collapse as the owner settles all exits.
        loop {
            let Some(layout) = self.layout.as_ref() else {
                return Ok(());
            };
            let snapshot = layout.snapshot();
            if snapshot.pane_count() <= 1 {
                if let Some(target) = self.zoomed_target {
                    if matches!(self.launches.state(target), Ok(SessionState::Exited)) {
                        self.finish_pane_exit(target, &mut |effects: &mut E, rows, columns| {
                            effects.apply_size(rows, columns)
                        })?;
                    }
                }
                return Ok(());
            }
            let exited = snapshot.panes.into_iter().flatten().find(|pane| {
                !self
                    .active_closes
                    .iter()
                    .any(|close| close.request.target == pane.target)
                    && matches!(self.launches.state(pane.target), Ok(SessionState::Exited))
            });
            let Some(pane) = exited else {
                return Ok(());
            };
            self.finish_pane_exit(pane.target, &mut |effects: &mut E, rows, columns| {
                effects.apply_size(rows, columns)
            })?;
        }
    }

    /// Retire known-failed launch records outside the layout so their slots
    /// can host a new split. Identities stay spent.
    fn reclaim_failed_slots(&mut self) {
        // Without a layout no split can succeed; keep every record so
        // `all_sessions_ended` is unaffected by a doomed split attempt.
        let Some(layout) = self.layout_snapshot() else {
            return;
        };
        let layout_targets = layout.panes.map(|pane| pane.map(|p| p.target));
        let mut failed = [None; SESSION_CAPACITY];
        for (slot, record) in failed.iter_mut().zip(self.launches.records()) {
            if record.state == SessionState::Failed
                && !record.has_effects
                && !layout_targets.contains(&Some(record.target))
            {
                *slot = Some(record.target);
            }
        }
        for target in failed.into_iter().flatten() {
            let _ = self.launches.retire(target);
        }
    }
}

fn known_reply(
    global: &GlobalAuthority,
    key: OperationKey,
    payload: &[u8],
    now_secs: u64,
) -> Result<Option<Vec<u8>>, ServerError> {
    match global.ledger.existing_admission(key, payload, now_secs)? {
        None => Ok(None),
        Some(Admission::InProgress) => status_reply(key, OperationStatus::InProgress).map(Some),
        Some(Admission::Completed(outcome)) => {
            status_reply(key, OperationStatus::Completed(outcome)).map(Some)
        }
        Some(Admission::Execute) => Err(ServerError::Internal),
    }
}

#[derive(Clone, Copy)]
enum PaneMutation {
    Swap(PaneSwap),
    Divider(PaneDividerSet),
    Zoom(PaneZoomSet),
}

impl PaneMutation {
    fn revision(self) -> u64 {
        match self {
            Self::Swap(request) => request.expected_layout_revision,
            Self::Divider(request) => request.expected_layout_revision,
            Self::Zoom(request) => request.expected_layout_revision,
        }
    }

    fn targets(self) -> [Option<TargetRef>; 2] {
        match self {
            Self::Swap(request) => [Some(request.source), Some(request.destination)],
            Self::Divider(request) => [Some(request.target), None],
            Self::Zoom(request) => [Some(request.target), None],
        }
    }
}

fn layout_rejection(error: LayoutError) -> RejectionCode {
    match error {
        LayoutError::StaleTarget | LayoutError::TargetScopeMismatch => RejectionCode::StaleTarget,
        LayoutError::StaleRevision => RejectionCode::StaleOwner,
        LayoutError::PaneLimitExceeded | LayoutError::RevisionExhausted => RejectionCode::Capacity,
        _ => RejectionCode::InvalidRequest,
    }
}

fn full_root_rect(rows: u16, columns: u16) -> crate::pane_layout::Rect {
    crate::pane_layout::Rect {
        x: 0,
        y: 0,
        rows,
        columns,
    }
}

fn rejected_reply(key: OperationKey, code: RejectionCode) -> Result<Vec<u8>, ServerError> {
    encode_response(Response {
        key,
        body: ResponseBody::Rejected(code),
    })
}

fn status_reply(key: OperationKey, status: OperationStatus) -> Result<Vec<u8>, ServerError> {
    encode_response(Response {
        key,
        body: ResponseBody::Status(status),
    })
}

fn map_connect_registry_error(error: RegistryError) -> ServerError {
    match error {
        RegistryError::InvalidTarget | RegistryError::EffectsUnavailable => {
            ServerError::UnknownContext
        }
        _ => ServerError::Internal,
    }
}

fn map_watch_registry_error(error: RegistryError) -> ServerError {
    match error {
        RegistryError::InvalidTarget | RegistryError::EffectsUnavailable => {
            ServerError::TargetStale
        }
        _ => ServerError::Internal,
    }
}

fn scoped_event_page(target: TargetRef, page: EventPage) -> ScopedEventPage {
    ScopedEventPage {
        events: page.events,
        cursor: SessionCursor {
            generation: target.generation,
            session: target.session,
            sequence: page.cursor.sequence,
        },
        resync_required: page.resync_required,
    }
}

impl<E: EffectSink> EffectSink for TargetState<E> {
    fn validate(&self, command: Command<'_>) -> Result<(), RejectionCode> {
        self.effects.validate(command)
    }

    fn read(
        &mut self,
        source: crate::control_protocol::ReadSource,
        max_text_bytes: usize,
        max_lines: u16,
    ) -> Result<super::ReadResult, Outcome> {
        self.effects.read(source, max_text_bytes, max_lines)
    }

    fn input(&mut self, command: Command<'_>) -> Outcome {
        self.effects.input(command)
    }

    fn poll(&mut self) -> Result<(), Outcome> {
        self.effects.poll()
    }

    fn child_exited(&self) -> bool {
        self.effects.child_exited()
    }

    fn screen_revision(&self) -> Option<u64> {
        self.effects.screen_revision()
    }

    fn stop(&mut self) -> StopOutcome {
        self.effects.stop()
    }
}

#[cfg(test)]
#[path = "multi_session/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "multi_session/split_tests.rs"]
mod split_tests;

#[cfg(test)]
#[path = "multi_session/runtime_helper_tests.rs"]
mod runtime_helper_tests;

#[cfg(test)]
mod close_tests;
