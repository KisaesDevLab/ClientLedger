//! A formula's form (AC2): its caption, its terms with + and −, and "Add a
//! row", which offers earlier rows only; others are greyed with the
//! reason ("below this row", "adds up a different kind of thing").

use cl_model::report_def::editor::Op;
use cl_model::report_def::editor::choices::formula_terms;
use cl_model::report_def::validate::limits;
use cl_model::report_def::{Formula, Node, Term};
use eframe::egui::{self, RichText, Ui};

use super::picker::{self, Choice};
use super::{BuilderScreen, FieldKey};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;

/// The formula form.
pub(crate) fn draw(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens, f: &Formula) {
    picker::text(
        s,
        ui,
        FieldKey::Caption(f.id.clone()),
        &t("builder.row.caption"),
        &f.caption,
        270.0,
    );
    ui.add_space(6.0);
    ui.label(RichText::new(t("builder.formula.terms")).strong());
    let mut next: Option<Formula> = None;
    for (i, term) in f.terms.iter().enumerate() {
        ui.horizontal(|ui| {
            let caption = super::outline::caption_of(s, &term.node_id);
            let signs = [
                picker::on(t("builder.formula.plus")),
                picker::on(t("builder.formula.minus")),
            ];
            let at = usize::from(term.sign < 0);
            if let Some(k) = picker::segmented(
                ui,
                &t_with("builder.formula.sign_of", &[("row", &caption)]),
                &signs,
                Some(at),
            ) {
                let mut n = f.clone();
                n.terms[i].sign = if k == 1 { -1 } else { 1 };
                next = Some(n);
            }
            ui.label(&caption);
            ui.label(
                RichText::new(t("builder.formula.total"))
                    .size(12.0)
                    .color(tokens.text_tertiary),
            );
            let x = ui.small_button("\u{d7}");
            let name = t_with("builder.formula.remove", &[("row", &caption)]);
            x.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
            if x.on_hover_text(&name).clicked() {
                let mut n = f.clone();
                n.terms.remove(i);
                next = Some(n);
            }
        });
    }
    if f.terms.len() < limits::TERMS {
        let offered = formula_terms(s.ed.definition(), &f.id);
        let left: Vec<_> = offered
            .iter()
            .filter(|c| !f.terms.iter().any(|t| t.node_id == c.id))
            .collect();
        let mut choices: Vec<Choice> = left
            .iter()
            .map(|c| (super::outline::caption_of(s, &c.id), c.availability))
            .collect();
        choices.insert(0, picker::on(t("builder.formula.add")));
        if let Some(k) = picker::pick(
            ui,
            ("terms", &f.id),
            &t("builder.formula.add_label"),
            &choices,
            Some(0),
            260.0,
        ) && k > 0
            && let Some(c) = left.get(k - 1)
        {
            let mut n = f.clone();
            n.terms.push(Term {
                node_id: c.id.clone(),
                sign: 1,
            });
            next = Some(n);
        }
    }
    ui.add(
        egui::Label::new(
            RichText::new(t("builder.formula.hint"))
                .size(12.0)
                .color(tokens.text_tertiary),
        )
        .wrap(),
    );
    if let Some(n) = next {
        s.apply(Op::Edit(Node::Formula(n)));
    }
}
