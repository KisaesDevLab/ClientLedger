//! Printing choices kept on this computer (P19, owner 2026-10-29): the
//! check alignment offset per printer (±10 mm each way; the check style
//! stays per bank account in the engine) and the printer last used for
//! checks and for forms. Stored in `printers.json` in the app data folder.

use crate::store::{read_capped, write_atomic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// The printer preferences file name.
pub const PRINTER_PREFS_FILE: &str = "printers.json";
/// Largest offset either way, in millimetres.
pub const MAX_OFFSET_MM: f32 = 10.0;
/// Printers remembered at most (beyond it, names first in sort order are
/// dropped).
pub const MAX_PRINTERS: usize = 64;
/// Longest printer name kept.
pub const MAX_PRINTER_NAME: usize = 256;

/// A printer's check feed correction.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PrinterOffset {
    /// Positive moves right.
    pub offset_x_mm: f32,
    /// Positive moves down.
    pub offset_y_mm: f32,
}

/// What is being printed, for "last printer used".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintKind {
    /// Checks.
    Checks,
    /// Invoices, receipts, statements and other forms.
    Forms,
}

/// Why an offset was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OffsetError {
    /// Beyond ±10 mm or not a number.
    #[error("the adjustment must be within 10 mm in each direction")]
    OutOfRange,
    /// A blank or overlong printer name.
    #[error("the printer name is not usable")]
    BadPrinterName,
}

/// The remembered printing choices.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PrinterPrefs {
    /// Offset per printer name.
    pub offsets: BTreeMap<String, PrinterOffset>,
    /// The printer last used for checks.
    pub last_checks_printer: Option<String>,
    /// The printer last used for forms.
    pub last_forms_printer: Option<String>,
}

fn in_range(v: f32) -> bool {
    v.is_finite() && v.abs() <= MAX_OFFSET_MM
}

fn usable_name(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= MAX_PRINTER_NAME
}

impl PrinterPrefs {
    /// Parse stored text; unreadable text gives the defaults, and offsets
    /// out of range or names that are not usable are dropped.
    #[must_use]
    pub fn parse(text: &str) -> PrinterPrefs {
        let mut p: PrinterPrefs = serde_json::from_str(text).unwrap_or_default();
        p.normalize();
        p
    }

    /// Drop anything out of range.
    pub fn normalize(&mut self) {
        self.offsets.retain(|name, o| {
            usable_name(name) && in_range(o.offset_x_mm) && in_range(o.offset_y_mm)
        });
        while self.offsets.len() > MAX_PRINTERS {
            let Some(first) = self.offsets.keys().next().cloned() else {
                break;
            };
            self.offsets.remove(&first);
        }
        for p in [&mut self.last_checks_printer, &mut self.last_forms_printer] {
            if p.as_deref().is_some_and(|n| !usable_name(n)) {
                *p = None;
            }
        }
    }

    /// Text to store.
    #[must_use]
    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// The printer's offset (none saved: no correction).
    #[must_use]
    pub fn offset_for(&self, printer: &str) -> PrinterOffset {
        self.offsets.get(printer).copied().unwrap_or_default()
    }

    /// Save a printer's offset; a zero offset forgets the printer.
    ///
    /// # Errors
    /// [`OffsetError::OutOfRange`] or [`OffsetError::BadPrinterName`].
    pub fn set_offset(
        &mut self,
        printer: &str,
        right_mm: f32,
        down_mm: f32,
    ) -> Result<(), OffsetError> {
        if !usable_name(printer) {
            return Err(OffsetError::BadPrinterName);
        }
        if !in_range(right_mm) || !in_range(down_mm) {
            return Err(OffsetError::OutOfRange);
        }
        if right_mm.abs() < 0.001 && down_mm.abs() < 0.001 {
            self.offsets.remove(printer);
        } else {
            self.offsets.insert(
                printer.to_string(),
                PrinterOffset {
                    offset_x_mm: right_mm,
                    offset_y_mm: down_mm,
                },
            );
            self.normalize();
        }
        Ok(())
    }

    /// The printer last used for `kind`.
    #[must_use]
    pub fn last_printer(&self, kind: PrintKind) -> Option<&str> {
        match kind {
            PrintKind::Checks => self.last_checks_printer.as_deref(),
            PrintKind::Forms => self.last_forms_printer.as_deref(),
        }
    }

    /// Remember the printer used for `kind` (unusable names are ignored).
    pub fn set_last_printer(&mut self, kind: PrintKind, printer: &str) {
        if !usable_name(printer) {
            return;
        }
        let slot = match kind {
            PrintKind::Checks => &mut self.last_checks_printer,
            PrintKind::Forms => &mut self.last_forms_printer,
        };
        *slot = Some(printer.to_string());
    }
}

/// Where the printer preferences file lives.
#[derive(Debug, Clone)]
pub struct PrinterPrefsStore {
    path: PathBuf,
}

impl PrinterPrefsStore {
    /// The store in `dir` (created on first save).
    #[must_use]
    pub fn in_dir(dir: &Path) -> PrinterPrefsStore {
        PrinterPrefsStore {
            path: dir.join(PRINTER_PREFS_FILE),
        }
    }

    /// The file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load (defaults when missing or unreadable).
    #[must_use]
    pub fn load(&self) -> PrinterPrefs {
        match read_capped(&self.path) {
            Ok(Some(text)) => PrinterPrefs::parse(&text),
            _ => PrinterPrefs::default(),
        }
    }

    /// Save atomically.
    ///
    /// # Errors
    /// Any file error.
    pub fn save(&self, prefs: &PrinterPrefs) -> io::Result<()> {
        let mut p = prefs.clone();
        p.normalize();
        write_atomic(&self.path, &p.to_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_are_per_printer_and_limited_to_ten_millimetres() {
        let mut p = PrinterPrefs::default();
        assert_eq!(p.offset_for("Office Laser"), PrinterOffset::default());
        p.set_offset("Office Laser", 1.5, -2.0).unwrap();
        p.set_offset("Back Room Inkjet", -10.0, 10.0).unwrap();
        assert_eq!(
            p.offset_for("Office Laser"),
            PrinterOffset {
                offset_x_mm: 1.5,
                offset_y_mm: -2.0
            }
        );
        assert_eq!(p.offset_for("Back Room Inkjet").offset_x_mm, -10.0);
        assert_eq!(
            p.set_offset("Office Laser", 10.5, 0.0),
            Err(OffsetError::OutOfRange)
        );
        assert_eq!(
            p.set_offset("Office Laser", f32::NAN, 0.0),
            Err(OffsetError::OutOfRange)
        );
        assert_eq!(
            p.set_offset(" ", 1.0, 0.0),
            Err(OffsetError::BadPrinterName)
        );
        assert_eq!(p.offset_for("Office Laser").offset_x_mm, 1.5);
        p.set_offset("Office Laser", 0.0, 0.0).unwrap();
        assert!(!p.offsets.contains_key("Office Laser"));
    }

    #[test]
    fn last_printer_is_remembered_per_kind() {
        let mut p = PrinterPrefs::default();
        assert_eq!(p.last_printer(PrintKind::Checks), None);
        p.set_last_printer(PrintKind::Checks, "Office Laser");
        p.set_last_printer(PrintKind::Forms, "Back Room Inkjet");
        p.set_last_printer(PrintKind::Forms, "");
        assert_eq!(p.last_printer(PrintKind::Checks), Some("Office Laser"));
        assert_eq!(p.last_printer(PrintKind::Forms), Some("Back Room Inkjet"));
    }

    #[test]
    fn stored_text_round_trips_and_bad_values_are_dropped() {
        let mut p = PrinterPrefs::default();
        p.set_offset("A", 2.5, 0.5).unwrap();
        p.set_last_printer(PrintKind::Checks, "A");
        assert_eq!(PrinterPrefs::parse(&p.to_text()), p);
        assert_eq!(PrinterPrefs::parse("garbage"), PrinterPrefs::default());
        let bad = PrinterPrefs::parse(
            r#"{"offsets":{"A":{"offset_x_mm":40,"offset_y_mm":0},"B":{"offset_x_mm":1}," ":{}},
                "last_forms_printer":"   "}"#,
        );
        assert_eq!(bad.offsets.len(), 1);
        assert_eq!(bad.offset_for("B").offset_x_mm, 1.0);
        assert_eq!(bad.last_forms_printer, None);
        let mut many = PrinterPrefs::default();
        for i in 0..(MAX_PRINTERS + 5) {
            many.set_offset(&format!("P{i:03}"), 1.0, 1.0).unwrap();
        }
        assert_eq!(many.offsets.len(), MAX_PRINTERS);
    }

    #[test]
    fn store_saves_and_loads() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = PrinterPrefsStore::in_dir(dir.path());
        assert_eq!(store.load(), PrinterPrefs::default());
        let mut p = PrinterPrefs::default();
        p.set_offset("Office Laser", -1.0, 3.0).unwrap();
        store.save(&p).expect("save");
        assert_eq!(store.load(), p);
        assert!(store.path().ends_with(PRINTER_PREFS_FILE));
    }
}
