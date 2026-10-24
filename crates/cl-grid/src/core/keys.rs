//! egui-free inputs to the editing state machine (P10 plan §2.4, Rev 2
//! §2.4.4). `view/input.rs` turns egui events into these, matching
//! modifiers exactly (never `consume_key`, F8).

use super::model::CellPos;
use super::spec::GridCommand;
use cl_core::Weekday;

/// The keys the grid handles itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GridKey {
    /// Tab (Shift+Tab with `shift`).
    Tab,
    /// Enter.
    Enter,
    /// Esc (raw; grid screens normally receive it as `Revert`).
    Escape,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Home.
    Home,
    /// End.
    End,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// Space (with its text event already consumed).
    Space,
    /// Backspace.
    Backspace,
    /// Shift+F10 or the Menu key: the context menu on the cursor cell (the
    /// view opens it; with a menu open the core closes it).
    ContextMenu,
}

/// Modifiers, matched exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Mods {
    /// Ctrl (Cmd on macOS).
    pub ctrl: bool,
    /// Shift.
    pub shift: bool,
    /// Alt.
    pub alt: bool,
}

impl Mods {
    /// No modifiers.
    pub const NONE: Mods = Mods {
        ctrl: false,
        shift: false,
        alt: false,
    };
    /// Shift only.
    pub const SHIFT: Mods = Mods {
        ctrl: false,
        shift: true,
        alt: false,
    };
    /// Ctrl only.
    pub const CTRL: Mods = Mods {
        ctrl: true,
        shift: false,
        alt: false,
    };
    /// Ctrl+Shift.
    pub const CTRL_SHIFT: Mods = Mods {
        ctrl: true,
        shift: true,
        alt: false,
    };
}

/// One input to the state machine. `Debug` never prints typed text or
/// list labels.
#[derive(Clone, PartialEq, Eq)]
pub enum GridInput {
    /// A key press.
    Key(GridKey, Mods),
    /// Typed text (one `Text` event). In a date cell a single date-key
    /// character changes the date instead (Q-P10f).
    Text(String),
    /// The editor's text after egui's `TextEdit` changed it (the view sends
    /// this; tests may use it to set the buffer directly).
    BufferEdited(String),
    /// A pointer press on a cell.
    Click {
        /// The cell.
        pos: CellPos,
        /// A double click.
        double: bool,
        /// Modifiers.
        mods: Mods,
    },
    /// A forwarded command.
    Command(GridCommand),
    /// The grid lost keyboard focus (another widget, a modal).
    FocusLost,
    /// The grid gained keyboard focus.
    FocusGained {
        /// Shift+Tab brought it in from below.
        from_below: bool,
    },
    /// "Add …" was picked in a picker popup.
    AddNewPicked,
    /// A highlighted popup item was clicked.
    PopupPicked(usize),
    /// Space or Alt+Down on a dropdown cell: open its choices (labels
    /// already resolved by the view).
    OpenChoices(Vec<super::model::Suggestion>),
    /// The list chevron on the cursor cell was clicked: open its list, or
    /// close it when open (Q-250).
    ToggleList,
    /// A press on a row's select checkbox ([`super::spec::GridSpec::select_column`]):
    /// toggles the row, Shift extends from the anchor.
    GutterClick {
        /// The row.
        row: super::rowkey::RowKey,
        /// Modifiers.
        mods: Mods,
    },
    /// A context menu entry was clicked (by index).
    MenuPicked(usize),
}

impl std::fmt::Debug for GridInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GridInput::Key(k, m) => f.debug_tuple("Key").field(k).field(m).finish(),
            GridInput::Text(_) => f.write_str("Text(..)"),
            GridInput::BufferEdited(_) => f.write_str("BufferEdited(..)"),
            GridInput::Click { pos, double, mods } => f
                .debug_struct("Click")
                .field("pos", pos)
                .field("double", double)
                .field("mods", mods)
                .finish(),
            GridInput::Command(c) => f.debug_tuple("Command").field(c).finish(),
            GridInput::FocusLost => f.write_str("FocusLost"),
            GridInput::FocusGained { from_below } => f
                .debug_struct("FocusGained")
                .field("from_below", from_below)
                .finish(),
            GridInput::AddNewPicked => f.write_str("AddNewPicked"),
            GridInput::PopupPicked(i) => f.debug_tuple("PopupPicked").field(i).finish(),
            GridInput::OpenChoices(items) => write!(f, "OpenChoices({} items)", items.len()),
            GridInput::ToggleList => f.write_str("ToggleList"),
            GridInput::GutterClick { row, mods } => f
                .debug_struct("GutterClick")
                .field("row", row)
                .field("mods", mods)
                .finish(),
            GridInput::MenuPicked(i) => f.debug_tuple("MenuPicked").field(i).finish(),
        }
    }
}

/// Values the state machine needs from its surroundings ("today" and the
/// owner's formats), never a clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Formats {
    /// For `T` in date cells and `M/D` input.
    pub today: cl_core::CalendarDate,
    /// For `W` and `K`.
    pub first_weekday: Weekday,
    /// Date display order.
    pub date_order: cl_model::date_entry::DateOrder,
    /// Negative amounts.
    pub money: cl_core::MoneyStyle,
    /// What Enter does in editable grids (Q-190, Q-264).
    pub enter_mode: cl_model::forms::EnterMode,
}
