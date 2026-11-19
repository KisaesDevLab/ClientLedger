//! Spreadsheet transaction import (P31 D5; engine U-261 `imports-batch-v1`,
//! U-268 `imports-transactions-v1`).
//!
//! The desktop sends the owner's file as the canonical `transactions` CSV
//! (`cl_model::imports::csv_txn`), then one import batch holding that one
//! upload: commit (202, then the batch is polled), and undo the same way.
//! The engine posts every document of the batch in one transaction, so a
//! bad row posts nothing; undo removes everything the batch created.
//!
//! - Commit and undo take the screen's [`IdempotencyKey`] and are sent once
//!   ([`crate::client`] `once`): never again on their own. A lost answer is
//!   `OutcomeUnknown`; the screen reads the batch ("Check status") and only
//!   the owner's click sends again, with the same key.
//! - Both carry `backupRef`, the file name of the verified backup the
//!   desktop made just before (non-negotiable 9).
//! - Upload is refused for an already-imported file (409
//!   `IMPORT_ALREADY_COMMITTED`) and for the same file while an earlier
//!   upload of it is open (409 `IMPORT_SESSION_ACTIVE`: discard that one
//!   first); [`import_refusal`] reads both, and the commit's 409
//!   `IMPORT_HAS_ERRORS`.
//! - Row problems (U-268 codes) come back on the upload; nothing is
//!   created on import, so names, items, tags and accounts must exist.
//! - Nothing here reads an amount: the engine's answers carry codes, row
//!   numbers and counts, and the engine's `message` is never kept (it may
//!   embed names, plan §2a E3).
//!
//! The routes are P31 D1's shared rows in [`crate::OPERATIONS`] (the same
//! ones `resources/imports.rs` uses); the contract test runs on engines
//! with `imports-transactions-v1` (pin 1f2c9bb3 on).

use std::fmt;

use serde_json::{Value, json};

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::idempotency::IdempotencyKey;
use crate::resources::customer_payments::require;
use crate::resources::imports::{failure_from, row_problem_from};
use crate::resources::operations::{
    IMPORT_BATCH, IMPORT_BATCH_COMMIT, IMPORT_BATCH_CREATE, IMPORT_BATCH_UNDO, IMPORTS_DISCARD,
    IMPORTS_UPLOAD, Operation,
};
use crate::resources::posting::{ClosedPeriodAuthorization, PostingOptions, posting_body};
use crate::session::Session;
use crate::wire::Capabilities;

const WHAT: &str = "spreadsheet import";

/// The engine's upload limit (10 MB), checked before anything is sent.
pub const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

/// What the spreadsheet import screen needs, both.
pub const REQUIRED_CAPABILITIES: [&str; 2] = [
    Capabilities::IMPORTS_BATCH,
    Capabilities::IMPORTS_TRANSACTIONS,
];

/// True when the engine has everything the screen needs.
#[must_use]
pub fn transactions_supported(caps: &Capabilities) -> bool {
    REQUIRED_CAPABILITIES.iter().all(|c| caps.has(c))
}

fn require_all(s: &Session) -> Result<(), ApiError> {
    for c in REQUIRED_CAPABILITIES {
        require(s, c)?;
    }
    Ok(())
}

fn decode_err() -> ApiError {
    ApiError::Decode { what: WHAT }
}

// ---- Routes: the shared P31 rows in `operations.rs` (D1) -------------------

fn at(o: &Operation, id: &str) -> Call {
    Call::new(o.method(), o.route(), o.route().replace("{id}", id))
}

// ---- Types --------------------------------------------------------------------
//
// The ids, the batch status, the row problem, the batch failure, the undo
// blocker and the refusals are crate::imports's: one batch model for every
// import (P31 E4 adoption).

pub use crate::resources::imports::{
    BatchFailure, BatchStatus, ImportBatchId, ImportRefusal, ImportSessionId, Progress, RefProblem,
    RefReason, RowProblem, SessionStatus, UndoBlocker, import_refusal,
};
/// What an upload made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadOutcome {
    /// The upload.
    pub session: ImportSessionId,
    /// Data rows (lines) the engine read.
    pub total_rows: u64,
    /// Documents the engine found (`preview.documentCount`).
    pub documents: Option<u64>,
    /// Row problems it found (U-268 codes, the column, the file row with
    /// the header as row 1). Any one blocks the commit: 409
    /// `IMPORT_HAS_ERRORS` (or, for `IMPORT_UNKNOWN_REF`, a failed batch).
    pub problems: Vec<RowProblem>,
}

/// A batch as the engine has it now (the parts the spreadsheet screen
/// reads; `crate::imports::Imports::batch` reads it whole).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchState {
    /// The batch.
    pub id: ImportBatchId,
    /// Where it is.
    pub status: BatchStatus,
    /// Progress while running.
    pub progress: Progress,
    /// Why it failed (code, row, column; `IMPORT_UNKNOWN_REF` carries
    /// `details.refs`).
    pub error: Option<BatchFailure>,
    /// The backup file the commit named.
    pub backup_ref: Option<String>,
}
/// A commit or undo request.
#[derive(Clone, Default)]
pub struct BatchRunRequest {
    /// The verified backup made just before (its file name).
    pub backup_ref: String,
    /// The closing-date password, after a 423.
    pub closed_period: Option<ClosedPeriodAuthorization>,
}

/// No password, ever.
impl fmt::Debug for BatchRunRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchRunRequest")
            .field("closing_password", &self.closed_period.is_some())
            .finish_non_exhaustive()
    }
}

// ---- Parsing --------------------------------------------------------------------

fn parse_json(body: &[u8]) -> Result<Value, ApiError> {
    serde_json::from_slice(body).map_err(|_| decode_err())
}

pub(crate) fn upload_from(body: &[u8]) -> Result<UploadOutcome, ApiError> {
    let v = parse_json(body)?;
    let session = v["session"]["id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(decode_err)?;
    let problems = v["validationErrors"]
        .as_array()
        .map(|l| l.iter().filter_map(row_problem_from).collect())
        .unwrap_or_default();
    Ok(UploadOutcome {
        session,
        total_rows: v["preview"]["totalRows"]
            .as_u64()
            .or_else(|| v["session"]["rowCount"].as_u64())
            .unwrap_or(0),
        documents: v["preview"]["documentCount"].as_u64(),
        problems,
    })
}

pub(crate) fn batch_from(body: &[u8]) -> Result<BatchState, ApiError> {
    let v = parse_json(body)?;
    let b = &v["batch"];
    let id = b["id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(decode_err)?;
    let status = b["status"]
        .as_str()
        .and_then(BatchStatus::from_wire)
        .ok_or_else(decode_err)?;
    let p = &v["progress"];
    let err = if v["error"].is_object() {
        &v["error"]
    } else {
        &b["error"]
    };
    Ok(BatchState {
        id,
        status,
        progress: Progress {
            step: p["step"].as_str().unwrap_or_default().to_string(),
            done: p["done"].as_u64().unwrap_or(0),
            total: p["total"].as_u64().unwrap_or(0),
        },
        error: failure_from(err).filter(|e| !e.code.is_empty()),
        backup_ref: b["backupRef"].as_str().map(str::to_string),
    })
}
fn multipart(bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = format!("cl-import-{}", uuid::Uuid::new_v4().simple());
    let mut body = Vec::with_capacity(bytes.len() + 512);
    for (name, value) in [("kind", "transactions"), ("sourceSystem", "generic")] {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    // A neutral file name: the owner's never reaches the engine's history.
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"transactions.csv\"\r\nContent-Type: text/csv\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (boundary, body)
}

// ---- Calls ------------------------------------------------------------------------

/// Spreadsheet transaction import operations (`session.import_transactions()`).
#[derive(Clone, Copy, Debug)]
pub struct ImportTransactions<'a>(pub(crate) &'a Session);

impl Session {
    /// Spreadsheet transaction imports in the open company.
    #[must_use]
    pub fn import_transactions(&self) -> ImportTransactions<'_> {
        ImportTransactions(self)
    }
}

impl ImportTransactions<'_> {
    /// Uploads the canonical `transactions` CSV (checked first; nothing is
    /// sent when it is refused).
    ///
    /// # Errors
    /// `Local(Unsupported)` without the capabilities; `Local(AttachmentTooLarge)`
    /// over 10 MB; 409 `IMPORT_ALREADY_COMMITTED`; any [`ApiError`].
    pub async fn upload(&self, csv: &[u8]) -> Result<UploadOutcome, ApiError> {
        require_all(self.0)?;
        if csv.len() > MAX_UPLOAD_BYTES {
            return Err(ApiError::Local(
                crate::errors::LocalValidation::AttachmentTooLarge,
            ));
        }
        let (boundary, body) = multipart(csv);
        let call = Call::at(Method::Post, IMPORTS_UPLOAD.route()).multipart_body(&boundary, body);
        let reply = self.0.send(&IMPORTS_UPLOAD, call, None).await?;
        upload_from(&reply.body)
    }

    /// Discards an uncommitted upload.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn discard(&self, id: ImportSessionId) -> Result<(), ApiError> {
        require_all(self.0)?;
        let call = at(&IMPORTS_DISCARD, &id.to_string());
        self.0.send(&IMPORTS_DISCARD, call, None).await?;
        Ok(())
    }

    /// Makes the one-upload batch the commit runs on.
    ///
    /// # Errors
    /// 409 `IMPORT_SESSION_IN_BATCH`; any [`ApiError`].
    pub async fn create_batch(&self, session: ImportSessionId) -> Result<ImportBatchId, ApiError> {
        require_all(self.0)?;
        let body = json!({
            "sourceSystem": "generic",
            "kind": "transactions",
            "sessionIds": [session.to_string()],
            "options": {},
        });
        let bytes = serde_json::to_vec(&body).map_err(|_| decode_err())?;
        let call = Call::at(Method::Post, IMPORT_BATCH_CREATE.route()).json_body(bytes);
        let reply = self.0.send(&IMPORT_BATCH_CREATE, call, None).await?;
        let v = parse_json(&reply.body)?;
        v["batch"]["id"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .ok_or_else(decode_err)
    }

    /// Starts the commit (202): sent once with `key`; then poll
    /// [`ImportTransactions::batch`].
    ///
    /// # Errors
    /// `ClosedPeriod` (423) and the closing-password errors; 409
    /// `IMPORT_ALREADY_COMMITTED`, `IMPORT_BUSY`; 403
    /// `IMPORT_ADMIN_REQUIRED`; `OutcomeUnknown` when the answer was lost
    /// (read the batch).
    pub async fn commit(
        &self,
        id: ImportBatchId,
        req: &BatchRunRequest,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        self.run(&IMPORT_BATCH_COMMIT, id, req, key).await
    }

    /// Starts the undo (202), as for [`ImportTransactions::commit`].
    ///
    /// # Errors
    /// 409 `IMPORT_UNDO_BLOCKED`; the closed-period errors; as for commit.
    pub async fn undo(
        &self,
        id: ImportBatchId,
        req: &BatchRunRequest,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        self.run(&IMPORT_BATCH_UNDO, id, req, key).await
    }

    async fn run(
        &self,
        o: &Operation,
        id: ImportBatchId,
        req: &BatchRunRequest,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        require_all(self.0)?;
        if req.backup_ref.trim().is_empty() {
            // Non-negotiable 9: never without the backup made just before.
            return Err(ApiError::Local(
                crate::errors::LocalValidation::BackupRequired,
            ));
        }
        let body = json!({ "backupRef": req.backup_ref });
        let opts = PostingOptions {
            closed_period: req.closed_period.clone(),
        };
        let bytes = posting_body(self.0, body, &opts)?;
        let mut call = at(o, &id.to_string()).json_body(bytes);
        call.idempotency_key = Some(key);
        call.once = true;
        self.0.send(o, call, None).await?;
        Ok(())
    }

    /// The batch now (status, progress, error).
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn batch(&self, id: ImportBatchId) -> Result<BatchState, ApiError> {
        require_all(self.0)?;
        let call = at(&IMPORT_BATCH, &id.to_string());
        let reply = self.0.send(&IMPORT_BATCH, call, None).await?;
        batch_from(&reply.body)
    }
}

#[cfg(test)]
#[path = "import_transactions_tests.rs"]
mod tests;
