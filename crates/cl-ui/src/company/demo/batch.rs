//! The demo engine's Bulk Entry (P15A): the `--demo` twin of
//! `/api/v1/batch/*`. Synthetic.
//!
//! WP0 stub: every call answers `Local(BatchUnavailable)`. Lane C
//! (`target/plans/P15A.md` §3 WP-C1) builds the twin: name and account
//! resolution against [`DemoState`], an atomic save through the existing
//! demo postings (clone, post every group, swap), the idempotency replay,
//! the closing password per locked row, the account suggestion and
//! `parse_csv` over `cl_import::csv`.

use std::collections::BTreeMap;

use cl_api::{
    ApiError, BatchGroup, BatchKind, BatchSaved, BatchValidation, ColumnMapping, IdempotencyKey,
    LocalValidation, ParsedRows, SuggestedAccount,
};
use cl_core::ids::ContactId;

use super::DemoState;

/// The demo's Bulk Entry saves: each idempotency key with the hash of the
/// body it was first sent with and what that save returned.
#[derive(Clone, Debug, Default)]
pub struct DemoBatches {
    /// Replays by key: (body hash, first result).
    pub replays: BTreeMap<String, (u64, BatchSaved)>,
}

fn unavailable<T>() -> Result<T, ApiError> {
    Err(ApiError::Local(LocalValidation::BatchUnavailable))
}

/// `POST /batch/validate`: resolves names and accounts, posts nothing.
///
/// # Errors
/// `Local(BatchUnavailable)` until lane C.
pub fn validate(_state: &DemoState, _groups: &[BatchGroup]) -> Result<BatchValidation, ApiError> {
    unavailable()
}

/// `POST /batch/save`: every row or nothing (`BatchRows` names the
/// refused rows).
///
/// # Errors
/// `Local(BatchUnavailable)` until lane C.
pub fn save(
    _state: &mut DemoState,
    _groups: &[BatchGroup],
    _key: &IdempotencyKey,
    _add_new_names: bool,
) -> Result<BatchSaved, ApiError> {
    unavailable()
}

/// `GET /batch/suggest-account`: the name's default account, then the
/// most recent one.
///
/// # Errors
/// `Local(BatchUnavailable)` until lane C.
pub fn suggest_account(
    _state: &DemoState,
    _contact: ContactId,
) -> Result<Option<SuggestedAccount>, ApiError> {
    unavailable()
}

/// `POST /batch/parse-csv`: the file's rows as text.
///
/// # Errors
/// `Local(BatchUnavailable)` until lane C.
pub fn parse_csv(
    _kind: BatchKind,
    _bytes: &[u8],
    _mapping: &ColumnMapping,
) -> Result<ParsedRows, ApiError> {
    unavailable()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stubs_answer_unavailable() {
        let mut s = DemoState::new();
        let gate = ApiError::Local(LocalValidation::BatchUnavailable);
        assert_eq!(validate(&s, &[]), Err(gate.clone()));
        assert_eq!(
            save(&mut s, &[], &IdempotencyKey::new(), false),
            Err(gate.clone())
        );
        let c = ContactId::parse("00000000-0000-4000-8000-000000000001").expect("id");
        assert_eq!(suggest_account(&s, c), Err(gate.clone()));
        assert_eq!(
            parse_csv(BatchKind::Expense, b"", &ColumnMapping::default()),
            Err(gate)
        );
        assert!(s.batch.replays.is_empty());
    }
}
