//! The toolbar (mockup order), the hint line under it and the settings
//! band (Report name, dates, basis, adjusting entries).

use cl_core::report::{Adjusting, Basis};
use cl_model::report_def::editor::Op;
use cl_model::report_def::guided::GuidedState;
use cl_model::report_def::{DateMode, Dates};
use cl_model::reports::DateChoice;
use eframe::egui::{self, RichText, Ui};

use super::picker::{self, Choice};
use super::{BuilderScreen, FieldKey, PRESETS, preset_name};
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;
use crate::widgets::{button_with_chip, primary_button};
use crate::windows::{OpenPayload, Screen};

/// A toolbar button with its shortcut chip, off with a reason when it
/// can't act.
fn tool(ui: &mut Ui, label: &str, chip: &str, on: bool, why: &str) -> bool {
    let r = ui.add_enabled_ui(on, |ui| {
        if chip.is_empty() {
            ui.button(label)
        } else {
            button_with_chip(ui, label, chip)
        }
    });
    let r = if on {
        r.inner
    } else {
        r.inner.on_disabled_hover_text(why)
    };
    r.clicked()
}

/// The toolbar and the hint line.
pub(crate) fn toolbar(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    let can_run = s.ed.can_run();
    let fix = t("builder.status.fix_first");
    ui.horizontal_wrapped(|ui| {
        // A narrow window wraps whole buttons onto the next line; it never
        // squeezes a label into a column of letters (H1).
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        let run = ui.add_enabled_ui(can_run, |ui| {
            primary_button(ui, &t("builder.toolbar.run_report"))
        });
        if run.inner.on_disabled_hover_text(&fix).clicked() {
            s.run_report();
        }
        if tool(
            ui,
            &t("builder.toolbar.refresh"),
            "Ctrl Enter",
            can_run,
            &fix,
        ) {
            s.commit_pending();
            s.preview.run_now();
        }
        ui.separator();
        let save_why = if s.ed.saved().is_some_and(|x| !x.can_edit) {
            t("builder.save.not_author")
        } else {
            t("builder.toolbar.save_off")
        };
        if tool(
            ui,
            &t("builder.toolbar.save_changes"),
            "",
            s.ed.can_save_changes(),
            &save_why,
        ) {
            s.commit_pending();
            super::save::save_changes(s, false);
        }
        if tool(ui, &t("builder.toolbar.memorize"), "Ctrl M", true, "") {
            s.commit_pending();
            super::save::open_memorize(s, false);
        }
        ui.separator();
        if tool(
            ui,
            &t("builder.toolbar.undo"),
            "Ctrl Z",
            s.ed.can_undo() || s.pending.is_some(),
            &t("builder.toolbar.nothing_to_undo"),
        ) {
            s.command(cl_model::CommandId::new("builder.undo"));
        }
        if tool(
            ui,
            &t("builder.toolbar.redo"),
            "Ctrl Y",
            s.ed.can_redo(),
            &t("builder.toolbar.nothing_to_redo"),
        ) {
            s.command(cl_model::CommandId::new("builder.redo"));
        }
        ui.separator();
        // Export and Print run the full report in its tab, then open that
        // tab's export or print (Q-A08m, R13): never the capped preview.
        if tool(ui, &t("builder.toolbar.export"), "", can_run, &fix) {
            s.export_menu = !s.export_menu;
            s.export_menu_new = s.export_menu;
        }
        if tool(ui, &t("builder.toolbar.print"), "Ctrl P", can_run, &fix) {
            s.run_then(Some(super::super::builder_ref::AfterRun::Print));
        }
        ui.separator();
        let guided_ok = GuidedState::from_definition(s.ed.definition()).is_some();
        if tool(
            ui,
            &t("builder.toolbar.guided_view"),
            "",
            guided_ok,
            &t("builder.toolbar.guided_off"),
        ) {
            s.show_guided();
        }
    });
    if s.export_menu {
        export_menu(s, ui);
    }
    hint(s, ui, tokens);
}

/// Export ▾: Excel, CSV or PDF of the full run (Q-A08m). The first choice
/// takes the keyboard when it opens; Esc closes it.
fn export_menu(s: &mut BuilderScreen, ui: &mut Ui) {
    use super::super::builder_ref::AfterRun;
    use super::super::export::Kind;
    let mut close = ui.input(|i| i.key_pressed(egui::Key::Escape));
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (n, (key, kind)) in [
                ("builder.export.xlsx", Kind::Xlsx),
                ("builder.export.csv", Kind::Csv),
                ("builder.export.pdf", Kind::Pdf),
            ]
            .into_iter()
            .enumerate()
            {
                let r = ui.button(t(key));
                if n == 0 && s.export_menu_new {
                    r.request_focus();
                }
                if r.clicked() {
                    s.run_then(Some(AfterRun::Export(kind)));
                    close = true;
                }
            }
        });
    });
    s.export_menu_new = false;
    if close {
        s.export_menu = false;
    }
}

/// The report the editor started from, as its title and what opens it: a
/// standard report, or the ready-made report a starter is (UI review M6).
fn origin_link(origin: &cl_model::report_def::editor::Origin) -> Option<(String, OpenPayload)> {
    use cl_model::report_def::editor::Origin;
    let open = |key: &str| super::super::ReportOpen {
        key: key.to_string(),
        options: None,
    };
    match origin {
        Origin::Standard(k) => {
            let spec = cl_model::reports::find(k)?;
            Some((t(spec.title), OpenPayload::Report(Box::new(open(spec.key)))))
        }
        Origin::Starter(k) => {
            let title = cl_model::reports::REPORT_STARTERS
                .iter()
                .find(|(_, st)| st == k)
                .and_then(|(r, _)| cl_model::reports::find(r))
                .map_or_else(|| t("builder.origin.starter"), |spec| t(spec.title));
            Some((title, OpenPayload::BuilderStarter(k.clone())))
        }
        _ => None,
    }
}

/// "Saved 9:38 AM · Author: Kurt · Started from Profit & Loss", or "Not
/// saved yet"; then the status line.
fn hint(s: &BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    let mut parts = Vec::new();
    match s.ed.saved() {
        Some(saved) => {
            let at = saved
                .updated_at
                .map(|ts| {
                    ts.as_utc()
                        .with_timezone(&chrono::Local)
                        .format("%-I:%M %p")
                        .to_string()
                })
                .unwrap_or_default();
            parts.push(t_with("builder.hint.saved", &[("time", &at)]));
            if let Some(a) = &saved.author {
                parts.push(t_with("builder.hint.author", &[("name", a)]));
            }
        }
        None => parts.push(t("builder.hint.not_saved")),
    }
    let link = origin_link(s.ed.origin());
    let origin = match s.ed.origin() {
        cl_model::report_def::editor::Origin::Blank => t("builder.origin.blank"),
        cl_model::report_def::editor::Origin::Guided => t("builder.origin.guided"),
        cl_model::report_def::editor::Origin::Memorized => t("builder.origin.memorized"),
        _ => String::new(),
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let text = if link.is_some() {
            format!(
                "{} \u{b7} {}",
                parts.join(" \u{b7} "),
                t("builder.hint.started_link")
            )
        } else {
            parts.push(t_with("builder.hint.started", &[("origin", &origin)]));
            parts.join(" \u{b7} ")
        };
        ui.label(RichText::new(text).size(12.0).color(tokens.text_secondary));
        if let Some((title, payload)) = link {
            let r = ui.link(RichText::new(title).size(12.0));
            if r.on_hover_text(t("builder.hint.open_origin")).clicked() {
                s.link.push(crate::company::CompanyEvent::OpenWith(
                    cl_model::CommandId::new(super::super::KIND),
                    Box::new(payload),
                ));
            }
        }
    });
    if let Some(st) = &s.status {
        ui.label(RichText::new(st).size(12.0).color(tokens.warn_text));
    }
}

/// The settings band.
pub(crate) fn band(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    let _ = tokens;
    ui.horizontal_wrapped(|ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        let name = s.ed.name().to_string();
        picker::text(
            s,
            ui,
            FieldKey::Name,
            &t("builder.report.name"),
            &name,
            220.0,
        );
        ui.add_space(8.0);
        let def = s.ed.definition().clone();
        let mode = usize::from(def.dates.mode == DateMode::AsOf);
        if let Some(i) = picker::segmented(
            ui,
            &t("builder.band.date_mode"),
            &[
                picker::on(t("builder.band.range")),
                picker::on(t("builder.band.as_of")),
            ],
            Some(mode),
        ) {
            let mut d = def.dates.clone();
            d.mode = if i == 1 {
                DateMode::AsOf
            } else {
                DateMode::Range
            };
            s.band.custom = None;
            s.apply(Op::Dates(d));
        }
        dates(s, ui, &def.dates);
    });
    ui.horizontal_wrapped(|ui| {
        let def = s.ed.definition().clone();
        let basis = match def.basis {
            Some(Basis::Accrual) => 0,
            Some(Basis::Cash) => 1,
            None => 2,
        };
        let l = picker::caption(ui, &t("builder.band.basis"));
        l.on_hover_text(t("builder.band.basis_tip_advanced"));
        if let Some(i) = picker::segmented(
            ui,
            &t("builder.band.basis"),
            &[
                picker::on(t("builder.band.accrual")),
                picker::on(t("builder.band.cash")),
                picker::on(t("builder.band.company_default")),
            ],
            Some(basis),
        ) {
            let b = match i {
                0 => Some(Basis::Accrual),
                1 => Some(Basis::Cash),
                _ => None,
            };
            s.apply(Op::Basis(b));
        }
        ui.add_space(8.0);
        let adj = match def.adjusting {
            None | Some(Adjusting::Include) => 0,
            Some(Adjusting::Exclude) => 1,
            Some(Adjusting::Only) => 2,
        };
        let choices: Vec<Choice> = ["include", "exclude", "only"]
            .iter()
            .map(|k| picker::on(t(&format!("builder.band.adjusting.{k}"))))
            .collect();
        if let Some(i) = picker::pick_tip(
            ui,
            ("adjusting", s.id.0),
            &t("builder.band.adjusting"),
            Some(&t("reports.filter.adjusting.why")),
            &choices,
            Some(adj),
            110.0,
        ) {
            let a = match i {
                1 => Some(Adjusting::Exclude),
                2 => Some(Adjusting::Only),
                _ => None,
            };
            s.apply(Op::Adjusting(a));
        }
    });
}

/// The Dates preset and From/To (As of) fields. Typed dates are this
/// session's only (Q-A08q).
fn dates(s: &mut BuilderScreen, ui: &mut Ui, d: &Dates) {
    let mut choices: Vec<Choice> = PRESETS
        .iter()
        .map(|(p, _)| picker::on(t(p.label())))
        .collect();
    choices.push(picker::on(t("builder.band.each_time")));
    choices.push(picker::on(t("builder.band.custom")));
    let custom_at = choices.len() - 1;
    let current = if s.band.custom.is_some() {
        custom_at
    } else {
        match d.preset.as_deref().and_then(super::preset_of) {
            Some(p) => PRESETS.iter().position(|(x, _)| *x == p).unwrap_or(0),
            None => custom_at - 1,
        }
    };
    if let Some(i) = picker::pick(
        ui,
        ("dates", s.id.0),
        &t("builder.band.dates"),
        &choices,
        Some(current),
        160.0,
    ) {
        if i < PRESETS.len() {
            s.band.custom = None;
            let mut nd = d.clone();
            nd.preset = Some(preset_name(PRESETS[i].0).into());
            s.apply(Op::Dates(nd));
        } else if i == custom_at - 1 {
            s.band.custom = None;
            let mut nd = d.clone();
            nd.preset = None;
            s.apply(Op::Dates(nd));
        } else if let Some(r) = s.shown_dates() {
            s.band.custom = Some(if s.as_of() {
                DateChoice::AsOf(r.to)
            } else {
                DateChoice::Fixed {
                    from: r.from,
                    to: r.to,
                }
            });
            s.preview.edited(&s.ed);
        }
    }
    from_to(s, ui);
}

/// From and To (As of): they show the report's dates; typing makes them
/// this session's custom dates (Q-A08q). Guided step 4 shows them too.
pub(super) fn from_to(s: &mut BuilderScreen, ui: &mut Ui) {
    // The fields show the report's dates; typing makes them this
    // session's custom dates.
    if let Some(r) = s.shown_dates()
        && s.band.custom.is_none()
        && s.pending.is_none()
    {
        if s.band.from.value != Some(r.from) {
            s.band.from.set(r.from);
        }
        if s.band.to.value != Some(r.to) {
            s.band.to.set(r.to);
        }
    }
    let today = s.today;
    let mut changed = false;
    if !s.as_of() {
        let r = form::date_field(ui, &t("builder.band.from"), &mut s.band.from, today);
        changed |= r.lost_focus();
    }
    let label = if s.as_of() {
        t("builder.band.as_of_date")
    } else {
        t("builder.band.to")
    };
    let r = form::date_field(ui, &label, &mut s.band.to, today);
    changed |= r.lost_focus();
    if changed && let Some(to) = s.band.to.value {
        let next = if s.as_of() {
            Some(DateChoice::AsOf(to))
        } else {
            s.band
                .from
                .value
                .filter(|f| *f <= to)
                .map(|from| DateChoice::Fixed { from, to })
        };
        let same = s
            .shown_dates()
            .is_some_and(|r| r.to == to && (s.as_of() || Some(r.from) == s.band.from.value));
        if let Some(n) = next
            && !same
        {
            s.band.custom = Some(n);
            s.preview.edited(&s.ed);
        }
    }
    if s.band.custom.is_some() {
        ui.label(
            RichText::new(t("builder.band.custom_note"))
                .size(12.0)
                .italics(),
        );
    }
}
