//! The receipt inbox (P17, `nav.receipt_inbox`, plan §3 WP5, Q-P17p): the
//! files in the owner's inbox folder, a preview, and the actions that turn
//! one into a bill, a check or a bank transaction's attachment.
//!
//! # Handoff (WP0 skeleton, WP5 fills this file)
//!
//! WP5 replaces [`ReceiptInboxScreen`]'s body and keeps [`KIND`], [`make`]
//! and [`InboxDraft`]. The folder scan and filing away are
//! `cl_company::inbox`; inbox state is `cl_settings::inbox_state`; the
//! preview is `widgets::pdf_viewer` (WP1). "New bill" / "New check" upload
//! the file as a draft first, then push
//! `CompanyEvent::OpenWith(vendors.enter_bill | vendors.write_check,
//! OpenPayload::InboxDraft(..))`; the original moves to `Filed/` once the
//! engine confirmed the upload. Write Check takes the draft in
//! `write_check/attach.rs` (WP5), Enter Bill in `bill/attach.rs` (WP4).
//! Strings go in `i18n/inbox.rs` under `inbox.*`.

use std::sync::Arc;

use cl_api::{AttachmentId, DraftKey};
use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::CommandId;
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::i18n::t;
use crate::widgets::pdf_viewer::DocSource;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "nav.receipt_inbox";

/// The receipt inbox window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(ReceiptInboxScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// An inbox file already uploaded as a draft, handed to a new form.
#[derive(Clone, PartialEq, Eq)]
pub struct InboxDraft {
    /// The draft the file was uploaded under (sent on the create as
    /// `draftAttachmentId`).
    pub draft: DraftKey,
    /// The uploaded file.
    pub attachment: AttachmentId,
    /// Its name and bytes, for the preview (the name is owner data).
    pub file: DocSource,
}

/// Ids only.
impl std::fmt::Debug for InboxDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboxDraft")
            .field("draft", &self.draft)
            .field("attachment", &self.attachment)
            .finish_non_exhaustive()
    }
}

/// The receipt inbox window. Skeleton: WP5 builds it.
pub struct ReceiptInboxScreen {
    today: CalendarDate,
}

impl ReceiptInboxScreen {
    /// The inbox of the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        open: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> ReceiptInboxScreen {
        let _ = (link, open);
        ReceiptInboxScreen { today }
    }
}

impl Screen for ReceiptInboxScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        // Skeleton (WP5 replaces).
        let _ = ui;
    }

    fn state(&self) -> Value {
        json!({ "screen": "receipt_inbox", "date": self.today.to_string() })
    }
}
