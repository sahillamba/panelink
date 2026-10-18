//! A bounded, parser-only extraction of Crossterm's Unix input decoder.
//!
//! Callers must own raw terminal mode, input IO, error cleanup, and restoration.

#[cfg(not(unix))]
compile_error!("this parser extraction supports Unix targets only");

mod decoder;
mod event;
mod parse;

pub use decoder::{DecodeError, Decoder};
pub use event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, KeyboardEnhancementFlags,
    MediaKeyCode, ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind,
};
