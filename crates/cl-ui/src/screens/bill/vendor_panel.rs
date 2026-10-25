//! The vendor panel beside the Enter Bill form and the duplicate-ref
//! warning (P17, plan §3 WP8, D3, Q-P17m, Q-P17n).
//!
//! Rows in mockup order: Open balance (the engine's `vendor-summary
//! .totalOwed`, shown as sent), Default account, 1099 vendor, then the
//! duplicate line ("No other bill uses ref. no. X ✓", or the other bill
//! with a link that opens it). Then Q-P17m's additions: open bills (the
//! engine's count and total, up to five listed), unused credits (the
//! summary's count and total) and recent bills.
//!
//! Open bills come from `GET /bills?billStatus=unpaid,partial,overdue`, not
//! from `/bills/payable`, which leaves not-received bills out while the
//! open balance counts them (process review S4). Nothing here is added up
//! (non-negotiable 7): every amount is one the engine sent.
//!
//! The duplicate check (D3) runs 400 ms after the vendor, ref or date last
//! changed: `GET /bills?contactId&search=<ref>&startDate&endDate` (or
//! `/vendor-credits`) over the ±18-month window, at most 20 pages of 50,
//! then cl-model's exact filter (`find_duplicates`: trimmed, case-folded,
//! posted, another id). If the pages run out the panel says "Couldn't check
//! every bill". On Save with a duplicate the form asks "Save anyway / Go
//! back"; it warns, it never blocks (acceptance 2).
//!
//! The panel reads its vendor again on the data bus's `BillsChanged
//! {vendor}` and after this window's own saves (the screen tells it
//! directly; the bus never echoes to the sender).
//!
//! # Entry points (fixed by WP0; WP8 added fields and variants)
//! - [`VendorPanel::tick`] every frame, drawn or not.
//! - [`show`] draws the panel and the duplicate question.
//! - [`VendorPanel::bills_changed`] with the bus's [`Changes`].
//! - [`VendorPanel::clear_to_save`] before a save is sent;
//!   [`VendorPanel::take_resume`] the save to send once the check or the
//!   owner answered.
//!
//! Strings are `bill.panel.*` in `i18n/bill_panel.rs`. No amounts, names or
//! refs in logs or the control-channel state.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cl_api::{
    Account, BillListQuery, BillRecord, BillSort, Capabilities, Contact, CreditListQuery,
    CreditSort, PageRequest, RequestGroup, SortDir, TxnStatus, VendorSummary,
};
use cl_core::ids::{ContactId, TransactionId};
use cl_core::{CalendarDate, Money, MoneyStyle};
use cl_model::forms::SaveIntent;
use cl_model::forms::bill::{BillKind, DuplicateCandidate, duplicate_window, find_duplicates};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyLink, Task};
use crate::data_bus::Changes;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;

/// The quiet time after the last change before the duplicate check runs.
pub const DEBOUNCE: Duration = Duration::from_millis(400);
/// Rows per page of the duplicate lookup (D3).
pub const DUPLICATE_PAGE: u16 = 50;
/// Most pages the duplicate lookup reads (D3).
pub const DUPLICATE_PAGES: u32 = 20;
/// Open bills listed.
pub const OPEN_SHOWN: usize = 5;
/// Recent bills listed.
pub const RECENT_SHOWN: usize = 5;
/// Open bills read (void rows are dropped before five are kept).
const OPEN_READ: u16 = 25;
/// Recent bills read (void rows are dropped before five are kept).
const RECENT_READ: u16 = 15;

/// What the panel needs from the form, each frame.
pub struct PanelInput<'a> {
    /// The company.
    pub link: &'a Arc<CompanyLink>,
    /// Bill or vendor credit (the duplicate check looks at the same kind).
    pub kind: BillKind,
    /// The vendor picked, if any.
    pub vendor: Option<ContactId>,
    /// The Ref. no. as typed.
    pub ref_no: &'a str,
    /// The bill date, when it parses.
    pub date: Option<CalendarDate>,
    /// The saved bill or credit being edited (never its own duplicate).
    pub editing: Option<TransactionId>,
    /// The vendor's record (name, default account, 1099), once the list
    /// has it.
    pub contact: Option<&'a Contact>,
    /// The chart (the default account's name).
    pub accounts: &'a [Account],
    /// The owner's money display style.
    pub money_style: MoneyStyle,
}

/// A posted bill or credit of the same vendor that already uses the ref.
#[derive(Clone, PartialEq, Eq)]
pub struct DuplicateRef {
    /// The other record.
    pub id: TransactionId,
    /// Its ref as stored (owner data).
    pub ref_no: String,
    /// Its date.
    pub date: CalendarDate,
    /// Its number in the books (`BILL-00012`), when the engine sent one.
    pub number: Option<String>,
}

/// Ids only.
impl std::fmt::Debug for DuplicateRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DuplicateRef")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// What the panel asks the form to do this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PanelEvent {
    /// Nothing.
    #[default]
    None,
    /// Open this bill or credit (a recent bill, an open bill, the
    /// duplicate's link).
    OpenRecord(BillKind, TransactionId),
    /// The owner answered the duplicate question with Go back: put the
    /// keyboard in Ref. no.
    GoBack,
}

/// One bill as the panel lists it: the engine's figures as sent.
#[derive(Clone, PartialEq, Eq)]
pub struct PanelBill {
    /// The bill.
    pub id: TransactionId,
    /// Book number.
    pub number: Option<String>,
    /// The vendor's invoice number (owner data).
    pub reference: Option<String>,
    /// Bill date.
    pub date: CalendarDate,
    /// Due date.
    pub due: Option<CalendarDate>,
    /// Open bills: what is still owed; recent bills: the total.
    pub amount: Option<Money>,
}

/// Ids and dates only.
impl std::fmt::Debug for PanelBill {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PanelBill")
            .field("id", &self.id)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

impl PanelBill {
    /// The ref, else the book number.
    fn label(&self) -> String {
        self.reference
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .or(self.number.as_deref())
            .unwrap_or_default()
            .to_string()
    }
}

/// A card's data: loading, loaded, or failed.
enum Card<T> {
    Loading(Task<T>),
    Ready(T),
    Failed,
}

impl<T> Card<T> {
    fn of(task: Option<Task<T>>) -> Card<T> {
        task.map_or(Card::Failed, Card::Loading)
    }

    fn poll(&mut self) {
        if let Card::Loading(task) = self
            && let Some(r) = task.try_take()
        {
            *self = match r {
                Ok(v) => Card::Ready(v),
                Err(_) => Card::Failed,
            };
        }
    }

    fn ready(&self) -> Option<&T> {
        match self {
            Card::Ready(v) => Some(v),
            _ => None,
        }
    }

    fn loading(&self) -> bool {
        matches!(self, Card::Loading(_))
    }

    fn name(&self) -> &'static str {
        match self {
            Card::Loading(_) => "loading",
            Card::Ready(_) => "ready",
            Card::Failed => "failed",
        }
    }
}

/// The reads for one vendor.
struct Cards {
    vendor: ContactId,
    summary: Card<VendorSummary>,
    open: Card<Vec<PanelBill>>,
    recent: Card<Vec<PanelBill>>,
}

impl Cards {
    fn busy(&self) -> bool {
        self.summary.loading() || self.open.loading() || self.recent.loading()
    }

    fn poll(&mut self) {
        self.summary.poll();
        self.open.poll();
        self.recent.poll();
    }
}

/// What the duplicate check is for.
#[derive(Clone, PartialEq, Eq)]
struct DupKey {
    kind: BillKind,
    vendor: ContactId,
    /// Trimmed, as typed.
    reference: String,
    date: CalendarDate,
    own: Option<TransactionId>,
}

impl DupKey {
    fn of(input: &PanelInput<'_>) -> Option<DupKey> {
        let reference = input.ref_no.trim();
        if reference.is_empty() {
            return None;
        }
        Some(DupKey {
            kind: input.kind,
            vendor: input.vendor?,
            reference: reference.to_string(),
            date: input.date?,
            own: input.editing,
        })
    }
}

/// What the check found.
#[derive(Clone, Debug, PartialEq, Eq)]
struct DupFound {
    found: Option<DuplicateRef>,
    /// Every row in the window was looked at.
    complete: bool,
}

/// The duplicate check.
#[derive(Default)]
enum Dup {
    /// Nothing to check (no vendor, ref or date).
    #[default]
    Off,
    /// Waiting for the typing to stop.
    Waiting { key: DupKey, since: Instant },
    /// Reading.
    Running { key: DupKey, task: Task<DupFound> },
    /// Answered.
    Done { key: DupKey, found: DupFound },
}

impl Dup {
    fn key(&self) -> Option<&DupKey> {
        match self {
            Dup::Off => None,
            Dup::Waiting { key, .. } | Dup::Running { key, .. } | Dup::Done { key, .. } => {
                Some(key)
            }
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Dup::Off => "off",
            Dup::Waiting { .. } => "waiting",
            Dup::Running { .. } => "checking",
            Dup::Done { found, .. } if found.found.is_some() => "found",
            Dup::Done { found, .. } if !found.complete => "unchecked",
            Dup::Done { .. } => "clear",
        }
    }
}

/// The panel's state.
#[derive(Default)]
pub struct VendorPanel {
    vendor: Option<ContactId>,
    group: Option<RequestGroup>,
    cards: Option<Cards>,
    /// The same vendor read again after a save (the shown cards stay until
    /// these arrive).
    next: Option<Cards>,
    dup: Dup,
    /// Run the waiting check now (a save is waiting for it).
    flush: bool,
    /// A save waiting for the check.
    waiting_save: Option<SaveIntent>,
    /// The duplicate question is open for this save.
    prompt: Option<SaveIntent>,
    /// The save to send now (the check found nothing, or Save anyway).
    resume: Option<SaveIntent>,
    /// The duplicate the owner chose to save anyway beside.
    confirmed: Option<TransactionId>,
    /// The vendor's bills changed: read the cards and check again.
    reread: bool,
}

/// Flags only.
impl std::fmt::Debug for VendorPanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VendorPanel")
            .field("vendor", &self.vendor.is_some())
            .field("duplicate", &self.dup.name())
            .field("prompt", &self.prompt.is_some())
            .finish_non_exhaustive()
    }
}

impl VendorPanel {
    /// Start, poll and debounce the reads for `input` (every frame).
    pub fn tick(&mut self, input: &PanelInput<'_>, now: Instant) {
        if self.vendor != input.vendor {
            self.vendor = input.vendor;
            self.next = None;
            self.confirmed = None;
            self.cards = input.vendor.map(|v| self.read_cards(input.link, v));
            self.reread = false;
        }
        if std::mem::take(&mut self.reread)
            && let Some(v) = self.vendor
        {
            self.next = Some(self.read_cards(input.link, v));
        }
        if let Some(c) = self.cards.as_mut() {
            c.poll();
        }
        if let Some(n) = self.next.as_mut() {
            n.poll();
            if !n.busy() {
                let n = self.next.take();
                if n.as_ref().map(|n| n.vendor) == self.vendor {
                    self.cards = n;
                }
            }
        }
        self.sync_key(input, now);
        self.run_check(input.link, now);
        if self.waiting_save.is_some()
            && matches!(self.dup, Dup::Off | Dup::Done { .. })
            && let Some(intent) = self.waiting_save.take()
        {
            if self.unconfirmed_duplicate().is_some() {
                self.prompt = Some(intent);
            } else {
                self.resume = Some(intent);
            }
        }
    }

    /// The check starts over when the vendor, ref, date or kind changed.
    fn sync_key(&mut self, input: &PanelInput<'_>, now: Instant) {
        let key = DupKey::of(input);
        if key.as_ref() != self.dup.key() {
            self.dup = match key {
                None => Dup::Off,
                Some(key) => Dup::Waiting { key, since: now },
            };
        }
    }

    fn run_check(&mut self, link: &Arc<CompanyLink>, now: Instant) {
        match &mut self.dup {
            Dup::Waiting { key, since }
                if self.flush || now.saturating_duration_since(*since) >= DEBOUNCE =>
            {
                let key = key.clone();
                self.flush = false;
                let task = self.start_check(link, &key);
                self.dup = Dup::Running { key, task };
                // A ready (demo) answer lands this frame.
                self.run_check(link, now);
            }
            Dup::Running { key, task } => {
                if let Some(r) = task.try_take() {
                    let found = r.unwrap_or(DupFound {
                        found: None,
                        complete: false,
                    });
                    self.dup = Dup::Done {
                        key: key.clone(),
                        found,
                    };
                }
            }
            _ => {}
        }
    }

    fn group(&mut self, link: &CompanyLink) -> Option<&RequestGroup> {
        if self.group.is_none() {
            self.group = link.group().and_then(Result::ok);
        }
        self.group.as_ref()
    }

    /// The duplicate lookup for `key` (D3).
    fn start_check(&mut self, link: &Arc<CompanyLink>, key: &DupKey) -> Task<DupFound> {
        let credit = key.kind == BillKind::Credit;
        if let Some(mut demo) = link.demo_state() {
            let rows = demo_records(&mut demo, key.vendor, credit);
            drop(demo);
            let rows: Vec<(DuplicateCandidate, Option<String>)> = rows
                .iter()
                .filter_map(|r| demo_candidate(r, key.vendor))
                .collect();
            return Task::ready(found_in(key, &rows, true));
        }
        // Credit rows carry their reference only with `ap-edit-v1` (S6):
        // without it the check can't look.
        if credit && !link.capabilities().has(Capabilities::AP_EDIT) {
            return Task::ready(DupFound {
                found: None,
                complete: false,
            });
        }
        let Some(session) = link.session() else {
            return Task::ready(DupFound {
                found: None,
                complete: false,
            });
        };
        let Some(group) = self.group(link) else {
            return Task::ready(DupFound {
                found: None,
                complete: false,
            });
        };
        let key = key.clone();
        let (start, end) = duplicate_window(key.date);
        let pending = group.read(&session, move |s| async move {
            let (rows, complete) = if credit {
                let q = CreditListQuery {
                    vendor: Some(key.vendor),
                    search: Some(key.reference.clone()),
                    start: Some(start),
                    end: Some(end),
                    sort: Some((CreditSort::Date, SortDir::Desc)),
                };
                let c = s
                    .vendor_credits()
                    .list_capped(&q, DUPLICATE_PAGE, DUPLICATE_PAGES)
                    .await?;
                let rows = c
                    .rows
                    .into_iter()
                    .filter_map(|r| {
                        let cand = DuplicateCandidate {
                            id: r.id,
                            vendor: r.contact_id?,
                            date: r.txn_date,
                            reference: r.vendor_invoice_number?,
                            posted: r.status == TxnStatus::Posted,
                        };
                        Some((cand, r.number))
                    })
                    .collect::<Vec<_>>();
                (rows, c.complete)
            } else {
                let q = BillListQuery {
                    vendor: Some(key.vendor),
                    search: Some(key.reference.clone()),
                    start: Some(start),
                    end: Some(end),
                    sort: Some((BillSort::Date, SortDir::Desc)),
                    ..BillListQuery::default()
                };
                let c = s
                    .bills()
                    .list_capped(&q, DUPLICATE_PAGE, DUPLICATE_PAGES)
                    .await?;
                let rows = c
                    .rows
                    .into_iter()
                    .filter_map(|r| {
                        let cand = DuplicateCandidate {
                            id: r.id,
                            vendor: r.contact_id?,
                            date: r.txn_date,
                            reference: r.vendor_invoice_number?,
                            posted: r.status == TxnStatus::Posted,
                        };
                        Some((cand, r.number))
                    })
                    .collect::<Vec<_>>();
                (rows, c.complete)
            };
            Ok(found_in(&key, &rows, complete))
        });
        match pending {
            Ok(p) => Task::Pending(p),
            Err(_) => Task::ready(DupFound {
                found: None,
                complete: false,
            }),
        }
    }

    /// The summary, open bills and recent bills of `vendor`.
    fn read_cards(&mut self, link: &Arc<CompanyLink>, vendor: ContactId) -> Cards {
        if let Some(mut demo) = link.demo_state() {
            let summary = crate::company::demo::bills::vendor_summary(&mut demo, vendor);
            let rows = demo_records(&mut demo, vendor, false);
            drop(demo);
            let (open, recent) = demo_lists(&rows);
            return Cards {
                vendor,
                summary: Card::Ready(summary),
                open: Card::Ready(open),
                recent: Card::Ready(recent),
            };
        }
        let session = link.session();
        let group = self.group(link);
        let (Some(session), Some(group)) = (session, group) else {
            return Cards {
                vendor,
                summary: Card::Failed,
                open: Card::Failed,
                recent: Card::Failed,
            };
        };
        let summary = group
            .read(&session, move |s| async move {
                s.bills().vendor_summary(vendor).await
            })
            .ok()
            .map(Task::Pending);
        let open = group
            .read(&session, move |s| async move {
                let q = BillListQuery {
                    vendor: Some(vendor),
                    open_only: true,
                    sort: Some((BillSort::DueDate, SortDir::Asc)),
                    ..BillListQuery::default()
                };
                let page = s.bills().list(&q, PageRequest::new(OPEN_READ, 0)?).await?;
                Ok(open_bills(&page.data))
            })
            .ok()
            .map(Task::Pending);
        let recent = group
            .read(&session, move |s| async move {
                let q = BillListQuery {
                    vendor: Some(vendor),
                    sort: Some((BillSort::Date, SortDir::Desc)),
                    ..BillListQuery::default()
                };
                let page = s
                    .bills()
                    .list(&q, PageRequest::new(RECENT_READ, 0)?)
                    .await?;
                Ok(recent_bills(&page.data))
            })
            .ok()
            .map(Task::Pending);
        Cards {
            vendor,
            summary: Card::of(summary),
            open: Card::of(open),
            recent: Card::of(recent),
        }
    }

    /// Read again what `changes` touched (the data bus, Q-262): the
    /// vendor's cards (the shown ones stay until the new ones arrive) and
    /// the duplicate check.
    pub fn bills_changed(&mut self, changes: &Changes) {
        if self
            .dup
            .key()
            .is_some_and(|k| changes.bills_touch(k.vendor))
        {
            // The saved bill may now be the duplicate (or no longer be one).
            self.dup = Dup::Off;
        }
        if self.vendor.is_some_and(|v| changes.bills_touch(v)) {
            self.reread = true;
        }
    }

    /// The other bill that uses this ref, once the check found one.
    #[must_use]
    pub fn duplicate(&self) -> Option<&DuplicateRef> {
        match &self.dup {
            Dup::Done { found, .. } => found.found.as_ref(),
            _ => None,
        }
    }

    /// A duplicate the owner has not chosen to save beside yet.
    fn unconfirmed_duplicate(&self) -> Option<&DuplicateRef> {
        self.duplicate().filter(|d| self.confirmed != Some(d.id))
    }

    /// True when the duplicate check could not look at every bill (D3).
    #[must_use]
    pub fn duplicate_unchecked(&self) -> bool {
        matches!(&self.dup, Dup::Done { found, .. } if found.found.is_none() && !found.complete)
    }

    /// Before a save is sent: true to go on. False when the duplicate
    /// question is open, or while the check for what the form now holds is
    /// still running (the save is sent, or asked about, once it answers:
    /// [`VendorPanel::take_resume`]). Never a block: Save anyway sends it.
    pub fn clear_to_save(&mut self, input: &PanelInput<'_>, intent: SaveIntent) -> bool {
        let now = Instant::now();
        self.sync_key(input, now);
        self.prompt = None;
        self.resume = None;
        self.waiting_save = None;
        if matches!(self.dup, Dup::Waiting { .. } | Dup::Running { .. }) {
            // Don't wait for the typing pause: check now (a ready answer
            // lands at once).
            self.flush = true;
            self.run_check(input.link, now);
        }
        match &self.dup {
            Dup::Off => true,
            Dup::Done { .. } => {
                if self.unconfirmed_duplicate().is_some() {
                    self.prompt = Some(intent);
                    false
                } else {
                    true
                }
            }
            Dup::Waiting { .. } | Dup::Running { .. } => {
                self.waiting_save = Some(intent);
                false
            }
        }
    }

    /// The save to send now: the check found nothing, or the owner chose
    /// Save anyway.
    pub fn take_resume(&mut self) -> Option<SaveIntent> {
        self.resume.take()
    }

    /// The duplicate question is open.
    #[must_use]
    pub fn asking(&self) -> bool {
        self.prompt.is_some()
    }

    /// Esc: the question closes as Go back. True when it was open.
    pub fn go_back(&mut self) -> bool {
        self.waiting_save = None;
        self.prompt.take().is_some()
    }

    /// True while a read or the duplicate check is running (or waiting for
    /// the typing to stop).
    #[must_use]
    pub fn busy(&self) -> bool {
        self.reread
            || self.cards.as_ref().is_some_and(Cards::busy)
            || self.next.as_ref().is_some_and(Cards::busy)
            || matches!(self.dup, Dup::Waiting { .. } | Dup::Running { .. })
    }

    /// The control channel's view (no amounts, names or refs).
    #[must_use]
    pub fn state_json(&self) -> Value {
        let cards = self.cards.as_ref();
        let summary = cards.and_then(|c| c.summary.ready());
        json!({
            "vendor": self.vendor.is_some(),
            "summary": cards.map(|c| c.summary.name()),
            "open_bills_count": summary.map(|s| s.unpaid_count),
            "open_bills_shown": cards.and_then(|c| c.open.ready()).map(Vec::len),
            "credits_count": summary.map(|s| s.available_credits_count),
            "recent_bills": cards.and_then(|c| c.recent.ready()).map(Vec::len),
            "duplicate": self.duplicate().is_some(),
            "duplicate_check": self.dup.name(),
            "duplicate_unchecked": self.duplicate_unchecked(),
            "duplicate_prompt": self.prompt.is_some(),
            "save_waiting": self.waiting_save.is_some(),
        })
    }
}

/// The duplicate in `rows` for `key` (cl-model's exact rule), with its
/// book number.
fn found_in(
    key: &DupKey,
    rows: &[(DuplicateCandidate, Option<String>)],
    complete: bool,
) -> DupFound {
    let cands: Vec<DuplicateCandidate> = rows.iter().map(|(c, _)| c.clone()).collect();
    let found = find_duplicates(key.vendor, &key.reference, key.date, key.own, &cands)
        .first()
        .map(|c| DuplicateRef {
            id: c.id,
            ref_no: c.reference.clone(),
            date: c.date,
            number: rows
                .iter()
                .find(|(r, _)| r.id == c.id)
                .and_then(|(_, n)| n.clone()),
        });
    DupFound { found, complete }
}

/// Open bills from a `/bills?billStatus=…` page: posted, something owed,
/// soonest due first as the engine sorted them; five kept.
fn open_bills(rows: &[cl_api::BillSummary]) -> Vec<PanelBill> {
    rows.iter()
        .filter(|r| r.status == TxnStatus::Posted)
        .filter(|r| r.balance_due.is_some_and(Money::is_positive))
        .take(OPEN_SHOWN)
        .map(|r| PanelBill {
            id: r.id,
            number: r.number.clone(),
            reference: r.vendor_invoice_number.clone(),
            date: r.txn_date,
            due: r.due_date,
            amount: r.balance_due,
        })
        .collect()
}

/// Recent bills from a `/bills?sortBy=date&sortDir=desc` page: posted;
/// five kept.
fn recent_bills(rows: &[cl_api::BillSummary]) -> Vec<PanelBill> {
    rows.iter()
        .filter(|r| r.status == TxnStatus::Posted)
        .take(RECENT_SHOWN)
        .map(|r| PanelBill {
            id: r.id,
            number: r.number.clone(),
            reference: r.vendor_invoice_number.clone(),
            date: r.txn_date,
            due: r.due_date,
            amount: r.total,
        })
        .collect()
}

/// The demo engine's bills (or credits) of `vendor`, newest first, void
/// ones included (as `GET /bills?contactId` lists them).
fn demo_records(
    state: &mut crate::company::demo::DemoState,
    vendor: ContactId,
    credit: bool,
) -> Vec<BillRecord> {
    use crate::company::demo::bills;
    use crate::company::demo::register::DemoLedger;
    // The demo ledger keeps the vendor by name.
    let mut names: Vec<String> = state
        .contacts_mut()
        .iter()
        .filter(|c| c.id == vendor)
        .map(|c| c.fields.display_name.clone())
        .collect();
    names.extend(state.contact_names.get(&vendor).cloned());
    let kind = if credit { "vendor_credit" } else { "bill" };
    let mut txns: Vec<_> = state
        .ledger
        .transactions()
        .into_iter()
        .filter(|t| t.kind == kind && t.contact.as_ref().is_some_and(|c| names.contains(c)))
        .collect();
    txns.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
    txns.iter()
        .filter_map(|t| bills::get(state, DemoLedger::id_of(t)).map(|v| v.record))
        .collect()
}

fn demo_candidate(
    r: &BillRecord,
    vendor: ContactId,
) -> Option<(DuplicateCandidate, Option<String>)> {
    Some((
        DuplicateCandidate {
            id: r.txn.id,
            vendor: r.txn.contact_id.unwrap_or(vendor),
            date: r.txn.txn_date,
            reference: r.vendor_invoice_number.clone()?,
            posted: r.txn.status == TxnStatus::Posted,
        },
        r.txn.txn_number.clone(),
    ))
}

/// The demo's open and recent bills (newest-first `rows`).
fn demo_lists(rows: &[BillRecord]) -> (Vec<PanelBill>, Vec<PanelBill>) {
    let bill = |r: &BillRecord, amount: Option<Money>| PanelBill {
        id: r.txn.id,
        number: r.txn.txn_number.clone(),
        reference: r.vendor_invoice_number.clone(),
        date: r.txn.txn_date,
        due: r.due_date,
        amount,
    };
    let posted: Vec<&BillRecord> = rows
        .iter()
        .filter(|r| r.txn.status == TxnStatus::Posted)
        .collect();
    let mut open: Vec<&BillRecord> = posted
        .iter()
        .copied()
        .filter(|r| r.balance_due.is_some_and(Money::is_positive))
        .collect();
    // Soonest due first, as the engine sorts them.
    open.sort_by_key(|r| (r.due_date.is_none(), r.due_date, r.txn.txn_date));
    let open = open
        .into_iter()
        .take(OPEN_SHOWN)
        .map(|r| bill(r, r.balance_due))
        .collect();
    let recent = posted
        .into_iter()
        .take(RECENT_SHOWN)
        .map(|r| bill(r, r.txn.total))
        .collect();
    (open, recent)
}

/// MM/DD/YYYY.
fn date_text(d: CalendarDate) -> String {
    crate::shell::us_date(d)
}

/// A label on the left, a value on the right.
fn row(ui: &mut Ui, tokens: &Tokens, label: &str, value: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(tokens.text_secondary));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), value);
    });
}

fn mono(ui: &mut Ui, tokens: &Tokens, text: String) {
    ui.label(
        RichText::new(text)
            .text_style(egui::TextStyle::Monospace)
            .color(tokens.text_primary),
    );
}

/// "Couldn't load this" with Try again; true when clicked.
fn failed(ui: &mut Ui, tokens: &Tokens) -> bool {
    let mut again = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(t("bill.panel.failed"))
                .size(12.0)
                .color(tokens.text_secondary),
        );
        again = ui.button(t("bill.panel.try_again")).clicked();
    });
    again
}

/// Draw the panel (and the duplicate question when it is open).
pub fn show(
    ui: &mut Ui,
    panel: &mut VendorPanel,
    input: &PanelInput<'_>,
    tokens: &Tokens,
) -> PanelEvent {
    let mut event = prompt_ui(ui, panel, input);
    if input.vendor.is_none() {
        form::card_tight(ui, tokens, |ui| {
            form::hint(ui, tokens, &t("bill.panel.no_vendor"));
        });
        return event;
    }
    let style = input.money_style;
    let mut retry = false;
    form::card_tight(ui, tokens, |ui| {
        if let Some(c) = input.contact {
            ui.label(
                RichText::new(&c.fields.display_name)
                    .strong()
                    .color(tokens.text_primary),
            );
        }
        let summary = panel.cards.as_ref().map(|c| &c.summary);
        row(
            ui,
            tokens,
            &t("bill.panel.open_balance"),
            |ui| match summary {
                Some(Card::Ready(s)) => mono(ui, tokens, s.total_owed.format(style)),
                Some(Card::Loading(_)) => {
                    ui.add(egui::Spinner::new());
                }
                _ => {
                    ui.label(RichText::new("\u{2014}").color(tokens.text_secondary));
                }
            },
        );
        if matches!(summary, Some(Card::Failed)) {
            retry |= failed(ui, tokens);
        }
        let default_account = input
            .contact
            .and_then(|c| c.fields.default_expense_account_id)
            .and_then(|id| input.accounts.iter().find(|a| a.id == id))
            .map_or_else(|| t("bill.panel.none"), account_label);
        row(ui, tokens, &t("bill.panel.default_account"), |ui| {
            ui.label(RichText::new(default_account).color(tokens.text_primary));
        });
        let is_1099 = input.contact.map(|c| c.fields.is1099_eligible);
        row(ui, tokens, &t("bill.panel.is_1099"), |ui| {
            let text = match is_1099 {
                Some(true) => t("bill.panel.yes"),
                Some(false) => t("bill.panel.no"),
                None => "\u{2014}".to_string(),
            };
            ui.label(RichText::new(text).color(tokens.text_primary));
        });
        if let PanelEvent::OpenRecord(k, id) = duplicate_line(ui, panel, input, tokens) {
            event = PanelEvent::OpenRecord(k, id);
        }
    });
    ui.add_space(12.0);
    form::card_tight(ui, tokens, |ui| {
        if let Some(e) = more_ui(ui, panel, tokens, style, &mut retry) {
            event = e;
        }
    });
    if retry && let Some(v) = panel.vendor {
        panel.cards = Some(panel.read_cards(input.link, v));
    }
    event
}

/// `6210 · Equipment rental`.
fn account_label(a: &Account) -> String {
    match a.fields.account_number.as_deref().filter(|n| !n.is_empty()) {
        Some(n) => format!("{n} \u{b7} {}", a.fields.name),
        None => a.fields.name.clone(),
    }
}

/// The duplicate line under the 1099 row.
fn duplicate_line(
    ui: &mut Ui,
    panel: &VendorPanel,
    input: &PanelInput<'_>,
    tokens: &Tokens,
) -> PanelEvent {
    let credit = input.kind == BillKind::Credit;
    let pick =
        |bill: &'static str, credit_key: &'static str| if credit { credit_key } else { bill };
    let Some(key) = panel.dup.key() else {
        return PanelEvent::None;
    };
    let reference = key.reference.as_str();
    let small = |text: String, color| RichText::new(text).size(12.0).color(color);
    let mut event = PanelEvent::None;
    ui.add_space(4.0);
    match &panel.dup {
        Dup::Waiting { .. } | Dup::Running { .. } => {
            ui.label(small(
                t_with("bill.panel.dup.checking", &[("ref", reference)]),
                tokens.text_secondary,
            ));
        }
        Dup::Done { found, .. } => match &found.found {
            Some(d) => {
                let date = date_text(d.date);
                let text = match d.number.as_deref() {
                    Some(n) => t_with(
                        pick("bill.panel.dup.found", "bill.panel.dup.found_credit"),
                        &[("number", n), ("date", &date), ("ref", reference)],
                    ),
                    None => t_with(
                        pick(
                            "bill.panel.dup.found_no_number",
                            "bill.panel.dup.found_credit_no_number",
                        ),
                        &[("date", &date), ("ref", reference)],
                    ),
                };
                egui::Frame::new()
                    .fill(tokens.warn_tint)
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(small(text, tokens.warn_text)).wrap());
                        if ui
                            .link(t(pick("bill.panel.dup.open", "bill.panel.dup.open_credit")))
                            .clicked()
                        {
                            event = PanelEvent::OpenRecord(input.kind, d.id);
                        }
                    });
            }
            None if !found.complete => {
                ui.add(
                    egui::Label::new(small(
                        t_with(
                            pick(
                                "bill.panel.dup.unchecked",
                                "bill.panel.dup.unchecked_credit",
                            ),
                            &[("ref", reference)],
                        ),
                        tokens.warn_text,
                    ))
                    .wrap(),
                );
            }
            None => {
                ui.add(
                    egui::Label::new(small(
                        t_with(
                            pick("bill.panel.dup.none", "bill.panel.dup.none_credit"),
                            &[("ref", reference)],
                        ),
                        tokens.accent,
                    ))
                    .wrap(),
                );
            }
        },
        Dup::Off => {}
    }
    event
}

/// Open bills, unused credits and recent bills (Q-P17m).
fn more_ui(
    ui: &mut Ui,
    panel: &VendorPanel,
    tokens: &Tokens,
    style: MoneyStyle,
    retry: &mut bool,
) -> Option<PanelEvent> {
    let cards = panel.cards.as_ref()?;
    let mut event = None;
    let summary = cards.summary.ready();
    // Open bills: the engine's count and total, then up to five.
    row(ui, tokens, &t("bill.panel.open_bills"), |ui| {
        if let Some(s) = summary {
            mono(
                ui,
                tokens,
                t_with(
                    "bill.panel.count_total",
                    &[
                        ("count", &s.unpaid_count.to_string()),
                        ("amount", &s.total_owed.format(style)),
                    ],
                ),
            );
        }
    });
    match &cards.open {
        Card::Ready(rows) if rows.is_empty() => {
            form::hint(ui, tokens, &t("bill.panel.open_bills.none"));
        }
        Card::Ready(rows) => {
            for b in rows {
                let due = b
                    .due
                    .map(|d| t_with("bill.panel.due", &[("date", &date_text(d))]))
                    .unwrap_or_default();
                if bill_row(ui, tokens, b, &due, style) {
                    event = Some(PanelEvent::OpenRecord(BillKind::Bill, b.id));
                }
            }
            if let Some(s) = summary
                && usize::try_from(s.unpaid_count).is_ok_and(|n| n > rows.len())
            {
                form::hint(
                    ui,
                    tokens,
                    &t_with(
                        "bill.panel.open_bills.more",
                        &[
                            ("shown", &rows.len().to_string()),
                            ("count", &s.unpaid_count.to_string()),
                        ],
                    ),
                );
            }
        }
        Card::Failed => *retry |= failed(ui, tokens),
        Card::Loading(_) => form::progress_line(ui, tokens, &t("bill.panel.loading")),
    }
    ui.add_space(8.0);
    // Unused credits: the summary's count and total.
    row(ui, tokens, &t("bill.panel.credits"), |ui| match summary {
        Some(s) if s.available_credits_count == 0 => {
            ui.label(RichText::new(t("bill.panel.none")).color(tokens.text_primary));
        }
        Some(s) => mono(
            ui,
            tokens,
            t_with(
                "bill.panel.count_total",
                &[
                    ("count", &s.available_credits_count.to_string()),
                    ("amount", &s.total_available_credits.format(style)),
                ],
            ),
        ),
        None => {}
    });
    ui.add_space(8.0);
    ui.label(RichText::new(t("bill.panel.recent")).color(tokens.text_secondary));
    match &cards.recent {
        Card::Ready(rows) if rows.is_empty() => {
            form::hint(ui, tokens, &t("bill.panel.recent.none"));
        }
        Card::Ready(rows) => {
            for b in rows {
                if bill_row(ui, tokens, b, &date_text(b.date), style) {
                    event = Some(PanelEvent::OpenRecord(BillKind::Bill, b.id));
                }
            }
        }
        Card::Failed => *retry |= failed(ui, tokens),
        Card::Loading(_) => form::progress_line(ui, tokens, &t("bill.panel.loading")),
    }
    event
}

/// One listed bill: its ref (a link that opens it), a date note and the
/// engine's amount. True when the link was clicked.
fn bill_row(ui: &mut Ui, tokens: &Tokens, b: &PanelBill, note: &str, style: MoneyStyle) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        let label = b.label();
        let label = if label.is_empty() {
            t("bill.panel.bill")
        } else {
            label
        };
        clicked = ui
            .link(RichText::new(label).text_style(egui::TextStyle::Monospace))
            .clicked();
        ui.label(RichText::new(note).size(12.0).color(tokens.text_secondary));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            mono(
                ui,
                tokens,
                b.amount.map(|m| m.format(style)).unwrap_or_default(),
            );
        });
    });
    clicked
}

/// The question on Save with a duplicate: Save anyway / Go back.
fn prompt_ui(ui: &Ui, panel: &mut VendorPanel, input: &PanelInput<'_>) -> PanelEvent {
    let Some(intent) = panel.prompt else {
        return PanelEvent::None;
    };
    let Some(d) = panel.duplicate().cloned() else {
        // The duplicate went away (the ref changed): nothing to ask.
        panel.prompt = None;
        panel.resume = Some(intent);
        return PanelEvent::None;
    };
    let key = if input.kind == BillKind::Credit {
        "bill.panel.prompt.body_credit"
    } else {
        "bill.panel.prompt.body"
    };
    let reference = panel
        .dup
        .key()
        .map_or_else(String::new, |k| k.reference.clone());
    let body = t_with(key, &[("ref", &reference), ("date", &date_text(d.date))]);
    let mut answer = None;
    egui::Window::new(t("bill.panel.prompt.title"))
        .id(egui::Id::unique(("enter_bill", "duplicate_prompt")))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            ui.set_max_width(420.0);
            ui.add(egui::Label::new(body).wrap());
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button(t("bill.panel.prompt.save_anyway")).clicked() {
                    answer = Some(true);
                }
                let back = crate::widgets::primary_button(ui, &t("bill.panel.prompt.go_back"));
                if back.clicked() {
                    answer = Some(false);
                }
            });
        });
    answer.map_or(PanelEvent::None, |anyway| panel.answer(anyway))
}

impl VendorPanel {
    /// The owner answered the duplicate question: Save anyway (true) sends
    /// the save it was asked for; Go back sends nothing.
    pub fn answer(&mut self, save_anyway: bool) -> PanelEvent {
        let Some(intent) = self.prompt.take() else {
            return PanelEvent::None;
        };
        if save_anyway {
            self.confirmed = self.duplicate().map(|d| d.id);
            self.resume = Some(intent);
            PanelEvent::None
        } else {
            PanelEvent::GoBack
        }
    }
}

#[cfg(test)]
#[path = "vendor_panel_tests.rs"]
mod tests;
