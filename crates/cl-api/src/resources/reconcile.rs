//! Outcome reconciliation of ambiguous mutations (B1; P05 plan §2.6.1).
//!
//! Every mutation carries a [`Probe`] built just before sending: the
//! ledger-version stamp (for postings), the client time, and a matcher
//! that recognises our own write. When the executor reports an ambiguous
//! failure (timeout, connection lost after writing, 5xx), the probe decides:
//!
//! - **Committed**: our write is found; its current record is the answer.
//! - **Not committed**: the old request can no longer commit (the engine
//!   generation moved on) and the stamp did not move, or nothing matching
//!   exists; the executor retries once with the same key.
//! - **Unresolved**: anything else, including two identical candidates
//!   (never guess), within a 15 s budget polled every 500 ms.
//!
//! Stricter than the plan in one place (Q-P05z): a create resolves to
//! "not committed" only when no candidate row created in the window exists
//! at all. A candidate whose fingerprint differs from ours (for example
//! because the engine normalised a field) makes the outcome unresolved,
//! so a normalisation mismatch can never cause a duplicate.
//!
//! Records this session created itself are never candidates, so an
//! earlier identical save of ours is not mistaken for this one. A single
//! match is read back by id (journal entries and master data alike), and
//! a candidate id that does not parse as an id is never put into a path.
//!
//! Only the outcome class is logged: no names, amounts or fingerprints.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Timestamp};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::time::Instant;
use zeroize::Zeroizing;

use crate::client::{Call, EngineReply, Method};
use crate::errors::ApiError;
use crate::idempotency::{Ambiguity, BoxFuture, IdempotencyKey, Reconciled, Reconciler};
use crate::resources::common::{Query, Spec};
use crate::resources::operations::{LEDGER_VERSION, Operation, TXN_GET, TXNS_LIST};
use crate::resources::transactions::TxnType;
use crate::resources::versioning::EditableFields;
use crate::session::Session;
use crate::wire::{Page, PageRequest};

/// Candidates must have been created at most this long before sending.
const CREATED_SLACK_MS: i64 = 2_000;
/// Poll interval while the outcome is open.
const POLL: Duration = Duration::from_millis(500);
/// Pages a reconciliation search reads at most (2,500 rows).
const SEARCH_PAGES: u32 = 5;

/// Computes a row's editable fingerprint from its raw JSON.
pub(crate) type RowFp = fn(Value) -> Option<[u8; 32]>;

/// [`RowFp`] for a model.
pub(crate) fn row_fp<T: DeserializeOwned + EditableFields>(v: Value) -> Option<[u8; 32]> {
    serde_json::from_value::<T>(v).ok().map(|t| t.fingerprint())
}

/// Where a create's candidates are searched.
#[derive(Clone)]
pub(crate) enum Search {
    /// Transactions of `kind` dated `date`, touching `account` when given
    /// (listed, then each candidate read by id).
    Txn {
        kind: TxnType,
        date: CalendarDate,
        account: Option<AccountId>,
    },
    /// Master data whose `name_field` equals `name` exactly.
    Named {
        spec: &'static Spec,
        name_field: &'static str,
        /// Owner data: never logged.
        name: String,
    },
}

/// How to recognise our own write.
#[derive(Clone)]
pub(crate) enum OutcomeMatcher {
    /// A template posted now ("Enter now", `POST /templates/{id}/enter`):
    /// exactly one new transaction dated `date` whose provenance names the
    /// template (`source` `template` without a schedule, `recurring` with
    /// one; `sourceId` the template). The engine marks the provenance in
    /// the same database transaction as the posting, and a template without
    /// a schedule writes no occurrence row, so this is the safest signal the
    /// pinned engine gives (B2, Q-PR1c).
    FromTemplate {
        template: cl_core::ids::TemplateId,
        date: CalendarDate,
    },
    /// A create: exactly one new row with our fingerprint.
    Created {
        search: Search,
        fp: RowFp,
        fingerprint: [u8; 32],
    },
    /// An update: the record now has the requested fields.
    Updated {
        get: &'static Operation,
        path: String,
        key: &'static str,
        fp: RowFp,
        requested: [u8; 32],
        /// The fingerprint it had before (unknown with server versions).
        pre_image: Option<[u8; 32]>,
    },
    /// A void or deactivation: `field` now equals `value`.
    Field {
        get: &'static Operation,
        path: String,
        key: &'static str,
        field: &'static str,
        value: Value,
    },
    /// A delete: the record is gone.
    Deleted {
        get: &'static Operation,
        path: String,
    },
}

/// The reconciler of one mutation. It runs on a view pinned to the company
/// of the write: after a company switch it is `Unresolved` at once and
/// never reads the other company's ledger.
pub(crate) struct Probe {
    session: Session,
    stamp_before: Option<u64>,
    /// Client clock (Unix ms), same machine as the engine (P33 revisits).
    sent_at_ms: i64,
    matcher: OutcomeMatcher,
}

enum Check {
    Final(Reconciled<EngineReply>),
    Wait,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn synth(body: Vec<u8>, generation: u64) -> EngineReply {
    EngineReply {
        status: 200,
        body: Zeroizing::new(body),
        set_cookies: Vec::new(),
        retry_after: None,
        replayed: false,
        generation,
    }
}

fn inner(reply: &EngineReply, key: &str) -> Option<Value> {
    let mut v: Value = serde_json::from_slice(&reply.body).ok()?;
    v.get_mut(key).map(Value::take)
}

/// `GET /api/v1/ledger-version` → the stamp.
pub(crate) async fn read_stamp(session: &Session) -> Result<u64, ApiError> {
    #[derive(Deserialize)]
    struct Wire {
        stamp: u64,
    }
    let reply = session
        .send(
            &LEDGER_VERSION,
            Call::at(Method::Get, LEDGER_VERSION.route()),
            None,
        )
        .await?;
    Ok(crate::session::decode::<Wire>(&reply, "ledger version")?.stamp)
}

impl Probe {
    /// Builds the probe just before sending. Postings (`stamp_covered`)
    /// read the ledger-version stamp first; if that read fails the write
    /// still proceeds and only the matcher can resolve it. The stamp is read
    /// with engine idempotency (U-18) too: a keyed save never reaches the
    /// probe, but a route outside the engine's set, or a key too old to
    /// rely on, is reconciled as before, and the probe cannot tell which.
    pub(crate) async fn new(
        session: &Session,
        matcher: OutcomeMatcher,
        stamp_covered: bool,
    ) -> Probe {
        let session = session.pinned();
        let stamp_before = if stamp_covered {
            read_stamp(&session).await.ok()
        } else {
            None
        };
        Probe {
            session,
            stamp_before,
            sent_at_ms: now_ms(),
            matcher,
        }
    }

    async fn run(&self, amb: &Ambiguity) -> Reconciled<EngineReply> {
        let exec = self.session.client().exec();
        let transport = exec.transport();
        let budget = transport.config().timeouts.reconcile;
        let deadline = Instant::now() + budget;
        let poll = POLL.min(budget / 4);
        if transport
            .wait_ready(deadline, crate::endpoint::Purpose::Ordinary)
            .await
            .is_err()
        {
            return Reconciled::Unresolved;
        }
        loop {
            if self.session.pin_moved() {
                return Reconciled::Unresolved;
            }
            let settled = amb.settled(&transport.current());
            if let Check::Final(r) = self.check(settled, amb.generation).await {
                return r;
            }
            if Instant::now() + poll >= deadline {
                return Reconciled::Unresolved;
            }
            tokio::time::sleep(poll).await;
        }
    }

    async fn check(&self, settled: bool, generation: u64) -> Check {
        if settled && let Some(before) = self.stamp_before {
            match read_stamp(&self.session).await {
                Ok(after) if after == before => return Check::Final(Reconciled::NotCommitted),
                Ok(_) => {}
                Err(_) => return Check::Wait,
            }
        }
        match &self.matcher {
            OutcomeMatcher::Created {
                search,
                fp,
                fingerprint,
            } => {
                self.created(search, *fp, fingerprint, settled, generation)
                    .await
            }
            OutcomeMatcher::Updated {
                get,
                path,
                key,
                fp,
                requested,
                pre_image,
            } => {
                let Ok(reply) = self.get(get, path).await else {
                    return Check::Wait;
                };
                let now = inner(&reply, key).and_then(fp);
                match now {
                    Some(f) if &f == requested => Check::Final(Reconciled::Committed(reply)),
                    Some(f) if Some(f) == *pre_image => {
                        if settled {
                            Check::Final(Reconciled::NotCommitted)
                        } else {
                            Check::Wait
                        }
                    }
                    Some(_) => Check::Final(Reconciled::Unresolved),
                    None => Check::Wait,
                }
            }
            OutcomeMatcher::Field {
                get,
                path,
                key,
                field,
                value,
            } => {
                let Ok(reply) = self.get(get, path).await else {
                    return Check::Wait;
                };
                match inner(&reply, key).and_then(|v| v.get(*field).cloned()) {
                    Some(v) if &v == value => Check::Final(Reconciled::Committed(reply)),
                    Some(_) if settled => Check::Final(Reconciled::NotCommitted),
                    _ => Check::Wait,
                }
            }
            OutcomeMatcher::FromTemplate { template, date } => {
                self.posted_from_template(*template, *date, settled).await
            }
            OutcomeMatcher::Deleted { get, path } => match self.get(get, path).await {
                Err(ApiError::NotFound { .. }) => {
                    Check::Final(Reconciled::Committed(synth(b"{}".to_vec(), generation)))
                }
                Ok(_) if settled => Check::Final(Reconciled::NotCommitted),
                _ => Check::Wait,
            },
        }
    }

    async fn get(&self, op: &Operation, path: &str) -> Result<EngineReply, ApiError> {
        self.session
            .send(op, Call::new(Method::Get, op.route(), path), None)
            .await
    }

    /// Rows of `list` matching `q`, at most [`SEARCH_PAGES`] pages.
    async fn search_rows(
        &self,
        list: &'static Operation,
        list_key: &str,
        q: &Query,
    ) -> Option<Vec<Value>> {
        let mut rows = Vec::new();
        let mut page = PageRequest::first_max();
        for _ in 0..SEARCH_PAGES {
            let mut call = Call::at(Method::Get, list.route());
            call.query.clone_from(&q.0);
            call.query.extend(page.query());
            let reply = self.session.send(list, call, None).await.ok()?;
            let p: Page<Value> = Page::from_json(&reply.body, list_key).ok()?;
            let got = p.data.len();
            rows.extend(p.data);
            if got == 0 || u64::try_from(rows.len()).unwrap_or(u64::MAX) >= p.total {
                return Some(rows);
            }
            page = page.next();
        }
        None
    }

    fn recent(&self, row: &Value) -> bool {
        let created = row
            .get("createdAt")
            .and_then(Value::as_str)
            .and_then(|s| Timestamp::parse_wire(s).ok());
        // A row without a creation time is always a candidate (safe side).
        created.is_none_or(|c| c.unix_millis() >= self.sent_at_ms - CREATED_SLACK_MS)
    }

    /// The row is not a record this session created itself: an earlier
    /// save of ours with the same fields is never mistaken for this one.
    fn not_ours_before(&self, row: &Value) -> bool {
        row.get("id")
            .and_then(Value::as_str)
            .is_none_or(|id| !self.session.created_here(id))
    }

    /// "Enter now" of `template` on `date`: the new postings whose
    /// provenance names the template.
    async fn posted_from_template(
        &self,
        template: cl_core::ids::TemplateId,
        date: CalendarDate,
        settled: bool,
    ) -> Check {
        let want = template.to_string();
        let mut ids = Vec::new();
        for source in ["template", "recurring"] {
            let mut q = Query::default();
            q.put("source", source);
            q.put("startDate", date.to_wire());
            q.put("endDate", date.to_wire());
            let Some(rows) = self.search_rows(&TXNS_LIST, "data", &q).await else {
                return Check::Wait;
            };
            for r in rows
                .iter()
                .filter(|r| r.get("sourceId").and_then(Value::as_str) == Some(want.as_str()))
                .filter(|r| self.recent(r) && self.not_ours_before(r))
            {
                match r
                    .get("id")
                    .and_then(Value::as_str)
                    .and_then(|s| TransactionId::parse(s).ok())
                {
                    Some(id) if ids.contains(&id) => {}
                    Some(id) => ids.push(id),
                    None => return Check::Final(Reconciled::Unresolved),
                }
            }
        }
        match ids.as_slice() {
            [id] => {
                let path = format!("/api/v1/transactions/{id}");
                match self.get(&TXN_GET, &path).await {
                    Ok(reply) => Check::Final(Reconciled::Committed(reply)),
                    Err(_) => Check::Wait,
                }
            }
            [] if settled => Check::Final(Reconciled::NotCommitted),
            [] => Check::Wait,
            _ => Check::Final(Reconciled::Unresolved),
        }
    }

    async fn created(
        &self,
        search: &Search,
        fp: RowFp,
        fingerprint: &[u8; 32],
        settled: bool,
        generation: u64,
    ) -> Check {
        // (candidate count, matching replies)
        let (candidates, mut matches) = match search {
            Search::Txn {
                kind,
                date,
                account,
            } => {
                let mut q = Query::default();
                q.put("txnType", kind.as_str());
                q.put("startDate", date.to_wire());
                q.put("endDate", date.to_wire());
                if let Some(a) = account {
                    q.put("accountId", a.to_string());
                }
                let Some(rows) = self.search_rows(&TXNS_LIST, "data", &q).await else {
                    return Check::Wait;
                };
                let rows: Vec<&Value> = rows
                    .iter()
                    .filter(|r| self.recent(r) && self.not_ours_before(r))
                    .collect();
                // An id that is not a transaction id is never put into a
                // path; the outcome stays open instead.
                let Some(ids) = rows
                    .iter()
                    .map(|r| {
                        r.get("id")
                            .and_then(Value::as_str)
                            .and_then(|s| TransactionId::parse(s).ok())
                    })
                    .collect::<Option<Vec<TransactionId>>>()
                else {
                    return Check::Final(Reconciled::Unresolved);
                };
                let mut found = Vec::new();
                for id in &ids {
                    let path = format!("/api/v1/transactions/{id}");
                    let Ok(reply) = self.get(&TXN_GET, &path).await else {
                        return Check::Wait;
                    };
                    if inner(&reply, "transaction").and_then(fp).as_ref() == Some(fingerprint) {
                        found.push(reply);
                    }
                }
                (ids.len(), found)
            }
            Search::Named {
                spec,
                name_field,
                name,
            } => {
                let mut q = Query::default();
                q.put("search", name.clone());
                let Some(rows) = self.search_rows(spec.list, spec.list_key, &q).await else {
                    return Check::Wait;
                };
                let cands: Vec<Value> = rows
                    .into_iter()
                    .filter(|r| r.get(*name_field).and_then(Value::as_str) == Some(name.as_str()))
                    .filter(|r| self.recent(r) && self.not_ours_before(r))
                    .collect();
                let n = cands.len();
                let ids: Vec<Option<uuid::Uuid>> = cands
                    .into_iter()
                    .filter(|r| fp(r.clone()).as_ref() == Some(fingerprint))
                    .map(|r| {
                        r.get("id")
                            .and_then(Value::as_str)
                            .and_then(|s| cl_core::ids::parse_uuid(s).ok())
                    })
                    .collect();
                let mut found = Vec::new();
                match ids.as_slice() {
                    // Exactly one: read the record itself (as the journal
                    // path does); a list row may not carry every field.
                    [Some(id)] => {
                        let Ok(reply) = self.get(spec.get, &spec.path(&id.to_string())).await
                        else {
                            return Check::Wait;
                        };
                        found.push(reply);
                    }
                    [None] => return Check::Final(Reconciled::Unresolved),
                    // None or several: only the count matters below.
                    _ => found.extend(ids.iter().map(|_| synth(Vec::new(), generation))),
                }
                (n, found)
            }
        };
        match (matches.len(), candidates) {
            (1, _) => Check::Final(Reconciled::Committed(matches.remove(0))),
            (0, 0) if settled => Check::Final(Reconciled::NotCommitted),
            (0, _) if !settled => Check::Wait,
            // Two identical entries in the window, or candidates that are
            // not ours by fingerprint: never guess.
            _ => Check::Final(Reconciled::Unresolved),
        }
    }
}

/// A probe kept after its save ended with an unknown outcome, so the
/// owner's "Check again" can run it later ([`Session::recheck`]). It holds
/// no session (no reference cycle): the recheck rebuilds the pinned view.
#[derive(Clone)]
pub(crate) struct LostProbe {
    pin: crate::session::ScopePin,
    stamp_before: Option<u64>,
    sent_at_ms: i64,
    matcher: OutcomeMatcher,
    /// The engine generation the lost attempt used.
    generation: u64,
    cause: crate::idempotency::AmbiguityCause,
}

/// What the owner's recheck of an unknown save found (B2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recheck {
    /// The save is in the books; `transaction` is the record when the
    /// answer names one (a transaction save).
    Committed {
        /// The saved transaction.
        transaction: Option<TransactionId>,
    },
    /// It provably did not commit (the engine that took it is gone and
    /// nothing of ours is there): the same save may go again.
    NotCommitted,
    /// Still cannot tell: no stored probe for this key, the company was
    /// switched, the engine could not be reached, or two identical
    /// candidates. Save stays blocked until the owner confirms.
    Unknown,
}

/// The transaction a committed answer names (`{transaction}`, `{check}`,
/// a daily-sales day's journal entry `{entry: {transactionId}}`, or a bare
/// record).
fn committed_id(reply: &EngineReply) -> Option<TransactionId> {
    let v: Value = serde_json::from_slice(&reply.body).ok()?;
    [
        "/transaction/id",
        "/check/id",
        "/entry/transactionId",
        "/id",
    ]
    .iter()
    .find_map(|p| v.pointer(p).and_then(Value::as_str))
    .and_then(|s| TransactionId::parse(s).ok())
}

impl Session {
    /// Re-runs the stored probe of the save whose outcome was unknown
    /// (`key` from `OutcomeUnknown`). Read-only: it never sends the save.
    /// `Committed` and `NotCommitted` forget the probe; `Unknown` keeps it
    /// for the next try.
    ///
    /// # Errors
    /// None today (a failed read is `Unknown`); typed for later engines.
    pub async fn recheck(&self, key: &IdempotencyKey) -> Result<Recheck, ApiError> {
        let Some(lost) = self.lost_probe(key) else {
            return Ok(Recheck::Unknown);
        };
        let session = self.pinned_to(lost.pin);
        if session.pin_moved() {
            return Ok(Recheck::Unknown);
        }
        let probe = Probe {
            session,
            stamp_before: lost.stamp_before,
            sent_at_ms: lost.sent_at_ms,
            matcher: lost.matcher.clone(),
        };
        let amb = Ambiguity {
            request_id: cl_core::ids::RequestId::from_uuid(uuid::Uuid::new_v4()),
            generation: lost.generation,
            cause: lost.cause,
            key: Some(key.clone()),
        };
        let answer = match probe.run(&amb).await {
            Reconciled::Committed(reply) => Recheck::Committed {
                transaction: committed_id(&reply),
            },
            Reconciled::NotCommitted => Recheck::NotCommitted,
            Reconciled::Unresolved => Recheck::Unknown,
        };
        if answer != Recheck::Unknown {
            self.forget_lost(key);
        }
        tracing::info!(
            outcome = match answer {
                Recheck::Committed { .. } => "committed",
                Recheck::NotCommitted => "not_committed",
                Recheck::Unknown => "unknown",
            },
            "recheck of an unknown save"
        );
        Ok(answer)
    }
}

#[cfg(test)]
#[path = "recheck_tests.rs"]
mod recheck_tests;

impl Reconciler for Probe {
    fn reconcile<'a>(&'a self, ambiguity: &'a Ambiguity) -> BoxFuture<'a, Reconciled<EngineReply>> {
        Box::pin(async move {
            let r = self.run(ambiguity).await;
            // Kept for the owner's recheck unless it committed.
            if let Some(key) = &ambiguity.key {
                if matches!(r, Reconciled::Committed(_)) {
                    self.session.forget_lost(key);
                } else {
                    self.session.remember_lost(
                        key.clone(),
                        LostProbe {
                            pin: self.session.pin_now(),
                            stamp_before: self.stamp_before,
                            sent_at_ms: self.sent_at_ms,
                            matcher: self.matcher.clone(),
                            generation: ambiguity.generation,
                            cause: ambiguity.cause,
                        },
                    );
                }
            }
            let class = match &r {
                Reconciled::Committed(_) => "committed",
                Reconciled::NotCommitted => "not_committed",
                Reconciled::Unresolved => "unresolved",
            };
            tracing::debug!(
                outcome = class,
                cause = ?ambiguity.cause,
                request_id = %ambiguity.request_id,
                "reconciliation finished"
            );
            r
        })
    }
}
