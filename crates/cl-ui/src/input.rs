//! Turning egui key presses into `cl-model` shortcuts, and back into the
//! text menus and tooltips show.

use cl_model::{Key, Mods, Shortcut};
use eframe::egui::{self, InputState};

/// Convert an egui key to the keymap's key, if the keymap knows it.
#[must_use]
pub fn key_from_egui(key: egui::Key) -> Option<Key> {
    use egui::Key as E;
    Some(match key {
        E::Tab => Key::Tab,
        E::Enter => Key::Enter,
        E::Escape => Key::Escape,
        E::Insert => Key::Insert,
        E::Delete => Key::Delete,
        E::PageUp => Key::PageUp,
        E::PageDown => Key::PageDown,
        E::Slash => Key::Slash,
        E::Comma => Key::Comma,
        E::Equals | E::Plus => Key::Equals,
        E::Minus => Key::Minus,
        E::Num0 => Key::Digit(0),
        E::Num1 => Key::Digit(1),
        E::Num2 => Key::Digit(2),
        E::Num3 => Key::Digit(3),
        E::Num4 => Key::Digit(4),
        E::Num5 => Key::Digit(5),
        E::Num6 => Key::Digit(6),
        E::Num7 => Key::Digit(7),
        E::Num8 => Key::Digit(8),
        E::Num9 => Key::Digit(9),
        E::F1 => Key::F(1),
        E::F2 => Key::F(2),
        E::F3 => Key::F(3),
        E::F4 => Key::F(4),
        E::F5 => Key::F(5),
        E::F6 => Key::F(6),
        E::F7 => Key::F(7),
        E::F8 => Key::F(8),
        E::F9 => Key::F(9),
        E::F10 => Key::F(10),
        E::F11 => Key::F(11),
        E::F12 => Key::F(12),
        other => {
            let name = other.name();
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphabetic() => Key::Letter(c.to_ascii_uppercase()),
                _ => return None,
            }
        }
    })
}

/// Convert egui modifiers (`command` is Ctrl, or Cmd on macOS).
#[must_use]
pub fn mods_from_egui(m: egui::Modifiers, key: Key) -> Mods {
    Mods {
        command: m.command,
        // The `+` key needs Shift on most keyboards; zoom in is Ctrl and `+`.
        shift: m.shift && key != Key::Equals,
        alt: m.alt,
    }
}

/// Every shortcut pressed this frame, in order, without consuming anything.
#[must_use]
pub fn pressed_shortcuts(input: &InputState) -> Vec<Shortcut> {
    input
        .events
        .iter()
        .filter_map(|e| match e {
            egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } => {
                let k = key_from_egui(*key)?;
                Some(Shortcut::new(mods_from_egui(*modifiers, k), k))
            }
            _ => None,
        })
        .collect()
}

/// Keys that a focused text field needs for itself. While a text field has
/// focus, global shortcuts that would take these away are ignored: Ctrl+A is
/// "select all" there, not "Chart of Accounts" (Q-P06d).
#[must_use]
pub fn reserved_by_text_fields(s: Shortcut) -> bool {
    (s.mods == Mods::CMD && matches!(s.key, Key::Letter('A' | 'C' | 'V' | 'X')))
        // Delete deletes the next character, not the list entry (P15).
        || (s.mods == Mods::NONE && s.key == Key::Delete)
}

/// True when the platform uses Cmd and Option names.
#[must_use]
pub fn is_mac() -> bool {
    cfg!(target_os = "macos")
}

/// Shortcut text for the current platform.
#[must_use]
pub fn shortcut_text(s: Shortcut) -> String {
    s.display(is_mac())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_function_keys_map() {
        assert_eq!(key_from_egui(egui::Key::K), Some(Key::Letter('K')));
        assert_eq!(key_from_egui(egui::Key::Num0), Some(Key::Digit(0)));
        assert_eq!(key_from_egui(egui::Key::F1), Some(Key::F(1)));
        assert_eq!(key_from_egui(egui::Key::PageDown), Some(Key::PageDown));
        assert_eq!(key_from_egui(egui::Key::Plus), Some(Key::Equals));
        assert_eq!(key_from_egui(egui::Key::Backspace), None);
    }

    #[test]
    fn every_letter_maps() {
        for (k, c) in [
            (egui::Key::A, 'A'),
            (egui::Key::M, 'M'),
            (egui::Key::Z, 'Z'),
        ] {
            assert_eq!(key_from_egui(k), Some(Key::Letter(c)));
        }
    }

    #[test]
    fn shift_is_ignored_for_the_plus_key() {
        let m = egui::Modifiers {
            command: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(mods_from_egui(m, Key::Equals), Mods::CMD);
        assert_eq!(mods_from_egui(m, Key::Letter('R')), Mods::CMD_SHIFT);
    }

    #[test]
    fn text_fields_keep_their_edit_keys() {
        assert!(reserved_by_text_fields(Shortcut::new(
            Mods::CMD,
            Key::Letter('A')
        )));
        assert!(!reserved_by_text_fields(Shortcut::new(
            Mods::CMD,
            Key::Letter('K')
        )));
    }
}
