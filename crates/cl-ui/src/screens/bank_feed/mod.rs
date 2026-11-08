//! Bank Feed review (P26, `banking.feed`; DESIGN.md "Bank Feed", mockup
//! `BankFeed`). The engine owns the review queue: this window lists its
//! rows by bucket, keeps the cursor and the selection, and sends ids and
//! the owner's choices. Nothing here totals, posts or matches.
//!
//! Who builds what (plan §7):
//! - DI-1 (here): [`view`] (header, account picker, segments, Show and the
//!   "Entry voided" chip, the list, keyboard triage), [`bulk`] (the bulk
//!   bar), the rail badge and the demo.
//! - DI-2: `detail.rs` (categorize, split, matching, transfers, deposits
//!   with fees, payroll, Undo, Review again) and `rules.rs`. The seam is
//!   [`BankFeedScreen::detail_request`]: S and R (and Accept on a row that
//!   needs the owner) leave the row there for the detail panel, which
//!   [`view::detail_panel`] draws until `detail.rs` replaces it.
//!
//! Live engines need `feed-review-v1` for the buckets and counts; without
//! it the window says the engine is too old. Accept selected needs the one
//! accept route (`feed-accept-v1`, E3); until the engine has it the button
//! is disabled with a tooltip on a live engine (the demo accepts).

pub mod bulk;
pub mod detail;
pub mod receipts;
pub mod rules;
pub mod send;
pub mod view;

use std::sync::Arc;
use std::time::{Duration, Instant};

use cl_api::{
    ApiError, Capabilities, FeedChoice, FeedCounts, FeedItemId, FeedQuery, FeedRow, FeedStatus,
    RequestGroup, Session, UndoTarget,
};
use cl_core::CalendarDate;
use cl_core::ids::{AccountId, TenantId};
use cl_model::bank_feed::{self as model, FeedAction, ReviewList, Segment, Show, Why, WhyInput};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, Ui};
use serde_json::{Value, json};

use crate::company::demo::feed_review as demo;
use crate::company::{CompanyEvent, CompanyLink};
use crate::data_bus::Subscription;
use crate::i18n::t;
use crate::screens::templates::engine::{Job, finished, run};
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "banking.feed";

/// How often an open window reads the feed again (other computers).
const REFRESH: Duration = Duration::from_secs(15);

/// The Bank Feed window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    let _ = today;
    (kind.as_str() == KIND)
        .then(|| Box::new(BankFeedScreen::new(Arc::clone(link), open.map(|o| o.0))) as Box<_>)
}

/// One bank account in the picker.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct FeedAccount {
    pub connection: cl_api::BankConnectionId,
    pub account: Option<AccountId>,
    /// "1010 · Operating Checking".
    pub label: String,
    /// Waiting rows.
    pub waiting: u64,
}

/// What the window reads once (and after actions): accounts, the chart
/// for Change account, the AI switches and the user's role.
pub(crate) struct Meta {
    accounts: Vec<FeedAccount>,
    chart: Vec<(AccountId, String)>,
    ai_on: bool,
    view_only: bool,
    accept_route: bool,
    supported: bool,
    closing_supported: bool,
}

/// The Undo toast after an accept.
pub(crate) struct Toast {
    pub target: UndoTarget,
    pub rows: usize,
    /// How long it has been on screen (time behind another tab doesn't
    /// count, P26 QA-4).
    pub shown: Duration,
}

/// One read of the list.
pub(crate) struct Loaded {
    counts: FeedCounts,
    page: cl_api::FeedPage,
    last_import: Option<CalendarDate>,
    /// Waiting in every account (the rail badge).
    waiting_all: u64,
    /// A05: live drafts to match (`None`: receipts are off).
    drafts: Option<Vec<cl_api::DocumentDraft>>,
}

/// The Bank Feed window.
pub struct BankFeedScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) tenant: Option<TenantId>,
    pub(crate) group: Option<RequestGroup>,
    bus: Subscription,
    pub(crate) accounts: Vec<FeedAccount>,
    pub(crate) account: usize,
    pub(crate) chart: Vec<(AccountId, String)>,
    pub(crate) segment: Segment,
    pub(crate) show: Show,
    pub(crate) voided_only: bool,
    pub(crate) filter: String,
    pub(crate) offset: u32,
    pub(crate) counts: FeedCounts,
    pub(crate) rows: Vec<FeedRow>,
    pub(crate) total: u64,
    pub(crate) list: ReviewList<FeedItemId>,
    pub(crate) last_import: Option<CalendarDate>,
    pub(crate) ai_on: bool,
    pub(crate) view_only: bool,
    pub(crate) accept_route: bool,
    pub(crate) supported: bool,
    /// The row S, R or Accept left for the detail panel (DI-2's seam).
    pub(crate) detail_request: Option<FeedAction<FeedItemId>>,
    /// The bulk bar's Change account choice.
    pub(crate) change_to: Option<AccountId>,
    meta: Option<Job<Meta>>,
    load: Option<Job<Loaded>>,
    pub(crate) act: Option<Job<send::Outcome>>,
    /// The detail panel (DI-2).
    pub(crate) detail: detail::Detail,
    /// The closing-date password for a locked accept or Undo.
    pub(crate) closing: crate::forms::closing_prompt::ClosingPrompt<send::Send>,
    /// The engine takes the closing-date password on feed routes.
    pub(crate) closing_supported: bool,
    /// The Undo toast: what the last accept can take back.
    pub(crate) toast: Option<Toast>,
    /// 409 `FEED_UNDO_CHANGED`: Undo anyway?
    pub(crate) confirm: Option<(UndoTarget, usize)>,
    /// The transfer window in days (Preferences, Q-P26l).
    pub(crate) transfer_window: u8,
    /// The last action sent (control channel).
    pub(crate) last_send: Option<&'static str>,
    /// The Rules panel.
    pub(crate) rules: rules::RulesPanel,
    loaded_at: Option<Instant>,
    /// The last frame this window drew (a gap: it was behind another tab).
    drawn_at: Option<u64>,
    /// What this window accepted each row as (`deposit`, `transfer`,
    /// `match`, `categorize`), so Show › Accepted says what Undo will do
    /// (P26 QA-9, QA-15) on an engine whose rows don't carry `acceptKind`
    /// ([`BankFeedScreen::accepted_how`]).
    pub(crate) accepted_as: std::collections::BTreeMap<FeedItemId, &'static str>,
    pub(crate) load_failed: bool,
    /// The last action's message (i18n key and its count).
    pub(crate) notice: Option<(&'static str, usize)>,
    /// A05: receipts in the review queue that match waiting lines.
    pub(crate) receipts: receipts::Receipts,
    /// The cursor goes to this line when the list is read next (a demo
    /// receipt's line, control channel).
    focus_next: Option<FeedItemId>,
}

impl std::fmt::Debug for BankFeedScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BankFeedScreen")
            .field("segment", &self.segment)
            .field("show", &self.show)
            .field("rows", &self.rows.len())
            .finish_non_exhaustive()
    }
}

impl BankFeedScreen {
    /// A window for the open company; its data is asked for at once.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, tenant: Option<TenantId>) -> BankFeedScreen {
        let group = link.group().and_then(Result::ok);
        let bus = link.bus().subscribe();
        let link_for_prefs = Arc::clone(&link);
        let mut s = BankFeedScreen {
            link,
            tenant,
            group,
            bus,
            accounts: Vec::new(),
            account: 0,
            chart: Vec::new(),
            segment: Segment::All,
            show: Show::Waiting,
            voided_only: false,
            filter: String::new(),
            offset: 0,
            counts: FeedCounts::default(),
            rows: Vec::new(),
            total: 0,
            list: ReviewList::default(),
            last_import: None,
            ai_on: false,
            view_only: false,
            accept_route: false,
            supported: true,
            detail_request: None,
            change_to: None,
            meta: None,
            load: None,
            act: None,
            detail: detail::Detail::default(),
            closing: crate::forms::closing_prompt::ClosingPrompt::default(),
            closing_supported: false,
            toast: None,
            confirm: None,
            transfer_window: transfer_window(&link_for_prefs, tenant),
            last_send: None,
            rules: rules::RulesPanel::default(),
            loaded_at: None,
            drawn_at: None,
            accepted_as: std::collections::BTreeMap::new(),
            load_failed: false,
            notice: None,
            receipts: receipts::Receipts::default(),
            focus_next: None,
        };
        s.read_meta();
        rules::read(&mut s);
        s.poll();
        s
    }

    /// The account the list shows.
    pub(crate) fn current_account(&self) -> Option<&FeedAccount> {
        self.accounts.get(self.account)
    }

    /// What a row's accept was (`categorize`, `match`, `transfer`,
    /// `deposit`): the engine's `acceptKind` (U-178), else what this window
    /// accepted it as, else unknown (the texts then name both outcomes).
    pub(crate) fn accepted_how(&self, r: &FeedRow) -> Option<&'static str> {
        r.accept_kind
            .map(|k| match k {
                cl_api::AcceptKind::Post => "categorize",
                // A payment received from the feed is linked; Undo only
                // unlinks it, as a match.
                cl_api::AcceptKind::Match | cl_api::AcceptKind::Payment => "match",
                cl_api::AcceptKind::Transfer => "transfer",
                cl_api::AcceptKind::Deposit => "deposit",
            })
            .or_else(|| self.accepted_as.get(&r.id).copied())
    }

    /// The row with `id` on the page.
    pub(crate) fn row(&self, id: FeedItemId) -> Option<&FeedRow> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// The engine lists every accepted row in one read (`status=accepted`,
    /// with `feed-accept-v1`; the demo too).
    pub(crate) fn accepted_filter(&self) -> bool {
        self.accept_route || self.link.is_demo()
    }

    /// The list query for the current choices.
    pub(crate) fn query(&self) -> FeedQuery {
        let mut q = FeedQuery {
            connection: self.current_account().map(|a| a.connection),
            // The list badge and Matches follow the owner's window (P26
            // QA-16, U-177); left out for an engine without it (`live_load`).
            transfer_window_days: Some(self.transfer_window),
            search: Some(self.filter.clone()).filter(|f| !f.trim().is_empty()),
            limit: model::PAGE,
            offset: self.offset,
            ..FeedQuery::default()
        };
        match self.show {
            Show::Waiting => {
                q.waiting_only = true;
                q.bucket = self.segment.bucket().map(|b| match b {
                    model::Bucket::Ready => cl_api::FeedBucket::Ready,
                    model::Bucket::Review => cl_api::FeedBucket::Review,
                    model::Bucket::Matches => cl_api::FeedBucket::Matches,
                });
            }
            Show::Accepted if self.voided_only => q.entry_voided = Some(true),
            // Accepted: one read with `status=accepted` (Q-P26y); an engine
            // without it: posted and matched rows, two reads (`reload`).
            Show::Accepted if self.accepted_filter() => q.accepted = true,
            Show::Accepted => q.status = Some(FeedStatus::Categorized),
            Show::Excluded => q.status = Some(FeedStatus::Excluded),
        }
        q
    }

    /// Follows a transfer window changed in Preferences (P26 QA-16): the
    /// panel reads its row's partners again with the new window.
    pub(crate) fn follow_transfer_window(&mut self) {
        let now = transfer_window(&self.link, self.tenant);
        if now != self.transfer_window {
            self.transfer_window = now;
            self.detail.row = None;
        }
    }

    pub(crate) fn read_meta(&mut self) {
        self.follow_transfer_window();
        let tenant = self.tenant;
        let under_cap = crate::forms::suggest::under_cap(&self.link, tenant);
        self.meta = Some(run(
            &self.link,
            self.group.as_ref(),
            None,
            |d| {
                let chart = chart_names(crate::company::demo::chart());
                let conns = demo::connections(d);
                let accounts = conns
                    .into_iter()
                    .map(|c| FeedAccount {
                        connection: c.id,
                        account: c.account,
                        label: label_of(c.account, c.account_name.as_deref(), &chart),
                        waiting: demo::counts(d, Some(c.id)).waiting,
                    })
                    .collect();
                Ok(Meta {
                    accounts,
                    chart,
                    ai_on: d.ai.enabled && d.ai.categorization,
                    view_only: d.feed_review.view_only,
                    accept_route: true,
                    supported: true,
                    closing_supported: true,
                })
            },
            move |s| async move { live_meta(s, tenant, under_cap).await },
        ));
    }

    /// Reads the list and the counts again.
    pub(crate) fn reload(&mut self) {
        if self.load.is_some() || !self.supported {
            return;
        }
        let q = self.query();
        let accepted_too =
            self.show == Show::Accepted && !self.voided_only && !self.accepted_filter();
        let account = self.current_account().and_then(|a| a.account);
        let connection = q.connection;
        let live = q.clone();
        // Receipts are matched against waiting lines only.
        let waiting = self.show == Show::Waiting && !self.view_only;
        let tenant = self.tenant;
        self.loaded_at = Some(Instant::now());
        self.load = Some(run(
            &self.link,
            self.group.as_ref(),
            None,
            move |d| {
                let mut page = demo::list(d, &q)?;
                if accepted_too {
                    let m = FeedQuery {
                        status: Some(FeedStatus::Matched),
                        ..q.clone()
                    };
                    merge(&mut page, demo::list(d, &m)?);
                }
                Ok(Loaded {
                    counts: demo::counts_in(d, connection, q.transfer_window_days),
                    page,
                    last_import: account.and_then(|a| demo::last_import(d, a)),
                    waiting_all: demo::waiting(d),
                    drafts: if waiting {
                        receipts::load_demo(d, tenant)
                    } else {
                        None
                    },
                })
            },
            move |s| async move { live_load(s, live, accepted_too, account, waiting).await },
        ));
        self.poll();
    }

    /// Takes the answers that arrived and starts what is due.
    pub(crate) fn poll(&mut self) {
        if let Some(r) = finished(&mut self.meta) {
            match r {
                Ok(m) => {
                    let keep = self.current_account().map(|a| a.connection);
                    self.accounts = m.accounts;
                    self.account = keep
                        .and_then(|c| self.accounts.iter().position(|a| a.connection == c))
                        .unwrap_or(0);
                    self.chart = m.chart;
                    self.ai_on = m.ai_on;
                    self.view_only = self.view_only || m.view_only;
                    self.accept_route = m.accept_route;
                    self.supported = m.supported;
                    self.closing_supported = m.closing_supported;
                    self.reload();
                }
                Err(_) => self.load_failed = true,
            }
        }
        if let Some(r) = finished(&mut self.load) {
            match r {
                Ok(l) => {
                    self.load_failed = false;
                    self.counts = l.counts;
                    self.total = l.page.total;
                    self.rows = l.page.rows;
                    self.last_import = l.last_import;
                    self.list.set_rows(self.rows.iter().map(|r| r.id).collect());
                    if let Some(id) = self.focus_next.take() {
                        self.list.focus(id);
                    }
                    self.receipts.take(l.drafts, &self.rows);
                    receipts::refresh(self);
                    let n = u32::try_from(l.waiting_all).unwrap_or(u32::MAX);
                    self.link.push(CompanyEvent::BankFeedCount(n));
                }
                Err(_) => self.load_failed = true,
            }
        }
        rules::poll(self);
        if let Some(r) = finished(&mut self.act) {
            let o = r.unwrap_or_else(|_| send::Outcome {
                failed: 1,
                ..send::Outcome::default()
            });
            self.take_outcome(o);
        }
    }

    /// Shows what an action did: the password prompt, the "changed since"
    /// question, the Undo toast, a notice; the next page of a rule's rows.
    fn take_outcome(&mut self, o: send::Outcome) {
        self.list.drop_rows(&o.stale);
        // A05: a refused receipt is read again (a fresh change token, or
        // gone from the offers once the list is read); one the user may not
        // file is no longer offered on this line.
        if o.receipt_forbidden {
            receipts::forbidden(self);
        } else if o.receipt_refused {
            receipts::read_again(self);
        }
        if o.view_only {
            self.view_only = true;
        }
        let closed = o.closed.is_some();
        match o.closed {
            Some((e, again)) => {
                self.closing.failed(&e, again, self.closing_supported, None);
            }
            None => self.closing.saved(),
        }
        self.confirm.clone_from(&o.changed);
        self.accepted_as.extend(o.accepted_as.iter().copied());
        if o.undone > 0 {
            self.toast = None;
        }
        if let Some((target, rows)) = o.undo.clone() {
            self.toast = Some(Toast {
                target,
                rows,
                shown: Duration::ZERO,
            });
        }
        self.notice = if o.view_only {
            Some(("bank_feed.notice.view_only", 0))
        } else if closed || self.closing.dialog_open() {
            Some(("bank_feed.notice.closed", 0))
        } else if o.changed.is_some() {
            Some(("bank_feed.notice.undo_changed", 0))
        } else if let Some(k) = o.notice {
            Some((k, o.failed.max(o.done)))
        } else if !o.stale.is_empty() {
            Some(("bank_feed.notice.changed", o.stale.len()))
        } else if o.failed > 0 {
            Some(("bank_feed.notice.failed", o.failed))
        } else if o.skipped > 0 {
            Some(("bank_feed.notice.skipped", o.skipped))
        } else {
            Some(("bank_feed.notice.done", o.done))
        };
        if o.done > 0 {
            self.link
                .bus()
                .publish_from(&self.bus, crate::data_bus::Change::BankFeed);
            // Accepts and Undo post or remove entries: open registers and the
            // Chart of Accounts read their balances again (P26 QA-5).
            self.link
                .bus()
                .publish_from(&self.bus, crate::data_bus::Change::Ledger(None));
            // The panel reads its row again (candidates, payments).
            self.detail.row = None;
        }
        self.read_meta();
        self.reload();
        if let Some(next) = o.next_rule {
            send::start(self, send::Send::Rule(next), None);
        }
    }

    /// The badge of `r`.
    pub(crate) fn why(&self, r: &FeedRow) -> Why {
        // Rows no longer waiting say what they are (P26 QA-8), not "Needs
        // you".
        if !r.entry_voided && r.status.is_accepted() {
            return Why::Accepted;
        }
        if r.status == FeedStatus::Excluded {
            return Why::Excluded;
        }
        let bucket = r.bucket.map(|b| match b {
            cl_api::FeedBucket::Ready => model::Bucket::Ready,
            cl_api::FeedBucket::Review => model::Bucket::Review,
            cl_api::FeedBucket::Matches => model::Bucket::Matches,
        });
        let confidence_bp = r.confidence.and_then(|c| {
            let bp = (c * rust_decimal::Decimal::from(10_000)).trunc();
            u16::try_from(bp.mantissa() / 10_i128.pow(bp.scale())).ok()
        });
        model::why(
            WhyInput {
                bucket,
                source: model::Source::parse(r.match_type.as_deref()),
                has_rule: r.suggested_rule.is_some(),
                confidence_bp,
                possible_duplicate: r.possible_duplicate,
                match_candidates: r.match_candidate_count,
                // The engine's one counterpart (Q-P26y) or the demo's word.
                transfer: r.status.is_waiting()
                    && (r.transfer_candidate.is_some()
                        || r.match_type.as_deref() == Some("transfer")),
                has_account: r.assigned_account.is_some() || r.suggested_account.is_some(),
                entry_voided: r.entry_voided,
                has_receipt: !self.receipts.of(r.id).is_empty(),
            },
            self.ai_on,
        )
    }

    /// Carries out the key or button the list recorded.
    fn act_on_keys(&mut self) {
        let Some(action) = self.list.action.take() else {
            return;
        };
        match action {
            FeedAction::Accept(id) => self.accept_one(id),
            FeedAction::AcceptSelected(ids) => bulk::accept_selected(self, &ids),
            FeedAction::Rule(id) => {
                self.detail_request = Some(FeedAction::Rule(id));
                rules::from_row(self, id);
            }
            other @ FeedAction::Split(id) => {
                self.rules.open = false;
                self.detail_request = Some(other);
                // The row already in the panel goes to its split grid now;
                // another row does when the panel follows it.
                if self.detail.row == Some(id) {
                    detail::enter_split(self);
                }
            }
        }
    }

    /// A: accepts the row as the panel has it (the owner's account, split,
    /// match, transfer or deposit), else as proposed (staged or suggested
    /// account). A row that needs the owner goes to the detail panel.
    pub(crate) fn accept_one(&mut self, id: FeedItemId) {
        if self.view_only || self.act.is_some() || self.show != Show::Waiting {
            return;
        }
        let Some(r) = self.row(id).cloned() else {
            return;
        };
        if self.detail.row == Some(id)
            && let Some(x) = self.detail.ready.clone()
        {
            send::start(self, x, None);
            return;
        }
        let why = self.why(&r);
        let has_account = r.assigned_account.is_some() || r.suggested_account.is_some();
        let x = match why {
            Why::Match
            | Why::Transfer
            | Why::Duplicate
            | Why::NeedsYou
            | Why::EntryVoided
            | Why::Receipt => None,
            _ if r.status == FeedStatus::Assigned => Some(send::Send::Categorize(id, None)),
            // As proposed through the one accept route, so a split a rule
            // staged posts as staged (P26 review m3).
            _ if has_account && self.accepted_filter() => {
                Some(send::Send::Accept(vec![cl_api::AcceptItem::AsProposed(id)]))
            }
            _ if has_account => Some(send::Send::Categorize(
                id,
                Some(FeedChoice {
                    account: r.suggested_account.as_ref().map(|a| a.id),
                    contact: r.suggested_contact.as_ref().map(|c| c.id),
                    ..FeedChoice::default()
                }),
            )),
            _ => None,
        };
        if let Some(x) = x {
            send::start(self, x, None);
        } else {
            // A never does nothing silently (P26 QA-24, QA-18): the panel
            // says what's missing.
            self.detail_request = Some(FeedAction::Accept(id));
            self.list.focus(id);
            let key = if why == Why::Receipt && !has_account {
                "bank_feed.notice.pick_receipt_account"
            } else {
                incomplete_notice(why)
            };
            self.notice = Some((key, 0));
        }
    }

    fn dev_command(&mut self, id: &str) -> bool {
        let Some(mut d) = self.link.demo_state() else {
            return false;
        };
        match id {
            "dev.demo_feed_view_only" => d.feed_review.view_only = true,
            "dev.demo_feed_conflict" => d.feed_review.conflict_next = true,
            "dev.demo_feed_void_entry" => {
                demo::void_first_accepted(&mut d);
            }
            "dev.demo_feed_duplicate" => {
                demo::mark_duplicate(&mut d);
            }
            "dev.demo_feed_next_file" => {
                demo::add_next_file(&mut d);
            }
            // The books closed through 11/01 (the sample's first bank day).
            "dev.demo_feed_close_books" => {
                if let Some(open) = d.open
                    && let Some(c) = d.companies.iter_mut().find(|c| c.tenant_id == open)
                {
                    c.closed_through = CalendarDate::from_ymd(2026, 11, 1).ok();
                }
            }
            // A05: a receipt in the review queue that matches a waiting
            // money-out line (its two lines add up to the bank amount).
            "dev.demo_feed_receipt" => {
                // The cursor goes to its line once the list is read.
                self.focus_next = demo::seed_receipt(&mut d, true).map(|(line, _)| line);
            }
            // A05 QA-1: the same on a line nothing was suggested for.
            "dev.demo_feed_receipt_bare" => {
                self.focus_next = demo::seed_receipt_on(&mut d, demo::SeedLine::Bare, false)
                    .map(|(line, _)| line);
            }
            // A05 QA-5: two copies of one receipt for the same line.
            "dev.demo_feed_receipt_two" => {
                self.focus_next = demo::seed_two_receipts(&mut d).map(|(line, _)| line);
            }
            "dev.demo_ai_off" => {
                d.ai.enabled = false;
            }
            "dev.demo_feed_reset" => {
                demo::reset(&mut d);
                demo::discard_seeded(&mut d);
                // The window starts over too (scenarios run light, then dark).
                self.segment = Segment::All;
                self.show = Show::Waiting;
                self.voided_only = false;
                self.filter.clear();
                self.offset = 0;
                self.view_only = false;
                self.notice = None;
                self.detail_request = None;
                self.list = ReviewList::default();
                self.detail = detail::Detail::default();
                self.toast = None;
                self.confirm = None;
                self.closing.cancel();
                self.rules = rules::RulesPanel::default();
                self.receipts = receipts::Receipts::default();
                if let Some(open) = d.open
                    && let Some(c) = d.companies.iter_mut().find(|c| c.tenant_id == open)
                {
                    c.closed_through = CalendarDate::from_ymd(2026, 9, 30).ok();
                }
            }
            _ => return false,
        }
        drop(d);
        self.read_meta();
        self.reload();
        true
    }
}

/// The bank's own text for "From the bank" (P26 QA-13, QA-2): the raw
/// description, as the import preview and the mockup show it. The engine's
/// cleaned name cuts some to a word ("ONLINE TRANSFER TO CARD" reads
/// "Online", "CHECK 1005" reads "Check"), so it is only the fallback.
pub(crate) fn bank_text(r: &FeedRow) -> &str {
    r.original_description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .unwrap_or(&r.description)
}

/// What A says when the row isn't complete yet (P26 QA-24, QA-18).
fn incomplete_notice(why: Why) -> &'static str {
    match why {
        Why::Match => "bank_feed.notice.pick_match",
        Why::Transfer => "bank_feed.notice.pick_transfer",
        Why::Duplicate => "bank_feed.notice.pick_duplicate",
        Why::EntryVoided => "bank_feed.notice.pick_voided",
        Why::Receipt => "bank_feed.notice.pick_receipt",
        _ => "bank_feed.notice.pick_account",
    }
}

/// The transfer window from the company's preferences (Q-P26l).
fn transfer_window(link: &CompanyLink, tenant: Option<TenantId>) -> u8 {
    tenant
        .and_then(|t| link.store().load().ok().and_then(|l| l.get(t).cloned()))
        .map_or(cl_settings::FEED_TRANSFER_WINDOW_DEFAULT, |r| {
            r.prefs.transfer_window()
        })
}

fn merge(into: &mut cl_api::FeedPage, more: cl_api::FeedPage) {
    into.total += more.total;
    into.skipped += more.skipped;
    into.rows.extend(more.rows);
    into.rows
        .sort_by(|a, b| b.date.cmp(&a.date).then(a.id.cmp(&b.id)));
}

fn chart_names(accounts: Vec<cl_api::Account>) -> Vec<(AccountId, String)> {
    let mut v: Vec<(AccountId, String)> = accounts
        .into_iter()
        .filter(|a| a.is_active)
        .map(|a| {
            let shown = crate::account_names::shown_name(&a);
            let name = match a.fields.account_number.as_deref().map(str::trim) {
                Some(n) if !n.is_empty() => format!("{n} \u{b7} {shown}"),
                _ => shown,
            };
            (a.id, name)
        })
        .collect();
    v.sort_by(|a, b| a.1.cmp(&b.1));
    v
}

fn label_of(
    account: Option<AccountId>,
    name: Option<&str>,
    chart: &[(AccountId, String)],
) -> String {
    account
        .and_then(|a| chart.iter().find(|(id, _)| *id == a))
        .map(|(_, n)| n.clone())
        .or_else(|| name.map(str::to_string))
        .unwrap_or_default()
}

async fn live_meta(
    s: Session,
    tenant: Option<TenantId>,
    under_cap: bool,
) -> Result<Meta, ApiError> {
    let caps = s.client().capabilities();
    let supported = caps.has(Capabilities::FEED_REVIEW);
    let chart = chart_names(
        s.accounts()
            .list_all(&cl_api::AccountFilter::default())
            .await?,
    );
    let mut accounts = Vec::new();
    if supported {
        for c in s.feed().connections().await? {
            let waiting = s.feed().counts(Some(c.id)).await?.waiting;
            accounts.push(FeedAccount {
                connection: c.id,
                account: c.account,
                label: label_of(c.account, c.account_name.as_deref(), &chart),
                waiting,
            });
        }
    }
    let config = s.ai_config().await.ok();
    let consent = s.company_ai_disclosure().await.ok();
    let ai_on =
        crate::forms::suggest::gate_of(config.as_ref(), consent.as_ref(), under_cap).is_open();
    // A `readonly` user reads but can't act (practice routes answer 403).
    let view_only = match tenant {
        Some(t) => s
            .companies()
            .await
            .is_ok_and(|c| c.iter().any(|x| x.tenant_id == t && x.role == "readonly")),
        None => false,
    };
    Ok(Meta {
        accounts,
        chart,
        ai_on,
        view_only,
        accept_route: caps.has(Capabilities::FEED_ACCEPT),
        supported,
        closing_supported: caps.has(Capabilities::FEED_CLOSING_DATE),
    })
}

async fn live_load(
    s: Session,
    q: FeedQuery,
    accepted_too: bool,
    account: Option<AccountId>,
    waiting: bool,
) -> Result<Loaded, ApiError> {
    let mut q = q;
    if !s
        .client()
        .capabilities()
        .has(Capabilities::FEED_TRANSFER_WINDOW)
    {
        q.transfer_window_days = None;
    }
    let f = s.feed();
    let mut page = f.list(&q).await?;
    if accepted_too {
        let m = FeedQuery {
            status: Some(FeedStatus::Matched),
            ..q.clone()
        };
        merge(&mut page, f.list(&m).await?);
    }
    let counts = f.counts_in(q.connection, q.transfer_window_days).await?;
    let waiting_all = f.counts(None).await?.waiting;
    let last_import = match account {
        Some(a) => s
            .bank_import()
            .batches(Some(a), None)
            .await
            .ok()
            .and_then(|b| {
                b.into_iter()
                    .filter(|x| x.undone_at.is_none())
                    .find_map(|x| x.file_ending_date)
            }),
        None => None,
    };
    let drafts = if waiting {
        receipts::load_live(&s).await
    } else {
        None
    };
    Ok(Loaded {
        counts,
        page,
        last_import,
        waiting_all,
        drafts,
    })
}

impl Screen for BankFeedScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        // Back on this tab after another one (Preferences, a register):
        // the transfer window and the list are read again (P26 QA-16).
        let pass = ctx.cumulative_pass_nr();
        if self.drawn_at.is_some_and(|p| pass > p + 1) {
            self.follow_transfer_window();
            self.reload();
        }
        self.drawn_at = Some(pass);
        self.poll();
        let changes = self.link.bus().take(&mut self.bus);
        if !changes.is_empty() {
            // A new bank file (another window): the last action's notice no
            // longer describes the list (P26 QA-21).
            if changes.bank_feed {
                self.notice = None;
            }
            self.read_meta();
            self.reload();
        }
        if self.loaded_at.is_none_or(|at| at.elapsed() >= REFRESH) {
            self.reload();
        }
        self.act_on_keys();
        detail::follow(self);
        receipts::follow(self, &ctx);
        if self.load.is_some() || self.meta.is_some() || self.act.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else {
            ctx.request_repaint_after(REFRESH);
        }
        view::draw(self, ui);
    }

    fn scope(&self) -> Scope {
        Scope::Feed
    }

    fn command(&mut self, id: CommandId) -> bool {
        let s = id.as_str();
        if s.starts_with("dev.") {
            return self.dev_command(s);
        }
        if let Some(done) = self.closing.command(id) {
            return done;
        }
        if matches!(
            s,
            "form.insert_line" | "form.delete_line" | "feed.insert_line" | "feed.delete_line"
        ) {
            return detail::line_command(self, s);
        }
        if s == "form.revert" && self.confirm.take().is_some() {
            return true;
        }
        if s == "form.revert" && self.detail.find.take().is_some() {
            return true;
        }
        if s == "feed.choose_account" {
            return detail::choose_account(self);
        }
        if s.starts_with("feed.") {
            // Writes are refused at once for a view-only user; moving and
            // selecting still work.
            if self.view_only && matches!(s, "feed.accept" | "feed.accept_selected" | "feed.rule") {
                return true;
            }
            return self.list.command(s);
        }
        false
    }

    fn document(&self) -> Option<crate::windows::doc_viewer::DocRef> {
        receipts::document(self)
    }

    fn keeps_text_key(&self, ctx: &egui::Context, s: cl_model::Shortcut) -> bool {
        // Ctrl+Insert / Ctrl+Delete reach the split and fee grids from
        // their amount and memo boxes.
        let line_key = s.mods == cl_model::Mods::CMD
            && matches!(s.key, cl_model::Key::Insert | cl_model::Key::Delete);
        ctx.text_edit_focused() && !line_key
    }

    fn state(&self) -> Value {
        let mut badges = serde_json::Map::new();
        for w in [
            Why::Rule,
            Why::Match,
            Why::Transfer,
            Why::Ai { high: true },
            Why::Ai { high: false },
            Why::History,
            Why::Duplicate,
            Why::NeedsYou,
            Why::EntryVoided,
            Why::Receipt,
        ] {
            badges.insert(w.as_str().into(), json!(0));
        }
        for r in &self.rows {
            let k = self.why(r).as_str();
            let n = badges.get(k).and_then(Value::as_u64).unwrap_or(0);
            badges.insert(k.into(), json!(n + 1));
        }
        let ai_badges = self.rows.iter().filter(|r| self.why(r).is_ai()).count();
        json!({
            "screen": "bank_feed",
            "supported": self.supported,
            "account": self.current_account().map(|a| a.label.clone()),
            "accounts": self.accounts.len(),
            "segment": self.segment.as_str(),
            "show": self.show.as_str(),
            "entry_voided_only": self.voided_only,
            "counts": {
                "waiting": self.counts.waiting,
                "ready": self.counts.ready,
                "review": self.counts.review,
                "matches": self.counts.matches,
                "accepted": self.counts.accepted,
                "entry_voided": self.counts.entry_voided,
                "excluded": self.counts.excluded,
            },
            "rows": self.rows.len(),
            "total": self.total,
            "cursor": self.list.cursor(),
            "selected": self.list.selected().len(),
            "badges": badges,
            "ai_badges": ai_badges,
            "ai_on": self.ai_on,
            "view_only": self.view_only,
            "accept_route": self.accept_route,
            "busy": self.act.is_some() || self.load.is_some(),
            "load_failed": self.load_failed,
            "notice": self.notice.map(|(k, _)| k),
            "detail": detail::state(self),
            "rules": rules::state(self),
            "receipts": receipts::state(self),
            "toast": self.toast.as_ref().map(|x| x.rows),
            "undo_changed": self.confirm.as_ref().map(|c| c.1),
            "closing": self.closing.state_json(self.closing_supported),
            "last_send": self.last_send,
            "transfer_window": self.transfer_window,
            "detail_request": self.detail_request.as_ref().map(|a| match a {
                FeedAction::Accept(_) => "accept",
                FeedAction::Split(_) => "split",
                FeedAction::Rule(_) => "rule",
                FeedAction::AcceptSelected(_) => "accept_selected",
            }),
        })
    }
}

#[cfg(test)]
mod tests;
