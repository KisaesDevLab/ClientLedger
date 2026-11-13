//! A section's form (plan §3.8, AC2): caption, what to report on, what it
//! adds up, the filters ("Include only"), up to three groupings, how it
//! shows, its total and which way amounts are positive. Choices that
//! don't apply are greyed with the reason.
//!
//! The one money input is the amount filter: typed amounts are read with
//! the app's money rules and kept as the exact digits typed (plan §4).

use cl_core::report::Basis;
use cl_model::report_def::editor::Op;
use cl_model::report_def::editor::choices::{filter_ops, group_by_choices};
use cl_model::report_def::fields::{self, Availability, UNAVAILABLE_SOURCES};
use cl_model::report_def::validate::limits;
use cl_model::report_def::{
    Display, Filter, FilterOp, GroupBy, Measure, Node, Polarity, Section, SourceKey,
};
use eframe::egui::{self, RichText, Ui};

use super::lists::{self, Which};
use super::picker::{self, Choice};
use super::{BuilderScreen, FieldKey};
use crate::i18n::t;
use crate::theme::Tokens;

const W: f32 = 270.0;

/// Every measure, in menu order.
const MEASURES: [Measure; 9] = [
    Measure::Amount,
    Measure::Debit,
    Measure::Credit,
    Measure::Balance,
    Measure::TbNet,
    Measure::Count,
    Measure::Quantity,
    Measure::OpenBalance,
    Measure::OriginalAmount,
];

fn measure_key(m: Measure) -> &'static str {
    match m {
        Measure::Amount => "builder.measure.amount",
        Measure::Debit => "builder.measure.debit",
        Measure::Credit => "builder.measure.credit",
        Measure::Balance => "builder.measure.balance",
        Measure::TbNet => "builder.measure.tb_net",
        Measure::Count => "builder.measure.count",
        Measure::Quantity => "builder.measure.quantity",
        Measure::OpenBalance => "builder.measure.open_balance",
        Measure::OriginalAmount => "builder.measure.original_amount",
    }
}

fn op_key(op: FilterOp) -> &'static str {
    match op {
        FilterOp::In => "builder.filter.op.in",
        FilterOp::NotIn => "builder.filter.op.not_in",
        FilterOp::IsNone => "builder.filter.op.is_none",
        FilterOp::Gte => "builder.filter.op.gte",
        FilterOp::Lte => "builder.filter.op.lte",
    }
}

/// A value's words (account types, transaction types, ...).
pub(crate) fn value_text(field: &str, v: &str) -> String {
    if field == "txn_type" {
        let key = cl_api::resources::reports::adapters::ledger::type_label(v);
        return super::super::label(&key);
    }
    t(&format!("builder.value.{v}"))
}

fn edit(s: &mut BuilderScreen, sec: Section) {
    s.apply(Op::Edit(Node::Section(sec)));
}

/// The section form.
pub(crate) fn section(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens, sec: &Section) {
    picker::text(
        s,
        ui,
        FieldKey::Caption(sec.id.clone()),
        &t("builder.row.caption"),
        &sec.caption,
        W,
    );
    ui.add_space(4.0);
    // What to report on: the sources, then the greyed ones.
    let mut choices: Vec<Choice> = SourceKey::ALL
        .iter()
        .map(|k| picker::on(t(&format!("builder.source.{}", k.as_str()))))
        .collect();
    choices.extend(
        UNAVAILABLE_SOURCES
            .iter()
            .map(|(k, why)| (t(&format!("builder.source.{k}")), Availability::Greyed(why))),
    );
    let at = SourceKey::ALL.iter().position(|k| *k == sec.source);
    if let Some(i) = picker::pick(
        ui,
        ("source", &sec.id),
        &t("builder.row.source"),
        &choices,
        at,
        W,
    ) && let Some(src) = SourceKey::ALL.get(i).copied()
    {
        let mut n = sec.clone();
        n.source = src;
        let info = fields::of(src);
        if !info.measures.contains(&n.measure) {
            n.measure = cl_model::report_def::guided::default_measure(src);
        }
        if info.open_items {
            n.polarity = None;
        }
        n.group_by
            .retain(|g| info.group_bys.contains(&g.key.as_str()));
        if n.group_by.is_empty() {
            n.group_by.push(GroupBy {
                key: cl_model::report_def::guided::default_group(src).into(),
                tag_group_id: None,
            });
        }
        n.filters
            .retain(|f| info.filters.contains(&f.field.as_str()));
        edit(s, n);
        return;
    }
    // Add up.
    let basis: Option<Basis> = s.ed.definition().basis;
    let m_choices: Vec<Choice> = MEASURES
        .iter()
        .map(|m| {
            (
                t(measure_key(*m)),
                fields::measure_availability(sec.source, *m, basis),
            )
        })
        .collect();
    let at = MEASURES.iter().position(|m| *m == sec.measure);
    if let Some(i) = picker::pick(
        ui,
        ("measure", &sec.id),
        &t("builder.row.add_up"),
        &m_choices,
        at,
        W,
    ) {
        let mut n = sec.clone();
        n.measure = MEASURES[i];
        edit(s, n);
        return;
    }
    ui.add_space(6.0);
    filters(s, ui, tokens, sec);
    ui.add_space(6.0);
    group_bys(s, ui, tokens, sec);
    ui.add_space(6.0);
    show(s, ui, sec);
    ui.add_space(6.0);
    total(s, ui, sec);
}

fn filters(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens, sec: &Section) {
    ui.label(RichText::new(t("builder.row.include_only")).strong());
    let offered = fields::of(sec.source).filters;
    for (i, f) in sec.filters.iter().enumerate() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(W);
            if filter_row(s, ui, sec, i, f, offered) {
                // A change was made; the form redraws next frame.
            }
        });
    }
    let full = sec.filters.len() >= limits::FILTERS;
    // A stable id: the button keeps the keyboard when a filter is added
    // above it (AC6).
    let r = ui
        .push_id(("filter-add", &sec.id), |ui| {
            ui.add_enabled(!full, egui::Button::new(t("builder.filter.add")))
        })
        .inner;
    let r = r.on_disabled_hover_text(t("builder.filter.full"));
    if r.clicked()
        && let Some(field) = offered.first()
    {
        let mut n = sec.clone();
        n.filters.push(new_filter(field));
        // The new filter's "Filter on" takes the keyboard.
        picker::want_focus(ui, ("ffield", &sec.id, n.filters.len() - 1));
        edit(s, n);
    }
    let names: Vec<String> = offered
        .iter()
        .map(|f| t(&format!("builder.filter.field.{f}")))
        .collect();
    ui.label(
        RichText::new(format!(
            "{}: {}",
            t("builder.filter.offered"),
            names.join(", ")
        ))
        .size(11.0)
        .color(tokens.text_secondary),
    );
}

fn new_filter(field: &str) -> Filter {
    let op = filter_ops(field)[0];
    Filter {
        field: field.to_string(),
        op,
        ids: fields::ID_FILTER_FIELDS.contains(&field).then(Vec::new),
        values: fields::filter_values(field).map(|_| Vec::new()),
        value: fields::SCALAR_FILTER_FIELDS
            .contains(&field)
            .then(|| "0".to_string()),
    }
}

#[allow(
    clippy::many_single_char_names,
    reason = "s, ui, i, f and k are the editor's usual short names"
)]
fn filter_row(
    s: &mut BuilderScreen,
    ui: &mut Ui,
    sec: &Section,
    i: usize,
    f: &Filter,
    offered: &[&str],
) -> bool {
    let mut changed = None;
    ui.horizontal_wrapped(|ui| {
        let choices: Vec<Choice> = offered
            .iter()
            .map(|x| picker::on(t(&format!("builder.filter.field.{x}"))))
            .collect();
        let at = offered.iter().position(|x| *x == f.field);
        if let Some(k) = picker::pick(
            ui,
            ("ffield", &sec.id, i),
            &t("builder.filter.field"),
            &choices,
            at,
            110.0,
        ) {
            changed = Some(new_filter(offered[k]));
        }
        let ops = filter_ops(&f.field);
        let op_choices: Vec<Choice> = ops.iter().map(|o| picker::on(t(op_key(*o)))).collect();
        let at = ops.iter().position(|o| *o == f.op);
        if let Some(k) = picker::pick(
            ui,
            ("fop", &sec.id, i),
            &t("builder.filter.how"),
            &op_choices,
            at,
            90.0,
        ) {
            let mut nf = f.clone();
            nf.op = ops[k];
            if nf.op == FilterOp::IsNone {
                nf.ids = None;
                nf.values = None;
            } else if nf.ids.is_none() && fields::ID_FILTER_FIELDS.contains(&nf.field.as_str()) {
                nf.ids = Some(Vec::new());
            }
            changed = Some(nf);
        }
        let x = ui.small_button("\u{d7}");
        let name = t("builder.filter.remove");
        x.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
        if x.on_hover_text(&name).clicked() {
            let mut n = sec.clone();
            n.filters.remove(i);
            edit(s, n);
        }
    });
    if let Some(nf) = changed {
        let mut n = sec.clone();
        n.filters[i] = nf;
        edit(s, n);
        return true;
    }
    // The values.
    if f.op == FilterOp::IsNone {
        return false;
    }
    if fields::SCALAR_FILTER_FIELDS.contains(&f.field.as_str()) {
        let current = f.value.clone().unwrap_or_default();
        picker::text(
            s,
            ui,
            FieldKey::Amount(sec.id.clone(), i),
            &t("builder.filter.amount"),
            &current,
            120.0,
        );
        return false;
    }
    if let Some(which) = Which::for_field(&f.field) {
        lists::ensure(s, which);
        let ids = f.ids.clone().unwrap_or_default();
        chips(
            s,
            ui,
            sec,
            i,
            &ids,
            |s, id| lists::caption_of(s, which, id),
            true,
        );
        let list = s.lists.get(which).cloned().unwrap_or_default();
        if ids.len() >= limits::IDS {
            crate::screens::company::form::inline_note(
                ui,
                &Tokens::of(ui.ctx()),
                &crate::i18n::t_with(
                    "builder.filter.ids_full",
                    &[
                        ("n", &ids.len().to_string()),
                        ("max", &limits::IDS.to_string()),
                    ],
                ),
            );
        } else {
            let left: Vec<&(String, String)> =
                list.iter().filter(|(x, _)| !ids.contains(x)).collect();
            let choices: Vec<Choice> = left.iter().map(|(_, n)| picker::on(n.clone())).collect();
            if let Some(k) = picker::pick_add(
                ui,
                ("fids", &sec.id, i),
                &t("builder.filter.names"),
                &t("builder.filter.add_name"),
                &choices,
                W - 20.0,
            ) && let Some((id, _)) = left.get(k)
            {
                let mut n = sec.clone();
                n.filters[i]
                    .ids
                    .get_or_insert_with(Vec::new)
                    .push(id.clone());
                edit(s, n);
                return true;
            }
        }
        return false;
    }
    if let Some(allowed) = fields::filter_values(&f.field) {
        let values = f.values.clone().unwrap_or_default();
        let field = f.field.clone();
        chips(
            s,
            ui,
            sec,
            i,
            &values,
            move |_, v| value_text(&field, v),
            false,
        );
        let left: Vec<&&str> = allowed
            .iter()
            .filter(|v| !values.iter().any(|x| x == **v))
            .collect();
        let choices: Vec<Choice> = left
            .iter()
            .map(|v| picker::on(value_text(&f.field, v)))
            .collect();
        if let Some(k) = picker::pick_add(
            ui,
            ("fvals", &sec.id, i),
            &t("builder.filter.values"),
            &t("builder.filter.add_value"),
            &choices,
            W - 20.0,
        ) && let Some(v) = left.get(k)
        {
            let mut n = sec.clone();
            n.filters[i]
                .values
                .get_or_insert_with(Vec::new)
                .push((**v).to_string());
            edit(s, n);
            return true;
        }
    }
    false
}

/// Value chips, each with ✕.
fn chips(
    s: &mut BuilderScreen,
    ui: &mut Ui,
    sec: &Section,
    i: usize,
    items: &[String],
    text: impl Fn(&BuilderScreen, &str) -> String,
    ids: bool,
) {
    let mut gone = None;
    ui.horizontal_wrapped(|ui| {
        for (k, v) in items.iter().enumerate() {
            let shown = text(s, v);
            let r = ui.button(format!("{shown} \u{d7}"));
            let name = crate::i18n::t_with("builder.filter.remove_value", &[("value", &shown)]);
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
            if r.clicked() {
                gone = Some(k);
            }
        }
    });
    if let Some(k) = gone {
        let mut n = sec.clone();
        let target = if ids {
            n.filters[i].ids.as_mut()
        } else {
            n.filters[i].values.as_mut()
        };
        if let Some(v) = target {
            v.remove(k);
        }
        edit(s, n);
    }
}

fn group_bys(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens, sec: &Section) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(t("builder.row.group_by")).strong());
        ui.label(
            RichText::new(t("builder.row.up_to_3"))
                .size(12.0)
                .color(tokens.text_secondary),
        );
    });
    let all = group_by_choices(sec.source);
    for slot in 0..limits::GROUP_BYS {
        if slot > sec.group_by.len() {
            break;
        }
        let used: Vec<&str> = sec
            .group_by
            .iter()
            .enumerate()
            .filter(|(k, _)| *k != slot)
            .map(|(_, g)| g.key.as_str())
            .collect();
        let mut choices: Vec<Choice> = vec![picker::on(t(if slot == 0 {
            "builder.group.none_first"
        } else {
            "builder.group.then_optional"
        }))];
        choices.extend(all.iter().map(|(k, a)| {
            let a = if used.contains(k) {
                Availability::Greyed("builder.greyed.used")
            } else {
                *a
            };
            (t(&format!("builder.group.{k}")), a)
        }));
        let at = sec
            .group_by
            .get(slot)
            .and_then(|g| all.iter().position(|(k, _)| *k == g.key))
            .map_or(0, |p| p + 1);
        let label = if slot == 0 {
            t("builder.row.group_by")
        } else {
            t("builder.row.then_by")
        };
        if let Some(k) = picker::pick(ui, ("gb", &sec.id, slot), &label, &choices, Some(at), 200.0)
        {
            let mut n = sec.clone();
            if k == 0 {
                n.group_by.truncate(slot);
            } else {
                let key = all[k - 1].0.to_string();
                let gb = GroupBy {
                    tag_group_id: None,
                    key,
                };
                if slot < n.group_by.len() {
                    n.group_by[slot] = gb;
                } else {
                    n.group_by.push(gb);
                }
            }
            edit(s, n);
            return;
        }
        // A tag grouping names its tag group.
        if let Some(g) = sec.group_by.get(slot)
            && g.key == "tag"
        {
            lists::ensure(s, Which::TagGroups);
            let groups = s.lists.get(Which::TagGroups).cloned().unwrap_or_default();
            let choices: Vec<Choice> = groups.iter().map(|(_, n)| picker::on(n.clone())).collect();
            let at = g
                .tag_group_id
                .as_ref()
                .and_then(|id| groups.iter().position(|(x, _)| x == id));
            if let Some(k) = picker::pick(
                ui,
                ("gbtag", &sec.id, slot),
                &t("builder.row.tag_group"),
                &choices,
                at,
                200.0,
            ) {
                let mut n = sec.clone();
                n.group_by[slot].tag_group_id = Some(groups[k].0.clone());
                edit(s, n);
                return;
            }
        }
    }
}

fn show(s: &mut BuilderScreen, ui: &mut Ui, sec: &Section) {
    ui.label(RichText::new(t("builder.row.show")).strong());
    let displays = [Display::Summary, Display::Detail, Display::Expandable];
    let choices: Vec<Choice> = ["summary", "detail", "expandable"]
        .iter()
        .map(|k| picker::on(t(&format!("builder.display.{k}"))))
        .collect();
    let at = displays.iter().position(|d| *d == sec.display);
    if let Some(i) = picker::segmented(ui, &t("builder.row.show"), &choices, at) {
        let mut n = sec.clone();
        n.display = displays[i];
        edit(s, n);
        return;
    }
    if sec.display == Display::Expandable
        && let Some(v) = picker::check(ui, sec.default_expanded, &t("builder.row.start_open"))
    {
        let mut n = sec.clone();
        n.default_expanded = v;
        edit(s, n);
        return;
    }
    if let Some(v) = picker::check(ui, sec.hide_zero, &t("builder.row.hide_zero")) {
        let mut n = sec.clone();
        n.hide_zero = v;
        edit(s, n);
    }
}

fn total(s: &mut BuilderScreen, ui: &mut Ui, sec: &Section) {
    ui.label(RichText::new(t("builder.row.total")).strong());
    if let Some(v) = picker::check(ui, sec.show_total, &t("builder.row.show_total")) {
        let mut n = sec.clone();
        n.show_total = v;
        edit(s, n);
        return;
    }
    if sec.show_total {
        picker::text(
            s,
            ui,
            FieldKey::TotalCaption(sec.id.clone()),
            &t("builder.row.total_caption"),
            sec.total_caption.as_deref().unwrap_or_default(),
            W,
        );
    }
    // Which way amounts are positive (open items: owed is positive).
    let open = fields::of(sec.source).open_items;
    let choices: Vec<Choice> = ["natural", "debit", "credit"]
        .iter()
        .map(|k| {
            let a = if open {
                Availability::Greyed("builder.greyed.polarity_open")
            } else {
                Availability::Available
            };
            (t(&format!("builder.polarity.{k}")), a)
        })
        .collect();
    let at = match sec.polarity {
        None | Some(Polarity::Natural) => 0,
        Some(Polarity::Debit) => 1,
        Some(Polarity::Credit) => 2,
    };
    let l = ui.label(RichText::new(t("builder.row.positive")).underline());
    l.on_hover_text(t("builder.row.positive_tip"));
    if let Some(i) = picker::pick(
        ui,
        ("polarity", &sec.id),
        &t("builder.row.positive"),
        &choices,
        (!open).then_some(at),
        W,
    ) {
        let mut n = sec.clone();
        n.polarity = match i {
            1 => Some(Polarity::Debit),
            2 => Some(Polarity::Credit),
            _ => None,
        };
        let id = n.id.clone();
        edit(s, n);
        sign_status(s, &id);
    }
}

/// After "Show as positive" changed on section `id`: formulas add their
/// rows as shown (the engine's rule, Q-A08ai), so a formula that now adds
/// it with the other sign is named in the status line (QA-12).
pub(crate) fn sign_status(s: &mut BuilderScreen, id: &str) {
    let def = s.ed.definition();
    let hit = cl_model::report_def::outline::walk(&def.nodes)
        .into_iter()
        .filter_map(|v| match v.node {
            Node::Formula(f) => Some(f),
            _ => None,
        })
        .find(|f| {
            cl_model::report_def::editor::choices::flipped_terms(def, &f.id)
                .iter()
                .any(|x| x == id)
        })
        .map(|f| f.caption.clone());
    if let Some(formula) = hit {
        let section = super::outline::caption_of(s, id);
        s.status = Some(crate::i18n::t_with(
            "builder.status.formula_sign",
            &[("formula", &formula), ("section", &section)],
        ));
    }
}
