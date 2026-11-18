//! The demo engine's payroll import (P30 WP7): it answers every payroll
//! operation from the committed goldens (`fixtures/payroll/expected/`), so
//! the screen and its control-channel scenarios run with no engine.
//!
//! A file is recognised by comparing its bytes with the fixtures compiled
//! in (`include_bytes!`); any other file is refused as the engine refuses a
//! file it can't read. The refusals use the engine's codes (U-244): a
//! second post of the same file is 409 `PAYROLL_DUPLICATE_FILE`, an
//! overlapping period answers `NeedsConfirmation`, a posted import refuses
//! a second post (`PAYROLL_ALREADY_POSTED`), reversing an unposted one is
//! `PAYROLL_NOT_POSTED`, and a date on or before the closing date needs
//! the demo closing password.
//!
//! The demo never adds amounts from the file: entries are the goldens'
//! lines. Without a department map the demo merges the golden's department
//! lines per line type (the engine's U-246 "no map" output).

// M10: no floating point in payroll code (money is `rust_decimal`).
#![deny(clippy::float_arithmetic)]

use std::collections::BTreeMap;

use cl_api::payroll::{
    AccountMapping, ColumnMap, DescriptionMapping, ImportMode, Overlap, PayrollSessionId,
    PayrollTemplate, PostOutcome, Preview, PreviewEntry, PreviewLine, ReverseOutcome, SampleRows,
    SessionPage, SessionStatus, SessionSummary, UploadOutcome, UploadRequest, ValidationSummary,
};
use cl_api::{
    ApiError, ConflictDetails, ConflictReason, ErrorCode, IdempotencyKey, UserText, ValidationError,
};
use cl_core::ids::{AccountId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use serde_json::{Value, json};

/// A compiled-in fixture: its stem, bytes, golden and mode.
struct Fixture {
    stem: &'static str,
    bytes: &'static [u8],
    golden: &'static str,
    mode: ImportMode,
    header_row: usize,
}

macro_rules! fixture {
    ($stem:literal, $mode:expr, $header:expr) => {
        Fixture {
            stem: $stem,
            bytes: include_bytes!(concat!("../../../../../fixtures/payroll/", $stem, ".csv")),
            golden: include_str!(concat!(
                "../../../../../fixtures/payroll/expected/",
                $stem,
                ".json"
            )),
            mode: $mode,
            header_row: $header,
        }
    };
}

const FIXTURES: [Fixture; 8] = [
    fixture!("gusto", ImportMode::EmployeeLevel, 0),
    fixture!("adp_run_gli", ImportMode::PrebuiltJe, 0),
    fixture!("quickbooks_payroll", ImportMode::EmployeeLevel, 3),
    fixture!("paychex_flex_gls", ImportMode::PrebuiltJe, 0),
    fixture!("square", ImportMode::EmployeeLevel, 0),
    fixture!("onpay_gl_summary", ImportMode::PrebuiltJe, 0),
    fixture!("generic_period1", ImportMode::EmployeeLevel, 0),
    fixture!("generic_period2", ImportMode::EmployeeLevel, 0),
];

/// One golden line.
#[derive(Clone, Debug)]
struct GoldenLine {
    line_type: String,
    department: Option<String>,
    debit: bool,
    amount: Money,
}

/// One golden entry.
#[derive(Clone, Debug)]
struct GoldenEntry {
    date: CalendarDate,
    lines: Vec<GoldenLine>,
    total: Money,
}

fn golden_of(f: &Fixture) -> (String, Vec<GoldenEntry>) {
    // Invariant: the goldens are committed files checked by
    // `cl-api/tests/payroll_fixtures.rs`; a parse failure is a broken build.
    let v: Value = serde_json::from_str(f.golden).unwrap_or(Value::Null);
    let provider = v["provider"].as_str().unwrap_or("custom").to_string();
    let entries = v["entries"]
        .as_array()
        .map(|l| {
            l.iter()
                .filter_map(|e| {
                    Some(GoldenEntry {
                        date: CalendarDate::parse_wire(e["date"].as_str()?).ok()?,
                        total: Money::parse_wire(e["totalDebits"].as_str()?).ok()?,
                        lines: e["lines"]
                            .as_array()?
                            .iter()
                            .filter_map(|l| {
                                Some(GoldenLine {
                                    line_type: l["lineType"].as_str()?.to_string(),
                                    department: l["department"].as_str().map(str::to_string),
                                    debit: l["side"].as_str()? == "debit",
                                    amount: Money::parse_wire(l["amount"].as_str()?).ok()?,
                                })
                            })
                            .collect(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    (provider, entries)
}

/// The cells of one CSV line (quotes and doubled quotes; no newlines in
/// cells, as in every fixture).
fn cells(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            '\r' => {}
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// One demo import.
#[derive(Clone, Debug)]
struct DemoSession {
    id: PayrollSessionId,
    fixture: usize,
    status: SessionStatus,
    period: (CalendarDate, CalendarDate),
    check_date: CalendarDate,
    entries: Vec<TransactionId>,
    discarded: bool,
    /// The key and outcome of the post that posted it (a replay answers
    /// the same).
    posted_with: Option<IdempotencyKey>,
    reversed_with: Option<IdempotencyKey>,
}

/// The demo's payroll imports, layouts and maps.
#[derive(Clone, Debug, Default)]
pub struct DemoPayroll {
    sessions: Vec<DemoSession>,
    templates: Vec<PayrollTemplate>,
    accounts: BTreeMap<String, AccountId>,
    descriptions: BTreeMap<(String, String), (AccountId, Option<TagId>)>,
    next: u64,
    /// Posts and reverses received (control channel: never sent twice).
    pub posts_received: u32,
    /// Discards received (B1: none while a post is out or unconfirmed).
    pub discards_received: u32,
    /// The before-post backups the demo "made" (Q-P30h; no files).
    pub backups_taken: u32,
    /// How the next before-post backup answers (scenarios and tests).
    pub backup_mode: DemoBackup,
}

/// How the demo answers the next before-post backup (Q-P30h). The
/// one-shot modes go back to [`DemoBackup::Succeed`] once used.
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

/// The demo's run key for a fixture (one pay run per fixture).
fn demo_run_key(fixture: usize) -> String {
    format!("demo-run-{fixture}")
}

/// The demo's file hash for a fixture.
fn demo_file_hash(fixture: usize) -> String {
    format!("demo-file-{fixture}")
}

fn uuid(n: u64, kind: u8) -> uuid::Uuid {
    uuid::Uuid::from_u128(
        (u128::from(kind) << 64) | u128::from(n) | 0x0d30_0000_0000_4000_8000_0000_0000_0000,
    )
}

impl DemoPayroll {
    fn id(&mut self, kind: u8) -> uuid::Uuid {
        self.next += 1;
        uuid(self.next, kind)
    }

    fn session(&self, id: PayrollSessionId) -> Result<&DemoSession, ApiError> {
        self.sessions
            .iter()
            .find(|s| s.id == id)
            .ok_or(ApiError::NotFound { code: None })
    }

    fn session_mut(&mut self, id: PayrollSessionId) -> Result<&mut DemoSession, ApiError> {
        self.sessions
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(ApiError::NotFound { code: None })
    }

    /// Forgets every import, layout and map (scenario reset).
    pub fn reset(&mut self) {
        *self = DemoPayroll::default();
    }

    /// The before-post backup (Q-P30h), simulated: no file is written.
    /// `Some(Ok)` made, `Some(Err)` failed, `None` still running.
    pub fn backup(&mut self) -> Option<Result<(), ()>> {
        match std::mem::take(&mut self.backup_mode) {
            DemoBackup::Succeed => {
                self.backups_taken += 1;
                Some(Ok(()))
            }
            DemoBackup::FailOnce => Some(Err(())),
            DemoBackup::HoldOnce => None,
        }
    }

    /// Uploads: the file must be one of the fixtures.
    ///
    /// # Errors
    /// A local check's refusal; `Validation` for a file the demo can't read.
    pub fn upload(&mut self, req: &UploadRequest) -> Result<UploadOutcome, ApiError> {
        req.check().map_err(ApiError::Local)?;
        let Some(fixture) = FIXTURES
            .iter()
            .position(|f| f.bytes == req.bytes.as_slice())
        else {
            return Err(refused("PAYROLL_FILE_UNREADABLE", json!({})));
        };
        let f = &FIXTURES[fixture];
        let text = String::from_utf8_lossy(f.bytes);
        let lines: Vec<&str> = text.lines().collect();
        let headers = cells(lines.get(f.header_row).copied().unwrap_or_default());
        let samples: Vec<Vec<String>> = lines
            .iter()
            .skip(f.header_row + 1)
            .take(3)
            .map(|l| cells(l))
            .collect();
        let (provider, _) = golden_of(f);
        let posted_same = self
            .sessions
            .iter()
            .any(|s| s.fixture == fixture && s.status == SessionStatus::Posted);
        let id = PayrollSessionId::parse(&self.id(1).to_string())?;
        self.sessions.push(DemoSession {
            id,
            fixture,
            status: SessionStatus::Uploaded,
            period: (req.period_start, req.period_end),
            check_date: req.check_date,
            entries: Vec::new(),
            discarded: false,
            posted_with: None,
            reversed_with: None,
        });
        Ok(UploadOutcome {
            session: id,
            row_count: u64::try_from(lines.len().saturating_sub(f.header_row + 1)).unwrap_or(0),
            headers,
            samples: SampleRows(samples),
            detected_provider: (provider != "custom").then_some(provider),
            mode: Some(f.mode),
            is_duplicate: posted_same,
            is_duplicate_by_key: false,
            run_key: Some(demo_run_key(fixture)),
            file_hash: Some(demo_file_hash(fixture)),
        })
    }

    /// Maps the columns: every required field must name a column of the file.
    ///
    /// # Errors
    /// `Validation` `MISSING_REQUIRED` naming the field.
    pub fn apply_mapping(&mut self, id: PayrollSessionId, map: &ColumnMap) -> Result<(), ApiError> {
        let s = self.session(id)?;
        let f = &FIXTURES[s.fixture];
        let text = String::from_utf8_lossy(f.bytes);
        let headers = cells(text.lines().nth(f.header_row).unwrap_or_default());
        for field in ["employee_name", "gross_pay", "net_pay"] {
            let ok = map
                .mappings
                .get(field)
                .is_some_and(|c| headers.iter().any(|h| h.trim() == c.source.trim()));
            if !ok {
                return Err(refused("MISSING_REQUIRED", json!({"field": field})));
            }
        }
        self.session_mut(id)?.status = SessionStatus::Mapped;
        Ok(())
    }

    /// A ready-made file's descriptions with their saved accounts.
    ///
    /// # Errors
    /// `NotFound`.
    pub fn description_map(
        &self,
        id: PayrollSessionId,
    ) -> Result<Vec<DescriptionMapping>, ApiError> {
        let s = self.session(id)?;
        let (provider, entries) = golden_of(&FIXTURES[s.fixture]);
        let mut seen = Vec::new();
        for e in &entries {
            for l in &e.lines {
                if seen
                    .iter()
                    .any(|m: &DescriptionMapping| m.description == l.line_type)
                {
                    continue;
                }
                let saved = self
                    .descriptions
                    .get(&(provider.clone(), l.line_type.clone()));
                seen.push(DescriptionMapping {
                    description: l.line_type.clone(),
                    side: Some(if l.debit { "debit" } else { "credit" }.into()),
                    account: saved.map(|s| s.0),
                    tag: saved.and_then(|s| s.1),
                    suggested: false,
                });
            }
        }
        Ok(seen)
    }

    /// Saves a ready-made file's descriptions (per provider).
    pub fn save_description_map(&mut self, provider: &str, mappings: &[DescriptionMapping]) {
        for m in mappings {
            if let Some(a) = m.account {
                self.descriptions
                    .insert((provider.to_string(), m.description.clone()), (a, m.tag));
            }
        }
    }

    /// The row check (the fixtures have no row problems).
    ///
    /// # Errors
    /// `NotFound`.
    pub fn validate(&mut self, id: PayrollSessionId) -> Result<ValidationSummary, ApiError> {
        let s = self.session_mut(id)?;
        s.status = SessionStatus::Validated;
        let f = &FIXTURES[s.fixture];
        let rows = String::from_utf8_lossy(f.bytes)
            .lines()
            .count()
            .saturating_sub(f.header_row + 1);
        // `payroll-import-review-v1` (S3): every department in the file
        // (the golden's, which are the file's).
        let departments = (f.mode == ImportMode::EmployeeLevel).then(|| {
            let (_, golden) = golden_of(f);
            golden
                .iter()
                .flat_map(|e| e.lines.iter().filter_map(|l| l.department.clone()))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect()
        });
        Ok(ValidationSummary {
            total_rows: u64::try_from(rows).unwrap_or(0),
            departments,
            ..ValidationSummary::default()
        })
    }

    fn account_for(&self, provider: &str, line_type: &str, mode: ImportMode) -> Option<AccountId> {
        match mode {
            ImportMode::EmployeeLevel => self.accounts.get(line_type).copied(),
            ImportMode::PrebuiltJe => self
                .descriptions
                .get(&(provider.to_string(), line_type.to_string()))
                .map(|s| s.0),
        }
    }

    /// The entries: the golden's, with the company's accounts and tags.
    ///
    /// # Errors
    /// `NotFound`.
    pub fn preview(
        &self,
        id: PayrollSessionId,
        departments: &BTreeMap<String, Option<TagId>>,
        names: &dyn Fn(AccountId) -> Option<String>,
    ) -> Result<Preview, ApiError> {
        let s = self.session(id)?;
        let f = &FIXTURES[s.fixture];
        let (provider, golden) = golden_of(f);
        let mut entries = Vec::new();
        for e in &golden {
            let mut lines: Vec<PreviewLine> = Vec::new();
            for l in &e.lines {
                let department = if departments.is_empty() {
                    None
                } else {
                    l.department.clone()
                };
                if let Some(same) = lines.iter_mut().find(|x| {
                    departments.is_empty()
                        && x.line_type == l.line_type
                        && f.mode == ImportMode::EmployeeLevel
                }) {
                    // No department map: one line per line type (U-246).
                    // M4: an overflow is refused as the engine refuses a
                    // bad amount, never hidden.
                    let side = if l.debit {
                        &mut same.debit
                    } else {
                        &mut same.credit
                    };
                    *side = side
                        .checked_add(l.amount)
                        .map_err(|_| refused("PAYROLL_AMOUNT_INVALID", json!({})))?;
                    continue;
                }
                let account = self.account_for(&provider, &l.line_type, f.mode);
                let tag = match f.mode {
                    ImportMode::EmployeeLevel => department
                        .as_ref()
                        .and_then(|d| departments.get(d).copied().flatten()),
                    ImportMode::PrebuiltJe => self
                        .descriptions
                        .get(&(provider.clone(), l.line_type.clone()))
                        .and_then(|s| s.1),
                };
                lines.push(PreviewLine {
                    description: description_of(&l.line_type),
                    line_type: l.line_type.clone(),
                    account,
                    account_name: account.and_then(names),
                    account_number: None,
                    department,
                    debit: if l.debit { l.amount } else { Money::ZERO },
                    credit: if l.debit { Money::ZERO } else { l.amount },
                    tag,
                });
            }
            // M4: the memo names the service as the owner knows it.
            let shown = cl_model::payroll_import::Provider::from_engine_key(&provider).map_or_else(
                || provider.clone(),
                |p| crate::i18n::t(&format!("payroll.provider.{}", p.key())),
            );
            entries.push(PreviewEntry {
                date: e.date,
                memo: format!(
                    "Payroll {}–{} — {} — pay date {}",
                    s.period.0.to_wire(),
                    s.period.1.to_wire(),
                    shown,
                    e.date.to_wire()
                ),
                lines,
                total_debits: e.total,
                total_credits: e.total,
                difference: Money::ZERO,
                is_balanced: true,
            });
        }
        Ok(Preview {
            entries,
            token: Some(format!("demo-{}-{}", s.id, departments.len())),
        })
    }

    /// Posts: the engine's checks in the engine's order.
    ///
    /// # Errors
    /// The engine's 409/422 refusals, `ClosedPeriod`, `ClosingPasswordRejected`.
    #[allow(clippy::too_many_arguments)]
    pub fn post(
        &mut self,
        id: PayrollSessionId,
        force: bool,
        key: &IdempotencyKey,
        password: Option<&Secret>,
        closing: Option<CalendarDate>,
    ) -> Result<PostOutcome, ApiError> {
        self.posts_received += 1;
        let s = self.session(id)?.clone();
        if s.posted_with.as_ref() == Some(key) {
            return Ok(PostOutcome::Posted {
                entries: s.entries,
                replayed: true,
            });
        }
        if s.status == SessionStatus::Posted {
            return Err(conflict(
                "PAYROLL_ALREADY_POSTED",
                json!({"status": "posted"}),
            ));
        }
        if s.status != SessionStatus::Validated {
            return Err(conflict(
                "PAYROLL_NOT_READY",
                json!({"status": s.status.as_wire()}),
            ));
        }
        let f = &FIXTURES[s.fixture];
        let (provider, golden) = golden_of(f);
        let unmapped: Vec<String> = golden
            .iter()
            .flat_map(|e| e.lines.iter())
            .filter(|l| self.account_for(&provider, &l.line_type, f.mode).is_none())
            .map(|l| l.line_type.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if !unmapped.is_empty() {
            return Err(refused("PAYROLL_UNMAPPED", json!({"lineTypes": unmapped})));
        }
        if let Some(prior) = self
            .sessions
            .iter()
            .find(|p| p.id != id && p.fixture == s.fixture && p.status == SessionStatus::Posted)
        {
            return Err(conflict(
                "PAYROLL_DUPLICATE_FILE",
                json!({"priorSessionId": prior.id.to_string(), "postedAt": format!("{}T12:00:00.000Z", prior.check_date.to_wire())}),
            ));
        }
        let dates: Vec<CalendarDate> = golden.iter().map(|e| e.date).collect();
        if !force {
            let overlaps: Vec<Overlap> = self
                .sessions
                .iter()
                .filter(|p| p.id != id && p.status == SessionStatus::Posted)
                .filter(|p| p.period.0 <= s.period.1 && s.period.0 <= p.period.1)
                .map(|p| Overlap {
                    session: Some(p.id),
                    period: Some(p.period),
                    check_date: None,
                    matching_dates: Vec::new(),
                    posted_on: Some(p.check_date.to_wire()),
                })
                .collect();
            if !overlaps.is_empty() {
                return Ok(PostOutcome::NeedsConfirmation { overlaps });
            }
        }
        super::register::check_closing(closing, &dates, password)?;
        let mut entries = Vec::new();
        for _ in &dates {
            entries.push(TransactionId::from_uuid(self.id(2)));
        }
        let me = self.session_mut(id)?;
        me.status = SessionStatus::Posted;
        me.entries.clone_from(&entries);
        me.discarded = true;
        me.posted_with = Some(key.clone());
        Ok(PostOutcome::Posted {
            entries,
            replayed: false,
        })
    }

    /// Reverses a posted import.
    ///
    /// # Errors
    /// 409 `PAYROLL_NOT_POSTED`; the closed-period errors.
    pub fn reverse(
        &mut self,
        id: PayrollSessionId,
        key: &IdempotencyKey,
        password: Option<&Secret>,
        closing: Option<CalendarDate>,
    ) -> Result<ReverseOutcome, ApiError> {
        self.posts_received += 1;
        let s = self.session(id)?.clone();
        if s.reversed_with.as_ref() == Some(key) {
            return Ok(ReverseOutcome {
                reversed: u64::try_from(s.entries.len()).unwrap_or(0),
                entries: s.entries,
            });
        }
        if s.status != SessionStatus::Posted {
            return Err(conflict(
                "PAYROLL_NOT_POSTED",
                json!({"status": s.status.as_wire()}),
            ));
        }
        let (_, golden) = golden_of(&FIXTURES[s.fixture]);
        let dates: Vec<CalendarDate> = golden.iter().map(|e| e.date).collect();
        super::register::check_closing(closing, &dates, password)?;
        let me = self.session_mut(id)?;
        me.status = SessionStatus::Cancelled;
        me.reversed_with = Some(key.clone());
        Ok(ReverseOutcome {
            reversed: u64::try_from(me.entries.len()).unwrap_or(0),
            entries: me.entries.clone(),
        })
    }

    /// Discards an unposted import.
    ///
    /// # Errors
    /// `Validation` for a posted one.
    pub fn discard(&mut self, id: PayrollSessionId) -> Result<(), ApiError> {
        self.discards_received += 1;
        if self.session(id)?.status == SessionStatus::Posted {
            return Err(refused("PAYROLL_POSTED", json!({})));
        }
        self.sessions.retain(|s| s.id != id);
        Ok(())
    }

    /// History, newest first.
    #[must_use]
    pub fn sessions(&self, status: Option<SessionStatus>) -> SessionPage {
        let sessions: Vec<SessionSummary> = self
            .sessions
            .iter()
            .rev()
            .filter(|s| status.is_none_or(|w| s.status == w))
            .map(|s| SessionSummary {
                id: s.id,
                status: s.status,
                mode: Some(FIXTURES[s.fixture].mode),
                provider: Some(golden_of(&FIXTURES[s.fixture]).0),
                period_start: Some(s.period.0),
                period_end: Some(s.period.1),
                check_date: Some(s.check_date),
                row_count: 0,
                error_count: 0,
                entries: s.entries.clone(),
                detail_discarded: s.discarded,
                // As the duplicate refusal names it (noon UTC on the pay
                // date: the same local day here).
                posted_at: (s.status == SessionStatus::Posted)
                    .then(|| format!("{}T12:00:00.000Z", s.check_date.to_wire())),
                run_key: Some(demo_run_key(s.fixture)),
                file_hash: Some(demo_file_hash(s.fixture)),
            })
            .collect();
        SessionPage {
            total: u64::try_from(sessions.len()).unwrap_or(0),
            sessions,
        }
    }

    /// Unposted drafts (Home's count).
    #[must_use]
    pub fn unposted(&self) -> u64 {
        u64::try_from(
            self.sessions
                .iter()
                .filter(|s| s.status.is_unposted_draft())
                .count(),
        )
        .unwrap_or(0)
    }

    /// The company's layouts.
    #[must_use]
    pub fn templates(&self) -> Vec<PayrollTemplate> {
        self.templates.clone()
    }

    /// Drops every layout's left-out columns (scenarios: a layout saved
    /// before they were kept).
    pub fn forget_left_out(&mut self) {
        for t in &mut self.templates {
            if let Some(m) = &mut t.column_map {
                m.left_out = None;
            }
        }
    }

    /// Saves or changes a layout.
    pub fn save_template(
        &mut self,
        existing: Option<uuid::Uuid>,
        name: &str,
        provider_key: &str,
        map: &ColumnMap,
        updated_at: String,
    ) -> PayrollTemplate {
        let id = existing.unwrap_or_else(|| self.id(3));
        self.templates.retain(|t| t.id != id);
        let t = PayrollTemplate {
            id,
            name: name.to_string(),
            provider_key: provider_key.to_string(),
            is_system: false,
            column_map: Some(map.clone()),
            updated_at: Some(updated_at),
        };
        self.templates.push(t.clone());
        t
    }

    /// The saved line type → account map.
    #[must_use]
    pub fn account_mappings(&self) -> Vec<AccountMapping> {
        self.accounts
            .iter()
            .map(|(k, a)| AccountMapping {
                line_type: k.clone(),
                account: Some(*a),
            })
            .collect()
    }

    /// Saves the line type → account map.
    pub fn save_account_mappings(&mut self, map: &BTreeMap<String, AccountId>) {
        self.accounts
            .extend(map.iter().map(|(k, v)| (k.clone(), *v)));
    }
}

/// The engine's description of a line type (Mode B: the description).
fn description_of(line_type: &str) -> String {
    match line_type {
        "gross_wages_expense" => "Gross Wages Expense",
        "employer_tax_expense" => "Employer Payroll Tax Expense",
        "employer_benefits_expense" => "Employer Benefits Expense",
        "reimbursement_expense" => "Employee Reimbursements",
        "fit_payable" => "Federal Income Tax Payable",
        "sit_payable" => "State/Local Tax Payable",
        "ss_payable" => "Social Security Payable",
        "medicare_payable" => "Medicare Payable",
        "futa_payable" => "FUTA Payable",
        "suta_payable" => "SUTA Payable",
        "health_ins_payable" => "Health Insurance Payable",
        "retirement_payable" => "Retirement (401k) Payable",
        "other_deduction_payable" => "Other Deductions Payable",
        "other_tax_payable" => "Other Payroll Taxes Payable",
        "payroll_clearing" => "Payroll Clearing / Cash",
        other => other,
    }
    .to_string()
}

/// The fixture bytes for a stem (scenarios pick files by stem).
#[must_use]
pub fn fixture_bytes(stem: &str) -> Option<&'static [u8]> {
    FIXTURES.iter().find(|f| f.stem == stem).map(|f| f.bytes)
}

#[cfg(test)]
#[path = "payroll_tests.rs"]
mod tests;
