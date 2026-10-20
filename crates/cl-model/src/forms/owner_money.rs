//! "Pay Yourself or Put Money In" (P14 C2.1, Q-61, Q-P14l, Q-P14m): the
//! form's value, the equity account by entity type, and [`check`].
//!
//! Paying yourself posts a check, or an online payment from the bank, with
//! one line to the draw account; putting money in posts a deposit with one
//! line from the contribution account. The amount is the owner's typed
//! [`Money`], sent as is. No tax advice: the screen only names the account
//! and says "Ask your accountant if you're not sure which to use."

use std::collections::BTreeMap;
use std::fmt;

use cl_core::ids::{AccountId, ContactId};
use cl_core::{CalendarDate, Money};

use crate::forms::card::{AmountIssue, amount_problem};
use crate::forms::check::parse_number;
use crate::forms::lines::trimmed;
use crate::text::TextKey;

/// The legal form of the business, as far as the owner-money helpers care
/// (mirrors the engine's `entityType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Entity {
    /// Sole proprietorship.
    SoleProp,
    /// Single-member LLC.
    SingleMemberLlc,
    /// Partnership (or multi-member LLC).
    Partnership,
    /// S corporation.
    SCorp,
    /// C corporation.
    CCorp,
}

impl Entity {
    /// Every entity type.
    pub const ALL: [Entity; 5] = [
        Entity::SoleProp,
        Entity::SingleMemberLlc,
        Entity::Partnership,
        Entity::SCorp,
        Entity::CCorp,
    ];

    /// The control channel's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Entity::SoleProp => "sole_prop",
            Entity::SingleMemberLlc => "single_member_llc",
            Entity::Partnership => "partnership",
            Entity::SCorp => "s_corp",
            Entity::CCorp => "c_corp",
        }
    }

    /// The neutral sentence the screen shows for this entity type (Q-61;
    /// no tax advice).
    #[must_use]
    pub const fn note(self) -> TextKey {
        match self {
            Entity::SoleProp | Entity::SingleMemberLlc => TextKey("owner_money.note.owner"),
            Entity::Partnership => TextKey("owner_money.note.partnership"),
            Entity::SCorp => TextKey("owner_money.note.s_corp"),
            Entity::CCorp => TextKey("owner_money.note.c_corp"),
        }
    }
}

/// Which way the money goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Money from the business to the owner.
    PayYourself,
    /// Money from the owner into the business.
    PutMoneyIn,
}

/// An equity role the engine keeps an account for (`equity-roles-v1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    /// Owner's draw.
    OwnerDraw,
    /// Owner's contribution.
    OwnerContribution,
    /// Partner draws.
    PartnerDraws,
    /// Partner contributions.
    PartnerContributions,
    /// Shareholder distributions.
    ShareholderDistributions,
    /// Paid-in capital.
    PaidInCapital,
}

/// The equity role money goes to by entity type and direction (Q-P14l):
/// sole proprietor and single-member LLC → owner draw / contribution;
/// partnership → partner draws / contributions; S and C corporations →
/// shareholder distributions / paid-in capital.
#[must_use]
pub const fn default_role(entity: Entity, direction: Direction) -> Role {
    match (entity, direction) {
        (Entity::SoleProp | Entity::SingleMemberLlc, Direction::PayYourself) => Role::OwnerDraw,
        (Entity::SoleProp | Entity::SingleMemberLlc, Direction::PutMoneyIn) => {
            Role::OwnerContribution
        }
        (Entity::Partnership, Direction::PayYourself) => Role::PartnerDraws,
        (Entity::Partnership, Direction::PutMoneyIn) => Role::PartnerContributions,
        (Entity::SCorp | Entity::CCorp, Direction::PayYourself) => Role::ShareholderDistributions,
        (Entity::SCorp | Entity::CCorp, Direction::PutMoneyIn) => Role::PaidInCapital,
    }
}

/// Where the equity account on the form came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EquitySource {
    /// The engine's role account for the entity type.
    Engine,
    /// The owner's earlier choice in this company (Q-P14m).
    Remembered,
}

/// The equity account to start the form on: the engine's role account for
/// the entity type when there is one, else the owner's remembered choice
/// (only if it is still one of `usable`), else none (the owner picks once).
#[must_use]
pub fn pick_equity(
    roles: &BTreeMap<Role, AccountId>,
    entity: Option<Entity>,
    direction: Direction,
    remembered: Option<AccountId>,
    usable: &[AccountId],
) -> Option<(AccountId, EquitySource)> {
    let engine = entity
        .and_then(|e| roles.get(&default_role(e, direction)).copied())
        .filter(|a| usable.contains(a));
    engine.map(|a| (a, EquitySource::Engine)).or_else(|| {
        remembered
            .filter(|a| usable.contains(a))
            .map(|a| (a, EquitySource::Remembered))
    })
}

/// How the money is paid out (or comes in).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerMethod {
    /// A check with this number (may be empty: no number).
    Check(String),
    /// A transfer, online or automatic payment.
    Electronic,
}

/// The form's complete editable value. `Debug` is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct OwnerMoneyForm {
    /// Pay yourself or put money in.
    pub direction: Direction,
    /// Who (the owner, partner or shareholder contact).
    pub who: Option<ContactId>,
    /// The name on a check when `who` has none (the contact's display name
    /// is used otherwise).
    pub who_name: String,
    /// The bank account.
    pub bank: Option<AccountId>,
    /// Paid by.
    pub method: OwnerMethod,
    /// Date.
    pub date: Option<CalendarDate>,
    /// The amount the owner typed.
    pub amount: Option<Money>,
    /// Memo.
    pub memo: String,
    /// "Recorded in": the equity account.
    pub equity: Option<AccountId>,
}

impl fmt::Debug for OwnerMoneyForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnerMoneyForm")
            .field("direction", &self.direction)
            .field("who", &self.who.is_some())
            .field("who_name", &!self.who_name.is_empty())
            .field("bank", &self.bank.is_some())
            .field("check", &matches!(self.method, OwnerMethod::Check(_)))
            .field("date", &self.date.is_some())
            .field("amount", &self.amount.is_some())
            .field("memo", &!self.memo.is_empty())
            .field("equity", &self.equity.is_some())
            .finish()
    }
}

impl OwnerMoneyForm {
    /// A blank form.
    #[must_use]
    pub fn new(
        direction: Direction,
        bank: Option<AccountId>,
        date: Option<CalendarDate>,
    ) -> OwnerMoneyForm {
        OwnerMoneyForm {
            direction,
            who: None,
            who_name: String::new(),
            bank,
            method: OwnerMethod::Electronic,
            date,
            amount: None,
            memo: String::new(),
            equity: None,
        }
    }
}

/// Why the form can't be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OwnerMoneyProblem {
    /// No bank account.
    BankMissing,
    /// No date.
    DateMissing,
    /// A check needs a name to pay.
    WhoMissing,
    /// The check number is not a whole number from 1 to 2,147,483,647.
    NumberInvalid,
    /// The amount is blank, zero or too large.
    Amount(AmountIssue),
    /// No equity account ("Recorded in").
    EquityMissing,
    /// The equity account is the bank account.
    SameAccount,
}

impl OwnerMoneyProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            OwnerMoneyProblem::BankMissing => TextKey("helper.problem.bank_missing"),
            OwnerMoneyProblem::DateMissing => TextKey("helper.problem.date_missing"),
            OwnerMoneyProblem::WhoMissing => TextKey("owner_money.problem.who_missing"),
            OwnerMoneyProblem::NumberInvalid => TextKey("helper.problem.number_invalid"),
            OwnerMoneyProblem::Amount(AmountIssue::Missing) => {
                TextKey("helper.problem.amount_missing")
            }
            OwnerMoneyProblem::Amount(AmountIssue::NotPositive) => {
                TextKey("helper.problem.amount_not_positive")
            }
            OwnerMoneyProblem::Amount(AmountIssue::TooLarge) => {
                TextKey("helper.problem.amount_too_large")
            }
            OwnerMoneyProblem::EquityMissing => TextKey("owner_money.problem.equity_missing"),
            OwnerMoneyProblem::SameAccount => TextKey("owner_money.problem.same_account"),
        }
    }

    /// The control channel's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            OwnerMoneyProblem::BankMissing => "bank_missing",
            OwnerMoneyProblem::DateMissing => "date_missing",
            OwnerMoneyProblem::WhoMissing => "who_missing",
            OwnerMoneyProblem::NumberInvalid => "number_invalid",
            OwnerMoneyProblem::Amount(AmountIssue::Missing) => "amount_missing",
            OwnerMoneyProblem::Amount(AmountIssue::NotPositive) => "amount_not_positive",
            OwnerMoneyProblem::Amount(AmountIssue::TooLarge) => "amount_too_large",
            OwnerMoneyProblem::EquityMissing => "equity_missing",
            OwnerMoneyProblem::SameAccount => "same_account",
        }
    }
}

/// What the checked form posts as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OwnerPosting {
    /// A check on the bank with one line to the equity account.
    Check {
        /// The check number (`None`: no number).
        number: Option<u32>,
    },
    /// An online payment from the bank with one line to the equity account.
    Payment,
    /// A deposit into the bank with one line from the equity account.
    Deposit,
}

/// A form that passed [`check`]. `Debug` is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedOwnerMoney {
    /// What it posts as.
    pub posting: OwnerPosting,
    /// Who.
    pub who: Option<ContactId>,
    /// The name on a check (trimmed; may be empty: use the contact's name).
    pub who_name: String,
    /// The bank account.
    pub bank: AccountId,
    /// Date.
    pub date: CalendarDate,
    /// The amount, more than zero (the one line's amount, exactly).
    pub amount: Money,
    /// Memo, trimmed.
    pub memo: Option<String>,
    /// The equity account.
    pub equity: AccountId,
}

impl fmt::Debug for CheckedOwnerMoney {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedOwnerMoney")
            .field("posting", &self.posting)
            .finish_non_exhaustive()
    }
}

/// Checks the form.
///
/// # Errors
/// Every problem found, in field order.
pub fn check(form: &OwnerMoneyForm) -> Result<CheckedOwnerMoney, Vec<OwnerMoneyProblem>> {
    let mut problems = Vec::new();
    if form.bank.is_none() {
        problems.push(OwnerMoneyProblem::BankMissing);
    }
    if form.date.is_none() {
        problems.push(OwnerMoneyProblem::DateMissing);
    }
    let posting = match (&form.method, form.direction) {
        (_, Direction::PutMoneyIn) => OwnerPosting::Deposit,
        (OwnerMethod::Electronic, Direction::PayYourself) => OwnerPosting::Payment,
        (OwnerMethod::Check(n), Direction::PayYourself) => {
            if form.who.is_none() && form.who_name.trim().is_empty() {
                problems.push(OwnerMoneyProblem::WhoMissing);
            }
            if let Ok(number) = parse_number(n) {
                OwnerPosting::Check { number }
            } else {
                problems.push(OwnerMoneyProblem::NumberInvalid);
                OwnerPosting::Check { number: None }
            }
        }
    };
    if let Some(i) = amount_problem(form.amount) {
        problems.push(OwnerMoneyProblem::Amount(i));
    }
    match form.equity {
        None => problems.push(OwnerMoneyProblem::EquityMissing),
        Some(e) if Some(e) == form.bank => problems.push(OwnerMoneyProblem::SameAccount),
        Some(_) => {}
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    match (form.bank, form.date, form.amount, form.equity) {
        (Some(bank), Some(date), Some(amount), Some(equity)) => Ok(CheckedOwnerMoney {
            posting,
            who: form.who,
            who_name: form.who_name.trim().to_string(),
            bank,
            date,
            amount,
            memo: trimmed(&form.memo),
            equity,
        }),
        _ => Err(vec![OwnerMoneyProblem::Amount(AmountIssue::Missing)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forms::card::tests_support::{account, money};

    fn date() -> CalendarDate {
        CalendarDate::from_ymd(2026, 11, 4).expect("date")
    }

    fn roles() -> BTreeMap<Role, AccountId> {
        [
            (Role::OwnerDraw, account(3200)),
            (Role::OwnerContribution, account(3100)),
            (Role::PartnerDraws, account(3180)),
            (Role::PartnerContributions, account(3190)),
            (Role::ShareholderDistributions, account(3300)),
            (Role::PaidInCapital, account(3140)),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn default_roles_for_all_five_types_and_both_directions() {
        let want = [
            (Entity::SoleProp, Role::OwnerDraw, Role::OwnerContribution),
            (
                Entity::SingleMemberLlc,
                Role::OwnerDraw,
                Role::OwnerContribution,
            ),
            (
                Entity::Partnership,
                Role::PartnerDraws,
                Role::PartnerContributions,
            ),
            (
                Entity::SCorp,
                Role::ShareholderDistributions,
                Role::PaidInCapital,
            ),
            (
                Entity::CCorp,
                Role::ShareholderDistributions,
                Role::PaidInCapital,
            ),
        ];
        for (e, out, inn) in want {
            assert_eq!(default_role(e, Direction::PayYourself), out, "{e:?}");
            assert_eq!(default_role(e, Direction::PutMoneyIn), inn, "{e:?}");
        }
        assert_eq!(Entity::ALL.len(), 5);
        assert_ne!(Entity::SCorp.note(), Entity::Partnership.note());
    }

    #[test]
    fn engine_role_first_then_the_remembered_choice() {
        let usable: Vec<AccountId> = roles().values().copied().chain([account(3999)]).collect();
        assert_eq!(
            pick_equity(
                &roles(),
                Some(Entity::SingleMemberLlc),
                Direction::PayYourself,
                Some(account(3999)),
                &usable
            ),
            Some((account(3200), EquitySource::Engine))
        );
        // Without the capability (no roles) or the entity type: remembered.
        assert_eq!(
            pick_equity(
                &BTreeMap::new(),
                Some(Entity::SCorp),
                Direction::PutMoneyIn,
                Some(account(3999)),
                &usable
            ),
            Some((account(3999), EquitySource::Remembered))
        );
        assert_eq!(
            pick_equity(
                &roles(),
                None,
                Direction::PayYourself,
                Some(account(3999)),
                &usable
            ),
            Some((account(3999), EquitySource::Remembered))
        );
        // An inactive (unusable) account is dropped.
        assert_eq!(
            pick_equity(
                &BTreeMap::new(),
                None,
                Direction::PayYourself,
                Some(account(4444)),
                &usable
            ),
            None
        );
        assert_eq!(
            pick_equity(
                &roles(),
                Some(Entity::CCorp),
                Direction::PayYourself,
                None,
                &[]
            ),
            None
        );
    }

    fn filled(direction: Direction, method: OwnerMethod) -> OwnerMoneyForm {
        let mut f = OwnerMoneyForm::new(direction, Some(account(1010)), Some(date()));
        f.method = method;
        f.amount = Some(money("500.00"));
        f.equity = Some(account(3200));
        f.who_name = "Kurt (owner)".into();
        f
    }

    #[test]
    fn posting_kind_by_direction_and_method() {
        let c = check(&filled(Direction::PayYourself, OwnerMethod::Electronic)).expect("ok");
        assert_eq!(c.posting, OwnerPosting::Payment);
        assert_eq!(c.amount, money("500.00"));
        let c = check(&filled(
            Direction::PayYourself,
            OwnerMethod::Check("1045".into()),
        ))
        .expect("ok");
        assert_eq!(c.posting, OwnerPosting::Check { number: Some(1045) });
        let c = check(&filled(
            Direction::PutMoneyIn,
            OwnerMethod::Check("x".into()),
        ))
        .expect("a deposit has no check number");
        assert_eq!(c.posting, OwnerPosting::Deposit);
    }

    #[test]
    fn problems_in_field_order() {
        let mut f = OwnerMoneyForm::new(Direction::PayYourself, None, None);
        f.method = OwnerMethod::Check("0".into());
        assert_eq!(
            check(&f).expect_err("blank"),
            vec![
                OwnerMoneyProblem::BankMissing,
                OwnerMoneyProblem::DateMissing,
                OwnerMoneyProblem::WhoMissing,
                OwnerMoneyProblem::NumberInvalid,
                OwnerMoneyProblem::Amount(AmountIssue::Missing),
                OwnerMoneyProblem::EquityMissing,
            ]
        );
        let mut f = filled(Direction::PutMoneyIn, OwnerMethod::Electronic);
        f.equity = f.bank;
        f.amount = Some(Money::ZERO);
        assert_eq!(
            check(&f).expect_err("same"),
            vec![
                OwnerMoneyProblem::Amount(AmountIssue::NotPositive),
                OwnerMoneyProblem::SameAccount
            ]
        );
        assert_eq!(OwnerMoneyProblem::SameAccount.name(), "same_account");
    }

    #[test]
    fn debug_hides_owner_data() {
        let mut f = filled(Direction::PayYourself, OwnerMethod::Electronic);
        f.memo = "secret".into();
        let s = format!("{f:?} {:?}", check(&f).expect("ok"));
        assert!(!s.contains("500") && !s.contains("secret") && !s.contains("Kurt"));
    }
}
