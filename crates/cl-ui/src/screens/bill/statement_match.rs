//! Match Vendor Statement (P17, `nav.statement_match`, plan §3 WP6, D4,
//! Q-P17q): a vendor statement typed or pasted against the books.
//!
//! # Handoff (WP0 skeleton, WP6 fills this file)
//!
//! WP6 replaces [`StatementMatchScreen`]'s body and keeps [`KIND`] and
//! [`make`]. It opens from Vendors › Match Vendor Statement… and from the
//! Enter Bill toolbar overflow (WP4 pushes `CompanyEvent::OpenWindow` of
//! [`KIND`]). "Create bill" / "Create credit" push
//! `CompanyEvent::OpenWith(vendors.enter_bill,
//! OpenPayload::BillPrefill(..))`. Books items come from WP2's `bills`,
//! `vendor_credits` and `bill_payments` lists (posted only); matching is
//! `cl_model::statement_match` (WP3). No running balance. Strings go in
//! `i18n/statement.rs` under `statement.*`.

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::CommandId;
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::i18n::t;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "nav.statement_match";

/// The statement match window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(StatementMatchScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// The statement match window. Skeleton: WP6 builds it.
pub struct StatementMatchScreen {
    today: CalendarDate,
}

impl StatementMatchScreen {
    /// An empty statement for `today`'s period.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        open: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> StatementMatchScreen {
        let _ = (link, open);
        StatementMatchScreen { today }
    }
}

impl Screen for StatementMatchScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        // Skeleton (WP6 replaces).
        let _ = ui;
    }

    fn state(&self) -> Value {
        json!({ "screen": "statement_match", "date": self.today.to_string() })
    }
}
