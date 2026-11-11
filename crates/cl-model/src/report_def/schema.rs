//! Definition schema v1 (A07 plan §3): the serde mirror of the engine's
//! strict zod schema. Every struct refuses unknown fields and duplicate
//! keys, so a file the engine would refuse is refused here too.
//!
//! Shapes the plan does not pin yet are marked "contract guess" and are
//! replaced by PR-A's shared fixtures when they land.

use std::collections::BTreeMap;
use std::fmt;

use cl_core::report::{Adjusting, Basis};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::reports::DatePreset;

/// `kind`: always `builder` for this schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuilderKind {
    /// A Report Builder definition.
    Builder,
}

/// A Report Builder definition (schema v1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuilderDefinition {
    /// Schema version (1).
    pub schema_version: u32,
    /// `builder`.
    pub kind: BuilderKind,
    /// The report's title (owner text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Range or as-of, with an optional preset.
    pub dates: Dates,
    /// Cash or accrual (`None`: the company preference).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<Basis>,
    /// The adjusting-entries lens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjusting: Option<Adjusting>,
    /// Columns.
    pub columns: Columns,
    /// The tree.
    pub nodes: Vec<Node>,
}

/// Date mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DateMode {
    /// A from-to range.
    Range,
    /// One as-of date.
    AsOf,
}

/// The report's dates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Dates {
    /// Range or as-of.
    pub mode: DateMode,
    /// A preset (`None`: the dates are chosen when it runs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<DatePreset>,
}

/// Periods side by side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "period lengths, as named")]
pub enum Periods {
    Month,
    Quarter,
    Year,
}

/// What a compare set compares with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareBase {
    /// The period before.
    PriorPeriod,
    /// The same period a year before.
    PriorYear,
}

/// One compare set (≤ 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompareSet {
    /// Compared with.
    pub base: CompareBase,
    /// Show the compared amount.
    #[serde(default)]
    pub amount: bool,
    /// Show the $ change.
    #[serde(default)]
    pub dollar_change: bool,
    /// Show the % change.
    #[serde(default)]
    pub percent_change: bool,
}

/// The report's columns.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Columns {
    /// Detail fields in order (≤ 24).
    #[serde(default)]
    pub detail_fields: Vec<String>,
    /// Periods side by side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub periods: Option<Periods>,
    /// Aging-bucket columns (open-item sections only, never with periods).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aging_buckets: Option<bool>,
    /// Compare sets (≤ 2).
    #[serde(default)]
    pub compare: Vec<CompareSet>,
    /// `% of`: a money node's id, or `section`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent_of: Option<String>,
    /// Renamed headers: column key to caption (≤ 80 characters).
    #[serde(default, deserialize_with = "unique_map")]
    pub headers: BTreeMap<String, String>,
}

/// A transaction source (plan §4 table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "sources, as named in the plan's §4 table")]
pub enum SourceKey {
    Sales,
    ItemSales,
    Purchases,
    ItemPurchases,
    ArOpen,
    ApOpen,
    Gl,
    Bank,
    Tax,
    /// Greyed out in v1 (Q-A07g).
    Inventory,
}

impl SourceKey {
    /// Every source, in menu order.
    pub const ALL: [SourceKey; 10] = [
        SourceKey::Sales,
        SourceKey::ItemSales,
        SourceKey::Purchases,
        SourceKey::ItemPurchases,
        SourceKey::ArOpen,
        SourceKey::ApOpen,
        SourceKey::Gl,
        SourceKey::Bank,
        SourceKey::Tax,
        SourceKey::Inventory,
    ];

    /// An open-item source (as of a date, Q-307).
    #[must_use]
    pub const fn is_open_items(self) -> bool {
        matches!(self, SourceKey::ArOpen | SourceKey::ApOpen)
    }

    /// The wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            SourceKey::Sales => "sales",
            SourceKey::ItemSales => "item_sales",
            SourceKey::Purchases => "purchases",
            SourceKey::ItemPurchases => "item_purchases",
            SourceKey::ArOpen => "ar_open",
            SourceKey::ApOpen => "ap_open",
            SourceKey::Gl => "gl",
            SourceKey::Bank => "bank",
            SourceKey::Tax => "tax",
            SourceKey::Inventory => "inventory",
        }
    }
}

/// What a section adds up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "measures, as named in the plan's §4 table")]
pub enum Measure {
    Amount,
    Quantity,
    Count,
    OpenBalance,
    OriginalAmount,
    Debit,
    Credit,
    Balance,
    TbNet,
}

/// The unit a measure is in (formulas can't mix them, `FORMULA_UNITS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    /// Dollars.
    Money,
    /// A count of rows.
    Count,
    /// Item quantity.
    Quantity,
}

impl Measure {
    /// Its unit.
    #[must_use]
    pub const fn unit(self) -> Unit {
        match self {
            Measure::Count => Unit::Count,
            Measure::Quantity => Unit::Quantity,
            _ => Unit::Money,
        }
    }

    /// The wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Measure::Amount => "amount",
            Measure::Quantity => "quantity",
            Measure::Count => "count",
            Measure::OpenBalance => "open_balance",
            Measure::OriginalAmount => "original_amount",
            Measure::Debit => "debit",
            Measure::Credit => "credit",
            Measure::Balance => "balance",
            Measure::TbNet => "tb_net",
        }
    }
}

/// Which way a section's amounts are positive (Q-314).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Polarity {
    /// Each account's normal side.
    Natural,
    /// Debits positive.
    Debit,
    /// Credits positive.
    Credit,
}

/// How a section shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Display {
    /// Group lines with subtotals, no detail.
    Summary,
    /// Every transaction.
    Detail,
    /// Summary lines that open to their transactions.
    Expandable,
}

/// The engine's earnings rows in an equity section (Q-313).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Earnings {
    /// Prior years' and this year's net income (Balance Sheet).
    BalanceSheet,
    /// Retained earnings (prior years) only (Trial Balance, Q-A07n).
    TrialBalance,
}

/// A filter operator. Contract guess: PR-A's `schemas.ts` pins the set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "operators, as named")]
pub enum FilterOp {
    In,
    NotIn,
    Eq,
    Gte,
    Lte,
}

/// One filter (≤ 20 per section).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Filter {
    /// The field it filters ([`super::fields`]).
    pub field: String,
    /// How.
    pub op: FilterOp,
    /// Entity ids (≤ 500 UUIDs; the only place ids appear).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
    /// Enumerated values (≤ 500: transaction types, account types,
    /// buckets, due status, cleared state).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
    /// One value (an exact amount or a date, as text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// One group-by (≤ 3 per section).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupBy {
    /// The key ([`super::fields`]).
    pub key: String,
    /// The tag group, for a tag group-by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_group_id: Option<String>,
}

/// One formula term (≤ 20).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Term {
    /// An earlier node.
    pub node_id: String,
    /// 1 to add, -1 to subtract.
    pub sign: i8,
}

/// A source section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Section {
    /// Node id.
    pub id: String,
    /// Caption (owner text).
    pub caption: String,
    /// Source.
    pub source: SourceKey,
    /// Measure.
    pub measure: Measure,
    /// Sign (`None`: the source's default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polarity: Option<Polarity>,
    /// Filters.
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// Group-bys, outermost first.
    #[serde(default)]
    pub group_by: Vec<GroupBy>,
    /// Display.
    pub display: Display,
    /// Expandable lines start open.
    #[serde(default)]
    pub default_expanded: bool,
    /// Show a total row.
    #[serde(default)]
    pub show_total: bool,
    /// The total's caption (owner text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_caption: Option<String>,
    /// Hide rows that are all zero.
    #[serde(default)]
    pub hide_zero: bool,
    /// The engine's earnings rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earnings: Option<Earnings>,
}

/// A heading over other nodes, with a total.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Group {
    /// Node id.
    pub id: String,
    /// Caption.
    pub caption: String,
    /// The nodes under it.
    #[serde(default)]
    pub children: Vec<Node>,
    /// Show a total row.
    #[serde(default)]
    pub show_total: bool,
    /// The total's caption.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_caption: Option<String>,
}

/// Adds and subtracts earlier rows' totals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Formula {
    /// Node id.
    pub id: String,
    /// Caption.
    pub caption: String,
    /// Terms.
    pub terms: Vec<Term>,
}

/// A text row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextRow {
    /// Node id.
    pub id: String,
    /// The text (≤ 500 characters).
    pub text: String,
}

/// A blank row or a page break.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Spacer {
    /// Node id.
    pub id: String,
}

/// One node of the tree. The discriminator is `type` (contract guess).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    /// A source section.
    Section(Section),
    /// A group.
    Group(Group),
    /// A formula.
    Formula(Formula),
    /// A text row.
    Text(TextRow),
    /// A blank row.
    Blank(Spacer),
    /// A page break.
    PageBreak(Spacer),
}

impl Node {
    /// Its id.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Node::Section(s) => &s.id,
            Node::Group(g) => &g.id,
            Node::Formula(f) => &f.id,
            Node::Text(t) => &t.id,
            Node::Blank(s) | Node::PageBreak(s) => &s.id,
        }
    }

    /// Its children (groups only).
    #[must_use]
    pub fn children(&self) -> &[Node] {
        match self {
            Node::Group(g) => &g.children,
            _ => &[],
        }
    }

    /// The wire name of its type.
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Node::Section(_) => "section",
            Node::Group(_) => "group",
            Node::Formula(_) => "formula",
            Node::Text(_) => "text",
            Node::Blank(_) => "blank",
            Node::PageBreak(_) => "page_break",
        }
    }
}

/// A string map that refuses a repeated key (serde maps keep the last one).
fn unique_map<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<String, String>, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a map of column keys to captions")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
            let mut out = BTreeMap::new();
            while let Some((k, v)) = m.next_entry::<String, String>()? {
                if out.contains_key(&k) {
                    return Err(serde::de::Error::custom(format!("duplicate field `{k}`")));
                }
                out.insert(k, v);
            }
            Ok(out)
        }
    }
    d.deserialize_map(V)
}
