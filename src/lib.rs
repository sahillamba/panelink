//! Launch/protocol building blocks and an experimental terminal core.
//!
//! Includes a disposable terminal probe and an experimental one-child local
//! server. This does not provide a full interactive multiplexer or agent recovery.

pub mod control_protocol;
pub mod event_journal;
pub mod event_protocol;
pub mod handshake_protocol;
pub mod input_owner;
pub mod launch;
pub mod protocol;
pub mod protocol_wire;
pub mod response_protocol;
pub mod server;
pub mod session_protocol;
pub mod watch_protocol;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod server_io;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod server_runtime;

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

#[cfg(unix)]
pub mod terminal;
