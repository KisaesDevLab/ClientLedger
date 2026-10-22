//! Who answers Bulk Entry: the `--demo` engine ([`DemoBatchEngine`], the
//! `company::demo::batch` twin of `/api/v1/batch/*`) or a test double
//! through [`BatchEngine`], or the live engine through the window's request
//! group ([`live`]; never on the egui thread).

use cl_api::{
    ApiError, BatchGroup, BatchKind, BatchSaved, BatchValidation, ColumnMapping, IdempotencyKey,
    LocalValidation, OrphanLabel, ParsedRows, RequestGroup, SuggestedAccount,
};
use cl_core::ids::ContactId;

use crate::company::demo::{DemoState, batch};
use crate::company::{CompanyLink, Task};

/// Bulk Entry's engine when it is not the live one.
pub(crate) trait BatchEngine: Send {
    /// Checks the rows (posts nothing).
    fn validate(
        &mut self,
        link: &CompanyLink,
        groups: &[BatchGroup],
    ) -> Result<BatchValidation, ApiError>;
    /// Posts every row, or nothing.
    fn save(
        &mut self,
        link: &CompanyLink,
        groups: &[BatchGroup],
        key: &IdempotencyKey,
        add_new_names: bool,
    ) -> Result<BatchSaved, ApiError>;
    /// The account to suggest for a name.
    fn suggest_account(
        &mut self,
        link: &CompanyLink,
        contact: ContactId,
    ) -> Result<Option<SuggestedAccount>, ApiError>;
    /// Reads a CSV or pasted file into rows of text.
    fn parse_csv(
        &mut self,
        link: &CompanyLink,
        kind: BatchKind,
        bytes: &[u8],
        mapping: &ColumnMapping,
    ) -> Result<ParsedRows, ApiError>;
}

/// The `--demo` engine's Bulk Entry.
#[derive(Debug, Default)]
pub(crate) struct DemoBatchEngine;

fn demo(link: &CompanyLink) -> Result<std::sync::MutexGuard<'_, DemoState>, ApiError> {
    link.demo_state()
        .ok_or(ApiError::Local(LocalValidation::InvalidValue))
}

impl BatchEngine for DemoBatchEngine {
    fn validate(
        &mut self,
        link: &CompanyLink,
        groups: &[BatchGroup],
    ) -> Result<BatchValidation, ApiError> {
        batch::validate(&*demo(link)?, groups)
    }

    fn save(
        &mut self,
        link: &CompanyLink,
        groups: &[BatchGroup],
        key: &IdempotencyKey,
        add_new_names: bool,
    ) -> Result<BatchSaved, ApiError> {
        batch::save(&mut *demo(link)?, groups, key, add_new_names)
    }

    fn suggest_account(
        &mut self,
        link: &CompanyLink,
        contact: ContactId,
    ) -> Result<Option<SuggestedAccount>, ApiError> {
        batch::suggest_account(&*demo(link)?, contact)
    }

    fn parse_csv(
        &mut self,
        _link: &CompanyLink,
        kind: BatchKind,
        bytes: &[u8],
        mapping: &ColumnMapping,
    ) -> Result<ParsedRows, ApiError> {
        batch::parse_csv(kind, bytes, mapping)
    }
}

/// The live engine's Bulk Entry over the window's request group: the same
/// four calls as [`BatchEngine`], each a queued task.
///
/// The `/batch/*` client calls (`validate`, `save`, `parse_csv`) arrive
/// with lane A (`target/plans/P15A.md` WP-A1); until then they answer
/// `BatchUnavailable` ("needs an engine update"), which the screen shows
/// in plain words. The screen only calls them when the engine advertises
/// the `batch-*-v1` capabilities, which the pinned engine does not yet.
/// The account suggestion runs the engine's existing contact lookup in the
/// suggestion task (`suggest.rs`), with the rules and the AI.
pub(crate) mod live {
    use super::{
        ApiError, BatchGroup, BatchKind, BatchSaved, BatchValidation, ColumnMapping,
        IdempotencyKey, LocalValidation, OrphanLabel, ParsedRows, RequestGroup, Task,
    };
    use crate::company::CompanyLink;

    fn unavailable<T>() -> Result<T, ApiError> {
        Err(ApiError::Local(LocalValidation::BatchUnavailable))
    }

    /// `POST /batch/validate`.
    pub(crate) fn validate(
        link: &CompanyLink,
        group: Option<&RequestGroup>,
        groups: Vec<BatchGroup>,
    ) -> Option<Task<BatchValidation>> {
        let (session, group) = (link.session()?, group?);
        let p = group
            .read(&session, move |_s| async move {
                let _ = groups;
                unavailable()
            })
            .ok()?;
        Some(Task::Pending(p))
    }

    /// `POST /batch/save` with the idempotency key.
    pub(crate) fn save(
        link: &CompanyLink,
        group: Option<&RequestGroup>,
        groups: Vec<BatchGroup>,
        key: IdempotencyKey,
        add_new_names: bool,
    ) -> Option<Task<BatchSaved>> {
        let (session, group) = (link.session()?, group?);
        let label = OrphanLabel {
            what: "orphan.batch_save",
            key: Some(key.clone()),
        };
        let p = group
            .write(&session, label, move |_s| async move {
                let _ = (groups, key, add_new_names);
                unavailable()
            })
            .ok()?;
        Some(Task::Pending(p))
    }

    /// `POST /batch/parse-csv`.
    pub(crate) fn parse_csv(
        link: &CompanyLink,
        group: Option<&RequestGroup>,
        kind: BatchKind,
        bytes: Vec<u8>,
        mapping: ColumnMapping,
    ) -> Option<Task<ParsedRows>> {
        let (session, group) = (link.session()?, group?);
        let p = group
            .read(&session, move |_s| async move {
                let _ = (kind, bytes, mapping);
                unavailable()
            })
            .ok()?;
        Some(Task::Pending(p))
    }
}
