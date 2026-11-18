//! Spot-check a few items (step 7, QuickBooks Desktop only, Q-144): the
//! guided list from `cl_model::imports::spot_check` (bank balances, the
//! largest open invoices and bills, sales tax and payroll liabilities, net
//! income year to date, inventory), each with a tick and a note. The list
//! is saved with the conversion record: a moment after each change
//! (debounced), on Finish and when the window closes. A batch reopened
//! from History shows its saved list.
//!
//! Sample transactions: the engine has no list of a batch's transactions
//! yet, so the five samples are left out (P31-A-status D4).

use std::time::{Duration, Instant};

use cl_api::Account;
use cl_api::imports::{ImportBatch, ImportBatchId, SpotCheckItem, TieOut};
use cl_model::imports::spot_check::{self, AccountRef, Input, OpenDoc};
use eframe::egui::{self, RichText, Ui};

use super::{ImportScreen, Job, poll};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;

/// How long after the last change the list is saved.
pub const DEBOUNCE: Duration = Duration::from_millis(1200);

/// The list and its saving.
#[derive(Default)]
pub struct SpotList {
    /// The items, as saved (key, label, tick, note).
    pub(crate) items: Vec<SpotCheckItem>,
    /// The batch the list belongs to.
    pub(crate) batch: Option<ImportBatchId>,
    /// Changed and not saved since (the debounce runs from here).
    pub(crate) changed_at: Option<Instant>,
    save_job: Option<Job<()>>,
    /// Saves the engine took (control channel).
    pub(crate) saves: u32,
    /// The last save failed (the next change tries again).
    pub(crate) failed: bool,
}

/// An item's words.
fn label(item: &spot_check::Item) -> String {
    t_with(
        &format!("imports.spot.{}", item.kind.key()),
        &[("subject", item.subject.as_deref().unwrap_or_default())],
    )
}

fn account_ref(a: &Account) -> AccountRef {
    AccountRef {
        id: a.id.to_string(),
        name: a.fields.name.clone(),
    }
}

fn open_docs(items: &[cl_api::imports::TieOutItem]) -> Vec<OpenDoc> {
    items
        .iter()
        .map(|i| OpenDoc {
            source_row: i.source_row,
            number: i.number.clone().unwrap_or_default(),
            name: i.name.clone(),
            open: i.books,
        })
        .collect()
}

/// What the generator reads: the company's accounts (after the import)
/// and the tie-out's open items.
#[must_use]
pub fn input(batch: &ImportBatch, tie: Option<&TieOut>, chart: &[Account]) -> Input {
    let active = || chart.iter().filter(|a| a.is_active);
    let detail = |a: &Account| a.fields.detail_type.clone().unwrap_or_default();
    Input {
        banks: active()
            .filter(|a| {
                let d = detail(a);
                d == "credit_card" || cl_model::register::is_bank_detail(&d)
            })
            .map(account_ref)
            .collect(),
        open_invoices: tie.map(|t| open_docs(&t.ar_items)).unwrap_or_default(),
        unpaid_bills: tie.map(|t| open_docs(&t.ap_items)).unwrap_or_default(),
        sales_tax: active()
            .find(|a| a.system_tag.as_deref() == Some("sales_tax_payable"))
            .map(account_ref),
        payroll: active()
            .find(|a| {
                a.system_tag.as_deref() == Some("payroll_liabilities")
                    || detail(a) == "payroll_liabilities"
            })
            .map(account_ref),
        inventory: tie.is_some_and(|t| !t.inventory_items.is_empty()),
        transactions: Vec::new(),
        batch_id: uuid::Uuid::parse_str(&batch.id.to_string()).map_or(0, |u| u.as_u128()),
    }
}

impl SpotList {
    /// The list of `batch`: its saved one, or a new one from the tie-out
    /// (marked changed, so it is saved).
    pub(crate) fn build(&mut self, batch: &ImportBatch, tie: Option<&TieOut>, chart: &[Account]) {
        self.batch = Some(batch.id);
        self.failed = false;
        if !batch.spot_check.is_empty() {
            self.items.clone_from(&batch.spot_check);
            self.changed_at = None;
            return;
        }
        self.items = spot_check::generate(&input(batch, tie, chart))
            .iter()
            .map(|i| SpotCheckItem {
                key: i.key.clone(),
                label: label(i),
                done: i.done,
                note: i.note.clone(),
            })
            .collect();
        self.changed_at = Some(Instant::now());
    }

    /// Changed since the last save.
    pub(crate) fn unsaved(&self) -> bool {
        self.changed_at.is_some()
    }

    /// Every item ticked.
    pub(crate) fn complete(&self) -> bool {
        self.items.iter().all(|i| i.done)
    }

    /// A save is out.
    pub(crate) fn saving(&self) -> bool {
        self.save_job.is_some()
    }
}

impl ImportScreen {
    /// Saves the list now (Finish, the window closing, the debounce).
    pub(crate) fn save_spot_check(&mut self) {
        let Some(id) = self.spot.batch else {
            return;
        };
        if self.spot.items.is_empty() || !self.allowed() {
            self.spot.changed_at = None;
            return;
        }
        let items = self.spot.items.clone();
        self.spot.changed_at = None;
        self.spot.save_job = Some(self.write(
            "orphan.import_spot_check",
            None,
            {
                let items = items.clone();
                move |d| d.imports.save_spot_check(id, &items)
            },
            move |s| async move { s.imports().save_spot_check(id, &items).await },
        ));
    }

    /// Takes a save's answer; saves once the owner paused.
    pub(crate) fn poll_spot_check(&mut self) {
        if let Some(r) = poll(&mut self.spot.save_job) {
            match r {
                Ok(()) => {
                    self.spot.saves += 1;
                    self.spot.failed = false;
                }
                Err(e) => {
                    tracing::info!(target: "imports", code = super::Trouble::of(&e).code(), "spot-check not saved");
                    self.spot.failed = true;
                }
            }
        }
        if self.spot.save_job.is_none()
            && self
                .spot
                .changed_at
                .is_some_and(|at| at.elapsed() >= DEBOUNCE)
        {
            self.save_spot_check();
        }
    }

    /// The list after the window was closed or reopened: from History's
    /// batch when it has one.
    pub(crate) fn load_spot_check(&mut self, b: &ImportBatch) {
        self.spot.build(b, self.tieout.as_ref(), &self.chart);
    }
}

/// Draws the step.
pub fn spot_check_ui(s: &mut ImportScreen, ui: &mut Ui, tokens: &Tokens) {
    ui.add(
        egui::Label::new(RichText::new(t("imports.spot.intro")).color(tokens.text_secondary))
            .wrap(),
    );
    ui.add_space(8.0);
    if s.wizard.verified {
        ui.label(RichText::new(t("imports.spot.verified")).color(tokens.accent));
    }
    ui.add_space(6.0);
    if s.spot.items.is_empty() {
        ui.label(RichText::new(t("imports.spot.empty")).color(tokens.text_tertiary));
        return;
    }
    let mut changed = false;
    let editable = s.allowed();
    for item in &mut s.spot.items {
        ui.horizontal_wrapped(|ui| {
            changed |= ui
                .add_enabled(editable, egui::Checkbox::new(&mut item.done, &item.label))
                .changed();
        });
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            let r = ui.add_enabled(
                editable,
                egui::TextEdit::singleline(&mut item.note)
                    .hint_text(t("imports.spot.note_hint"))
                    .desired_width(360.0),
            );
            let name = t_with("imports.spot.note_for", &[("item", &item.label)]);
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, editable, &name));
            changed |= r.changed();
        });
        ui.add_space(4.0);
    }
    if changed {
        s.spot.changed_at = Some(Instant::now());
        ui.ctx().request_repaint_after(DEBOUNCE);
    } else if s.spot.unsaved() {
        ui.ctx().request_repaint_after(Duration::from_millis(200));
    }
    ui.add_space(6.0);
    let status = if s.spot.failed {
        t("imports.spot.not_saved")
    } else if s.spot.unsaved() || s.spot.saving() {
        t("imports.spot.saving")
    } else {
        t("imports.spot.saved")
    };
    let color = if s.spot.failed {
        tokens.warn_text
    } else {
        tokens.text_secondary
    };
    ui.label(RichText::new(status).color(color));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spot_check_kind_has_words() {
        for k in [
            spot_check::Kind::Bank,
            spot_check::Kind::OpenInvoice,
            spot_check::Kind::UnpaidBill,
            spot_check::Kind::SalesTax,
            spot_check::Kind::Payroll,
            spot_check::Kind::NetIncome,
            spot_check::Kind::Inventory,
            spot_check::Kind::Sample,
        ] {
            assert!(
                crate::i18n::has(&format!("imports.spot.{}", k.key())),
                "{k:?}"
            );
        }
    }
}
