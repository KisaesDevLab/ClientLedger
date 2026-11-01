//! The bounced-check guided action (P21 D8, Q-82, Q-P21d/i/u): Confirm →
//! Fees → Done. The bounce (one dated "Bounced payment" entry with the
//! bank fee) and the optional fee invoice to the client are two engine
//! calls with their own idempotency keys (kept by the screen); once the
//! bounce is recorded a retry sends only the fee invoice
//! ([`BouncedCheckFlow::next_call`]).

use std::fmt;

use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Money, MoneyInputError, UserInput};

/// Where the flow is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BounceStep {
    /// What happens, in plain words; the bounce date.
    Confirm,
    /// The bank's fee and the optional fee invoice.
    Fees,
    /// The result.
    Done,
}

/// What the bounce reopened (the engine's figures).
#[derive(Clone, PartialEq, Eq)]
pub struct BounceOutcome {
    /// The "Bounced payment" entry.
    pub return_id: TransactionId,
    /// The bank fee expense.
    pub fee_id: Option<TransactionId>,
    /// The account the money came back out of.
    pub held_in: AccountId,
    /// The check was never deposited (Undeposited Funds).
    pub undeposited: bool,
    /// Each invoice reopened, with its balance now.
    pub reopened: Vec<(TransactionId, Money)>,
}

impl fmt::Debug for BounceOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BounceOutcome")
            .field("return_id", &self.return_id)
            .field("fee_id", &self.fee_id)
            .field("reopened", &self.reopened.len())
            .finish_non_exhaustive()
    }
}

/// The bounce call's input.
#[derive(Clone, PartialEq, Eq)]
pub struct BounceInput {
    /// The payment.
    pub payment: TransactionId,
    /// The bounce date.
    pub date: CalendarDate,
    /// The bank's fee.
    pub bank_fee: Option<Money>,
    /// Its expense account (else the company's).
    pub bank_fee_account: Option<AccountId>,
    /// The bank it comes from (a check never deposited).
    pub fee_bank_account: Option<AccountId>,
}

impl fmt::Debug for BounceInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BounceInput")
            .field("payment", &self.payment)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

/// The fee invoice's input (one non-taxable line, Q-P21p).
#[derive(Clone, PartialEq, Eq)]
pub struct FeeInvoiceInput {
    /// The date (the bounce date).
    pub date: CalendarDate,
    /// The fee.
    pub amount: Money,
    /// The income account.
    pub income_account: AccountId,
}

impl fmt::Debug for FeeInvoiceInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FeeInvoiceInput")
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

/// The next engine call of the flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BounceCall {
    /// Record the bounce (and the bank fee).
    Bounce(BounceInput),
    /// Create the fee invoice (the bounce is done).
    FeeInvoice(FeeInvoiceInput),
    /// Nothing left to send.
    Nothing,
}

/// Why the flow can't go on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BounceProblem {
    /// No bounce date.
    NoDate,
    /// The bank fee isn't an amount (at most 2 places, above 0).
    BankFee,
    /// The client fee isn't an amount.
    CustomerFee,
    /// A bank fee on a check never deposited needs the bank.
    FeeBank,
    /// A client fee needs the income account.
    FeeIncomeAccount,
}

/// The flow's state.
#[derive(Clone, PartialEq, Eq)]
pub struct BouncedCheckFlow {
    /// The step.
    pub step: BounceStep,
    /// The payment that bounced.
    pub payment: TransactionId,
    /// It was never deposited (the fee asks for its bank).
    pub never_deposited: bool,
    /// The bounce date (today by default).
    pub bounce_date: Option<CalendarDate>,
    /// The bank's fee as typed.
    pub bank_fee: String,
    /// The fee's expense account (the company default).
    pub bank_fee_account: Option<AccountId>,
    /// The bank the fee comes from, asked only when never deposited.
    pub fee_bank_account: Option<AccountId>,
    /// The client fee as typed (the Preferences default, blank until set).
    pub customer_fee: String,
    /// The fee invoice's income account (the company default).
    pub fee_income_account: Option<AccountId>,
    /// The bounce, once recorded.
    pub bounce: Option<BounceOutcome>,
    /// The fee invoice, once created.
    pub fee_invoice: Option<TransactionId>,
    /// The fee invoice failed after the bounce ("Try again").
    pub fee_failed: bool,
}

impl fmt::Debug for BouncedCheckFlow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BouncedCheckFlow")
            .field("step", &self.step)
            .field("payment", &self.payment)
            .field("bounce", &self.bounce)
            .field("fee_invoice", &self.fee_invoice)
            .field("fee_failed", &self.fee_failed)
            .finish_non_exhaustive()
    }
}

fn amount(text: &str) -> Result<Option<Money>, MoneyInputError> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    Money::parse_user(t, UserInput::MONEY).map(Some)
}

impl BouncedCheckFlow {
    /// A new flow for `payment`, dated `today`, with the company defaults.
    #[must_use]
    pub fn new(
        payment: TransactionId,
        never_deposited: bool,
        today: CalendarDate,
        defaults: BounceDefaults,
    ) -> BouncedCheckFlow {
        BouncedCheckFlow {
            step: BounceStep::Confirm,
            payment,
            never_deposited,
            bounce_date: Some(today),
            bank_fee: String::new(),
            bank_fee_account: defaults.bank_fee_account,
            fee_bank_account: None,
            customer_fee: defaults
                .customer_fee
                .map(|m| m.format(cl_core::MoneyStyle::default()))
                .unwrap_or_default(),
            fee_income_account: defaults.fee_income_account,
            bounce: None,
            fee_invoice: None,
            fee_failed: false,
        }
    }

    /// The bank's fee, if one was typed.
    ///
    /// # Errors
    /// [`BounceProblem::BankFee`] for text that isn't a positive amount.
    pub fn bank_fee(&self) -> Result<Option<Money>, BounceProblem> {
        match amount(&self.bank_fee) {
            Ok(Some(m)) if m.is_positive() => Ok(Some(m)),
            Ok(None) => Ok(None),
            _ => Err(BounceProblem::BankFee),
        }
    }

    /// The client fee, if one was typed.
    ///
    /// # Errors
    /// [`BounceProblem::CustomerFee`] for text that isn't a positive amount.
    pub fn customer_fee(&self) -> Result<Option<Money>, BounceProblem> {
        match amount(&self.customer_fee) {
            Ok(Some(m)) if m.is_positive() => Ok(Some(m)),
            Ok(None) => Ok(None),
            _ => Err(BounceProblem::CustomerFee),
        }
    }

    /// Confirm → Fees.
    ///
    /// # Errors
    /// [`BounceProblem::NoDate`].
    pub fn next_step(&mut self) -> Result<(), BounceProblem> {
        if self.step == BounceStep::Confirm {
            if self.bounce_date.is_none() {
                return Err(BounceProblem::NoDate);
            }
            self.step = BounceStep::Fees;
        }
        Ok(())
    }

    /// Back to Confirm (before anything is recorded).
    pub fn back(&mut self) {
        if self.step == BounceStep::Fees && self.bounce.is_none() {
            self.step = BounceStep::Confirm;
        }
    }

    /// The next call: the bounce until it is recorded, then the fee
    /// invoice (if a fee was typed) until it is created.
    ///
    /// # Errors
    /// The [`BounceProblem`] that stops it.
    pub fn next_call(&self) -> Result<BounceCall, BounceProblem> {
        let date = self.bounce_date.ok_or(BounceProblem::NoDate)?;
        let fee = self.customer_fee()?;
        if fee.is_some() && self.fee_income_account.is_none() {
            return Err(BounceProblem::FeeIncomeAccount);
        }
        if self.bounce.is_none() {
            let bank_fee = self.bank_fee()?;
            if bank_fee.is_some() && self.never_deposited && self.fee_bank_account.is_none() {
                return Err(BounceProblem::FeeBank);
            }
            return Ok(BounceCall::Bounce(BounceInput {
                payment: self.payment,
                date,
                bank_fee,
                bank_fee_account: self.bank_fee_account,
                fee_bank_account: self.fee_bank_account.filter(|_| self.never_deposited),
            }));
        }
        match (fee, self.fee_income_account, self.fee_invoice) {
            (Some(amount), Some(income_account), None) => {
                Ok(BounceCall::FeeInvoice(FeeInvoiceInput {
                    date,
                    amount,
                    income_account,
                }))
            }
            _ => Ok(BounceCall::Nothing),
        }
    }

    /// The bounce was recorded.
    pub fn bounced(&mut self, outcome: BounceOutcome) {
        self.bounce = Some(outcome);
        self.fee_failed = false;
        self.finish_if_done();
    }

    /// The fee invoice was created.
    pub fn fee_invoiced(&mut self, id: TransactionId) {
        self.fee_invoice = Some(id);
        self.fee_failed = false;
        self.finish_if_done();
    }

    /// The fee invoice failed: the bounce stays recorded ("Try again").
    pub fn fee_failed(&mut self) {
        self.fee_failed = true;
        self.step = BounceStep::Done;
    }

    fn finish_if_done(&mut self) {
        if matches!(self.next_call(), Ok(BounceCall::Nothing)) {
            self.step = BounceStep::Done;
        }
    }
}

/// The company defaults of the flow (Preferences › Sales).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BounceDefaults {
    /// The bank fee expense account.
    pub bank_fee_account: Option<AccountId>,
    /// The fee invoice's income account.
    pub fee_income_account: Option<AccountId>,
    /// The client fee (blank until set, Q-P21d).
    pub customer_fee: Option<Money>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> TransactionId {
        TransactionId::parse(&format!("00000000-0000-4000-8000-0000000000{n:02x}")).unwrap()
    }

    fn acct(n: u8) -> AccountId {
        AccountId::parse(&format!("00000000-0000-4000-8000-0000000001{n:02x}")).unwrap()
    }

    fn day() -> CalendarDate {
        CalendarDate::from_ymd(2026, 11, 12).unwrap()
    }

    fn outcome() -> BounceOutcome {
        BounceOutcome {
            return_id: id(9),
            fee_id: None,
            held_in: acct(1),
            undeposited: false,
            reopened: vec![(id(2), Money::parse_wire("4385").unwrap())],
        }
    }

    #[test]
    fn a_retry_after_the_bounce_sends_only_the_fee_invoice() {
        let mut f = BouncedCheckFlow::new(
            id(1),
            false,
            day(),
            BounceDefaults {
                bank_fee_account: Some(acct(2)),
                fee_income_account: Some(acct(3)),
                customer_fee: Some(Money::parse_wire("25").unwrap()),
            },
        );
        assert_eq!(f.customer_fee, "25.00");
        f.next_step().unwrap();
        f.bank_fee = "35".into();
        let BounceCall::Bounce(b) = f.next_call().unwrap() else {
            panic!("bounce first")
        };
        assert_eq!(b.bank_fee, Some(Money::parse_wire("35").unwrap()));
        assert_eq!(b.fee_bank_account, None);
        f.bounced(outcome());
        assert_eq!(f.step, BounceStep::Fees);
        f.fee_failed();
        assert_eq!(f.step, BounceStep::Done);
        // Try again: the bounce is never sent twice.
        let BounceCall::FeeInvoice(i) = f.next_call().unwrap() else {
            panic!("fee invoice only")
        };
        assert_eq!(i.amount, Money::parse_wire("25").unwrap());
        f.fee_invoiced(id(5));
        assert_eq!(f.next_call().unwrap(), BounceCall::Nothing);
        assert_eq!(f.step, BounceStep::Done);
        assert!(!format!("{f:?}").contains("25"));
    }

    #[test]
    fn a_check_never_deposited_asks_for_the_fee_bank_and_fees_must_be_amounts() {
        let mut f = BouncedCheckFlow::new(id(1), true, day(), BounceDefaults::default());
        assert_eq!(f.customer_fee, "");
        f.bank_fee = "35".into();
        assert_eq!(f.next_call(), Err(BounceProblem::FeeBank));
        f.fee_bank_account = Some(acct(1));
        assert!(matches!(f.next_call(), Ok(BounceCall::Bounce(_))));
        f.bank_fee = "3.5.0".into();
        assert_eq!(f.next_call(), Err(BounceProblem::BankFee));
        f.bank_fee = String::new();
        f.customer_fee = "25".into();
        assert_eq!(f.next_call(), Err(BounceProblem::FeeIncomeAccount));
        f.customer_fee = String::new();
        f.bounce_date = None;
        assert_eq!(f.next_step(), Err(BounceProblem::NoDate));
        // No fees: the bounce alone finishes the flow.
        f.bounce_date = Some(day());
        f.next_step().unwrap();
        f.bounced(outcome());
        assert_eq!(f.step, BounceStep::Done);
    }
}
