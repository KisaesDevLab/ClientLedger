//! "We could not confirm the save" (B2, Q-PR1a): the one component every
//! posting form uses after a save whose outcome is unknown.
//!
//! An engine without `idempotency-v1` (U-18) honours `Idempotency-Key` only
//! on `/batch/save`, so sending the same save again could post it twice.
//! With it, the engine replays the key on its route set: such an unknown
//! outcome comes back `keyed` ([`ApiError::needs_recheck`] is false), and
//! the form does not block at all. Save stays enabled and sends the same
//! save under the same key (the form keeps it), which the engine answers
//! with the first result, never a second posting. Everything below is for
//! the unkeyed case (a route outside the set, an engine without the
//! capability, or a key too old to rely on). The rule itself lives in
//! `cl_model::forms::FormState` (Save answers `Unconfirmed` while the mark
//! is set); this component runs the owner's **Check again**
//! (`Session::recheck`, the stored lost-reply probe, read-only) and the
//! explicit **I checked the register — save again**, which asks once more
//! in a dialog before it lifts the block (owner answer 2026-10-24). The
//! override is logged without amounts, names or memo text.
//!
//! The block outlives the window (review fix-PR-1 blocking 2): an unknown
//! save is also written in the company link's [`LostBook`] under its
//! [`LostScope`] (the form, or the template for "Enter now"), for the
//! whole app session. Closing the window and opening another of the same
//! form (or a second window already open) picks the block up again with
//! the same key, so Check again still runs the stored probe; an answer in
//! one window reaches the others. Only an app restart forgets it (the
//! session's stored probes are gone then too; Q-PR1a).

use std::sync::Arc;

use cl_api::{ApiError, IdempotencyKey, Recheck, RequestGroup};
use cl_core::ids::{TemplateId, TenantId, TransactionId};
use cl_model::forms::{FormState, RecheckAnswer};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyLink, Task};
use crate::i18n::t;
use crate::theme::Tokens;

/// What the recheck or the owner decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LostEvent {
    /// It is in the books (`Some` names the transaction): show it.
    Saved(Option<TransactionId>),
    /// It provably was not saved: Save works again.
    NotSaved,
    /// Still cannot tell.
    StillUnknown,
    /// The owner looked in the register and confirmed saving again.
    OwnerConfirmed,
}

/// What an unknown save blocks: one form (every window of it), or "Enter
/// now" of one template (Templates and Templates Due alike).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LostScope {
    /// A form, by the name its banner uses (`write_check`, `deposit`, ...).
    Form(&'static str),
    /// "Enter now" of a template.
    Template(TemplateId),
}

/// Where an unknown save stands in the [`LostBook`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BookState {
    /// Still unknown: every window of the scope blocks Save.
    Open,
    /// Found in the books (the transaction, when known).
    Saved(Option<TransactionId>),
    /// Not saved, or the owner confirmed saving again.
    Cleared,
}

/// The open company's unknown saves, for the whole app session (on the
/// [`CompanyLink`]): keyed by company and [`LostScope`]. Keys only; no
/// amounts, names or memo text.
#[derive(Debug, Default)]
pub struct LostBook {
    company: Option<TenantId>,
    entries: Vec<(
        Option<TenantId>,
        LostScope,
        Option<IdempotencyKey>,
        BookState,
    )>,
}

/// The most entries kept (oldest answered ones go first).
const BOOK_MAX: usize = 256;

impl LostBook {
    /// The company now open (`None`: none). Entries of other companies are
    /// kept for when it opens again, but never seen meanwhile.
    pub fn set_company(&mut self, company: Option<TenantId>) {
        self.company = company;
    }

    fn find(&self, scope: LostScope) -> Option<usize> {
        self.entries
            .iter()
            .position(|(c, s, _, _)| *c == self.company && *s == scope)
    }

    /// An unknown save of `scope` with `key`.
    pub fn remember(&mut self, scope: LostScope, key: Option<IdempotencyKey>) {
        if let Some(i) = self.find(scope) {
            self.entries.remove(i);
        }
        self.entries
            .push((self.company, scope, key, BookState::Open));
        while self.entries.len() > BOOK_MAX {
            let old = self
                .entries
                .iter()
                .position(|e| e.3 != BookState::Open)
                .unwrap_or(0);
            self.entries.remove(old);
        }
    }

    /// The unknown save of `scope` with `key` was answered.
    fn answer(&mut self, scope: LostScope, key: Option<&IdempotencyKey>, state: BookState) {
        if let Some(i) = self.find(scope)
            && self.entries[i].2.as_ref() == key
        {
            self.entries[i].3 = state;
        }
    }

    /// The key of the still unknown save of `scope`, if one is open
    /// (`Some(None)`: open without a key).
    #[must_use]
    pub fn open(&self, scope: LostScope) -> Option<Option<IdempotencyKey>> {
        self.find(scope)
            .map(|i| &self.entries[i])
            .filter(|e| e.3 == BookState::Open)
            .map(|e| e.2.clone())
    }

    /// True while an unknown save of `scope` is open.
    #[must_use]
    pub fn is_open(&self, scope: LostScope) -> bool {
        self.open(scope).is_some()
    }

    /// The templates whose "Enter now" is still unknown, oldest first.
    #[must_use]
    pub fn open_templates(&self) -> Vec<TemplateId> {
        self.entries
            .iter()
            .filter(|e| e.0 == self.company && e.3 == BookState::Open)
            .filter_map(|e| match e.1 {
                LostScope::Template(id) => Some(id),
                LostScope::Form(_) => None,
            })
            .collect()
    }

    fn state_of(&self, scope: LostScope, key: Option<&IdempotencyKey>) -> Option<BookState> {
        self.find(scope)
            .map(|i| &self.entries[i])
            .filter(|e| e.2.as_ref() == key)
            .map(|e| e.3)
    }
}

/// One form's unknown save.
#[derive(Debug, Default)]
pub struct LostSave {
    /// A save of this form ended with an unknown outcome.
    pending: bool,
    /// Its idempotency key (finds the stored probe).
    key: Option<IdempotencyKey>,
    task: Option<Task<Recheck>>,
    /// The "save again?" dialog is open.
    pub(crate) confirm_open: bool,
    /// The last answer, as a message key.
    pub(crate) notice: Option<&'static str>,
    /// The owner confirmed saving again after an unknown outcome (Write
    /// Check then looks up a "number used" check instead of offering the
    /// next number).
    pub(crate) after_unknown: bool,
    /// Rechecks run (control channel).
    pub(crate) checks: u32,
    /// The company link whose [`LostBook`] keeps this form's unknown save
    /// beyond the window, and the scope it is kept under.
    book: Option<(Arc<CompanyLink>, LostScope)>,
    /// The pending save is in the book (ours or picked up from it).
    shared: bool,
    /// "Enter now" of templates: the scope is the template being entered
    /// ([`Self::set_scope`]), never the window's form name.
    templates: bool,
}

impl LostSave {
    /// The unknown "Enter now" of the Templates and Templates Due windows:
    /// kept per template ([`LostScope::Template`]).
    #[must_use]
    pub fn for_templates() -> LostSave {
        LostSave {
            templates: true,
            ..LostSave::default()
        }
    }

    /// Keeps this form's unknown saves in `link`'s [`LostBook`] under
    /// `scope`, and picks up one already open there (another window, or a
    /// window closed before an answer): `form` blocks Save then.
    pub fn bind<V: Clone + PartialEq>(
        &mut self,
        link: &Arc<CompanyLink>,
        scope: LostScope,
        form: &mut FormState<V>,
    ) -> Option<LostEvent> {
        let same = self
            .book
            .as_ref()
            .is_some_and(|(l, s)| Arc::ptr_eq(l, link) && *s == scope);
        if !same {
            self.book = Some((Arc::clone(link), scope));
            if !self.pending {
                self.shared = false;
            }
        }
        self.sync(form)
    }

    /// [`Self::lost`] for a save kept under `scope` ("Enter now" of one
    /// template): an unknown outcome is written in `link`'s book there, and
    /// this window's banner is for it now.
    pub fn lost_as(&mut self, e: &ApiError, link: &Arc<CompanyLink>, scope: LostScope) -> bool {
        if !e.needs_recheck() {
            return false;
        }
        self.book = Some((Arc::clone(link), scope));
        self.shared = false;
        self.lost(e)
    }

    /// Brings this window in step with the book: an unknown save of the
    /// scope opened elsewhere blocks this form too; one answered elsewhere
    /// is answered here the same way.
    pub fn sync<V: Clone + PartialEq>(&mut self, form: &mut FormState<V>) -> Option<LostEvent> {
        let (link, scope) = self.book.clone()?;
        if self.pending {
            if !self.shared {
                return None;
            }
            let state = link.lost_saves().state_of(scope, self.key.as_ref());
            let ev = match state {
                // Still open, or the entry was replaced by a newer unknown
                // save of the scope: ours stays unknown.
                Some(BookState::Open) | None => return None,
                Some(BookState::Saved(txn)) => LostEvent::Saved(txn),
                // Answered elsewhere: not saved, or the owner looked in the
                // register there and confirmed saving again.
                Some(BookState::Cleared) => {
                    self.end();
                    form.recheck_answered(RecheckAnswer::NotSaved);
                    self.notice = Some("lost.answered_elsewhere");
                    return Some(LostEvent::NotSaved);
                }
            };
            self.apply(form, ev);
            return Some(ev);
        }
        let key = link.lost_saves().open(scope)?;
        self.pending = true;
        self.shared = true;
        self.key = key;
        self.notice = None;
        self.confirm_open = false;
        form.mark_unconfirmed();
        Some(LostEvent::StillUnknown)
    }

    /// Takes note of a failed save. True when its outcome is unknown and
    /// a second send could post twice ([`ApiError::needs_recheck`]): the
    /// form then calls `save_unknown` instead of `save_failed` and shows
    /// this banner instead of the error. A `keyed` unknown outcome is
    /// false: the form shows the error ("save again, it is not entered
    /// twice") and Save sends the same key again (U-18, Q-PR1a).
    pub fn lost(&mut self, e: &ApiError) -> bool {
        match e {
            ApiError::OutcomeUnknown {
                idempotency_key, ..
            } if e.needs_recheck() => {
                self.pending = true;
                self.key.clone_from(idempotency_key);
                self.notice = None;
                self.confirm_open = false;
                if let Some((link, scope)) = &self.book {
                    link.lost_saves().remember(*scope, self.key.clone());
                    self.shared = true;
                }
                true
            }
            _ => false,
        }
    }

    /// Writes this form's answer in the book.
    fn tell_book(&self, state: BookState) {
        if self.shared
            && let Some((link, scope)) = &self.book
        {
            link.lost_saves().answer(*scope, self.key.as_ref(), state);
        }
    }

    /// An unknown save is waiting for an answer.
    #[must_use]
    pub fn pending(&self) -> bool {
        self.pending
    }

    /// A recheck is running.
    #[must_use]
    pub fn checking(&self) -> bool {
        self.task.is_some()
    }

    /// The save that was sent went through normally (the form's own
    /// success path): nothing is unknown any more.
    pub fn saved(&mut self) {
        if self.pending {
            self.tell_book(BookState::Cleared);
        }
        self.pending = false;
        self.shared = false;
        self.key = None;
        self.task = None;
        self.confirm_open = false;
        self.after_unknown = false;
    }

    /// Check again: re-runs the stored probe (reads only; nothing is
    /// sent). Without a session (demo) or a key the answer is "still
    /// unknown" at once.
    pub fn recheck<V: Clone + PartialEq>(
        &mut self,
        form: &mut FormState<V>,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
    ) {
        if !self.pending || !form.begin_recheck() {
            return;
        }
        self.checks += 1;
        self.notice = None;
        let (Some(key), Some(session), Some(group)) = (self.key.clone(), link.session(), group)
        else {
            self.task = Some(Task::ready(Recheck::Unknown));
            return;
        };
        self.task = Some(
            match group.read(&session, move |s| async move { s.recheck(&key).await }) {
                Ok(p) => Task::Pending(p),
                Err(_) => Task::ready(Recheck::Unknown),
            },
        );
    }

    /// Tests: a recheck whose engine answers `r`.
    #[cfg(test)]
    pub(crate) fn recheck_answering<V: Clone + PartialEq>(
        &mut self,
        form: &mut FormState<V>,
        r: Recheck,
    ) {
        if self.pending && form.begin_recheck() {
            self.checks += 1;
            self.task = Some(Task::ready(r));
        }
    }

    /// The recheck's answer, applied to `form`'s block. `Saved` and
    /// `NotSaved` end the unknown save; the form shows the saved record
    /// for `Saved`.
    pub fn poll<V: Clone + PartialEq>(&mut self, form: &mut FormState<V>) -> Option<LostEvent> {
        let r = self.task.as_mut().and_then(Task::try_take)?;
        self.task = None;
        // The engine could not be reached: still unknown (the owner may
        // override, owner answer 2026-10-24).
        let ev = match r {
            Ok(Recheck::Committed { transaction }) => LostEvent::Saved(transaction),
            Ok(Recheck::NotCommitted) => LostEvent::NotSaved,
            Ok(Recheck::Unknown) | Err(_) => LostEvent::StillUnknown,
        };
        self.apply(form, ev);
        Some(ev)
    }

    fn apply<V: Clone + PartialEq>(&mut self, form: &mut FormState<V>, ev: LostEvent) {
        match ev {
            LostEvent::Saved(txn) => {
                form.recheck_answered(RecheckAnswer::Saved);
                self.tell_book(BookState::Saved(txn));
                self.end();
                self.notice = Some("lost.saved");
            }
            LostEvent::NotSaved => {
                form.recheck_answered(RecheckAnswer::NotSaved);
                self.tell_book(BookState::Cleared);
                self.end();
                self.notice = Some("lost.not_saved");
            }
            LostEvent::StillUnknown => {
                form.recheck_answered(RecheckAnswer::Unknown);
                self.notice = Some("lost.still_unknown");
            }
            LostEvent::OwnerConfirmed => {
                form.owner_confirms();
                self.tell_book(BookState::Cleared);
                self.end();
                self.after_unknown = true;
            }
        }
    }

    fn end(&mut self) {
        self.pending = false;
        self.shared = false;
        self.key = None;
        self.confirm_open = false;
    }

    /// "I checked the register — save again": opens the confirm dialog.
    pub fn ask_owner(&mut self) {
        if self.pending {
            self.confirm_open = true;
        }
    }

    /// The dialog's "It is not there — save again": lifts the block and
    /// logs the override (`what` names the form; no amounts, names or memo
    /// text). The form may now save again.
    pub fn owner_confirmed<V: Clone + PartialEq>(
        &mut self,
        form: &mut FormState<V>,
        what: &'static str,
    ) -> Option<LostEvent> {
        if !self.pending {
            return None;
        }
        tracing::info!(
            form = what,
            rechecks = self.checks,
            "owner confirmed saving again after an unknown outcome"
        );
        self.apply(form, LostEvent::OwnerConfirmed);
        Some(LostEvent::OwnerConfirmed)
    }

    /// The banner (when a save is unknown) and the confirm dialog. Returns
    /// what the owner clicked: `Some(true)` Check again, the dialog's
    /// confirmation as an event through [`Self::owner_confirmed`].
    pub fn ui<V: Clone + PartialEq>(
        &mut self,
        ui: &mut Ui,
        form: &mut FormState<V>,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        what: &'static str,
    ) -> Option<LostEvent> {
        let tokens = Tokens::of(ui.ctx());
        // Every frame: the form's unknown saves live in the company's book
        // under its name, and one opened or answered in another window of
        // the same form shows here too.
        let mut out = if self.book.is_none() && !self.templates {
            self.bind(link, LostScope::Form(what), form)
        } else {
            self.sync(form)
        };
        if let Some(n) = self.notice.filter(|_| !self.pending) {
            ui.label(RichText::new(t(n)).color(tokens.text_secondary));
        }
        if !self.pending {
            return out;
        }
        egui::Frame::new()
            .fill(tokens.warn_tint)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(t("lost.title"))
                        .strong()
                        .color(tokens.warn_text),
                );
                let body = if self.checking() {
                    "lost.checking"
                } else {
                    self.notice.unwrap_or("lost.body")
                };
                ui.add(egui::Label::new(RichText::new(t(body)).color(tokens.text_primary)).wrap());
                ui.horizontal(|ui| {
                    let check =
                        ui.add_enabled(!self.checking(), egui::Button::new(t("lost.check_again")));
                    if check.clicked() {
                        self.recheck(form, link, group);
                    }
                    if ui
                        .add_enabled(!self.checking(), egui::Button::new(t("lost.save_anyway")))
                        .clicked()
                    {
                        self.ask_owner();
                    }
                });
            });
        if self.confirm_open {
            let mut answer = None;
            egui::Window::new(t("lost.confirm.title"))
                .id(egui::Id::unique(("lost-save-confirm", what)))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.add(egui::Label::new(t("lost.confirm.body")).wrap());
                    ui.horizontal(|ui| {
                        if ui.button(t("lost.confirm.yes")).clicked() {
                            answer = Some(true);
                        }
                        if ui.button(t("lost.confirm.no")).clicked() {
                            answer = Some(false);
                        }
                    });
                });
            match answer {
                Some(true) => out = self.owner_confirmed(form, what),
                Some(false) => self.confirm_open = false,
                None => {}
            }
        }
        if let Some(ev) = self.poll(form) {
            out = Some(ev);
        }
        out
    }

    /// The control-channel state (flags only).
    #[must_use]
    pub fn state_json(&self) -> Value {
        json!({
            "pending": self.pending,
            "checking": self.checking(),
            "confirm_open": self.confirm_open,
            "notice": self.notice,
            "checks": self.checks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::OwnerMessage;
    use cl_model::forms::{SaveFailure, SaveIntent, SaveStart};

    fn unknown() -> ApiError {
        ApiError::OutcomeUnknown {
            request_id: cl_core::ids::RequestId::parse("00000000-0000-4000-8000-000000000001")
                .expect("id"),
            idempotency_key: Some(IdempotencyKey::new()),
            keyed: false,
        }
    }

    fn failure() -> SaveFailure {
        SaveFailure {
            message: OwnerMessage::plain("error.api.outcome_unknown"),
            detail: String::new(),
        }
    }

    /// Review fix-PR-1 blocking 2: the book keeps an unknown save per
    /// company and scope; a second window picks it up and hears the answer.
    #[test]
    fn the_book_spans_windows_and_is_kept_per_company() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let a = TenantId::parse("d0000000-0000-4000-8000-000000000001").expect("t");
        let b = TenantId::parse("d0000000-0000-4000-8000-000000000002").expect("t");
        link.lost_saves().set_company(Some(a));
        let scope = LostScope::Form("deposit");
        let (mut one, mut two) = (FormState::new(String::new()), FormState::new(String::new()));
        let (mut first, mut second) = (LostSave::default(), LostSave::default());
        assert_eq!(first.bind(&link, scope, &mut one), None);
        assert_eq!(second.bind(&link, scope, &mut two), None);
        let _ = one.begin_save(SaveIntent::Save);
        assert!(first.lost(&unknown()));
        one.save_unknown(failure());
        // The other window of the form blocks too, with the same key.
        assert_eq!(second.sync(&mut two), Some(LostEvent::StillUnknown));
        assert!(second.pending() && two.is_unconfirmed());
        assert_eq!(second.key, first.key);
        // Another company never sees it; back in this one it is there.
        link.lost_saves().set_company(Some(b));
        assert!(!link.lost_saves().is_open(scope));
        link.lost_saves().set_company(Some(a));
        assert!(link.lost_saves().is_open(scope));
        // Found in the first window: the second shows it as saved.
        first.recheck_answering(&mut one, Recheck::Committed { transaction: None });
        assert_eq!(first.poll(&mut one), Some(LostEvent::Saved(None)));
        assert_eq!(second.sync(&mut two), Some(LostEvent::Saved(None)));
        assert!(!second.pending() && !two.is_unconfirmed());
        assert!(!link.lost_saves().is_open(scope));
        // A new unknown save, answered "not saved" elsewhere: lifted here.
        let _ = one.begin_save(SaveIntent::Save);
        assert!(first.lost(&unknown()));
        one.save_unknown(failure());
        assert_eq!(second.sync(&mut two), Some(LostEvent::StillUnknown));
        first.recheck_answering(&mut one, Recheck::NotCommitted);
        let _ = first.poll(&mut one);
        assert_eq!(second.sync(&mut two), Some(LostEvent::NotSaved));
        assert_eq!(second.notice, Some("lost.answered_elsewhere"));
        assert!(!two.is_unconfirmed());
    }

    /// U-18 (Q-PR1a): a keyed unknown outcome never blocks Save; the same
    /// key goes again and the engine replays it.
    #[test]
    fn a_keyed_unknown_outcome_does_not_block() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let scope = LostScope::Form("deposit");
        let mut form = FormState::new(String::new());
        let mut lost = LostSave::default();
        assert_eq!(lost.bind(&link, scope, &mut form), None);
        form.edit(1, |v| v.push('x'));
        let _ = form.begin_save(SaveIntent::Save);
        let keyed = ApiError::OutcomeUnknown {
            request_id: cl_core::ids::RequestId::parse("00000000-0000-4000-8000-000000000002")
                .expect("id"),
            idempotency_key: Some(IdempotencyKey::new()),
            keyed: true,
        };
        assert!(!keyed.needs_recheck());
        assert!(!lost.lost(&keyed));
        assert!(!lost.lost_as(&keyed, &link, scope));
        assert!(!lost.pending());
        assert!(!link.lost_saves().is_open(scope));
        form.save_failed(failure());
        assert!(matches!(
            form.begin_save(SaveIntent::Save),
            SaveStart::Send(_)
        ));
        // An unkeyed one still blocks.
        assert!(unknown().needs_recheck());
    }

    #[test]
    fn demo_recheck_is_still_unknown_and_only_the_owner_lifts_it() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let mut form = FormState::new(String::new());
        form.edit(1, |v| v.push('x'));
        let _ = form.begin_save(SaveIntent::Save);
        let mut lost = LostSave::default();
        assert!(!lost.lost(&ApiError::Timeout), "a timeout is not this");
        assert!(lost.lost(&unknown()));
        form.save_unknown(failure());
        lost.recheck(&mut form, &link, None);
        assert_eq!(lost.poll(&mut form), Some(LostEvent::StillUnknown));
        assert_eq!(form.begin_save(SaveIntent::Save), SaveStart::Unconfirmed);
        // The dialog opens first; only its confirmation lifts the block.
        lost.ask_owner();
        assert!(lost.confirm_open);
        assert_eq!(
            lost.owner_confirmed(&mut form, "test"),
            Some(LostEvent::OwnerConfirmed)
        );
        assert!(lost.after_unknown);
        assert!(!lost.pending());
        assert!(matches!(
            form.begin_save(SaveIntent::Save),
            SaveStart::Send(_)
        ));
        assert_eq!(lost.owner_confirmed(&mut form, "test"), None);
    }
}
