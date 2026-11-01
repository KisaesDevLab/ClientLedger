//! Credit Memo's form state (P22 D3): the shared invoice form of kind
//! [`SalesDocKind::CreditMemo`], the engine's preview of it, and the
//! "Apply to an invoice or keep as credit" question asked on Save when the
//! client has open invoices (Q-P22b). No egui and no money arithmetic: the
//! totals, the oldest-first suggestion and the remaining credit come from
//! the engine's preview ([`CreditMemoFigures`]); a save copies the
//! suggestion's amounts and each invoice's [`BalanceToken`] byte for byte,
//! or sends the owner's typed amount.
//!
//! - The figures belong to the request they answered: once the owner edits
//!   anything the engine prices, they are stale and Save waits (the same
//!   rule as P21's revisions).
//! - The engine refuses an application above the memo or an invoice's
//!   balance (409 `CREDIT_NOT_AVAILABLE`, 400 `OVERAPPLIED`); nothing here
//!   compares amounts.

use std::fmt;

use cl_core::ids::TransactionId;
use cl_core::{BalanceToken, CalendarDate, Money};

use crate::forms::invoice::{
    InvoiceCaps, InvoiceContext, InvoiceForm, InvoiceProblem, InvoiceRequest, SalesDocKind,
};
use crate::rowkey::RowKeyGen;
use crate::text::TextKey;

/// One open invoice of the client, as the preview listed it.
#[derive(Clone, PartialEq, Eq)]
pub struct OpenInvoiceFigure {
    /// The invoice.
    pub invoice: TransactionId,
    /// Its number (owner data).
    pub number: String,
    /// Its date.
    pub date: CalendarDate,
    /// When it is due.
    pub due: Option<CalendarDate>,
    /// Its cent balance (display).
    pub balance_due: Money,
    /// Its stored balance, echoed back on save.
    pub token: BalanceToken,
}

impl fmt::Debug for OpenInvoiceFigure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenInvoiceFigure")
            .field("invoice", &self.invoice)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

/// The engine's oldest-first allocation for one invoice.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SuggestedFigure {
    /// The invoice.
    pub invoice: TransactionId,
    /// The amount, as the engine wrote it.
    pub amount: Money,
}

impl fmt::Debug for SuggestedFigure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SuggestedFigure")
            .field("invoice", &self.invoice)
            .finish_non_exhaustive()
    }
}

/// What the engine's credit memo preview answered (the UI copies the
/// `cl_api::CreditMemoPreview` strings in).
#[derive(Clone, PartialEq, Eq)]
pub struct CreditMemoFigures {
    /// The memo total.
    pub total: Money,
    /// The client's open invoices, oldest first.
    pub open_invoices: Vec<OpenInvoiceFigure>,
    /// Oldest first up to the total.
    pub suggested: Vec<SuggestedFigure>,
    /// What would stay a credit after the suggestion.
    pub remaining_credit: Money,
    /// The preview's problem codes.
    pub problems: Vec<String>,
}

impl fmt::Debug for CreditMemoFigures {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreditMemoFigures")
            .field("open_invoices", &self.open_invoices.len())
            .field("suggested", &self.suggested.len())
            .field("problems", &self.problems)
            .finish_non_exhaustive()
    }
}

impl CreditMemoFigures {
    fn open(&self, invoice: TransactionId) -> Option<&OpenInvoiceFigure> {
        self.open_invoices.iter().find(|i| i.invoice == invoice)
    }

    fn suggested_for(&self, invoice: TransactionId) -> Option<Money> {
        self.suggested
            .iter()
            .find(|s| s.invoice == invoice)
            .map(|s| s.amount)
    }
}

/// One row of the apply panel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ApplyAmount {
    /// The engine's suggested amount for this invoice.
    Fill,
    /// The owner's amount (the owner's money; the engine checks it).
    Typed(Money),
}

impl fmt::Debug for ApplyAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ApplyAmount::Fill => "Fill",
            ApplyAmount::Typed(_) => "Typed(..)",
        })
    }
}

/// An invoice the memo is applied to at save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApplyRow {
    /// The invoice.
    pub invoice: TransactionId,
    /// How much.
    pub amount: ApplyAmount,
}

/// The answer to "Apply to an invoice or keep as credit?" (Q-P22b).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum AfterSave {
    /// Not asked or not answered yet.
    #[default]
    Unanswered,
    /// Apply to these invoices in the same save.
    Apply(Vec<ApplyRow>),
    /// Keep the whole memo as a credit.
    KeepCredit,
}

/// One application of the save, copied from the figures.
#[derive(Clone, PartialEq, Eq)]
pub struct SaveApplicationRow {
    /// The invoice.
    pub invoice: TransactionId,
    /// The suggestion's string or the owner's amount.
    pub amount: Money,
    /// The invoice's stored balance as the preview read it.
    pub token: BalanceToken,
}

impl fmt::Debug for SaveApplicationRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SaveApplicationRow")
            .field("invoice", &self.invoice)
            .finish_non_exhaustive()
    }
}

/// A credit memo save, wire-ready (the UI maps it onto
/// `cl_api::NewCreditMemo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreditMemoSave {
    /// The memo (kind `CreditMemo`).
    pub request: InvoiceRequest,
    /// Applications at save (empty: kept as credit).
    pub applications: Vec<SaveApplicationRow>,
}

/// Why a credit memo can't be saved yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CreditMemoProblem {
    /// A problem of the memo's fields or lines.
    Form(InvoiceProblem),
    /// The figures are being recalculated ("Updating…").
    PreviewPending,
    /// The engine couldn't calculate the memo ("Can't calculate").
    PreviewFailed,
    /// The memo totals zero or less (Q-P22r).
    TotalNotPositive,
    /// The client has open invoices: ask Apply or Keep first.
    Unanswered,
    /// Apply was chosen with no invoice.
    NothingToApply,
    /// An invoice in the apply panel is no longer open (reload).
    InvoiceGone(TransactionId),
}

impl CreditMemoProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            CreditMemoProblem::Form(p) => p.key(),
            CreditMemoProblem::PreviewPending => TextKey("credit_memo.problem.updating"),
            CreditMemoProblem::PreviewFailed => TextKey("credit_memo.problem.cant_calculate"),
            CreditMemoProblem::TotalNotPositive => {
                TextKey("credit_memo.problem.total_not_positive")
            }
            CreditMemoProblem::Unanswered => TextKey("credit_memo.problem.apply_or_keep"),
            CreditMemoProblem::NothingToApply => TextKey("credit_memo.problem.nothing_to_apply"),
            CreditMemoProblem::InvoiceGone(_) => TextKey("credit_memo.problem.invoice_gone"),
        }
    }
}

/// The credit memo being edited.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct CreditMemoForm {
    /// The memo itself (kind `CreditMemo`).
    pub doc: InvoiceForm,
    after_save: AfterSave,
    /// The request the figures answered, and the figures.
    figures: Option<(InvoiceRequest, CreditMemoFigures)>,
    /// The request whose preview failed.
    failed: Option<InvoiceRequest>,
}

impl fmt::Debug for CreditMemoForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreditMemoForm")
            .field("doc", &self.doc)
            .field("after_save", &self.after_save)
            .field("figures", &self.figures.as_ref().map(|(_, f)| f))
            .finish_non_exhaustive()
    }
}

impl CreditMemoForm {
    /// A blank credit memo dated `date`.
    #[must_use]
    pub fn new(date: Option<CalendarDate>, keys: &mut RowKeyGen) -> CreditMemoForm {
        CreditMemoForm {
            doc: InvoiceForm::new_kind(SalesDocKind::CreditMemo, date, keys),
            ..CreditMemoForm::default()
        }
    }

    /// The preview request for the memo as it stands (`None`: nothing to
    /// price yet).
    #[must_use]
    pub fn preview_request(&self, caps: InvoiceCaps) -> Option<InvoiceRequest> {
        self.doc.preview_request(caps)
    }

    /// Stores the engine's answer to `request`. An answer to an older
    /// request is dropped (false).
    pub fn apply_preview(
        &mut self,
        request: &InvoiceRequest,
        figures: CreditMemoFigures,
        caps: InvoiceCaps,
    ) -> bool {
        if self.preview_request(caps).as_ref() != Some(request) {
            return false;
        }
        // A changed list of open invoices drops rows that left it.
        if let AfterSave::Apply(rows) = &mut self.after_save {
            rows.retain(|r| figures.open(r.invoice).is_some());
        }
        self.failed = None;
        self.figures = Some((request.clone(), figures));
        true
    }

    /// The preview of `request` failed.
    pub fn fail_preview(&mut self, request: &InvoiceRequest, caps: InvoiceCaps) {
        if self.preview_request(caps).as_ref() == Some(request) {
            self.failed = Some(request.clone());
        }
    }

    /// The figures, only while they answer the memo as it stands.
    #[must_use]
    pub fn figures(&self, caps: InvoiceCaps) -> Option<&CreditMemoFigures> {
        let now = self.preview_request(caps)?;
        self.figures
            .as_ref()
            .filter(|(r, _)| *r == now)
            .map(|(_, f)| f)
    }

    /// The answer so far.
    #[must_use]
    pub fn after_save(&self) -> &AfterSave {
        &self.after_save
    }

    /// True when Save must ask Apply or Keep first: the client has open
    /// invoices and nothing was answered (Q-P22b).
    #[must_use]
    pub fn must_ask(&self, caps: InvoiceCaps) -> bool {
        self.after_save == AfterSave::Unanswered
            && self
                .figures(caps)
                .is_some_and(|f| !f.open_invoices.is_empty())
    }

    /// "Apply to an invoice": the rows the engine suggested, each filled
    /// with its suggestion.
    pub fn choose_apply(&mut self, caps: InvoiceCaps) {
        let rows = self
            .figures(caps)
            .map(|f| {
                f.suggested
                    .iter()
                    .map(|s| ApplyRow {
                        invoice: s.invoice,
                        amount: ApplyAmount::Fill,
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.after_save = AfterSave::Apply(rows);
    }

    /// "Keep as credit".
    pub fn choose_keep(&mut self) {
        self.after_save = AfterSave::KeepCredit;
    }

    /// Asks again (the panel's Back, or a changed client).
    pub fn reset_answer(&mut self) {
        self.after_save = AfterSave::Unanswered;
    }

    /// Sets one invoice's row in the apply panel: `None` takes it off,
    /// `Some(Fill)` the suggestion, `Some(Typed)` the owner's amount.
    pub fn set_row(&mut self, invoice: TransactionId, amount: Option<ApplyAmount>) {
        let AfterSave::Apply(rows) = &mut self.after_save else {
            return;
        };
        rows.retain(|r| r.invoice != invoice);
        if let Some(amount) = amount {
            rows.push(ApplyRow { invoice, amount });
        }
    }

    /// The save, or every reason it can't go yet. Nothing is added up.
    ///
    /// # Errors
    /// The memo's form problems first; then a preview that is pending or
    /// failed, a total not above zero, an unanswered question, Apply with
    /// no invoice or an invoice no longer open.
    pub fn save_plan(
        &self,
        ctx: &InvoiceContext,
    ) -> Result<CreditMemoSave, Vec<CreditMemoProblem>> {
        let request = check_kind(&self.doc, ctx)?;
        let Some(figures) = self.figures(ctx.caps) else {
            let failed = self.failed.is_some() && self.failed == self.preview_request(ctx.caps);
            return Err(vec![if failed {
                CreditMemoProblem::PreviewFailed
            } else {
                CreditMemoProblem::PreviewPending
            }]);
        };
        if figures
            .problems
            .iter()
            .any(|c| c == "CREDIT_MEMO_TOTAL_NOT_POSITIVE")
        {
            return Err(vec![CreditMemoProblem::TotalNotPositive]);
        }
        let applications = match &self.after_save {
            AfterSave::Unanswered if !figures.open_invoices.is_empty() => {
                return Err(vec![CreditMemoProblem::Unanswered]);
            }
            AfterSave::Unanswered | AfterSave::KeepCredit => Vec::new(),
            AfterSave::Apply(rows) if rows.is_empty() => {
                return Err(vec![CreditMemoProblem::NothingToApply]);
            }
            AfterSave::Apply(rows) => {
                let mut out = Vec::new();
                for r in rows {
                    let open = figures
                        .open(r.invoice)
                        .ok_or_else(|| vec![CreditMemoProblem::InvoiceGone(r.invoice)])?;
                    let amount = match r.amount {
                        ApplyAmount::Typed(m) => m,
                        ApplyAmount::Fill => figures
                            .suggested_for(r.invoice)
                            .ok_or_else(|| vec![CreditMemoProblem::NothingToApply])?,
                    };
                    out.push(SaveApplicationRow {
                        invoice: r.invoice,
                        amount,
                        token: open.token.clone(),
                    });
                }
                out
            }
        };
        Ok(CreditMemoSave {
            request,
            applications,
        })
    }
}

fn check_kind(
    doc: &InvoiceForm,
    ctx: &InvoiceContext,
) -> Result<InvoiceRequest, Vec<CreditMemoProblem>> {
    doc.to_new_invoice(ctx)
        .map_err(|ps| ps.into_iter().map(CreditMemoProblem::Form).collect())
}

#[cfg(test)]
#[path = "credit_memo_tests.rs"]
mod tests;
