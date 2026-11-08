//! The demo's Bank Feed review (P26 demo parity): the sample's 37 waiting
//! items of Operating (28 ready, 6 to review, 3 matches; README sample)
//! plus the rows Import Bank File added and hasn't undone, with the
//! engine's buckets, counts, filters and single-row actions.
//!
//! - Suggestions are the sample's: a rule for most ready rows, AI for some
//!   (shown as past entries while AI is off), nothing for the rows that
//!   need the owner. Amounts are the bank's, exactly, in the feed sign.
//! - Accepting marks the row accepted (the demo posts nothing to its
//!   ledger in DI-1); an imported row is settled in the import's feed so an
//!   import Undo leaves it alone.
//! - Developer switches stand in for other computers and roles:
//!   `view_only` answers every write 403 (a `readonly` user) and
//!   `conflict_next` lets "another computer" take the next row written
//!   (409 `FEED_ITEM_NOT_PENDING`).

use std::collections::BTreeMap;

use cl_api::{
    AcceptKind, ApiError, BankConnection, BankConnectionId, ConflictDetails, ConflictReason,
    FeedBucket, FeedChoice, FeedCounts, FeedItemId, FeedPage, FeedQuery, FeedRow, FeedStatus,
    Named, SuggestedRule, TRANSFER_WINDOW_DAYS, TransferHint, UndoGroupId,
};
use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Money};
use rust_decimal::Decimal;

use super::DemoState;

#[path = "feed_review_posting.rs"]
mod posting;
pub use posting::*;

#[path = "feed_review_receipts.rs"]
mod receipts;
pub use receipts::{
    SeedLine, discard_seeded, seed_credit, seed_duplicate, seed_receipt, seed_receipt_for,
    seed_receipt_on, seed_two_receipts,
};

/// One row of the demo review.
#[derive(Clone, Debug)]
struct Row {
    id: FeedItemId,
    account: AccountId,
    date: CalendarDate,
    /// Feed sign (money out positive).
    amount: Money,
    description: String,
    status: FeedStatus,
    /// The sample's word (`ready`, `review`, `match`) or `review` for an
    /// imported row.
    kind: &'static str,
    suggested: Option<Named<AccountId>>,
    payee: Option<String>,
    source: Option<&'static str>,
    confidence: Option<Decimal>,
    rule: Option<SuggestedRule>,
    assigned: Option<Named<AccountId>>,
    transfer: bool,
    entry_voided: bool,
    exclusion_reason: Option<String>,
    matched: Option<TransactionId>,
    /// The import feed's key of an imported row.
    import_key: Option<String>,
    /// The accept posted `matched` (Undo deletes it; else only unlinks).
    posted: bool,
    /// What the accept was (`categorize`, `match`, `transfer`, `deposit`).
    how: &'static str,
    /// The review state before the accept (Undo and Review again restore it).
    prior: Option<Prior>,
    /// The accept group.
    group: Option<UndoGroupId>,
    /// The posted entry as it was posted (Undo asks when it changed).
    fingerprint: Option<String>,
    /// Flagged as a possible duplicate.
    possible_duplicate: bool,
    /// The row it looks like.
    duplicate_of: Option<FeedItemId>,
}

/// A row's review state before an accept: the status, what was staged and
/// the suggestion (the engine's `PRIOR_STATE_COLUMNS`).
#[derive(Clone, Debug)]
struct Prior {
    status: FeedStatus,
    assigned: Option<Named<AccountId>>,
    suggested: Option<Named<AccountId>>,
    payee: Option<String>,
    source: Option<&'static str>,
    confidence: Option<Decimal>,
    rule: Option<SuggestedRule>,
}

impl Row {
    fn waiting(&self) -> bool {
        self.status.is_waiting()
    }

    fn bucket(&self) -> Option<FeedBucket> {
        if !self.waiting() {
            return None;
        }
        if self.possible_duplicate {
            return Some(FeedBucket::Review);
        }
        Some(match self.kind {
            // The sample's own buckets (README: 28 / 6 / 3); a transfer
            // stays where the sample has it and shows its own badge.
            "match" => FeedBucket::Matches,
            "ready" => FeedBucket::Ready,
            _ if self.assigned.is_some() => FeedBucket::Ready,
            _ => FeedBucket::Review,
        })
    }

    fn to_row(&self, ai_on: bool) -> FeedRow {
        let connection = connection_of(self.account);
        // AI off: an older AI suggestion stays, but no AI score shows.
        let (source, confidence) = match self.source {
            Some("ai") if !ai_on => (Some("history"), None),
            // The demo's stand-in for a found transfer partner (the engine
            // reports partners per row, read by the detail panel).
            _ if self.transfer => (Some("transfer"), None),
            s => (s, self.confidence),
        };
        FeedRow {
            id: self.id,
            connection: Some(connection),
            bank_account_name: None,
            date: self.date,
            amount: self.amount,
            description: self.description.clone(),
            original_description: Some(self.description.clone()),
            memo: None,
            check_number: None,
            status: self.status,
            bucket: self.bucket(),
            suggested_account: self.suggested.clone(),
            suggested_contact: None,
            suggested_tag: None,
            confidence,
            match_type: source.map(str::to_string),
            assigned_account: self.assigned.clone(),
            assigned_contact: None,
            assigned_tag: None,
            assigned_memo: self.payee.clone(),
            suggested_rule: self.rule.clone(),
            possible_duplicate: self.possible_duplicate && self.waiting(),
            duplicate_of: None,
            matched_transaction: self.matched,
            matched_line: None,
            entry_voided: self.entry_voided,
            entry_voided_at: None,
            exclusion_reason: self.exclusion_reason.clone(),
            match_candidate_count: u32::from(self.kind == "match"),
            transfer_candidate: None,
            accept_kind: if self.status.is_accepted() {
                accept_kind_of(self.how)
            } else {
                None
            },
            updated_at: None,
        }
    }
}

/// The engine's `acceptKind` for what the demo accept was (U-178).
fn accept_kind_of(how: &str) -> Option<AcceptKind> {
    match how {
        "categorize" => Some(AcceptKind::Post),
        "match" => Some(AcceptKind::Match),
        "transfer" => Some(AcceptKind::Transfer),
        "deposit" => Some(AcceptKind::Deposit),
        _ => None,
    }
}

/// The demo review store.
#[derive(Clone, Debug, Default)]
pub struct DemoFeedReview {
    seeded: bool,
    rows: Vec<Row>,
    /// Imported rows' ids by their import key.
    imported: BTreeMap<String, FeedItemId>,
    next_imported: u64,
    /// Writes answer 403 (a view-only user).
    pub view_only: bool,
    /// The next row written was taken by another computer.
    pub conflict_next: bool,
    /// Accept groups: what each accept took in (Undo takes it back).
    groups: BTreeMap<UndoGroupId, Vec<FeedItemId>>,
    /// The rules as edited in the demo (seeded from the rows' rules).
    rules: Option<Vec<cl_api::FeedRule>>,
    /// Rule edits so far (each makes a new change token).
    rule_edits: u32,
    /// What the owner chose per description and account: (accepted,
    /// overridden) (Q-P26k; Undo counts as an override).
    learned: BTreeMap<(String, AccountId), (u32, u32)>,
    /// The choice each categorized row taught (for Undo).
    taught: BTreeMap<FeedItemId, (String, AccountId)>,
    /// Suggestions the owner dismissed.
    dismissed: std::collections::BTreeSet<(String, AccountId)>,
}

fn id(prefix: &str, n: u64) -> FeedItemId {
    FeedItemId::parse(&format!("{prefix}-0000-4000-8000-{n:012}"))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

/// The demo's connection of `account` (one per bank account).
#[must_use]
pub fn connection_of(account: AccountId) -> BankConnectionId {
    BankConnectionId::parse(&account.to_string())
        .unwrap_or_else(|_| unreachable!("an account id is a uuid"))
}

/// The sample's suggestion for a description: (account number, payee).
fn suggestion_for(description: &str) -> Option<(&'static str, &'static str)> {
    const TABLE: [(&str, &str, &str); 19] = [
        ("OZARK OFFICE SUPPLY", "6250", "Ozark Office Supply"),
        (
            "SHEPHERD HILLS OFFICE",
            "6250",
            "Shepherd Hills Office Supply",
        ),
        ("MONETT TIRE", "6350", "Monett Tire & Lube"),
        ("PINE RIDGE STORAGE", "6300", "Pine Ridge Storage Units"),
        ("THUNDER RIDGE", "5000", "Thunder Ridge Subcontracting"),
        ("BRANSON EQUIPMENT", "6350", "Branson Equipment Rental"),
        ("CEDAR HOLLOW SIGNS", "6000", "Cedar Hollow Signs"),
        ("LAKEVIEW CREDIT UNION", "6050", "Lakeview Credit Union"),
        ("RIVERBEND FUEL", "6100", "Riverbend Fuel"),
        ("LAKESIDE INSURANCE", "6150", "Lakeside Insurance Agency"),
        ("BLUE EYE LUMBER", "5090", "Blue Eye Lumber"),
        ("OAK GROVE UNIFORMS", "5200", "Oak Grove Uniforms"),
        ("MOUNTAIN SOFTWARE", "6250", "Mountain Software Co"),
        ("BUSINESS CREDIT CARD", "2050", "Business Credit Card"),
        ("PERSIMMON HILL", "5100", "Persimmon Hill Plants"),
        ("PMT-INV", "4000", ""),
        ("RIDGE TOOL RENTAL", "6350", "Ridge Tool Rental"),
        ("VEHICLE REG", "6700", "Missouri Dept. of Revenue"),
        ("COPPER KETTLE", "6000", "Copper Kettle Catering"),
    ];
    TABLE
        .iter()
        .find(|(k, ..)| description.contains(k))
        .map(|(_, a, p)| (*a, *p))
}

/// The demo rule for a payee: one rule per payee (its id is fixed by the
/// payee's account and name), auto-accept for the regular fuel and office
/// supply payees ("Accept all from this rule").
fn demo_rule(account: &str, payee: &str) -> SuggestedRule {
    let n: u64 = payee
        .bytes()
        .fold(u64::from(account.parse::<u16>().unwrap_or(0)), |h, b| {
            h.wrapping_mul(31).wrapping_add(u64::from(b))
        })
        % 1_000_000_000_000;
    SuggestedRule {
        id: format!("d26a0000-0000-4000-8000-{n:012}"),
        kind: "conditional".into(),
        name: if payee.is_empty() {
            "Customer payments".into()
        } else {
            payee.to_string()
        },
        auto_accept: matches!(payee, "Riverbend Fuel" | "Ozark Office Supply"),
        updated_at: Some("2026-10-01T09:00:00.000Z".into()),
    }
}

fn account_named(chart: &[cl_api::Account], number: &str) -> Option<Named<AccountId>> {
    chart
        .iter()
        .find(|a| a.fields.account_number.as_deref() == Some(number))
        .map(|a| Named {
            id: a.id,
            name: format!("{number} \u{b7} {}", a.fields.name),
        })
}

impl DemoFeedReview {
    /// Seeds the sample's rows once.
    fn seed(&mut self) {
        if self.seeded {
            return;
        }
        self.seeded = true;
        let sample = cl_company::Sample::load();
        let chart = super::chart();
        let Some(operating) = chart
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some("1010"))
            .map(|a| a.id)
        else {
            return;
        };
        let mut n = 0u64;
        for item in sample.feed_through_last_file() {
            n += 1;
            // The sample keeps the bank's sign (money in positive).
            let amount = item.amount.checked_neg().unwrap_or(item.amount);
            let kind: &'static str = match item.status.as_str() {
                "ready" => "ready",
                "match" => "match",
                _ => "review",
            };
            let found = suggestion_for(&item.description);
            let transfer = item.description.contains("CREDIT CARD");
            let suggested = match kind {
                "match" => None,
                _ => found.and_then(|(num, _)| account_named(&chart, num)),
            };
            // Most ready rows come from rules; every fourth from AI (high);
            // review rows with a guess are AI, unsure.
            let (source, confidence, rule) = match (kind, suggested.is_some()) {
                ("ready", true) if n.is_multiple_of(4) => {
                    (Some("ai"), Some(Decimal::new(92, 2)), None)
                }
                ("ready", true) => (Some("rule"), None, found.map(|(a, p)| demo_rule(a, p))),
                ("review", true) => (Some("ai"), Some(Decimal::new(55, 2)), None),
                _ => (None, None, None),
            };
            self.rows.push(Row {
                id: id("d26f0000", n),
                account: operating,
                date: item.date,
                amount,
                description: item.description.trim().to_string(),
                status: FeedStatus::Pending,
                kind,
                suggested,
                payee: found.map(|(_, p)| p.to_string()).filter(|p| !p.is_empty()),
                source,
                confidence,
                rule,
                assigned: None,
                transfer,
                entry_voided: false,
                exclusion_reason: None,
                matched: None,
                import_key: None,
                posted: false,
                how: "",
                prior: None,
                group: None,
                fingerprint: None,
                possible_duplicate: false,
                duplicate_of: None,
            });
        }
    }

    /// Mirrors Import Bank File's waiting rows (new ones arrive, undone
    /// ones leave).
    fn sync(&mut self, feed: &super::bank_import::DemoBankFeed) {
        self.seed();
        let waiting = feed.waiting_rows();
        self.rows.retain(|r| {
            r.import_key
                .as_ref()
                .is_none_or(|k| !r.waiting() || waiting.iter().any(|w| &w.key == k))
        });
        for w in waiting {
            if self.imported.contains_key(&w.key) {
                continue;
            }
            self.next_imported += 1;
            let rid = id("d26f1000", self.next_imported);
            self.imported.insert(w.key.clone(), rid);
            let Ok(amount) = Money::try_from(w.amount) else {
                continue;
            };
            self.rows.push(Row {
                id: rid,
                account: w.account,
                date: w.date,
                amount,
                description: w.description,
                status: FeedStatus::Pending,
                kind: "review",
                suggested: None,
                payee: None,
                source: None,
                confidence: None,
                rule: None,
                assigned: None,
                transfer: false,
                entry_voided: false,
                exclusion_reason: None,
                matched: None,
                import_key: Some(w.key),
                posted: false,
                how: "",
                prior: None,
                group: None,
                fingerprint: None,
                possible_duplicate: false,
                duplicate_of: None,
            });
        }
    }

    /// Forgets the rows Import Bank File added (`dev.demo_bank_import_reset`
    /// empties the import feed, whose next batches reuse their row keys; a
    /// key remembered from before would hide the new rows).
    pub fn forget_imports(&mut self) {
        self.rows.retain(|r| r.import_key.is_none());
        self.imported.clear();
    }

    fn row_mut(&mut self, id: FeedItemId) -> Result<&mut Row, ApiError> {
        self.rows
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or(ApiError::NotFound { code: None })
    }

    /// The checks every write makes: a view-only user, another computer.
    fn write_check(&mut self, id: FeedItemId) -> Result<(), ApiError> {
        if self.view_only {
            return Err(ApiError::Forbidden { code: None });
        }
        if self.conflict_next {
            self.conflict_next = false;
            let r = self.row_mut(id)?;
            if r.waiting() {
                r.status = FeedStatus::Categorized;
            }
            let status = r.status.as_str();
            return Err(conflict(
                "FEED_ITEM_NOT_PENDING",
                serde_json::json!({ "status": status }),
            ));
        }
        Ok(())
    }
}

fn conflict(code: &str, details: serde_json::Value) -> ApiError {
    ApiError::Conflict {
        reason: ConflictReason::Unspecified,
        code: cl_api::ErrorCode::parse(code),
        current_version: None,
        message: cl_api::UserText::default(),
        details: ConflictDetails::new(details),
    }
}

fn not_pending(status: FeedStatus) -> ApiError {
    conflict(
        "FEED_ITEM_NOT_PENDING",
        serde_json::json!({ "status": status.as_str() }),
    )
}

fn ai_on(state: &DemoState) -> bool {
    state.ai.enabled && state.ai.categorization
}

/// The other side of a transfer per waiting row within `window` days (the
/// engine's transfer twin, `feed-transfer-window-v1`).
/// The same test as [`transfer_candidates`]: waiting, another account, the
/// opposite amount, within the window.
fn transfer_twins(state: &DemoState, window: u8) -> BTreeMap<FeedItemId, Vec<FeedItemId>> {
    let waiting: Vec<&Row> = state
        .feed_review
        .rows
        .iter()
        .filter(|r| r.waiting())
        .collect();
    let mut out: BTreeMap<FeedItemId, Vec<FeedItemId>> = BTreeMap::new();
    for r in &waiting {
        let Ok(opposite) = r.amount.checked_neg() else {
            continue;
        };
        let twins: Vec<FeedItemId> = waiting
            .iter()
            .filter(|o| {
                o.account != r.account
                    && o.amount == opposite
                    && r.date.days_until(o.date).abs() <= i64::from(window)
            })
            .map(|o| o.id)
            .collect();
        if !twins.is_empty() {
            out.insert(r.id, twins);
        }
    }
    out
}

/// A row's bucket with the transfer twins: the other side of a transfer is
/// a match signal (a possible duplicate stays in review).
fn bucket_of(r: &Row, twins: &BTreeMap<FeedItemId, Vec<FeedItemId>>) -> Option<FeedBucket> {
    match r.bucket() {
        Some(FeedBucket::Review) if r.possible_duplicate => Some(FeedBucket::Review),
        Some(_) if twins.contains_key(&r.id) => Some(FeedBucket::Matches),
        b => b,
    }
}

fn window_of(days: Option<u8>) -> u8 {
    days.unwrap_or(TRANSFER_WINDOW_DAYS).clamp(1, 10)
}

fn matches_query(r: &Row, q: &FeedQuery, bucket: Option<FeedBucket>) -> bool {
    if q.connection.is_some_and(|c| c != connection_of(r.account)) {
        return false;
    }
    if let Some(s) = q.status {
        if r.status != s {
            return false;
        }
    } else if q.accepted {
        if !r.status.is_accepted() {
            return false;
        }
    } else if q.waiting_only && !r.waiting() {
        return false;
    }
    if let Some(b) = q.bucket
        && bucket != Some(b)
    {
        return false;
    }
    if q.possible_duplicate == Some(true) {
        return false;
    }
    if let Some(v) = q.entry_voided
        && r.entry_voided != v
    {
        return false;
    }
    if let Some(text) = q.search.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let t = text.to_lowercase();
        let amount = r.amount.abs().as_decimal().normalize().to_string();
        if !r.description.to_lowercase().contains(&t) && !amount.contains(&t) {
            return false;
        }
    }
    true
}

/// Mirrors the import feed and flags rows whose entry was voided in the
/// register (Q-P26u: they stay accepted, marked).
fn refresh(state: &mut DemoState) {
    let feed = state.bank_feed.clone();
    state.feed_review.sync(&feed);
    let ledger = &state.ledger;
    for r in &mut state.feed_review.rows {
        if r.status.is_accepted() && !r.entry_voided && r.matched.is_some_and(|t| ledger.is_void(t))
        {
            r.entry_voided = true;
        }
    }
}

/// `GET /banking/feed`.
///
/// # Errors
/// `Local(PageLimit)` as the engine's client.
pub fn list(state: &mut DemoState, q: &FeedQuery) -> Result<FeedPage, ApiError> {
    q.to_query()?;
    let ai = ai_on(state);
    refresh(state);
    let signals = posting::match_signals(state);
    let twins = transfer_twins(state, window_of(q.transfer_window_days));
    let review = &state.feed_review;
    let mut rows: Vec<&Row> = review
        .rows
        .iter()
        .filter(|r| matches_query(r, q, bucket_of(r, &twins)))
        .collect();
    // Newest first, then the sample's order.
    rows.sort_by(|a, b| b.date.cmp(&a.date).then(a.id.cmp(&b.id)));
    let total = rows.len() as u64;
    let chart = std::cell::OnceCell::new();
    let page = rows
        .into_iter()
        .skip(usize::try_from(q.offset).unwrap_or(usize::MAX))
        .take(usize::from(q.limit))
        .map(|r| {
            let mut row = r.to_row(ai);
            if let Some(n) = signals.get(&r.id) {
                row.match_candidate_count = *n;
            }
            row.bucket = bucket_of(r, &twins);
            // Exactly one other side: the list names it (Q-P26y).
            if let Some([one]) = twins.get(&r.id).map(Vec::as_slice) {
                let other = review.rows.iter().find(|x| x.id == *one);
                row.transfer_candidate = other.map(|o| TransferHint {
                    feed_item: o.id,
                    account: Some(o.account),
                    account_name: chart
                        .get_or_init(super::chart)
                        .iter()
                        .find(|a| a.id == o.account)
                        .map(|a| a.fields.name.clone()),
                });
            }
            row.duplicate_of = r
                .duplicate_of
                .filter(|_| row.possible_duplicate)
                .and_then(|d| review.rows.iter().find(|x| x.id == d))
                .map(|d| cl_api::DuplicateOf {
                    id: d.id,
                    date: Some(d.date),
                    amount: Some(d.amount),
                    description: Some(d.description.clone()),
                    status: Some(d.status),
                });
            row
        })
        .collect();
    Ok(FeedPage {
        rows: page,
        total,
        skipped: 0,
    })
}

/// `GET /banking/feed/counts` for one connection or all.
#[must_use]
pub fn counts(state: &mut DemoState, connection: Option<BankConnectionId>) -> FeedCounts {
    counts_in(state, connection, None)
}

/// `GET /banking/feed/counts?transferWindowDays` (the list's window, so
/// Matches agrees with the list).
#[must_use]
pub fn counts_in(
    state: &mut DemoState,
    connection: Option<BankConnectionId>,
    transfer_window_days: Option<u8>,
) -> FeedCounts {
    refresh(state);
    let twins = transfer_twins(state, window_of(transfer_window_days));
    let review = &state.feed_review;
    let mut c = FeedCounts::default();
    for r in review
        .rows
        .iter()
        .filter(|r| connection.is_none_or(|x| x == connection_of(r.account)))
    {
        match bucket_of(r, &twins) {
            Some(FeedBucket::Ready) => c.ready += 1,
            Some(FeedBucket::Review) => c.review += 1,
            Some(FeedBucket::Matches) => c.matches += 1,
            None => {}
        }
        match r.status {
            FeedStatus::Categorized | FeedStatus::Matched => c.accepted += 1,
            FeedStatus::Excluded => c.excluded += 1,
            FeedStatus::Categorizing => c.in_progress += 1,
            _ => {}
        }
        if r.entry_voided {
            c.entry_voided += 1;
        }
    }
    c.waiting = c.ready + c.review + c.matches;
    c
}

/// Waiting rows in all accounts (the rail badge).
#[must_use]
pub fn waiting(state: &mut DemoState) -> u64 {
    counts(state, None).waiting
}

/// The bank accounts with rows (`GET /banking/connections`).
#[must_use]
pub fn connections(state: &mut DemoState) -> Vec<BankConnection> {
    let feed = state.bank_feed.clone();
    let review = &mut state.feed_review;
    review.sync(&feed);
    let mut accounts: Vec<AccountId> = review.rows.iter().map(|r| r.account).collect();
    accounts.sort();
    accounts.dedup();
    let chart = super::chart();
    accounts
        .into_iter()
        .map(|a| BankConnection {
            id: connection_of(a),
            account: Some(a),
            account_name: chart
                .iter()
                .find(|x| x.id == a)
                .map(|x| x.fields.name.clone()),
        })
        .collect()
}

/// When a file was last imported into `account` (the sample's 11/03 bank
/// file for Operating).
#[must_use]
pub fn last_import(state: &DemoState, account: AccountId) -> Option<CalendarDate> {
    state.bank_feed.last_import(account).or_else(|| {
        let chart = super::chart();
        let operating = chart
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some("1010"))?;
        (operating.id == account).then(|| cl_company::Sample::load().company.bank.last_file_date)
    })
}

fn settle(state: &mut DemoState, id: FeedItemId) {
    let key = state
        .feed_review
        .rows
        .iter()
        .find(|r| r.id == id)
        .and_then(|r| r.import_key.clone());
    if let Some(k) = key {
        state.bank_feed.settle(&k);
    }
}

/// `PUT /banking/feed/:id/assign`: stages an account.
///
/// # Errors
/// As [`categorize`].
pub fn assign(state: &mut DemoState, id: FeedItemId, choice: &FeedChoice) -> Result<(), ApiError> {
    state.feed_review.write_check(id)?;
    let chart = super::chart();
    let account = choice
        .account
        .ok_or(ApiError::Local(cl_api::LocalValidation::InvalidValue))?;
    let named = chart
        .iter()
        .find(|a| a.id == account)
        .map(|a| Named {
            id: a.id,
            name: match a.fields.account_number.as_deref() {
                Some(n) => format!("{n} \u{b7} {}", a.fields.name),
                None => a.fields.name.clone(),
            },
        })
        .ok_or(ApiError::NotFound { code: None })?;
    let r = state.feed_review.row_mut(id)?;
    if !r.waiting() {
        return Err(not_pending(r.status));
    }
    r.assigned = Some(named);
    r.status = FeedStatus::Assigned;
    Ok(())
}

/// `PUT /banking/feed/:id/exclude`.
///
/// # Errors
/// As [`categorize`].
pub fn exclude(state: &mut DemoState, id: FeedItemId) -> Result<(), ApiError> {
    state.feed_review.write_check(id)?;
    let r = state.feed_review.row_mut(id)?;
    if !r.waiting() {
        return Err(not_pending(r.status));
    }
    r.status = FeedStatus::Excluded;
    settle(state, id);
    Ok(())
}

/// `PUT /banking/feed/:id/unexclude`.
///
/// # Errors
/// As [`categorize`].
pub fn unexclude(state: &mut DemoState, id: FeedItemId) -> Result<(), ApiError> {
    state.feed_review.write_check(id)?;
    let r = state.feed_review.row_mut(id)?;
    if r.status != FeedStatus::Excluded || r.import_key.is_some() {
        return Err(not_pending(r.status));
    }
    r.status = FeedStatus::Pending;
    r.exclusion_reason = None;
    Ok(())
}

/// Starts the review over (`dev.demo_bank_import_reset` empties the feed
/// too).
pub fn reset(state: &mut DemoState) {
    state.feed_review = DemoFeedReview::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> DemoState {
        DemoState::default()
    }

    #[test]
    fn the_sample_is_37_waiting_28_6_3() {
        let mut s = state();
        let c = counts(&mut s, None);
        assert_eq!((c.waiting, c.ready, c.review, c.matches), (37, 28, 6, 3));
        let q = FeedQuery {
            bucket: Some(FeedBucket::Ready),
            limit: 500,
            ..FeedQuery::default()
        };
        assert_eq!(list(&mut s, &q).expect("list").total, 28);
        let all = list(
            &mut s,
            &FeedQuery {
                waiting_only: true,
                limit: 500,
                ..FeedQuery::default()
            },
        )
        .expect("all");
        assert_eq!(all.total, 37);
        // Money out is positive in the feed sign.
        let fuel = all
            .rows
            .iter()
            .find(|r| r.description.starts_with("RIVERBEND FUEL"))
            .expect("fuel");
        assert_eq!(fuel.amount.to_wire(), "463.8300");
        assert!(fuel.is_money_out());
        assert_eq!(connections(&mut s).len(), 1);
    }

    #[test]
    fn ai_off_shows_no_ai_score() {
        let mut s = state();
        let q = FeedQuery {
            waiting_only: true,
            limit: 500,
            ..FeedQuery::default()
        };
        let on = list(&mut s, &q).expect("on");
        assert!(
            on.rows
                .iter()
                .any(|r| r.match_type.as_deref() == Some("ai"))
        );
        s.ai.enabled = false;
        let off = list(&mut s, &q).expect("off");
        assert!(
            off.rows
                .iter()
                .all(|r| r.match_type.as_deref() != Some("ai"))
        );
        assert!(off.rows.iter().all(|r| r.confidence.is_none()));
    }

    #[test]
    fn accept_exclude_view_only_and_another_computer() {
        let mut s = state();
        let q = FeedQuery {
            bucket: Some(FeedBucket::Ready),
            limit: 500,
            ..FeedQuery::default()
        };
        let rows = list(&mut s, &q).expect("list").rows;
        categorize(&mut s, rows[0].id, None, None).expect("accepted");
        exclude(&mut s, rows[1].id).expect("excluded");
        let c = counts(&mut s, None);
        assert_eq!((c.waiting, c.accepted, c.excluded), (35, 1, 1));
        let again = categorize(&mut s, rows[0].id, None, None).expect_err("not waiting");
        assert!(cl_api::is_stale_row(&again));
        unexclude(&mut s, rows[1].id).expect("restored");
        s.feed_review.view_only = true;
        assert!(cl_api::is_view_only(
            &exclude(&mut s, rows[2].id).expect_err("view only")
        ));
        s.feed_review.view_only = false;
        s.feed_review.conflict_next = true;
        let e = categorize(&mut s, rows[2].id, None, None).expect_err("taken");
        assert!(cl_api::is_stale_row(&e));
        assert_eq!(
            counts(&mut s, None).accepted,
            2,
            "the other computer took it"
        );
        assert!(void_first_accepted(&mut s));
        assert_eq!(counts(&mut s, None).entry_voided, 1);
        let voided = list(
            &mut s,
            &FeedQuery {
                entry_voided: Some(true),
                ..FeedQuery::default()
            },
        )
        .expect("voided");
        assert_eq!(voided.total, 1);
    }
}
