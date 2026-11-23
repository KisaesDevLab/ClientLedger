//! Receive Payments' side of the save-conflict dialog (P33 D4 step 11). A
//! 409 `STALE` on an edit reads their payment; nothing editable changed on
//! their side: it saves again on their stamp without asking; otherwise the
//! shared dialog opens.
//!
//! The three versions are this form's own fields: "as opened" and theirs
//! are each loaded into a scratch form the same way the form loads a saved
//! payment, so the invoice applications compare in one shape.
//!
//! Applications move money between invoices, so they never merge (Q-P33ao):
//! when their applications changed, Keep mine shows their applications
//! with my other changes, saves nothing, and asks the owner to check which
//! invoices the payment pays. When only header fields changed, Keep mine
//! keeps my applications and saves the merge on their stamp.

use cl_api::{ApiError, PaymentRecord};
use cl_model::forms::SaveIntent;
use cl_model::forms::payment::{AmountInput, Editing, PaymentForm};
use cl_model::lan::Conflict;
use serde_json::{Value, json};

use super::{After, ReceivePaymentScreen, engine};
use crate::forms::conflict::{GONE, OpenConflict, Read, Words, read_of};
use crate::screens::company::form::DateInput;
use crate::screens::conflict_dialog::{ConflictChoice, fields_from_theirs};

/// The notice after Keep mine when their applications changed (Q-P33ao).
pub(crate) const RECHECK: &str = "lan.payment.recheck";

/// What the save was for, as the dialog keeps it.
fn intent_of(after: After) -> SaveIntent {
    match after {
        After::Stay => SaveIntent::Save,
        After::New => SaveIntent::SaveAndNew,
        After::Close => SaveIntent::SaveAndClose,
    }
}

fn after_of(intent: SaveIntent) -> After {
    match intent {
        SaveIntent::Save => After::Stay,
        SaveIntent::SaveAndNew => After::New,
        SaveIntent::SaveAndClose => After::Close,
    }
}

/// The editable fields of a payment as this form holds them; the invoice
/// applications, credits used and a held deposit are one field.
#[must_use]
pub fn payment_value(f: &PaymentForm) -> Value {
    let req = f.to_preview_request();
    let mut rows: Vec<Value> = req
        .as_ref()
        .and_then(|r| r.rows.as_ref())
        .map(|rows| {
            rows.iter()
                .map(|r| {
                    json!({
                        "invoice": r.invoice.to_string(),
                        "amount": r.amount.map(cl_core::Money::to_wire),
                        "discount": r.discount.map(cl_core::Money::to_wire),
                        "writeOff": r.write_off.map(cl_core::Money::to_wire),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    rows.sort_by_key(cl_model::lan::canonical);
    let mut credits: Vec<Value> = f
        .credits()
        .iter()
        .map(|c| {
            json!({
                "source": c.source.to_string(),
                "invoice": c.invoice.to_string(),
                "amount": c.amount.map(cl_core::Money::to_wire),
            })
        })
        .collect();
    credits.sort_by_key(cl_model::lan::canonical);
    json!({
        "client": f.client.map(|c| c.to_string()),
        "txnDate": f.date.map(cl_core::CalendarDate::to_wire),
        "amount": match f.amount() {
            AmountInput::Typed(m) => Some(m.to_wire()),
            AmountInput::Empty => None,
        },
        "depositTo": f.deposit_to().map(|a| a.to_string()),
        "paymentMethod": f.method.trim(),
        "refNo": f.ref_no.trim(),
        "memo": f.memo.trim(),
        "applications": {
            "rows": rows,
            "credits": credits,
            "hold": req.as_ref().is_some_and(|r| r.hold),
        },
    })
}

impl ReceivePaymentScreen {
    /// `rec` loaded into a scratch form, as [`Self::open_saved`] loads it.
    fn form_of(&self, rec: &PaymentRecord) -> PaymentForm {
        let mut f = PaymentForm::default();
        f.load_saved(
            Editing {
                id: rec.id,
                updated_at: rec.updated_at,
            },
            self.loaded_of(rec),
        );
        f
    }

    /// A save came back stale: read their version (the dialog follows).
    pub(crate) fn conflict_start(&mut self, after: After) {
        let Some(id) = self.editing.as_ref().map(|e| e.id) else {
            self.notice = Some("payment.refusal.stale");
            return;
        };
        self.conflict_job = Some((
            engine::read(
                &self.link,
                self.group.as_ref(),
                move |d| crate::company::demo::receivables::get(d, id),
                move |s| async move { s.customer_payments().get(id).await },
            ),
            after,
        ));
        self.poll_conflict();
    }

    /// Polls the read of their version.
    pub(crate) fn poll_conflict(&mut self) {
        let Some((mut job, after)) = self.conflict_job.take() else {
            return;
        };
        match job.take() {
            Some(r) => self.conflict_read(r, after),
            None => self.conflict_job = Some((job, after)),
        }
    }

    pub(crate) fn conflict_read(&mut self, r: Result<PaymentRecord, ApiError>, after: After) {
        let theirs = match read_of(r) {
            Read::Theirs(v) => v,
            Read::Gone => {
                self.notice = Some(GONE);
                return;
            }
            Read::Reload => {
                self.notice = Some("payment.refusal.stale");
                return;
            }
        };
        let Some(opened) = self.editing.clone() else {
            self.notice = Some("payment.refusal.stale");
            return;
        };
        // Bounced, void or a write-off-only payment: shown, not merged.
        if theirs.is_bounced() || theirs.is_void() || theirs.deposit_to.is_none() {
            self.notice = Some("payment.refusal.stale");
            return;
        }
        let conflict = Conflict::new(
            &payment_value(&self.form_of(&opened)),
            &payment_value(&self.form),
            &payment_value(&self.form_of(&theirs)),
            "lines",
        );
        if conflict.silent_resave() {
            // Nothing editable changed on their side: save again on their
            // stamp without asking (once: a second stale in a row says it
            // changed elsewhere instead of trying again).
            if std::mem::replace(&mut self.silent_tried, true) {
                self.notice = Some("payment.refusal.stale");
                return;
            }
            self.editing = Some(theirs);
            self.save_key.clear();
            self.start_save(after, None);
            return;
        }
        let words = Words::new(
            std::sync::Arc::clone(&self.books.lists),
            None,
            self.money_style,
            &["amount"],
        );
        let dialog =
            crate::screens::conflict_dialog::ConflictDialog::new(conflict, words.line_text())
                .with_field_text(Box::new(move |f, v| {
                    if f == "applications" {
                        let n = v.get("rows").and_then(Value::as_array).map_or(0, Vec::len)
                            + v.get("credits")
                                .and_then(Value::as_array)
                                .map_or(0, Vec::len);
                        Some(crate::i18n::t_with(
                            "lan.payment.applications",
                            &[("n", &crate::i18n::count_text(n as u64))],
                        ))
                    } else {
                        words.field(f, v)
                    }
                }));
        self.conflict.open = Some(OpenConflict {
            dialog,
            theirs,
            intent: intent_of(after),
        });
    }

    /// Their header changes I didn't make, onto the form.
    fn take_their_fields(&mut self, theirs: &PaymentForm, fields: &[String]) {
        for field in fields {
            match field.as_str() {
                "client" => self.form.set_client(theirs.client),
                "txnDate" => {
                    self.form.set_date(theirs.date);
                    if let Some(d) = theirs.date {
                        self.date = DateInput::new(d);
                    }
                }
                "amount" => {
                    self.form.set_amount(theirs.amount());
                    self.amount_text = match theirs.amount() {
                        AmountInput::Typed(m) => super::shown(m),
                        AmountInput::Empty => String::new(),
                    };
                }
                "depositTo" => self.form.set_deposit_to(theirs.deposit_to()),
                "paymentMethod" => self.form.method.clone_from(&theirs.method),
                "refNo" => self.form.ref_no.clone_from(&theirs.ref_no),
                "memo" => self.form.memo.clone_from(&theirs.memo),
                _ => {}
            }
        }
    }

    /// Draws the open dialog and runs the owner's choice.
    pub(crate) fn conflict_ui(&mut self, ctx: &eframe::egui::Context) {
        if let Some((open, choice)) = self.conflict.ui(ctx) {
            self.run_choice(open, choice);
        }
    }

    #[allow(clippy::needless_pass_by_value)] // the dialog hands over its choice
    pub(crate) fn run_choice(&mut self, open: OpenConflict<PaymentRecord>, choice: ConflictChoice) {
        match choice {
            ConflictChoice::Back => self.notice = Some("payment.refusal.stale"),
            ConflictChoice::UseTheirs => self.open_saved(open.theirs),
            ConflictChoice::KeepMine(_) => {
                let c = open.dialog.conflict();
                let diffs = c.field_changes();
                let changed = |name: &str, theirs: bool| {
                    diffs.iter().any(|f| {
                        f.field == name
                            && if theirs {
                                f.theirs_changed()
                            } else {
                                f.mine_changed()
                            }
                    })
                };
                // Their applications, or their amount over my applications,
                // could misapply money: never merged (Q-P33ao).
                let their_apps = changed("applications", true)
                    || (changed("amount", true) && changed("applications", false));
                let theirs_form = self.form_of(&open.theirs);
                if their_apps {
                    // Q-P33ao: their applications, my other changes; the
                    // owner checks the invoices, then saves.
                    let mine: Vec<String> = c
                        .field_changes()
                        .into_iter()
                        .filter(|f| f.mine_changed() && f.field != "applications")
                        .map(|f| f.field)
                        .collect();
                    let my_form = self.form.clone();
                    self.open_saved(open.theirs);
                    self.take_their_fields(&my_form, &mine);
                    self.dirty = true;
                    self.notice = Some(RECHECK);
                    return;
                }
                let fields = fields_from_theirs(c);
                self.take_their_fields(&theirs_form, &fields);
                self.editing = Some(open.theirs);
                self.save_key.clear();
                self.dirty = true;
                self.resave = Some(after_of(open.intent));
            }
        }
    }

    /// A Keep mine waiting for the preview of the merged form.
    pub(crate) fn poll_resave(&mut self) {
        if self.resave.is_some()
            && self.preview.is_none()
            && matches!(
                self.form.figures(),
                cl_model::forms::payment::FiguresView::Ready(_)
            )
            && let Some(after) = self.resave.take()
        {
            self.start_save(after, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_has_plain_words() {
        let v = payment_value(&PaymentForm::default());
        for k in v.as_object().expect("object").keys() {
            assert!(crate::i18n::has(&format!("lan.field.{k}")), "{k}");
        }
        assert!(crate::i18n::has(RECHECK));
    }
}
