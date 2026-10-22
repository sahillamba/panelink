//! Launch/protocol building blocks and an experimental terminal core.
//!
//! Includes a disposable terminal probe and an experimental one-child local
//! server. This does not provide a full interactive multiplexer or agent recovery.

pub mod close_protocol;
pub mod control_protocol;
pub mod event_journal;
pub mod event_protocol;
pub mod handshake_protocol;
pub mod history_rows;
pub mod history_store;
pub mod input_owner;
pub mod launch;
pub mod managed_launch_protocol;
pub mod pane_layout;
pub mod protocol;
pub mod protocol_wire;
pub mod provider_binding;
pub mod provider_binding_codec;
pub mod response_protocol;
pub mod saved_layout;
pub mod scoped_watch_protocol;
pub mod server;
pub mod session_protocol;
pub mod session_registry;
pub mod split_menu;
pub mod split_protocol;
pub mod watch_protocol;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod spawn_gate;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod server_io;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod server_runtime;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod multi_runtime;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod multi_startup;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod server_pty;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod server_startup;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod client_context;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod transport;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod peer_identity;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod server_storage;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod history_diagnostics;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod history_writer;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) mod history_capture;

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod history_tail_tests;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod managed_launch;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod managed_launch_prepare;

#[cfg(unix)]
pub mod terminal;

#[cfg(unix)]
pub mod screen_checkpoint;

#[cfg(unix)]
pub mod ui_protocol;
