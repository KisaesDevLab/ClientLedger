//! The bank line a saved entry was matched to or recorded from (PR-5,
//! Q-PR5a): read when a form or a register edits the entry, so the owner
//! is told before an amount or bank-account change sends the line back to
//! the Bank Feed (QA-2), and when a delete is refused, so the message says
//! what Undo in the Bank Feed will do (QA-3).

use cl_api::{AcceptKind, FeedQuery, FeedRow, FeedStatus, RequestGroup};
use cl_core::Money;
use cl_core::ids::{AccountId, TransactionId};

use crate::company::CompanyLink;
use crate::screens::templates::engine::{Job, run};

/// The bank line on an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeedLink {
    /// The bank line's amount in the feed sign (money out positive).
    pub amount: Money,
    /// How the line was accepted: `Some(true)` the Bank Feed recorded the
    /// entry (Undo removes it), `Some(false)` it was matched to an entry
    /// already in the books (Undo only unlinks it), `None` not known.
    pub posted: Option<bool>,
}

impl FeedLink {
    /// What the engine row says.
    #[must_use]
    pub fn of(r: &FeedRow) -> FeedLink {
        let posted = match r.accept_kind {
            Some(AcceptKind::Post | AcceptKind::Transfer | AcceptKind::Deposit) => Some(true),
            Some(AcceptKind::Match | AcceptKind::Payment) => Some(false),
            None => match r.status {
                FeedStatus::Matched => Some(false),
                FeedStatus::Categorized => Some(true),
                _ => None,
            },
        };
        FeedLink {
            amount: r.amount,
            posted,
        }
    }

    /// The amount as the owner reads it in the notice (no sign).
    #[must_use]
    pub fn shown(&self, style: cl_core::MoneyStyle) -> String {
        self.amount.abs().format(style)
    }
}

/// Reads the line on `entry` in the Bank Feed of the first of `accounts`
/// that has one: the demo's store at once; an engine with `feed-entry-changed-v1`, its accepted lines on the
/// account, newest first, at most 1,000. `None` (no job) on an older engine,
/// which doesn't flag a changed line.
pub(crate) fn read(
    link: &CompanyLink,
    group: Option<&RequestGroup>,
    accounts: Vec<AccountId>,
    entry: TransactionId,
) -> Option<Job<Option<FeedLink>>> {
    if !link.is_demo()
        && !link
            .capabilities()
            .has(cl_api::Capabilities::FEED_ENTRY_CHANGED)
    {
        return None;
    }
    if accounts.is_empty() {
        return None;
    }
    Some(run(
        link,
        group,
        None,
        move |d| {
            Ok(crate::company::demo::feed_review::rows_on(d, entry)
                .first()
                .map(FeedLink::of))
        },
        move |s| async move {
            let f = s.feed();
            let conns = f.connections().await?;
            let Some(c) = conns
                .iter()
                .find(|c| c.account.is_some_and(|a| accounts.contains(&a)))
                .map(|c| c.id)
            else {
                return Ok(None);
            };
            for page in 0..2u32 {
                let q = FeedQuery {
                    connection: Some(c),
                    accepted: true,
                    limit: 500,
                    offset: page * 500,
                    ..FeedQuery::default()
                };
                let p = f.list(&q).await?;
                if let Some(r) = p.rows.iter().find(|r| r.matched_transaction == Some(entry)) {
                    return Ok(Some(FeedLink::of(r)));
                }
                if p.rows.len() < 500 {
                    break;
                }
            }
            Ok(None)
        },
    ))
}

/// What a form's edit is compared on: each bank-side line's account and
/// amount (a check's or a deposit's one bank line; a journal entry's lines).
pub type Shape = Vec<(Option<AccountId>, Option<Money>)>;

/// PR-5 QA-2: a form editing a saved entry watches for a change of the
/// amount or bank account of an entry a bank line is matched to (or was
/// recorded from), and says before Save that the line will wait in the
/// Bank Feed again (Q-PR5a: the engine flags it on save).
#[derive(Default)]
pub(crate) struct Watch {
    read: Option<Job<Option<FeedLink>>>,
    link: Option<FeedLink>,
    opened: Shape,
}

impl std::fmt::Debug for Watch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watch")
            .field("linked", &self.link.is_some())
            .finish_non_exhaustive()
    }
}

impl Watch {
    /// The entry `entry` was loaded with `opened` (on the bank or card
    /// `accounts`): reads its bank line.
    pub(crate) fn start(
        &mut self,
        link: &CompanyLink,
        group: Option<&RequestGroup>,
        entry: TransactionId,
        accounts: Vec<AccountId>,
        opened: Shape,
    ) {
        *self = Watch {
            read: read(link, group, accounts, entry),
            link: None,
            opened,
        };
        self.poll();
    }

    /// Takes the read's answer.
    pub(crate) fn poll(&mut self) {
        if let Some(r) = crate::screens::templates::engine::finished(&mut self.read) {
            self.link = r.ok().flatten();
        }
    }

    /// The entry has a bank line.
    pub(crate) fn linked(&self) -> bool {
        self.link.is_some()
    }

    /// The bank line to name, when `now` differs from the entry as opened.
    pub(crate) fn notice(&self, now: &Shape) -> Option<FeedLink> {
        self.link.filter(|_| *now != self.opened)
    }

    /// Draws the notice (polling first); true when shown.
    pub(crate) fn ui(
        &mut self,
        ui: &mut eframe::egui::Ui,
        tokens: &crate::theme::Tokens,
        now: &Shape,
        style: cl_core::MoneyStyle,
    ) -> bool {
        self.poll();
        let Some(l) = self.notice(now) else {
            return false;
        };
        crate::screens::company::form::inline_warning(
            ui,
            tokens,
            &crate::i18n::t_with("register.edit.feed_linked", &[("amount", &l.shown(style))]),
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: Option<AcceptKind>, status: FeedStatus) -> FeedRow {
        let dir = tempfile::tempdir().expect("tmp");
        let link = CompanyLink::demo(dir.path());
        let mut d = link.demo_state().expect("demo");
        let mut r = crate::company::demo::feed_review::list(&mut d, &FeedQuery::default())
            .expect("list")
            .rows
            .remove(0);
        r.accept_kind = kind;
        r.status = status;
        r
    }

    /// QA-3: the link kind decides what Undo does; the row's accept kind
    /// first, its status for an older engine.
    #[test]
    fn the_link_kind_follows_the_accept() {
        assert_eq!(
            FeedLink::of(&row(Some(AcceptKind::Match), FeedStatus::Matched)).posted,
            Some(false)
        );
        assert_eq!(
            FeedLink::of(&row(Some(AcceptKind::Payment), FeedStatus::Matched)).posted,
            Some(false)
        );
        assert_eq!(
            FeedLink::of(&row(Some(AcceptKind::Post), FeedStatus::Categorized)).posted,
            Some(true)
        );
        assert_eq!(
            FeedLink::of(&row(None, FeedStatus::Categorized)).posted,
            Some(true)
        );
        assert_eq!(
            FeedLink::of(&row(None, FeedStatus::Matched)).posted,
            Some(false)
        );
        assert_eq!(FeedLink::of(&row(None, FeedStatus::Pending)).posted, None);
    }
}
