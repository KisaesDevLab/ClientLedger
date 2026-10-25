//! "We could not confirm the save" (B2, Q-PR1a): the one component every
//! posting form uses after a save whose outcome is unknown.
//!
//! The engine has no request idempotency on its posting routes until U-18
//! (only `/batch/save` honours `Idempotency-Key`), so sending the same save
//! again could post it twice. The rule itself lives in
//! `cl_model::forms::FormState` (Save answers `Unconfirmed` while the mark
//! is set); this component runs the owner's **Check again**
//! (`Session::recheck`, the stored lost-reply probe, read-only) and the
//! explicit **I checked the register — save again**, which asks once more
//! in a dialog before it lifts the block (owner answer 2026-10-24). The
//! override is logged without amounts, names or memo text.

use std::sync::Arc;

use cl_api::{ApiError, IdempotencyKey, Recheck, RequestGroup};
use cl_core::ids::TransactionId;
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
    /// Check again on a form with its own, safer check
    /// ([`LostSave::own_check`]): the form reads what it needs and hands
    /// the answer to [`LostSave::answer`].
    CheckRequested,
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
    /// The form answers Check again itself (Daily Sales reads the day's
    /// entry by id: its status settles it).
    own_check: bool,
}

impl LostSave {
    /// Takes note of a failed save. True when its outcome is unknown: the
    /// form then calls `save_unknown` instead of `save_failed` and shows
    /// this banner instead of the error.
    pub fn lost(&mut self, e: &ApiError) -> bool {
        match e {
            ApiError::OutcomeUnknown {
                idempotency_key, ..
            } => {
                self.pending = true;
                self.key.clone_from(idempotency_key);
                self.notice = None;
                self.confirm_open = false;
                self.own_check = false;
                true
            }
            _ => false,
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
        self.pending = false;
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

    /// The form runs its own check for this unknown save (`true`) or the
    /// stored probe (`false`, the default).
    pub fn own_check(&mut self, on: bool) {
        self.own_check = on;
    }

    /// The form's own check started: false when nothing is unknown or a
    /// check is running.
    pub fn begin_own<V: Clone + PartialEq>(&mut self, form: &mut FormState<V>) -> bool {
        if !self.pending || !form.begin_recheck() {
            return false;
        }
        self.checks += 1;
        self.notice = None;
        true
    }

    /// The form's own check answered.
    pub fn answer<V: Clone + PartialEq>(
        &mut self,
        form: &mut FormState<V>,
        r: Recheck,
    ) -> LostEvent {
        let ev = match r {
            Recheck::Committed { transaction } => LostEvent::Saved(transaction),
            Recheck::NotCommitted => LostEvent::NotSaved,
            Recheck::Unknown => LostEvent::StillUnknown,
        };
        self.apply(form, ev);
        ev
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
            LostEvent::Saved(_) => {
                form.recheck_answered(RecheckAnswer::Saved);
                self.end();
                self.notice = Some("lost.saved");
            }
            LostEvent::NotSaved => {
                form.recheck_answered(RecheckAnswer::NotSaved);
                self.end();
                self.notice = Some("lost.not_saved");
            }
            LostEvent::StillUnknown => {
                form.recheck_answered(RecheckAnswer::Unknown);
                self.notice = Some("lost.still_unknown");
            }
            LostEvent::OwnerConfirmed => {
                form.owner_confirms();
                self.end();
                self.after_unknown = true;
            }
            LostEvent::CheckRequested => {}
        }
    }

    fn end(&mut self) {
        self.pending = false;
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
        let mut out = None;
        if let Some(n) = self.notice.filter(|_| !self.pending) {
            ui.label(RichText::new(t(n)).color(tokens.text_secondary));
        }
        if !self.pending {
            return None;
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
                        if self.own_check {
                            out = Some(LostEvent::CheckRequested);
                        } else {
                            self.recheck(form, link, group);
                        }
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
        }
    }

    fn failure() -> SaveFailure {
        SaveFailure {
            message: OwnerMessage::plain("error.api.outcome_unknown"),
            detail: String::new(),
        }
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
