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

use cl_api::resources::report_builder::RunRef;
use cl_core::CalendarDate;
use cl_core::fiscal::FiscalYearSpec;
use cl_model::report_def::editor::{Origin, Saved};
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
#[allow(
    dead_code,
    reason = "A08 seam: WP-B2 builds run_ref, WP-A2 and WP-B3 call it"
)]
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
#[allow(dead_code, reason = "A08 seam: WP-A2 and WP-B3 call it")]
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

/// The run's reference, with its period and compare ranges and the fiscal
/// year starts it touches; adjusting comes from the definition.
///
/// # Errors
/// The i18n key of the problem to show (`reports.problem.fiscal` when the
/// dates can't be placed in the fiscal calendar).
#[allow(
    dead_code,
    reason = "A08 seam: WP-B2 builds it, WP-A2 and WP-B3 call it"
)]
pub(crate) fn run_ref(input: &RunInput<'_>) -> Result<RunRef, &'static str> {
    let _ = input;
    Err("reports.problem.fiscal")
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
