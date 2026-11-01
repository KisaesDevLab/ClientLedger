//! Refund a Credit's form state (P22 D3): money paid back to a client from
//! a payment credit, a credit memo or a held deposit. No egui and no money
//! arithmetic: the amount defaults to the source's remaining string as the
//! engine wrote it; an amount the owner types is the owner's money, and the
//! engine answers `CREDIT_NOT_AVAILABLE` / `DEPOSIT_NOT_AVAILABLE` when it
//! is more than is left (nothing here compares).
//!
//! A check is queued to print by default (Q-P22d); a typed check number
//! turns that off. One save attempt keeps one [`RefundForm::attempt`]
//! (the UI's idempotency key), reused through the closing prompt.

use std::fmt;

use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money};

use crate::text::TextKey;

/// The longest check number.
pub const CHECK_NO_MAX: usize = 50;
/// The longest memo.
pub const MEMO_MAX: usize = 4000;

/// What kind of credit is refunded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefundSourceKind {
    /// A payment's unapplied credit.
    PaymentCredit,
    /// A credit memo.
    CreditMemo,
    /// A held customer deposit.
    HeldDeposit,
}

/// A credit the owner picked, with the engine's remaining figure.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RefundSourceChoice {
    /// The payment, memo or deposit.
    pub id: TransactionId,
    /// Which.
    pub kind: RefundSourceKind,
    /// What is left of it, as the engine wrote it.
    pub remaining: Money,
}

impl fmt::Debug for RefundSourceChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RefundSourceChoice")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// How the refund is paid (Q-88).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum RefundMethodChoice {
    /// A check (the default).
    #[default]
    Check,
    /// Cash.
    Cash,
    /// Back to the client's card.
    Card,
    /// Bank transfer.
    BankTransfer,
    /// Anything else.
    Other,
}

impl RefundMethodChoice {
    /// The engine's `paymentMethod`.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            RefundMethodChoice::Check => "check",
            RefundMethodChoice::Cash => "cash",
            RefundMethodChoice::Card => "credit_card",
            RefundMethodChoice::BankTransfer => "ach",
            RefundMethodChoice::Other => "other",
        }
    }

    /// The label's text key.
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            RefundMethodChoice::Check => TextKey("refund.method.check"),
            RefundMethodChoice::Cash => TextKey("refund.method.cash"),
            RefundMethodChoice::Card => TextKey("refund.method.card"),
            RefundMethodChoice::BankTransfer => TextKey("refund.method.bank_transfer"),
            RefundMethodChoice::Other => TextKey("refund.method.other"),
        }
    }

    /// Every method, in menu order.
    pub const ALL: [RefundMethodChoice; 5] = [
        RefundMethodChoice::Check,
        RefundMethodChoice::Cash,
        RefundMethodChoice::Card,
        RefundMethodChoice::BankTransfer,
        RefundMethodChoice::Other,
    ];
}

/// How a refund check is numbered in the plan.
#[derive(Clone, PartialEq, Eq)]
pub enum CheckPlan {
    /// Not a check.
    NotACheck,
    /// Queued to print.
    PrintLater,
    /// A hand-written check with this number.
    Number(String),
}

impl fmt::Debug for CheckPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CheckPlan::NotACheck => "NotACheck",
            CheckPlan::PrintLater => "PrintLater",
            CheckPlan::Number(_) => "Number(..)",
        })
    }
}

/// The checked refund (the UI maps it onto `cl_api::NewRefund`).
#[derive(Clone, PartialEq, Eq)]
pub struct RefundPlan {
    /// The client.
    pub client: ContactId,
    /// What it refunds.
    pub source: RefundSourceChoice,
    /// How much.
    pub amount: Money,
    /// How it is paid.
    pub method: RefundMethodChoice,
    /// The bank it comes from.
    pub from_account: AccountId,
    /// The date.
    pub date: CalendarDate,
    /// Check numbering.
    pub check: CheckPlan,
    /// Memo (`None` when blank).
    pub memo: Option<String>,
    /// The save attempt (one idempotency key each).
    pub attempt: u64,
}

impl fmt::Debug for RefundPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RefundPlan")
            .field("client", &self.client)
            .field("source", &self.source)
            .field("method", &self.method)
            .field("from_account", &self.from_account)
            .field("date", &self.date)
            .field("check", &self.check)
            .field("attempt", &self.attempt)
            .finish_non_exhaustive()
    }
}

/// Why a refund can't be saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefundProblem {
    /// No client.
    NoClient,
    /// No credit picked.
    NoSource,
    /// No amount, or not above zero.
    NoAmount,
    /// No bank account.
    NoAccount,
    /// No date.
    NoDate,
    /// A check needs a number or Print later.
    CheckNoOrPrintLater,
    /// The check number or memo is too long.
    TextTooLong,
    /// Dated on or before the closing date: the closing-date password.
    Closing,
}

impl RefundProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            RefundProblem::NoClient => TextKey("refund.problem.no_client"),
            RefundProblem::NoSource => TextKey("refund.problem.no_source"),
            RefundProblem::NoAmount => TextKey("refund.problem.no_amount"),
            RefundProblem::NoAccount => TextKey("refund.problem.no_account"),
            RefundProblem::NoDate => TextKey("refund.problem.no_date"),
            RefundProblem::CheckNoOrPrintLater => TextKey("refund.problem.check_no_or_print_later"),
            RefundProblem::TextTooLong => TextKey("refund.problem.text_too_long"),
            RefundProblem::Closing => TextKey("refund.problem.closing"),
        }
    }
}

/// What a refund is checked against.
#[derive(Clone, Copy, Debug, Default)]
pub struct RefundContext {
    /// The company's closing date.
    pub closing_date: Option<CalendarDate>,
    /// The owner entered the closing-date password for this attempt.
    pub password_given: bool,
}

/// The refund being entered.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct RefundForm {
    client: Option<ContactId>,
    source: Option<RefundSourceChoice>,
    /// `Some` once the owner typed an amount.
    typed: Option<Money>,
    method: RefundMethodChoice,
    from_account: Option<AccountId>,
    date: Option<CalendarDate>,
    print_later: bool,
    check_no: String,
    memo: String,
    attempt: u64,
}

impl fmt::Debug for RefundForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RefundForm")
            .field("client", &self.client.is_some())
            .field("source", &self.source)
            .field("method", &self.method)
            .field("print_later", &self.print_later)
            .field("attempt", &self.attempt)
            .finish_non_exhaustive()
    }
}

impl RefundForm {
    /// A blank refund dated `date`, paid by check, queued to print.
    #[must_use]
    pub fn new(date: Option<CalendarDate>, from_account: Option<AccountId>) -> RefundForm {
        RefundForm {
            date,
            from_account,
            print_later: true,
            ..RefundForm::default()
        }
    }

    /// A refund prefilled from a source (Receive Payment's "Refund it", a
    /// credit memo's Refund).
    #[must_use]
    pub fn for_source(
        client: ContactId,
        source: RefundSourceChoice,
        date: Option<CalendarDate>,
        from_account: Option<AccountId>,
    ) -> RefundForm {
        let mut f = RefundForm::new(date, from_account);
        f.client = Some(client);
        f.source = Some(source);
        f
    }

    fn touched(&mut self) {
        self.attempt += 1;
    }

    /// The client; a change drops the picked credit.
    pub fn set_client(&mut self, client: Option<ContactId>) {
        if self.client != client {
            self.client = client;
            self.source = None;
            self.typed = None;
            self.touched();
        }
    }

    /// The credit to refund (its remaining becomes the default amount).
    pub fn set_source(&mut self, source: Option<RefundSourceChoice>) {
        self.source = source;
        self.typed = None;
        self.touched();
    }

    /// The owner's amount (`None` goes back to the source's remaining).
    pub fn set_amount(&mut self, amount: Option<Money>) {
        self.typed = amount;
        self.touched();
    }

    /// The method; a check queues to print by default (Q-P22d).
    pub fn set_method(&mut self, method: RefundMethodChoice) {
        if self.method != method {
            self.method = method;
            self.print_later = method == RefundMethodChoice::Check;
            self.check_no.clear();
            self.touched();
        }
    }

    /// The bank it comes from.
    pub fn set_from_account(&mut self, a: Option<AccountId>) {
        self.from_account = a;
        self.touched();
    }

    /// The date.
    pub fn set_date(&mut self, d: Option<CalendarDate>) {
        self.date = d;
        self.touched();
    }

    /// Print later (checks only).
    pub fn set_print_later(&mut self, on: bool) {
        self.print_later = on && self.method == RefundMethodChoice::Check;
        if self.print_later {
            self.check_no.clear();
        }
        self.touched();
    }

    /// A hand-written check's number (turns Print later off).
    pub fn set_check_no(&mut self, n: &str) {
        self.check_no = n.to_string();
        if !n.trim().is_empty() {
            self.print_later = false;
        }
        self.touched();
    }

    /// The memo.
    pub fn set_memo(&mut self, m: &str) {
        self.memo = m.to_string();
        self.touched();
    }

    /// The amount shown: the owner's, else the source's remaining string.
    #[must_use]
    pub fn amount(&self) -> Option<Money> {
        self.typed.or_else(|| self.source.map(|s| s.remaining))
    }

    /// The picked credit.
    #[must_use]
    pub fn source(&self) -> Option<RefundSourceChoice> {
        self.source
    }

    /// The method.
    #[must_use]
    pub fn method(&self) -> RefundMethodChoice {
        self.method
    }

    /// Print later.
    #[must_use]
    pub fn print_later(&self) -> bool {
        self.print_later
    }

    /// The save attempt: changes with every edit, so a retry of the same
    /// refund (a timeout, the closing prompt) keeps its key.
    #[must_use]
    pub fn attempt(&self) -> u64 {
        self.attempt
    }

    /// The checked refund.
    ///
    /// # Errors
    /// Every [`RefundProblem`] found, in form order.
    pub fn save_plan(&self, ctx: &RefundContext) -> Result<RefundPlan, Vec<RefundProblem>> {
        let mut p = Vec::new();
        if self.client.is_none() {
            p.push(RefundProblem::NoClient);
        }
        if self.source.is_none() {
            p.push(RefundProblem::NoSource);
        }
        if !self.amount().is_some_and(Money::is_positive) {
            p.push(RefundProblem::NoAmount);
        }
        if self.from_account.is_none() {
            p.push(RefundProblem::NoAccount);
        }
        match self.date {
            None => p.push(RefundProblem::NoDate),
            Some(d) => {
                if ctx.closing_date.is_some_and(|c| d <= c) && !ctx.password_given {
                    p.push(RefundProblem::Closing);
                }
            }
        }
        let number = self.check_no.trim();
        let check = match self.method {
            RefundMethodChoice::Check if self.print_later => CheckPlan::PrintLater,
            RefundMethodChoice::Check if number.is_empty() => {
                p.push(RefundProblem::CheckNoOrPrintLater);
                CheckPlan::NotACheck
            }
            RefundMethodChoice::Check => CheckPlan::Number(number.to_string()),
            _ => CheckPlan::NotACheck,
        };
        if number.chars().count() > CHECK_NO_MAX || self.memo.chars().count() > MEMO_MAX {
            p.push(RefundProblem::TextTooLong);
        }
        if !p.is_empty() {
            return Err(p);
        }
        match (
            self.client,
            self.source,
            self.amount(),
            self.from_account,
            self.date,
        ) {
            (Some(client), Some(source), Some(amount), Some(from_account), Some(date)) => {
                Ok(RefundPlan {
                    client,
                    source,
                    amount,
                    method: self.method,
                    from_account,
                    date,
                    check,
                    memo: Some(self.memo.trim().to_string()).filter(|m| !m.is_empty()),
                    attempt: self.attempt,
                })
            }
            _ => Err(vec![RefundProblem::NoSource]),
        }
    }
}

#[cfg(test)]
#[path = "refund_tests.rs"]
mod tests;
