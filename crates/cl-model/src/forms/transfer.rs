//! The Transfer Funds form (P14 WP-A2.2): from, to, date, amount, memo,
//! and [`check`], the gate before anything is sent. The amount is the
//! owner's typed `Money` (cents, Q-232); balances shown beside the
//! accounts are the engine's, never derived here.

use std::fmt;

use cl_core::ids::AccountId;
use cl_core::{CalendarDate, Money};

use crate::forms::amount_words::MAX_CHECK_UNITS;
use crate::forms::lines::trimmed;
use crate::register::AccountClass;
use crate::text::TextKey;

/// The Transfer Funds form.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TransferForm {
    /// Money leaves this account.
    pub from: Option<AccountId>,
    /// Money arrives in this account.
    pub to: Option<AccountId>,
    /// Date.
    pub date: Option<CalendarDate>,
    /// Amount (typed).
    pub amount: Option<Money>,
    /// Memo.
    pub memo: String,
}

impl fmt::Debug for TransferForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransferForm")
            .field("from", &self.from)
            .field("to", &self.to)
            .field("date", &self.date)
            .field("amount", &self.amount.is_some())
            .finish_non_exhaustive()
    }
}

/// Why a transfer can't be saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TransferProblem {
    /// No From account.
    FromMissing,
    /// No To account.
    ToMissing,
    /// From and To are the same account.
    SameAccount,
    /// No date.
    DateMissing,
    /// No amount.
    AmountMissing,
    /// Zero or negative.
    AmountNotPositive,
    /// More than 999,999,999,999.99.
    AmountTooLarge,
}

impl TransferProblem {
    /// Plain-language message.
    #[must_use]
    pub const fn key(self) -> TextKey {
        TextKey(match self {
            TransferProblem::FromMissing => "transfer.problem.from_missing",
            TransferProblem::ToMissing => "transfer.problem.to_missing",
            TransferProblem::SameAccount => "transfer.problem.same_account",
            TransferProblem::DateMissing => "transfer.problem.date_missing",
            TransferProblem::AmountMissing => "transfer.problem.amount_missing",
            TransferProblem::AmountNotPositive => "transfer.problem.amount_not_positive",
            TransferProblem::AmountTooLarge => "transfer.problem.amount_too_large",
        })
    }

    /// The control-channel name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            TransferProblem::FromMissing => "from_missing",
            TransferProblem::ToMissing => "to_missing",
            TransferProblem::SameAccount => "same_account",
            TransferProblem::DateMissing => "date_missing",
            TransferProblem::AmountMissing => "amount_missing",
            TransferProblem::AmountNotPositive => "amount_not_positive",
            TransferProblem::AmountTooLarge => "amount_too_large",
        }
    }
}

/// A transfer that passed [`check`].
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedTransfer {
    /// From.
    pub from: AccountId,
    /// To.
    pub to: AccountId,
    /// Date.
    pub date: CalendarDate,
    /// Amount (above zero).
    pub amount: Money,
    /// Memo, when typed.
    pub memo: Option<String>,
}

impl fmt::Debug for CheckedTransfer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedTransfer")
            .field("from", &self.from)
            .field("to", &self.to)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

/// The checked transfer, or every problem.
///
/// # Errors
/// See [`TransferProblem`].
pub fn check(f: &TransferForm) -> Result<CheckedTransfer, Vec<TransferProblem>> {
    let mut problems = Vec::new();
    if f.from.is_none() {
        problems.push(TransferProblem::FromMissing);
    }
    if f.to.is_none() {
        problems.push(TransferProblem::ToMissing);
    }
    if f.from.is_some() && f.from == f.to {
        problems.push(TransferProblem::SameAccount);
    }
    if f.date.is_none() {
        problems.push(TransferProblem::DateMissing);
    }
    match f.amount {
        None => problems.push(TransferProblem::AmountMissing),
        Some(m) if !m.is_positive() => problems.push(TransferProblem::AmountNotPositive),
        Some(m) if m.to_ten_thousandths() > MAX_CHECK_UNITS => {
            problems.push(TransferProblem::AmountTooLarge);
        }
        Some(_) => {}
    }
    match (f.from, f.to, f.date, f.amount) {
        (Some(from), Some(to), Some(date), Some(amount)) if problems.is_empty() => {
            Ok(CheckedTransfer {
                from,
                to,
                date,
                amount,
                memo: trimmed(&f.memo),
            })
        }
        _ => Err(problems),
    }
}

/// An account as the transfer pickers see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferCandidate {
    /// The account.
    pub id: AccountId,
    /// Its register class.
    pub class: AccountClass,
    /// The engine keeps it for its own postings (Payments Clearing, sales
    /// tax, …).
    pub system: bool,
    /// Active.
    pub active: bool,
}

/// The accounts money can move between (Q-P14h): bank, credit card, other
/// current assets and liabilities, loans; never A/R, A/P, equity, income
/// or expense accounts, the engine's system accounts or inactive ones.
#[must_use]
pub fn transfer_accounts(chart: &[TransferCandidate]) -> Vec<AccountId> {
    chart
        .iter()
        .filter(|a| a.active && !a.system)
        .filter(|a| {
            matches!(
                a.class,
                AccountClass::Bank
                    | AccountClass::CreditCard
                    | AccountClass::OtherAsset
                    | AccountClass::OtherLiability
            )
        })
        .map(|a| a.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(n: u64) -> AccountId {
        AccountId::parse(&format!("00000000-0000-0000-0000-{n:012}")).expect("id")
    }

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn valid() -> TransferForm {
        TransferForm {
            from: Some(account(1010)),
            to: Some(account(2050)),
            date: CalendarDate::from_ymd(2026, 11, 4).ok(),
            amount: Some(money("300.00")),
            memo: "  Card payment ".into(),
        }
    }

    #[test]
    fn a_valid_transfer_checks_and_keeps_the_typed_amount() {
        let c = check(&valid()).expect("ok");
        assert_eq!(c.amount, money("300.00"));
        assert_eq!(c.memo.as_deref(), Some("Card payment"));
        assert!(!format!("{c:?} {:?}", valid()).contains("300"));
    }

    #[test]
    fn every_problem_is_reported() {
        assert_eq!(
            check(&TransferForm::default()).err(),
            Some(vec![
                TransferProblem::FromMissing,
                TransferProblem::ToMissing,
                TransferProblem::DateMissing,
                TransferProblem::AmountMissing
            ])
        );
        let same = TransferForm {
            to: Some(account(1010)),
            ..valid()
        };
        assert_eq!(check(&same).err(), Some(vec![TransferProblem::SameAccount]));
        for (amount, p) in [
            ("0", TransferProblem::AmountNotPositive),
            ("-5", TransferProblem::AmountNotPositive),
            ("1000000000000", TransferProblem::AmountTooLarge),
        ] {
            let f = TransferForm {
                amount: Some(money(amount)),
                ..valid()
            };
            assert_eq!(check(&f).err(), Some(vec![p]));
            assert!(p.key().0.starts_with("transfer.problem."));
        }
        assert_eq!(TransferProblem::SameAccount.name(), "same_account");
    }

    #[test]
    fn transfer_accounts_follow_q_p14h() {
        let c = |n, class, system, active| TransferCandidate {
            id: account(n),
            class,
            system,
            active,
        };
        let chart = [
            c(1010, AccountClass::Bank, false, true),
            c(2050, AccountClass::CreditCard, false, true),
            c(1300, AccountClass::OtherAsset, false, true),
            c(2200, AccountClass::OtherLiability, false, true),
            c(1200, AccountClass::Receivable, false, true),
            c(2000, AccountClass::Payable, false, true),
            c(3000, AccountClass::Equity, false, true),
            c(4000, AccountClass::IncomeOrExpense, false, true),
            c(10150, AccountClass::OtherAsset, true, true),
            c(1020, AccountClass::Bank, false, false),
        ];
        assert_eq!(
            transfer_accounts(&chart),
            [account(1010), account(2050), account(1300), account(2200)]
        );
    }
}
