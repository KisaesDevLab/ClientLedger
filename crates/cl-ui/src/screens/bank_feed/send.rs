//! What the Bank Feed sends (P26 DI-2): one [`Send`] per owner action,
//! run against the demo or the engine, folded into an [`Outcome`] the
//! window shows (a notice, the Undo toast, the closing-date password, the
//! "changed since" question, the next page of a rule's rows).
//!
//! - Every run takes a **fresh** idempotency key; the client reuses it for
//!   its own network retries. The password retry of locked rows is a new
//!   run (plan §5; test #1: the first accept's replay stays identical).
//! - Single-row Accept of the owner's values, a staged row, a match, a
//!   transfer or a deposit posts through its own route (a 423 asks for the
//!   password); a row as proposed, the selection and "Accept all from this
//!   rule" go through the one accept route, where locked rows come back as
//!   `TB_PERIOD_LOCKED` failures (one prompt, then those rows again as
//!   proposed).

use cl_api::{
    AcceptFailure, AcceptItem, AcceptOutcome, AcceptRequest, ApiError, Capabilities,
    ClosedPeriodAuthorization, DuplicateDecision, FeeLine, FeedChoice, FeedItemId, FeedRefusal,
    IdempotencyKey, RuleAccept, Session, TransferTarget, UndoOutcome, UndoTarget, feed_refusal,
};
use cl_core::Secret;
use cl_core::ids::{JournalLineId, TransactionId};

use super::BankFeedScreen;
use super::detail::{DetailData, Held, Pay};
use crate::company::demo::feed_review as demo;
use crate::screens::templates::engine::run;

/// One owner action.
#[derive(Clone)]
pub(crate) enum Send {
    /// Post one row: the owner's values, or (`None`) what is staged.
    Categorize(FeedItemId, Option<FeedChoice>),
    /// Match one row to a bank line in the books.
    Match(FeedItemId, TransactionId, Option<JournalLineId>),
    /// One transfer.
    Transfer(FeedItemId, TransferTarget),
    /// One deposit of waiting payments less fees.
    Deposit(FeedItemId, Vec<TransactionId>, Vec<FeeLine>),
    /// Keep both / Remove this one.
    Duplicate(FeedItemId, DuplicateDecision),
    /// A voided entry's row back to waiting.
    ReviewAgain(FeedItemId),
    /// Undo rows or a group; `true` after the owner confirmed the entries
    /// changed since.
    Undo(UndoTarget, bool),
    /// Accept selected (the one accept route).
    Accept(Vec<AcceptItem>),
    /// Accept all from this rule (one page).
    Rule(RuleAccept),
}

impl Send {
    /// The control channel's word.
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Send::Categorize(..) => "categorize",
            Send::Match(..) => "match",
            Send::Transfer(..) => "transfer",
            Send::Deposit(..) => "deposit",
            Send::Duplicate(..) => "duplicate",
            Send::ReviewAgain(_) => "review_again",
            Send::Undo(..) => "undo",
            Send::Accept(_) => "accept",
            Send::Rule(_) => "rule",
        }
    }
}

/// What an action did.
#[derive(Default)]
pub(crate) struct Outcome {
    pub done: usize,
    pub skipped: usize,
    pub failed: usize,
    /// Rows that changed elsewhere (refreshed, out of the selection).
    pub stale: Vec<FeedItemId>,
    pub view_only: bool,
    /// The closed period: the error for the prompt and what to send again.
    pub closed: Option<(ApiError, Send)>,
    /// The Undo toast: what to take back and how many rows.
    pub undo: Option<(UndoTarget, usize)>,
    /// 409 `FEED_UNDO_CHANGED`: ask, then send again with `force`.
    pub changed: Option<(UndoTarget, usize)>,
    /// A notice in the screen's words (i18n key).
    pub notice: Option<&'static str>,
    /// Rule mode: the next page.
    pub next_rule: Option<RuleAccept>,
    /// Rows Undo took back.
    pub undone: usize,
    /// What each single-row accept was (the Accepted view's text).
    pub accepted_as: Vec<(FeedItemId, &'static str)>,
    /// Receipts Undo put back in the review queue (A05).
    pub reopened: usize,
    /// A05: the engine refused a row's receipt (the row didn't post): the
    /// panel reads the receipt again before the next A.
    pub receipt_refused: bool,
    /// A05: 403 for a request carrying a receipt (the receipt's or the
    /// entry's permission): not view-only; the panel stops offering it.
    pub receipt_forbidden: bool,
}

impl Outcome {
    /// Folds one row's answer in.
    pub(crate) fn take(&mut self, id: FeedItemId, r: Result<(), ApiError>) {
        match r {
            Ok(()) => self.done += 1,
            Err(e) => self.error(Some(id), &e),
        }
    }

    fn error(&mut self, id: Option<FeedItemId>, e: &ApiError) {
        if cl_api::is_view_only(e) {
            self.view_only = true;
        } else if let Some(r) = feed_refusal(e) {
            self.notice = Some(refusal_key(&r));
            if cl_api::is_stale_row(e) {
                self.stale.extend(id);
            } else {
                self.failed += 1;
            }
        } else if cl_api::is_stale_row(e) {
            self.stale.extend(id);
        } else {
            self.failed += 1;
        }
    }

    fn accepted(&mut self, o: &AcceptOutcome, send: &Send) {
        self.done += (o.posted + o.matched) as usize;
        for (id, why) in &o.skipped {
            if why.is_stale() {
                self.stale.push(*id);
            } else {
                self.skipped += 1;
            }
        }
        let locked = o.locked();
        self.failed += o.failures.len() - locked.len();
        // A05: a refused receipt says why (the row didn't post). Only rows
        // that carried one: another row's code never reads as a receipt's.
        let carried = |id: FeedItemId| match send {
            Send::Accept(items) => items.iter().any(|i| i.id() == id && i.draft().is_some()),
            _ => false,
        };
        if let Some(f) = o.failures.iter().find(|f| f.is_receipt() && carried(f.id)) {
            self.notice = Some(receipt_key(&f.code));
            // Changed or used: read again (a fresh token, or gone from the
            // offers). Any other refusal would repeat: no longer offered.
            match f.code.as_str() {
                AcceptFailure::DRAFT_STALE
                | AcceptFailure::DRAFT_CLOSED
                | AcceptFailure::DRAFT_FILE_MISSING => self.receipt_refused = true,
                _ => self.receipt_forbidden = true,
            }
        }
        if o.posted + o.matched > 0 {
            self.undo = Some((
                UndoTarget::Group(o.undo_group),
                (o.posted + o.matched) as usize,
            ));
        }
        if !locked.is_empty() {
            // The locked rows again, with the password: a selection's own
            // items; a rule's rows as proposed (what the rule staged).
            let again = match send {
                Send::Accept(items) => Some(
                    items
                        .iter()
                        .filter(|i| locked.contains(&i.id()))
                        .cloned()
                        .collect(),
                ),
                Send::Rule(_) => Some(locked.iter().copied().map(AcceptItem::AsProposed).collect()),
                _ => None,
            };
            if let Some(again) = again {
                self.closed = Some((locked_error(), Send::Accept(again)));
            }
        }
        if let (Some(after), Send::Rule(r)) = (o.next_after, send) {
            self.next_rule = Some(RuleAccept {
                after: Some(after),
                ..r.clone()
            });
        }
    }

    fn undid(&mut self, o: &UndoOutcome, send: &Send) {
        self.undone += o.undone.len();
        self.done += o.undone.len();
        let locked = o.locked();
        for f in &o.failures {
            match f.code.as_str() {
                AcceptFailure::UNDO_RECONCILED => self.notice = Some("bank_feed.notice.reconciled"),
                AcceptFailure::PAYMENT_BOUNCED => self.notice = Some("bank_feed.notice.bounced"),
                AcceptFailure::NOT_ACCEPTED => self.notice = Some("bank_feed.notice.undo_stale"),
                AcceptFailure::IN_USE => {
                    self.notice = Some("bank_feed.notice.in_use");
                    self.failed += 1;
                }
                AcceptFailure::PERIOD_LOCKED => {}
                _ => self.failed += 1,
            }
        }
        if !locked.is_empty()
            && let Send::Undo(_, force) = send
        {
            self.closed = Some((
                locked_error(),
                Send::Undo(UndoTarget::Items(locked), *force),
            ));
        }
        self.reopened += o.reopened_drafts.len();
        if self.done > 0 && self.notice.is_none() {
            // A05: the receipt went back to the review queue (U-180).
            self.notice = Some(if o.reopened_drafts.is_empty() {
                "bank_feed.notice.undone"
            } else {
                "bank_feed.notice.undone_receipt"
            });
        }
    }
}

/// An accept that failed as a whole: a 403 for a request carrying a
/// receipt is the receipt's permission (U-181: the draft kind's resource
/// or the entry's), not a view-only user (A05 review 3).
fn accept_failed(o: &mut Outcome, send: &Send, e: ApiError) {
    if let Send::Accept(items) = send
        && cl_api::is_view_only(&e)
        && items.iter().any(|i| i.draft().is_some())
    {
        o.receipt_forbidden = true;
        o.failed += 1;
        o.notice = Some("bank_feed.notice.receipt_forbidden");
        return;
    }
    single(o, send, Err(e));
}

/// The notice for a refused receipt (U-181).
pub(crate) fn receipt_key(code: &str) -> &'static str {
    match code {
        AcceptFailure::DRAFT_CLOSED => "bank_feed.notice.receipt_used",
        AcceptFailure::DRAFT_STALE => "bank_feed.notice.receipt_changed",
        AcceptFailure::DRAFT_FILE_MISSING => "bank_feed.notice.receipt_file_missing",
        AcceptFailure::DRAFT_DENIED => "bank_feed.notice.receipt_forbidden",
        _ => "bank_feed.notice.receipt_refused",
    }
}

/// The closed period as the accept routes report it (a failure, not a
/// 423): the prompt asks for the password.
fn locked_error() -> ApiError {
    ApiError::ClosedPeriod(cl_api::ClosedPeriod {
        lock_date: None,
        can_override: false,
        password_required: Some(true),
        message: cl_api::UserText::default(),
    })
}

/// The notice for a refusal.
pub(crate) fn refusal_key(r: &FeedRefusal) -> &'static str {
    match r {
        FeedRefusal::SplitsDontBalance { .. } => "bank_feed.notice.split_off",
        FeedRefusal::AmountNotCents => "bank_feed.split.not_cents",
        FeedRefusal::ItemMismatch { .. } => "bank_feed.notice.deposit_off",
        FeedRefusal::FeeAccountInvalid { .. } => "bank_feed.notice.fee_account",
        FeedRefusal::PaymentNotPendingDeposit => "bank_feed.notice.payment_gone",
        FeedRefusal::PaymentBounced => "bank_feed.notice.bounced",
        FeedRefusal::PossibleDuplicate { .. } => "bank_feed.notice.possible_duplicate",
        FeedRefusal::InUse => "bank_feed.notice.in_use",
        FeedRefusal::RuleChanged { .. } => "bank_feed.notice.rule_changed",
        FeedRefusal::RuleNotAutoAccept => "bank_feed.notice.rule_not_auto",
        FeedRefusal::RuleNotFound => "bank_feed.notice.rule_gone",
        FeedRefusal::TransferMismatch { .. } => "bank_feed.notice.transfer_off",
        // apply-match: the bank amount is above the bill's or invoice's
        // balance (400 `MATCH_MISMATCH {field: amount}`).
        FeedRefusal::MatchMismatch { field } if field.as_deref() == Some("amount") => {
            "bank_feed.notice.match_amount"
        }
        FeedRefusal::MatchMismatch { .. } | FeedRefusal::LineRequired { .. } => {
            "bank_feed.notice.match_off"
        }
        _ => "bank_feed.notice.changed",
    }
}

/// The row an action is about (for the stale list).
fn row_of(send: &Send) -> Option<FeedItemId> {
    match send {
        Send::Categorize(id, _)
        | Send::Match(id, ..)
        | Send::Transfer(id, _)
        | Send::Deposit(id, ..)
        | Send::Duplicate(id, _)
        | Send::ReviewAgain(id) => Some(*id),
        _ => None,
    }
}

/// Folds a single-row posting's answer in (the closed period asks for the
/// password; a group answer offers Undo).
fn single(o: &mut Outcome, send: &Send, r: Result<Option<UndoTarget>, ApiError>) {
    match r {
        Ok(undo) => {
            o.done += 1;
            o.undo = undo.map(|u| (u, 1));
            match send {
                Send::Categorize(id, _) => o.accepted_as.push((*id, "categorize")),
                Send::Match(id, ..) => o.accepted_as.push((*id, "match")),
                Send::Transfer(id, _) => o.accepted_as.push((*id, "transfer")),
                Send::Deposit(id, ..) => o.accepted_as.push((*id, "deposit")),
                // Keep both / Remove this one and Review again record
                // nothing (P26 QA-20).
                Send::Duplicate(_, DuplicateDecision::Keep) => {
                    o.notice = Some("bank_feed.notice.kept_both");
                }
                Send::Duplicate(_, DuplicateDecision::Remove) => {
                    o.notice = Some("bank_feed.notice.removed_duplicate");
                }
                Send::ReviewAgain(_) => o.notice = Some("bank_feed.notice.review_again"),
                Send::Undo(..) | Send::Accept(_) | Send::Rule(_) => {}
            }
        }
        Err(
            e @ (ApiError::ClosedPeriod(_)
            | ApiError::ClosingPasswordRejected
            | ApiError::ClosingPasswordRateLimited { .. }),
        ) => {
            o.closed = Some((e, send.clone()));
        }
        Err(e) if matches!(feed_refusal(&e), Some(FeedRefusal::UndoChanged { .. })) => {
            if let Send::Undo(target, _) = send {
                let n = match feed_refusal(&e) {
                    Some(FeedRefusal::UndoChanged { ids }) => ids.len(),
                    _ => 0,
                };
                o.changed = Some((target.clone(), n));
            }
        }
        Err(e) => o.error(row_of(send), &e),
    }
}

/// Runs `send` (with the password when the owner typed it).
#[allow(
    clippy::many_single_char_names,
    reason = "the demo and live closures' short names"
)]
pub(crate) fn start(s: &mut BankFeedScreen, send: Send, password: Option<Secret>) {
    if s.view_only || s.act.is_some() {
        return;
    }
    s.last_send = Some(send.as_str());
    let demo_send = send.clone();
    let demo_pw = password.clone();
    let key = IdempotencyKey::new();
    s.act = Some(run(
        &s.link,
        s.group.as_ref(),
        Some("bank_feed.orphan.accept"),
        move |d| {
            let mut o = Outcome::default();
            let pw = demo_pw.as_ref();
            match &demo_send {
                Send::Accept(items) => {
                    match demo::accept(d, &AcceptRequest::Items(items.clone()), pw) {
                        Ok(a) => o.accepted(&a, &demo_send),
                        Err(e) => accept_failed(&mut o, &demo_send, e),
                    }
                }
                Send::Rule(r) => match demo::accept(d, &AcceptRequest::Rule(r.clone()), pw) {
                    Ok(a) => o.accepted(&a, &demo_send),
                    Err(e) => o.error(None, &e),
                },
                Send::Undo(target, force) => match demo::undo(d, target, *force, pw) {
                    Ok(u) => o.undid(&u, &demo_send),
                    Err(e) => single(&mut o, &demo_send, Err(e)),
                },
                other => {
                    let r = demo_single(d, other, pw);
                    single(&mut o, &demo_send, r);
                }
            }
            Ok(o)
        },
        move |sess| async move {
            let mut o = Outcome::default();
            let auth = password.map(ClosedPeriodAuthorization::with_password);
            let auth = auth.as_ref();
            let f = sess.feed();
            match &send {
                Send::Accept(items) => {
                    match f
                        .accept(&AcceptRequest::Items(items.clone()), auth, key)
                        .await
                    {
                        Ok(a) => o.accepted(&a, &send),
                        Err(e) => accept_failed(&mut o, &send, e),
                    }
                }
                Send::Rule(r) => match f.accept(&AcceptRequest::Rule(r.clone()), auth, key).await {
                    Ok(a) => o.accepted(&a, &send),
                    Err(e) => o.error(None, &e),
                },
                Send::Undo(target, force) => match f.undo(target, *force, auth, key).await {
                    Ok(u) => o.undid(&u, &send),
                    Err(e) => single(&mut o, &send, Err(e)),
                },
                other => {
                    let r = live_single(&sess, other, auth, key).await;
                    single(&mut o, &send, r);
                }
            }
            Ok(o)
        },
    ));
    s.poll();
}

/// Undo of one row by its id (categorize answers no group).
#[allow(
    clippy::unnecessary_wraps,
    reason = "used as the fallback of an optional group"
)]
fn by_row(id: FeedItemId) -> Option<UndoTarget> {
    Some(UndoTarget::Items(vec![id]))
}

fn demo_single(
    d: &mut crate::company::demo::DemoState,
    send: &Send,
    pw: Option<&Secret>,
) -> Result<Option<UndoTarget>, ApiError> {
    match send {
        Send::Categorize(id, c) => demo::categorize(d, *id, c.as_ref(), pw).map(|_| by_row(*id)),
        Send::Match(id, t, _) => {
            demo::match_line(d, *id, *t).map(|g| g.map(UndoTarget::Group).or_else(|| by_row(*id)))
        }
        Send::Transfer(id, target) => demo::transfer(d, *id, *target, pw)
            .map(|p| p.undo_group.map(UndoTarget::Group).or_else(|| by_row(*id))),
        Send::Deposit(id, pays, fees) => demo::deposit(d, *id, pays, fees, pw)
            .map(|p| p.undo_group.map(UndoTarget::Group).or_else(|| by_row(*id))),
        Send::Duplicate(id, dec) => demo::decide_duplicate(d, *id, *dec).map(|()| None),
        Send::ReviewAgain(id) => demo::review_again(d, *id).map(|()| None),
        Send::Undo(..) | Send::Accept(_) | Send::Rule(_) => Ok(None),
    }
}

async fn live_single(
    s: &Session,
    send: &Send,
    auth: Option<&ClosedPeriodAuthorization>,
    key: IdempotencyKey,
) -> Result<Option<UndoTarget>, ApiError> {
    let f = s.feed();
    let undo_ok = s.client().capabilities().has(Capabilities::FEED_UNDO);
    let row = |id: FeedItemId| by_row(id).filter(|_| undo_ok);
    match send {
        Send::Categorize(id, Some(c)) => f.categorize(*id, c, auth, key).await.map(|_| row(*id)),
        Send::Categorize(id, None) => f.approve(*id, auth, key).await.map(|_| row(*id)),
        Send::Match(id, t, line) => f
            .match_line(*id, *t, *line, key)
            .await
            .map(|g| g.map(UndoTarget::Group).or_else(|| row(*id))),
        Send::Transfer(id, target) => f
            .transfer(*id, *target, auth, key)
            .await
            .map(|p| p.undo_group.map(UndoTarget::Group).or_else(|| row(*id))),
        Send::Deposit(id, pays, fees) => f
            .deposit(*id, pays, fees, auth, key)
            .await
            .map(|p| p.undo_group.map(UndoTarget::Group).or_else(|| row(*id))),
        Send::Duplicate(id, dec) => f.decide_duplicate(*id, *dec, key).await.map(|()| None),
        Send::ReviewAgain(id) => f.review_again(*id, key).await.map(|()| None),
        Send::Undo(..) | Send::Accept(_) | Send::Rule(_) => Ok(None),
    }
}

/// The detail panel's reads on a live engine: candidates and payroll
/// lines (money out), transfer partners, deposit groupings and waiting
/// payments (money in). A 403 on the candidates is "not for your role".
pub(crate) async fn live_detail(
    s: Session,
    id: FeedItemId,
    money_in: bool,
    window: u8,
) -> Result<DetailData, ApiError> {
    let f = s.feed();
    let caps = s.client().capabilities();
    let mut d = DetailData {
        row: Some(id),
        ..DetailData::default()
    };
    if caps.has(Capabilities::FEED_MATCH_V2) {
        match f.match_candidates(id).await {
            Ok(c) => d.candidates = c,
            Err(e) if cl_api::is_view_only(&e) => d.forbidden = true,
            Err(e) => return Err(e),
        }
        if !money_in && !d.forbidden {
            d.payroll = f.payroll_overlap(id).await.unwrap_or_default();
        }
    }
    if caps.has(Capabilities::FEED_MATCH_POST) && !d.forbidden {
        d.transfers = f.transfer_candidates(id, window).await.unwrap_or_default();
    }
    if money_in && !d.forbidden {
        if let Ok(g) = f.deposit_groupings(id).await {
            // A grouping with an amount that isn't exact isn't offered (its
            // exact part wouldn't add up).
            d.groupings = g
                .candidates
                .iter()
                .filter_map(|c| c.items.iter().map(|p| pay_of(p).ok()).collect())
                .collect();
        }
        if let Ok(p) = s.payments().pending_deposits().await {
            (d.payments, d.held) = split_payments(&p.items);
        }
    }
    Ok(d)
}

/// A waiting payment the panel may deposit, or (its amount display only)
/// one it shows as "can't be deposited here".
fn pay_of(p: &cl_api::PendingPayment) -> Result<Pay, Held> {
    let label = super::detail::payer_label(
        p.received_from.as_deref(),
        p.ref_no.as_deref(),
        Some(p.txn_type.as_str()),
    );
    match p.amount.exact() {
        Some(amount) => Ok(Pay {
            id: p.source,
            amount,
            date: p.date,
            label,
        }),
        None => Err(Held {
            date: p.date,
            label,
            amount: p.amount.display(false).unwrap_or_default(),
        }),
    }
}

/// The waiting payments: the depositable ones and the held ones.
fn split_payments(items: &[cl_api::PendingPayment]) -> (Vec<Pay>, Vec<Held>) {
    let mut pays = Vec::new();
    let mut held = Vec::new();
    for p in items {
        match pay_of(p) {
            Ok(x) => pays.push(x),
            Err(h) => held.push(h),
        }
    }
    (pays, held)
}

/// The plan's rows as accept items: ready rows as proposed, matches with
/// their one candidate (read now; ambiguous ones are left for the owner).
pub(crate) fn plan_items(
    s: &BankFeedScreen,
    categorize: &[FeedItemId],
    matches: &[FeedItemId],
) -> (Vec<AcceptItem>, Vec<FeedItemId>) {
    let mut items: Vec<AcceptItem> = categorize
        .iter()
        .map(|id| AcceptItem::AsProposed(*id))
        .collect();
    // The candidates of the cursor's row are known; the others are read by
    // the accept job itself.
    let mut unknown = Vec::new();
    for id in matches {
        match (
            s.detail.row == Some(*id),
            s.detail.data.candidates.as_slice(),
        ) {
            (true, [one]) => items.push(AcceptItem::Match {
                id: *id,
                transaction: one.transaction,
                line: Some(one.line),
            }),
            _ => unknown.push(*id),
        }
    }
    (items, unknown)
}

/// Accept selected: the plan's rows through the one accept route; the
/// matches' single candidates are read first.
pub(crate) fn accept_selected(
    s: &mut BankFeedScreen,
    categorize: &[FeedItemId],
    matches: &[FeedItemId],
    skipped: usize,
) {
    if s.view_only || s.act.is_some() {
        return;
    }
    let (items, unknown) = plan_items(s, categorize, matches);
    s.last_send = Some("accept");
    let (demo_items, demo_unknown) = (items.clone(), unknown.clone());
    let key = IdempotencyKey::new();
    s.act = Some(run(
        &s.link,
        s.group.as_ref(),
        Some("bank_feed.orphan.accept"),
        move |d| {
            let mut items = demo_items;
            let mut o = Outcome {
                skipped,
                ..Outcome::default()
            };
            for id in demo_unknown {
                match demo::match_candidates(d, id).as_slice() {
                    [one] => items.push(AcceptItem::Match {
                        id,
                        transaction: one.transaction,
                        line: Some(one.line),
                    }),
                    _ => o.skipped += 1,
                }
            }
            if items.is_empty() {
                return Ok(o);
            }
            let send = Send::Accept(items.clone());
            match demo::accept(d, &AcceptRequest::Items(items), None) {
                Ok(a) => o.accepted(&a, &send),
                Err(e) => o.error(None, &e),
            }
            Ok(o)
        },
        move |sess| async move {
            let mut items = items;
            let mut o = Outcome {
                skipped,
                ..Outcome::default()
            };
            for id in unknown {
                match sess.feed().match_candidates(id).await.as_deref() {
                    Ok([one]) => items.push(AcceptItem::Match {
                        id,
                        transaction: one.transaction,
                        line: Some(one.line),
                    }),
                    _ => o.skipped += 1,
                }
            }
            if items.is_empty() {
                return Ok(o);
            }
            let send = Send::Accept(items.clone());
            match sess
                .feed()
                .accept(&AcceptRequest::Items(items), None, key)
                .await
            {
                Ok(a) => o.accepted(&a, &send),
                Err(e) => o.error(None, &e),
            }
            Ok(o)
        },
    ));
    s.poll();
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_api::UndoGroupId;

    fn item(n: u8) -> FeedItemId {
        format!("d26f9100-0000-4000-8000-0000000000{n:02}")
            .parse()
            .expect("id")
    }

    fn receipt_link() -> cl_api::DraftLink {
        cl_api::DraftLink {
            id: "d0000000-0000-4000-8000-dd0000000001".parse().expect("d"),
            expected_updated_at: "2026-11-08T10:00:00.000Z".into(),
        }
    }

    /// A05 review 3: a 403 for an accept carrying a receipt is the
    /// receipt's permission, not a view-only user; without a receipt it is.
    #[test]
    fn a_forbidden_receipt_is_not_view_only() {
        let with = Send::Accept(vec![AcceptItem::Receipt {
            item: Box::new(AcceptItem::AsProposed(item(1))),
            draft: receipt_link(),
        }]);
        let mut o = Outcome::default();
        accept_failed(&mut o, &with, ApiError::Forbidden { code: None });
        assert!(o.receipt_forbidden && !o.view_only);
        assert_eq!(o.notice, Some("bank_feed.notice.receipt_forbidden"));
        let without = Send::Accept(vec![AcceptItem::AsProposed(item(1))]);
        let mut o = Outcome::default();
        accept_failed(&mut o, &without, ApiError::Forbidden { code: None });
        assert!(o.view_only && !o.receipt_forbidden);
    }

    /// A05 review 4: a receipt code on a row that carried no receipt isn't
    /// the receipt's (no re-read, no receipt notice); on a row that did, a
    /// refusal that would repeat declines the receipt.
    #[test]
    fn a_receipt_code_counts_only_on_a_row_that_carried_one() {
        let o = AcceptOutcome {
            undo_group: UndoGroupId::new_random(),
            posted: 0,
            matched: 0,
            skipped: Vec::new(),
            failures: vec![AcceptFailure {
                id: item(1),
                code: AcceptFailure::DRAFT_NOT_FOUND.into(),
                details: cl_api::ConflictDetails::default(),
            }],
            remaining: 0,
            next_after: None,
        };
        let mut plain = Outcome::default();
        plain.accepted(&o, &Send::Accept(vec![AcceptItem::AsProposed(item(1))]));
        assert!(!plain.receipt_refused);
        let mut with = Outcome::default();
        with.accepted(
            &o,
            &Send::Accept(vec![AcceptItem::Receipt {
                item: Box::new(AcceptItem::AsProposed(item(1))),
                draft: receipt_link(),
            }]),
        );
        assert!(with.receipt_forbidden && !with.receipt_refused);
        assert_eq!(with.notice, Some("bank_feed.notice.receipt_refused"));
        // A changed receipt is read again instead.
        let stale = AcceptOutcome {
            failures: vec![AcceptFailure {
                id: item(1),
                code: AcceptFailure::DRAFT_STALE.into(),
                details: cl_api::ConflictDetails::default(),
            }],
            ..o.clone()
        };
        let mut again = Outcome::default();
        again.accepted(
            &stale,
            &Send::Accept(vec![AcceptItem::Receipt {
                item: Box::new(AcceptItem::AsProposed(item(1))),
                draft: receipt_link(),
            }]),
        );
        assert!(again.receipt_refused && !again.receipt_forbidden);
    }

    /// P26 review m2: "Accept all from this rule" asks for the password for
    /// its locked rows and sends them again as proposed.
    #[test]
    fn rule_mode_locked_rows_retry_as_proposed() {
        let (open, locked) = (item(1), item(2));
        let o = AcceptOutcome {
            undo_group: UndoGroupId::new_random(),
            posted: 1,
            matched: 0,
            skipped: Vec::new(),
            failures: vec![AcceptFailure {
                id: locked,
                code: AcceptFailure::PERIOD_LOCKED.into(),
                details: cl_api::ConflictDetails::default(),
            }],
            remaining: 0,
            next_after: None,
        };
        let send = Send::Rule(RuleAccept {
            rule_id: "r".into(),
            kind: "conditional".into(),
            updated_at: "t".into(),
            connection: None,
            after: None,
        });
        let mut out = Outcome::default();
        out.accepted(&o, &send);
        assert_eq!((out.done, out.failed), (1, 0));
        let Some((e, Send::Accept(again))) = out.closed else {
            panic!("the password prompt with the locked rows");
        };
        assert!(matches!(e, ApiError::ClosedPeriod(_)));
        assert_eq!(again, vec![AcceptItem::AsProposed(locked)]);
        assert!(!again.iter().any(|i| i.id() == open));
    }

    /// P26 review m8: a waiting payment whose amount isn't exact is shown
    /// as "can't be deposited here", not dropped.
    #[test]
    fn a_payment_that_isnt_exact_is_held_not_dropped() {
        let p = |n: u8, amount: cl_api::PendingAmount| cl_api::PendingPayment {
            source: format!("d26f9200-0000-4000-8000-0000000000{n:02}")
                .parse()
                .expect("id"),
            txn_type: "customer_payment".into(),
            date: cl_core::CalendarDate::from_ymd(2026, 11, 2).expect("date"),
            received_from: Some("Synthetic Client".into()),
            contact: None,
            ref_no: None,
            method: None,
            memo: None,
            amount,
        };
        let exact = p(
            1,
            cl_api::PendingAmount::Exact(cl_core::Money::parse_wire("1250.00").expect("m")),
        );
        let shown = p(
            2,
            cl_api::PendingAmount::DisplayOnly(serde_json::from_str("842.5").expect("amount")),
        );
        let (pays, held) = split_payments(&[exact, shown]);
        assert_eq!(pays.len(), 1);
        assert_eq!(pays[0].amount.to_wire(), "1250.0000");
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].amount, "842.50");
        assert_eq!(held[0].label, "Synthetic Client");
    }

    /// Engine #238: an Undo of an entry someone else holds says "try again".
    #[test]
    fn undo_in_use_says_try_again() {
        let o = UndoOutcome {
            undone: Vec::new(),
            failures: vec![AcceptFailure {
                id: item(3),
                code: AcceptFailure::IN_USE.into(),
                details: cl_api::ConflictDetails::default(),
            }],
            reopened_drafts: Vec::new(),
        };
        let mut out = Outcome::default();
        out.undid(&o, &Send::Undo(UndoTarget::Items(vec![item(3)]), false));
        assert_eq!(out.notice, Some("bank_feed.notice.in_use"));
        assert_eq!(out.done, 0);
    }
}
