//! One entry point for the report window (P28 plan §0 step 3):
//! [`Reports::run`] sends a [`ReportRequest`] to its route family and turns
//! the answer into a [`ReportTable`].

use std::collections::HashMap;

use cl_core::CalendarDate;
use cl_core::report::{DateRange, ReportDates, ReportRequest, ReportTable, Source};

use super::adapters;
use crate::errors::{ApiError, LocalValidation};
use crate::resources::reports::Reports;
use crate::resources::sales_tax_reports::LiabilityQuery;
use crate::resources::transactions::TxnType;

/// The engine's name of a transaction type (`journal_entry`, ...).
#[must_use]
pub fn txn_name(t: TxnType) -> String {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The as-of date of a balance request (a range's end).
fn as_of(d: &ReportDates) -> Result<CalendarDate, ApiError> {
    d.span()
        .map(|r| r.to)
        .ok_or(ApiError::Local(LocalValidation::BadRange))
}

/// The period of a range request.
fn range(d: &ReportDates) -> Result<DateRange, ApiError> {
    match d {
        ReportDates::Range(r) => Ok(*r),
        ReportDates::AsOf(_) | ReportDates::Ranges(_) | ReportDates::None => {
            Err(ApiError::Local(LocalValidation::BadRange))
        }
    }
}

/// True when this build reads `source` (WP1: the routes exact today).
#[must_use]
pub const fn supported(source: Source) -> bool {
    matches!(
        source,
        Source::ArAgingSummary
            | Source::SalesTaxLiability
            | Source::InventoryValuation
            | Source::InventoryValuationDetail
            | Source::InventoryStockStatus
    )
}

impl Reports<'_> {
    /// Runs `req` and returns its table.
    ///
    /// # Errors
    /// `Local(Unsupported)` for a route family this build doesn't read yet
    /// or an engine without its capability; `Local(BadRange)` for dates the
    /// route doesn't take; [`ApiError::ReportShape`] when the answer lacks
    /// its exact fields; any [`ApiError`].
    pub async fn run(&self, req: &ReportRequest) -> Result<ReportTable, ApiError> {
        match req.source {
            Source::ArAgingSummary => {
                let d = as_of(&req.dates)?;
                let r = self.ar_aging_summary(d).await?;
                Ok(adapters::ar_aging::summary(&r, d))
            }
            Source::SalesTaxLiability => {
                let span = range(&req.dates)?;
                let r = self
                    .sales_tax_liability(LiabilityQuery {
                        start_date: span.from,
                        end_date: span.to,
                        basis: None,
                        agency_id: None,
                    })
                    .await?;
                Ok(adapters::sales_tax::liability(&r, span))
            }
            Source::InventoryValuation => {
                let v = self.0.inventory().valuation(as_of(&req.dates)?).await?;
                Ok(adapters::inventory::valuation(&v))
            }
            Source::InventoryValuationDetail => {
                let span = range(&req.dates)?;
                let inv = self.0.inventory();
                let item = req.filters.item_ids.first().copied();
                let rows = inv.valuation_detail(span.from, span.to, item).await?;
                // The detail carries no names: the parts' names as of the end.
                let names: HashMap<_, _> = inv
                    .valuation(span.to)
                    .await?
                    .rows
                    .into_iter()
                    .filter_map(|r| r.name.map(|n| (r.item_id, n)))
                    .collect();
                Ok(adapters::inventory::valuation_detail(&rows, &names, span))
            }
            Source::InventoryStockStatus => {
                let d = as_of(&req.dates)?;
                let rows = self.0.inventory().stock_status(d).await?;
                Ok(adapters::inventory::stock_status(&rows, d))
            }
            _ => Err(ApiError::Local(LocalValidation::Unsupported)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_type_names_are_the_engine_s() {
        assert_eq!(txn_name(TxnType::JournalEntry), "journal_entry");
        assert_eq!(txn_name(TxnType::Bill), "bill");
    }

    #[test]
    fn wp1_reads_only_the_routes_exact_today() {
        assert!(supported(Source::ArAgingSummary));
        assert!(supported(Source::InventoryStockStatus));
        assert!(
            !supported(Source::ProfitLoss),
            "needs reports-exact-v1 (WP2)"
        );
    }
}
