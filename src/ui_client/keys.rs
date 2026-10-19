//! Pure mapping from decoded host events to child keys, local commands and
//! menu inputs. Nothing here performs IO.

use crossterm_input::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use panelink::split_menu::MenuInput;
use panelink::terminal::Key;

/// What one key press means while the menu is closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KeyAction {
    /// Forward this typed key to the focused pane.
    Forward(Key),
    /// Ctrl-X: detach the client (children keep running).
    Quit,
    /// Ctrl-B: the next key is a local prefix command.
    Prefix,
    /// Not representable as a typed key; dropped locally.
    Ignore,
}

/// Map one key event, mirroring `p2_probe::dispatch_key`.
///
/// Ctrl-C becomes the typed interrupt key, Ctrl-X quits locally, Ctrl-B is
/// the prototype prefix. Other control chords and function keys have no
/// typed-key encoding in the schema-8 wire format and are dropped.
pub(crate) fn map_key(key: KeyEvent) -> KeyAction {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return KeyAction::Ignore;
    }
    if key.modifiers == KeyModifiers::CONTROL {
        return match key.code {
            KeyCode::Char('c' | 'C') => KeyAction::Forward(Key::Interrupt),
            KeyCode::Char('x' | 'X') => KeyAction::Quit,
            KeyCode::Char('b' | 'B') => KeyAction::Prefix,
            _ => KeyAction::Ignore,
        };
    }
    let none = key.modifiers.is_empty();
    let typed = match key.code {
        KeyCode::Char(character)
            if (none || key.modifiers == KeyModifiers::SHIFT) && !character.is_control() =>
        {
            Key::Char(character)
        }
        KeyCode::Enter if none => Key::Enter,
        KeyCode::Tab if none => Key::Tab,
        KeyCode::Backspace if none => Key::Backspace,
        KeyCode::Esc if none => Key::Escape,
        KeyCode::Up if none => Key::Up,
        KeyCode::Down if none => Key::Down,
        KeyCode::Left if none => Key::Left,
        KeyCode::Right if none => Key::Right,
        KeyCode::Home if none => Key::Home,
        KeyCode::End if none => Key::End,
        _ => return KeyAction::Ignore,
    };
    KeyAction::Forward(typed)
}

/// Translate any host event into the menu's own input vocabulary.
///
/// Used only while the menu is open; every result is consumed by the menu.
pub(crate) fn menu_input(event: &Event) -> MenuInput {
    match event {
        Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
            if !key.modifiers.is_empty() && key.modifiers != KeyModifiers::SHIFT {
                return MenuInput::Other;
            }
            match key.code {
                KeyCode::Up => MenuInput::Up,
                KeyCode::Down => MenuInput::Down,
                KeyCode::Home => MenuInput::Home,
                KeyCode::End => MenuInput::End,
                KeyCode::Enter => MenuInput::Activate,
                KeyCode::Esc => MenuInput::Cancel,
                KeyCode::Char(character) if !character.is_control() => MenuInput::Letter(character),
                _ => MenuInput::Other,
            }
        }
        Event::Mouse(mouse) => match mouse_button(mouse) {
            Some((true, right)) => MenuInput::MousePress {
                x: mouse.column,
                y: mouse.row,
                right,
            },
            Some((false, right)) => MenuInput::MouseRelease {
                x: mouse.column,
                y: mouse.row,
                right,
            },
            None => MenuInput::Other,
        },
        _ => MenuInput::Other,
    }
}

/// `(pressed, right_button)` for left/right press or release events.
pub(crate) fn mouse_button(mouse: &MouseEvent) -> Option<(bool, bool)> {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => Some((true, false)),
        MouseEventKind::Down(MouseButton::Right) => Some((true, true)),
        MouseEventKind::Up(MouseButton::Left) => Some((false, false)),
        MouseEventKind::Up(MouseButton::Right) => Some((false, true)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm_input::Decoder;

    fn decode(bytes: &[u8]) -> Event {
        let mut decoder = Decoder::new();
        let mut last = None;
        for (index, byte) in bytes.iter().enumerate() {
            last = decoder.advance(*byte, index + 1 < bytes.len()).unwrap();
        }
        last.or_else(|| decoder.finish_escape().unwrap())
            .expect("complete event")
    }

    fn key_of(bytes: &[u8]) -> KeyAction {
        match decode(bytes) {
            Event::Key(key) => map_key(key),
            _ => panic!("not a key"),
        }
    }

    #[test]
    fn host_bytes_map_to_typed_keys_and_local_commands() {
        assert_eq!(key_of(b"a"), KeyAction::Forward(Key::Char('a')));
        assert_eq!(key_of(b"A"), KeyAction::Forward(Key::Char('A')));
        assert_eq!(key_of("é".as_bytes()), KeyAction::Forward(Key::Char('é')));
        assert_eq!(key_of(b"\r"), KeyAction::Forward(Key::Enter));
        assert_eq!(key_of(b"\t"), KeyAction::Forward(Key::Tab));
        assert_eq!(key_of(b"\x7f"), KeyAction::Forward(Key::Backspace));
        assert_eq!(key_of(b"\x1b"), KeyAction::Forward(Key::Escape));
        assert_eq!(key_of(b"\x1b[A"), KeyAction::Forward(Key::Up));
        assert_eq!(key_of(b"\x1b[B"), KeyAction::Forward(Key::Down));
        assert_eq!(key_of(b"\x1b[C"), KeyAction::Forward(Key::Right));
        assert_eq!(key_of(b"\x1b[D"), KeyAction::Forward(Key::Left));
        assert_eq!(key_of(b"\x1b[H"), KeyAction::Forward(Key::Home));
        assert_eq!(key_of(b"\x1b[F"), KeyAction::Forward(Key::End));
        assert_eq!(key_of(b"\x03"), KeyAction::Forward(Key::Interrupt));
        assert_eq!(key_of(b"\x18"), KeyAction::Quit);
        assert_eq!(key_of(b"\x02"), KeyAction::Prefix);
        // Other control chords and function keys are dropped, never sent raw.
        assert_eq!(key_of(b"\x01"), KeyAction::Ignore);
        assert_eq!(key_of(b"\x11"), KeyAction::Ignore);
        assert_eq!(key_of(b"\x1bOP"), KeyAction::Ignore);
    }

    #[test]
    fn sgr_mouse_and_menu_keys_map_to_menu_inputs() {
        assert_eq!(
            menu_input(&decode(b"\x1b[<2;10;5M")),
            MenuInput::MousePress {
                x: 9,
                y: 4,
                right: true
            }
        );
        assert_eq!(
            menu_input(&decode(b"\x1b[<2;10;5m")),
            MenuInput::MouseRelease {
                x: 9,
                y: 4,
                right: true
            }
        );
        assert_eq!(
            menu_input(&decode(b"\x1b[<0;1;1M")),
            MenuInput::MousePress {
                x: 0,
                y: 0,
                right: false
            }
        );
        assert_eq!(menu_input(&decode(b"\x1b[<64;1;1M")), MenuInput::Other);
        assert_eq!(menu_input(&decode(b"\x1b")), MenuInput::Cancel);
        assert_eq!(menu_input(&decode(b"\r")), MenuInput::Activate);
        assert_eq!(menu_input(&decode(b"\x1b[A")), MenuInput::Up);
        assert_eq!(menu_input(&decode(b"r")), MenuInput::Letter('r'));
        assert_eq!(menu_input(&decode(b"\x18")), MenuInput::Other);
        assert_eq!(
            menu_input(&decode(b"\x1b[200~x\x1b[201~")),
            MenuInput::Other
        );
    }
}
