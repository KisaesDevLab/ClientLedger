//! A standard memorized report whose saved filters name a deleted or
//! merged record (PR-A2, Q-PRA2c, U-226 `report-refs-v1`). The engine's
//! definition rows list them in `missing` for standard reports too
//! (`filters.accountIds`, `filters.contactIds`, `filters.tagIds`,
//! `filters.itemIds`, `tagSplit.tagGroupId`, `columnTagGroupId`,
//! `budgetId`). A standard report runs through the plain report routes,
//! which would match nothing and show "No activity", so the window holds
//! the run and says which filter uses the name until every one is settled:
//! "Use <name>" for a merge, "Remove it" or another pick in Customize.
//! Nothing is saved until the owner memorizes it (Q-310, Q-PRA2f).

use cl_api::resources::reports::definitions::MissingRef;
use cl_model::reports::{ColumnBy, ReportOptions};
use eframe::egui::Ui;

use super::window::ReportWindow;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;

fn same(a: &impl ToString, id: &str) -> bool {
    a.to_string().eq_ignore_ascii_case(id)
}

/// The ids the options hold at the entry's place, as text.
fn held(o: &ReportOptions, field: &str) -> Vec<String> {
    match field {
        "filters.accountIds" => o
            .filters
            .account_ids
            .iter()
            .map(ToString::to_string)
            .collect(),
        "filters.contactIds" => o
            .filters
            .contact_ids
            .iter()
            .map(ToString::to_string)
            .collect(),
        "filters.tagIds" => o.filters.tag_ids.iter().map(ToString::to_string).collect(),
        "filters.itemIds" => o.filters.item_ids.iter().map(ToString::to_string).collect(),
        "tagSplit.tagGroupId" => o
            .tag_split
            .as_ref()
            .map(|s| s.tag_group_id.to_string())
            .into_iter()
            .collect(),
        "columnTagGroupId" => o
            .column_tag_group_id
            .as_ref()
            .map(ToString::to_string)
            .into_iter()
            .collect(),
        "budgetId" => o.budget_id.clone().into_iter().collect(),
        _ => Vec::new(),
    }
}

/// The options still use a name of this entry.
pub(crate) fn uses(o: &ReportOptions, m: &MissingRef) -> bool {
    held(o, &m.field)
        .iter()
        .any(|h| m.ids.iter().any(|id| h.eq_ignore_ascii_case(id)))
}

/// The entries the options still use (none: the report runs).
pub(crate) fn pending(w: &ReportWindow) -> Vec<MissingRef> {
    w.std_missing
        .iter()
        .filter(|m| uses(&w.options, m))
        .cloned()
        .collect()
}

/// Takes the entry's names out of the options: out of a filter list, or
/// the tag split, tag group columns or budget unset (as the engine's own
/// drop rules for a report file, U-220).
pub(crate) fn remove(o: &mut ReportOptions, m: &MissingRef) {
    let gone = |s: &dyn ToString| m.ids.iter().any(|id| same(&s.to_string(), id));
    match m.field.as_str() {
        "filters.accountIds" => o.filters.account_ids.retain(|x| !gone(x)),
        "filters.contactIds" => o.filters.contact_ids.retain(|x| !gone(x)),
        "filters.tagIds" => o.filters.tag_ids.retain(|x| !gone(x)),
        "filters.itemIds" => o.filters.item_ids.retain(|x| !gone(x)),
        "tagSplit.tagGroupId" => {
            if o.tag_split.as_ref().is_some_and(|s| gone(&s.tag_group_id)) {
                o.tag_split = None;
            }
        }
        "columnTagGroupId" => {
            if o.column_tag_group_id.as_ref().is_some_and(|g| gone(g)) {
                o.column_tag_group_id = None;
                o.columns = ColumnBy::Total;
            }
        }
        "budgetId" => {
            if o.budget_id.as_ref().is_some_and(|b| gone(b)) {
                o.budget_id = None;
            }
        }
        _ => {}
    }
}

/// Puts `to` (the record a merged name went into) in place of the entry's
/// names; a list keeps one of each. False when `to` isn't an id of that
/// kind.
pub(crate) fn replace(o: &mut ReportOptions, m: &MissingRef, to: &str) -> bool {
    fn swap<T: Clone + PartialEq + ToString>(list: &mut Vec<T>, ids: &[String], to: T) {
        let mut out: Vec<T> = Vec::with_capacity(list.len());
        for x in list.drain(..) {
            let x = if ids.iter().any(|id| same(&x, id)) {
                to.clone()
            } else {
                x
            };
            if !out.contains(&x) {
                out.push(x);
            }
        }
        *list = out;
    }
    let ids = &m.ids;
    match m.field.as_str() {
        "filters.accountIds" => to.parse().map(|v| swap(&mut o.filters.account_ids, ids, v)),
        "filters.contactIds" => to.parse().map(|v| swap(&mut o.filters.contact_ids, ids, v)),
        "filters.tagIds" => to.parse().map(|v| swap(&mut o.filters.tag_ids, ids, v)),
        "filters.itemIds" => to.parse().map(|v| swap(&mut o.filters.item_ids, ids, v)),
        "tagSplit.tagGroupId" => to.parse().map(|v| {
            if let Some(s) = o.tag_split.as_mut() {
                s.tag_group_id = v;
            }
        }),
        "columnTagGroupId" => to.parse().map(|v| o.column_tag_group_id = Some(v)),
        "budgetId" => {
            o.budget_id = Some(to.to_string());
            Ok(())
        }
        _ => return false,
    }
    .is_ok()
}

/// A changed pick at an entry's place drops the entry's names from it: the
/// lists offer only live names, so a deleted one can't be unticked there.
pub(crate) fn settle(missing: &[MissingRef], old: &ReportOptions, next: &mut ReportOptions) {
    for m in missing {
        if held(old, &m.field) != held(next, &m.field) && uses(next, m) {
            remove(next, m);
        }
    }
}

/// The filter's name for the message.
fn field_name(field: &str) -> String {
    t(match field {
        "filters.accountIds" => "reports.missing.field.accounts",
        "filters.contactIds" => "reports.missing.field.contacts",
        "filters.tagIds" => "reports.missing.field.tags",
        "filters.itemIds" => "reports.missing.field.items",
        "tagSplit.tagGroupId" => "reports.missing.field.tag_split",
        "columnTagGroupId" => "reports.missing.field.tag_group_columns",
        "budgetId" => "reports.missing.field.budget",
        _ => "reports.missing.field.other",
    })
}

/// The lines under the message: each filter that uses a gone name, with
/// "Use <name>" for a merge and "Remove it" for any.
pub(crate) fn ui(w: &mut ReportWindow, ui: &mut Ui, tokens: &Tokens) {
    if w.builder.is_some() {
        return;
    }
    let list = pending(w);
    if list.is_empty() {
        return;
    }
    enum Click {
        Use(usize, String),
        Remove(usize),
    }
    let mut click = None;
    for (i, m) in list.iter().enumerate() {
        let filter = field_name(&m.field);
        ui.horizontal_wrapped(|ui| {
            if let Some(into) = m.merged() {
                ui.label(t_with(
                    "reports.missing.merged_in",
                    &[("filter", &filter), ("name", &into.label)],
                ));
                if ui
                    .button(t_with(
                        "reports.builder.use_merged",
                        &[("name", &into.label)],
                    ))
                    .clicked()
                {
                    click = Some(Click::Use(i, into.id.clone()));
                }
            } else {
                ui.label(t_with("reports.missing.deleted_in", &[("filter", &filter)]));
            }
            if ui
                .button(t_with("reports.missing.remove", &[("filter", &filter)]))
                .clicked()
            {
                click = Some(Click::Remove(i));
            }
        });
    }
    form::inline_note(ui, tokens, &t("reports.missing.fix"));
    let mut next = w.options.clone();
    match click {
        Some(Click::Use(i, to)) => {
            if !replace(&mut next, &list[i], &to) {
                remove(&mut next, &list[i]);
            }
        }
        Some(Click::Remove(i)) => remove(&mut next, &list[i]),
        None => return,
    }
    w.set_options(next);
}
