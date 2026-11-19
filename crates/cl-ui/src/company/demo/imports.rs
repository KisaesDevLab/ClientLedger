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
    AccountMatch, BalanceKey, BalanceRow, BatchFailure, BatchFile, BatchPage, BatchState,
    BatchStatus, BatchTransaction, CommitRequest, FilePreview, FileRole, ImportBatch,
    ImportBatchId, ImportKind, ImportSession, ImportSessionId, JobAccepted, MatchStatus, NewBatch,
    Progress, ReferenceTrialBalance, RowProblem, SessionStatus, SourceSystem, SpotCheckItem,
    SystemRole, TieOut, TieOutAccount, TieOutBooks, TieOutItem, TieOutStock, TieOutTotal,
    TieOutTotals, TrialBalanceTotals, UndoBlocker, UploadOutcome, UploadRequest, Verified,
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
    /// The account list's Retained Earnings row the engine recognises (QBD:
    /// Equity named Retained Earnings; QBO: that detail type; U-272).
    retained: Option<&'static str>,
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
        retained: Some("3900"),
    },
    // 1: bill re-pointed from 6100 to 6200.
    Set {
        base: expected!("qbd/shoal-creek"),
        altered: Some(expected!("qbd/shoal-creek-altered/repointed")),
        qbd: true,
        retained: Some("3900"),
    },
    // 2: open invoice 1005 raised by 10.00.
    Set {
        base: expected!("qbd/shoal-creek"),
        altered: Some(expected!("qbd/shoal-creek-altered/open-item")),
        qbd: true,
        retained: Some("3900"),
    },
    // 3: a journal that overlaps the base journals.
    Set {
        base: expected!("qbd/shoal-creek"),
        altered: None,
        qbd: true,
        retained: Some("3900"),
    },
    // 4..8: other systems.
    Set {
        base: expected!("imports/accounting-power"),
        altered: None,
        qbd: false,
        retained: None,
    },
    Set {
        base: expected!("imports/generic"),
        altered: None,
        qbd: false,
        retained: None,
    },
    Set {
        base: expected!("imports/generic"),
        altered: Some(expected!("imports/generic-altered")),
        qbd: false,
        retained: None,
    },
    Set {
        base: expected!("imports/quickbooks-online"),
        altered: None,
        qbd: false,
        retained: Some("3200"),
    },
    Set {
        base: expected!("imports/qbd-gl"),
        altered: None,
        qbd: false,
        retained: Some("3200"),
    },
    // 9: spreadsheet transactions.
    Set {
        base: expected!("imports/transactions"),
        altered: None,
        qbd: false,
        retained: None,
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
    /// P31 QA-1: an account list key the demo matches by number to one of
    /// the company's accounts (another name): Review matches asks about it.
    pub number_match: Option<(String, cl_core::ids::AccountId)>,
    /// How the next backup before an import or undo answers.
    pub backup_mode: DemoBackup,
    /// The before-import backups the demo "made" (no files).
    pub backups_taken: u32,
    /// The next commit job fails with this batch error code (once), as an
    /// engine restart mid-job would (`IMPORT_INTERRUPTED`).
    pub fail_next_commit: Option<&'static str>,
    /// The company has a closing-date password. Without one, a batch in
    /// the closed period needs the owner's explicit confirmation
    /// (`confirmClosedPeriod`, plan §2a E1).
    pub closing_password_set: bool,
    /// Commits sent with `confirmClosedPeriod` (control channel).
    pub confirmed_closed: u32,
    /// A running commit or undo stays running (one step short of the end)
    /// until released: the close and quit prompts can be shown mid-job.
    pub hold_jobs: bool,
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
            number_match: None,
            backup_mode: DemoBackup::Succeed,
            backups_taken: 0,
            fail_next_commit: None,
            hold_jobs: false,
            closing_password_set: true,
            confirmed_closed: 0,
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

    /// The accounts a file names, as the engine matches them (E2b): each
    /// file against the chart as it is now (the demo company has none of the
    /// set's accounts). The account list makes every account of the set's
    /// reference trial balance (`create`); a trial balance or journal names
    /// them `unmatched` (the screen combines it with the account list), and
    /// a journal also names the scenario's unmatched ones.
    fn account_matches(&self, set: &Set, kind: ImportKind) -> Vec<AccountMatch> {
        let status = match kind {
            ImportKind::Coa => MatchStatus::Create,
            ImportKind::GlTransactions | ImportKind::TrialBalance => MatchStatus::Unmatched,
            _ => return Vec::new(),
        };
        let mut out: Vec<AccountMatch> = section(set.base, "reference_tb")
            .into_keys()
            .map(|k| AccountMatch {
                // U-272: only the account list carries the role.
                system_role: (kind == ImportKind::Coa && set.retained == Some(k.as_str()))
                    .then_some(SystemRole::RetainedEarnings),
                source_key: k.clone(),
                number: Some(k.clone()),
                name: k,
                status,
                account: None,
                matched_by: None,
                rows: 1,
            })
            .collect();
        if let Some((key, id)) = &self.number_match
            && let Some(m) = out.iter_mut().find(|m| &m.source_key == key)
        {
            m.status = MatchStatus::Existing;
            m.account = Some(*id);
            m.matched_by = Some(cl_api::imports::MatchedBy::Number);
        }
        if kind == ImportKind::GlTransactions {
            out.extend(self.unmatched.iter().map(|k| AccountMatch {
                source_key: k.clone(),
                number: None,
                name: k.clone(),
                status: MatchStatus::Unmatched,
                account: None,
                matched_by: None,
                rows: 1,
                system_role: None,
            }));
        }
        out.sort_by(|a, b| a.source_key.cmp(&b.source_key));
        out
    }

    /// What the batch's dates mean (E2c): the posted trial balance's date,
    /// the first journal day on or after the history start, and a gap
    /// warning between them.
    fn date_check(
        &self,
        set: &Set,
        sessions: &[ImportSessionId],
        options: &cl_api::imports::BatchOptions,
    ) -> cl_api::imports::DateCheck {
        let mine = || {
            sessions
                .iter()
                .filter_map(|sid| self.sessions.iter().find(|s| s.id == *sid))
                .filter(|s| s.role == FileRole::Post)
        };
        let opening = mine()
            .filter(|s| s.kind == ImportKind::TrialBalance)
            .filter_map(|s| report_date(set, s.kind, s.role))
            .min();
        let first = mine()
            .filter(|s| s.kind == ImportKind::GlTransactions)
            .filter_map(|s| self.period(s.id))
            .filter(|(_, to)| options.history_from.is_none_or(|f| *to >= f))
            .map(|(from, _)| options.history_from.map_or(from, |f| from.max(f)))
            .min();
        let mut warnings = Vec::new();
        if let (Some(o), Some(f)) = (opening, first) {
            let gap = o.days_until(f) - 1;
            if gap > 0 {
                warnings.push(cl_api::imports::DateWarning {
                    code: cl_api::imports::DateWarningCode::HistoryGap,
                    opening_balance_date: Some(o),
                    first_journal_date: Some(f),
                    days: u64::try_from(gap).ok(),
                });
            } else if gap < 0 {
                warnings.push(cl_api::imports::DateWarning {
                    code: cl_api::imports::DateWarningCode::JournalBeforeOpening,
                    opening_balance_date: Some(o),
                    first_journal_date: Some(f),
                    days: None,
                });
            }
        }
        cl_api::imports::DateCheck {
            conversion_date: options.conversion_date,
            history_from: options.history_from,
            history_skipped: 0,
            opening_balance_date: opening,
            first_journal_date: first,
            warnings,
        }
    }

    /// The batch's files as the engine lists them (no results until
    /// committed).
    fn batch_files(&self, set: &Set, sessions: &[ImportSessionId]) -> Vec<BatchFile> {
        sessions
            .iter()
            .filter_map(|sid| self.sessions.iter().find(|s| s.id == *sid))
            .map(|s| BatchFile {
                id: s.id,
                kind: Some(s.kind),
                role: Some(s.role),
                status: Some(SessionStatus::Uploaded),
                report_date: report_date(set, s.kind, s.role),
                result: None,
            })
            .collect()
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
        let company = section(SETS[set].base, "company");
        let conversion = SETS[set]
            .qbd
            .then(|| company.get("conversion_date").and_then(|d| day(d)))
            .flatten();
        self.check_dates(&SETS[set], &new.sessions, &new.options, conversion)?;
        let id = ImportBatchId::parse(&self.next_id(2))?;
        let files = self.batch_files(&SETS[set], &new.sessions);
        let date_check = Some(self.date_check(&SETS[set], &new.sessions, &new.options));
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
            files,
            date_check,
            error: None,
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

    /// The E2c refusals at create: 400 `IMPORT_BAD_DATE` for a history
    /// start after the conversion date; 422 `IMPORT_DATE_MISMATCH` naming
    /// every reference, open-item and valuation file not dated the
    /// conversion date the owner gave (the set's own date is what the files
    /// carry).
    fn check_dates(
        &self,
        set: &Set,
        sessions: &[ImportSessionId],
        options: &cl_api::imports::BatchOptions,
        carried: Option<CalendarDate>,
    ) -> Result<(), ApiError> {
        let Some(conv) = options.conversion_date else {
            return Ok(());
        };
        if options.history_from.is_some_and(|f| f > conv) {
            return Err(refused(
                "IMPORT_BAD_DATE",
                json!({"historyFrom": options.history_from.map(CalendarDate::to_wire), "conversionDate": conv.to_wire()}),
            ));
        }
        let files: Vec<Value> = sessions
            .iter()
            .filter_map(|sid| self.sessions.iter().find(|s| s.id == *sid))
            .filter(|s| {
                (s.kind == ImportKind::TrialBalance && s.role == FileRole::Reference)
                    || matches!(
                        s.kind,
                        ImportKind::OpenInvoices
                            | ImportKind::UnpaidBills
                            | ImportKind::InventoryValuation
                    )
            })
            .filter_map(|s| {
                let date = report_date(set, s.kind, s.role).or(carried);
                (date != Some(conv)).then(|| {
                    json!({
                        "sessionId": s.id.to_string(),
                        "kind": s.kind.as_wire(),
                        "role": s.role.as_wire(),
                        "reportDate": date.map(CalendarDate::to_wire),
                        "reason": if date.is_some() { "not_conversion_date" } else { "no_date" },
                    })
                })
            })
            .collect();
        if files.is_empty() {
            return Ok(());
        }
        Err(refused(
            "IMPORT_DATE_MISMATCH",
            json!({"conversionDate": conv.to_wire(), "files": files}),
        ))
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
        confirm: bool,
    ) -> Result<(), ApiError> {
        let dates: Vec<CalendarDate> = self.earliest(b).into_iter().collect();
        if !self.closing_password_set {
            // No closing-date password: the engine's staff confirmation
            // (`ledger.service.ts` 470-483) lets a confirmed commit in.
            let locked = dates.iter().any(|d| closing.is_some_and(|c| *d <= c));
            if locked && !confirm {
                return Err(ApiError::ClosedPeriod(cl_api::ClosedPeriod {
                    lock_date: closing,
                    can_override: true,
                    password_required: Some(false),
                    message: cl_api::UserText::default(),
                }));
            }
            return Ok(());
        }
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
        self.closing_check(&b, closing, password, req.confirm_closed_period)?;
        if req.confirm_closed_period {
            self.confirmed_closed += 1;
        }
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
                "IMPORT_BATCH_NOT_COMMITTED",
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
        self.closing_check(&b, closing, password, false)?;
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
            let polls = if self.hold_jobs {
                (polls + 1).min(POLLS_TO_FINISH - 1)
            } else {
                polls + 1
            };
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
            Job::Commit if self.fail_next_commit.is_some() => {
                // The job stopped: nothing posted, the batch can be retried
                // (or, for an unmapped account or a missing valuation, its
                // files go in a new batch: options are fixed, E2b).
                me.batch.status = BatchStatus::Failed;
                me.error = self.fail_next_commit.take().map(|code| BatchFailure {
                    code: code.to_string(),
                    details: cl_api::ConflictDetails::new(match code {
                        "IMPORT_UNMAPPED_ACCOUNT" => {
                            json!({"unmappedAccountKeys": ["6999"], "unmappedCount": 1})
                        }
                        "IMPORT_VALUATION_REQUIRED" => {
                            json!({"accounts": [{"sourceKey": "1300", "balance": "0.00"}]})
                        }
                        _ => json!({}),
                    }),
                    ..BatchFailure::default()
                });
                me.batch.error.clone_from(&me.error);
            }
            Job::Commit => {
                me.batch.status = BatchStatus::Committed;
                me.batch.committed_at = Some(COMMITTED_AT.to_string());
                me.batch.company_was_empty = Some(self.company_empty);
                me.batch.error = None;
                me.emptied = self.company_empty;
                self.company_empty = false;
                let set = &SETS[me.set];
                let history = me.batch.history_from.is_some();
                let rows: BTreeMap<ImportSessionId, u64> =
                    self.sessions.iter().map(|s| (s.id, s.row_count)).collect();
                for f in &mut me.batch.files {
                    f.status = Some(SessionStatus::Committed);
                    if f.role != Some(FileRole::Post) {
                        continue;
                    }
                    let created = rows.get(&f.id).copied().unwrap_or(0);
                    f.result = Some(demo_result(set, f.kind, created, history));
                }
                if let Some(dc) = &mut me.batch.date_check {
                    dc.history_skipped = 0;
                }
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
        match b.batch.status {
            BatchStatus::Committed => Ok(b),
            BatchStatus::Undone => Err(conflict(
                "IMPORT_BATCH_UNDONE",
                json!({"batchId": id.to_string()}),
            )),
            other => Err(conflict(
                "IMPORT_BATCH_NOT_COMMITTED",
                json!({"batchId": id.to_string(), "status": other.as_wire()}),
            )),
        }
    }

    /// The batch's tie-out against the reference files (after verify, the
    /// one kept).
    ///
    /// # Errors
    /// `NotFound`; 409 `IMPORT_BATCH_NOT_COMMITTED`, `IMPORT_BATCH_UNDONE`.
    pub fn tieout(&self, id: ImportBatchId) -> Result<TieOut, ApiError> {
        let b = self.committed(id)?;
        let mut t = tieout_of(&SETS[b.set], b.batch.conversion_date)?;
        t.batch = Some(id);
        t.verified_at.clone_from(&b.batch.verified_at);
        Ok(t)
    }

    /// Marks the batch verified when every difference is zero; again
    /// answers with what it kept.
    ///
    /// # Errors
    /// `NotFound`; 409 `IMPORT_BATCH_NOT_COMMITTED`, `IMPORT_BATCH_UNDONE`,
    /// `TIEOUT_NOT_ZERO` (ids and rows only, like the engine).
    pub fn verify(&mut self, id: ImportBatchId) -> Result<Verified, ApiError> {
        let t = self.tieout(id)?;
        if !t.all_zero {
            return Err(conflict(
                "TIEOUT_NOT_ZERO",
                json!({
                    "batchId": id.to_string(),
                    "differenceCount": t.difference_count,
                    "firstDifferences": first_differences(&t),
                }),
            ));
        }
        let i = self.batch_index(id)?;
        let b = &mut self.batches[i].batch;
        if b.verified_at.is_none() {
            b.verified_at = Some("2026-10-27T13:00:00.000Z".into());
            b.verified_by = Some("demo-admin".into());
        }
        let batch = b.clone();
        let mut tieout = t;
        tieout.verified_at.clone_from(&batch.verified_at);
        Ok(Verified { batch, tieout })
    }

    /// Saves the spot-check list on a committed batch.
    ///
    /// # Errors
    /// `NotFound`; 409 `IMPORT_BATCH_NOT_COMMITTED`, `IMPORT_BATCH_UNDONE`.
    pub fn save_spot_check(
        &mut self,
        id: ImportBatchId,
        items: &[SpotCheckItem],
    ) -> Result<(), ApiError> {
        self.committed(id)?;
        let i = self.batch_index(id)?;
        self.batches[i].batch.spot_check = items.to_vec();
        Ok(())
    }

    /// The transactions a committed batch made (the spot-check pool): one
    /// per open invoice and unpaid bill of the set, with the set's own
    /// amounts. Empty for any other batch.
    #[must_use]
    pub fn transactions(&self, id: ImportBatchId) -> Vec<BatchTransaction> {
        let Ok(b) = self.committed(id) else {
            return Vec::new();
        };
        let set = &SETS[b.set];
        if !set.qbd {
            return Vec::new();
        }
        let date = day(section(set.base, "company")
            .get("conversion_date")
            .map_or("", String::as_str));
        let mut out = Vec::new();
        for (n, (kind, list)) in [("invoice", "open_invoices"), ("bill", "unpaid_bills")]
            .into_iter()
            .enumerate()
        {
            for (k, (number, value)) in (0_u64..).zip(section(set.base, list)) {
                let Some(total) = money(Some(&value)) else {
                    continue;
                };
                let Ok(txn) = uuid(k + 1, 0x70 + u8::try_from(n).unwrap_or(0)).parse() else {
                    continue;
                };
                out.push(BatchTransaction {
                    id: txn,
                    txn_type: kind.into(),
                    date,
                    number: Some(number),
                    vendor_invoice_number: None,
                    status: Some("posted".into()),
                    contact: None,
                    contact_name: None,
                    total,
                });
            }
        }
        out
    }
}

/// `firstDifferences` as the engine sends them: section, key, source row
/// and ids only (never a name), at most 20.
fn first_differences(t: &TieOut) -> Vec<Value> {
    let row = |section: &str, key: Option<&str>, r: Option<u64>| {
        json!({"section": section, "key": key, "sourceRow": r, "accountId": null,
               "transactionId": null, "itemId": null})
    };
    let mut out: Vec<Value> = t
        .accounts
        .iter()
        .filter(|a| !a.diff.is_zero())
        .map(|a| row("accounts", None, a.source_row))
        .collect();
    out.extend(
        t.ar_items
            .iter()
            .filter(|i| !i.diff.is_zero())
            .map(|i| row("arItems", None, i.source_row)),
    );
    out.extend(
        t.ap_items
            .iter()
            .filter(|i| !i.diff.is_zero())
            .map(|i| row("apItems", None, i.source_row)),
    );
    out.extend(
        t.inventory_items
            .iter()
            .filter(|i| !i.ties())
            .map(|i| row("inventoryItems", None, i.source_row)),
    );
    out.extend(
        t.balance_rows
            .iter()
            .filter(|r| !r.diff.is_zero())
            .map(|r| row("balanceRows", Some(r.key.as_wire()), None)),
    );
    out.truncate(20);
    out
}

/// One posted file's result as the engine reports it: counts, and the
/// valuation's exact total from `EXPECTED.toml` (never summed here).
fn demo_result(
    set: &Set,
    kind: Option<ImportKind>,
    created: u64,
    history: bool,
) -> cl_api::imports::CommitSummary {
    let mut r = cl_api::imports::CommitSummary {
        created,
        ..cl_api::imports::CommitSummary::default()
    };
    match kind {
        Some(ImportKind::InventoryValuation) => {
            r.items_valued = created;
            r.valuation_total = money(section(set.base, "totals").get("inventory_valuation"));
        }
        Some(ImportKind::Items) => {
            r.items_skipped = Some(cl_api::imports::ItemsSkipped::default());
        }
        Some(ImportKind::GlTransactions) if history => r.skipped_before_history = Some(0),
        _ => {}
    }
    r
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

/// The tie-out of a set (E3 as built): the reference trial balance, plus
/// the differences the altered set's expectation names (books minus
/// source). Every amount is `EXPECTED.toml`'s: the trial balance columns
/// and the aging totals are its `[totals]`, an altered set's total
/// differences its `[totals_diff]`. The demo plays the engine, so it counts
/// the rows that differ (`differenceCount`), as the engine does.
fn tieout_of(set: &Set, conversion: Option<CalendarDate>) -> Result<TieOut, ApiError> {
    let reference = section(set.base, "reference_tb");
    let diffs = set.altered.map(|t| section(t, "diffs")).unwrap_or_default();
    let mut keys: Vec<&String> = reference.keys().chain(diffs.keys()).collect();
    keys.sort();
    keys.dedup();
    let zero = Money::ZERO;
    let mut accounts = Vec::new();
    for (row, key) in (2_u64..).zip(keys) {
        let source = money(reference.get(key)).unwrap_or(zero);
        let diff = money(diffs.get(key)).unwrap_or(zero);
        let books = source.checked_add(diff).map_err(|_| decode())?;
        let in_source = reference.contains_key(key);
        accounts.push(TieOutAccount {
            account: None,
            number: Some(key.clone()),
            name: String::new(),
            source_row: in_source.then_some(row),
            source_rows: if in_source { vec![row] } else { Vec::new() },
            source_keys: if in_source {
                vec![key.clone()]
            } else {
                Vec::new()
            },
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
        .map(|(name, key)| {
            let diff = money(balances.get(name)).unwrap_or(zero);
            BalanceRow {
                key,
                account: None,
                source: zero,
                books: diff,
                diff,
            }
        })
        .collect()
    } else {
        Vec::new()
    };
    let as_of = conversion
        .or_else(|| day(section(set.base, "company").get("conversion_date")?))
        .ok_or_else(decode)?;
    let item_diffs = set.altered.map(|t| section(t, "items")).unwrap_or_default();
    let items = |name: &str| -> Result<Vec<TieOutItem>, ApiError> {
        if !set.qbd {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        // The file's rows in the EXPECTED order (row 1 is the header).
        for (row, (key, value)) in (2_u64..).zip(section(set.base, name)) {
            let source = money(Some(&value)).ok_or_else(decode)?;
            let diff = money(item_diffs.get(&key)).unwrap_or(zero);
            out.push(TieOutItem {
                source_row: Some(row),
                session: None,
                transaction: None,
                name: String::new(),
                number: Some(key),
                doc_type: None,
                date: Some(as_of),
                source,
                books: source.checked_add(diff).map_err(|_| decode())?,
                diff,
            });
        }
        Ok(out)
    };
    let receivables = items("open_invoices")?;
    let payables = items("unpaid_bills")?;
    let mut inventory_items = Vec::new();
    if set.qbd {
        for (row, (key, value)) in (2_u64..).zip(section(set.base, "inventory_valuation")) {
            let source = money(Some(&value)).ok_or_else(decode)?;
            let diff = money(item_diffs.get(&key)).unwrap_or(zero);
            inventory_items.push(TieOutStock {
                source_row: Some(row),
                name: key,
                item: None,
                // EXPECTED.toml has values only: the demo shows no
                // quantity difference.
                source_quantity: rust_decimal::Decimal::ZERO,
                books_quantity: rust_decimal::Decimal::ZERO,
                quantity_diff: rust_decimal::Decimal::ZERO,
                source,
                books: source.checked_add(diff).map_err(|_| decode())?,
                diff,
            });
        }
    }
    let totals_of = section(set.base, "totals");
    let total_diffs = set
        .altered
        .map(|t| section(t, "totals_diff"))
        .unwrap_or_default();
    let side = |file: &str, key: &str, present: bool| -> Result<Option<TieOutTotal>, ApiError> {
        let Some(source) = money(totals_of.get(file)).filter(|_| present) else {
            return Ok(None);
        };
        let diff = money(total_diffs.get(key)).unwrap_or(zero);
        Ok(Some(TieOutTotal {
            source,
            books: source.checked_add(diff).map_err(|_| decode())?,
            diff,
        }))
    };
    let tb = money(totals_of.get("reference_tb_debits"));
    let totals = TieOutTotals {
        accounts: tb.map(|d| TrialBalanceTotals {
            source_debit: d,
            source_credit: d,
            books_debit: d,
            books_credit: d,
        }),
        ar: side("open_invoices", "ar", !receivables.is_empty())?,
        ap: side("unpaid_bills", "ap", !payables.is_empty())?,
        inventory: side(
            "inventory_valuation",
            "inventory",
            !inventory_items.is_empty(),
        )?,
    };
    let differing = accounts.iter().filter(|a| !a.diff.is_zero()).count()
        + receivables
            .iter()
            .chain(&payables)
            .filter(|i| !i.diff.is_zero())
            .count()
        + inventory_items.iter().filter(|i| !i.ties()).count()
        + balance_rows.iter().filter(|r| !r.diff.is_zero()).count()
        + [totals.ar, totals.ap, totals.inventory]
            .iter()
            .flatten()
            .filter(|x| !x.diff.is_zero())
            .count();
    let reference_tb = section(set.base, "files")
        .values()
        .any(|v| v.starts_with("trial_balance:reference"));
    Ok(TieOut {
        batch: None,
        as_of,
        computed_at: Some(COMMITTED_AT.into()),
        books: Some(TieOutBooks::Company),
        reference: reference_tb.then_some(ReferenceTrialBalance {
            session: None,
            report_date: Some(as_of),
        }),
        accounts,
        ar_items: receivables,
        ap_items: payables,
        inventory_items,
        balance_rows,
        totals,
        difference_count: u64::try_from(differing).unwrap_or(u64::MAX),
        all_zero: differing == 0,
        verified_at: None,
    })
}

#[cfg(test)]
#[path = "imports_tests.rs"]
mod tests;
