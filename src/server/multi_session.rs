//! Trusted bounded multi-session owner core.
//!
//! This library core does not authenticate peers or impose launch policy.
//! Callers must validate authority before launch, hold the server storage lock
//! for the owner's lifetime, and keep runtime routing disabled until the
//! parent separately adopts it. One caller-supplied ledger owns every lease,
//! launch, control result, status, and receipt in this generation.

use super::{
    decode_request_payload, encode_response, is_capacity_error, owner_rejection, preflight_reply,
    terminal_outcome, Connection, EffectSink, EventPage, GlobalAuthority, OwnerMutation,
    OwnerObservation, RequestPreflight, ServerError, SessionHandler, SessionReply, TargetState,
};
use crate::control_protocol::{Command, TargetRef};
use crate::event_journal::{EventCursor, EventJournal};
use crate::event_protocol::Event;
use crate::handshake_protocol::{Welcome, CAPABILITY_EVENTS, CAPABILITY_INPUT, CAPABILITY_READ};
use crate::input_owner::{InputOwner, Owner};
use crate::managed_launch::{LaunchJournalPort, ManagedLaunchCoordinator, ManagedLaunchError};
use crate::managed_launch_protocol::{decode_launch_request, LaunchRequestView, LaunchResult};
use crate::pane_layout::{LayoutError, LayoutSnapshot, PaneLayout};
use crate::protocol::{
    Admission, Ledger, OperationKey, OperationStatus, Outcome, ProtocolError, MAX_ACTIVE_LEASES,
};
use crate::response_protocol::{RejectionCode, Response, ResponseBody};
use crate::scoped_watch_protocol::SessionCursor;
use crate::session_protocol::{decode_session_request, SessionHello, SessionRequest};
use crate::session_registry::{RegistryError, SessionState, SESSION_CAPACITY};
use crate::split_protocol::{decode_split_request, SplitCodecError, SplitResult};
use std::collections::HashMap;
use std::fmt;

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

/// One trusted owner for a generation's shared authority and two retained
/// process sessions. This does not authenticate callers or select launch
/// policy; a runtime must perform those checks before using the owner.
pub struct MultiSessionOwner<E: EffectSink> {
    global: GlobalAuthority,
    launches: ManagedLaunchCoordinator<TargetState<E>>,
    /// Volatile layout established by the first Applied `execute_launch`.
    layout: Option<PaneLayout>,
    /// The one committed split this two-pane prototype can retain.
    committed_split: Option<SplitResult>,
    /// Bounded per-pane size state keyed by full target: last confirmed
    /// absolute PTY size and, while a resize is pending, the desired size.
    pane_sizes: [Option<PaneSizeState>; 2],
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
            .field("split_committed", &self.committed_split.is_some())
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
    /// The two-session or two-pane prototype capacity is exhausted.
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
        failed: [bool; 2],
    },
}

impl fmt::Display for LayoutResizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoLayout => "no pane layout exists",
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
            committed_split: None,
            pane_sizes: [None; 2],
        })
    }

    /// Return whether at least one session exists and every retained session
    /// has exited or failed, so no live child remains.
    pub fn all_sessions_ended(&self) -> bool {
        let mut any = false;
        for record in self.launches.records() {
            any = true;
            if !matches!(record.state, SessionState::Exited | SessionState::Failed) {
                return false;
            }
        }
        any
    }

    /// Return a copy of the current pane layout, if one is established.
    pub fn layout_snapshot(&self) -> Option<LayoutSnapshot> {
        self.layout.as_ref().map(PaneLayout::snapshot)
    }

    /// Return every layout pane whose absolute PTY resize is still pending.
    ///
    /// Entries are bounded by the two-pane prototype; unused entries are
    /// `None`. A pending pane is retried by [`Self::resize_layout`], including
    /// at the current root size, without changing the layout revision.
    pub fn pending_resizes(&self) -> [Option<PendingResize>; 2] {
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
                            desired: None,
                        }),
                        None,
                    ];
                }
            }
        }
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
        match self
            .global
            .ledger
            .admit(key, payload, now_secs)
            .map_err(SplitError::Ledger)?
        {
            Admission::Execute => {}
            Admission::InProgress | Admission::Completed(_) => {
                return Err(SplitError::InternalInvariant)
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
        self.committed_split = Some(result);
        if self.global.ledger.complete(key, outcome).is_err() {
            let unknown = SplitResult {
                outcome: Outcome::Unknown,
                ..result
            };
            self.committed_split = Some(unknown);
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
        let layout = self.layout.as_mut().ok_or(LayoutResizeError::NoLayout)?;
        let before = layout.snapshot().revision;
        let snapshot = layout
            .resize(expected_revision, rows, columns)
            .map_err(LayoutResizeError::Layout)?;
        let retry_pending_only = snapshot.revision == before;
        let mut failed = [false; 2];
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
        if let Some(result) = self.committed_split.filter(|result| result.key == key) {
            return Ok(result);
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
        if request.launch().dimensions() != (planned.rect.rows, planned.rect.columns) {
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
        let desired = (rect.rows, rect.columns);
        let applied = match self.launches.session_mut(target) {
            Ok(session) => resize(&mut session.effects, rect.rows, rect.columns).is_ok(),
            Err(_) => false,
        };
        if applied {
            self.record_pane_size(target, desired, None);
        } else {
            let confirmed = self
                .pane_state(target)
                .map_or(desired, |state| state.confirmed);
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
        // Both prototype panes are tracked from layout establishment and split
        // commit, so a third distinct target cannot reach this point.
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
        let session = self
            .launches
            .session_mut(target)
            .map_err(map_connect_registry_error)?;
        self.global.connect_with_capability_mask(
            session,
            hello,
            now_secs,
            CAPABILITY_READ | CAPABILITY_INPUT | CAPABILITY_EVENTS,
        )
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
    pub fn poll(&mut self, now_secs: u64) -> Result<(), ServerError> {
        self.global.expire(now_secs);

        let mut records = [None; SESSION_CAPACITY];
        for (record_count, record) in self.launches.records().take(SESSION_CAPACITY).enumerate() {
            records[record_count] = Some(record);
        }

        let mut first_error = None;
        for record in records.into_iter().flatten() {
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

        first_error.map_or(Ok(()), Err)
    }
}

/// One layout snapshot with its per-pane resize-pending state.
pub type LayoutView = (LayoutSnapshot, [Option<PendingResize>; 2]);

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
        pending: [bool; 2],
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
                    return status_reply(key, OperationStatus::Completed(outcome))
                }
                Err(error) if is_capacity_error(&error) => {
                    return rejected_reply(key, RejectionCode::Capacity)
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
        match self.global.ledger.admit(key, payload, now_secs) {
            Ok(Admission::Execute) => {}
            Ok(Admission::InProgress) => {
                return status_reply(key, OperationStatus::InProgress)
                    .map(RootResizeReply::Response)
            }
            Ok(Admission::Completed(outcome)) => {
                return status_reply(key, OperationStatus::Completed(outcome))
                    .map(RootResizeReply::Response)
            }
            Err(error) if is_capacity_error(&error) => {
                return rejected_reply(key, RejectionCode::Capacity).map(RootResizeReply::Response)
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

    fn pending_flags(&self, snapshot: &LayoutSnapshot) -> [bool; 2] {
        let mut flags = [false; 2];
        for (flag, pane) in flags.iter_mut().zip(snapshot.panes) {
            *flag = pane.is_some_and(|pane| self.is_resize_pending(pane.target));
        }
        flags
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
}

#[cfg(test)]
mod tests {
    use super::MultiSessionOwner;
    use crate::control_protocol::{
        encode_request_payload, Command, ControlRequest, ReadSource, TargetRef,
    };
    use crate::handshake_protocol::{Hello, CAPABILITY_EVENTS, CAPABILITY_READ};
    use crate::managed_launch::{LaunchJournalPort, ManagedLaunchCoordinator, ManagedLaunchError};
    use crate::managed_launch_protocol::{encode_launch_request, LaunchRequest};
    use crate::protocol::{
        Ledger, OperationKey, OperationStatus, Outcome, ProtocolError, LEASE_TTL_SECS,
    };
    use crate::response_protocol::decode_response_payload;
    use crate::response_protocol::{RejectionCode, ResponseBody};
    use crate::scoped_watch_protocol::SessionCursor;
    use crate::server::SessionReply;
    use crate::server::{Connection, EffectSink, ReadResult, ServerError};
    use crate::server_storage::{LaunchRecord, LaunchState};
    use crate::session_protocol::{SessionHello, SessionInfo, SessionRequest};
    use crate::session_registry::SessionState;
    use std::cell::Cell;
    use std::io;
    use std::sync::{Arc, Mutex};

    const GENERATION: u64 = 71;
    const INTERNAL_LEASE: u64 = 1;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Event {
        Prepare,
        JournalPreflight,
        Intent,
        Spawn,
        Spawned,
        Applied,
        Failed,
        Unknown,
    }

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    struct EffectCounts {
        validations: usize,
        reads: usize,
        inputs: usize,
        polls: usize,
        metadata: usize,
    }

    #[derive(Default)]
    struct Sink {
        id: u64,
        validations: Cell<usize>,
        reads: usize,
        inputs: usize,
        polls: usize,
        screen_observations: Cell<usize>,
        fail_poll: bool,
        exited: bool,
        revision: Option<u64>,
        read_text: Option<String>,
        read_revision: Option<u64>,
        read_truncated: bool,
        fail_read: bool,
        read_bounds: Option<(usize, u16)>,
        revision_after_read: Option<u64>,
        shared_counts: Option<Arc<Mutex<EffectCounts>>>,
    }

    impl Sink {
        fn count(&self, update: impl FnOnce(&mut EffectCounts)) {
            if let Some(counts) = &self.shared_counts {
                update(&mut counts.lock().unwrap());
            }
        }
    }

    impl EffectSink for Sink {
        fn validate(&self, _command: Command<'_>) -> Result<(), RejectionCode> {
            self.validations.set(self.validations.get() + 1);
            self.count(|counts| counts.validations += 1);
            Ok(())
        }

        fn read(
            &mut self,
            _source: ReadSource,
            max_text_bytes: usize,
            max_lines: u16,
        ) -> Result<ReadResult, Outcome> {
            self.reads += 1;
            self.count(|counts| counts.reads += 1);
            self.read_bounds = Some((max_text_bytes, max_lines));
            if self.fail_read {
                return Err(Outcome::Failed);
            }
            let revision = self.read_revision.unwrap_or(1);
            if let Some(after_read) = self.revision_after_read {
                self.revision = Some(after_read);
            }
            Ok(ReadResult {
                text: self
                    .read_text
                    .clone()
                    .unwrap_or_else(|| format!("sink-{}", self.id)),
                revision,
                truncated: self.read_truncated,
            })
        }

        fn input(&mut self, _command: Command<'_>) -> Outcome {
            self.inputs += 1;
            self.count(|counts| counts.inputs += 1);
            Outcome::Applied
        }

        fn poll(&mut self) -> Result<(), Outcome> {
            self.polls += 1;
            self.count(|counts| counts.polls += 1);
            if self.fail_poll {
                Err(Outcome::Failed)
            } else {
                Ok(())
            }
        }

        fn child_exited(&self) -> bool {
            self.exited
        }

        fn screen_revision(&self) -> Option<u64> {
            self.screen_observations
                .set(self.screen_observations.get() + 1);
            self.count(|counts| counts.metadata += 1);
            self.revision
        }
    }

    struct Journal {
        events: Arc<Mutex<Vec<Event>>>,
        preflight_failure: bool,
        record_failure: Option<LaunchState>,
    }

    impl Journal {
        fn new(events: &Arc<Mutex<Vec<Event>>>) -> Self {
            Self {
                events: Arc::clone(events),
                preflight_failure: false,
                record_failure: None,
            }
        }
    }

    impl LaunchJournalPort for Journal {
        fn preflight_launch(&self, _key: OperationKey, _session: u64) -> io::Result<()> {
            self.events.lock().unwrap().push(Event::JournalPreflight);
            if self.preflight_failure {
                Err(io::Error::other("private journal detail"))
            } else {
                Ok(())
            }
        }

        fn record(&mut self, record: LaunchRecord) -> io::Result<()> {
            let event = match record.state {
                LaunchState::Intent => Event::Intent,
                LaunchState::Spawned => Event::Spawned,
                LaunchState::Applied => Event::Applied,
                LaunchState::Failed => Event::Failed,
                LaunchState::Unknown => Event::Unknown,
            };
            self.events.lock().unwrap().push(event);
            if self.record_failure == Some(record.state) {
                Err(io::Error::other("private journal detail"))
            } else {
                Ok(())
            }
        }
    }

    fn owner() -> MultiSessionOwner<Sink> {
        let mut ledger = Ledger::new(GENERATION).unwrap();
        ledger.open_lease(INTERNAL_LEASE, 1).unwrap();
        MultiSessionOwner::with_ledger(ledger, true).unwrap()
    }

    fn key(lease: u64, sequence: u64) -> OperationKey {
        OperationKey {
            generation: GENERATION,
            lease,
            sequence,
        }
    }

    fn launch_payload(key: OperationKey, executable: &[u8]) -> Vec<u8> {
        let mut payload = Vec::new();
        encode_launch_request(
            &LaunchRequest {
                key,
                rows: 24,
                columns: 80,
                requested_executable: executable,
                requested_working_directory: b"/",
                client_working_directory: b"/",
                arguments: &[],
                environment: &[],
            },
            &mut payload,
        )
        .unwrap();
        payload
    }

    fn launch(
        owner: &mut MultiSessionOwner<Sink>,
        sequence: u64,
        journal: &mut Journal,
        events: &Arc<Mutex<Vec<Event>>>,
    ) -> Result<crate::managed_launch_protocol::LaunchResult, ManagedLaunchError> {
        owner.execute_launch(
            &launch_payload(key(INTERNAL_LEASE, sequence), b"/bin/sh"),
            2,
            journal,
            |_, target| {
                events.lock().unwrap().push(Event::Prepare);
                Ok::<_, ()>(target.session)
            },
            |id| {
                events.lock().unwrap().push(Event::Spawn);
                Ok::<_, ()>(Sink {
                    id,
                    revision: Some(1),
                    ..Sink::default()
                })
            },
        )
    }

    fn hello() -> SessionHello {
        SessionHello {
            hello: Hello {
                nonce: 17,
                required_capabilities: crate::handshake_protocol::CAPABILITY_READ,
                optional_capabilities: crate::handshake_protocol::CAPABILITY_INPUT
                    | CAPABILITY_EVENTS,
            },
            resume: None,
        }
    }

    fn control_payload(key: OperationKey, target: TargetRef, command: Command<'_>) -> Vec<u8> {
        let request = ControlRequest {
            key,
            target,
            command,
        };
        let mut bytes = Vec::new();
        encode_request_payload(&request, &mut bytes).unwrap();
        bytes
    }

    fn response(payload: &[u8]) -> crate::response_protocol::Response<'_> {
        decode_response_payload(payload).unwrap()
    }

    fn session_payload(request: SessionRequest) -> Vec<u8> {
        crate::session_protocol::encode_session_request(&request).unwrap()
    }

    fn session_result(reply: SessionReply) -> Vec<u8> {
        match reply {
            SessionReply::Payload(payload) => payload,
            other => panic!("expected encoded operation result, got {other:?}"),
        }
    }

    fn session_info(reply: SessionReply) -> SessionInfo {
        match reply {
            SessionReply::Info(info) => info,
            other => panic!("expected session info, got {other:?}"),
        }
    }

    #[test]
    fn preparation_failures_do_not_spend_identity_but_intent_failure_does() {
        let mut first_owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        let failed_prepare = first_owner.execute_launch(
            &launch_payload(key(INTERNAL_LEASE, 1), b"/bin/sh"),
            2,
            &mut journal,
            |_, _| Err::<u64, _>("private prepare detail"),
            |_| Ok::<_, ()>(Sink::default()),
        );
        assert!(matches!(
            failed_prepare,
            Err(ManagedLaunchError::PreparationFailed)
        ));
        assert!(first_owner.launches.records().next().is_none());

        journal.preflight_failure = true;
        assert!(matches!(
            launch(&mut first_owner, 1, &mut journal, &events),
            Err(ManagedLaunchError::JournalPreflightFailed)
        ));
        assert!(first_owner.launches.records().next().is_none());

        journal.preflight_failure = false;
        journal.record_failure = Some(LaunchState::Intent);
        let result = launch(&mut first_owner, 1, &mut journal, &events).unwrap();
        let target = result.target.unwrap();
        assert_eq!(target.session, 1);
        assert_eq!(result.outcome, Outcome::Failed);
        assert_eq!(first_owner.launches.state(target), Ok(SessionState::Failed));
        assert_eq!(first_owner.launches.records().count(), 1);

        // A journal failure after admission spends the identity and fails
        // closed; it does not permit a later launch through this coordinator.
        assert!(matches!(
            launch(&mut first_owner, 2, &mut journal, &events),
            Err(ManagedLaunchError::Unhealthy)
        ));

        // A spawn failure also spends its slot, but remains recoverable when
        // the journal itself is healthy.
        let mut spawn_owner = owner();
        journal.record_failure = None;
        let spawn_failed = spawn_owner
            .execute_launch(
                &launch_payload(key(INTERNAL_LEASE, 1), b"/bin/sh"),
                2,
                &mut journal,
                |_, target| Ok::<_, ()>(target.session),
                |_| Err::<Sink, _>("private spawn detail"),
            )
            .unwrap();
        assert_eq!(spawn_failed.outcome, Outcome::Failed);
        assert_eq!(spawn_failed.target.unwrap().session, 1);
        journal.record_failure = None;
        let second = launch(&mut spawn_owner, 2, &mut journal, &events).unwrap();
        assert_eq!(second.target.unwrap().session, 2);
        assert!(matches!(
            launch(&mut spawn_owner, 3, &mut journal, &events),
            Err(ManagedLaunchError::Registry(
                crate::session_registry::RegistryError::Capacity
            ))
        ));
        assert_eq!(spawn_owner.launches.records().count(), 2);
    }

    #[test]
    fn launch_orders_seed_prepare_spawn_and_retains_handles_after_unknown_result() {
        for failed_state in [LaunchState::Spawned, LaunchState::Applied] {
            let mut owner = owner();
            let events = Arc::new(Mutex::new(Vec::new()));
            let mut journal = Journal::new(&events);
            journal.record_failure = Some(failed_state);
            let result = launch(&mut owner, 1, &mut journal, &events).unwrap();
            let target = result.target.unwrap();
            assert_eq!(result.outcome, Outcome::Unknown);
            let mut expected = vec![
                Event::Prepare,
                Event::JournalPreflight,
                Event::Intent,
                Event::Spawn,
                Event::Spawned,
            ];
            if failed_state == LaunchState::Applied {
                expected.push(Event::Applied);
            }
            assert_eq!(*events.lock().unwrap(), expected);
            assert_eq!(owner.launches.state(target), Ok(SessionState::Unknown));
            let session = owner.launches.effects(target).unwrap();
            assert_eq!(session.effects.id, target.session);
            assert_eq!(session.effects.polls, 0);

            // The owner can continue polling and route a fresh public read to
            // the sink retained before either post-spawn journal fault.
            owner.poll(3).unwrap();
            assert_eq!(owner.launches.effects(target).unwrap().effects.polls, 1);
            let (connection, _) = owner.connect(target, &hello(), 4).unwrap();
            let read = control_payload(
                key(connection.lease(), 1),
                target,
                Command::Read {
                    source: ReadSource::Screen,
                    max_bytes: 256,
                    max_lines: 1,
                },
            );
            assert!(matches!(
                response(&owner.handle_control(&connection, &read, 5).unwrap()).body,
                ResponseBody::Read { target: read_target, .. } if read_target == target
            ));
            assert_eq!(owner.launches.effects(target).unwrap().effects.reads, 1);
            assert!(matches!(
                format!("{owner:?}"),
                rendered if !rendered.contains("/bin/sh") && !rendered.contains("private journal")
            ));
        }
    }

    #[test]
    fn scoped_events_are_advertised_only_when_available_and_never_added_to_resumed_grants() {
        let mut owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        let target = launch(&mut owner, 1, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        {
            let session = owner.launches.session_mut(target).unwrap();
            session.notifications_available = false;
            session.effects.revision = None;
        }

        let optional_events = SessionHello {
            hello: Hello {
                nonce: 18,
                required_capabilities: CAPABILITY_READ,
                optional_capabilities: crate::handshake_protocol::CAPABILITY_INPUT
                    | CAPABILITY_EVENTS,
            },
            resume: None,
        };
        let (connection, welcome) = owner.connect(target, &optional_events, 4).unwrap();
        assert_eq!(connection.lease(), 2);
        assert_eq!(
            welcome.selected_capabilities,
            CAPABILITY_READ | crate::handshake_protocol::CAPABILITY_INPUT
        );

        let required_events = SessionHello {
            hello: Hello {
                nonce: 19,
                required_capabilities: CAPABILITY_READ | CAPABILITY_EVENTS,
                optional_capabilities: 0,
            },
            resume: None,
        };
        assert!(matches!(
            owner.connect(target, &required_events, 4),
            Err(ServerError::Handshake(_))
        ));
        assert_eq!(owner.global.grants.len(), 1);

        {
            let session = owner.launches.session_mut(target).unwrap();
            session.notifications_available = true;
            session.effects.revision = Some(1);
        }
        let resume = SessionHello {
            resume: Some(crate::session_protocol::LeaseRef {
                generation: GENERATION,
                lease: connection.lease(),
            }),
            ..optional_events
        };
        let (_, resumed_welcome) = owner.connect(target, &resume, 5).unwrap();
        assert_eq!(
            resumed_welcome.selected_capabilities,
            CAPABILITY_READ | crate::handshake_protocol::CAPABILITY_INPUT
        );
        let (event_connection, event_welcome) = owner
            .connect(
                target,
                &SessionHello {
                    hello: Hello {
                        nonce: 21,
                        required_capabilities: CAPABILITY_READ | CAPABILITY_EVENTS,
                        optional_capabilities: 0,
                    },
                    resume: None,
                },
                6,
            )
            .unwrap();
        assert_eq!(event_connection.lease(), 3);
        assert_eq!(
            event_welcome.selected_capabilities,
            CAPABILITY_READ | CAPABILITY_EVENTS
        );
        assert_eq!(owner.global.grants.len(), 2);
    }

    #[test]
    fn global_launch_dedup_connect_scope_and_control_dispatch_share_authority() {
        let mut owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        let first_payload = launch_payload(key(INTERNAL_LEASE, 1), b"/bin/sh");
        let first = owner
            .execute_launch(
                &first_payload,
                2,
                &mut journal,
                |_, target| Ok::<_, ()>(target.session),
                |id| {
                    Ok::<_, ()>(Sink {
                        id,
                        revision: Some(1),
                        ..Sink::default()
                    })
                },
            )
            .unwrap();
        let target_one = first.target.unwrap();
        let events_before_duplicate = events.lock().unwrap().len();
        let duplicate = owner
            .execute_launch(
                &first_payload,
                3,
                &mut journal,
                |_, _| -> Result<u64, ()> { panic!("exact launch retry must not prepare") },
                |_: u64| -> Result<Sink, ()> { panic!("exact launch retry must not spawn") },
            )
            .unwrap();
        assert_eq!(duplicate, first);
        assert_eq!(events.lock().unwrap().len(), events_before_duplicate);

        let changed = launch_payload(key(INTERNAL_LEASE, 1), b"/bin/zsh");
        assert!(matches!(
            owner.execute_launch(
                &changed,
                3,
                &mut journal,
                |_, _| Ok::<_, ()>(()),
                |_| Ok::<_, ()>(Sink::default()),
            ),
            Err(ManagedLaunchError::Ledger(ProtocolError::OperationConflict))
        ));

        // A second key is a distinct target and lease while the ledger remains shared.
        let second = owner
            .execute_launch(
                &launch_payload(key(INTERNAL_LEASE, 2), b"/bin/sh"),
                3,
                &mut journal,
                |_, target| Ok::<_, ()>(target.session),
                |id| {
                    Ok::<_, ()>(Sink {
                        id,
                        revision: Some(1),
                        ..Sink::default()
                    })
                },
            )
            .unwrap();
        let target_two = second.target.unwrap();
        assert_eq!(target_one.session, 1);
        assert_eq!(target_two.session, 2);
        assert_ne!(target_one, target_two);

        let (connection_one, _) = owner.connect(target_one, &hello(), 4).unwrap();
        let (connection_two, _) = owner.connect(target_two, &hello(), 4).unwrap();
        assert_eq!(connection_two.lease(), connection_one.lease() + 1);
        let original_expiry = owner.global.grants[&connection_one.lease()].expires_secs;
        let resumed_other = SessionHello {
            resume: Some(crate::session_protocol::LeaseRef {
                generation: GENERATION,
                lease: connection_one.lease(),
            }),
            ..hello()
        };
        assert!(matches!(
            owner.connect(target_two, &resumed_other, 5),
            Err(ServerError::UnknownContext)
        ));
        assert_eq!(
            owner.global.grants[&connection_one.lease()].expires_secs,
            original_expiry
        );

        let denied_key = key(connection_one.lease(), 1);
        let denied = control_payload(
            denied_key,
            target_two,
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 256,
                max_lines: 1,
            },
        );
        assert_eq!(
            response(&owner.handle_control(&connection_one, &denied, 5).unwrap()).body,
            ResponseBody::Rejected(RejectionCode::Unauthorized)
        );
        assert_eq!(
            owner.global.ledger.status(denied_key, 5).unwrap(),
            OperationStatus::NotAdmitted
        );
        assert_eq!(owner.launches.effects(target_two).unwrap().effects.reads, 0);

        // Input ownership is staged only inside this trusted fixture; the new
        // owner intentionally has no public Claim/Release/Handoff route yet.
        let session_one = owner.launches.session_mut(target_one).unwrap();
        session_one
            .input_owner
            .claim(
                crate::input_owner::Owner::Client(connection_one.lease()),
                1,
                30,
                5,
            )
            .unwrap();
        let read_key = key(connection_one.lease(), 2);
        let read = control_payload(
            read_key,
            target_one,
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 256,
                max_lines: 1,
            },
        );
        assert!(matches!(
            response(&owner.handle_control(&connection_one, &read, 6).unwrap()).body,
            ResponseBody::Read { target, .. } if target == target_one
        ));
        let input_key = key(connection_one.lease(), 3);
        let input = control_payload(
            input_key,
            target_one,
            Command::Submit {
                owner_revision: owner
                    .launches
                    .session_mut(target_one)
                    .unwrap()
                    .input_owner
                    .snapshot()
                    .revision,
                text: "accepted",
            },
        );
        assert_eq!(
            response(&owner.handle_control(&connection_one, &input, 6).unwrap()).body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(
            owner.launches.effects(target_one).unwrap().effects.inputs,
            1
        );
        assert_eq!(
            owner.launches.effects(target_two).unwrap().effects.inputs,
            0
        );
    }

    #[test]
    fn global_control_preflight_short_circuits_before_session_lookup() {
        let mut owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        let target_one = launch(&mut owner, 1, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let target_two = launch(&mut owner, 2, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let (connection_one, _) = owner.connect(target_one, &hello(), 4).unwrap();
        let (connection_two, _) = owner.connect(target_two, &hello(), 4).unwrap();

        let screen_before = owner
            .launches
            .effects(target_one)
            .unwrap()
            .effects
            .screen_observations
            .get();
        let wrong_lease = control_payload(
            key(connection_two.lease(), 9),
            target_one,
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 256,
                max_lines: 1,
            },
        );
        assert_eq!(
            response(
                &owner
                    .handle_control(&connection_one, &wrong_lease, 5)
                    .unwrap()
            )
            .body,
            ResponseBody::Rejected(RejectionCode::Unauthorized)
        );
        assert_eq!(
            owner
                .launches
                .effects(target_one)
                .unwrap()
                .effects
                .screen_observations
                .get(),
            screen_before
        );

        let global_key = key(connection_one.lease(), 10);
        let exact_payload = control_payload(
            global_key,
            target_one,
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 256,
                max_lines: 1,
            },
        );
        owner
            .handle_control(&connection_one, &exact_payload, 5)
            .unwrap();
        let reads = owner.launches.effects(target_one).unwrap().effects.reads;
        let screens = owner
            .launches
            .effects(target_one)
            .unwrap()
            .effects
            .screen_observations
            .get();
        assert_eq!(
            response(
                &owner
                    .handle_control(&connection_one, &exact_payload, 6)
                    .unwrap()
            )
            .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(
            owner.launches.effects(target_one).unwrap().effects.reads,
            reads
        );
        assert_eq!(
            owner
                .launches
                .effects(target_one)
                .unwrap()
                .effects
                .screen_observations
                .get(),
            screens
        );

        let changed_payload = control_payload(
            global_key,
            target_two,
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 257,
                max_lines: 1,
            },
        );
        assert!(matches!(
            owner.handle_control(&connection_one, &changed_payload, 6),
            Err(ServerError::Protocol(ProtocolError::OperationConflict))
        ));
        assert_eq!(owner.launches.effects(target_two).unwrap().effects.reads, 0);
        assert_eq!(
            owner
                .launches
                .effects(target_two)
                .unwrap()
                .effects
                .screen_observations
                .get(),
            1,
            "the target seed is the only metadata sample before lookup"
        );
    }

    #[test]
    fn cached_control_results_and_conflicts_precede_an_empty_target_registry() {
        let mut owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        let target = launch(&mut owner, 1, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let (connection, _) = owner.connect(target, &hello(), 4).unwrap();
        let completed_key = key(connection.lease(), 1);
        let exact_payload = control_payload(
            completed_key,
            target,
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 256,
                max_lines: 1,
            },
        );
        owner
            .handle_control(&connection, &exact_payload, 5)
            .unwrap();

        // Simulate losing every in-memory target record while the one global
        // ledger remains authoritative for cached operation identities.
        owner.launches = ManagedLaunchCoordinator::new(GENERATION).unwrap();
        assert_eq!(
            response(
                &owner
                    .handle_control(&connection, &exact_payload, 6)
                    .unwrap()
            )
            .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );

        let changed_target = TargetRef {
            pane: target.pane + 1,
            ..target
        };
        let changed_payload = control_payload(
            completed_key,
            changed_target,
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 257,
                max_lines: 1,
            },
        );
        assert!(matches!(
            owner.handle_control(&connection, &changed_payload, 6),
            Err(ServerError::Protocol(ProtocolError::OperationConflict))
        ));

        let fresh_key = key(connection.lease(), 2);
        let fresh_payload = control_payload(
            fresh_key,
            target,
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 256,
                max_lines: 1,
            },
        );
        assert_eq!(
            response(
                &owner
                    .handle_control(&connection, &fresh_payload, 6)
                    .unwrap()
            )
            .body,
            ResponseBody::Rejected(RejectionCode::StaleTarget)
        );
        assert_eq!(
            owner.global.ledger.status(fresh_key, 6).unwrap(),
            OperationStatus::NotAdmitted
        );
    }

    #[test]
    fn global_status_receipt_and_polling_never_select_the_wrong_target() {
        let mut owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        let first = launch(&mut owner, 1, &mut journal, &events).unwrap();
        let second = launch(&mut owner, 2, &mut journal, &events).unwrap();
        let target_one = first.target.unwrap();
        let target_two = second.target.unwrap();
        let (connection_one, _) = owner.connect(target_one, &hello(), 4).unwrap();
        let (connection_two, _) = owner.connect(target_two, &hello(), 4).unwrap();

        let first_key = key(connection_one.lease(), 1);
        let first_read = control_payload(
            first_key,
            target_one,
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 256,
                max_lines: 1,
            },
        );
        owner
            .handle_control(&connection_one, &first_read, 5)
            .unwrap();
        let second_key = key(connection_two.lease(), 1);
        let second_read = control_payload(
            second_key,
            target_two,
            Command::Read {
                source: ReadSource::Screen,
                max_bytes: 256,
                max_lines: 1,
            },
        );
        owner
            .handle_control(&connection_two, &second_read, 5)
            .unwrap();

        assert_eq!(
            response(
                &owner
                    .operation_status(&connection_one, first_key, 6)
                    .unwrap()
            )
            .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(
            response(
                &owner
                    .operation_status(&connection_two, second_key, 6)
                    .unwrap()
            )
            .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(
            response(
                &owner
                    .operation_status(&connection_one, second_key, 6)
                    .unwrap()
            )
            .body,
            ResponseBody::Rejected(RejectionCode::Unauthorized)
        );
        assert!(matches!(
            owner.receipt(&connection_one, GENERATION, connection_two.lease(), 1, 6,),
            Err(ServerError::Unauthorized)
        ));
        owner
            .receipt(&connection_one, GENERATION, connection_one.lease(), 1, 6)
            .unwrap();
        assert_eq!(
            owner.global.ledger.status(first_key, 6),
            Err(ProtocolError::ResultExpired)
        );
        assert_eq!(
            owner.global.ledger.status(second_key, 6),
            Ok(OperationStatus::Completed(Outcome::Applied))
        );

        owner
            .launches
            .session_mut(target_one)
            .unwrap()
            .input_owner
            .claim(
                crate::input_owner::Owner::Client(connection_one.lease()),
                1,
                1,
                6,
            )
            .unwrap();
        owner
            .launches
            .session_mut(target_two)
            .unwrap()
            .input_owner
            .claim(
                crate::input_owner::Owner::Client(connection_two.lease()),
                1,
                1,
                6,
            )
            .unwrap();
        owner
            .launches
            .session_mut(target_one)
            .unwrap()
            .effects
            .fail_poll = true;
        owner
            .launches
            .session_mut(target_one)
            .unwrap()
            .effects
            .exited = true;
        owner
            .launches
            .session_mut(target_two)
            .unwrap()
            .effects
            .exited = true;

        assert_eq!(
            owner.poll(LEASE_TTL_SECS + 10),
            Err(ServerError::EffectPollFailed)
        );
        assert_eq!(owner.launches.effects(target_one).unwrap().effects.polls, 1);
        assert_eq!(owner.launches.effects(target_two).unwrap().effects.polls, 1);
        assert_eq!(owner.launches.state(target_one), Ok(SessionState::Exited));
        assert_eq!(owner.launches.state(target_two), Ok(SessionState::Exited));
        assert_eq!(
            owner.launches.records().filter(|r| r.has_effects).count(),
            2
        );
        assert_eq!(owner.global.grants.len(), 0);
        assert_eq!(owner.global.ledger.usage().active_leases, 0);
    }

    #[test]
    fn public_session_claim_then_input_is_independent_on_both_targets() {
        let mut owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        let target_a = launch(&mut owner, 1, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let target_b = launch(&mut owner, 2, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let (connection_a, _) = owner.connect(target_a, &hello(), 4).unwrap();
        let (connection_b, _) = owner.connect(target_b, &hello(), 4).unwrap();
        let initial_a = session_info(
            owner
                .handle_session(&connection_a, &session_payload(SessionRequest::Describe), 5)
                .unwrap(),
        );
        let initial_b = session_info(
            owner
                .handle_session(&connection_b, &session_payload(SessionRequest::Describe), 5)
                .unwrap(),
        );
        assert_eq!(initial_a.target, target_a);
        assert_eq!(initial_b.target, target_b);
        assert_eq!(initial_a.owner_lease, 0);
        assert_eq!(initial_b.owner_lease, 0);

        let claim_a_key = key(connection_a.lease(), 1);
        let claim_a = session_payload(SessionRequest::Claim {
            key: claim_a_key,
            target: target_a,
            expected_owner_revision: initial_a.owner_revision,
            ttl_seconds: 30,
        });
        assert_eq!(
            response(&session_result(
                owner.handle_session(&connection_a, &claim_a, 6).unwrap()
            ))
            .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        let after_a_claim_b = session_info(
            owner
                .handle_session(&connection_b, &session_payload(SessionRequest::Describe), 6)
                .unwrap(),
        );
        assert_eq!(after_a_claim_b, initial_b);

        let claim_b_key = key(connection_b.lease(), 1);
        let claim_b = session_payload(SessionRequest::Claim {
            key: claim_b_key,
            target: target_b,
            expected_owner_revision: initial_b.owner_revision,
            ttl_seconds: 30,
        });
        assert_eq!(
            response(&session_result(
                owner.handle_session(&connection_b, &claim_b, 6).unwrap()
            ))
            .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        let owner_a = session_info(
            owner
                .handle_session(&connection_a, &session_payload(SessionRequest::Describe), 7)
                .unwrap(),
        );
        let owner_b = session_info(
            owner
                .handle_session(&connection_b, &session_payload(SessionRequest::Describe), 7)
                .unwrap(),
        );
        assert_eq!(owner_a.owner_lease, connection_a.lease());
        assert_eq!(owner_b.owner_lease, connection_b.lease());

        let submit_a = control_payload(
            key(connection_a.lease(), 2),
            target_a,
            Command::Submit {
                owner_revision: owner_a.owner_revision,
                text: "from-a",
            },
        );
        let submit_b = control_payload(
            key(connection_b.lease(), 2),
            target_b,
            Command::Submit {
                owner_revision: owner_b.owner_revision,
                text: "from-b",
            },
        );
        for (connection, payload, target) in [
            (&connection_a, &submit_a, target_a),
            (&connection_b, &submit_b, target_b),
        ] {
            assert_eq!(
                response(&owner.handle_control(connection, payload, 8).unwrap()).body,
                ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
            );
            assert_eq!(owner.launches.effects(target).unwrap().effects.inputs, 1);
        }
        assert_eq!(owner.launches.effects(target_a).unwrap().effects.inputs, 1);
        assert_eq!(owner.launches.effects(target_b).unwrap().effects.inputs, 1);
    }

    #[test]
    fn public_release_handoff_and_preflight_denials_preserve_target_owners() {
        let mut owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        let target_a = launch(&mut owner, 1, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let target_b = launch(&mut owner, 2, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let (connection_a, _) = owner.connect(target_a, &hello(), 4).unwrap();
        let (connection_b, _) = owner.connect(target_b, &hello(), 4).unwrap();
        let read_only_hello = SessionHello {
            hello: Hello {
                nonce: 33,
                required_capabilities: CAPABILITY_READ,
                optional_capabilities: 0,
            },
            resume: None,
        };
        let (read_only_recipient, _) = owner.connect(target_a, &read_only_hello, 4).unwrap();

        let counts_a = Arc::new(Mutex::new(EffectCounts::default()));
        let counts_b = Arc::new(Mutex::new(EffectCounts::default()));
        owner
            .launches
            .session_mut(target_a)
            .unwrap()
            .effects
            .shared_counts = Some(Arc::clone(&counts_a));
        owner
            .launches
            .session_mut(target_b)
            .unwrap()
            .effects
            .shared_counts = Some(Arc::clone(&counts_b));

        for (connection, target) in [(&connection_a, target_a), (&connection_b, target_b)] {
            let key = key(connection.lease(), 1);
            let request = session_payload(SessionRequest::Claim {
                key,
                target,
                expected_owner_revision: 1,
                ttl_seconds: 30,
            });
            assert_eq!(
                response(&session_result(
                    owner.handle_session(connection, &request, 5).unwrap()
                ))
                .body,
                ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
            );
        }
        let info_a = session_info(
            owner
                .handle_session(&connection_a, &session_payload(SessionRequest::Describe), 6)
                .unwrap(),
        );
        let info_b = session_info(
            owner
                .handle_session(&connection_b, &session_payload(SessionRequest::Describe), 6)
                .unwrap(),
        );

        let release = session_payload(SessionRequest::Release {
            key: key(connection_a.lease(), 2),
            target: target_a,
            expected_owner_revision: info_a.owner_revision,
        });
        assert_eq!(
            response(&session_result(
                owner.handle_session(&connection_a, &release, 7).unwrap()
            ))
            .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        let after_release_a = session_info(
            owner
                .handle_session(&connection_a, &session_payload(SessionRequest::Describe), 8)
                .unwrap(),
        );
        let after_release_b = session_info(
            owner
                .handle_session(&connection_b, &session_payload(SessionRequest::Describe), 8)
                .unwrap(),
        );
        assert_eq!(after_release_a.owner_lease, 0);
        assert_eq!(after_release_b, info_b);

        // Reclaim A, then try handing it to B's live input grant. The grant is
        // valid but pinned to the wrong target, so no admission or transition.
        let reclaim_key = key(connection_a.lease(), 3);
        let reclaim = session_payload(SessionRequest::Claim {
            key: reclaim_key,
            target: target_a,
            expected_owner_revision: after_release_a.owner_revision,
            ttl_seconds: 30,
        });
        owner.handle_session(&connection_a, &reclaim, 9).unwrap();
        let reclaimed = session_info(
            owner
                .handle_session(&connection_a, &session_payload(SessionRequest::Describe), 9)
                .unwrap(),
        );
        let handoff_key = key(connection_a.lease(), 4);
        let handoff = session_payload(SessionRequest::Handoff {
            key: handoff_key,
            target: target_a,
            expected_owner_revision: reclaimed.owner_revision,
            recipient_lease: connection_b.lease(),
            ttl_seconds: 30,
        });
        assert_eq!(
            response(&session_result(
                owner.handle_session(&connection_a, &handoff, 10).unwrap()
            ))
            .body,
            ResponseBody::Rejected(RejectionCode::Unauthorized)
        );
        assert_eq!(
            owner.global.ledger.status(handoff_key, 10).unwrap(),
            OperationStatus::NotAdmitted
        );
        assert_eq!(
            session_info(
                owner
                    .handle_session(
                        &connection_a,
                        &session_payload(SessionRequest::Describe),
                        10,
                    )
                    .unwrap(),
            )
            .owner_lease,
            connection_a.lease()
        );

        let missing_recipient_key = key(connection_a.lease(), 5);
        let missing_recipient = session_payload(SessionRequest::Handoff {
            key: missing_recipient_key,
            target: target_a,
            expected_owner_revision: reclaimed.owner_revision,
            recipient_lease: connection_b.lease() + 100,
            ttl_seconds: 30,
        });
        assert_eq!(
            response(&session_result(
                owner
                    .handle_session(&connection_a, &missing_recipient, 10)
                    .unwrap()
            ))
            .body,
            ResponseBody::Rejected(RejectionCode::UnknownContext)
        );
        assert_eq!(
            owner
                .global
                .ledger
                .status(missing_recipient_key, 10)
                .unwrap(),
            OperationStatus::NotAdmitted
        );

        let no_input_key = key(connection_a.lease(), 6);
        let no_input_handoff = session_payload(SessionRequest::Handoff {
            key: no_input_key,
            target: target_a,
            expected_owner_revision: reclaimed.owner_revision,
            recipient_lease: read_only_recipient.lease(),
            ttl_seconds: 30,
        });
        assert_eq!(
            response(&session_result(
                owner
                    .handle_session(&connection_a, &no_input_handoff, 10)
                    .unwrap()
            ))
            .body,
            ResponseBody::Rejected(RejectionCode::Unauthorized)
        );
        assert_eq!(
            owner.global.ledger.status(no_input_key, 10).unwrap(),
            OperationStatus::NotAdmitted
        );

        let expired_recipient_key = key(connection_a.lease(), 7);
        owner
            .global
            .grants
            .get_mut(&read_only_recipient.lease())
            .unwrap()
            .expires_secs = 10;
        let expired_recipient = session_payload(SessionRequest::Handoff {
            key: expired_recipient_key,
            target: target_a,
            expected_owner_revision: reclaimed.owner_revision,
            recipient_lease: read_only_recipient.lease(),
            ttl_seconds: 30,
        });
        assert_eq!(
            response(&session_result(
                owner
                    .handle_session(&connection_a, &expired_recipient, 10)
                    .unwrap()
            ))
            .body,
            ResponseBody::Rejected(RejectionCode::UnknownContext)
        );
        assert_eq!(
            owner
                .global
                .ledger
                .status(expired_recipient_key, 10)
                .unwrap(),
            OperationStatus::NotAdmitted
        );

        // Wrong target generation and lease are global denials before exact
        // target selection, and therefore do not synchronize metadata.
        let before_a = *counts_a.lock().unwrap();
        let wrong_generation = TargetRef {
            generation: GENERATION + 1,
            ..target_a
        };
        let wrong_generation_payload = session_payload(SessionRequest::Claim {
            key: OperationKey {
                generation: GENERATION + 1,
                lease: connection_a.lease(),
                sequence: 6,
            },
            target: wrong_generation,
            expected_owner_revision: reclaimed.owner_revision,
            ttl_seconds: 30,
        });
        assert_eq!(
            response(&session_result(
                owner
                    .handle_session(&connection_a, &wrong_generation_payload, 10)
                    .unwrap()
            ))
            .body,
            ResponseBody::Rejected(RejectionCode::Unauthorized)
        );
        let wrong_lease_payload = session_payload(SessionRequest::Claim {
            key: key(connection_b.lease(), 6),
            target: target_a,
            expected_owner_revision: reclaimed.owner_revision,
            ttl_seconds: 30,
        });
        assert_eq!(
            response(&session_result(
                owner
                    .handle_session(&connection_a, &wrong_lease_payload, 10)
                    .unwrap()
            ))
            .body,
            ResponseBody::Rejected(RejectionCode::Unauthorized)
        );
        assert_eq!(*counts_a.lock().unwrap(), before_a);
        assert_eq!(counts_b.lock().unwrap().inputs, 0);
    }

    #[test]
    fn public_same_target_handoff_grants_recipient_input_and_release() {
        let mut owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        let target_a = launch(&mut owner, 1, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let target_b = launch(&mut owner, 2, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let (owner_a, _) = owner.connect(target_a, &hello(), 4).unwrap();
        let (sibling_owner, _) = owner.connect(target_b, &hello(), 4).unwrap();
        let (recipient, _) = owner.connect(target_a, &hello(), 4).unwrap();

        for (connection, target) in [(&owner_a, target_a), (&sibling_owner, target_b)] {
            let request = session_payload(SessionRequest::Claim {
                key: key(connection.lease(), 1),
                target,
                expected_owner_revision: 1,
                ttl_seconds: 30,
            });
            assert_eq!(
                response(&session_result(
                    owner.handle_session(connection, &request, 5).unwrap()
                ))
                .body,
                ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
            );
        }
        let owner_info = session_info(
            owner
                .handle_session(&owner_a, &session_payload(SessionRequest::Describe), 6)
                .unwrap(),
        );
        let sibling_info = session_info(
            owner
                .handle_session(
                    &sibling_owner,
                    &session_payload(SessionRequest::Describe),
                    6,
                )
                .unwrap(),
        );
        let handoff_key = key(owner_a.lease(), 2);
        let handoff = session_payload(SessionRequest::Handoff {
            key: handoff_key,
            target: target_a,
            expected_owner_revision: owner_info.owner_revision,
            recipient_lease: recipient.lease(),
            ttl_seconds: 30,
        });
        assert_eq!(
            response(&session_result(
                owner.handle_session(&owner_a, &handoff, 7).unwrap()
            ))
            .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        let handed_off = session_info(
            owner
                .handle_session(&recipient, &session_payload(SessionRequest::Describe), 8)
                .unwrap(),
        );
        assert_eq!(handed_off.owner_lease, recipient.lease());

        let recipient_submit = control_payload(
            key(recipient.lease(), 1),
            target_a,
            Command::Submit {
                owner_revision: handed_off.owner_revision,
                text: "recipient-input",
            },
        );
        assert_eq!(
            response(
                &owner
                    .handle_control(&recipient, &recipient_submit, 9)
                    .unwrap()
            )
            .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(owner.launches.effects(target_a).unwrap().effects.inputs, 1);

        // Use the current revision so this denial proves the prior owner no
        // longer holds ownership, rather than merely presenting a stale CAS.
        let previous_owner_submit = control_payload(
            key(owner_a.lease(), 3),
            target_a,
            Command::Submit {
                owner_revision: handed_off.owner_revision,
                text: "old-owner-input",
            },
        );
        assert_eq!(
            response(
                &owner
                    .handle_control(&owner_a, &previous_owner_submit, 9)
                    .unwrap()
            )
            .body,
            ResponseBody::Rejected(RejectionCode::Unauthorized)
        );
        assert_eq!(
            owner
                .global
                .ledger
                .status(key(owner_a.lease(), 3), 9)
                .unwrap(),
            OperationStatus::NotAdmitted
        );
        assert_eq!(owner.launches.effects(target_a).unwrap().effects.inputs, 1);
        assert_eq!(
            session_info(
                owner
                    .handle_session(
                        &sibling_owner,
                        &session_payload(SessionRequest::Describe),
                        9,
                    )
                    .unwrap(),
            ),
            sibling_info
        );
        assert_eq!(owner.launches.effects(target_b).unwrap().effects.inputs, 0);

        let recipient_release = session_payload(SessionRequest::Release {
            key: key(recipient.lease(), 2),
            target: target_a,
            expected_owner_revision: handed_off.owner_revision,
        });
        assert_eq!(
            response(&session_result(
                owner
                    .handle_session(&recipient, &recipient_release, 10)
                    .unwrap()
            ))
            .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(
            session_info(
                owner
                    .handle_session(&recipient, &session_payload(SessionRequest::Describe), 11,)
                    .unwrap(),
            )
            .owner_lease,
            0
        );
        assert_eq!(
            session_info(
                owner
                    .handle_session(
                        &sibling_owner,
                        &session_payload(SessionRequest::Describe),
                        11,
                    )
                    .unwrap(),
            ),
            sibling_info
        );
    }

    #[test]
    fn session_dedup_status_and_stale_results_survive_empty_target_registry() {
        let mut owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        let target = launch(&mut owner, 1, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let (connection, _) = owner.connect(target, &hello(), 4).unwrap();
        let counts = Arc::new(Mutex::new(EffectCounts::default()));
        owner
            .launches
            .session_mut(target)
            .unwrap()
            .effects
            .shared_counts = Some(Arc::clone(&counts));
        let mutation_key = key(connection.lease(), 1);
        let exact = session_payload(SessionRequest::Claim {
            key: mutation_key,
            target,
            expected_owner_revision: 1,
            ttl_seconds: 30,
        });
        owner.handle_session(&connection, &exact, 5).unwrap();
        let counts_after_admission = *counts.lock().unwrap();

        owner.launches = ManagedLaunchCoordinator::new(GENERATION).unwrap();
        assert_eq!(
            response(&session_result(
                owner.handle_session(&connection, &exact, 6).unwrap()
            ))
            .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        let changed_target = TargetRef {
            pane: target.pane + 1,
            ..target
        };
        let changed = session_payload(SessionRequest::Claim {
            key: mutation_key,
            target: changed_target,
            expected_owner_revision: 2,
            ttl_seconds: 30,
        });
        assert!(matches!(
            owner.handle_session(&connection, &changed, 6),
            Err(ServerError::Protocol(ProtocolError::OperationConflict))
        ));

        let stale_key = key(connection.lease(), 2);
        let stale = session_payload(SessionRequest::Claim {
            key: stale_key,
            target,
            expected_owner_revision: 1,
            ttl_seconds: 30,
        });
        assert_eq!(
            response(&session_result(
                owner.handle_session(&connection, &stale, 6).unwrap()
            ))
            .body,
            ResponseBody::Rejected(RejectionCode::StaleTarget)
        );
        assert_eq!(
            owner.global.ledger.status(stale_key, 6).unwrap(),
            OperationStatus::NotAdmitted
        );
        assert_eq!(*counts.lock().unwrap(), counts_after_admission);
    }

    #[test]
    fn describe_pins_target_and_targetless_requests_need_no_session() {
        let mut owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        let target_a = launch(&mut owner, 1, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let target_b = launch(&mut owner, 2, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let (connection_a, _) = owner.connect(target_a, &hello(), 4).unwrap();
        let (connection_b, _) = owner.connect(target_b, &hello(), 4).unwrap();
        assert_eq!(
            session_info(
                owner
                    .handle_session(&connection_a, &session_payload(SessionRequest::Describe), 5,)
                    .unwrap(),
            )
            .target,
            target_a
        );
        assert_eq!(
            session_info(
                owner
                    .handle_session(&connection_b, &session_payload(SessionRequest::Describe), 5,)
                    .unwrap(),
            )
            .target,
            target_b
        );

        let claim_key = key(connection_a.lease(), 1);
        let claim = session_payload(SessionRequest::Claim {
            key: claim_key,
            target: target_a,
            expected_owner_revision: 1,
            ttl_seconds: 30,
        });
        owner.handle_session(&connection_a, &claim, 6).unwrap();
        let no_read_hello = SessionHello {
            hello: Hello {
                nonce: 88,
                required_capabilities: crate::handshake_protocol::CAPABILITY_INPUT,
                optional_capabilities: 0,
            },
            resume: None,
        };
        let (no_read, _) = owner.connect(target_a, &no_read_hello, 6).unwrap();

        let counts_a = Arc::new(Mutex::new(EffectCounts::default()));
        let counts_b = Arc::new(Mutex::new(EffectCounts::default()));
        owner
            .launches
            .session_mut(target_a)
            .unwrap()
            .effects
            .shared_counts = Some(Arc::clone(&counts_a));
        owner
            .launches
            .session_mut(target_b)
            .unwrap()
            .effects
            .shared_counts = Some(Arc::clone(&counts_b));
        let before_a = *counts_a.lock().unwrap();
        let before_b = *counts_b.lock().unwrap();

        owner.launches = ManagedLaunchCoordinator::new(GENERATION).unwrap();
        assert_eq!(
            response(&session_result(
                owner
                    .handle_session(
                        &connection_a,
                        &session_payload(SessionRequest::OperationStatus { key: claim_key }),
                        7,
                    )
                    .unwrap()
            ))
            .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        let wrong_key = key(connection_b.lease(), 1);
        assert_eq!(
            response(&session_result(
                owner
                    .handle_session(
                        &connection_a,
                        &session_payload(SessionRequest::OperationStatus { key: wrong_key }),
                        7,
                    )
                    .unwrap()
            ))
            .body,
            ResponseBody::Rejected(RejectionCode::Unauthorized)
        );
        assert!(matches!(
            owner.handle_session(
                &no_read,
                &session_payload(SessionRequest::OperationStatus { key: claim_key }),
                7
            ),
            Err(ServerError::Unauthorized)
        ));
        assert!(matches!(
            owner.handle_session(
                &connection_a,
                &session_payload(SessionRequest::Receipt {
                    generation: GENERATION,
                    lease: connection_b.lease(),
                    through_sequence: 1,
                }),
                7
            ),
            Err(ServerError::Unauthorized)
        ));
        assert!(matches!(
            owner.handle_session(&connection_a, &[0, 0, 0, 0], 7),
            Err(ServerError::Session(_))
        ));
        assert_eq!(
            response(&session_result(
                owner
                    .handle_session(
                        &connection_a,
                        &session_payload(SessionRequest::Receipt {
                            generation: GENERATION,
                            lease: connection_a.lease(),
                            through_sequence: 1,
                        }),
                        7,
                    )
                    .unwrap()
            ))
            .body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert!(matches!(
            owner.handle_session(
                &connection_a,
                &session_payload(SessionRequest::OperationStatus { key: claim_key }),
                8
            ),
            Err(ServerError::Protocol(ProtocolError::ResultExpired))
        ));
        assert!(matches!(
            owner.handle_session(&connection_a, &session_payload(SessionRequest::Detach), 8),
            Ok(SessionReply::Detached)
        ));
        assert_eq!(*counts_a.lock().unwrap(), before_a);
        assert_eq!(*counts_b.lock().unwrap(), before_b);
    }

    #[test]
    fn selected_describe_error_synchronizes_and_expired_detach_fails_closed() {
        let mut owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        let target = launch(&mut owner, 1, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let (connection, _) = owner.connect(target, &hello(), 4).unwrap();
        let no_read_hello = SessionHello {
            hello: Hello {
                nonce: 34,
                required_capabilities: crate::handshake_protocol::CAPABILITY_INPUT,
                optional_capabilities: 0,
            },
            resume: None,
        };
        let (no_read, _) = owner.connect(target, &no_read_hello, 4).unwrap();
        let before_denials = owner
            .launches
            .effects(target)
            .unwrap()
            .effects
            .screen_observations
            .get();
        assert!(matches!(
            owner.handle_session(&no_read, &session_payload(SessionRequest::Describe), 5),
            Err(ServerError::Unauthorized)
        ));
        let missing_context = Connection {
            generation: GENERATION,
            lease: u64::MAX,
            selected_capabilities: CAPABILITY_READ,
        };
        assert!(matches!(
            owner.handle_session(
                &missing_context,
                &session_payload(SessionRequest::Describe),
                5
            ),
            Err(ServerError::UnknownContext)
        ));
        assert!(matches!(
            owner.handle_session(
                &connection,
                &session_payload(SessionRequest::Describe),
                4 + LEASE_TTL_SECS
            ),
            Err(ServerError::UnknownContext)
        ));
        assert_eq!(
            owner
                .launches
                .effects(target)
                .unwrap()
                .effects
                .screen_observations
                .get(),
            before_denials
        );
        owner
            .launches
            .session_mut(target)
            .unwrap()
            .input_owner
            .claim(crate::input_owner::Owner::Human(7), 1, 30, 5)
            .unwrap();
        let before = owner
            .launches
            .effects(target)
            .unwrap()
            .effects
            .screen_observations
            .get();
        assert!(matches!(
            owner.handle_session(&connection, &session_payload(SessionRequest::Describe), 6),
            Err(ServerError::UnsupportedOwner)
        ));
        assert_eq!(
            owner
                .launches
                .effects(target)
                .unwrap()
                .effects
                .screen_observations
                .get(),
            before + 1
        );
        assert!(matches!(
            owner.handle_session(
                &connection,
                &session_payload(SessionRequest::Detach),
                4 + LEASE_TTL_SECS
            ),
            Err(ServerError::UnknownContext)
        ));
    }

    #[test]
    fn scoped_reads_keep_equal_sequences_in_separate_targets_and_reject_crossed_cursors() {
        let mut owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        let target_a = launch(&mut owner, 1, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let target_b = launch(&mut owner, 2, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let (connection_a, _) = owner.connect(target_a, &hello(), 4).unwrap();
        let (connection_b, _) = owner.connect(target_b, &hello(), 4).unwrap();
        for target in [target_a, target_b] {
            owner
                .launches
                .session_mut(target)
                .unwrap()
                .effects
                .read_text = Some(String::new());
        }
        let a = owner.reconcile(&connection_a, target_a, 128, 1, 5).unwrap();
        let b = owner.reconcile(&connection_b, target_b, 128, 1, 5).unwrap();
        assert_eq!(a.cursor.session, target_a.session);
        assert_eq!(b.cursor.session, target_b.session);
        assert_eq!(a.cursor.sequence, b.cursor.sequence);
        assert_eq!(
            owner
                .launches
                .effects(target_a)
                .unwrap()
                .effects
                .read_bounds,
            Some((0, 1))
        );
        assert_eq!(
            owner
                .launches
                .effects(target_b)
                .unwrap()
                .effects
                .read_bounds,
            Some((0, 1))
        );

        owner
            .launches
            .session_mut(target_a)
            .unwrap()
            .effects
            .revision = Some(2);
        owner.poll(6).unwrap();
        owner
            .launches
            .session_mut(target_b)
            .unwrap()
            .effects
            .revision = Some(2);
        owner.poll(6).unwrap();
        let initial = |target: TargetRef| SessionCursor {
            generation: GENERATION,
            session: target.session,
            sequence: 0,
        };
        let page_a = owner
            .read_events(&connection_a, target_a, initial(target_a), 1, 7)
            .unwrap();
        let page_b = owner
            .read_events(&connection_b, target_b, initial(target_b), 1, 7)
            .unwrap();
        assert_eq!(page_a.cursor.sequence, page_b.cursor.sequence);
        assert_eq!(page_a.events[0].target, target_a);
        assert_eq!(page_b.events[0].target, target_b);
        let before_b = owner
            .launches
            .effects(target_b)
            .unwrap()
            .effects
            .screen_observations
            .get();
        assert!(matches!(
            owner.read_events(&connection_b, target_b, page_a.cursor, 1, 8),
            Err(ServerError::TargetStale)
        ));
        assert_eq!(
            owner
                .launches
                .effects(target_b)
                .unwrap()
                .effects
                .screen_observations
                .get(),
            before_b
        );
    }

    #[test]
    fn scoped_read_denials_and_invalid_bounds_precede_metadata_and_sink_reads() {
        let mut owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        let target = launch(&mut owner, 1, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let (connection, _) = owner.connect(target, &hello(), 4).unwrap();
        let no_events = SessionHello {
            hello: Hello {
                nonce: 33,
                required_capabilities: CAPABILITY_READ,
                optional_capabilities: 0,
            },
            resume: None,
        };
        let (no_events, _) = owner.connect(target, &no_events, 4).unwrap();
        let no_read_hello = SessionHello {
            hello: Hello {
                nonce: 34,
                required_capabilities: crate::handshake_protocol::CAPABILITY_INPUT,
                optional_capabilities: CAPABILITY_EVENTS,
            },
            resume: None,
        };
        let (no_read, _) = owner.connect(target, &no_read_hello, 4).unwrap();
        let before = owner
            .launches
            .effects(target)
            .unwrap()
            .effects
            .screen_observations
            .get();
        assert!(matches!(
            owner.reconcile(&no_events, target, 128, 1, 5),
            Err(ServerError::Unauthorized)
        ));
        assert!(matches!(
            owner.reconcile(&no_read, target, 128, 1, 5),
            Err(ServerError::Unauthorized)
        ));
        assert!(matches!(
            owner.reconcile(&connection, target, 127, 1, 5),
            Err(ServerError::InvalidWatchRequest)
        ));
        assert!(matches!(
            owner.reconcile(&connection, target, 128, 0, 5),
            Err(ServerError::InvalidWatchRequest)
        ));
        assert!(matches!(
            owner.reconcile(&connection, target, 128, 1_025, 5),
            Err(ServerError::InvalidWatchRequest)
        ));
        let cursor = SessionCursor {
            generation: GENERATION,
            session: target.session,
            sequence: 0,
        };
        assert!(matches!(
            owner.read_events(&connection, target, cursor, 0, 5),
            Err(ServerError::InvalidWatchRequest)
        ));
        assert!(matches!(
            owner.read_events(
                &connection,
                target,
                cursor,
                crate::watch_protocol::MAX_WATCH_EVENTS + 1,
                5
            ),
            Err(ServerError::InvalidWatchRequest)
        ));
        assert!(matches!(
            owner.read_events(
                &connection,
                target,
                SessionCursor {
                    generation: GENERATION + 1,
                    ..cursor
                },
                1,
                5
            ),
            Err(ServerError::WrongGeneration)
        ));
        assert!(matches!(
            owner.read_events(
                &connection,
                target,
                SessionCursor {
                    session: target.session + 1,
                    ..cursor
                },
                1,
                5
            ),
            Err(ServerError::TargetStale)
        ));
        let missing = Connection {
            generation: GENERATION,
            lease: u64::MAX,
            selected_capabilities: CAPABILITY_READ | CAPABILITY_EVENTS,
        };
        assert!(matches!(
            owner.reconcile(&missing, target, 128, 1, 5),
            Err(ServerError::UnknownContext)
        ));
        assert!(matches!(
            owner.reconcile(&connection, target, 128, 1, 4 + LEASE_TTL_SECS),
            Err(ServerError::UnknownContext)
        ));
        assert_eq!(
            owner
                .launches
                .effects(target)
                .unwrap()
                .effects
                .screen_observations
                .get(),
            before
        );
        assert_eq!(owner.launches.effects(target).unwrap().effects.reads, 0);
    }

    #[test]
    fn scoped_reconciliation_preserves_exact_budgets_snapshot_errors_and_debug_redaction() {
        let mut owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        let target = launch(&mut owner, 1, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let (connection, _) = owner.connect(target, &hello(), 4).unwrap();
        owner
            .launches
            .session_mut(target)
            .unwrap()
            .effects
            .read_text = Some("abcd".to_owned());
        let state = owner.reconcile(&connection, target, 132, 1, 5).unwrap();
        assert_eq!(state.text, "abcd");
        assert_eq!(
            owner.launches.effects(target).unwrap().effects.read_bounds,
            Some((4, 1))
        );
        let wire = crate::scoped_watch_protocol::encode_reconciliation(
            &crate::scoped_watch_protocol::Reconciliation {
                info: state.info,
                cursor: state.cursor,
                screen_revision: state.screen_revision,
                truncated: state.truncated,
                text: &state.text,
            },
        )
        .unwrap();
        assert_eq!(wire.len(), 132);
        assert!(matches!(
            owner.reconcile(&connection, target, 65_537, 1, 5),
            Err(ServerError::InvalidWatchRequest)
        ));
        owner
            .launches
            .session_mut(target)
            .unwrap()
            .effects
            .read_text = Some("a\nb".to_owned());
        assert!(matches!(
            owner.reconcile(&connection, target, 128, 1, 6),
            Err(ServerError::InvalidSnapshot)
        ));
        owner
            .launches
            .session_mut(target)
            .unwrap()
            .effects
            .read_text = Some("x".repeat(65_408));
        let max = owner
            .reconcile(&connection, target, 65_536, 1_024, 7)
            .unwrap();
        assert_eq!(max.text.len(), 65_408);
        assert_eq!(
            owner.launches.effects(target).unwrap().effects.read_bounds,
            Some((65_408, 1_024))
        );
        assert_eq!(
            crate::scoped_watch_protocol::encode_reconciliation(
                &crate::scoped_watch_protocol::Reconciliation {
                    info: max.info,
                    cursor: max.cursor,
                    screen_revision: max.screen_revision,
                    truncated: max.truncated,
                    text: &max.text,
                }
            )
            .unwrap()
            .len(),
            65_536
        );
        {
            let effects = &mut owner.launches.session_mut(target).unwrap().effects;
            effects.revision_after_read = Some(2);
            effects.read_revision = Some(1);
        }
        assert!(matches!(
            owner.reconcile(&connection, target, 128, 1, 8),
            Err(ServerError::SnapshotChanged)
        ));
        {
            let effects = &mut owner.launches.session_mut(target).unwrap().effects;
            effects.revision_after_read = None;
            effects.read_revision = Some(2);
        }
        owner
            .launches
            .session_mut(target)
            .unwrap()
            .effects
            .fail_read = true;
        assert!(matches!(
            owner.reconcile(&connection, target, 128, 1, 9),
            Err(ServerError::SnapshotReadFailed)
        ));
        owner
            .launches
            .session_mut(target)
            .unwrap()
            .effects
            .fail_read = false;
        owner
            .launches
            .session_mut(target)
            .unwrap()
            .effects
            .read_revision = Some(0);
        assert!(matches!(
            owner.reconcile(&connection, target, 128, 1, 10),
            Err(ServerError::NotificationsUnavailable)
        ));
        let state_debug = format!("{max:?}");
        assert!(state_debug.contains("info: \"<redacted>\""));
        assert!(state_debug.contains("cursor: \"<redacted>\""));
        assert!(state_debug.contains("screen_revision: \"<redacted>\""));
        assert!(!state_debug.contains(&max.text));
    }

    #[test]
    fn scoped_event_pages_return_target_local_resync_state_and_redacted_debug() {
        let mut owner = owner();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut journal = Journal::new(&events);
        let target = launch(&mut owner, 1, &mut journal, &events)
            .unwrap()
            .target
            .unwrap();
        let (connection, _) = owner.connect(target, &hello(), 4).unwrap();
        let cursor = SessionCursor {
            generation: GENERATION,
            session: target.session,
            sequence: 0,
        };
        {
            let session = owner.launches.session_mut(target).unwrap();
            for _ in 0..=crate::event_journal::MAX_RETAINED_EVENTS {
                session
                    .journal
                    .push(target, crate::event_protocol::EventBody::ChildExited)
                    .unwrap();
            }
        }
        let expired = owner
            .read_events(&connection, target, cursor, 64, 5)
            .unwrap();
        assert!(expired.resync_required);
        assert!(expired.events.is_empty());
        assert_eq!(expired.cursor.session, target.session);
        assert_eq!(expired.cursor.sequence, 65);
        let retained = SessionCursor {
            sequence: 1,
            ..cursor
        };
        let current = owner
            .read_events(&connection, target, retained, 64, 6)
            .unwrap();
        assert!(!current.resync_required);
        assert_eq!(current.events.len(), 64);
        assert_eq!(current.cursor.session, target.session);
        let event_debug = format!("{current:?}");
        assert!(event_debug.contains("cursor: \"<redacted>\""));
        assert!(!event_debug.contains("ChildExited"));
    }
}

#[cfg(test)]
mod split_tests {
    use super::{LayoutResizeError, MultiSessionOwner, SplitError, SplitFailure};
    use crate::control_protocol::{Command, ReadSource, TargetRef};
    use crate::managed_launch::LaunchJournalPort;
    use crate::managed_launch_protocol::{encode_launch_request, EnvironmentEntry, LaunchRequest};
    use crate::pane_layout::{LayoutError, Rect, SplitDirection};
    use crate::protocol::{Ledger, OperationKey, OperationStatus, Outcome, ProtocolError};
    use crate::response_protocol::RejectionCode;
    use crate::server::{EffectSink, ReadResult};
    use crate::server_storage::{LaunchRecord, LaunchState};
    use crate::session_registry::{RegistryError, SessionState};
    use crate::split_protocol::{encode_split_request, SplitRequest, SplitResult};
    use std::cell::RefCell;
    use std::io;
    use std::rc::Rc;

    const GENERATION: u64 = 73;
    const LEASE: u64 = 1;
    const NOW: u64 = 2;
    const ROOT_ROWS: u16 = 24;
    const ROOT_COLUMNS: u16 = 81;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Effect {
        Prepare(u64),
        JournalPreflight,
        Record(LaunchState),
        Spawn(u64),
        Resize {
            session: u64,
            rows: u16,
            columns: u16,
        },
        Input(u64),
    }

    type Log = Rc<RefCell<Vec<Effect>>>;

    struct Sink {
        session: u64,
        log: Log,
    }

    impl EffectSink for Sink {
        fn validate(&self, _command: Command<'_>) -> Result<(), RejectionCode> {
            Ok(())
        }

        fn read(
            &mut self,
            _source: ReadSource,
            _max_text_bytes: usize,
            _max_lines: u16,
        ) -> Result<ReadResult, Outcome> {
            Ok(ReadResult {
                text: String::new(),
                revision: 1,
                truncated: false,
            })
        }

        fn input(&mut self, _command: Command<'_>) -> Outcome {
            self.log.borrow_mut().push(Effect::Input(self.session));
            Outcome::Applied
        }

        fn poll(&mut self) -> Result<(), Outcome> {
            Ok(())
        }

        fn child_exited(&self) -> bool {
            false
        }

        fn screen_revision(&self) -> Option<u64> {
            Some(1)
        }
    }

    struct Journal {
        log: Log,
        preflight_failure: bool,
        record_failure: Option<LaunchState>,
    }

    impl LaunchJournalPort for Journal {
        fn preflight_launch(&self, _key: OperationKey, _session: u64) -> io::Result<()> {
            self.log.borrow_mut().push(Effect::JournalPreflight);
            if self.preflight_failure {
                Err(io::Error::other("PRIVATE-JOURNAL"))
            } else {
                Ok(())
            }
        }

        fn record(&mut self, record: LaunchRecord) -> io::Result<()> {
            self.log.borrow_mut().push(Effect::Record(record.state));
            if self.record_failure == Some(record.state) {
                Err(io::Error::other("PRIVATE-JOURNAL"))
            } else {
                Ok(())
            }
        }
    }

    struct Fixture {
        owner: MultiSessionOwner<Sink>,
        log: Log,
        journal: Journal,
        first: TargetRef,
    }

    fn key(sequence: u64) -> OperationKey {
        OperationKey {
            generation: GENERATION,
            lease: LEASE,
            sequence,
        }
    }

    fn launch_bytes(key: OperationKey, rows: u16, columns: u16, env_value: &[u8]) -> Vec<u8> {
        let environment = [EnvironmentEntry {
            name: b"SPLIT_TEST_VALUE",
            value: env_value,
        }];
        let mut bytes = Vec::new();
        encode_launch_request(
            &LaunchRequest {
                key,
                rows,
                columns,
                requested_executable: b"/bin/sh",
                requested_working_directory: b"/",
                client_working_directory: b"/",
                arguments: &[],
                environment: &environment,
            },
            &mut bytes,
        )
        .unwrap();
        bytes
    }

    fn split_bytes(
        key: OperationKey,
        target: TargetRef,
        revision: u64,
        direction: SplitDirection,
        (rows, columns): (u16, u16),
        env_value: &[u8],
    ) -> Vec<u8> {
        let environment = [EnvironmentEntry {
            name: b"SPLIT_TEST_VALUE",
            value: env_value,
        }];
        let launch = LaunchRequest {
            key,
            rows,
            columns,
            requested_executable: b"/bin/sh",
            requested_working_directory: b"/",
            client_working_directory: b"/",
            arguments: &[],
            environment: &environment,
        };
        let mut bytes = Vec::new();
        encode_split_request(
            &SplitRequest {
                key,
                target,
                expected_layout_revision: revision,
                direction,
                launch: &launch,
            },
            &mut bytes,
        )
        .unwrap();
        bytes
    }

    fn fixture() -> Fixture {
        let mut ledger = Ledger::new(GENERATION).unwrap();
        ledger.open_lease(LEASE, 1).unwrap();
        let mut owner = MultiSessionOwner::with_ledger(ledger, true).unwrap();
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut journal = Journal {
            log: Rc::clone(&log),
            preflight_failure: false,
            record_failure: None,
        };
        let spawn_log = Rc::clone(&log);
        let result = owner
            .execute_launch(
                &launch_bytes(key(1), ROOT_ROWS, ROOT_COLUMNS, b"first"),
                NOW,
                &mut journal,
                |_, target| Ok::<_, ()>(target.session),
                move |session| {
                    Ok::<_, ()>(Sink {
                        session,
                        log: spawn_log,
                    })
                },
            )
            .unwrap();
        assert_eq!(result.outcome, Outcome::Applied);
        log.borrow_mut().clear();
        Fixture {
            owner,
            log,
            journal,
            first: result.target.unwrap(),
        }
    }

    fn planned_size(direction: SplitDirection) -> (u16, u16) {
        match direction {
            SplitDirection::Left | SplitDirection::Right => (24, 40),
            SplitDirection::Up => (11, 81),
            SplitDirection::Down => (12, 81),
        }
    }

    fn rect(x: u16, y: u16, rows: u16, columns: u16) -> Rect {
        Rect {
            x,
            y,
            rows,
            columns,
        }
    }

    /// Run one split with fakes; `spawn_ok`/`resize_ok` inject known failures.
    fn split(
        fixture: &mut Fixture,
        payload: &[u8],
        spawn_ok: bool,
        resize_ok: bool,
    ) -> Result<SplitResult, SplitError> {
        let prepare_log = Rc::clone(&fixture.log);
        let spawn_log = Rc::clone(&fixture.log);
        let resize_log = Rc::clone(&fixture.log);
        fixture.owner.execute_split(
            payload,
            NOW,
            &mut fixture.journal,
            move |_, target| {
                prepare_log
                    .borrow_mut()
                    .push(Effect::Prepare(target.session));
                Ok::<_, ()>(target.session)
            },
            move |session| {
                spawn_log.borrow_mut().push(Effect::Spawn(session));
                if spawn_ok {
                    Ok(Sink {
                        session,
                        log: Rc::clone(&spawn_log),
                    })
                } else {
                    Err("PRIVATE-SPAWN")
                }
            },
            move |sink: &mut Sink, rows, columns| {
                resize_log.borrow_mut().push(Effect::Resize {
                    session: sink.session,
                    rows,
                    columns,
                });
                if resize_ok {
                    Ok(())
                } else {
                    Err("PRIVATE-RESIZE")
                }
            },
        )
    }

    fn no_effects_split(fixture: &mut Fixture, payload: &[u8]) -> Result<SplitResult, SplitError> {
        fixture.owner.execute_split(
            payload,
            NOW,
            &mut fixture.journal,
            |_, _| -> Result<u64, ()> { panic!("must not prepare") },
            |_| -> Result<Sink, ()> { panic!("must not spawn") },
            |_: &mut Sink, _, _| -> Result<(), ()> { panic!("must not resize") },
        )
    }

    fn ledger_status(fixture: &Fixture, sequence: u64) -> OperationStatus {
        fixture
            .owner
            .global
            .ledger
            .status(key(sequence), NOW)
            .unwrap()
    }

    fn assert_no_input(fixture: &Fixture) {
        assert!(!fixture
            .log
            .borrow()
            .iter()
            .any(|effect| matches!(effect, Effect::Input(_))));
    }

    #[test]
    fn layout_is_established_by_the_first_applied_launch() {
        let fixture = fixture();
        let snapshot = fixture.owner.layout_snapshot().unwrap();
        assert_eq!(snapshot.revision, 1);
        assert_eq!((snapshot.rows, snapshot.columns), (ROOT_ROWS, ROOT_COLUMNS));
        assert_eq!(snapshot.pane_count(), 1);
        assert_eq!(snapshot.panes[0].unwrap().target, fixture.first);
    }

    #[test]
    fn applied_split_in_each_direction_spawns_once_and_resizes_existing_once() {
        let cases = [
            (
                SplitDirection::Left,
                rect(41, 0, 24, 40),
                rect(0, 0, 24, 40),
            ),
            (
                SplitDirection::Right,
                rect(0, 0, 24, 40),
                rect(41, 0, 24, 40),
            ),
            (SplitDirection::Up, rect(0, 12, 12, 81), rect(0, 0, 11, 81)),
            (
                SplitDirection::Down,
                rect(0, 0, 11, 81),
                rect(0, 12, 12, 81),
            ),
        ];
        for (direction, existing_rect, new_rect) in cases {
            let mut fixture = fixture();
            let payload = split_bytes(
                key(2),
                fixture.first,
                1,
                direction,
                planned_size(direction),
                b"env",
            );
            let result = split(&mut fixture, &payload, true, true).unwrap();
            assert_eq!(result.outcome, Outcome::Applied);
            assert_eq!(result.committed_layout_revision, 2);
            assert_eq!(result.existing_target, fixture.first);
            let new_target = result.new_target.unwrap();
            assert_eq!(new_target.session, 2);
            assert_eq!(
                *fixture.log.borrow(),
                [
                    Effect::Prepare(2),
                    Effect::JournalPreflight,
                    Effect::Record(LaunchState::Intent),
                    Effect::Spawn(2),
                    Effect::Record(LaunchState::Spawned),
                    Effect::Record(LaunchState::Applied),
                    Effect::Resize {
                        session: 1,
                        rows: existing_rect.rows,
                        columns: existing_rect.columns,
                    },
                ],
                "{direction:?}"
            );
            let snapshot = fixture.owner.layout_snapshot().unwrap();
            assert_eq!(snapshot.revision, 2);
            let rect_of = |target| {
                snapshot
                    .panes
                    .into_iter()
                    .flatten()
                    .find(|pane| pane.target == target)
                    .unwrap()
                    .rect
            };
            assert_eq!(rect_of(fixture.first), existing_rect);
            assert_eq!(rect_of(new_target), new_rect);
            assert_eq!((new_rect.rows, new_rect.columns), planned_size(direction));
            assert_eq!(
                fixture.owner.launches.state(new_target),
                Ok(SessionState::Applied)
            );
            assert_eq!(
                ledger_status(&fixture, 2),
                OperationStatus::Completed(Outcome::Applied)
            );
            assert_no_input(&fixture);
        }
    }

    #[test]
    fn duplicate_after_commit_returns_original_result_despite_stale_revision() {
        let mut fixture = fixture();
        let payload = split_bytes(
            key(2),
            fixture.first,
            1,
            SplitDirection::Right,
            (24, 40),
            b"env",
        );
        let first = split(&mut fixture, &payload, true, true).unwrap();
        // Advance the layout so the original expected revision is stale.
        let resized = fixture
            .owner
            .resize_layout(2, 30, 101, |_: &mut Sink, _, _| Ok::<_, ()>(()))
            .unwrap();
        assert_eq!(resized.revision, 3);
        fixture.log.borrow_mut().clear();

        let retry = no_effects_split(&mut fixture, &payload).unwrap();
        assert_eq!(retry, first);
        let mut first_bytes = Vec::new();
        let mut retry_bytes = Vec::new();
        crate::split_protocol::encode_split_result(&first, &mut first_bytes).unwrap();
        crate::split_protocol::encode_split_result(&retry, &mut retry_bytes).unwrap();
        assert_eq!(first_bytes, retry_bytes);
        assert!(fixture.log.borrow().is_empty());
        assert_eq!(fixture.owner.launches.records().count(), 2);
        assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 3);
    }

    #[test]
    fn changed_payloads_conflict_before_effects_even_at_capacity() {
        let mut fixture = fixture();
        let original = split_bytes(
            key(2),
            fixture.first,
            1,
            SplitDirection::Right,
            (24, 40),
            b"env",
        );
        split(&mut fixture, &original, true, true).unwrap();
        assert!(matches!(
            fixture.owner.launches.next_target(),
            Err(RegistryError::Capacity)
        ));
        fixture.log.borrow_mut().clear();
        let stale_target = TargetRef {
            occupant_revision: 2,
            ..fixture.first
        };
        let changed = [
            split_bytes(
                key(2),
                fixture.first,
                1,
                SplitDirection::Left,
                (24, 40),
                b"env",
            ),
            split_bytes(
                key(2),
                stale_target,
                1,
                SplitDirection::Right,
                (24, 40),
                b"env",
            ),
            split_bytes(
                key(2),
                fixture.first,
                2,
                SplitDirection::Right,
                (24, 40),
                b"env",
            ),
            split_bytes(
                key(2),
                fixture.first,
                1,
                SplitDirection::Right,
                (24, 40),
                b"enw",
            ),
        ];
        for payload in changed {
            assert_ne!(payload, original);
            let error = no_effects_split(&mut fixture, &payload).unwrap_err();
            assert_eq!(
                error.ledger_error(),
                Some(&ProtocolError::OperationConflict)
            );
        }
        assert!(fixture.log.borrow().is_empty());
        assert_eq!(
            ledger_status(&fixture, 2),
            OperationStatus::Completed(Outcome::Applied)
        );
    }

    #[test]
    fn rejected_new_splits_record_failed_without_ids_layout_or_effects() {
        let mut fixture = fixture();
        let stale_target = TargetRef {
            occupant_revision: 9,
            ..fixture.first
        };
        let cases = [
            (
                split_bytes(
                    key(2),
                    fixture.first,
                    7,
                    SplitDirection::Right,
                    (24, 40),
                    b"env",
                ),
                SplitFailure::StaleLayout,
            ),
            (
                split_bytes(
                    key(3),
                    stale_target,
                    1,
                    SplitDirection::Right,
                    (24, 40),
                    b"env",
                ),
                SplitFailure::StaleTarget,
            ),
            (
                split_bytes(
                    key(4),
                    fixture.first,
                    1,
                    SplitDirection::Right,
                    (24, 41),
                    b"env",
                ),
                SplitFailure::LaunchSizeMismatch,
            ),
        ];
        for (payload, expected) in cases {
            let next_before = fixture.owner.launches.next_target();
            let error = no_effects_split(&mut fixture, &payload).unwrap_err();
            let (reason, result) = error.failure().unwrap();
            assert_eq!(reason, expected);
            assert_eq!(result.outcome, Outcome::Failed);
            assert_eq!(result.committed_layout_revision, 0);
            assert_eq!(result.new_target, None);
            assert_eq!(
                ledger_status(&fixture, result.key.sequence),
                OperationStatus::Completed(Outcome::Failed)
            );
            assert_eq!(fixture.owner.launches.next_target(), next_before);
            assert_eq!(fixture.owner.launches.records().count(), 1);
            assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 1);
            assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 1);
            // The exact retry reports the same recorded result, still with no effects.
            let retry = no_effects_split(&mut fixture, &payload).unwrap_err();
            assert_eq!(retry.failure().unwrap().1, result);
        }
        assert!(fixture.log.borrow().is_empty());

        // A geometry rejection from the layout is also Failed before effects.
        let mut narrow = narrow_fixture();
        let payload = split_bytes(
            key(2),
            narrow.first,
            1,
            SplitDirection::Left,
            (24, 20),
            b"env",
        );
        let error = no_effects_split(&mut narrow, &payload).unwrap_err();
        assert_eq!(
            error.failure().unwrap().0,
            SplitFailure::Layout(LayoutError::LayoutTooSmall)
        );
        assert_eq!(narrow.owner.launches.records().count(), 1);
    }

    fn narrow_fixture() -> Fixture {
        let mut ledger = Ledger::new(GENERATION).unwrap();
        ledger.open_lease(LEASE, 1).unwrap();
        let mut owner = MultiSessionOwner::with_ledger(ledger, true).unwrap();
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut journal = Journal {
            log: Rc::clone(&log),
            preflight_failure: false,
            record_failure: None,
        };
        let spawn_log = Rc::clone(&log);
        let result = owner
            .execute_launch(
                &launch_bytes(key(1), 24, 40, b"narrow"),
                NOW,
                &mut journal,
                |_, target| Ok::<_, ()>(target.session),
                move |session| {
                    Ok::<_, ()>(Sink {
                        session,
                        log: spawn_log,
                    })
                },
            )
            .unwrap();
        log.borrow_mut().clear();
        Fixture {
            owner,
            log,
            journal,
            first: result.target.unwrap(),
        }
    }

    #[test]
    fn third_session_and_missing_layout_fail_before_effects() {
        let mut fixture = fixture();
        let spawn_log = Rc::clone(&fixture.log);
        let second = fixture
            .owner
            .execute_launch(
                &launch_bytes(key(2), 10, 40, b"second"),
                NOW,
                &mut fixture.journal,
                |_, target| Ok::<_, ()>(target.session),
                move |session| {
                    Ok::<_, ()>(Sink {
                        session,
                        log: spawn_log,
                    })
                },
            )
            .unwrap();
        assert_eq!(second.outcome, Outcome::Applied);
        // The layout belongs to the first launch only.
        assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 1);
        fixture.log.borrow_mut().clear();
        let payload = split_bytes(
            key(3),
            fixture.first,
            1,
            SplitDirection::Right,
            (24, 40),
            b"env",
        );
        let error = no_effects_split(&mut fixture, &payload).unwrap_err();
        assert_eq!(error.failure().unwrap().0, SplitFailure::Capacity);
        assert_eq!(error.to_string(), "pane or session capacity is exhausted");
        assert_eq!(fixture.owner.launches.records().count(), 2);
        assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 1);
        assert!(fixture.log.borrow().is_empty());

        let mut ledger = Ledger::new(GENERATION).unwrap();
        ledger.open_lease(LEASE, 1).unwrap();
        let owner = MultiSessionOwner::<Sink>::with_ledger(ledger, true).unwrap();
        let mut empty = Fixture {
            owner,
            log: Rc::new(RefCell::new(Vec::new())),
            journal: Journal {
                log: Rc::new(RefCell::new(Vec::new())),
                preflight_failure: false,
                record_failure: None,
            },
            first: fixture.first,
        };
        assert!(empty.owner.layout_snapshot().is_none());
        let error = no_effects_split(&mut empty, &payload).unwrap_err();
        assert_eq!(error.failure().unwrap().0, SplitFailure::NoLayout);
        assert_eq!(empty.owner.launches.records().count(), 0);
    }

    #[test]
    fn malformed_and_wrong_generation_payloads_reject_before_ledger() {
        let mut fixture = fixture();
        let mut payload = split_bytes(
            key(2),
            fixture.first,
            1,
            SplitDirection::Right,
            (24, 40),
            b"env",
        );
        payload[80] = 1; // nonzero reserved byte
        assert!(matches!(
            no_effects_split(&mut fixture, &payload),
            Err(SplitError::InvalidPayload(_))
        ));
        assert_eq!(ledger_status(&fixture, 2), OperationStatus::NotAdmitted);
        let wrong = OperationKey {
            generation: GENERATION + 1,
            ..key(2)
        };
        let wrong_target = TargetRef {
            generation: GENERATION + 1,
            ..fixture.first
        };
        let payload = split_bytes(
            wrong,
            wrong_target,
            1,
            SplitDirection::Right,
            (24, 40),
            b"env",
        );
        assert!(matches!(
            no_effects_split(&mut fixture, &payload),
            Err(SplitError::GenerationMismatch)
        ));
        assert_eq!(ledger_status(&fixture, 2), OperationStatus::NotAdmitted);
    }

    #[test]
    fn preparation_and_journal_preflight_failures_record_failed_without_reservation() {
        for preflight in [false, true] {
            let mut fixture = fixture();
            let payload = split_bytes(
                key(2),
                fixture.first,
                1,
                SplitDirection::Right,
                (24, 40),
                b"env",
            );
            fixture.journal.preflight_failure = preflight;
            let error = fixture
                .owner
                .execute_split(
                    &payload,
                    NOW,
                    &mut fixture.journal,
                    |_, target| {
                        if preflight {
                            Ok(target.session)
                        } else {
                            Err("PRIVATE-PREPARE")
                        }
                    },
                    |_| -> Result<Sink, ()> { panic!("must not spawn") },
                    |_: &mut Sink, _, _| -> Result<(), ()> { panic!("must not resize") },
                )
                .unwrap_err();
            let expected = if preflight {
                SplitFailure::JournalPreflightFailed
            } else {
                SplitFailure::PreparationFailed
            };
            assert_eq!(error.failure().unwrap().0, expected);
            assert_eq!(fixture.owner.launches.records().count(), 1);
            assert_eq!(fixture.owner.launches.next_target().unwrap().session, 2);
            assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 1);
            assert_eq!(
                ledger_status(&fixture, 2),
                OperationStatus::Completed(Outcome::Failed)
            );
            let rendered = format!("{error:?} {error} {:?}", fixture.owner);
            assert!(!rendered.contains("PRIVATE"));
        }
    }

    #[test]
    fn known_spawn_failure_before_commit_is_failed_and_keeps_layout() {
        let mut fixture = fixture();
        let payload = split_bytes(
            key(2),
            fixture.first,
            1,
            SplitDirection::Down,
            (12, 81),
            b"env",
        );
        let error = split(&mut fixture, &payload, false, true).unwrap_err();
        let (reason, result) = error.failure().unwrap();
        assert_eq!(reason, SplitFailure::LaunchFailed);
        assert_eq!(result.new_target, None);
        assert_eq!(
            *fixture.log.borrow(),
            [
                Effect::Prepare(2),
                Effect::JournalPreflight,
                Effect::Record(LaunchState::Intent),
                Effect::Spawn(2),
                Effect::Record(LaunchState::Failed),
            ]
        );
        // The identity is spent by the known launch failure, as for launches.
        let spent = fixture.owner.launches.records().nth(1).unwrap();
        assert_eq!(spent.state, SessionState::Failed);
        assert!(!spent.has_effects);
        let snapshot = fixture.owner.layout_snapshot().unwrap();
        assert_eq!((snapshot.revision, snapshot.pane_count()), (1, 1));
        assert_eq!(
            ledger_status(&fixture, 2),
            OperationStatus::Completed(Outcome::Failed)
        );
        fixture.log.borrow_mut().clear();
        let retry = no_effects_split(&mut fixture, &payload).unwrap_err();
        assert_eq!(retry.failure().unwrap().1, result);
    }

    #[test]
    fn post_spawn_journal_fault_commits_unknown_retains_child_and_never_respawns() {
        for failed_state in [LaunchState::Spawned, LaunchState::Applied] {
            let mut fixture = fixture();
            fixture.journal.record_failure = Some(failed_state);
            let payload = split_bytes(
                key(2),
                fixture.first,
                1,
                SplitDirection::Right,
                (24, 40),
                b"env",
            );
            let result = split(&mut fixture, &payload, true, true).unwrap();
            assert_eq!(result.outcome, Outcome::Unknown);
            assert_eq!(result.committed_layout_revision, 2);
            let new_target = result.new_target.unwrap();
            assert_eq!(
                fixture.owner.launches.state(new_target),
                Ok(SessionState::Unknown)
            );
            assert!(fixture.owner.launches.effects(new_target).is_ok());
            assert!(fixture.owner.launches.effects(fixture.first).is_ok());
            assert_eq!(fixture.owner.layout_snapshot().unwrap().pane_count(), 2);
            assert!(fixture.log.borrow().contains(&Effect::Resize {
                session: 1,
                rows: 24,
                columns: 40
            }));
            assert_eq!(
                ledger_status(&fixture, 2),
                OperationStatus::Completed(Outcome::Unknown)
            );
            fixture.log.borrow_mut().clear();
            let retry = no_effects_split(&mut fixture, &payload).unwrap();
            assert_eq!(retry, result);
            assert!(fixture.log.borrow().is_empty());
            assert_no_input(&fixture);
        }
    }

    #[test]
    fn existing_session_resize_failure_is_partial_and_layout_stays_committed() {
        let mut fixture = fixture();
        let payload = split_bytes(
            key(2),
            fixture.first,
            1,
            SplitDirection::Up,
            (11, 81),
            b"env",
        );
        let result = split(&mut fixture, &payload, true, false).unwrap();
        assert_eq!(result.outcome, Outcome::Partial);
        assert_eq!(result.committed_layout_revision, 2);
        let new_target = result.new_target.unwrap();
        assert_eq!(
            fixture.owner.launches.state(new_target),
            Ok(SessionState::Applied)
        );
        assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 2);
        assert_eq!(
            ledger_status(&fixture, 2),
            OperationStatus::Completed(Outcome::Partial)
        );
        fixture.log.borrow_mut().clear();
        assert_eq!(no_effects_split(&mut fixture, &payload).unwrap(), result);
        assert!(fixture.log.borrow().is_empty());

        // The existing pane is pending at its desired split rectangle; the
        // new pane was spawned at its planned size and is not pending.
        let pending = fixture.owner.pending_resizes();
        assert_eq!(pending.iter().flatten().count(), 1);
        let entry = pending.into_iter().flatten().next().unwrap();
        assert_eq!(entry.target, fixture.first);
        assert_eq!(entry.desired, (12, 81));
        assert_eq!(entry.confirmed, (ROOT_ROWS, ROOT_COLUMNS));
        assert!(!format!("{entry:?}").contains(&GENERATION.to_string()));
        assert_no_input(&fixture);
    }

    fn recording_resize(
        log: &Log,
        fail: bool,
    ) -> impl FnMut(&mut Sink, u16, u16) -> Result<(), ()> {
        let log = Rc::clone(log);
        move |sink: &mut Sink, rows, columns| {
            log.borrow_mut().push(Effect::Resize {
                session: sink.session,
                rows,
                columns,
            });
            if fail {
                Err(())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn same_size_resize_layout_retries_only_pending_split_resize_without_new_revision() {
        let mut fixture = fixture();
        let payload = split_bytes(
            key(2),
            fixture.first,
            1,
            SplitDirection::Up,
            (11, 81),
            b"env",
        );
        assert_eq!(
            split(&mut fixture, &payload, true, false).unwrap().outcome,
            Outcome::Partial
        );
        fixture.log.borrow_mut().clear();

        // Still failing: ResizePending again, same revision, only the pending pane.
        let error = fixture
            .owner
            .resize_layout(
                2,
                ROOT_ROWS,
                ROOT_COLUMNS,
                recording_resize(&fixture.log, true),
            )
            .unwrap_err();
        match error {
            LayoutResizeError::ResizePending { snapshot, failed } => {
                assert_eq!(snapshot.revision, 2);
                // Up places the existing pane second (bottom).
                assert_eq!(failed, [false, true]);
            }
            other => panic!("expected pending resize, got {other:?}"),
        }
        assert_eq!(
            *fixture.log.borrow(),
            [Effect::Resize {
                session: 1,
                rows: 12,
                columns: 81
            }]
        );
        assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 2);
        assert_eq!(fixture.owner.pending_resizes().iter().flatten().count(), 1);
        fixture.log.borrow_mut().clear();

        // Succeeding: Ok, same revision, pending cleared, exactly one call.
        let snapshot = fixture
            .owner
            .resize_layout(
                2,
                ROOT_ROWS,
                ROOT_COLUMNS,
                recording_resize(&fixture.log, false),
            )
            .unwrap();
        assert_eq!(snapshot.revision, 2);
        assert_eq!(
            *fixture.log.borrow(),
            [Effect::Resize {
                session: 1,
                rows: 12,
                columns: 81
            }]
        );
        assert_eq!(fixture.owner.pending_resizes(), [None, None]);
        fixture.log.borrow_mut().clear();

        // Nothing pending: a same-size call has no PTY effect.
        fixture
            .owner
            .resize_layout(
                2,
                ROOT_ROWS,
                ROOT_COLUMNS,
                recording_resize(&fixture.log, false),
            )
            .unwrap();
        assert!(fixture.log.borrow().is_empty());
        assert_no_input(&fixture);
    }

    #[test]
    fn same_size_resize_layout_retries_failed_root_resize_panes_only() {
        let mut fixture = fixture();
        let payload = split_bytes(
            key(2),
            fixture.first,
            1,
            SplitDirection::Right,
            (24, 40),
            b"env",
        );
        split(&mut fixture, &payload, true, true).unwrap();
        fixture.log.borrow_mut().clear();
        let log = Rc::clone(&fixture.log);
        let error = fixture
            .owner
            .resize_layout(2, 30, 101, move |sink: &mut Sink, rows, columns| {
                log.borrow_mut().push(Effect::Resize {
                    session: sink.session,
                    rows,
                    columns,
                });
                if sink.session == 2 {
                    Err(())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert!(matches!(
            error,
            LayoutResizeError::ResizePending { ref snapshot, failed }
                if snapshot.revision == 3 && failed == [false, true]
        ));
        assert_eq!(fixture.log.borrow().len(), 2);
        let pending = fixture.owner.pending_resizes();
        let entry = pending.into_iter().flatten().next().unwrap();
        assert_eq!(pending.iter().flatten().count(), 1);
        assert_eq!(entry.target.session, 2);
        assert_eq!((entry.desired, entry.confirmed), ((30, 50), (24, 40)));
        fixture.log.borrow_mut().clear();

        let error = fixture
            .owner
            .resize_layout(3, 30, 101, recording_resize(&fixture.log, true))
            .unwrap_err();
        assert!(matches!(
            error,
            LayoutResizeError::ResizePending { ref snapshot, failed }
                if snapshot.revision == 3 && failed == [false, true]
        ));
        let only_second = [Effect::Resize {
            session: 2,
            rows: 30,
            columns: 50,
        }];
        assert_eq!(*fixture.log.borrow(), only_second);
        fixture.log.borrow_mut().clear();

        let snapshot = fixture
            .owner
            .resize_layout(3, 30, 101, recording_resize(&fixture.log, false))
            .unwrap();
        assert_eq!(snapshot.revision, 3);
        assert_eq!(*fixture.log.borrow(), only_second);
        assert_eq!(fixture.owner.pending_resizes(), [None, None]);
        assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 3);
        assert_no_input(&fixture);
    }

    #[test]
    fn resize_layout_applies_absolute_sizes_rejects_small_and_reports_pending() {
        let mut fixture = fixture();
        let payload = split_bytes(
            key(2),
            fixture.first,
            1,
            SplitDirection::Right,
            (24, 40),
            b"env",
        );
        split(&mut fixture, &payload, true, true).unwrap();
        fixture.log.borrow_mut().clear();
        let log = Rc::clone(&fixture.log);
        let mut record = move |sink: &mut Sink, rows: u16, columns: u16| {
            log.borrow_mut().push(Effect::Resize {
                session: sink.session,
                rows,
                columns,
            });
            Ok::<_, ()>(())
        };

        // Same size: no revision change and no PTY effect.
        let same = fixture.owner.resize_layout(2, 24, 81, &mut record).unwrap();
        assert_eq!(same.revision, 2);
        assert!(fixture.log.borrow().is_empty());

        // Too small for two 20-column panes, and stale: no change, no effect.
        assert!(matches!(
            fixture.owner.resize_layout(2, 24, 40, &mut record),
            Err(LayoutResizeError::Layout(LayoutError::LayoutTooSmall))
        ));
        assert!(matches!(
            fixture.owner.resize_layout(1, 30, 101, &mut record),
            Err(LayoutResizeError::Layout(LayoutError::StaleRevision))
        ));
        assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 2);
        assert!(fixture.log.borrow().is_empty());

        let grown = fixture
            .owner
            .resize_layout(2, 30, 101, &mut record)
            .unwrap();
        assert_eq!(grown.revision, 3);
        assert_eq!(
            *fixture.log.borrow(),
            [
                Effect::Resize {
                    session: 1,
                    rows: 30,
                    columns: 50
                },
                Effect::Resize {
                    session: 2,
                    rows: 30,
                    columns: 50
                },
            ]
        );

        let error = fixture
            .owner
            .resize_layout(3, 20, 61, |sink: &mut Sink, _, _| {
                if sink.session == 2 {
                    Err("PRIVATE-RESIZE")
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        match error {
            LayoutResizeError::ResizePending { snapshot, failed } => {
                assert_eq!(snapshot.revision, 4);
                assert_eq!(failed, [false, true]);
            }
            other => panic!("expected pending resize, got {other:?}"),
        }
        assert_eq!(fixture.owner.layout_snapshot().unwrap().revision, 4);
        assert_no_input(&fixture);
    }
}

#[cfg(test)]
mod runtime_helper_tests {
    //! Deterministic coverage of the owner helpers used by the multi-session
    //! runtime: typed-key input, root resize, split authorization, scoped
    //! observation and launch status.
    use super::{MultiSessionOwner, RootResizeReply};
    use crate::control_protocol::{Command, ReadSource, TargetRef};
    use crate::handshake_protocol::{Hello, CAPABILITY_INPUT, CAPABILITY_READ};
    use crate::managed_launch::LaunchJournalPort;
    use crate::managed_launch_protocol::{encode_launch_request, LaunchRequest};
    use crate::pane_layout::SplitDirection;
    use crate::protocol::{Ledger, OperationKey, OperationStatus, Outcome};
    use crate::response_protocol::{decode_response_payload, RejectionCode, ResponseBody};
    use crate::server::{Connection, EffectSink, ReadResult, ServerError, SessionReply};
    use crate::server_storage::LaunchRecord;
    use crate::session_protocol::{encode_session_request, SessionHello, SessionRequest};
    use crate::split_protocol::{decode_split_request, encode_split_request, SplitRequest};
    use crate::terminal::Key;
    use crate::ui_protocol::{
        decode_ui_request, encode_key_input, KeyInput, RootResize, UiRequest,
    };
    use std::io;

    const GENERATION: u64 = 91;
    const INTERNAL: u64 = 1;
    const NOW: u64 = 5;

    #[derive(Default)]
    struct Sink {
        delivered: Vec<Key>,
        resizes: Vec<(u16, u16)>,
        fail_resize: bool,
    }

    impl EffectSink for Sink {
        fn validate(&self, _: Command<'_>) -> Result<(), RejectionCode> {
            Ok(())
        }
        fn read(&mut self, _: ReadSource, _: usize, _: u16) -> Result<ReadResult, Outcome> {
            Err(Outcome::Failed)
        }
        fn input(&mut self, _: Command<'_>) -> Outcome {
            Outcome::Applied
        }
        fn poll(&mut self) -> Result<(), Outcome> {
            Ok(())
        }
        fn child_exited(&self) -> bool {
            false
        }
        fn screen_revision(&self) -> Option<u64> {
            Some(1)
        }
    }

    struct Journal;

    impl LaunchJournalPort for Journal {
        fn preflight_launch(&self, _: OperationKey, _: u64) -> io::Result<()> {
            Ok(())
        }
        fn record(&mut self, _: LaunchRecord) -> io::Result<()> {
            Ok(())
        }
    }

    fn launch(key: OperationKey, rows: u16, columns: u16) -> Vec<u8> {
        let mut bytes = Vec::new();
        encode_launch_request(
            &LaunchRequest {
                key,
                rows,
                columns,
                requested_executable: b"/bin/sh",
                requested_working_directory: b"/",
                client_working_directory: b"/",
                arguments: &[],
                environment: &[],
            },
            &mut bytes,
        )
        .unwrap();
        bytes
    }

    fn owner_with_first(allow_input: bool) -> (MultiSessionOwner<Sink>, TargetRef) {
        let mut ledger = Ledger::new(GENERATION).unwrap();
        ledger.open_lease(INTERNAL, 0).unwrap();
        let mut owner = MultiSessionOwner::with_ledger(ledger, allow_input).unwrap();
        let key = OperationKey {
            generation: GENERATION,
            lease: INTERNAL,
            sequence: 1,
        };
        let result = owner
            .execute_launch(
                &launch(key, 24, 81),
                NOW,
                &mut Journal,
                |_, _| Ok::<_, ()>(()),
                |()| Ok::<_, ()>(Sink::default()),
            )
            .unwrap();
        (owner, result.target.unwrap())
    }

    fn connect(owner: &mut MultiSessionOwner<Sink>, target: TargetRef, nonce: u64) -> Connection {
        let hello = SessionHello {
            hello: Hello {
                nonce,
                required_capabilities: CAPABILITY_READ,
                optional_capabilities: CAPABILITY_INPUT,
            },
            resume: None,
        };
        owner.connect(target, &hello, NOW).unwrap().0
    }

    fn key(connection: &Connection, sequence: u64) -> OperationKey {
        OperationKey {
            generation: GENERATION,
            lease: connection.lease(),
            sequence,
        }
    }

    fn claim(owner: &mut MultiSessionOwner<Sink>, connection: &Connection, target: TargetRef) {
        let payload = encode_session_request(&SessionRequest::Claim {
            key: key(connection, 1),
            target,
            expected_owner_revision: 1,
            ttl_seconds: 30,
        })
        .unwrap();
        let SessionReply::Payload(reply) = owner.handle_session(connection, &payload, NOW).unwrap()
        else {
            panic!("claim reply");
        };
        assert_eq!(
            decode_response_payload(&reply).unwrap().body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
    }

    fn keys(
        owner: &mut MultiSessionOwner<Sink>,
        connection: &Connection,
        payload: &[u8],
    ) -> ResponseBody<'static> {
        let UiRequest::KeyInput(view) = decode_ui_request(payload).unwrap() else {
            panic!("key input");
        };
        let bytes = owner
            .handle_key_input(
                connection,
                &view,
                payload,
                NOW,
                |_| Ok(()),
                |sink| {
                    sink.delivered.extend(view.keys());
                    Outcome::Applied
                },
            )
            .unwrap();
        match decode_response_payload(&bytes).unwrap().body {
            ResponseBody::Status(status) => ResponseBody::Status(status),
            ResponseBody::Rejected(code) => ResponseBody::Rejected(code),
            ResponseBody::Read { .. } => panic!("unexpected read"),
        }
    }

    fn key_payload(key: OperationKey, target: TargetRef, revision: u64) -> Vec<u8> {
        encode_key_input(&KeyInput {
            key,
            target,
            expected_owner_revision: revision,
            keys: &[Key::Char('a'), Key::Enter],
        })
        .unwrap()
    }

    #[test]
    fn key_input_requires_input_capability_ownership_and_never_replays() {
        let (mut owner, first) = owner_with_first(true);
        let connection = connect(&mut owner, first, 1);
        // No ownership yet: rejected before admission, nothing delivered.
        let early = key_payload(key(&connection, 2), first, 1);
        assert_eq!(
            keys(&mut owner, &connection, &early),
            ResponseBody::Rejected(RejectionCode::Unauthorized)
        );
        claim(&mut owner, &connection, first);
        let payload = key_payload(key(&connection, 3), first, 2);
        assert_eq!(
            keys(&mut owner, &connection, &payload),
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        // Exact duplicate returns the recorded status without re-delivery.
        assert_eq!(
            keys(&mut owner, &connection, &payload),
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        let delivered = owner
            .launches
            .effects(first)
            .unwrap()
            .effects
            .delivered
            .clone();
        assert_eq!(delivered, vec![Key::Char('a'), Key::Enter]);
        // Another lease's key is unauthorized.
        let foreign = key_payload(
            OperationKey {
                lease: connection.lease() + 7,
                ..key(&connection, 4)
            },
            first,
            2,
        );
        assert_eq!(
            keys(&mut owner, &connection, &foreign),
            ResponseBody::Rejected(RejectionCode::Unauthorized)
        );
    }

    #[test]
    fn key_input_without_server_input_or_outside_pinned_scope_is_rejected() {
        let (mut owner, first) = owner_with_first(false);
        let read_only = connect(&mut owner, first, 1);
        assert_eq!(read_only.selected_capabilities() & CAPABILITY_INPUT, 0);
        let payload = key_payload(key(&read_only, 1), first, 1);
        assert_eq!(
            keys(&mut owner, &read_only, &payload),
            ResponseBody::Rejected(RejectionCode::Unauthorized)
        );

        let (mut owner, first) = owner_with_first(true);
        let connection = connect(&mut owner, first, 1);
        let other = TargetRef {
            pane: 9,
            session: 9,
            ..first
        };
        let payload = key_payload(key(&connection, 1), other, 1);
        assert_eq!(
            keys(&mut owner, &connection, &payload),
            ResponseBody::Rejected(RejectionCode::Unauthorized)
        );
        assert!(owner
            .launches
            .effects(first)
            .unwrap()
            .effects
            .delivered
            .is_empty());
    }

    #[test]
    fn root_resize_records_outcomes_and_duplicates_do_not_resize_again() {
        let (mut owner, first) = owner_with_first(true);
        let connection = connect(&mut owner, first, 1);
        let request = RootResize {
            key: key(&connection, 1),
            expected_layout_revision: 1,
            rows: 30,
            columns: 100,
        };
        let payload = crate::ui_protocol::encode_root_resize(&request).unwrap();
        let resize = |sink: &mut Sink, rows, columns| {
            sink.resizes.push((rows, columns));
            if sink.fail_resize {
                Err(())
            } else {
                Ok(())
            }
        };
        let RootResizeReply::Resized { snapshot, pending } = owner
            .handle_root_resize(&connection, request, &payload, NOW, resize)
            .unwrap()
        else {
            panic!("resized");
        };
        assert_eq!(
            (snapshot.revision, snapshot.rows, snapshot.columns),
            (2, 30, 100)
        );
        assert_eq!(pending, [false, false]);
        let RootResizeReply::Response(duplicate) = owner
            .handle_root_resize(&connection, request, &payload, NOW, resize)
            .unwrap()
        else {
            panic!("duplicate");
        };
        assert_eq!(
            decode_response_payload(&duplicate).unwrap().body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Applied))
        );
        assert_eq!(
            owner.launches.effects(first).unwrap().effects.resizes,
            vec![(30, 100)]
        );

        // Stale revision: Failed, no PTY effect.
        let stale = RootResize {
            key: key(&connection, 2),
            ..request
        };
        let payload = crate::ui_protocol::encode_root_resize(&stale).unwrap();
        assert!(matches!(
            owner
                .handle_root_resize(&connection, stale, &payload, NOW, resize)
                .unwrap(),
            RootResizeReply::LayoutRejected(_)
        ));
        assert_eq!(
            owner.launches.effects(first).unwrap().effects.resizes.len(),
            1
        );

        // A failing PTY resize is Partial with the pane marked pending.
        owner
            .launches
            .session_mut(first)
            .unwrap()
            .effects
            .fail_resize = true;
        let grow = RootResize {
            key: key(&connection, 3),
            expected_layout_revision: 2,
            rows: 31,
            columns: 100,
        };
        let payload = crate::ui_protocol::encode_root_resize(&grow).unwrap();
        let RootResizeReply::Resized { pending, .. } = owner
            .handle_root_resize(&connection, grow, &payload, NOW, resize)
            .unwrap()
        else {
            panic!("pending");
        };
        assert_eq!(pending, [true, false]);
        let RootResizeReply::Response(again) = owner
            .handle_root_resize(&connection, grow, &payload, NOW, resize)
            .unwrap()
        else {
            panic!("duplicate");
        };
        assert_eq!(
            decode_response_payload(&again).unwrap().body,
            ResponseBody::Status(OperationStatus::Completed(Outcome::Partial))
        );
    }

    #[test]
    fn root_resize_and_split_require_input_capability_and_own_lease() {
        let (mut owner, first) = owner_with_first(false);
        let connection = connect(&mut owner, first, 1);
        let request = RootResize {
            key: key(&connection, 1),
            expected_layout_revision: 1,
            rows: 30,
            columns: 100,
        };
        let payload = crate::ui_protocol::encode_root_resize(&request).unwrap();
        let RootResizeReply::Response(bytes) = owner
            .handle_root_resize(&connection, request, &payload, NOW, |_: &mut Sink, _, _| {
                Ok::<_, ()>(())
            })
            .unwrap()
        else {
            panic!("rejected");
        };
        assert_eq!(
            decode_response_payload(&bytes).unwrap().body,
            ResponseBody::Rejected(RejectionCode::Unauthorized)
        );
        assert_eq!(owner.layout_snapshot().unwrap().revision, 1);

        let launch_request = LaunchRequest {
            key: key(&connection, 2),
            rows: 24,
            columns: 40,
            requested_executable: b"/bin/zsh",
            requested_working_directory: b"/",
            client_working_directory: b"/",
            arguments: &[],
            environment: &[],
        };
        let mut split = Vec::new();
        encode_split_request(
            &SplitRequest {
                key: key(&connection, 2),
                target: first,
                expected_layout_revision: 1,
                direction: SplitDirection::Right,
                launch: &launch_request,
            },
            &mut split,
        )
        .unwrap();
        let view = decode_split_request(&split).unwrap();
        assert_eq!(
            owner.authorize_split(&connection, &view, NOW),
            Err(ServerError::Unauthorized)
        );

        let (mut owner, first) = owner_with_first(true);
        let connection = connect(&mut owner, first, 1);
        let view = decode_split_request(&split).unwrap();
        // Both owners issue the same first client lease, so the bytes apply.
        assert_eq!(view.key().lease, connection.lease());
        assert_eq!(owner.authorize_split(&connection, &view, NOW), Ok(()));
        let other = TargetRef {
            pane: 5,
            session: 5,
            ..first
        };
        let mut scoped = Vec::new();
        encode_split_request(
            &SplitRequest {
                key: key(&connection, 2),
                target: other,
                expected_layout_revision: 1,
                direction: SplitDirection::Right,
                launch: &launch_request,
            },
            &mut scoped,
        )
        .unwrap();
        assert_eq!(
            owner.authorize_split(&connection, &decode_split_request(&scoped).unwrap(), NOW),
            Err(ServerError::Unauthorized)
        );
    }

    #[test]
    fn observation_is_scoped_and_layout_is_shared_with_read_connections() {
        let (mut owner, first) = owner_with_first(false);
        let connection = connect(&mut owner, first, 1);
        assert_eq!(
            owner
                .observe_target(&connection, first, NOW, |sink| sink.delivered.len())
                .unwrap(),
            0
        );
        let other = TargetRef {
            pane: 4,
            session: 4,
            ..first
        };
        assert_eq!(
            owner
                .observe_target(&connection, other, NOW, |_| ())
                .unwrap_err(),
            ServerError::Unauthorized
        );
        let (layout, pending) = owner.layout_view(&connection, NOW).unwrap().unwrap();
        assert_eq!(layout.panes[0].unwrap().target, first);
        assert_eq!(pending, [None, None]);
        assert_eq!(owner.first_pane_target(), Some(first));
        assert_eq!(owner.lease_target(connection.lease()), Some(first));
        assert_eq!(
            owner.lease_target(INTERNAL),
            None,
            "internal lease has no grant"
        );
        // Launch status for another lease's key is unauthorized.
        assert_eq!(
            owner
                .launch_status(
                    &connection,
                    OperationKey {
                        generation: GENERATION,
                        lease: INTERNAL,
                        sequence: 1,
                    },
                    NOW,
                )
                .unwrap_err(),
            ServerError::Unauthorized
        );
    }
}
