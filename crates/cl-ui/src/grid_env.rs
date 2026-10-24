//! What a grid screen needs from the shell each frame (P10 plan §2.3,
//! §2.4.5, §2.13): the theme's grid style, today, date and money formats,
//! whether a shell modal is open, and the remembered column widths.
//!
//! The shell publishes a [`ShellFrame`] for every viewport it draws before
//! the screens draw. Screens capture it with [`EnvParts::capture`] and build
//! a [`GridEnv`] from it; commands that arrive between frames (the keymap,
//! the close prompt) reuse the parts captured last frame.

use std::sync::{Arc, Mutex};

use cl_core::{CalendarDate, MoneyStyle, NegativeStyle, Weekday};
use cl_grid::{ColumnId, GridEnv, GridHeight, GridStyle, Lookups, TextKey};
use cl_model::date_entry::{DateOrder, US_FIRST_WEEKDAY};
use cl_model::forms::EnterMode;
use eframe::egui::{Context, Id, ViewportId};

use crate::prefs::{GridWidths, UiPrefs};

/// The shell's per-frame facts for grids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellFrame {
    /// Today (fixed in the demo).
    pub today: CalendarDate,
    /// The first day of the week (US: Sunday).
    pub first_weekday: Weekday,
    /// Date display order from the preferences.
    pub date_order: DateOrder,
    /// Negative-amount style from the preferences.
    pub money_style: MoneyStyle,
    /// A shell modal (prompt, palette, dialog) is open: grids give up focus
    /// and ignore input (B6).
    pub modal_open: bool,
    /// What Enter does in forms and grids ("Enter moves between fields",
    /// Q-264).
    pub enter_mode: EnterMode,
}

impl ShellFrame {
    /// The facts for these preferences.
    #[must_use]
    pub fn new(today: CalendarDate, prefs: &UiPrefs, modal_open: bool) -> ShellFrame {
        use cl_settings::user_prefs::{DateFormat, NegativeStyle as PrefNegative};
        ShellFrame {
            today,
            first_weekday: US_FIRST_WEEKDAY,
            date_order: match prefs.date_format {
                DateFormat::MonthDayYear => DateOrder::Mdy,
                DateFormat::YearMonthDay => DateOrder::Ymd,
            },
            money_style: MoneyStyle {
                negative: match prefs.negatives {
                    PrefNegative::Minus => NegativeStyle::Minus,
                    PrefNegative::Parentheses => NegativeStyle::Parens,
                },
                symbol: false,
            },
            modal_open,
            enter_mode: EnterMode::from_pref(prefs.enter_moves_fields),
        }
    }
}

/// The Enter mode of the viewport being drawn (Save & New outside a shell).
#[must_use]
pub fn enter_mode(ctx: &Context) -> EnterMode {
    shell_frame(ctx).enter_mode
}

impl Default for ShellFrame {
    fn default() -> Self {
        ShellFrame::new(crate::shell::today(), &UiPrefs::default(), false)
    }
}

fn frame_id(viewport: ViewportId) -> Id {
    Id::unique(("cl.shell_frame", viewport))
}

fn widths_id() -> Id {
    Id::unique("cl.grid_widths")
}

/// Publish the facts for `viewport` (the shell calls this before drawing
/// that viewport's screens).
pub fn publish(ctx: &Context, viewport: ViewportId, frame: ShellFrame) {
    ctx.data_mut(|d| d.insert_temp(frame_id(viewport), frame));
}

/// The facts for the viewport being drawn; the root viewport's when this
/// one has none; defaults outside a shell (tests).
#[must_use]
pub fn shell_frame(ctx: &Context) -> ShellFrame {
    let here = ctx.viewport_id();
    ctx.data(|d| {
        d.get_temp::<ShellFrame>(frame_id(here))
            .or_else(|| d.get_temp(frame_id(ViewportId::ROOT)))
    })
    .unwrap_or_default()
}

/// Remembered column widths shared between the shell (which loads and
/// writes the file) and the screens (which read and update entries).
#[derive(Debug, Default)]
pub struct WidthsState {
    /// The widths.
    pub book: GridWidths,
    /// When an entry last changed and is not written yet (egui time).
    pub changed_at: Option<f64>,
    /// The register choices per company and account (P12), shared the same
    /// way and written by the same rules.
    pub registers: crate::prefs::RegisterPrefsBook,
    /// When a register entry last changed and is not written yet.
    pub registers_changed_at: Option<f64>,
    /// The detail panels the owner hid, per screen kind (Q-266).
    pub panels: crate::prefs::PanelPrefsBook,
    /// When a panel choice last changed and is not written yet.
    pub panels_changed_at: Option<f64>,
}

/// True when the owner hid the detail panel of screen kind `key`; `None`
/// when never set or outside a shell.
#[must_use]
pub fn load_panel_collapsed(ctx: &Context, key: &str) -> Option<bool> {
    let handle: WidthsHandle = ctx.data(|d| d.get_temp(widths_id()))?;
    let state = handle.lock().ok()?;
    state.panels.get(key)
}

/// Remember the panel choice of screen kind `key`; the shell writes the
/// file a second after the last change, and never during a controlled run.
pub fn store_panel_collapsed(ctx: &Context, key: &str, collapsed: bool) {
    let Some(handle): Option<WidthsHandle> = ctx.data(|d| d.get_temp(widths_id())) else {
        return;
    };
    let now = ctx.input(|i| i.time);
    if let Ok(mut state) = handle.lock()
        && state.panels.set(key, collapsed)
    {
        state.panels_changed_at = Some(now);
    }
}

/// The register choices remembered for `key` (see
/// [`crate::prefs::RegisterPrefsBook::key`]).
#[must_use]
pub fn load_register_prefs(ctx: &Context, key: &str) -> Option<cl_model::register::RegisterPrefs> {
    let handle: WidthsHandle = ctx.data(|d| d.get_temp(widths_id()))?;
    let state = handle.lock().ok()?;
    state.registers.get(key)
}

/// Remember a register's choices; the shell writes the file a second
/// after the last change, and never during a controlled run.
pub fn store_register_prefs(ctx: &Context, key: &str, prefs: cl_model::register::RegisterPrefs) {
    let Some(handle): Option<WidthsHandle> = ctx.data(|d| d.get_temp(widths_id())) else {
        return;
    };
    let now = ctx.input(|i| i.time);
    if let Ok(mut state) = handle.lock()
        && state.registers.set(key, prefs)
    {
        state.registers_changed_at = Some(now);
    }
}

/// The shared widths.
pub type WidthsHandle = Arc<Mutex<WidthsState>>;

/// Make the widths available to screens.
pub fn publish_widths(ctx: &Context, handle: &WidthsHandle) {
    ctx.data_mut(|d| d.insert_temp(widths_id(), Arc::clone(handle)));
}

/// The widths remembered for a grid key.
#[must_use]
pub fn load_widths(ctx: &Context, key: &str) -> Option<Vec<(ColumnId, u16)>> {
    let handle: WidthsHandle = ctx.data(|d| d.get_temp(widths_id()))?;
    let state = handle.lock().ok()?;
    state.book.get(key)
}

/// Remember a grid's widths; the shell writes the file a second after the
/// last change, and never during a controlled run.
pub fn store_widths(ctx: &Context, key: &str, widths: &[(ColumnId, u16)]) {
    let Some(handle): Option<WidthsHandle> = ctx.data(|d| d.get_temp(widths_id())) else {
        return;
    };
    let now = ctx.input(|i| i.time);
    if let Ok(mut state) = handle.lock()
        && state.book.set(key, widths)
    {
        state.changed_at = Some(now);
    }
}

/// The grid's text resolver: every `TextKey` goes through the catalog.
#[must_use]
pub fn grid_text(key: TextKey) -> String {
    crate::i18n::t(key.0)
}

/// Everything a [`GridEnv`] needs except the screen's own parts (lookups,
/// label, height). Kept by the screen between frames.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvParts {
    /// Colours and fonts.
    pub style: GridStyle,
    /// The shell's facts.
    pub frame: ShellFrame,
}

impl Default for EnvParts {
    fn default() -> Self {
        EnvParts {
            style: crate::theme::grid_style(&crate::theme::Tokens::LIGHT),
            frame: ShellFrame::default(),
        }
    }
}

impl EnvParts {
    /// The parts for the viewport being drawn.
    #[must_use]
    pub fn capture(ctx: &Context) -> EnvParts {
        EnvParts {
            style: crate::theme::grid_style_of(ctx),
            frame: shell_frame(ctx),
        }
    }

    /// A grid environment.
    #[must_use]
    pub fn env<'a>(
        &'a self,
        lookups: &'a dyn Lookups,
        label: &'a str,
        height: GridHeight,
    ) -> GridEnv<'a> {
        GridEnv {
            style: &self.style,
            text: &grid_text,
            lookups,
            today: self.frame.today,
            first_weekday: self.frame.first_weekday,
            date_order: self.frame.date_order,
            money_style: self.frame.money_style,
            height,
            label,
            input_enabled: !self.frame.modal_open,
            enter_mode: self.frame.enter_mode,
            context: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_grid::NoLookups;

    #[test]
    fn frames_are_per_viewport_with_the_root_as_fallback() {
        let ctx = Context::default();
        let today = CalendarDate::from_ymd(2026, 11, 4).unwrap();
        let root = ShellFrame::new(today, &UiPrefs::default(), true);
        publish(&ctx, ViewportId::ROOT, root);
        let mut seen = None;
        let mut out = ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
            seen = Some(shell_frame(ui.ctx()));
        });
        out.textures_delta.clear();
        assert_eq!(seen, Some(root));
        let parts = EnvParts {
            frame: root,
            ..EnvParts::default()
        };
        let env = parts.env(&NoLookups, "Lines", GridHeight::Rows { min: 3, max: 8 });
        assert!(!env.input_enabled, "a modal disables grid input");
        assert_eq!(env.today, today);
        assert_eq!((env.text)(TextKey("grid.a11y.line")), "line {n}");
    }

    #[test]
    fn preferences_choose_date_order_and_negatives() {
        use cl_settings::user_prefs::{DateFormat, NegativeStyle as P};
        let prefs = UiPrefs {
            date_format: DateFormat::YearMonthDay,
            negatives: P::Parentheses,
            ..UiPrefs::default()
        };
        let f = ShellFrame::new(CalendarDate::min(), &prefs, false);
        assert_eq!(f.date_order, DateOrder::Ymd);
        assert_eq!(f.money_style.negative, NegativeStyle::Parens);
        assert_eq!(f.first_weekday, Weekday::Sun);
        assert_eq!(f.enter_mode, EnterMode::SaveAndNew, "off by default");
        let on = UiPrefs {
            enter_moves_fields: true,
            ..UiPrefs::default()
        };
        let f = ShellFrame::new(CalendarDate::min(), &on, false);
        assert_eq!(f.enter_mode, EnterMode::MoveBetweenFields);
        let parts = EnvParts {
            frame: f,
            ..EnvParts::default()
        };
        let env = parts.env(&NoLookups, "Lines", GridHeight::Rows { min: 3, max: 8 });
        assert_eq!(env.enter_mode, EnterMode::MoveBetweenFields);
    }

    #[test]
    fn widths_go_through_the_shared_book() {
        let ctx = Context::default();
        assert_eq!(load_widths(&ctx, "a/lines"), None);
        store_widths(&ctx, "a/lines", &[(ColumnId(1), 120)]); // no handle: ignored
        let handle = WidthsHandle::default();
        publish_widths(&ctx, &handle);
        store_widths(&ctx, "a/lines", &[(ColumnId(1), 120)]);
        assert_eq!(load_widths(&ctx, "a/lines"), Some(vec![(ColumnId(1), 120)]));
        assert!(handle.lock().unwrap().changed_at.is_some());
    }
}
