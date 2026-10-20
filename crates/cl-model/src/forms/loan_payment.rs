//! The Loan Payment helper (P14 C2.3, Q-62, Q-P14p, Q-P14q): the owner types
//! the payment and the interest from the lender's statement; the principal
//! is `payment − interest − Σextras`, exactly ([`principal`]). No schedules
//! and no amortization (Q-62).
//!
//! The payment posts as a check, or an online payment, from the bank: one
//! line to the loan (the principal, when above zero), one to the interest
//! account (when above zero) and the extra lines (fees, escrow). The posted
//! lines always add up to the payment exactly. The loan's balance after the
//! save is the engine's, never `before − principal` (plan §4).

use std::fmt;

use cl_core::ids::{AccountId, ContactId};
use cl_core::{CalendarDate, Money};

use crate::forms::card::{AmountIssue, LineIssue, MAX_LINES, amount_problem, check_lines};
use crate::forms::check::parse_number;
use crate::forms::lines::{FormLines, PostingLine, lines_total, trimmed};
use crate::lists::tags::TagList;
use crate::rowkey::RowKey;
use crate::text::TextKey;

/// How the payment is made (Q-P14p).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoanMethod {
    /// A check with this number (may be empty: no number).
    Check(String),
    /// Online or automatic.
    Online,
}

/// The form's complete editable value. `Debug` is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct LoanPaymentForm {
    /// The loan (liability) account.
    pub loan: Option<AccountId>,
    /// The lender.
    pub lender: Option<ContactId>,
    /// The bank (or card) it is paid from.
    pub pay_from: Option<AccountId>,
    /// Paid by.
    pub method: LoanMethod,
    /// Date.
    pub date: Option<CalendarDate>,
    /// The payment the owner typed.
    pub payment: Option<Money>,
    /// The interest the owner typed from the statement (0 must be typed).
    pub interest: Option<Money>,
    /// Where the interest goes.
    pub interest_account: Option<AccountId>,
    /// Extra lines (fees, escrow).
    pub extras: FormLines,
    /// Optional: the lender's balance after this payment, from the
    /// statement (compared with the engine's after the save, Q-P14q).
    pub lender_balance: Option<Money>,
    /// Memo.
    pub memo: String,
}

impl fmt::Debug for LoanPaymentForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoanPaymentForm")
            .field("loan", &self.loan.is_some())
            .field("lender", &self.lender.is_some())
            .field("pay_from", &self.pay_from.is_some())
            .field("check", &matches!(self.method, LoanMethod::Check(_)))
            .field("date", &self.date.is_some())
            .field("payment", &self.payment.is_some())
            .field("interest", &self.interest.is_some())
            .field("interest_account", &self.interest_account.is_some())
            .field("extras", &self.extras.lines.len())
            .field("lender_balance", &self.lender_balance.is_some())
            .field("memo", &!self.memo.is_empty())
            .finish()
    }
}

impl LoanPaymentForm {
    /// A blank form.
    #[must_use]
    pub fn new(date: Option<CalendarDate>) -> LoanPaymentForm {
        LoanPaymentForm {
            loan: None,
            lender: None,
            pay_from: None,
            method: LoanMethod::Online,
            date,
            payment: None,
            interest: None,
            interest_account: None,
            extras: FormLines::default(),
            lender_balance: None,
            memo: String::new(),
        }
    }

    /// The principal as the form shows it (read-only): `None` until the
    /// payment and the interest are typed.
    #[must_use]
    pub fn principal(&self) -> Option<Money> {
        principal(self.payment?, self.interest?, &self.extras)
    }
}

/// `payment − interest − Σextras`, exactly; `None` only when the result
/// leaves the money range. May be negative (the save is then refused).
#[must_use]
pub fn principal(payment: Money, interest: Money, extras: &FormLines) -> Option<Money> {
    payment
        .checked_sub(interest)
        .ok()?
        .checked_sub(lines_total(extras)?)
        .ok()
}

/// The field a problem belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoanField {
    /// Loan account.
    Loan,
    /// Pay from.
    PayFrom,
    /// Check number.
    Number,
    /// Date.
    Date,
    /// Payment.
    Payment,
    /// Interest.
    Interest,
    /// Interest account.
    InterestAccount,
    /// One extra line.
    Line(RowKey),
}

/// Why a loan payment can't be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoanProblem {
    /// No loan account.
    LoanMissing,
    /// No account to pay from.
    PayFromMissing,
    /// The check number is not a whole number from 1 to 2,147,483,647.
    NumberInvalid,
    /// A check needs a lender to pay.
    LenderMissing,
    /// No date.
    DateMissing,
    /// The payment is blank, zero or too large.
    Payment(AmountIssue),
    /// The interest is blank (type 0 when there is none).
    InterestMissing,
    /// The interest is below zero.
    InterestNegative,
    /// No account for the interest (when there is interest).
    InterestAccountMissing,
    /// Two of the loan, the pay-from and the interest accounts are the same.
    SameAccount,
    /// The interest and extras are more than the payment by `by`.
    PrincipalNegative {
        /// How much more, exact (positive).
        by: Money,
    },
    /// More than [`MAX_LINES`] lines.
    TooManyLines,
    /// One extra line's issue.
    Line(RowKey, LineIssue),
}

impl LoanProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            LoanProblem::LoanMissing => TextKey("loan.problem.loan_missing"),
            LoanProblem::PayFromMissing => TextKey("loan.problem.pay_from_missing"),
            LoanProblem::NumberInvalid => TextKey("check.problem.number_invalid"),
            LoanProblem::LenderMissing => TextKey("loan.problem.lender_missing"),
            LoanProblem::DateMissing => TextKey("check.problem.date_missing"),
            LoanProblem::Payment(AmountIssue::Missing) => TextKey("loan.problem.payment_missing"),
            LoanProblem::Payment(AmountIssue::NotPositive) => {
                TextKey("check.problem.amount_not_positive")
            }
            LoanProblem::Payment(AmountIssue::TooLarge) => {
                TextKey("check.problem.amount_too_large")
            }
            LoanProblem::InterestMissing => TextKey("loan.problem.interest_missing"),
            LoanProblem::InterestNegative => TextKey("loan.problem.interest_negative"),
            LoanProblem::InterestAccountMissing => TextKey("loan.problem.interest_account_missing"),
            LoanProblem::SameAccount => TextKey("loan.problem.same_account"),
            LoanProblem::PrincipalNegative { .. } => TextKey("loan.problem.principal_negative"),
            LoanProblem::TooManyLines => TextKey("check.problem.too_many_lines"),
            LoanProblem::Line(_, i) => i.key(),
        }
    }

    /// The control channel's name (no amounts).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            LoanProblem::LoanMissing => "loan_missing",
            LoanProblem::PayFromMissing => "pay_from_missing",
            LoanProblem::NumberInvalid => "number_invalid",
            LoanProblem::LenderMissing => "lender_missing",
            LoanProblem::DateMissing => "date_missing",
            LoanProblem::Payment(AmountIssue::Missing) => "payment_missing",
            LoanProblem::Payment(AmountIssue::NotPositive) => "payment_not_positive",
            LoanProblem::Payment(AmountIssue::TooLarge) => "payment_too_large",
            LoanProblem::InterestMissing => "interest_missing",
            LoanProblem::InterestNegative => "interest_negative",
            LoanProblem::InterestAccountMissing => "interest_account_missing",
            LoanProblem::SameAccount => "same_account",
            LoanProblem::PrincipalNegative { .. } => "principal_negative",
            LoanProblem::TooManyLines => "too_many_lines",
            LoanProblem::Line(_, i) => i.name(),
        }
    }

    /// The field to focus.
    #[must_use]
    pub const fn field(self) -> LoanField {
        match self {
            LoanProblem::LoanMissing | LoanProblem::SameAccount => LoanField::Loan,
            LoanProblem::PayFromMissing => LoanField::PayFrom,
            LoanProblem::NumberInvalid | LoanProblem::LenderMissing => LoanField::Number,
            LoanProblem::DateMissing => LoanField::Date,
            LoanProblem::Payment(_) | LoanProblem::TooManyLines => LoanField::Payment,
            LoanProblem::InterestMissing
            | LoanProblem::InterestNegative
            | LoanProblem::PrincipalNegative { .. } => LoanField::Interest,
            LoanProblem::InterestAccountMissing => LoanField::InterestAccount,
            LoanProblem::Line(k, _) => LoanField::Line(k),
        }
    }
}

/// What the checked payment posts as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoanPosting {
    /// A check (`None`: no number).
    Check(Option<u32>),
    /// An online or automatic payment (an expense from `pay_from`).
    Online,
}

/// A payment that passed [`check`]: the lines to post, which add up to the
/// payment exactly. `Debug` is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedLoanPayment {
    /// Check or online.
    pub posting: LoanPosting,
    /// The loan account.
    pub loan: AccountId,
    /// The lender.
    pub lender: Option<ContactId>,
    /// Paid from.
    pub pay_from: AccountId,
    /// Date.
    pub date: CalendarDate,
    /// The payment, more than zero.
    pub payment: Money,
    /// The principal (zero or more).
    pub principal: Money,
    /// The interest (zero or more).
    pub interest: Money,
    /// Memo, trimmed.
    pub memo: Option<String>,
    /// The posted lines: principal to the loan (when above zero), interest
    /// (when above zero), then the extras. Their sum is the payment.
    pub lines: Vec<PostingLine>,
}

impl fmt::Debug for CheckedLoanPayment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedLoanPayment")
            .field("posting", &self.posting)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

/// Checks the form and builds the lines.
///
/// # Errors
/// Every problem found, in field order.
pub fn check(
    form: &LoanPaymentForm,
    tags: &TagList,
) -> Result<CheckedLoanPayment, Vec<LoanProblem>> {
    let mut problems = Vec::new();
    let posting = header_problems(form, &mut problems);
    if form.extras.used().count() > MAX_LINES.saturating_sub(2) {
        problems.push(LoanProblem::TooManyLines);
    }
    let (extras, issues) = check_lines(&form.extras, form.pay_from, &[], tags, false);
    problems.extend(issues.into_iter().map(|(k, i)| LoanProblem::Line(k, i)));
    let principal = form.principal();
    if let Some(p) = principal.filter(|p| p.is_negative()) {
        problems.push(LoanProblem::PrincipalNegative { by: p.abs() });
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let (Some(loan), Some(pay_from), Some(date), Some(payment), Some(interest), Some(principal)) = (
        form.loan,
        form.pay_from,
        form.date,
        form.payment,
        form.interest,
        principal,
    ) else {
        // Every `None` above pushed a problem (an overflowing principal
        // can't happen with amounts in range; refuse rather than panic).
        return Err(vec![LoanProblem::Payment(AmountIssue::TooLarge)]);
    };
    let memo = trimmed(&form.memo);
    let mut lines = Vec::new();
    if principal.is_positive() {
        lines.push(PostingLine {
            account: loan,
            amount: principal,
            memo: memo.clone(),
            contact: None,
            billable: false,
            tag: None,
        });
    }
    if interest.is_positive()
        && let Some(account) = form.interest_account
    {
        lines.push(PostingLine {
            account,
            amount: interest,
            memo: memo.clone(),
            contact: None,
            billable: false,
            tag: None,
        });
    }
    lines.extend(extras);
    // The exact invariant: the posted lines are the payment, to the cent
    // (principal + interest + extras = payment by construction; kept as a
    // second gate so nothing is ever sent otherwise).
    if Money::sum(lines.iter().map(|l| l.amount)).ok() != Some(payment) {
        return Err(vec![LoanProblem::PrincipalNegative { by: Money::ZERO }]);
    }
    Ok(CheckedLoanPayment {
        posting,
        loan,
        lender: form.lender,
        pay_from,
        date,
        payment,
        principal,
        interest,
        memo,
        lines,
    })
}

/// The header's problems (accounts, method, date, payment, interest), in
/// field order; returns what the payment posts as.
fn header_problems(form: &LoanPaymentForm, problems: &mut Vec<LoanProblem>) -> LoanPosting {
    if form.loan.is_none() {
        problems.push(LoanProblem::LoanMissing);
    }
    if form.pay_from.is_none() {
        problems.push(LoanProblem::PayFromMissing);
    }
    let posting = match &form.method {
        LoanMethod::Online => LoanPosting::Online,
        LoanMethod::Check(n) => {
            if form.lender.is_none() {
                problems.push(LoanProblem::LenderMissing);
            }
            if let Ok(number) = parse_number(n) {
                LoanPosting::Check(number)
            } else {
                problems.push(LoanProblem::NumberInvalid);
                LoanPosting::Check(None)
            }
        }
    };
    if form.date.is_none() {
        problems.push(LoanProblem::DateMissing);
    }
    if let Some(i) = amount_problem(form.payment) {
        problems.push(LoanProblem::Payment(i));
    }
    match form.interest {
        None => problems.push(LoanProblem::InterestMissing),
        Some(i) if i.is_negative() => problems.push(LoanProblem::InterestNegative),
        Some(i) if i.is_positive() && form.interest_account.is_none() => {
            problems.push(LoanProblem::InterestAccountMissing);
        }
        Some(_) => {}
    }
    let accounts = [form.loan, form.pay_from, form.interest_account];
    let same = accounts
        .iter()
        .enumerate()
        .any(|(i, a)| a.is_some() && accounts[i + 1..].iter().any(|b| b == a));
    if same {
        problems.push(LoanProblem::SameAccount);
    }
    posting
}

/// The engine's balance after the save against the lender's statement
/// (Q-P14q).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LenderMatch {
    /// The same to the cent.
    Matches,
    /// Engine − lender, exact (non-zero).
    Differs(Money),
}

/// Compares the engine's balance owed with the lender's (both as amounts
/// owed); `None` when the difference leaves the money range.
#[must_use]
pub fn lender_match(engine_owed: Money, lender_owed: Money) -> Option<LenderMatch> {
    let d = engine_owed.checked_sub(lender_owed).ok()?;
    Some(if d.is_zero() {
        LenderMatch::Matches
    } else {
        LenderMatch::Differs(d)
    })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::forms::card::tests_support::{account, money};
    use crate::forms::lines::FormLine;
    use crate::rowkey::RowKeyGen;

    fn date() -> CalendarDate {
        CalendarDate::from_ymd(2026, 11, 4).expect("date")
    }

    fn form(payment: &str, interest: &str) -> LoanPaymentForm {
        let mut f = LoanPaymentForm::new(Some(date()));
        f.loan = Some(account(2200));
        f.pay_from = Some(account(1010));
        f.interest_account = Some(account(6900));
        f.payment = Some(money(payment));
        f.interest = Some(money(interest));
        f
    }

    #[test]
    fn the_lender_statement_splits_exactly() {
        // fixtures/sample/ozark_ridge/lender_statement.json
        let f = form("892.00", "207.43");
        assert_eq!(f.principal(), Some(money("684.57")));
        let c = check(&f, &TagList::default()).expect("ok");
        assert_eq!(c.principal, money("684.57"));
        assert_eq!(c.lines.len(), 2);
        assert_eq!(c.lines[0].account, account(2200));
        assert_eq!(c.lines[0].amount, money("684.57"));
        assert_eq!(c.lines[1].account, account(6900));
        assert_eq!(c.lines[1].amount, money("207.43"));
        assert_eq!(c.posting, LoanPosting::Online);
    }

    #[test]
    fn zero_principal_or_interest_posts_one_line() {
        let c = check(&form("207.43", "207.43"), &TagList::default()).expect("interest only");
        assert_eq!(c.lines.len(), 1);
        assert_eq!(c.lines[0].account, account(6900));
        let mut f = form("500", "0");
        f.interest_account = None;
        let c = check(&f, &TagList::default()).expect("principal only");
        assert_eq!(c.lines.len(), 1);
        assert_eq!(c.lines[0].account, account(2200));
    }

    #[test]
    fn problems() {
        let tags = TagList::default();
        let e = check(&form("892.00", "900.00"), &tags).expect_err("neg");
        assert_eq!(
            e,
            vec![LoanProblem::PrincipalNegative { by: money("8.00") }]
        );
        assert_eq!(e[0].name(), "principal_negative");
        assert_eq!(e[0].field(), LoanField::Interest);
        let mut f = LoanPaymentForm::new(None);
        f.method = LoanMethod::Check("abc".into());
        assert_eq!(
            check(&f, &tags).expect_err("blank"),
            vec![
                LoanProblem::LoanMissing,
                LoanProblem::PayFromMissing,
                LoanProblem::LenderMissing,
                LoanProblem::NumberInvalid,
                LoanProblem::DateMissing,
                LoanProblem::Payment(AmountIssue::Missing),
                LoanProblem::InterestMissing,
            ]
        );
        let mut f = form("10", "-1");
        assert!(
            check(&f, &tags)
                .expect_err("neg interest")
                .contains(&LoanProblem::InterestNegative)
        );
        f.interest = Some(money("1"));
        f.interest_account = None;
        assert!(
            check(&f, &tags)
                .expect_err("interest account")
                .contains(&LoanProblem::InterestAccountMissing)
        );
        f.interest_account = f.loan;
        assert!(
            check(&f, &tags)
                .expect_err("same")
                .contains(&LoanProblem::SameAccount)
        );
        let mut f = form("10", "1");
        f.pay_from = f.loan;
        assert!(
            check(&f, &tags)
                .expect_err("same")
                .contains(&LoanProblem::SameAccount)
        );
    }

    #[test]
    fn extras_come_out_of_the_principal() {
        let mut keys = RowKeyGen::new();
        let mut f = form("892.00", "207.43");
        f.extras = FormLines {
            lines: vec![FormLine {
                account: Some(account(6150)),
                amount: Some(money("25.00")),
                ..FormLine::blank(keys.next_key())
            }],
        };
        f.extras.ensure_entry(&mut keys);
        assert_eq!(f.principal(), Some(money("659.57")));
        let c = check(&f, &TagList::default()).expect("ok");
        assert_eq!(c.lines.len(), 3);
        assert_eq!(
            Money::sum(c.lines.iter().map(|l| l.amount)).expect("sum"),
            money("892.00")
        );
        f.extras.lines[0].account = f.pay_from;
        let k = f.extras.lines[0].key;
        assert!(
            check(&f, &TagList::default())
                .expect_err("own")
                .contains(&LoanProblem::Line(k, LineIssue::OwnAccount))
        );
    }

    #[test]
    fn lender_match_is_exact() {
        assert_eq!(
            lender_match(money("30515.43"), money("30515.43")),
            Some(LenderMatch::Matches)
        );
        assert_eq!(
            lender_match(money("30515.44"), money("30515.43")),
            Some(LenderMatch::Differs(money("0.01")))
        );
    }

    #[test]
    fn three_month_statement_ends_exactly() {
        // fixtures/loans/lender_statement_3mo.json (AC3).
        let raw = include_str!("../../../../fixtures/loans/lender_statement_3mo.json");
        let v: serde_json::Value = serde_json::from_str(raw).expect("json");
        let m = |v: &serde_json::Value| money(v.as_str().expect("string amount"));
        let mut balance = m(&v["opening"]);
        for p in v["payments"].as_array().expect("payments") {
            let f = form(
                p["payment"].as_str().expect("s"),
                p["interest"].as_str().expect("s"),
            );
            let c = check(&f, &TagList::default()).expect("ok");
            assert_eq!(c.principal, m(&p["principal"]));
            balance = balance.checked_sub(c.principal).expect("sub");
        }
        assert_eq!(balance, m(&v["closing"]));
        assert_eq!(balance, money("21906.64"));
    }

    #[test]
    fn debug_hides_owner_data() {
        let mut f = form("892.00", "207.43");
        f.memo = "secret".into();
        let s = format!("{f:?} {:?}", check(&f, &TagList::default()).expect("ok"));
        assert!(!s.contains("892") && !s.contains("207") && !s.contains("secret"));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// Σposted lines == payment whenever it is Ok; principal ≥ 0 iff
        /// Ok (everything else on the form valid).
        #[test]
        fn posted_lines_equal_the_payment(
            payment in 1_i64..=100_000_000,
            interest in 0_i64..=100_000_000,
            extras in proptest::collection::vec(1_i64..=1_000_000, 0..3),
        ) {
            let mut keys = RowKeyGen::new();
            let mut f = form("0", "0");
            f.payment = Some(Money::from_cents(payment).expect("m"));
            f.interest = Some(Money::from_cents(interest).expect("m"));
            f.extras = FormLines {
                lines: extras
                    .iter()
                    .enumerate()
                    .map(|(i, c)| FormLine {
                        account: Some(account(7000 + i as u64)),
                        amount: Some(Money::from_cents(*c).expect("m")),
                        ..FormLine::blank(keys.next_key())
                    })
                    .collect(),
            };
            let principal = payment - interest - extras.iter().sum::<i64>();
            let r = check(&f, &TagList::default());
            prop_assert_eq!(r.is_ok(), principal >= 0);
            match r {
                Ok(c) => {
                    let sum = Money::sum(c.lines.iter().map(|l| l.amount)).expect("sum");
                    prop_assert_eq!(sum, c.payment);
                    prop_assert_eq!(c.principal, Money::from_cents(principal).expect("m"));
                    prop_assert!(c.lines.iter().all(|l| l.amount.is_positive()));
                }
                Err(e) => {
                    let by = Money::from_cents(-principal).expect("m");
                    prop_assert_eq!(e, vec![LoanProblem::PrincipalNegative { by }]);
                }
            }
        }
    }
}
