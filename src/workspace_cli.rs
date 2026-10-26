//! Opt-in project and workspace-tab routing commands.
use panelink::workspace_index::{
    codec::MetadataRecord, store::WorkspaceIndexStore, ProjectId, WorkspaceIndex, WorkspaceTabId,
};
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

const ERROR: &str = "workspace command failed";
const REGISTRY: &str = "workspace-index.v1";

pub(crate) fn run(args: Vec<OsString>) -> i32 {
    let words: Option<Vec<String>> = args.iter().map(|s| s.to_str().map(str::to_owned)).collect();
    let Some(words) = words else {
        return fail("invalid workspace command");
    };
    match words.as_slice() {
        [action] if action == "list" => list(),
        [group, action, name, tab] if group == "project" && action == "create" => {
            mutate_create_project(name, tab)
        }
        [group, action, project, name] if group == "tab" && action == "create" => parse_id(project)
            .map_or_else(
                || fail("invalid workspace id"),
                |p| mutate_create_tab(p, name),
            ),
        [action, project, tab] if action == "select" => ids(project, tab).map_or_else(
            || fail("invalid workspace id"),
            |(p, t)| mutate_select(p, t),
        ),
        [action] if action == "open" => {
            active().map_or_else(fail, |(p, t)| crate::default_open::open_workspace_ids(p, t))
        }
        [action, project, tab] if action == "open" => ids(project, tab)
            .map_or_else(|| fail("invalid workspace id"), |(p, t)| open_exact(p, t)),
        [action, project, tab] if action == "stop" => ids(project, tab).map_or_else(
            || fail("invalid workspace id"),
            |(p, t)| {
                if route_exists(p, t).is_err() {
                    fail(ERROR)
                } else {
                    match crate::default_open::stop_workspace_ids(p, t) {
                        Ok(()) => {
                            println!("panelink: workspace stopped");
                            0
                        }
                        Err(e) => fail(e),
                    }
                }
            },
        ),
        [action, project, tab] if action == "remove" => {
            ids(project, tab).map_or_else(|| fail("invalid workspace id"), |(p, t)| remove(p, t))
        }
        _ => fail("invalid workspace command"),
    }
}
fn fail(message: &str) -> i32 {
    eprintln!("panelink: {message}");
    1
}
fn parse_id(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u64>().ok().filter(|x| *x > 0)
}
fn ids(p: &str, t: &str) -> Option<(u64, u64)> {
    Some((parse_id(p)?, parse_id(t)?))
}
fn app_root() -> Result<PathBuf, &'static str> {
    let state = crate::default_open::resolve_root(
        std::env::var_os("PANELINK_HOME"),
        std::env::var_os("HOME"),
    )
    .map_err(|_| ERROR)?;
    Ok(state.root)
}
fn prepare_app() -> Result<PathBuf, &'static str> {
    crate::default_open::prepare_app_root().map_err(|_| ERROR)
}
fn list() -> i32 {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let root = match app_root() {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    match crate::default_open::validate_app_root_readonly() {
        Ok(true) => {}
        Ok(false) => {
            println!("workspace registry is not initialized (legacy Default/Main available)");
            return 0;
        }
        Err(_) => return fail(ERROR),
    }
    let path = root.join(REGISTRY);
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("workspace registry is not initialized (legacy Default/Main available)");
            return 0;
        }
        Err(_) => return fail(ERROR),
    };
    let m = match file.metadata() {
        Ok(m) => m,
        Err(_) => return fail(ERROR),
    };
    if !m.is_file()
        || m.uid() != unsafe { libc::getuid() }
        || m.mode() & 0o7777 != 0o600
        || m.nlink() != 1
    {
        return fail(ERROR);
    }
    let mut bytes = Vec::new();
    use std::io::Read;
    if file.take(24_000).read_to_end(&mut bytes).is_err() {
        return fail(ERROR);
    }
    let record = match MetadataRecord::decode(&bytes) {
        Ok(r) => r,
        Err(_) => return fail(ERROR),
    };
    print_record(&record);
    0
}
fn print_record(r: &MetadataRecord) {
    for p in r.index().projects() {
        println!(
            "Project {} {}{}",
            p.id().get(),
            p.display_name(),
            if Some(p.id()) == r.index().active_project_id() {
                " *"
            } else {
                ""
            }
        );
        for t in p.tabs() {
            println!(
                "  Tab {} {}{}",
                t.id.get(),
                t.display_name,
                if Some(t.id) == p.selected_tab_id()
                    && Some(p.id()) == r.index().active_project_id()
                {
                    " *"
                } else {
                    ""
                }
            );
        }
    }
}
fn open_store() -> Result<WorkspaceIndexStore, &'static str> {
    let root = prepare_app()?;
    let mut s = WorkspaceIndexStore::open(root).map_err(|_| ERROR)?;
    if s.load().map_err(|_| ERROR)?.is_none() {
        s.initialize_legacy().map_err(|_| ERROR)?;
    }
    Ok(s)
}
fn active() -> Result<(u64, u64), &'static str> {
    let s = open_store()?;
    let r = s.load().map_err(|_| ERROR)?.ok_or(ERROR)?;
    Ok((
        r.index().active_project_id().ok_or(ERROR)?.get(),
        r.index().active_tab_id().ok_or(ERROR)?.get(),
    ))
}
fn open_exact(p: u64, t: u64) -> i32 {
    let result = route_exists(p, t);
    match result {
        Ok(()) => crate::default_open::open_workspace_ids(p, t),
        Err(e) => fail(e),
    }
}
pub(crate) fn route_exists(p: u64, t: u64) -> Result<(), &'static str> {
    let s = open_store()?;
    let r = s.load().map_err(|_| ERROR)?.ok_or(ERROR)?;
    let project = ProjectId::new(p).map_err(|_| ERROR)?;
    let tab = WorkspaceTabId::new(t).map_err(|_| ERROR)?;
    let exists = r
        .index()
        .project(project)
        .map_err(|_| ERROR)?
        .tabs()
        .any(|x| x.id == tab);
    exists.then_some(()).ok_or(ERROR)
}
fn save(
    mut s: WorkspaceIndexStore,
    old: MetadataRecord,
    index: WorkspaceIndex,
    next: u64,
) -> Result<(), &'static str> {
    let rev = old.revision().checked_add(1).ok_or(ERROR)?;
    let new = MetadataRecord::new(index, rev, next).map_err(|_| ERROR)?;
    s.save(old.revision(), &new).map_err(|_| ERROR)?;
    Ok(())
}
fn mutate_create_project(name: &str, tabname: &str) -> i32 {
    let result = (|| {
        let s = open_store()?;
        let old = s.load().map_err(|_| ERROR)?.ok_or(ERROR)?;
        let mut i = old.index().clone();
        let p = old.next_id();
        let t = p.checked_add(1).ok_or(ERROR)?;
        i.add_project(
            ProjectId::new(p).map_err(|_| ERROR)?,
            name,
            WorkspaceTabId::new(t).map_err(|_| ERROR)?,
            tabname,
        )
        .map_err(|_| ERROR)?;
        save(s, old, i, t.checked_add(1).ok_or(ERROR)?)
    })();
    result.map_or_else(fail, |_| 0)
}
fn mutate_create_tab(p: u64, name: &str) -> i32 {
    let result = (|| {
        let s = open_store()?;
        let old = s.load().map_err(|_| ERROR)?.ok_or(ERROR)?;
        let mut i = old.index().clone();
        let t = old.next_id();
        i.add_tab(
            ProjectId::new(p).map_err(|_| ERROR)?,
            WorkspaceTabId::new(t).map_err(|_| ERROR)?,
            name,
        )
        .map_err(|_| ERROR)?;
        save(s, old, i, t.checked_add(1).ok_or(ERROR)?)
    })();
    result.map_or_else(fail, |_| 0)
}
fn mutate_select(p: u64, t: u64) -> i32 {
    let result = (|| {
        let s = open_store()?;
        let old = s.load().map_err(|_| ERROR)?.ok_or(ERROR)?;
        let mut i = old.index().clone();
        i.select_tab(
            ProjectId::new(p).map_err(|_| ERROR)?,
            WorkspaceTabId::new(t).map_err(|_| ERROR)?,
        )
        .map_err(|_| ERROR)?;
        let n = old.next_id();
        save(s, old, i, n)
    })();
    result.map_or_else(fail, |_| 0)
}
fn remove(p: u64, t: u64) -> i32 {
    // Legacy commands always address this route, independently of registry
    // selection. Keep its metadata reachable for as long as that entrypoint
    // remains supported.
    if p == 1 && t == 1 {
        return fail("the legacy default workspace cannot be removed");
    }
    if route_exists(p, t).is_err() {
        return fail(ERROR);
    }
    let result = crate::default_open::workspace_remove_guard(p, t, || {
        let s = open_store()?;
        let old = s.load().map_err(|_| ERROR)?.ok_or(ERROR)?;
        let mut i = old.index().clone();
        i.remove_tab(
            ProjectId::new(p).map_err(|_| ERROR)?,
            WorkspaceTabId::new(t).map_err(|_| ERROR)?,
        )
        .map_err(|_| ERROR)?;
        let n = old.next_id();
        save(s, old, i, n)
    });
    result.map_or_else(fail, |_| 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids_are_decimal_nonzero_and_checked() {
        assert_eq!(parse_id("01"), Some(1));
        assert_eq!(parse_id("0"), None);
        assert_eq!(parse_id("+1"), None);
        assert_eq!(parse_id("18446744073709551616"), None);
    }

    #[test]
    fn command_grammar_rejects_partial_routes_and_extra_arguments() {
        assert_ne!(run(vec!["open".into(), "1".into()]), 0);
        assert_ne!(
            run(vec![
                "select".into(),
                "1".into(),
                "2".into(),
                "extra".into()
            ]),
            0
        );
        assert_ne!(run(vec!["remove".into(), "0".into(), "1".into()]), 0);
        assert_ne!(run(vec!["unknown".into()]), 0);
    }
}
