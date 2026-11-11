//! The report filters (phase step 1, Q-207, review B3): accounts, name,
//! tag, item, amount, cleared, transaction type and the adjusting lens,
//! in the Customize panel. A filter shows only on a report whose route
//! applies it (the catalog's `filters`, Q-P28ah) and, in the demo, only
//! when the demo applies it too, so no filter is ever shown and ignored.
//! The amount filter is the owner's typed amount (`Money::parse_user`),
//! sent as an exact decimal string; nothing here adds up money.

use cl_core::ids::{AccountId, ContactId, ItemId, TagId};
use cl_core::money::UserInput;
use cl_core::report::{Adjusting, Cleared};
use cl_core::Money;
use cl_model::reports::{AmountFilter, FilterKind, ReportOptions};
use eframe::egui::{self, RichText, Ui};

use super::window::ReportWindow;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;

/// One pick list: (id, caption).
type List<T> = Vec<(T, String)>;

/// The lists the filters choose from, loaded when a report that offers
/// them opens its Customize panel, and the amount filter's text.
#[derive(Default)]
pub(crate) struct FilterLists {
    accounts: Option<List<AccountId>>,
    accounts_job: Option<Job<List<AccountId>>>,
    contacts: Option<List<ContactId>>,
    contacts_job: Option<Job<List<ContactId>>>,
    tags: Option<List<TagId>>,
    tags_job: Option<Job<List<TagId>>>,
    items: Option<List<ItemId>>,
    items_job: Option<Job<List<ItemId>>>,
    /// The amount filter as typed.
    pub(crate) amount_min: String,
    pub(crate) amount_max: String,
    /// The typed amounts aren't amounts, or the first is the larger.
    pub(crate) amount_problem: bool,
}

impl FilterLists {
    pub(crate) fn busy(&self) -> bool {
        self.accounts_job.is_some()
            || self.contacts_job.is_some()
            || self.tags_job.is_some()
            || self.items_job.is_some()
    }

    /// The amount text follows the options of a report just opened.
    pub(crate) fn sync(&mut self, o: &ReportOptions) {
        let a = o.filters.amount.clone().unwrap_or_default();
        self.amount_min = a.min.as_deref().map(display_amount).unwrap_or_default();
        self.amount_max = a.max.as_deref().map(display_amount).unwrap_or_default();
        self.amount_problem = false;
    }
}

/// A wire amount as the owner reads it in the field (`1250.0000` →
/// `1250.00`).
fn display_amount(wire: &str) -> String {
    Money::parse_wire(wire).map_or_else(
        |_| wire.to_string(),
        |m| {
            let mut d = m.as_decimal();
            d.rescale(2);
            d.to_string()
        },
    )
}

/// True when the report window offers filter `k` on its report: the route
/// applies it, and the demo plays it when the company is the demo.
pub(crate) fn offered(w: &ReportWindow, k: FilterKind) -> bool {
    w.spec.filters.contains(&k) && (!w.link.is_demo() || demo_applies(k))
}

/// The filters the demo's ledger and statements apply (Q-P28ah): it keeps
/// no cleared state, adjusting flag or contact ids on its lines.
pub(crate) const fn demo_applies(k: FilterKind) -> bool {
    matches!(
        k,
        FilterKind::Accounts
            | FilterKind::Tags
            | FilterKind::Items
            | FilterKind::Amount
            | FilterKind::TxnTypes
    )
}

/// The filter kinds this panel draws (the pickers that must be chosen
/// before a report runs live in the filter bar).
const DRAWN: [FilterKind; 8] = [
    FilterKind::Accounts,
    FilterKind::Contacts,
    FilterKind::Tags,
    FilterKind::Items,
    FilterKind::Amount,
    FilterKind::Cleared,
    FilterKind::TxnTypes,
    FilterKind::Adjusting,
];

/// True when the report has any filter for this panel.
pub(crate) fn any(w: &ReportWindow) -> bool {
    DRAWN.iter().any(|k| offered(w, *k))
}

/// The transaction types the filter lists (the engine's names).
pub(crate) const TXN_TYPES: [&str; 14] = [
    "invoice",
    "credit_memo",
    "cash_sale",
    "customer_payment",
    "customer_refund",
    "bill",
    "vendor_credit",
    "bill_payment",
    "expense",
    "deposit",
    "transfer",
    "journal_entry",
    "inventory_adjustment",
    "sales_tax_payment",
];

/// Starts loading the lists the report's filters need.
pub(crate) fn load(w: &mut ReportWindow) {
    if offered(w, FilterKind::Accounts) && w.filter_lists.accounts.is_none() && w.filter_lists.accounts_job.is_none() {
        w.filter_lists.accounts_job = Some(engine::read(
            &w.link,
            w.group.as_ref(),
            |d| Ok(accounts_of(&d.chart.clone().unwrap_or_else(crate::company::demo::chart))),
            |s| async move {
                let all = s.accounts().list_all(&cl_api::AccountFilter::default()).await?;
                Ok(accounts_of(&all))
            },
        ));
    }
    if offered(w, FilterKind::Contacts) && w.filter_lists.contacts.is_none() && w.filter_lists.contacts_job.is_none() {
        w.filter_lists.contacts_job = Some(engine::read(
            &w.link,
            w.group.as_ref(),
            |_| Ok(Vec::new()),
            |s| async move {
                let f = cl_api::ContactFilter {
                    is_active: Some(true),
                    ..cl_api::ContactFilter::default()
                };
                let mut v: List<ContactId> = s
                    .contacts()
                    .list_all(&f)
                    .await?
                    .into_iter()
                    .map(|c| (c.id, c.fields.display_name))
                    .collect();
                v.sort_by_key(|(_, n)| n.to_lowercase());
                Ok(v)
            },
        ));
    }
    if offered(w, FilterKind::Tags) && w.filter_lists.tags.is_none() && w.filter_lists.tags_job.is_none() {
        w.filter_lists.tags_job = Some(engine::read(
            &w.link,
            w.group.as_ref(),
            |d| Ok(tags_of(d.tags.clone().unwrap_or_else(crate::company::demo::lists::tags))),
            |s| async move {
                let all = s.tags().list_all(&cl_api::TagFilter::default()).await?;
                Ok(tags_of(all))
            },
        ));
    }
    if offered(w, FilterKind::Items) && w.filter_lists.items.is_none() && w.filter_lists.items_job.is_none() {
        w.filter_lists.items_job = Some(engine::read(
            &w.link,
            w.group.as_ref(),
            |d| Ok(items_of(d.items.clone().unwrap_or_else(crate::company::demo::lists::items))),
            |s| async move {
                let all = s.items().list_all(&cl_api::ItemFilter::default()).await?;
                Ok(items_of(all))
            },
        ));
    }
}

fn accounts_of(all: &[cl_api::Account]) -> List<AccountId> {
    let mut v: Vec<(String, AccountId, String)> = all
        .iter()
        .filter(|a| a.is_active)
        .map(|a| {
            let caption = match a.fields.account_number.as_deref().filter(|n| !n.is_empty()) {
                Some(n) => format!("{n} \u{b7} {}", a.fields.name),
                None => a.fields.name.clone(),
            };
            (caption.to_lowercase(), a.id, caption)
        })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v.into_iter().map(|(_, id, c)| (id, c)).collect()
}

fn tags_of(all: Vec<cl_api::Tag>) -> List<TagId> {
    let mut v: List<TagId> = all
        .into_iter()
        .filter(|t| t.is_active)
        .map(|t| (t.id, t.fields.name))
        .collect();
    v.sort_by_key(|(_, n)| n.to_lowercase());
    v
}

fn items_of(all: Vec<cl_api::Item>) -> List<ItemId> {
    let mut v: List<ItemId> = all
        .into_iter()
        .filter(|i| i.is_active)
        .map(|i| (i.id, i.fields.name))
        .collect();
    v.sort_by_key(|(_, n)| n.to_lowercase());
    v
}

pub(crate) fn poll(w: &mut ReportWindow) {
    let f = &mut w.filter_lists;
    // Without a list its picker stays empty; the report still runs.
    if let Some(r) = engine::poll(&mut f.accounts_job) {
        f.accounts = Some(r.unwrap_or_default());
    }
    if let Some(r) = engine::poll(&mut f.contacts_job) {
        f.contacts = Some(r.unwrap_or_default());
    }
    if let Some(r) = engine::poll(&mut f.tags_job) {
        f.tags = Some(r.unwrap_or_default());
    }
    if let Some(r) = engine::poll(&mut f.items_job) {
        f.items = Some(r.unwrap_or_default());
    }
}

/// The typed amount filter, or `None` when it isn't usable: each side an
/// amount (or blank), the first not more than the second (the engine
/// refuses that, review B3).
pub(crate) fn amount_filter(min: &str, max: &str) -> Result<Option<AmountFilter>, ()> {
    let one = |s: &str| -> Result<Option<Money>, ()> {
        let s = s.trim();
        if s.is_empty() {
            return Ok(None);
        }
        Money::parse_user(s, UserInput::MONEY).map(|m| Some(m.abs())).map_err(|_| ())
    };
    let (lo, hi) = (one(min)?, one(max)?);
    if let (Some(a), Some(b)) = (lo, hi)
        && a > b
    {
        return Err(());
    }
    if lo.is_none() && hi.is_none() {
        return Ok(None);
    }
    Ok(Some(AmountFilter {
        min: lo.map(Money::to_wire),
        max: hi.map(Money::to_wire),
    }))
}

/// "All", one name, or "N chosen".
fn chosen_text<T: PartialEq>(list: &[(T, String)], chosen: &[T], all: &str) -> String {
    match chosen {
        [] => t(all),
        [one] => list
            .iter()
            .find(|(id, _)| id == one)
            .map_or_else(|| t_with("reports.filter.n_chosen", &[("n", "1")]), |(_, n)| n.clone()),
        many => t_with("reports.filter.n_chosen", &[("n", &many.len().to_string())]),
    }
}

/// A list where any number can be ticked (none: all).
fn multi<T: Copy + PartialEq>(
    ui: &mut Ui,
    salt: &str,
    label: &str,
    all: &str,
    list: &[(T, String)],
    chosen: &mut Vec<T>,
) {
    let l = ui.label(RichText::new(t(label)).size(12.0));
    egui::ComboBox::from_id_salt(("report_filter", salt))
        .selected_text(chosen_text(list, chosen, all))
        .height(320.0)
        .show_ui(ui, |ui| {
            if ui.selectable_label(chosen.is_empty(), t(all)).clicked() {
                chosen.clear();
            }
            for (id, name) in list {
                let mut on = chosen.contains(id);
                if ui.checkbox(&mut on, name).changed() {
                    if on {
                        chosen.push(*id);
                    } else {
                        chosen.retain(|c| c != id);
                    }
                }
            }
        })
        .response
        .labelled_by(l.id);
}

/// A list where one (or all) is chosen.
fn single<T: Copy + PartialEq>(
    ui: &mut Ui,
    salt: &str,
    label: &str,
    all: &str,
    list: &[(T, String)],
    chosen: &mut Vec<T>,
) {
    let l = ui.label(RichText::new(t(label)).size(12.0));
    egui::ComboBox::from_id_salt(("report_filter", salt))
        .selected_text(chosen_text(list, chosen, all))
        .height(320.0)
        .show_ui(ui, |ui| {
            if ui.selectable_label(chosen.is_empty(), t(all)).clicked() {
                chosen.clear();
            }
            for (id, name) in list {
                if ui.selectable_label(chosen.first() == Some(id), name).clicked() {
                    *chosen = vec![*id];
                }
            }
        })
        .response
        .labelled_by(l.id);
}

/// The Filters part of the Customize panel.
pub(super) fn ui(w: &mut ReportWindow, ui: &mut Ui, tokens: &Tokens, next: &mut ReportOptions) {
    if !any(w) {
        return;
    }
    load(w);
    ui.add_space(6.0);
    form::heading(ui, tokens, &t("reports.filter.title"));
    let lists = &w.filter_lists;
    ui.horizontal_wrapped(|ui| {
        if offered(w, FilterKind::Accounts) {
            let list = lists.accounts.clone().unwrap_or_default();
            multi(ui, "accounts", "reports.filter.accounts", "reports.filter.all_accounts", &list, &mut next.filters.account_ids);
        }
        if offered(w, FilterKind::Contacts) {
            // The ledger and the aging details take one name.
            let list = lists.contacts.clone().unwrap_or_default();
            single(ui, "contacts", "reports.filter.name", "reports.filter.all_names", &list, &mut next.filters.contact_ids);
        }
        if offered(w, FilterKind::Tags) {
            let list = lists.tags.clone().unwrap_or_default();
            multi(ui, "tags", "reports.filter.tags", "reports.filter.all_tags", &list, &mut next.filters.tag_ids);
        }
        if offered(w, FilterKind::Items) {
            // The inventory detail takes one item.
            let list = lists.items.clone().unwrap_or_default();
            single(ui, "items", "reports.filter.item", "reports.filter.all_items", &list, &mut next.filters.item_ids);
        }
    });
    ui.horizontal_wrapped(|ui| {
        if offered(w, FilterKind::TxnTypes) {
            let list: Vec<(&str, String)> = TXN_TYPES
                .iter()
                .map(|k| (*k, t(&format!("reports.txn.{k}"))))
                .collect();
            let mut chosen: Vec<&str> = next.filters.txn_types.iter().map(String::as_str).collect();
            let before = chosen.clone();
            multi(ui, "types", "reports.filter.types", "reports.filter.all_types", &list, &mut chosen);
            if chosen != before {
                next.filters.txn_types = chosen.into_iter().map(str::to_string).collect();
            }
        }
        if offered(w, FilterKind::Cleared) {
            let l = ui.label(RichText::new(t("reports.filter.cleared")).size(12.0));
            let text = |c: Option<Cleared>| {
                t(match c {
                    None => "reports.filter.cleared_any",
                    Some(Cleared::Cleared) => "reports.filter.cleared_cleared",
                    Some(Cleared::Uncleared) => "reports.filter.cleared_uncleared",
                    Some(Cleared::Reconciled) => "reports.filter.cleared_reconciled",
                })
            };
            egui::ComboBox::from_id_salt("report_filter_cleared")
                .selected_text(text(next.filters.cleared))
                .show_ui(ui, |ui| {
                    for c in [None, Some(Cleared::Cleared), Some(Cleared::Uncleared), Some(Cleared::Reconciled)] {
                        ui.selectable_value(&mut next.filters.cleared, c, text(c));
                    }
                })
                .response
                .labelled_by(l.id);
        }
        if offered(w, FilterKind::Adjusting) {
            let l = ui.label(RichText::new(t("reports.filter.adjusting")).size(12.0));
            let text = |a: Option<Adjusting>| {
                t(match a {
                    None | Some(Adjusting::Include) => "reports.filter.adjusting_include",
                    Some(Adjusting::Exclude) => "reports.filter.adjusting_exclude",
                    Some(Adjusting::Only) => "reports.filter.adjusting_only",
                })
            };
            egui::ComboBox::from_id_salt("report_filter_adjusting")
                .selected_text(text(next.filters.adjusting))
                .show_ui(ui, |ui| {
                    for a in [None, Some(Adjusting::Exclude), Some(Adjusting::Only)] {
                        ui.selectable_value(&mut next.filters.adjusting, a, text(a));
                    }
                })
                .response
                .labelled_by(l.id)
                .on_hover_text(t("reports.filter.adjusting.why"));
        }
    });
    if offered(w, FilterKind::Amount) {
        let f = &mut w.filter_lists;
        let mut left = false;
        ui.horizontal(|ui| {
            for (label, text) in [
                ("reports.filter.amount_min", &mut f.amount_min),
                ("reports.filter.amount_max", &mut f.amount_max),
            ] {
                let l = ui.label(RichText::new(t(label)).size(12.0));
                let r = ui
                    .add(egui::TextEdit::singleline(text).desired_width(90.0))
                    .labelled_by(l.id);
                left |= r.lost_focus();
            }
        });
        // Commits when the owner leaves a field, not on every key.
        if left {
            match amount_filter(&f.amount_min, &f.amount_max) {
                Ok(a) => {
                    f.amount_problem = false;
                    next.filters.amount = a;
                }
                Err(()) => f.amount_problem = true,
            }
        }
        if f.amount_problem {
            form::inline_warning(ui, tokens, &t("reports.filter.amount_bad"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_amount_filter_is_typed_amounts_smallest_first() {
        let a = amount_filter("1,250", "").expect("ok").expect("some");
        assert_eq!(a.min.as_deref(), Some("1250.0000"));
        assert_eq!(a.max, None);
        assert_eq!(amount_filter(" ", ""), Ok(None), "blank: no filter");
        assert!(amount_filter("abc", "").is_err());
        assert!(amount_filter("100", "50").is_err(), "the engine refuses min > max");
        let b = amount_filter("-20", "20").expect("ok").expect("some");
        assert_eq!(b.min.as_deref(), Some("20.0000"), "sizes, not signs");
        assert_eq!(display_amount("1250.0000"), "1250.00");
    }

    #[test]
    fn the_demo_hides_filters_it_does_not_apply() {
        assert!(demo_applies(FilterKind::Tags));
        assert!(!demo_applies(FilterKind::Cleared));
        assert!(!demo_applies(FilterKind::Adjusting));
        assert!(!demo_applies(FilterKind::Contacts));
    }
}
