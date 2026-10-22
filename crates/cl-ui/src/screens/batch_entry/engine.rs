//! Who answers Bulk Entry when it is not the live engine over the queue:
//! the `--demo` engine ([`DemoBatchEngine`], the `company::demo::batch`
//! twin of `/api/v1/batch/*`) or a test double. The live engine is reached
//! through the window's request group (never on the egui thread).
//!
//! WP0 scaffold: the trait and the demo adapter; lane D (WP-D1) wires them
//! into the screen.

#![allow(dead_code)] // wired into the screen by lane D (WP-D1)

use cl_api::{
    ApiError, BatchGroup, BatchKind, BatchSaved, BatchValidation, ColumnMapping, IdempotencyKey,
    LocalValidation, ParsedRows, SuggestedAccount,
};
use cl_core::ids::ContactId;

use crate::company::CompanyLink;
use crate::company::demo::{DemoState, batch};

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
