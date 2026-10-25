//! Per-user preferences (P09 step 1): what one owner on one computer likes,
//! independent of the company that is open. Stored as `ui-prefs.json` in
//! the app data folder (the file the shell has used since P06 for theme and
//! zoom; the new fields are additive, so an older file still reads).
//!
//! Company-wide rules (basis, closing date, sales tax, invoice numbering)
//! live in the engine (`/company/settings`); company-local choices the
//! engine has no column for live in `companies.json` (`CompanyPrefs`).

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::{read_capped, write_atomic};

/// The owner's colour choice (Window menu, Preferences).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    /// Light colours.
    Light,
    /// Dark colours.
    Dark,
    /// Follow the operating system.
    #[default]
    Auto,
}

impl ThemeChoice {
    /// Next in the Light, Dark, Automatic cycle.
    #[must_use]
    pub fn next(self) -> ThemeChoice {
        match self {
            ThemeChoice::Light => ThemeChoice::Dark,
            ThemeChoice::Dark => ThemeChoice::Auto,
            ThemeChoice::Auto => ThemeChoice::Light,
        }
    }

    /// i18n key of the name.
    #[must_use]
    pub fn label_key(self) -> &'static str {
        match self {
            ThemeChoice::Light => "theme.light",
            ThemeChoice::Dark => "theme.dark",
            ThemeChoice::Auto => "theme.auto",
        }
    }

    /// Every choice, in menu order.
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::Light, ThemeChoice::Dark, ThemeChoice::Auto];
}

/// How dates are shown and typed (Q&A round 42: `MM/DD/YYYY` by default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateFormat {
    /// `11/04/2026`.
    #[default]
    MonthDayYear,
    /// `2026-11-04`.
    YearMonthDay,
}

impl DateFormat {
    /// Every choice.
    pub const ALL: [DateFormat; 2] = [DateFormat::MonthDayYear, DateFormat::YearMonthDay];

    /// i18n key of the name.
    #[must_use]
    pub fn label_key(self) -> &'static str {
        match self {
            DateFormat::MonthDayYear => "prefs.user.date_mdy",
            DateFormat::YearMonthDay => "prefs.user.date_ymd",
        }
    }
}

/// How negative amounts are shown (DESIGN.md: leading minus by default,
/// parentheses as a setting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NegativeStyle {
    /// `-1,234.56`.
    #[default]
    Minus,
    /// `(1,234.56)`.
    Parentheses,
}

impl NegativeStyle {
    /// Every choice.
    pub const ALL: [NegativeStyle; 2] = [NegativeStyle::Minus, NegativeStyle::Parentheses];

    /// i18n key of the name.
    #[must_use]
    pub fn label_key(self) -> &'static str {
        match self {
            NegativeStyle::Minus => "prefs.user.neg_minus",
            NegativeStyle::Parentheses => "prefs.user.neg_parens",
        }
    }
}

/// What opens when a company opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartScreen {
    /// The Home workspace.
    #[default]
    Home,
    /// The windows that were open when the company was last closed.
    LastWindows,
    /// The Bank Feed.
    BankFeed,
}

impl StartScreen {
    /// Every choice.
    pub const ALL: [StartScreen; 3] = [
        StartScreen::Home,
        StartScreen::LastWindows,
        StartScreen::BankFeed,
    ];

    /// i18n key of the name.
    #[must_use]
    pub fn label_key(self) -> &'static str {
        match self {
            StartScreen::Home => "prefs.user.start_home",
            StartScreen::LastWindows => "prefs.user.start_last",
            StartScreen::BankFeed => "prefs.user.start_feed",
        }
    }
}

/// When the app looks for a new version (Q-162 family: manual only in
/// beta; nothing phones home by itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateCheck {
    /// Only when the owner clicks "Check for updates".
    #[default]
    Manual,
}

/// Date-warning windows (Q-97): a date further back or ahead than this gets
/// a warning before the save.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DateWarnings {
    /// Days in the past (default 90).
    pub past_days: u16,
    /// Days in the future (default 30).
    pub future_days: u16,
}

impl Default for DateWarnings {
    fn default() -> Self {
        DateWarnings {
            past_days: 90,
            future_days: 30,
        }
    }
}

/// Largest warning window accepted (ten years).
pub const DATE_WARNING_MAX_DAYS: u16 = 3660;

/// Allowed zoom percentages, smallest to largest (Q-66).
pub const ZOOM_STEPS: [u16; 9] = [80, 90, 100, 110, 125, 150, 175, 190, 200];

/// Default zoom.
pub const ZOOM_DEFAULT: u16 = 100;

/// The step nearest to `percent`.
#[must_use]
pub fn nearest_step(percent: u16) -> u16 {
    ZOOM_STEPS
        .iter()
        .copied()
        .min_by_key(|s| s.abs_diff(percent))
        .unwrap_or(ZOOM_DEFAULT)
}

/// One step up (no change at the top).
#[must_use]
pub fn zoom_in(percent: u16) -> u16 {
    let cur = nearest_step(percent);
    ZOOM_STEPS.iter().copied().find(|s| *s > cur).unwrap_or(cur)
}

/// One step down (no change at the bottom).
#[must_use]
pub fn zoom_out(percent: u16) -> u16 {
    let cur = nearest_step(percent);
    ZOOM_STEPS
        .iter()
        .rev()
        .copied()
        .find(|s| *s < cur)
        .unwrap_or(cur)
}

/// What is remembered for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserPrefs {
    /// Light, dark, or follow the system.
    pub theme: ThemeChoice,
    /// Zoom in percent; always one of [`ZOOM_STEPS`].
    pub zoom_percent: u16,
    /// Date display.
    pub date_format: DateFormat,
    /// Negative amounts.
    pub negatives: NegativeStyle,
    /// First window after a company opens.
    pub start_screen: StartScreen,
    /// Date-warning windows.
    pub date_warnings: DateWarnings,
    /// Update checks.
    pub update_check: UpdateCheck,
    /// "Enter moves between fields" (Q-264): Enter goes to the next field
    /// or grid cell like Tab, Shift+Enter goes back, and Ctrl+Enter is
    /// Save & New. Off by default (Enter = Save & New, Q-190). A personal
    /// keyboard habit, so it stays with the user, not the company (Q-P09c,
    /// P16 plan D3).
    pub enter_moves_fields: bool,
}

impl Default for UserPrefs {
    fn default() -> Self {
        UserPrefs {
            theme: ThemeChoice::Auto,
            zoom_percent: ZOOM_DEFAULT,
            date_format: DateFormat::default(),
            negatives: NegativeStyle::default(),
            start_screen: StartScreen::default(),
            date_warnings: DateWarnings::default(),
            update_check: UpdateCheck::Manual,
            enter_moves_fields: false,
        }
    }
}

impl UserPrefs {
    /// Parse stored text. Unreadable text gives the defaults; a zoom outside
    /// the allowed range is snapped to the nearest step and the warning
    /// windows are clamped.
    #[must_use]
    pub fn parse(text: &str) -> UserPrefs {
        let mut p: UserPrefs = serde_json::from_str(text).unwrap_or_default();
        p.normalize();
        p
    }

    /// Snaps every field into its allowed range.
    pub fn normalize(&mut self) {
        self.zoom_percent = nearest_step(self.zoom_percent);
        self.date_warnings.past_days = self.date_warnings.past_days.min(DATE_WARNING_MAX_DAYS);
        self.date_warnings.future_days = self.date_warnings.future_days.min(DATE_WARNING_MAX_DAYS);
    }

    /// Text to store.
    #[must_use]
    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// The factor egui wants (1.0 = 100 percent). Display only.
    #[must_use]
    pub fn zoom_factor(&self) -> f32 {
        f32::from(self.zoom_percent) / 100.0
    }
}

/// Name of the preferences file.
pub const USER_PREFS_FILE: &str = "ui-prefs.json";

/// Where the user preferences file lives.
#[derive(Debug, Clone)]
pub struct UserPrefsStore {
    path: PathBuf,
}

impl UserPrefsStore {
    /// The store in `dir` (created on first save).
    #[must_use]
    pub fn in_dir(dir: &Path) -> UserPrefsStore {
        UserPrefsStore {
            path: dir.join(USER_PREFS_FILE),
        }
    }

    /// The file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load (defaults when missing or unreadable).
    #[must_use]
    pub fn load(&self) -> UserPrefs {
        match read_capped(&self.path) {
            Ok(Some(text)) => UserPrefs::parse(&text),
            _ => UserPrefs::default(),
        }
    }

    /// Save atomically.
    ///
    /// # Errors
    /// Any file error.
    pub fn save(&self, prefs: &UserPrefs) -> io::Result<()> {
        let mut p = *prefs;
        p.normalize();
        write_atomic(&self.path, &p.to_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps_stay_inside_80_to_200() {
        assert_eq!(ZOOM_STEPS.first(), Some(&80));
        assert_eq!(ZOOM_STEPS.last(), Some(&200));
        let mut z = 100;
        for _ in 0..20 {
            z = zoom_in(z);
        }
        assert_eq!(z, 200);
        for _ in 0..20 {
            z = zoom_out(z);
        }
        assert_eq!(z, 80);
        assert_eq!(zoom_in(100), 110);
        assert_eq!(zoom_out(100), 90);
        assert_eq!(nearest_step(0), 80);
        assert_eq!(nearest_step(999), 200);
        assert_eq!(nearest_step(104), 100);
    }

    #[test]
    fn theme_cycle_and_labels() {
        assert_eq!(ThemeChoice::Light.next(), ThemeChoice::Dark);
        assert_eq!(ThemeChoice::Dark.next(), ThemeChoice::Auto);
        assert_eq!(ThemeChoice::Auto.next(), ThemeChoice::Light);
        assert_eq!(ThemeChoice::Auto.label_key(), "theme.auto");
        assert_eq!(DateFormat::YearMonthDay.label_key(), "prefs.user.date_ymd");
        assert_eq!(
            NegativeStyle::Parentheses.label_key(),
            "prefs.user.neg_parens"
        );
        assert_eq!(StartScreen::BankFeed.label_key(), "prefs.user.start_feed");
    }

    #[test]
    fn prefs_round_trip_old_files_and_clamping() {
        let p = UserPrefs {
            theme: ThemeChoice::Dark,
            zoom_percent: 150,
            date_format: DateFormat::YearMonthDay,
            negatives: NegativeStyle::Parentheses,
            start_screen: StartScreen::LastWindows,
            date_warnings: DateWarnings {
                past_days: 10,
                future_days: 5,
            },
            update_check: UpdateCheck::Manual,
            enter_moves_fields: true,
        };
        assert_eq!(UserPrefs::parse(&p.to_text()), p);
        assert_eq!(UserPrefs::parse("garbage"), UserPrefs::default());
        // The P06 file had only theme and zoom.
        let old = UserPrefs::parse(r#"{"theme":"light","zoom_percent":500}"#);
        assert_eq!(old.theme, ThemeChoice::Light);
        assert_eq!(old.zoom_percent, 200);
        assert_eq!(old.date_format, DateFormat::MonthDayYear);
        assert_eq!(old.date_warnings, DateWarnings::default());
        assert!((old.zoom_factor() - 2.0).abs() < f32::EPSILON);
        let wide = UserPrefs::parse(r#"{"date_warnings":{"past_days":65535,"future_days":4000}}"#);
        assert_eq!(wide.date_warnings.past_days, DATE_WARNING_MAX_DAYS);
        assert_eq!(wide.date_warnings.future_days, DATE_WARNING_MAX_DAYS);
    }

    #[test]
    fn enter_moves_fields_is_off_in_older_files() {
        // A P09-era file (every field before P16) still reads, with the
        // Enter option off (Q-264: off by default).
        let p09 = UserPrefs::parse(
            r#"{"theme":"dark","zoom_percent":110,"date_format":"month_day_year",
                "negatives":"minus","start_screen":"home",
                "date_warnings":{"past_days":90,"future_days":30},
                "update_check":"manual"}"#,
        );
        assert_eq!(p09.theme, ThemeChoice::Dark);
        assert!(!p09.enter_moves_fields);
        assert!(!UserPrefs::default().enter_moves_fields);
        let on = UserPrefs::parse(r#"{"enter_moves_fields":true}"#);
        assert!(on.enter_moves_fields);
        assert!(on.to_text().contains("\"enter_moves_fields\": true"));
    }

    #[test]
    fn store_saves_and_loads() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = UserPrefsStore::in_dir(dir.path());
        assert_eq!(store.load(), UserPrefs::default());
        let p = UserPrefs {
            theme: ThemeChoice::Light,
            zoom_percent: 125,
            ..UserPrefs::default()
        };
        store.save(&p).expect("save");
        assert_eq!(store.load(), p);
        assert!(store.path().ends_with(USER_PREFS_FILE));
        std::fs::write(store.path(), "x".repeat(70)).expect("write");
        assert_eq!(store.load(), UserPrefs::default());
    }
}
