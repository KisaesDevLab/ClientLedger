//! The report window (P28 plan §0, §4; mockup `docs/design/mockups/
//! Reports.html`): one generic window driven by the catalog's report specs.
//! The catalog with search on the left, the toolbar and filter bar on top,
//! the report's header (Q-107) and a virtualized table of the engine's
//! figures below. Collapse, sort and every export read the neutral
//! [`cl_core::report::ReportTable`]; nothing here adds up money.
//!
//! WP1 runs the reports the engine reports exactly today (A/R aging, sales
//! tax liability, inventory); the rest of the catalog shows greyed with
//! "Needs an engine update" until their engine PRs merge (Q-220).

mod catalog;
mod drill;
mod export;
mod filters;
mod memorized;
mod window;

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

/// True when this build runs `spec` on the open company: the engine has
/// every capability it needs, and this build reads its route family (the
/// demo plays the routes WP1 reads).
#[must_use]
pub(crate) fn available(link: &CompanyLink, spec: &ReportSpec) -> bool {
    if !cl_api::resources::reports::run::supported(spec.source) {
        return false;
    }
    link.is_demo() || spec.available(|c| link.capabilities().has(c))
}

/// Text for a caption: `@key` is an i18n key, anything else owner data.
#[must_use]
pub(crate) fn label(s: &str) -> String {
    match s.strip_prefix('@') {
        Some(key) => crate::i18n::t(key),
        None => s.to_string(),
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
        _ => Err(ApiError::Local(cl_api::LocalValidation::Unsupported)),
    }
}
