//! The single keyboard registry (DESIGN.md keyboard map, Q-161).
//!
//! Shortcuts are plain data here so menus, tooltips, the palette, the
//! shortcut overlay and `docs/KEYBOARD.md` all read the same table. egui
//! types never appear in this crate; `cl-ui` converts a key press into a
//! [`Shortcut`] and asks [`Keymap::resolve`].
//!
//! Scope rules: the innermost scope wins (`Form` over `Window` over
//! `Global`), so Ctrl+D can mean "Make Deposit" globally and "delete this
//! transaction" inside a form. Two commands may not share a shortcut in the
//! same scope ([`Keymap::new`] refuses).

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::commands::{Command, CommandId};

/// Where a command is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    /// Anywhere in the app.
    Global,
    /// While a window (tab) has focus.
    Window,
    /// While a form or grid has focus.
    Form,
}

impl Scope {
    /// Heading used in `KEYBOARD.md` and the overlay.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Scope::Global => "Anywhere",
            Scope::Window => "In a window",
            Scope::Form => "In a form or grid",
        }
    }

    /// Scopes active when the focus is in `self`, innermost first.
    #[must_use]
    pub fn active_from(self) -> &'static [Scope] {
        match self {
            Scope::Form => &[Scope::Form, Scope::Window, Scope::Global],
            Scope::Window => &[Scope::Window, Scope::Global],
            Scope::Global => &[Scope::Global],
        }
    }
}

/// A key the keymap knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// `A`-`Z` (upper case).
    Letter(char),
    /// `0`-`9`.
    Digit(u8),
    /// `F1`-`F12`.
    F(u8),
    /// Tab.
    Tab,
    /// Enter.
    Enter,
    /// Escape.
    Escape,
    /// Insert.
    Insert,
    /// Delete.
    Delete,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// `/`.
    Slash,
    /// `,`.
    Comma,
    /// The `=` / `+` key (zoom in).
    Equals,
    /// The `-` key (zoom out).
    Minus,
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Letter(c) => write!(f, "{}", c.to_ascii_uppercase()),
            Key::Digit(d) => write!(f, "{d}"),
            Key::F(n) => write!(f, "F{n}"),
            Key::Tab => f.write_str("Tab"),
            Key::Enter => f.write_str("Enter"),
            Key::Escape => f.write_str("Esc"),
            Key::Insert => f.write_str("Insert"),
            Key::Delete => f.write_str("Delete"),
            Key::PageUp => f.write_str("Page Up"),
            Key::PageDown => f.write_str("Page Down"),
            Key::Slash => f.write_str("/"),
            Key::Comma => f.write_str(","),
            Key::Equals => f.write_str("+"),
            Key::Minus => f.write_str("-"),
        }
    }
}

/// Modifier keys. `command` is Ctrl on Windows and Linux and Cmd on macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods {
    /// Ctrl (Windows, Linux) or Cmd (macOS).
    pub command: bool,
    /// Shift.
    pub shift: bool,
    /// Alt (Option on macOS).
    pub alt: bool,
}

impl Mods {
    /// No modifiers.
    pub const NONE: Mods = Mods {
        command: false,
        shift: false,
        alt: false,
    };
    /// Ctrl/Cmd only.
    pub const CMD: Mods = Mods {
        command: true,
        shift: false,
        alt: false,
    };
    /// Ctrl/Cmd and Shift.
    pub const CMD_SHIFT: Mods = Mods {
        command: true,
        shift: true,
        alt: false,
    };
    /// Shift only.
    pub const SHIFT: Mods = Mods {
        command: false,
        shift: true,
        alt: false,
    };
    /// Alt only.
    pub const ALT: Mods = Mods {
        command: false,
        shift: false,
        alt: true,
    };
}

/// A key with modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Shortcut {
    /// Modifiers held.
    pub mods: Mods,
    /// The key.
    pub key: Key,
}

impl Shortcut {
    /// Build a shortcut.
    #[must_use]
    pub const fn new(mods: Mods, key: Key) -> Shortcut {
        Shortcut { mods, key }
    }

    /// Text for menus and tooltips. `mac` shows Cmd/Option names.
    #[must_use]
    pub fn display(&self, mac: bool) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.mods.command {
            parts.push(if mac { "Cmd" } else { "Ctrl" }.to_string());
        }
        if self.mods.alt {
            parts.push(if mac { "Option" } else { "Alt" }.to_string());
        }
        if self.mods.shift {
            parts.push("Shift".to_string());
        }
        parts.push(self.key.to_string());
        parts.join("+")
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display(false))
    }
}

/// Why a keymap could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeymapError {
    /// Two commands claim the same shortcut in the same scope.
    #[error("{shortcut} is used by both {first} and {second} in the same scope")]
    Conflict {
        /// The shared shortcut, as text.
        shortcut: String,
        /// First command id.
        first: &'static str,
        /// Second command id.
        second: &'static str,
    },
    /// Two commands share an id.
    #[error("command id {0} is registered twice")]
    DuplicateId(&'static str),
}

/// Shortcut lookup built from the command table.
#[derive(Debug, Clone)]
pub struct Keymap {
    by_shortcut: HashMap<(Scope, Shortcut), CommandId>,
}

impl Keymap {
    /// Build the lookup.
    ///
    /// # Errors
    /// [`KeymapError`] on a duplicate id or a shortcut shared inside a scope.
    pub fn new(commands: &[Command]) -> Result<Keymap, KeymapError> {
        let mut ids: HashSet<&'static str> = HashSet::new();
        let mut by_shortcut: HashMap<(Scope, Shortcut), CommandId> = HashMap::new();
        for c in commands {
            if !ids.insert(c.id.as_str()) {
                return Err(KeymapError::DuplicateId(c.id.as_str()));
            }
            if let Some(s) = c.shortcut
                && let Some(prev) = by_shortcut.insert((c.scope, s), c.id)
            {
                return Err(KeymapError::Conflict {
                    shortcut: s.to_string(),
                    first: prev.as_str(),
                    second: c.id.as_str(),
                });
            }
        }
        Ok(Keymap { by_shortcut })
    }

    /// The command for `shortcut` when focus is in `focus`: the innermost
    /// scope that binds it wins.
    #[must_use]
    pub fn resolve(&self, focus: Scope, shortcut: Shortcut) -> Option<CommandId> {
        focus
            .active_from()
            .iter()
            .find_map(|scope| self.by_shortcut.get(&(*scope, shortcut)).copied())
    }

    /// Number of bound shortcuts.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_shortcut.len()
    }

    /// True when nothing is bound.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_shortcut.is_empty()
    }
}

/// Keys that belong to text fields and grids and are not commands; listed in
/// the overlay and `KEYBOARD.md` so the document is complete.
pub const FIXED_KEYS: &[(&str, &str)] = &[
    ("Tab / Shift+Tab", "Next / previous field or cell"),
    (
        "Enter",
        "Record the row (registers and grids); Save & New in form header fields",
    ),
    ("Arrow keys", "Move between rows and cells"),
    (
        "Page Up / Page Down",
        "Previous / next record in forms; page in grids",
    ),
    (
        "Esc",
        "Revert the field, then the form, then close (asks first if there are unsaved changes)",
    ),
];

/// Date field keys (DESIGN.md "Date fields").
pub const DATE_KEYS: &[(&str, &str)] = &[
    ("+ / -", "Next / previous day"),
    ("T", "Today"),
    ("M / H", "First / last day of the month"),
    ("Y / R", "First / last day of the year"),
    ("W / K", "First / last day of the week"),
];

/// Bank Feed keys (DESIGN.md).
pub const BANK_FEED_KEYS: &[(&str, &str)] = &[
    ("J / K", "Move down / up"),
    ("Space", "Select the row"),
    ("A", "Accept"),
    ("S", "Split"),
    ("R", "Make a rule"),
    ("Ctrl+Enter", "Accept selected"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{ALL, COMMANDS, Menu};

    #[test]
    fn default_table_has_no_conflicts() {
        let km = Keymap::new(COMMANDS).expect("the shipped table must be conflict free");
        assert!(km.len() > 20);
    }

    #[test]
    fn same_shortcut_in_one_scope_is_refused() {
        let mut v = COMMANDS.to_vec();
        let mut dup = *v.iter().find(|c| c.id.as_str() == "nav.home").unwrap();
        dup.id = CommandId::new("test.dup");
        v.push(dup);
        assert!(matches!(Keymap::new(&v), Err(KeymapError::Conflict { .. })));
    }

    #[test]
    fn duplicate_ids_are_refused() {
        let mut v = COMMANDS.to_vec();
        let mut dup = v[0];
        dup.shortcut = None;
        v.push(dup);
        assert!(matches!(Keymap::new(&v), Err(KeymapError::DuplicateId(_))));
    }

    #[test]
    fn innermost_scope_wins() {
        let km = Keymap::new(COMMANDS).unwrap();
        let ctrl_d = Shortcut::new(Mods::CMD, Key::Letter('D'));
        assert_eq!(
            km.resolve(Scope::Global, ctrl_d).map(CommandId::as_str),
            Some("banking.deposit")
        );
        assert_eq!(
            km.resolve(Scope::Form, ctrl_d).map(CommandId::as_str),
            Some("form.delete_item")
        );
        // Window focus still sees the global binding.
        assert_eq!(
            km.resolve(Scope::Window, ctrl_d).map(CommandId::as_str),
            Some("banking.deposit")
        );
    }

    #[test]
    fn form_keys_do_not_fire_outside_a_form() {
        let km = Keymap::new(COMMANDS).unwrap();
        let alt_n = Shortcut::new(Mods::ALT, Key::Letter('N'));
        assert_eq!(km.resolve(Scope::Global, alt_n), None);
        assert_eq!(
            km.resolve(Scope::Form, alt_n).map(CommandId::as_str),
            Some("form.save_new")
        );
    }

    #[test]
    fn shortcut_text() {
        let s = Shortcut::new(Mods::CMD_SHIFT, Key::Letter('r'));
        assert_eq!(s.to_string(), "Ctrl+Shift+R");
        assert_eq!(s.display(true), "Cmd+Shift+R");
        assert_eq!(
            Shortcut::new(Mods::ALT, Key::Letter('N')).to_string(),
            "Alt+N"
        );
        assert_eq!(Shortcut::new(Mods::NONE, Key::F(1)).to_string(), "F1");
    }

    #[test]
    fn every_menu_and_rail_entry_has_a_command_a_palette_entry_and_a_label() {
        for c in COMMANDS {
            assert_ne!(c.label, "");
            assert!(c.palette, "{} must be reachable from the palette", c.id);
        }
        // A menu entry is a command with a menu; every menu has entries.
        for m in Menu::ALL {
            assert!(
                COMMANDS.iter().any(|c| c.menu == Some(*m)),
                "menu {m:?} is empty"
            );
        }
        for id in ALL.rail_workspace.iter().chain(ALL.rail_tools) {
            assert!(
                COMMANDS.iter().any(|c| c.id.as_str() == *id),
                "rail entry {id} has no command"
            );
        }
    }
}
