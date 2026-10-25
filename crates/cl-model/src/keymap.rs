//! The single keyboard registry (DESIGN.md keyboard map, Q-161).
//!
//! Shortcuts are plain data here so menus, tooltips, the palette, the
//! shortcut overlay and `docs/KEYBOARD.md` all read the same table. egui
//! types never appear in this crate; `cl-ui` converts a key press into a
//! [`Shortcut`] and asks [`Keymap::resolve`].
//!
//! Scope rules: the innermost scope wins (`Grid` or `Viewer` over `Form`
//! over `Window` over `Global`), so Ctrl+D can mean "Make Deposit" globally, "delete this
//! transaction" inside a form and "copy down" in a grid that opted in. Two
//! commands may not share a shortcut in the same scope ([`Keymap::new`]
//! refuses).
//!
//! The Enter key's meaning is a preference ([`EnterMode`], Q-264): build the
//! keymap with [`Keymap::with_mode`] so "Enter moves between fields" also
//! binds Ctrl+Enter to Save & New.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::commands::{Command, CommandId};
pub use crate::forms::EnterMode;

/// Where a command is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    /// Anywhere in the app.
    Global,
    /// While a window (tab) has focus.
    Window,
    /// While a form or grid has focus.
    Form,
    /// While a grid that opted in to grid commands (copy down, select all)
    /// has focus. Innermost; the form, window and global keys still apply
    /// underneath it.
    Grid,
    /// While a document preview (the PDF and image viewer, P17) has focus,
    /// in a form's source-document card or its own full-size tab.
    /// Innermost like [`Scope::Grid`]: its zoom keys win over the window
    /// zoom and Page Up/Down turn pages instead of records; the form,
    /// window and global keys still apply underneath it.
    Viewer,
}

impl Scope {
    /// Every scope, outermost first (the order of `KEYBOARD.md`).
    pub const ALL: &'static [Scope] = &[
        Scope::Global,
        Scope::Window,
        Scope::Form,
        Scope::Grid,
        Scope::Viewer,
    ];

    /// Heading used in `KEYBOARD.md` and the overlay.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Scope::Global => "Anywhere",
            Scope::Window => "In a window",
            Scope::Form => "In a form or grid",
            Scope::Grid => "In a grid",
            Scope::Viewer => "In a document preview",
        }
    }

    /// Scopes active when the focus is in `self`, innermost first.
    #[must_use]
    pub fn active_from(self) -> &'static [Scope] {
        match self {
            Scope::Grid => &[Scope::Grid, Scope::Form, Scope::Window, Scope::Global],
            Scope::Viewer => &[Scope::Viewer, Scope::Form, Scope::Window, Scope::Global],
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

/// Ctrl+Enter: Save & New when Enter moves between fields (Q-264).
const CTRL_ENTER: Shortcut = Shortcut::new(Mods::CMD, Key::Enter);

/// Bindings that exist only in one [`EnterMode`], on top of the command
/// table: `(scope, shortcut, command id)`.
fn mode_bindings(mode: EnterMode) -> &'static [(Scope, Shortcut, &'static str)] {
    match mode {
        EnterMode::SaveAndNew => &[],
        EnterMode::MoveBetweenFields => &[(Scope::Form, CTRL_ENTER, "form.save_new")],
    }
}

/// Shortcut lookup built from the command table.
#[derive(Debug, Clone)]
pub struct Keymap {
    by_shortcut: HashMap<(Scope, Shortcut), CommandId>,
    /// Every shortcut of a command, the table's own first.
    by_command: HashMap<&'static str, Vec<Shortcut>>,
    mode: EnterMode,
}

impl Keymap {
    /// Build the lookup with the default Enter behavior
    /// ([`EnterMode::SaveAndNew`]).
    ///
    /// # Errors
    /// [`KeymapError`] on a duplicate id or a shortcut shared inside a scope.
    pub fn new(commands: &[Command]) -> Result<Keymap, KeymapError> {
        Keymap::with_mode(commands, EnterMode::SaveAndNew)
    }

    /// Build the lookup for an Enter preference. In
    /// [`EnterMode::MoveBetweenFields`] Ctrl+Enter is also Save & New in
    /// forms (Q-264). A mode binding whose command is not in `commands` is
    /// skipped.
    ///
    /// # Errors
    /// [`KeymapError`] on a duplicate id or a shortcut shared inside a scope.
    pub fn with_mode(commands: &[Command], mode: EnterMode) -> Result<Keymap, KeymapError> {
        let mut ids: HashSet<&'static str> = HashSet::new();
        let mut by_shortcut: HashMap<(Scope, Shortcut), CommandId> = HashMap::new();
        let mut by_command: HashMap<&'static str, Vec<Shortcut>> = HashMap::new();
        let mut bind = |scope: Scope, s: Shortcut, id: CommandId| {
            if let Some(prev) = by_shortcut.insert((scope, s), id) {
                return Err(KeymapError::Conflict {
                    shortcut: s.to_string(),
                    first: prev.as_str(),
                    second: id.as_str(),
                });
            }
            by_command.entry(id.as_str()).or_default().push(s);
            Ok(())
        };
        for c in commands {
            if !ids.insert(c.id.as_str()) {
                return Err(KeymapError::DuplicateId(c.id.as_str()));
            }
            if let Some(s) = c.shortcut {
                bind(c.scope, s, c.id)?;
            }
        }
        for (scope, s, id) in mode_bindings(mode) {
            if let Some(c) = commands.iter().find(|c| c.id.as_str() == *id) {
                bind(*scope, *s, c.id)?;
            }
        }
        Ok(Keymap {
            by_shortcut,
            by_command,
            mode,
        })
    }

    /// The Enter behavior this keymap was built for.
    #[must_use]
    pub fn mode(&self) -> EnterMode {
        self.mode
    }

    /// Every shortcut bound to the command `id`, the table's own first
    /// (tooltips and the overlay show "Alt+N or Ctrl+Enter"). Empty when the
    /// command has none or does not exist.
    #[must_use]
    pub fn shortcuts_of(&self, id: &str) -> Vec<Shortcut> {
        self.by_command.get(id).cloned().unwrap_or_default()
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

/// Keys that belong to text fields and grids and are not commands, for an
/// Enter preference; listed in the overlay and `KEYBOARD.md` so the document
/// is complete.
#[must_use]
pub fn fixed_keys(mode: EnterMode) -> &'static [(&'static str, &'static str)] {
    match mode {
        EnterMode::SaveAndNew => FIXED_KEYS,
        EnterMode::MoveBetweenFields => FIXED_KEYS_MOVE,
    }
}

/// [`fixed_keys`] for [`EnterMode::SaveAndNew`], the default (Q-190).
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
    ("Ctrl+Home / Ctrl+End", "First / last row in grids"),
    (
        "Space",
        "Tick a checkbox or open the choices in a grid cell; select the row in registers",
    ),
    (
        "+ - * / in an amount",
        "Work the amount out; the exact result shows under the cell before you leave it",
    ),
    (
        "Shift+F10 / Menu key",
        "Open the actions menu of a grid cell or column heading (Bulk Entry: Copy Down, Columns\u{2026})",
    ),
    (
        "Shift+Up / Shift+Down",
        "Select rows from the cursor row in grids with a select column (Bulk Entry)",
    ),
    (
        "Alt+Up / Alt+Down",
        "Move the focused column up or down in Bulk Entry's Columns\u{2026}",
    ),
];

/// [`fixed_keys`] for [`EnterMode::MoveBetweenFields`] (Q-264).
pub const FIXED_KEYS_MOVE: &[(&str, &str)] = &[
    ("Tab / Shift+Tab", "Next / previous field or cell"),
    (
        "Enter / Shift+Enter",
        "Next / previous field or cell; Enter in the last cell of a grid line starts a new line",
    ),
    ("Ctrl+Enter", "Save & New"),
    ("Arrow keys", "Move between rows and cells"),
    (
        "Page Up / Page Down",
        "Previous / next record in forms; page in grids",
    ),
    (
        "Esc",
        "Revert the field, then the form, then close (asks first if there are unsaved changes)",
    ),
    ("Ctrl+Home / Ctrl+End", "First / last row in grids"),
    (
        "Space",
        "Tick a checkbox or open the choices in a grid cell; select the row in registers",
    ),
    (
        "+ - * / in an amount",
        "Work the amount out; the exact result shows under the cell before you leave it",
    ),
    (
        "Shift+F10 / Menu key",
        "Open the actions menu of a grid cell or column heading (Bulk Entry: Copy Down, Columns\u{2026})",
    ),
    (
        "Shift+Up / Shift+Down",
        "Select rows from the cursor row in grids with a select column (Bulk Entry)",
    ),
    (
        "Alt+Up / Alt+Down",
        "Move the focused column up or down in Bulk Entry's Columns\u{2026}",
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
    fn grid_scope_is_innermost() {
        let km = Keymap::new(COMMANDS).unwrap();
        let at = |focus: Scope, c: char| {
            km.resolve(focus, Shortcut::new(Mods::CMD, Key::Letter(c)))
                .map(CommandId::as_str)
        };
        // Ctrl+D: copy down in an opted-in grid, delete in a form.
        assert_eq!(at(Scope::Grid, 'D'), Some("grid.copy_down"));
        assert_eq!(at(Scope::Form, 'D'), Some("form.delete_item"));
        assert_eq!(at(Scope::Global, 'D'), Some("banking.deposit"));
        // Ctrl+A: select all in a grid, the Chart of Accounts elsewhere.
        assert_eq!(at(Scope::Grid, 'A'), Some("grid.select_all"));
        assert_eq!(at(Scope::Form, 'A'), Some("lists.chart_of_accounts"));
        assert_eq!(at(Scope::Global, 'A'), Some("lists.chart_of_accounts"));
        // Form keys still work underneath the grid scope.
        assert_eq!(
            km.resolve(Scope::Grid, Shortcut::new(Mods::ALT, Key::Letter('N')))
                .map(CommandId::as_str),
            Some("form.save_new")
        );
        assert_eq!(Scope::Grid.title(), "In a grid");
        assert_eq!(Scope::Grid.active_from()[0], Scope::Grid);
        assert_eq!(Scope::ALL.len(), 5);
    }

    #[test]
    fn viewer_scope_is_innermost_and_keeps_form_keys() {
        let km = Keymap::new(COMMANDS).unwrap();
        let at = |focus: Scope, mods: Mods, k: Key| {
            km.resolve(focus, Shortcut::new(mods, k))
                .map(CommandId::as_str)
        };
        // Zoom keys zoom the document in a preview, the app elsewhere.
        assert_eq!(
            at(Scope::Viewer, Mods::CMD, Key::Equals),
            Some("viewer.zoom_in")
        );
        assert_eq!(
            at(Scope::Viewer, Mods::CMD, Key::Minus),
            Some("viewer.zoom_out")
        );
        assert_eq!(
            at(Scope::Viewer, Mods::CMD, Key::Digit(0)),
            Some("viewer.fit")
        );
        assert_eq!(
            at(Scope::Form, Mods::CMD, Key::Equals),
            Some("window.zoom_in")
        );
        assert_eq!(
            at(Scope::Window, Mods::CMD, Key::Digit(0)),
            Some("window.zoom_reset")
        );
        // Page Up/Down turn pages in a preview, records in a form.
        assert_eq!(
            at(Scope::Viewer, Mods::NONE, Key::PageDown),
            Some("viewer.page_next")
        );
        assert_eq!(
            at(Scope::Viewer, Mods::NONE, Key::PageUp),
            Some("viewer.page_prev")
        );
        assert_eq!(
            at(Scope::Form, Mods::NONE, Key::PageDown),
            Some("form.next_record")
        );
        // Form keys still work underneath the preview.
        assert_eq!(
            at(Scope::Viewer, Mods::ALT, Key::Letter('S')),
            Some("form.save_close")
        );
        assert_eq!(
            at(Scope::Viewer, Mods::ALT, Key::Letter('A')),
            Some("bill.accept_read")
        );
        // A grid never sees viewer keys.
        assert!(!Scope::Grid.active_from().contains(&Scope::Viewer));
        assert_eq!(Scope::Viewer.title(), "In a document preview");
    }

    #[test]
    fn enter_mode_binds_ctrl_enter_to_save_and_new() {
        let ctrl_enter = Shortcut::new(Mods::CMD, Key::Enter);
        let alt_n = Shortcut::new(Mods::ALT, Key::Letter('N'));

        let def = Keymap::new(COMMANDS).unwrap();
        assert_eq!(def.mode(), EnterMode::SaveAndNew);
        assert_eq!(def.resolve(Scope::Form, ctrl_enter), None);
        assert_eq!(def.shortcuts_of("form.save_new"), vec![alt_n]);
        assert_eq!(EnterMode::default(), EnterMode::SaveAndNew);

        let mv = Keymap::with_mode(COMMANDS, EnterMode::MoveBetweenFields).unwrap();
        assert_eq!(mv.mode(), EnterMode::MoveBetweenFields);
        for focus in [Scope::Form, Scope::Grid] {
            assert_eq!(
                mv.resolve(focus, ctrl_enter).map(CommandId::as_str),
                Some("form.save_new")
            );
        }
        assert_eq!(mv.resolve(Scope::Global, ctrl_enter), None);
        assert_eq!(mv.shortcuts_of("form.save_new"), vec![alt_n, ctrl_enter]);
        assert_eq!(mv.len(), def.len() + 1);
        // Unknown or unbound commands have no shortcuts.
        assert_eq!(mv.shortcuts_of("nope"), Vec::<Shortcut>::new());
        assert_eq!(mv.shortcuts_of("form.attach"), Vec::<Shortcut>::new());
    }

    #[test]
    fn mode_binding_skips_a_missing_command_and_refuses_a_clash() {
        let without: Vec<Command> = COMMANDS
            .iter()
            .copied()
            .filter(|c| c.id.as_str() != "form.save_new")
            .collect();
        let km = Keymap::with_mode(&without, EnterMode::MoveBetweenFields).unwrap();
        assert_eq!(
            km.resolve(Scope::Form, Shortcut::new(Mods::CMD, Key::Enter)),
            None
        );
        // A table command already on Ctrl+Enter in a form clashes.
        let mut clash = COMMANDS.to_vec();
        let mut c = *clash
            .iter()
            .find(|c| c.id.as_str() == "form.attach")
            .unwrap();
        c.id = CommandId::new("test.ctrl_enter");
        c.shortcut = Some(Shortcut::new(Mods::CMD, Key::Enter));
        clash.push(c);
        assert!(Keymap::new(&clash).is_ok());
        assert!(matches!(
            Keymap::with_mode(&clash, EnterMode::MoveBetweenFields),
            Err(KeymapError::Conflict { .. })
        ));
    }

    #[test]
    fn fixed_keys_follow_the_enter_mode() {
        assert_eq!(fixed_keys(EnterMode::SaveAndNew), FIXED_KEYS);
        let mv = fixed_keys(EnterMode::MoveBetweenFields);
        assert!(
            mv.iter()
                .any(|(k, a)| *k == "Ctrl+Enter" && *a == "Save & New")
        );
        assert!(mv.iter().any(|(k, _)| k.starts_with("Enter / Shift+Enter")));
        assert!(FIXED_KEYS.iter().all(|(k, _)| *k != "Ctrl+Enter"));
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
