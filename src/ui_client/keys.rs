//! Pure mapping from decoded host events to child keys, local commands and
//! menu inputs. Nothing here performs IO.

use crossterm_input::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use panelink::split_menu::{ConfirmInput, MenuInput};
use panelink::terminal::Key;
use panelink::workspace_tabs::TabNavigation;

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

/// Keys consumed by the bounded local keyboard copy mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CopyModeKey {
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    ToggleAnchor,
    Copy,
    Cancel,
    Detach,
    Ignore,
}

/// Bounded prototype pane actions accepted after Ctrl-B.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PaneOperationKey {
    Zoom,
    Swap,
    Divider,
}

pub(crate) fn pane_operation_key(
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Option<PaneOperationKey> {
    let plain = modifiers.is_empty() || modifiers == KeyModifiers::SHIFT;
    if !plain {
        return None;
    }
    match code {
        KeyCode::Char('z' | 'Z') => Some(PaneOperationKey::Zoom),
        KeyCode::Char('m' | 'M') => Some(PaneOperationKey::Swap),
        KeyCode::Char('e' | 'E') => Some(PaneOperationKey::Divider),
        _ => None,
    }
}

/// Interpret one event while copy mode owns input. Unsupported combinations,
/// releases, paste, and mouse are intentionally handled as local Ignore.
pub(crate) fn copy_mode_key(key: KeyEvent) -> CopyModeKey {
    if key.kind == KeyEventKind::Release {
        return CopyModeKey::Ignore;
    }
    if key.modifiers == KeyModifiers::CONTROL && matches!(key.code, KeyCode::Char('x' | 'X')) {
        return CopyModeKey::Detach;
    }
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return CopyModeKey::Ignore;
    }
    let plain = key.modifiers.is_empty();
    let plain_char = plain || key.modifiers == KeyModifiers::SHIFT;
    match key.code {
        KeyCode::Left if plain => CopyModeKey::MoveLeft,
        KeyCode::Right if plain => CopyModeKey::MoveRight,
        KeyCode::Up if plain => CopyModeKey::MoveUp,
        KeyCode::Down if plain => CopyModeKey::MoveDown,
        KeyCode::Char('h' | 'H') if plain_char => CopyModeKey::MoveLeft,
        KeyCode::Char('l' | 'L') if plain_char => CopyModeKey::MoveRight,
        KeyCode::Char('k' | 'K') if plain_char => CopyModeKey::MoveUp,
        KeyCode::Char('j' | 'J') if plain_char => CopyModeKey::MoveDown,
        KeyCode::Char(' ') if plain && key.kind == KeyEventKind::Press => CopyModeKey::ToggleAnchor,
        KeyCode::Enter if plain && key.kind == KeyEventKind::Press => CopyModeKey::Copy,
        KeyCode::Char('y' | 'Y') if plain_char && key.kind == KeyEventKind::Press => {
            CopyModeKey::Copy
        }
        KeyCode::Esc if plain && key.kind == KeyEventKind::Press => CopyModeKey::Cancel,
        _ => CopyModeKey::Ignore,
    }
}

/// Workspace navigation shortcuts accepted after Ctrl-B.
pub(crate) fn workspace_navigation(
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Option<TabNavigation> {
    let plain = modifiers.is_empty() || modifiers == KeyModifiers::SHIFT;
    if !plain {
        return None;
    }
    match code {
        KeyCode::Char('n' | 'N') => Some(TabNavigation::NextTab),
        KeyCode::Char('p' | 'P') => Some(TabNavigation::PreviousTab),
        KeyCode::Char(']') => Some(TabNavigation::NextProject),
        KeyCode::Char('[') => Some(TabNavigation::PreviousProject),
        _ => None,
    }
}

/// Map one key event, mirroring `p2_probe::dispatch_key`.
///
/// Ctrl-C becomes the typed interrupt key, Ctrl-X quits locally, Ctrl-B is
/// the prototype prefix. Supported keys have typed schema-8 representations.
pub(crate) fn map_key(key: KeyEvent) -> KeyAction {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return KeyAction::Ignore;
    }
    if key.modifiers == KeyModifiers::CONTROL {
        return match key.code {
            KeyCode::Char('c' | 'C') => KeyAction::Forward(Key::Interrupt),
            KeyCode::Char('x' | 'X') => KeyAction::Quit,
            KeyCode::Char('b' | 'B') => KeyAction::Prefix,
            KeyCode::Char(character @ ('a'..='z' | 'A'..='Z')) => {
                let lower = character.to_ascii_lowercase();
                KeyAction::Forward(Key::Control(lower as u8 - b'a' + 1))
            }
            _ => KeyAction::Ignore,
        };
    }
    let none = key.modifiers.is_empty();
    let alt = key.modifiers == KeyModifiers::ALT;
    let alt_shift = key.modifiers == (KeyModifiers::ALT | KeyModifiers::SHIFT);
    let typed = match key.code {
        KeyCode::Char(character)
            if (none || key.modifiers == KeyModifiers::SHIFT) && !character.is_control() =>
        {
            Key::Char(character)
        }
        KeyCode::Char(character) if (alt || alt_shift) && !character.is_control() => {
            Key::AltChar(character)
        }
        KeyCode::F(number @ 1..=12) if none => Key::Function(number),
        KeyCode::Delete if none => Key::Delete,
        KeyCode::Insert if none => Key::Insert,
        KeyCode::PageUp if none => Key::PageUp,
        KeyCode::PageDown if none => Key::PageDown,
        KeyCode::BackTab if key.modifiers == KeyModifiers::SHIFT => Key::BackTab,
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

/// Translate any host event into the close prompt's input vocabulary.
///
/// Only an unmodified (or shifted) `y` confirms. Every other key press,
/// paste and mouse press cancels; key releases, mouse releases/motion/wheel,
/// focus and resize events are ignored so a stray release cannot cancel the
/// prompt that a click just opened. Every result is consumed by the prompt.
pub(crate) fn confirm_input(event: &Event) -> ConfirmInput {
    match event {
        Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
            let plain = key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT;
            match key.code {
                KeyCode::Char('y' | 'Y') if plain && key.kind == KeyEventKind::Press => {
                    ConfirmInput::Yes
                }
                _ => ConfirmInput::Cancel,
            }
        }
        Event::Key(_) => ConfirmInput::Ignore,
        Event::Paste(_) => ConfirmInput::Cancel,
        Event::Mouse(mouse) => match mouse_button(mouse) {
            Some((true, _)) => ConfirmInput::Cancel,
            _ => ConfirmInput::Ignore,
        },
        Event::FocusGained | Event::FocusLost | Event::Resize(_, _) => ConfirmInput::Ignore,
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

/// `true` for an upward wheel step, `false` for downward movement.
pub(crate) fn mouse_wheel(mouse: &MouseEvent) -> Option<bool> {
    match mouse.kind {
        MouseEventKind::ScrollUp => Some(true),
        MouseEventKind::ScrollDown => Some(false),
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
        assert_eq!(key_of(b"\x01"), KeyAction::Forward(Key::Control(1)));
        assert_eq!(key_of(b"\x11"), KeyAction::Forward(Key::Control(17)));
        assert_eq!(key_of(b"\x1bOP"), KeyAction::Forward(Key::Function(1)));
        assert_eq!(key_of(b"\x1bOQ"), KeyAction::Forward(Key::Function(2)));
        assert_eq!(key_of(b"\x1bOR"), KeyAction::Forward(Key::Function(3)));
        assert_eq!(key_of(b"\x1bOS"), KeyAction::Forward(Key::Function(4)));
        assert_eq!(key_of(b"\x1b[15~"), KeyAction::Forward(Key::Function(5)));
        assert_eq!(key_of(b"\x1b[24~"), KeyAction::Forward(Key::Function(12)));
        assert_eq!(key_of(b"\x1ba"), KeyAction::Forward(Key::AltChar('a')));
        assert_eq!(
            key_of(b"\x1b\xc3\xa9"),
            KeyAction::Forward(Key::AltChar('é'))
        );
        assert_eq!(key_of(b"\x1b[3~"), KeyAction::Forward(Key::Delete));
        assert_eq!(key_of(b"\x1b[2~"), KeyAction::Forward(Key::Insert));
        assert_eq!(key_of(b"\x1b[5~"), KeyAction::Forward(Key::PageUp));
        assert_eq!(key_of(b"\x1b[6~"), KeyAction::Forward(Key::PageDown));
        assert_eq!(key_of(b"\x1b[Z"), KeyAction::Forward(Key::BackTab));
    }

    #[test]
    fn releases_and_unsupported_modifier_combinations_are_ignored() {
        let release =
            KeyEvent::new_with_kind(KeyCode::F(1), KeyModifiers::NONE, KeyEventKind::Release);
        assert_eq!(map_key(release), KeyAction::Ignore);

        let alt_shift = KeyEvent::new_with_kind(
            KeyCode::Char('É'),
            KeyModifiers::ALT | KeyModifiers::SHIFT,
            KeyEventKind::Press,
        );
        assert_eq!(map_key(alt_shift), KeyAction::Forward(Key::AltChar('É')));

        let control_shift = KeyEvent::new_with_kind(
            KeyCode::Char('A'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            KeyEventKind::Press,
        );
        assert_eq!(map_key(control_shift), KeyAction::Ignore);

        let alt_function =
            KeyEvent::new_with_kind(KeyCode::F(1), KeyModifiers::ALT, KeyEventKind::Press);
        assert_eq!(map_key(alt_function), KeyAction::Ignore);
    }

    #[test]
    fn workspace_commands_require_plain_navigation_keys() {
        assert_eq!(
            workspace_navigation(KeyCode::Char('n'), KeyModifiers::NONE),
            Some(TabNavigation::NextTab)
        );
        assert_eq!(
            workspace_navigation(KeyCode::Char('P'), KeyModifiers::SHIFT),
            Some(TabNavigation::PreviousTab)
        );
        assert_eq!(
            workspace_navigation(KeyCode::Char(']'), KeyModifiers::NONE),
            Some(TabNavigation::NextProject)
        );
        assert_eq!(
            workspace_navigation(KeyCode::Char('['), KeyModifiers::NONE),
            Some(TabNavigation::PreviousProject)
        );
        assert_eq!(
            workspace_navigation(KeyCode::Char('n'), KeyModifiers::CONTROL),
            None
        );
    }

    #[test]
    fn pane_operation_prototypes_require_plain_prefix_keys() {
        assert_eq!(
            pane_operation_key(KeyCode::Char('z'), KeyModifiers::NONE),
            Some(PaneOperationKey::Zoom)
        );
        assert_eq!(
            pane_operation_key(KeyCode::Char('M'), KeyModifiers::SHIFT),
            Some(PaneOperationKey::Swap)
        );
        assert_eq!(
            pane_operation_key(KeyCode::Char('e'), KeyModifiers::NONE),
            Some(PaneOperationKey::Divider)
        );
        assert_eq!(
            pane_operation_key(KeyCode::Char('z'), KeyModifiers::CONTROL),
            None
        );
    }

    #[test]
    fn copy_mode_keys_are_local_and_one_shot_actions_ignore_repeats() {
        let key = |code, modifiers, kind| KeyEvent::new_with_kind(code, modifiers, kind);
        assert_eq!(
            copy_mode_key(key(KeyCode::Left, KeyModifiers::NONE, KeyEventKind::Press)),
            CopyModeKey::MoveLeft
        );
        assert_eq!(
            copy_mode_key(key(
                KeyCode::Char('h'),
                KeyModifiers::NONE,
                KeyEventKind::Repeat
            )),
            CopyModeKey::MoveLeft
        );
        assert_eq!(
            copy_mode_key(key(
                KeyCode::Char(' '),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            CopyModeKey::ToggleAnchor
        );
        assert_eq!(
            copy_mode_key(key(
                KeyCode::Char(' '),
                KeyModifiers::NONE,
                KeyEventKind::Repeat
            )),
            CopyModeKey::Ignore
        );
        assert_eq!(
            copy_mode_key(key(
                KeyCode::Char('y'),
                KeyModifiers::NONE,
                KeyEventKind::Press
            )),
            CopyModeKey::Copy
        );
        assert_eq!(
            copy_mode_key(key(
                KeyCode::Char('y'),
                KeyModifiers::NONE,
                KeyEventKind::Repeat
            )),
            CopyModeKey::Ignore
        );
        assert_eq!(
            copy_mode_key(key(
                KeyCode::Enter,
                KeyModifiers::NONE,
                KeyEventKind::Repeat
            )),
            CopyModeKey::Ignore
        );
        assert_eq!(
            copy_mode_key(key(KeyCode::Left, KeyModifiers::SHIFT, KeyEventKind::Press)),
            CopyModeKey::Ignore
        );
        assert_eq!(
            copy_mode_key(key(KeyCode::Esc, KeyModifiers::NONE, KeyEventKind::Release)),
            CopyModeKey::Ignore
        );
        assert_eq!(
            copy_mode_key(key(
                KeyCode::Char('x'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            )),
            CopyModeKey::Detach
        );
        assert_eq!(
            copy_mode_key(key(
                KeyCode::Char('q'),
                KeyModifiers::ALT,
                KeyEventKind::Press
            )),
            CopyModeKey::Ignore
        );
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
        match decode(b"\x1b[<32;10;5M") {
            Event::Mouse(mouse) => {
                assert_eq!(mouse.kind, MouseEventKind::Drag(MouseButton::Left));
                assert_eq!((mouse.column, mouse.row), (9, 4));
            }
            event => panic!("expected SGR left drag, got {event:?}"),
        }
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

    #[test]
    fn only_y_confirms_and_other_presses_cancel() {
        assert_eq!(confirm_input(&decode(b"y")), ConfirmInput::Yes);
        assert_eq!(confirm_input(&decode(b"Y")), ConfirmInput::Yes);
        for bytes in [
            &b"n"[..],
            b"N",
            b"\x1b",
            b"\r",
            b"x",
            b" ",
            b"\x19", // Ctrl-Y
            b"\x03",
            b"\x02",
            b"\x1b[A",
            b"\x1b[200~y\x1b[201~",
            b"\x1b[<0;5;5M",
            b"\x1b[<2;5;5M",
        ] {
            assert_eq!(
                confirm_input(&decode(bytes)),
                ConfirmInput::Cancel,
                "{bytes:?}"
            );
        }
        // Releases and wheel events do not cancel.
        assert_eq!(
            confirm_input(&decode(b"\x1b[<2;5;5m")),
            ConfirmInput::Ignore
        );
        assert_eq!(
            confirm_input(&decode(b"\x1b[<64;1;1M")),
            ConfirmInput::Ignore
        );
    }
}
