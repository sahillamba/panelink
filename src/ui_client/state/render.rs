//! Bounded frame assembly from immutable owner views. Output acceptance remains
//! with the host and compositor; rendering cannot mutate application state.
use super::{
    copy::{CopyContext, CopyState},
    history_label_kind,
    interaction::{
        ActionsMenu, ActionsMenuPage, ConfirmUiGeometry, Interaction, InteractionContext,
        CONFIRM_PADDED_BUTTON_ROW, CONFIRM_PADDED_ROWS,
    },
    shifted_rect,
    workspace::WorkspaceState,
    EmptyWorkspaceGeometry, PointerSeek, EMPTY_ACTIONS, EMPTY_MESSAGE, HELP_LINES, NAV_BANNER,
    STATUS_TTL, TOO_SMALL, WORKSPACE_HELP_LINES, WORKSPACE_NAV_BANNER,
};
use panelink::split_menu::OverlayStyle as MenuStyle;
use panelink::terminal::compose::{
    ComposeFrame, Compositor, OverlayControl, OverlayLine, OverlayStyle, OverlayView, PaneChrome,
    PaneContent, PaneView, Scrollbar,
};
use panelink::terminal::controls::{ControlRole, ControlState};
use panelink::{
    pane_layout::{PaneLayout, Rect},
    pane_operations_protocol::PaneOperationsView,
    split_menu::{Availability, PaneMenuAction},
    workspace_tabs::{TabsLayout, WorkspaceRouteId},
};
use std::{io, time::Instant};
pub(super) struct RenderView<'a> {
    pub source: CopyContext<'a>,
    pub workspace_pressed: Option<Rect>,
    pub surface_pressed: Option<(super::SurfaceAction, Rect)>,
    pub modal_pressed: Option<super::interaction::ModalControl>,
    pub header_pressed: Option<(panelink::control_protocol::TargetRef, bool)>,
    pub copy: &'a CopyState,
    pub workspace: &'a WorkspaceState,
    pub interaction: &'a Interaction,
    pub local: Option<&'a PaneLayout>,
    pub pane_operations: Option<PaneOperationsView>,
    pub viewport: (u16, u16),
    pub screen_viewport: (u16, u16),
    pub pointer_seek: Option<PointerSeek>,
    pub server_present: bool,
    pub persistent_status: Option<&'static str>,
}
impl<'a> RenderView<'a> {
    fn interaction_context(&self) -> InteractionContext<'a> {
        InteractionContext {
            layout: self.source.layout,
            local: self.local,
            pane_operations: self.pane_operations,
            viewport: self.viewport,
            header_offset: u16::from(self.workspace.view().workspace_index.is_some()),
        }
    }
    fn tabs_layout(&self) -> Option<TabsLayout> {
        self.workspace.tabs_layout(self.screen_viewport)
    }
    fn confirm_ui_geometry(&self, rows: u16, columns: u16) -> Option<ConfirmUiGeometry> {
        self.interaction.confirm_ui_geometry(
            rows,
            columns,
            u16::from(self.workspace.view().workspace_index.is_some()),
        )
    }
    fn actions_page_geometry(&self, menu: ActionsMenu) -> Option<ActionsMenuPage> {
        self.interaction
            .actions_page_geometry(self.interaction_context(), menu)
    }
    fn local_action_item(
        &self,
        menu: ActionsMenu,
        index: usize,
    ) -> Option<(PaneMenuAction, &'static str, Availability)> {
        self.interaction.local_action_item(menu, index)
    }
    fn padded_tab_actions(&self) -> bool {
        self.workspace.padded_tab_actions(self.viewport)
    }
    fn tab_dialog_geometry(&self) -> Option<Rect> {
        self.workspace.tab_dialog_geometry(self.viewport)
    }
    fn picker_overlay_text(
        &self,
        lines: &mut [String; panelink::split_menu::MAX_OVERLAY_ROWS],
    ) -> usize {
        self.workspace.picker_overlay_text(lines, self.viewport)
    }
    fn picker_routes(&self) -> Vec<WorkspaceRouteId> {
        self.workspace.picker_routes()
    }
    fn picker_scroll_offset(&self) -> Option<usize> {
        self.workspace.picker_scroll_offset(self.viewport)
    }
    fn picker_screen_geometry(&self) -> Option<(u16, u16)> {
        self.workspace.picker_screen_geometry(self.viewport)
    }
    fn picker_horizontal_bounds(&self) -> std::ops::Range<u16> {
        self.workspace.picker_horizontal_bounds(self.viewport)
    }
    fn empty_action_geometry(&self) -> Option<EmptyWorkspaceGeometry> {
        self.workspace.empty_action_geometry(self.viewport)
    }
    fn is_empty_workspace(&self) -> bool {
        self.workspace.is_empty_workspace()
    }
    pub(super) fn render<W: io::Write>(
        &self,
        compositor: &mut Compositor,
        output: &mut W,
        now: Instant,
    ) -> io::Result<()> {
        let tabs_layout = self.tabs_layout();
        let header_cells = tabs_layout.as_ref().map(|layout| {
            layout
                .header_cells()
                .iter()
                .enumerate()
                .map(|(column, cell)| {
                    if layout.hit_test(column as u16, 0)
                        == Some(panelink::workspace_tabs::TabAction::Create)
                        && self
                            .workspace
                            .pending_create
                            .is_some_and(|p| !p.awaiting_reconcile)
                    {
                        return cell.with_pending();
                    }
                    if self.surface_pressed.is_some_and(|(action, bounds)| {
                        matches!(action, super::SurfaceAction::Strip(_))
                            && bounds.contains(column as u16, 0)
                    }) {
                        cell.with_pressed()
                    } else {
                        *cell
                    }
                })
                .collect::<Vec<_>>()
        });
        let header_enabled = self.workspace.view().workspace_index.is_some();
        let header_offset = if header_enabled { 1 } else { 0 };
        // Both legacy attach and workspace tabs use the bounded render area,
        // even when the physical terminal is larger than the screen caches.
        let (host_rows, host_columns) = self.screen_viewport;
        if header_enabled && (host_rows == 0 || host_columns == 0) {
            return Ok(());
        }
        let mut panes = [None; crate::pane_layout::MAX_PANES];
        let mut scrollbars = [None; crate::pane_layout::MAX_PANES];
        let mut history_controls = [None; crate::pane_layout::MAX_PANES];
        let mut header_pressed = [None; crate::pane_layout::MAX_PANES];
        let mut labels = [None; crate::pane_layout::MAX_PANES];
        let mut divider = None;
        let mut dividers = [None; crate::pane_layout::MAX_PANES - 1];
        let mut focused = None;
        let fits = self.source.fits;
        let display_layout = self.source.display_layout;
        if let (Some(layout), true) = (display_layout, fits) {
            for (index, pane) in layout.panes.iter().enumerate() {
                let Some(pane) = pane else { continue };
                if self.copy.view().copied_feedback[index]
                    .is_some_and(|(target, expires_at)| target == pane.target && now < expires_at)
                {
                    labels[index] = Some("Copied");
                }
                let screen = self
                    .source
                    .screens
                    .iter()
                    .find(|slot| slot.target == Some(pane.target));
                let content_rect = panelink::pane_layout::content_rect(pane.rect);
                let actual = self.pane_operations.and_then(|view| {
                    let full = self.source.layout?;
                    let index = full.panes.iter().position(|candidate| {
                        candidate.is_some_and(|candidate| candidate.target == pane.target)
                    })?;
                    view.confirmed_sizes[index]
                });
                let pending = if self.pane_operations.is_some() {
                    actual.is_some_and(|actual| {
                        content_rect.is_some_and(|rect| actual != (rect.rows, rect.columns))
                    })
                } else {
                    pane.pending_confirmed.is_some()
                };
                let content = match (pending, screen, content_rect) {
                    (true, _, _) => PaneContent::Placeholder("resize pending"),
                    (false, Some(_), Some(rect))
                        if self.copy.view().copy_mode
                            && self.copy.view().selection.as_ref().is_some_and(|drag| {
                                drag.target == pane.target
                                    && drag.content_rect == rect
                                    && self.source.current_selection_identity(drag).is_some()
                            }) =>
                    {
                        PaneContent::Snapshot(
                            &self
                                .copy
                                .view()
                                .selection
                                .as_ref()
                                .expect("checked above")
                                .frozen,
                        )
                    }
                    (false, Some(slot), Some(rect))
                        if slot.history_offset > 0
                            && slot.history_revision > 0
                            && slot.history.rows() == rect.rows
                            && slot.history.columns() == rect.columns =>
                    {
                        PaneContent::Snapshot(&slot.history)
                    }
                    (false, Some(slot), Some(rect))
                        if slot.snapshot.rows() == rect.rows
                            && slot.snapshot.columns() == rect.columns =>
                    {
                        PaneContent::Snapshot(&slot.snapshot)
                    }
                    (false, Some(slot), Some(_))
                        if slot.snapshot.rows() == pane.rect.rows
                            && slot.snapshot.columns() == pane.rect.columns =>
                    {
                        PaneContent::Placeholder("restart server for pane padding")
                    }
                    (false, Some(_), Some(_)) => PaneContent::Placeholder("resizing..."),
                    (false, None, _) => PaneContent::Placeholder("connecting..."),
                    (false, Some(_), None) => PaneContent::Placeholder("pane too small"),
                };
                if let (false, Some(slot), PaneContent::Snapshot(_)) = (pending, screen, content) {
                    history_controls[index] = history_label_kind(slot);
                    if !slot.snapshot.alternate_screen() {
                        scrollbars[index] = Some(Scrollbar {
                            // A capture still awaiting extent discovery has
                            // only a conservative range; draw the real view
                            // until the discovered extent can map the pointer.
                            offset_rows: self
                                .pointer_seek
                                .filter(|drag| drag.target == pane.target && !drag.awaiting_extent)
                                .map_or(slot.history_offset, |drag| drag.preview_offset),
                            total_rows: self
                                .pointer_seek
                                .filter(|drag| drag.target == pane.target && !drag.awaiting_extent)
                                .map_or(slot.history_total.max(slot.history_offset), |drag| {
                                    drag.total_rows
                                }),
                        });
                    }
                }
                header_pressed[index] = self
                    .header_pressed
                    .filter(|(target, _)| *target == pane.target)
                    .map(|(_, close)| close);
                panes[index] = Some(PaneView {
                    rect: shifted_rect(pane.rect, header_offset),
                    content,
                });
                if Some(pane.target) == self.source.focus {
                    focused = Some(index);
                }
            }
            divider = layout.divider.map(|rect| shifted_rect(rect, header_offset));
            dividers = layout
                .dividers
                .map(|rect| rect.map(|rect| shifted_rect(rect, header_offset)));
        }

        let (view_rows, view_columns) = self.viewport;
        let menu_lines = self.interaction.view().menu.overlay_lines();
        let pressed = self.modal_pressed;
        let confirm = self.confirm_ui_geometry(view_rows, view_columns);
        let mut confirm_text: [String; CONFIRM_PADDED_ROWS as usize] =
            std::array::from_fn(|_| String::new());
        let mut lines = [OverlayLine {
            text: "",
            style: OverlayStyle::Normal,
        }; panelink::split_menu::MAX_OVERLAY_ROWS];
        let mut action_text: [String; panelink::split_menu::MAX_OVERLAY_ROWS] =
            std::array::from_fn(|_| String::new());
        let action_count = if let Some(menu) = self.interaction.view().actions_menu {
            let Some(page) =
                self.interaction
                    .format_actions(self.interaction_context(), menu, &mut action_text)
            else {
                return Ok(());
            };
            page.line_count
        } else {
            0
        };
        let mut picker_text: [String; panelink::split_menu::MAX_OVERLAY_ROWS] =
            std::array::from_fn(|_| String::new());
        let picker_count = if self.workspace.view().picker_open {
            self.picker_overlay_text(&mut picker_text)
        } else {
            0
        };
        for (index, text) in picker_text.iter().enumerate().take(picker_count) {
            lines[index] = OverlayLine {
                text,
                style: if index == 0 {
                    OverlayStyle::Status
                } else if self
                    .picker_routes()
                    .get(self.picker_scroll_offset().unwrap_or(0) + index.saturating_sub(1))
                    .is_some_and(|route| Some(*route) == self.workspace.view().picker_selection)
                {
                    OverlayStyle::Selected
                } else {
                    OverlayStyle::Normal
                },
            };
        }
        let mut tab_dialog_text: [String; 9] = std::array::from_fn(|_| String::new());
        let mut tab_dialog_selected = [false; 9];
        let tab_dialog_count = self.workspace.format_dialog(
            self.viewport,
            &mut tab_dialog_text,
            &mut tab_dialog_selected,
        );
        for (index, text) in tab_dialog_text.iter().enumerate().take(tab_dialog_count) {
            lines[index] = OverlayLine {
                text,
                style: if tab_dialog_selected[index] {
                    OverlayStyle::Selected
                } else if index == 0 || (self.padded_tab_actions() && index == 1) {
                    OverlayStyle::Status
                } else {
                    OverlayStyle::Normal
                },
            };
        }
        let mut pane_mode_text: [String; panelink::split_menu::MAX_OVERLAY_ROWS] =
            std::array::from_fn(|_| String::new());
        let pane_mode_count = self
            .interaction
            .format_mode(self.interaction_context(), &mut pane_mode_text);
        let empty_geometry = self.empty_action_geometry();
        let mut empty_text = [String::new(), String::new()];
        let overlay = if self.interaction.view().help_open && view_rows > 0 && view_columns > 0 {
            let help_lines: &[&str] = if header_enabled {
                &WORKSPACE_HELP_LINES
            } else {
                &HELP_LINES
            };
            let rows = view_rows.min(help_lines.len() as u16);
            let columns = view_columns.min(72);
            let x = view_columns.saturating_sub(columns) / 2;
            let y = view_rows.saturating_sub(rows) / 2;
            for (slot, text) in lines
                .iter_mut()
                .zip(help_lines.iter())
                .take(usize::from(rows))
            {
                *slot = OverlayLine {
                    text,
                    style: if text == &help_lines[0] {
                        OverlayStyle::Selected
                    } else if text == &help_lines[help_lines.len() - 1] {
                        OverlayStyle::Status
                    } else {
                        OverlayStyle::Normal
                    },
                };
            }
            Some((
                Rect {
                    x,
                    y,
                    rows,
                    columns,
                },
                usize::from(rows),
            ))
        } else if tab_dialog_count > 0 {
            let rect = self.tab_dialog_geometry().unwrap_or(Rect {
                x: 0,
                y: 0,
                rows: 0,
                columns: 0,
            });
            Some((rect, tab_dialog_count.min(usize::from(rect.rows))))
        } else if action_count > 0 {
            let page = self
                .interaction
                .view()
                .actions_menu
                .and_then(|menu| self.actions_page_geometry(menu))
                .expect("actions menu page was rendered above");
            for (index, text) in action_text.iter().enumerate().take(action_count) {
                lines[index] = OverlayLine {
                    text,
                    style: if pressed.is_some_and(|p| {
                        p.kind == 2
                            && p.bounds.y
                                == page.rect.y
                                    + index as u16
                                    + u16::from(self.workspace.view().workspace_index.is_some())
                    }) {
                        OverlayStyle::Status
                    } else if index == 0 {
                        OverlayStyle::Selected
                    } else if index == page.reason_row {
                        OverlayStyle::Status
                    } else if text.starts_with('>') {
                        OverlayStyle::Selected
                    } else if self.interaction.view().actions_menu.is_some_and(|menu| {
                        (page.start..page.end)
                            .contains(&(page.start + index.saturating_sub(page.action_row)))
                            && self
                                .local_action_item(
                                    menu,
                                    page.start + index.saturating_sub(page.action_row),
                                )
                                .is_some_and(|(_, _, availability)| {
                                    availability != Availability::Enabled
                                })
                    }) {
                        OverlayStyle::Disabled
                    } else {
                        OverlayStyle::Normal
                    },
                };
            }
            Some((page.rect, action_count.min(usize::from(page.rect.rows))))
        } else if pane_mode_count > 0 {
            for (index, text) in pane_mode_text.iter().enumerate().take(pane_mode_count) {
                lines[index] = OverlayLine {
                    text,
                    style: if pressed.is_some_and(|p| p.kind == 3 && p.index / 2 == index) {
                        OverlayStyle::Status
                    } else if index == 0 || text.starts_with('>') {
                        OverlayStyle::Selected
                    } else {
                        OverlayStyle::Normal
                    },
                };
            }
            let rows = u16::try_from(pane_mode_count)
                .unwrap_or(u16::MAX)
                .min(view_rows);
            let width = view_columns.min(76);
            Some((
                Rect {
                    x: view_columns.saturating_sub(width) / 2,
                    y: view_rows.saturating_sub(rows) / 2,
                    rows,
                    columns: width,
                },
                pane_mode_count.min(usize::from(rows)),
            ))
        } else if let (Some(rect), true) = (
            self.interaction.view().menu.overlay_rect(),
            self.interaction.view().menu.is_open(),
        ) {
            for (index, (slot, line)) in lines.iter_mut().zip(menu_lines.as_slice()).enumerate() {
                *slot = OverlayLine {
                    text: line.text(),
                    style: if pressed.is_some_and(|p| {
                        p.kind == 4
                            && p.bounds.y
                                == rect.y
                                    + index as u16
                                    + u16::from(self.workspace.view().workspace_index.is_some())
                    }) {
                        OverlayStyle::Status
                    } else {
                        compose_style(line.style())
                    },
                };
            }
            Some((rect, menu_lines.len()))
        } else if let Some(geometry) = confirm {
            let close_selected = self.interaction.view().confirm_close_selected;
            if geometry.padded {
                format_padded_confirm(&geometry, close_selected, &mut confirm_text);
                for (index, (line, text)) in lines.iter_mut().zip(&confirm_text).enumerate() {
                    *line = OverlayLine {
                        text,
                        style: if index == 0 || index + 1 == confirm_text.len() {
                            OverlayStyle::Border
                        } else if index == 2 {
                            OverlayStyle::Status
                        } else {
                            OverlayStyle::Normal
                        },
                    };
                }
            } else {
                confirm_text[0] = format!(
                    "Close pane {}? {}. (y/n)",
                    geometry.position, geometry.consequence
                );
                lines[0] = OverlayLine {
                    text: &confirm_text[0],
                    style: OverlayStyle::Status,
                };
                lines[1] = OverlayLine {
                    text: if close_selected {
                        "[Close]> [Cancel]"
                    } else {
                        "[Close] >[Cancel]"
                    },
                    style: OverlayStyle::Normal,
                };
            }
            Some((geometry.overlay, usize::from(geometry.overlay.rows)))
        } else if let (Some((top, rows)), true) = (
            self.picker_screen_geometry(),
            self.workspace.view().picker_open,
        ) {
            Some((
                Rect {
                    x: self.picker_horizontal_bounds().start,
                    y: top.saturating_sub(1),
                    rows,
                    columns: self.picker_horizontal_bounds().len() as u16,
                },
                usize::from(rows).min(picker_count),
            ))
        } else if self.is_empty_workspace() {
            empty_geometry.map(|geometry| {
                empty_text[0] = centered_ascii(
                    if let Some(pending) = self.workspace.view().pending_create {
                        if pending.awaiting_reconcile {
                            "Create outcome unknown; New tab rechecks it"
                        } else {
                            "Opening tab..."
                        }
                    } else {
                        EMPTY_MESSAGE
                    },
                    view_columns,
                );
                empty_text[1] = centered_ascii(EMPTY_ACTIONS, view_columns);
                lines[0] = OverlayLine {
                    text: &empty_text[0],
                    style: OverlayStyle::Status,
                };
                lines[1] = OverlayLine {
                    text: &empty_text[1],
                    style: OverlayStyle::Selected,
                };
                (geometry.overlay, 2)
            })
        } else {
            let notice = if !self.server_present {
                Some(("Disconnected", view_rows.saturating_sub(1)))
            } else if let Some(label) = self.persistent_status {
                Some((label, view_rows.saturating_sub(1)))
            } else if self.copy.view().copy_mode {
                None
            } else if self.workspace.view().route_switching {
                Some((
                    self.workspace
                        .view()
                        .route_switch_notice
                        .unwrap_or("workspace switch preparing"),
                    view_rows.saturating_sub(1),
                ))
            } else if let Some(pending) = self.workspace.view().pending_create {
                Some((
                    if pending.awaiting_reconcile {
                        "Create outcome unknown; press + to reconcile"
                    } else {
                        "Opening tab..."
                    },
                    view_rows.saturating_sub(1),
                ))
            } else if !fits && self.source.layout.is_some() {
                Some((TOO_SMALL, 0))
            } else if self.interaction.view().prefix.is_some() {
                Some((
                    if header_enabled {
                        WORKSPACE_NAV_BANNER
                    } else {
                        NAV_BANNER
                    },
                    view_rows.saturating_sub(1),
                ))
            } else {
                self.source
                    .status
                    .filter(|(_, at)| now.saturating_duration_since(*at) < STATUS_TTL)
                    .map(|(text, _)| (text, view_rows.saturating_sub(1)))
            };
            notice.and_then(|(text, row)| {
                lines[0] = OverlayLine {
                    text,
                    style: if self.interaction.view().prefix.is_some() && fits {
                        OverlayStyle::Selected
                    } else {
                        OverlayStyle::Status
                    },
                };
                (view_rows > 0 && view_columns > 0).then_some((
                    Rect {
                        x: 0,
                        y: row,
                        rows: 1,
                        columns: view_columns,
                    },
                    1,
                ))
            })
        };
        let pane_overlay = action_count > 0
            || pane_mode_count > 0
            || confirm.is_some()
            || self.interaction.view().menu.is_open();
        let mut overlay_controls: Vec<_> = if action_count > 0
            || pane_mode_count > 0
            || confirm.is_some()
            || self.interaction.view().menu.is_open()
        {
            self.interaction
                .modal_controls(self.interaction_context())
                .into_iter()
                .map(|control| {
                    let mut role = ControlRole::Secondary;
                    let mut state = ControlState::Normal;
                    if control.kind == 1 {
                        role = if control.index == 0 {
                            ControlRole::Destructive
                        } else {
                            ControlRole::Secondary
                        };
                        if (control.index == 0) == self.interaction.view().confirm_close_selected {
                            state = ControlState::Focus;
                        }
                    } else if control.kind == 2 {
                        if let Some(menu) = self.interaction.view().actions_menu {
                            if control.index == menu.selected
                                || (control.index == 102 && menu.selected == 12)
                            {
                                state = ControlState::Focus;
                            }
                            if let Some((action, _, availability)) =
                                self.local_action_item(menu, control.index)
                            {
                                if action == PaneMenuAction::ClosePane {
                                    role = ControlRole::Destructive;
                                }
                                if availability != Availability::Enabled {
                                    state = ControlState::Disabled;
                                }
                            }
                        }
                    } else {
                        let row = usize::from(
                            control
                                .bounds
                                .y
                                .saturating_sub(header_offset)
                                .saturating_sub(overlay.map_or(0, |(rect, _)| rect.y)),
                        );
                        if lines
                            .get(row)
                            .is_some_and(|line| line.style == OverlayStyle::Selected)
                        {
                            state = ControlState::Focus;
                        } else if lines
                            .get(row)
                            .is_some_and(|line| line.style == OverlayStyle::Disabled)
                        {
                            state = ControlState::Disabled;
                        }
                    }
                    if pressed == Some(control) {
                        state = ControlState::Pressed;
                    }
                    OverlayControl {
                        bounds: control.bounds,
                        role,
                        state,
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        let workspace_overlay = tab_dialog_count > 0 || picker_count > 0;
        if workspace_overlay {
            overlay_controls = self
                .workspace
                .workspace_controls(self.viewport)
                .into_iter()
                .map(|(control, bounds)| {
                    let (role, mut state) = self.workspace.control_visual(control);
                    if self.workspace_pressed == Some(bounds)
                        && !matches!(state, ControlState::Pending | ControlState::Disabled)
                    {
                        state = ControlState::Pressed;
                    }
                    OverlayControl {
                        bounds,
                        role,
                        state,
                    }
                })
                .collect();
        } else if self.is_empty_workspace() {
            if let Some(geometry) = empty_geometry {
                for (action, bounds) in [
                    (super::SurfaceAction::NewTab, geometry.new_tab),
                    (super::SurfaceAction::Detach, geometry.detach),
                ] {
                    let state = if action == super::SurfaceAction::NewTab
                        && self.workspace.pending_create.is_some()
                    {
                        ControlState::Pending
                    } else if self.surface_pressed == Some((action, bounds)) {
                        ControlState::Pressed
                    } else {
                        ControlState::Normal
                    };
                    overlay_controls.push(OverlayControl {
                        bounds,
                        role: ControlRole::Secondary,
                        state,
                    });
                }
            }
        }
        if let Some(bounds) = self.interaction.help_control(self.viewport, header_offset) {
            overlay_controls.push(OverlayControl {
                bounds,
                role: ControlRole::Secondary,
                state: if self.interaction.help_pressed(self.viewport, header_offset) {
                    ControlState::Pressed
                } else {
                    ControlState::Normal
                },
            });
            lines[0].style = OverlayStyle::Normal;
        }

        if pane_overlay || workspace_overlay || self.is_empty_workspace() {
            for line in &mut lines {
                line.style = OverlayStyle::Normal;
            }
        }
        let frame = ComposeFrame {
            host_rows,
            host_columns,
            panes,
            divider,
            dividers,
            focused,
            overlay: overlay.map(|(rect, count)| OverlayView {
                controls: &overlay_controls,
                rect: shifted_rect(rect, header_offset),
                lines: &lines[..count],
            }),
        };
        let selection = self.copy.selection_overlay(&self.source);
        let caret = self.copy.caret_overlay(&self.source, overlay.is_none());
        compositor.render_with_history_controls(
            &frame,
            selection,
            caret,
            header_cells.as_deref(),
            PaneChrome {
                scrollbars,
                history: history_controls,
                labels,
                header_pressed,
                history_pressed: std::array::from_fn(|index| {
                    self.source
                        .layout
                        .and_then(|layout| layout.panes[index])
                        .is_some_and(|pane| {
                            self.surface_pressed.is_some_and(|(action, _)| {
                                action == super::SurfaceAction::History(pane.target)
                            })
                        })
                }),
            },
            output,
        )
    }
}

fn compose_style(style: MenuStyle) -> OverlayStyle {
    match style {
        MenuStyle::Normal => OverlayStyle::Normal,
        MenuStyle::Selected => OverlayStyle::Selected,
        MenuStyle::Disabled => OverlayStyle::Disabled,
        MenuStyle::Border => OverlayStyle::Border,
        MenuStyle::Status => OverlayStyle::Status,
    }
}
/// Fill the bordered, padded pane-close dialog rows. Button columns come from
/// the same geometry used for hit-testing so pointer targets match the text.
fn format_padded_confirm(
    geometry: &ConfirmUiGeometry,
    close_selected: bool,
    rows: &mut [String; CONFIRM_PADDED_ROWS as usize],
) {
    let width = usize::from(geometry.overlay.columns);
    if width < 2 {
        return;
    }
    let inner = width - 2;
    let border = format!("+{}+", "-".repeat(inner));
    let boxed = |text: &str| -> String {
        let mut row = vec![b' '; width];
        row[0] = b'|';
        row[width - 1] = b'|';
        for (cell, byte) in row[CONFIRM_TEXT_INDENT..width - 1]
            .iter_mut()
            .zip(text.bytes())
        {
            *cell = byte;
        }
        String::from_utf8(row).unwrap_or_default()
    };
    rows[0] = border.clone();
    rows[1] = boxed("");
    rows[2] = boxed(&format!("Close pane {}?", geometry.position));
    rows[3] = boxed(&format!("{}.", geometry.consequence));
    rows[4] = boxed("");
    let mut buttons = boxed("").into_bytes();
    let place = |row: &mut [u8], x: u16, marker: bool, label: &str| {
        let start = usize::from(x - geometry.overlay.x);
        if start == 0 || start + label.len() >= width {
            return;
        }
        row[start - 1] = if marker { b'>' } else { b' ' };
        row[start..start + label.len()].copy_from_slice(label.as_bytes());
    };
    place(&mut buttons, geometry.close.x, close_selected, "[Close]");
    place(&mut buttons, geometry.cancel.x, !close_selected, "[Cancel]");
    rows[CONFIRM_PADDED_BUTTON_ROW as usize] = String::from_utf8(buttons).unwrap_or_default();
    rows[6] = boxed("");
    rows[7] = boxed(CONFIRM_KEY_HINT);
    rows[8] = border;
}

/// Columns from the dialog's left border to its text.
const CONFIRM_TEXT_INDENT: usize = 4;
const CONFIRM_KEY_HINT: &str = "y: close   n/Esc: cancel   Tab: switch";

fn centered_ascii(text: &str, columns: u16) -> String {
    let padding = usize::from(columns).saturating_sub(text.len()) / 2;
    format!("{}{}", " ".repeat(padding), text)
}
