//! "Paid with Personal Funds" (P14 C2.2, Q-85, Q-P14n, Q-P14o): the owner
//! paid a business expense with their own money. It posts as an expense
//! paid from an equity account ("Money I put into the business") or a
//! liability account ("The business will pay me back").
//!
//! Money rules (plan §4): the amount and every line amount are the owner's
//! typed [`Money`]; the lines must add up to the amount exactly.

use std::collections::BTreeMap;
use std::fmt;

use cl_core::ids::{AccountId, ContactId, TagId};
use cl_core::{CalendarDate, Money};

use crate::forms::card::{AmountIssue, LineIssue, MAX_LINES, amount_problem, check_lines};
use crate::forms::lines::{
    FormLines, PostingLine, TotalsStatus, difference, exact_totals, trimmed,
};
use crate::forms::owner_money::{Direction, Entity, EquitySource, Role, pick_equity};
use crate::lists::tags::TagList;
use crate::rowkey::RowKey;
use crate::text::TextKey;

/// How the owner's money is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Treatment {
    /// "Money I put into the business": an equity contribution.
    PutIn,
    /// "The business will pay me back": a liability to the owner.
    PayBack,
}

impl Treatment {
    /// The control channel's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Treatment::PutIn => "put_in",
            Treatment::PayBack => "pay_back",
        }
    }
}

/// The default treatment by entity type (Q-P14n): sole proprietor,
/// single-member LLC and partnership → put in; S and C corporations → pay
/// back. Unknown → put in.
#[must_use]
pub const fn default_treatment(entity: Option<Entity>) -> Treatment {
    match entity {
        Some(Entity::SCorp | Entity::CCorp) => Treatment::PayBack,
        Some(Entity::SoleProp | Entity::SingleMemberLlc | Entity::Partnership) | None => {
            Treatment::PutIn
        }
    }
}

/// The account the expense is paid from to start with: for "put in", the
/// contribution account as Pay Yourself / Put Money In picks it; for "pay
/// back", the remembered liability (only if still usable).
#[must_use]
pub fn pick_offset(
    treatment: Treatment,
    roles: &BTreeMap<Role, AccountId>,
    entity: Option<Entity>,
    remembered: Option<AccountId>,
    usable: &[AccountId],
) -> Option<(AccountId, EquitySource)> {
    match treatment {
        Treatment::PutIn => pick_equity(roles, entity, Direction::PutMoneyIn, remembered, usable),
        Treatment::PayBack => remembered
            .filter(|a| usable.contains(a))
            .map(|a| (a, EquitySource::Remembered)),
    }
}

/// The form's complete editable value. `Debug` is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct PersonalFundsForm {
    /// Who paid (the owner contact; optional).
    pub who: Option<ContactId>,
    /// Paid to (optional).
    pub payee: Option<ContactId>,
    /// Date.
    pub date: Option<CalendarDate>,
    /// Memo.
    pub memo: String,
    /// The amount paid (or the lines total).
    pub amount: Option<Money>,
    /// `amount` was taken from the lines total.
    pub amount_from_lines: bool,
    /// What the money was for.
    pub lines: FormLines,
    /// How it is recorded.
    pub treatment: Treatment,
    /// The equity or liability account it is paid from.
    pub offset: Option<AccountId>,
    /// Header tags.
    pub header_tags: Vec<TagId>,
}

impl fmt::Debug for PersonalFundsForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PersonalFundsForm")
            .field("who", &self.who.is_some())
            .field("payee", &self.payee.is_some())
            .field("date", &self.date.is_some())
            .field("memo", &!self.memo.is_empty())
            .field("amount", &self.amount.is_some())
            .field("amount_from_lines", &self.amount_from_lines)
            .field("lines", &self.lines.lines.len())
            .field("treatment", &self.treatment)
            .field("offset", &self.offset.is_some())
            .field("header_tags", &self.header_tags.len())
            .finish()
    }
}

impl PersonalFundsForm {
    /// A blank form.
    #[must_use]
    pub fn new(date: Option<CalendarDate>, treatment: Treatment) -> PersonalFundsForm {
        PersonalFundsForm {
            who: None,
            payee: None,
            date,
            memo: String::new(),
            amount: None,
            amount_from_lines: false,
            lines: FormLines::default(),
            treatment,
            offset: None,
            header_tags: Vec::new(),
        }
    }

    /// The owner left the lines: a blank amount takes the lines total; an
    /// amount from the lines follows them; a typed one stays.
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
}

/// Why the form can't be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PersonalFundsProblem {
    /// No date.
    DateMissing,
    /// The amount is blank, zero or too large.
    Amount(AmountIssue),
    /// No equity or liability account to record it in.
    OffsetMissing,
    /// The lines don't add up to the amount.
    LinesDisagree {
        /// Amount − lines total, exact.
        difference: Money,
    },
    /// No used line.
    NoLines,
    /// More than [`MAX_LINES`] lines.
    TooManyLines,
    /// One line's issue.
    Line(RowKey, LineIssue),
}

impl PersonalFundsProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            PersonalFundsProblem::DateMissing => TextKey("helper.problem.date_missing"),
            PersonalFundsProblem::Amount(AmountIssue::Missing) => {
                TextKey("helper.problem.amount_missing")
            }
            PersonalFundsProblem::Amount(AmountIssue::NotPositive) => {
                TextKey("helper.problem.amount_not_positive")
            }
            PersonalFundsProblem::Amount(AmountIssue::TooLarge) => {
                TextKey("helper.problem.amount_too_large")
            }
            PersonalFundsProblem::OffsetMissing => TextKey("personal_funds.problem.offset_missing"),
            PersonalFundsProblem::LinesDisagree { .. } => TextKey("helper.problem.lines_disagree"),
            PersonalFundsProblem::NoLines => TextKey("card.problem.no_lines"),
            PersonalFundsProblem::TooManyLines => TextKey("helper.problem.too_many_lines"),
            PersonalFundsProblem::Line(_, i) => i.key(),
        }
    }

    /// The control channel's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            PersonalFundsProblem::DateMissing => "date_missing",
            PersonalFundsProblem::Amount(AmountIssue::Missing) => "amount_missing",
            PersonalFundsProblem::Amount(AmountIssue::NotPositive) => "amount_not_positive",
            PersonalFundsProblem::Amount(AmountIssue::TooLarge) => "amount_too_large",
            PersonalFundsProblem::OffsetMissing => "offset_missing",
            PersonalFundsProblem::LinesDisagree { .. } => "lines_disagree",
            PersonalFundsProblem::NoLines => "no_lines",
            PersonalFundsProblem::TooManyLines => "too_many_lines",
            PersonalFundsProblem::Line(_, i) => i.name(),
        }
    }
}

/// A form that passed [`check`]: an expense paid from `offset`. `Debug` is
/// redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedPersonalFunds {
    /// The equity or liability account it is paid from.
    pub offset: AccountId,
    /// How it is recorded.
    pub treatment: Treatment,
    /// Paid to.
    pub payee: Option<ContactId>,
    /// Who paid.
    pub who: Option<ContactId>,
    /// Date.
    pub date: CalendarDate,
    /// Memo, trimmed.
    pub memo: Option<String>,
    /// The amount: the lines' sum, exactly.
    pub amount: Money,
    /// The lines.
    pub lines: Vec<PostingLine>,
}

impl fmt::Debug for CheckedPersonalFunds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedPersonalFunds")
            .field("treatment", &self.treatment)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

/// Checks the form. The lines must add up to the amount exactly.
///
/// # Errors
/// Every problem found, in field order.
pub fn check(
    form: &PersonalFundsForm,
    tags: &TagList,
) -> Result<CheckedPersonalFunds, Vec<PersonalFundsProblem>> {
    let mut problems = Vec::new();
    if form.date.is_none() {
        problems.push(PersonalFundsProblem::DateMissing);
    }
    if let Some(i) = amount_problem(form.amount) {
        problems.push(PersonalFundsProblem::Amount(i));
    }
    if form.offset.is_none() {
        problems.push(PersonalFundsProblem::OffsetMissing);
    }
    let used = form.lines.used().count();
    if used == 0 {
        problems.push(PersonalFundsProblem::NoLines);
    } else if used > MAX_LINES {
        problems.push(PersonalFundsProblem::TooManyLines);
    }
    let (lines, issues) = check_lines(&form.lines, form.offset, &form.header_tags, tags, true);
    problems.extend(
        issues
            .into_iter()
            .map(|(k, i)| PersonalFundsProblem::Line(k, i)),
    );
    if let Some(header) = form.amount {
        let difference = difference(header, &form.lines);
        if !difference.is_zero() {
            problems.push(PersonalFundsProblem::LinesDisagree { difference });
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    match (form.offset, form.date, form.amount) {
        (Some(offset), Some(date), Some(amount)) => Ok(CheckedPersonalFunds {
            offset,
            treatment: form.treatment,
            payee: form.payee,
            who: form.who,
            date,
            memo: trimmed(&form.memo),
            amount,
            lines,
        }),
        _ => Err(vec![PersonalFundsProblem::Amount(AmountIssue::Missing)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forms::card::tests_support::{account, money};
    use crate::forms::lines::FormLine;
    use crate::rowkey::RowKeyGen;

    fn date() -> CalendarDate {
        CalendarDate::from_ymd(2026, 11, 4).expect("date")
    }

    #[test]
    fn default_treatment_for_all_five_types() {
        let want = [
            (Entity::SoleProp, Treatment::PutIn),
            (Entity::SingleMemberLlc, Treatment::PutIn),
            (Entity::Partnership, Treatment::PutIn),
            (Entity::SCorp, Treatment::PayBack),
            (Entity::CCorp, Treatment::PayBack),
        ];
        for (e, t) in want {
            assert_eq!(default_treatment(Some(e)), t, "{e:?}");
        }
        assert_eq!(default_treatment(None), Treatment::PutIn);
    }

    #[test]
    fn offset_follows_the_treatment() {
        let roles: BTreeMap<Role, AccountId> = [(Role::OwnerContribution, account(3100))].into();
        let usable = [account(3100), account(2400)];
        assert_eq!(
            pick_offset(
                Treatment::PutIn,
                &roles,
                Some(Entity::SingleMemberLlc),
                None,
                &usable
            ),
            Some((account(3100), EquitySource::Engine))
        );
        assert_eq!(
            pick_offset(
                Treatment::PayBack,
                &roles,
                Some(Entity::SCorp),
                Some(account(2400)),
                &usable
            ),
            Some((account(2400), EquitySource::Remembered))
        );
        assert_eq!(
            pick_offset(Treatment::PayBack, &roles, None, None, &usable),
            None
        );
    }

    fn form(amount: &str, parts: &[(u64, &str)]) -> PersonalFundsForm {
        let mut keys = RowKeyGen::new();
        let mut f = PersonalFundsForm::new(Some(date()), Treatment::PutIn);
        f.offset = Some(account(3100));
        f.amount = Some(money(amount));
        f.lines = FormLines {
            lines: parts
                .iter()
                .map(|(a, m)| FormLine {
                    account: Some(account(*a)),
                    amount: Some(money(m)),
                    ..FormLine::blank(keys.next_key())
                })
                .collect(),
        };
        f.lines.ensure_entry(&mut keys);
        f
    }

    #[test]
    fn checks_exactly_and_refuses_the_offset_as_a_line() {
        let tags = TagList::default();
        let c = check(&form("42.18", &[(6250, "42.18")]), &tags).expect("ok");
        assert_eq!(c.offset, account(3100));
        assert_eq!(c.amount, money("42.18"));
        assert_eq!(c.lines[0].amount, money("42.18"));
        let e = check(&form("42.18", &[(6250, "42.17")]), &tags).expect_err("cent");
        assert_eq!(
            e,
            vec![PersonalFundsProblem::LinesDisagree {
                difference: money("0.01")
            }]
        );
        let f = form("1", &[(3100, "1")]);
        let k = f.lines.lines[0].key;
        assert_eq!(
            check(&f, &tags).expect_err("own"),
            vec![PersonalFundsProblem::Line(k, LineIssue::OwnAccount)]
        );
        let mut f = PersonalFundsForm::new(None, Treatment::PayBack);
        f.leave_lines();
        assert_eq!(
            check(&f, &tags).expect_err("blank"),
            vec![
                PersonalFundsProblem::DateMissing,
                PersonalFundsProblem::Amount(AmountIssue::Missing),
                PersonalFundsProblem::OffsetMissing,
                PersonalFundsProblem::NoLines,
            ]
        );
        assert_eq!(PersonalFundsProblem::OffsetMissing.name(), "offset_missing");
    }

    #[test]
    fn blank_amount_takes_the_lines_total() {
        let mut f = form("1", &[(6250, "40.00"), (6400, "2.18")]);
        f.amount = None;
        f.leave_lines();
        assert_eq!(f.amount, Some(money("42.18")));
        assert_eq!(f.totals_status(), Some(TotalsStatus::Balanced));
    }

    #[test]
    fn debug_hides_owner_data() {
        let mut f = form("42.18", &[(6250, "42.18")]);
        f.memo = "secret".into();
        let s = format!("{f:?} {:?}", check(&f, &TagList::default()).expect("ok"));
        assert!(!s.contains("42.18") && !s.contains("secret"));
    }
}
