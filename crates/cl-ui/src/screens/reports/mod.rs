//! The report window (P28 plan §0, §4; mockup `docs/design/mockups/
//! Reports.html`): one generic window driven by the catalog's report specs.
//! The catalog with search on the left, the toolbar and filter bar on top,
//! the report's header (Q-107) and a virtualized table of the engine's
//! figures below. Collapse, sort and every export read the neutral
//! [`cl_core::report::ReportTable`]; nothing here adds up money.
//!
//! WP1 runs the reports that were exact before P28 (A/R aging, sales tax
//! liability, inventory) and WP2 the `reports-exact-v1` statements and
//! ledgers; the rest of the catalog, and options an engine update brings
//! (period columns, tag split, the ledger's drill filters), show greyed
//! with "Needs an engine update" until their engine PRs merge (Q-220).

mod ask;
mod budgets;
// A08: the Report Builder editor, and the one RunRef its preview, its
// Run report and the window's builder mode share (plan §3.4).
pub mod builder;
pub mod builder_ref;
mod builder_run;
mod catalog;
mod drill;
mod email;
mod export;
mod filter_lists;
mod filters;
mod memorized;
// A09: the Memorized Reports list.
pub mod memorized_list;
mod window;

#[cfg(test)]
mod builder_tests;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use cl_api::ApiError;
use cl_core::CalendarDate;
use cl_core::report::{ReportRequest, ReportTable, Source};
use cl_model::CommandId;
use cl_model::reports::{ReportOptions, ReportSpec};

use crate::company::CompanyLink;
use crate::company::demo::DemoState;
use crate::windows::Screen;

pub use builder_run::BuilderDetailOpen;
pub(crate) use drill::profit_loss_for;
pub use window::ReportWindow;

/// The report center (Reports menu, rail).
pub const KIND: &str = "reports.center";

/// What a new report window opens on (a drill-down, Home's chart, a
/// memorized report).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportOpen {
    /// The catalog report.
    pub key: String,
    /// Its options (`None`: the report's defaults).
    pub options: Option<ReportOptions>,
}

/// The catalog report a menu command opens.
fn key_of(kind: &str) -> Option<&'static str> {
    Some(match kind {
        KIND | "reports.profit_loss" => "profit_loss",
        "reports.balance_sheet" => "balance_sheet",
        "reports.trial_balance" => "trial_balance",
        "reports.ar_aging" => "ar_aging_summary",
        _ => return None,
    })
}

/// The window for a Reports command.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(cl_core::ids::TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    if kind.as_str() == memorized_list::KIND {
        return memorized_list::make(kind, link, open);
    }
    if kind.as_str() == builder::KIND {
        return Some(Box::new(builder::BuilderScreen::new(
            Arc::clone(link),
            open,
            today,
        )));
    }
    let key = key_of(kind.as_str())?;
    let spec = cl_model::reports::find(key)?;
    Some(Box::new(ReportWindow::new(
        Arc::clone(link),
        kind,
        open,
        spec,
        today,
    )))
}

/// The route families the demo plays (P28 WP5: the statements and the
/// ledger from the sample's postings, plus canned lists).
const fn demo_plays(source: Source) -> bool {
    matches!(
        source,
        Source::ArAgingSummary
            | Source::SalesTaxLiability
            | Source::InventoryValuation
            | Source::InventoryValuationDetail
            | Source::InventoryStockStatus
            | Source::ProfitLoss
            | Source::BalanceSheet
            | Source::TrialBalance
            | Source::GeneralLedger
            | Source::BudgetVsActual
    )
}

/// True when this build runs `spec` on the open company: this build reads
/// its route family, and the engine has every capability it needs (the
/// demo: plays that family).
#[must_use]
pub(crate) fn available(link: &CompanyLink, spec: &ReportSpec) -> bool {
    if !cl_api::resources::reports::run::supported(spec.source) {
        return false;
    }
    if link.is_demo() {
        return demo_plays(spec.source);
    }
    spec.available(|c| link.capabilities().has(c))
}

/// True when the engine can answer `req` as asked (Q-220): an option an
/// engine update brings (period columns, tag split, the ledger's drill
/// filters) waits for it rather than being sent and ignored.
#[must_use]
pub(crate) fn request_runs(link: &CompanyLink, req: &ReportRequest) -> bool {
    link.is_demo()
        || cl_api::resources::reports::run::missing_capability(req, |c| link.capabilities().has(c))
            .is_none()
}

/// Text for a caption: `@key` is an i18n key, `@key name` one that names
/// an account (`Total {name}`), anything else owner data. Owner text that
/// happens to start with `@` ("@Home Depot") is not a key the app knows,
/// so it shows as typed (S3). `@@` starts text an adapter marked as never a
/// key (`cl_core::report::literal_caption`, S-L2): one `@` shows.
#[must_use]
pub(crate) fn label(s: &str) -> String {
    let Some(rest) = s.strip_prefix('@') else {
        return s.to_string();
    };
    if rest.starts_with('@') {
        return rest.to_string();
    }
    let (key, name) = match rest.split_once(' ') {
        Some((key, name)) => (key, Some(name)),
        None => (rest, None),
    };
    if !crate::i18n::has(key) {
        return s.to_string();
    }
    match name {
        Some(name) => crate::i18n::t(key).replace("{name}", name),
        None => crate::i18n::t(key),
    }
}

/// The demo's answer to `req` (the same adapters as the live engine).
pub(crate) fn demo_run(d: &mut DemoState, req: &ReportRequest) -> Result<ReportTable, ApiError> {
    use cl_api::resources::reports::adapters;
    let span = req
        .dates
        .span()
        .ok_or(ApiError::Local(cl_api::LocalValidation::BadRange))?;
    match req.source {
        Source::ArAgingSummary => {
            let r = crate::company::demo::statements::ar_aging(d, span.to)?;
            Ok(adapters::ar_aging::summary(&r, span.to))
        }
        Source::SalesTaxLiability => {
            let r = crate::company::demo::sales_tax_owed::liability(
                d,
                cl_api::LiabilityQuery {
                    start_date: span.from,
                    end_date: span.to,
                    basis: None,
                    agency_id: None,
                },
            )?;
            Ok(adapters::sales_tax::liability(&r, span))
        }
        Source::InventoryValuation => {
            let v = crate::company::demo::stock::valuation(d, span.to)?;
            Ok(adapters::inventory::valuation(&v))
        }
        Source::InventoryValuationDetail => {
            let item = req.filters.item_ids.first().copied();
            let rows = crate::company::demo::stock::valuation_detail(d, span.from, span.to, item)?;
            let names: std::collections::HashMap<_, _> =
                crate::company::demo::stock::valuation(d, span.to)?
                    .rows
                    .into_iter()
                    .filter_map(|r| r.name.map(|n| (r.item_id, n)))
                    .collect();
            Ok(adapters::inventory::valuation_detail(&rows, &names, span))
        }
        Source::InventoryStockStatus => {
            let rows = crate::company::demo::stock::stock_status(d, span.to)?;
            Ok(adapters::inventory::stock_status(&rows, span.to))
        }
        Source::ProfitLoss
        | Source::BalanceSheet
        | Source::TrialBalance
        | Source::GeneralLedger => crate::company::demo::reports::statement(d, req),
        Source::BudgetVsActual => crate::company::demo::report_store::budget_vs_actual(d, req),
        _ => Err(ApiError::Local(cl_api::LocalValidation::Unsupported)),
    }
}

/// The ask request for `question` over the whole catalog (demo tests).
#[cfg(test)]
pub(crate) fn ask_request_for_test(
    question: &str,
    today: CalendarDate,
) -> cl_api::resources::reports::ask::AskRequest {
    ask::request(question, |_| true, today)
}
