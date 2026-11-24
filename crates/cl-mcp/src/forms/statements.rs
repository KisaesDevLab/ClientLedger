//! Customer Statements (P23): one chunk of the checked run plan as the
//! engine's statement run.

use cl_api::{NewStatementRun, StatementFormat};
use cl_model::forms::statements::{RunChunk, RunPlan, StatementFormatChoice};

/// The engine's layout of the form's.
#[must_use]
pub fn wire_format(f: StatementFormatChoice) -> StatementFormat {
    match f {
        StatementFormatChoice::OpenItem => StatementFormat::OpenItem,
        StatementFormatChoice::BalanceForward => StatementFormat::BalanceForward,
        StatementFormatChoice::Transaction => StatementFormat::Transaction,
    }
}

/// One chunk of the plan's clients as a statement run.
#[must_use]
pub fn run(plan: &RunPlan, chunk: &RunChunk) -> NewStatementRun {
    NewStatementRun {
        as_of: plan.as_of,
        format: wire_format(plan.format),
        start: plan.start,
        contact_ids: chunk.clients.clone(),
    }
}
