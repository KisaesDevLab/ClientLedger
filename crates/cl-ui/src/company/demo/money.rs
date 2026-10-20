//! The demo engine's owner and company money (P14 WP-D.3): the entity
//! type, the equity role accounts (`GET /company/equity-roles`,
//! `equity-roles-v1`) and vendor refunds (`vendor-refund-v1`). Synthetic.
//!
//! It plays the engine's part for `--demo` only: the roles are the chart's
//! equity accounts tagged by role (the sample's Owner Contributions 3100
//! and Owner Draws 3200 carry `owner_contribution` and `owner_draw`), and
//! changing the entity type seeds the new type's roles the way the engine
//! does (adopt an untagged account with a known name or number, else
//! create one). A vendor refund is a deposit with a vendor-credit source
//! line on Accounts Payable ([`super::deposits::create`]). The closing
//! password is "demo-close", as everywhere in the demo.

use cl_api::{
    Account, AccountFields, AccountType, ApiError, ConflictReason, DepositDraft, DepositSource,
    EntityType, GroupedPayment, LocalValidation, SplitLine,
};
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};

use super::DemoState;
use super::register::{DemoJournal, DemoLine, conflict_code};

/// An equity role the engine tags an account with (`accounts.system_tag`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EquityRole {
    /// Owner's Draw (sole proprietor, single-member LLC).
    OwnerDraw,
    /// Owner's Contribution (sole proprietor, single-member LLC).
    OwnerContribution,
    /// Partner Draws.
    PartnerDraws,
    /// Partner Contributions.
    PartnerContributions,
    /// Shareholder Distributions (S corporation), Dividends Paid (C
    /// corporation).
    ShareholderDistributions,
    /// Paid-in Capital (corporations).
    PaidInCapital,
    /// Retained Earnings.
    RetainedEarnings,
}

impl EquityRole {
    /// Every role, in the engine's key order (all seven keys are always in
    /// the answer).
    pub const ALL: [EquityRole; 7] = [
        EquityRole::OwnerDraw,
        EquityRole::OwnerContribution,
        EquityRole::PartnerDraws,
        EquityRole::PartnerContributions,
        EquityRole::ShareholderDistributions,
        EquityRole::PaidInCapital,
        EquityRole::RetainedEarnings,
    ];

    /// The engine's key and `system_tag`.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            EquityRole::OwnerDraw => "owner_draw",
            EquityRole::OwnerContribution => "owner_contribution",
            EquityRole::PartnerDraws => "partner_draws",
            EquityRole::PartnerContributions => "partner_contributions",
            EquityRole::ShareholderDistributions => "shareholder_distributions",
            EquityRole::PaidInCapital => "paid_in_capital",
            EquityRole::RetainedEarnings => "retained_earnings",
        }
    }

    /// The role of an engine key.
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<EquityRole> {
        EquityRole::ALL.into_iter().find(|r| r.tag() == tag)
    }
}

/// The role of a sample chart account (its own Owner Contributions and
/// Owner Draws, which its history posts to).
#[must_use]
pub fn sample_equity_role(number: &str) -> Option<EquityRole> {
    match number {
        "3100" => Some(EquityRole::OwnerContribution),
        "3200" => Some(EquityRole::OwnerDraw),
        _ => None,
    }
}

/// `GET /company/equity-roles` as the demo engine answers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoEquityRoles {
    /// The company's entity type.
    pub entity_type: EntityType,
    /// Every role (in [`EquityRole::ALL`] order) with its account, if any.
    pub roles: Vec<(EquityRole, Option<AccountId>)>,
}

impl DemoEquityRoles {
    /// The account holding `role`.
    #[must_use]
    pub fn account(&self, role: EquityRole) -> Option<AccountId> {
        self.roles
            .iter()
            .find(|(r, _)| *r == role)
            .and_then(|(_, a)| *a)
    }
}

fn chart_mut(state: &mut DemoState) -> &mut Vec<Account> {
    state.chart.get_or_insert_with(super::chart)
}

/// The company's equity roles (the chart's tagged equity accounts).
#[must_use]
pub fn equity_roles(state: &mut DemoState) -> DemoEquityRoles {
    let entity_type = state.entity_type;
    let chart = chart_mut(state);
    let roles = EquityRole::ALL
        .into_iter()
        .map(|r| {
            let a = chart
                .iter()
                .find(|a| a.system_tag.as_deref() == Some(r.tag()))
                .map(|a| a.id);
            (r, a)
        })
        .collect();
    DemoEquityRoles { entity_type, roles }
}

/// What the engine seeds for an entity type: (role, name, number).
fn seeded(entity: EntityType) -> &'static [(EquityRole, &'static str, u32)] {
    match entity {
        EntityType::SoleProp | EntityType::SingleMemberLlc => &[
            (EquityRole::OwnerDraw, "Owner's Draw", 30170),
            (EquityRole::OwnerContribution, "Owner's Contribution", 30160),
        ],
        EntityType::Partnership => &[
            (EquityRole::PartnerDraws, "Partner Draws", 30180),
            (
                EquityRole::PartnerContributions,
                "Partner Contributions",
                30190,
            ),
        ],
        EntityType::SCorp => &[
            (
                EquityRole::ShareholderDistributions,
                "Shareholder Distributions",
                30200,
            ),
            (EquityRole::PaidInCapital, "Paid-in Capital", 30140),
        ],
        EntityType::CCorp => &[
            (EquityRole::PaidInCapital, "Paid-in Capital", 30140),
            (
                EquityRole::ShareholderDistributions,
                "Dividends Paid",
                30200,
            ),
        ],
    }
}

/// Template names an untagged account is adopted by (besides the seeded
/// name and number). Generic names (Distributions, Dividends, Common
/// Stock, Capital Stock) are never adopted.
fn template_name(role: EquityRole) -> Option<&'static str> {
    match role {
        EquityRole::OwnerDraw => Some("Owner Withdraw"),
        EquityRole::OwnerContribution => Some("Owner Contribution"),
        _ => None,
    }
}

/// Changes the company's entity type and seeds its equity roles, like the
/// engine's `PUT /company`: a role already tagged stays; else an untagged
/// equity account with the seeded name, the template name or the seeded
/// number is adopted; else a system account is created (without a number
/// when the number is taken). Idempotent.
pub fn set_entity_type(state: &mut DemoState, entity: EntityType) {
    state.entity_type = entity;
    let chart = chart_mut(state);
    for &(role, name, number) in seeded(entity) {
        if chart
            .iter()
            .any(|a| a.system_tag.as_deref() == Some(role.tag()))
        {
            continue;
        }
        let number_text = number.to_string();
        let adoptable = |a: &Account| {
            a.fields.account_type == AccountType::Equity
                && a.system_tag.is_none()
                && (a.fields.name.eq_ignore_ascii_case(name)
                    || template_name(role).is_some_and(|t| a.fields.name.eq_ignore_ascii_case(t))
                    || a.fields.account_number.as_deref() == Some(number_text.as_str()))
        };
        if let Some(a) = chart.iter_mut().find(|a| adoptable(a)) {
            a.system_tag = Some(role.tag().to_string());
            continue;
        }
        let taken = chart
            .iter()
            .any(|a| a.fields.account_number.as_deref() == Some(number_text.as_str()));
        let id = if taken {
            super::account_id(number + 900_000)
        } else {
            super::account_id(number)
        };
        chart.push(Account {
            id,
            fields: AccountFields {
                name: name.to_string(),
                account_number: (!taken).then_some(number_text),
                account_type: AccountType::Equity,
                detail_type: None,
                description: None,
                parent_id: None,
            },
            is_active: true,
            is_system: true,
            system_tag: Some(role.tag().to_string()),
            balance: Some(Money::ZERO),
            created_at: Some(DemoState::now()),
            updated_at: Some(DemoState::now()),
        });
    }
}

fn tagged(state: &mut DemoState, tag: &str) -> Option<AccountId> {
    chart_mut(state)
        .iter()
        .find(|a| a.system_tag.as_deref() == Some(tag))
        .map(|a| a.id)
}

/// Accounts Payable (`system_tag accounts_payable`).
#[must_use]
pub fn accounts_payable(state: &mut DemoState) -> Option<AccountId> {
    tagged(state, "accounts_payable")
}

fn contact_name(state: &mut DemoState, id: ContactId) -> Option<String> {
    state
        .contacts_mut()
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.fields.display_name.clone())
}

/// A vendor credit as the demo engine keeps it.
#[derive(Clone, PartialEq, Eq)]
pub struct DemoVendorCredit {
    /// The credit.
    pub id: TransactionId,
    /// Its date.
    pub date: CalendarDate,
    /// Its total.
    pub total: Money,
    /// Total less the refunds still standing (`balanceDue`).
    pub open: Money,
}

/// Ids and the date only: amounts are owner data.
impl std::fmt::Debug for DemoVendorCredit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DemoVendorCredit")
            .field("id", &self.id)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

/// Records a vendor credit (Dr Accounts Payable / Cr `account`): what a
/// vendor refund applies to. The demo has none until one is recorded.
///
/// # Errors
/// `NotFound` (unknown vendor), `Local(NonPositiveAmount)`,
/// `Local(InvalidValue)` (an account outside the chart).
pub fn record_vendor_credit(
    state: &mut DemoState,
    vendor: ContactId,
    date: CalendarDate,
    amount: Money,
    account: AccountId,
) -> Result<TransactionId, ApiError> {
    let name = contact_name(state, vendor).ok_or(ApiError::NotFound { code: None })?;
    if !amount.is_positive() {
        return Err(ApiError::Local(LocalValidation::NonPositiveAmount));
    }
    let ap = accounts_payable(state).ok_or(ApiError::Local(LocalValidation::InvalidValue))?;
    let j = DemoJournal {
        kind: "vendor_credit",
        date,
        number: String::new(),
        contact: Some(name),
        memo: String::new(),
        lines: vec![
            DemoLine {
                account: ap,
                debit: amount,
                credit: Money::ZERO,
                memo: None,
            },
            DemoLine {
                account,
                debit: Money::ZERO,
                credit: amount,
                memo: None,
            },
        ],
    };
    let chart = chart_mut(state).clone();
    state
        .ledger
        .save_lines(&chart, &j, None)
        .ok_or(ApiError::Local(LocalValidation::InvalidValue))
}

/// A vendor credit's open balance: its A/P debit less every refund still
/// standing (a void deposit gives its refund back). `None` for anything
/// that is not a live vendor credit.
#[must_use]
pub fn vendor_credit_open(state: &DemoState, id: TransactionId) -> Option<(String, Money)> {
    if state.ledger.is_void(id) {
        return None;
    }
    let t = state
        .ledger
        .transactions()
        .into_iter()
        .find(|t| super::register::DemoLedger::id_of(t) == id && t.kind == "vendor_credit")?;
    let total = Money::sum(t.lines.iter().map(|l| l.debit)).ok()?;
    let refunded = Money::sum(state.ledger.live_links_of(id).into_iter().map(|(_, m)| m)).ok()?;
    Some((
        t.contact.unwrap_or_default(),
        total.checked_sub(refunded).ok()?,
    ))
}

/// The live vendor credits of `vendor`, oldest first.
#[must_use]
pub fn vendor_credits(state: &mut DemoState, vendor: ContactId) -> Vec<DemoVendorCredit> {
    let Some(name) = contact_name(state, vendor) else {
        return Vec::new();
    };
    let mut out: Vec<DemoVendorCredit> = state
        .ledger
        .transactions()
        .iter()
        .filter(|t| t.kind == "vendor_credit" && t.contact.as_deref() == Some(name.as_str()))
        .filter_map(|t| {
            let id = super::register::DemoLedger::id_of(t);
            let (_, open) = vendor_credit_open(state, id)?;
            Some(DemoVendorCredit {
                id,
                date: t.date,
                total: Money::sum(t.lines.iter().map(|l| l.debit)).ok()?,
                open,
            })
        })
        .collect();
    out.sort_by_key(|c| c.date);
    out
}

/// A vendor refund (the engine's `vendor_refunds().create`).
#[derive(Clone, PartialEq, Eq)]
pub struct DemoVendorRefund {
    /// The vendor.
    pub vendor: ContactId,
    /// The bank account the refund goes into.
    pub deposit_to: AccountId,
    /// Date.
    pub date: CalendarDate,
    /// Amount (greater than zero).
    pub amount: Money,
    /// The vendor credit it applies to; `None` for an overpayment (a plain
    /// Accounts Payable line naming the vendor).
    pub applies_to: Option<TransactionId>,
    /// Memo.
    pub memo: Option<String>,
}

/// Ids and the date only.
impl std::fmt::Debug for DemoVendorRefund {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DemoVendorRefund")
            .field("vendor", &self.vendor)
            .field("deposit_to", &self.deposit_to)
            .field("date", &self.date)
            .field("applies_to", &self.applies_to)
            .finish_non_exhaustive()
    }
}

/// Records a vendor refund: a deposit into `deposit_to` with one Accounts
/// Payable line naming the vendor (a vendor-credit source line when it
/// applies to a credit).
///
/// # Errors
/// As [`super::deposits::create`]: `Conflict` with
/// `VENDOR_CREDIT_EXCEEDED` above the credit's open balance; the
/// closing-date rule.
pub fn vendor_refund(
    state: &mut DemoState,
    r: &DemoVendorRefund,
    password: Option<&Secret>,
) -> Result<TransactionId, ApiError> {
    let ap = accounts_payable(state).ok_or(ApiError::Local(LocalValidation::InvalidValue))?;
    let mut d = DepositDraft::plain(r.deposit_to, r.date, r.memo.clone(), Vec::new());
    match r.applies_to {
        Some(source) => d.payments.push(GroupedPayment {
            source,
            kind: DepositSource::VendorCredit,
            account: ap,
            amount: r.amount,
            contact: Some(r.vendor),
            memo: None,
        }),
        None => d.lines.push(SplitLine {
            account: ap,
            amount: r.amount,
            memo: None,
            contact: Some(r.vendor),
            billable: false,
            tag: None,
        }),
    }
    super::deposits::create(state, &d, password)
}

/// The engine's 409 for a refund above the credit's open balance.
pub(super) fn credit_exceeded() -> ApiError {
    conflict_code(ConflictReason::Unspecified, "VENDOR_CREDIT_EXCEEDED")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(s: &str) -> Money {
        Money::parse_wire(s).expect("m")
    }

    fn date(y: i32, mo: u32, d: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, mo, d).expect("d")
    }

    /// Mirrors `contract_money_helpers` (equity roles): all seven keys,
    /// the sample's own accounts by role, and seeding by entity type.
    #[test]
    fn equity_roles_follow_the_entity_type() {
        let mut state = DemoState::new();
        let roles = equity_roles(&mut state);
        assert_eq!(roles.entity_type, EntityType::SingleMemberLlc);
        assert_eq!(roles.roles.len(), 7);
        assert_eq!(
            roles.account(EquityRole::OwnerDraw),
            Some(super::super::account_id(3200))
        );
        assert_eq!(
            roles.account(EquityRole::OwnerContribution),
            Some(super::super::account_id(3100))
        );
        assert_eq!(
            roles.account(EquityRole::RetainedEarnings),
            Some(super::super::account_id(3900))
        );
        assert_eq!(roles.account(EquityRole::PartnerDraws), None);

        // A sole proprietor keeps the same accounts (idempotent).
        let before = state.chart.as_ref().map(Vec::len);
        set_entity_type(&mut state, EntityType::SoleProp);
        assert_eq!(state.chart.as_ref().map(Vec::len), before);

        // A partnership gets its two role accounts, once.
        set_entity_type(&mut state, EntityType::Partnership);
        set_entity_type(&mut state, EntityType::Partnership);
        let roles = equity_roles(&mut state);
        assert_eq!(roles.entity_type, EntityType::Partnership);
        assert_eq!(
            roles.account(EquityRole::PartnerDraws),
            Some(super::super::account_id(30180))
        );
        assert!(roles.account(EquityRole::PartnerContributions).is_some());
        assert_eq!(state.chart.as_ref().map(Vec::len), before.map(|n| n + 2));

        // S and C corporations share one Paid-in Capital.
        set_entity_type(&mut state, EntityType::SCorp);
        set_entity_type(&mut state, EntityType::CCorp);
        let roles = equity_roles(&mut state);
        assert!(
            roles
                .account(EquityRole::ShareholderDistributions)
                .is_some()
        );
        assert!(roles.account(EquityRole::PaidInCapital).is_some());
        assert_eq!(state.chart.as_ref().map(Vec::len), before.map(|n| n + 4));
        for r in EquityRole::ALL {
            assert_eq!(EquityRole::from_tag(r.tag()), Some(r));
        }
    }

    /// Mirrors the vendor-refund contract: a refund up to the credit's
    /// open balance posts Dr bank / Cr A/P; more is a 409; a void gives the
    /// balance back and frees the credit.
    #[test]
    fn vendor_refunds_draw_down_the_credit() {
        let mut state = DemoState::new();
        let vendor = state
            .contacts_mut()
            .iter()
            .find(|c| c.fields.display_name == "Ozark Feed & Seed")
            .map(|c| c.id)
            .expect("a vendor");
        let bank = super::super::account_id(1010);
        let supplies = super::super::account_id(6250);
        let credit =
            record_vendor_credit(&mut state, vendor, date(2026, 11, 1), m("100.00"), supplies)
                .expect("credit");
        assert_eq!(vendor_credits(&mut state, vendor).len(), 1);
        let refund = |amount: &str| DemoVendorRefund {
            vendor,
            deposit_to: bank,
            date: date(2026, 11, 4),
            amount: m(amount),
            applies_to: Some(credit),
            memo: None,
        };
        let first = vendor_refund(&mut state, &refund("60.00"), None).expect("refund");
        assert_eq!(
            vendor_credit_open(&state, credit).map(|(_, o)| o),
            Some(m("40.00"))
        );
        let err = vendor_refund(&mut state, &refund("40.01"), None).expect_err("too much");
        assert!(matches!(
            err,
            ApiError::Conflict { code: Some(ref c), .. } if c.as_str() == "VENDOR_CREDIT_EXCEEDED"
        ));
        // The credit can't be voided while a refund stands.
        assert!(state.ledger.void(credit).is_err());
        let rec = super::super::deposits::record(&mut state, first).expect("record");
        assert_eq!(rec.total, m("60.00"));
        super::super::deposits::void(&mut state, first, None).expect("void");
        assert_eq!(
            vendor_credit_open(&state, credit).map(|(_, o)| o),
            Some(m("100.00"))
        );
        // An overpayment refund is a plain A/P line naming the vendor.
        let mut over = refund("15.00");
        over.applies_to = None;
        vendor_refund(&mut state, &over, None).expect("overpayment");
    }
}
