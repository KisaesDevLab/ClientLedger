//! Assess Finance Charges (P23): one chunk of the checked plan as the
//! engine's request, each client with the preview's expected charge
//! (copied, never recomputed).

use cl_api::{AssessContact, AssessRequest};
use cl_core::CalendarDate;
use cl_model::forms::finance_charge::AssessChunk;

/// One chunk of clients assessed on `date`.
#[must_use]
pub fn assess_request(date: CalendarDate, chunk: &AssessChunk) -> AssessRequest {
    AssessRequest {
        assessment_date: date,
        contacts: chunk
            .clients
            .iter()
            .map(|(contact_id, expected_charge)| AssessContact {
                contact_id: *contact_id,
                expected_charge: *expected_charge,
            })
            .collect(),
    }
}
