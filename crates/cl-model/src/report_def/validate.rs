//! The definition rules: a port of the engine's `checkBuilderRules`
//! (`packages/shared/src/report-builder/schemas.ts`), in the same order and
//! with the same `{path, code}`, plus the shape checks zod makes that serde
//! can't (lengths in UTF-16 units, patterns, enumerations, caps), reported
//! as `SCHEMA` first, as the engine does. The engine decides; this copy
//! lets the editor (A08) explain a problem before anything is sent.

use std::collections::{HashMap, HashSet};

use super::fields::{self, Sign};
use super::outline::{self, Visit};
use super::schema::{
    BuilderDefinition, Display, Earnings, FilterOp, Measure, Node, Polarity, Section, SourceKey,
    Unit,
};

/// Caps (`RB_MAX_*`).
pub mod limits {
    /// Nodes of every kind at any depth.
    pub const NODES: usize = 200;
    /// Deepest `children` nesting (a top-level node is depth 1).
    pub const DEPTH: usize = 8;
    /// Ids per filter.
    pub const IDS: usize = 500;
    /// Values per filter.
    pub const VALUES: usize = 500;
    /// Filters per section.
    pub const FILTERS: usize = 20;
    /// Group-bys per section.
    pub const GROUP_BYS: usize = 3;
    /// Detail fields.
    pub const DETAIL_FIELDS: usize = 24;
    /// Compare sets.
    pub const COMPARE: usize = 2;
    /// Terms per formula.
    pub const TERMS: usize = 20;
    /// Renamed headers.
    pub const HEADERS: usize = 64;
    /// UTF-16 units in a caption or title.
    pub const CAPTION: usize = 200;
    /// UTF-16 units in a header caption.
    pub const HEADER_CAPTION: usize = 80;
    /// UTF-16 units in a text row.
    pub const TEXT: usize = 500;
    /// UTF-16 units in an enumeration value.
    pub const VALUE_LEN: usize = 40;
    /// Period columns a run may ask for.
    pub const PERIODS: usize = 36;
}

/// Why a definition is invalid (`RB_ERROR_CODES`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs, reason = "the engine's codes, as named")]
pub enum Code {
    Schema,
    TooManyNodes,
    TooDeep,
    DuplicateId,
    UuidInText,
    NotInSource,
    FilterOp,
    FormulaForwardRef,
    FormulaUnits,
    PercentBase,
    EarningsFilter,
    BucketsPeriods,
    RunningBalance,
    MixedPolarity,
}

impl Code {
    /// Every code.
    pub const ALL: [Code; 14] = [
        Code::Schema,
        Code::TooManyNodes,
        Code::TooDeep,
        Code::DuplicateId,
        Code::UuidInText,
        Code::NotInSource,
        Code::FilterOp,
        Code::FormulaForwardRef,
        Code::FormulaUnits,
        Code::PercentBase,
        Code::EarningsFilter,
        Code::BucketsPeriods,
        Code::RunningBalance,
        Code::MixedPolarity,
    ];

    /// The engine's code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Code::Schema => "SCHEMA",
            Code::TooManyNodes => "TOO_MANY_NODES",
            Code::TooDeep => "TOO_DEEP",
            Code::DuplicateId => "DUPLICATE_ID",
            Code::UuidInText => "UUID_IN_TEXT",
            Code::NotInSource => "NOT_IN_SOURCE",
            Code::FilterOp => "FILTER_OP",
            Code::FormulaForwardRef => "FORMULA_FORWARD_REF",
            Code::FormulaUnits => "FORMULA_UNITS",
            Code::PercentBase => "PERCENT_BASE",
            Code::EarningsFilter => "EARNINGS_FILTER",
            Code::BucketsPeriods => "BUCKETS_PERIODS",
            Code::RunningBalance => "RUNNING_BALANCE",
            Code::MixedPolarity => "MIXED_POLARITY",
        }
    }

    /// The code named `s`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Code> {
        Code::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

/// One problem: where (a path from `definition`) and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    /// Path, e.g. `definition.nodes.0.terms.1.nodeId`.
    pub path: String,
    /// Code.
    pub code: Code,
}

struct Issues(Vec<Issue>);

impl Issues {
    fn add(&mut self, path: impl Into<String>, code: Code) {
        self.0.push(Issue {
            path: path.into(),
            code,
        });
    }
}

/// UTF-16 units, as JavaScript counts a string's length.
fn units16(s: &str) -> usize {
    s.encode_utf16().count()
}

/// A caption: 1–`max` units, no space at either end (text is stored as
/// sent, never trimmed).
fn caption_ok(s: &str, max: usize) -> bool {
    !s.is_empty() && s.trim() == s && units16(s) <= max
}

/// `^[a-z0-9_-]{1,32}$`.
#[must_use]
pub fn valid_node_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn preset_ok(s: &str) -> bool {
    (1..=40).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn column_key_ok(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b':' | b'-')
        })
}

/// `^-?\d{1,15}(\.\d{1,4})?$`.
fn decimal_ok(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let digits =
        |x: &str, max: usize| (1..=max).contains(&x.len()) && x.bytes().all(|b| b.is_ascii_digit());
    digits(int, 15) && frac.is_none_or(|f| digits(f, 4))
}

/// A whole uuid, any case.
fn is_uuid(s: &str) -> bool {
    s.len() == 36 && uuid_at(s.as_bytes(), 0)
}

fn uuid_at(b: &[u8], start: usize) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
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
}

/// True when `text` holds a uuid token: ids belong only at the id paths,
/// never in owner text.
#[must_use]
pub fn contains_uuid(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() >= 36 && (0..=b.len() - 36).any(|start| uuid_at(b, start))
}

/// Every problem in `def`: the shape checks first (`SCHEMA`), then the
/// engine's rules in its order. The engine reports the first.
#[must_use]
pub fn issues(def: &BuilderDefinition) -> Vec<Issue> {
    let visits = outline::walk(&def.nodes);
    let mut out = Issues(Vec::new());
    shape(def, &visits, &mut out);
    rules(def, &visits, &mut out);
    out.0
}

fn shape(def: &BuilderDefinition, visits: &[Visit<'_>], out: &mut Issues) {
    if def
        .title
        .as_deref()
        .is_some_and(|t| !caption_ok(t, limits::CAPTION))
    {
        out.add("definition.title", Code::Schema);
    }
    if def.dates.preset.as_deref().is_some_and(|p| !preset_ok(p)) {
        out.add("definition.dates.preset", Code::Schema);
    }
    let c = &def.columns;
    for (i, f) in c.detail_fields.iter().enumerate() {
        if !fields::DETAIL_FIELDS.contains(&f.as_str()) {
            out.add(format!("definition.columns.detailFields.{i}"), Code::Schema);
        }
    }
    let unique: HashSet<&String> = c.detail_fields.iter().collect();
    if c.detail_fields.len() > limits::DETAIL_FIELDS || unique.len() != c.detail_fields.len() {
        out.add("definition.columns.detailFields", Code::Schema);
    }
    if c.aging_buckets == Some(false) {
        out.add("definition.columns.agingBuckets", Code::Schema);
    }
    if c.compare.len() > limits::COMPARE {
        out.add("definition.columns.compare", Code::Schema);
    }
    if c.percent_of
        .as_deref()
        .is_some_and(|p| p != "section" && !valid_node_id(p))
    {
        out.add("definition.columns.percentOf", Code::Schema);
    }
    for (k, v) in &c.headers {
        if !column_key_ok(k) || !caption_ok(v, limits::HEADER_CAPTION) {
            out.add(format!("definition.columns.headers.{k}"), Code::Schema);
        }
    }
    if c.headers.len() > limits::HEADERS {
        out.add("definition.columns.headers", Code::Schema);
    }
    for v in visits {
        node_shape(v, out);
    }
}

fn node_shape(v: &Visit<'_>, out: &mut Issues) {
    let p = &v.path;
    if !valid_node_id(v.node.id()) {
        out.add(format!("{p}.id"), Code::Schema);
    }
    let captions: [Option<&String>; 2] = match v.node {
        Node::Section(s) => [Some(&s.caption), s.total_caption.as_ref()],
        Node::Group(g) => [Some(&g.caption), g.total_caption.as_ref()],
        Node::Formula(f) => [Some(&f.caption), None],
        _ => [None, None],
    };
    for (key, c) in ["caption", "totalCaption"].iter().zip(captions) {
        if c.is_some_and(|c| !caption_ok(c, limits::CAPTION)) {
            out.add(format!("{p}.{key}"), Code::Schema);
        }
    }
    match v.node {
        Node::Section(s) => section_shape(s, p, out),
        Node::Formula(f) => {
            if f.terms.is_empty() || f.terms.len() > limits::TERMS {
                out.add(format!("{p}.terms"), Code::Schema);
            }
            for (i, t) in f.terms.iter().enumerate() {
                if !valid_node_id(&t.node_id) {
                    out.add(format!("{p}.terms.{i}.nodeId"), Code::Schema);
                }
                if t.sign != 1 && t.sign != -1 {
                    out.add(format!("{p}.terms.{i}.sign"), Code::Schema);
                }
            }
        }
        Node::Text(t) => {
            if units16(&t.text) > limits::TEXT {
                out.add(format!("{p}.text"), Code::Schema);
            }
        }
        Node::Group(_) | Node::Blank(_) | Node::PageBreak(_) => {}
    }
}

fn section_shape(s: &Section, p: &str, out: &mut Issues) {
    if s.filters.len() > limits::FILTERS {
        out.add(format!("{p}.filters"), Code::Schema);
    }
    for (i, f) in s.filters.iter().enumerate() {
        let fp = format!("{p}.filters.{i}");
        if !fields::FILTER_FIELDS.contains(&f.field.as_str()) {
            out.add(format!("{fp}.field"), Code::Schema);
        }
        if let Some(ids) = &f.ids {
            if let Some(j) = ids.iter().position(|x| !is_uuid(x)) {
                out.add(format!("{fp}.ids.{j}"), Code::Schema);
            }
            if ids.len() > limits::IDS {
                out.add(format!("{fp}.ids"), Code::Schema);
            }
        }
        if let Some(values) = &f.values {
            if let Some(j) = values
                .iter()
                .position(|x| x.is_empty() || units16(x) > limits::VALUE_LEN)
            {
                out.add(format!("{fp}.values.{j}"), Code::Schema);
            }
            if values.len() > limits::VALUES {
                out.add(format!("{fp}.values"), Code::Schema);
            }
        }
        if f.value.as_deref().is_some_and(|v| !decimal_ok(v)) {
            out.add(format!("{fp}.value"), Code::Schema);
        }
    }
    if s.group_by.len() > limits::GROUP_BYS {
        out.add(format!("{p}.groupBy"), Code::Schema);
    }
    for (i, g) in s.group_by.iter().enumerate() {
        if !fields::GROUP_KEYS.contains(&g.key.as_str()) {
            out.add(format!("{p}.groupBy.{i}.key"), Code::Schema);
        }
        if g.tag_group_id.as_deref().is_some_and(|x| !is_uuid(x)) {
            out.add(format!("{p}.groupBy.{i}.tagGroupId"), Code::Schema);
        }
    }
}

/// A section's sign when its polarity is left out, or `natural` on a
/// non-gl source (`effectivePolarity`). `None`: open items (owed-positive).
#[must_use]
pub fn effective_polarity(s: &Section) -> Option<Polarity> {
    match fields::of(s.source).sign {
        Sign::OwedPositive => None,
        _ if s.polarity.is_some_and(|p| p != Polarity::Natural) => s.polarity,
        Sign::Natural => Some(Polarity::Natural),
        Sign::DebitMinusCredit => Some(Polarity::Debit),
        Sign::CreditMinusDebit => Some(Polarity::Credit),
    }
}

/// A node's unit (`nodeUnits`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeUnit {
    /// One unit.
    One(Unit),
    /// A group or formula over different units.
    Mixed,
    /// No figures (text and spacing).
    Nothing,
}

/// Every node's unit, computed as the engine does: groups from their
/// children, formulas from the units of the terms already seen.
#[must_use]
pub fn node_units(nodes: &[Node]) -> HashMap<String, NodeUnit> {
    fn combine(parts: impl Iterator<Item = NodeUnit>) -> NodeUnit {
        let kids: Vec<NodeUnit> = parts.filter(|u| *u != NodeUnit::Nothing).collect();
        match kids.first() {
            None => NodeUnit::Nothing,
            Some(first) if kids.iter().all(|k| k == first) => *first,
            Some(_) => NodeUnit::Mixed,
        }
    }
    fn unit_of(n: &Node, units: &mut HashMap<String, NodeUnit>) -> NodeUnit {
        let u = match n {
            Node::Section(s) => NodeUnit::One(s.measure.unit()),
            Node::Group(g) => {
                let kids: Vec<NodeUnit> = g.children.iter().map(|c| unit_of(c, units)).collect();
                combine(kids.into_iter())
            }
            Node::Formula(f) => combine(
                f.terms
                    .iter()
                    .filter_map(|t| units.get(&t.node_id).copied()),
            ),
            Node::Text(_) | Node::Blank(_) | Node::PageBreak(_) => NodeUnit::Nothing,
        };
        units.insert(n.id().to_string(), u);
        u
    }
    let mut units = HashMap::new();
    for n in nodes {
        unit_of(n, &mut units);
    }
    units
}

fn rules(def: &BuilderDefinition, visits: &[Visit<'_>], out: &mut Issues) {
    if visits.len() > limits::NODES {
        out.add("definition.nodes", Code::TooManyNodes);
    }
    if let Some(v) = visits.iter().find(|v| v.depth > limits::DEPTH) {
        out.add(v.path.clone(), Code::TooDeep);
    }
    let mut seen = HashSet::new();
    for v in visits {
        let id = v.node.id();
        if id == "section" || !seen.insert(id) {
            out.add(format!("{}.id", v.path), Code::DuplicateId);
        }
    }
    text_rules(def, visits, out);

    let units = node_units(&def.nodes);
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (i, v) in visits.iter().enumerate() {
        // The engine's Map keeps the last node of a repeated id.
        index.insert(v.node.id(), i);
    }
    let running = def
        .columns
        .detail_fields
        .iter()
        .any(|f| f == "running_balance");
    let sections: Vec<(&Section, &str)> = visits
        .iter()
        .filter_map(|v| match v.node {
            Node::Section(s) => Some((s, v.path.as_str())),
            _ => None,
        })
        .collect();
    for (s, p) in &sections {
        section_rules(s, p, running, out);
    }

    let cols = &def.columns;
    if cols.aging_buckets == Some(true) {
        if cols.periods.is_some() || !cols.compare.is_empty() {
            out.add("definition.columns.agingBuckets", Code::BucketsPeriods);
        }
        for (s, p) in &sections {
            if !fields::of(s.source).open_items {
                out.add(format!("{p}.source"), Code::BucketsPeriods);
            }
        }
    }
    if running && (cols.periods.is_some() || !cols.compare.is_empty()) {
        out.add("definition.columns.detailFields", Code::RunningBalance);
    }
    if let Some(base) = cols.percent_of.as_deref() {
        if base != "section"
            && (!index.contains_key(base) || units.get(base) != Some(&NodeUnit::One(Unit::Money)))
        {
            out.add("definition.columns.percentOf", Code::PercentBase);
        }
        for (s, p) in &sections {
            if s.measure.unit() != Unit::Money {
                out.add(format!("{p}.measure"), Code::PercentBase);
            }
        }
    }
    formula_rules(visits, &index, &units, out);
}

fn text_rules(def: &BuilderDefinition, visits: &[Visit<'_>], out: &mut Issues) {
    if def.title.as_deref().is_some_and(contains_uuid) {
        out.add("definition.title", Code::UuidInText);
    }
    for (k, v) in &def.columns.headers {
        if contains_uuid(k) || contains_uuid(v) {
            out.add(format!("definition.columns.headers.{k}"), Code::UuidInText);
        }
    }
    for v in visits {
        let p = &v.path;
        let (caption, total) = match v.node {
            Node::Section(s) => (Some(&s.caption), s.total_caption.as_ref()),
            Node::Group(g) => (Some(&g.caption), g.total_caption.as_ref()),
            Node::Formula(f) => (Some(&f.caption), None),
            _ => (None, None),
        };
        if caption.is_some_and(|c| contains_uuid(c)) {
            out.add(format!("{p}.caption"), Code::UuidInText);
        }
        if total.is_some_and(|c| contains_uuid(c)) {
            out.add(format!("{p}.totalCaption"), Code::UuidInText);
        }
        if let Node::Text(t) = v.node
            && contains_uuid(&t.text)
        {
            out.add(format!("{p}.text"), Code::UuidInText);
        }
    }
}

fn formula_rules(
    visits: &[Visit<'_>],
    index: &HashMap<&str, usize>,
    units: &HashMap<String, NodeUnit>,
    out: &mut Issues,
) {
    for (i, v) in visits.iter().enumerate() {
        let Node::Formula(f) = v.node else { continue };
        for (ti, t) in f.terms.iter().enumerate() {
            let tp = format!("{}.terms.{ti}.nodeId", v.path);
            let ok = index.get(t.node_id.as_str()).is_some_and(|&j| {
                j < i
                    && !v.ancestors.contains(&t.node_id.as_str())
                    && matches!(
                        visits[j].node,
                        Node::Section(_) | Node::Group(_) | Node::Formula(_)
                    )
            });
            if !ok {
                out.add(tp, Code::FormulaForwardRef);
                continue;
            }
            if !matches!(
                units.get(&t.node_id),
                Some(NodeUnit::One(Unit::Money | Unit::Count | Unit::Quantity))
            ) {
                out.add(tp, Code::FormulaUnits);
            }
        }
        if units.get(&f.id) == Some(&NodeUnit::Mixed) {
            out.add(format!("{}.terms", v.path), Code::FormulaUnits);
        }
    }
}

fn filter_ok(f: &super::schema::Filter, fp: &str, out: &mut Issues) -> bool {
    let field = f.field.as_str();
    let list_op = matches!(f.op, FilterOp::In | FilterOp::NotIn);
    let (has_ids, has_values, has_value) = (f.ids.is_some(), f.values.is_some(), f.value.is_some());
    match f.op {
        FilterOp::IsNone => {
            fields::NULLABLE_FILTER_FIELDS.contains(&field) && !has_ids && !has_values && !has_value
        }
        FilterOp::Gte | FilterOp::Lte => {
            fields::SCALAR_FILTER_FIELDS.contains(&field) && has_value && !has_ids && !has_values
        }
        _ if fields::ID_FILTER_FIELDS.contains(&field) => {
            list_op && f.ids.as_ref().is_some_and(|x| !x.is_empty()) && !has_values && !has_value
        }
        _ => match fields::filter_values(field) {
            Some(allowed) => {
                for (vi, v) in f.values.iter().flatten().enumerate() {
                    if !allowed.contains(&v.as_str()) {
                        out.add(format!("{fp}.values.{vi}"), Code::FilterOp);
                    }
                }
                list_op
                    && f.values.as_ref().is_some_and(|x| !x.is_empty())
                    && !has_ids
                    && !has_value
            }
            None => false,
        },
    }
}

fn section_rules(s: &Section, p: &str, running: bool, out: &mut Issues) {
    let info = fields::of(s.source);
    if !info.measures.contains(&s.measure) {
        out.add(format!("{p}.measure"), Code::NotInSource);
    }
    if info.sign == Sign::OwedPositive && s.polarity.is_some() {
        out.add(format!("{p}.polarity"), Code::NotInSource);
    }
    for (gi, g) in s.group_by.iter().enumerate() {
        if !info.group_bys.contains(&g.key.as_str()) {
            out.add(format!("{p}.groupBy.{gi}.key"), Code::NotInSource);
        }
        if g.tag_group_id.is_some() != (g.key == "tag") {
            out.add(format!("{p}.groupBy.{gi}.tagGroupId"), Code::Schema);
        }
    }
    let keys: HashSet<&str> = s.group_by.iter().map(|g| g.key.as_str()).collect();
    if keys.len() != s.group_by.len() {
        out.add(format!("{p}.groupBy"), Code::Schema);
    }
    for (fi, f) in s.filters.iter().enumerate() {
        let fp = format!("{p}.filters.{fi}");
        if !info.filters.contains(&f.field.as_str()) {
            out.add(format!("{fp}.field"), Code::NotInSource);
        }
        if !filter_ok(f, &fp, out) {
            out.add(format!("{fp}.op"), Code::FilterOp);
        }
    }
    if let Some(e) = s.earnings {
        let need = match e {
            Earnings::BalanceSheet => Measure::Balance,
            Earnings::TrialBalance => Measure::TbNet,
        };
        if s.source != SourceKey::Gl || s.measure != need {
            out.add(format!("{p}.earnings"), Code::EarningsFilter);
        }
        for (fi, f) in s.filters.iter().enumerate() {
            if !matches!(f.field.as_str(), "tag" | "account" | "account_type") {
                out.add(format!("{p}.filters.{fi}.field"), Code::EarningsFilter);
            }
        }
    }
    if s.display != Display::Summary && running {
        let inner = s.group_by.last().map(|g| g.key.as_str());
        if !matches!(s.source, SourceKey::Gl | SourceKey::Bank) || inner != Some("account") {
            out.add(format!("{p}.groupBy"), Code::RunningBalance);
        }
    }
    if effective_polarity(s) == Some(Polarity::Natural)
        && matches!(s.measure, Measure::Amount | Measure::Balance)
    {
        let type_filter = s
            .filters
            .iter()
            .find(|f| f.field == "account_type" && f.op == FilterOp::In);
        let account_filter = s
            .filters
            .iter()
            .any(|f| f.field == "account" && f.op == FilterOp::In);
        let sides: HashSet<bool> = type_filter
            .and_then(|f| f.values.as_ref())
            .into_iter()
            .flatten()
            .map(|t| fields::DEBIT_NORMAL_TYPES.contains(&t.as_str()))
            .collect();
        // An account filter is checked by the run (one normal side).
        let one_side = type_filter.is_some() && sides.len() == 1;
        if !one_side && !account_filter {
            out.add(format!("{p}.polarity"), Code::MixedPolarity);
        }
    }
}
