//! Owns workspace-tab catalog, picker/dialog state and metadata request identity.
//! Process and storage work remains in the existing route/metadata adapters.
use super::super::routes::{MetadataRequestId, MetadataResult};
use super::{
    EmptyWorkspaceGeometry, WorkspaceMetadataMutation, EMPTY_ACTIONS, TAB_FILTER_MAX_BYTES,
};
use crate::workspace_lifecycle::{
    CreateIntentId, CreateShellTab, CreateStatus, ExpectedRuntimeIdentity, LifecycleError,
    RenameOutcome,
};
use crossterm_input::{Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use panelink::pane_layout::Rect;
use panelink::workspace_index::{
    ProjectId, TabPresentationSnapshot, WorkspaceIndex, MAX_DISPLAY_NAME_BYTES,
    MAX_TABS_PER_PROJECT,
};
use panelink::workspace_tabs::{self, TabNavigation, TabsLayout, WorkspaceRouteId};
use std::time::Instant;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum TabDialog {
    Actions {
        route: WorkspaceRouteId,
        selected: usize,
    },
    Rename {
        route: WorkspaceRouteId,
        expected_revision: u64,
        draft: String,
        error: Option<&'static str>,
        saving: bool,
    },
    Close {
        expected: ExpectedRuntimeIdentity,
        expected_revision: u64,
        close_selected: bool,
        pending: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PendingCreate {
    pub(super) project: ProjectId,
    pub(super) source_route: Option<WorkspaceRouteId>,
    pub(super) request_id: Option<MetadataRequestId>,
    pub(super) intent_id: Option<CreateIntentId>,
    pub(super) dispatched: bool,
    /// Outcome unknown: the intent is kept, and only an explicit New tab
    /// re-dispatches it (same intent ID) for reconciliation.
    pub(super) awaiting_reconcile: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PendingMetadata {
    pub(super) request_id: MetadataRequestId,
    pub(super) route: WorkspaceRouteId,
    pub(super) source_route: Option<WorkspaceRouteId>,
    pub(super) expected_revision: u64,
}

#[derive(Default)]
pub(super) struct WorkspaceFeedback {
    pub dirty: bool,
    pub cancel_selection: bool,
    pub clear_prefix: bool,
    pub clear_actions: bool,
    pub clear_applying: bool,
    pub status: Option<(&'static str, Instant)>,
    pub persistent: Option<(&'static str, Instant)>,
    pub begin_switch: Option<Instant>,
}
#[derive(Default)]
pub(super) struct WorkspaceState {
    pub(super) workspace_index: Option<WorkspaceIndex>,
    pub(super) current_route: Option<WorkspaceRouteId>,
    pub(super) tab_dialog: Option<TabDialog>,
    pub(super) pending_tab_mutation: Option<WorkspaceMetadataMutation>,
    pub(super) picker_selection: Option<WorkspaceRouteId>,
    pub(super) picker_open: bool,
    pub(super) picker_filter: String,
    pub(super) catalog_refresh_requested: bool,
    pub(super) route_request: Option<WorkspaceRouteId>,
    pub(super) route_switching: bool,
    pub(super) route_switch_notice: Option<&'static str>,
    pub(super) workspace_revision: Option<u64>,
    pub(super) pending_create: Option<PendingCreate>,
    /// A partial/unknown tab close keeps this client open on server end.
    pub(super) keep_tab_visible_on_server_end: bool,
    pub(super) pending_metadata: Option<PendingMetadata>,
    feedback: WorkspaceFeedback,
}
impl WorkspaceState {
    pub(super) const fn view(&self) -> &Self {
        self
    }
    pub(super) fn take_feedback(&mut self) -> WorkspaceFeedback {
        std::mem::take(&mut self.feedback)
    }
    fn set_status(&mut self, label: &'static str, now: Instant) {
        self.feedback.status = Some((label, now));
        self.feedback.dirty = true;
    }
    fn set_persistent_status(&mut self, label: &'static str, now: Instant) {
        self.feedback.persistent = Some((label, now));
        self.feedback.dirty = true;
    }
    fn clear_applying_status(&mut self) {
        self.feedback.clear_applying = true;
    }
    fn cancel_selection(&mut self) {
        self.feedback.cancel_selection = true;
    }
    fn begin_route_switch(&mut self, now: Instant) {
        self.feedback.begin_switch = Some(now);
    }
    pub(super) fn freeze(&mut self, notice: &'static str) {
        self.route_switching = true;
        self.picker_selection = None;
        self.picker_open = false;
        self.picker_filter.clear();
        self.tab_dialog = None;
        self.route_switch_notice = Some(notice);
    }
}

impl WorkspaceState {
    pub(super) fn install_workspace_tabs(
        &mut self,
        snapshot: TabPresentationSnapshot,
        current: Option<WorkspaceRouteId>,
    ) -> bool {
        let Some(index) = presentation_index(&snapshot, current) else {
            return false;
        };
        if current.is_some_and(|route| workspace_tabs::layout(&index, route, 0, 0).is_err()) {
            return false;
        }
        if self.current_route != current {
            self.cancel_selection();
        }
        self.workspace_index = Some(index);
        self.current_route = current;
        self.tab_dialog = None;
        self.pending_tab_mutation = None;
        self.picker_selection = None;
        self.picker_open = false;
        self.picker_filter.clear();
        self.feedback.clear_actions = true;
        self.route_request = None;
        self.route_switching = false;
        self.route_switch_notice = None;
        self.pending_create = None;
        self.pending_metadata = None;
        self.keep_tab_visible_on_server_end = false;
        self.feedback.dirty = true;
        true
    }
    pub(super) fn install_workspace_index(
        &mut self,
        index: WorkspaceIndex,
        current: WorkspaceRouteId,
    ) {
        let snapshot = TabPresentationSnapshot::new(&index, false);
        let _ = self.install_workspace_tabs(snapshot, Some(current));
    }
    pub(super) fn set_workspace_revision(&mut self, revision: u64) {
        self.workspace_revision = Some(revision);
    }
    pub(super) fn track_metadata_request(
        &mut self,
        request_id: MetadataRequestId,
        route: WorkspaceRouteId,
        expected_revision: u64,
    ) {
        self.pending_metadata = Some(PendingMetadata {
            request_id,
            route,
            source_route: self.current_route,
            expected_revision,
        });
    }
    pub(super) fn take_workspace_metadata_mutation(&mut self) -> Option<WorkspaceMetadataMutation> {
        self.pending_tab_mutation.take()
    }
    pub(super) fn reject_workspace_metadata_mutation(&mut self, label: &'static str, now: Instant) {
        self.pending_metadata = None;
        self.clear_applying_status();
        match self.tab_dialog.as_mut() {
            Some(TabDialog::Rename { error, saving, .. }) => {
                *error = Some(label);
                *saving = false;
            }
            Some(TabDialog::Close { pending, .. }) => *pending = false,
            _ => {}
        }
        self.set_status(label, now);
        self.feedback.dirty = true;
    }
    pub(super) fn accept_metadata_result(
        &mut self,
        request_id: MetadataRequestId,
        route: WorkspaceRouteId,
        observed_revision: u64,
    ) -> bool {
        let Some(pending) = self.pending_metadata else {
            return false;
        };
        if pending.request_id != request_id
            || pending.route != route
            || self.current_route != pending.source_route
            || observed_revision < pending.expected_revision
            || self
                .workspace_revision
                .is_some_and(|revision| observed_revision < revision)
            || self.route_switching
        {
            return false;
        }
        self.pending_metadata = None;
        self.workspace_revision = Some(observed_revision);
        true
    }
    pub(super) fn complete_rename_metadata(
        &mut self,
        metadata: MetadataResult,
        now: Instant,
    ) -> bool {
        let MetadataResult::Renamed {
            request_id,
            route,
            expected_revision,
            result,
            observed_revision,
            snapshot,
        } = metadata
        else {
            return false;
        };
        if expected_revision == 0
            || self
                .pending_metadata
                .is_none_or(|pending| pending.expected_revision != expected_revision)
            || !self.take_metadata_result(request_id, route, observed_revision)
        {
            return false;
        }
        let result_revision = match &result {
            Ok(RenameOutcome::Saved { revision })
            | Ok(RenameOutcome::Conflict { revision, .. }) => Some(*revision),
            Err(_) => None,
        };
        let current_revision = observed_revision.or(result_revision);
        if let Some(revision) = current_revision {
            self.workspace_revision = Some(revision);
        }
        if let (Some(snapshot), Some(revision), Some(source_route)) =
            (snapshot, observed_revision, self.current_route)
        {
            self.update_workspace_presentation(Some(source_route), snapshot, revision);
        }
        match result {
            Ok(RenameOutcome::Saved { .. }) => {
                // Close only the rename dialog that is still awaiting this
                // save; a newer dialog opened after Esc stays open.
                if matches!(
                    self.tab_dialog,
                    Some(TabDialog::Rename {
                        route: editing_route,
                        saving: true,
                        ..
                    }) if editing_route == route
                ) {
                    self.tab_dialog = None;
                }
                self.set_status("tab renamed", now);
            }
            Ok(RenameOutcome::Conflict { revision, .. }) => {
                if let Some(TabDialog::Rename {
                    route: editing_route,
                    expected_revision,
                    error,
                    saving,
                    ..
                }) = self.tab_dialog.as_mut()
                {
                    if *editing_route == route {
                        *expected_revision = observed_revision.unwrap_or(revision);
                        *error = Some("tab changed elsewhere; your edit is preserved");
                        *saving = false;
                    }
                }
                self.set_status("workspace metadata changed; edit preserved", now);
            }
            Err(error) => {
                if let Some(TabDialog::Rename {
                    route: editing_route,
                    error: edit_error,
                    saving,
                    ..
                }) = self.tab_dialog.as_mut()
                {
                    if *editing_route == route {
                        *edit_error = Some(lifecycle_error_label(error));
                        *saving = false;
                    }
                }
                self.set_status(lifecycle_error_label(error), now);
            }
        }
        self.feedback.dirty = true;
        true
    }
    pub(super) fn complete_close_metadata(
        &mut self,
        metadata: MetadataResult,
        now: Instant,
    ) -> bool {
        let MetadataResult::Closed {
            request_id,
            route,
            expected,
            outcome,
            observed_revision,
            snapshot,
        } = metadata
        else {
            return false;
        };
        if expected.route != route
            || !self.take_metadata_result(request_id, route, observed_revision)
        {
            return false;
        }
        if let Some(revision) = observed_revision {
            self.workspace_revision = Some(revision);
        }
        self.tab_dialog = None;
        match outcome {
            crate::workspace_lifecycle::CloseOutcome::Stopped => {
                self.keep_tab_visible_on_server_end = false;
                self.clear_applying_status();
                if self.current_route != Some(route) {
                    if let (Some(snapshot), Some(revision), Some(source_route)) =
                        (snapshot, observed_revision, self.current_route)
                    {
                        self.update_workspace_presentation(Some(source_route), snapshot, revision);
                    }
                } else if snapshot.is_none() || observed_revision.is_none() {
                    self.set_status("tab stopped; workspace metadata unavailable", now);
                }
            }
            crate::workspace_lifecycle::CloseOutcome::Partial { reason }
            | crate::workspace_lifecycle::CloseOutcome::Unknown { reason } => {
                self.keep_tab_visible_on_server_end = true;
                self.set_persistent_status("Outcome unknown", now);
                if let (Some(snapshot), Some(revision), Some(source_route)) =
                    (snapshot, observed_revision, self.current_route)
                {
                    self.update_workspace_presentation(Some(source_route), snapshot, revision);
                }
                self.set_status(reason, now);
            }
        }
        self.feedback.dirty = true;
        true
    }
    pub(super) fn take_metadata_result(
        &mut self,
        request_id: MetadataRequestId,
        route: WorkspaceRouteId,
        observed_revision: Option<u64>,
    ) -> bool {
        if let Some(revision) = observed_revision {
            return self.accept_metadata_result(request_id, route, revision);
        }
        let Some(pending) = self.pending_metadata else {
            return false;
        };
        if pending.request_id != request_id
            || pending.route != route
            || self.current_route != pending.source_route
            || self.route_switching
        {
            return false;
        }
        self.pending_metadata = None;
        true
    }
    pub(super) fn take_create_request(&mut self) -> Option<(ProjectId, Option<CreateIntentId>)> {
        let pending = self.pending_create.as_mut()?;
        if pending.dispatched || pending.awaiting_reconcile {
            return None;
        }
        pending.dispatched = true;
        Some((pending.project, pending.intent_id))
    }
    pub(super) fn hold_uncertain_create(&mut self, now: Instant) {
        if let Some(pending) = self.pending_create.as_mut() {
            pending.dispatched = false;
            pending.request_id = None;
            pending.awaiting_reconcile = true;
        }
        self.set_status("tab creation outcome unknown; New tab rechecks it", now);
    }
    pub(super) fn record_create_request(
        &mut self,
        request_id: MetadataRequestId,
        intent_id: CreateIntentId,
    ) -> bool {
        let Some(pending) = self.pending_create.as_mut() else {
            return false;
        };
        if !pending.dispatched || pending.source_route != self.current_route {
            return false;
        }
        pending.request_id = Some(request_id);
        pending.intent_id = Some(intent_id);
        true
    }
    pub(super) fn reject_create_request(&mut self, label: &'static str, now: Instant) {
        self.pending_create = None;
        self.set_status(label, now);
    }
    pub(super) fn complete_create_result(
        &mut self,
        metadata: MetadataResult,
        now: Instant,
    ) -> Option<WorkspaceRouteId> {
        let MetadataResult::Created {
            request_id,
            intent_id,
            project,
            result,
            observed_revision,
            snapshot,
        } = metadata
        else {
            return None;
        };
        let pending = self.pending_create?;
        if pending.request_id != Some(request_id)
            || pending.intent_id != Some(intent_id)
            || pending.project != project
            || pending.source_route != self.current_route
        {
            return None;
        }
        if result
            .as_ref()
            .is_ok_and(|created| created.route.project != pending.project)
        {
            return None;
        }
        match result {
            Ok(created) if created.status == CreateStatus::Ready => {
                let current_revision = self.workspace_revision;
                let revision_is_fresh = observed_revision.is_some_and(|revision| {
                    revision > 0 && current_revision.is_none_or(|current| revision >= current)
                });
                let route_is_visible = snapshot.is_some_and(|snapshot| {
                    snapshot.projects.iter().any(|project| {
                        project.id == created.route.project
                            && project.tabs.iter().any(|tab| tab.route == created.route)
                    })
                });
                if !revision_is_fresh || !route_is_visible {
                    self.hold_uncertain_create(now);
                    return None;
                }
                self.workspace_revision = observed_revision;
                self.pending_create = None;
                self.feedback.dirty = true;
                Some(created.route)
            }
            Ok(CreateShellTab {
                status: CreateStatus::Unknown,
                ..
            }) => {
                // Preserve the admitted intent: a later explicit reconciliation
                // must reuse it rather than allocating a fresh route.
                self.hold_uncertain_create(now);
                None
            }
            Ok(_) => {
                self.pending_create = None;
                self.set_status("tab metadata changed; refresh and try again", now);
                None
            }
            Err(LifecycleError::IntentUncertain) => {
                self.hold_uncertain_create(now);
                None
            }
            Err(error) => {
                self.pending_create = None;
                self.set_status(lifecycle_error_label(error), now);
                None
            }
        }
    }
    pub(super) fn update_workspace_catalog(&mut self, index: WorkspaceIndex) -> bool {
        let (Some(current), Some(_)) = (self.current_route, self.workspace_index.as_ref()) else {
            return false;
        };
        if workspace_tabs::layout(&index, current, 0, 0).is_err() {
            return false;
        }
        self.workspace_index = Some(index);
        if (self.picker_open || self.picker_selection.is_some())
            && self
                .picker_selection
                .is_none_or(|selection| !self.picker_routes().contains(&selection))
        {
            self.picker_selection = self.picker_routes().first().copied();
        }
        self.feedback.dirty = true;
        true
    }
    pub(super) fn update_workspace_presentation(
        &mut self,
        source_route: Option<WorkspaceRouteId>,
        snapshot: TabPresentationSnapshot,
        observed_revision: u64,
    ) -> bool {
        if self.route_switching
            || self.current_route != source_route
            || self
                .workspace_revision
                .is_some_and(|revision| observed_revision < revision)
        {
            return false;
        }
        if source_route.is_some_and(|route| {
            !snapshot.projects.iter().any(|project| {
                project.id == route.project && project.tabs.iter().any(|tab| tab.route == route)
            })
        }) || (source_route.is_none()
            && (snapshot.active_route.is_some()
                || snapshot
                    .projects
                    .iter()
                    .any(|project| !project.tabs.is_empty())))
        {
            return false;
        }
        let Some(index) = presentation_index(&snapshot, source_route) else {
            return false;
        };
        self.workspace_index = Some(index);
        if (self.picker_open || self.picker_selection.is_some())
            && self
                .picker_selection
                .is_none_or(|selection| !self.picker_routes().contains(&selection))
        {
            self.picker_selection = self
                .current_route
                .filter(|route| self.route_matches_filter(*route))
                .or_else(|| self.picker_routes().first().copied());
        }
        self.feedback.dirty = true;
        self.workspace_revision = Some(observed_revision);
        true
    }
    pub(super) fn take_catalog_refresh_request(&mut self) -> bool {
        std::mem::replace(&mut self.catalog_refresh_requested, false)
    }
    pub(super) fn route_exists(&self, route: WorkspaceRouteId) -> bool {
        self.workspace_index
            .as_ref()
            .and_then(|index| index.project(route.project).ok())
            .is_some_and(|project| project.tabs().any(|tab| tab.id == route.tab))
    }
    pub(super) fn open_workspace_picker(&mut self) {
        self.cancel_selection();
        self.picker_open = true;
        self.picker_filter.clear();
        let routes = self.picker_routes();
        self.picker_selection = routes
            .iter()
            .copied()
            .find(|route| Some(*route) == self.current_route)
            .or_else(|| routes.first().copied());
        self.tab_dialog = None;
        self.feedback.clear_prefix = true;
        self.catalog_refresh_requested = true;
        self.feedback.dirty = true;
    }
    pub(super) fn take_route_request(&mut self) -> Option<WorkspaceRouteId> {
        self.route_request.take()
    }
    pub(super) fn tab_close_pending(&self) -> bool {
        matches!(
            self.tab_dialog,
            Some(TabDialog::Close { pending: true, .. })
        )
    }
    pub(super) fn finish_route_switch(&mut self, failed: bool, now: Instant) {
        self.route_switching = false;
        self.route_switch_notice = None;
        self.route_request = None;
        self.picker_selection = None;
        self.picker_open = false;
        self.picker_filter.clear();
        self.feedback.clear_prefix = true;
        if failed {
            self.set_status("workspace switch failed", now);
        }
        self.feedback.dirty = true;
    }
    pub(super) fn tabs_layout(&self, screen_viewport: (u16, u16)) -> Option<TabsLayout> {
        let (rows, columns) = screen_viewport;
        let index = self.workspace_index.as_ref()?;
        match self.current_route {
            Some(route) => workspace_tabs::layout(index, route, rows, columns).ok(),
            None if self.is_empty_workspace() => Some(workspace_tabs::empty_layout(rows, columns)),
            None => None,
        }
    }
    pub(super) fn is_empty_workspace(&self) -> bool {
        self.workspace_index.is_some() && self.current_route.is_none()
    }
    pub(super) fn empty_action_geometry(
        &self,
        viewport: (u16, u16),
    ) -> Option<EmptyWorkspaceGeometry> {
        if !self.is_empty_workspace() {
            return None;
        }
        let (rows, columns) = viewport;
        if rows < 2 || columns == 0 {
            return None;
        }
        let action_width = u16::try_from(EMPTY_ACTIONS.len()).ok()?;
        let new_tab_width = u16::try_from("[ New tab (N) ]".len()).ok()?;
        let separator_width = 3_u16;
        let detach_width = u16::try_from("[ Detach (Ctrl-X) ]".len()).ok()?;
        let action_start = columns.saturating_sub(action_width) / 2;
        let top = rows.saturating_sub(2) / 2;
        let button_row = top
            .saturating_add(1)
            .saturating_add(u16::from(self.workspace_index.is_some()));
        Some(EmptyWorkspaceGeometry {
            overlay: Rect {
                x: 0,
                y: top,
                rows: 2,
                columns,
            },
            new_tab: Rect {
                x: action_start,
                y: button_row,
                rows: 1,
                columns: new_tab_width,
            },
            detach: Rect {
                x: action_start
                    .saturating_add(new_tab_width)
                    .saturating_add(separator_width),
                y: button_row,
                rows: 1,
                columns: detach_width,
            },
        })
    }
    pub(super) fn request_empty_create(&mut self, now: Instant) {
        self.request_tab_create(
            ProjectId::new(1).expect("Default project ID is nonzero"),
            now,
        );
    }
    pub(super) fn request_tab_create(&mut self, project: ProjectId, now: Instant) {
        self.feedback.clear_prefix = true;
        if let Some(pending) = self.pending_create.as_mut() {
            // Explicit New tab after an unknown outcome reconciles the same
            // intent; any other pending create ignores the repeat.
            if pending.awaiting_reconcile {
                pending.awaiting_reconcile = false;
                self.feedback.dirty = true;
            }
            return;
        }
        if self.pending_metadata.is_some() || self.pending_tab_mutation.is_some() {
            self.set_status("workspace metadata busy", now);
            return;
        }
        if self
            .workspace_index
            .as_ref()
            .and_then(|index| index.project(project).ok())
            .is_some_and(|project| project.tabs().len() >= MAX_TABS_PER_PROJECT)
        {
            self.set_status("project tab limit reached (8)", now);
            return;
        }
        self.pending_create = Some(PendingCreate {
            project,
            source_route: self.current_route,
            request_id: None,
            intent_id: None,
            dispatched: false,
            awaiting_reconcile: false,
        });
        self.feedback.dirty = true;
    }
    pub(super) fn open_tab_actions(&mut self, route: WorkspaceRouteId) {
        if !self.route_exists(route) {
            return;
        }
        self.cancel_selection();
        self.picker_open = false;
        self.picker_selection = None;
        self.tab_dialog = Some(TabDialog::Actions { route, selected: 0 });
        self.feedback.clear_prefix = true;
        self.feedback.dirty = true;
    }
    pub(super) fn begin_tab_rename(&mut self, route: WorkspaceRouteId) {
        let Some(expected_revision) = self.workspace_revision else {
            self.set_status("workspace metadata is still loading", Instant::now());
            return;
        };
        let Some(name) = self
            .workspace_index
            .as_ref()
            .and_then(|index| index.project(route.project).ok())
            .and_then(|project| project.tabs().find(|tab| tab.id == route.tab))
            .map(|tab| tab.display_name.to_owned())
        else {
            self.set_status("workspace tab unavailable", Instant::now());
            return;
        };
        self.tab_dialog = Some(TabDialog::Rename {
            route,
            expected_revision,
            draft: name,
            error: None,
            saving: false,
        });
        self.feedback.dirty = true;
    }
    pub(super) fn begin_tab_close(&mut self, route: WorkspaceRouteId, generation: Option<u64>) {
        if !self.route_exists(route) {
            self.set_status("workspace tab unavailable", Instant::now());
            return;
        }
        let Some(expected_revision) = self.workspace_revision else {
            self.set_status("workspace metadata is still loading", Instant::now());
            return;
        };
        // Close must bind the exact runtime generation so a confirmation can
        // never stop a replacement server. Only the attached route's generation
        // is known here; an inactive tab must be opened first.
        if self.current_route != Some(route) {
            self.set_status("open this tab first to close it", Instant::now());
            return;
        }
        let Some(generation) = generation else {
            self.set_status("tab is still loading; try again", Instant::now());
            return;
        };
        self.tab_dialog = Some(TabDialog::Close {
            expected: ExpectedRuntimeIdentity {
                route,
                generation: Some(generation),
            },
            expected_revision,
            close_selected: false,
            pending: false,
        });
        self.feedback.dirty = true;
    }
    pub(super) fn submit_tab_rename(&mut self, now: Instant) {
        let Some(TabDialog::Rename {
            route,
            expected_revision,
            draft,
            saving,
            error,
        }) = self.tab_dialog.as_mut()
        else {
            return;
        };
        if *saving {
            return;
        }
        if *error == Some("Tab names are limited to 64 UTF-8 bytes") {
            self.feedback.dirty = true;
            return;
        }
        if !valid_tab_name(draft) {
            *error = Some("Name must be 1–64 bytes, trimmed, and control-free");
            self.feedback.dirty = true;
            return;
        }
        if self.pending_metadata.is_some() || self.pending_tab_mutation.is_some() {
            *error = Some("workspace metadata busy");
            self.feedback.dirty = true;
            return;
        }
        self.pending_tab_mutation = Some(WorkspaceMetadataMutation::Rename {
            route: *route,
            name: draft.clone(),
            expected_revision: *expected_revision,
        });
        *error = None;
        *saving = true;
        let _ = now;
        self.feedback.dirty = true;
    }
    pub(super) fn confirm_tab_close(&mut self, now: Instant) {
        let Some(TabDialog::Close {
            expected,
            expected_revision,
            pending,
            ..
        }) = self.tab_dialog.as_mut()
        else {
            return;
        };
        if *pending {
            return;
        }
        if self.pending_metadata.is_some() || self.pending_tab_mutation.is_some() {
            self.set_status("workspace metadata busy", now);
            return;
        }
        self.pending_tab_mutation = Some(WorkspaceMetadataMutation::Close {
            route: expected.route,
            expected: *expected,
            expected_revision: *expected_revision,
        });
        *pending = true;
        self.set_persistent_status("Applying…", now);
        self.feedback.dirty = true;
    }
    pub(super) fn append_rename_text(&mut self, text: &str) {
        let Some(TabDialog::Rename {
            draft,
            error,
            saving,
            ..
        }) = self.tab_dialog.as_mut()
        else {
            return;
        };
        if *saving {
            return;
        }
        *error = None;
        for character in text.chars().filter(|character| !character.is_control()) {
            if draft.len().saturating_add(character.len_utf8()) > MAX_DISPLAY_NAME_BYTES {
                *error = Some("Tab names are limited to 64 UTF-8 bytes");
                break;
            }
            draft.push(character);
        }
        self.feedback.dirty = true;
    }
    pub(super) fn update_picker_filter(&mut self, text: &str) {
        let mut changed = false;
        for character in text.chars().filter(|character| !character.is_control()) {
            if self
                .picker_filter
                .len()
                .saturating_add(character.len_utf8())
                > TAB_FILTER_MAX_BYTES
            {
                break;
            }
            self.picker_filter.push(character);
            changed = true;
        }
        if changed {
            self.picker_selection = self.picker_routes().first().copied();
            self.feedback.dirty = true;
        }
    }
    pub(super) fn request_route(&mut self, route: WorkspaceRouteId, now: Instant) {
        if self.route_switching {
            return;
        }
        self.cancel_selection();
        self.tab_dialog = None;
        self.picker_open = false;
        self.picker_filter.clear();
        self.picker_selection = None;
        self.feedback.clear_prefix = true;
        self.feedback.dirty = true;
        if self.current_route == Some(route) {
            return;
        }
        self.route_request = Some(route);
        self.begin_route_switch(now);
    }
    pub(super) fn navigate_workspace(&mut self, action: TabNavigation, now: Instant) {
        let (Some(index), Some(current)) = (&self.workspace_index, self.current_route) else {
            return;
        };
        if let Ok(route) = workspace_tabs::navigate(index, current, action) {
            self.request_route(route, now);
        }
    }
    pub(super) fn padded_tab_actions(&self, viewport: (u16, u16)) -> bool {
        let (rows, columns) = viewport;
        rows >= 9 && columns >= 28
    }
    pub(super) fn tab_dialog_geometry(&self, viewport: (u16, u16)) -> Option<Rect> {
        let dialog = self.tab_dialog.as_ref()?;
        let (rows, columns) = viewport;
        let (overlay_rows, preferred_columns) = match dialog {
            TabDialog::Actions { .. } if self.padded_tab_actions(viewport) => (9, 28),
            TabDialog::Actions { .. } => (4, 28),
            TabDialog::Rename { .. } => (4, 56),
            TabDialog::Close { .. } => (3, 44),
        };
        if rows < overlay_rows || columns == 0 {
            return None;
        }
        let width = columns.min(preferred_columns);
        Some(Rect {
            x: columns.saturating_sub(width) / 2,
            y: rows.saturating_sub(overlay_rows) / 2,
            rows: overlay_rows,
            columns: width,
        })
    }
    pub(super) fn cancel_tab_dialog(&mut self) {
        self.tab_dialog = None;
        self.feedback.dirty = true;
    }
    pub(super) fn activate_tab_action(
        &mut self,
        route: WorkspaceRouteId,
        selected: usize,
        now: Instant,
        generation: Option<u64>,
    ) {
        match selected {
            0 => {
                self.tab_dialog = None;
                self.request_route(route, now);
            }
            1 => self.begin_tab_rename(route),
            2 => self.begin_tab_close(route, generation),
            _ => return,
        }
        self.feedback.dirty = true;
    }
    pub(super) fn handle_tab_dialog_event(
        &mut self,
        event: &Event,
        now: Instant,
        viewport: (u16, u16),
        generation: Option<u64>,
    ) {
        let Some(dialog) = self.tab_dialog.clone() else {
            return;
        };
        let geometry = self.tab_dialog_geometry(viewport);
        match dialog {
            TabDialog::Actions {
                route,
                mut selected,
            } => match event {
                Event::Key(key) if key.is_press() || key.is_repeat() => {
                    let plain = key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;
                    match (key.code, plain) {
                        (KeyCode::Esc, true) => self.cancel_tab_dialog(),
                        (KeyCode::Up, true) => {
                            selected = (selected + 2) % 3;
                            self.tab_dialog = Some(TabDialog::Actions { route, selected });
                            self.feedback.dirty = true;
                        }
                        (KeyCode::Down, true) => {
                            selected = (selected + 1) % 3;
                            self.tab_dialog = Some(TabDialog::Actions { route, selected });
                            self.feedback.dirty = true;
                        }
                        (KeyCode::Enter, true) if key.is_press() => {
                            self.activate_tab_action(route, selected, now, generation)
                        }
                        (KeyCode::Char('s' | 'S'), true) => {
                            self.activate_tab_action(route, 0, now, generation)
                        }
                        (KeyCode::Char('r' | 'R'), true) => {
                            self.activate_tab_action(route, 1, now, generation)
                        }
                        (KeyCode::Char('x' | 'X'), true) => {
                            self.activate_tab_action(route, 2, now, generation)
                        }
                        _ => {}
                    }
                }
                Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(rect) = geometry {
                        let screen_y = rect.y + u16::from(self.workspace_index.is_some());
                        let row = mouse.row.saturating_sub(screen_y);
                        let first = if self.padded_tab_actions(viewport) {
                            3
                        } else {
                            1
                        };
                        let padding = if self.padded_tab_actions(viewport) {
                            2
                        } else {
                            0
                        };
                        if (first..first + 3).contains(&row)
                            && (rect.x + padding..rect.x + rect.columns - padding)
                                .contains(&mouse.column)
                        {
                            self.activate_tab_action(
                                route,
                                usize::from(row - first),
                                now,
                                generation,
                            );
                        }
                    }
                }
                Event::Resize(_, _) => self.feedback.dirty = true,
                _ => {}
            },
            TabDialog::Rename {
                route,
                expected_revision,
                mut draft,
                error,
                saving,
            } => match event {
                Event::Key(key) if key.is_press() || key.is_repeat() => {
                    let plain = key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;
                    match (key.code, plain) {
                        (KeyCode::Esc, true) if key.is_press() => self.cancel_tab_dialog(),
                        (KeyCode::Enter, true) if key.is_press() => self.submit_tab_rename(now),
                        (KeyCode::Backspace, true) if !saving => {
                            draft.pop();
                            self.tab_dialog = Some(TabDialog::Rename {
                                route,
                                expected_revision,
                                draft,
                                error: None,
                                saving,
                            });
                            self.feedback.dirty = true;
                        }
                        (KeyCode::Char(character), true) if !saving => {
                            self.append_rename_text(&character.to_string());
                        }
                        _ => {}
                    }
                }
                Event::Paste(text) if !saving => self.append_rename_text(text),
                Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(rect) = geometry {
                        let save_y = rect
                            .y
                            .saturating_add(3 + u16::from(self.workspace_index.is_some()));
                        if mouse.row == save_y
                            && (rect.x..rect.x + rect.columns.min(6)).contains(&mouse.column)
                        {
                            self.submit_tab_rename(now);
                        } else if mouse.row == save_y
                            && (rect.x + 7..rect.x + rect.columns.min(15)).contains(&mouse.column)
                        {
                            self.cancel_tab_dialog();
                        }
                    }
                }
                Event::Resize(_, _) => self.feedback.dirty = true,
                _ => {
                    let _ = error;
                }
            },
            TabDialog::Close {
                expected,
                expected_revision,
                mut close_selected,
                pending,
            } => match event {
                Event::Key(key) if key.is_press() || key.is_repeat() => {
                    let plain = key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;
                    match (key.code, plain) {
                        (KeyCode::Esc, true) if key.is_press() => self.cancel_tab_dialog(),
                        (KeyCode::Left | KeyCode::Right | KeyCode::Tab, true) if !pending => {
                            close_selected = !close_selected;
                            self.tab_dialog = Some(TabDialog::Close {
                                expected,
                                expected_revision,
                                close_selected,
                                pending,
                            });
                            self.feedback.dirty = true;
                        }
                        (KeyCode::Enter, true) if key.is_press() && !pending => {
                            if close_selected {
                                self.confirm_tab_close(now);
                            } else {
                                self.cancel_tab_dialog();
                            }
                        }
                        _ => {}
                    }
                }
                Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(rect) = geometry {
                        let button_y = rect
                            .y
                            .saturating_add(2 + u16::from(self.workspace_index.is_some()));
                        if mouse.row == button_y
                            && (rect.x..rect.x + rect.columns.min(11)).contains(&mouse.column)
                            && !pending
                        {
                            self.confirm_tab_close(now);
                        } else if mouse.row == button_y
                            && (rect.x + 12..rect.x + rect.columns.min(20)).contains(&mouse.column)
                            && !pending
                        {
                            self.cancel_tab_dialog();
                        }
                    }
                }
                Event::Resize(_, _) => self.feedback.dirty = true,
                _ => {}
            },
        }
    }
    pub(super) fn clear_picker_filter(&mut self) {
        if !self.picker_filter.is_empty() {
            self.picker_filter.clear();
            self.picker_selection = self
                .current_route
                .filter(|route| self.route_matches_filter(*route))
                .or_else(|| self.picker_routes().first().copied());
            self.feedback.dirty = true;
        }
    }
    pub(super) fn route_matches_filter(&self, route: WorkspaceRouteId) -> bool {
        let Some(index) = self.workspace_index.as_ref() else {
            return false;
        };
        let Some(project) = index.project(route.project).ok() else {
            return false;
        };
        let Some(tab) = project.tabs().find(|tab| tab.id == route.tab) else {
            return false;
        };
        let query = self.picker_filter.to_lowercase();
        query.is_empty()
            || project.display_name().to_lowercase().contains(&query)
            || tab.display_name.to_lowercase().contains(&query)
    }
    pub(super) fn handle_workspace_picker_event(
        &mut self,
        event: &Event,
        now: Instant,
        viewport: (u16, u16),
    ) {
        match event {
            Event::Key(key) if key.is_press() || key.is_repeat() => {
                let plain = key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;
                match (key.code, plain) {
                    (KeyCode::Esc, true) => {
                        self.picker_open = false;
                        self.picker_filter.clear();
                        self.picker_selection = None;
                        self.feedback.dirty = true;
                    }
                    (KeyCode::Enter, true) => {
                        if let Some(route) = self.picker_selection {
                            self.request_route(route, now);
                        }
                    }
                    (KeyCode::Up, true) => self.move_picker_selection(-1),
                    (KeyCode::Down, true) => self.move_picker_selection(1),
                    (KeyCode::Left, true) => self.move_picker_project(-1),
                    (KeyCode::Right, true) => self.move_picker_project(1),
                    (KeyCode::Backspace, true) => {
                        self.picker_filter.pop();
                        self.picker_selection = self.picker_routes().first().copied();
                        self.feedback.dirty = true;
                    }
                    _ if key.modifiers == KeyModifiers::CONTROL
                        && key.code == KeyCode::Char('u') =>
                    {
                        self.clear_picker_filter();
                    }
                    (KeyCode::Char(character), true) => {
                        self.update_picker_filter(&character.to_string());
                    }
                    _ => {}
                }
            }
            Event::Paste(text) => self.update_picker_filter(text),
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                if let Some((top, rows)) = self.picker_screen_geometry(viewport) {
                    if self
                        .picker_clear_rect(viewport)
                        .is_some_and(|rect| rect.contains(mouse.column, mouse.row))
                    {
                        self.clear_picker_filter();
                    } else if self
                        .picker_horizontal_bounds(viewport)
                        .contains(&mouse.column)
                        && mouse.row > top
                        && mouse.row < top.saturating_add(rows)
                    {
                        if let Some(route) = self.picker_route_at_row(mouse.row - top - 1, viewport)
                        {
                            self.request_route(route, now);
                        }
                    }
                }
            }
            Event::Resize(_, _) => self.feedback.dirty = true,
            _ => {}
        }
    }
    pub(super) fn picker_routes(&self) -> Vec<WorkspaceRouteId> {
        self.workspace_index
            .as_ref()
            .into_iter()
            .flat_map(|index| index.projects())
            .flat_map(|project| {
                project.tabs().map(move |tab| WorkspaceRouteId {
                    project: project.id(),
                    tab: tab.id,
                })
            })
            .filter(|route| self.route_matches_filter(*route))
            .collect()
    }
    pub(super) fn picker_horizontal_bounds(&self, viewport: (u16, u16)) -> std::ops::Range<u16> {
        let (_, columns) = viewport;
        let width = columns.min(64);
        let left = columns.saturating_sub(width) / 2;
        left..left + width
    }
    pub(super) fn picker_clear_rect(&self, viewport: (u16, u16)) -> Option<Rect> {
        let (top, _) = self.picker_screen_geometry(viewport)?;
        let bounds = self.picker_horizontal_bounds(viewport);
        (bounds.len() >= 8).then_some(Rect {
            x: bounds.start,
            y: top,
            rows: 1,
            columns: 7,
        })
    }
    pub(super) fn picker_route_at_row(
        &self,
        row: u16,
        viewport: (u16, u16),
    ) -> Option<WorkspaceRouteId> {
        let offset = self.picker_scroll_offset(viewport)?;
        let routes = self.picker_routes();
        routes.get(offset.checked_add(usize::from(row))?).copied()
    }
    pub(super) fn picker_screen_geometry(&self, viewport: (u16, u16)) -> Option<(u16, u16)> {
        let offset = self.picker_scroll_offset(viewport)?;
        let routes = self.picker_routes();
        let (view_rows, columns) = viewport;
        if view_rows == 0 || columns == 0 {
            return None;
        }
        let rows = if routes.is_empty() {
            2.min(view_rows)
        } else {
            let visible_routes = routes
                .len()
                .saturating_sub(offset)
                .min(self.picker_visible_route_count(viewport));
            (visible_routes + 1).min(usize::from(view_rows)) as u16
        };
        Some((1 + view_rows.saturating_sub(rows) / 2, rows))
    }
    pub(super) fn picker_visible_route_count(&self, viewport: (u16, u16)) -> usize {
        let (view_rows, _) = viewport;
        usize::from(view_rows)
            .min(12)
            .min(panelink::split_menu::MAX_OVERLAY_ROWS)
            .saturating_sub(1)
    }
    pub(super) fn picker_scroll_offset(&self, viewport: (u16, u16)) -> Option<usize> {
        let routes = self.picker_routes();
        if routes.is_empty() {
            return Some(0);
        }
        let Some(selection) = self.picker_selection else {
            return Some(0);
        };
        let selected = routes.iter().position(|route| *route == selection)?;
        let visible = self.picker_visible_route_count(viewport);
        Some(selected.saturating_sub(visible.saturating_sub(1)))
    }
    pub(super) fn move_picker_selection(&mut self, delta: isize) {
        let routes = self.picker_routes();
        if routes.is_empty() {
            return;
        }
        let current = self
            .picker_selection
            .and_then(|selection| routes.iter().position(|route| *route == selection))
            .unwrap_or(0);
        let next = (isize::try_from(current).unwrap_or(0) + delta)
            .rem_euclid(isize::try_from(routes.len()).unwrap_or(1));
        self.picker_selection = routes.get(usize::try_from(next).unwrap_or(0)).copied();
        self.feedback.dirty = true;
    }
    pub(super) fn move_picker_project(&mut self, delta: isize) {
        let (Some(index), Some(selection)) = (&self.workspace_index, self.picker_selection) else {
            return;
        };
        let action = if delta < 0 {
            TabNavigation::PreviousProject
        } else {
            TabNavigation::NextProject
        };
        if let Ok(route) = workspace_tabs::navigate(index, selection, action) {
            self.picker_selection = Some(route);
            self.feedback.dirty = true;
        }
    }
    pub(super) fn picker_overlay_text(
        &self,
        lines: &mut [String; panelink::split_menu::MAX_OVERLAY_ROWS],
        viewport: (u16, u16),
    ) -> usize {
        let routes = self.picker_routes();
        let Some(offset) = self.picker_scroll_offset(viewport) else {
            return 0;
        };
        let Some(index) = self.workspace_index.as_ref() else {
            return 0;
        };
        lines[0] = format!("[Clear] All tabs: {} (Esc closes)", self.picker_filter);
        if routes.is_empty() {
            lines[1] = "No tabs match your search.".into();
            return 2;
        }
        let visible = self
            .picker_visible_route_count(viewport)
            .min(lines.len().saturating_sub(1));
        let mut count = 1;
        for route in routes.iter().skip(offset).take(visible) {
            let Some(project) = index.project(route.project).ok() else {
                continue;
            };
            let Some(tab) = project.tabs().find(|tab| tab.id == route.tab) else {
                continue;
            };
            let active = self.current_route == Some(*route);
            let selected = self.picker_selection == Some(*route);
            lines[count] = format!(
                "{}{} / {}",
                if selected {
                    "> "
                } else if active {
                    "* "
                } else {
                    "  "
                },
                project.display_name(),
                tab.display_name
            );
            count += 1;
        }
        count
    }
}

fn presentation_index(
    snapshot: &TabPresentationSnapshot,
    current: Option<WorkspaceRouteId>,
) -> Option<WorkspaceIndex> {
    let has_routes = snapshot
        .projects
        .iter()
        .any(|project| !project.tabs.is_empty());
    match current {
        Some(route)
            if !snapshot.projects.iter().any(|project| {
                project.id == route.project && project.tabs.iter().any(|tab| tab.route == route)
            }) =>
        {
            return None;
        }
        None if has_routes || snapshot.active_route.is_some() => return None,
        _ => {}
    }

    let mut index = WorkspaceIndex::new();
    for project in &snapshot.projects {
        let first = project.tabs.first()?;
        if first.route.project != project.id {
            return None;
        }
        index
            .add_project(
                project.id,
                &project.display_name,
                first.route.tab,
                &first.display_name,
            )
            .ok()?;
        for tab in project.tabs.iter().skip(1) {
            if tab.route.project != project.id {
                return None;
            }
            index
                .add_tab(project.id, tab.route.tab, &tab.display_name)
                .ok()?;
        }
    }
    if let Some(route) = current {
        index.select_tab(route.project, route.tab).ok()?;
    }
    Some(index)
}

fn valid_tab_name(name: &str) -> bool {
    (1..=MAX_DISPLAY_NAME_BYTES).contains(&name.len())
        && !name.trim().is_empty()
        && name.trim() == name
        && !name.chars().any(char::is_control)
}

fn lifecycle_error_label(error: LifecycleError) -> &'static str {
    match error {
        LifecycleError::MetadataUnavailable => "workspace metadata unavailable",
        LifecycleError::InvalidRoute => "workspace tab unavailable",
        LifecycleError::InvalidName => "tab name is invalid",
        LifecycleError::TabLimitReached => "project tab limit reached",
        LifecycleError::RevisionConflict => "workspace metadata changed; refresh and retry",
        LifecycleError::IntentConflict => "tab creation intent conflict",
        LifecycleError::IntentUncertain => "tab creation outcome unknown",
        LifecycleError::LaunchContextInvalid => "launch context unavailable",
    }
}
