//! The Columns tab (AC3): columns across, aging columns, compare (up to
//! two: the compared amount, $ change, % change), "% of", transaction
//! columns (pick and order, at most 24) and renamed headers (keyed by the
//! last good preview's columns, Q-A08v).

use cl_model::report_def::editor::Op;
use cl_model::report_def::editor::choices::column_rules;
use cl_model::report_def::fields::{self, Availability};
use cl_model::report_def::validate::limits;
use cl_model::report_def::{CompareBase, CompareSet, Node, Periods, outline as tree};
use eframe::egui::{self, RichText, Ui};

use super::picker::{self, Choice};
use super::{BuilderScreen, FieldKey};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;

fn greyed(a: Availability) -> Option<&'static str> {
    match a {
        Availability::Greyed(k) => Some(k),
        Availability::Available => None,
    }
}

/// The tab.
pub(crate) fn draw(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    let def = s.ed.definition().clone();
    let c = def.columns.clone();
    let rules = column_rules(&def);
    // Columns across.
    let periods = [
        None,
        Some(Periods::Month),
        Some(Periods::Quarter),
        Some(Periods::Year),
    ];
    let choices: Vec<Choice> = ["total", "month", "quarter", "year"]
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let a = if i == 0 {
                Availability::Available
            } else {
                rules.periods
            };
            (t(&format!("builder.columns.across.{k}")), a)
        })
        .collect();
    picker::caption(ui, &t("builder.columns.across"));
    let at = periods.iter().position(|p| *p == c.periods);
    if let Some(i) = picker::segmented(ui, &t("builder.columns.across"), &choices, at) {
        let mut n = c.clone();
        n.periods = periods[i];
        s.apply(Op::Columns(n));
        return;
    }
    ui.add_space(6.0);
    // Aging columns.
    let aging_on = c.aging_buckets == Some(true);
    ui.add_enabled_ui(aging_on || rules.aging.is_available(), |ui| {
        let r = ui.checkbox(&mut aging_on.clone(), t("builder.columns.aging"));
        let r = match greyed(rules.aging) {
            Some(k) if !aging_on => r.on_disabled_hover_text(t(k)),
            _ => r,
        };
        if r.clicked() {
            let mut n = c.clone();
            n.aging_buckets = (!aging_on).then_some(true);
            s.apply(Op::Columns(n));
        }
    });
    ui.add_space(6.0);
    compare(s, ui, &c, rules.compare);
    ui.add_space(6.0);
    percent_of(s, ui, &def, rules.percent_of);
    ui.add_space(6.0);
    detail_fields(s, ui, tokens, &def, rules.running_balance);
    ui.add_space(6.0);
    headers(s, ui, tokens);
}

fn compare(
    s: &mut BuilderScreen,
    ui: &mut Ui,
    c: &cl_model::report_def::Columns,
    rule: Availability,
) {
    ui.label(RichText::new(t("builder.columns.compare")).strong());
    for (i, set) in c.compare.iter().enumerate() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let bases = [CompareBase::PriorPeriod, CompareBase::PriorYear];
            let choices: Vec<Choice> = ["prior_period", "prior_year"]
                .iter()
                .map(|k| picker::on(t(&format!("builder.columns.base.{k}"))))
                .collect();
            let at = bases.iter().position(|b| *b == set.base);
            let mut n = c.clone();
            let mut changed = false;
            if let Some(k) = picker::pick(
                ui,
                ("cmp", s.id.0, i),
                &t("builder.columns.compare_with"),
                &choices,
                at,
                200.0,
            ) {
                n.compare[i].base = bases[k];
                changed = true;
            }
            ui.horizontal_wrapped(|ui| {
                for (key, value) in [
                    ("builder.columns.amount", set.amount),
                    ("builder.columns.dollar_change", set.dollar_change),
                    ("builder.columns.percent_change", set.percent_change),
                ] {
                    if let Some(v) = picker::check(ui, value, &t(key)) {
                        let x = &mut n.compare[i];
                        match key {
                            "builder.columns.amount" => x.amount = v,
                            "builder.columns.dollar_change" => x.dollar_change = v,
                            _ => x.percent_change = v,
                        }
                        changed = true;
                    }
                }
                let r = ui.small_button("\u{d7}");
                let name = t("builder.columns.remove_compare");
                r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
                if r.on_hover_text(&name).clicked() {
                    n.compare.remove(i);
                    changed = true;
                }
            });
            if changed {
                s.apply(Op::Columns(n));
            }
        });
    }
    if c.compare.len() < limits::COMPARE {
        let on = rule.is_available();
        // A stable id: the button keeps the keyboard when a compare is
        // added above it (AC6).
        let r = ui
            .push_id(("compare-add", s.id.0), |ui| {
                ui.add_enabled(on, egui::Button::new(t("builder.columns.add_compare")))
            })
            .inner;
        let r = match greyed(rule) {
            Some(k) => r.on_disabled_hover_text(t(k)),
            None => r,
        };
        if r.clicked() {
            let mut n = c.clone();
            // The new comparison's "Compare with" takes the keyboard.
            picker::want_focus(ui, ("cmp", s.id.0, n.compare.len()));
            n.compare.push(CompareSet {
                base: CompareBase::PriorPeriod,
                amount: true,
                dollar_change: true,
                percent_change: false,
            });
            s.apply(Op::Columns(n));
        }
    }
}

fn percent_of(
    s: &mut BuilderScreen,
    ui: &mut Ui,
    def: &cl_model::report_def::BuilderDefinition,
    rule: Availability,
) {
    let units = cl_model::report_def::node_units(&def.nodes);
    let money: Vec<String> = tree::walk(&def.nodes)
        .iter()
        .filter(|v| {
            matches!(v.node, Node::Section(_) | Node::Group(_) | Node::Formula(_))
                && units.get(v.node.id())
                    == Some(&cl_model::report_def::NodeUnit::One(
                        cl_model::report_def::Unit::Money,
                    ))
        })
        .map(|v| v.node.id().to_string())
        .collect();
    let a = |x: Availability| if rule.is_available() { x } else { rule };
    let mut choices: Vec<Choice> = vec![
        picker::on(t("builder.columns.percent.none")),
        (
            t("builder.columns.percent.section"),
            a(Availability::Available),
        ),
    ];
    choices.extend(money.iter().map(|id| {
        (
            super::outline::caption_of(s, id),
            a(Availability::Available),
        )
    }));
    let at = match def.columns.percent_of.as_deref() {
        None => Some(0),
        Some("section") => Some(1),
        Some(id) => money.iter().position(|m| m == id).map(|p| p + 2),
    };
    if let Some(i) = picker::pick(
        ui,
        ("pct", s.id.0),
        &t("builder.columns.percent"),
        &choices,
        at,
        220.0,
    ) {
        let mut n = def.columns.clone();
        n.percent_of = match i {
            0 => None,
            1 => Some("section".into()),
            k => money.get(k - 2).cloned(),
        };
        s.apply(Op::Columns(n));
    }
}

fn detail_fields(
    s: &mut BuilderScreen,
    ui: &mut Ui,
    tokens: &Tokens,
    def: &cl_model::report_def::BuilderDefinition,
    running: Availability,
) {
    ui.label(RichText::new(t("builder.columns.detail")).strong());
    ui.label(
        RichText::new(t_with(
            "builder.columns.detail_count",
            &[
                ("n", &def.columns.detail_fields.len().to_string()),
                ("max", &limits::DETAIL_FIELDS.to_string()),
            ],
        ))
        .size(11.0)
        .color(tokens.text_secondary),
    );
    let chosen = &def.columns.detail_fields;
    let full = chosen.len() >= limits::DETAIL_FIELDS;
    let mut next = None;
    // The chosen ones, in order, with ↑ ↓.
    for (i, f) in chosen.iter().enumerate() {
        ui.horizontal(|ui| {
            let mut on = true;
            let name = t(&format!("reports.builder.field.{f}"));
            if ui.checkbox(&mut on, &name).changed() {
                let mut n = def.columns.clone();
                n.detail_fields.remove(i);
                next = Some(n);
            }
            for (up, label, key) in [
                (true, "\u{2191}", "builder.columns.field_up"),
                (false, "\u{2193}", "builder.columns.field_down"),
            ] {
                let can = if up { i > 0 } else { i + 1 < chosen.len() };
                let r = ui.add_enabled(can, egui::Button::new(label).small());
                let n2 = t_with(key, &[("field", &name)]);
                r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, can, &n2));
                if r.clicked() {
                    let mut n = def.columns.clone();
                    let j = if up { i - 1 } else { i + 1 };
                    n.detail_fields.swap(i, j);
                    next = Some(n);
                }
            }
        });
    }
    // The rest, off.
    for f in fields::DETAIL_FIELDS {
        if chosen.iter().any(|c| c == f) {
            continue;
        }
        let why = if full {
            Some("builder.greyed.fields_full")
        } else if f == "running_balance" {
            greyed(running)
        } else {
            None
        };
        let mut on = false;
        let name = t(&format!("reports.builder.field.{f}"));
        let r = ui.add_enabled(why.is_none(), egui::Checkbox::new(&mut on, &name));
        let r = match why {
            Some(k) => r.on_disabled_hover_text(t(k)),
            None => r,
        };
        if r.changed() {
            let mut n = def.columns.clone();
            n.detail_fields.push(f.to_string());
            next = Some(n);
        }
    }
    if let Some(n) = next {
        s.apply(Op::Columns(n));
    }
}

/// Rename headers: the last good preview's columns (Q-A08v).
fn headers(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    ui.label(RichText::new(t("builder.columns.headers")).strong());
    let Some(cols) = s.preview.table.as_ref().map(|t| t.columns.clone()) else {
        ui.label(
            RichText::new(t("builder.columns.headers_wait"))
                .size(11.0)
                .color(tokens.text_secondary),
        );
        return;
    };
    for c in cols.iter().take(limits::HEADERS) {
        let shown = super::super::label(&c.caption);
        let current =
            s.ed.definition()
                .columns
                .headers
                .get(&c.key)
                .cloned()
                .unwrap_or_default();
        ui.horizontal(|ui| {
            let l = ui.label(RichText::new(&shown).size(12.0));
            picker::text_bare(
                s,
                ui,
                FieldKey::Header(c.key.clone()),
                &current,
                150.0,
                Some(l.id),
            );
        });
    }
}
