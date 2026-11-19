//! `QuickBooks` Desktop and other-system imports (P31; engine U-260 to U-268).
//!
//! The engine does all of the accounting. It reads every file, parses every
//! amount exactly (`imports-amounts-v1`), commits a whole batch of files in
//! one transaction as a background job, undoes it the same way, and computes
//! the tie-out against the reference files. This module only carries its
//! answers.
//!
//! - Nothing here parses an amount from a file or adds one up. Preview totals
//!   and tie-out amounts are the engine's exact decimal strings read into
//!   [`Money`]; a JSON number is `InexactMoney`, never rounded.
//! - Every upload asks the engine never to post an unmapped account to
//!   suspense (`unmappedAccountsToSuspense: false`, Q-P31c);
//!   [`UploadRequest`] has no way to change that. The file goes up under a
//!   neutral name (`import.<ext>`): the owner's file name never reaches the
//!   engine.
//! - Commit and undo take the screen's [`IdempotencyKey`] (`imports-batch-v1`
//!   replays it) and a backup reference, and are sent once: the engine
//!   answers 202 and the screen polls [`Imports::batch`]. "Try again" after a
//!   lost answer is the owner's click with the same key; "Check status"
//!   reads the batch.
//! - Engine messages can name people (U-270); refusals keep codes, row
//!   numbers and ids only.
//! - `Debug` output names ids, kinds, statuses, dates and counts only: never
//!   amounts, account or contact names, or file contents.

use std::collections::BTreeMap;
use std::fmt;

use cl_core::ids::{AccountId, ContactId, ItemId, TransactionId, parse_uuid};
use cl_core::{CalendarDate, Money};
use rust_decimal::Decimal;
use serde_json::{Map, Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::customer_payments::{error_code, require};
use crate::resources::operations::{
    IMPORT_BATCH, IMPORT_BATCH_COMMIT, IMPORT_BATCH_CREATE, IMPORT_BATCH_SPOT_CHECK,
    IMPORT_BATCH_TIEOUT, IMPORT_BATCH_TRANSACTIONS, IMPORT_BATCH_UNDO, IMPORT_BATCH_VERIFY,
    IMPORT_BATCHES, IMPORTS_DISCARD, IMPORTS_SESSION, IMPORTS_UPLOAD, Operation,
};
use crate::resources::posting::{ClosedPeriodAuthorization, PostingOptions, posting_body};
use crate::session::Session;
use crate::wire::Capabilities;

const WHAT: &str = "import";

/// The engine's upload limit (10 MB per file), checked before sending.
pub const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;
/// The file types the engine reads.
pub const FILE_EXTENSIONS: [&str; 5] = ["csv", "txt", "tsv", "xlsx", "xls"];
/// History page size.
pub const HISTORY_PAGE: u16 = 50;
/// The most History pages read at once.
pub const HISTORY_MAX_PAGES: usize = 20;
/// The longest spot-check note the desktop sends.
pub const MAX_NOTE_CHARS: usize = 500;
/// The engine's spot-check limits: items, key and label lengths.
pub const MAX_SPOT_ITEMS: usize = 200;
/// The longest spot-check key.
pub const MAX_SPOT_KEY_CHARS: usize = 100;
/// The longest spot-check label.
pub const MAX_SPOT_LABEL_CHARS: usize = 300;
/// The largest page of a batch's transactions the engine gives.
pub const TRANSACTIONS_PAGE: u16 = 200;
/// Tie-out and verify always compare with the reference trial balance
/// (S9); the desktop never turns that off.
const REQUIRE_REFERENCE: &[u8] = br#"{"requireReference":true}"#;

/// Which import flow a screen runs; each needs its own capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Flow {
    /// `QuickBooks` Desktop conversion (lists, history, open items, valuation).
    QbdConversion,
    /// Another accounting system (or QBD GL only) on the Bulk Import kinds.
    OtherSystem,
    /// Invoices, bills and journal entries from a spreadsheet (kind
    /// `transactions`).
    Transactions,
}

impl Flow {
    /// Every capability the flow needs.
    #[must_use]
    pub const fn capabilities(self) -> &'static [&'static str] {
        match self {
            Flow::QbdConversion => &[
                Capabilities::IMPORTS_AMOUNTS,
                Capabilities::IMPORTS_BATCH,
                Capabilities::QBD_OPEN_ITEMS,
                Capabilities::QBD_MAPPING,
                Capabilities::QBD_DATES,
                Capabilities::IMPORTS_TIEOUT,
            ],
            Flow::OtherSystem => &[
                Capabilities::IMPORTS_AMOUNTS,
                Capabilities::IMPORTS_BATCH,
                Capabilities::QBD_MAPPING,
                Capabilities::IMPORTS_TIEOUT,
            ],
            Flow::Transactions => &[
                Capabilities::IMPORTS_AMOUNTS,
                Capabilities::IMPORTS_BATCH,
                Capabilities::IMPORTS_TRANSACTIONS,
            ],
        }
    }

    /// True when the engine has everything the flow needs.
    #[must_use]
    pub fn supported(self, caps: &Capabilities) -> bool {
        self.capabilities().iter().all(|c| caps.has(c))
    }
}

fn decode_err() -> ApiError {
    ApiError::Decode { what: WHAT }
}

fn local(v: LocalValidation) -> ApiError {
    ApiError::Local(v)
}

macro_rules! uuid_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(uuid::Uuid);

        impl $name {
            /// Parses an id.
            ///
            /// # Errors
            /// `Decode` when it is not a UUID.
            pub fn parse(s: &str) -> Result<$name, ApiError> {
                parse_uuid(s).map($name).map_err(|_| decode_err())
            }

            /// The raw UUID (seeds, keys).
            #[must_use]
            pub const fn as_uuid(self) -> uuid::Uuid {
                self.0
            }
        }

        impl std::str::FromStr for $name {
            type Err = ApiError;

            fn from_str(s: &str) -> Result<$name, ApiError> {
                $name::parse(s)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

uuid_id!(
    /// One uploaded import file (a session in the engine).
    ImportSessionId
);
uuid_id!(
    /// One import batch: the unit of commit, undo and verification.
    ImportBatchId
);

macro_rules! wire_enum {
    ($(#[$doc:meta])* $name:ident { $($(#[$vdoc:meta])* $v:ident => $w:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $($(#[$vdoc])* $v,)+
        }

        impl $name {
            /// Every value, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$v),+];

            /// The wire name.
            #[must_use]
            pub const fn as_wire(self) -> &'static str {
                match self {
                    $($name::$v => $w,)+
                }
            }

            /// Reads a wire name.
            #[must_use]
            pub fn from_wire(s: &str) -> Option<$name> {
                match s {
                    $($w => Some($name::$v),)+
                    _ => None,
                }
            }
        }
    };
}

wire_enum!(
    /// The system a file came from.
    SourceSystem {
        /// Accounting Power.
        AccountingPower => "accounting_power",
        /// `QuickBooks` Online.
        QuickBooksOnline => "quickbooks_online",
        /// `QuickBooks` Desktop (conversion and GL only).
        QuickBooksDesktop => "quickbooks_desktop",
        /// The engine's generic CSV layout.
        Generic => "generic",
    }
);

wire_enum!(
    /// What a file holds.
    ImportKind {
        /// Chart of accounts.
        Coa => "coa",
        /// Customers, vendors or other names (one list per file).
        Contacts => "contacts",
        /// A trial balance (opening balances, or a reference to tie out to).
        TrialBalance => "trial_balance",
        /// Journal or general ledger transactions.
        GlTransactions => "gl_transactions",
        /// QBD Open Invoices (E2a).
        OpenInvoices => "open_invoices",
        /// QBD Unpaid Bills Detail (E2a).
        UnpaidBills => "unpaid_bills",
        /// QBD Item List (E2b).
        Items => "items",
        /// QBD Inventory Valuation Summary (E2b).
        InventoryValuation => "inventory_valuation",
        /// Invoices, bills and journal entries from a spreadsheet (E4).
        Transactions => "transactions",
    }
);

impl ImportKind {
    /// The capability the engine needs for this kind beyond the batch.
    #[must_use]
    pub const fn capability(self) -> Option<&'static str> {
        match self {
            ImportKind::OpenInvoices | ImportKind::UnpaidBills => {
                Some(Capabilities::QBD_OPEN_ITEMS)
            }
            ImportKind::Items | ImportKind::InventoryValuation => Some(Capabilities::QBD_MAPPING),
            ImportKind::Transactions => Some(Capabilities::IMPORTS_TRANSACTIONS),
            ImportKind::Coa
            | ImportKind::Contacts
            | ImportKind::TrialBalance
            | ImportKind::GlTransactions => None,
        }
    }
}

wire_enum!(
    /// Whether a file is posted or only compared against (S9).
    FileRole {
        /// Posted by the commit.
        Post => "post",
        /// Never posted; the tie-out compares the books with it.
        Reference => "reference",
    }
);

wire_enum!(
    /// Which list a contacts file is.
    ContactKind {
        /// Customer list.
        Customer => "customer",
        /// Vendor list.
        Vendor => "vendor",
        /// QBD Other Names: imported as vendors (Q-78).
        Other => "other",
    }
);

wire_enum!(
    /// Which Accounting Power balance column a trial balance uses.
    TbColumn {
        /// Beginning balance.
        Beginning => "beginning",
        /// Adjusted balance.
        Adjusted => "adjusted",
    }
);

wire_enum!(
    /// Where one uploaded file stands.
    SessionStatus {
        /// Read and checked.
        Uploaded => "uploaded",
        /// Checked again against the books.
        Validated => "validated",
        /// Its batch is committing.
        Committing => "committing",
        /// Posted by a committed batch.
        Committed => "committed",
        /// Its batch failed (nothing posted).
        Failed => "failed",
        /// Discarded.
        Cancelled => "cancelled",
        /// Its batch was undone.
        Undone => "undone",
    }
);

wire_enum!(
    /// What a batch is.
    BatchKind {
        /// A full conversion (QBD, or another system's history).
        Conversion => "conversion",
        /// One file on its own.
        Single => "single",
        /// Spreadsheet transactions.
        Transactions => "transactions",
    }
);

wire_enum!(
    /// Where a batch stands (S1).
    BatchStatus {
        /// Grouped, not committed.
        Draft => "draft",
        /// The commit job is running.
        Committing => "committing",
        /// Posted.
        Committed => "committed",
        /// The commit job failed; nothing was posted. Retry with a new key.
        Failed => "failed",
        /// The undo job is running.
        Undoing => "undoing",
        /// Undone: nothing it created is left.
        Undone => "undone",
    }
);

impl BatchStatus {
    /// A job is running: keep polling.
    #[must_use]
    pub const fn is_running(self) -> bool {
        matches!(self, BatchStatus::Committing | BatchStatus::Undoing)
    }
}

/// One file to upload, checked before anything is sent.
#[derive(Clone, PartialEq, Eq)]
pub struct UploadRequest {
    /// What the file holds.
    pub kind: ImportKind,
    /// Where it came from.
    pub source: SourceSystem,
    /// Posted, or a reference for the tie-out.
    pub role: FileRole,
    /// The file's bytes, exactly as read.
    pub bytes: Vec<u8>,
    /// The owner's file name; only its extension is used.
    pub file_name: String,
    /// Which list a contacts file is (required for contacts, else `None`).
    pub contact_kind: Option<ContactKind>,
    /// Accounting Power trial balance column.
    pub tb_column: Option<TbColumn>,
    /// The trial balance date when the file carries none (Accounting Power,
    /// Generic).
    pub tb_report_date: Option<CalendarDate>,
    /// An account list row whose number the company already has updates
    /// that account to the file's name and type (engine
    /// `updateExistingCoa`; the commit remembers the old ones and undo puts
    /// them back). Account lists only (P31 QA-1, Q-P31ac: a QBD switch
    /// into a new company uses the `QuickBooks` chart over the starter one).
    pub update_existing_coa: bool,
}

impl fmt::Debug for UploadRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UploadRequest")
            .field("kind", &self.kind)
            .field("source", &self.source)
            .field("role", &self.role)
            .field("bytes", &self.bytes.len())
            .field("contact_kind", &self.contact_kind)
            .field("tb_column", &self.tb_column)
            .field("tb_report_date", &self.tb_report_date)
            .field("update_existing_coa", &self.update_existing_coa)
            .finish_non_exhaustive()
    }
}

/// The file's extension when the engine reads that type (lower-case).
#[must_use]
pub fn file_extension(file_name: &str) -> Option<String> {
    let ext = file_name.rsplit_once('.')?.1.to_ascii_lowercase();
    FILE_EXTENSIONS.contains(&ext.as_str()).then_some(ext)
}

impl UploadRequest {
    /// Checks the request before anything is sent.
    ///
    /// # Errors
    /// `AttachmentTooLarge` over 10 MB; `AttachmentType` for another file
    /// type; `InvalidValue` for an empty file, a contacts file without its
    /// list, a list on another kind, or a trial balance option on another
    /// kind.
    pub fn check(&self) -> Result<String, LocalValidation> {
        if self.bytes.len() > MAX_UPLOAD_BYTES {
            return Err(LocalValidation::AttachmentTooLarge);
        }
        let ext = file_extension(&self.file_name).ok_or(LocalValidation::AttachmentType)?;
        let contacts = self.kind == ImportKind::Contacts;
        let tb = self.kind == ImportKind::TrialBalance;
        if self.bytes.is_empty()
            || contacts != self.contact_kind.is_some()
            || (!tb && (self.tb_column.is_some() || self.tb_report_date.is_some()))
            || (self.update_existing_coa && self.kind != ImportKind::Coa)
        {
            return Err(LocalValidation::InvalidValue);
        }
        Ok(ext)
    }

    /// The engine's `options` JSON. Always refuses suspense for unmapped
    /// accounts (Q-P31c).
    #[must_use]
    pub fn options(&self) -> Value {
        let mut m = Map::new();
        m.insert("unmappedAccountsToSuspense".into(), json!(false));
        if let Some(k) = self.contact_kind {
            m.insert("contactKind".into(), json!(k.as_wire()));
        }
        if let Some(c) = self.tb_column {
            m.insert("tbColumn".into(), json!(c.as_wire()));
        }
        if let Some(d) = self.tb_report_date {
            m.insert("tbReportDate".into(), json!(d.to_wire()));
        }
        if self.update_existing_coa {
            m.insert("updateExistingCoa".into(), json!(true));
        }
        Value::Object(m)
    }

    /// The capabilities this upload needs.
    fn capabilities(&self) -> Vec<&'static str> {
        let mut caps = vec![Capabilities::IMPORTS_AMOUNTS];
        if let Some(c) = self.kind.capability() {
            caps.push(c);
        }
        if self.role == FileRole::Reference {
            caps.push(Capabilities::IMPORTS_BATCH);
        }
        if self.contact_kind == Some(ContactKind::Other) {
            caps.push(Capabilities::QBD_MAPPING);
        }
        caps
    }

    /// The multipart body: `kind`, `sourceSystem`, `role`, `options` (JSON
    /// text) and the file under a neutral name.
    fn multipart(&self, ext: &str) -> (String, Vec<u8>) {
        let boundary = format!("cl-import-{}", uuid::Uuid::new_v4().simple());
        let mut body = Vec::with_capacity(self.bytes.len() + 1024);
        for (name, value) in [
            ("kind", self.kind.as_wire().to_string()),
            ("sourceSystem", self.source.as_wire().to_string()),
            ("role", self.role.as_wire().to_string()),
            ("options", self.options().to_string()),
        ] {
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                )
                .as_bytes(),
            );
        }
        let content_type = match ext {
            "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "xls" => "application/vnd.ms-excel",
            _ => "text/csv",
        };
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"import.{ext}\"\r\nContent-Type: {content_type}\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&self.bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        (boundary, body)
    }
}

/// A row the engine could not read: row number, column and code only (the
/// engine's message can quote cells).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowProblem {
    /// 1-based file row; 0 for the whole file.
    pub row_number: u64,
    /// The column, when the engine named one.
    pub field: Option<String>,
    /// The engine code (`IMPORT_AMOUNT_INVALID`, `IMPORT_BAD_DATE`, …).
    pub code: String,
}

/// One uploaded file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportSession {
    /// Its id.
    pub id: ImportSessionId,
    /// What it holds.
    pub kind: ImportKind,
    /// Where it came from.
    pub source: SourceSystem,
    /// Posted or reference (`None` from an engine without E1).
    pub role: Option<FileRole>,
    /// Where it stands.
    pub status: SessionStatus,
    /// Data rows.
    pub row_count: u64,
    /// Rows with problems.
    pub error_count: u64,
    /// The report date the engine read (trial balances, agings, valuation).
    pub report_date: Option<CalendarDate>,
    /// The batch it is in.
    pub batch: Option<ImportBatchId>,
}

/// What the engine read from a file. Totals are exact; `Debug` hides them.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct FilePreview {
    /// Rows read.
    pub total_rows: u64,
    /// Rows with problems.
    pub error_count: u64,
    /// Transactions found (journal files).
    pub entry_count: Option<u64>,
    /// The report date (trial balances).
    pub report_date: Option<CalendarDate>,
    /// Debits, as the engine summed them (trial balances).
    pub total_debit: Option<Money>,
    /// Credits, as the engine summed them (trial balances).
    pub total_credit: Option<Money>,
    /// How each account the file names was matched (E2b
    /// `accountMatches[]`, every row; empty before E2b).
    pub account_matches: Vec<AccountMatch>,
}

/// How the engine matched an account a file names (E2b, `qbd-mapping-v1`):
/// `{sourceKey, number?, name?, match, accountId?, matchedBy?, rows,
/// systemRole?}`, one per source key over every row of the file.
///
/// The engine matches each file against the chart as it is now: a trial
/// balance or journal names an account the batch's own chart file creates
/// as `unmatched`. The screen combines the files (and the batch's
/// `accountMap`). Unmatched accounts never go to suspense (Q-P31c).
#[derive(Clone, PartialEq, Eq)]
pub struct AccountMatch {
    /// The key the batch's `accountMap` uses (the source number, else name).
    pub source_key: String,
    /// The source account number, when it has one.
    pub number: Option<String>,
    /// The source account name (owner data; never logged).
    pub name: String,
    /// What the engine will do with it.
    pub status: MatchStatus,
    /// The company account it matched (`Existing`).
    pub account: Option<AccountId>,
    /// How an existing account was found.
    pub matched_by: Option<MatchedBy>,
    /// Rows (chart, trial balance, item) or journal lines naming it.
    pub rows: u64,
    /// A chart row of type A/R or A/P (merged into the company's own,
    /// Q-P31k), or the source's retained earnings (U-272).
    pub system_role: Option<SystemRole>,
}

impl fmt::Debug for AccountMatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccountMatch")
            .field("status", &self.status)
            .field("matched_by", &self.matched_by)
            .field("rows", &self.rows)
            .field("system_role", &self.system_role)
            .finish_non_exhaustive()
    }
}

wire_enum!(
    /// How an existing account was found ([`AccountMatch::matched_by`]).
    MatchedBy {
        /// By account number.
        Number => "number",
        /// By name (the oldest account of that name and type).
        Name => "name",
    }
);

wire_enum!(
    /// The system account a chart row merges into.
    SystemRole {
        /// Accounts receivable.
        AccountsReceivable => "accounts_receivable",
        /// Accounts payable.
        AccountsPayable => "accounts_payable",
        /// Retained earnings (U-272, `qbd-retained-earnings-v1`): a QBD
        /// Equity row named "Retained Earnings" (number stripped) or a QBO
        /// row of that detail type. In a batch it merges into the company's
        /// retained earnings account even though `status` says `create`;
        /// `accountMap` "create" keeps it separate, an account id sends it
        /// there.
        RetainedEarnings => "retained_earnings",
    }
);

/// What a source account becomes (a value of the batch's `accountMap`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountTarget {
    /// Post to this company account (a chart row mapped here is not
    /// created).
    Account(AccountId),
    /// Create it from the batch's chart file even when an account of that
    /// name exists (`"create"`). Only for keys a chart row of the batch has:
    /// any other key fails the commit with `IMPORT_UNMAPPED_ACCOUNT`.
    Create,
}

impl AccountTarget {
    fn wire(self) -> String {
        match self {
            AccountTarget::Account(a) => a.to_string(),
            AccountTarget::Create => "create".to_string(),
        }
    }
}

/// Source account keys from the engine (account numbers or names: owner
/// data). `Debug` shows the count only.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SourceKeys(pub Vec<String>);

impl fmt::Debug for SourceKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SourceKeys({})", self.0.len())
    }
}

/// [`AccountMatch::status`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MatchStatus {
    /// An account already in the company (by number, then name).
    Existing,
    /// Made from the file's chart of accounts.
    Create,
    /// Neither: the owner must pick an account.
    Unmatched,
}

impl MatchStatus {
    /// Reads the engine's `match`.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<MatchStatus> {
        match s {
            "existing" => Some(MatchStatus::Existing),
            "create" => Some(MatchStatus::Create),
            "unmatched" => Some(MatchStatus::Unmatched),
            _ => None,
        }
    }
}

impl fmt::Debug for FilePreview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FilePreview")
            .field("total_rows", &self.total_rows)
            .field("error_count", &self.error_count)
            .field("entry_count", &self.entry_count)
            .field("report_date", &self.report_date)
            .field("account_matches", &self.account_matches.len())
            .finish_non_exhaustive()
    }
}

/// The engine's answer to an upload (and to reading a session back).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadOutcome {
    /// The file.
    pub session: ImportSession,
    /// What was read.
    pub preview: FilePreview,
    /// Rows the engine could not read.
    pub problems: Vec<RowProblem>,
}

impl UploadOutcome {
    /// Nothing blocks putting the file in a batch.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.problems.is_empty() && self.session.error_count == 0
    }
}

/// A QBD class mapped to a tag (built with `cl_model::lists::class_map`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassTarget {
    /// The tag group, for a `Parent:Child` class.
    pub group: Option<String>,
    /// The tag.
    pub tag: String,
}

/// Batch options (E1, E2b, E2c). Owner data (account keys, class names):
/// `Debug` shows counts only.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct BatchOptions {
    /// The fiscal year's first month (1-12); only for an empty company.
    pub fiscal_year_start_month: Option<u8>,
    /// History before this date is skipped (Q-142 "two years"); `None` is
    /// "All".
    pub history_from: Option<CalendarDate>,
    /// The conversion date (E2c): reference trial balances, open items and
    /// the valuation must be dated it; journal rows after it are refused.
    pub conversion_date: Option<CalendarDate>,
    /// Refuse the commit unless the company is empty (QBD, Q-P31h).
    pub require_empty_company: bool,
    /// Source account key → account or "create" (the mapping review's
    /// choices). Options can't change once the batch exists.
    pub account_map: BTreeMap<String, AccountTarget>,
    /// Source class → tag.
    pub class_map: BTreeMap<String, ClassTarget>,
}

impl fmt::Debug for BatchOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchOptions")
            .field("fiscal_year_start_month", &self.fiscal_year_start_month)
            .field("history_from", &self.history_from)
            .field("conversion_date", &self.conversion_date)
            .field("require_empty_company", &self.require_empty_company)
            .field("account_map", &self.account_map.len())
            .field("class_map", &self.class_map.len())
            .finish()
    }
}

impl BatchOptions {
    /// The engine's `options`.
    ///
    /// # Errors
    /// `InvalidValue` for a month outside 1-12 or a blank class tag.
    pub fn body(&self) -> Result<Value, LocalValidation> {
        let mut m = Map::new();
        if let Some(month) = self.fiscal_year_start_month {
            if !(1..=12).contains(&month) {
                return Err(LocalValidation::InvalidValue);
            }
            m.insert("fiscalYearStartMonth".into(), json!(month));
        }
        if let Some(d) = self.history_from {
            m.insert("historyFrom".into(), json!(d.to_wire()));
        }
        if let Some(d) = self.conversion_date {
            m.insert("conversionDate".into(), json!(d.to_wire()));
        }
        m.insert(
            "requireEmptyCompany".into(),
            json!(self.require_empty_company),
        );
        if !self.account_map.is_empty() {
            if self.account_map.keys().any(|k| k.trim().is_empty()) {
                return Err(LocalValidation::InvalidValue);
            }
            let am: Map<String, Value> = self
                .account_map
                .iter()
                .map(|(k, a)| (k.clone(), json!(a.wire())))
                .collect();
            m.insert("accountMap".into(), Value::Object(am));
        }
        if !self.class_map.is_empty() {
            let mut cm = Map::new();
            for (class, t) in &self.class_map {
                if t.tag.trim().is_empty() {
                    return Err(LocalValidation::InvalidValue);
                }
                let mut o = Map::new();
                if let Some(g) = &t.group {
                    o.insert("group".into(), json!(g));
                }
                o.insert("tag".into(), json!(t.tag));
                cm.insert(class.clone(), Value::Object(o));
            }
            m.insert("classMap".into(), Value::Object(cm));
        }
        Ok(Value::Object(m))
    }

    fn capabilities(&self) -> Vec<&'static str> {
        let mut caps = vec![Capabilities::IMPORTS_BATCH];
        if self.history_from.is_some() || self.conversion_date.is_some() {
            caps.push(Capabilities::QBD_DATES);
        }
        if !self.account_map.is_empty() || !self.class_map.is_empty() {
            caps.push(Capabilities::QBD_MAPPING);
        }
        caps
    }
}

/// A new batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewBatch {
    /// Where the files came from.
    pub source: SourceSystem,
    /// What the batch is.
    pub kind: BatchKind,
    /// The uploaded files, in commit order.
    pub sessions: Vec<ImportSessionId>,
    /// Options.
    pub options: BatchOptions,
}

/// One item of the spot-check list (Q-144).
#[derive(Clone, PartialEq, Eq)]
pub struct SpotCheckItem {
    /// Stable key (`bank:<account>`, `ar:<row>`, `sample:<n>`, …).
    pub key: String,
    /// The owner's label, as shown.
    pub label: String,
    /// Ticked.
    pub done: bool,
    /// The owner's note.
    pub note: String,
}

impl fmt::Debug for SpotCheckItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpotCheckItem")
            .field("key", &self.key)
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

/// A batch: the conversion record (Q-144).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportBatch {
    /// Its id.
    pub id: ImportBatchId,
    /// Where its files came from.
    pub source: SourceSystem,
    /// What it is.
    pub kind: BatchKind,
    /// Where it stands.
    pub status: BatchStatus,
    /// Its files.
    pub sessions: Vec<ImportSessionId>,
    /// The verified backup taken before the commit (file name).
    pub backup_ref: Option<String>,
    /// The company had no transactions at commit (tie-out uses the whole
    /// trial balance).
    pub company_was_empty: Option<bool>,
    /// The conversion date (the reference trial balance's date).
    pub conversion_date: Option<CalendarDate>,
    /// History start (`None` = all history).
    pub history_from: Option<CalendarDate>,
    /// When the tie-out was verified (engine text).
    pub verified_at: Option<String>,
    /// Who verified it (a user id).
    pub verified_by: Option<String>,
    /// The saved spot-check list.
    pub spot_check: Vec<SpotCheckItem>,
    /// When it was made (engine text).
    pub created_at: Option<String>,
    /// When it was committed (engine text).
    pub committed_at: Option<String>,
    /// Its files, in commit order, with what each commit did.
    pub files: Vec<BatchFile>,
    /// What the batch's dates mean (E2c; on create and on reading one
    /// batch, not in the History list).
    pub date_check: Option<DateCheck>,
    /// Why its last job failed (a failed batch, or a committed one whose
    /// undo failed).
    pub error: Option<BatchFailure>,
}

/// One file of a batch (`batch.sessions[]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchFile {
    /// The file.
    pub id: ImportSessionId,
    /// What it holds.
    pub kind: Option<ImportKind>,
    /// Posted or reference.
    pub role: Option<FileRole>,
    /// Where it stands.
    pub status: Option<SessionStatus>,
    /// Its report date.
    pub report_date: Option<CalendarDate>,
    /// What its commit did (once committed).
    pub result: Option<CommitSummary>,
}

/// Which item rows were listed but not imported (`itemsSkipped`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemsSkipped {
    /// Sales tax items.
    pub sales_tax: u64,
    /// Item groups.
    pub group: u64,
    /// Payment items.
    pub payment: u64,
    /// Already in the company.
    pub exists: u64,
    /// No income account (or a part with no asset or cost account).
    pub no_account: u64,
}

impl ItemsSkipped {
    /// All of them.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.sales_tax + self.group + self.payment + self.exists + self.no_account
    }
}

/// One file's commit result (`commitResult`): counts, and the valuation's
/// exact total. `Debug` hides the total.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CommitSummary {
    /// Rows or documents created.
    pub created: u64,
    /// Rows skipped (already there, or mapped to an existing account).
    pub skipped: u64,
    /// Names on both lists made one customer-and-vendor (E2b, Q-77).
    pub merged_to_both: u64,
    /// Customer:Job rows folded into their customer (no projects yet).
    pub jobs_folded: u64,
    /// Names or items brought in inactive.
    pub inactive: u64,
    /// Item rows listed but not imported, by reason.
    pub items_skipped: Option<ItemsSkipped>,
    /// Classes made tags through the batch's class map.
    pub classes_mapped: u64,
    /// Items given opening stock.
    pub items_valued: u64,
    /// The valuation's exact total (the engine's string; never summed here).
    pub valuation_total: Option<Money>,
    /// Journal rows before the history start, not posted (present whenever
    /// the batch has a history start).
    pub skipped_before_history: Option<u64>,
}

impl fmt::Debug for CommitSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommitSummary")
            .field("created", &self.created)
            .field("skipped", &self.skipped)
            .field("merged_to_both", &self.merged_to_both)
            .field("jobs_folded", &self.jobs_folded)
            .field("inactive", &self.inactive)
            .field("items_skipped", &self.items_skipped)
            .field("classes_mapped", &self.classes_mapped)
            .field("items_valued", &self.items_valued)
            .field("skipped_before_history", &self.skipped_before_history)
            .finish_non_exhaustive()
    }
}

wire_enum!(
    /// A date warning of a batch (never blocks).
    DateWarningCode {
        /// The journal starts later than the day after the opening
        /// balances: anything in between is missing.
        HistoryGap => "IMPORT_HISTORY_GAP",
        /// Journal rows on or before the opening balance date (they count
        /// twice).
        JournalBeforeOpening => "IMPORT_JOURNAL_BEFORE_OPENING",
    }
);

/// One date warning (codes, dates and a day count; the engine's message is
/// never read).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateWarning {
    /// Which warning.
    pub code: DateWarningCode,
    /// The opening balances' date.
    pub opening_balance_date: Option<CalendarDate>,
    /// The first journal row that posts.
    pub first_journal_date: Option<CalendarDate>,
    /// Whole days with no rows between them (`HistoryGap`).
    pub days: Option<u64>,
}

/// What the batch's dates mean for the commit (`batch.dateCheck`, E2c).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DateCheck {
    /// The conversion date.
    pub conversion_date: Option<CalendarDate>,
    /// The history start.
    pub history_from: Option<CalendarDate>,
    /// Journal rows before the history start (never posted).
    pub history_skipped: u64,
    /// The earliest posted trial balance (the opening balances).
    pub opening_balance_date: Option<CalendarDate>,
    /// The first journal row that posts.
    pub first_journal_date: Option<CalendarDate>,
    /// Warnings (unknown codes from a newer engine are left out).
    pub warnings: Vec<DateWarning>,
}

impl ImportBatch {
    /// The tie-out was verified.
    #[must_use]
    pub fn is_verified(&self) -> bool {
        self.verified_at.is_some()
    }
}

/// A running job's progress.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Progress {
    /// The engine's step name (`accounts`, `journal`, …).
    pub step: String,
    /// Units done.
    pub done: u64,
    /// Units in all.
    pub total: u64,
}

/// Why a commit or undo job failed (nothing was posted, S1).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BatchFailure {
    /// The engine code.
    pub code: String,
    /// The file row, when one row caused it.
    pub row_number: Option<u64>,
    /// The file it was in.
    pub session: Option<ImportSessionId>,
    /// The column at fault (U-268: top level, also kept in `details`).
    pub field: Option<String>,
    /// The engine's `details` (unmapped account keys, the accounts a
    /// valuation is needed for). `Debug` names the keys only.
    pub details: crate::ConflictDetails,
}

impl BatchFailure {
    /// The failure as a refusal the screen explains, when it is one.
    #[must_use]
    pub fn refusal(&self) -> Option<ImportRefusal> {
        refusal_from(&self.code, Some(&self.details)).map(|r| match r {
            ImportRefusal::UnknownRef {
                field: None,
                refs,
                ref_count,
            } => ImportRefusal::UnknownRef {
                field: self.field.clone(),
                refs,
                ref_count,
            },
            r => r,
        })
    }
}

wire_enum!(
    /// Why a spreadsheet reference was not found (U-268 `refs[].reason`).
    RefReason {
        /// No name, account, item or tag by that name.
        Unknown => "unknown",
        /// More than one matches.
        Ambiguous => "ambiguous",
        /// The item has no account to post to.
        NoAccount => "no_account",
    }
);

/// One reference a spreadsheet row makes that the books lack (U-268,
/// `IMPORT_UNKNOWN_REF` `details.refs[]`): row, column and reason, never
/// the name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefProblem {
    /// The file row (header = 1).
    pub row_number: u64,
    /// The column: `name`, `line_account`, `line_item` or `line_tag`.
    pub field: Option<String>,
    /// Why (`None` for a reason this build doesn't know).
    pub reason: Option<RefReason>,
}

fn refs_from(details: Option<&crate::ConflictDetails>) -> (Vec<RefProblem>, u64) {
    let refs: Vec<RefProblem> = details
        .and_then(|d| d.raw("refs"))
        .and_then(Value::as_array)
        .map(|l| {
            l.iter()
                .filter_map(|r| {
                    Some(RefProblem {
                        row_number: r.get("rowNumber")?.as_u64()?,
                        field: r.get("field").and_then(Value::as_str).map(str::to_string),
                        reason: r
                            .get("reason")
                            .and_then(Value::as_str)
                            .and_then(RefReason::from_wire),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let count = details
        .and_then(|d| d.raw("refCount"))
        .and_then(Value::as_u64)
        .unwrap_or_else(|| u64::try_from(refs.len()).unwrap_or(u64::MAX));
    (refs, count)
}

fn row_problems_from(details: Option<&crate::ConflictDetails>, key: &str) -> Vec<RowProblem> {
    details
        .and_then(|d| d.raw(key))
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(row_problem_from).collect())
        .unwrap_or_default()
}

pub(crate) fn row_problem_from(v: &Value) -> Option<RowProblem> {
    Some(RowProblem {
        row_number: v.get("rowNumber").and_then(Value::as_u64).unwrap_or(0),
        field: v.get("field").and_then(Value::as_str).map(str::to_string),
        code: v.get("code")?.as_str()?.to_string(),
    })
}

/// A batch with its job state (`GET /imports/batches/{id}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchState {
    /// The batch.
    pub batch: ImportBatch,
    /// The running job's progress.
    pub progress: Option<Progress>,
    /// Why the last job failed.
    pub error: Option<BatchFailure>,
}

/// One page of History.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BatchPage {
    /// Newest first.
    pub batches: Vec<ImportBatch>,
    /// The next page's cursor.
    pub next_cursor: Option<String>,
}

/// What to send with a commit. The closing-date password and the staff
/// confirmation are for a batch dated in a closed period only (S2).
#[derive(Clone, Debug, Default)]
pub struct CommitRequest {
    /// The verified backup's file name (required).
    pub backup_ref: String,
    /// The closing-date password, when the owner gave it.
    pub closed_period: Option<ClosedPeriodAuthorization>,
    /// The owner confirmed posting into a closed period when no closing
    /// password is set (plan S2, `ledger.service.ts` staff confirm). Only
    /// sent when true; never set by default.
    pub confirm_closed_period: bool,
}

/// The engine accepted a commit or undo (202); poll the batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobAccepted {
    /// The batch.
    pub batch: ImportBatchId,
    /// A replay of an earlier send with the same key.
    pub replayed: bool,
}

/// One tie-out account row (one per books account; several source rows can
/// merge, e.g. two QBD A/R accounts). Owner data: `Debug` hides names and
/// amounts.
#[derive(Clone, PartialEq, Eq)]
pub struct TieOutAccount {
    /// The books' account (`None`: a source row that matched nothing, or
    /// "Retained Earnings (Prior Years)" with no tagged account).
    pub account: Option<AccountId>,
    /// The books account's number.
    pub number: Option<String>,
    /// The account's name (the books', else the source's).
    pub name: String,
    /// The first reference trial balance row (`None`: books only).
    pub source_row: Option<u64>,
    /// Every reference trial balance row merged here.
    pub source_rows: Vec<u64>,
    /// The source accounts that map here (number, else name; Q-P31k).
    pub source_keys: Vec<String>,
    /// Source balance (debit positive).
    pub source: Money,
    /// Books balance (debit positive).
    pub books: Money,
    /// Books minus source.
    pub diff: Money,
}

impl TieOutAccount {
    /// What the row is called on screen: the source account's key (the
    /// owner's own number or name) when there is one, else the books'.
    #[must_use]
    ///
    /// The account's name follows its number ("1000 · Farm Checking"), so
    /// the owner can tell the accounts apart (P31 UI review).
    pub fn label(&self) -> String {
        let key = self
            .source_keys
            .first()
            .map(String::as_str)
            .or(self.number.as_deref());
        match (key, self.name.as_str()) {
            (Some(k), "") => k.to_string(),
            (Some(k), name) if k == name || k.ends_with(name) => k.to_string(),
            (Some(k), name) => format!("{k} · {name}"),
            (None, name) => name.to_string(),
        }
    }
}

impl fmt::Debug for TieOutAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TieOutAccount")
            .field("account", &self.account)
            .field("source_row", &self.source_row)
            .field("zero", &self.diff.is_zero())
            .finish_non_exhaustive()
    }
}

/// One open item (A/R or A/P). Owner data.
#[derive(Clone, PartialEq, Eq)]
pub struct TieOutItem {
    /// The source file row (the item's key; `None`: an open item in the
    /// books no row made).
    pub source_row: Option<u64>,
    /// The file it came from.
    pub session: Option<ImportSessionId>,
    /// The document the import made (or found).
    pub transaction: Option<TransactionId>,
    /// Customer or vendor (the source's name, "Customer:Job" kept).
    pub name: String,
    /// The document number.
    pub number: Option<String>,
    /// The document type (`invoice`, `credit_memo`, `bill`, …).
    pub doc_type: Option<String>,
    /// The document date.
    pub date: Option<CalendarDate>,
    /// Source amount (signed: credits negative).
    pub source: Money,
    /// Books open balance as of the conversion date.
    pub books: Money,
    /// Books minus source.
    pub diff: Money,
}

impl fmt::Debug for TieOutItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TieOutItem")
            .field("source_row", &self.source_row)
            .field("transaction", &self.transaction)
            .field("zero", &self.diff.is_zero())
            .finish_non_exhaustive()
    }
}

/// One inventory item (the valuation file against the books' valuation).
/// Quantities are the engine's exact decimals (any number of places), never
/// computed here. Owner data.
#[derive(Clone, PartialEq, Eq)]
pub struct TieOutStock {
    /// The valuation file row (`None`: stock in the books no row made).
    pub source_row: Option<u64>,
    /// The item's name.
    pub name: String,
    /// The books' item.
    pub item: Option<ItemId>,
    /// Quantity on hand in the file.
    pub source_quantity: Decimal,
    /// Quantity on hand in the books.
    pub books_quantity: Decimal,
    /// Books minus file quantity.
    pub quantity_diff: Decimal,
    /// File value.
    pub source: Money,
    /// Books value (cents).
    pub books: Money,
    /// Books minus file value.
    pub diff: Money,
}

impl TieOutStock {
    /// Both the value and the quantity agree.
    #[must_use]
    pub fn ties(&self) -> bool {
        self.diff.is_zero() && self.quantity_diff.is_zero()
    }
}

impl fmt::Debug for TieOutStock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TieOutStock")
            .field("source_row", &self.source_row)
            .field("item", &self.item)
            .field("ties", &self.ties())
            .finish_non_exhaustive()
    }
}

wire_enum!(
    /// A balance check of the tie-out.
    BalanceKey {
        /// "Receivables before conversion" must be 0.00.
        HistoryAr => "history_ar",
        /// "Payables before conversion" must be 0.00.
        HistoryAp => "history_ap",
        /// "Inventory before conversion" must be 0.00.
        HistoryInventory => "history_inventory",
        /// The valuation equals the Inventory Asset accounts.
        ValuationVsAsset => "valuation_vs_asset",
    }
);

/// One balance check row.
#[derive(Clone, PartialEq, Eq)]
pub struct BalanceRow {
    /// Which check.
    pub key: BalanceKey,
    /// The account checked (a history account; `None` for the valuation).
    pub account: Option<AccountId>,
    /// Source figure (0.00 for a history account).
    pub source: Money,
    /// Books figure.
    pub books: Money,
    /// The difference (0.00 when it holds).
    pub diff: Money,
}

impl fmt::Debug for BalanceRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BalanceRow")
            .field("key", &self.key)
            .field("zero", &self.diff.is_zero())
            .finish_non_exhaustive()
    }
}

/// One side's totals (the A/R, A/P or inventory file against the books).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TieOutTotal {
    /// The file's total.
    pub source: Money,
    /// The books' total.
    pub books: Money,
    /// Books minus source.
    pub diff: Money,
}

impl fmt::Debug for TieOutTotal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TieOutTotal")
            .field("zero", &self.diff.is_zero())
            .finish_non_exhaustive()
    }
}

/// The trial balance's column totals (the engine sends no difference total:
/// the screen shows these as given and never subtracts them).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TrialBalanceTotals {
    /// Source debits.
    pub source_debit: Money,
    /// Source credits.
    pub source_credit: Money,
    /// Books debits.
    pub books_debit: Money,
    /// Books credits.
    pub books_credit: Money,
}

impl fmt::Debug for TrialBalanceTotals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TrialBalanceTotals(..)")
    }
}

/// Tie-out totals: each `None` when the batch has no such file.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct TieOutTotals {
    /// The trial balance's column totals.
    pub accounts: Option<TrialBalanceTotals>,
    /// The A/R aging (open invoices) against the books.
    pub ar: Option<TieOutTotal>,
    /// The A/P aging (unpaid bills) against the books.
    pub ap: Option<TieOutTotal>,
    /// The valuation file against the books.
    pub inventory: Option<TieOutTotal>,
}

impl fmt::Debug for TieOutTotals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TieOutTotals(..)")
    }
}

wire_enum!(
    /// What the books side of the tie-out is.
    TieOutBooks {
        /// The whole company (it was empty at the commit).
        Company => "company",
        /// Only the batch's own transactions (Q-P31h).
        Batch => "batch",
    }
);

/// The reference trial balance compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReferenceTrialBalance {
    /// Its file.
    pub session: Option<ImportSessionId>,
    /// Its report date.
    pub report_date: Option<CalendarDate>,
}

/// The engine's tie-out of a batch (E3). The screen shows ✓ only when
/// `all_zero` is true; it never re-derives a balance.
#[derive(Clone, PartialEq, Eq)]
pub struct TieOut {
    /// The batch.
    pub batch: Option<ImportBatchId>,
    /// The date compared (the conversion date).
    pub as_of: CalendarDate,
    /// When the engine computed it (engine text).
    pub computed_at: Option<String>,
    /// Whole company, or the batch's own lines.
    pub books: Option<TieOutBooks>,
    /// The reference trial balance.
    pub reference: Option<ReferenceTrialBalance>,
    /// Accounts.
    pub accounts: Vec<TieOutAccount>,
    /// Open invoices.
    pub ar_items: Vec<TieOutItem>,
    /// Unpaid bills.
    pub ap_items: Vec<TieOutItem>,
    /// Inventory items.
    pub inventory_items: Vec<TieOutStock>,
    /// Balance checks.
    pub balance_rows: Vec<BalanceRow>,
    /// Totals.
    pub totals: TieOutTotals,
    /// How many differences the engine counted.
    pub difference_count: u64,
    /// The engine says every difference is exactly zero.
    pub all_zero: bool,
    /// When it was verified (engine text; the stored tie-out after verify).
    pub verified_at: Option<String>,
}

impl fmt::Debug for TieOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Totals are amounts: left out.
        f.debug_struct("TieOut")
            .field("batch", &self.batch)
            .field("as_of", &self.as_of)
            .field("books", &self.books)
            .field("accounts", &self.accounts.len())
            .field("ar_items", &self.ar_items.len())
            .field("ap_items", &self.ap_items.len())
            .field("inventory_items", &self.inventory_items.len())
            .field("balance_rows", &self.balance_rows)
            .field("difference_count", &self.difference_count)
            .field("all_zero", &self.all_zero)
            .finish_non_exhaustive()
    }
}

/// A verified batch (`POST …/verify` 200): the batch and the tie-out kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    /// The batch (`verified_at` set).
    pub batch: ImportBatch,
    /// The tie-out it was verified on.
    pub tieout: TieOut,
}

wire_enum!(
    /// Which part of the tie-out a first difference is in.
    DifferenceSection {
        /// A trial balance account.
        Accounts => "accounts",
        /// An open invoice.
        ArItems => "arItems",
        /// An unpaid bill.
        ApItems => "apItems",
        /// An inventory item.
        InventoryItems => "inventoryItems",
        /// A balance check.
        BalanceRows => "balanceRows",
        /// A side's totals.
        Totals => "totals",
    }
);

/// One of the first differences of a refused verify: ids and rows only
/// (the engine never names accounts or contacts here; amounts are read
/// from the tie-out, not from this list).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TieOutDifference {
    /// Where it is (`None`: a section from a newer engine).
    pub section: Option<DifferenceSection>,
    /// A balance check's key, or `ar` / `ap` / `inventory` for totals.
    pub key: Option<String>,
    /// The source file row.
    pub source_row: Option<u64>,
    /// The account.
    pub account: Option<AccountId>,
    /// The document.
    pub transaction: Option<TransactionId>,
    /// The item.
    pub item: Option<ItemId>,
}

/// One transaction a batch created (`GET …/transactions`). Owner data.
#[derive(Clone, PartialEq, Eq)]
pub struct BatchTransaction {
    /// Its id.
    pub id: TransactionId,
    /// The engine's type (`invoice`, `bill`, `journal_entry`, …).
    pub txn_type: String,
    /// Its date.
    pub date: Option<CalendarDate>,
    /// Its number.
    pub number: Option<String>,
    /// The vendor's invoice number (bills).
    pub vendor_invoice_number: Option<String>,
    /// The engine's status.
    pub status: Option<String>,
    /// The customer or vendor.
    pub contact: Option<ContactId>,
    /// Their name.
    pub contact_name: Option<String>,
    /// The document total (a journal entry's debits), exact.
    pub total: Money,
}

impl fmt::Debug for BatchTransaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchTransaction")
            .field("id", &self.id)
            .field("txn_type", &self.txn_type)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

/// One page of a batch's transactions (order: date, id).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BatchTransactionPage {
    /// The page.
    pub transactions: Vec<BatchTransaction>,
    /// The next page's cursor.
    pub next_cursor: Option<String>,
    /// How many match in all.
    pub total: u64,
}

/// Something that keeps a batch from being undone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UndoBlocker {
    /// What it is (`payment`, `reconciliation`, `deposit`, …).
    pub kind: String,
    /// Its id.
    pub id: Option<String>,
    /// The engine's reason code.
    pub reason: Option<String>,
}

/// An import refusal the screen explains (codes only; engine messages can
/// name people, U-270).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportRefusal {
    /// 409 `IMPORT_ALREADY_COMMITTED`: this file was imported before.
    AlreadyCommitted {
        /// The earlier batch.
        batch: Option<ImportBatchId>,
        /// The earlier file.
        session: Option<ImportSessionId>,
        /// When it was committed (engine text).
        committed_at: Option<String>,
    },
    /// 409 `IMPORT_PERIOD_OVERLAP`: journal dates overlap an earlier import
    /// or another file of this batch.
    PeriodOverlap {
        /// The earlier batch (none for an overlap inside this batch).
        batch: Option<ImportBatchId>,
        /// First overlapping day.
        from: Option<CalendarDate>,
        /// Last overlapping day.
        to: Option<CalendarDate>,
    },
    /// 409 `IMPORT_BUSY`: another import job is running.
    Busy,
    /// 409 `IMPORT_UNDO_BLOCKED`: something now depends on what the batch
    /// created; restore the backup instead, or remove the blockers.
    UndoBlocked {
        /// What blocks it.
        blockers: Vec<UndoBlocker>,
    },
    /// 409 `TIEOUT_NOT_ZERO`: verify refused.
    TieOutNotZero {
        /// How many differences the engine counted (all of them).
        difference_count: u64,
        /// The first ones (at most 20; ids and rows only).
        first: Vec<TieOutDifference>,
    },
    /// 422 `TIEOUT_REFERENCE_REQUIRED`: the batch has no reference trial
    /// balance to compare with (every source needs one, S9).
    TieOutReferenceRequired {
        /// The date it should be dated.
        as_of: Option<CalendarDate>,
    },
    /// 422 `TIEOUT_NO_DATE`: no conversion date and no dated reference
    /// trial balance.
    TieOutNoDate,
    /// 422 `TIEOUT_REFERENCE_AMBIGUOUS`: more than one reference trial
    /// balance and no conversion date to pick one.
    TieOutReferenceAmbiguous,
    /// 409 `IMPORT_COMPANY_NOT_EMPTY`: a QBD conversion needs an empty
    /// company.
    CompanyNotEmpty,
    /// 409 `IMPORT_SESSION_IN_BATCH`: the file is already in a batch.
    SessionInBatch,
    /// 409 `IMPORT_FISCAL_YEAR_LOCKED`: the fiscal year can be set only in
    /// an empty company.
    FiscalYearLocked,
    /// 403 `IMPORT_ADMIN_REQUIRED`: commit, undo and verify are the
    /// company Admin's.
    AdminRequired,
    /// 422 `IMPORT_DATE_MISMATCH`: files not dated the conversion date
    /// (every one listed).
    DateMismatch {
        /// The conversion date.
        conversion_date: Option<CalendarDate>,
        /// The files.
        files: Vec<DateMismatchFile>,
    },
    /// 422 `IMPORT_JOURNAL_AFTER_CONVERSION`: journal rows dated after the
    /// conversion date.
    JournalAfterConversion {
        /// The conversion date.
        conversion_date: Option<CalendarDate>,
        /// How many rows.
        row_count: u64,
        /// The first rows (file, row number).
        rows: Vec<(Option<ImportSessionId>, u64)>,
    },
    /// 400 `IMPORT_BAD_DATE`: the history start is after the conversion
    /// date, or a report has no date the engine can use.
    BadDate,
    /// 422 `IMPORT_UNMAPPED_ACCOUNT` (a failed commit): accounts the
    /// files name that match nothing and have no account picked.
    UnmappedAccount {
        /// Their source keys (owner data).
        keys: SourceKeys,
        /// How many in all.
        count: u64,
    },
    /// 400 `IMPORT_ACCOUNT_MAP_INVALID`: a picked account is not in this
    /// company (any more).
    AccountMapInvalid {
        /// The source keys mapped to it.
        keys: SourceKeys,
    },
    /// 409 `IMPORT_INVENTORY_OFF`: the files bring in inventory while the
    /// company's inventory is off.
    InventoryOff,
    /// 409 `INVENTORY_PERIOD_CLOSED`: the opening stock is dated on or
    /// before the closing date. No password lifts it.
    InventoryPeriodClosed {
        /// The closing date.
        closing_date: Option<CalendarDate>,
    },
    /// 422 `IMPORT_VALUATION_REQUIRED` (a failed commit): the trial balance
    /// shows inventory, so the Inventory Valuation Summary is needed.
    ValuationRequired {
        /// The inventory accounts' source keys (owner data).
        keys: SourceKeys,
    },
    /// 422 `IMPORT_AMOUNT_INVALID`: amounts the engine will not read
    /// (exponents, more than 4 decimals, `1.234,56`).
    AmountInvalid {
        /// The file rows, in order, without repeats.
        rows: Vec<u64>,
    },
    /// `IMPORT_UNBALANCED`: a spreadsheet document does not balance.
    Unbalanced,
    /// `IMPORT_UNKNOWN_REF`: a spreadsheet row names an unknown account,
    /// item, name or tag (U-268: at upload, or as a failed batch when a
    /// reference went away in between).
    UnknownRef {
        /// The column of the first one.
        field: Option<String>,
        /// Each one (at most 500; row, column and reason only).
        refs: Vec<RefProblem>,
        /// How many in all.
        ref_count: u64,
    },
    /// `IMPORT_MIXED_GROUP`: one spreadsheet group mixes document types.
    MixedGroup,
    /// 409 `IMPORT_HAS_ERRORS`: the upload's row problems block the
    /// commit (U-268); show them and choose a fixed file.
    HasErrors {
        /// The upload.
        session: Option<ImportSessionId>,
        /// Its problems (the first 25).
        errors: Vec<RowProblem>,
    },
    /// 409 `IMPORT_SESSION_ACTIVE`: the same file is already uploaded and
    /// not finished; discard that upload first.
    SessionActive {
        /// The earlier upload.
        session: Option<ImportSessionId>,
        /// Where it stands.
        status: Option<SessionStatus>,
    },
    /// 422 `IMPORT_DOCUMENT_TOTAL` (a failed batch): an invoice or bill
    /// total is not more than zero, or posted other than its lines' sum.
    DocumentTotal,
    /// `JE_AR_AP_NAME_REQUIRED` (a failed batch): a journal entry line on
    /// accounts receivable or payable needs a customer or vendor.
    ArApNameRequired,
    /// `IMPORT_INTERRUPTED` (a batch error): the engine stopped while the
    /// job ran (a restart settled it); nothing was posted.
    Interrupted,
    /// 409 `IMPORT_BATCH_UNDONE`: the batch was undone; its files go in a
    /// new import.
    BatchUndone,
    /// 409 `IMPORT_BATCH_NOT_COMMITTED`: tie-out, verify or undo of a batch
    /// that was never imported.
    BatchNotCommitted,
    /// 409/422 `IMPORT_BATCH_EMPTY`: the batch holds no files.
    BatchEmpty,
    /// 409/422 `IMPORT_SOURCE_MISMATCH`: a file was read for another
    /// program than the batch's.
    SourceMismatch,
}

wire_enum!(
    /// Why a file's date does not fit the conversion date.
    DateMismatchReason {
        /// Dated another day.
        NotConversionDate => "not_conversion_date",
        /// The report has no date.
        NoDate => "no_date",
        /// A posted trial balance dated after the conversion date.
        AfterConversion => "after_conversion",
    }
);

/// One file of an `IMPORT_DATE_MISMATCH` (ids, kinds and dates only: the
/// engine's file name is never read).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateMismatchFile {
    /// The file.
    pub session: Option<ImportSessionId>,
    /// What it holds.
    pub kind: Option<ImportKind>,
    /// Its report date.
    pub report_date: Option<CalendarDate>,
    /// Why.
    pub reason: Option<DateMismatchReason>,
}

/// String entries of the list under `key` (`unmappedAccountKeys`), or the
/// `field` of each object entry (`accounts[].sourceKey`).
fn details_keys(d: Option<&crate::ConflictDetails>, key: &str, field: &str) -> SourceKeys {
    SourceKeys(
        d.and_then(|d| d.raw(key))
            .and_then(Value::as_array)
            .map(|l| {
                l.iter()
                    .filter_map(|v| v.as_str().or_else(|| v.get(field)?.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    )
}

fn details_id<T: std::str::FromStr>(d: Option<&crate::ConflictDetails>, key: &str) -> Option<T> {
    d.and_then(|d| d.id(key))
}

/// The refusal for an engine code, reading `details` when there are any.
fn refusal_from(code: &str, details: Option<&crate::ConflictDetails>) -> Option<ImportRefusal> {
    let text = |k: &str| details.and_then(|d| d.text(k)).map(str::to_string);
    let day = |k: &str| text(k).and_then(|d| CalendarDate::parse_wire(d.get(..10)?).ok());
    Some(match code {
        "IMPORT_ALREADY_COMMITTED" => ImportRefusal::AlreadyCommitted {
            batch: details_id(details, "batchId"),
            session: details_id(details, "sessionId"),
            committed_at: text("committedAt"),
        },
        "IMPORT_PERIOD_OVERLAP" => ImportRefusal::PeriodOverlap {
            batch: details_id(details, "batchId"),
            from: day("from"),
            to: day("to"),
        },
        "IMPORT_BUSY" => ImportRefusal::Busy,
        "IMPORT_UNDO_BLOCKED" => ImportRefusal::UndoBlocked {
            blockers: details
                .and_then(|d| d.raw("blockers"))
                .and_then(Value::as_array)
                .map(|l| {
                    l.iter()
                        .map(|b| UndoBlocker {
                            kind: b["kind"].as_str().unwrap_or_default().to_string(),
                            id: b["id"].as_str().map(str::to_string),
                            reason: b["reason"].as_str().map(str::to_string),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        "TIEOUT_NOT_ZERO" => {
            let first: Vec<TieOutDifference> = details
                .and_then(|d| d.raw("firstDifferences"))
                .and_then(Value::as_array)
                .map(|l| l.iter().map(difference_from).collect())
                .unwrap_or_default();
            ImportRefusal::TieOutNotZero {
                difference_count: details
                    .and_then(|d| d.raw("differenceCount"))
                    .and_then(Value::as_u64)
                    .unwrap_or_else(|| u64::try_from(first.len()).unwrap_or(u64::MAX)),
                first,
            }
        }
        "TIEOUT_REFERENCE_REQUIRED" => {
            ImportRefusal::TieOutReferenceRequired { as_of: day("asOf") }
        }
        "TIEOUT_NO_DATE" => ImportRefusal::TieOutNoDate,
        "TIEOUT_REFERENCE_AMBIGUOUS" => ImportRefusal::TieOutReferenceAmbiguous,
        "IMPORT_COMPANY_NOT_EMPTY" => ImportRefusal::CompanyNotEmpty,
        "IMPORT_SESSION_IN_BATCH" => ImportRefusal::SessionInBatch,
        "IMPORT_FISCAL_YEAR_LOCKED" => ImportRefusal::FiscalYearLocked,
        "IMPORT_ADMIN_REQUIRED" => ImportRefusal::AdminRequired,
        "IMPORT_AMOUNT_INVALID" => {
            let mut rows: Vec<u64> = details
                .and_then(|d| d.raw("rows"))
                .and_then(Value::as_array)
                .map(|l| {
                    l.iter()
                        .filter_map(|r| r.as_u64().or_else(|| r.get("rowNumber")?.as_u64()))
                        .collect()
                })
                .unwrap_or_default();
            rows.sort_unstable();
            rows.dedup();
            ImportRefusal::AmountInvalid { rows }
        }
        "IMPORT_UNBALANCED" => ImportRefusal::Unbalanced,
        "IMPORT_UNKNOWN_REF" => {
            let (refs, ref_count) = refs_from(details);
            ImportRefusal::UnknownRef {
                field: text("field"),
                refs,
                ref_count,
            }
        }
        "IMPORT_MIXED_GROUP" => ImportRefusal::MixedGroup,
        "IMPORT_HAS_ERRORS" => ImportRefusal::HasErrors {
            session: details_id(details, "sessionId"),
            errors: row_problems_from(details, "errors"),
        },
        "IMPORT_SESSION_ACTIVE" => ImportRefusal::SessionActive {
            session: details_id(details, "existingSessionId"),
            status: text("status").and_then(|s| SessionStatus::from_wire(&s)),
        },
        "IMPORT_DOCUMENT_TOTAL" => ImportRefusal::DocumentTotal,
        "JE_AR_AP_NAME_REQUIRED" => ImportRefusal::ArApNameRequired,
        "IMPORT_INTERRUPTED" => ImportRefusal::Interrupted,
        "IMPORT_BATCH_UNDONE" => ImportRefusal::BatchUndone,
        "IMPORT_BATCH_NOT_COMMITTED" => ImportRefusal::BatchNotCommitted,
        "IMPORT_BATCH_EMPTY" => ImportRefusal::BatchEmpty,
        "IMPORT_SOURCE_MISMATCH" => ImportRefusal::SourceMismatch,
        _ => return mapping_or_date_refusal(code, details),
    })
}

/// The E2b and E2c codes (mapping, inventory, dates).
fn mapping_or_date_refusal(
    code: &str,
    details: Option<&crate::ConflictDetails>,
) -> Option<ImportRefusal> {
    let day = |k: &str| {
        details
            .and_then(|d| d.text(k))
            .and_then(|d| CalendarDate::parse_wire(d.get(..10)?).ok())
    };
    let list = |k: &str| -> Vec<Value> {
        details
            .and_then(|d| d.raw(k))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    Some(match code {
        "IMPORT_DATE_MISMATCH" => ImportRefusal::DateMismatch {
            conversion_date: day("conversionDate"),
            files: list("files")
                .iter()
                .map(|f| DateMismatchFile {
                    session: opt_id(&f["sessionId"]),
                    kind: f["kind"].as_str().and_then(ImportKind::from_wire),
                    report_date: date(&f["reportDate"]),
                    reason: f["reason"].as_str().and_then(DateMismatchReason::from_wire),
                })
                .collect(),
        },
        "IMPORT_JOURNAL_AFTER_CONVERSION" => ImportRefusal::JournalAfterConversion {
            conversion_date: day("conversionDate"),
            row_count: details
                .and_then(|d| d.raw("rowCount"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
            rows: list("rows")
                .iter()
                .filter_map(|r| Some((opt_id(&r["sessionId"]), r["rowNumber"].as_u64()?)))
                .collect(),
        },
        "IMPORT_BAD_DATE" => ImportRefusal::BadDate,
        "IMPORT_UNMAPPED_ACCOUNT" => {
            let keys = details_keys(details, "unmappedAccountKeys", "sourceKey");
            let count = details
                .and_then(|d| d.raw("unmappedCount"))
                .and_then(Value::as_u64)
                .unwrap_or_else(|| u64::try_from(keys.0.len()).unwrap_or(u64::MAX));
            ImportRefusal::UnmappedAccount { keys, count }
        }
        "IMPORT_ACCOUNT_MAP_INVALID" => ImportRefusal::AccountMapInvalid {
            keys: details_keys(details, "keys", "sourceKey"),
        },
        "IMPORT_INVENTORY_OFF" => ImportRefusal::InventoryOff,
        "INVENTORY_PERIOD_CLOSED" => ImportRefusal::InventoryPeriodClosed {
            closing_date: day("closingDate"),
        },
        "IMPORT_VALUATION_REQUIRED" => ImportRefusal::ValuationRequired {
            keys: details_keys(details, "accounts", "sourceKey"),
        },
        _ => return None,
    })
}

/// Classifies `e` (`None` for anything else: closed periods, passwords,
/// hidden accounts, network, plain validation).
#[must_use]
pub fn import_refusal(e: &ApiError) -> Option<ImportRefusal> {
    let code = error_code(e)?;
    let details = match e {
        ApiError::Conflict { details, .. } => Some(details),
        ApiError::Validation(v) => Some(v.details()),
        _ => None,
    };
    refusal_from(code, details)
}

// ── Wire readers ──

fn money(v: &Value) -> Result<Money, ApiError> {
    // Exact decimal strings only: a JSON number may have been rounded.
    let s = v.as_str().ok_or(ApiError::InexactMoney { what: WHAT })?;
    Money::parse_wire(s).map_err(|_| decode_err())
}

fn opt_money(v: &Value) -> Result<Option<Money>, ApiError> {
    if v.is_null() {
        Ok(None)
    } else {
        money(v).map(Some)
    }
}

fn date(v: &Value) -> Option<CalendarDate> {
    let s = v.as_str()?;
    CalendarDate::parse_wire(s.get(..10).unwrap_or(s)).ok()
}

fn opt_id<T: std::str::FromStr>(v: &Value) -> Option<T> {
    v.as_str().and_then(|s| s.parse().ok())
}

fn count(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

fn text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn parse_json(body: &[u8]) -> Result<Value, ApiError> {
    serde_json::from_slice(body).map_err(|_| decode_err())
}

fn wire<T>(v: &Value, f: fn(&str) -> Option<T>) -> Result<T, ApiError> {
    v.as_str().and_then(f).ok_or_else(decode_err)
}

fn session_from(s: &Value) -> Result<ImportSession, ApiError> {
    Ok(ImportSession {
        id: ImportSessionId::parse(s["id"].as_str().ok_or_else(decode_err)?)?,
        kind: wire(&s["kind"], ImportKind::from_wire)?,
        source: wire(&s["sourceSystem"], SourceSystem::from_wire)?,
        role: s["role"].as_str().and_then(FileRole::from_wire),
        status: wire(&s["status"], SessionStatus::from_wire)?,
        row_count: count(&s["rowCount"]),
        error_count: count(&s["errorCount"]),
        report_date: date(&s["reportDate"]),
        batch: opt_id(&s["batchId"]),
    })
}

/// Reads `{session, preview, validationErrors}` (upload and session read).
pub(crate) fn upload_from(body: &[u8]) -> Result<UploadOutcome, ApiError> {
    let v = parse_json(body)?;
    let session = session_from(&v["session"])?;
    let p = &v["preview"];
    let preview = FilePreview {
        total_rows: count(&p["totalRows"]),
        error_count: count(&p["errorCount"]),
        entry_count: p["jeGroupCount"].as_u64(),
        report_date: date(&p["reportDate"]),
        total_debit: opt_money(&p["totalDebit"])?,
        total_credit: opt_money(&p["totalCredit"])?,
        account_matches: account_matches_from(&p["accountMatches"]),
    };
    let problems = v["validationErrors"]
        .as_array()
        .map(|l| {
            l.iter()
                .map(|e| RowProblem {
                    row_number: count(&e["rowNumber"]),
                    field: text(&e["field"]),
                    // `message` is never read: it can quote cells.
                    code: e["code"].as_str().unwrap_or_default().to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(UploadOutcome {
        session,
        preview,
        problems,
    })
}

/// `preview.accountMatches[]`; rows of an unknown kind are skipped (and
/// so never counted as matched).
fn account_matches_from(v: &Value) -> Vec<AccountMatch> {
    v.as_array()
        .map(|l| {
            l.iter()
                .filter_map(|m| {
                    let number = text(&m["number"]);
                    let name = text(&m["name"]).unwrap_or_default();
                    let source_key = text(&m["sourceKey"])
                        .or_else(|| number.clone())
                        .or_else(|| (!name.is_empty()).then(|| name.clone()))?;
                    Some(AccountMatch {
                        source_key,
                        number,
                        name,
                        status: m["match"].as_str().and_then(MatchStatus::from_wire)?,
                        account: opt_id(&m["accountId"]),
                        matched_by: m["matchedBy"].as_str().and_then(MatchedBy::from_wire),
                        rows: count(&m["rows"]),
                        system_role: m["systemRole"].as_str().and_then(SystemRole::from_wire),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn spot_check_from(v: &Value) -> Vec<SpotCheckItem> {
    let list = v
        .as_array()
        .or_else(|| v["items"].as_array())
        .cloned()
        .unwrap_or_default();
    list.iter()
        .filter_map(|i| {
            Some(SpotCheckItem {
                key: i["key"].as_str()?.to_string(),
                label: i["label"].as_str().unwrap_or_default().to_string(),
                done: i["done"].as_bool().unwrap_or(false),
                note: i["note"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

fn summary_from(r: &Value) -> Result<CommitSummary, ApiError> {
    let s = &r["itemsSkipped"];
    Ok(CommitSummary {
        created: count(&r["created"]),
        skipped: count(&r["skipped"]),
        merged_to_both: count(&r["mergedToBoth"]),
        jobs_folded: count(&r["jobsFolded"]),
        inactive: count(&r["inactive"]),
        items_skipped: s.is_object().then(|| ItemsSkipped {
            sales_tax: count(&s["sales_tax"]),
            group: count(&s["group"]),
            payment: count(&s["payment"]),
            exists: count(&s["exists"]),
            no_account: count(&s["no_account"]),
        }),
        classes_mapped: count(&r["classesMapped"]),
        items_valued: count(&r["itemsValued"]),
        valuation_total: opt_money(&r["valuationTotal"])?,
        skipped_before_history: r["skippedBeforeHistory"].as_u64(),
    })
}

/// `batch.sessions[]` (E1 as built), with each file's `commitResult`.
fn files_from(v: &Value) -> Result<Vec<BatchFile>, ApiError> {
    let Some(list) = v.as_array() else {
        return Ok(Vec::new());
    };
    list.iter()
        .filter_map(|s| {
            let id = opt_id::<ImportSessionId>(&s["id"])?;
            let r = &s["commitResult"];
            Some(
                (if r.is_object() {
                    summary_from(r).map(Some)
                } else {
                    Ok(None)
                })
                .map(|result| BatchFile {
                    id,
                    kind: s["kind"].as_str().and_then(ImportKind::from_wire),
                    role: s["role"].as_str().and_then(FileRole::from_wire),
                    status: s["status"].as_str().and_then(SessionStatus::from_wire),
                    report_date: date(&s["reportDate"]),
                    result,
                }),
            )
        })
        .collect()
}

/// `batch.dateCheck` (create, GET), else the committed `result.dateCheck`.
fn date_check_from(b: &Value) -> Option<DateCheck> {
    let d = if b["dateCheck"].is_object() {
        &b["dateCheck"]
    } else {
        &b["result"]["dateCheck"]
    };
    d.is_object().then(|| DateCheck {
        conversion_date: date(&d["conversionDate"]),
        history_from: date(&d["historyFrom"]),
        history_skipped: count(&d["historySkipped"]),
        opening_balance_date: date(&d["openingBalanceDate"]),
        first_journal_date: date(&d["firstJournalDate"]),
        warnings: d["warnings"]
            .as_array()
            .map(|l| {
                l.iter()
                    .filter_map(|w| {
                        // `message` is never read.
                        Some(DateWarning {
                            code: w["code"].as_str().and_then(DateWarningCode::from_wire)?,
                            opening_balance_date: date(&w["openingBalanceDate"]),
                            first_journal_date: date(&w["firstJournalDate"]),
                            days: w["days"].as_u64(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn batch_from(b: &Value) -> Result<ImportBatch, ApiError> {
    let files = files_from(&b["sessions"])?;
    let sessions = if files.is_empty() {
        b["sessionIds"]
            .as_array()
            .map(|l| l.iter().filter_map(opt_id).collect())
            .unwrap_or_default()
    } else {
        files.iter().map(|f| f.id).collect()
    };
    Ok(ImportBatch {
        id: ImportBatchId::parse(b["id"].as_str().ok_or_else(decode_err)?)?,
        source: wire(&b["sourceSystem"], SourceSystem::from_wire)?,
        kind: wire(&b["kind"], BatchKind::from_wire)?,
        status: wire(&b["status"], BatchStatus::from_wire)?,
        sessions,
        files,
        date_check: date_check_from(b),
        error: failure_from(&b["error"]),
        backup_ref: text(&b["backupRef"]),
        company_was_empty: b["companyWasEmpty"].as_bool(),
        conversion_date: date(&b["conversionDate"]),
        history_from: date(&b["historyFrom"]),
        verified_at: text(&b["verifiedAt"]),
        verified_by: text(&b["verifiedBy"]),
        spot_check: spot_check_from(&b["spotCheck"]),
        created_at: text(&b["createdAt"]),
        committed_at: text(&b["committedAt"]),
    })
}

/// Reads `{batch}` (create, verify) or a bare batch.
pub(crate) fn one_batch_from(body: &[u8]) -> Result<ImportBatch, ApiError> {
    let v = parse_json(body)?;
    if v["batch"].is_object() {
        batch_from(&v["batch"])
    } else {
        batch_from(&v)
    }
}

/// A job's `error` object (`None` when absent or null).
pub(crate) fn failure_from(e: &Value) -> Option<BatchFailure> {
    e.is_object().then(|| BatchFailure {
        code: e["code"].as_str().unwrap_or_default().to_string(),
        row_number: e["rowNumber"].as_u64(),
        session: opt_id(&e["sessionId"]),
        field: e["field"]
            .as_str()
            .or_else(|| e["details"]["field"].as_str())
            .map(str::to_string),
        // `message` is never read: it can name accounts and people.
        details: crate::ConflictDetails::new(e["details"].clone()),
    })
}

/// Reads `{batch, progress, error?}`.
pub(crate) fn state_from(body: &[u8]) -> Result<BatchState, ApiError> {
    let v = parse_json(body)?;
    let batch = batch_from(&v["batch"])?;
    let p = &v["progress"];
    let progress = p.is_object().then(|| Progress {
        step: p["step"].as_str().unwrap_or_default().to_string(),
        done: count(&p["done"]),
        total: count(&p["total"]),
    });
    let error = failure_from(&v["error"]).or_else(|| batch.error.clone());
    Ok(BatchState {
        batch,
        progress,
        error,
    })
}

/// Reads a History page (`data` or `batches`, and `nextCursor`).
pub(crate) fn page_from(body: &[u8]) -> Result<BatchPage, ApiError> {
    let v = parse_json(body)?;
    let list = v["data"]
        .as_array()
        .or_else(|| v["batches"].as_array())
        .ok_or_else(decode_err)?;
    Ok(BatchPage {
        batches: list.iter().map(batch_from).collect::<Result<_, _>>()?,
        next_cursor: text(&v["nextCursor"]),
    })
}

fn accepted_from(body: &[u8], replayed: bool) -> Result<JobAccepted, ApiError> {
    let v = parse_json(body)?;
    let id = v["batchId"]
        .as_str()
        .or_else(|| v["batch"]["id"].as_str())
        .ok_or_else(decode_err)?;
    Ok(JobAccepted {
        batch: ImportBatchId::parse(id)?,
        replayed,
    })
}

/// An exact decimal string that is not money (a quantity, any places).
fn quantity(v: &Value) -> Result<Decimal, ApiError> {
    let s = v.as_str().ok_or(ApiError::InexactMoney { what: WHAT })?;
    Decimal::from_str_exact(s).map_err(|_| decode_err())
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|l| {
            l.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn item_from(i: &Value) -> Result<TieOutItem, ApiError> {
    Ok(TieOutItem {
        source_row: i["sourceRow"].as_u64(),
        session: opt_id(&i["sessionId"]),
        transaction: opt_id(&i["transactionId"]),
        name: i["contactName"].as_str().unwrap_or_default().to_string(),
        number: text(&i["number"]),
        doc_type: text(&i["docType"]),
        date: date(&i["date"]),
        source: money(&i["sourceExact"])?,
        books: money(&i["booksExact"])?,
        diff: money(&i["diffExact"])?,
    })
}

fn items_from(v: &Value) -> Result<Vec<TieOutItem>, ApiError> {
    v.as_array()
        .map_or_else(|| Ok(Vec::new()), |l| l.iter().map(item_from).collect())
}

fn stock_from(i: &Value) -> Result<TieOutStock, ApiError> {
    Ok(TieOutStock {
        source_row: i["sourceRow"].as_u64(),
        name: i["item"].as_str().unwrap_or_default().to_string(),
        item: opt_id(&i["itemId"]),
        source_quantity: quantity(&i["sourceQuantity"])?,
        books_quantity: quantity(&i["booksQuantity"])?,
        quantity_diff: quantity(&i["quantityDiff"])?,
        source: money(&i["sourceExact"])?,
        books: money(&i["booksExact"])?,
        diff: money(&i["diffExact"])?,
    })
}

fn total_from(t: &Value) -> Result<Option<TieOutTotal>, ApiError> {
    if !t.is_object() {
        return Ok(None);
    }
    Ok(Some(TieOutTotal {
        source: money(&t["sourceExact"])?,
        books: money(&t["booksExact"])?,
        diff: money(&t["diffExact"])?,
    }))
}

fn totals_from(t: &Value) -> Result<TieOutTotals, ApiError> {
    let a = &t["accounts"];
    Ok(TieOutTotals {
        accounts: if a.is_object() {
            Some(TrialBalanceTotals {
                source_debit: money(&a["sourceDebitExact"])?,
                source_credit: money(&a["sourceCreditExact"])?,
                books_debit: money(&a["booksDebitExact"])?,
                books_credit: money(&a["booksCreditExact"])?,
            })
        } else {
            None
        },
        ar: total_from(&t["ar"])?,
        ap: total_from(&t["ap"])?,
        inventory: total_from(&t["inventory"])?,
    })
}

/// One of `firstDifferences` (ids only; its amount is not read).
fn difference_from(d: &Value) -> TieOutDifference {
    TieOutDifference {
        section: d["section"].as_str().and_then(DifferenceSection::from_wire),
        key: text(&d["key"]),
        source_row: d["sourceRow"].as_u64(),
        account: opt_id(&d["accountId"]),
        transaction: opt_id(&d["transactionId"]),
        item: opt_id(&d["itemId"]),
    }
}

/// Reads a tie-out object (E3 as built).
fn tieout_value(v: &Value) -> Result<TieOut, ApiError> {
    let accounts = v["accounts"]
        .as_array()
        .ok_or_else(decode_err)?
        .iter()
        .map(|a| {
            Ok(TieOutAccount {
                account: opt_id(&a["accountId"]),
                number: text(&a["number"]),
                name: a["name"].as_str().unwrap_or_default().to_string(),
                source_row: a["sourceRow"].as_u64(),
                source_rows: a["sourceRows"]
                    .as_array()
                    .map(|l| l.iter().filter_map(Value::as_u64).collect())
                    .unwrap_or_default(),
                source_keys: strings(&a["sourceKeys"]),
                source: money(&a["sourceExact"])?,
                books: money(&a["booksExact"])?,
                diff: money(&a["diffExact"])?,
            })
        })
        .collect::<Result<_, ApiError>>()?;
    let balance_rows = v["balanceRows"]
        .as_array()
        .map(|l| {
            l.iter()
                .filter_map(|r| {
                    // An unknown check from a newer engine is skipped, but
                    // `all_zero` still covers it.
                    let key = r["key"].as_str().and_then(BalanceKey::from_wire)?;
                    Some((|| {
                        Ok(BalanceRow {
                            key,
                            account: opt_id(&r["accountId"]),
                            source: money(&r["sourceExact"])?,
                            books: money(&r["booksExact"])?,
                            diff: money(&r["diffExact"])?,
                        })
                    })())
                })
                .collect::<Result<_, ApiError>>()
        })
        .transpose()?
        .unwrap_or_default();
    let r = &v["referenceTrialBalance"];
    Ok(TieOut {
        batch: opt_id(&v["batchId"]),
        as_of: date(&v["asOf"]).ok_or_else(decode_err)?,
        computed_at: text(&v["computedAt"]),
        books: v["books"].as_str().and_then(TieOutBooks::from_wire),
        reference: r.is_object().then(|| ReferenceTrialBalance {
            session: opt_id(&r["sessionId"]),
            report_date: date(&r["reportDate"]),
        }),
        accounts,
        ar_items: items_from(&v["arItems"])?,
        ap_items: items_from(&v["apItems"])?,
        inventory_items: v["inventoryItems"].as_array().map_or_else(
            || Ok(Vec::new()),
            |l| l.iter().map(stock_from).collect::<Result<_, _>>(),
        )?,
        balance_rows,
        totals: totals_from(&v["totals"])?,
        difference_count: count(&v["differenceCount"]),
        all_zero: v["allZero"].as_bool().ok_or_else(decode_err)?,
        verified_at: text(&v["verifiedAt"]),
    })
}

/// Reads the tie-out (E3): a bare tie-out, or `{tieout}`.
pub(crate) fn tieout_from(body: &[u8]) -> Result<TieOut, ApiError> {
    let v = parse_json(body)?;
    if v["tieout"].is_object() {
        tieout_value(&v["tieout"])
    } else {
        tieout_value(&v)
    }
}

/// Reads a verify answer `{batch, tieout}`.
pub(crate) fn verified_from(body: &[u8]) -> Result<Verified, ApiError> {
    let v = parse_json(body)?;
    Ok(Verified {
        batch: batch_from(&v["batch"])?,
        tieout: tieout_value(&v["tieout"])?,
    })
}

/// Reads `{transactions, nextCursor, total}`.
pub(crate) fn transactions_from(body: &[u8]) -> Result<BatchTransactionPage, ApiError> {
    let v = parse_json(body)?;
    let list = v["transactions"].as_array().ok_or_else(decode_err)?;
    let transactions = list
        .iter()
        .map(|t| {
            Ok(BatchTransaction {
                id: opt_id(&t["id"]).ok_or_else(decode_err)?,
                txn_type: t["txnType"].as_str().ok_or_else(decode_err)?.to_string(),
                date: date(&t["txnDate"]),
                number: text(&t["txnNumber"]),
                vendor_invoice_number: text(&t["vendorInvoiceNumber"]),
                status: text(&t["status"]),
                contact: opt_id(&t["contactId"]),
                contact_name: text(&t["contactName"]),
                total: money(&t["total"])?,
            })
        })
        .collect::<Result<_, ApiError>>()?;
    Ok(BatchTransactionPage {
        transactions,
        next_cursor: text(&v["nextCursor"]),
        total: count(&v["total"]),
    })
}

// ── Operations ──

/// Import operations of a session (`session.imports()`).
#[derive(Clone, Copy, Debug)]
pub struct Imports<'a>(pub(crate) &'a Session);

impl Session {
    /// QBD and other-system imports in the open company.
    #[must_use]
    pub fn imports(&self) -> Imports<'_> {
        Imports(self)
    }
}

fn at(op: &Operation, id: &str) -> Call {
    Call::new(op.method(), op.route(), op.route().replace("{id}", id))
}

fn require_all(s: &Session, caps: &[&str]) -> Result<(), ApiError> {
    for c in caps {
        require(s, c)?;
    }
    Ok(())
}

impl Imports<'_> {
    /// Uploads one file (checked first; nothing is sent when it is
    /// refused).
    ///
    /// # Errors
    /// `Local(Unsupported)` without the capabilities the file needs;
    /// `Local(AttachmentTooLarge | AttachmentType | InvalidValue)` from
    /// [`UploadRequest::check`]; 409 `IMPORT_ALREADY_COMMITTED`
    /// ([`import_refusal`]); any [`ApiError`].
    pub async fn upload(&self, req: &UploadRequest) -> Result<UploadOutcome, ApiError> {
        require_all(self.0, &req.capabilities())?;
        let ext = req.check().map_err(local)?;
        let (boundary, body) = req.multipart(&ext);
        let call = Call::at(Method::Post, IMPORTS_UPLOAD.route()).multipart_body(&boundary, body);
        let reply = self.0.send(&IMPORTS_UPLOAD, call, None).await?;
        upload_from(&reply.body)
    }

    /// One uploaded file, as the engine read it.
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn session(&self, id: ImportSessionId) -> Result<UploadOutcome, ApiError> {
        require(self.0, Capabilities::IMPORTS_AMOUNTS)?;
        let call = at(&IMPORTS_SESSION, &id.to_string());
        let reply = self.0.send(&IMPORTS_SESSION, call, None).await?;
        upload_from(&reply.body)
    }

    /// Discards an uploaded file that is not in a committed batch.
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn discard(&self, id: ImportSessionId) -> Result<(), ApiError> {
        require(self.0, Capabilities::IMPORTS_AMOUNTS)?;
        let call = at(&IMPORTS_DISCARD, &id.to_string());
        self.0.send(&IMPORTS_DISCARD, call, None).await?;
        Ok(())
    }

    /// Groups uploaded files into a batch.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for no files, a repeated file or bad options;
    /// 409 `IMPORT_SESSION_IN_BATCH`, `IMPORT_PERIOD_OVERLAP`,
    /// `IMPORT_FISCAL_YEAR_LOCKED`; any [`ApiError`].
    pub async fn create_batch(&self, new: &NewBatch) -> Result<ImportBatch, ApiError> {
        require_all(self.0, &new.options.capabilities())?;
        let mut seen = std::collections::BTreeSet::new();
        if new.sessions.is_empty() || !new.sessions.iter().all(|s| seen.insert(*s)) {
            return Err(local(LocalValidation::InvalidValue));
        }
        let body = json!({
            "sourceSystem": new.source.as_wire(),
            "kind": new.kind.as_wire(),
            "sessionIds": new.sessions.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "options": new.options.body().map_err(local)?,
        });
        let bytes = crate::session::json_body(&body)?;
        let call = Call::at(Method::Post, IMPORT_BATCH_CREATE.route()).json_body(bytes);
        let reply = self.0.send(&IMPORT_BATCH_CREATE, call, None).await?;
        one_batch_from(&reply.body)
    }

    /// Commits a batch under `key` (made when the owner clicked Import; a
    /// changed body — password, confirmation — gets a new key). Answers
    /// when the job is accepted; poll [`Imports::batch`].
    ///
    /// # Errors
    /// `Local(BackupRequired)` without a backup reference; the refusals
    /// [`import_refusal`] reads; `ClosedPeriod`, `ClosingPasswordRejected`,
    /// `ClosingPasswordRateLimited`; `Sensitive`; `OutcomeUnknown` when the
    /// answer was lost (Check status, or send again with the same key).
    pub async fn commit(
        &self,
        id: ImportBatchId,
        req: &CommitRequest,
        key: IdempotencyKey,
    ) -> Result<JobAccepted, ApiError> {
        require(self.0, Capabilities::IMPORTS_BATCH)?;
        let backup = req.backup_ref.trim();
        if backup.is_empty() {
            return Err(local(LocalValidation::BackupRequired));
        }
        let mut body = Map::new();
        body.insert("backupRef".into(), json!(backup));
        if req.confirm_closed_period {
            body.insert("confirmClosedPeriod".into(), json!(true));
        }
        self.job(
            &IMPORT_BATCH_COMMIT,
            id,
            Value::Object(body),
            req.closed_period.clone(),
            key,
        )
        .await
    }

    /// Undoes a committed batch under `key`, after a verified backup.
    ///
    /// # Errors
    /// As for [`Imports::commit`], and 409 `IMPORT_UNDO_BLOCKED`.
    pub async fn undo(
        &self,
        id: ImportBatchId,
        backup_ref: &str,
        closed_period: Option<ClosedPeriodAuthorization>,
        key: IdempotencyKey,
    ) -> Result<JobAccepted, ApiError> {
        require(self.0, Capabilities::IMPORTS_BATCH)?;
        let backup = backup_ref.trim();
        if backup.is_empty() {
            return Err(local(LocalValidation::BackupRequired));
        }
        self.job(
            &IMPORT_BATCH_UNDO,
            id,
            json!({ "backupRef": backup }),
            closed_period,
            key,
        )
        .await
    }

    async fn job(
        &self,
        op: &Operation,
        id: ImportBatchId,
        body: Value,
        closed_period: Option<ClosedPeriodAuthorization>,
        key: IdempotencyKey,
    ) -> Result<JobAccepted, ApiError> {
        let bytes = posting_body(self.0, body, &PostingOptions { closed_period })?;
        let mut call = at(op, &id.to_string()).json_body(bytes);
        call.idempotency_key = Some(key);
        call.once = true;
        let reply = self.0.send(op, call, None).await?;
        accepted_from(&reply.body, reply.replayed)
    }

    /// One batch with its job progress and failure (Check status).
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn batch(&self, id: ImportBatchId) -> Result<BatchState, ApiError> {
        require(self.0, Capabilities::IMPORTS_BATCH)?;
        let call = at(&IMPORT_BATCH, &id.to_string());
        let reply = self.0.send(&IMPORT_BATCH, call, None).await?;
        state_from(&reply.body)
    }

    /// One page of History, newest first.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn batches(&self, cursor: Option<&str>) -> Result<BatchPage, ApiError> {
        require(self.0, Capabilities::IMPORTS_BATCH)?;
        let mut call = Call::at(Method::Get, IMPORT_BATCHES.route());
        call.query.push(("limit".into(), HISTORY_PAGE.to_string()));
        if let Some(c) = cursor {
            call.query.push(("cursor".into(), c.to_string()));
        }
        let reply = self.0.send(&IMPORT_BATCHES, call, None).await?;
        page_from(&reply.body)
    }

    /// Every page of History, newest first, up to [`HISTORY_MAX_PAGES`].
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn batches_all(&self) -> Result<Vec<ImportBatch>, ApiError> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..HISTORY_MAX_PAGES {
            let page = self.batches(cursor.as_deref()).await?;
            let got = page.batches.len();
            all.extend(page.batches);
            match page.next_cursor {
                Some(c) if got > 0 => cursor = Some(c),
                _ => break,
            }
        }
        Ok(all)
    }

    /// The batch's tie-out, computed by the engine.
    ///
    /// # Errors
    /// `InexactMoney` when an amount is not an exact string; any
    /// [`ApiError`].
    pub async fn tieout(&self, id: ImportBatchId) -> Result<TieOut, ApiError> {
        require(self.0, Capabilities::IMPORTS_TIEOUT)?;
        let call = at(&IMPORT_BATCH_TIEOUT, &id.to_string()).json_body(REQUIRE_REFERENCE.to_vec());
        let reply = self.0.send(&IMPORT_BATCH_TIEOUT, call, None).await?;
        tieout_from(&reply.body)
    }

    /// Marks the batch verified: the engine recomputes the tie-out and
    /// refuses unless every difference is zero. Verifying a verified batch
    /// answers with what it kept.
    ///
    /// # Errors
    /// 409 `TIEOUT_NOT_ZERO` (with the first differences' ids); 403
    /// `IMPORT_ADMIN_REQUIRED`; 409 `IMPORT_BUSY`; the tie-out's refusals;
    /// any [`ApiError`].
    pub async fn verify(&self, id: ImportBatchId) -> Result<Verified, ApiError> {
        require(self.0, Capabilities::IMPORTS_TIEOUT)?;
        let call = at(&IMPORT_BATCH_VERIFY, &id.to_string()).json_body(REQUIRE_REFERENCE.to_vec());
        let reply = self.0.send(&IMPORT_BATCH_VERIFY, call, None).await?;
        verified_from(&reply.body)
    }

    /// One page of the transactions the batch created (date, then id).
    ///
    /// # Errors
    /// 400 `INVALID_CURSOR`; any [`ApiError`].
    pub async fn transactions(
        &self,
        id: ImportBatchId,
        cursor: Option<&str>,
        limit: u16,
    ) -> Result<BatchTransactionPage, ApiError> {
        require(self.0, Capabilities::IMPORTS_TIEOUT)?;
        let mut call = at(&IMPORT_BATCH_TRANSACTIONS, &id.to_string());
        call.query.push((
            "limit".into(),
            limit.clamp(1, TRANSACTIONS_PAGE).to_string(),
        ));
        if let Some(c) = cursor {
            call.query.push(("cursor".into(), c.to_string()));
        }
        let reply = self.0.send(&IMPORT_BATCH_TRANSACTIONS, call, None).await?;
        transactions_from(&reply.body)
    }

    /// Saves the spot-check list on the batch.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a blank or repeated key or a note over
    /// [`MAX_NOTE_CHARS`]; any [`ApiError`].
    pub async fn save_spot_check(
        &self,
        id: ImportBatchId,
        items: &[SpotCheckItem],
    ) -> Result<(), ApiError> {
        require(self.0, Capabilities::IMPORTS_TIEOUT)?;
        let mut seen = std::collections::BTreeSet::new();
        if items.len() > MAX_SPOT_ITEMS
            || items.iter().any(|i| {
                i.key.trim().is_empty()
                    || i.key.chars().count() > MAX_SPOT_KEY_CHARS
                    || i.label.chars().count() > MAX_SPOT_LABEL_CHARS
                    || !seen.insert(i.key.as_str())
                    || i.note.chars().count() > MAX_NOTE_CHARS
            })
        {
            return Err(local(LocalValidation::InvalidValue));
        }
        let list: Vec<Value> = items
            .iter()
            .map(|i| json!({"key": i.key, "label": i.label, "done": i.done, "note": i.note}))
            .collect();
        let bytes = crate::session::json_body(&json!({ "items": list }))?;
        let call = at(&IMPORT_BATCH_SPOT_CHECK, &id.to_string()).json_body(bytes);
        self.0.send(&IMPORT_BATCH_SPOT_CHECK, call, None).await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "imports_tests.rs"]
mod tests;
