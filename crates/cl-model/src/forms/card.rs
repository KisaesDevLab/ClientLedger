//! Record Card Charge / Credit (P14 C2.1, Q-60, Q-241): the form's value,
//! the exact lines-versus-amount rule it shares with Write Check
//! ([`crate::forms::lines::exact_totals`]) and [`check`], the one exact gate
//! before anything is sent.
//!
//! A charge posts as an expense paid from the card; a credit (a refund to
//! the card) posts as a deposit into the card (Q-P14i). Money rules (plan
//! §4): the header amount and every line amount are the owner's typed
//! [`Money`]; the lines must add up to the header amount exactly.
//!
//! The line rules here ([`check_lines`]) are shared with Paid with Personal
//! Funds and the Loan Payment helper's extra lines.

use std::fmt;

use cl_core::ids::{AccountId, ContactId, TagId};
use cl_core::{CalendarDate, Money};

use crate::forms::PayeePrefill;
use crate::forms::amount_words::MAX_CHECK_UNITS;
use crate::forms::check::line_tag_default;
use crate::forms::doc_read::{
    Confidence, DocReading, ReadState, ReadTarget, blocks_save, read_lines,
};
use crate::forms::lines::{
    FormLine, FormLines, PostingLine, TotalsStatus, difference, exact_totals, trimmed,
};
use crate::lists::tags::TagList;
use crate::rowkey::{RowKey, RowKeyGen};
use crate::text::TextKey;

/// Most lines one form takes (the engine's `MAX_LINES`).
pub const MAX_LINES: usize = 500;
/// Longest reference number (the engine's 50 characters).
pub const REF_MAX: usize = 50;
/// Largest header amount a P14 helper takes (999,999,999,999.99, as on a
/// check), in ten-thousandths.
pub const MAX_AMOUNT_UNITS: i128 = MAX_CHECK_UNITS;

/// Charge (money spent on the card) or credit (money back onto it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CardMode {
    /// A purchase: the card's balance owed goes up.
    Charge,
    /// A refund or credit: the card's balance owed goes down.
    Credit,
}

/// Why one line can't be posted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineIssue {
    /// A used line has no account.
    AccountMissing,
    /// A used line's amount is blank, zero or less.
    AmountNotPositive,
    /// A billable line has no customer.
    BillableNeedsCustomer,
    /// The line is billable where that isn't possible (a card credit).
    BillableNotAllowed,
    /// The line goes to the form's own account (the card, the bank).
    OwnAccount,
}

impl LineIssue {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            LineIssue::AccountMissing => TextKey("check.problem.line_account_missing"),
            LineIssue::AmountNotPositive => TextKey("check.problem.line_amount_not_positive"),
            LineIssue::BillableNeedsCustomer => TextKey("check.problem.billable_needs_customer"),
            LineIssue::BillableNotAllowed => TextKey("card.problem.billable_on_credit"),
            LineIssue::OwnAccount => TextKey("card.problem.line_own_account"),
        }
    }

    /// The control channel's name (no amounts).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            LineIssue::AccountMissing => "line_account_missing",
            LineIssue::AmountNotPositive => "line_amount_not_positive",
            LineIssue::BillableNeedsCustomer => "billable_needs_customer",
            LineIssue::BillableNotAllowed => "billable_not_allowed",
            LineIssue::OwnAccount => "line_own_account",
        }
    }
}

/// The used lines as posting lines (each with its effective tag, Q-178),
/// and every line issue in line order. `own` is the form's own account (a
/// line may not go there); `billable` is whether billable lines are allowed.
#[must_use]
pub fn check_lines(
    lines: &FormLines,
    own: Option<AccountId>,
    header_tags: &[TagId],
    tags: &TagList,
    billable: bool,
) -> (Vec<PostingLine>, Vec<(RowKey, LineIssue)>) {
    let mut out = Vec::new();
    let mut issues = Vec::new();
    for l in lines.used() {
        let amount = l.amount.filter(|m| m.is_positive());
        if l.account.is_none() {
            issues.push((l.key, LineIssue::AccountMissing));
        } else if l.account.is_some() && l.account == own {
            issues.push((l.key, LineIssue::OwnAccount));
        }
        if amount.is_none() {
            issues.push((l.key, LineIssue::AmountNotPositive));
        }
        if l.billable && !billable {
            issues.push((l.key, LineIssue::BillableNotAllowed));
        } else if l.billable && l.customer.is_none() {
            issues.push((l.key, LineIssue::BillableNeedsCustomer));
        }
        if let (Some(account), Some(amount)) = (l.account, amount) {
            out.push(PostingLine {
                account,
                amount,
                memo: trimmed(&l.memo),
                contact: l.customer,
                billable: l.billable && billable,
                tag: line_tag_default(header_tags, l.tag, tags),
            });
        }
    }
    (out, issues)
}

/// The header amount's problem, if any.
pub(crate) fn amount_problem(amount: Option<Money>) -> Option<AmountIssue> {
    match amount {
        None => Some(AmountIssue::Missing),
        Some(m) if !m.is_positive() => Some(AmountIssue::NotPositive),
        Some(m) if m.to_ten_thousandths() > MAX_AMOUNT_UNITS => Some(AmountIssue::TooLarge),
        Some(_) => None,
    }
}

/// What is wrong with a header amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AmountIssue {
    /// Blank.
    Missing,
    /// Zero or less.
    NotPositive,
    /// More than 999,999,999,999.99.
    TooLarge,
}

/// The Record Card Charge / Credit form's complete editable value. `Debug`
/// shows which fields are filled, never their contents.
#[derive(Clone, PartialEq, Eq)]
pub struct CardForm {
    /// The credit card account.
    pub card: Option<AccountId>,
    /// Charge or credit.
    pub mode: CardMode,
    /// Purchased from (optional).
    pub payee: Option<ContactId>,
    /// Date.
    pub date: Option<CalendarDate>,
    /// Reference number (receipt, confirmation).
    pub ref_no: String,
    /// The amount the owner typed (or the lines total, see
    /// [`CardForm::leave_lines`]).
    pub amount: Option<Money>,
    /// `amount` was taken from the lines total, not typed.
    pub amount_from_lines: bool,
    /// Memo.
    pub memo: String,
    /// Header tags.
    pub header_tags: Vec<TagId>,
    /// The lines (where the money went, or came back from).
    pub lines: FormLines,
    /// A04 draft mode: which fields came from the document and whether the
    /// owner accepted them (empty outside draft mode).
    pub read: ReadState,
}

impl fmt::Debug for CardForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CardForm")
            .field("card", &self.card.is_some())
            .field("mode", &self.mode)
            .field("payee", &self.payee.is_some())
            .field("date", &self.date.is_some())
            .field("ref_no", &!self.ref_no.is_empty())
            .field("amount", &self.amount.is_some())
            .field("amount_from_lines", &self.amount_from_lines)
            .field("memo", &!self.memo.is_empty())
            .field("header_tags", &self.header_tags.len())
            .field("lines", &self.lines.lines.len())
            .field("read", &self.read.unaccepted().len())
            .finish()
    }
}

impl CardForm {
    /// A blank charge on `card`, dated `date`, with no lines (the grid
    /// holds them).
    #[must_use]
    pub fn new(card: Option<AccountId>, date: Option<CalendarDate>) -> CardForm {
        CardForm {
            card,
            mode: CardMode::Charge,
            payee: None,
            date,
            ref_no: String::new(),
            amount: None,
            amount_from_lines: false,
            memo: String::new(),
            header_tags: Vec::new(),
            lines: FormLines::default(),
            read: ReadState::default(),
        }
    }

    /// The owner typed the amount: one used line with a blank amount (or
    /// the previous amount) follows it (Q-P13r a, as on a check).
    pub fn set_amount(&mut self, amount: Option<Money>) {
        let previous = self.amount;
        self.amount = amount;
        self.amount_from_lines = false;
        let mut used = self.lines.used_mut();
        if let (Some(line), None) = (used.next(), used.next())
            && (line.amount.is_none() || line.amount == previous)
        {
            line.amount = amount;
        }
    }

    /// The owner left the lines (Q-P13r b): a blank amount takes the lines
    /// total; an amount from the lines follows them; a typed one stays.
    pub fn leave_lines(&mut self) {
        if self.amount.is_some() && !self.amount_from_lines {
            return;
        }
        if self.lines.lines.iter().any(|l| l.amount.is_some()) {
            if let Some(total) = crate::forms::lines::lines_total(&self.lines) {
                self.amount = Some(total);
                self.amount_from_lines = true;
            }
        } else if self.amount_from_lines {
            self.amount = None;
            self.amount_from_lines = false;
        }
    }

    /// The footer (tri-state).
    #[must_use]
    pub fn totals_status(&self) -> Option<TotalsStatus> {
        exact_totals(self.amount, &self.lines)
    }

    /// Prefill (Q-191) may fill the form: no line used and no memo typed.
    #[must_use]
    pub fn can_prefill(&self) -> bool {
        self.lines.used().next().is_none() && self.memo.trim().is_empty()
    }

    /// The vendor's default expense account and tag (Q-192, Q-178) fill the
    /// first line only when no line is used; a typed amount follows onto
    /// that single line (as on a check). Returns true when it filled.
    pub fn apply_vendor_default(
        &mut self,
        account: AccountId,
        tag: Option<TagId>,
        keys: &mut RowKeyGen,
    ) -> bool {
        if self.lines.used().next().is_some() {
            return false;
        }
        if self.lines.lines.is_empty() {
            self.lines.lines.push(FormLine::blank(keys.next_key()));
        }
        let Some(first) = self.lines.lines.first_mut() else {
            return false;
        };
        first.account = Some(account);
        first.tag = tag;
        first.amount = self.amount;
        self.lines.ensure_entry(keys);
        true
    }
}

/// Copies the payee's last charge (Q-191): line accounts, memos,
/// customers, billable marks and tags, plus the memo and header tags. Every
/// line amount is cleared and the amount is not touched (non-negotiable 4).
/// `last` must have been built with this form's [`RowKeyGen`].
impl PayeePrefill for CardForm {
    fn prefill_from_last(&mut self, last: &CardForm) {
        let entry = self.lines.lines.last().filter(|l| l.is_blank()).cloned();
        let billable = self.mode == CardMode::Charge;
        let mut lines: Vec<FormLine> = last
            .lines
            .lines
            .iter()
            .map(|l| FormLine {
                amount: None,
                billable: l.billable && billable,
                ..l.clone()
            })
            .filter(|l| !l.is_blank())
            .collect();
        lines.extend(entry);
        self.lines = FormLines { lines };
        self.memo.clone_from(&last.memo);
        self.header_tags.clone_from(&last.header_tags);
    }
}

/// The field a problem belongs to (the screen focuses it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CardField {
    /// Card.
    Card,
    /// Date.
    Date,
    /// Ref no.
    RefNo,
    /// Amount.
    Amount,
    /// The lines as a whole.
    Lines,
    /// One line.
    Line(RowKey),
    /// The banner of values read from the document (A04 draft mode): its
    /// Accept button.
    ReadBanner,
}

/// Why a card charge or credit can't be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CardProblem {
    /// No card account.
    CardMissing,
    /// No date.
    DateMissing,
    /// The reference number is longer than [`REF_MAX`] characters.
    RefTooLong,
    /// The header amount is blank, zero or too large.
    Amount(AmountIssue),
    /// The lines don't add up to the amount: amount − lines, exact.
    LinesDisagree {
        /// Amount − lines total, exact (negative when the lines are more).
        difference: Money,
    },
    /// No used line.
    NoLines,
    /// More than [`MAX_LINES`] lines.
    TooManyLines,
    /// One line's issue.
    Line(RowKey, LineIssue),
    /// A04 draft mode: values read from the document are not accepted yet
    /// (Accept, Alt+A, or edit them).
    ReadNotAccepted,
}

impl CardProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            CardProblem::CardMissing => TextKey("card.problem.card_missing"),
            CardProblem::DateMissing => TextKey("helper.problem.date_missing"),
            CardProblem::RefTooLong => TextKey("card.problem.ref_too_long"),
            CardProblem::Amount(AmountIssue::Missing) => TextKey("helper.problem.amount_missing"),
            CardProblem::Amount(AmountIssue::NotPositive) => {
                TextKey("helper.problem.amount_not_positive")
            }
            CardProblem::Amount(AmountIssue::TooLarge) => {
                TextKey("helper.problem.amount_too_large")
            }
            CardProblem::LinesDisagree { .. } => TextKey("helper.problem.lines_disagree"),
            CardProblem::NoLines => TextKey("card.problem.no_lines"),
            CardProblem::TooManyLines => TextKey("helper.problem.too_many_lines"),
            CardProblem::Line(_, i) => i.key(),
            CardProblem::ReadNotAccepted => TextKey("card.problem.read_not_accepted"),
        }
    }

    /// The control channel's name (no amounts).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            CardProblem::CardMissing => "card_missing",
            CardProblem::DateMissing => "date_missing",
            CardProblem::RefTooLong => "ref_too_long",
            CardProblem::Amount(AmountIssue::Missing) => "amount_missing",
            CardProblem::Amount(AmountIssue::NotPositive) => "amount_not_positive",
            CardProblem::Amount(AmountIssue::TooLarge) => "amount_too_large",
            CardProblem::LinesDisagree { .. } => "lines_disagree",
            CardProblem::NoLines => "no_lines",
            CardProblem::TooManyLines => "too_many_lines",
            CardProblem::Line(_, i) => i.name(),
            CardProblem::ReadNotAccepted => "read_not_accepted",
        }
    }

    /// The field to focus.
    #[must_use]
    pub const fn field(self) -> CardField {
        match self {
            CardProblem::CardMissing => CardField::Card,
            CardProblem::DateMissing => CardField::Date,
            CardProblem::RefTooLong => CardField::RefNo,
            CardProblem::Amount(_) => CardField::Amount,
            CardProblem::LinesDisagree { .. }
            | CardProblem::NoLines
            | CardProblem::TooManyLines => CardField::Lines,
            CardProblem::Line(k, _) => CardField::Line(k),
            CardProblem::ReadNotAccepted => CardField::ReadBanner,
        }
    }
}

/// A card charge or credit that passed [`check`], ready to map field by
/// field onto the engine draft. `Debug` is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedCard {
    /// The card.
    pub card: AccountId,
    /// Charge or credit.
    pub mode: CardMode,
    /// Purchased from.
    pub payee: Option<ContactId>,
    /// Date.
    pub date: CalendarDate,
    /// Reference number, trimmed.
    pub ref_no: Option<String>,
    /// The amount: more than zero, equal to the lines' sum exactly.
    pub amount: Money,
    /// Memo, trimmed.
    pub memo: Option<String>,
    /// Header tags.
    pub header_tags: Vec<TagId>,
    /// The lines.
    pub lines: Vec<PostingLine>,
}

impl fmt::Debug for CheckedCard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedCard")
            .field("mode", &self.mode)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

/// Checks the form. The lines must add up to the amount exactly; a credit
/// has no billable lines.
///
/// # Errors
/// Every problem found, in field order.
pub fn check(form: &CardForm, tags: &TagList) -> Result<CheckedCard, Vec<CardProblem>> {
    let mut problems = Vec::new();
    if form.card.is_none() {
        problems.push(CardProblem::CardMissing);
    }
    if form.date.is_none() {
        problems.push(CardProblem::DateMissing);
    }
    if form.ref_no.trim().chars().count() > REF_MAX {
        problems.push(CardProblem::RefTooLong);
    }
    if let Some(i) = amount_problem(form.amount) {
        problems.push(CardProblem::Amount(i));
    }
    let used = form.lines.used().count();
    if used == 0 {
        problems.push(CardProblem::NoLines);
    } else if used > MAX_LINES {
        problems.push(CardProblem::TooManyLines);
    }
    let billable = form.mode == CardMode::Charge;
    let (lines, issues) = check_lines(&form.lines, form.card, &form.header_tags, tags, billable);
    problems.extend(issues.into_iter().map(|(k, i)| CardProblem::Line(k, i)));
    if let Some(header) = form.amount {
        let difference = difference(header, &form.lines);
        if !difference.is_zero() {
            problems.push(CardProblem::LinesDisagree { difference });
        }
    }
    if blocks_save(&form.read, &form.lines) {
        problems.push(CardProblem::ReadNotAccepted);
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    match (form.card, form.date, form.amount) {
        (Some(card), Some(date), Some(amount)) => Ok(CheckedCard {
            card,
            mode: form.mode,
            payee: form.payee,
            date,
            ref_no: trimmed(&form.ref_no),
            amount,
            memo: trimmed(&form.memo),
            header_tags: form.header_tags.clone(),
            lines,
        }),
        // Every `None` above pushed a problem.
        _ => Err(vec![CardProblem::Amount(AmountIssue::Missing)]),
    }
}

/// A04 draft mode: fills the empty fields of a card charge from `reading`
/// and marks each one *read, not accepted* (the bill's
/// [`crate::forms::doc_read::apply`] rules). Payee from `vendor`, date,
/// amount from the printed `total`, the card from `paid_from`, and the
/// lines when no line is used. The screen opens a draft on a form built
/// with no date and no card, so the document's own fill them. Fields the
/// owner already filled are left alone; nothing is computed. Returns the
/// fields filled.
pub fn apply(reading: &DocReading, form: &mut CardForm, keys: &mut RowKeyGen) -> Vec<ReadTarget> {
    let mut filled = Vec::new();
    let level = |c: Option<Confidence>| c.unwrap_or(Confidence::Unrated);
    let conf = reading.confidence;
    if form.card.is_none()
        && let Some(a) = reading.paid_from
    {
        form.card = Some(a);
        form.read
            .mark_read(ReadTarget::PaidFrom, level(conf.payment));
        filled.push(ReadTarget::PaidFrom);
    }
    if form.payee.is_none()
        && let Some(v) = reading.vendor
    {
        form.payee = Some(v);
        form.read.mark_read(ReadTarget::Payee, level(conf.vendor));
        filled.push(ReadTarget::Payee);
    }
    if form.date.is_none()
        && let Some(d) = reading.bill_date
    {
        form.date = Some(d);
        form.read.mark_read(ReadTarget::Date, level(conf.bill_date));
        filled.push(ReadTarget::Date);
    }
    if form.amount.is_none()
        && let Some(t) = reading.total
    {
        form.amount = Some(t);
        form.amount_from_lines = false;
        form.read.mark_read(ReadTarget::Total, level(conf.total));
        filled.push(ReadTarget::Total);
    }
    if form.lines.used().next().is_none()
        && let Some(v) = read_lines(reading, keys, &mut form.read, &mut filled)
    {
        form.lines = v;
    }
    filled
}

/// Helpers the P14 form tests share.
#[cfg(test)]
pub(crate) mod tests_support {
    use cl_core::Money;
    use cl_core::ids::AccountId;

    pub(crate) fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    pub(crate) fn account(n: u64) -> AccountId {
        AccountId::parse(&format!("00000000-0000-0000-0000-{n:012}")).expect("id")
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::tests_support::{account, money};
    use super::*;
    use crate::forms::lines::FormLine;
    use crate::rowkey::RowKeyGen;

    fn contact(n: u64) -> ContactId {
        ContactId::parse(&format!("00000000-0000-0000-0000-{n:012}")).expect("id")
    }

    fn date() -> CalendarDate {
        CalendarDate::from_ymd(2026, 11, 4).expect("date")
    }

    fn lines(keys: &mut RowKeyGen, parts: &[(u64, &str)]) -> FormLines {
        let mut v = FormLines {
            lines: parts
                .iter()
                .map(|(a, m)| FormLine {
                    account: Some(account(*a)),
                    amount: Some(money(m)),
                    ..FormLine::blank(keys.next_key())
                })
                .collect(),
        };
        v.ensure_entry(keys);
        v
    }

    fn charge(amount: &str, parts: &[(u64, &str)]) -> CardForm {
        let mut keys = RowKeyGen::new();
        let mut f = CardForm::new(Some(account(2050)), Some(date()));
        f.amount = Some(money(amount));
        f.lines = lines(&mut keys, parts);
        f
    }

    #[test]
    fn prefill_copies_the_last_charge_without_any_amount() {
        let mut keys = RowKeyGen::new();
        let mut last = CardForm::new(Some(account(2050)), Some(date()));
        last.memo = "monthly".into();
        last.header_tags = Vec::new();
        last.lines = lines(&mut keys, &[(6250, "40.00"), (6400, "24.10")]);
        last.lines.lines[0].billable = true;
        last.lines.lines[0].customer = Some(contact(7));
        let mut f = CardForm::new(Some(account(2050)), Some(date()));
        f.amount = Some(money("99.00"));
        f.lines.ensure_entry(&mut keys);
        assert!(f.can_prefill());
        f.prefill_from_last(&last);
        assert_eq!(
            f.amount,
            Some(money("99.00")),
            "the amount is never touched"
        );
        assert_eq!(f.memo, "monthly");
        let used: Vec<_> = f.lines.used().collect();
        assert_eq!(used.len(), 2);
        assert!(used.iter().all(|l| l.amount.is_none()), "no amount copied");
        assert!(used[0].billable && used[0].customer == Some(contact(7)));
        assert!(!f.can_prefill(), "a filled form is not prefilled again");
        // A credit takes no billable marks.
        let mut credit = CardForm::new(Some(account(2050)), Some(date()));
        credit.mode = CardMode::Credit;
        credit.prefill_from_last(&last);
        assert!(credit.lines.used().all(|l| !l.billable));
    }

    #[test]
    fn the_vendor_default_fills_only_an_untouched_first_line() {
        let mut keys = RowKeyGen::new();
        let mut f = CardForm::new(Some(account(2050)), Some(date()));
        f.amount = Some(money("12.00"));
        assert!(f.apply_vendor_default(account(6250), None, &mut keys));
        let first = f.lines.used().next().expect("line");
        assert_eq!(first.account, Some(account(6250)));
        assert_eq!(
            first.amount,
            Some(money("12.00")),
            "the typed amount follows"
        );
        assert!(!f.apply_vendor_default(account(6400), None, &mut keys));
    }

    #[test]
    fn a_split_charge_must_add_up_to_the_cent() {
        let tags = TagList::default();
        let f = charge("64.20", &[(6250, "40.00"), (6400, "24.10")]);
        assert_eq!(
            f.totals_status(),
            Some(TotalsStatus::Remaining(money("0.10")))
        );
        let e = check(&f, &tags).expect_err("0.10 left");
        assert_eq!(
            e,
            vec![CardProblem::LinesDisagree {
                difference: money("0.10")
            }]
        );
        assert_eq!(e[0].name(), "lines_disagree");
        assert_eq!(e[0].field(), CardField::Lines);
        let f = charge("64.10", &[(6250, "40.00"), (6400, "24.10")]);
        let c = check(&f, &tags).expect("balanced");
        assert_eq!(c.amount, money("64.10"));
        assert_eq!(c.lines.len(), 2);
        assert_eq!(c.mode, CardMode::Charge);
    }

    #[test]
    fn header_and_line_problems_in_field_order() {
        let tags = TagList::default();
        let mut keys = RowKeyGen::new();
        let mut f = CardForm::new(None, None);
        f.ref_no = "x".repeat(51);
        assert_eq!(
            check(&f, &tags).expect_err("blank"),
            vec![
                CardProblem::CardMissing,
                CardProblem::DateMissing,
                CardProblem::RefTooLong,
                CardProblem::Amount(AmountIssue::Missing),
                CardProblem::NoLines,
            ]
        );
        f = charge("10", &[(2050, "10")]);
        let k = f.lines.lines[0].key;
        assert_eq!(
            check(&f, &tags).expect_err("own"),
            vec![CardProblem::Line(k, LineIssue::OwnAccount)]
        );
        f.lines = lines(&mut keys, &[(6250, "0")]);
        f.lines.lines[0].billable = true;
        let k = f.lines.lines[0].key;
        let e = check(&f, &tags).expect_err("zero");
        assert!(e.contains(&CardProblem::Line(k, LineIssue::AmountNotPositive)));
        assert!(e.contains(&CardProblem::Line(k, LineIssue::BillableNeedsCustomer)));
        f.amount = Some(money("-1"));
        assert!(
            check(&f, &tags)
                .expect_err("neg")
                .contains(&CardProblem::Amount(AmountIssue::NotPositive))
        );
        f.amount = Some(money("1000000000000"));
        assert!(
            check(&f, &tags)
                .expect_err("big")
                .contains(&CardProblem::Amount(AmountIssue::TooLarge))
        );
        f.lines.lines[0].account = None;
        assert!(
            check(&f, &tags)
                .expect_err("no account")
                .contains(&CardProblem::Line(k, LineIssue::AccountMissing))
        );
    }

    #[test]
    fn a_credit_has_no_billable_lines_and_keeps_customers() {
        let tags = TagList::default();
        let mut f = charge("15.00", &[(6250, "15.00")]);
        f.mode = CardMode::Credit;
        f.lines.lines[0].customer = Some(contact(9));
        f.lines.lines[0].billable = true;
        let k = f.lines.lines[0].key;
        assert_eq!(
            check(&f, &tags).expect_err("billable"),
            vec![CardProblem::Line(k, LineIssue::BillableNotAllowed)]
        );
        f.lines.lines[0].billable = false;
        let c = check(&f, &tags).expect("credit");
        assert_eq!(c.mode, CardMode::Credit);
        assert_eq!(c.lines[0].contact, Some(contact(9)));
        assert!(!c.lines[0].billable);
        // A charge keeps the customer and the billable mark (Q-P14j).
        f.mode = CardMode::Charge;
        f.lines.lines[0].billable = true;
        let c = check(&f, &tags).expect("charge");
        assert!(c.lines[0].billable);
    }

    #[test]
    fn amount_follows_one_line_and_lines_fill_a_blank_amount() {
        let mut keys = RowKeyGen::new();
        let mut f = CardForm::new(Some(account(2050)), Some(date()));
        f.lines = lines(&mut keys, &[(6250, "5.00")]);
        f.leave_lines();
        assert_eq!(f.amount, Some(money("5.00")));
        assert!(f.amount_from_lines);
        f.set_amount(Some(money("7.25")));
        assert_eq!(f.lines.lines[0].amount, Some(money("7.25")));
        assert!(!f.amount_from_lines);
        f.lines.lines[0].amount = Some(money("1"));
        f.leave_lines();
        assert_eq!(f.amount, Some(money("7.25")), "a typed amount stays");
    }

    #[test]
    fn a_draft_fills_empty_fields_and_blocks_save_until_accepted() {
        use crate::forms::doc_read::{Confidence, DocReading, ReadField, ReadLine, ReadTarget};
        let tags = TagList::default();
        let mut keys = RowKeyGen::new();
        let reading = DocReading {
            vendor: Some(contact(5)),
            bill_date: Some(date()),
            total: Some(money("64.10")),
            paid_from: Some(account(2050)),
            lines: vec![
                ReadLine {
                    description: "Paper".into(),
                    amount: Some(money("40.00")),
                    account: Some(account(6250)),
                    confidence: Some(Confidence::High),
                },
                ReadLine {
                    description: "Ink".into(),
                    amount: Some(money("24.10")),
                    account: Some(account(6400)),
                    confidence: None,
                },
            ],
            ..DocReading::default()
        };
        let mut f = CardForm::new(None, None);
        let filled = apply(&reading, &mut f, &mut keys);
        assert_eq!(filled.len(), 6);
        assert_eq!(f.card, Some(account(2050)));
        assert_eq!(f.payee, Some(contact(5)));
        assert_eq!(f.amount, Some(money("64.10")));
        assert_eq!(f.lines.used().count(), 2);
        assert_eq!(
            f.read.state(ReadTarget::Total),
            Some(ReadField::Read {
                level: Confidence::Unrated
            })
        );
        assert_eq!(
            check(&f, &tags).expect_err("not accepted"),
            vec![CardProblem::ReadNotAccepted]
        );
        assert_eq!(CardProblem::ReadNotAccepted.name(), "read_not_accepted");
        assert_eq!(CardProblem::ReadNotAccepted.field(), CardField::ReadBanner);
        f.read.accept_all();
        let c = check(&f, &tags).expect("accepted");
        assert_eq!(c.amount, money("64.10"));
        // A deleted read line no longer blocks Save.
        let mut g = CardForm::new(None, None);
        apply(&reading, &mut g, &mut keys);
        for t in [
            ReadTarget::PaidFrom,
            ReadTarget::Payee,
            ReadTarget::Date,
            ReadTarget::Total,
        ] {
            g.read.edited(t);
        }
        let first = g.lines.lines[0].key;
        assert!(check(&g, &tags).is_err());
        g.lines.lines.retain(|l| l.key == first || l.is_blank());
        g.read.edited(ReadTarget::Line(first));
        g.amount = Some(money("40.00"));
        assert!(check(&g, &tags).is_ok());
        // What the owner typed stays.
        let mut typed = charge("9.00", &[(6250, "9.00")]);
        typed.payee = Some(contact(8));
        let filled = apply(&reading, &mut typed, &mut keys);
        assert!(filled.is_empty(), "{filled:?}");
        assert_eq!(typed.amount, Some(money("9.00")));
        assert!(check(&typed, &tags).is_ok());
    }

    #[test]
    fn debug_hides_owner_data() {
        let mut f = charge("1286.40", &[(6250, "1286.40")]);
        f.memo = "secret memo".into();
        f.ref_no = "R-7712".into();
        let s = format!("{f:?}");
        assert!(!s.contains("1286") && !s.contains("secret") && !s.contains("7712"));
        let c = check(&f, &TagList::default()).expect("ok");
        let s = format!("{c:?}");
        assert!(!s.contains("1286") && !s.contains("secret") && !s.contains("7712"));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// Ok iff the typed lines equal the amount exactly; the checked
        /// lines then sum to the amount.
        #[test]
        fn lines_equal_amount_iff_ok(
            parts in proptest::collection::vec(1_i64..=10_000_000, 1..6),
            delta in -3_i64..=3,
        ) {
            let mut keys = RowKeyGen::new();
            let mut f = CardForm::new(Some(account(2050)), Some(date()));
            f.lines = FormLines {
                lines: parts
                    .iter()
                    .enumerate()
                    .map(|(i, c)| FormLine {
                        account: Some(account(6000 + i as u64)),
                        amount: Some(Money::from_cents(*c).expect("m")),
                        ..FormLine::blank(keys.next_key())
                    })
                    .collect(),
            };
            let sum: i64 = parts.iter().sum();
            f.amount = Some(Money::from_cents(sum + delta).expect("m"));
            let r = check(&f, &TagList::default());
            prop_assert_eq!(r.is_ok(), delta == 0);
            if let Ok(c) = r {
                let total = Money::sum(c.lines.iter().map(|l| l.amount)).expect("sum");
                prop_assert_eq!(total, c.amount);
            } else {
                let e = r.expect_err("err");
                let want = Money::from_cents(delta).expect("m");
                prop_assert_eq!(e, vec![CardProblem::LinesDisagree { difference: want }]);
            }
        }
    }
}
