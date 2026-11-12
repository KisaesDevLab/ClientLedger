//! The one `RunRef` every Report Builder run is sent with (ADD-03 A08,
//! plan §3.4, AC8): the editor's preview, its Run report and the report
//! window's builder mode all build it here from the same inputs, so the
//! preview always matches a full run.
//!
//! Also what an editor's Run report hands the report window
//! ([`BuilderRunOpen`], `OpenPayload::BuilderDefinition`).
//!
//! The seam commit declares the signatures; WP-B2 lifts the body out of
//! `builder_run.rs` and adds the compare ranges (`report_def::ranges`).

use cl_api::resources::report_builder::{RunDates, RunRef};
use cl_core::CalendarDate;
use cl_core::fiscal::FiscalYearSpec;
use cl_model::report_def::editor::{Origin, Saved};
use cl_model::report_def::ranges::{self, RunSpan};
use cl_model::report_def::{BuilderDefinition, DateMode};
use cl_model::reports::{
    BUILDER_AS_OF, BUILDER_AS_OF_TOTAL, BUILDER_RANGE, BUILDER_RANGE_TOTAL, ReportOptions,
    ReportSpec,
};
use serde_json::Value;

use super::builder::EditorId;

/// What a run is built from. The editor and the report window fill it the
/// same way: the stand-in spec for the definition's shape, the window
/// options (dates, basis, period columns), the company's fiscal year and
/// today.
pub(crate) struct RunInput<'a> {
    /// The definition (its columns decide the period and compare ranges).
    pub(crate) def: &'a BuilderDefinition,
    /// The same definition as JSON, sent as `RunRef::definition`.
    pub(crate) json: &'a Value,
    /// The stand-in spec (`cl_model::reports::BUILDER_RANGE`, ...).
    pub(crate) spec: &'static ReportSpec,
    /// Dates, basis and period columns.
    pub(crate) options: &'a ReportOptions,
    /// The company's fiscal year.
    pub(crate) fiscal: &'a FiscalYearSpec,
    /// Today (presets resolve against it).
    pub(crate) today: CalendarDate,
}

/// The stand-in spec a definition runs under: as of a date or a range, and
/// whether its columns are fixed (aging buckets refuse period and compare
/// ranges, U-119e; a running balance takes no period columns).
pub(crate) fn spec_for(def: &BuilderDefinition) -> &'static ReportSpec {
    let as_of = def.dates.mode == DateMode::AsOf;
    let fixed = def.columns.aging_buckets == Some(true)
        || def
            .columns
            .detail_fields
            .iter()
            .any(|f| f == "running_balance");
    match (as_of, fixed) {
        (true, true) => &BUILDER_AS_OF_TOTAL,
        (true, false) => &BUILDER_AS_OF,
        (false, true) => &BUILDER_RANGE_TOTAL,
        (false, false) => &BUILDER_RANGE,
    }
}

/// The run's reference: the definition inline, the window's dates and
/// basis, the period and compare ranges and the fiscal year starts they
/// touch ([`cl_model::report_def::ranges`]); adjusting comes from the
/// definition (sent, so an earnings drill keeps it, A07 #13).
///
/// # Errors
/// The i18n key of the problem to show (`reports.problem.fiscal` when the
/// dates can't be placed in the fiscal calendar).
pub(crate) fn run_ref(input: &RunInput<'_>) -> Result<RunRef, &'static str> {
    let r = ranges::run_ranges(
        input.def,
        input.spec,
        input.options,
        input.fiscal,
        input.today,
    )
    .map_err(|_| "reports.problem.fiscal")?;
    let dates = match r.dates {
        RunSpan::Range(range) => RunDates::range(range),
        RunSpan::AsOf(day) => RunDates::as_of(day),
    };
    Ok(RunRef {
        definition: Some(input.json.clone()),
        definition_id: None,
        version: None,
        dates,
        period_ranges: r.period_ranges,
        compare_ranges: r.compare_ranges,
        fiscal_year_starts: r.fiscal_year_starts,
        basis: input.options.basis,
        adjusting: input.def.adjusting,
    })
}

/// An editor's Run report: its definition in a report window, one result
/// tab per editor (Q-A08x). Its `Debug` shows no captions or names (owner
/// data).
#[derive(Clone, PartialEq, Eq)]
#[allow(dead_code, reason = "A08 seam: WP-A2 builds it, WP-B3 opens it")]
pub struct BuilderRunOpen {
    /// The editor that ran it (its result tab takes the next run in place,
    /// and Customize from the tab returns to it).
    pub(crate) editor: EditorId,
    /// The report's name, the tab's title (owner data).
    pub(crate) title: String,
    /// The definition as edited (inline; saved or not).
    pub(crate) def: BuilderDefinition,
    /// The stand-in spec's options: dates, basis, period columns.
    pub(crate) options: ReportOptions,
    /// The memorized row behind the editor, if any (Memorize in the tab).
    pub(crate) saved: Option<Saved>,
    /// Where the editor's report started.
    pub(crate) origin: Origin,
}

impl std::fmt::Debug for BuilderRunOpen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuilderRunOpen")
            .field("editor", &self.editor)
            .field("saved", &self.saved)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_model::reports::{ColumnBy, DateChoice};

    fn day(y: i32, m: u32, d: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, d).expect("date")
    }

    fn def() -> (BuilderDefinition, Value) {
        let json = serde_json::json!({
            "schemaVersion": 1, "kind": "builder", "dates": {"mode": "range"},
            "adjusting": "exclude",
            "columns": {"detailFields": [], "headers": {},
                        "compare": [{"base": "prior_period", "amount": true,
                                     "dollarChange": true, "percentChange": false}]},
            "nodes": [{"type": "section", "id": "s1", "caption": "Income", "source": "gl",
                       "measure": "amount", "polarity": "credit", "filters": [], "groupBy": [],
                       "display": "summary", "defaultExpanded": false, "showTotal": true,
                       "hideZero": false}]
        });
        let def = cl_model::report_def::parse(&json.to_string()).expect("definition");
        (def, json)
    }

    #[test]
    fn the_engines_cap_on_fiscal_year_starts_is_the_models() {
        assert_eq!(
            cl_model::report_def::ranges::MAX_FISCAL_YEAR_STARTS,
            cl_api::resources::report_builder::MAX_FISCAL_YEAR_STARTS
        );
    }

    #[test]
    fn run_ref_sends_the_compare_ranges_and_the_definitions_adjusting() {
        let (def, json) = def();
        let spec = spec_for(&def);
        let mut options = ReportOptions::for_spec(spec);
        options.dates = DateChoice::Fixed {
            from: day(2026, 10, 1),
            to: day(2026, 10, 31),
        };
        options.columns = ColumnBy::Total;
        let fiscal = FiscalYearSpec::CALENDAR_YEAR;
        let input = RunInput {
            def: &def,
            json: &json,
            spec,
            options: &options,
            fiscal: &fiscal,
            today: day(2026, 11, 12),
        };
        let r = run_ref(&input).expect("reference");
        assert_eq!(r.definition.as_ref(), Some(&json));
        assert_eq!(r.dates.from, Some(day(2026, 10, 1)));
        assert_eq!(r.dates.to, Some(day(2026, 10, 31)));
        assert_eq!(r.period_ranges.len(), 0);
        assert_eq!(r.compare_ranges.len(), 1);
        assert_eq!(r.compare_ranges[0][0].from, day(2026, 9, 1));
        assert_eq!(r.compare_ranges[0][0].to, day(2026, 9, 30));
        assert_eq!(r.adjusting, def.adjusting);
        // The same inputs give the same reference (AC8: preview = Run report).
        assert_eq!(run_ref(&input), Ok(r));
    }
}
