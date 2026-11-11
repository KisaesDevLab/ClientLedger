//! Ask a question (Q-73, Q-P28g, U-206): the catalog the app sends, the
//! options a checked mapping becomes, and the "Ran as" line.
//!
//! The model only ever picks among tokens this module sends (report keys,
//! date presets, column and extra names) and names the engine resolves to
//! ids. The "Ran as" line is built from the resulting [`ReportOptions`]
//! alone: the report's title, the dates the options stand for, the basis,
//! the columns and extras, and which filters are set. No model text and no
//! number other than the option dates (and a report title such as "1099
//! Summary") ever reaches it, and nothing here sets an amount filter
//! (non-negotiable 4).

use cl_core::CalendarDate;
use cl_core::fiscal::FiscalYearSpec;
use cl_core::ids::{AccountId, ContactId, TagId};
use cl_core::report::{Basis, Extra};

use super::catalog::CATALOG;
use super::options::{ColumnBy, DateChoice, ReportOptions};
use super::presets::DatePreset;
use super::spec::{BasisMode, DateMode, FilterKind, ReportSpec};

/// An option a catalog report takes (the engine's `params`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AskParam {
    /// A from/to period.
    Range,
    /// A balance date.
    AsOf,
    /// Cash or accrual.
    Basis,
    /// Accounts.
    Accounts,
    /// Clients or vendors.
    Contacts,
    /// Tags.
    Tags,
}

/// One catalog entry as sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskEntry {
    /// The key.
    pub key: &'static str,
    /// The title's i18n key.
    pub title: &'static str,
    /// Options it takes.
    pub params: Vec<AskParam>,
    /// Column tokens.
    pub columns: Vec<String>,
    /// Extra tokens.
    pub extras: Vec<String>,
}

/// The wire tokens (each enum's serde name), one table per kind.
const PRESETS: [(DatePreset, &str); 9] = [
    (DatePreset::Today, "today"),
    (DatePreset::ThisMonth, "this_month"),
    (DatePreset::LastMonth, "last_month"),
    (DatePreset::ThisQuarter, "this_quarter"),
    (DatePreset::LastQuarter, "last_quarter"),
    (DatePreset::ThisYear, "this_year"),
    (DatePreset::LastYear, "last_year"),
    (DatePreset::YearToDate, "year_to_date"),
    (DatePreset::EndOfLastMonth, "end_of_last_month"),
];
const COLUMNS: [(ColumnBy, &str); 8] = [
    (ColumnBy::Total, "total"),
    (ColumnBy::Month, "month"),
    (ColumnBy::FiscalPeriod, "fiscal_period"),
    (ColumnBy::Quarter, "quarter"),
    (ColumnBy::Year, "year"),
    (ColumnBy::Client, "client"),
    (ColumnBy::TagGroup, "tag_group"),
    (ColumnBy::Project, "project"),
];
const EXTRAS: [(Extra, &str); 4] = [
    (Extra::PctOfIncome, "pct_of_income"),
    (Extra::Ytd, "ytd"),
    (Extra::PriorPeriod, "prior_period"),
    (Extra::PriorYear, "prior_year"),
];

fn token<T: PartialEq + Copy>(table: &[(T, &'static str)], v: T) -> String {
    table
        .iter()
        .find(|(x, _)| *x == v)
        .map_or_else(String::new, |(_, s)| (*s).to_string())
}

fn from_token<T: Copy>(table: &[(T, &'static str)], s: &str) -> Option<T> {
    table.iter().find(|(_, t)| *t == s).map(|(x, _)| *x)
}
/// The date preset tokens.
#[must_use]
pub fn preset_tokens() -> Vec<String> {
    DatePreset::ALL
        .iter()
        .map(|p| token(&PRESETS, *p))
        .collect()
}

/// True when `spec` can be asked for: it runs without a pick first (a
/// vendor, bank account or budget the model can't choose, Q-P28ab).
#[must_use]
pub fn askable(spec: &ReportSpec) -> bool {
    !spec.filters.iter().any(|f| {
        matches!(
            f,
            FilterKind::Vendor | FilterKind::BankAccount | FilterKind::Budget
        )
    })
}

/// The catalog entry sent for `spec`.
#[must_use]
pub fn entry(spec: &ReportSpec) -> AskEntry {
    let mut params = Vec::new();
    match spec.dates {
        DateMode::Range | DateMode::Year => params.push(AskParam::Range),
        DateMode::AsOf => params.push(AskParam::AsOf),
        DateMode::None => {}
    }
    if spec.basis == BasisMode::Toggle {
        params.push(AskParam::Basis);
    }
    for f in spec.filters {
        match f {
            FilterKind::Accounts => params.push(AskParam::Accounts),
            FilterKind::Contacts => params.push(AskParam::Contacts),
            FilterKind::Tags => params.push(AskParam::Tags),
            _ => {}
        }
    }
    AskEntry {
        key: spec.key,
        title: spec.title,
        params,
        columns: if spec.columns.len() > 1 {
            spec.columns
                .iter()
                .filter(|c| !matches!(c, ColumnBy::TagGroup | ColumnBy::Project))
                .map(|c| token(&COLUMNS, *c))
                .collect()
        } else {
            Vec::new()
        },
        extras: spec.extras.iter().map(|e| token(&EXTRAS, *e)).collect(),
    }
}

/// The catalog sent: the reports `runs` says this company can run, minus
/// those that need a pick first.
pub fn catalog(runs: impl Fn(&ReportSpec) -> bool) -> Vec<AskEntry> {
    CATALOG
        .iter()
        .filter(|s| askable(s) && runs(s))
        .map(entry)
        .collect()
}

/// A checked mapping's dates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AskDates {
    /// A preset token.
    Preset(String),
    /// A period.
    Range(CalendarDate, CalendarDate),
    /// A balance date.
    AsOf(CalendarDate),
    /// The report's default.
    Default,
}

/// A checked mapping, in the model's terms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AskChoice {
    /// The report key.
    pub report_key: String,
    /// Dates.
    pub dates: AskDates,
    /// Basis.
    pub basis: Option<Basis>,
    /// A column token.
    pub columns: Option<String>,
    /// Extra tokens.
    pub extras: Vec<String>,
    /// Accounts.
    pub account_ids: Vec<AccountId>,
    /// Clients or vendors.
    pub contact_ids: Vec<ContactId>,
    /// Tags.
    pub tag_ids: Vec<TagId>,
}

/// The report and options a mapping stands for (`None` for a report or
/// token outside the catalog). Never an amount filter.
#[must_use]
pub fn options_for(c: &AskChoice) -> Option<(&'static ReportSpec, ReportOptions)> {
    let spec = CATALOG
        .iter()
        .find(|s| s.key == c.report_key && askable(s))?;
    let mut o = ReportOptions::for_spec(spec);
    o.dates = match &c.dates {
        AskDates::Preset(p) => DateChoice::Preset(from_token(&PRESETS, p)?),
        AskDates::Range(from, to) => DateChoice::Fixed {
            from: *from,
            to: *to,
        },
        AskDates::AsOf(d) => DateChoice::AsOf(*d),
        AskDates::Default => o.dates,
    };
    if spec.basis == BasisMode::Toggle && c.basis.is_some() {
        o.basis = c.basis;
    }
    if let Some(col) = &c.columns {
        let col = from_token(&COLUMNS, col)?;
        if !spec.columns.contains(&col) {
            return None;
        }
        o.columns = col;
    }
    for e in &c.extras {
        let e = from_token(&EXTRAS, e)?;
        if !spec.extras.contains(&e) {
            return None;
        }
        o.extras.insert(e);
    }
    o.filters.account_ids.clone_from(&c.account_ids);
    o.filters.contact_ids.clone_from(&c.contact_ids);
    o.filters.tag_ids.clone_from(&c.tag_ids);
    o.filters.amount = None;
    Some((spec, o))
}

/// `MM/DD/YYYY`.
#[must_use]
pub fn us_date(d: CalendarDate) -> String {
    format!("{:02}/{:02}/{}", d.month(), d.day(), d.year())
}

/// The "Ran as" line: built only from `spec` and `o` (titles, labels and
/// the dates the options stand for), through `tr` for every label.
#[must_use]
pub fn ran_as(
    spec: &ReportSpec,
    o: &ReportOptions,
    fiscal: &FiscalYearSpec,
    today: CalendarDate,
    tr: &dyn Fn(&str) -> String,
) -> String {
    let mut parts = vec![tr(spec.title)];
    let span = match o.dates {
        DateChoice::Preset(p) => {
            parts.push(tr(p.label()));
            p.range(fiscal, today).ok().map(|r| (r.from, r.to))
        }
        DateChoice::Fixed { from, to } => Some((from, to)),
        DateChoice::AsOf(d) => Some((d, d)),
    };
    if spec.dates != DateMode::None
        && let Some((from, to)) = span
    {
        parts.push(if spec.dates == DateMode::AsOf || from == to {
            us_date(to)
        } else {
            format!("{}\u{2013}{}", us_date(from), us_date(to))
        });
    }
    if spec.basis == BasisMode::Toggle {
        parts.push(tr(match o.basis.unwrap_or(Basis::Accrual) {
            Basis::Accrual => "reports.basis.accrual_basis",
            Basis::Cash => "reports.basis.cash_basis",
        }));
    }
    if spec.columns.len() > 1 {
        parts.push(tr(o.columns.label()));
    }
    for e in &o.extras {
        parts.push(tr(match e {
            Extra::PctOfIncome => "reports.extra.pct_of_income",
            Extra::Ytd => "reports.extra.ytd",
            Extra::PriorPeriod => "reports.extra.prior_period",
            Extra::PriorYear => "reports.extra.prior_year",
        }));
    }
    for (set, key) in [
        (
            !o.filters.account_ids.is_empty(),
            "reports.ask.filtered_accounts",
        ),
        (
            !o.filters.contact_ids.is_empty(),
            "reports.ask.filtered_contacts",
        ),
        (!o.filters.tag_ids.is_empty(), "reports.ask.filtered_tags"),
    ] {
        if set {
            parts.push(tr(key));
        }
    }
    parts.join(" \u{b7} ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn tr(k: &str) -> String {
        // Labels without digits, as the English catalog's are (titles
        // aside).
        k.rsplit('.').next().unwrap_or(k).replace('_', " ")
    }

    fn d(y: i32, m: u32, dd: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, dd).expect("date")
    }

    fn choice(key: &str) -> AskChoice {
        AskChoice {
            report_key: key.into(),
            dates: AskDates::Preset("last_month".into()),
            basis: Some(Basis::Cash),
            columns: Some("month".into()),
            extras: vec!["pct_of_income".into()],
            account_ids: vec![
                AccountId::parse("11111111-1111-4111-8111-111111111111").expect("id"),
            ],
            contact_ids: vec![],
            tag_ids: vec![],
        }
    }

    #[test]
    fn the_catalog_leaves_out_reports_that_need_a_pick() {
        let c = catalog(|_| true);
        assert!(c.iter().any(|e| e.key == "profit_loss"));
        assert!(!c.iter().any(|e| e.key == "budget_vs_actual"));
        assert!(!c.iter().any(|e| e.key == "check_register"));
        let pl = c.iter().find(|e| e.key == "profit_loss").expect("p&l");
        assert!(pl.params.contains(&AskParam::Basis));
        assert!(preset_tokens().contains(&"last_month".to_string()));
    }

    #[test]
    fn a_mapping_becomes_options_and_never_an_amount_filter() {
        let (spec, o) = options_for(&choice("profit_loss")).expect("options");
        assert_eq!(spec.key, "profit_loss");
        assert_eq!(o.dates, DateChoice::Preset(DatePreset::LastMonth));
        assert_eq!(o.basis, Some(Basis::Cash));
        assert_eq!(o.columns, ColumnBy::Month);
        assert!(o.extras.contains(&Extra::PctOfIncome));
        assert!(o.filters.amount.is_none());
        let mut bad = choice("profit_loss");
        bad.columns = Some("by_magic".into());
        assert!(options_for(&bad).is_none());
        assert!(
            options_for(&choice("budget_vs_actual")).is_none(),
            "needs a budget"
        );
        assert!(options_for(&choice("not_a_report")).is_none());
    }

    /// Every run of digits in the line.
    fn digit_runs(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        for ch in s.chars() {
            if ch.is_ascii_digit() {
                cur.push(ch);
            } else if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    #[test]
    fn tokens_are_the_serde_names() {
        for (p, s) in PRESETS {
            assert_eq!(serde_json::to_value(p).expect("json"), *s);
        }
        for (c, s) in COLUMNS {
            assert_eq!(serde_json::to_value(c).expect("json"), *s);
        }
        for (e, s) in EXTRAS {
            assert_eq!(serde_json::to_value(e).expect("json"), *s);
        }
    }
    proptest! {
        /// The "Ran as" line shows no number that isn't part of the option
        /// dates or the report's title: no figure can get into it.
        #[test]
        fn ran_as_numbers_come_only_from_the_option_dates(
            key in 0usize..64,
            y in 2000i32..2090,
            m in 1u32..=12,
            dd in 1u32..=28,
            span in 0i64..400,
            preset in 0usize..9,
            fixed in any::<bool>(),
        ) {
            let entries = catalog(|_| true);
            let e = &entries[key % entries.len()];
            let from = d(y, m, dd);
            let to = from.add_days(span).expect("to");
            let mut c = choice(e.key);
            c.columns = None;
            c.extras.clear();
            c.dates = if fixed {
                AskDates::Range(from, to)
            } else {
                AskDates::Preset(preset_tokens()[preset % 9].clone())
            };
            if let Some((spec, o)) = options_for(&c) {
                let today = d(2026, 11, 10);
                let line = ran_as(spec, &o, &FiscalYearSpec::CALENDAR_YEAR, today, &tr);
                let mut allowed = tr(spec.title);
                if let DateChoice::Preset(p) = o.dates
                    && let Ok(r) = p.range(&FiscalYearSpec::CALENDAR_YEAR, today)
                {
                    allowed = allowed + " " + &us_date(r.from) + " " + &us_date(r.to);
                }
                if let DateChoice::Fixed { from, to } = o.dates {
                    allowed = allowed + " " + &us_date(from) + " " + &us_date(to);
                }
                if let DateChoice::AsOf(a) = o.dates {
                    allowed = allowed + " " + &us_date(a);
                }
                let ok = digit_runs(&allowed);
                for run in digit_runs(&line) {
                    prop_assert!(ok.contains(&run), "{run} in {line}");
                }
            }
        }
    }
}
