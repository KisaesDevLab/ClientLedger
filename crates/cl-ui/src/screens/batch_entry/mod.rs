//! Bulk Entry (P15A, `banking.bulk_entry`; Banking menu and the palette, no
//! global shortcut, Q-P15Ah), on the Write Check pattern with no mockup
//! (Q-P15An).
//!
//! One window holds sections (one type and account each) shown one at a
//! time in a single grid; Save all posts every section in one atomic engine
//! call (Q-P15Aa). See `target/plans/P15A.md`.
//!
//! WP0 scaffold: the command opens this window with a placeholder body.
//! Lane D builds the sections, grid, validation, paste, suggestions, save
//! and summary.

mod engine;

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::{CommandId, Scope};
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::i18n::t;
use crate::screens::company::form;
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "banking.bulk_entry";

/// The Bulk Entry window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    _open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(BulkEntryScreen::new(Arc::clone(link), today)) as Box<_>)
}

/// The Bulk Entry window.
pub struct BulkEntryScreen {
    #[allow(dead_code)] // lane D (WP-D1) reaches the engine through it
    link: Arc<CompanyLink>,
    #[allow(dead_code)] // lane D (WP-D1): new rows' default date
    today: CalendarDate,
}

impl BulkEntryScreen {
    /// A new, empty batch.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, today: CalendarDate) -> BulkEntryScreen {
        BulkEntryScreen { link, today }
    }
}

impl Screen for BulkEntryScreen {
    fn title(&self) -> String {
        t("cmd.banking.bulk_entry")
    }

    fn ui(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        crate::forms::header::title_only(ui, &tokens, &t("cmd.banking.bulk_entry"));
        ui.add_space(6.0);
        form::hint(ui, &tokens, &t("batch_entry.intro"));
    }

    fn scope(&self) -> Scope {
        Scope::Form
    }

    fn command(&mut self, id: CommandId) -> bool {
        id.as_str() == KIND
    }

    fn state(&self) -> Value {
        json!({
            "screen": "batch_entry",
            "sections": 0,
            "can_save": false,
            "saved_count": 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_its_own_kind_with_a_plain_state() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
        let cmd = cl_model::commands::find(KIND).expect("command").id;
        let mut s = make(cmd, &link, None, today).expect("screen");
        assert_eq!(s.title(), "Bulk Entry");
        assert_eq!(s.scope(), Scope::Form);
        assert!(s.command(cmd));
        assert_eq!(s.state()["screen"], "batch_entry");
        assert_eq!(s.state()["can_save"], false);
        let other = cl_model::commands::find("vendors.write_check")
            .expect("write check")
            .id;
        assert!(make(other, &link, None, today).is_none());
        assert!(crate::i18n::has("batch_entry.intro"));
    }
}
