//! Enter Bill (P17, `vendors.enter_bill`, Ctrl+B): one form for a bill and
//! a vendor credit, beside the vendor panel and the source-document card
//! (mockup `docs/design/mockups/EnterBill.html`, plan §1).
//!
//! # Handoff (WP0 skeleton)
//!
//! Files and owners (plan §3 "Conflict rules"):
//! - `mod.rs`, `form.rs`, `items.rs`, `save.rs`, `attach.rs`, `tests.rs`:
//!   WP4 (the screen, the header card, the Items grid, saving with the
//!   PR-1 unknown-save rule, attachments, the demo). WP4 replaces
//!   [`EnterBillScreen`]'s body and keeps [`KIND`], [`make`] and
//!   [`BillPrefill`].
//! - `vendor_panel.rs`: WP8. WP4 calls [`vendor_panel::VendorPanel::tick`]
//!   every frame and [`vendor_panel::show`] where the panel is drawn.
//! - `doc_read.rs` (+ `doc_read_tests.rs`): WP7. WP4 calls
//!   [`doc_read::read_button`] on the source-document card,
//!   [`doc_read::banner`] above the header, and
//!   [`doc_read::ReadState::poll`] every frame.
//! - `statement_match.rs`: WP6 (its own tab, `nav.statement_match`).
//!
//! Opening a prefilled bill: another screen pushes
//! `CompanyEvent::OpenWith(vendors.enter_bill, OpenPayload::BillPrefill(..))`
//! (statement match) or `OpenPayload::InboxDraft(..)` (receipt inbox); WP4
//! implements `Screen::open_with` for both. After every save, void or
//! delete WP4 publishes `data_bus::Change::Bills(vendor)` (and the ledger
//! change). Strings go in `i18n/bill.rs`.

mod attach;
pub mod doc_read;
mod form;
mod items;
mod save;
pub mod statement_match;
pub mod vendor_panel;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use cl_core::ids::{ContactId, TenantId};
use cl_core::{CalendarDate, Money};
use cl_model::forms::bill::BillKind;
use cl_model::{CommandId, Scope};
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::i18n::t;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "vendors.enter_bill";

/// The Enter Bill window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(EnterBillScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// A fresh bill or credit filled from a vendor statement line ("Create
/// bill" / "Create credit", WP6). The amount is the one the owner typed or
/// pasted from the statement; the form stays an ordinary unsaved form.
#[derive(Clone, PartialEq, Eq)]
pub struct BillPrefill {
    /// Bill or vendor credit.
    pub kind: BillKind,
    /// The vendor.
    pub vendor: ContactId,
    /// The statement line's date.
    pub date: CalendarDate,
    /// The statement line's reference (owner data).
    pub ref_no: String,
    /// The statement line's amount.
    pub amount: Money,
}

/// No refs or amounts.
impl std::fmt::Debug for BillPrefill {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BillPrefill")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// The Enter Bill window. Skeleton: WP4 builds the form.
pub struct EnterBillScreen {
    today: CalendarDate,
}

impl EnterBillScreen {
    /// A blank bill dated `today`.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        open: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> EnterBillScreen {
        let _ = (link, open);
        EnterBillScreen { today }
    }
}

impl Screen for EnterBillScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        // Skeleton (WP4 replaces): the form arrives with WP4.
        let _ = ui;
    }

    fn scope(&self) -> Scope {
        Scope::Form
    }

    fn state(&self) -> Value {
        json!({
            "screen": "enter_bill",
            "kind": "bill",
            "date": self.today.to_string(),
        })
    }
}
