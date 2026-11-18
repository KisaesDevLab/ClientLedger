//! The demo's spreadsheet transaction import (P31 D5): answers
//! `cl_api::import_transactions` as the engine does (U-261, U-268), so the
//! screen works without one.
//!
//! - An upload reads the canonical CSV: a line account the sample chart
//!   doesn't have is `IMPORT_UNKNOWN_REF` on its row.
//! - A commit or undo answers 202 and runs over a few polls (`committing`,
//!   `undoing`), so the running state shows.
//! - The same file again is 409 `IMPORT_ALREADY_COMMITTED` until undone.
//! - The closing date asks for the demo's closing password.
//! - Developer switches lose the next commit's answer (it still runs) and
//!   fail the next backup.

use std::collections::BTreeMap;

use cl_api::import_transactions::{
    BatchError, BatchProgress, BatchState, BatchStatus, EngineRowProblem, ImportBatchId,
    ImportSessionId, UploadOutcome,
};
use cl_api::{ApiError, ConflictDetails, ConflictReason, ErrorCode, IdempotencyKey, UserText};
use cl_core::{CalendarDate, Secret};
use serde_json::{Value, json};
use sha2::Digest as _;

/// Polls a demo commit or undo runs for.
pub const RUN_POLLS: u8 = 2;

/// The good and the bad fixture (scenarios and the developer menu).
#[must_use]
pub fn fixture_bytes(stem: &str) -> Option<&'static [u8]> {
    match stem {
        "good" => Some(include_bytes!(
            "../../../../../fixtures/import_transactions/good.csv"
        )),
        "bad_rows" => Some(include_bytes!(
            "../../../../../fixtures/import_transactions/bad-rows.csv"
        )),
        _ => None,
    }
}

#[derive(Clone)]
struct Upload {
    hash: String,
    dates: Vec<CalendarDate>,
    documents: u64,
}

#[derive(Clone)]
struct Batch {
    session: ImportSessionId,
    status: BatchStatus,
    polls_left: u8,
    commit_key: Option<IdempotencyKey>,
    undo_key: Option<IdempotencyKey>,
    backup_ref: Option<String>,
}

/// The demo's imports.
#[derive(Clone, Default)]
pub struct DemoTxnImport {
    uploads: BTreeMap<ImportSessionId, Upload>,
    batches: BTreeMap<ImportBatchId, Batch>,
    /// File hash → the batch that imported it (while not undone).
    imported: BTreeMap<String, ImportBatchId>,
    /// Commits the demo received (tests: "sent once").
    pub commits_received: u32,
    /// Undos the demo received.
    pub undos_received: u32,
    /// Backups made before a commit or undo.
    pub backups_made: u32,
    /// Lose the next commit's answer (it still runs).
    pub lose_next_answer: bool,
    /// Fail the next backup.
    pub fail_next_backup: bool,
    /// Documents posted by committed batches (Home and tests).
    pub posted_documents: u64,
}

/// Counts only (no file content).
impl std::fmt::Debug for DemoTxnImport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DemoTxnImport")
            .field("uploads", &self.uploads.len())
            .field("batches", &self.batches.len())
            .finish_non_exhaustive()
    }
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

fn not_found() -> ApiError {
    ApiError::NotFound {
        code: ErrorCode::parse("IMPORT_BATCH_NOT_FOUND"),
    }
}

impl DemoTxnImport {
    /// Forgets everything (scenarios run light and dark in one app).
    pub fn reset(&mut self) {
        *self = DemoTxnImport::default();
    }

    /// The verified backup before a commit or undo: its file name.
    ///
    /// # Errors
    /// `()` when the developer switch fails it.
    pub fn backup(&mut self, company: &str) -> Result<String, ()> {
        if std::mem::take(&mut self.fail_next_backup) {
            return Err(());
        }
        self.backups_made += 1;
        Ok(cl_company::backup::backup_file_name(
            company,
            super::DemoState::now(),
            crate::screens::csv_transactions::import_backup_kind(),
        ))
    }

    /// Reads an upload.
    ///
    /// # Errors
    /// 409 `IMPORT_ALREADY_COMMITTED` for a file imported and not undone.
    pub fn upload(&mut self, csv: &[u8], accounts: &[String]) -> Result<UploadOutcome, ApiError> {
        let hash: String = sha2::Sha256::digest(csv)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if let Some(b) = self.imported.get(&hash) {
            return Err(conflict(
                "IMPORT_ALREADY_COMMITTED",
                json!({"batchId": b.to_string(), "committedAt": "2026-10-27T12:00:00.000Z"}),
            ));
        }
        let text = cl_import::csv::decode(csv);
        let rows = cl_import::csv::parse(&text).unwrap_or_default();
        let mut problems = Vec::new();
        let mut dates = Vec::new();
        let mut groups = std::collections::BTreeSet::new();
        for (i, r) in rows.iter().enumerate().skip(1) {
            let cell = |n: usize| r.fields.get(n).map_or("", |s| s.trim());
            groups.insert(cell(0).to_string());
            if let Ok(d) = CalendarDate::parse_wire(cell(2)) {
                dates.push(d);
            }
            let account = cell(7);
            if !account.is_empty() && !accounts.iter().any(|a| a.eq_ignore_ascii_case(account)) {
                problems.push(EngineRowProblem {
                    row: u64::try_from(i + 1).unwrap_or(0),
                    field: Some("line_account".into()),
                    code: "IMPORT_UNKNOWN_REF".into(),
                });
            }
        }
        let id = ImportSessionId::new_v4();
        self.uploads.insert(
            id,
            Upload {
                hash,
                dates,
                documents: u64::try_from(groups.len()).unwrap_or(0),
            },
        );
        Ok(UploadOutcome {
            session: id,
            total_rows: u64::try_from(rows.len().saturating_sub(1)).unwrap_or(0),
            problems,
        })
    }

    /// Discards an upload.
    pub fn discard(&mut self, id: ImportSessionId) {
        self.uploads.remove(&id);
    }

    /// Makes a draft batch.
    ///
    /// # Errors
    /// `NotFound` for an unknown upload; 409 `IMPORT_SESSION_IN_BATCH`.
    pub fn create_batch(&mut self, session: ImportSessionId) -> Result<ImportBatchId, ApiError> {
        if !self.uploads.contains_key(&session) {
            return Err(not_found());
        }
        if self.batches.values().any(|b| b.session == session) {
            return Err(conflict("IMPORT_SESSION_IN_BATCH", json!({})));
        }
        let id = ImportBatchId::new_v4();
        self.batches.insert(
            id,
            Batch {
                session,
                status: BatchStatus::Draft,
                polls_left: 0,
                commit_key: None,
                undo_key: None,
                backup_ref: None,
            },
        );
        Ok(id)
    }

    /// Starts a commit (202).
    ///
    /// # Errors
    /// The closed-period errors; 409 `IMPORT_ALREADY_COMMITTED`,
    /// `IMPORT_BUSY`; `OutcomeUnknown` when the developer switch loses the
    /// answer (the commit still runs).
    pub fn commit(
        &mut self,
        id: ImportBatchId,
        backup_ref: &str,
        key: &IdempotencyKey,
        password: Option<&Secret>,
        closing: Option<CalendarDate>,
    ) -> Result<(), ApiError> {
        self.commits_received += 1;
        let b = self.batches.get(&id).ok_or_else(not_found)?;
        if b.commit_key.as_ref() == Some(key) {
            return Ok(()); // the same key replays the 202
        }
        match b.status {
            BatchStatus::Draft | BatchStatus::Failed => {}
            BatchStatus::Committing | BatchStatus::Undoing => {
                return Err(conflict("IMPORT_BUSY", json!({})));
            }
            BatchStatus::Committed | BatchStatus::Undone => {
                return Err(conflict(
                    "IMPORT_ALREADY_COMMITTED",
                    json!({"batchId": id.to_string()}),
                ));
            }
        }
        let up = self.uploads.get(&b.session).ok_or_else(not_found)?;
        if let Some(prior) = self.imported.get(&up.hash) {
            return Err(conflict(
                "IMPORT_ALREADY_COMMITTED",
                json!({"batchId": prior.to_string()}),
            ));
        }
        super::register::check_closing(closing, &up.dates, password)?;
        let b = self.batches.get_mut(&id).ok_or_else(not_found)?;
        b.status = BatchStatus::Committing;
        b.polls_left = RUN_POLLS;
        b.commit_key = Some(key.clone());
        b.backup_ref = Some(backup_ref.to_string());
        if std::mem::take(&mut self.lose_next_answer) {
            return Err(ApiError::OutcomeUnknown {
                request_id: cl_core::ids::RequestId::from_uuid(uuid::Uuid::new_v4()),
                idempotency_key: Some(key.clone()),
                keyed: true,
            });
        }
        Ok(())
    }

    /// Starts an undo (202).
    ///
    /// # Errors
    /// The closed-period errors; 409 `IMPORT_BUSY`; `NotFound`.
    pub fn undo(
        &mut self,
        id: ImportBatchId,
        key: &IdempotencyKey,
        password: Option<&Secret>,
        closing: Option<CalendarDate>,
    ) -> Result<(), ApiError> {
        self.undos_received += 1;
        let b = self.batches.get(&id).ok_or_else(not_found)?;
        if b.undo_key.as_ref() == Some(key) {
            return Ok(());
        }
        if b.status != BatchStatus::Committed {
            return Err(conflict("IMPORT_BUSY", json!({})));
        }
        let dates = self
            .uploads
            .get(&b.session)
            .map(|u| u.dates.clone())
            .unwrap_or_default();
        super::register::check_closing(closing, &dates, password)?;
        let b = self.batches.get_mut(&id).ok_or_else(not_found)?;
        b.status = BatchStatus::Undoing;
        b.polls_left = RUN_POLLS;
        b.undo_key = Some(key.clone());
        Ok(())
    }

    /// The batch now; a running one moves a step each poll.
    ///
    /// # Errors
    /// `NotFound`.
    pub fn batch(&mut self, id: ImportBatchId) -> Result<BatchState, ApiError> {
        let b = self.batches.get_mut(&id).ok_or_else(not_found)?;
        let up = self.uploads.get(&b.session);
        let total = up.map_or(0, |u| u.documents);
        if b.status.running() {
            if b.polls_left > 0 {
                b.polls_left -= 1;
            } else if b.status == BatchStatus::Committing {
                b.status = BatchStatus::Committed;
                if let Some(u) = up {
                    self.imported.insert(u.hash.clone(), id);
                }
                self.posted_documents += total;
            } else {
                b.status = BatchStatus::Undone;
                b.commit_key = None;
                if let Some(u) = up {
                    self.imported.remove(&u.hash);
                }
                self.posted_documents = self.posted_documents.saturating_sub(total);
            }
        }
        let done = match b.status {
            BatchStatus::Committed | BatchStatus::Undone => total,
            BatchStatus::Committing | BatchStatus::Undoing => {
                total.saturating_sub(u64::from(b.polls_left) * total / u64::from(RUN_POLLS + 1))
            }
            BatchStatus::Draft | BatchStatus::Failed => 0,
        };
        Ok(BatchState {
            id,
            status: b.status,
            progress: BatchProgress {
                step: Some("posting".into()),
                done,
                total,
            },
            error: None::<BatchError>,
            backup_ref: b.backup_ref.clone(),
        })
    }
}
