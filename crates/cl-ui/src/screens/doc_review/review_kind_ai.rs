//! Two draft-mode tools of [`DraftSession`] (A04 D2, D5):
//!
//! - **Enter this as** (Q-A04n): the same draft in another form (bill,
//!   card charge, cash expense, check). Nothing is written to the engine:
//!   the kind is the convert body's (`ConvertBody::kind`; the engine and the
//!   demo set the draft's kind from it). The owner's work goes with it as a
//!   [`Carry`]: the session's values (an in-session reading or AI's help
//!   are not stored on the draft) and every field the owner already
//!   **accepted** or **changed**, with its value, mapped onto the other
//!   form's fields (vendor/payee, date, total, paid from, the read lines by
//!   their order in the reading). Fields still *read, not accepted* fill
//!   again from the same reading and still need Accept; a changed value is
//!   carried as typed and never turns into an accepted read value
//!   (non-negotiable 4).
//! - **Ask AI to help** (Q-A03j, Q-A03k): on the card and check forms
//!   (Enter Bill keeps A03's own button, fed by [`DraftSession::assist_base`]).
//!   Offered under A03's rules (the stored reading left a main field empty
//!   or unsure, the company's switches and consent allow a route, one ask
//!   per reading) and **enabled once the page words are ready**: the AI
//!   amount gate needs them, and the stored reading carries values only
//!   (S11). The answer is merged by `cl_docread::ai_gate::merge` before any
//!   form value exists: an AI amount is kept only when a word on the page
//!   equals it. The form then fills again: unaccepted values give way,
//!   accepted and changed ones stay (and keep their marks).

use std::sync::{Arc, Mutex, PoisonError};

use cl_api::{ApiError, AssistSuggestion, DocumentDraftId, DraftKind};
use cl_core::ids::{AccountId, ContactId};
use cl_core::{CalendarDate, Money};
use cl_docread::vendor_match::VendorCandidate;
use cl_model::doc_draft::DraftForm;
use cl_model::forms::doc_read::{ReadField, ReadState as ReadMarks, ReadTarget};
use cl_model::forms::lines::FormLine;
use cl_model::rowkey::RowKey;
use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use super::{DraftEvent, DraftSession, Stored};
use crate::company::Task;
use crate::i18n::t;
use crate::reading::LocalReading;
use crate::reading::assist::{self, NoRoute, Picture, Route};
use crate::screens::bill::doc_read::ReadSwitches;
use crate::screens::bill::doc_read_local::Offer;
use crate::theme::Tokens;

// ============================================================ enter as

/// The draft kind a form posts as.
#[must_use]
pub(crate) fn kind_of(form: DraftForm) -> DraftKind {
    match form {
        DraftForm::EnterBill => DraftKind::Bill,
        DraftForm::CardCharge => DraftKind::CardCharge,
        DraftForm::CashExpense => DraftKind::CashExpense,
        DraftForm::WriteCheck => DraftKind::Check,
    }
}

/// A field shared by the draft forms, whatever each form calls it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shared {
    /// Vendor (bill), payee (card, check).
    Payee,
    /// Bill date, date.
    Date,
    /// Amount due, amount.
    Total,
    /// The card, or the bank or cash account (not on a bill).
    PaidFrom,
}

impl Shared {
    /// The form's own read target for this field (`None`: not on it).
    #[must_use]
    pub(crate) fn target(self, form: DraftForm) -> Option<ReadTarget> {
        let bill = form == DraftForm::EnterBill;
        Some(match self {
            Shared::Payee if bill => ReadTarget::Vendor,
            Shared::Date if bill => ReadTarget::BillDate,
            Shared::Total if bill => ReadTarget::AmountDue,
            Shared::PaidFrom if bill => return None,
            Shared::Payee => ReadTarget::Payee,
            Shared::Date => ReadTarget::Date,
            Shared::Total => ReadTarget::Total,
            Shared::PaidFrom => ReadTarget::PaidFrom,
        })
    }
}

/// One field the owner accepted or changed: its value and that state.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Kept<T> {
    /// The value in the form when the owner switched.
    pub value: T,
    /// [`ReadField::Accepted`] or [`ReadField::Edited`].
    pub state: ReadField,
}

/// One read line the owner accepted or changed.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct KeptLine {
    /// Its account.
    pub account: Option<AccountId>,
    /// Its amount (as the owner left it).
    pub amount: Option<Money>,
    /// Its memo.
    pub memo: String,
    /// [`ReadField::Accepted`] or [`ReadField::Edited`].
    pub state: ReadField,
}

/// The form's values as the owner left them (what "Enter this as" takes).
#[derive(Clone, Default)]
pub(crate) struct FormValues {
    /// Vendor or payee.
    pub payee: Option<ContactId>,
    /// Bill date or date.
    pub date: Option<CalendarDate>,
    /// Amount due or amount.
    pub total: Option<Money>,
    /// Card, bank or cash account (`None` on a bill).
    pub paid_from: Option<AccountId>,
    /// The lines (expense lines on a bill).
    pub lines: Vec<FormLine>,
    /// The form's read marks.
    pub marks: ReadMarks,
}

/// The owner's work on a draft, carried to another form. Owner data:
/// `Debug` shows the forms only.
#[derive(Clone)]
pub(crate) struct Carry {
    /// The draft.
    pub id: DocumentDraftId,
    /// The form it opens in.
    pub form: DraftForm,
    /// The draft's change token the values belong to.
    pub updated_at: Option<String>,
    /// The session's values (with an in-session reading or AI's help).
    pub reading: Option<Box<LocalReading>>,
    /// AI already helped with this reading (one ask per reading).
    pub helped: bool,
    /// Vendor or payee.
    pub payee: Option<Kept<ContactId>>,
    /// The date.
    pub date: Option<Kept<CalendarDate>>,
    /// The total.
    pub total: Option<Kept<Money>>,
    /// The account it was paid from.
    pub paid_from: Option<Kept<AccountId>>,
    /// The read lines, by their order in the reading.
    pub lines: Vec<Option<KeptLine>>,
}

impl std::fmt::Debug for Carry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Carry")
            .field("form", &self.form)
            .finish_non_exhaustive()
    }
}

/// The owner's own state of a field, when there is one to carry.
fn owners(marks: &ReadMarks, target: Option<ReadTarget>) -> Option<ReadField> {
    marks
        .state(target?)
        .filter(|s| matches!(s, ReadField::Accepted | ReadField::Edited))
}

impl Carry {
    /// How many fields it carries (control channel).
    #[must_use]
    pub(crate) fn kept(&self) -> usize {
        usize::from(self.payee.is_some())
            + usize::from(self.date.is_some())
            + usize::from(self.total.is_some())
            + usize::from(self.paid_from.is_some())
            + self.lines.iter().flatten().count()
    }

    /// Puts the carried values and their marks into `form`'s fields (after
    /// the reading filled them). `line`: the form's row for a read line's
    /// index. `paid_from_ok`: the account is one this form lists.
    pub(crate) fn apply_to(
        &self,
        form: DraftForm,
        v: &mut FormValues,
        line: impl Fn(usize) -> Option<RowKey>,
        paid_from_ok: impl Fn(AccountId) -> bool,
    ) {
        fn put<T: Copy>(
            kept: Option<Kept<T>>,
            target: Option<ReadTarget>,
            slot: &mut Option<T>,
            marks: &mut ReadMarks,
        ) {
            if let (Some(k), Some(t)) = (kept, target) {
                *slot = Some(k.value);
                marks.restore(t, k.state);
            }
        }
        put(
            self.payee,
            Shared::Payee.target(form),
            &mut v.payee,
            &mut v.marks,
        );
        put(
            self.date,
            Shared::Date.target(form),
            &mut v.date,
            &mut v.marks,
        );
        put(
            self.total,
            Shared::Total.target(form),
            &mut v.total,
            &mut v.marks,
        );
        let paid = self.paid_from.filter(|k| paid_from_ok(k.value));
        put(
            paid,
            Shared::PaidFrom.target(form),
            &mut v.paid_from,
            &mut v.marks,
        );
        for (i, kept) in self.lines.iter().enumerate() {
            let (Some(k), Some(key)) = (kept, line(i)) else {
                continue;
            };
            if let Some(l) = v.lines.iter_mut().find(|l| l.key == key) {
                l.account = k.account;
                l.amount = k.amount;
                l.memo.clone_from(&k.memo);
                v.marks.restore(ReadTarget::Line(key), k.state);
            }
        }
    }
}

/// The carry waiting for the form that opens next (one at a time).
#[derive(Clone, Default)]
struct SharedCarry(Arc<Mutex<Option<Carry>>>);

fn carry_id() -> egui::Id {
    egui::Id::unique("a04/review/enter_as")
}

fn carry_slot(ctx: &egui::Context) -> Arc<Mutex<Option<Carry>>> {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<SharedCarry>(carry_id())
            .0
            .clone()
    })
}

/// Leaves `carry` for the form it opens in.
pub(crate) fn publish_carry(ctx: &egui::Context, carry: Carry) {
    *carry_slot(ctx)
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(carry);
}

/// The carry for draft `id` opening in `form`, once.
pub(crate) fn take_carry(
    ctx: &egui::Context,
    id: DocumentDraftId,
    form: DraftForm,
) -> Option<Carry> {
    let slot = carry_slot(ctx);
    let mut g = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if g.as_ref().is_some_and(|c| c.id == id && c.form == form) {
        g.take()
    } else {
        None
    }
}

// ============================================================== assist

/// Ask AI to help in draft mode (the card and check forms).
#[derive(Default)]
pub(crate) struct DraftAssist {
    /// The switches as known (the demo's, or the engine's once read).
    pub(super) switches: Option<ReadSwitches>,
    switches_task: Option<Task<ReadSwitches>>,
    /// The running ask.
    task: Option<Task<Result<AssistSuggestion, ApiError>>>,
    /// What was sent: the stored values with the page words.
    base: Option<Box<LocalReading>>,
    vendors: Vec<VendorCandidate>,
    /// AI already helped with this reading (one ask per reading).
    pub helped: bool,
    /// Asks sent (control channel and tests).
    pub sent: u32,
    /// The class of the last outcome (control channel).
    pub outcome: Option<&'static str>,
    /// Whether the last ask took the document's picture.
    pub picture: Option<Picture>,
}

impl DraftAssist {
    /// An ask is running.
    #[must_use]
    pub(crate) fn busy(&self) -> bool {
        self.task.is_some() || self.switches_task.is_some()
    }

    /// The ask itself is running.
    #[must_use]
    pub(crate) fn asking(&self) -> bool {
        self.task.is_some()
    }
}

impl DraftSession {
    /// The reading Ask AI to help works on: the stored values with the page
    /// words and fingerprint of the re-read (the stored reading carries
    /// values only, S11; without the words the gate would refuse every AI
    /// amount). `None` until both are there.
    #[must_use]
    pub(crate) fn assist_base(&self) -> Option<LocalReading> {
        let words = self.words.as_ref()?;
        let mut base = self.values()?.clone();
        base.reading.words.clone_from(&words.reading.words);
        base.reading
            .fingerprint
            .clone_from(&words.reading.fingerprint);
        base.reading.pages.clone_from(&words.reading.pages);
        base.reading.page_count = words.reading.page_count;
        Some(base)
    }

    /// Tests: the re-read's words arrive (the worker isn't running).
    #[cfg(test)]
    pub(crate) fn give_words(&mut self, w: Arc<LocalReading>) {
        self.set_words(w);
    }

    /// The page words are ready (Ask AI to help may be clicked).
    #[must_use]
    pub(crate) fn words_ready(&self) -> bool {
        self.words.is_some()
    }

    /// Every frame: the switches (once the draft has stored values) and a
    /// finished ask.
    pub(super) fn poll_assist(&mut self) {
        if self.form != DraftForm::EnterBill
            && self.assist.switches.is_none()
            && self.assist.switches_task.is_none()
            && matches!(self.stored, Some(Stored::Read(_)))
        {
            self.read_assist_switches();
        }
        if let Some(r) = self.assist.switches_task.as_mut().and_then(Task::try_take) {
            self.assist.switches_task = None;
            // Unreadable (offline, signed out): nothing is offered.
            self.assist.switches = Some(r.unwrap_or_default());
        }
        let Some(result) = self.assist.task.as_mut().and_then(Task::poll_result) else {
            return;
        };
        self.assist.task = None;
        let answer = match result {
            Ok(a) => a,
            Err(e) => {
                let key = crate::screens::bill::doc_read_local::assist_problem(&e);
                self.note = Some(key);
                let class = key.rsplit('.').next().unwrap_or(key);
                self.assist.outcome = Some(class);
                tracing::info!(outcome = class, "draft assist failed");
                return;
            }
        };
        let Some(base) = self.assist.base.take() else {
            return;
        };
        // Entered, discarded or being entered meanwhile: the form's values
        // are what was sent; the answer changes nothing.
        if self.closed() || self.locked() {
            self.assist.outcome = Some("dropped");
            return;
        }
        // The gate runs before any form value exists (Q-A03j).
        let merged = assist::merge(&base, &answer, &self.assist.vendors);
        self.assist.outcome = Some("helped");
        self.assist.helped = true;
        tracing::info!("draft assist helped");
        // The form fills again from the merged reading (spots and learning
        // use it too).
        self.stored = Some(Stored::Read(Box::new(merged.clone())));
        self.fill_taken = true;
        self.read_result = Some(Box::new(merged));
        self.events.push(DraftEvent::Helped);
    }

    fn read_assist_switches(&mut self) {
        if self.link.is_demo() {
            self.assist.switches = ReadSwitches::demo(&self.link);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.assist.switches = Some(ReadSwitches::default());
            return;
        };
        match crate::screens::bill::doc_read::fetch_switches(group, &session) {
            Ok(p) => self.assist.switches_task = Some(Task::Pending(p)),
            Err(_) => self.assist.switches = Some(ReadSwitches::default()),
        }
    }

    /// What the banner offers (A03's rules; Enter Bill has its own).
    #[must_use]
    pub(crate) fn assist_offer(&self) -> Offer {
        if self.form == DraftForm::EnterBill
            || self.assist.helped
            || self.assist.asking()
            || self.closed()
        {
            return Offer::None;
        }
        let Some(values) = self.values() else {
            return Offer::None;
        };
        if !assist::needs_help(values) {
            return Offer::None;
        }
        let Some(s) = self.assist.switches else {
            return Offer::None;
        };
        match assist::route(s.assist_switches()) {
            Ok(r) => Offer::Ask(r),
            Err(NoRoute::NeedsConsent) => Offer::NeedsConsent,
            Err(NoRoute::Off | NoRoute::Unsupported) => Offer::None,
        }
    }

    /// **Ask AI to help** (the owner's click): the page text (and a small
    /// PNG when pictures may go) to the company's AI. Nothing is sent
    /// before the words are ready, without a route, or (online) without
    /// room under the monthly cap. True when it was sent.
    pub(crate) fn ask_ai(&mut self, vendors: Vec<VendorCandidate>) -> bool {
        if self.locked() {
            return false;
        }
        let Offer::Ask(route) = self.assist_offer() else {
            return false;
        };
        let Some(base) = self.assist_base() else {
            return false;
        };
        self.note = None;
        if matches!(route, Route::Cloud { .. })
            && !crate::forms::suggest::under_cap(&self.link, self.tenant)
        {
            self.note = Some("bill.read.problem.cap");
            self.assist.outcome = Some("cap");
            return false;
        }
        let bytes = self
            .file
            .as_ref()
            .map_or_else(|| Arc::from([]), |f| Arc::clone(&f.bytes));
        let request = assist::request(&base, route, &bytes, kind_of(self.form));
        self.assist.picture = Some(assist::picture(route, &bytes));
        self.assist.vendors = vendors;
        self.assist.base = Some(Box::new(base));
        tracing::info!("draft assist started");
        if self.link.is_demo() {
            self.assist.sent += 1;
            self.assist.task = Some(Task::ready(
                crate::screens::bill::doc_read_local::demo_assist(&self.link),
            ));
            return true;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.note = Some("bill.read.problem.unreachable");
            return false;
        };
        match crate::screens::bill::doc_read::send_assist(group, &session, request) {
            Ok(p) => {
                self.assist.sent += 1;
                self.assist.task = Some(Task::Pending(p));
                true
            }
            Err(_) => {
                self.note = Some("bill.read.problem.busy");
                false
            }
        }
    }

    /// The banner's Ask AI to help (disabled until the words are ready),
    /// where to allow it, or what AI did. True when clicked.
    pub(super) fn assist_banner(&self, ui: &mut egui::Ui, tokens: &Tokens) -> bool {
        if self.assist.asking() {
            ui.label(RichText::new(t("bill.read.assist.reading")).color(tokens.text_secondary));
            return false;
        }
        if self.assist.helped && self.assist.sent > 0 {
            ui.add(
                egui::Label::new(
                    RichText::new(t("bill.read.assist.helped")).color(tokens.text_primary),
                )
                .wrap(),
            );
            if self.assist.picture == Some(Picture::TooLarge) {
                ui.add(
                    egui::Label::new(
                        RichText::new(t("bill.read.assist.picture_too_large"))
                            .color(tokens.text_secondary),
                    )
                    .wrap(),
                );
            }
        }
        match self.assist_offer() {
            Offer::Ask(route) => {
                let tip = match route {
                    Route::Local => "bill.read.assist.button.tip.local",
                    Route::Cloud { .. } => "bill.read.assist.button.tip.cloud",
                };
                let ready = self.words_ready() && !self.locked();
                ui.add_enabled(ready, egui::Button::new(t("bill.read.assist.button")))
                    .on_hover_text(t(tip))
                    .on_disabled_hover_text(t("doc_form.assist.waiting"))
                    .clicked()
            }
            Offer::NeedsConsent => {
                ui.add(
                    egui::Label::new(
                        RichText::new(t("bill.read.assist.consent")).color(tokens.text_secondary),
                    )
                    .wrap(),
                );
                false
            }
            Offer::None => false,
        }
    }

    /// Enter Bill's own Ask AI to help merged an answer: the session's
    /// values are the merged reading from now on (spots, learning, Enter
    /// this as).
    pub(crate) fn adopt_help(&mut self, merged: &LocalReading) {
        self.stored = Some(Stored::Read(Box::new(merged.clone())));
        self.fill_taken = true;
        self.assist.helped = true;
    }

    // ---------------------------------------------------------- enter as

    /// "Enter this as" `to`: what goes with the draft (`None` when it can't
    /// switch now: the same form, busy, asking or closed).
    #[must_use]
    pub(crate) fn enter_as(&self, to: DraftForm, v: &FormValues) -> Option<Carry> {
        if to == self.form || self.locked() || self.closed() || self.draft.is_none() {
            return None;
        }
        let kept = |shared: Shared| owners(&v.marks, shared.target(self.form));
        let lines = self
            .line_rows
            .iter()
            .map(|key| {
                let state = owners(&v.marks, Some(ReadTarget::Line(*key)))?;
                let l = v.lines.iter().find(|l| l.key == *key)?;
                Some(KeptLine {
                    account: l.account,
                    amount: l.amount,
                    memo: l.memo.clone(),
                    state,
                })
            })
            .collect();
        Some(Carry {
            id: self.id,
            form: to,
            updated_at: self.draft.as_ref().map(|d| d.updated_at.clone()),
            reading: self.values().map(|r| Box::new(r.clone())),
            helped: self.assist.helped,
            payee: kept(Shared::Payee)
                .zip(v.payee)
                .map(|(state, value)| Kept { value, state }),
            date: kept(Shared::Date)
                .zip(v.date)
                .map(|(state, value)| Kept { value, state }),
            total: kept(Shared::Total)
                .zip(v.total)
                .map(|(state, value)| Kept { value, state }),
            paid_from: kept(Shared::PaidFrom)
                .zip(v.paid_from)
                .map(|(state, value)| Kept { value, state }),
            lines,
        })
    }

    /// The form's row of the reading's line `i` (after the fill).
    #[must_use]
    pub(crate) fn line_key(&self, i: usize) -> Option<RowKey> {
        self.line_rows.get(i).copied()
    }

    /// The carry this form opened with, once (after the fill).
    pub(crate) fn take_carry(&mut self) -> Option<Carry> {
        self.carry.take()
    }

    /// The draft loaded: a carry for this very version brings the
    /// session's values (an in-session reading, AI's help).
    pub(super) fn carried_values(&mut self) {
        let Some(c) = self.carry.as_ref() else {
            return;
        };
        let same = c.updated_at.is_some()
            && c.updated_at.as_deref() == self.draft.as_ref().map(|d| d.updated_at.as_str());
        if !same || !self.draft.as_ref().is_some_and(|d| d.status.is_live()) {
            return;
        }
        if let Some(r) = c.reading.clone() {
            self.stored = Some(Stored::Read(r));
        }
        self.assist.helped |= c.helped;
    }

    /// `state().draft.assist` (states and counts only).
    #[must_use]
    pub(crate) fn assist_json(&self) -> Value {
        json!({
            "offer": self.assist_offer().key(),
            "ready": self.words_ready(),
            "needs_help": self.values().is_some_and(assist::needs_help),
            "helped": self.assist.helped,
            "busy": self.assist.asking(),
            "requests": self.assist.sent,
            "outcome": self.assist.outcome,
            "picture": self.assist.picture.map(Picture::key),
        })
    }
}

#[cfg(test)]
#[path = "review_kind_ai_tests.rs"]
mod tests;
