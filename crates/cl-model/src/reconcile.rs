//! Reconcile (P27): what the window keeps between frames, with no egui and
//! no engine types.
//!
//! - [`BeginForm`]: the Begin step's fields and their checks (the ending
//!   balance may be negative; charges must be above zero; at most 2
//!   decimals everywhere).
//! - [`ReconKind`]: a bank account or a card; it picks the labels and the
//!   display sign (a card's books balance is negative, the owner sees the
//!   printed figure, Q-P12h).
//! - [`Pane`] and [`pane_of`]: which pane a line goes in by its sign.
//! - [`TickQueue`]: the owner's ticks, one batch at a time to the engine
//!   (Q-P27f). The engine's answer replaces every total; the queue only
//!   remembers what the owner asked for and what is in flight.
//! - [`can_finish`]: Reconcile now waits for the engine's difference to be
//!   exactly zero with nothing in flight or waiting.
//! - [`hint`]: "one unchecked item equals the difference", an exact
//!   signed equality test, never a sum.
//!
//! Nothing here adds amounts: every balance is the engine's.

use std::collections::BTreeMap;

use cl_core::ids::{AccountId, JournalLineId};
use cl_core::money::{MoneyInputError, UserInput};
use cl_core::{CalendarDate, Money, MoneyStyle};

/// A bank account or a card (the label set and the display sign).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReconKind {
    /// Checking, savings, cash: asset accounts.
    Bank,
    /// A credit card or line of credit: liability accounts.
    Card,
}

impl ReconKind {
    /// The pane on the left: money in for a bank, charges for a card.
    #[must_use]
    pub const fn left(self) -> Pane {
        match self {
            ReconKind::Bank => Pane::MoneyIn,
            ReconKind::Card => Pane::MoneyOut,
        }
    }

    /// The pane on the right.
    #[must_use]
    pub const fn right(self) -> Pane {
        match self {
            ReconKind::Bank => Pane::MoneyOut,
            ReconKind::Card => Pane::MoneyIn,
        }
    }

    /// The other pane than `p` (Tab).
    #[must_use]
    pub const fn other(p: Pane) -> Pane {
        match p {
            Pane::MoneyIn => Pane::MoneyOut,
            Pane::MoneyOut => Pane::MoneyIn,
        }
    }

    /// i18n key of a pane's title.
    #[must_use]
    pub const fn pane_title(self, p: Pane) -> &'static str {
        match (self, p) {
            (ReconKind::Bank, Pane::MoneyIn) => "reconcile.pane.deposits",
            (ReconKind::Bank, Pane::MoneyOut) => "reconcile.pane.payments",
            (ReconKind::Card, Pane::MoneyOut) => "reconcile.pane.card_charges",
            (ReconKind::Card, Pane::MoneyIn) => "reconcile.pane.card_payments",
        }
    }

    /// A balance as the owner reads it: a card's books balance flipped
    /// (owed shows positive). Text only; the value sent is never flipped.
    #[must_use]
    pub fn shown(self, books: Money) -> Money {
        match self {
            ReconKind::Bank => books,
            ReconKind::Card => books.checked_neg().unwrap_or(books),
        }
    }
}

/// One of the two panes, by the line's sign in the books.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Pane {
    /// Debits to the account (deposits; card payments and credits).
    MoneyIn,
    /// Credits to the account (checks and payments; card charges). Zero
    /// lines go here too.
    MoneyOut,
}

/// The pane a line with `amount` (debit − credit) belongs in.
#[must_use]
pub fn pane_of(amount: Money) -> Pane {
    if amount.is_positive() {
        Pane::MoneyIn
    } else {
        Pane::MoneyOut
    }
}

/// An amount for display: 2 decimals, half away from zero, never `-0.00`;
/// a nonzero amount smaller than a cent keeps its 4 decimals (Q-P27l), so
/// "0.00 and still disabled" can't happen.
#[must_use]
pub fn display(m: Money) -> String {
    let style = MoneyStyle::default();
    let units = m.to_ten_thousandths();
    if units != 0 && units.abs() < 100 {
        return m.format(style);
    }
    // Half away from zero at the cent (100 ten-thousandths).
    let cents = (units.abs() + 50) / 100 * 100;
    let rounded = if units < 0 { -cents } else { cents };
    Money::from_ten_thousandths(rounded).map_or_else(|_| m.format(style), |r| r.format(style))
}

/// What is wrong with a Begin field (i18n key suffixes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BeginProblem {
    /// Pick an account.
    NoAccount,
    /// Enter the statement date.
    NoDate,
    /// Enter the ending balance.
    NoEnding,
    /// The ending balance isn't a number with at most 2 decimals.
    BadEnding,
    /// The service charge isn't above zero with at most 2 decimals.
    BadServiceCharge,
    /// The service charge needs its account.
    ServiceChargeAccount,
    /// The interest isn't above zero with at most 2 decimals.
    BadInterest,
    /// The interest needs its account.
    InterestAccount,
    /// The charge account is the account being reconciled.
    SameAccount,
    /// The statement date is on or before the last finished
    /// reconciliation's statement date.
    DateNotAfterLast,
}

impl BeginProblem {
    /// i18n key.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            BeginProblem::NoAccount => "reconcile.problem.no_account",
            BeginProblem::NoDate => "reconcile.problem.no_date",
            BeginProblem::NoEnding => "reconcile.problem.no_ending",
            BeginProblem::BadEnding => "reconcile.problem.bad_ending",
            BeginProblem::BadServiceCharge => "reconcile.problem.bad_service_charge",
            BeginProblem::ServiceChargeAccount => "reconcile.problem.service_charge_account",
            BeginProblem::BadInterest => "reconcile.problem.bad_interest",
            BeginProblem::InterestAccount => "reconcile.problem.interest_account",
            BeginProblem::SameAccount => "reconcile.problem.same_account",
            BeginProblem::DateNotAfterLast => "reconcile.problem.date_not_after_last",
        }
    }
}

/// A charge's three fields as typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChargeInput {
    /// The amount text.
    pub amount: String,
    /// Its date (the statement date when empty).
    pub date: Option<CalendarDate>,
    /// Its account.
    pub account: Option<AccountId>,
    /// Already posted (after a reopen). Still editable: the engine takes a
    /// change only once that entry was voided or deleted (U-193), and the
    /// screen sends a charge only when it differs from the one held.
    pub posted: bool,
}

/// A charge as Begin sends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChargeValue {
    /// > 0, at most 2 decimals.
    pub amount: Money,
    /// The entry's date.
    pub date: CalendarDate,
    /// Its account.
    pub account: AccountId,
}

/// The Begin step's fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BeginForm {
    /// The account being reconciled.
    pub account: Option<AccountId>,
    /// Bank or card.
    pub kind: ReconKind,
    /// The statement date.
    pub statement_date: Option<CalendarDate>,
    /// The ending balance as printed.
    pub ending: String,
    /// The service charge (a card's finance charge).
    pub service_charge: ChargeInput,
    /// The interest earned (banks only).
    pub interest: ChargeInput,
    /// The statement date of the account's last finished reconciliation:
    /// a new statement must be dated after it.
    pub last_reconciled: Option<CalendarDate>,
}

/// The checked Begin values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeginValues {
    /// The account.
    pub account: AccountId,
    /// The statement date.
    pub statement_date: CalendarDate,
    /// The ending balance as printed (a card's owed positive).
    pub ending: Money,
    /// The service charge.
    pub service_charge: Option<ChargeValue>,
    /// The interest.
    pub interest: Option<ChargeValue>,
}

fn cents(text: &str) -> Result<Money, MoneyInputError> {
    Money::parse_user(text, UserInput::CENTS)
}

impl BeginForm {
    /// An empty form on `today`.
    #[must_use]
    pub fn new(today: CalendarDate) -> BeginForm {
        BeginForm {
            account: None,
            kind: ReconKind::Bank,
            statement_date: Some(today),
            ending: String::new(),
            service_charge: ChargeInput::default(),
            interest: ChargeInput::default(),
            last_reconciled: None,
        }
    }

    fn charge(
        &self,
        c: &ChargeInput,
        bad: BeginProblem,
        no_account: BeginProblem,
        problems: &mut Vec<BeginProblem>,
    ) -> Option<ChargeValue> {
        if c.amount.trim().is_empty() {
            return None;
        }
        let amount = match cents(&c.amount) {
            Ok(m) if m.is_positive() => m,
            _ => {
                problems.push(bad);
                return None;
            }
        };
        let Some(account) = c.account else {
            problems.push(no_account);
            return None;
        };
        if Some(account) == self.account {
            problems.push(BeginProblem::SameAccount);
            return None;
        }
        Some(ChargeValue {
            amount,
            date: c.date.or(self.statement_date)?,
            account,
        })
    }

    /// The values to send, or every problem found.
    ///
    /// # Errors
    /// The problems, in field order.
    pub fn validate(&self) -> Result<BeginValues, Vec<BeginProblem>> {
        let mut problems = Vec::new();
        if self.account.is_none() {
            problems.push(BeginProblem::NoAccount);
        }
        match (self.statement_date, self.last_reconciled) {
            (None, _) => problems.push(BeginProblem::NoDate),
            (Some(d), Some(last)) if d <= last => problems.push(BeginProblem::DateNotAfterLast),
            _ => {}
        }
        let ending = if self.ending.trim().is_empty() {
            problems.push(BeginProblem::NoEnding);
            None
        } else {
            cents(&self.ending)
                .map_err(|_| problems.push(BeginProblem::BadEnding))
                .ok()
        };
        let service_charge = self.charge(
            &self.service_charge,
            BeginProblem::BadServiceCharge,
            BeginProblem::ServiceChargeAccount,
            &mut problems,
        );
        let interest = if self.kind == ReconKind::Card {
            None
        } else {
            self.charge(
                &self.interest,
                BeginProblem::BadInterest,
                BeginProblem::InterestAccount,
                &mut problems,
            )
        };
        match (self.account, self.statement_date, ending) {
            (Some(account), Some(statement_date), Some(ending)) if problems.is_empty() => {
                Ok(BeginValues {
                    account,
                    statement_date,
                    ending,
                    service_charge,
                    interest,
                })
            }
            _ => Err(problems),
        }
    }
}

/// The owner's ticks on their way to the engine (Q-P27f): ticks made while
/// a batch is in flight wait and go in the next one; a failed batch is
/// dropped (its lines show the engine's state again).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TickQueue {
    /// The engine's last known state of each line.
    base: BTreeMap<JournalLineId, bool>,
    /// Sent and not answered.
    in_flight: BTreeMap<JournalLineId, bool>,
    /// Asked for and not sent yet.
    waiting: BTreeMap<JournalLineId, bool>,
}

impl TickQueue {
    /// Starts from the engine's lines (`(line, cleared)`); anything waiting
    /// or in flight is kept.
    pub fn load(&mut self, lines: impl IntoIterator<Item = (JournalLineId, bool)>) {
        self.base = lines.into_iter().collect();
        // A waiting tick that now matches the engine is done; one for a
        // line no longer on the worksheet (its entry voided, U-193) is
        // dropped rather than sent.
        let base = &self.base;
        self.waiting
            .retain(|id, on| base.get(id).is_some_and(|b| b != on));
    }

    /// The state shown for `id`: waiting, else in flight, else the engine's.
    #[must_use]
    pub fn ticked(&self, id: JournalLineId) -> bool {
        self.waiting
            .get(&id)
            .or_else(|| self.in_flight.get(&id))
            .or_else(|| self.base.get(&id))
            .copied()
            .unwrap_or(false)
    }

    /// Sets `ids` to `on` (Space, a click, Mark all, Unmark all).
    pub fn set(&mut self, ids: impl IntoIterator<Item = JournalLineId>, on: bool) {
        for id in ids {
            if self.ticked(id) != on {
                self.waiting.insert(id, on);
            }
        }
    }

    /// Flips `id`.
    pub fn toggle(&mut self, id: JournalLineId) {
        let on = !self.ticked(id);
        self.waiting.insert(id, on);
    }

    /// The next batch to send, when nothing is in flight.
    pub fn take_batch(&mut self) -> Option<Vec<(JournalLineId, bool)>> {
        if !self.in_flight.is_empty() || self.waiting.is_empty() {
            return None;
        }
        self.in_flight = std::mem::take(&mut self.waiting);
        Some(self.in_flight.iter().map(|(k, v)| (*k, *v)).collect())
    }

    /// The engine took the batch: it becomes the known state.
    pub fn sent(&mut self) {
        let done = std::mem::take(&mut self.in_flight);
        self.base.extend(done);
    }

    /// The batch failed: its ticks are dropped (rolled back to the
    /// engine's state); later ticks still wait.
    pub fn failed(&mut self) {
        self.in_flight.clear();
    }

    /// Nothing in flight and nothing waiting.
    #[must_use]
    pub fn idle(&self) -> bool {
        self.in_flight.is_empty() && self.waiting.is_empty()
    }

    /// A batch is on its way.
    #[must_use]
    pub fn busy(&self) -> bool {
        !self.in_flight.is_empty()
    }
}

/// Reconcile now is on only when the engine's difference is exactly zero
/// and the engine has every tick (Q-P27a, Q-P27f).
#[must_use]
pub fn can_finish(engine_difference: Money, queue: &TickQueue) -> bool {
    engine_difference.is_zero() && queue.idle()
}

/// The single unchecked line whose amount equals the difference, if there
/// is exactly one: exact signed equality, no arithmetic. The difference is
/// ending − cleared and ticking a line adds its amount (debit − credit) to
/// cleared, so only a line of the same sign brings it to zero.
#[must_use]
pub fn hint<I>(unchecked: I, difference: Money) -> Option<JournalLineId>
where
    I: IntoIterator<Item = (JournalLineId, Money)>,
{
    if difference.is_zero() {
        return None;
    }
    let mut found = unchecked
        .into_iter()
        .filter(|(_, a)| *a == difference)
        .map(|(id, _)| id);
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn id(n: u8) -> JournalLineId {
        JournalLineId::parse(&format!("00000000-0000-4000-8000-0000000000{n:02}")).expect("id")
    }

    fn acct(n: u8) -> AccountId {
        AccountId::parse(&format!("00000000-0000-4000-8000-0000000001{n:02}")).expect("id")
    }

    fn day(d: u32) -> CalendarDate {
        CalendarDate::from_ymd(2026, 10, d).expect("date")
    }

    #[test]
    fn display_rounds_and_keeps_sub_cent() {
        assert_eq!(display(m("-412.18")), "-412.18");
        assert_eq!(display(m("71436.2")), "71,436.20");
        assert_eq!(display(m("0.005")), "0.005");
        assert_eq!(display(m("-0.0001")), "-0.0001");
        assert_eq!(display(m("0")), "0.00");
        assert_eq!(display(m("1.005")), "1.01");
        assert_eq!(display(m("-1.005")), "-1.01");
    }

    #[test]
    fn panes_by_sign_and_kind() {
        assert_eq!(pane_of(m("8450")), Pane::MoneyIn);
        assert_eq!(pane_of(m("-412.18")), Pane::MoneyOut);
        assert_eq!(pane_of(m("0")), Pane::MoneyOut, "zero goes to payments");
        assert_eq!(ReconKind::Bank.left(), Pane::MoneyIn);
        assert_eq!(ReconKind::Card.left(), Pane::MoneyOut);
        assert_eq!(
            ReconKind::Card.pane_title(Pane::MoneyOut),
            "reconcile.pane.card_charges"
        );
        assert_eq!(ReconKind::Card.shown(m("-80")), m("80"));
        assert_eq!(ReconKind::Bank.shown(m("-80")), m("-80"));
        assert_eq!(ReconKind::other(Pane::MoneyIn), Pane::MoneyOut);
    }

    #[test]
    fn begin_checks_every_field() {
        let mut f = BeginForm::new(day(31));
        assert_eq!(
            f.validate(),
            Err(vec![BeginProblem::NoAccount, BeginProblem::NoEnding])
        );
        f.account = Some(acct(1));
        f.ending = "86,902.13".into();
        let v = f.validate().expect("ok");
        assert_eq!(v.ending, m("86902.13"));
        assert_eq!(v.service_charge, None);
        f.ending = "-5.001".into();
        assert_eq!(f.validate(), Err(vec![BeginProblem::BadEnding]));
        f.ending = "-5".into();
        assert_eq!(f.validate().expect("negative ok").ending, m("-5"));
        f.service_charge.amount = "48".into();
        assert_eq!(f.validate(), Err(vec![BeginProblem::ServiceChargeAccount]));
        f.service_charge.account = Some(acct(1));
        assert_eq!(f.validate(), Err(vec![BeginProblem::SameAccount]));
        f.service_charge.account = Some(acct(2));
        let v = f.validate().expect("ok");
        assert_eq!(
            v.service_charge,
            Some(ChargeValue {
                amount: m("48"),
                date: day(31),
                account: acct(2)
            })
        );
        f.interest.amount = "0".into();
        f.interest.account = Some(acct(3));
        assert_eq!(f.validate(), Err(vec![BeginProblem::BadInterest]));
        // A card has no interest field: it is never sent.
        f.kind = ReconKind::Card;
        assert_eq!(f.validate().expect("card").interest, None);
        // A posted charge is checked like any other (the screen sends it
        // only when changed; the engine refuses it while still posted).
        f.service_charge.posted = true;
        assert_eq!(
            f.validate()
                .expect("posted")
                .service_charge
                .map(|c| c.amount),
            Some(m("48"))
        );
    }

    #[test]
    fn begin_refuses_a_date_not_after_the_last_reconciliation() {
        let mut f = BeginForm::new(day(31));
        f.account = Some(acct(1));
        f.ending = "10".into();
        f.last_reconciled = Some(day(15));
        assert!(f.validate().is_ok(), "after the last one");
        f.statement_date = Some(day(15));
        assert_eq!(f.validate(), Err(vec![BeginProblem::DateNotAfterLast]));
        f.statement_date = Some(day(14));
        assert_eq!(f.validate(), Err(vec![BeginProblem::DateNotAfterLast]));
        f.last_reconciled = None;
        assert!(f.validate().is_ok(), "no finished reconciliation yet");
    }

    #[test]
    fn tick_queue_batches_and_rolls_back() {
        let mut q = TickQueue::default();
        q.load([(id(1), true), (id(2), false), (id(3), false)]);
        assert!(q.idle());
        q.toggle(id(2));
        assert!(q.ticked(id(2)));
        let batch = q.take_batch().expect("batch");
        assert_eq!(batch, vec![(id(2), true)]);
        assert!(q.busy());
        // Ticks while in flight wait for the next batch.
        q.toggle(id(3));
        assert!(q.take_batch().is_none());
        q.sent();
        assert!(q.ticked(id(2)));
        assert_eq!(q.take_batch(), Some(vec![(id(3), true)]));
        q.failed();
        assert!(!q.ticked(id(3)), "rolled back to the engine's state");
        assert!(q.idle());
        q.set([id(1), id(2), id(3)], false);
        assert_eq!(
            q.take_batch().map(|b| b.len()),
            Some(2),
            "id 3 is already off"
        );
        q.sent();
        // Loading the engine's state drops waiting ticks it already has.
        q.set([id(1)], true);
        q.load([(id(1), true), (id(2), false), (id(3), false)]);
        assert!(q.idle());
        // A line gone from the worksheet (entry voided): its tick is dropped.
        q.toggle(id(3));
        q.load([(id(1), true), (id(2), false)]);
        assert!(q.idle(), "nothing sent for a line the engine dropped");
        assert!(!q.ticked(id(3)));
    }

    #[test]
    fn can_finish_only_at_engine_zero_and_idle() {
        let mut q = TickQueue::default();
        q.load([(id(1), false)]);
        assert!(can_finish(m("0"), &q));
        assert!(!can_finish(m("-0.0001"), &q));
        assert!(!can_finish(m("0.01"), &q));
        q.toggle(id(1));
        assert!(!can_finish(m("0"), &q), "a tick waits");
        let _ = q.take_batch();
        assert!(!can_finish(m("0"), &q), "a tick in flight");
        q.sent();
        assert!(can_finish(m("0"), &q));
    }

    #[test]
    fn hint_is_exact_and_single() {
        let lines = [(id(1), m("-412.18")), (id(2), m("12.00"))];
        assert_eq!(hint(lines, m("-412.18")), Some(id(1)));
        // Ticking a -412.18 line moves a +412.18 difference to +824.36.
        assert_eq!(hint(lines, m("412.18")), None, "opposite sign: no hint");
        assert_eq!(hint(lines, m("-412.17")), None);
        assert_eq!(hint(lines, m("12")), Some(id(2)));
        assert_eq!(hint(lines, m("-12")), None);
        assert_eq!(hint(lines, m("0")), None);
        let twins = [(id(1), m("-5")), (id(2), m("-5"))];
        assert_eq!(hint(twins, m("-5")), None, "two candidates: no hint");
        let mixed = [(id(1), m("-5")), (id(2), m("5"))];
        assert_eq!(
            hint(mixed, m("-5")),
            Some(id(1)),
            "only the same sign counts"
        );
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// For random tick sets, Reconcile now is on exactly when the
            /// engine's difference string is zero and nothing is pending.
            #[test]
            fn can_finish_iff_zero_and_idle(
                cents in -100_000i64..100_000,
                toggles in proptest::collection::vec(0u8..8, 0..12),
                settle in any::<bool>(),
            ) {
                let diff = Money::from_ten_thousandths(i128::from(cents) * 100)
                    .expect("in range");
                let mut q = TickQueue::default();
                q.load((0..8).map(|n| (id(n), false)));
                for t in &toggles {
                    q.toggle(id(*t));
                }
                if settle {
                    while q.take_batch().is_some() {
                        q.sent();
                    }
                }
                let parsed = Money::parse_wire(&diff.to_wire()).expect("wire");
                prop_assert_eq!(
                    can_finish(parsed, &q),
                    parsed.is_zero() && q.idle()
                );
                if settle {
                    prop_assert!(q.idle());
                }
            }
        }
    }
}
