//! Recover a bad debt (P23 plan §2.8, Q-P23q): part of a written-off
//! invoice comes back. The invoice reopens by the amount; the engine works
//! out the recovered tax; the owner then receives the payment normally
//! ("Receive the payment now"). A 409 `RECOVERY_EXCEEDS_WRITE_OFF` names
//! what is left, which the form offers as the amount (a copy, never
//! computed).

use std::fmt;

use cl_core::ids::TransactionId;
use cl_core::{CalendarDate, Money, UserInput};

use crate::text::TextKey;

/// Why a recovery can't go yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RecoveryProblem {
    /// No invoice.
    NoInvoice,
    /// No date.
    NoDate,
    /// The amount is not an amount above zero.
    BadAmount,
}

impl RecoveryProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            RecoveryProblem::NoInvoice => TextKey("recover_bad_debt.problem.no_invoice"),
            RecoveryProblem::NoDate => TextKey("recover_bad_debt.problem.no_date"),
            RecoveryProblem::BadAmount => TextKey("recover_bad_debt.problem.bad_amount"),
        }
    }
}

/// What a recovery sends.
#[derive(Clone, PartialEq, Eq)]
pub struct RecoveryPlan {
    /// The invoice.
    pub invoice: TransactionId,
    /// The date.
    pub date: CalendarDate,
    /// The amount (whole cents, above zero).
    pub amount: Money,
    /// Memo (`None` when blank).
    pub memo: Option<String>,
    /// The save attempt (keyed once, reused through the closing prompt).
    pub attempt: u64,
}

impl fmt::Debug for RecoveryPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoveryPlan")
            .field("invoice", &self.invoice)
            .field("date", &self.date)
            .field("attempt", &self.attempt)
            .finish_non_exhaustive()
    }
}

/// The Recover Bad Debt dialog.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct RecoveryForm {
    /// The written-off invoice.
    pub invoice: Option<TransactionId>,
    /// The date.
    pub date: Option<CalendarDate>,
    amount: String,
    /// Memo (owner data).
    pub memo: String,
    /// What the engine said is left to recover (after a 409).
    available: Option<Money>,
    attempt: u64,
}

impl fmt::Debug for RecoveryForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoveryForm")
            .field("invoice", &self.invoice)
            .field("date", &self.date)
            .field("attempt", &self.attempt)
            .finish_non_exhaustive()
    }
}

impl RecoveryForm {
    /// A recovery of `invoice` dated `date`.
    #[must_use]
    pub fn new(invoice: TransactionId, date: CalendarDate) -> RecoveryForm {
        RecoveryForm {
            invoice: Some(invoice),
            date: Some(date),
            ..RecoveryForm::default()
        }
    }

    /// The amount as typed.
    #[must_use]
    pub fn amount_text(&self) -> &str {
        &self.amount
    }

    /// Types the amount (a new save attempt).
    pub fn set_amount(&mut self, text: &str) {
        self.amount = text.to_string();
        self.attempt += 1;
    }

    /// Records the engine's "available" figure from a 409 and offers it as
    /// the amount (its exact string).
    pub fn apply_available(&mut self, available: Money) {
        self.available = Some(available);
        self.amount = available
            .format(cl_core::MoneyStyle::default())
            .replace(',', "");
        self.attempt += 1;
    }

    /// What the engine last said is left to recover.
    #[must_use]
    pub fn available(&self) -> Option<Money> {
        self.available
    }

    /// The plan to send.
    ///
    /// # Errors
    /// The first [`RecoveryProblem`].
    pub fn plan(&self) -> Result<RecoveryPlan, RecoveryProblem> {
        let invoice = self.invoice.ok_or(RecoveryProblem::NoInvoice)?;
        let date = self.date.ok_or(RecoveryProblem::NoDate)?;
        let amount = Money::parse_user(&self.amount, UserInput::CENTS)
            .ok()
            .filter(|m| m.is_positive())
            .ok_or(RecoveryProblem::BadAmount)?;
        let memo = self.memo.trim();
        Ok(RecoveryPlan {
            invoice,
            date,
            amount,
            memo: (!memo.is_empty()).then(|| memo.to_string()),
            attempt: self.attempt,
        })
    }
}
