//! Typed client for the local bookkeeping engine API.
//!
//! - Plain HTTP to `127.0.0.1` only (the host is a constant); no proxy, no
//!   TLS (P33 adds it for LAN), no redirects, no cookie jar.
//! - The engine endpoint (port, launch token, generation) comes from an
//!   [`EndpointSource`] the host implements and is re-read for every attempt.
//! - Money is exact (CLAUDE.md non-negotiable 6): money values on the wire are
//!   `cl_core::Money`/`Quantity`/`Rate` strings. The float lints below are
//!   deny-level, `clippy.toml` bans every float conversion, and
//!   `tests/float_ban.rs` proves each ban fires; the xtask `nofloat` scan
//!   covers `src/`.
//! - Nothing secret or personal is logged: no tokens, cookies, passwords,
//!   amounts, names or memo text.
#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

mod client;
pub mod endpoint;
pub mod errors;
pub mod idempotency;
pub mod queue;
pub mod resources;
pub mod session;
pub mod wire;

pub use client::{Budget, ClientConfig, LOOPBACK_HOST, Method, Timeouts};
pub use endpoint::{EndpointSource, EndpointState, EngineEndpoint, FixedEndpoint};
pub use errors::{
    ApiError, AuthFailure, ClosedPeriod, ConflictReason, ErrorCode, FieldPath, LocalValidation,
    UserText, ValidationError, ValidationIssue,
};
pub use idempotency::IdempotencyKey;
pub use queue::{
    ApiQueue, OrphanLabel, OrphanOutcome, OrphanResult, Pending, QueueConfig, QueueError,
    RequestGroup, ScopeSource, ShutdownReport,
};
pub use resources::accounts::{
    Account, AccountFields, AccountFilter, AccountSort, AccountType, AccountUpdate, Accounts,
};
pub use resources::common::SortDir;
pub use resources::company::{CompanyProfile, CompanyProfileUpdate, CompanySettings, EntityType};
pub use resources::contacts::{
    Contact, ContactFields, ContactFilter, ContactSort, ContactType, ContactUpdate, Contacts,
    NewContact,
};
pub use resources::items::{Item, ItemFields, ItemFilter, ItemSort, ItemUpdate, Items};
pub use resources::ledger::LedgerStamp;
pub use resources::operations::{OPERATIONS, Operation, RetryClass};
pub use resources::posting::{ClosedPeriodAuthorization, PostingOptions};
pub use resources::system::{SmtpTest, SmtpTestResult};
pub use resources::tags::{Tag, TagFields, TagFilter, TagUpdate, Tags};
pub use resources::transactions::{
    Basis, JeLine, MAX_LINES, NewJournalEntry, ReportBasis, Side, Transaction, TransactionFilter,
    TransactionLine, TransactionSummary, Transactions, TxnSort, TxnStatus, TxnType,
};
pub use resources::versioning::{EditableFields, RecordVersion, Versioned};
pub use session::{
    Client, CompanySummary, CreatedCompany, LoginOutcome, NewCompany, OpenCompany, Scope, Session,
    TfaChallenge, TfaMethod,
};
pub use wire::{Capabilities, DesktopInfo, Health, Page, PageRequest, ReportAmount};
