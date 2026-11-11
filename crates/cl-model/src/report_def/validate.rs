//! The definition rules (A07 plan §3): limits, ids, the uuid-in-text
//! refusal and the `DEFINITION_INVALID {path, code}` codes. The engine
//! checks the same rules and decides; this copy lets the editor (A08)
//! explain a problem before anything is sent.

use std::collections::{HashMap, HashSet};

use super::fields::{self, Availability};
use super::outline::{self, Visit};
use super::schema::{
    BuilderDefinition, Display, Measure, Node, Polarity, Section, SourceKey, Unit,
};

/// Caps (ADD-03 step 1, plan §3).
pub mod limits {
    /// Nodes of every kind at any depth.
    pub const NODES: usize = 200;
    /// Detail fields.
    pub const DETAIL_FIELDS: usize = 24;
    /// Compare sets.
    pub const COMPARE: usize = 2;
    /// Filters per section.
    pub const FILTERS: usize = 20;
    /// Ids or values per filter.
    pub const LIST: usize = 500;
    /// Group-bys per section.
    pub const GROUP_BYS: usize = 3;
    /// Terms per formula.
    pub const TERMS: usize = 20;
    /// Characters in a header caption.
    pub const HEADER_CAPTION: usize = 80;
    /// Characters in a text row.
    pub const TEXT: usize = 500;
    /// Period columns a run may ask for.
    pub const PERIODS: usize = 36;
}

/// Why a definition is invalid (the engine's `code`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Code {
    /// A formula term is not an earlier row, is an ancestor, or is a
    /// text, blank or page-break row.
    FormulaForwardRef,
    /// A formula mixes money with counts or quantities.
    FormulaUnits,
    /// `percentOf` is not a money node or `section`.
    PercentBase,
    /// `earnings` outside `gl` balance or `tb_net`, or with filters other
    /// than tag and adjusting.
    EarningsFilter,
    /// Aging buckets outside open items, or with periods.
    BucketsPeriods,
    /// A running balance whose innermost group is not the account.
    RunningBalance,
    /// `natural` polarity over accounts on both normal sides.
    MixedPolarity,
    /// Over a cap (contract guess: `LIMIT`).
    Limit,
    /// A node id outside `^[a-z0-9_-]{1,32}$`, or a uuid (guess).
    NodeId,
    /// Two nodes share an id (guess).
    DuplicateId,
    /// A uuid in a caption or text (guess).
    UuidInText,
    /// A field, group-by or measure the source doesn't have (guess).
    UnknownField,
    /// A choice greyed out in v1 (guess).
    Unavailable,
    /// An entity id that is not a uuid (guess).
    BadId,
    /// A formula sign other than 1 or -1 (guess).
    BadSign,
}

impl Code {
    /// The engine's code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Code::FormulaForwardRef => "FORMULA_FORWARD_REF",
            Code::FormulaUnits => "FORMULA_UNITS",
            Code::PercentBase => "PERCENT_BASE",
            Code::EarningsFilter => "EARNINGS_FILTER",
            Code::BucketsPeriods => "BUCKETS_PERIODS",
            Code::RunningBalance => "RUNNING_BALANCE",
            Code::MixedPolarity => "MIXED_POLARITY",
            Code::Limit => "LIMIT",
            Code::NodeId => "NODE_ID",
            Code::DuplicateId => "DUPLICATE_ID",
            Code::UuidInText => "UUID_IN_TEXT",
            Code::UnknownField => "UNKNOWN_FIELD",
            Code::Unavailable => "FIELD_UNAVAILABLE",
            Code::BadId => "BAD_ID",
            Code::BadSign => "BAD_SIGN",
        }
    }
}

/// One problem: where (a JSON path such as `nodes.0.terms.1`) and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    /// JSON path.
    pub path: String,
    /// Code.
    pub code: Code,
}

/// Every problem in `def`, in tree order.
#[must_use]
pub fn issues(def: &BuilderDefinition) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut push = |path: String, code: Code| out.push(Issue { path, code });
    let visits = outline::walk(&def.nodes);
    if visits.len() > limits::NODES {
        push("nodes".into(), Code::Limit);
    }
    if def.title.as_deref().is_some_and(contains_uuid) {
        push("title".into(), Code::UuidInText);
    }
    check_columns(def, &mut push);

    let mut order: HashMap<&str, &Visit<'_>> = HashMap::new();
    let mut seen = HashSet::new();
    for v in &visits {
        let id = v.node.id();
        if !valid_node_id(id) {
            push(format!("{}.id", v.path), Code::NodeId);
        } else if !seen.insert(id) {
            push(format!("{}.id", v.path), Code::DuplicateId);
        }
        order.entry(id).or_insert(v);
    }
    let units = Units::new(&visits);
    for v in &visits {
        match v.node {
            Node::Section(s) => check_section(def, s, &v.path, &mut push),
            Node::Group(g) => {
                caption(&g.caption, &format!("{}.caption", v.path), &mut push);
                if let Some(t) = &g.total_caption {
                    caption(t, &format!("{}.totalCaption", v.path), &mut push);
                }
            }
            Node::Formula(f) => {
                caption(&f.caption, &format!("{}.caption", v.path), &mut push);
                if f.terms.len() > limits::TERMS {
                    push(format!("{}.terms", v.path), Code::Limit);
                }
                let mut seen_units = HashSet::new();
                for (i, t) in f.terms.iter().enumerate() {
                    let at = format!("{}.terms.{i}", v.path);
                    if t.sign != 1 && t.sign != -1 {
                        push(format!("{at}.sign"), Code::BadSign);
                    }
                    let ok = order.get(t.node_id.as_str()).is_some_and(|target| {
                        target.order < v.order
                            && !v.ancestors.contains(&t.node_id.as_str())
                            && matches!(
                                target.node,
                                Node::Section(_) | Node::Group(_) | Node::Formula(_)
                            )
                    });
                    if ok {
                        seen_units.insert(units.of(&t.node_id));
                    } else {
                        push(at, Code::FormulaForwardRef);
                    }
                }
                if seen_units.len() > 1 || seen_units.contains(&NodeUnit::Mixed) {
                    push(format!("{}.terms", v.path), Code::FormulaUnits);
                }
            }
            Node::Text(t) => {
                if t.text.chars().count() > limits::TEXT {
                    push(format!("{}.text", v.path), Code::Limit);
                }
                if contains_uuid(&t.text) {
                    push(format!("{}.text", v.path), Code::UuidInText);
                }
            }
            Node::Blank(_) | Node::PageBreak(_) => {}
        }
    }
    if let Some(p) = &def.columns.percent_of {
        let ok = p == "section"
            || (order.contains_key(p.as_str()) && units.of(p) == NodeUnit::One(Unit::Money));
        if !ok {
            push("columns.percentOf".into(), Code::PercentBase);
        }
    }
    if def.columns.aging_buckets == Some(true) {
        for v in &visits {
            if let Node::Section(s) = v.node
                && !s.source.is_open_items()
            {
                push(format!("{}.source", v.path), Code::BucketsPeriods);
            }
        }
    }
    out
}

fn check_columns(def: &BuilderDefinition, push: &mut impl FnMut(String, Code)) {
    let c = &def.columns;
    if c.detail_fields.len() > limits::DETAIL_FIELDS {
        push("columns.detailFields".into(), Code::Limit);
    }
    let known = fields::all_detail_fields();
    for (i, f) in c.detail_fields.iter().enumerate() {
        if !known.contains(&f.as_str()) {
            push(format!("columns.detailFields.{i}"), Code::UnknownField);
        }
    }
    if c.compare.len() > limits::COMPARE {
        push("columns.compare".into(), Code::Limit);
    }
    if c.aging_buckets == Some(true) && c.periods.is_some() {
        push("columns.agingBuckets".into(), Code::BucketsPeriods);
    }
    for (k, v) in &c.headers {
        let at = format!("columns.headers.{k}");
        if v.chars().count() > limits::HEADER_CAPTION {
            push(at.clone(), Code::Limit);
        }
        if contains_uuid(v) || contains_uuid(k) {
            push(at, Code::UuidInText);
        }
    }
}

fn caption(text: &str, at: &str, push: &mut impl FnMut(String, Code)) {
    if contains_uuid(text) {
        push(at.to_string(), Code::UuidInText);
    }
}

fn check_section(
    def: &BuilderDefinition,
    s: &Section,
    path: &str,
    push: &mut impl FnMut(String, Code),
) {
    caption(&s.caption, &format!("{path}.caption"), push);
    if let Some(t) = &s.total_caption {
        caption(t, &format!("{path}.totalCaption"), push);
    }
    if let Availability::Greyed(_) = fields::source_availability(s.source) {
        push(format!("{path}.source"), Code::Unavailable);
    }
    let offered = fields::of(s.source);
    match fields::measure_availability(s.source, s.measure, def.basis) {
        Availability::Available => {}
        Availability::Greyed(_) if !offered.measures.contains(&s.measure) => {
            push(format!("{path}.measure"), Code::UnknownField);
        }
        Availability::Greyed(_) => push(format!("{path}.measure"), Code::Unavailable),
    }
    if s.filters.len() > limits::FILTERS {
        push(format!("{path}.filters"), Code::Limit);
    }
    for (i, f) in s.filters.iter().enumerate() {
        let at = format!("{path}.filters.{i}");
        if !offered.filters.contains(&f.field.as_str()) {
            push(format!("{at}.field"), Code::UnknownField);
        }
        if let Some(ids) = &f.ids {
            if ids.len() > limits::LIST {
                push(format!("{at}.ids"), Code::Limit);
            }
            if let Some(j) = ids
                .iter()
                .position(|x| cl_core::ids::parse_uuid(x).is_err())
            {
                push(format!("{at}.ids.{j}"), Code::BadId);
            }
        }
        if f.values.as_ref().is_some_and(|v| v.len() > limits::LIST) {
            push(format!("{at}.values"), Code::Limit);
        }
    }
    if s.group_by.len() > limits::GROUP_BYS {
        push(format!("{path}.groupBy"), Code::Limit);
    }
    for (i, g) in s.group_by.iter().enumerate() {
        let at = format!("{path}.groupBy.{i}");
        if !offered.group_bys.contains(&g.key.as_str()) {
            push(format!("{at}.key"), Code::UnknownField);
        } else if !fields::group_availability(s.source, &g.key).is_available() {
            push(format!("{at}.key"), Code::Unavailable);
        }
        if g.tag_group_id
            .as_deref()
            .is_some_and(|x| cl_core::ids::parse_uuid(x).is_err())
        {
            push(format!("{at}.tagGroupId"), Code::BadId);
        }
    }
    if s.earnings.is_some() {
        let measure_ok = matches!(s.measure, Measure::Balance | Measure::TbNet);
        let filters_ok = s
            .filters
            .iter()
            .all(|f| matches!(f.field.as_str(), "tag" | "adjusting"));
        if s.source != SourceKey::Gl || !measure_ok || !filters_ok {
            push(format!("{path}.earnings"), Code::EarningsFilter);
        }
    }
    if s.measure == Measure::Balance
        && s.display != Display::Summary
        && s.group_by.last().is_none_or(|g| g.key != "account")
    {
        push(format!("{path}.groupBy"), Code::RunningBalance);
    }
    if mixed_polarity(s) {
        push(format!("{path}.polarity"), Code::MixedPolarity);
    }
}

/// `natural` on a `gl` amount or balance needs its accounts on one normal
/// side. An account-id filter is left to the engine (only it knows each
/// account's type); account-type values are checked here.
fn mixed_polarity(s: &Section) -> bool {
    if s.source != SourceKey::Gl
        || s.earnings.is_some()
        || !matches!(s.measure, Measure::Amount | Measure::Balance)
        || s.polarity.is_some_and(|p| p != Polarity::Natural)
    {
        return false;
    }
    let mut sides = HashSet::new();
    let mut narrowed = false;
    for f in &s.filters {
        match (f.field.as_str(), f.op) {
            ("account", super::schema::FilterOp::In) => return false,
            ("account_type", super::schema::FilterOp::In) => {
                narrowed = true;
                for v in f.values.iter().flatten() {
                    sides.insert(normal_side(v));
                }
            }
            _ => {}
        }
    }
    !narrowed || sides.len() != 1 || sides.contains(&None)
}

/// An account type's normal side (`Some(true)`: debit).
fn normal_side(account_type: &str) -> Option<bool> {
    match account_type {
        "asset"
        | "bank"
        | "accounts_receivable"
        | "other_current_asset"
        | "fixed_asset"
        | "other_asset"
        | "expense"
        | "cogs"
        | "cost_of_goods_sold"
        | "other_expense" => Some(true),
        "liability"
        | "accounts_payable"
        | "credit_card"
        | "other_current_liability"
        | "long_term_liability"
        | "equity"
        | "revenue"
        | "income"
        | "other_revenue"
        | "other_income" => Some(false),
        _ => None,
    }
}

/// A node's unit for formulas and `percentOf`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum NodeUnit {
    One(Unit),
    Mixed,
    Nothing,
}

struct Units<'a> {
    by_id: HashMap<&'a str, &'a Node>,
}

impl<'a> Units<'a> {
    fn new(visits: &[Visit<'a>]) -> Units<'a> {
        let mut by_id = HashMap::new();
        for v in visits {
            by_id.entry(v.node.id()).or_insert(v.node);
        }
        Units { by_id }
    }

    fn of(&self, id: &str) -> NodeUnit {
        let mut seen = HashSet::new();
        self.unit(id, &mut seen)
    }

    fn unit(&self, id: &str, seen: &mut HashSet<String>) -> NodeUnit {
        if !seen.insert(id.to_string()) {
            return NodeUnit::Mixed; // a cycle can't be a unit
        }
        let Some(node) = self.by_id.get(id) else {
            return NodeUnit::Nothing;
        };
        let parts: Vec<NodeUnit> = match node {
            Node::Section(s) => return NodeUnit::One(s.measure.unit()),
            Node::Group(g) => g.children.iter().map(|c| self.unit(c.id(), seen)).collect(),
            Node::Formula(f) => f
                .terms
                .iter()
                .map(|t| self.unit(&t.node_id, seen))
                .collect(),
            Node::Text(_) | Node::Blank(_) | Node::PageBreak(_) => return NodeUnit::Nothing,
        };
        let mut found = NodeUnit::Nothing;
        for p in parts {
            found = match (found, p) {
                (x, NodeUnit::Nothing) => x,
                (NodeUnit::Nothing, y) => y,
                (NodeUnit::One(a), NodeUnit::One(b)) if a == b => NodeUnit::One(a),
                _ => NodeUnit::Mixed,
            };
        }
        found
    }
}

/// `^[a-z0-9_-]{1,32}$` and not a 32-digit uuid.
#[must_use]
pub fn valid_node_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        && !(id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// True when `text` holds a uuid token (8-4-4-4-12 hex digits): ids
/// belong only at the id paths, never in owner text.
#[must_use]
pub fn contains_uuid(text: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let b = text.as_bytes();
    if b.len() < 36 {
        return false;
    }
    (0..=b.len() - 36).any(|start| {
        let mut i = start;
        for (n, len) in GROUPS.iter().enumerate() {
            if !b[i..i + len].iter().all(u8::is_ascii_hexdigit) {
                return false;
            }
            i += len;
            if n < 4 {
                if b[i] != b'-' {
                    return false;
                }
                i += 1;
            }
        }
        true
    })
}
