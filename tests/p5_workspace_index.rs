use panelink::workspace_index::{
    ProjectId, WorkspaceIndex, WorkspaceIndexError as Error, WorkspaceTabId,
    MAX_DISPLAY_NAME_BYTES, MAX_PROJECTS, MAX_TABS_PER_PROJECT,
};

fn pid(value: u64) -> ProjectId {
    ProjectId::new(value).unwrap()
}
fn tid(value: u64) -> WorkspaceTabId {
    WorkspaceTabId::new(value).unwrap()
}

fn populated() -> WorkspaceIndex {
    let mut index = WorkspaceIndex::new();
    index.add_project(pid(1), "Alpha", tid(11), "One").unwrap();
    index.add_tab(pid(1), tid(12), "Two").unwrap();
    index.add_tab(pid(1), tid(13), "Three").unwrap();
    index.add_project(pid(2), "Beta", tid(21), "First").unwrap();
    index.add_tab(pid(2), tid(22), "Second").unwrap();
    index
}

#[test]
fn ids_are_typed_nonzero_and_debug_does_not_include_names() {
    assert_eq!(ProjectId::new(0), Err(Error::InvalidId));
    assert_eq!(WorkspaceTabId::new(0), Err(Error::InvalidId));
    assert_eq!(pid(7).get(), 7);
    assert_eq!(tid(8).get(), 8);
    let index = populated();
    let debug = format!("{index:?}");
    assert!(!debug.contains("Alpha"));
    assert!(!debug.contains("First"));
}

#[test]
fn project_and_tab_selection_are_independent_of_identity_name_and_order() {
    let mut index = populated();
    assert_eq!(index.active_project_id(), Some(pid(1)));
    assert_eq!(index.active_tab_id(), Some(tid(11)));
    index.select_tab(pid(1), tid(12)).unwrap();
    index.select_project(pid(2)).unwrap();
    assert_eq!(index.active_tab_id(), Some(tid(21)));
    index.select_tab(pid(2), tid(22)).unwrap();
    index.rename_project(pid(2), "Renamed Beta").unwrap();
    index.rename_tab(pid(2), tid(22), "Renamed tab").unwrap();
    index.reorder_tab(pid(2), tid(22), 0).unwrap();
    assert_eq!(index.active_project_id(), Some(pid(2)));
    assert_eq!(index.active_tab_id(), Some(tid(22)));
    assert_eq!(
        index
            .projects()
            .map(|project| project.id())
            .collect::<Vec<_>>(),
        vec![pid(1), pid(2)]
    );
    let beta = index.project(pid(2)).unwrap();
    assert_eq!(beta.display_name(), "Renamed Beta");
    assert_eq!(
        beta.tabs().map(|tab| tab.id).collect::<Vec<_>>(),
        vec![tid(22), tid(21)]
    );
    assert_eq!(beta.tabs().next().unwrap().display_name, "Renamed tab");
    let view_debug = format!("{beta:?}");
    assert!(!view_debug.contains("Renamed Beta"));
    assert!(!view_debug.contains("Renamed tab"));
    let alpha = index.project(pid(1)).unwrap();
    assert_eq!(alpha.selected_tab_id(), Some(tid(12)));
    assert_eq!(
        alpha.tabs().map(|tab| tab.id).collect::<Vec<_>>(),
        vec![tid(11), tid(12), tid(13)]
    );
}

#[test]
fn names_accept_utf8_byte_boundaries_without_normalization() {
    let mut index = WorkspaceIndex::new();
    let sixty_four_bytes = "é".repeat(MAX_DISPLAY_NAME_BYTES / 2);
    index
        .add_project(pid(1), &sixty_four_bytes, tid(1), "é")
        .unwrap();
    index.rename_tab(pid(1), tid(1), "界界").unwrap();
    assert_eq!(
        index.project(pid(1)).unwrap().display_name(),
        sixty_four_bytes
    );
    assert_eq!(
        index.tabs(pid(1)).unwrap().next().unwrap().display_name,
        "界界"
    );
    for invalid in [
        "",
        " \t",
        " leading",
        "trailing ",
        "line\nbreak",
        "nul\0",
        &"x".repeat(65),
    ] {
        let before = index.clone();
        assert_eq!(
            index.add_tab(pid(1), tid(2), invalid),
            Err(Error::InvalidName)
        );
        assert_eq!(index, before);
        assert_eq!(
            index.rename_project(pid(1), invalid),
            Err(Error::InvalidName)
        );
        assert_eq!(index, before);
    }
}

#[test]
fn duplicate_ids_are_global_and_rejections_are_atomic() {
    let mut index = populated();
    let before = index.clone();
    assert_eq!(
        index.add_project(pid(1), "Other", tid(30), "Tab"),
        Err(Error::DuplicateProjectId)
    );
    assert_eq!(index, before);
    assert_eq!(
        index.add_project(pid(3), "Other", tid(22), "Tab"),
        Err(Error::DuplicateTabId)
    );
    assert_eq!(index, before);
    assert_eq!(
        index.add_tab(pid(2), tid(12), "Cross project duplicate"),
        Err(Error::DuplicateTabId)
    );
    assert_eq!(index, before);
    assert_eq!(
        index.add_tab(pid(88), tid(30), "Unknown project"),
        Err(Error::UnknownProject)
    );
    assert_eq!(index, before);
    assert_eq!(
        index.rename_tab(pid(1), tid(99), "Unknown tab"),
        Err(Error::UnknownTab)
    );
    assert_eq!(index, before);
    assert_eq!(index.select_project(pid(99)), Err(Error::UnknownProject));
    assert_eq!(index.select_tab(pid(1), tid(99)), Err(Error::UnknownTab));
    assert_eq!(index, before);
}

#[test]
fn max_project_and_tab_counts_are_enforced_atomically() {
    let mut tabs = WorkspaceIndex::new();
    tabs.add_project(pid(1), "P", tid(1), "T1").unwrap();
    for number in 2..=MAX_TABS_PER_PROJECT as u64 {
        tabs.add_tab(pid(1), tid(number), "T").unwrap();
    }
    let before = tabs.clone();
    assert_eq!(
        tabs.add_tab(pid(1), tid(9), "Overflow"),
        Err(Error::TabLimitReached)
    );
    assert_eq!(tabs, before);

    let mut projects = WorkspaceIndex::new();
    for number in 1..=MAX_PROJECTS as u64 {
        projects
            .add_project(pid(number), "P", tid(number), "T")
            .unwrap();
    }
    let before = projects.clone();
    assert_eq!(
        projects.add_project(pid(33), "Overflow", tid(33), "T"),
        Err(Error::ProjectLimitReached)
    );
    assert_eq!(projects, before);
}

#[test]
fn remove_selected_tab_chooses_next_then_previous_and_preserves_other_metadata() {
    let mut index = populated();
    index.select_tab(pid(1), tid(12)).unwrap();
    assert_eq!(index.remove_tab(pid(1), tid(12)), Ok(tid(12)));
    assert_eq!(index.active_tab_id(), Some(tid(13)));
    index.select_tab(pid(1), tid(13)).unwrap();
    assert_eq!(index.remove_tab(pid(1), tid(13)), Ok(tid(13)));
    assert_eq!(index.active_tab_id(), Some(tid(11)));
    assert_eq!(
        index
            .project(pid(2))
            .unwrap()
            .tabs()
            .map(|tab| tab.id)
            .collect::<Vec<_>>(),
        vec![tid(21), tid(22)]
    );
}

#[test]
fn inactive_project_can_become_empty_then_removed_but_cannot_be_selected() {
    let mut index = populated();
    assert_eq!(index.remove_tab(pid(2), tid(21)), Ok(tid(21)));
    assert_eq!(index.remove_tab(pid(2), tid(22)), Ok(tid(22)));
    let before = index.clone();
    assert_eq!(index.select_project(pid(2)), Err(Error::ProjectHasNoTabs));
    assert_eq!(index, before);
    assert_eq!(index.remove_project(pid(2)), Ok(()));
    assert_eq!(index.active_project_id(), Some(pid(1)));
    assert_eq!(index.remove_project(pid(1)), Err(Error::ProjectHasTabs));
    assert_eq!(index.project_count(), 1);
}

#[test]
fn final_tab_and_bad_reorder_rejections_leave_snapshot_unchanged() {
    let mut index = WorkspaceIndex::new();
    index
        .add_project(pid(1), "Only", tid(11), "Only tab")
        .unwrap();
    index
        .add_project(pid(2), "Other", tid(21), "Other tab")
        .unwrap();
    let before = index.clone();
    assert_eq!(
        index.remove_tab(pid(1), tid(11)),
        Err(Error::ActiveProjectCannotBeEmpty)
    );
    assert_eq!(index, before);
    assert_eq!(
        index.reorder_tab(pid(1), tid(11), 3),
        Err(Error::ReorderIndexOutOfRange)
    );
    assert_eq!(index, before);
    assert_eq!(
        index.reorder_tab(pid(1), tid(99), 0),
        Err(Error::UnknownTab)
    );
    assert_eq!(index, before);
    assert_eq!(index.remove_project(pid(1)), Err(Error::ProjectHasTabs));
    assert_eq!(index, before);
}

#[test]
fn adding_a_tab_does_not_change_selection_even_when_an_inactive_project_is_empty() {
    let mut index = populated();
    index.add_tab(pid(1), tid(14), "Four").unwrap();
    assert_eq!(index.active_tab_id(), Some(tid(11)));
    index.remove_tab(pid(2), tid(21)).unwrap();
    index.remove_tab(pid(2), tid(22)).unwrap();
    index.add_tab(pid(2), tid(23), "New first").unwrap();
    assert_eq!(index.project(pid(2)).unwrap().selected_tab_id(), None);
    index.select_project(pid(2)).unwrap();
    assert_eq!(
        index.project(pid(2)).unwrap().selected_tab_id(),
        Some(tid(23))
    );
    assert_eq!(index.active_tab_id(), Some(tid(23)));
}
