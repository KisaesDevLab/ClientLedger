//! The field catalogue (A07 plan §4 source table): per source, its
//! measures, group-bys, filters and detail fields, and what is greyed out
//! with the reason (an i18n key). Mirrors the engine's `fields.ts`;
//! contract guess until PR-A's `GET /report-builder/fields` sample lands.

use cl_core::report::Basis;

use super::schema::{Measure, SourceKey};

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

/// What a source offers.
#[derive(Clone, Copy, Debug)]
pub struct SourceFields {
    /// The source.
    pub source: SourceKey,
    /// Its measures.
    pub measures: &'static [Measure],
    /// Its group-by keys.
    pub group_bys: &'static [&'static str],
    /// Its filter fields.
    pub filters: &'static [&'static str],
    /// Its detail fields.
    pub detail_fields: &'static [&'static str],
}

const ACTIVITY_GROUPS: &[&str] = &[
    "account",
    "account_type",
    "contact",
    "item",
    "txn_type",
    "project",
    "tag",
    "month",
    "quarter",
    "year",
];
const OPEN_GROUPS: &[&str] = &["contact", "due_status", "aging_bucket", "tag", "month"];
const ACTIVITY_FILTERS: &[&str] = &[
    "account",
    "account_type",
    "contact",
    "item",
    "txn_type",
    "tag",
    "adjusting",
    "amount",
];
const OPEN_FILTERS: &[&str] = &["contact", "due_status", "aging_bucket", "tag", "amount"];
const ACTIVITY_DETAIL: &[&str] = &[
    "date", "txn_type", "number", "contact", "memo", "account", "item", "tag", "quantity", "amount",
];
const OPEN_DETAIL: &[&str] = &[
    "date",
    "txn_type",
    "number",
    "contact",
    "memo",
    "due_date",
    "days_past_due",
    "original_amount",
    "open_balance",
];
const LEDGER_DETAIL: &[&str] = &[
    "date", "txn_type", "number", "contact", "memo", "account", "split", "tag", "debit", "credit",
    "amount", "balance", "cleared",
];

/// The catalogue, one entry per source.
pub const SOURCES: [SourceFields; 10] = [
    SourceFields {
        source: SourceKey::Sales,
        measures: &[Measure::Amount, Measure::Quantity, Measure::Count],
        group_bys: ACTIVITY_GROUPS,
        filters: ACTIVITY_FILTERS,
        detail_fields: ACTIVITY_DETAIL,
    },
    SourceFields {
        source: SourceKey::ItemSales,
        measures: &[Measure::Amount, Measure::Quantity, Measure::Count],
        group_bys: ACTIVITY_GROUPS,
        filters: ACTIVITY_FILTERS,
        detail_fields: ACTIVITY_DETAIL,
    },
    SourceFields {
        source: SourceKey::Purchases,
        measures: &[Measure::Amount, Measure::Count],
        group_bys: ACTIVITY_GROUPS,
        filters: ACTIVITY_FILTERS,
        detail_fields: ACTIVITY_DETAIL,
    },
    SourceFields {
        source: SourceKey::ItemPurchases,
        measures: &[Measure::Amount, Measure::Quantity, Measure::Count],
        group_bys: ACTIVITY_GROUPS,
        filters: ACTIVITY_FILTERS,
        detail_fields: ACTIVITY_DETAIL,
    },
    SourceFields {
        source: SourceKey::ArOpen,
        measures: &[
            Measure::OpenBalance,
            Measure::OriginalAmount,
            Measure::Count,
        ],
        group_bys: OPEN_GROUPS,
        filters: OPEN_FILTERS,
        detail_fields: OPEN_DETAIL,
    },
    SourceFields {
        source: SourceKey::ApOpen,
        measures: &[
            Measure::OpenBalance,
            Measure::OriginalAmount,
            Measure::Count,
        ],
        group_bys: OPEN_GROUPS,
        filters: OPEN_FILTERS,
        detail_fields: OPEN_DETAIL,
    },
    SourceFields {
        source: SourceKey::Gl,
        measures: &[
            Measure::Amount,
            Measure::Debit,
            Measure::Credit,
            Measure::Balance,
            Measure::TbNet,
            Measure::Count,
        ],
        group_bys: ACTIVITY_GROUPS,
        filters: &[
            "account",
            "account_type",
            "contact",
            "item",
            "txn_type",
            "tag",
            "adjusting",
            "amount",
            "cleared",
        ],
        detail_fields: LEDGER_DETAIL,
    },
    SourceFields {
        source: SourceKey::Bank,
        measures: &[Measure::Amount, Measure::Balance, Measure::Count],
        group_bys: &[
            "account", "contact", "txn_type", "tag", "month", "quarter", "year",
        ],
        filters: &[
            "account",
            "contact",
            "txn_type",
            "tag",
            "adjusting",
            "amount",
            "cleared",
        ],
        detail_fields: LEDGER_DETAIL,
    },
    SourceFields {
        source: SourceKey::Tax,
        measures: &[Measure::Amount, Measure::Count],
        group_bys: &["account", "contact", "txn_type", "month", "quarter", "year"],
        filters: &["account", "contact", "txn_type", "adjusting", "amount"],
        detail_fields: ACTIVITY_DETAIL,
    },
    SourceFields {
        source: SourceKey::Inventory,
        measures: &[Measure::Quantity, Measure::Amount, Measure::Count],
        group_bys: &["item", "month", "quarter", "year"],
        filters: &["item"],
        detail_fields: &["date", "txn_type", "number", "item", "quantity", "amount"],
    },
];

/// Every detail field key any source offers.
#[must_use]
pub fn all_detail_fields() -> Vec<&'static str> {
    let mut v: Vec<&str> = SOURCES
        .iter()
        .flat_map(|s| s.detail_fields.iter().copied())
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// What `source` offers.
#[must_use]
pub fn of(source: SourceKey) -> &'static SourceFields {
    // Every SourceKey has an entry (a test checks it); Gl as the fallback
    // keeps this total without a panic.
    SOURCES
        .iter()
        .find(|s| s.source == source)
        .unwrap_or(&SOURCES[6])
}

/// Whether `source` can be picked (inventory is greyed in v1, Q-A07g).
#[must_use]
pub const fn source_availability(source: SourceKey) -> Availability {
    match source {
        SourceKey::Inventory => Availability::Greyed("reports.builder.greyed.inventory"),
        _ => Availability::Available,
    }
}

/// Whether `measure` can be used on `source` on `basis`: quantity is
/// accrual-only.
#[must_use]
pub fn measure_availability(
    source: SourceKey,
    measure: Measure,
    basis: Option<Basis>,
) -> Availability {
    if !of(source).measures.contains(&measure) {
        return Availability::Greyed("reports.builder.greyed.not_for_source");
    }
    if measure == Measure::Quantity && basis == Some(Basis::Cash) {
        return Availability::Greyed("reports.builder.greyed.quantity_cash");
    }
    Availability::Available
}

/// Whether group-by `key` can be used on `source`: projects wait for U-35;
/// open items take a tag only as a document filter (Q-A07c).
#[must_use]
pub fn group_availability(source: SourceKey, key: &str) -> Availability {
    if !of(source).group_bys.contains(&key) {
        return Availability::Greyed("reports.builder.greyed.not_for_source");
    }
    match key {
        "project" => Availability::Greyed("reports.builder.greyed.project"),
        "tag" if source.is_open_items() => Availability::Greyed("reports.builder.greyed.tag_open"),
        _ => Availability::Available,
    }
}

/// True when `field` filters by entity ids (`ids[]`), false when it takes
/// `values[]` or `value`.
#[must_use]
pub fn filter_takes_ids(field: &str) -> bool {
    matches!(field, "account" | "contact" | "item" | "tag")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_source_has_one_entry() {
        for s in SourceKey::ALL {
            assert_eq!(SOURCES.iter().filter(|f| f.source == s).count(), 1, "{s:?}");
            assert_eq!(of(s).source, s);
        }
    }

    #[test]
    fn greyed_choices_say_why() {
        assert!(!source_availability(SourceKey::Inventory).is_available());
        assert_eq!(
            measure_availability(SourceKey::Sales, Measure::Quantity, Some(Basis::Cash)),
            Availability::Greyed("reports.builder.greyed.quantity_cash")
        );
        assert!(
            measure_availability(SourceKey::Sales, Measure::Quantity, Some(Basis::Accrual))
                .is_available()
        );
        assert!(!measure_availability(SourceKey::Sales, Measure::TbNet, None).is_available());
        assert!(!group_availability(SourceKey::Gl, "project").is_available());
        assert!(!group_availability(SourceKey::ArOpen, "tag").is_available());
        assert!(group_availability(SourceKey::Gl, "tag").is_available());
        assert!(all_detail_fields().contains(&"open_balance"));
    }
}
