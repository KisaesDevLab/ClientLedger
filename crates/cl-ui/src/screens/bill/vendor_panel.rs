//! The vendor panel beside the Enter Bill form and the duplicate-ref
//! warning (P17, plan §3 WP8, D3, Q-P17m, Q-P17n).
//!
//! # Handoff (WP0 skeleton, WP8 fills this file and its tests)
//!
//! Fixed by WP0, called by WP4 (WP8 may add, not change):
//! - [`VendorPanel`] (`Default` = no vendor): the panel's own state and
//!   requests.
//! - [`VendorPanel::tick`]`(&mut self, &PanelInput, now)`: every frame,
//!   drawn or not (the panel may be collapsed with F9). It starts the reads
//!   for a new vendor, polls them, and runs the duplicate check debounced
//!   400 ms after the vendor, ref or date changed.
//! - [`show`]`(ui, &mut VendorPanel, &PanelInput, &Tokens) -> PanelEvent`:
//!   draws the rows in mockup order (Open balance = `vendor-summary
//!   .totalOwed` as sent, Default account, 1099 vendor), the duplicate line,
//!   then open bills, unused credits and recent bills.
//! - [`VendorPanel::bills_changed`]: WP4 passes the data bus's
//!   [`Changes`]; the panel reads the summary again when its vendor's bills
//!   changed. It never sums.
//! - [`VendorPanel::duplicate`]: the bill that already uses this ref (save
//!   asks "Save anyway / Go back", never blocks);
//!   [`VendorPanel::duplicate_unchecked`]: the lookup could not finish
//!   ("Couldn't check every bill").
//! - [`PanelEvent::OpenRecord`]: the owner clicked a bill or credit; WP4
//!   opens it.
//!
//! Strings go in `i18n/bill_panel.rs` under `bill.panel.*`. No amounts, names or
//! refs in logs.

use std::sync::Arc;
use std::time::Instant;

use cl_core::CalendarDate;
use cl_core::ids::{ContactId, TransactionId};
use cl_model::forms::bill::BillKind;
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::data_bus::Changes;
use crate::theme::Tokens;

/// What the panel needs from the form, each frame.
pub struct PanelInput<'a> {
    /// The company.
    pub link: &'a Arc<CompanyLink>,
    /// Bill or vendor credit (the duplicate check looks at the same kind).
    pub kind: BillKind,
    /// The vendor picked, if any.
    pub vendor: Option<ContactId>,
    /// The Ref. no. as typed.
    pub ref_no: &'a str,
    /// The bill date, when it parses.
    pub date: Option<CalendarDate>,
    /// The saved bill or credit being edited (never its own duplicate).
    pub editing: Option<TransactionId>,
}

/// A posted bill or credit of the same vendor that already uses the ref.
#[derive(Clone, PartialEq, Eq)]
pub struct DuplicateRef {
    /// The other record.
    pub id: TransactionId,
    /// Its ref as stored (owner data).
    pub ref_no: String,
    /// Its date.
    pub date: CalendarDate,
}

/// Ids only.
impl std::fmt::Debug for DuplicateRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DuplicateRef")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// What the panel asks the form to do this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PanelEvent {
    /// Nothing.
    #[default]
    None,
    /// Open this bill or credit (a recent bill, an open bill, the
    /// duplicate's link).
    OpenRecord(BillKind, TransactionId),
}

/// The panel's state. Skeleton: WP8 adds the cards and requests.
#[derive(Debug, Default)]
pub struct VendorPanel {
    vendor: Option<ContactId>,
    duplicate: Option<DuplicateRef>,
}

impl VendorPanel {
    /// Start, poll and debounce the reads for `input` (every frame).
    pub fn tick(&mut self, input: &PanelInput<'_>, now: Instant) {
        let _ = now;
        if self.vendor != input.vendor {
            self.vendor = input.vendor;
            self.duplicate = None;
        }
    }

    /// Read again what `changes` touched (the data bus, Q-262).
    pub fn bills_changed(&mut self, changes: &Changes) {
        let _ = changes;
    }

    /// The other bill that uses this ref, once the check found one.
    #[must_use]
    pub fn duplicate(&self) -> Option<&DuplicateRef> {
        self.duplicate.as_ref()
    }

    /// True when the duplicate check could not look at every bill (D3).
    #[must_use]
    pub fn duplicate_unchecked(&self) -> bool {
        false
    }

    /// True while a read or the duplicate check is running.
    #[must_use]
    pub fn busy(&self) -> bool {
        false
    }

    /// The control channel's view (no amounts, names or refs).
    #[must_use]
    pub fn state_json(&self) -> Value {
        json!({
            "vendor": self.vendor.is_some(),
            "duplicate": self.duplicate.is_some(),
        })
    }
}

/// Draw the panel.
pub fn show(
    ui: &mut Ui,
    panel: &mut VendorPanel,
    input: &PanelInput<'_>,
    tokens: &Tokens,
) -> PanelEvent {
    // Skeleton (WP8 replaces): draws nothing yet.
    let _ = (ui, panel, input, tokens);
    PanelEvent::None
}
