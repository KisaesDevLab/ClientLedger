//! Reading a bill from its document with AI (P17, plan §3 WP7, Q-73,
//! Q-P17j, Q-P17o; non-negotiable 4).
//!
//! # Handoff (WP0 skeleton, WP7 fills this file and `doc_read_tests.rs`)
//!
//! Fixed by WP0, called by WP4 (WP7 may add, not change):
//! - [`ReadState`] (`Default` = nothing read): the extract job and what it
//!   filled.
//! - [`read_button`]`(ui, &mut ReadState, &ReadInput, &Tokens) -> bool` on
//!   the source-document card: drawn only when AI is on, bill reading is
//!   switched on and a provider is set (P09 preferences), after the token
//!   cap check. It starts the job itself; true when clicked.
//!   `bill.read_document` runs the same through [`ReadState::start`].
//! - [`banner`]`(ui, &mut ReadState, &ReadInput, &Tokens) -> BannerEvent`
//!   above the header: "Attached from <file> in your receipt inbox."
//!   without a reading; after one, "Filled from <file>… check them before
//!   saving" with **Accept these values** (Alt+A, `bill.accept_read`) and
//!   **Clear and type it myself**, the reconcile message, the found text of
//!   `ocr_only` and "Add as a new vendor?".
//! - [`ReadState::poll`]`(&mut self, &mut BillForm) -> bool` every frame:
//!   fills the form through `cl_model::forms::doc_read::apply` (read, never
//!   accepted). [`ReadState::accept`] and [`ReadState::clear`] act on the
//!   form for the banner's buttons.
//!
//! Nothing runs on upload or without the owner's click. Nothing is logged
//! except the job id and the outcome class. Strings go in`
//! `i18n/bill_read.rs` under `bill.read.*`.

use std::sync::Arc;

use cl_api::AttachmentId;
use cl_model::forms::bill::BillForm;
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::theme::Tokens;

/// What reading needs from the form, each frame.
pub struct ReadInput<'a> {
    /// The company (AI preferences, the request queue).
    pub link: &'a Arc<CompanyLink>,
    /// The source document's file name (owner data, shown, never logged).
    pub file_name: Option<&'a str>,
    /// The uploaded file to read (a draft or a saved bill's), if uploaded.
    pub attachment: Option<AttachmentId>,
    /// The file came from the receipt inbox (the banner says so).
    pub from_inbox: bool,
}

/// What the banner asks the form to do this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BannerEvent {
    /// Nothing.
    #[default]
    None,
    /// The owner accepted the read values (the form saves normally now).
    Accepted,
    /// The owner cleared the read values to type them.
    Cleared,
    /// "Add as a new vendor?": open the vendor form with the read name.
    AddVendor,
}

/// The reading's state. Skeleton: WP7 adds the job and the read fields.
#[derive(Debug, Default)]
pub struct ReadState {
    started: bool,
}

impl ReadState {
    /// Start reading `input.attachment` (Read with AI).
    pub fn start(&mut self, input: &ReadInput<'_>) {
        // Skeleton (WP7 replaces): nothing is sent yet.
        let _ = input;
    }

    /// Take a finished reading into `form`. True when it filled the form
    /// this frame.
    pub fn poll(&mut self, form: &mut BillForm) -> bool {
        let _ = form;
        false
    }

    /// Accept every read value in `form`.
    pub fn accept(&mut self, form: &mut BillForm) {
        let _ = form;
    }

    /// Clear the read values from `form`.
    pub fn clear(&mut self, form: &mut BillForm) {
        let _ = form;
        self.started = false;
    }

    /// True while the extract job runs.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.started
    }

    /// The control channel's view (no amounts, names or file names).
    #[must_use]
    pub fn state_json(&self) -> Value {
        json!({ "busy": self.started })
    }
}

/// The Read with AI button on the source-document card. True when clicked.
pub fn read_button(
    ui: &mut Ui,
    state: &mut ReadState,
    input: &ReadInput<'_>,
    tokens: &Tokens,
) -> bool {
    // Skeleton (WP7 replaces): no button until WP7.
    let _ = (ui, state, input, tokens);
    false
}

/// The source-document banner above the header.
pub fn banner(
    ui: &mut Ui,
    state: &mut ReadState,
    input: &ReadInput<'_>,
    tokens: &Tokens,
) -> BannerEvent {
    // Skeleton (WP7 replaces): draws nothing yet.
    let _ = (ui, state, input, tokens);
    BannerEvent::None
}
