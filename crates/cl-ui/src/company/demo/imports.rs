//! The demo engine's QBD and other-system imports (P31 D5a): it answers
//! every call of `cl_api::imports` from the synthetic fixtures under
//! `fixtures/qbd/` and `fixtures/imports/`, so the screens and their
//! control-channel scenarios run with no engine.
//!
//! A file is recognised by comparing its bytes with the fixtures compiled
//! in (`include_bytes!`); any other file is refused (422
//! `IMPORT_UNREADABLE`). Row counts and dates come from the file; every
//! amount the demo shows is read as an exact decimal string from the set's
//! hand-worked `EXPECTED.toml` (`[totals]`, `[reference_tb]`, `[diffs]`,
//! `[balance_rows]`). The demo never adds a file's amounts itself.
//!
//! What the demo plays, with the engine's codes (U-260 to U-268):
//! - upload answers a session, a preview and row problems (`bad-rows.csv`
//!   has one per bad group);
//! - commit and undo answer 202, and the batch reports progress over a few
//!   polls ([`POLLS_TO_FINISH`]) before it is committed (undone);
//! - 409 `IMPORT_BUSY`, `IMPORT_ALREADY_COMMITTED` (at upload and commit),
//!   `IMPORT_PERIOD_OVERLAP`, `IMPORT_SESSION_IN_BATCH`,
//!   `IMPORT_COMPANY_NOT_EMPTY`, `IMPORT_FISCAL_YEAR_LOCKED`,
//!   `IMPORT_UNDO_BLOCKED` (blockers added with
//!   [`DemoImports::add_blocker`]) and `TIEOUT_NOT_ZERO`;
//! - a batch dated on or before the closing date needs the closing
//!   password: `ClosedPeriod`, `ClosingPasswordRejected`; the demo accepts
//!   [`PRACTICE_PASSWORD`] and the other demo screens' closing password.

// M10: no floating point in import code (money is exact decimal strings).
#![deny(clippy::float_arithmetic)]

use std::collections::BTreeMap;

use cl_api::imports::{
    AccountMatch, BalanceKey, BalanceRow, BatchFailure, BatchPage, BatchState, BatchStatus, CommitRequest,
    FilePreview, FileRole, ImportBatch, ImportBatchId, ImportKind, ImportSession, ImportSessionId,
    JobAccepted, MatchStatus, NewBatch, Progress, RowProblem, SessionStatus, SourceSystem, SpotCheckItem,
    TieOut, TieOutAccount, TieOutTotals, UndoBlocker, UploadOutcome, UploadRequest,
};
use cl_api::{
    ApiError, ConflictDetails, ConflictReason, ErrorCode, IdempotencyKey, UserText, ValidationError,
};
use cl_core::{CalendarDate, Money, Secret};
use serde_json::{Value, json};

use super::register::{DEMO_CLOSING_PASSWORD, check_closing};

/// The closing-date password the import scenarios use.
pub const PRACTICE_PASSWORD: &str = "practice";

/// `batch` polls a commit or undo takes before it ends (the first ones
/// answer a running progress).
pub const POLLS_TO_FINISH: u8 = 3;

/// The progress steps a running job reports, one per poll.
const STEPS: [&str; 2] = ["accounts", "journal"];

const COMMITTED_AT: &str = "2026-10-27T12:00:00.000Z";

/// One fixture set: the base `EXPECTED.toml` and the altered one, if any.
struct Set {
    base: &'static str,
    altered: Option<&'static str>,
    /// A QBD conversion: the tie-out also carries the four balance rows.
    qbd: bool,
}

macro_rules! expected {
    ($path:literal) => {
        include_str!(concat!("../../../../../fixtures/", $path, "/EXPECTED.toml"))
    };
}

const SETS: [Set; 10] = [
    // 0: Shoal Creek.
    Set {
        base: expected!("qbd/shoal-creek"),
        altered: None,
        qbd: true,
    },
    // 1: bill re-pointed from 6100 to 6200.
    Set {
        base: expected!("qbd/shoal-creek"),
        altered: Some(expected!("qbd/shoal-creek-altered/repointed")),
        qbd: true,
    },
    // 2: open invoice 1005 raised by 10.00.
    Set {
        base: expected!("qbd/shoal-creek"),
        altered: Some(expected!("qbd/shoal-creek-altered/open-item")),
        qbd: true,
    },
    // 3: a journal that overlaps the base journals.
    Set {
        base: expected!("qbd/shoal-creek"),
        altered: None,
        qbd: true,
    },
    // 4..8: other systems.
    Set {
        base: expected!("imports/accounting-power"),
        altered: None,
        qbd: false,
    },
    Set {
        base: expected!("imports/generic"),
        altered: None,
        qbd: false,
    },
    Set {
        base: expected!("imports/generic"),
        altered: Some(expected!("imports/generic-altered")),
        qbd: false,
    },
    Set {
        base: expected!("imports/quickbooks-online"),
        altered: None,
        qbd: false,
    },
    Set {
        base: expected!("imports/qbd-gl"),
        altered: None,
        qbd: false,
    },
    // 9: spreadsheet transactions.
    Set {
        base: expected!("imports/transactions"),
        altered: None,
        qbd: false,
    },
];

/// A compiled-in file: its set, bytes and (journals) the dates it covers.
struct Fixture {
    set: usize,
    name: &'static str,
    bytes: &'static [u8],
    period: Option<(&'static str, &'static str)>,
}

macro_rules! fixture {
    ($set:expr, $path:literal) => {
        fixture!($set, $path, None)
    };
    ($set:expr, $path:literal, $period:expr) => {
        Fixture {
            set: $set,
            name: $path,
            bytes: include_bytes!(concat!("../../../../../fixtures/", $path)),
            period: $period,
        }
    };
}

const FIXTURES: [Fixture; 41] = [
    fixture!(0, "qbd/shoal-creek/account-list.csv"),
    fixture!(0, "qbd/shoal-creek/customer-list.csv"),
    fixture!(0, "qbd/shoal-creek/vendor-list.csv"),
    fixture!(0, "qbd/shoal-creek/other-names.csv"),
    fixture!(0, "qbd/shoal-creek/item-list.csv"),
    fixture!(0, "qbd/shoal-creek/inventory-valuation.csv"),
    fixture!(0, "qbd/shoal-creek/tb-opening-2023-12-31.csv"),
    fixture!(0, "qbd/shoal-creek/tb-2026-10-27.csv"),
    fixture!(
        0,
        "qbd/shoal-creek/journal-2024.csv",
        Some(("2024-01-01", "2024-12-31"))
    ),
    fixture!(
        0,
        "qbd/shoal-creek/journal-2025-2026.csv",
        Some(("2025-01-01", "2026-10-27"))
    ),
    fixture!(0, "qbd/shoal-creek/open-invoices.csv"),
    fixture!(0, "qbd/shoal-creek/unpaid-bills.csv"),
    fixture!(
        1,
        "qbd/shoal-creek-altered/repointed/journal-2025-2026.csv",
        Some(("2025-01-01", "2026-10-27"))
    ),
    fixture!(2, "qbd/shoal-creek-altered/open-item/open-invoices.csv"),
    fixture!(
        3,
        "qbd/shoal-creek-altered/journal-overlap.csv",
        Some(("2025-12-01", "2026-01-31"))
    ),
    fixture!(4, "imports/accounting-power/coa.csv"),
    fixture!(4, "imports/accounting-power/customers.csv"),
    fixture!(4, "imports/accounting-power/vendors.csv"),
    fixture!(4, "imports/accounting-power/tb-opening.csv"),
    fixture!(4, "imports/accounting-power/tb-reference.csv"),
    fixture!(
        4,
        "imports/accounting-power/gl.csv",
        Some(("2026-01-01", "2026-06-30"))
    ),
    fixture!(5, "imports/generic/coa.csv"),
    fixture!(5, "imports/generic/contacts.csv"),
    fixture!(5, "imports/generic/tb-opening.csv"),
    fixture!(5, "imports/generic/tb-reference.csv"),
    fixture!(
        5,
        "imports/generic/gl.csv",
        Some(("2026-01-01", "2026-06-30"))
    ),
    fixture!(
        6,
        "imports/generic-altered/gl.csv",
        Some(("2026-01-01", "2026-06-30"))
    ),
    fixture!(7, "imports/quickbooks-online/coa.csv"),
    fixture!(7, "imports/quickbooks-online/customers.csv"),
    fixture!(7, "imports/quickbooks-online/vendors.csv"),
    fixture!(7, "imports/quickbooks-online/tb-opening.csv"),
    fixture!(7, "imports/quickbooks-online/tb-reference.csv"),
    fixture!(
        7,
        "imports/quickbooks-online/gl.csv",
        Some(("2026-01-01", "2026-06-30"))
    ),
    fixture!(8, "imports/qbd-gl/account-list.csv"),
    fixture!(8, "imports/qbd-gl/customer-list.csv"),
    fixture!(8, "imports/qbd-gl/vendor-list.csv"),
    fixture!(8, "imports/qbd-gl/tb-opening.csv"),
    fixture!(8, "imports/qbd-gl/tb-reference.csv"),
    fixture!(
        8,
        "imports/qbd-gl/journal.csv",
        Some(("2026-01-01", "2026-06-30"))
    ),
    fixture!(9, "imports/transactions/good.csv"),
    fixture!(9, "imports/transactions/bad-rows.csv"),
];

/// The bytes of a compiled-in fixture by its path under `fixtures/`
/// (scenarios and tests upload these).
#[must_use]
pub fn fixture_bytes(name: &str) -> Option<&'static [u8]> {
    FIXTURES.iter().find(|f| f.name == name).map(|f| f.bytes)
}

// ── EXPECTED.toml readers ──

/// The `key = "value"` pairs of one `[section]` (quotes stripped).
fn section(text: &str, name: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(h) = line.strip_prefix('[') {
            inside = h.trim_end_matches(']') == name;
        } else if inside && let Some((k, v)) = line.split_once('=') {
            out.insert(
                k.trim().trim_matches('"').to_string(),
                v.trim().trim_matches('"').to_string(),
            );
        }
    }
    out
}

fn day(s: &str) -> Option<CalendarDate> {
    CalendarDate::parse_wire(s).ok()
}

fn money(s: Option<&String>) -> Option<Money> {
    s.and_then(|s| Money::parse_wire(s).ok())
}

// ── Errors ──

fn conflict(code: &str, details: Value) -> ApiError {
    ApiError::Conflict {
        reason: ConflictReason::Unspecified,
        current_version: None,
        code: ErrorCode::parse(code),
        message: UserText::default(),
        details: ConflictDetails::new(details),
    }
}

fn refused(code: &str, details: Value) -> ApiError {
    ApiError::Validation(ValidationError::coded(code, details))
}

fn not_found() -> ApiError {
    ApiError::NotFound { code: None }
}

fn decode() -> ApiError {
    ApiError::Decode { what: "import" }
}

// ── State ──

/// One demo file.
#[derive(Clone, Debug)]
struct DemoSession {
    id: ImportSessionId,
    fixture: usize,
    kind: ImportKind,
    source: SourceSystem,
    role: FileRole,
    discarded: bool,
    batch: Option<ImportBatchId>,
    row_count: u64,
    problems: Vec<RowProblem>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Job {
    Commit,
    Undo,
}

/// One demo batch.
#[derive(Clone, Debug)]
struct DemoBatch {
    batch: ImportBatch,
    set: usize,
    /// The running job and the polls it has had.
    job: Option<(Job, u8)>,
    commit_key: Option<IdempotencyKey>,
    undo_key: Option<IdempotencyKey>,
    blockers: Vec<UndoBlocker>,
    error: Option<BatchFailure>,
    /// The commit refuses a company that has transactions (QBD).
    require_empty: bool,
    /// The commit took the company from empty to not empty.
    emptied: bool,
}

/// The demo's imports.
#[derive(Clone, Debug)]
pub struct DemoImports {
    sessions: Vec<DemoSession>,
    batches: Vec<DemoBatch>,
    next: u64,
    /// The company has no transactions yet (a QBD conversion needs that;
    /// the fiscal year can be set only then).
    pub company_empty: bool,
    /// Commits and undos received (control channel: never sent twice).
    pub jobs_received: u32,
    /// Source accounts the journal files name that match nothing (the
    /// mapping step's "pick an account"; scenarios add one).
    pub unmatched: Vec<String>,
    /// How the next backup before an import or undo answers.
    pub backup_mode: DemoBackup,
    /// The before-import backups the demo "made" (no files).
    pub backups_taken: u32,
}

/// How the demo answers the next backup before an import or an undo
/// (mirrors Import Payroll's). The one-shot modes go back to
/// [`DemoBackup::Succeed`] once used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DemoBackup {
    /// Made at once.
    #[default]
    Succeed,
    /// The next backup fails (as a missing passphrase would).
    FailOnce,
    /// The next backup stays running until released.
    HoldOnce,
}

impl Default for DemoImports {
    fn default() -> DemoImports {
        DemoImports {
            sessions: Vec::new(),
            batches: Vec::new(),
            next: 0,
            company_empty: true,
            jobs_received: 0,
            unmatched: Vec::new(),
            backup_mode: DemoBackup::Succeed,
            backups_taken: 0,
        }
    }
}

fn uuid(n: u64, kind: u8) -> String {
    uuid::Uuid::from_u128(
        (u128::from(kind) << 64) | u128::from(n) | 0x0d31_0000_0000_4000_8000_0000_0000_0000,
    )
    .to_string()
}

/// The data rows of a file (1-based file row numbers): the title lines of a report-style file
/// (`"Company",,` first line) and the header excluded.
fn data_rows(bytes: &[u8]) -> Vec<usize> {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    let head = if lines.first().is_some_and(|l| l.trim_end().ends_with(",,")) {
        4
    } else {
        1
    };
    lines
        .iter()
        .enumerate()
        .skip(head)
        .filter(|(_, l)| {
            let t = l.trim_matches(|c: char| c == ',' || c == '"' || c.is_whitespace());
            // Blank lines and the report's TOTAL line are not rows.
            !t.is_empty() && !t.to_ascii_uppercase().starts_with("TOTAL")
        })
        .map(|(i, _)| i + 1)
        .collect()
}

/// The report date of a file the engine reads one from (trial balances,
/// open items, valuation).
fn report_date(set: &Set, kind: ImportKind, role: FileRole) -> Option<CalendarDate> {
    let company = section(set.base, "company");
    match (kind, role) {
        (ImportKind::TrialBalance, FileRole::Post) => day(company.get("opening_tb_date")?),
        (
            ImportKind::TrialBalance
            | ImportKind::OpenInvoices
            | ImportKind::UnpaidBills
            | ImportKind::InventoryValuation,
            _,
        ) => day(company.get("conversion_date")?),
        _ => None,
    }
}

/// Row problems of `bad-rows.csv`: the first row of each bad group, with
/// the engine code the EXPECTED file names.
fn bad_row_problems(set: &Set, bytes: &[u8]) -> Vec<RowProblem> {
    let codes = section(set.base, "bad");
    let text = String::from_utf8_lossy(bytes);
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate().skip(1) {
        let group = line.split(',').next().unwrap_or_default().to_string();
        if let Some(code) = codes.get(&group)
            && !seen.contains(&group)
        {
            seen.push(group);
            out.push(RowProblem {
                row_number: u64::try_from(i + 1).unwrap_or(0),
                field: None,
                code: code.clone(),
            });
        }
    }
    out
}

fn overlaps(a: (CalendarDate, CalendarDate), b: (CalendarDate, CalendarDate)) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

impl DemoImports {
    fn next_id(&mut self, kind: u8) -> String {
        self.next += 1;
        uuid(self.next, kind)
    }

    fn session(&self, id: ImportSessionId) -> Result<&DemoSession, ApiError> {
        self.sessions
            .iter()
            .find(|s| s.id == id && !s.discarded)
            .ok_or_else(not_found)
    }

    fn batch_index(&self, id: ImportBatchId) -> Result<usize, ApiError> {
        self.batches
            .iter()
            .position(|b| b.batch.id == id)
            .ok_or_else(not_found)
    }

    /// Forgets every file and batch (scenario reset).
    pub fn reset(&mut self) {
        *self = DemoImports::default();
    }

    /// The backup before an import or an undo, simulated (no file):
    /// `Some(Ok(name))` made, `Some(Err)` failed, `None` still running.
    pub fn backup(&mut self) -> Option<Result<String, ()>> {
        match std::mem::take(&mut self.backup_mode) {
            DemoBackup::Succeed => {
                self.backups_taken += 1;
                Some(Ok(format!(
                    "demo-{}-before-import.clbackup",
                    self.backups_taken
                )))
            }
            DemoBackup::FailOnce => Some(Err(())),
            DemoBackup::HoldOnce => None,
        }
    }

    /// The accounts a file names, as the engine would match them: every
    /// account of the set's reference trial balance comes from the account
    /// list (`create`); a journal also names the scenario's unmatched ones.
    fn account_matches(&self, set: &Set, kind: ImportKind) -> Vec<AccountMatch> {
        if !matches!(kind, ImportKind::GlTransactions | ImportKind::TrialBalance) {
            return Vec::new();
        }
        let mut out: Vec<AccountMatch> = section(set.base, "reference_tb")
            .into_keys()
            .map(|k| AccountMatch {
                source_key: k.clone(),
                number: Some(k.clone()),
                name: k,
                status: MatchStatus::Create,
                account: None,
            })
            .collect();
        if kind == ImportKind::GlTransactions {
            out.extend(self.unmatched.iter().map(|k| AccountMatch {
                source_key: k.clone(),
                number: None,
                name: k.clone(),
                status: MatchStatus::Unmatched,
                account: None,
            }));
        }
        out
    }

    /// Something that keeps `batch` from being undone (a payment applied
    /// to an imported invoice, a reconciliation, …). Scenarios add one to
    /// play the 409.
    ///
    /// # Errors
    /// `NotFound` for an unknown batch.
    pub fn add_blocker(
        &mut self,
        batch: ImportBatchId,
        blocker: UndoBlocker,
    ) -> Result<(), ApiError> {
        let i = self.batch_index(batch)?;
        self.batches[i].blockers.push(blocker);
        Ok(())
    }

    /// Removes every blocker of `batch`.
    ///
    /// # Errors
    /// `NotFound` for an unknown batch.
    pub fn clear_blockers(&mut self, batch: ImportBatchId) -> Result<(), ApiError> {
        let i = self.batch_index(batch)?;
        self.batches[i].blockers.clear();
        Ok(())
    }

    /// The batch whose commit holds this file, when it is not undone.
    fn active_batch_of(&self, fixture: usize, source: SourceSystem) -> Option<&DemoBatch> {
        self.batches.iter().find(|b| {
            matches!(
                b.batch.status,
                BatchStatus::Committed | BatchStatus::Committing | BatchStatus::Undoing
            ) && b.batch.source == source
                && b.batch.sessions.iter().any(|sid| {
                    self.sessions
                        .iter()
                        .any(|s| s.id == *sid && s.fixture == fixture && s.role == FileRole::Post)
                })
        })
    }

    fn already_committed(b: &DemoBatch, sid: ImportSessionId) -> ApiError {
        conflict(
            "IMPORT_ALREADY_COMMITTED",
            json!({
                "batchId": b.batch.id.to_string(),
                "sessionId": sid.to_string(),
                "committedAt": COMMITTED_AT,
            }),
        )
    }

    /// Uploads one file: it must be a compiled-in fixture.
    ///
    /// # Errors
    /// `Local(..)` as the real client checks; 422 `IMPORT_UNREADABLE`;
    /// 409 `IMPORT_ALREADY_COMMITTED` for a file a live batch posted.
    pub fn upload(&mut self, req: &UploadRequest) -> Result<UploadOutcome, ApiError> {
        req.check().map_err(ApiError::Local)?;
        let fixture = FIXTURES
            .iter()
            .position(|f| f.bytes == req.bytes.as_slice())
            .ok_or_else(|| refused("IMPORT_UNREADABLE", json!({})))?;
        let set = &SETS[FIXTURES[fixture].set];
        if req.role == FileRole::Post
            && let Some(b) = self.active_batch_of(fixture, req.source)
        {
            let sid = b
                .batch
                .sessions
                .iter()
                .copied()
                .find(|sid| {
                    self.sessions
                        .iter()
                        .any(|s| s.id == *sid && s.fixture == fixture)
                })
                .ok_or_else(decode)?;
            return Err(Self::already_committed(b, sid));
        }
        let rows = data_rows(req.bytes.as_slice());
        let problems =
            if FIXTURES[fixture].set == 9 && FIXTURES[fixture].name.ends_with("bad-rows.csv") {
                bad_row_problems(set, &req.bytes)
            } else {
                Vec::new()
            };
        let id = ImportSessionId::parse(&self.next_id(1))?;
        let row_count = u64::try_from(rows.len()).unwrap_or(0);
        self.sessions.push(DemoSession {
            id,
            fixture,
            kind: req.kind,
            source: req.source,
            role: req.role,
            discarded: false,
            batch: None,
            row_count,
            problems,
        });
        self.session_outcome(id)
    }

    fn session_outcome(&self, id: ImportSessionId) -> Result<UploadOutcome, ApiError> {
        let s = self.session(id)?;
        let set = &SETS[FIXTURES[s.fixture].set];
        let date = report_date(set, s.kind, s.role);
        let totals = (s.kind == ImportKind::TrialBalance)
            .then(|| {
                let key = if s.role == FileRole::Post {
                    "opening_tb_debits"
                } else {
                    "reference_tb_debits"
                };
                money(section(set.base, "totals").get(key))
            })
            .flatten();
        let batch = s
            .batch
            .and_then(|b| self.batches.iter().find(|x| x.batch.id == b));
        let status = match batch.map(|b| b.batch.status) {
            Some(BatchStatus::Committed | BatchStatus::Undoing) => SessionStatus::Committed,
            Some(BatchStatus::Committing) => SessionStatus::Committing,
            Some(BatchStatus::Failed) => SessionStatus::Failed,
            _ => SessionStatus::Uploaded,
        };
        let errors = u64::try_from(s.problems.len()).unwrap_or(0);
        Ok(UploadOutcome {
            session: ImportSession {
                id: s.id,
                kind: s.kind,
                source: s.source,
                role: Some(s.role),
                status,
                row_count: s.row_count,
                error_count: errors,
                report_date: date,
                batch: s.batch,
            },
            preview: FilePreview {
                total_rows: s.row_count,
                error_count: errors,
                entry_count: None,
                report_date: date,
                // A trial balance balances: debits equal credits.
                total_debit: totals,
                total_credit: totals,
                account_matches: self.account_matches(set, s.kind),
            },
            problems: s.problems.clone(),
        })
    }

    /// One uploaded file as the engine read it.
    ///
    /// # Errors
    /// `NotFound`.
    pub fn session_read(&self, id: ImportSessionId) -> Result<UploadOutcome, ApiError> {
        self.session_outcome(id)
    }

    /// Discards a file that is not in a live batch.
    ///
    /// # Errors
    /// `NotFound`; 409 `IMPORT_SESSION_IN_BATCH`.
    pub fn discard(&mut self, id: ImportSessionId) -> Result<(), ApiError> {
        let in_batch = self.session(id)?.batch.is_some_and(|b| {
            self.batches
                .iter()
                .any(|x| x.batch.id == b && x.batch.status != BatchStatus::Undone)
        });
        if in_batch {
            return Err(conflict("IMPORT_SESSION_IN_BATCH", json!({})));
        }
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.discarded = true;
        }
        Ok(())
    }

    /// The period a journal file covers.
    fn period(&self, sid: ImportSessionId) -> Option<(CalendarDate, CalendarDate)> {
        let s = self.sessions.iter().find(|s| s.id == sid)?;
        let (a, b) = FIXTURES[s.fixture].period?;
        Some((day(a)?, day(b)?))
    }

    /// 409 `IMPORT_PERIOD_OVERLAP` when two files of the batch, or a file
    /// and a live batch of the same source, cover the same days.
    fn check_overlap(
        &self,
        sessions: &[ImportSessionId],
        source: SourceSystem,
        own: Option<ImportBatchId>,
    ) -> Result<(), ApiError> {
        let periods: Vec<(ImportSessionId, (CalendarDate, CalendarDate))> = sessions
            .iter()
            .filter_map(|s| Some((*s, self.period(*s)?)))
            .collect();
        for (i, (_, a)) in periods.iter().enumerate() {
            for (_, b) in &periods[i + 1..] {
                if overlaps(*a, *b) {
                    return Err(overlap_refusal(None, *a, *b));
                }
            }
            for other in self.batches.iter().filter(|o| {
                Some(o.batch.id) != own
                    && o.batch.source == source
                    && matches!(
                        o.batch.status,
                        BatchStatus::Committed | BatchStatus::Committing | BatchStatus::Undoing
                    )
            }) {
                for sid in &other.batch.sessions {
                    if let Some(b) = self.period(*sid)
                        && overlaps(*a, b)
                    {
                        return Err(overlap_refusal(Some(other.batch.id), *a, b));
                    }
                }
            }
        }
        Ok(())
    }

    /// Groups uploaded files into a batch.
    ///
    /// # Errors
    /// `NotFound`; 409 `IMPORT_SESSION_IN_BATCH`, `IMPORT_PERIOD_OVERLAP`,
    /// `IMPORT_FISCAL_YEAR_LOCKED`.
    pub fn create_batch(&mut self, new: &NewBatch) -> Result<ImportBatch, ApiError> {
        if new.options.fiscal_year_start_month.is_some() && !self.company_empty {
            return Err(conflict("IMPORT_FISCAL_YEAR_LOCKED", json!({})));
        }
        let mut set = None;
        for sid in &new.sessions {
            let s = self.session(*sid)?;
            let in_batch = s.batch.is_some_and(|b| {
                self.batches
                    .iter()
                    .any(|x| x.batch.id == b && x.batch.status != BatchStatus::Undone)
            });
            if in_batch {
                return Err(conflict("IMPORT_SESSION_IN_BATCH", json!({})));
            }
            let fx_set = FIXTURES[s.fixture].set;
            // An altered set wins: its files are the point of the batch.
            if set.is_none() || SETS[fx_set].altered.is_some() || fx_set == 3 {
                set = Some(fx_set);
            }
        }
        self.check_overlap(&new.sessions, new.source, None)?;
        let set = set.ok_or_else(decode)?;
        let id = ImportBatchId::parse(&self.next_id(2))?;
        let company = section(SETS[set].base, "company");
        let conversion = SETS[set]
            .qbd
            .then(|| company.get("conversion_date").and_then(|d| day(d)))
            .flatten();
        let batch = ImportBatch {
            id,
            source: new.source,
            kind: new.kind,
            status: BatchStatus::Draft,
            sessions: new.sessions.clone(),
            backup_ref: None,
            company_was_empty: None,
            conversion_date: conversion,
            history_from: new.options.history_from,
            verified_at: None,
            verified_by: None,
            spot_check: Vec::new(),
            created_at: Some("2026-10-27T11:00:00.000Z".into()),
            committed_at: None,
        };
        for sid in &new.sessions {
            if let Some(s) = self.sessions.iter_mut().find(|s| s.id == *sid) {
                s.batch = Some(id);
            }
        }
        self.batches.push(DemoBatch {
            batch: batch.clone(),
            set,
            job: None,
            commit_key: None,
            undo_key: None,
            blockers: Vec::new(),
            error: None,
            require_empty: new.options.require_empty_company,
            emptied: false,
        });
        Ok(batch)
    }

    /// The earliest day the batch posts on (what the closed-period check
    /// looks at).
    fn earliest(&self, b: &DemoBatch) -> Option<CalendarDate> {
        let from = b.batch.history_from;
        b.batch
            .sessions
            .iter()
            .filter_map(|sid| {
                let s = self.sessions.iter().find(|s| s.id == *sid)?;
                if s.role != FileRole::Post {
                    return None;
                }
                let set = &SETS[FIXTURES[s.fixture].set];
                match s.kind {
                    ImportKind::GlTransactions => {
                        let first = self.period(*sid)?.0;
                        Some(from.map_or(first, |f| first.max(f)))
                    }
                    ImportKind::TrialBalance => report_date(set, s.kind, s.role),
                    _ => None,
                }
            })
            .min()
    }

    fn closing_check(
        &self,
        b: &DemoBatch,
        closing: Option<CalendarDate>,
        password: Option<&Secret>,
    ) -> Result<(), ApiError> {
        let dates: Vec<CalendarDate> = self.earliest(b).into_iter().collect();
        let pw = password.map(|p| {
            if p.expose() == PRACTICE_PASSWORD {
                Secret::new(DEMO_CLOSING_PASSWORD)
            } else {
                Secret::new(p.expose())
            }
        });
        check_closing(closing, &dates, pw.as_ref())
    }

    fn running(&self) -> bool {
        self.batches.iter().any(|b| b.job.is_some())
    }

    /// Commits a batch under `key`: answers 202 and the batch reports
    /// progress over the next polls.
    ///
    /// # Errors
    /// `Local(BackupRequired)`; `NotFound`; 409 `IMPORT_BUSY`,
    /// `IMPORT_ALREADY_COMMITTED`, `IMPORT_PERIOD_OVERLAP`,
    /// `IMPORT_COMPANY_NOT_EMPTY`; `ClosedPeriod`,
    /// `ClosingPasswordRejected`.
    pub fn commit(
        &mut self,
        id: ImportBatchId,
        req: &CommitRequest,
        key: &IdempotencyKey,
        password: Option<&Secret>,
        closing: Option<CalendarDate>,
    ) -> Result<JobAccepted, ApiError> {
        self.jobs_received += 1;
        if req.backup_ref.trim().is_empty() {
            return Err(ApiError::Local(cl_api::LocalValidation::BackupRequired));
        }
        let i = self.batch_index(id)?;
        if self.batches[i].commit_key.as_ref() == Some(key) {
            return Ok(JobAccepted {
                batch: id,
                replayed: true,
            });
        }
        let b = self.batches[i].clone();
        if b.batch.status == BatchStatus::Committed {
            let sid = b.batch.sessions.first().copied().ok_or_else(decode)?;
            return Err(Self::already_committed(&b, sid));
        }
        if !matches!(b.batch.status, BatchStatus::Draft | BatchStatus::Failed) {
            return Err(conflict("IMPORT_BUSY", json!({})));
        }
        if self.running() {
            return Err(conflict("IMPORT_BUSY", json!({})));
        }
        for sid in &b.batch.sessions {
            let s = self.session(*sid)?;
            if s.role == FileRole::Post
                && let Some(o) = self.active_batch_of(s.fixture, b.batch.source)
                && o.batch.id != id
            {
                return Err(Self::already_committed(o, *sid));
            }
        }
        self.check_overlap(&b.batch.sessions, b.batch.source, Some(id))?;
        if b.require_empty && !self.company_empty {
            return Err(conflict("IMPORT_COMPANY_NOT_EMPTY", json!({})));
        }
        self.closing_check(&b, closing, password)?;
        let me = &mut self.batches[i];
        me.batch.status = BatchStatus::Committing;
        me.batch.backup_ref = Some(req.backup_ref.trim().to_string());
        me.job = Some((Job::Commit, 0));
        me.commit_key = Some(key.clone());
        me.error = None;
        Ok(JobAccepted {
            batch: id,
            replayed: false,
        })
    }

    /// Undoes a committed batch under `key`.
    ///
    /// # Errors
    /// `Local(BackupRequired)`; `NotFound`; 409 `IMPORT_BUSY`,
    /// `IMPORT_UNDO_BLOCKED`; `ClosedPeriod`, `ClosingPasswordRejected`.
    pub fn undo(
        &mut self,
        id: ImportBatchId,
        backup_ref: &str,
        key: &IdempotencyKey,
        password: Option<&Secret>,
        closing: Option<CalendarDate>,
    ) -> Result<JobAccepted, ApiError> {
        self.jobs_received += 1;
        if backup_ref.trim().is_empty() {
            return Err(ApiError::Local(cl_api::LocalValidation::BackupRequired));
        }
        let i = self.batch_index(id)?;
        if self.batches[i].undo_key.as_ref() == Some(key) {
            return Ok(JobAccepted {
                batch: id,
                replayed: true,
            });
        }
        let b = self.batches[i].clone();
        if self.running() {
            return Err(conflict("IMPORT_BUSY", json!({})));
        }
        if b.batch.status != BatchStatus::Committed {
            return Err(conflict(
                "IMPORT_NOT_COMMITTED",
                json!({"status": b.batch.status.as_wire()}),
            ));
        }
        if !b.blockers.is_empty() {
            let list: Vec<Value> = b
                .blockers
                .iter()
                .map(|x| json!({"kind": x.kind, "id": x.id, "reason": x.reason}))
                .collect();
            return Err(conflict("IMPORT_UNDO_BLOCKED", json!({"blockers": list})));
        }
        self.closing_check(&b, closing, password)?;
        let me = &mut self.batches[i];
        me.batch.status = BatchStatus::Undoing;
        me.job = Some((Job::Undo, 0));
        me.undo_key = Some(key.clone());
        Ok(JobAccepted {
            batch: id,
            replayed: false,
        })
    }

    /// One batch with its job progress. Every poll of a running job moves
    /// it on; the [`POLLS_TO_FINISH`]th ends it.
    ///
    /// # Errors
    /// `NotFound`.
    pub fn batch(&mut self, id: ImportBatchId) -> Result<BatchState, ApiError> {
        let i = self.batch_index(id)?;
        let mut progress = None;
        if let Some((job, polls)) = self.batches[i].job {
            let polls = polls + 1;
            if polls >= POLLS_TO_FINISH {
                self.finish(i, job);
            } else {
                let step = STEPS[usize::from(polls - 1).min(STEPS.len() - 1)];
                progress = Some(Progress {
                    step: step.to_string(),
                    done: u64::from(polls),
                    total: u64::from(POLLS_TO_FINISH),
                });
                self.batches[i].job = Some((job, polls));
            }
        }
        let b = &self.batches[i];
        Ok(BatchState {
            batch: b.batch.clone(),
            progress,
            error: b.error.clone(),
        })
    }

    fn finish(&mut self, i: usize, job: Job) {
        let me = &mut self.batches[i];
        me.job = None;
        match job {
            Job::Commit => {
                me.batch.status = BatchStatus::Committed;
                me.batch.committed_at = Some(COMMITTED_AT.to_string());
                me.batch.company_was_empty = Some(self.company_empty);
                me.emptied = self.company_empty;
                self.company_empty = false;
            }
            Job::Undo => {
                me.batch.status = BatchStatus::Undone;
                me.batch.verified_at = None;
                me.batch.verified_by = None;
                me.batch.committed_at = None;
                // Undone: the files can be put in a new batch.
                let sessions = me.batch.sessions.clone();
                for s in self
                    .sessions
                    .iter_mut()
                    .filter(|s| sessions.contains(&s.id))
                {
                    s.batch = None;
                }
                // A conversion undone leaves the company empty again.
                if self.batches[i].emptied {
                    self.company_empty = true;
                }
            }
        }
    }

    /// One page of History, newest first (the demo has one page).
    #[must_use]
    pub fn batches(&self) -> BatchPage {
        BatchPage {
            batches: self.batches.iter().rev().map(|b| b.batch.clone()).collect(),
            next_cursor: None,
        }
    }

    fn committed(&self, id: ImportBatchId) -> Result<&DemoBatch, ApiError> {
        let b = &self.batches[self.batch_index(id)?];
        if b.batch.status == BatchStatus::Committed {
            Ok(b)
        } else {
            Err(conflict(
                "IMPORT_NOT_COMMITTED",
                json!({"status": b.batch.status.as_wire()}),
            ))
        }
    }

    /// The batch's tie-out against the reference files.
    ///
    /// # Errors
    /// `NotFound`; 409 `IMPORT_NOT_COMMITTED`.
    pub fn tieout(&self, id: ImportBatchId) -> Result<TieOut, ApiError> {
        let b = self.committed(id)?;
        tieout_of(&SETS[b.set], b.batch.conversion_date)
    }

    /// Marks the batch verified when every difference is zero.
    ///
    /// # Errors
    /// `NotFound`; 409 `IMPORT_NOT_COMMITTED`, `TIEOUT_NOT_ZERO`.
    pub fn verify(&mut self, id: ImportBatchId) -> Result<ImportBatch, ApiError> {
        let t = self.tieout(id)?;
        if !t.all_zero {
            let first: Vec<Value> = t
                .accounts
                .iter()
                .filter(|a| !a.diff.is_zero())
                .take(10)
                .map(|a| json!({"name": a.name, "diffExact": a.diff.to_wire()}))
                .collect();
            return Err(conflict(
                "TIEOUT_NOT_ZERO",
                json!({"firstDifferences": first}),
            ));
        }
        let i = self.batch_index(id)?;
        let b = &mut self.batches[i].batch;
        b.verified_at = Some("2026-10-27T13:00:00.000Z".into());
        b.verified_by = Some("demo-admin".into());
        Ok(b.clone())
    }

    /// Saves the spot-check list on the batch.
    ///
    /// # Errors
    /// `NotFound`.
    pub fn save_spot_check(
        &mut self,
        id: ImportBatchId,
        items: &[SpotCheckItem],
    ) -> Result<(), ApiError> {
        let i = self.batch_index(id)?;
        self.batches[i].batch.spot_check = items.to_vec();
        Ok(())
    }
}

fn overlap_refusal(
    batch: Option<ImportBatchId>,
    a: (CalendarDate, CalendarDate),
    b: (CalendarDate, CalendarDate),
) -> ApiError {
    let from = a.0.max(b.0);
    let to = a.1.min(b.1);
    let mut d = json!({"from": from.to_wire(), "to": to.to_wire()});
    if let Some(id) = batch {
        d["batchId"] = json!(id.to_string());
    }
    conflict("IMPORT_PERIOD_OVERLAP", d)
}

/// The tie-out of a set: the reference trial balance, plus the differences
/// the altered set's expectation names (books minus source).
fn tieout_of(set: &Set, conversion: Option<CalendarDate>) -> Result<TieOut, ApiError> {
    let reference = section(set.base, "reference_tb");
    let diffs = set.altered.map(|t| section(t, "diffs")).unwrap_or_default();
    let mut keys: Vec<&String> = reference.keys().chain(diffs.keys()).collect();
    keys.sort();
    keys.dedup();
    let zero = Money::from_cents(0).map_err(|_| decode())?;
    let mut accounts = Vec::new();
    for key in keys {
        let source = money(reference.get(key)).unwrap_or(zero);
        let diff = money(diffs.get(key)).unwrap_or(zero);
        let books = source.checked_add(diff).map_err(|_| decode())?;
        accounts.push(TieOutAccount {
            account: None,
            name: key.clone(),
            source_row: None,
            source_keys: vec![key.clone()],
            source,
            books,
            diff,
        });
    }
    let balances = set
        .altered
        .map(|t| section(t, "balance_rows"))
        .unwrap_or_default();
    let balance_rows: Vec<BalanceRow> = if set.qbd {
        [
            ("history_ar", BalanceKey::HistoryAr),
            ("history_ap", BalanceKey::HistoryAp),
            ("history_inventory", BalanceKey::HistoryInventory),
            ("valuation_vs_asset", BalanceKey::ValuationVsAsset),
        ]
        .into_iter()
        .map(|(name, key)| BalanceRow {
            key,
            diff: money(balances.get(name)).unwrap_or(zero),
        })
        .collect()
    } else {
        Vec::new()
    };
    let all_zero =
        accounts.iter().all(|a| a.diff.is_zero()) && balance_rows.iter().all(|r| r.diff.is_zero());
    let total =
        |f: fn(&TieOutAccount) -> Money| Money::sum(accounts.iter().map(f)).map_err(|_| decode());
    let totals = TieOutTotals {
        source: Some(total(|a| a.source)?),
        books: Some(total(|a| a.books)?),
        diff: Some(total(|a| a.diff)?),
    };
    let as_of = conversion
        .or_else(|| day(section(set.base, "company").get("conversion_date")?))
        .ok_or_else(decode)?;
    Ok(TieOut {
        as_of,
        accounts,
        ar_items: Vec::new(),
        ap_items: Vec::new(),
        inventory_items: Vec::new(),
        balance_rows,
        totals,
        all_zero,
    })
}

#[cfg(test)]
#[path = "imports_tests.rs"]
mod tests;
