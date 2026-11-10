//! The toolbar and filter bar (mockup): Customize, Memorize, Print,
//! Export Excel, Export CSV, Save PDF, Collapse, Refresh; Dates, From/To
//! (or As of), Columns, the basis toggle and the extra columns. Choices the
//! engine can't do yet are greyed with "Needs an engine update" (Q-220).
//! Email is hidden until U-207 (Q-P28i).

use cl_core::report::{Basis, Extra};
use cl_model::reports::catalog::{column_capability, extra_capability};
use cl_model::reports::{BasisMode, DateChoice, DateMode, DatePreset};
use eframe::egui::{self, RichText, Ui};

use super::window::ReportWindow;
use crate::i18n::t;
use crate::screens::company::form::{self, DateInput};
use crate::theme::Tokens;

/// The From/To (or As of) inputs.
#[derive(Debug)]
pub(crate) struct DateInputs {
    pub(crate) from: DateInput,
    pub(crate) to: DateInput,
}

impl DateInputs {
    pub(crate) fn new(today: cl_core::CalendarDate) -> DateInputs {
        DateInputs {
            from: DateInput::new(today),
            to: DateInput::new(today),
        }
    }
}

fn has(w: &ReportWindow, cap: &str) -> bool {
    w.link.is_demo() || w.link.capabilities().has(cap)
}

pub(super) fn toolbar(w: &mut ReportWindow, ui: &mut Ui, _tokens: &Tokens) {
    enum Click {
        Customize,
        Memorize,
        Print,
        Xlsx,
        Csv,
        Pdf,
        Collapse,
        Refresh,
    }
    let mut click = None;
    let shown = w.table.is_some();
    ui.horizontal_wrapped(|ui| {
        if ui.button(t("reports.tool.customize")).clicked() {
            click = Some(Click::Customize);
        }
        if super::memorized::available(&w.link)
            && ui
                .button(t("reports.tool.memorize"))
                .on_hover_text(t("reports.tool.memorize.key"))
                .clicked()
        {
            click = Some(Click::Memorize);
        }
        ui.separator();
        ui.add_enabled_ui(shown, |ui| {
            if ui
                .button(t("reports.tool.print"))
                .on_hover_text(t("reports.tool.print.key"))
                .clicked()
            {
                click = Some(Click::Print);
            }
            if ui.button(t("reports.tool.export_excel")).clicked() {
                click = Some(Click::Xlsx);
            }
            if ui.button(t("reports.tool.export_csv")).clicked() {
                click = Some(Click::Csv);
            }
            if ui.button(t("reports.tool.save_pdf")).clicked() {
                click = Some(Click::Pdf);
            }
        });
        ui.separator();
        if ui
            .add_enabled(shown, egui::Button::new(t("reports.tool.collapse")))
            .clicked()
        {
            click = Some(Click::Collapse);
        }
        if ui
            .button(t("reports.tool.refresh"))
            .on_hover_text(t("reports.tool.refresh.key"))
            .clicked()
        {
            click = Some(Click::Refresh);
        }
    });
    match click {
        Some(Click::Customize) => w.customize = !w.customize,
        Some(Click::Memorize) => super::memorized::open_dialog(w),
        Some(Click::Print) => super::export::print(w),
        Some(Click::Xlsx) => super::export::start(w, ui.ctx(), super::export::Kind::Xlsx),
        Some(Click::Csv) => super::export::start(w, ui.ctx(), super::export::Kind::Csv),
        Some(Click::Pdf) => super::export::start(w, ui.ctx(), super::export::Kind::Pdf),
        Some(Click::Collapse) => w.toggle_collapse_all(),
        Some(Click::Refresh) => w.run(),
        None => {}
    }
}

fn preset_text(choice: DateChoice) -> String {
    match choice {
        DateChoice::Preset(p) => t(p.label()),
        DateChoice::Fixed { .. } | DateChoice::AsOf(_) => t("reports.preset.custom"),
    }
}

#[allow(
    clippy::many_single_char_names,
    clippy::too_many_lines,
    reason = "one bar, in mockup order"
)]
pub(super) fn filter_bar(w: &mut ReportWindow, ui: &mut Ui, tokens: &Tokens) {
    let mut next = w.options.clone();
    let spec = w.spec;
    ui.horizontal_wrapped(|ui| {
        if spec.dates != DateMode::None {
            let l = ui.label(RichText::new(t("reports.filter.dates")).size(12.0));
            egui::ComboBox::from_id_salt("report_dates")
                .selected_text(preset_text(next.dates))
                .show_ui(ui, |ui| {
                    for p in DatePreset::ALL {
                        ui.selectable_value(&mut next.dates, DateChoice::Preset(p), t(p.label()));
                    }
                })
                .response
                .labelled_by(l.id);
            let today = w.today;
            if spec.dates == DateMode::AsOf {
                let r = form::date_field(ui, &t("reports.filter.as_of"), &mut w.dates.to, today);
                // Leaving the field unchanged keeps the preset.
                let shown = match next.dates {
                    DateChoice::Preset(p) => p.range(&w.fiscal, w.today).ok().map(|r| r.to),
                    other => other.as_of_hint(),
                };
                if r.lost_focus()
                    && let Some(d) = w.dates.to.value
                    && shown != Some(d)
                {
                    next.dates = DateChoice::AsOf(d);
                }
            } else {
                let a = form::date_field(ui, &t("reports.filter.from"), &mut w.dates.from, today);
                let b = form::date_field(ui, &t("reports.filter.to"), &mut w.dates.to, today);
                if (a.lost_focus() || b.lost_focus())
                    && let (Some(from), Some(to)) = (w.dates.from.value, w.dates.to.value)
                {
                    let fixed = DateChoice::Fixed { from, to };
                    let same = match next.dates {
                        DateChoice::Preset(p) => p
                            .range(&w.fiscal, w.today)
                            .is_ok_and(|r| r.from == from && r.to == to),
                        other => other == fixed,
                    };
                    if !same {
                        next.dates = fixed;
                    }
                }
            }
        }
        if spec.columns.len() > 1 {
            let l = ui.label(RichText::new(t("reports.filter.columns")).size(12.0));
            egui::ComboBox::from_id_salt("report_columns")
                .selected_text(t(next.columns.label()))
                .show_ui(ui, |ui| {
                    for c in spec.columns {
                        let ok = column_capability(*c).is_none_or(|cap| has(w, cap));
                        ui.add_enabled_ui(ok, |ui| {
                            let r = ui.selectable_value(&mut next.columns, *c, t(c.label()));
                            if !ok {
                                r.on_disabled_hover_text(t("reports.needs_update"));
                            }
                        });
                    }
                })
                .response
                .labelled_by(l.id);
        }
        if spec.basis == BasisMode::Toggle {
            ui.separator();
            for (b, key) in [
                (Basis::Accrual, "reports.basis.accrual"),
                (Basis::Cash, "reports.basis.cash"),
            ] {
                if ui.selectable_label(next.basis == Some(b), t(key)).clicked() {
                    next.basis = Some(b);
                }
            }
        } else {
            ui.label(
                RichText::new(t("reports.basis.not_applicable"))
                    .size(12.0)
                    .color(tokens.text_secondary),
            )
            .on_hover_text(t("reports.basis.not_applicable.why"));
        }
        for e in spec.extras {
            let ok = has(w, extra_capability(*e));
            let mut on = next.extras.contains(e);
            let r = ui.add_enabled(ok, egui::Checkbox::new(&mut on, t(extra_label(*e))));
            if !ok {
                r.on_disabled_hover_text(t("reports.needs_update"));
            } else if r.changed() {
                if on {
                    next.extras.insert(*e);
                } else {
                    next.extras.remove(e);
                }
            }
        }
    });
    if next != w.options {
        w.set_options(next);
    }
    ui.separator();
}

const fn extra_label(e: Extra) -> &'static str {
    match e {
        Extra::PctOfIncome => "reports.extra.pct_of_income",
        Extra::Ytd => "reports.extra.ytd",
        Extra::PriorPeriod => "reports.extra.prior_period",
        Extra::PriorYear => "reports.extra.prior_year",
    }
}

/// Customize: the owner's title, the run time on or off, the adjusting
/// lens. (Tag split and the id filters come with the engine update.)
pub(super) fn options_panel(w: &mut ReportWindow, ui: &mut Ui, tokens: &Tokens) {
    let mut next = w.options.clone();
    form::card(ui, tokens, |ui| {
        form::heading(ui, tokens, &t("reports.customize.title"));
        let mut title = next.header.title.clone().unwrap_or_default();
        let r = form::field(ui, &t("reports.customize.report_title"), &mut title);
        if r.changed() {
            next.header.title = Some(title.trim().to_string()).filter(|s| !s.is_empty());
        }
        let mut run_time = !next.header.hide_run_time;
        if ui
            .checkbox(&mut run_time, t("reports.customize.run_time"))
            .changed()
        {
            next.header.hide_run_time = !run_time;
        }
        if w.spec.tag_split {
            form::hint(ui, tokens, &t("reports.customize.tag_split_later"));
        }
        if ui.button(t("reports.customize.done")).clicked() {
            w.customize = false;
        }
    });
    if next != w.options {
        w.set_options(next);
    }
}

/// The as-of date a choice stands for, when it is one.
trait AsOfHint {
    fn as_of_hint(&self) -> Option<cl_core::CalendarDate>;
}

impl AsOfHint for DateChoice {
    fn as_of_hint(&self) -> Option<cl_core::CalendarDate> {
        match self {
            DateChoice::AsOf(d) => Some(*d),
            _ => None,
        }
    }
}
