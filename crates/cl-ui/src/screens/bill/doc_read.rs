//! Reading a bill from its document with AI (P17, plan §3 WP7, Q-73,
//! Q-P17j, Q-P17o; non-negotiable 4), and the ghost account suggestion on
//! bill lines (Q-P13l, Q-P10w; WP4's scope cut).
//!
//! # Read with AI
//!
//! - **Only on the owner's click** ([`read_button`] on the source-document
//!   card, or `bill.read_document`): the engine never reads on upload (U-89,
//!   engine #174). The button is drawn only when a document is shown and AI
//!   is on, reading bills is switched on and a provider is set (P09
//!   preferences, [`ReadSwitches`]). For a cloud provider the monthly token
//!   cap is checked first, as P13 does; at the cap nothing is sent.
//! - The answer fills the **empty** fields through
//!   `cl_model::forms::doc_read::apply`: every one is *read, not accepted*.
//!   Each is tinted with the caption "Read from the document — check it";
//!   a low-confidence one is marked more strongly. Confidence is what the
//!   model reported: it never skips the review (engine #174 doc).
//! - The banner offers **Accept these values** (Alt+A) and **Clear and type
//!   it myself**. Editing a read field accepts that field (a read line: its
//!   amount or description). Save with an unaccepted field is refused
//!   (`ReadNotAccepted`) and the keyboard goes to Accept.
//! - When the document's own figures disagree (exact `Money`, no
//!   tolerance), the banner says so; the lines-total rule still decides
//!   Save.
//! - A vendor name that matches no vendor shows "Add as a new vendor?", an
//!   explicit action. `ocr_only` shows the text found, read-only, and fills
//!   nothing.
//! - Failures are plain messages; manual entry is never affected.
//! - Logs carry the job number and the outcome class only: never the
//!   document's text, names, amounts or the file name.
//!
//! # Ghost accounts on lines
//!
//! A line with a description or an amount but no account asks, once the
//! vendor is set: the engine's usual account for the vendor (its default or
//! the account last used, `suggest-account`), then the AI preview when every
//! AI switch is on (P13's [`crate::forms::suggest::gate_of`] with the cap).
//! Lines filled from a document get the reading's `defaultExpenseAccountId`
//! (else the vendor's default, Q-192) as their ghost at once. Rules are not
//! asked: they are bank-feed rules and a bill has no bank account. A ghost
//! is an account only (non-negotiable 4) and is set only by Tab on the
//! empty, unchanged cell or the caption's Accept (Q-P10w).
//!
//! Strings are in `i18n/bill_read.rs` under `bill.read.*`.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;

use cl_api::{
    AI_ALL_PROVIDERS_FAILED, AI_OCR_FAILED, AI_PARSE_FAILED, AiPreviewRow, ApiError, AttachmentId,
    BillReadGate, BillReading, OCR_UNREADABLE_AT_PII_LEVEL, PDF_RENDER_EMPTY, PDF_RENDER_FAILED,
    ReadStatus, RequestGroup, SuggestSource,
};
use cl_core::ids::{AccountId, ContactId, TenantId};
use cl_core::{CalendarDate, Money, MoneyStyle};
use cl_grid::RowKey;
use cl_model::forms::bill::BillForm;
use cl_model::forms::doc_read::{
    self as model, Confidence, DocReading, FieldConfidence, ReadCheck, ReadField, ReadTarget,
};
use cl_settings::AiProvider;
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke, Ui};
use serde_json::{Value, json};

use super::{EnterBillScreen, field};
use crate::company::{CompanyLink, Task};
use crate::forms::suggest::{Asking, Basis, Suggested, TIMEOUT};
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;

#[cfg(test)]
#[path = "doc_read_tests.rs"]
mod tests;

/// The header-undo key for marking read lines as edited (the lines' own
/// edits live in the grid's history).
const LINE_MARKS: u64 = 100;

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
    /// The open company (the monthly AI cap is kept per company).
    pub tenant: Option<TenantId>,
    /// The owner's money display style (the banner's figures).
    pub money_style: MoneyStyle,
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
    /// "Ask AI to help" after a reading made on this computer (A03).
    AskAi,
}

/// The owner's switches for reading documents (P09 preferences), and the
/// engine's document reading assist with its consent switches (A03 step
/// 21).
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadSwitches {
    /// AI is on (and, with an engine, the company agreed to the current
    /// AI notice).
    pub ai: bool,
    /// "Read bills and receipts" is on (`receipt_ocr`).
    pub reading: bool,
    /// The provider reading uses; `None` when none is set (or unknown).
    pub provider: Option<AiProvider>,
    /// The engine offers the document reading assist
    /// (`document-assist-v1`): Enter Bill has one Read document entry with
    /// Ask AI to help, and P17's Read with AI is not offered (plan D7).
    pub assist: bool,
    /// The company's consent to send document text online.
    pub text_cloud: bool,
    /// The company's consent to send document pictures online.
    pub images_cloud: bool,
    /// The engine sends pictures to an online provider at all.
    pub cloud_vision: bool,
    /// The provider is Ollama but its model runs online (`…:cloud`,
    /// `…-cloud`): treated as an online provider (review S5).
    pub ollama_cloud: bool,
}

impl ReadSwitches {
    /// The gate cl-api checks before sending.
    #[must_use]
    pub fn gate(self) -> BillReadGate {
        BillReadGate {
            ai_enabled: self.ai,
            bill_reading_on: self.reading,
            provider_set: self.provider.is_some(),
        }
    }

    /// Every switch is on.
    #[must_use]
    pub fn open(self) -> bool {
        self.gate().is_open()
    }

    /// P17's Read with AI is offered: every switch is on and the engine
    /// has no document reading assist (with it, Read document and Ask AI
    /// to help replace it, plan D7).
    #[must_use]
    pub fn read_with_ai(self) -> bool {
        self.open() && !self.assist
    }

    /// The switches the optional readers decide on.
    #[must_use]
    pub fn assist_switches(self) -> crate::reading::assist::AssistSwitches {
        crate::reading::assist::AssistSwitches {
            capability: self.assist,
            ai: self.ai,
            reading: self.reading,
            provider: self.provider,
            text_cloud: self.text_cloud,
            images_cloud: self.images_cloud,
            cloud_vision: self.cloud_vision,
            ollama_cloud: self.ollama_cloud,
        }
    }

    /// From the engine's AI configuration, the company's consent and the
    /// engine's capabilities.
    #[must_use]
    pub fn from_engine_with(
        config: Option<&cl_api::AiConfig>,
        consent: Option<&cl_api::CompanyConsent>,
        caps: &cl_api::Capabilities,
    ) -> ReadSwitches {
        let tasks = consent.map(|c| c.enabled_tasks).unwrap_or_default();
        ReadSwitches {
            assist: caps.has(cl_api::Capabilities::DOCUMENT_ASSIST),
            text_cloud: tasks.document_text_cloud_on(),
            images_cloud: tasks.document_images_cloud_on(),
            cloud_vision: config.is_some_and(cl_api::AiConfig::cloud_vision_allowed),
            ..ReadSwitches::from_engine(config, consent)
        }
    }

    /// From the engine's AI configuration and the company's consent (no
    /// document reading assist).
    #[must_use]
    pub fn from_engine(
        config: Option<&cl_api::AiConfig>,
        consent: Option<&cl_api::CompanyConsent>,
    ) -> ReadSwitches {
        let current = consent.is_some_and(|c| {
            c.ai_enabled
                && c.accepted_at.is_some()
                && !c.is_stale
                && c.accepted_version.is_some()
                && c.accepted_version == c.system_version
        });
        let provider = config.and_then(|c| {
            c.ocr_provider
                .as_deref()
                .filter(|p| !p.is_empty())
                .or_else(|| c.fallback_chain.first().map(String::as_str))
                .and_then(AiProvider::from_wire)
        });
        // Either model named online is enough (the reading may use either).
        let ollama_cloud = provider == Some(AiProvider::Ollama)
            && config.is_some_and(|c| {
                [&c.ocr_model, &c.categorization_model]
                    .into_iter()
                    .flatten()
                    .any(|m| cl_settings::is_ollama_cloud_model(m))
            });
        ReadSwitches {
            ai: config.is_some_and(|c| c.is_enabled) && current,
            reading: consent.is_some_and(|c| c.enabled_tasks.receipt_ocr == Some(true)),
            provider,
            ollama_cloud,
            ..ReadSwitches::default()
        }
    }

    /// The demo's switches (a local model; consent given in the demo).
    #[must_use]
    pub fn demo(link: &CompanyLink) -> Option<ReadSwitches> {
        let ai = link.demo_state()?.ai;
        Some(ReadSwitches {
            ai: ai.enabled,
            reading: ai.reading,
            provider: Some(AiProvider::Ollama),
            assist: ai.assist,
            text_cloud: ai.text_cloud,
            images_cloud: ai.text_cloud && ai.images_cloud,
            cloud_vision: false,
            ollama_cloud: false,
        })
    }
}

/// What a finished reading filled, for the banner (owner data: shown,
/// never logged; `Debug` is redacted).
#[derive(Clone, Default)]
pub(crate) struct Filled {
    /// The file it was read from.
    pub file_name: String,
    /// The document's own figures, checked exactly.
    pub check: Option<ReadCheck>,
    /// The document's total (for the reconcile message).
    pub total: Option<Money>,
    /// The vendor's name as printed when it matched no vendor.
    pub unknown_vendor: Option<String>,
    /// Some figures were printed but couldn't be read exactly.
    pub unreadable: bool,
    /// Nothing was empty to fill.
    pub nothing_new: bool,
    /// The values as read, to notice the owner's edits.
    pub snapshot: Snapshot,
    /// The new bill's own date the document's date replaced ("Clear" puts
    /// it back).
    pub prior_date: Option<CalendarDate>,
    /// The terms came with the vendor the reading filled.
    pub terms_from_vendor: bool,
    /// Read on this computer (A03 Read document), not by AI.
    pub local: bool,
    /// The local reading's checks, listed on the banner (never applied).
    pub checks: Vec<cl_docread::model::Check>,
    /// A balance due printed apart from a different total: both shown.
    pub balance_due: Option<Money>,
}

impl std::fmt::Debug for Filled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Filled")
            .field("unknown_vendor", &self.unknown_vendor.is_some())
            .field("unreadable", &self.unreadable)
            .field("local", &self.local)
            .field("checks", &self.checks.len())
            .finish_non_exhaustive()
    }
}

/// A reading on its way into the form, from either reader: P17's Read with
/// AI or A03's Read document. Owner data: never logged.
pub(crate) struct Incoming {
    /// The values, each filled as *read, not accepted*.
    pub doc: DocReading,
    /// The vendor's name as printed when it matched no vendor.
    pub unknown_vendor: Option<String>,
    /// Some figures were printed but couldn't be read exactly.
    pub unreadable: bool,
    /// The account the reader suggests for the lines (else the vendor's
    /// default).
    pub default_account: Option<AccountId>,
    /// The local reading's checks (shown, never applied).
    pub checks: Vec<cl_docread::model::Check>,
    /// Read on this computer.
    pub local: bool,
    /// A balance due printed apart from a different total.
    pub balance_due: Option<Money>,
}

/// The values a reading put in the form.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub vendor: Option<ContactId>,
    pub date: Option<CalendarDate>,
    pub due: Option<CalendarDate>,
    pub reference: String,
    pub amount_due: Option<Money>,
    /// Each read line's amount (only an amount change accepts a read line;
    /// non-negotiable 4).
    pub lines: BTreeMap<RowKey, Option<Money>>,
}

/// Ghost account suggestions on the expense lines.
#[derive(Default)]
pub(crate) struct GhostState {
    /// What each line was last asked about (vendor, memo and amount).
    pub asked: BTreeMap<RowKey, u64>,
    pub asking: Vec<Asking>,
    /// The suggestions shown, by line.
    pub shown: BTreeMap<RowKey, Suggested>,
    /// Suggestions accepted on this bill.
    pub accepted: u32,
    /// A plain hint when a request failed or took too long (i18n key).
    pub hint: Option<&'static str>,
    /// Ghosts still open (cached each frame for the control channel).
    pub open: usize,
}

/// The reading's state.
#[derive(Default)]
pub struct ReadState {
    /// The switches as last known (the demo reads them every frame).
    switches: Option<ReadSwitches>,
    switches_task: Option<Task<ReadSwitches>>,
    /// The queue for the reading (made on first use).
    group: Option<RequestGroup>,
    /// The extract job.
    job: Option<Task<Result<BillReading, ApiError>>>,
    /// The job's number (the only thing logged besides the outcome).
    pub(crate) job_no: u64,
    /// Requests sent (control channel and tests: none with AI off).
    sent: u32,
    /// A finished reading waiting to be put into the form.
    done: Option<BillReading>,
    /// The file name the job is for.
    pub(crate) job_file: String,
    /// Read document, on this computer (A03).
    pub(crate) local: super::doc_read_local::LocalRead,
    /// What the last reading filled.
    pub(crate) filled: Option<Filled>,
    /// `ocr_only`: the text found (shown read-only).
    pub(crate) found_text: Option<String>,
    /// Show the found text.
    pub(crate) show_text: bool,
    /// Why the last attempt failed (i18n key).
    pub(crate) problem: Option<&'static str>,
    /// The outcome class of the last job (control channel).
    outcome: Option<&'static str>,
    /// Read fields not accepted yet (cached each frame).
    pub(crate) unaccepted: usize,
    /// Of those, the ones read with low confidence.
    pub(crate) low: usize,
    /// Put the keyboard on Accept (a save was refused for it).
    pub(crate) focus_accept: bool,
    /// The refusal that moved the keyboard (so it moves once per save).
    refusal_seen: bool,
    /// The grid's suggestions were cleared for this state.
    initialized: bool,
    /// Ghost accounts on the lines.
    pub(crate) ghosts: GhostState,
}

impl std::fmt::Debug for ReadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadState")
            .field("busy", &self.busy())
            .field("outcome", &self.outcome)
            .field("unaccepted", &self.unaccepted)
            .finish_non_exhaustive()
    }
}

/// Sends the extract on the write lane: it spends AI tokens, so a reply
/// lost when the window closes is labelled ("reading a bill with AI") like
/// any other unconfirmed write instead of being cancelled unseen.
///
/// # Errors
/// The queue refused it (full or closed).
pub(crate) fn send_extract(
    group: &RequestGroup,
    session: &cl_api::Session,
    gate: BillReadGate,
    attachment: AttachmentId,
) -> Result<cl_api::Pending<Result<BillReading, ApiError>>, cl_api::QueueError> {
    let key = cl_api::IdempotencyKey::new();
    let label = cl_api::OrphanLabel {
        what: "orphan.bill_read",
        key: Some(key.clone()),
    };
    // With the engine's document reading assist, P17's extract path (whose
    // cloud call rides on `receipt_ocr`) is never used: documents go to an
    // AI only through the assist and its own consent (#179 review).
    let gate = if session
        .client()
        .capabilities()
        .has(cl_api::Capabilities::DOCUMENT_ASSIST)
    {
        BillReadGate::default()
    } else {
        gate
    };
    group.write(session, label, move |s| async move {
        Ok(s.bill_reading().extract(gate, attachment, key).await)
    })
}

/// Reads the owner's and the engine's switches for reading documents (the
/// AI configuration, the company's consent and the engine's capabilities).
///
/// # Errors
/// The queue refused it (full or closed).
pub(crate) fn fetch_switches(
    group: &RequestGroup,
    session: &cl_api::Session,
) -> Result<cl_api::Pending<ReadSwitches>, cl_api::QueueError> {
    group.read(session, |s| async move {
        let config = s.ai_config().await.ok();
        let consent = s.company_ai_disclosure().await.ok();
        let caps = s.client().capabilities();
        Ok(ReadSwitches::from_engine_with(
            config.as_ref(),
            consent.as_ref(),
            &caps,
        ))
    })
}

/// Sends Ask AI to help on the write lane, as the extract: it spends AI
/// tokens, so a reply lost when the window closes is labelled like any
/// other unconfirmed write.
///
/// # Errors
/// The queue refused it (full or closed).
pub(crate) fn send_assist(
    group: &RequestGroup,
    session: &cl_api::Session,
    request: cl_api::AssistRequest,
) -> Result<cl_api::Pending<Result<cl_api::AssistSuggestion, ApiError>>, cl_api::QueueError> {
    let key = cl_api::IdempotencyKey::new();
    let label = cl_api::OrphanLabel {
        what: "orphan.bill_read",
        key: Some(key.clone()),
    };
    group.write(session, label, move |s| async move {
        Ok(s.document_assist(&request, key).await)
    })
}

/// The plain message for a failed reading (i18n key).
#[must_use]
pub(crate) fn problem_of(e: &ApiError) -> &'static str {
    match e {
        ApiError::AiUnavailable => "bill.read.problem.off",
        ApiError::OfflineUnavailable { .. }
        | ApiError::EngineUnavailable { .. }
        | ApiError::Timeout
        | ApiError::OutcomeUnknown { .. } => "bill.read.problem.unreachable",
        ApiError::NotFound { .. } => "bill.read.problem.gone",
        ApiError::RateLimited { .. } => "bill.read.problem.busy",
        e if cl_api::has_code(e, AI_ALL_PROVIDERS_FAILED) => "bill.read.problem.unreachable",
        e if cl_api::has_code(e, OCR_UNREADABLE_AT_PII_LEVEL) => "bill.read.problem.private",
        // A scanned PDF needs page pictures, which ClientLedger can't make
        // yet (no GPL poppler): say so plainly, never the engine's code.
        e if cl_api::has_code(e, PDF_RENDER_FAILED) || cl_api::has_code(e, PDF_RENDER_EMPTY) => {
            "bill.read.problem.scanned_pdf"
        }
        e if cl_api::has_code(e, AI_OCR_FAILED) || cl_api::has_code(e, AI_PARSE_FAILED) => {
            "bill.read.problem.unreadable"
        }
        _ => "bill.read.problem.failed",
    }
}

/// The class logged for an outcome (no content).
fn outcome_class(key: &'static str) -> &'static str {
    key.strip_prefix("bill.read.problem.").unwrap_or(key)
}

fn level_of(c: Option<cl_api::Confidence>) -> Option<Confidence> {
    c.map(|c| match c {
        cl_api::Confidence::High => Confidence::High,
        cl_api::Confidence::Medium => Confidence::Medium,
        cl_api::Confidence::Low => Confidence::Low,
    })
}

/// The engine's reading as the form model takes it: the document's own
/// figures, exactly; an unreadable line amount stays blank and is marked
/// low. Line accounts are left empty (they become ghosts).
#[must_use]
pub(crate) fn doc_reading(r: &BillReading) -> DocReading {
    let field = |name: &str| level_of(r.field_confidence.get(name).copied());
    DocReading {
        vendor: r.vendor,
        bill_date: r.bill_date,
        due_date: r.due_date,
        reference: r.vendor_invoice_number.clone(),
        subtotal: r.subtotal,
        tax: r.tax,
        total: r.total,
        lines: r
            .lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let unreadable = r.unreadable.contains(&cl_api::ReadField::LineAmount(i));
                model::ReadLine {
                    description: l.description.clone().unwrap_or_default(),
                    amount: l.amount,
                    account: None,
                    confidence: if unreadable {
                        Some(Confidence::Low)
                    } else {
                        level_of(l.confidence)
                    },
                }
            })
            .collect(),
        // The engine's AI reading of a bill has neither (A04 adds them for
        // the card and check forms' local readings).
        paid_from: None,
        check_number: None,
        confidence: FieldConfidence {
            vendor: field("vendor"),
            bill_date: field("billDate"),
            due_date: field("dueDate"),
            reference: field("vendorInvoiceNumber"),
            total: field("total"),
            // The engine's reading has no confidence for these (A03 N6).
            terms: None,
            amount_paid: None,
            payment: None,
            check_number: None,
        },
    }
}

/// The demo engine's canned reading (`--demo` and tests; no provider is
/// ever called): the story invoice reads as the mockup's bill, any other
/// file as text only.
///
/// # Errors
/// `AiUnavailable` with AI or reading off; `NotFound` for an unknown file.
pub(crate) fn demo_reading(
    link: &CompanyLink,
    attachment: AttachmentId,
) -> Result<BillReading, ApiError> {
    use crate::company::demo::bills::{STORY_PDF, STORY_VENDOR};
    let Some(mut demo) = link.demo_state() else {
        return Err(ApiError::NoCompanyOpen);
    };
    if !(demo.ai.enabled && demo.ai.reading) {
        return Err(ApiError::AiUnavailable);
    }
    let bytes = crate::company::demo::bills::download(&demo, attachment)?;
    let body = if bytes == STORY_PDF {
        let vendor = demo
            .contacts_mut()
            .iter()
            .find(|c| c.fields.display_name == STORY_VENDOR)
            .map(|c| (c.id.to_string(), c.fields.default_expense_account_id));
        json!({"extraction": {
            "status": "ok",
            "vendor": STORY_VENDOR,
            "contactId": vendor.as_ref().map(|v| v.0.clone()),
            "defaultExpenseAccountId": vendor.and_then(|v| v.1).map(|a| a.to_string()),
            "vendorInvoiceNumber": "INV-55871",
            "billDate": "2026-11-03",
            "dueDate": "2026-11-18",
            "paymentTerms": "net_15",
            "subtotal": "1480.00",
            "tax": "0.00",
            "total": "1480.00",
            "lineItems": [
                {"description": "Mini excavator, 2 days", "amount": "1330.00", "quantity": "2", "confidence": "high"},
                {"description": "Delivery and pickup", "amount": "150.00", "quantity": "1", "confidence": "low"},
            ],
            "confidence": "medium",
            "fieldConfidence": {
                "vendor": "high", "billDate": "high", "dueDate": "medium",
                "vendorInvoiceNumber": "high", "total": "high",
            },
        }})
    } else {
        json!({"extraction": {
            "status": "ocr_only",
            "rawText": "Text found on the page, but no bill fields.",
        }})
    };
    cl_api::reading_from_json(body.to_string().as_bytes())
}

impl ReadState {
    /// The switches: the demo's now, an engine's once read.
    fn switches(&mut self, link: &Arc<CompanyLink>) -> Option<ReadSwitches> {
        if link.is_demo() {
            self.switches = ReadSwitches::demo(link);
            return self.switches;
        }
        if let Some(r) = self.switches_task.as_mut().and_then(Task::try_take) {
            self.switches_task = None;
            // Unreadable (offline, signed out): no button.
            self.switches = Some(r.unwrap_or_default());
        }
        if self.switches.is_none() && self.switches_task.is_none() {
            self.read_switches(link);
        }
        self.switches
    }

    fn ensure_queue(&mut self, link: &CompanyLink) {
        if self.group.is_none() {
            self.group = link.group().and_then(Result::ok);
        }
    }

    /// The reading's request queue (made on first use).
    pub(crate) fn assist_queue(&mut self, link: &CompanyLink) -> Option<&RequestGroup> {
        self.ensure_queue(link);
        self.group.as_ref()
    }

    fn read_switches(&mut self, link: &Arc<CompanyLink>) {
        self.ensure_queue(link);
        let (Some(session), Some(group)) = (link.session(), self.group.as_ref()) else {
            self.switches = Some(ReadSwitches::default());
            return;
        };
        match fetch_switches(group, &session) {
            Ok(p) => self.switches_task = Some(Task::Pending(p)),
            Err(_) => self.switches = Some(ReadSwitches::default()),
        }
    }

    /// Starts reading the switches when they aren't known yet (A04 draft
    /// mode: Ask AI to help is offered once the words are ready).
    pub(crate) fn ensure_switches(&mut self, link: &Arc<CompanyLink>) {
        let _ = self.switches(link);
    }

    /// Read with AI is offered for this document (never with the engine's
    /// document reading assist, plan D7).
    pub fn available(&mut self, input: &ReadInput<'_>) -> bool {
        input.attachment.is_some()
            && self
                .switches(input.link)
                .is_some_and(ReadSwitches::read_with_ai)
    }

    /// The engine offers the document reading assist (as last known).
    #[must_use]
    pub(crate) fn assist_capable(&self) -> bool {
        self.switches.is_some_and(|s| s.assist)
    }
    /// Start reading `input.attachment` (Read with AI). Nothing is sent
    /// unless every switch is on and, for a cloud provider, the monthly cap
    /// allows it.
    pub fn start(&mut self, input: &ReadInput<'_>) {
        if self.busy() {
            return;
        }
        self.problem = None;
        let Some(attachment) = input.attachment else {
            self.problem = Some("bill.read.problem.no_document");
            return;
        };
        let Some(switches) = self.switches(input.link).filter(|s| s.read_with_ai()) else {
            self.problem = Some("bill.read.problem.off");
            return;
        };
        // The token cap first (P13): a cloud call needs room this month.
        if switches.provider.is_some_and(AiProvider::is_cloud)
            && !crate::forms::suggest::under_cap(input.link, input.tenant)
        {
            self.problem = Some("bill.read.problem.cap");
            self.outcome = Some("cap");
            return;
        }
        self.job_no += 1;
        self.job_file = input.file_name.unwrap_or_default().to_string();
        tracing::info!(job = self.job_no, "bill reading started");
        if input.link.is_demo() {
            self.sent += 1;
            self.job = Some(Task::ready(demo_reading(input.link, attachment)));
            return;
        }
        self.ensure_queue(input.link);
        let (Some(session), Some(group)) = (input.link.session(), self.group.as_ref()) else {
            self.problem = Some("bill.read.problem.unreachable");
            return;
        };
        match send_extract(group, &session, switches.gate(), attachment) {
            Ok(p) => {
                self.sent += 1;
                self.job = Some(Task::Pending(p));
            }
            Err(_) => self.problem = Some("bill.read.problem.busy"),
        }
    }

    /// Takes the finished job, if any: a reading to fill, else the problem.
    fn take_job(&mut self) -> Option<BillReading> {
        let r = self.job.as_mut()?.poll_result()?;
        self.job = None;
        match r {
            Ok(reading) if reading.status == ReadStatus::OcrOnly => {
                self.outcome = Some("ocr_only");
                tracing::info!(job = self.job_no, outcome = "ocr_only", "bill reading done");
                self.found_text = Some(reading.raw_text.unwrap_or_default());
                self.show_text = true;
                self.filled = None;
                None
            }
            Ok(reading) => Some(reading),
            Err(e) => {
                let key = problem_of(&e);
                self.problem = Some(key);
                self.outcome = Some(outcome_class(key));
                tracing::info!(
                    job = self.job_no,
                    outcome = outcome_class(key),
                    "bill reading failed"
                );
                None
            }
        }
    }

    /// Accept every read value in `form`.
    pub fn accept(&mut self, form: &mut BillForm) {
        form.read.accept_all();
        self.unaccepted = 0;
        self.low = 0;
        self.focus_accept = false;
    }

    /// Clear the read values from `form` (those the owner has not edited or
    /// accepted) and forget the reading.
    pub fn clear(&mut self, form: &mut BillForm) {
        let prior_date = self.filled.as_ref().and_then(|f| f.prior_date);
        for target in form.read.unaccepted() {
            match target {
                ReadTarget::Vendor => {
                    form.vendor = None;
                    if self.filled.as_ref().is_some_and(|f| f.terms_from_vendor) {
                        form.terms = None;
                        form.terms_days = None;
                    }
                }
                ReadTarget::BillDate => {
                    // Back to the date the new bill had before (never
                    // blank).
                    if prior_date.is_some() {
                        form.date = prior_date;
                    }
                }
                ReadTarget::DueDate => {
                    form.due_date = None;
                    form.due_overridden = false;
                }
                ReadTarget::Reference => form.reference.clear(),
                ReadTarget::AmountDue => {
                    form.amount_due = None;
                    form.amount_from_lines = false;
                }
                ReadTarget::Line(k) => form.expenses.lines.retain(|l| l.key != k),
                // The card and check forms' targets (A04); a bill never
                // marks them.
                ReadTarget::Payee
                | ReadTarget::Date
                | ReadTarget::Total
                | ReadTarget::CheckNumber
                | ReadTarget::PaidFrom => {}
            }
        }
        form.read.clear();
        self.filled = None;
        self.found_text = None;
        self.unaccepted = 0;
        self.low = 0;
        self.focus_accept = false;
    }

    /// True while the extract job (or a ghost request) runs.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.job.is_some()
            || self.local.busy()
            || self.switches_task.is_some()
            || !self.ghosts.asking.is_empty()
    }

    /// A document is being read (by AI or on this computer, or AI is
    /// helping).
    #[must_use]
    pub(crate) fn busy_reading(&self) -> bool {
        self.job.is_some() || self.local.busy()
    }

    /// What the banner offers after a reading made on this computer.
    #[must_use]
    pub(crate) fn assist_offer(&self) -> super::doc_read_local::Offer {
        self.local.offer(self.switches)
    }

    /// The control channel's view (no amounts, names or file names).
    #[must_use]
    pub fn state_json(&self) -> Value {
        let check = self.filled.as_ref().and_then(|f| f.check);
        let local = self.filled.as_ref().filter(|f| f.local);
        // A local reading's own checks say whether its figures agree.
        let reconciled = match local {
            Some(f) => Some(
                !f.checks
                    .iter()
                    .any(|c| super::doc_read_local::is_sum_check(c.code)),
            ),
            None => check.and_then(|c| c.total_ok),
        };
        json!({
            "busy": self.job.is_some(),
            "available": self.switches.is_some_and(ReadSwitches::read_with_ai),
            "requests": self.sent,
            "outcome": self.outcome,
            "problem": self.problem,
            "filled": self.filled.is_some(),
            "nothing_new": self.filled.as_ref().is_some_and(|f| f.nothing_new),
            "unaccepted": self.unaccepted,
            "low": self.low,
            "reconciled": reconciled,
            "subtotal_tax_ok": check.and_then(|c| c.subtotal_tax_ok),
            "local": self.local.state_json(),
            "assist": self.local.assist_json(self.switches),
            "filled_locally": local.is_some(),
            "checks": self
                .filled
                .as_ref()
                .map(|f| {
                    f.checks
                        .iter()
                        .map(|c| super::doc_read_local::code_name(c.code))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            "unknown_vendor": self.filled.as_ref().is_some_and(|f| f.unknown_vendor.is_some()),
            "unreadable": self.filled.as_ref().is_some_and(|f| f.unreadable),
            "ocr_only": self.found_text.is_some(),
            "focus_accept": self.focus_accept,
            "save_refused": self.refusal_seen,
            "ghosts": {
                "open": self.ghosts.open,
                "asking": self.ghosts.asking.len(),
                "accepted": self.ghosts.accepted,
                "ai": self.ghosts.shown.values().any(|s| s.basis.source() == "ai"),
            },
        })
    }
}

/// The reading buttons on the source-document card: **Read document** (on
/// this computer, A03) whenever a document is shown, and P17's **Read with
/// AI** when every AI switch is on. Read with AI starts here; true when
/// Read document was clicked (the screen starts it: it needs the shown
/// document and the vendors).
pub fn read_button(
    ui: &mut Ui,
    state: &mut ReadState,
    input: &ReadInput<'_>,
    tokens: &Tokens,
    has_document: bool,
) -> bool {
    let mut local = false;
    let ai = state.available(input);
    if has_document || ai {
        let busy = state.busy();
        ui.horizontal_wrapped(|ui| {
            if has_document {
                let r = ui
                    .add_enabled(!busy, egui::Button::new(t("bill.read.local.button")))
                    .on_hover_text(t("bill.read.local.button.tip"));
                if std::mem::take(&mut state.local.focus_read) && !busy {
                    r.request_focus();
                }
                if r.clicked() {
                    local = true;
                    // The button is disabled while reading: the keyboard
                    // goes to Cancel reading, not to the menu bar.
                    state.local.focus_cancel = r.has_focus();
                }
            }
            if ai
                && ui
                    .add_enabled(!busy, egui::Button::new(t("bill.read.button")))
                    .on_hover_text(t("bill.read.button.tip"))
                    .clicked()
            {
                state.start(input);
            }
        });
        if state.local.job.is_some() {
            form::progress_line(ui, tokens, &t("bill.read.local.reading"));
            // A long scan can be stopped (review S2), with the mouse, the
            // keyboard or Esc.
            let r = ui
                .button(t("bill.read.local.cancel"))
                .on_hover_text(t("bill.read.local.cancel.tip"));
            if std::mem::take(&mut state.local.focus_cancel) {
                r.request_focus();
            }
            state.local.cancel_focused = r.has_focus();
            if r.clicked() {
                state.local.cancel_job();
            }
        } else if std::mem::take(&mut state.local.cancel_focused) {
            // The reading ended (or was cancelled) with the keyboard on
            // Cancel reading: it goes back to Read document.
            state.local.focus_read = true;
            state.local.focus_cancel = false;
            ui.ctx().request_repaint();
        } else if state.local.focus_cancel && !local && !state.local.busy() {
            // The reading ended (or never started) before Cancel was drawn.
            state.local.focus_cancel = false;
            state.local.focus_read = true;
            ui.ctx().request_repaint();
        }
        if state.local.assist.is_some() {
            form::progress_line(ui, tokens, &t("bill.read.assist.reading"));
        }
        if state.job.is_some() {
            form::progress_line(ui, tokens, &t("bill.read.reading"));
        }
    }
    if let Some(key) = state.problem {
        ui.add(egui::Label::new(RichText::new(t(key)).size(12.0).color(tokens.warn_text)).wrap());
    }
    local
}

/// The shortcut chip text of a command ("Alt A"), from the keymap.
fn chip(id: &str) -> String {
    cl_model::commands::find(id)
        .and_then(|c| c.shortcut)
        .map(crate::input::shortcut_text)
        .unwrap_or_default()
        .replace('+', " ")
}

/// The source-document banner above the header: the reading's values to
/// accept, the reconcile message, "Add as a new vendor?" and the found
/// text of `ocr_only`. (The screen draws "Attached from <file> in your
/// receipt inbox" itself.)
pub fn banner(
    ui: &mut Ui,
    state: &mut ReadState,
    input: &ReadInput<'_>,
    tokens: &Tokens,
) -> BannerEvent {
    let mut event = BannerEvent::None;
    if let Some(text) = state.found_text.clone() {
        banner_frame(ui, tokens, tokens.info_tint, |ui| {
            ui.add(
                egui::Label::new(RichText::new(t("bill.read.ocr_only")).color(tokens.text_primary))
                    .wrap(),
            );
            let r = egui::CollapsingHeader::new(t("bill.read.found_text"))
                .id_salt("enter_bill/read/found_text")
                .open(Some(state.show_text))
                .show(ui, |ui| {
                    let mut shown = text.clone();
                    ui.add(
                        egui::TextEdit::multiline(&mut shown)
                            .interactive(false)
                            .desired_rows(4)
                            .desired_width(ui.available_width()),
                    );
                });
            if r.header_response.clicked() {
                state.show_text = !state.show_text;
            }
        });
    }
    let Some(filled) = state.filled.clone() else {
        return event;
    };
    let style = input.money_style;
    let pending = state.unaccepted > 0;
    if pending || filled.nothing_new {
        banner_frame(ui, tokens, tokens.warn_tint, |ui| {
            let name = if filled.file_name.is_empty() {
                t("bill.read.the_document")
            } else {
                filled.file_name.clone()
            };
            let key = if filled.nothing_new {
                "bill.read.banner.nothing_new"
            } else {
                "bill.read.banner"
            };
            ui.add(
                egui::Label::new(
                    RichText::new(t_with(key, &[("name", &name)])).color(tokens.text_primary),
                )
                .wrap(),
            );
            if state.low > 0 {
                let low_key = if filled.local {
                    "bill.read.local.banner.low"
                } else {
                    "bill.read.banner.low"
                };
                ui.add(
                    egui::Label::new(RichText::new(t(low_key)).strong().color(tokens.warn_text))
                        .wrap(),
                );
            }
            if let Some(c) = filled.check {
                if c.total_ok == Some(false)
                    && let (Some(sum), Some(total)) = (c.lines_sum, filled.total)
                {
                    let text = t_with(
                        "bill.read.reconcile.lines",
                        &[
                            ("lines", &sum.format(style)),
                            ("total", &total.format(style)),
                        ],
                    );
                    ui.add(egui::Label::new(RichText::new(text).color(tokens.warn_text)).wrap());
                }
                if c.subtotal_tax_ok == Some(false) {
                    ui.add(
                        egui::Label::new(
                            RichText::new(t("bill.read.reconcile.tax")).color(tokens.warn_text),
                        )
                        .wrap(),
                    );
                }
            }
            if filled.unreadable {
                ui.add(
                    egui::Label::new(
                        RichText::new(t("bill.read.unreadable")).color(tokens.warn_text),
                    )
                    .wrap(),
                );
            }
            // A local reading's checks: shown, never applied (AC3).
            if !filled.checks.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new(t("bill.read.local.checks"))
                            .strong()
                            .color(tokens.warn_text),
                    )
                    .wrap(),
                );
                for c in &filled.checks {
                    let text = super::doc_read_local::check_text(c, style);
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("\u{2022} {text}")).color(tokens.warn_text),
                        )
                        .wrap(),
                    );
                }
            }
            // A deposit invoice: the total and the balance due both shown.
            if let (Some(total), Some(balance)) = (filled.total, filled.balance_due) {
                ui.add(
                    egui::Label::new(
                        RichText::new(t_with(
                            "bill.read.local.total_and_balance",
                            &[
                                ("total", &total.format(style)),
                                ("balance", &balance.format(style)),
                            ],
                        ))
                        .color(tokens.text_primary),
                    )
                    .wrap(),
                );
            }
            // A03 step 21: the optional readers, only on the owner's click.
            if filled.local {
                if state.local.helped {
                    ui.add(
                        egui::Label::new(
                            RichText::new(t("bill.read.assist.helped")).color(tokens.text_primary),
                        )
                        .wrap(),
                    );
                }
                // Engine #179: a picture over its limits went without the
                // picture; the owner is told (never shrunk here, A03 D5).
                if state.local.picture == Some(crate::reading::assist::Picture::TooLarge) {
                    ui.add(
                        egui::Label::new(
                            RichText::new(t("bill.read.assist.picture_too_large"))
                                .color(tokens.text_primary),
                        )
                        .wrap(),
                    );
                }
                if super::doc_read_local::assist_ui(ui, state, tokens) {
                    event = BannerEvent::AskAi;
                }
            }
            if pending {
                ui.horizontal_wrapped(|ui| {
                    let accept = ui
                        .add(egui::Button::new(
                            RichText::new(t("bill.read.accept")).strong(),
                        ))
                        .on_hover_text(t("bill.read.accept.tip"));
                    crate::widgets::kbd(ui, &chip("form.accept_read"));
                    if std::mem::take(&mut state.focus_accept) {
                        accept.request_focus();
                    }
                    if accept.clicked() {
                        event = BannerEvent::Accepted;
                    }
                    if ui
                        .button(t("bill.read.clear"))
                        .on_hover_text(t("bill.read.clear.tip"))
                        .clicked()
                    {
                        event = BannerEvent::Cleared;
                    }
                });
            }
        });
    }
    if let Some(name) = &filled.unknown_vendor {
        banner_frame(ui, tokens, tokens.info_tint, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(t_with("bill.read.unknown_vendor", &[("name", name)]))
                        .color(tokens.text_primary),
                );
                if ui
                    .button(t("bill.read.add_vendor"))
                    .on_hover_text(t("bill.read.add_vendor.tip"))
                    .clicked()
                {
                    event = BannerEvent::AddVendor;
                }
            });
        });
    }
    event
}

fn banner_frame(ui: &mut Ui, tokens: &Tokens, fill: egui::Color32, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, tokens.border_default))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
    ui.add_space(4.0);
}

/// A painter slot behind a field, filled once the field's rect is known.
#[must_use]
pub(crate) fn mark_slot(ui: &Ui) -> egui::layers::ShapeIdx {
    ui.painter().add(egui::Shape::Noop)
}

/// The field's read mark: a `warn.tint` behind it and a caption under it;
/// a low-confidence field gets a stronger border and caption.
pub(crate) fn field_mark(
    ui: &mut Ui,
    tokens: &Tokens,
    state: Option<ReadField>,
    slot: egui::layers::ShapeIdx,
    r: &egui::Response,
) {
    let Some(ReadField::Read { level }) = state else {
        return;
    };
    let low = level == Confidence::Low;
    let rect = r.rect.expand(2.0);
    let stroke = if low {
        Stroke::new(2.0, tokens.warn_text)
    } else {
        Stroke::new(1.0, tokens.warn)
    };
    ui.painter().set(
        slot,
        egui::Shape::rect_filled(rect, CornerRadius::same(4), tokens.warn_tint),
    );
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(4),
        stroke,
        egui::StrokeKind::Outside,
    );
    // The same three levels the page's boxes show (UI check item 10): a
    // value read surely is not called "check it" here while the page
    // shows it as sure.
    let key = match level {
        Confidence::Low => "bill.read.mark.low",
        Confidence::High => "bill.read.mark.sure",
        _ => "bill.read.mark",
    };
    // The caption under the field is tied to it for screen readers (and
    // the control channel): the field itself says it was read.
    ui.ctx().accesskit_node_builder(r.id, |node| {
        node.set_description(t(key));
    });
    let text = RichText::new(t(key)).size(11.0).color(tokens.warn_text);
    ui.add(egui::Label::new(if low { text.strong() } else { text }).wrap());
}

fn signature(vendor: ContactId, memo: &str, amount: Option<Money>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    vendor.hash(&mut h);
    memo.trim().hash(&mut h);
    amount.map(Money::to_wire).hash(&mut h);
    h.finish()
}

impl EnterBillScreen {
    /// Every frame before drawing: a finished reading into the form (read,
    /// never accepted), the ghost accounts asked and answered, and the
    /// keyboard to Accept after a refused save.
    pub(crate) fn poll_read(&mut self) {
        if !self.read.initialized {
            self.read.initialized = true;
            self.expenses.clear_suggestions();
        }
        // The switches decide Read with AI and Ask AI to help (read once
        // from an engine, every frame from the demo).
        let link = Arc::clone(&self.link);
        let _ = self.read.switches(&link);
        if let Some(reading) = self.read.take_job() {
            self.read.done = Some(reading);
        }
        if let Some(reading) = self.read.done.take() {
            self.fill_from_reading(&reading);
        }
        self.poll_local_read();
        let refused = self
            .save
            .problems
            .contains(&cl_model::forms::bill::BillProblem::ReadNotAccepted);
        if refused && !self.read.refusal_seen {
            self.read.focus_accept = true;
        }
        self.read.refusal_seen = refused;
        self.maybe_suggest_lines();
        self.poll_line_suggestions();
    }

    /// Puts a reading into the form: empty fields only, every one read and
    /// not accepted (`doc_read::apply`).
    pub(crate) fn fill_from_reading(&mut self, reading: &BillReading) {
        let unknown_vendor = reading
            .vendor
            .is_none()
            .then(|| reading.vendor_name.clone())
            .flatten();
        self.fill(&Incoming {
            doc: doc_reading(reading),
            unknown_vendor,
            unreadable: !reading.unreadable.is_empty(),
            default_account: reading.default_expense_account,
            checks: Vec::new(),
            local: false,
            balance_due: None,
        });
    }

    /// Puts a reading (from AI or from this computer) into the form: empty
    /// fields only, every one read and not accepted.
    pub(crate) fn fill(&mut self, incoming: &Incoming) {
        if self.editing.as_ref().is_some_and(|e| e.read_only) {
            self.read.problem = Some("bill.read.problem.read_only");
            return;
        }
        let doc = &incoming.doc;
        // A local reading's own checks speak for its figures; P17's
        // lines-against-total compare is for the AI's reading only (a
        // document with fees would otherwise be flagged twice).
        let check = (!incoming.local).then(|| model::reconcile(doc));
        let mut form = self.compose();
        // A new bill's date is still the one it opened with: the
        // document's date may replace it.
        let prior_date = form.date;
        let untouched_date = self.editing.is_none() && self.header.original().date == form.date;
        if untouched_date && doc.bill_date.is_some() {
            form.date = None;
        }
        let vendor_before = form.vendor;
        let keys = &mut self.expenses.form_mut().adapter_mut().keys;
        let filled = model::apply(doc, &mut form, keys);
        if form.date.is_none() {
            form.date = prior_date;
        }
        // The vendor's terms come with a vendor the reading filled (as
        // picking one does), unless the owner chose terms.
        let terms_from_vendor = form.vendor != vendor_before && !self.prefill.terms_chosen;
        if terms_from_vendor && let Some(v) = form.vendor {
            form.terms = self
                .contacts
                .iter()
                .find(|c| c.id == v)
                .and_then(|c| c.fields.default_payment_terms.clone());
        }
        let unknown_vendor = incoming
            .unknown_vendor
            .as_deref()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty() && form.vendor.is_none());
        let snapshot = Snapshot {
            vendor: form.vendor,
            date: form.date,
            due: form.due_date,
            reference: form.reference.clone(),
            amount_due: form.amount_due,
            lines: form
                .expenses
                .lines
                .iter()
                .filter(|l| filled.contains(&ReadTarget::Line(l.key)))
                .map(|l| (l.key, l.amount))
                .collect(),
        };
        let read_lines: Vec<RowKey> = snapshot.lines.keys().copied().collect();
        self.read.outcome = Some(if filled.is_empty() {
            "nothing_new"
        } else {
            "filled"
        });
        tracing::info!(
            job = self.read.job_no,
            outcome = self.read.outcome.unwrap_or_default(),
            fields = filled.len(),
            "bill reading done"
        );
        self.read.filled = Some(Filled {
            file_name: std::mem::take(&mut self.read.job_file),
            check,
            total: doc.total,
            unknown_vendor,
            unreadable: incoming.unreadable,
            nothing_new: filled.is_empty(),
            snapshot,
            prior_date: prior_date.filter(|_| untouched_date),
            terms_from_vendor,
            local: incoming.local,
            checks: incoming.checks.clone(),
            balance_due: incoming.balance_due,
        });
        self.read.found_text = None;
        if filled.is_empty() {
            return;
        }
        self.take_form(form);
        if self.vendor() != vendor_before {
            self.items
                .set_default_account(self.vendor_default_account());
        }
        self.refresh_terms_dates();
        // Read lines get the reading's account for the vendor (else the
        // vendor's default) as a ghost: the owner accepts it with Tab.
        let ghost = incoming
            .default_account
            .filter(|a| self.accounts.iter().any(|x| x.id == *a && x.is_active))
            .or_else(|| self.vendor_default_account());
        let vendor = self.vendor();
        for key in read_lines {
            let line = self
                .expenses
                .value()
                .lines
                .iter()
                .find(|l| l.key == key)
                .cloned();
            if let (Some(account), Some(l), Some(v)) = (ghost, line, vendor) {
                self.read
                    .ghosts
                    .asked
                    .insert(key, signature(v, &l.memo, l.amount));
                self.expenses.set_suggestion(key, Some(account));
                self.read.ghosts.shown.insert(
                    key,
                    Suggested {
                        account,
                        basis: Basis::Usual(SuggestSource::Default),
                    },
                );
            }
        }
        self.track_read();
    }

    /// Every frame after drawing: a read field the owner changed counts as
    /// accepted (a line: only its amount); deleted read lines are
    /// forgotten. The counts for the banner are kept.
    pub(crate) fn track_read(&mut self) {
        let Some(filled) = self.read.filled.as_ref() else {
            self.read.unaccepted = 0;
            self.read.low = 0;
            self.read.ghosts.open = self.expenses.open_suggestions().len();
            return;
        };
        let snap = filled.snapshot.clone();
        let f = self.header.current().clone();
        let lines = &self.expenses.value().lines;
        let mut edited = Vec::new();
        let mut gone = Vec::new();
        for target in f.read.unaccepted() {
            let changed = match target {
                ReadTarget::Vendor => f.vendor != snap.vendor,
                ReadTarget::BillDate => f.date != snap.date,
                ReadTarget::DueDate => f.due_date != snap.due,
                ReadTarget::Reference => f.reference != snap.reference,
                ReadTarget::AmountDue => f.amount_due != snap.amount_due,
                ReadTarget::Line(k) => match lines.iter().find(|l| l.key == k) {
                    None => {
                        gone.push(k);
                        false
                    }
                    // Only the amount: a description or account change
                    // leaves the AI-read amount unaccepted (non-negotiable
                    // 4); Accept or typing the amount accepts it.
                    Some(l) => snap.lines.get(&k).is_none_or(|amount| l.amount != *amount),
                },
                // Not a bill's (A04 card and check targets).
                ReadTarget::Payee
                | ReadTarget::Date
                | ReadTarget::Total
                | ReadTarget::CheckNumber
                | ReadTarget::PaidFrom => false,
            };
            if changed {
                edited.push(target);
            }
        }
        for target in edited {
            let key = match target {
                ReadTarget::Vendor => field::VENDOR,
                ReadTarget::BillDate => field::DATE,
                ReadTarget::DueDate => field::DUE,
                ReadTarget::Reference => field::REFERENCE,
                ReadTarget::AmountDue => field::AMOUNT,
                ReadTarget::Line(_)
                | ReadTarget::Payee
                | ReadTarget::Date
                | ReadTarget::Total
                | ReadTarget::CheckNumber
                | ReadTarget::PaidFrom => LINE_MARKS,
            };
            self.header.edit(key, |h| h.read.edited(target));
        }
        if !gone.is_empty() {
            self.header.edit(LINE_MARKS, |h| {
                for k in gone {
                    h.read.forget_line(k);
                }
            });
        }
        let f = self.header.current();
        let un = f.read.unaccepted();
        self.read.low = un
            .iter()
            .filter(|t| {
                matches!(
                    f.read.state(**t),
                    Some(ReadField::Read {
                        level: Confidence::Low
                    })
                )
            })
            .count();
        self.read.unaccepted = un.len();
        self.read.ghosts.open = self.expenses.open_suggestions().len();
        if self.read.unaccepted == 0 {
            self.read.focus_accept = false;
        }
    }

    /// "Clear and type it myself".
    pub(crate) fn clear_read(&mut self) {
        let mut form = self.compose();
        let read_lines: Vec<RowKey> = self
            .read
            .filled
            .as_ref()
            .map(|f| f.snapshot.lines.keys().copied().collect())
            .unwrap_or_default();
        self.read.clear(&mut form);
        form.expenses
            .ensure_entry(&mut self.expenses.form_mut().adapter_mut().keys);
        self.take_form(form);
        for k in read_lines {
            self.expenses.set_suggestion(k, None);
            self.read.ghosts.shown.remove(&k);
            self.read.ghosts.asked.remove(&k);
        }
        self.refresh_terms_dates();
        self.track_read();
    }

    /// The caption under the lines for lines read from the document.
    pub(crate) fn read_lines_ui(&self, ui: &mut Ui, tokens: &Tokens) {
        let f = self.header.current();
        let mut read = Vec::new();
        let mut low = Vec::new();
        for (i, l) in self.expenses.value().lines.iter().enumerate() {
            if let Some(ReadField::Read { level }) = f.read.state(ReadTarget::Line(l.key)) {
                read.push(i + 1);
                if level == Confidence::Low {
                    low.push(i + 1);
                }
            }
        }
        if read.is_empty() {
            return;
        }
        let list = |v: &[usize]| {
            v.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        egui::Frame::new()
            .fill(tokens.warn_tint)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(
                    egui::Label::new(
                        RichText::new(t_with("bill.read.lines", &[("lines", &list(&read))]))
                            .color(tokens.warn_text),
                    )
                    .wrap(),
                );
                if !low.is_empty() {
                    ui.add(
                        egui::Label::new(
                            RichText::new(t_with("bill.read.lines.low", &[("lines", &list(&low))]))
                                .strong()
                                .color(tokens.warn_text),
                        )
                        .wrap(),
                    );
                }
            });
        ui.add_space(4.0);
    }

    /// The marks state of a header field.
    pub(crate) fn read_state_of(&self, target: ReadTarget) -> Option<ReadField> {
        self.header.current().read.state(target)
    }

    /// Lines with a description or an amount and no account: ask for a
    /// ghost account once the vendor is set.
    pub(crate) fn maybe_suggest_lines(&mut self) {
        let Some(vendor) = self.vendor() else { return };
        if self.editing.as_ref().is_some_and(|e| e.read_only) || self.save.task.is_some() {
            return;
        }
        let cursor = self
            .expenses
            .has_focus()
            .then(|| self.expenses.cursor())
            .flatten();
        let lines = self.expenses.value().lines.clone();
        for l in lines {
            if l.account.is_some() || (l.memo.trim().is_empty() && l.amount.is_none()) {
                continue;
            }
            if cursor.is_some_and(|c| {
                c.row == l.key
                    && (c.col == crate::forms::lines_grid::L_MEMO
                        || c.col == crate::forms::lines_grid::L_AMOUNT)
            }) {
                continue;
            }
            let sig = signature(vendor, &l.memo, l.amount);
            if self.read.ghosts.asked.get(&l.key) == Some(&sig)
                || self.read.ghosts.asking.iter().any(|a| a.key == l.key)
            {
                continue;
            }
            self.read.ghosts.asked.insert(l.key, sig);
            self.read.ghosts.shown.remove(&l.key);
            self.expenses.set_suggestion(l.key, None);
            self.ask_line(l.key, vendor, &l.memo, l.amount);
        }
    }

    fn ask_line(&mut self, key: RowKey, vendor: ContactId, memo: &str, amount: Option<Money>) {
        let header = self.header.current();
        let name = self.lists.name_text(vendor).unwrap_or_default().to_string();
        let text = if memo.trim().is_empty() {
            header.memo.trim().to_string()
        } else {
            memo.trim().to_string()
        };
        // The line's own amount as money out (the bank-feed sign); it only
        // lets the AI tell expense from income accounts.
        let typed = amount.unwrap_or(Money::ZERO);
        let ai_row = AiPreviewRow {
            description: crate::forms::suggest::clip(format!("{name} {text}").trim()),
            amount: typed.checked_neg().unwrap_or(typed),
        };
        let started = Instant::now();
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let mut found =
                crate::company::demo::checks::suggest_account(&mut demo, vendor).map(|s| {
                    Suggested {
                        account: s.account,
                        basis: Basis::Usual(s.source),
                    }
                });
            drop(demo);
            if found.is_none() && crate::forms::suggest::demo_gate(&link).is_open() {
                let mut demo = link.demo_state();
                found = demo
                    .as_deref_mut()
                    .and_then(|d| crate::company::demo::checks::ai_preview(d, &[ai_row]).ok())
                    .and_then(|rows| rows.into_iter().find(|r| r.index == 0))
                    .and_then(|r| {
                        Some(Suggested {
                            account: r.account_id?,
                            basis: Basis::Ai(r.basis),
                        })
                    });
            }
            self.read.ghosts.asking.push(Asking {
                key,
                started,
                task: Task::ready(found),
            });
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let under_cap =
            crate::forms::suggest::under_cap(&self.link, self.tenant.as_ref().map(|(t, _)| *t));
        if let Ok(p) = group.read(&session, move |s| async move {
            if let Ok(Some(found)) = s.contacts().suggest_account(vendor).await {
                return Ok(Some(Suggested {
                    account: found.account,
                    basis: Basis::Usual(found.source),
                }));
            }
            let config = s.ai_config().await.ok();
            let consent = s.company_ai_disclosure().await.ok();
            let gate = crate::forms::suggest::gate_of(config.as_ref(), consent.as_ref(), under_cap);
            if !gate.is_open() {
                // Every switch must be on: nothing is sent otherwise.
                return Ok(None);
            }
            let rows = s.ai_categorize_preview(gate, &[ai_row]).await?;
            Ok(rows.into_iter().find(|r| r.index == 0).and_then(|r| {
                Some(Suggested {
                    account: r.account_id?,
                    basis: Basis::Ai(r.basis),
                })
            }))
        }) {
            self.read.ghosts.asking.push(Asking {
                key,
                started,
                task: Task::Pending(p),
            });
        }
    }

    /// Ghost answers in, slow ones given up, accepts counted (Q-141).
    pub(crate) fn poll_line_suggestions(&mut self) {
        let mut done = Vec::new();
        let mut i = 0;
        while i < self.read.ghosts.asking.len() {
            let a = &mut self.read.ghosts.asking[i];
            if let Some(r) = a.task.try_take() {
                done.push((a.key, r));
                self.read.ghosts.asking.remove(i);
            } else if a.started.elapsed() > TIMEOUT {
                self.read.ghosts.asking.remove(i);
                self.read.ghosts.hint = Some("check.suggest.slow");
            } else {
                i += 1;
            }
        }
        for (key, r) in done {
            match r {
                Ok(Some(s)) => {
                    let open = self
                        .expenses
                        .value()
                        .lines
                        .iter()
                        .any(|l| l.key == key && l.account.is_none());
                    if open
                        && self
                            .accounts
                            .iter()
                            .any(|a| a.id == s.account && a.is_active)
                    {
                        self.expenses.set_suggestion(key, Some(s.account));
                        self.read.ghosts.shown.insert(key, s);
                    }
                }
                Ok(None) => {}
                Err(_) => self.read.ghosts.hint = Some("check.suggest.failed"),
            }
        }
        for (key, account) in self.expenses.take_accepted() {
            self.count_line_accept(key, account);
        }
        self.read.ghosts.open = self.expenses.open_suggestions().len();
    }

    fn count_line_accept(&mut self, key: RowKey, account: AccountId) {
        let Some(s) = self.read.ghosts.shown.remove(&key) else {
            return;
        };
        self.expenses.set_suggestion(key, None);
        self.read.ghosts.accepted += 1;
        let Some(vendor) = self.vendor() else { return };
        let tenant = self.tenant.as_ref().map(|(t, _)| *t);
        // The rule offer needs rules on bills (bank-feed rules): only the
        // count is kept here.
        let _ = crate::forms::suggest::count_accept(&self.link, tenant, vendor, account, &s.basis);
    }

    /// The ghosts' captions under the lines: what suggests the account,
    /// with Tab and Accept.
    pub(crate) fn line_suggestions_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let open = self.expenses.open_suggestions();
        let mut accept = None;
        for (key, account) in &open {
            let Some(s) = self.read.ghosts.shown.get(key) else {
                continue;
            };
            let line = self
                .expenses
                .value()
                .lines
                .iter()
                .position(|l| l.key == *key)
                .map_or(0, |i| i + 1);
            let name = self
                .lists
                .account_text(*account)
                .unwrap_or_default()
                .to_string();
            let caption = match &s.basis {
                Basis::Rule(rule) => t_with("check.suggest.rule", &[("rule", rule)]),
                Basis::Ai(Some(basis)) => t_with("check.suggest.ai_basis", &[("basis", basis)]),
                Basis::Ai(None) => t("check.suggest.ai"),
                Basis::Usual(SuggestSource::Default) => t("bill.read.ghost.default"),
                Basis::Usual(_) => t("bill.read.ghost.recent"),
            };
            egui::Frame::new()
                .fill(tokens.warn_tint)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(t_with(
                                "bill.read.ghost.line",
                                &[("line", &line.to_string())],
                            ))
                            .color(tokens.warn_text),
                        );
                        ui.label(RichText::new(&name).strong().color(tokens.warn_text));
                        ui.label(RichText::new(caption).color(tokens.warn_text));
                        crate::widgets::kbd(ui, "Tab");
                        ui.label(
                            RichText::new(t("check.suggest.to_accept")).color(tokens.warn_text),
                        );
                        let r = ui.button(t("check.suggest.accept")).on_hover_text(t_with(
                            "bill.read.ghost.accept.tip",
                            &[("line", &line.to_string())],
                        ));
                        // Each line's Accept has its own name ("Accept,
                        // line 2"), which starts with the word shown.
                        let name = t_with(
                            "bill.read.ghost.accept.name",
                            &[("line", &line.to_string())],
                        );
                        ui.ctx().accesskit_node_builder(r.id, |node| {
                            node.set_label(name);
                        });
                        if r.clicked() {
                            accept = Some(*key);
                        }
                    });
                });
            ui.add_space(4.0);
        }
        if let Some(key) = accept
            && self.expenses.accept_suggestion(key)
        {
            self.poll_line_suggestions();
        }
        if let Some(key) = self.read.ghosts.hint {
            form::hint(ui, tokens, &t(key));
        }
    }
}
