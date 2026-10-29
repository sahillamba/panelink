//! Pure metadata model for projects and their workspace tabs.
//!
//! This module owns no storage or runtime state. Removing a tab only removes
//! its metadata and never authorizes stopping or dispatching to a process.

use std::fmt;

pub mod codec;
pub mod store;
pub mod tab_visibility;

pub const MAX_PROJECTS: usize = 32;
pub const MAX_TABS_PER_PROJECT: usize = 8;
pub const MIN_DISPLAY_NAME_BYTES: usize = 1;
pub const MAX_DISPLAY_NAME_BYTES: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectId(u64);

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkspaceTabId(u64);

impl ProjectId {
    pub fn new(value: u64) -> Result<Self, WorkspaceIndexError> {
        if value == 0 {
            Err(WorkspaceIndexError::InvalidId)
        } else {
            Ok(Self(value))
        }
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl WorkspaceTabId {
    pub fn new(value: u64) -> Result<Self, WorkspaceIndexError> {
        if value == 0 {
            Err(WorkspaceIndexError::InvalidId)
        } else {
            Ok(Self(value))
        }
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Debug for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ProjectId").field(&self.0).finish()
    }
}

impl fmt::Debug for WorkspaceTabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("WorkspaceTabId").field(&self.0).finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceIndexError {
    InvalidId,
    InvalidName,
    DuplicateProjectId,
    DuplicateTabId,
    UnknownProject,
    UnknownTab,
    ProjectLimitReached,
    TabLimitReached,
    ProjectHasTabs,
    ActiveProjectCannotBeEmpty,
    ProjectHasNoTabs,
    ReorderIndexOutOfRange,
    ReservedRouteRequiresSidecar,
    NoFallbackRoute,
}

#[derive(Clone, PartialEq, Eq)]
struct Project {
    id: ProjectId,
    display_name: String,
    tabs: Vec<WorkspaceTab>,
    selected_tab_id: Option<WorkspaceTabId>,
}

#[derive(Clone, PartialEq, Eq)]
struct WorkspaceTab {
    id: WorkspaceTabId,
    display_name: String,
}

/// Stable-identity project/workspace metadata and current selection.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct WorkspaceIndex {
    projects: Vec<Project>,
    active_project_id: Option<ProjectId>,
}

/// Borrowed tab metadata, in project order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceTabView<'a> {
    pub id: WorkspaceTabId,
    pub display_name: &'a str,
}

/// Borrowed project metadata, including its ordered tabs and selected tab.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ProjectView<'a> {
    project: &'a Project,
}

impl fmt::Debug for WorkspaceTabView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkspaceTabView")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for ProjectView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectView")
            .field("id", &self.project.id)
            .field("tab_count", &self.project.tabs.len())
            .field("selected_tab_id", &self.project.selected_tab_id)
            .finish_non_exhaustive()
    }
}

impl<'a> ProjectView<'a> {
    pub const fn id(&self) -> ProjectId {
        self.project.id
    }

    pub fn display_name(&self) -> &'a str {
        &self.project.display_name
    }

    pub fn selected_tab_id(&self) -> Option<WorkspaceTabId> {
        self.project.selected_tab_id
    }

    pub fn tabs(&self) -> impl ExactSizeIterator<Item = WorkspaceTabView<'a>> {
        self.project.tabs.iter().map(|tab| WorkspaceTabView {
            id: tab.id,
            display_name: &tab.display_name,
        })
    }
}

impl fmt::Debug for WorkspaceIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkspaceIndex")
            .field("project_count", &self.projects.len())
            .field("active_project_id", &self.active_project_id)
            .finish()
    }
}

impl WorkspaceIndex {
    pub const fn new() -> Self {
        Self {
            projects: Vec::new(),
            active_project_id: None,
        }
    }

    pub fn add_project(
        &mut self,
        id: ProjectId,
        display_name: &str,
        initial_tab_id: WorkspaceTabId,
        initial_tab_name: &str,
    ) -> Result<(), WorkspaceIndexError> {
        validate_name(display_name)?;
        validate_name(initial_tab_name)?;
        if self.projects.iter().any(|project| project.id == id) {
            return Err(WorkspaceIndexError::DuplicateProjectId);
        }
        if self.contains_tab_id(initial_tab_id) {
            return Err(WorkspaceIndexError::DuplicateTabId);
        }
        if self.projects.len() == MAX_PROJECTS {
            return Err(WorkspaceIndexError::ProjectLimitReached);
        }
        self.projects.push(Project {
            id,
            display_name: display_name.to_owned(),
            tabs: vec![WorkspaceTab {
                id: initial_tab_id,
                display_name: initial_tab_name.to_owned(),
            }],
            selected_tab_id: Some(initial_tab_id),
        });
        if self.active_project_id.is_none() {
            self.active_project_id = Some(id);
        }
        Ok(())
    }

    pub fn rename_project(
        &mut self,
        id: ProjectId,
        display_name: &str,
    ) -> Result<(), WorkspaceIndexError> {
        validate_name(display_name)?;
        let project = self.project_mut(id)?;
        project.display_name = display_name.to_owned();
        Ok(())
    }

    pub fn remove_project(&mut self, id: ProjectId) -> Result<(), WorkspaceIndexError> {
        let index = self.project_index(id)?;
        if !self.projects[index].tabs.is_empty() {
            return Err(WorkspaceIndexError::ProjectHasTabs);
        }
        if self.active_project_id == Some(id) {
            return Err(WorkspaceIndexError::ActiveProjectCannotBeEmpty);
        }
        self.projects.remove(index);
        Ok(())
    }

    pub fn add_tab(
        &mut self,
        project_id: ProjectId,
        id: WorkspaceTabId,
        display_name: &str,
    ) -> Result<(), WorkspaceIndexError> {
        validate_name(display_name)?;
        if self.contains_tab_id(id) {
            return Err(WorkspaceIndexError::DuplicateTabId);
        }
        let project = self.project_mut(project_id)?;
        if project.tabs.len() == MAX_TABS_PER_PROJECT {
            return Err(WorkspaceIndexError::TabLimitReached);
        }
        project.tabs.push(WorkspaceTab {
            id,
            display_name: display_name.to_owned(),
        });
        Ok(())
    }

    pub fn rename_tab(
        &mut self,
        project_id: ProjectId,
        tab_id: WorkspaceTabId,
        display_name: &str,
    ) -> Result<(), WorkspaceIndexError> {
        validate_name(display_name)?;
        let project = self.project_mut(project_id)?;
        let tab = project
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .ok_or(WorkspaceIndexError::UnknownTab)?;
        tab.display_name = display_name.to_owned();
        Ok(())
    }

    /// Remove only metadata and return the stable ID to the lifecycle caller.
    pub fn remove_tab(
        &mut self,
        project_id: ProjectId,
        tab_id: WorkspaceTabId,
    ) -> Result<WorkspaceTabId, WorkspaceIndexError> {
        let removing_active_project = self.active_project_id == Some(project_id);
        let project = self.project_mut(project_id)?;
        let index = project
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .ok_or(WorkspaceIndexError::UnknownTab)?;
        if project.tabs.len() == 1 && removing_active_project {
            return Err(WorkspaceIndexError::ActiveProjectCannotBeEmpty);
        }
        project.tabs.remove(index);
        if project.selected_tab_id == Some(tab_id) {
            project.selected_tab_id = project
                .tabs
                .get(index)
                .or_else(|| {
                    index
                        .checked_sub(1)
                        .and_then(|previous| project.tabs.get(previous))
                })
                .map(|tab| tab.id);
        }
        Ok(tab_id)
    }

    /// Move a tab to its final zero-based position within its project.
    pub fn reorder_tab(
        &mut self,
        project_id: ProjectId,
        tab_id: WorkspaceTabId,
        final_index: usize,
    ) -> Result<(), WorkspaceIndexError> {
        let project = self.project_mut(project_id)?;
        let current_index = project
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .ok_or(WorkspaceIndexError::UnknownTab)?;
        if final_index >= project.tabs.len() {
            return Err(WorkspaceIndexError::ReorderIndexOutOfRange);
        }
        let tab = project.tabs.remove(current_index);
        project.tabs.insert(final_index, tab);
        Ok(())
    }

    pub fn select_project(&mut self, id: ProjectId) -> Result<(), WorkspaceIndexError> {
        let project = self.project_mut(id)?;
        if project.tabs.is_empty() {
            return Err(WorkspaceIndexError::ProjectHasNoTabs);
        }
        if project.selected_tab_id.is_none() {
            project.selected_tab_id = project.tabs.first().map(|tab| tab.id);
        }
        self.active_project_id = Some(id);
        Ok(())
    }

    pub fn select_tab(
        &mut self,
        project_id: ProjectId,
        tab_id: WorkspaceTabId,
    ) -> Result<(), WorkspaceIndexError> {
        let project = self.project_mut(project_id)?;
        if !project.tabs.iter().any(|tab| tab.id == tab_id) {
            return Err(WorkspaceIndexError::UnknownTab);
        }
        project.selected_tab_id = Some(tab_id);
        self.active_project_id = Some(project_id);
        Ok(())
    }

    pub fn active_project_id(&self) -> Option<ProjectId> {
        self.active_project_id
    }

    pub fn active_tab_id(&self) -> Option<WorkspaceTabId> {
        self.active_project_id
            .and_then(|id| self.project_ref(id).ok())
            .and_then(|project| project.selected_tab_id)
    }

    pub fn project_count(&self) -> usize {
        self.projects.len()
    }

    pub fn project(&self, id: ProjectId) -> Result<ProjectView<'_>, WorkspaceIndexError> {
        Ok(ProjectView {
            project: self.project_ref(id)?,
        })
    }

    pub fn projects(&self) -> impl ExactSizeIterator<Item = ProjectView<'_>> {
        self.projects.iter().map(|project| ProjectView { project })
    }

    pub fn tabs(
        &self,
        project_id: ProjectId,
    ) -> Result<impl ExactSizeIterator<Item = WorkspaceTabView<'_>>, WorkspaceIndexError> {
        Ok(self
            .project_ref(project_id)?
            .tabs
            .iter()
            .map(|tab| WorkspaceTabView {
                id: tab.id,
                display_name: &tab.display_name,
            }))
    }

    fn project_index(&self, id: ProjectId) -> Result<usize, WorkspaceIndexError> {
        self.projects
            .iter()
            .position(|project| project.id == id)
            .ok_or(WorkspaceIndexError::UnknownProject)
    }

    fn project_ref(&self, id: ProjectId) -> Result<&Project, WorkspaceIndexError> {
        Ok(&self.projects[self.project_index(id)?])
    }

    fn project_mut(&mut self, id: ProjectId) -> Result<&mut Project, WorkspaceIndexError> {
        let index = self.project_index(id)?;
        Ok(&mut self.projects[index])
    }

    fn contains_tab_id(&self, id: WorkspaceTabId) -> bool {
        self.projects
            .iter()
            .any(|project| project.tabs.iter().any(|tab| tab.id == id))
    }
}

/// One visible tab in a presentation snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabPresentationTab {
    pub route: crate::workspace_tabs::WorkspaceRouteId,
    pub display_name: String,
}

/// One project and its visible tabs, retaining workspace-index ordering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabPresentationProject {
    pub id: ProjectId,
    pub display_name: String,
    pub tabs: Vec<TabPresentationTab>,
}

/// Visible tab state projected from the v1 catalog and reserved-route sidecar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabPresentationSnapshot {
    pub projects: Vec<TabPresentationProject>,
    pub active_project: Option<ProjectId>,
    pub active_route: Option<crate::workspace_tabs::WorkspaceRouteId>,
}

impl TabPresentationSnapshot {
    /// Projects with no visible tabs are omitted. A hidden reserved selection
    /// falls back to the first visible route; no visible route yields no active
    /// project or route.
    pub fn new(index: &WorkspaceIndex, reserved_hidden: bool) -> Self {
        use crate::workspace_tabs::WorkspaceRouteId;

        let projects: Vec<_> = index
            .projects
            .iter()
            .filter_map(|project| {
                let tabs: Vec<_> = project
                    .tabs
                    .iter()
                    .filter(|tab| !(reserved_hidden && project.id.get() == 1 && tab.id.get() == 1))
                    .map(|tab| TabPresentationTab {
                        route: WorkspaceRouteId {
                            project: project.id,
                            tab: tab.id,
                        },
                        display_name: tab.display_name.clone(),
                    })
                    .collect();
                (!tabs.is_empty()).then(|| TabPresentationProject {
                    id: project.id,
                    display_name: project.display_name.clone(),
                    tabs,
                })
            })
            .collect();

        let selected = index.active_project_id.and_then(|active_id| {
            let project = index
                .projects
                .iter()
                .find(|project| project.id == active_id)?;
            let selected_tab = project.selected_tab_id?;
            (!reserved_hidden || active_id.get() != 1 || selected_tab.get() != 1).then_some(
                WorkspaceRouteId {
                    project: active_id,
                    tab: selected_tab,
                },
            )
        });
        let active_route = selected.or_else(|| {
            projects
                .first()
                .and_then(|project| project.tabs.first())
                .map(|tab| tab.route)
        });
        let active_project = active_route.map(|route| route.project);

        Self {
            projects,
            active_project,
            active_route,
        }
    }
}

/// Return the smallest positive `Shell N` not already used in `project`.
pub fn next_shell_name(index: &WorkspaceIndex, project_id: ProjectId) -> String {
    let project = index
        .project_ref(project_id)
        .expect("project must exist in the workspace index");
    let used: Vec<_> = project
        .tabs
        .iter()
        .map(|tab| tab.display_name.as_str())
        .collect();
    let number = (1..)
        .find(|number| {
            let candidate = format!("Shell {number}");
            !used.iter().any(|name| candidate.eq_ignore_ascii_case(name))
        })
        .expect("a finite tab set cannot occupy every shell name");
    format!("Shell {number}")
}

/// Plan metadata removal for a non-reserved tab without authorizing runtime IO.
pub fn plan_close_removal(
    index: &WorkspaceIndex,
    project_id: ProjectId,
    tab_id: WorkspaceTabId,
) -> Result<WorkspaceIndex, WorkspaceIndexError> {
    use crate::workspace_tabs::WorkspaceRouteId;

    if project_id.get() == 1 && tab_id.get() == 1 {
        return Err(WorkspaceIndexError::ReservedRouteRequiresSidecar);
    }
    let project_index = index.project_index(project_id)?;
    let project = &index.projects[project_index];
    let tab_index = project
        .tabs
        .iter()
        .position(|tab| tab.id == tab_id)
        .ok_or(WorkspaceIndexError::UnknownTab)?;
    let closing_active_only_tab =
        index.active_project_id == Some(project_id) && project.tabs.len() == 1;

    let mut next = index.clone();
    if closing_active_only_tab {
        let first_nonreserved = |candidate: &Project| {
            candidate
                .tabs
                .iter()
                .find(|tab| !(candidate.id.get() == 1 && tab.id.get() == 1))
                .map(|tab| WorkspaceRouteId {
                    project: candidate.id,
                    tab: tab.id,
                })
        };
        let adjacent = index
            .projects
            .iter()
            .skip(project_index + 1)
            .find_map(first_nonreserved)
            .or_else(|| {
                index.projects[..project_index]
                    .iter()
                    .rev()
                    .find_map(first_nonreserved)
            });
        let fallback = adjacent.or_else(|| {
            index.projects.iter().find_map(|candidate| {
                candidate
                    .tabs
                    .iter()
                    .find(|tab| candidate.id.get() == 1 && tab.id.get() == 1)
                    .map(|tab| WorkspaceRouteId {
                        project: candidate.id,
                        tab: tab.id,
                    })
            })
        });
        let fallback = fallback.ok_or(WorkspaceIndexError::NoFallbackRoute)?;
        next.select_tab(fallback.project, fallback.tab)?;
    }

    // The selected-only case was redirected above, allowing the v1 mutation to
    // leave the old project empty only after another project owns selection.
    next.remove_tab(project_id, tab_id)?;
    debug_assert!(tab_index < project.tabs.len());
    Ok(next)
}

fn validate_name(name: &str) -> Result<(), WorkspaceIndexError> {
    let bytes = name.len();
    if !(MIN_DISPLAY_NAME_BYTES..=MAX_DISPLAY_NAME_BYTES).contains(&bytes)
        || name.trim().is_empty()
        || name.trim() != name
        || name.chars().any(char::is_control)
    {
        return Err(WorkspaceIndexError::InvalidName);
    }
    Ok(())
}
