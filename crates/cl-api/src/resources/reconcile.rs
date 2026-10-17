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

use cl_core::ids::TransactionId;
use cl_core::{CalendarDate, Timestamp};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::time::Instant;
use zeroize::Zeroizing;

use crate::client::{Call, EngineReply, Method};
use crate::errors::ApiError;
use crate::idempotency::{Ambiguity, BoxFuture, Reconciled, Reconciler};
use crate::resources::common::{Query, Spec};
use crate::resources::operations::{LEDGER_VERSION, Operation, TXN_GET, TXNS_LIST};
use crate::resources::versioning::EditableFields;
use crate::session::Session;
use crate::wire::{Capabilities, Page, PageRequest};

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
pub(crate) enum Search {
    /// Journal entries dated `date` (listed, then each candidate read).
    JournalEntry { date: CalendarDate },
    /// Master data whose `name_field` equals `name` exactly.
    Named {
        spec: &'static Spec,
        name_field: &'static str,
        /// Owner data: never logged.
        name: String,
    },
}

/// How to recognise our own write.
pub(crate) enum OutcomeMatcher {
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
    /// still proceeds and only the matcher can resolve it. With engine
    /// idempotency (U-18) nothing is read: reconciliation never runs.
    pub(crate) async fn new(
        session: &Session,
        matcher: OutcomeMatcher,
        stamp_covered: bool,
    ) -> Probe {
        let session = session.pinned();
        let keyed = session
            .client()
            .capabilities()
            .has(Capabilities::IDEMPOTENCY);
        let stamp_before = if stamp_covered && !keyed {
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
            Search::JournalEntry { date } => {
                let mut q = Query::default();
                q.put("txnType", "journal_entry");
                q.put("startDate", date.to_wire());
                q.put("endDate", date.to_wire());
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

impl Reconciler for Probe {
    fn reconcile<'a>(&'a self, ambiguity: &'a Ambiguity) -> BoxFuture<'a, Reconciled<EngineReply>> {
        Box::pin(async move {
            let r = self.run(ambiguity).await;
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
