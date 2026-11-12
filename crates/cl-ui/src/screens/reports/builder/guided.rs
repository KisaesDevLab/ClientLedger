//! Guided mode (plan §3.2, Q-A08a): five steps with Back (Alt+B) and Next
//! (Alt+N); a done step's "change" link reopens it; each answer updates
//! the preview. Done offers Run report, Memorize and Open in Advanced.

use cl_model::report_def::fields::{self, Availability, UNAVAILABLE_SOURCES};
use cl_model::report_def::guided::{GuidedState, Step};
use cl_model::report_def::validate::limits;
use cl_model::report_def::{CompareBase, Display, GroupBy, Periods, SourceKey};
use cl_model::reports::DatePreset;
use eframe::egui::{self, RichText, Ui};

use super::Pane;
use super::picker::{self, Choice};
use super::{BuilderScreen, FieldKey, PRESETS, preset_name};
use crate::i18n::t;
use crate::screens::company::form;
use crate::theme::Tokens;
use crate::windows::Screen;

/// Guided's main buttons are at least 44 points tall (the mockup's
/// touch-size buttons).
const TALL: f32 = 44.0;

/// Transaction columns (when a line is opened): chips with ✕ and "+ Add"
/// (UI review M4).
fn detail_chips(s: &mut BuilderScreen, ui: &mut Ui, g: &GuidedState) {
    picker::caption(ui, &t("builder.guided.detail"));
    let mut next = None;
    ui.horizontal_wrapped(|ui| {
        for (i, f) in g.detail_fields.iter().enumerate() {
            let name = t(&format!("reports.builder.field.{f}"));
            let r = ui.button(format!("{name} \u{d7}"));
            let label = crate::i18n::t_with("builder.filter.remove_value", &[("value", &name)]);
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
            if r.clicked() {
                let mut v = g.detail_fields.clone();
                v.remove(i);
                next = Some(v);
            }
        }
    });
    let left: Vec<&str> = fields::DETAIL_FIELDS
        .iter()
        .copied()
        .filter(|f| *f != "running_balance" && !g.detail_fields.iter().any(|x| x == f))
        .collect();
    if g.detail_fields.len() < limits::DETAIL_FIELDS && !left.is_empty() {
        let mut choices: Vec<Choice> = vec![picker::on(t("builder.guided.detail_add"))];
        choices.extend(
            left.iter()
                .map(|f| picker::on(t(&format!("reports.builder.field.{f}")))),
        );
        if let Some(k) = picker::pick(
            ui,
            ("g-detail", s.id.0),
            &t("builder.guided.detail_add_label"),
            &choices,
            Some(0),
            200.0,
        ) && k > 0
        {
            let mut v = g.detail_fields.clone();
            v.push(left[k - 1].to_string());
            next = Some(v);
        }
    }
    if let Some(v) = next {
        change(s, |g| g.detail_fields = v);
    }
}

const fn step_key(s: Step) -> &'static str {
    match s {
        Step::Source => "builder.guided.step.source",
        Step::Grouping => "builder.guided.step.grouping",
        Step::Lines => "builder.guided.step.lines",
        Step::Dates => "builder.guided.step.dates",
        Step::Done => "builder.guided.step.done",
    }
}

const fn question_key(s: Step) -> &'static str {
    match s {
        Step::Source => "builder.guided.q.source",
        Step::Grouping => "builder.guided.q.grouping",
        Step::Lines => "builder.guided.q.lines",
        Step::Dates => "builder.guided.q.dates",
        Step::Done => "builder.guided.q.done",
    }
}

/// The Guided screen: steps on the left, the preview on the right; on a
/// narrow window (or at 200% zoom) the preview moves below the steps (H1).
pub(crate) fn draw(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    toolbar(s, ui);
    if s.starter_menu {
        starter_menu(s, ui);
    }
    ui.separator();
    let w = ui.available_width();
    let h = ui.available_height();
    if w >= 760.0 {
        s.layout = super::Layout::Wide;
        ui.horizontal_top(|ui| {
            let left = (w * 0.58).clamp(380.0, w - 340.0);
            ui.allocate_ui_with_layout(
                egui::vec2(left, h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("guided", s.id.0))
                        .auto_shrink([false, false])
                        .show(ui, |ui| steps(s, ui, tokens));
                },
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| super::preview::draw(s, ui, tokens),
            );
        });
    } else {
        s.layout = super::Layout::Stacked;
        egui::ScrollArea::vertical()
            .id_salt(("guided", s.id.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                steps(s, ui, tokens);
                ui.separator();
                let w = ui.available_width();
                ui.allocate_ui_with_layout(
                    egui::vec2(w, 420.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_size(egui::vec2(w, 420.0));
                        super::preview::draw(s, ui, tokens);
                    },
                );
            });
    }
}

/// "New report", Start from a ready-made report ▾, Blank report in
/// Advanced, and Switch to Advanced at the right (UI review M3).
fn toolbar(s: &mut BuilderScreen, ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(t("builder.guided.title")).strong());
        ui.separator();
        let ready = s.starters.as_ref().is_none_or(|l| !l.is_empty());
        let r = ui
            .add_enabled(ready, egui::Button::new(t("builder.guided.ready_made")))
            .on_disabled_hover_text(t("builder.guided.no_starters"));
        if r.clicked() {
            s.starter_menu = !s.starter_menu;
            s.starter_menu_new = s.starter_menu;
            s.load_starters();
        }
        if ui.button(t("builder.guided.blank_advanced")).clicked() {
            s.guided = None;
            s.ed = cl_model::report_def::editor::BuilderEditor::blank();
            s.preview.edited(&s.ed);
            s.pane = Pane::Outline;
            s.focus_to = Some(Pane::Outline);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(t("builder.guided.switch")).clicked() {
                s.open_in_advanced();
            }
        });
    });
}

/// The ready-made reports: one opens in Advanced. The first takes the
/// keyboard; Esc closes the list.
fn starter_menu(s: &mut BuilderScreen, ui: &mut Ui) {
    let mut close = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let Some(list) = s.starters.clone() else {
        form::progress_line(
            ui,
            &Tokens::of(ui.ctx()),
            &t("builder.guided.loading_starters"),
        );
        return;
    };
    let mut pick = None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (n, st) in list.iter().enumerate() {
                let r = ui.button(&st.title);
                if n == 0 && s.starter_menu_new {
                    r.request_focus();
                }
                if r.clicked() {
                    pick = Some(st.clone());
                }
            }
        });
    });
    s.starter_menu_new = false;
    if let Some(st) = pick {
        close = true;
        let text = serde_json::to_string(&st.definition).unwrap_or_default();
        match cl_model::report_def::parse(&text) {
            Ok(def) => s.open_starter(st.key, def, st.title),
            Err(_) => s.status = Some(t("reports.builder.unreadable")),
        }
    }
    if close {
        s.starter_menu = false;
    }
}
fn steps(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    let Some(g) = s.guided.clone() else {
        return;
    };
    // The step strip.
    ui.horizontal_wrapped(|ui| {
        for (i, step) in Step::ALL.iter().enumerate() {
            let done = *step < g.step && *step <= g.reached;
            let mark = if done {
                "\u{2713}".to_string()
            } else {
                (i + 1).to_string()
            };
            let text = format!("{mark} {}", t(step_key(*step)));
            let rich = if *step == g.step {
                RichText::new(text).strong().color(tokens.accent)
            } else {
                RichText::new(text).color(tokens.text_secondary)
            };
            ui.label(rich);
        }
    });
    ui.add_space(8.0);
    for step in Step::ALL {
        if step > g.reached {
            break;
        }
        let open = step == g.step;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                // The open step's question takes the keyboard when the
                // editor opens, after Next or Back and on F6: Tab then
                // reaches its choices (AC6).
                let q = ui.add(
                    egui::Label::new(RichText::new(t(question_key(step))).strong())
                        .sense(egui::Sense::focusable_noninteractive()),
                );
                if open && matches!(s.focus_to, Some(Pane::Outline | Pane::Properties)) {
                    q.request_focus();
                }
                if open && q.has_focus() {
                    s.pane = Pane::Outline;
                }
                if !open {
                    ui.label(
                        RichText::new(summary(&g, step))
                            .size(12.0)
                            .color(tokens.text_secondary),
                    );
                    if ui.link(t("builder.guided.change")).clicked()
                        && let Some(g) = s.guided.as_mut()
                    {
                        g.reopen(step);
                    }
                }
            });
            if open {
                body(s, ui, tokens, step);
                nav(s, ui, step);
            }
        });
        ui.add_space(6.0);
    }
}

fn summary(g: &GuidedState, step: Step) -> String {
    match step {
        Step::Source => g
            .source
            .map(|x| t(&format!("builder.source.{}", x.as_str())))
            .unwrap_or_default(),
        Step::Grouping => g
            .group_by
            .iter()
            .map(|x| t(&format!("builder.group.{}", x.key)))
            .collect::<Vec<_>>()
            .join(", "),
        Step::Lines => g.display.map(|d| t(display_key(d))).unwrap_or_default(),
        Step::Dates => g
            .preset
            .as_deref()
            .and_then(super::preset_of)
            .map_or_else(|| t("builder.band.each_time"), |p| t(p.label())),
        Step::Done => String::new(),
    }
}

const fn display_key(d: Display) -> &'static str {
    match d {
        Display::Summary => "builder.guided.lines.summary",
        Display::Detail => "builder.guided.lines.detail",
        Display::Expandable => "builder.guided.lines.expandable",
    }
}

fn change(s: &mut BuilderScreen, f: impl FnOnce(&mut GuidedState)) {
    if let Some(g) = s.guided.as_mut() {
        f(g);
    }
    s.guided_changed();
}

fn body(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens, step: Step) {
    let Some(g) = s.guided.clone() else {
        return;
    };
    match step {
        Step::Source => {
            for src in SourceKey::ALL {
                let r = form::choice(
                    ui,
                    tokens,
                    g.source == Some(src),
                    &t(&format!("builder.guided.source.{}", src.as_str())),
                    &t(&format!("builder.guided.source_hint.{}", src.as_str())),
                );
                if form::activated(&r) {
                    change(s, |g| g.set_source(src));
                }
            }
            for (k, why) in UNAVAILABLE_SOURCES {
                ui.add_enabled_ui(false, |ui| {
                    form::choice(
                        ui,
                        tokens,
                        false,
                        &t(&format!("builder.source.{k}")),
                        &t(why),
                    )
                });
            }
        }
        Step::Grouping => {
            let Some(src) = g.source else {
                return;
            };
            let offered = fields::of(src).group_bys;
            for slot in 0..limits::GROUP_BYS {
                if slot > g.group_by.len() {
                    break;
                }
                let mut choices: Vec<Choice> = vec![picker::on(t("builder.group.none"))];
                choices.extend(offered.iter().filter(|k| **k != "tag").map(|k| {
                    let used = g
                        .group_by
                        .iter()
                        .enumerate()
                        .any(|(i, x)| i != slot && x.key == *k);
                    (
                        t(&format!("builder.group.{k}")),
                        if used {
                            Availability::Greyed("builder.greyed.used")
                        } else {
                            Availability::Available
                        },
                    )
                }));
                let keys: Vec<&str> = offered.iter().copied().filter(|k| *k != "tag").collect();
                let at = g
                    .group_by
                    .get(slot)
                    .and_then(|x| keys.iter().position(|k| *k == x.key))
                    .map_or(0, |p| p + 1);
                let label = if slot == 0 {
                    t("builder.row.group_by")
                } else {
                    t("builder.row.then_by")
                };
                if let Some(k) = picker::pick(
                    ui,
                    ("g-gb", s.id.0, slot),
                    &label,
                    &choices,
                    Some(at),
                    200.0,
                ) {
                    change(s, |g| {
                        if k == 0 {
                            g.group_by.truncate(slot.max(1));
                        } else {
                            let gb = GroupBy {
                                key: keys[k - 1].to_string(),
                                tag_group_id: None,
                            };
                            if slot < g.group_by.len() {
                                g.group_by[slot] = gb;
                            } else {
                                g.group_by.push(gb);
                            }
                        }
                    });
                    return;
                }
            }
        }
        Step::Lines => {
            for d in [Display::Summary, Display::Detail, Display::Expandable] {
                let key = display_key(d);
                let r = form::choice(
                    ui,
                    tokens,
                    g.display == Some(d),
                    &t(key),
                    &t(&format!("{key}_hint")),
                );
                if form::activated(&r) {
                    change(s, |g| g.display = Some(d));
                }
            }
            if !g.open_items() {
                let periods = [
                    None,
                    Some(Periods::Month),
                    Some(Periods::Quarter),
                    Some(Periods::Year),
                ];
                let choices: Vec<Choice> = ["total", "month", "quarter", "year"]
                    .iter()
                    .map(|k| picker::on(t(&format!("builder.columns.across.{k}"))))
                    .collect();
                picker::caption(ui, &t("builder.columns.across"));
                let at = periods.iter().position(|p| *p == g.periods);
                if let Some(i) = picker::segmented(ui, &t("builder.columns.across"), &choices, at) {
                    change(s, |g| g.periods = periods[i]);
                    return;
                }
                let bases = [
                    None,
                    Some(CompareBase::PriorPeriod),
                    Some(CompareBase::PriorYear),
                ];
                let choices: Vec<Choice> = ["none", "prior_period", "prior_year"]
                    .iter()
                    .map(|k| picker::on(t(&format!("builder.columns.base.{k}"))))
                    .collect();
                let at = bases.iter().position(|b| *b == g.compare);
                if let Some(i) = picker::pick(
                    ui,
                    ("g-cmp", s.id.0),
                    &t("builder.columns.compare_with"),
                    &choices,
                    at,
                    200.0,
                ) {
                    change(s, |g| g.compare = bases[i]);
                    return;
                }
            }
            if let Some(v) = picker::check(ui, g.percent_of_total, &t("builder.guided.percent")) {
                change(s, |g| g.percent_of_total = v);
                return;
            }
            detail_chips(s, ui, &g);
        }
        Step::Dates => {
            let mut choices: Vec<Choice> = PRESETS
                .iter()
                .map(|(p, _)| picker::on(t(p.label())))
                .collect();
            choices.push(picker::on(t("builder.band.each_time")));
            let at = g
                .preset
                .as_deref()
                .and_then(super::preset_of)
                .and_then(|p| PRESETS.iter().position(|(x, _)| *x == p))
                .unwrap_or(PRESETS.len());
            if let Some(i) = picker::pick(
                ui,
                ("g-dates", s.id.0),
                &t("builder.band.dates"),
                &choices,
                Some(at),
                200.0,
            ) {
                let preset: Option<DatePreset> = PRESETS.get(i).map(|(p, _)| *p);
                change(s, |g| g.preset = preset.map(|p| preset_name(p).to_string()));
                return;
            }
            // From/To (As of): this session's dates, as in Advanced's band
            // (Q-A08q), shown under the date choice (UI review M5).
            ui.horizontal_wrapped(|ui| super::chrome::from_to(s, ui));
            if g.open_items() {
                form::inline_note(ui, tokens, &t("builder.guided.open_items_basis"));
            }
            let basis = match g.basis {
                Some(cl_core::report::Basis::Accrual) => 0,
                Some(cl_core::report::Basis::Cash) => 1,
                None => 2,
            };
            picker::caption(ui, &t("builder.band.basis"))
                .on_hover_text(t("builder.band.basis_tip"));
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
                change(s, |g| {
                    g.basis = match i {
                        0 => Some(cl_core::report::Basis::Accrual),
                        1 => Some(cl_core::report::Basis::Cash),
                        _ => None,
                    };
                });
            }
        }
        Step::Done => {
            let name = g.name.clone();
            picker::text(
                s,
                ui,
                FieldKey::GuidedName,
                &t("builder.report.name"),
                &name,
                320.0,
            );
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().interact_size.y = TALL;
                let run = ui.add(
                    egui::Button::new(
                        RichText::new(t("builder.toolbar.run_report"))
                            .strong()
                            .color(tokens.on_accent),
                    )
                    .fill(tokens.accent)
                    .min_size(egui::vec2(0.0, TALL)),
                );
                if run.clicked() {
                    s.commit_pending();
                    s.run_report();
                }
                let memo = ui
                    .horizontal(|ui| {
                        let r = ui.add(
                            egui::Button::new(t("builder.toolbar.memorize"))
                                .min_size(egui::vec2(0.0, TALL)),
                        );
                        crate::widgets::kbd(ui, "Ctrl M");
                        r
                    })
                    .inner;
                if memo.clicked() {
                    s.commit_pending();
                    super::save::open_memorize(s, false);
                }
                if ui
                    .add(
                        egui::Button::new(t("builder.guided.open_advanced"))
                            .min_size(egui::vec2(0.0, TALL)),
                    )
                    .clicked()
                {
                    s.open_in_advanced();
                }
            });
            ui.label(
                RichText::new(t("builder.guided.later"))
                    .size(12.0)
                    .color(tokens.text_secondary),
            );
        }
    }
}

fn nav(s: &mut BuilderScreen, ui: &mut Ui, step: Step) {
    if step == Step::Done {
        return;
    }
    let complete = s.guided.as_ref().is_some_and(|g| g.complete(step));
    ui.horizontal(|ui| {
        let back = ui.add_enabled(
            step != Step::Source,
            egui::Button::new(t("builder.guided.back")).min_size(egui::vec2(0.0, TALL)),
        );
        if back.clicked() {
            s.command(cl_model::CommandId::new("builder.guided_back"));
        }
        crate::widgets::kbd(ui, "Alt B");
        let tokens = Tokens::of(ui.ctx());
        let next = ui.add_enabled(
            complete,
            egui::Button::new(
                RichText::new(t("builder.guided.next"))
                    .strong()
                    .color(tokens.on_accent),
            )
            .fill(tokens.accent)
            .min_size(egui::vec2(0.0, TALL)),
        );
        let next = next.on_disabled_hover_text(t("builder.guided.answer_first"));
        if next.clicked() {
            s.command(cl_model::CommandId::new("builder.guided_next"));
        }
        crate::widgets::kbd(ui, "Alt N");
    });
}
