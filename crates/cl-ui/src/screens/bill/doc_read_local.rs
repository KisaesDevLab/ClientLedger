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
//! P17's Read with AI stays beside it (the new cloud consent is step 21).

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use cl_core::MoneyStyle;
use cl_core::intake::IntakeKind;
use cl_docread::model::{Check, CheckCode, CheckValue};
use cl_docread::protocol::JobHint;
use serde_json::{Value, json};

#[cfg(test)]
#[path = "doc_read_local_tests.rs"]
mod tests;

use super::EnterBillScreen;
use super::doc_read::Incoming;
use crate::i18n::{t, t_with};
use crate::reading::{ReadFailure, ReadOutcome, ReadRequest, ReadingService, map};

/// The local reading's state (part of the bill's `ReadState`).
#[derive(Default)]
pub(crate) struct LocalRead {
    /// The running job's answer.
    pub job: Option<Receiver<ReadOutcome>>,
    /// Documents sent to the reader (control channel).
    pub sent: u32,
    /// The class of the last outcome (control channel).
    pub outcome: Option<&'static str>,
    /// A service set by tests instead of the app's.
    pub service: Option<Arc<ReadingService>>,
}

impl LocalRead {
    pub(crate) fn busy(&self) -> bool {
        self.job.is_some()
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
        ReadFailure::Unreadable | ReadFailure::GaveUp => "bill.read.local.failed",
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
        CheckCode::ReadersDisagree | CheckCode::AiDisagrees => t("bill.read.check.disagree"),
        CheckCode::PickTheCard => t("bill.read.check.pick_card"),
        CheckCode::LinesNotRead => t("bill.read.check.lines_not_read"),
        CheckCode::PageNotRead => t("bill.read.check.page_not_read"),
        CheckCode::PagesSkipped => t("bill.read.check.pages_skipped"),
        CheckCode::KindUnclear => t("bill.read.check.kind"),
    }
}

impl EnterBillScreen {
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
        self.read.local.sent += 1;
        self.read.local.outcome = None;
        self.read.local.job = Some(service.submit(request));
    }

    /// Every frame: a finished local reading into the form, or its failure.
    pub(crate) fn poll_local_read(&mut self) {
        let Some(outcome) = self.read.local.take() else {
            return;
        };
        self.read.local.outcome = Some(outcome.class());
        match outcome {
            ReadOutcome::Read(local) => {
                let m = map::to_doc(&local);
                self.fill(&Incoming {
                    doc: m.doc,
                    unknown_vendor: m.unknown_vendor,
                    unreadable: false,
                    default_account: None,
                    checks: m.checks,
                    local: true,
                });
            }
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
