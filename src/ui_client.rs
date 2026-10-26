//! Experimental interactive attach client for `panelink serve`.
//!
//! Binary-private, like `p2_probe`: it owns the host TTY (raw mode, alternate
//! screen, bracketed paste and SGR mouse reporting) and talks to the server
//! with one authenticated connection per pane. PTYs and children stay
//! server-owned; quitting the client (Ctrl-X) only detaches.

mod clipboard;
mod conn;
mod host;
mod keys;
mod net;
mod pane_operations;
pub(crate) mod recovery;
mod routes;
mod selection;
mod state;
mod worker;

use panelink::workspace_index::WorkspaceIndex;
use panelink::workspace_tabs::WorkspaceRouteId;
use std::path::PathBuf;

/// Run the attach client; returns the process exit code.
pub(crate) fn run(directory: PathBuf, context_dir: PathBuf) -> i32 {
    host::run(directory, context_dir)
}

/// Run the workspace-aware attach client with an immutable route catalog.
pub(crate) fn run_workspace(
    directory: PathBuf,
    context_dir: PathBuf,
    app_root: PathBuf,
    index: WorkspaceIndex,
    current: WorkspaceRouteId,
) -> i32 {
    host::run_workspace(directory, context_dir, app_root, index, current)
}
