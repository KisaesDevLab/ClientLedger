//! Guided mode (ADD-03 A08, plan §3.2, Q-A08a): five questions (what to
//! report on, grouping, what each line shows, dates, done) that build a
//! single-section definition, and the reverse check that offers Guided
//! view only for that shape (Q-A08l).
//!
//! - One section, its source's default measure. "All accounts" is shown
//!   as debits positive (`polarity: debit`, Q-A08u): one natural section
//!   over every account would mix normal sides (`MIXED_POLARITY`).
//! - Open items ("Who owes me", "What I owe") run as of a date, with no
//!   columns across or compare, and aging columns on by default (the
//!   mockup has no Guided control for them, Q-A08i).
//!
//! No egui, no I/O.

use std::collections::BTreeMap;

use cl_core::report::Basis;

use super::fields;
use super::schema::{
    BuilderDefinition, BuilderKind, Columns, CompareBase, CompareSet, DateMode, Dates, Display,
    GroupBy, Measure, Node, Periods, Polarity, Section, SourceKey,
};
use super::validate::limits;
use super::{SCHEMA_VERSION, editor::blank_definition};

/// The guide's steps, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Step {
    /// What do you want to report on?
    Source,
    /// How should it be grouped?
    Grouping,
    /// What should each line show?
    Lines,
    /// Which dates?
    Dates,
    /// Name it; Run report, Memorize or Open in Advanced.
    Done,
}

impl Step {
    /// Every step.
    pub const ALL: [Step; 5] = [
        Step::Source,
        Step::Grouping,
        Step::Lines,
        Step::Dates,
        Step::Done,
    ];

    /// The step after this one.
    #[must_use]
    pub const fn next(self) -> Step {
        match self {
            Step::Source => Step::Grouping,
            Step::Grouping => Step::Lines,
            Step::Lines => Step::Dates,
            Step::Dates | Step::Done => Step::Done,
        }
    }

    /// The step before this one.
    #[must_use]
    pub const fn prev(self) -> Step {
        match self {
            Step::Source | Step::Grouping => Step::Source,
            Step::Lines => Step::Grouping,
            Step::Dates => Step::Lines,
            Step::Done => Step::Dates,
        }
    }
}

/// The compare set Guided makes: the compared amount and the $ change.
const fn compare_set(base: CompareBase) -> CompareSet {
    CompareSet {
        base,
        amount: true,
        dollar_change: true,
        percent_change: false,
    }
}

/// The measure a Guided section adds up.
#[must_use]
pub fn default_measure(source: SourceKey) -> Measure {
    if fields::of(source).open_items {
        Measure::OpenBalance
    } else {
        Measure::Amount
    }
}

/// The sign a Guided section shows (Q-A08u).
#[must_use]
pub fn default_polarity(source: SourceKey) -> Option<Polarity> {
    (source == SourceKey::Gl).then_some(Polarity::Debit)
}

/// The grouping a source starts with.
#[must_use]
pub fn default_group(source: SourceKey) -> &'static str {
    match source {
        SourceKey::ItemSales | SourceKey::ItemPurchases => "item",
        SourceKey::Sales | SourceKey::Purchases | SourceKey::ArOpen | SourceKey::ApOpen => {
            "contact"
        }
        SourceKey::Gl | SourceKey::Bank | SourceKey::Tax => "account",
    }
}

/// The transaction columns a source starts with (when a line is opened).
#[must_use]
pub fn default_detail_fields(source: SourceKey) -> Vec<String> {
    let offered = fields::of(source).detail_fields;
    let money = if fields::of(source).open_items {
        "open_balance"
    } else {
        "amount"
    };
    ["date", "txn_type", "number", "memo", money]
        .into_iter()
        .filter(|f| offered.contains(f))
        .map(str::to_string)
        .collect()
}

/// The guide's answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuidedState {
    /// The step shown open.
    pub step: Step,
    /// The furthest step reached (a done step's "change" reopens it).
    pub reached: Step,
    /// What it reports on.
    pub source: Option<SourceKey>,
    /// Up to three groupings, outermost first.
    pub group_by: Vec<GroupBy>,
    /// What each line shows.
    pub display: Option<Display>,
    /// Columns across.
    pub periods: Option<Periods>,
    /// Compare with.
    pub compare: Option<CompareBase>,
    /// "% of total".
    pub percent_of_total: bool,
    /// Aging columns (open items).
    pub aging: bool,
    /// Transaction columns.
    pub detail_fields: Vec<String>,
    /// A date preset (`None`: chosen each time it runs).
    pub preset: Option<String>,
    /// Basis (`None`: the company default).
    pub basis: Option<Basis>,
    /// The Report name (owner data).
    pub name: String,
}

impl Default for GuidedState {
    fn default() -> GuidedState {
        GuidedState::new()
    }
}

impl GuidedState {
    /// A blank guide on its first step.
    #[must_use]
    pub fn new() -> GuidedState {
        GuidedState {
            step: Step::Source,
            reached: Step::Source,
            source: None,
            group_by: Vec::new(),
            display: None,
            periods: None,
            compare: None,
            percent_of_total: false,
            aging: false,
            detail_fields: Vec::new(),
            preset: blank_definition().dates.preset,
            basis: None,
            name: String::new(),
        }
    }

    /// True when the source reports open items as of a date.
    #[must_use]
    pub fn open_items(&self) -> bool {
        self.source.is_some_and(|s| fields::of(s).open_items)
    }

    /// Picks what to report on: its default grouping, columns and aging.
    pub fn set_source(&mut self, source: SourceKey) {
        if self.source == Some(source) {
            return;
        }
        self.source = Some(source);
        self.group_by = vec![GroupBy {
            key: default_group(source).into(),
            tag_group_id: None,
        }];
        self.detail_fields = default_detail_fields(source);
        let open = fields::of(source).open_items;
        self.aging = open;
        if open {
            self.periods = None;
            self.compare = None;
            self.preset = Some("today".into());
        } else if self.preset.as_deref() == Some("today") {
            self.preset = blank_definition().dates.preset;
        }
    }

    /// True when `step`'s question is answered.
    #[must_use]
    pub fn complete(&self, step: Step) -> bool {
        match step {
            Step::Source => self.source.is_some(),
            Step::Grouping => !self.group_by.is_empty() && self.group_by.len() <= limits::GROUP_BYS,
            Step::Lines => self.display.is_some(),
            Step::Dates | Step::Done => true,
        }
    }

    /// Next (Alt+N): on when the open step is answered. Returns true when
    /// it moved.
    pub fn go_next(&mut self) -> bool {
        if self.step == Step::Done || !self.complete(self.step) {
            return false;
        }
        self.step = self.step.next();
        self.reached = self.reached.max(self.step);
        true
    }

    /// Back (Alt+B). Returns true when it moved.
    pub fn go_back(&mut self) -> bool {
        if self.step == Step::Source {
            return false;
        }
        self.step = self.step.prev();
        true
    }

    /// A done step's "change" link: reopens a step already reached.
    pub fn reopen(&mut self, step: Step) {
        if step <= self.reached {
            self.step = step;
        }
    }

    /// The definition the answers make (`None` until a source is picked).
    /// `caption` is the section's caption in the owner's language.
    #[must_use]
    pub fn to_definition(&self, caption: &str) -> Option<BuilderDefinition> {
        let source = self.source?;
        let open = fields::of(source).open_items;
        let name = self.name.trim();
        Some(BuilderDefinition {
            schema_version: SCHEMA_VERSION,
            kind: BuilderKind::Builder,
            title: (!name.is_empty()).then(|| name.to_string()),
            dates: Dates {
                mode: if open {
                    DateMode::AsOf
                } else {
                    DateMode::Range
                },
                preset: self.preset.clone(),
            },
            basis: self.basis,
            adjusting: None,
            columns: Columns {
                detail_fields: self.detail_fields.clone(),
                periods: if open { None } else { self.periods },
                aging_buckets: (open && self.aging).then_some(true),
                compare: if open {
                    Vec::new()
                } else {
                    self.compare.map(compare_set).into_iter().collect()
                },
                percent_of: self.percent_of_total.then(|| "section".to_string()),
                headers: BTreeMap::new(),
            },
            nodes: vec![Node::Section(Section {
                id: "s1".into(),
                caption: caption.trim().to_string(),
                source,
                measure: default_measure(source),
                polarity: default_polarity(source),
                filters: Vec::new(),
                group_by: self.group_by.clone(),
                display: self.display.unwrap_or(Display::Summary),
                default_expanded: false,
                show_total: true,
                total_caption: None,
                hide_zero: true,
                earnings: None,
            })],
        })
    }

    /// The answers behind `def` when it has the shape Guided makes (one
    /// section with its source's defaults, no filters, renamed headers or
    /// adjusting lens); `None` otherwise (Guided view is off, Q-A08l).
    #[must_use]
    pub fn from_definition(def: &BuilderDefinition) -> Option<GuidedState> {
        let [Node::Section(s)] = def.nodes.as_slice() else {
            return None;
        };
        let open = fields::of(s.source).open_items;
        let c = &def.columns;
        let compare = match c.compare.as_slice() {
            [] => None,
            [one] if *one == compare_set(one.base) => Some(one.base),
            _ => return None,
        };
        let shape_ok = s.id == "s1"
            && s.measure == default_measure(s.source)
            && s.polarity == default_polarity(s.source)
            && s.filters.is_empty()
            && !s.group_by.is_empty()
            && s.group_by.len() <= limits::GROUP_BYS
            && !s.default_expanded
            && s.show_total
            && s.total_caption.is_none()
            && s.hide_zero
            && s.earnings.is_none()
            && def.adjusting.is_none()
            && c.headers.is_empty()
            && matches!(c.percent_of.as_deref(), None | Some("section"))
            && (def.dates.mode == DateMode::AsOf) == open
            && (!open || (c.periods.is_none() && compare.is_none()))
            && (open || c.aging_buckets.is_none());
        if !shape_ok {
            return None;
        }
        Some(GuidedState {
            step: Step::Done,
            reached: Step::Done,
            source: Some(s.source),
            group_by: s.group_by.clone(),
            display: Some(s.display),
            periods: c.periods,
            compare,
            percent_of_total: c.percent_of.is_some(),
            aging: c.aging_buckets == Some(true),
            detail_fields: c.detail_fields.clone(),
            preset: def.dates.preset.clone(),
            basis: def.basis,
            name: def.title.clone().unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report_def::{issues, parse, to_text};

    fn answered(source: SourceKey) -> GuidedState {
        let mut g = GuidedState::new();
        g.set_source(source);
        g.display = Some(Display::Expandable);
        g.name = " Sales by client ".into();
        g
    }

    #[test]
    fn steps_need_their_answer_and_back_always_works() {
        let mut g = GuidedState::new();
        assert!(!g.go_back());
        assert!(!g.go_next(), "no source yet");
        g.set_source(SourceKey::Sales);
        assert!(g.go_next());
        assert_eq!(g.step, Step::Grouping);
        assert!(g.go_next());
        assert!(!g.go_next(), "lines not chosen");
        g.display = Some(Display::Summary);
        assert!(g.go_next() && g.go_next());
        assert_eq!((g.step, g.reached), (Step::Done, Step::Done));
        assert!(!g.go_next());
        g.reopen(Step::Grouping);
        assert_eq!(g.step, Step::Grouping);
        assert!(g.go_back());
        assert_eq!(g.step, Step::Source);
        let mut fresh = GuidedState::new();
        fresh.reopen(Step::Dates);
        assert_eq!(fresh.step, Step::Source, "not reached yet");
    }

    #[test]
    fn every_source_makes_a_valid_definition_that_round_trips() {
        for source in SourceKey::ALL {
            let mut g = answered(source);
            if !g.open_items() {
                g.periods = Some(Periods::Month);
                g.compare = Some(CompareBase::PriorPeriod);
            }
            g.percent_of_total = true;
            let def = g.to_definition("Sales").unwrap();
            assert_eq!(issues(&def), Vec::new(), "{source:?}");
            let text = to_text(&def).unwrap();
            assert_eq!(parse(&text).unwrap(), def, "{source:?}");
            let back = GuidedState::from_definition(&def).expect("guided shape");
            assert_eq!(back.to_definition("Sales").unwrap(), def, "{source:?}");
            assert_eq!(back.name, "Sales by client");
        }
    }

    #[test]
    fn open_items_run_as_of_a_date_with_aging_and_nothing_across() {
        let mut g = answered(SourceKey::ArOpen);
        g.periods = Some(Periods::Month);
        g.compare = Some(CompareBase::PriorYear);
        let def = g.to_definition("Owed").unwrap();
        assert_eq!(def.dates.mode, DateMode::AsOf);
        assert_eq!(def.columns.aging_buckets, Some(true));
        assert!(def.columns.periods.is_none() && def.columns.compare.is_empty());
        let Node::Section(s) = &def.nodes[0] else {
            panic!()
        };
        assert_eq!(
            (s.measure, s.group_by[0].key.as_str()),
            (Measure::OpenBalance, "contact")
        );
        assert!(
            def.columns
                .detail_fields
                .contains(&"open_balance".to_string())
        );
    }

    #[test]
    fn all_accounts_is_debits_positive() {
        let def = answered(SourceKey::Gl).to_definition("All").unwrap();
        let Node::Section(s) = &def.nodes[0] else {
            panic!()
        };
        assert_eq!(s.polarity, Some(Polarity::Debit));
        assert_eq!(issues(&def), Vec::new());
    }

    #[test]
    fn other_shapes_are_not_guided() {
        let def = answered(SourceKey::Sales).to_definition("Sales").unwrap();
        let mut two = def.clone();
        two.nodes.push(two.nodes[0].clone());
        assert!(GuidedState::from_definition(&two).is_none(), "two sections");
        let mut filtered = def.clone();
        if let Node::Section(s) = &mut filtered.nodes[0] {
            s.filters.push(crate::report_def::Filter {
                field: "amount".into(),
                op: crate::report_def::FilterOp::Gte,
                ids: None,
                values: None,
                value: Some("10".into()),
            });
        }
        assert!(
            GuidedState::from_definition(&filtered).is_none(),
            "a filter"
        );
        let mut renamed = def.clone();
        renamed.columns.headers.insert("total".into(), "Oct".into());
        assert!(GuidedState::from_definition(&renamed).is_none(), "a header");
        assert!(GuidedState::from_definition(&blank_definition()).is_none());
        assert!(GuidedState::new().to_definition("x").is_none());
    }
}
