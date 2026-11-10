//! Questions the Bank Feed asks before it sends (PR-5):
//!
//! - **S3, record it again.** A waiting bank line that matches an entry
//!   already in the books (`matchCandidateCount > 0`) sent as a new entry
//!   (Categorize, a split, a staged account) or a transfer asks once first,
//!   as receipts do (Q-A05h): "Go back to Match" (default) or "Record it
//!   again". An engine that refuses such a transfer itself (409
//!   `FEED_ITEM_HAS_MATCH`) is answered the same way; after the owner's yes
//!   the transfer goes again with `force`.
//! - **N7, a ticked line.** Undo of a row whose entry is ticked in the
//!   account's reconciliation in progress says so first: Undo removes it
//!   from there too.

use cl_api::{AcceptItem, ApiError, FeedItemId, FeedRow, ReconStatus, UndoTarget};
use cl_core::ids::{AccountId, TransactionId};
use eframe::egui::{self, Ui};

use super::BankFeedScreen;
use super::send::{self, Send};
use crate::i18n::t;
use crate::screens::company::form;
use crate::screens::templates::engine::{Job, finished, run};
use crate::theme::Tokens;

/// A question waiting for the owner.
#[derive(Clone)]
pub(crate) enum Ask {
    /// S3: the line matches an entry in the books; record it again?
    RecordAgain(Send),
    /// N7: the entry is ticked in a reconciliation in progress; Undo anyway?
    UndoTicked(Send),
}

impl Ask {
    /// The control channel's word.
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Ask::RecordAgain(_) => "record_again",
            Ask::UndoTicked(_) => "undo_ticked",
        }
    }
}

/// What the screen keeps for the questions.
#[derive(Default)]
pub(crate) struct Asks {
    /// The question on screen.
    pub ask: Option<Ask>,
    /// The row the owner said "Record it again" for (the next send of it
    /// goes without asking, a refused transfer with `force`).
    pub confirmed: Option<FeedItemId>,
    /// The row Undo was checked for (the next Undo of it goes).
    pub undo_checked: Option<FeedItemId>,
    /// Reading the reconciliation in progress for an Undo.
    pub check: Option<Job<bool>>,
    /// The Undo that read is for.
    pub check_send: Option<Send>,
}

/// The row a send posts as new (not a match, a deposit or Undo).
fn posts_new(send: &Send) -> Option<FeedItemId> {
    match send {
        Send::Categorize(id, _) | Send::Transfer(id, _) => Some(*id),
        // The panel's Accept of a row with a receipt goes through the accept
        // route; only one row of the owner's values (never a match).
        Send::Accept(items) => match items.as_slice() {
            [one] if !is_match(one) => Some(one.id()),
            _ => None,
        },
        _ => None,
    }
}

fn is_match(item: &AcceptItem) -> bool {
    match item {
        AcceptItem::Match { .. } => true,
        AcceptItem::Receipt { item, .. } => is_match(item),
        _ => false,
    }
}

/// S3: holds a send that would record a line again; true when it asked.
pub(crate) fn holds_record_again(s: &mut BankFeedScreen, send: &Send) -> bool {
    let Some(id) = posts_new(send) else {
        return false;
    };
    if s.asks.confirmed == Some(id) {
        return false;
    }
    let candidates = s.row(id).map_or(0, |r| r.match_candidate_count);
    if candidates == 0 {
        return false;
    }
    s.asks.ask = Some(Ask::RecordAgain(send.clone()));
    true
}

/// True when the engine refused a transfer because the line matches an
/// entry in the books (409 `FEED_ITEM_HAS_MATCH`, PR-5 S3).
pub(crate) fn is_has_match(e: &ApiError) -> bool {
    matches!(
        cl_api::feed_refusal(e),
        Some(cl_api::FeedRefusal::HasMatch { .. })
    )
}

/// The row a single-row Undo is about, when it is accepted with an entry.
fn undo_row(s: &BankFeedScreen, send: &Send) -> Option<(FeedItemId, TransactionId)> {
    let Send::Undo(UndoTarget::Items(ids), _) = send else {
        return None;
    };
    let [id] = ids.as_slice() else {
        return None;
    };
    let r = s.row(*id)?;
    Some((*id, r.matched_transaction?))
}

/// The account of a row (the picker's connection).
fn account_of(s: &BankFeedScreen, r: &FeedRow) -> Option<AccountId> {
    s.accounts
        .iter()
        .find(|a| Some(a.connection) == r.connection)
        .and_then(|a| a.account)
}

/// N7: Undo of a row whose entry may be ticked in a reconciliation in
/// progress reads it first; true when the send waits for that.
pub(crate) fn holds_undo(s: &mut BankFeedScreen, send: &Send) -> bool {
    let Some((id, entry)) = undo_row(s, send) else {
        return false;
    };
    if s.asks.undo_checked == Some(id) {
        s.asks.undo_checked = None;
        return false;
    }
    let Some(account) = s.row(id).and_then(|r| account_of(s, r)) else {
        return false;
    };
    let job = run(
        &s.link,
        s.group.as_ref(),
        None,
        move |d| {
            let h = crate::company::demo::reconcile::history(d, account)?;
            match h.iter().find(|x| x.status == ReconStatus::InProgress) {
                Some(x) => Ok(ticked(
                    &crate::company::demo::reconcile::get(d, x.id)?,
                    entry,
                )),
                None => Ok(false),
            }
        },
        move |sess| async move {
            let r = sess.reconciliations();
            let h = r.history(account).await?;
            match h.iter().find(|x| x.status == ReconStatus::InProgress) {
                Some(x) => Ok(ticked(&r.get(x.id).await?, entry)),
                None => Ok(false),
            }
        },
    );
    s.asks.check = Some(job);
    s.asks.check_send = Some(send.clone());
    poll(s);
    true
}

/// The entry has a ticked line on the worksheet.
pub(crate) fn ticked(rec: &cl_api::Reconciliation, entry: TransactionId) -> bool {
    rec.status == ReconStatus::InProgress
        && rec
            .lines
            .as_ref()
            .is_some_and(|l| l.iter().any(|x| x.cleared && x.transaction == entry))
}

/// Takes the reconciliation read: asks, or sends the Undo (a failed read
/// never stops it: the engine still drops the line, U-193).
pub(crate) fn poll(s: &mut BankFeedScreen) {
    let Some(r) = finished(&mut s.asks.check) else {
        return;
    };
    let Some(send) = s.asks.check_send.take() else {
        return;
    };
    if r.unwrap_or(false) {
        s.asks.ask = Some(Ask::UndoTicked(send));
    } else {
        go_undo(s, send);
    }
}

fn go_undo(s: &mut BankFeedScreen, send: Send) {
    if let Some((id, _)) = undo_row(s, &send) {
        s.asks.undo_checked = Some(id);
    }
    send::start(s, send, None);
}

/// The owner's answer (control channel and the card).
pub(crate) fn answer(s: &mut BankFeedScreen, yes: bool) {
    let Some(ask) = s.asks.ask.take() else {
        return;
    };
    match (ask, yes) {
        (Ask::RecordAgain(send), true) => {
            s.asks.confirmed = posts_new(&send);
            send::start(s, send, None);
        }
        (Ask::RecordAgain(send), false) => {
            // Back to Match on that line.
            if let Some(id) = posts_new(&send) {
                s.list.focus(id);
                if s.detail.row == Some(id) {
                    s.detail.mode = super::detail::Mode::Match;
                }
            }
        }
        (Ask::UndoTicked(send), true) => go_undo(s, send),
        (Ask::UndoTicked(_), false) => {}
    }
}

/// The question's card above the list.
pub(crate) fn ui(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens) {
    let Some(ask) = s.asks.ask.as_ref() else {
        return;
    };
    let (text, yes, no) = match ask {
        Ask::RecordAgain(_) => (
            "bank_feed.ask.record_again",
            "bank_feed.ask.record_again.yes",
            "bank_feed.ask.record_again.no",
        ),
        Ask::UndoTicked(_) => (
            "bank_feed.ask.undo_ticked",
            "bank_feed.ask.undo_ticked.yes",
            "bank_feed.ask.undo_ticked.no",
        ),
    };
    let mut answered = None;
    form::card(ui, tokens, |ui| {
        ui.add(egui::Label::new(t(text)).wrap());
        ui.horizontal(|ui| {
            // The safe choice first (the default).
            if ui.button(t(no)).clicked() {
                answered = Some(false);
            }
            if ui.button(t(yes)).clicked() {
                answered = Some(true);
            }
        });
    });
    if let Some(a) = answered {
        answer(s, a);
    }
}
