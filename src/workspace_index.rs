//! Pure metadata model for projects and their workspace tabs.
//!
//! This module owns no storage or runtime state. Removing a tab only removes
//! its metadata and never authorizes stopping or dispatching to a process.

use std::fmt;

pub mod codec;
pub mod store;

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
