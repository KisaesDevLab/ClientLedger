//! **Read document** on Enter Bill (A03 step 18, plan D7, Q-A03l, S11):
//! the shown document is read on this computer by the reading service
//! ([`crate::reading`]); AI may be off and nothing goes online.
//!
//! - Only on the owner's click (the button on the source-document card or
//!   `bill.read_local`). The bytes are the preview's: nothing is fetched
//!   and **no draft is created** (S11, Q-A03l). P17's duplicate check on
//!   save stays as it is. With `document-drafts-v1` the company's saved
//!   layouts and card accounts are used (step 19a).
//! - The reading fills the **empty** fields through the same machinery as
//!   P17's Read with AI (`cl_model::forms::doc_read::apply`): every value is
//!   the document's own and *read, not accepted* until the owner accepts it
//!   (non-negotiable 4). Lines get the vendor's default account as a ghost
//!   (Q-A03r).
//! - The vendor is matched here on the extracted strings (Q-A03o); a
//!   printed name that matches no vendor offers "Add as a new vendor?".
//! - The reading's checks ("the lines don't add up to the subtotal") are
//!   listed on the banner and never fix anything (AC3).
//! - Any failure says "Couldn't read, enter by hand"; the form keeps
//!   working.
//! - Logs carry the job number and the outcome class only.
//!
//! # Ask AI to help (A03 step 21, plan D5 and D7)
//!
//! With the engine's document reading assist (`document-assist-v1`) Enter
//! Bill has **one** Read document entry: P17's Read with AI is not offered
//! (it stays for engines without the assist). When the local reading left
//! a main field empty or unsure, the banner offers **Ask AI to help**: the
//! page text goes to the company's AI (Ollama, or an online service only
//! with the company's consent, [`crate::reading::assist`]); the answer is
//! merged under the AI amount gate before anything reaches the form, then
//! replaces the reading's unaccepted values. Every value still needs
//! Accept. An online provider without consent shows where to allow it.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use cl_api::{ApiError, AssistSuggestion};
use cl_core::MoneyStyle;
use cl_core::intake::IntakeKind;
use cl_docread::model::{Check, CheckCode, CheckValue};
use cl_docread::protocol::JobHint;
use cl_docread::vendor_match::VendorCandidate;
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

#[cfg(test)]
#[path = "doc_read_local_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "doc_read_assist_tests.rs"]
mod assist_tests;

use super::EnterBillScreen;
use super::doc_read::{Incoming, ReadState, ReadSwitches};
use crate::company::{CompanyLink, Task};
use crate::i18n::{t, t_with};
use crate::reading::assist::{self, NoRoute, Route};
use crate::reading::{
    LocalReading, ReadCancel, ReadFailure, ReadOutcome, ReadRequest, ReadingService, map,
};
use crate::theme::Tokens;

/// What the banner offers after a reading made on this computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Offer {
    /// Nothing (all read, already helped, or AI can't help here).
    None,
    /// Ask AI to help, on this route.
    Ask(Route),
    /// The provider is online and the company hasn't allowed documents to
    /// go to it: say where to allow it.
    NeedsConsent,
}

impl Offer {
    /// The control channel's name.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Offer::None => "none",
            Offer::Ask(Route::Local) => "local",
            Offer::Ask(Route::Cloud { .. }) => "cloud",
            Offer::NeedsConsent => "needs_consent",
        }
    }
}

/// The local reading's state (part of the bill's `ReadState`).
#[derive(Default)]
pub(crate) struct LocalRead {
    /// The running job's answer.
    pub job: Option<Receiver<ReadOutcome>>,
    /// Stops the running job (Cancel, a new bill, the window closed).
    pub cancel: Option<ReadCancel>,
    /// The demo engine's write count when the last reading started
    /// (control channel: reading writes nothing, A03 S8).
    pub writes_at_read: Option<u32>,
    /// Documents sent to the reader (control channel).
    pub sent: u32,
    /// The class of the last outcome (control channel).
    pub outcome: Option<&'static str>,
    /// A service set by tests instead of the app's.
    pub service: Option<Arc<ReadingService>>,
    /// The document being read (its bytes: a PNG may go to the assist).
    pub bytes: Option<Arc<[u8]>>,
    /// The vendors it was matched against (matched again after help).
    pub vendors: Vec<VendorCandidate>,
    /// The last reading made here, with its page words (owner data).
    pub last: Option<Box<LocalReading>>,
    /// Its file name (shown on the banner again after help).
    pub file_name: String,
    /// It left a main field empty or unsure.
    pub needs_help: bool,
    /// AI already helped with it (one ask per reading).
    pub helped: bool,
    /// The running assist.
    pub assist: Option<Task<Result<AssistSuggestion, ApiError>>>,
    /// Assists sent (control channel and tests).
    pub assist_sent: u32,
    /// The class of the last assist outcome (control channel).
    pub assist_outcome: Option<&'static str>,
    /// Whether the last assist took the document's picture (a PNG over the
    /// engine's limits goes as text only, and the banner says so).
    pub picture: Option<assist::Picture>,
    /// The keyboard was on Read document when the reading started: it
    /// moves to Cancel reading (the button it was on is disabled while
    /// reading), and back to Read document when the reading ends.
    pub focus_cancel: bool,
    /// Cancel reading had the keyboard last frame.
    pub cancel_focused: bool,
    /// Give Read document the keyboard next frame.
    pub focus_read: bool,
}

/// A new bill or a closed window stops a reading still running (review
/// S2): the worker is never left reading for nobody.
impl Drop for LocalRead {
    fn drop(&mut self) {
        if let Some(c) = self.cancel.take() {
            c.cancel();
        }
    }
}

impl LocalRead {
    pub(crate) fn busy(&self) -> bool {
        self.job.is_some() || self.assist.is_some()
    }

    /// What the banner offers, given the switches as known.
    pub(crate) fn offer(&self, switches: Option<ReadSwitches>) -> Offer {
        if !self.needs_help || self.helped || self.last.is_none() || self.busy() {
            return Offer::None;
        }
        let Some(s) = switches else {
            return Offer::None;
        };
        match assist::route(s.assist_switches()) {
            Ok(r) => Offer::Ask(r),
            Err(NoRoute::NeedsConsent) => Offer::NeedsConsent,
            Err(NoRoute::Off | NoRoute::Unsupported) => Offer::None,
        }
    }

    /// Cancel: the running reading is stopped and its answer ignored; no
    /// message (A03 review S2).
    pub(crate) fn cancel_job(&mut self) {
        if let Some(c) = self.cancel.take() {
            c.cancel();
        }
        if self.job.take().is_some() {
            self.outcome = Some(ReadFailure::Cancelled.class());
        }
    }

    /// Forgets the last reading (a new one starts, or the bill cleared).
    fn forget(&mut self) {
        self.last = None;
        self.bytes = None;
        self.needs_help = false;
        self.helped = false;
        self.assist = None;
        self.picture = None;
    }

    pub(crate) fn assist_json(&self, switches: Option<ReadSwitches>) -> Value {
        json!({
            "offer": self.offer(switches).key(),
            "needs_help": self.needs_help,
            "helped": self.helped,
            "busy": self.assist.is_some(),
            "requests": self.assist_sent,
            "outcome": self.assist_outcome,
            "picture": self.picture.map(assist::Picture::key),
        })
    }

    /// The answer, once.
    fn take(&mut self) -> Option<ReadOutcome> {
        let rx = self.job.as_ref()?;
        let outcome = match rx.try_recv() {
            Ok(o) => o,
            Err(TryRecvError::Empty) => return None,
            // The service went away mid-read.
            Err(TryRecvError::Disconnected) => ReadOutcome::Failed(ReadFailure::Unavailable),
        };
        self.job = None;
        self.cancel = None;
        Some(outcome)
    }

    pub(crate) fn state_json(&self) -> Value {
        json!({
            "busy": self.busy(),
            "requests": self.sent,
            "outcome": self.outcome,
        })
    }
}

/// The plain message for a failure (i18n key).
#[must_use]
pub(crate) fn failure_key(f: ReadFailure) -> &'static str {
    match f {
        ReadFailure::Protected => "bill.read.local.failed.protected",
        ReadFailure::TooLarge => "bill.read.local.failed.too_large",
        ReadFailure::Unsupported => "bill.read.local.failed.unsupported",
        ReadFailure::NeedsOcr => "bill.read.local.failed.needs_ocr",
        ReadFailure::Unavailable => "bill.read.local.failed.unavailable",
        // Cancelled shows nothing; the key is never used for it.
        ReadFailure::Unreadable | ReadFailure::GaveUp | ReadFailure::Cancelled => {
            "bill.read.local.failed"
        }
    }
}

/// A check's code as the control channel shows it (`linesVsSubtotal`).
#[must_use]
pub(crate) fn code_name(code: CheckCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The checks that say the document's figures don't agree.
#[must_use]
pub(crate) fn is_sum_check(code: CheckCode) -> bool {
    matches!(
        code,
        CheckCode::LinesVsSubtotal | CheckCode::SubtotalTaxVsTotal | CheckCode::TotalPaidVsBalance
    )
}

fn value_text(v: Option<&CheckValue>, style: MoneyStyle) -> Option<String> {
    match v? {
        CheckValue::Money(m) => Some(m.format(style)),
        CheckValue::Date(d) => Some(d.to_string()),
        CheckValue::Text(_) => None,
    }
}

/// What the banner says about one check (owner text; shown, never logged).
#[must_use]
pub(crate) fn check_text(c: &Check, style: MoneyStyle) -> String {
    let money_pair = |key: &str, plain: &str| match (
        value_text(c.expected.as_ref(), style),
        value_text(c.found.as_ref(), style),
    ) {
        (Some(expected), Some(found)) => t_with(key, &[("expected", &expected), ("found", &found)]),
        _ => t(plain),
    };
    match c.code {
        CheckCode::LinesVsSubtotal => money_pair(
            "bill.read.check.lines_subtotal",
            "bill.read.check.lines_subtotal.plain",
        ),
        // No subtotal printed: the lines stood in for it (review S6).
        CheckCode::SubtotalTaxVsTotal if c.rule.as_str() == "validate.lines-total" => money_pair(
            "bill.read.check.lines_total",
            "bill.read.check.lines_total.plain",
        ),
        CheckCode::SubtotalTaxVsTotal => money_pair(
            "bill.read.check.subtotal_total",
            "bill.read.check.subtotal_total.plain",
        ),
        CheckCode::TotalPaidVsBalance => t("bill.read.check.paid_balance"),
        CheckCode::DueDateVsTerms => t("bill.read.check.due_terms"),
        CheckCode::LineQuantityTimesPrice => t_with(
            "bill.read.check.qty_price",
            &[(
                "line",
                &c.line
                    .map_or_else(String::new, |l| (u32::from(l) + 1).to_string()),
            )],
        ),
        CheckCode::DateCouldBeDayFirst => t("bill.read.check.day_first"),
        CheckCode::PickedUsingTotals => t("bill.read.check.picked_total"),
        // A second OCR read gave another amount (review S1): name it.
        CheckCode::ReadersDisagree if c.rule.as_str() == "ocr.second-read" => {
            money_pair("bill.read.check.second_read", "bill.read.check.disagree")
        }
        // A printed name won over another vendor's layout (review S4).
        CheckCode::ReadersDisagree if c.rule.as_str() == "vendor.layout" => match &c.expected {
            Some(CheckValue::Text(name)) => {
                t_with("bill.read.check.vendor_layout", &[("name", name)])
            }
            _ => t("bill.read.check.disagree"),
        },
        CheckCode::ReadersDisagree | CheckCode::AiDisagrees => t("bill.read.check.disagree"),
        CheckCode::PickTheCard => t("bill.read.check.pick_card"),
        CheckCode::LinesNotRead => t("bill.read.check.lines_not_read"),
        CheckCode::PageNotRead => t("bill.read.check.page_not_read"),
        CheckCode::PagesSkipped => t("bill.read.check.pages_skipped"),
        CheckCode::KindUnclear => t("bill.read.check.kind"),
    }
}

/// The banner's Ask AI to help (or where to allow it). True when clicked.
pub(crate) fn assist_ui(ui: &mut Ui, state: &ReadState, tokens: &Tokens) -> bool {
    match state.assist_offer() {
        Offer::Ask(route) => {
            let tip = match route {
                Route::Local => "bill.read.assist.button.tip.local",
                Route::Cloud { .. } => "bill.read.assist.button.tip.cloud",
            };
            ui.button(t("bill.read.assist.button"))
                .on_hover_text(t(tip))
                .clicked()
        }
        Offer::NeedsConsent => {
            ui.add(
                egui::Label::new(
                    RichText::new(t("bill.read.assist.consent")).color(tokens.text_secondary),
                )
                .wrap(),
            );
            false
        }
        Offer::None => false,
    }
}

/// The plain message for a failed assist (i18n key).
#[must_use]
pub(crate) fn assist_problem(e: &ApiError) -> &'static str {
    if cl_api::has_code(e, cl_api::CONSENT_REQUIRED) {
        "bill.read.assist.problem.consent"
    } else if cl_api::has_code(e, cl_api::CLOUD_VISION_DISABLED) {
        "bill.read.assist.problem.pictures"
    } else if cl_api::has_code(e, cl_api::AI_RATE_LIMIT) {
        "bill.read.problem.busy"
    } else {
        super::doc_read::problem_of(e)
    }
}

/// The demo engine's canned assist answer (no AI is called): the story
/// invoice's printed values, read on this network.
pub(crate) fn demo_assist(link: &CompanyLink) -> Result<AssistSuggestion, ApiError> {
    use crate::company::demo::bills::STORY_VENDOR;
    let Some(demo) = link.demo_state() else {
        return Err(ApiError::NoCompanyOpen);
    };
    if !(demo.ai.enabled && demo.ai.reading && demo.ai.assist) {
        return Err(ApiError::AiUnavailable);
    }
    let body = json!({"suggestion": {
        "reader": "self_hosted", "usedImages": false,
        "vendor": STORY_VENDOR, "vendorInvoiceNumber": "INV-55871",
        "billDate": "2026-11-03", "dueDate": "2026-11-18", "paymentTerms": "net_15",
        "subtotal": "1480.00", "tax": null, "total": "1480.00",
        "lineItems": [
            {"description": "Mini excavator, 2 days", "amount": "1330.00", "quantity": "2",
                "unitPrice": "665.00", "confidence": "high"},
            {"description": "Delivery and pickup", "amount": "150.00", "quantity": "1",
                "unitPrice": "150.00", "confidence": "high"},
        ],
        "confidence": 0.8,
    }});
    cl_api::suggestion_from_json(body.to_string().as_bytes())
}

impl EnterBillScreen {
    /// Ask AI to help with the last reading made here (the owner's click).
    /// Nothing is sent unless the switches allow it; an online provider
    /// also needs room under the monthly cap.
    pub(crate) fn start_assist(&mut self) {
        if self.read.busy_reading() {
            return;
        }
        let Offer::Ask(route) = self.read.assist_offer() else {
            return;
        };
        let Some(last) = self.read.local.last.clone() else {
            return;
        };
        self.read.problem = None;
        if matches!(route, Route::Cloud { .. })
            && !crate::forms::suggest::under_cap(&self.link, self.tenant.as_ref().map(|(t, _)| *t))
        {
            self.read.problem = Some("bill.read.problem.cap");
            self.read.local.assist_outcome = Some("cap");
            return;
        }
        let bytes = self
            .read
            .local
            .bytes
            .clone()
            .unwrap_or_else(|| Arc::from([]));
        let request = assist::request(&last, route, &bytes, cl_api::DraftKind::Bill);
        self.read.local.picture = Some(assist::picture(route, &bytes));
        self.read.job_no += 1;
        tracing::info!(job = self.read.job_no, "document assist started");
        if self.link.is_demo() {
            self.read.local.assist_sent += 1;
            self.read.local.assist = Some(Task::ready(demo_assist(&self.link)));
            return;
        }
        let link = Arc::clone(&self.link);
        let (Some(session), Some(group)) = (link.session(), self.read.assist_queue(&link)) else {
            self.read.problem = Some("bill.read.problem.unreachable");
            return;
        };
        match super::doc_read::send_assist(group, &session, request) {
            Ok(p) => {
                self.read.local.assist_sent += 1;
                self.read.local.assist = Some(Task::Pending(p));
            }
            Err(_) => self.read.problem = Some("bill.read.problem.busy"),
        }
    }

    /// Every frame: a finished assist, merged under the gate, replaces the
    /// reading's unaccepted values; or its failure.
    pub(crate) fn poll_assist(&mut self) {
        let Some(result) = self.read.local.assist.as_mut().and_then(Task::poll_result) else {
            return;
        };
        self.read.local.assist = None;
        let answer = match result {
            Ok(a) => a,
            Err(e) => {
                let key = assist_problem(&e);
                self.read.problem = Some(key);
                let class = key.rsplit('.').next().unwrap_or(key);
                self.read.local.assist_outcome = Some(class);
                tracing::info!(
                    job = self.read.job_no,
                    outcome = class,
                    "document assist failed"
                );
                return;
            }
        };
        let Some(last) = self.read.local.last.take() else {
            return;
        };
        // The gate runs before any form value exists (Q-A03j).
        let merged = assist::merge(&last, &answer, &self.read.local.vendors);
        self.read.local.assist_outcome = Some("helped");
        self.read.local.helped = true;
        // A04 draft mode: what the owner accepted or changed keeps its mark
        // (spots, learning and "Enter this as" read it).
        let kept: Vec<_> = if self.draft.is_some() {
            self.header
                .current()
                .read
                .entries()
                .filter(|(_, s)| !matches!(s, cl_model::forms::doc_read::ReadField::Read { .. }))
                .collect()
        } else {
            Vec::new()
        };
        // The reading's unaccepted values give way to the merged reading;
        // what the owner typed, changed or accepted stays.
        self.clear_read();
        self.read.job_file.clone_from(&self.read.local.file_name);
        let m = map::to_doc(&merged);
        self.fill(&Incoming {
            doc: m.doc,
            unknown_vendor: m.unknown_vendor,
            unreadable: false,
            default_account: None,
            checks: m.checks,
            local: true,
            balance_due: m.balance_due,
        });
        if self.draft.is_some() {
            // The read lines, in the merged reading's order (their spots).
            let read = self.header.current().read.clone();
            let lines: Vec<cl_model::forms::doc_read::ReadTarget> = self
                .expenses
                .value()
                .lines
                .iter()
                .map(|l| cl_model::forms::doc_read::ReadTarget::Line(l.key))
                .filter(|t| read.state(*t).is_some())
                .collect();
            if let Some(s) = self.draft.as_mut() {
                s.adopt_help(&merged);
                s.filled(&lines);
            }
        }
        if !kept.is_empty() {
            let mut f = self.compose();
            for (t, s) in kept {
                f.read.restore(t, s);
            }
            self.take_form(f);
            self.track_read();
        }
        self.read.local.last = Some(Box::new(merged));
    }

    /// Read document: send the shown document to the reading service.
    pub(crate) fn start_local_read(&mut self) {
        if self.read.local.busy() || self.read.busy_reading() {
            return;
        }
        self.read.problem = None;
        let Some(src) = self.attach.viewer.source().cloned() else {
            self.read.problem = Some("bill.read.problem.no_document");
            return;
        };
        let service = self
            .read
            .local
            .service
            .clone()
            .or_else(crate::reading::shared);
        let Some(service) = service else {
            self.read.problem = Some(failure_key(ReadFailure::Unavailable));
            self.read.local.outcome = Some(ReadFailure::Unavailable.class());
            return;
        };
        self.read.job_no += 1;
        self.read.job_file.clone_from(&src.name);
        tracing::info!(job = self.read.job_no, "local document reading started");
        // Saved layouts and card accounts come with `document-drafts-v1`
        // (step 19a); without it the reading is rules only (S12). Nothing
        // is claimed here: Enter Bill creates no draft (Q-A03l).
        let (vendors, templates) = crate::reading::service::with_layouts(
            map::vendor_candidates(&self.contacts),
            &self.read_context.layouts,
        );
        let request = ReadRequest {
            bytes: Arc::clone(&src.bytes),
            hint: JobHint {
                kind: Some(IntakeKind::Bills),
            },
            today: self.today,
            vendors,
            templates,
            card_accounts: self.read_context.card_accounts.clone(),
        };
        self.read.local.forget();
        self.read.local.bytes = Some(Arc::clone(&src.bytes));
        self.read.local.vendors.clone_from(&request.vendors);
        self.read.local.sent += 1;
        self.read.local.outcome = None;
        self.read.local.writes_at_read = self.link.demo_state().map(|d| d.bills.writes);
        let job = service.submit_cancellable(request);
        self.read.local.job = Some(job.answer);
        self.read.local.cancel = Some(job.cancel);
    }

    /// Every frame: a finished local reading into the form, or its failure.
    pub(crate) fn poll_local_read(&mut self) {
        self.poll_assist();
        let Some(outcome) = self.read.local.take() else {
            return;
        };
        self.read.local.outcome = Some(outcome.class());
        match outcome {
            ReadOutcome::Read(local) => {
                self.read.local.needs_help = assist::needs_help(&local);
                self.read.local.file_name.clone_from(&self.read.job_file);
                let m = map::to_doc(&local);
                self.read.local.last = Some(local);
                self.fill(&Incoming {
                    doc: m.doc,
                    unknown_vendor: m.unknown_vendor,
                    unreadable: false,
                    default_account: None,
                    checks: m.checks,
                    local: true,
                    balance_due: m.balance_due,
                });
            }
            // The owner cancelled: nothing to say.
            ReadOutcome::Failed(ReadFailure::Cancelled) => self.read.job_file.clear(),
            ReadOutcome::Failed(f) => {
                tracing::info!(
                    job = self.read.job_no,
                    outcome = f.class(),
                    "local document reading failed"
                );
                self.read.job_file.clear();
                self.read.problem = Some(failure_key(f));
            }
        }
    }
}
