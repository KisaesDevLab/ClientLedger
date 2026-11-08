//! Before a reviewed document is entered (A05 QA-4, Q-A05h): is a bank
//! line that would be the same purchase waiting in the Bank Feed? If so,
//! accepting the line there with the receipt (A05) records it once;
//! entering the document here and accepting the line later would record it
//! twice.
//!
//! - **The lines**: the Bank Feed's waiting lines (all accounts, paged),
//!   not possible duplicates, not voided entries, read only on an engine
//!   that offers receipts in the Bank Feed (`feed-accept-draft-v1`, the
//!   same gate as [`crate::screens::bank_feed`]'s receipts); the demo
//!   answers from its own feed.
//! - **The rules**: the Bank Feed's own ([`cl_docread::feed_match::check`],
//!   Q-A05a): the exact total, the money's direction, ±7 days. The nearest
//!   date wins.
//! - A list that can't be read never blocks the save (the engine's own
//!   duplicate check still runs).
//!
//! Nothing here logs; the hit is drawn in the question, never sent to the
//! control channel.

use cl_api::{Capabilities, FeedQuery, FeedRow, RequestGroup};
use cl_docread::feed_match::{self, DraftFacts};

use super::attach::MatchProbe;
use super::review::FeedHit;
use crate::company::{CompanyLink, Task};

/// Waiting lines read at most (500 each).
const MAX_PAGES: u32 = 10;
/// The list's page.
const PAGE: u16 = 500;

/// The nearest waiting line `p` matches among `rows`.
#[must_use]
pub(crate) fn hit(rows: &[FeedRow], p: &MatchProbe) -> Option<FeedHit> {
    let facts = DraftFacts {
        id: (),
        total: Some(p.amount),
        date: Some(p.date),
        contact: p.contact,
        statement: false,
    };
    rows.iter()
        .filter(|r| r.status.is_waiting() && !r.possible_duplicate && !r.entry_voided)
        .filter_map(|r| {
            let days =
                feed_match::check(&crate::screens::bank_feed::receipts::bank_line(r), &facts)
                    .ok()?;
            Some((days, r))
        })
        .min_by_key(|(days, r)| (*days, r.date, r.id))
        .map(|(_, r)| FeedHit {
            date: r.date,
            amount: r.amount,
        })
}

fn query(offset: u32) -> FeedQuery {
    FeedQuery {
        waiting_only: true,
        limit: PAGE,
        offset,
        ..FeedQuery::default()
    }
}

/// Reads the waiting lines and finds the one `p` matches. `None`: nothing
/// to search (no Bank Feed receipts on this engine, no session): the save
/// goes ahead.
pub(crate) fn search(
    link: &CompanyLink,
    group: Option<&RequestGroup>,
    p: &MatchProbe,
) -> Option<Task<Option<FeedHit>>> {
    let probe = p.clone();
    if let Some(mut demo) = link.demo_state() {
        if !demo.drafts.supported() {
            return None;
        }
        let mut rows = Vec::new();
        for page in 0..MAX_PAGES {
            let Ok(got) =
                crate::company::demo::feed_review::list(&mut demo, &query(page * u32::from(PAGE)))
            else {
                break;
            };
            let n = got.rows.len();
            rows.extend(got.rows);
            if n < usize::from(PAGE) {
                break;
            }
        }
        return Some(Task::ready(hit(&rows, &probe)));
    }
    let caps = link.capabilities();
    if !caps.has(Capabilities::FEED_REVIEW) || !caps.has(Capabilities::FEED_ACCEPT_DRAFT) {
        return None;
    }
    let (session, group) = (link.session()?, group?);
    group
        .read(&session, move |s| async move {
            let mut rows = Vec::new();
            for page in 0..MAX_PAGES {
                let got = s.feed().list(&query(page * u32::from(PAGE))).await?;
                let n = got.rows.len();
                rows.extend(got.rows);
                if n < usize::from(PAGE) {
                    break;
                }
            }
            Ok(hit(&rows, &probe))
        })
        .ok()
        .map(Task::Pending)
}
