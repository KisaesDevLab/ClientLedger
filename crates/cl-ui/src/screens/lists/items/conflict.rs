//! The Item List's side of the save-conflict dialog (P33 D4 step 11), as
//! the Tags list's (header-only): an edit is guarded by the item as the
//! panel opened it, a 409 `STALE` reads their item, saves again without
//! asking when nothing editable changed on their side (once per save), and
//! opens [`crate::screens::conflict_dialog`] otherwise. The opening-stock
//! fields belong to a new part only and are never compared.

use std::collections::HashMap;

use cl_api::{ApiError, EditableFields, Item, Versioned};
use cl_core::ids::ItemId;
use cl_model::forms::SaveIntent;
use cl_model::lan::Conflict;
use cl_model::lists::items::ItemDraft;
use serde_json::{Value, json};

use super::{ItemsScreen, entry_of};
use crate::company::Task;
use crate::forms::conflict::{GONE, OpenConflict, Read, Words, read_of};
use crate::i18n::t;
use crate::screens::conflict_dialog::{ConflictChoice, ConflictDialog, fields_from_theirs};

/// The "changed elsewhere" notice (the plain refusal).
const CHANGED: &str = "lan.list.changed_elsewhere";

fn id_text<T: ToString>(v: Option<T>) -> Option<String> {
    v.map(|x| x.to_string())
}

/// The editable fields of an item as the panel holds them.
#[must_use]
pub fn item_value(d: &ItemDraft) -> Value {
    let p = &d.part;
    json!({
        "itemType": d.kind.label().as_str(),
        "name": d.name.trim(),
        "description": d.description.trim(),
        "price": d.price.trim(),
        "discountPercent": d.discount.trim(),
        "incomeAccount": id_text(d.income_account),
        "taxable": d.taxable,
        "taxCode": id_text(d.tax_code),
        "purchaseDescription": p.purchase_description.trim(),
        "cost": p.cost.trim(),
        "expenseAccount": id_text(p.expense_account),
        "assetAccount": id_text(p.asset_account),
        "cogsAccount": id_text(p.cogs_account),
        "preferredVendor": id_text(p.preferred_vendor),
        "reorderPoint": p.reorder_point.trim(),
        "reorderQuantity": p.reorder_quantity.trim(),
        "unitLabel": p.unit_label.trim(),
        "unitSet": id_text(p.unit_set),
        "purchaseUnit": id_text(p.purchase_unit),
        "saleUnit": id_text(p.sale_unit),
    })
}

/// An item as the panel would show it (the list's own conversion, so the
/// price and percent read the same as when the panel opened it).
pub(crate) fn draft_of(i: &Item) -> ItemDraft {
    ItemDraft::from_entry(&entry_of(i))
}

/// Applies the header fields Keep mine takes from `theirs` onto `mine`.
fn take_theirs(mine: &mut ItemDraft, theirs: &ItemDraft, fields: &[String]) {
    let (m, tp) = (&mut mine.part, &theirs.part);
    for field in fields {
        match field.as_str() {
            "itemType" => mine.kind = theirs.kind,
            "name" => mine.name.clone_from(&theirs.name),
            "description" => mine.description.clone_from(&theirs.description),
            "price" => mine.price.clone_from(&theirs.price),
            "discountPercent" => mine.discount.clone_from(&theirs.discount),
            "incomeAccount" => mine.income_account = theirs.income_account,
            "taxable" => mine.taxable = theirs.taxable,
            "taxCode" => mine.tax_code = theirs.tax_code,
            "purchaseDescription" => m.purchase_description.clone_from(&tp.purchase_description),
            "cost" => m.cost.clone_from(&tp.cost),
            "expenseAccount" => m.expense_account = tp.expense_account,
            "assetAccount" => m.asset_account = tp.asset_account,
            "cogsAccount" => m.cogs_account = tp.cogs_account,
            "preferredVendor" => m.preferred_vendor = tp.preferred_vendor,
            "reorderPoint" => m.reorder_point.clone_from(&tp.reorder_point),
            "reorderQuantity" => m.reorder_quantity.clone_from(&tp.reorder_quantity),
            "unitLabel" => m.unit_label.clone_from(&tp.unit_label),
            "unitSet" => m.unit_set = tp.unit_set,
            "purchaseUnit" => m.purchase_unit = tp.purchase_unit,
            "saleUnit" => m.sale_unit = tp.sale_unit,
            _ => {}
        }
    }
}

impl ItemsScreen {
    /// The ids the dialog may show, as the owner reads them.
    pub(crate) fn item_names(&self) -> HashMap<String, String> {
        let mut names = HashMap::new();
        for a in &self.accounts {
            names.insert(a.id.to_string(), self.account_name(a.id));
        }
        for c in &self.contacts {
            names.insert(c.id.to_string(), c.fields.display_name.clone());
        }
        for c in &self.tax_codes {
            names.insert(c.id.to_string(), format!("{} \u{b7} {}", c.code, c.name));
        }
        for s in &self.unit_sets {
            names.insert(s.id.to_string(), s.name.clone());
            for u in &s.units {
                if let Some(id) = u.id {
                    names.insert(id.to_string(), u.name.clone());
                }
            }
        }
        names
    }

    /// A save came back stale: read their item (the dialog follows).
    pub(crate) fn item_conflict_start(&mut self, id: ItemId, intent: SaveIntent) {
        self.conflict_for = Some(id);
        if self.link.is_demo() {
            let found = self
                .link
                .demo_state()
                .and_then(|mut d| d.items_mut().iter().find(|x| x.id == id).cloned());
            let got = found
                .map(|i| Versioned {
                    version: cl_api::RecordVersion::Fingerprint(i.fingerprint()),
                    record: i,
                    stock_warnings: Vec::new(),
                })
                .ok_or(ApiError::NotFound { code: None });
            self.item_conflict_read(got, intent);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.warn = Some(CHANGED);
            return;
        };
        match group.read(&session, move |s| async move { s.items().get(id).await }) {
            Ok(p) => self.item_conflict.reading = Some((Task::Pending(p), intent)),
            Err(_) => self.warn = Some(CHANGED),
        }
    }

    /// Polls the read of their item.
    pub(crate) fn poll_item_conflict(&mut self) {
        if let Some((r, intent)) = self.item_conflict.poll() {
            self.item_conflict_read(r, intent);
        }
    }

    /// The panel still edits the item the read was for.
    fn conflict_panel_matches(&self) -> bool {
        let editing = self.panel.as_ref().and_then(|p| p.editing);
        editing.is_some() && editing == self.conflict_for
    }

    pub(crate) fn item_conflict_read(
        &mut self,
        r: Result<Versioned<Item>, ApiError>,
        intent: SaveIntent,
    ) {
        if !self.conflict_panel_matches() {
            // Another item or a closed panel: the read is for nothing.
            return;
        }
        let theirs = match read_of(r) {
            Read::Theirs(v) => v,
            Read::Gone => {
                self.warn = Some(GONE);
                return;
            }
            Read::Reload => {
                self.warn = Some(CHANGED);
                return;
            }
        };
        let Some(p) = self.panel.as_ref() else {
            return;
        };
        let conflict = Conflict::new(
            &item_value(p.form.original()),
            &item_value(p.form.current()),
            &item_value(&draft_of(&theirs.record)),
            "lines",
        );
        if conflict.silent_resave() {
            if !std::mem::take(&mut self.resave_left) {
                // A second stale save in a row: say so, never loop.
                self.warn = Some(CHANGED);
                return;
            }
            let mine = p.form.current().clone();
            self.adopt_item(theirs.record, mine);
            self.save_inner(intent);
            return;
        }
        let names = self.item_names();
        let words = Words::new(
            std::sync::Arc::default(),
            None,
            cl_core::MoneyStyle::default(),
            &[],
        );
        let dialog = ConflictDialog::new(conflict, words.line_text()).with_field_text(Box::new(
            move |field, v| {
                let s = v.as_str()?;
                match field {
                    "itemType" => Some(t(s)),
                    _ => names.get(s).cloned(),
                }
            },
        ));
        self.item_conflict.open = Some(OpenConflict {
            dialog,
            theirs,
            intent,
        });
    }

    /// Their item becomes the panel's "as opened" and `mine` its fields;
    /// the list shows their item.
    fn adopt_item(&mut self, theirs: Item, mine: ItemDraft) {
        self.remember_item(&theirs);
        if let Some(p) = self.panel.as_mut() {
            p.form.load(draft_of(&theirs));
            if *p.form.current() != mine {
                p.form.edit_step(|d| *d = mine);
            }
            p.error = None;
            p.refusal = None;
            p.opened = Some(theirs);
        }
        self.warn = None;
    }

    /// The list's copy of `item` becomes `item`.
    fn remember_item(&mut self, item: &Item) {
        match self.items.iter_mut().find(|i| i.id == item.id) {
            Some(i) => *i = item.clone(),
            None => self.items.push(item.clone()),
        }
        self.want_cursor = Some(item.id);
        self.apply();
    }

    /// Draws the open dialog and runs the owner's choice.
    pub(crate) fn item_conflict_ui(&mut self, ctx: &eframe::egui::Context) {
        if let Some((open, choice)) = self.item_conflict.ui(ctx) {
            self.run_item_choice(open, choice);
        }
    }

    #[allow(clippy::needless_pass_by_value)] // the dialog hands over its choice
    pub(crate) fn run_item_choice(
        &mut self,
        open: OpenConflict<Versioned<Item>>,
        choice: ConflictChoice,
    ) {
        if !self.conflict_panel_matches() {
            return;
        }
        match choice {
            ConflictChoice::Back => self.warn = Some(CHANGED),
            ConflictChoice::UseTheirs => {
                let theirs = open.theirs.record;
                self.remember_item(&theirs);
                if let Some(p) = self.panel.as_mut() {
                    p.form.load(draft_of(&theirs));
                    p.error = None;
                    p.refusal = None;
                    p.opened = Some(theirs);
                }
                self.warn = None;
            }
            ConflictChoice::KeepMine(_) => {
                let Some(p) = self.panel.as_ref() else {
                    return;
                };
                let mut mine = p.form.current().clone();
                take_theirs(
                    &mut mine,
                    &draft_of(&open.theirs.record),
                    &fields_from_theirs(open.dialog.conflict()),
                );
                self.adopt_item(open.theirs.record, mine);
                self.resave_left = true;
                self.save_inner(open.intent);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_has_plain_words() {
        for k in item_value(&ItemDraft::new(None))
            .as_object()
            .expect("object")
            .keys()
        {
            assert!(crate::i18n::has(&format!("lan.field.{k}")), "{k}");
        }
    }

    #[test]
    fn keep_mine_takes_every_field_it_is_given() {
        let mine0 = ItemDraft::new(None);
        let mut theirs = ItemDraft::new(None);
        theirs.name = "T".into();
        theirs.part.cost = "2.00".into();
        theirs.taxable = false;
        let fields: Vec<String> = item_value(&theirs)
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect();
        let mut mine = mine0.clone();
        take_theirs(&mut mine, &theirs, &fields);
        assert_eq!(item_value(&mine), item_value(&theirs));
    }
}
