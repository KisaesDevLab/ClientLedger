//! The field catalogue: the mirror of the engine's
//! `packages/shared/src/report-builder/fields.ts` (`RB_SOURCE_INFO` and the
//! key lists). Per source: its measures, filters, group-bys and detail
//! fields; what is greyed out, with the reason (an i18n key).

use cl_core::report::Basis;

use super::schema::{Measure, SourceKey};

/// Every group-by key (`RB_GROUP_KEYS`).
pub const GROUP_KEYS: [&str; 12] = [
    "account",
    "account_type",
    "contact",
    "item",
    "txn_type",
    "tag",
    "month",
    "quarter",
    "year",
    "cleared",
    "due_status",
    "aging_bucket",
];

/// Every filter field (`RB_FILTER_FIELDS`).
pub const FILTER_FIELDS: [&str; 10] = [
    "account",
    "account_type",
    "contact",
    "item",
    "tag",
    "txn_type",
    "cleared",
    "due_status",
    "aging_bucket",
    "amount",
];

/// Every detail field (`RB_DETAIL_FIELDS`); definition-wide: a field a
/// source lacks is an empty cell, not an error.
pub const DETAIL_FIELDS: [&str; 22] = [
    "date",
    "txn_type",
    "number",
    "contact",
    "memo",
    "description",
    "account",
    "item",
    "quantity",
    "unit_price",
    "tag",
    "cleared",
    "debit",
    "credit",
    "amount",
    "running_balance",
    "due_date",
    "days_past_due",
    "aging_bucket",
    "open_balance",
    "original_amount",
    "service_date",
];

/// Filter fields whose `in`/`not_in` take entity ids.
pub const ID_FILTER_FIELDS: [&str; 4] = ["account", "contact", "item", "tag"];
/// Filter fields whose `in`/`not_in` take enumeration values.
pub const VALUE_FILTER_FIELDS: [&str; 5] = [
    "account_type",
    "txn_type",
    "cleared",
    "due_status",
    "aging_bucket",
];
/// Filter fields that may be empty on a line (`is_none`).
pub const NULLABLE_FILTER_FIELDS: [&str; 3] = ["contact", "item", "tag"];
/// The one scalar filter (`gte`/`lte` with `value`).
pub const SCALAR_FILTER_FIELDS: [&str; 1] = ["amount"];

/// Account types (`RB_ACCOUNT_TYPES`).
pub const ACCOUNT_TYPES: [&str; 8] = [
    "asset",
    "liability",
    "equity",
    "revenue",
    "cogs",
    "expense",
    "other_revenue",
    "other_expense",
];
/// Account types whose natural balance is a debit; the rest are credits.
pub const DEBIT_NORMAL_TYPES: [&str; 4] = ["asset", "cogs", "expense", "other_expense"];

/// Transaction types as the builder names them (`check` is its own value).
pub const TXN_TYPES: [&str; 19] = [
    "invoice",
    "customer_payment",
    "cash_sale",
    "expense",
    "check",
    "deposit",
    "transfer",
    "journal_entry",
    "credit_memo",
    "customer_refund",
    "bill",
    "vendor_credit",
    "bill_payment",
    "daily_sales",
    "aje",
    "returned_payment",
    "sales_tax_payment",
    "bad_debt_recovery",
    "inventory_adjustment",
];
/// Cleared states.
pub const CLEARED: [&str; 3] = ["uncleared", "cleared", "reconciled"];
/// Due status.
pub const DUE_STATUS: [&str; 2] = ["current", "overdue"];
/// Aging buckets (Q-105).
pub const AGING_BUCKETS: [&str; 5] = ["current", "d1_30", "d31_60", "d61_90", "over_90"];

/// The values a value filter takes (`RB_FILTER_VALUES`).
#[must_use]
pub fn filter_values(field: &str) -> Option<&'static [&'static str]> {
    Some(match field {
        "account_type" => &ACCOUNT_TYPES,
        "txn_type" => &TXN_TYPES,
        "cleared" => &CLEARED,
        "due_status" => &DUE_STATUS,
        "aging_bucket" => &AGING_BUCKETS,
        _ => return None,
    })
}

/// A source's default sign (`RbSign`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sign {
    /// Credits positive.
    CreditMinusDebit,
    /// Debits positive.
    DebitMinusCredit,
    /// Each account's normal side.
    Natural,
    /// Open items: amounts owed are positive.
    OwedPositive,
}

/// What a source offers (`RbSourceInfo`).
#[derive(Clone, Copy, Debug)]
pub struct SourceFields {
    /// The source.
    pub source: SourceKey,
    /// Its default sign.
    pub sign: Sign,
    /// Open items as of a date.
    pub open_items: bool,
    /// Its measures.
    pub measures: &'static [Measure],
    /// Measures refused on cash basis.
    pub accrual_only: &'static [Measure],
    /// Its filter fields.
    pub filters: &'static [&'static str],
    /// Its group-by keys.
    pub group_bys: &'static [&'static str],
    /// The detail fields it fills.
    pub detail_fields: &'static [&'static str],
}

const LINE_FIELDS: &[&str] = &[
    "date",
    "txn_type",
    "number",
    "contact",
    "memo",
    "description",
    "account",
    "item",
    "tag",
    "debit",
    "credit",
    "amount",
    "service_date",
];
const ITEM_LINE_FIELDS: &[&str] = &[
    "date",
    "txn_type",
    "number",
    "contact",
    "memo",
    "description",
    "account",
    "item",
    "tag",
    "debit",
    "credit",
    "amount",
    "service_date",
    "quantity",
    "unit_price",
];
const LEDGER_FIELDS: &[&str] = &[
    "date",
    "txn_type",
    "number",
    "contact",
    "memo",
    "description",
    "account",
    "item",
    "tag",
    "debit",
    "credit",
    "amount",
    "service_date",
    "cleared",
    "running_balance",
];
const LINE_GROUPS: &[&str] = &[
    "account",
    "account_type",
    "contact",
    "item",
    "txn_type",
    "tag",
    "month",
    "quarter",
    "year",
];
const BANK_GROUPS: &[&str] = &[
    "account",
    "account_type",
    "contact",
    "item",
    "txn_type",
    "tag",
    "month",
    "quarter",
    "year",
    "cleared",
];
const LINE_FILTERS: &[&str] = &[
    "account",
    "account_type",
    "contact",
    "item",
    "tag",
    "txn_type",
    "amount",
];
const LEDGER_FILTERS: &[&str] = &[
    "account",
    "account_type",
    "contact",
    "item",
    "tag",
    "txn_type",
    "amount",
    "cleared",
];
const OPEN_FIELDS: &[&str] = &[
    "date",
    "txn_type",
    "number",
    "contact",
    "memo",
    "due_date",
    "days_past_due",
    "aging_bucket",
    "open_balance",
    "original_amount",
];
// On open items a tag is a document filter only; group by tag is not
// offered (Q-A07c).
const OPEN_FILTERS: &[&str] = &["contact", "tag", "txn_type", "due_status", "aging_bucket"];
const OPEN_GROUPS: &[&str] = &["contact", "txn_type", "due_status", "aging_bucket"];
const SALES_MEASURES: &[Measure] = &[Measure::Amount, Measure::Quantity, Measure::Count];
const OPEN_MEASURES: &[Measure] = &[
    Measure::OpenBalance,
    Measure::OriginalAmount,
    Measure::Count,
];

/// The catalogue, one entry per source, in [`SourceKey::ALL`] order.
pub const SOURCES: [SourceFields; 9] = [
    SourceFields {
        source: SourceKey::Sales,
        sign: Sign::CreditMinusDebit,
        open_items: false,
        measures: SALES_MEASURES,
        accrual_only: &[Measure::Quantity],
        filters: LINE_FILTERS,
        group_bys: LINE_GROUPS,
        detail_fields: ITEM_LINE_FIELDS,
    },
    SourceFields {
        source: SourceKey::ItemSales,
        sign: Sign::CreditMinusDebit,
        open_items: false,
        measures: SALES_MEASURES,
        accrual_only: &[Measure::Quantity],
        filters: LINE_FILTERS,
        group_bys: LINE_GROUPS,
        detail_fields: ITEM_LINE_FIELDS,
    },
    SourceFields {
        source: SourceKey::Purchases,
        sign: Sign::DebitMinusCredit,
        open_items: false,
        measures: &[Measure::Amount, Measure::Count],
        accrual_only: &[],
        filters: LINE_FILTERS,
        group_bys: LINE_GROUPS,
        detail_fields: LINE_FIELDS,
    },
    SourceFields {
        source: SourceKey::ItemPurchases,
        sign: Sign::DebitMinusCredit,
        open_items: false,
        measures: SALES_MEASURES,
        accrual_only: &[Measure::Quantity],
        filters: LINE_FILTERS,
        group_bys: LINE_GROUPS,
        detail_fields: ITEM_LINE_FIELDS,
    },
    SourceFields {
        source: SourceKey::ArOpen,
        sign: Sign::OwedPositive,
        open_items: true,
        measures: OPEN_MEASURES,
        accrual_only: &[],
        filters: OPEN_FILTERS,
        group_bys: OPEN_GROUPS,
        detail_fields: OPEN_FIELDS,
    },
    SourceFields {
        source: SourceKey::ApOpen,
        sign: Sign::OwedPositive,
        open_items: true,
        measures: OPEN_MEASURES,
        accrual_only: &[],
        filters: OPEN_FILTERS,
        group_bys: OPEN_GROUPS,
        detail_fields: OPEN_FIELDS,
    },
    SourceFields {
        source: SourceKey::Gl,
        sign: Sign::Natural,
        open_items: false,
        measures: &[
            Measure::Amount,
            Measure::Debit,
            Measure::Credit,
            Measure::Balance,
            Measure::TbNet,
            Measure::Count,
        ],
        accrual_only: &[],
        filters: LEDGER_FILTERS,
        group_bys: LINE_GROUPS,
        detail_fields: LEDGER_FIELDS,
    },
    SourceFields {
        source: SourceKey::Bank,
        sign: Sign::DebitMinusCredit,
        open_items: false,
        measures: &[Measure::Amount, Measure::Balance, Measure::Count],
        accrual_only: &[],
        filters: LEDGER_FILTERS,
        group_bys: BANK_GROUPS,
        detail_fields: LEDGER_FIELDS,
    },
    SourceFields {
        source: SourceKey::Tax,
        sign: Sign::CreditMinusDebit,
        open_items: false,
        measures: &[Measure::Amount, Measure::Count],
        accrual_only: &[],
        filters: LINE_FILTERS,
        group_bys: LINE_GROUPS,
        detail_fields: LINE_FIELDS,
    },
];

/// Sources shown greyed out in v1, with the reason (i18n keys): not
/// accepted by the schema (Q-A07g, U-35).
pub const UNAVAILABLE_SOURCES: [(&str, &str); 2] = [
    ("inventory", "reports.builder.greyed.inventory"),
    ("project", "reports.builder.greyed.project"),
];

/// What `source` offers.
#[must_use]
pub const fn of(source: SourceKey) -> &'static SourceFields {
    match source {
        SourceKey::Sales => &SOURCES[0],
        SourceKey::ItemSales => &SOURCES[1],
        SourceKey::Purchases => &SOURCES[2],
        SourceKey::ItemPurchases => &SOURCES[3],
        SourceKey::ArOpen => &SOURCES[4],
        SourceKey::ApOpen => &SOURCES[5],
        SourceKey::Gl => &SOURCES[6],
        SourceKey::Bank => &SOURCES[7],
        SourceKey::Tax => &SOURCES[8],
    }
}

/// Whether a choice can be used, or why not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// Usable.
    Available,
    /// Shown greyed with this reason (an i18n key).
    Greyed(&'static str),
}

impl Availability {
    /// Usable.
    #[must_use]
    pub const fn is_available(self) -> bool {
        matches!(self, Availability::Available)
    }
}

/// Whether `measure` can be used on `source` on `basis` (the editor greys
/// it; the run refuses quantity on cash with `NOT_ON_CASH`).
#[must_use]
pub fn measure_availability(
    source: SourceKey,
    measure: Measure,
    basis: Option<Basis>,
) -> Availability {
    let f = of(source);
    if !f.measures.contains(&measure) {
        return Availability::Greyed("reports.builder.greyed.not_for_source");
    }
    if basis == Some(Basis::Cash) && f.accrual_only.contains(&measure) {
        return Availability::Greyed("reports.builder.greyed.quantity_cash");
    }
    Availability::Available
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_source_has_its_entry() {
        for s in SourceKey::ALL {
            assert_eq!(of(s).source, s);
        }
        for f in &SOURCES {
            assert!(f.filters.iter().all(|x| FILTER_FIELDS.contains(x)));
            assert!(f.group_bys.iter().all(|x| GROUP_KEYS.contains(x)));
            assert!(f.detail_fields.iter().all(|x| DETAIL_FIELDS.contains(x)));
        }
    }

    #[test]
    fn quantity_is_accrual_only() {
        assert_eq!(
            measure_availability(SourceKey::Sales, Measure::Quantity, Some(Basis::Cash)),
            Availability::Greyed("reports.builder.greyed.quantity_cash")
        );
        assert!(
            measure_availability(SourceKey::Sales, Measure::Quantity, Some(Basis::Accrual))
                .is_available()
        );
        assert!(!measure_availability(SourceKey::Gl, Measure::Quantity, None).is_available());
        assert_eq!(filter_values("cleared"), Some(&CLEARED[..]));
        assert_eq!(filter_values("contact"), None);
    }
}
