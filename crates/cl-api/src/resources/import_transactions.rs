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
//!   `IMPORT_ALREADY_COMMITTED`); [`txn_import_refusal`] reads it.
//! - Nothing here reads an amount: the engine's answers carry codes, row
//!   numbers and counts, and the engine's `message` is never kept (it may
//!   embed names, plan §2a E3).
//!
//! The routes are P31 D1's shared rows in [`crate::OPERATIONS`] (the same
//! ones `resources/imports.rs` uses); the contract test is pending by
//! capability.

use std::fmt;

use serde_json::{Value, json};

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::idempotency::IdempotencyKey;
use crate::resources::customer_payments::{error_code, require};
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

macro_rules! uuid_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(uuid::Uuid);

        impl $name {
            /// A new random id (the demo's).
            #[must_use]
            pub fn new_v4() -> $name {
                $name(uuid::Uuid::new_v4())
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                uuid::Uuid::parse_str(s).map($name)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

uuid_id!(ImportSessionId, "An upload (an engine import session).");
uuid_id!(
    ImportBatchId,
    "An import batch: the unit of commit and undo."
);

/// One row problem the engine found (codes and row numbers only; the
/// engine's message is never kept).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineRowProblem {
    /// The engine's row number in the uploaded file (1 is the header; 0 the
    /// whole file).
    pub row: u64,
    /// The engine's column name, when one cell is at fault.
    pub field: Option<String>,
    /// The engine's code (`IMPORT_UNKNOWN_REF`, `IMPORT_UNBALANCED`, …).
    pub code: String,
}

/// What an upload made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadOutcome {
    /// The upload.
    pub session: ImportSessionId,
    /// Data rows the engine read.
    pub total_rows: u64,
    /// Problems it found (nothing can be committed while there are any).
    pub problems: Vec<EngineRowProblem>,
}

/// Where a batch is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchStatus {
    /// Created, not committed.
    Draft,
    /// The commit is running.
    Committing,
    /// Everything posted.
    Committed,
    /// The commit (or a restart during it) failed; nothing posted.
    Failed,
    /// The undo is running.
    Undoing,
    /// Undone: nothing the batch made remains.
    Undone,
}

impl BatchStatus {
    fn from_wire(s: &str) -> Option<BatchStatus> {
        Some(match s {
            "draft" => BatchStatus::Draft,
            "committing" => BatchStatus::Committing,
            "committed" => BatchStatus::Committed,
            "failed" => BatchStatus::Failed,
            "undoing" => BatchStatus::Undoing,
            "undone" => BatchStatus::Undone,
            _ => return None,
        })
    }

    /// The engine's name.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            BatchStatus::Draft => "draft",
            BatchStatus::Committing => "committing",
            BatchStatus::Committed => "committed",
            BatchStatus::Failed => "failed",
            BatchStatus::Undoing => "undoing",
            BatchStatus::Undone => "undone",
        }
    }

    /// A commit or undo is still running.
    #[must_use]
    pub const fn running(self) -> bool {
        matches!(self, BatchStatus::Committing | BatchStatus::Undoing)
    }
}

/// How far a running batch is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BatchProgress {
    /// The engine's step name (a code).
    pub step: Option<String>,
    /// Done so far.
    pub done: u64,
    /// Of.
    pub total: u64,
}

/// Why a batch failed (nothing posted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchError {
    /// The engine code.
    pub code: String,
    /// The uploaded file's row, when one row is at fault.
    pub row: Option<u64>,
    /// The column, when one cell is at fault (`IMPORT_UNKNOWN_REF`).
    pub field: Option<String>,
}

/// A batch as the engine has it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchState {
    /// The batch.
    pub id: ImportBatchId,
    /// Where it is.
    pub status: BatchStatus,
    /// Progress while running.
    pub progress: BatchProgress,
    /// Why it failed.
    pub error: Option<BatchError>,
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

/// Something the batch blocks undo on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UndoBlocker {
    /// What it is (`payment`, `reconciliation`, `deposit`, …).
    pub kind: String,
    /// Its id.
    pub id: Option<String>,
    /// Why (a code).
    pub reason: Option<String>,
}

/// A refusal the screen explains (U-261, U-263, U-268).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxnImportRefusal {
    /// 409 `IMPORT_ALREADY_COMMITTED`: this file was imported before.
    AlreadyCommitted {
        /// The earlier batch.
        batch: Option<ImportBatchId>,
        /// When (engine text).
        committed_at: Option<String>,
    },
    /// 409 `IMPORT_BUSY`: another import is running in this company.
    Busy,
    /// 409 `IMPORT_SESSION_IN_BATCH`: the upload is already in a batch.
    SessionInBatch,
    /// 409 `IMPORT_UNDO_BLOCKED`: later work uses what the batch made.
    UndoBlocked {
        /// What blocks it.
        blockers: Vec<UndoBlocker>,
    },
    /// 403 `IMPORT_ADMIN_REQUIRED`: only an Admin may commit or undo.
    AdminRequired,
}

/// Classifies `e` (`None` for anything else: closed periods, passwords,
/// network, plain validation).
#[must_use]
pub fn txn_import_refusal(e: &ApiError) -> Option<TxnImportRefusal> {
    let code = error_code(e)?;
    let details = match e {
        ApiError::Conflict { details, .. } => Some(details),
        _ => None,
    };
    Some(match code {
        "IMPORT_ALREADY_COMMITTED" => TxnImportRefusal::AlreadyCommitted {
            batch: details.and_then(|d| d.id("batchId")),
            committed_at: details
                .and_then(|d| d.text("committedAt"))
                .map(str::to_string),
        },
        "IMPORT_BUSY" => TxnImportRefusal::Busy,
        "IMPORT_SESSION_IN_BATCH" => TxnImportRefusal::SessionInBatch,
        "IMPORT_UNDO_BLOCKED" => TxnImportRefusal::UndoBlocked {
            blockers: details
                .and_then(|d| d.raw("blockers"))
                .and_then(Value::as_array)
                .map(|l| {
                    l.iter()
                        .filter_map(|b| {
                            Some(UndoBlocker {
                                kind: b.get("kind")?.as_str()?.to_string(),
                                id: b.get("id").and_then(Value::as_str).map(str::to_string),
                                reason: b.get("reason").and_then(Value::as_str).map(str::to_string),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        "IMPORT_ADMIN_REQUIRED" => TxnImportRefusal::AdminRequired,
        _ => return None,
    })
}

// ---- Parsing --------------------------------------------------------------------

fn parse_json(body: &[u8]) -> Result<Value, ApiError> {
    serde_json::from_slice(body).map_err(|_| decode_err())
}

fn row_problem(v: &Value) -> Option<EngineRowProblem> {
    Some(EngineRowProblem {
        row: v.get("rowNumber").and_then(Value::as_u64).unwrap_or(0),
        field: v.get("field").and_then(Value::as_str).map(str::to_string),
        code: v.get("code")?.as_str()?.to_string(),
    })
}

pub(crate) fn upload_from(body: &[u8]) -> Result<UploadOutcome, ApiError> {
    let v = parse_json(body)?;
    let session = v["session"]["id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(decode_err)?;
    let problems = v["validationErrors"]
        .as_array()
        .map(|l| l.iter().filter_map(row_problem).collect())
        .unwrap_or_default();
    Ok(UploadOutcome {
        session,
        total_rows: v["preview"]["totalRows"]
            .as_u64()
            .or_else(|| v["session"]["rowCount"].as_u64())
            .unwrap_or(0),
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
        progress: BatchProgress {
            step: p["step"].as_str().map(str::to_string),
            done: p["done"].as_u64().unwrap_or(0),
            total: p["total"].as_u64().unwrap_or(0),
        },
        error: err["code"].as_str().map(|c| BatchError {
            code: c.to_string(),
            row: err["rowNumber"].as_u64(),
            field: err["field"].as_str().map(str::to_string),
        }),
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
