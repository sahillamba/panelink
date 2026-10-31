//! Pure, bounded navigation and top-strip layout for workspace routes.
//!
//! This view reads names and order from [`WorkspaceIndex`] without changing
//! its selection. It owns no IO, storage, target references, or processes.

use std::fmt;

use unicode_width::UnicodeWidthChar;

use crate::pane_layout::{Rect, MIN_PANE_COLUMNS, MIN_PANE_ROWS};
use crate::terminal::compose::HeaderCell;
use crate::workspace_index::{ProjectId, WorkspaceIndex, WorkspaceTabId};

const MAX_ROWS: u16 = 80;
const MAX_COLUMNS: u16 = 240;
const MAX_LABEL_CELLS: usize = 24;
const MAX_GLYPH_BYTES: usize = 32;

/// Stable client-side identity for a selected project/workspace pair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceRouteId {
    /// Project containing the workspace tab.
    pub project: ProjectId,
    /// Workspace tab to attach to.
    pub tab: WorkspaceTabId,
}

/// A typed action produced by a strip hit region.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TabAction {
    /// Open the project chooser.
    ProjectChooser,
    /// Select this stable project and tab pair.
    Select(WorkspaceRouteId),
    /// Open the tab actions for this route.
    TabActions(WorkspaceRouteId),
    /// Close exactly this route without selecting it.
    Close(WorkspaceRouteId),
    /// Admit one managed shell-tab create intent for the current project.
    Create,
    /// Open the searchable all-tabs overlay.
    AllTabs,
    /// Navigate to the previous tab in the current project.
    PreviousTab,
    /// Navigate to the next tab in the current project.
    NextTab,
}

/// Relative navigation requested by the client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TabNavigation {
    /// Select the next tab in the current project, wrapping at either end.
    NextTab,
    /// Select the previous tab in the current project, wrapping at either end.
    PreviousTab,
    /// Select the next nonempty project in index order.
    NextProject,
    /// Select the previous nonempty project in index order.
    PreviousProject,
}

/// Deterministic failures from validating or navigating a route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TabsError {
    /// The route's project or tab is absent from this index snapshot.
    InvalidRoute,
    /// No project with a tab is available for project navigation.
    NoNavigableProject,
}

impl fmt::Display for TabsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRoute => f.write_str("workspace route is not present"),
            Self::NoNavigableProject => f.write_str("no project with a workspace tab is available"),
        }
    }
}

impl std::error::Error for TabsError {}

/// Navigate within an immutable index snapshot, without changing selection.
pub fn navigate(
    index: &WorkspaceIndex,
    current: WorkspaceRouteId,
    action: TabNavigation,
) -> Result<WorkspaceRouteId, TabsError> {
    let project = index
        .project(current.project)
        .map_err(|_| TabsError::InvalidRoute)?;
    let tabs: Vec<_> = project.tabs().collect();
    let current_index = tabs
        .iter()
        .position(|tab| tab.id == current.tab)
        .ok_or(TabsError::InvalidRoute)?;

    match action {
        TabNavigation::NextTab | TabNavigation::PreviousTab => {
            let len = tabs.len();
            if len == 0 {
                return Err(TabsError::InvalidRoute);
            }
            let next = match action {
                TabNavigation::NextTab => (current_index + 1) % len,
                TabNavigation::PreviousTab => (current_index + len - 1) % len,
                _ => unreachable!("tab navigation arm is exhaustive"),
            };
            Ok(WorkspaceRouteId {
                project: current.project,
                tab: tabs[next].id,
            })
        }
        TabNavigation::NextProject | TabNavigation::PreviousProject => {
            let projects: Vec<_> = index
                .projects()
                .filter(|project| project.tabs().next().is_some())
                .collect();
            if projects.is_empty() {
                return Err(TabsError::NoNavigableProject);
            }
            let project_index = projects
                .iter()
                .position(|p| p.id() == current.project)
                .ok_or(TabsError::InvalidRoute)?;
            let len = projects.len();
            let next = match action {
                TabNavigation::NextProject => (project_index + 1) % len,
                TabNavigation::PreviousProject => (project_index + len - 1) % len,
                _ => unreachable!("project navigation arm is exhaustive"),
            };
            let next_project = projects[next];
            let tab = next_project
                .selected_tab_id()
                .filter(|selected| next_project.tabs().any(|tab| tab.id == *selected))
                .or_else(|| next_project.tabs().next().map(|tab| tab.id))
                .ok_or(TabsError::NoNavigableProject)?;
            Ok(WorkspaceRouteId {
                project: next_project.id(),
                tab,
            })
        }
    }
}

/// Complete strip and pane-root geometry for one clamped host viewport.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabsLayout {
    rows: u16,
    columns: u16,
    header_cells: Vec<HeaderCell>,
    pane_root: Rect,
    hit_regions: Vec<HitRegion>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HitRegion {
    start: u16,
    end: u16,
    action: TabAction,
}

impl TabsLayout {
    /// Header cells for row zero; length never exceeds the clamped width.
    pub fn header_cells(&self) -> &[HeaderCell] {
        &self.header_cells
    }

    /// Pane compositor root below the reserved header row.
    pub const fn pane_root(&self) -> Rect {
        self.pane_root
    }

    /// Clamped visible screen rows.
    pub const fn rows(&self) -> u16 {
        self.rows
    }

    /// Clamped visible screen columns.
    pub const fn columns(&self) -> u16 {
        self.columns
    }

    /// Whether the reduced root can hold one pane at the existing minimum.
    pub const fn fits_single_pane(&self) -> bool {
        self.pane_root.rows >= MIN_PANE_ROWS && self.pane_root.columns >= MIN_PANE_COLUMNS
    }

    /// Map a screen-coordinate cell to its stable strip action.
    pub fn hit_test(&self, column: u16, row: u16) -> Option<TabAction> {
        if row != 0 || column >= self.columns {
            return None;
        }
        self.hit_regions
            .iter()
            .find(|region| column >= region.start && column < region.end)
            .map(|region| region.action)
    }
}

/// Build a bounded top strip and a pane root from the current index snapshot.
pub fn layout(
    index: &WorkspaceIndex,
    current: WorkspaceRouteId,
    host_rows: u16,
    host_columns: u16,
) -> Result<TabsLayout, TabsError> {
    let project = index
        .project(current.project)
        .map_err(|_| TabsError::InvalidRoute)?;
    let tabs: Vec<_> = project.tabs().collect();
    if !tabs.iter().any(|tab| tab.id == current.tab) {
        return Err(TabsError::InvalidRoute);
    }

    let rows = host_rows.min(MAX_ROWS);
    let columns = host_columns.min(MAX_COLUMNS);
    if rows == 0 || columns == 0 {
        return Ok(TabsLayout {
            rows,
            columns,
            header_cells: Vec::new(),
            pane_root: Rect {
                x: 0,
                y: 0,
                rows: 0,
                columns: 0,
            },
            hit_regions: Vec::new(),
        });
    }

    let mut cells = vec![HeaderCell::blank(false); usize::from(columns)];
    let mut hits = Vec::with_capacity(16);
    let screen_width = cells.len();

    // New tab and All tabs stay pinned right. Narrow hosts shorten names
    // and omit optional previous/next controls before losing active-tab access.
    let spacing = HeaderSpacing::for_width(screen_width);
    let controls_start = place_right_controls(&mut cells, &mut hits, tabs.len() > 1);

    let project_name = project.display_name();
    let project_cap = MAX_LABEL_CELLS.min((screen_width / 5).max(3));
    let project_label = bracketed_label("[ ", project_name, " ]", project_cap);
    // Left margin grows with the host width tier.
    let left = spacing.margin;
    let project_budget = controls_start.saturating_sub(spacing.project_gap + left);
    let project_width = place_label(
        &mut cells,
        left,
        &project_label,
        false,
        project_budget.min(project_cap),
    );
    if project_width > 0 {
        hits.push(HitRegion {
            start: left as u16,
            end: (left + project_width) as u16,
            action: TabAction::ProjectChooser,
        });
    }

    let mut cursor = if project_width > 0 {
        left + project_width + spacing.project_gap
    } else {
        left
    };
    let tab_end = controls_start.saturating_sub(spacing.group_gap.saturating_sub(1).max(1));
    if cursor < tab_end {
        let remaining = (tab_end - cursor).saturating_sub(4);
        let labels: Vec<Vec<Glyph>> = tabs
            .iter()
            .map(|tab| bracketed_label(" ", tab.display_name, " ", MAX_LABEL_CELLS))
            .collect();
        let current_index = tabs
            .iter()
            .position(|tab| tab.id == current.tab)
            .ok_or(TabsError::InvalidRoute)?;
        let total_width = labels
            .iter()
            .map(|label| label_width(label) + 4 + spacing.tab_gap)
            .sum::<usize>()
            - spacing.tab_gap;
        let (start, end) = if total_width <= remaining {
            (0, labels.len())
        } else {
            selected_window(&labels, current_index, remaining, spacing.tab_gap)
        };
        for index in start..end {
            if index > start && cursor < tab_end {
                cursor += spacing.tab_gap;
            }
            if cursor >= tab_end {
                break;
            }
            let width = place_label(
                &mut cells,
                cursor,
                &labels[index],
                index == current_index,
                (tab_end - cursor).saturating_sub(4),
            );
            if width > 0 {
                hits.push(HitRegion {
                    start: cursor as u16,
                    end: (cursor + width) as u16,
                    action: TabAction::Select(WorkspaceRouteId {
                        project: current.project,
                        tab: tabs[index].id,
                    }),
                });
                cursor += width;
                if cursor + 4 <= tab_end {
                    place_button(
                        &mut cells,
                        &mut hits,
                        cursor + 4,
                        " x ",
                        TabAction::Close(WorkspaceRouteId {
                            project: current.project,
                            tab: tabs[index].id,
                        }),
                    );
                    cursor += 4;
                }
            }
        }
    }

    for hit in &hits {
        for cell in &mut cells[usize::from(hit.start)..usize::from(hit.end)] {
            *cell = cell.with_filled();
        }
    }
    Ok(TabsLayout {
        rows,
        columns,
        header_cells: cells,
        pane_root: Rect {
            x: 0,
            y: 1,
            rows: rows.saturating_sub(1),
            columns,
        },
        hit_regions: hits,
    })
}

/// Reserve the workspace header row without advertising a route when no tabs
/// are visible. Empty-state controls are rendered by the attached client below
/// this header; this layout intentionally has no tab hit regions.
pub fn empty_layout(host_rows: u16, host_columns: u16) -> TabsLayout {
    let rows = host_rows.min(MAX_ROWS);
    let columns = host_columns.min(MAX_COLUMNS);
    let (mut header_cells, pane_root) = if rows == 0 || columns == 0 {
        (
            Vec::new(),
            Rect {
                x: 0,
                y: 0,
                rows: 0,
                columns: 0,
            },
        )
    } else {
        (
            vec![HeaderCell::blank(false); usize::from(columns)],
            Rect {
                x: 0,
                y: 1,
                rows: rows.saturating_sub(1),
                columns,
            },
        )
    };
    let mut hit_regions = Vec::new();
    if rows > 0 && columns > 0 {
        place_create(&mut header_cells, &mut hit_regions);
    }
    for hit in &hit_regions {
        for cell in &mut header_cells[usize::from(hit.start)..usize::from(hit.end)] {
            *cell = cell.with_filled();
        }
    }
    TabsLayout {
        rows,
        columns,
        header_cells,
        pane_root,
        hit_regions,
    }
}

#[derive(Clone, Debug)]
struct Glyph {
    text: String,
    width: usize,
}

fn bracketed_label(prefix: &str, label: &str, suffix: &str, cap: usize) -> Vec<Glyph> {
    let mut result = Vec::new();
    append_ascii(&mut result, prefix, cap);
    append_text(&mut result, label, cap);
    append_ascii(&mut result, suffix, cap);
    result
}

fn append_ascii(output: &mut Vec<Glyph>, text: &str, cap: usize) {
    for character in text.chars() {
        append_character(output, character, cap);
    }
}

fn append_text(output: &mut Vec<Glyph>, text: &str, cap: usize) {
    for character in text.chars() {
        append_character(output, character, cap);
    }
}

fn append_character(output: &mut Vec<Glyph>, character: char, cap: usize) {
    // Without grapheme segmentation, preserve a visible boundary instead of
    // allowing a ZWJ to join two independently measured header cells.
    let character = if character.is_control() || character == '\u{200d}' {
        '?'
    } else {
        character
    };
    match UnicodeWidthChar::width(character) {
        Some(0) => {
            if let Some(last) = output.last() {
                if last.text.len() + character.len_utf8() <= MAX_GLYPH_BYTES {
                    let old_width = last.width;
                    let mut cluster = last.text.clone();
                    cluster.push(character);
                    let new_width = unicode_width::UnicodeWidthStr::width(cluster.as_str());
                    let total_width = label_width(output) - old_width + new_width;
                    if (1..=2).contains(&new_width) && total_width <= cap {
                        let previous = output.last_mut().expect("last glyph remains present");
                        previous.text.push(character);
                        previous.width = new_width;
                    }
                }
            } else {
                output.push(Glyph {
                    text: "?".to_owned(),
                    width: 1,
                });
            }
        }
        Some(width @ 1..=2) => {
            if label_width(output) + width <= cap && character.len_utf8() <= MAX_GLYPH_BYTES {
                output.push(Glyph {
                    text: character.to_string(),
                    width,
                });
            }
        }
        _ => {
            if label_width(output) < cap {
                output.push(Glyph {
                    text: "?".to_owned(),
                    width: 1,
                });
            }
        }
    }
}

fn label_width(label: &[Glyph]) -> usize {
    label.iter().map(|glyph| glyph.width).sum()
}

fn place_label(
    cells: &mut [HeaderCell],
    start: usize,
    label: &[Glyph],
    selected: bool,
    cell_budget: usize,
) -> usize {
    let mut written = 0;
    for glyph in label {
        let Some(cell) = HeaderCell::glyph(&glyph.text, selected) else {
            continue;
        };
        // HeaderCell validates the complete UTF-8 cluster using the same
        // Unicode-width policy as the compositor. Do not trust a stale or
        // scalar-derived width when deciding whether the cell fits.
        let width = if cell.is_wide() { 2 } else { 1 };
        if written + width > cell_budget || start + written + width > cells.len() {
            break;
        }
        cells[start + written] = cell;
        if width == 2 {
            cells[start + written + 1] = HeaderCell::continuation(selected);
        }
        written += width;
    }
    written
}

fn emit_control(
    cells: &mut [HeaderCell],
    hits: &mut Vec<HitRegion>,
    at: usize,
    glyph: char,
    action: TabAction,
) {
    if let Some(cell) = HeaderCell::glyph(&glyph.to_string(), false) {
        cells[at] = cell;
        hits.push(HitRegion {
            start: at as u16,
            end: at as u16 + 1,
            action,
        });
    }
}

/// Width from which the header uses margins and full button labels.
const ROOMY_HEADER_COLUMNS: usize = 60;
/// Width from which the header uses wider margins and gaps.
const SPACIOUS_HEADER_COLUMNS: usize = 120;

/// Margins and gaps of the strip for one host width tier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HeaderSpacing {
    /// Blank cells before the project label and after the last button.
    margin: usize,
    /// Gap between the project label and the first tab.
    project_gap: usize,
    /// Gap between neighbouring tabs.
    tab_gap: usize,
    /// Gap between right-hand button groups.
    group_gap: usize,
    /// Gap between the previous and next buttons.
    nav_gap: usize,
}

impl HeaderSpacing {
    fn for_width(width: usize) -> Self {
        if width >= SPACIOUS_HEADER_COLUMNS {
            Self {
                margin: 2,
                project_gap: 4,
                tab_gap: 3,
                group_gap: 4,
                nav_gap: 2,
            }
        } else if width >= ROOMY_HEADER_COLUMNS {
            Self {
                margin: 1,
                project_gap: 2,
                tab_gap: 2,
                group_gap: 2,
                nav_gap: 1,
            }
        } else {
            Self {
                margin: 0,
                project_gap: 2,
                tab_gap: 2,
                group_gap: 1,
                nav_gap: 0,
            }
        }
    }
}
const CREATE_LABEL: &str = "[+ New]";
const ALL_LABEL: &str = "[All tabs]";

/// Place an ASCII button and its hit region; returns its start column.
fn place_button(
    cells: &mut [HeaderCell],
    hits: &mut Vec<HitRegion>,
    end: usize,
    text: &str,
    action: TabAction,
) -> Option<usize> {
    let start = end.checked_sub(text.len())?;
    place_ascii(cells, start, text);
    hits.push(HitRegion {
        start: start as u16,
        end: end as u16,
        action,
    });
    Some(start)
}

/// The create button, right-aligned inside the tier's margin on roomy hosts.
/// Returns its first column.
fn place_create(cells: &mut [HeaderCell], hits: &mut Vec<HitRegion>) -> usize {
    let width = cells.len();
    if width >= ROOMY_HEADER_COLUMNS {
        let end = width - HeaderSpacing::for_width(width).margin;
        if let Some(start) = place_button(cells, hits, end, CREATE_LABEL, TabAction::Create) {
            return start;
        }
    }
    if width >= 12 {
        if let Some(start) = place_button(cells, hits, width, "[+]", TabAction::Create) {
            return start;
        }
    }
    emit_control(cells, hits, width - 1, '+', TabAction::Create);
    width - 1
}

/// All tabs, New tab and previous/next controls from the right edge. Returns
/// the first column used by the controls.
fn place_right_controls(
    cells: &mut [HeaderCell],
    hits: &mut Vec<HitRegion>,
    navigation: bool,
) -> usize {
    let width = cells.len();
    let roomy = width >= ROOMY_HEADER_COLUMNS;
    let spacing = HeaderSpacing::for_width(width);
    let gap = spacing.group_gap;
    let mut start = if width >= ROOMY_HEADER_COLUMNS {
        place_button(
            cells,
            hits,
            width - spacing.margin,
            ALL_LABEL,
            TabAction::AllTabs,
        )
        .unwrap()
    } else if width >= 18 {
        place_button(cells, hits, width, " All ", TabAction::AllTabs).unwrap()
    } else {
        place_create(cells, hits)
    };
    if width >= 18 {
        let label = if roomy {
            CREATE_LABEL
        } else if width >= 30 {
            " New "
        } else {
            " + "
        };
        if let Some(next) = start
            .checked_sub(gap)
            .and_then(|end| place_button(cells, hits, end, label, TabAction::Create))
        {
            start = next;
        }
    }
    if width >= 60 && navigation {
        let (previous, next) = if roomy { ("[<]", "[>]") } else { ("<", ">") };
        let inner = spacing.nav_gap;
        if let Some(next_start) = start
            .checked_sub(gap)
            .and_then(|end| place_button(cells, hits, end, next, TabAction::NextTab))
        {
            start = next_start;
            if let Some(previous_start) = start
                .checked_sub(inner)
                .and_then(|end| place_button(cells, hits, end, previous, TabAction::PreviousTab))
            {
                start = previous_start;
            }
        }
    }
    start
}

fn place_ascii(cells: &mut [HeaderCell], start: usize, text: &str) {
    for (offset, character) in text.chars().enumerate() {
        if let Some(cell) = HeaderCell::glyph(&character.to_string(), false) {
            if let Some(target) = cells.get_mut(start + offset) {
                *target = cell;
            }
        }
    }
}

fn selected_window(
    labels: &[Vec<Glyph>],
    selected: usize,
    budget: usize,
    tab_gap: usize,
) -> (usize, usize) {
    let mut start = selected;
    let mut end = selected + 1;
    let selected_width = (label_width(&labels[selected]) + 4).min(budget);
    let mut used = selected_width;
    loop {
        let mut progressed = false;
        if start > 0 {
            let cost = tab_gap + 4 + label_width(&labels[start - 1]);
            if used + cost <= budget {
                start -= 1;
                used += cost;
                progressed = true;
            }
        }
        if end < labels.len() {
            let cost = tab_gap + 4 + label_width(&labels[end]);
            if used + cost <= budget {
                used += cost;
                end += 1;
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace_index::{ProjectId, WorkspaceIndex, WorkspaceTabId};
    use unicode_width::UnicodeWidthStr;

    fn id(value: u64) -> ProjectId {
        ProjectId::new(value).expect("nonzero project id")
    }

    fn tab(value: u64) -> WorkspaceTabId {
        WorkspaceTabId::new(value).expect("nonzero tab id")
    }

    fn index() -> WorkspaceIndex {
        let mut index = WorkspaceIndex::new();
        index
            .add_project(id(1), "Editors", tab(1), "Main")
            .expect("project");
        index.add_tab(id(1), tab(2), "Review").expect("tab");
        index
            .add_project(id(2), "Empty", tab(3), "Build")
            .expect("project");
        index.remove_tab(id(2), tab(3)).expect("empty project");
        index
            .add_project(id(4), "Tools", tab(4), "Logs")
            .expect("project");
        index
            .add_tab(id(4), tab(5), "Wide 界\u{301}")
            .expect("wide tab");
        for value in 6..=11 {
            index
                .add_tab(id(4), tab(value), &format!("Tab{value}"))
                .expect("tab");
        }
        index
    }

    fn route(project: u64, tab_id: u64) -> WorkspaceRouteId {
        WorkspaceRouteId {
            project: id(project),
            tab: tab(tab_id),
        }
    }

    #[test]
    fn btn04_close_bounds_and_minimum_width_reachability() {
        let index = index();
        for columns in 22..=240 {
            let strip = layout(&index, route(4, 5), 8, columns).unwrap();
            for action in [
                TabAction::Select(route(4, 5)),
                TabAction::Create,
                TabAction::AllTabs,
                TabAction::Close(route(4, 5)),
            ] {
                assert!(
                    strip.hit_regions.iter().any(|hit| hit.action == action),
                    "{columns}: {action:?}"
                );
            }
            for hit in &strip.hit_regions {
                assert!(hit.start < hit.end && hit.end <= columns);
                for x in hit.start..hit.end {
                    assert_eq!(strip.hit_test(x, 0), Some(hit.action));
                }
            }
            assert_header_topology(strip.header_cells());
        }
    }

    #[test]
    fn navigation_wraps_tabs_and_skips_empty_projects_without_mutating_index() {
        let index = index();
        assert_eq!(
            navigate(&index, route(1, 1), TabNavigation::PreviousTab),
            Ok(route(1, 2))
        );
        assert_eq!(
            navigate(&index, route(1, 2), TabNavigation::NextTab),
            Ok(route(1, 1))
        );
        assert_eq!(
            navigate(&index, route(1, 1), TabNavigation::NextProject),
            Ok(route(4, 4))
        );
        assert_eq!(
            navigate(&index, route(4, 4), TabNavigation::PreviousProject),
            Ok(route(1, 1))
        );
        assert_eq!(index.active_project_id(), Some(id(1)));
        assert_eq!(index.active_tab_id(), Some(tab(1)));
    }

    #[test]
    fn navigation_rejects_unknown_route_deterministically() {
        let index = index();
        assert_eq!(
            navigate(&index, route(1, 99), TabNavigation::NextTab),
            Err(TabsError::InvalidRoute)
        );
        assert_eq!(
            navigate(&index, route(99, 1), TabNavigation::NextProject),
            Err(TabsError::InvalidRoute)
        );
        let mut empty = WorkspaceIndex::new();
        empty
            .add_project(id(5), "Before", tab(5), "One")
            .expect("project");
        empty
            .add_project(id(6), "Temp", tab(6), "One")
            .expect("project");
        empty
            .add_project(id(7), "After", tab(7), "One")
            .expect("project");
        empty.select_project(id(7)).expect("select later project");
        empty
            .remove_tab(id(6), tab(6))
            .expect("inactive empty project");
        assert_eq!(
            navigate(&empty, route(5, 5), TabNavigation::NextProject),
            Ok(route(7, 7))
        );
    }

    #[test]
    fn geometry_clamps_and_uses_header_plus_existing_outer_minimum() {
        let index = index();
        let exact = layout(&index, route(1, 1), 8, MIN_PANE_COLUMNS).expect("layout");
        assert_eq!(exact.rows(), 8);
        assert_eq!(
            exact.pane_root(),
            Rect {
                x: 0,
                y: 1,
                rows: MIN_PANE_ROWS,
                columns: MIN_PANE_COLUMNS
            }
        );
        assert!(exact.fits_single_pane());
        assert!(!layout(&index, route(1, 1), 7, MIN_PANE_COLUMNS)
            .expect("layout")
            .fits_single_pane());
        assert!(!layout(&index, route(1, 1), 8, MIN_PANE_COLUMNS - 1)
            .expect("layout")
            .fits_single_pane());
        let clamped = layout(&index, route(1, 1), 100, 500).expect("layout");
        assert_eq!((clamped.rows(), clamped.columns()), (80, 240));
        assert_eq!(clamped.header_cells().len(), 240);
    }

    #[test]
    fn zero_and_one_cell_viewports_have_no_invalid_hits_or_half_glyphs() {
        let index = index();
        for (rows, columns) in [(0, 20), (5, 0)] {
            let rendered = super::layout(&index, route(1, 1), rows, columns).expect("empty layout");
            assert!(rendered.header_cells().is_empty());
            assert_eq!(
                rendered.pane_root(),
                Rect {
                    x: 0,
                    y: 0,
                    rows: 0,
                    columns: 0
                }
            );
            assert_eq!(rendered.hit_test(0, 0), None);
        }
        let one = super::layout(&index, route(1, 1), 1, 1).expect("one cell");
        assert_eq!(one.header_cells().len(), 1);
        assert_eq!(
            one.pane_root(),
            Rect {
                x: 0,
                y: 1,
                rows: 0,
                columns: 1
            }
        );
        assert!(one
            .header_cells()
            .iter()
            .all(|cell| !cell.is_continuation()));
        assert_eq!(one.hit_test(1, 0), None);
    }

    #[test]
    fn hits_are_exact_typed_regions_and_never_cover_pane_rows() {
        let index = index();
        let rendered = super::layout(&index, route(1, 1), 30, 100).expect("layout");
        // Roomy hosts keep a one-cell left margin before the project button.
        assert_eq!(rendered.hit_test(0, 0), None);
        assert_eq!(rendered.hit_test(1, 0), Some(TabAction::ProjectChooser));
        let tab_region = rendered
            .hit_regions
            .iter()
            .find(|region| region.action == TabAction::Select(route(1, 1)))
            .expect("selected tab region");
        assert_eq!(
            rendered.hit_test(tab_region.start, 0),
            Some(TabAction::Select(route(1, 1)))
        );
        for region in &rendered.hit_regions {
            assert_eq!(rendered.hit_test(region.start, 0), Some(region.action));
        }
        assert_eq!(rendered.hit_test(0, 1), None);
        assert_eq!(rendered.hit_test(100, 0), None);
        assert_eq!(rendered.hit_test(0, 2), None);
    }

    #[test]
    fn overflow_keeps_current_route_visible_and_wide_glyphs_whole() {
        let index = index();
        let rendered = super::layout(&index, route(4, 5), 20, 36).expect("layout");
        let selected = rendered
            .hit_regions
            .iter()
            .find(|region| region.action == TabAction::Select(route(4, 5)))
            .expect("selected tab region");
        assert_eq!(
            rendered.hit_test(selected.start, 0),
            Some(TabAction::Select(route(4, 5)))
        );
        for (column, cell) in rendered.header_cells().iter().enumerate() {
            if cell.is_wide() {
                assert!(rendered
                    .header_cells()
                    .get(column + 1)
                    .is_some_and(HeaderCell::is_continuation));
            }
            if cell.is_continuation() {
                assert!(column > 0 && rendered.header_cells()[column - 1].is_wide());
            }
        }
        let overflow = super::layout(&index, route(4, 8), 20, 36).expect("overflow");
        let current = overflow
            .hit_regions
            .iter()
            .find(|region| region.action == TabAction::Select(route(4, 8)))
            .expect("selected tab remains visible");
        assert_eq!(
            overflow.hit_test(current.start, 0),
            Some(TabAction::Select(route(4, 8)))
        );
        assert!(overflow
            .hit_regions
            .iter()
            .any(|r| r.action == TabAction::AllTabs));
        let narrow = super::layout(&index, route(4, 5), 4, 30).expect("clipped");
        assert_eq!(narrow.header_cells().len(), 30);
        assert_header_topology(narrow.header_cells());
    }

    #[test]
    fn combining_marks_are_grouped_with_the_preceding_glyph() {
        let glyphs = bracketed_label("[", "e\u{301}", "]", MAX_LABEL_CELLS);
        let grouped = glyphs
            .iter()
            .find(|glyph| glyph.text.starts_with('e'))
            .expect("base glyph");
        assert_eq!(grouped.text, "e\u{301}");
        assert_eq!(UnicodeWidthStr::width(grouped.text.as_str()), grouped.width);
        assert!(grouped.text.len() <= MAX_GLYPH_BYTES);
    }

    fn assert_header_topology(cells: &[HeaderCell]) {
        for (column, cell) in cells.iter().enumerate() {
            if !cell.text().is_empty() {
                assert!(HeaderCell::glyph(cell.text(), cell.selected()).is_some());
            }
            if cell.is_wide() {
                assert!(cells
                    .get(column + 1)
                    .is_some_and(HeaderCell::is_continuation));
            }
            if cell.is_continuation() {
                assert!(column > 0 && cells[column - 1].is_wide());
            }
        }
    }

    #[test]
    fn unicode_clusters_keep_compositor_topology_at_every_host_width() {
        let mut index = WorkspaceIndex::new();
        index
            .add_project(id(20), "Heart ❤️ e\u{301}", tab(20), "👩\u{200d}💻")
            .expect("unicode project");
        index
            .add_tab(id(20), tab(21), &format!("x{}界", "\u{301}".repeat(25)))
            .expect("long combining tab");

        for columns in 0..=MAX_COLUMNS {
            let rendered =
                super::layout(&index, route(20, 20), 12, columns).expect("unicode layout");
            assert_eq!(rendered.header_cells().len(), usize::from(columns));
            assert_header_topology(rendered.header_cells());
        }

        let heart = bracketed_label("", "❤️", "", MAX_LABEL_CELLS);
        let heart = heart.first().expect("heart cluster");
        assert_eq!(heart.width, UnicodeWidthStr::width(heart.text.as_str()));
        assert_eq!(heart.width, 2);

        let zwj = bracketed_label("", "👩\u{200d}💻", "", MAX_LABEL_CELLS);
        assert!(zwj.iter().all(|glyph| !glyph.text.contains('\u{200d}')));
        assert!(zwj.iter().any(|glyph| glyph.text == "?"));

        let long_marks = bracketed_label(
            "",
            &format!("e{}", "\u{301}".repeat(40)),
            "",
            MAX_LABEL_CELLS,
        );
        assert!(long_marks
            .iter()
            .all(|glyph| glyph.text.len() <= MAX_GLYPH_BYTES));
        assert!(long_marks.iter().all(|glyph| {
            (1..=2).contains(&UnicodeWidthStr::width(glyph.text.as_str()))
                && glyph.width == UnicodeWidthStr::width(glyph.text.as_str())
        }));
    }

    #[test]
    fn narrow_overflow_keeps_active_tab_create_and_navigation_reachable() {
        let mut index = WorkspaceIndex::new();
        let project = id(40);
        let active = tab(47);
        index
            .add_project(
                project,
                "A very long project label",
                tab(40),
                "A very long first tab",
            )
            .expect("project");
        for value in 41..=47 {
            let name = format!("Long workspace tab name {value}");
            index
                .add_tab(project, tab(value), &name)
                .expect("workspace tab");
        }
        let current = route(40, active.get());

        for columns in [40, 80, 120, 240] {
            let rendered = layout(&index, current, 24, columns).expect("layout");
            let selected = rendered
                .hit_regions
                .iter()
                .find(|region| region.action == TabAction::Select(current))
                .expect("active tab remains reachable");
            assert_eq!(
                rendered.hit_test(selected.start, 0),
                Some(TabAction::Select(current))
            );
            assert!(rendered.header_cells()[usize::from(selected.start)].selected());
            // Create is the rightmost control; roomy hosts keep a one-cell
            // right margin after its button.
            let create_end = columns
                - if usize::from(columns) >= ROOMY_HEADER_COLUMNS {
                    HeaderSpacing::for_width(usize::from(columns)).margin as u16
                } else {
                    0
                };
            assert_eq!(
                rendered.hit_test(create_end - 1, 0),
                Some(TabAction::AllTabs),
                "All tabs is the rightmost control at width {columns}"
            );
            assert_eq!(rendered.hit_test(create_end, 0), None);
            assert!(rendered
                .hit_regions
                .iter()
                .any(|region| region.action == TabAction::AllTabs));
            if columns >= 60 {
                assert!(rendered
                    .hit_regions
                    .iter()
                    .any(|region| region.action == TabAction::PreviousTab));
            }
            if columns >= 60 {
                assert!(rendered
                    .hit_regions
                    .iter()
                    .any(|region| region.action == TabAction::NextTab));
            }
            assert_eq!(rendered.header_cells().len(), usize::from(columns));
        }
    }

    #[test]
    fn roomy_header_uses_labelled_buttons_with_spacing() {
        let index = index();
        let rendered = super::layout(&index, route(4, 5), 24, 100).expect("layout");
        let text = header_text(&rendered);
        assert!(text.ends_with("[All tabs] "), "{text:?}");
        assert!(text.contains("[+ New]  [All tabs]"), "{text:?}");
        assert!(text.contains("[<] [>]  [+ New]"), "{text:?}");
        assert!(text.starts_with(" [ "), "{text:?}");
    }

    fn header_text(rendered: &TabsLayout) -> String {
        rendered
            .header_cells()
            .iter()
            .map(|cell| {
                if cell.text().is_empty() {
                    " "
                } else {
                    cell.text()
                }
            })
            .collect()
    }

    #[test]
    fn spacious_header_widens_margins_and_gaps() {
        let mut index = WorkspaceIndex::new();
        index
            .add_project(id(1), "Default", tab(1), "Main")
            .expect("project");
        index.add_tab(id(1), tab(2), "Shell 1").expect("tab");
        for columns in [120, 269] {
            let rendered = super::layout(&index, route(1, 1), 24, columns).expect("layout");
            let text = header_text(&rendered);
            assert!(text.starts_with("  [ Default ]     Main   x "), "{text:?}");
            assert!(text.contains(" x     Shell 1   x "), "{text:?}");
            assert!(
                text.ends_with("[<]  [>]    [+ New]    [All tabs]  "),
                "{text:?}"
            );
            for hit in &rendered.hit_regions {
                for other in &rendered.hit_regions {
                    if hit != other {
                        assert!(hit.end <= other.start || other.end <= hit.start);
                    }
                }
            }
        }
        // Just below the tier keeps the roomy spacing.
        let text = header_text(&super::layout(&index, route(1, 1), 24, 119).expect("layout"));
        assert!(text.ends_with("[<] [>]  [+ New]  [All tabs] "), "{text:?}");
        assert!(text.starts_with(" [ Default ]   Main   x "), "{text:?}");
    }
}
