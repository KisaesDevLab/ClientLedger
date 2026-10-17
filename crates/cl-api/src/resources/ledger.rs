//! The ledger-version stamp (`GET /api/v1/ledger-version`, P05 plan §2.5).
//!
//! A per-company counter bumped by database triggers on every journal-line
//! change and on balance-relevant transaction changes. Only its movement is
//! meaningful: registers refetch when it moves (P12). It is never a record
//! version.

use crate::errors::ApiError;
use crate::resources::reconcile::read_stamp;
use crate::session::Session;

/// One reading of the ledger-version stamp. Compare two readings with `==`;
/// the value itself means nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LedgerStamp(u64);

impl Session {
    /// Reads the ledger-version stamp of the open company.
    ///
    /// # Errors
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn ledger_version(&self) -> Result<LedgerStamp, ApiError> {
        read_stamp(self).await.map(LedgerStamp)
    }
}
