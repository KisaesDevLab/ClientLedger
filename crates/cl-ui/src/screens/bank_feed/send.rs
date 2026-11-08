//! What the Bank Feed sends (P26 DI-2): one [`Send`] per owner action,
//! run against the demo or the engine, folded into an [`Outcome`] the
//! window shows (a notice, the Undo toast, the closing-date password, the
//! "changed since" question, the next page of a rule's rows).
//!
//! - Every run takes a **fresh** idempotency key; the client reuses it for
//!   its own network retries. The password retry of locked rows is a new
//!   run (plan §5; test #1: the first accept's replay stays identical).
//! - Single-row Accept posts through categorize / approve / match (a 423
//!   asks for the password); the selection and "Accept all from this
//!   rule" go through the one accept route, where locked rows come back as
//!   `TB_PERIOD_LOCKED` failures (one prompt, then those rows again).

use cl_api::{
    AcceptFailure, AcceptItem, AcceptOutcome, AcceptRequest, ApiError, Capabilities,
    ClosedPeriodAuthorization, DuplicateDecision, FeeLine, FeedChoice, FeedItemId, FeedRefusal,
    IdempotencyKey, RuleAccept, Session, TransferTarget, UndoOutcome, UndoTarget, feed_refusal,
};
use cl_core::Secret;
use cl_core::ids::{JournalLineId, TransactionId};

use super::BankFeedScreen;
use super::detail::{DetailData, Pay};
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
        if o.posted + o.matched > 0 {
            self.undo = Some((
                UndoTarget::Group(o.undo_group),
                (o.posted + o.matched) as usize,
            ));
        }
        if !locked.is_empty()
            && let Send::Accept(items) = send
        {
            let again = items
                .iter()
                .filter(|i| locked.contains(&i.id()))
                .cloned()
                .collect();
            self.closed = Some((locked_error(), Send::Accept(again)));
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
        if self.done > 0 && self.notice.is_none() {
            self.notice = Some("bank_feed.notice.undone");
        }
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
                        Err(e) => single(&mut o, &demo_send, Err(e)),
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
                        Err(e) => single(&mut o, &send, Err(e)),
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
        let pay = |p: &cl_api::PendingPayment| {
            p.amount.exact().map(|amount| Pay {
                id: p.source,
                amount,
                date: p.date,
                label: p.received_from.clone().unwrap_or_default(),
            })
        };
        if let Ok(g) = f.deposit_groupings(id).await {
            d.groupings = g
                .candidates
                .iter()
                .map(|c| c.items.iter().filter_map(pay).collect())
                .collect();
        }
        if let Ok(p) = s.payments().pending_deposits().await {
            d.payments = p.items.iter().filter_map(pay).collect();
        }
    }
    Ok(d)
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
