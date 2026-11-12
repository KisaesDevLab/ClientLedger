//! What the editor's pickers offer, and why some choices are greyed (plan
//! §3.1 "Availability"), plus the one money input: the amount filter
//! (plan §4).

use cl_core::Money;
use cl_core::money::UserInput;

use crate::report_def::fields::{self, Availability};
use crate::report_def::outline;
use crate::report_def::schema::{
    BuilderDefinition, Display, FilterOp, Node, Section, SourceKey, Unit,
};
use crate::report_def::validate::{NodeUnit, node_units};

/// Reasons a choice is greyed (i18n keys).
pub mod greyed {
    /// A formula term below the formula, or a group the formula sits in.
    pub const BELOW: &str = "builder.greyed.below";
    /// A formula term that adds up a different kind of thing.
    pub const OTHER_UNIT: &str = "builder.greyed.other_unit";
    /// A row with no figures (text, blank, page break).
    pub const NO_FIGURES: &str = "builder.greyed.no_figures";
    /// Aging columns need every section to be open items.
    pub const AGING_OPEN_ITEMS: &str = "builder.greyed.aging_open_items";
    /// Aging columns don't go with columns across or compare.
    pub const AGING_PERIODS: &str = "builder.greyed.aging_periods";
    /// Columns across or compare don't go with aging columns.
    pub const PERIODS_AGING: &str = "builder.greyed.periods_aging";
    /// Columns across or compare don't go with a running balance.
    pub const PERIODS_RUNNING: &str = "builder.greyed.periods_running";
    /// A running balance needs ledger sections grouped by account last.
    pub const RUNNING_BALANCE: &str = "builder.greyed.running_balance";
    /// "% of" works only when every section adds up dollars.
    pub const PERCENT_MONEY: &str = "builder.greyed.percent_money";
    /// Not offered by this source (A07's key).
    pub const NOT_FOR_SOURCE: &str = "reports.builder.greyed.not_for_source";
}

/// A formula term choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermChoice {
    /// The row.
    pub id: String,
    /// Usable, or why not.
    pub availability: Availability,
}

/// The rows a formula's "Add a row" picker offers, in outline order:
/// earlier sections, groups and formulas in the unit its terms already
/// add up; the rest greyed with the reason.
#[must_use]
pub fn formula_terms(def: &BuilderDefinition, formula_id: &str) -> Vec<TermChoice> {
    let visits = outline::walk(&def.nodes);
    let units = node_units(&def.nodes);
    let Some(me) = visits.iter().find(|v| v.node.id() == formula_id) else {
        return Vec::new();
    };
    let mine: Option<NodeUnit> = match me.node {
        Node::Formula(f) => f
            .terms
            .iter()
            .find_map(|t| units.get(&t.node_id).copied())
            .filter(|u| matches!(u, NodeUnit::One(_))),
        _ => None,
    };
    visits
        .iter()
        .filter(|v| v.node.id() != formula_id)
        .map(|v| {
            let id = v.node.id();
            let availability =
                if !matches!(v.node, Node::Section(_) | Node::Group(_) | Node::Formula(_)) {
                    Availability::Greyed(greyed::NO_FIGURES)
                } else if v.order > me.order || me.ancestors.contains(&id) {
                    Availability::Greyed(greyed::BELOW)
                } else {
                    let u = units.get(id).copied();
                    let usable = matches!(
                        u,
                        Some(NodeUnit::One(Unit::Money | Unit::Count | Unit::Quantity))
                    ) && mine.is_none_or(|m| Some(m) == u);
                    if usable {
                        Availability::Available
                    } else {
                        Availability::Greyed(greyed::OTHER_UNIT)
                    }
                };
            TermChoice {
                id: id.to_string(),
                availability,
            }
        })
        .collect()
}

/// Every group-by key, usable or greyed for a section on `source`.
#[must_use]
pub fn group_by_choices(source: SourceKey) -> Vec<(&'static str, Availability)> {
    let offered = fields::of(source).group_bys;
    fields::GROUP_KEYS
        .iter()
        .map(|k| {
            let a = if offered.contains(k) {
                Availability::Available
            } else {
                Availability::Greyed(greyed::NOT_FOR_SOURCE)
            };
            (*k, a)
        })
        .collect()
}

/// The operators a filter on `field` takes.
#[must_use]
pub fn filter_ops(field: &str) -> &'static [FilterOp] {
    if fields::SCALAR_FILTER_FIELDS.contains(&field) {
        &[FilterOp::Gte, FilterOp::Lte]
    } else if fields::NULLABLE_FILTER_FIELDS.contains(&field) {
        &[FilterOp::In, FilterOp::NotIn, FilterOp::IsNone]
    } else {
        &[FilterOp::In, FilterOp::NotIn]
    }
}

/// What the Columns tab offers, with reasons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnRules {
    /// Aging columns.
    pub aging: Availability,
    /// Columns across (periods).
    pub periods: Availability,
    /// Compare.
    pub compare: Availability,
    /// The running balance transaction column.
    pub running_balance: Availability,
    /// "% of".
    pub percent_of: Availability,
}

/// The Columns tab's rules for `def`.
#[must_use]
pub fn column_rules(def: &BuilderDefinition) -> ColumnRules {
    let sections: Vec<&Section> = outline::walk(&def.nodes)
        .into_iter()
        .filter_map(|v| match v.node {
            Node::Section(s) => Some(s),
            _ => None,
        })
        .collect();
    let c = &def.columns;
    let aging_on = c.aging_buckets == Some(true);
    let running_on = c.detail_fields.iter().any(|f| f == "running_balance");
    let across = c.periods.is_some() || !c.compare.is_empty();
    let open_items =
        !sections.is_empty() && sections.iter().all(|s| fields::of(s.source).open_items);
    let aging = if !open_items {
        Availability::Greyed(greyed::AGING_OPEN_ITEMS)
    } else if across {
        Availability::Greyed(greyed::AGING_PERIODS)
    } else {
        Availability::Available
    };
    let periods = if aging_on {
        Availability::Greyed(greyed::PERIODS_AGING)
    } else if running_on {
        Availability::Greyed(greyed::PERIODS_RUNNING)
    } else {
        Availability::Available
    };
    let ledgers_by_account = sections.iter().all(|s| {
        s.display == Display::Summary
            || (matches!(s.source, SourceKey::Gl | SourceKey::Bank)
                && s.group_by.last().is_some_and(|g| g.key == "account"))
    });
    let running_balance = if across {
        Availability::Greyed(greyed::PERIODS_RUNNING)
    } else if ledgers_by_account {
        Availability::Available
    } else {
        Availability::Greyed(greyed::RUNNING_BALANCE)
    };
    let percent_of = if sections.iter().all(|s| s.measure.unit() == Unit::Money) {
        Availability::Available
    } else {
        Availability::Greyed(greyed::PERCENT_MONEY)
    };
    ColumnRules {
        aging,
        periods,
        compare: periods,
        running_balance,
        percent_of,
    }
}

/// The amount filter's value from what the owner typed (plan §4): read
/// with the app's money rules (up to four decimals, grouping commas, `$`,
/// a leading `-` or parentheses), then kept as the exact digits typed
/// (`1,234.50` is `1234.50`), `^-?\d{1,15}(\.\d{1,4})?$`. `None`: not an
/// amount (five decimals, `1e3`, letters, nothing).
#[must_use]
pub fn amount_filter_value(text: &str) -> Option<String> {
    Money::parse_user(text, UserInput::FULL).ok()?;
    let t = text.trim();
    let (neg, body) = if let Some(inner) = t.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        (true, inner.trim())
    } else if let Some(rest) = t.strip_prefix('-') {
        (true, rest.trim())
    } else {
        (false, t)
    };
    let body: String = body
        .trim_start_matches('$')
        .chars()
        .filter(|c| *c != ',')
        .collect();
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (if i.is_empty() { "0" } else { i }, f),
        None => (body.as_str(), ""),
    };
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if !(1..=15).contains(&int.len()) || !digits(int) || frac.len() > 4 || !digits(frac) {
        return None;
    }
    let mut out = String::with_capacity(int.len() + frac.len() + 2);
    if neg {
        out.push('-');
    }
    out.push_str(int);
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    Some(out)
}
