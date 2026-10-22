//! Experimental interactive attach client for `panelink serve`.
//!
//! Binary-private, like `p2_probe`: it owns the host TTY (raw mode, alternate
//! screen, bracketed paste and SGR mouse reporting) and talks to the server
//! with one authenticated connection per pane. PTYs and children stay
//! server-owned; quitting the client (Ctrl-X) only detaches.

mod conn;
mod host;
mod keys;
mod net;
pub(crate) mod recovery;
mod state;
mod worker;

use std::path::PathBuf;

/// Run the attach client; returns the process exit code.
pub(crate) fn run(directory: PathBuf, context_dir: PathBuf) -> i32 {
    host::run(directory, context_dir)
}
