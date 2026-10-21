//! Who answers Make Journal Entry when it is not the live engine over the
//! queue: the `--demo` engine ([`DemoJournalEngine`], the
//! `company::demo::journal` twin of the part A contract) or a test double.
//! The live engine is reached through the window's request group (reads
//! and writes on the queue, never on the egui thread).

use cl_api::{ApiError, LocalValidation, NewJournalEntry, RecordVersion, Transaction, Versioned};
use cl_core::Secret;
use cl_core::ids::TransactionId;

use crate::company::CompanyLink;
use crate::company::demo::DemoState;
use crate::company::demo::journal;

/// Make Journal Entry's engine when it is not the live one.
pub(crate) trait JournalEngine: Send {
    /// Posts (or, with `edit`, replaces) an entry.
    fn save(
        &mut self,
        link: &CompanyLink,
        entry: &NewJournalEntry,
        edit: Option<(TransactionId, RecordVersion)>,
        password: Option<&Secret>,
    ) -> Result<(TransactionId, RecordVersion), ApiError>;
    /// A saved entry.
    fn get(
        &mut self,
        link: &CompanyLink,
        id: TransactionId,
    ) -> Result<Versioned<Transaction>, ApiError>;
    /// Every entry, newest first (Prev/Next).
    fn list(&mut self, link: &CompanyLink) -> Vec<TransactionId>;
    /// Voids an entry (and its reversal).
    fn void(
        &mut self,
        link: &CompanyLink,
        id: TransactionId,
        password: Option<&Secret>,
    ) -> Result<(), ApiError>;
    /// Deletes an entry (and its reversal).
    fn delete(
        &mut self,
        link: &CompanyLink,
        id: TransactionId,
        password: Option<&Secret>,
    ) -> Result<(), ApiError>;
    /// The suggested next entry number.
    fn next_number(&mut self, link: &CompanyLink) -> Option<String>;
}

/// The `--demo` engine's journal entries.
#[derive(Debug, Default)]
pub(crate) struct DemoJournalEngine;

fn demo(link: &CompanyLink) -> Result<std::sync::MutexGuard<'_, DemoState>, ApiError> {
    link.demo_state()
        .ok_or(ApiError::Local(LocalValidation::InvalidValue))
}

impl JournalEngine for DemoJournalEngine {
    fn save(
        &mut self,
        link: &CompanyLink,
        entry: &NewJournalEntry,
        edit: Option<(TransactionId, RecordVersion)>,
        password: Option<&Secret>,
    ) -> Result<(TransactionId, RecordVersion), ApiError> {
        journal::save(&mut *demo(link)?, entry, edit, password)
    }

    fn get(
        &mut self,
        link: &CompanyLink,
        id: TransactionId,
    ) -> Result<Versioned<Transaction>, ApiError> {
        journal::get(&mut *demo(link)?, id)
    }

    fn list(&mut self, link: &CompanyLink) -> Vec<TransactionId> {
        demo(link)
            .map(|s| journal::list(&s).into_iter().map(|r| r.id).collect())
            .unwrap_or_default()
    }

    fn void(
        &mut self,
        link: &CompanyLink,
        id: TransactionId,
        password: Option<&Secret>,
    ) -> Result<(), ApiError> {
        journal::void(&mut *demo(link)?, id, password)
    }

    fn delete(
        &mut self,
        link: &CompanyLink,
        id: TransactionId,
        password: Option<&Secret>,
    ) -> Result<(), ApiError> {
        journal::delete(&mut *demo(link)?, id, password)
    }

    fn next_number(&mut self, link: &CompanyLink) -> Option<String> {
        demo(link).ok().map(|s| journal::next_number(&s))
    }
}
