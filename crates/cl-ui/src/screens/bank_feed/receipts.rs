//! Receipts in the Bank Feed (A05, Q-282, Q-A05a–c): document drafts in
//! the review queue that match a waiting bank line are offered as that
//! line's attachment and coding instead of a new transaction.
//!
//! - **Which drafts** ([`load_live`], [`load_demo`]): live, non-statement
//!   drafts with a file (`status=new,ready`, `view=summary`, paged), read
//!   with the list while Show › Waiting is up, only on an engine with
//!   `feed-accept-draft-v1` (the receipt is filed in the accept's own
//!   transaction, U-181; without it nothing is offered, never a second
//!   call after the posting) and `document-drafts-reopen-v1` (Undo puts
//!   the receipt back in the queue, U-180). At most [`MAX_PAGES`] pages of
//!   200, newest first: in a queue of more than 2,000 receipts only the
//!   newest 2,000 are offered (the rest still attach from the review
//!   queue, A04).
//! - **Which lines**: [`cl_docread::feed_match`] (exact total, money out,
//!   ±7 days, payee first, best 3).
//! - **The panel** ([`section`]): the offers, the best one picked, "Don't
//!   use this receipt", and "Show receipt" in the pop-out viewer (A02).
//! - **Accept** ([`with_receipt`]): the line posts (or matches) as the
//!   Bank Feed would, coded from the receipt (its vendor, a memo of the
//!   vendor and reference, its lines as splits on the panel's account when
//!   they add up exactly to the bank amount in whole cents, Q-A05b) and
//!   carries `documentDraft` through the one accept route.
//! - Bulk accept never takes a receipt (Q-A05a: offered, never applied by
//!   itself); the receipt stays in the queue and A04's attach finds the
//!   posted entry later.
//!
//! Nothing here logs; the control channel gets counts and flags only.

use std::collections::BTreeMap;
use std::sync::Arc;

use cl_api::{
    AcceptItem, ApiError, AttachmentId, Capabilities, DocumentDraft, DocumentDraftId, DraftKind,
    DraftLink, DraftListQuery, DraftStatus, FeedChoice, FeedItemId, FeedRow, FeedSplitLine,
    Session,
};
use cl_core::Money;
use cl_core::ids::{AccountId, ContactId};
use cl_docread::feed_match::{self, BankLine, DraftFacts};
use eframe::egui::{self, RichText, Ui};

use super::BankFeedScreen;
use super::detail::Mode;
use super::send::Send;
use crate::company::demo::DemoState;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::templates::engine::{Job, finished, run};
use crate::theme::Tokens;
use crate::widgets::pdf_viewer::DocSource;
use crate::windows::doc_viewer::{self, DocRef, PaneRequest};

/// Draft pages read at most (200 each).
const MAX_PAGES: u32 = 10;
/// A receipt larger than this isn't shown (the review form's cap).
const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;

/// The drafts the list matches against, and what matched.
#[derive(Default)]
pub(crate) struct Receipts {
    /// Receipts are offered (the engine files them in the accept).
    pub on: bool,
    /// Live, non-statement drafts (summaries).
    pub drafts: Vec<DocumentDraft>,
    /// The offers for each waiting line, best first.
    pub offers: BTreeMap<FeedItemId, Vec<DocumentDraftId>>,
}

impl Receipts {
    /// The drafts read with the list arrived (`None`: receipts are off).
    pub(crate) fn take(&mut self, drafts: Option<Vec<DocumentDraft>>, rows: &[FeedRow]) {
        self.on = drafts.is_some();
        self.drafts = drafts.unwrap_or_default();
        self.offers = offers_for(rows, &self.drafts);
    }

    /// The drafts offered for `id`, best first.
    pub(crate) fn of(&self, id: FeedItemId) -> &[DocumentDraftId] {
        self.offers.get(&id).map_or(&[], Vec::as_slice)
    }

    fn draft(&self, id: DocumentDraftId) -> Option<&DocumentDraft> {
        self.drafts.iter().find(|d| d.id == id)
    }
}

/// The line's payee (staged, else suggested).
fn payee_of(r: &FeedRow) -> Option<ContactId> {
    r.assigned_contact
        .as_ref()
        .or(r.suggested_contact.as_ref())
        .map(|c| c.id)
}

/// The rules' view of a draft.
pub(crate) fn facts(d: &DocumentDraft) -> DraftFacts<DocumentDraftId> {
    DraftFacts {
        id: d.id,
        total: d.doc_total,
        date: d.doc_date,
        contact: d.suggested_contact_id,
        statement: d.kind == DraftKind::Statement
            || !matches!(d.status, DraftStatus::New | DraftStatus::Ready),
    }
}

/// The rules' view of a bank line.
fn bank_line(r: &FeedRow) -> BankLine {
    BankLine {
        amount: r.amount,
        money_out: r.is_money_out(),
        date: r.date,
        contact: payee_of(r),
    }
}

/// The full receipt still goes with the line (its total, date and kind as
/// read in full, not only as listed: an edit in the review queue since the
/// list was read may have changed them, Q-A05c).
pub(crate) fn fits(r: &FeedRow, full: &DocumentDraft) -> bool {
    full.attachment_id.is_some() && feed_match::check(&bank_line(r), &facts(full)).is_ok()
}

/// The offers for every waiting line.
#[must_use]
pub(crate) fn offers_for(
    rows: &[FeedRow],
    drafts: &[DocumentDraft],
) -> BTreeMap<FeedItemId, Vec<DocumentDraftId>> {
    if drafts.is_empty() {
        return BTreeMap::new();
    }
    // A draft without a file (deleted since it was read) can't be filed:
    // the engine would refuse it (`DOCUMENT_FILE_MISSING`) every time.
    let facts: Vec<DraftFacts<DocumentDraftId>> = drafts
        .iter()
        .filter(|d| d.attachment_id.is_some())
        .map(facts)
        .collect();
    rows.iter()
        .filter(|r| r.status.is_waiting() && !r.possible_duplicate && !r.entry_voided)
        .filter_map(|r| {
            let o: Vec<DocumentDraftId> = feed_match::offers(&bank_line(r), &facts)
                .into_iter()
                .map(|o| o.id)
                .collect();
            (!o.is_empty()).then_some((r.id, o))
        })
        .collect()
}

fn query(offset: u32) -> DraftListQuery {
    DraftListQuery {
        statuses: vec![DraftStatus::New, DraftStatus::Ready],
        kind: None,
        limit: cl_api::draft_limits::MAX_PAGE,
        offset,
        summary: true,
    }
}

/// The demo's live drafts (`None` without drafts).
pub(crate) fn load_demo(
    d: &DemoState,
    tenant: Option<cl_core::ids::TenantId>,
) -> Option<Vec<DocumentDraft>> {
    if !d.drafts.supported() {
        return None;
    }
    let tenant = tenant.or(d.open)?;
    let mut all = Vec::new();
    for page in 0..MAX_PAGES {
        let got =
            crate::company::demo::drafts::list(d, tenant, &query(page * query(0).limit)).ok()?;
        let n = got.drafts.len();
        all.extend(got.drafts);
        if n < query(0).limit as usize {
            break;
        }
    }
    Some(all)
}

/// The engine's live drafts (`None` when receipts are off: no
/// `feed-accept-draft-v1`, no `document-drafts-reopen-v1` or no drafts). A
/// list that fails offers nothing.
pub(crate) async fn load_live(s: &Session) -> Option<Vec<DocumentDraft>> {
    let caps = s.client().capabilities();
    if !caps.has(Capabilities::FEED_ACCEPT_DRAFT)
        || !caps.has(Capabilities::DOCUMENT_DRAFTS)
        || !caps.has(Capabilities::DOCUMENT_DRAFTS_REOPEN)
    {
        return None;
    }
    let mut all = Vec::new();
    for page in 0..MAX_PAGES {
        let offset = page * cl_api::draft_limits::MAX_PAGE;
        if offset > cl_api::draft_limits::MAX_OFFSET {
            break;
        }
        let Ok(got) = s.document_drafts().list(&query(offset)).await else {
            return Some(Vec::new());
        };
        let n = got.drafts.len();
        all.extend(got.drafts);
        if n < cl_api::draft_limits::MAX_PAGE as usize {
            break;
        }
    }
    Some(all)
}

// ------------------------------------------------------------- the coding

/// What a receipt codes the line with (Q-A05b).
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct Coding {
    pub contact: Option<ContactId>,
    pub memo: Option<String>,
    /// The receipt's lines (amount, memo) when they add up exactly to the
    /// bank amount in whole cents and there are two or more.
    pub lines: Vec<(Money, Option<String>)>,
}

impl std::fmt::Debug for Coding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Coding")
            .field("contact", &self.contact.is_some())
            .field("memo", &self.memo.is_some())
            .field("lines", &self.lines.len())
            .finish()
    }
}

fn clean(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The coding of the full draft `d` for a bank line of `bank` (either
/// sign). Lines become splits only when every one is a positive whole-cent
/// amount and they total `|bank|` exactly; otherwise the line posts on one
/// account.
#[must_use]
pub(crate) fn coding(d: &DocumentDraft, bank: Money) -> Coding {
    let header: Option<cl_docread::model::Header> = d
        .fields
        .clone()
        .and_then(|v| serde_json::from_value(v).ok());
    let vendor = header
        .as_ref()
        .and_then(|h| h.vendor.as_ref())
        .and_then(|v| clean(Some(&v.value)));
    let reference = clean(d.doc_reference.as_deref());
    let memo = match (vendor, reference) {
        (Some(v), Some(r)) => Some(format!("{v} \u{b7} {r}")),
        (v, r) => v.or(r),
    };
    let read: Vec<cl_docread::model::Line> = d
        .lines
        .clone()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let lines: Vec<(Money, Option<String>)> = read
        .iter()
        .map(|l| {
            (
                l.amount.value,
                l.description.as_ref().and_then(|x| clean(Some(&x.value))),
            )
        })
        .collect();
    let exact = lines.len() >= 2
        && lines
            .iter()
            .all(|(m, _)| m.is_positive() && cl_model::bank_feed::whole_cents(*m))
        && Money::sum(lines.iter().map(|(m, _)| *m)).ok() == Some(bank.abs());
    Coding {
        contact: d.suggested_contact_id,
        memo,
        lines: if exact { lines } else { Vec::new() },
    }
}

/// The choice the line posts with: the panel's account, coded from the
/// receipt.
#[must_use]
pub(crate) fn choice_of(
    c: &Coding,
    account: AccountId,
    row_contact: Option<ContactId>,
) -> FeedChoice {
    FeedChoice {
        account: Some(account),
        contact: c.contact.or(row_contact),
        memo: c.memo.clone(),
        splits: c
            .lines
            .iter()
            .map(|(amount, memo)| FeedSplitLine {
                account,
                amount: *amount,
                tag: None,
                memo: memo.clone(),
            })
            .collect(),
        ..FeedChoice::default()
    }
}

// ------------------------------------------------------------- the panel

/// The panel's receipt for its row.
#[derive(Default)]
pub(crate) struct Panel {
    /// The receipt picked (`None`: "Don't use this receipt", or none).
    pub pick: Option<DocumentDraftId>,
    /// The picked draft in full (lines and header).
    pub full: Option<DocumentDraft>,
    load: Option<Job<DocumentDraft>>,
    /// The full draft couldn't be read.
    pub failed: bool,
    /// The picked receipt's file, for the viewer.
    pub doc: Option<(DocumentDraftId, DocSource)>,
    bytes: Option<Job<Option<DocSource>>>,
    /// The draft the file in flight is for.
    bytes_for: Option<DocumentDraftId>,
    /// "Show receipt" was clicked: pop out once the file is here.
    pop: bool,
    /// The owner chose "Don't use this receipt".
    pub declined: bool,
}

/// The row moved: its best receipt is picked.
pub(crate) fn reset(s: &mut BankFeedScreen, id: FeedItemId) {
    let doc = s.detail.receipt.doc.take();
    s.detail.receipt = Panel {
        pick: s.receipts.of(id).first().copied(),
        doc,
        ..Panel::default()
    };
    read_full(s);
}

/// The list was read again: a panel whose pick is no longer offered (used
/// elsewhere, or a receipt arrived) takes the best one now, unless the
/// owner declined; a pick still offered whose receipt changed since it was
/// read in full is read again (its new change token and coding).
pub(crate) fn refresh(s: &mut BankFeedScreen) {
    let Some(id) = s.detail.row else {
        return;
    };
    let offers = s.receipts.of(id);
    let p = &s.detail.receipt;
    if p.declined {
        return;
    }
    if let Some(x) = p.pick.filter(|x| offers.contains(x)) {
        let listed = s.receipts.draft(x).map(|d| d.updated_at.as_str());
        let read = p.full.as_ref().map(|d| d.updated_at.as_str());
        if read.is_some() && listed != read {
            read_again(s);
        }
        return;
    }
    if p.pick.is_some() || !offers.is_empty() {
        reset(s, id);
    }
}

/// The picked receipt is read in full again (after a refusal, or "Try
/// again"): Accept waits for the fresh copy and its change token.
pub(crate) fn read_again(s: &mut BankFeedScreen) {
    s.detail.receipt.full = None;
    s.detail.receipt.failed = false;
    read_full(s);
}

/// The engine refused to file the receipt for this request (403): the
/// panel stops offering it, so the next A posts the line alone.
pub(crate) fn forbidden(s: &mut BankFeedScreen) {
    s.detail.receipt.pick = None;
    s.detail.receipt.full = None;
    s.detail.receipt.declined = true;
}

/// Picks `pick` (or none) and reads it.
fn choose(s: &mut BankFeedScreen, pick: Option<DocumentDraftId>) {
    if s.detail.receipt.pick == pick {
        return;
    }
    s.detail.receipt.pick = pick;
    s.detail.receipt.full = None;
    s.detail.receipt.failed = false;
    read_full(s);
}

fn read_full(s: &mut BankFeedScreen) {
    let Some(id) = s.detail.receipt.pick else {
        return;
    };
    let tenant = s.tenant;
    s.detail.receipt.load = Some(run(
        &s.link,
        s.group.as_ref(),
        None,
        move |d| {
            let t = tenant.or(d.open).ok_or(ApiError::NoCompanyOpen)?;
            crate::company::demo::drafts::get(d, t, id)
        },
        move |sess| async move { sess.document_drafts().get(id).await },
    ));
}

fn fetch_file(s: &mut BankFeedScreen, id: DocumentDraftId, file: AttachmentId) {
    let job = run(
        &s.link,
        s.group.as_ref(),
        None,
        move |d| {
            let meta = crate::company::demo::checks::find_attachment(d, file);
            let bytes = crate::company::demo::checks::download(d, file)?;
            Ok(meta.map(|m| DocSource {
                name: m.file_name,
                bytes: Arc::from(bytes),
            }))
        },
        move |sess| async move {
            let meta = sess.attachments().get(file).await?;
            let bytes = sess.attachments().download(file).await?;
            Ok((bytes.len() <= MAX_FILE_BYTES).then(|| DocSource {
                name: meta.file_name,
                bytes: Arc::from(bytes),
            }))
        },
    );
    s.detail.receipt.bytes = Some(job);
    s.detail.receipt.bytes_for = Some(id);
}

/// Each frame: the full draft and the file arrive; the viewer pops out.
pub(crate) fn follow(s: &mut BankFeedScreen, ctx: &egui::Context) {
    let p = &mut s.detail.receipt;
    if let Some(r) = finished(&mut p.load) {
        match r {
            Ok(d) if Some(d.id) == p.pick => p.full = Some(d),
            Ok(_) => {}
            Err(_) => p.failed = true,
        }
    }
    if let Some(r) = finished(&mut p.bytes)
        && let (Ok(Some(src)), Some(id)) = (r, p.bytes_for.take())
    {
        p.doc = Some((id, src));
    }
    // The viewer follows the picked receipt while it is out.
    let wanted = p.pop || doc_viewer::is_popped(ctx);
    let have = p.doc.as_ref().map(|(id, _)| *id);
    if wanted
        && p.pick.is_some()
        && have != p.pick
        && p.bytes.is_none()
        && let Some(pick) = p.pick
        && let Some(file) = s.receipts.draft(pick).and_then(|d| d.attachment_id)
    {
        fetch_file(s, pick, file);
    }
    let p = &mut s.detail.receipt;
    if p.pop && p.doc.as_ref().map(|(id, _)| *id) == p.pick && p.pick.is_some() {
        p.pop = false;
        if !doc_viewer::is_popped(ctx) {
            doc_viewer::request(ctx, PaneRequest::PopOut);
        }
    }
    if p.load.is_some() || p.bytes.is_some() || p.pop {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}

/// What the pop-out viewer shows: the picked receipt.
pub(crate) fn document(s: &BankFeedScreen) -> Option<DocRef> {
    let p = &s.detail.receipt;
    let (id, src) = p.doc.as_ref()?;
    (Some(*id) == p.pick).then(|| DocRef::new(src.clone()))
}

/// The modes a receipt goes with (an expense or a match; a transfer or a
/// deposit of payments never takes one).
pub(crate) const fn takes_receipt(mode: Mode) -> bool {
    matches!(mode, Mode::Categorize | Mode::Split | Mode::Match)
}

/// The panel's "Receipt" section for a waiting line with offers.
pub(crate) fn section(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens, r: &FeedRow) {
    let offers: Vec<DocumentDraftId> = s.receipts.of(r.id).to_vec();
    if offers.is_empty() {
        return;
    }
    ui.label(RichText::new(t("bank_feed.receipt.title")).strong());
    if !takes_receipt(s.detail.mode) {
        form::hint(ui, tokens, &t("bank_feed.receipt.not_here"));
        ui.add_space(6.0);
        return;
    }
    let mut pick = s.detail.receipt.pick;
    for id in &offers {
        let Some(d) = s.receipts.draft(*id) else {
            continue;
        };
        let date = d.doc_date.map(crate::shell::us_date).unwrap_or_default();
        let amount = d
            .doc_total
            .map(|m| m.format(crate::grid_env::shell_frame(ui.ctx()).money_style))
            .unwrap_or_default();
        let who = d
            .suggested_contact_id
            .and_then(|c| contact_name(s, r, c))
            .unwrap_or_else(|| t("bank_feed.receipt.vendor_unknown"));
        let text = t_with(
            "bank_feed.receipt.offer",
            &[("date", &date), ("who", &who), ("amount", &amount)],
        );
        if ui.radio(pick == Some(*id), text).clicked() {
            pick = Some(*id);
        }
    }
    if ui
        .radio(pick.is_none(), t("bank_feed.receipt.none"))
        .on_hover_text(t("bank_feed.receipt.none.tip"))
        .clicked()
    {
        pick = None;
        s.detail.receipt.declined = true;
    }
    if pick.is_some() {
        s.detail.receipt.declined = false;
    }
    choose(s, pick);
    ui.horizontal(|ui| {
        let on = s.detail.receipt.pick.is_some();
        if ui
            .add_enabled(on, egui::Button::new(t("bank_feed.receipt.show")))
            .on_hover_text(t("bank_feed.receipt.show.tip"))
            .clicked()
        {
            s.detail.receipt.pop = true;
            // The file is fetched next frame (the window may be idle).
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(30));
        }
    });
    if s.detail.receipt.pick.is_some() {
        if s.detail.receipt.failed {
            form::inline_warning(ui, tokens, &t("bank_feed.receipt.read_failed"));
            if ui
                .button(t("bank_feed.receipt.try_again"))
                .on_hover_text(t("bank_feed.receipt.try_again.tip"))
                .clicked()
            {
                read_again(s);
            }
        } else if let Some(full) = &s.detail.receipt.full {
            if fits(r, full) {
                form::hint(ui, tokens, &t("bank_feed.receipt.goes_with"));
            } else {
                form::inline_warning(ui, tokens, &t("bank_feed.receipt.no_longer_fits"));
            }
        } else {
            form::progress_line(ui, tokens, &t("bank_feed.receipt.reading"));
        }
    }
    ui.add_space(6.0);
}

/// The receipt's vendor as the row names it (the only name the feed has
/// on hand; owner data, drawn, never logged).
fn contact_name(_s: &BankFeedScreen, r: &FeedRow, c: ContactId) -> Option<String> {
    r.assigned_contact
        .iter()
        .chain(r.suggested_contact.iter())
        .find(|n| n.id == c)
        .map(|n| n.name.clone())
}

/// Accept with the picked receipt: what the panel would send, coded from
/// the receipt and carrying it. `None` while the receipt is being read or
/// couldn't be read (Accept waits; "Try again" or "Don't use this
/// receipt"); the panel's own send without a receipt when none is picked,
/// or when the receipt as read in full no longer goes with the line (its
/// total edited since the list was read: the panel says so).
pub(crate) fn with_receipt(s: &BankFeedScreen, r: &FeedRow, send: Option<Send>) -> Option<Send> {
    let p = &s.detail.receipt;
    let Some(pick) = p.pick.filter(|_| takes_receipt(s.detail.mode)) else {
        return send;
    };
    if !s.receipts.of(r.id).contains(&pick) {
        return send;
    }
    if p.failed {
        return None;
    }
    let full = p.full.as_ref().filter(|d| d.id == pick)?;
    if !fits(r, full) {
        return send;
    }
    let draft = DraftLink {
        id: pick,
        expected_updated_at: full.updated_at.clone(),
    };
    let coded = coding(full, r.amount);
    let item = match send? {
        // The owner's split (S) as typed, with the receipt's vendor and
        // memo.
        Send::Categorize(id, Some(c)) if !c.splits.is_empty() => AcceptItem::Values(
            id,
            FeedChoice {
                contact: coded.contact.or(c.contact),
                memo: c.memo.clone().or(coded.memo),
                ..c
            },
        ),
        // Staged on the row (a rule's split or memo): posts as staged,
        // with the receipt filed on it.
        Send::Categorize(id, None) => AcceptItem::AsProposed(id),
        // One account: the receipt's coding on it (its lines as splits
        // when they add up exactly).
        Send::Categorize(id, Some(c)) => {
            let account = c.account.or(s.detail.account)?;
            AcceptItem::Values(id, choice_of(&coded, account, payee_of(r)))
        }
        Send::Match(id, transaction, line) => AcceptItem::Match {
            id,
            transaction,
            line,
        },
        _ => return None,
    };
    Some(Send::Accept(vec![AcceptItem::Receipt {
        item: Box::new(item),
        draft,
    }]))
}

/// The control channel's view (counts and flags only).
pub(crate) fn state(s: &BankFeedScreen) -> serde_json::Value {
    let row = s.detail.row;
    let p = &s.detail.receipt;
    serde_json::json!({
        "on": s.receipts.on,
        "drafts": s.receipts.drafts.len(),
        "lines": s.receipts.offers.len(),
        "offers": row.map_or(0, |id| s.receipts.of(id).len()),
        "picked": p.pick.is_some(),
        "best_picked": row.is_some_and(|id| p.pick.is_some() && s.receipts.of(id).first() == p.pick.as_ref()),
        "read": p.full.is_some(),
        "shown": document(s).is_some(),
    })
}

#[cfg(test)]
#[path = "receipts_tests.rs"]
mod tests;
