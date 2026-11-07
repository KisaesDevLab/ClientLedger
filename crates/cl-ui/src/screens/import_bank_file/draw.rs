//! Drawing the Import Bank File wizard (mockup `ImportBankFile`): the
//! heading, the four steps, each step's cards, and the button row.

use std::collections::BTreeMap;

use cl_api::{BankImportOutcome, ChangedAction, ImportRowResult, ImportRowStatus};
use cl_core::{CalendarDate, MoneyStyle};
use cl_import::{AccountKind, AmountColumns, DateOrder, FileFormat, Section, SkipReason};
use cl_model::CommandId;
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke, Ui};

use super::flow::{self, RowChoice};
use super::{EngineTrouble, ImportBankFileScreen, LinkedHow, Step};
use crate::company::CompanyEvent;
use crate::i18n::{t, t_count, t_with};
use crate::layout::{Body, FixedFrame, FrameOpts};
use crate::screens::company::form;
use crate::theme::Tokens;

/// Rows drawn at once in the check table (the rest are counted).
const SHOWN_ROWS: usize = 200;

/// Below this height (points) the step bar becomes one line of text.
const SHORT_HEIGHT: f32 = 300.0;

/// A command's shortcut as the keymap has it ("Alt+N").
fn shortcut_hint(id: &str) -> String {
    cl_model::commands::find(id)
        .and_then(|c| c.shortcut)
        .map(crate::input::shortcut_text)
        .unwrap_or_default()
}

fn short_date(d: CalendarDate) -> String {
    format!("{:02}/{:02}", d.month(), d.day())
}

fn local_date(t: cl_core::Timestamp) -> String {
    let d = t.as_utc().with_timezone(&chrono::Local).date_naive();
    format!(
        "{:02}/{:02}",
        chrono::Datelike::month(&d),
        chrono::Datelike::day(&d)
    )
}

fn format_name(f: FileFormat) -> &'static str {
    match f {
        FileFormat::Ofx => "bankimport.format.ofx",
        FileFormat::Qfx => "bankimport.format.qfx",
        FileFormat::Qif => "bankimport.format.qif",
        FileFormat::Csv => "bankimport.format.csv",
    }
}

/// The file's format as the mockup names it: an OFX or QFX file also
/// says its OFX version (2.x is XML, 1.x the older SGML).
fn format_label(l: &flow::Loaded) -> String {
    let name = t(format_name(l.parsed.format));
    if !matches!(l.parsed.format, FileFormat::Ofx | FileFormat::Qfx) {
        return name;
    }
    let head = &l.bytes[..l.bytes.len().min(1024)];
    let xml = head.windows(5).any(|w| w.eq_ignore_ascii_case(b"<?ofx"))
        || head.windows(5).any(|w| w.eq_ignore_ascii_case(b"<?xml"));
    t_with(
        if xml {
            "bankimport.format.v2"
        } else {
            "bankimport.format.v1"
        },
        &[("format", &name)],
    )
}

fn kind_name(k: AccountKind) -> &'static str {
    match k {
        AccountKind::CreditCard => "bankimport.kind.card",
        AccountKind::Savings => "bankimport.kind.savings",
        AccountKind::Cash => "bankimport.kind.cash",
        AccountKind::Unknown => "bankimport.kind.unknown",
        _ => "bankimport.kind.checking",
    }
}

/// A small rounded flag (the mockup's chips).
fn chip(ui: &mut Ui, tokens: &Tokens, text: &str, accent: bool) {
    chip_sensing(ui, tokens, text, accent, egui::Sense::hover());
}

/// A chip with a tooltip: it takes keyboard focus, so the tip is reached
/// by Tab and by a screen reader as well as by the mouse (UI review 4).
fn chip_with_tip(ui: &mut Ui, tokens: &Tokens, text: &str, tip: &str) {
    chip_sensing(ui, tokens, text, false, egui::Sense::click()).on_hover_text(tip);
}

fn chip_sensing(
    ui: &mut Ui,
    tokens: &Tokens,
    text: &str,
    accent: bool,
    sense: egui::Sense,
) -> egui::Response {
    let (fill, color) = if accent {
        (tokens.accent_tint, tokens.accent)
    } else {
        (tokens.neutral_chip, tokens.neutral_chip_text)
    };
    egui::Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(RichText::new(text).size(12.0).color(color))
                    .selectable(false)
                    .sense(sense),
            )
        })
        .inner
}

/// A label and a value on one line, the value right-aligned.
fn pair(ui: &mut Ui, tokens: &Tokens, label: &str, value: &str, mono: bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(tokens.text_secondary));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let v = RichText::new(value).color(tokens.text_primary);
            ui.label(if mono {
                v.text_style(egui::TextStyle::Monospace)
            } else {
                v
            });
        });
    });
}

/// A titled card of the mockup (white body, a heading bar).
fn titled_card(ui: &mut Ui, tokens: &Tokens, title: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(tokens.surface_window)
        .stroke(Stroke::new(1.0, tokens.border_default))
        .corner_radius(CornerRadius::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // The mockup's compact heading bar (UI review: the grid
            // below keeps its room).
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::Frame::new()
                .fill(tokens.surface_subtle)
                .corner_radius(CornerRadius {
                    nw: 6,
                    ne: 6,
                    sw: 0,
                    se: 0,
                })
                .inner_margin(Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(title).strong().color(tokens.text_primary));
                });
            ui.add(egui::Separator::default().spacing(0.0));
            egui::Frame::new()
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 6.0;
                    add(ui);
                });
        });
}

fn trouble_text(trouble: &EngineTrouble) -> String {
    match trouble {
        EngineTrouble::Unknown => t("bankimport.lost"),
        EngineTrouble::Other(k) => t(k),
        EngineTrouble::Refused(cl_api::BankImportRefusal::InvalidRow { line, .. }) => t_with(
            "bankimport.refused.row",
            &[("line", &line.map_or_else(String::new, |l| l.to_string()))],
        ),
        EngineTrouble::Refused(cl_api::BankImportRefusal::LinkedElsewhere(l)) => t_with(
            "bankimport.linked_elsewhere",
            &[("mask", &l.mask), ("account", &l.account_name)],
        ),
        EngineTrouble::Refused(r) => t(flow::refusal_key(r)),
    }
}

impl ImportBankFileScreen {
    pub(super) fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        // Q-265: the title and the step bar stay at the top, the buttons at
        // the bottom, and only the step's content scrolls between them (the
        // smallest window at 200% zoom keeps the buttons in reach).
        egui::Frame::new()
            .fill(tokens.surface_window)
            .inner_margin(Margin::symmetric(20, 12))
            .show(ui, |ui| {
                // A short window (the smallest at 200% zoom) says the step in
                // one line under the title, so the step's content keeps room.
                let short = ui.available_height() < SHORT_HEIGHT;
                let width = ui.available_width();
                let mut frame = FixedFrame::begin(ui, "import_bank_file", FrameOpts::default());
                frame.top(|ui| {
                    ui.set_max_width(width);
                    ui.horizontal_wrapped(|ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(t("cmd.banking.import_file"))
                                    .size(22.0)
                                    .strong()
                                    .color(tokens.text_primary),
                            )
                            .selectable(false),
                        );
                        ui.add_space(8.0);
                        if short {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(self.step_line())
                                        .strong()
                                        .color(tokens.accent),
                                )
                                .wrap(),
                            );
                        } else {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(t("bankimport.subtitle"))
                                        .color(tokens.text_secondary),
                                )
                                .wrap(),
                            );
                        }
                    });
                    if !short {
                        ui.add_space(6.0);
                        self.steps_ui(ui, &tokens);
                    }
                });
                frame.body(Body::Scroll, |ui| {
                    ui.set_width(ui.available_width());
                    match self.step {
                        Step::Choose => self.choose_ui(ui, &tokens),
                        Step::Link => self.link_ui(ui, &tokens, style),
                        Step::Check => self.check_ui(ui, &tokens, style),
                        Step::Done => self.done_ui(ui, &tokens),
                    }
                });
                frame.bottom(|ui| {
                    ui.add_space(4.0);
                    self.buttons_ui(ui, &tokens);
                });
                self.layout = frame.end();
            });
    }

    /// The step in words ("Step 3 of 4: Check transactions"), for a
    /// short window.
    fn step_line(&self) -> String {
        let (n, key) = match self.step {
            Step::Choose => (1, "bankimport.step.choose"),
            Step::Link => (2, "bankimport.step.link"),
            Step::Check => (3, "bankimport.step.check"),
            Step::Done => (4, "bankimport.step.send"),
        };
        t_with(
            "bankimport.step_of",
            &[("n", &n.to_string()), ("step", &t(key))],
        )
    }

    fn steps_ui(&self, ui: &mut Ui, tokens: &Tokens) {
        let steps = [
            (Step::Choose, "bankimport.step.choose"),
            (Step::Link, "bankimport.step.link"),
            (Step::Check, "bankimport.step.check"),
            (Step::Done, "bankimport.step.send"),
        ];
        egui::Frame::new()
            .stroke(Stroke::new(1.0, tokens.border_default))
            .corner_radius(CornerRadius::same(6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // The mockup's cells are at least 160 px wide: a narrower
                // bar (a small window or 200% zoom) puts them in two rows.
                let bar_w = ui.available_width();
                let per_row = if bar_w >= 4.0 * 160.0 { 4 } else { 2 };
                #[allow(clippy::cast_precision_loss)] // 2 or 4
                let w = bar_w / per_row as f32;
                ui.spacing_mut().item_spacing.y = 0.0;
                for row in steps.chunks(per_row) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for (step, key) in row {
                            let i = steps.iter().position(|(s, _)| s == step).unwrap_or(0);
                            let done = *step < self.step
                                || (self.step == Step::Done && *step == Step::Done);
                            let current = *step == self.step && self.step != Step::Done;
                            let fill = if current {
                                tokens.accent_tint
                            } else {
                                Color32::TRANSPARENT
                            };
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(w, 44.0), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 0.0, fill);
                            if i % per_row > 0 {
                                ui.painter().vline(
                                    rect.left(),
                                    rect.y_range(),
                                    Stroke::new(1.0, tokens.border_default),
                                );
                            }
                            let center = egui::pos2(rect.left() + 26.0, rect.center().y);
                            let dot = if done {
                                tokens.accent
                            } else {
                                Color32::TRANSPARENT
                            };
                            ui.painter().circle(
                                center,
                                11.0,
                                dot,
                                Stroke::new(
                                    1.5,
                                    if done || current {
                                        tokens.accent
                                    } else {
                                        tokens.border_strong
                                    },
                                ),
                            );
                            let mark = if done {
                                "\u{2713}".to_string()
                            } else {
                                (i + 1).to_string()
                            };
                            ui.painter().text(
                                center,
                                egui::Align2::CENTER_CENTER,
                                mark,
                                egui::FontId::proportional(12.0),
                                if done {
                                    tokens.on_accent
                                } else {
                                    tokens.text_primary
                                },
                            );
                            let label = t(key);
                            let font = if current {
                                egui::FontId::new(14.0, egui::FontFamily::Name("bold".into()))
                            } else {
                                egui::FontId::proportional(14.0)
                            };
                            let font = if ui.fonts(|f| f.families().contains(&font.family)) {
                                font
                            } else {
                                egui::FontId::proportional(14.0)
                            };
                            // Wrapped to the cell and clipped to it: a label
                            // never runs into the next step (UI review 2).
                            let wrap = (rect.width() - 46.0 - 8.0).max(20.0);
                            let galley =
                                ui.fonts_mut(|f| f.layout(label, font, tokens.text_primary, wrap));
                            let pos = egui::pos2(
                                rect.left() + 46.0,
                                rect.center().y - galley.size().y / 2.0,
                            );
                            ui.painter().with_clip_rect(rect).galley(
                                pos,
                                galley,
                                tokens.text_primary,
                            );
                        }
                    });
                    if row.len() == per_row && per_row == 2 && row[0].0 == Step::Choose {
                        let r = ui.min_rect();
                        ui.painter().hline(
                            r.x_range(),
                            r.bottom(),
                            Stroke::new(1.0, tokens.border_default),
                        );
                    }
                }
            });
    }

    // ---- Step 1 --------------------------------------------------------------

    fn choose_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("bankimport.choose.heading"));
            form::hint(ui, tokens, &t("bankimport.choose.hint"));
            ui.add_space(6.0);
            let busy = self.reading.is_some() || self.pick_rx.is_some();
            if ui
                .add_enabled_ui(!busy, |ui| {
                    crate::widgets::primary_button(ui, &t("bankimport.choose.browse"))
                })
                .inner
                .clicked()
            {
                self.browse(ui.ctx());
            }
            if self.reading.is_some() {
                form::progress_line(ui, tokens, &t("bankimport.reading"));
            }
            if let Some(e) = &self.read_error {
                let (_, line) = e.key();
                let text = match line {
                    Some(l) => t_with(
                        "bankimport.error.at_line",
                        &[("line", &l.to_string()), ("problem", &e.text())],
                    ),
                    None => e.text(),
                };
                ui.label(RichText::new(text).color(tokens.warn_text));
            }
        });
        if self.loaded.is_some() {
            ui.add_space(10.0);
            self.file_card(ui, tokens);
            self.problems_ui(ui, tokens);
        }
    }

    fn file_card(&self, ui: &mut Ui, tokens: &Tokens) {
        let Some(l) = self.loaded.as_ref() else {
            return;
        };
        let section = l.parsed.sections.get(self.section);
        titled_card(ui, tokens, &t("bankimport.card.file"), |ui| {
            pair(ui, tokens, &t("bankimport.file.name"), &l.file_name, true);
            pair(
                ui,
                tokens,
                &t("bankimport.file.format"),
                &t_with("bankimport.file.detected", &[("format", &format_label(l))]),
                false,
            );
            if let Some(s) = section {
                let dates = match (s.start, s.end) {
                    (Some(a), Some(b)) => {
                        format!("{} \u{2013} {}", short_date(a), crate::shell::us_date(b))
                    }
                    _ => String::new(),
                };
                pair(ui, tokens, &t("bankimport.file.dates"), &dates, true);
                if let Some(id) = &s.account {
                    pair(ui, tokens, &t("bankimport.file.account"), &id.mask(), true);
                }
            }
        });
    }

    fn problems_ui(&self, ui: &mut Ui, tokens: &Tokens) {
        let Some(s) = self.current_section() else {
            return;
        };
        let of = |reason: SkipReason| {
            s.skipped
                .iter()
                .filter(|k| k.reason == reason)
                .map(|k| k.line.to_string())
                .collect::<Vec<_>>()
        };
        let zero = of(SkipReason::ZeroAmount);
        if !zero.is_empty() {
            form::hint(
                ui,
                tokens,
                &t_count("bankimport.skipped_zero", zero.len(), &[]),
            );
        }
        // Review DS4: lines before the table are listed, not hidden.
        let preamble = of(SkipReason::Preamble);
        if !preamble.is_empty() {
            form::hint(
                ui,
                tokens,
                &t_count(
                    "bankimport.skipped_preamble",
                    preamble.len(),
                    &[("lines", &preamble.join(", "))],
                ),
            );
        }
        if !of(SkipReason::OpeningBalance).is_empty() {
            form::hint(ui, tokens, &t("bankimport.skipped_opening"));
        }
        for p in s.problems.iter().take(50) {
            ui.label(
                RichText::new(t_with(
                    "bankimport.error.at_line",
                    &[
                        ("line", &p.line.to_string()),
                        ("problem", &t(flow::problem_key(p.kind))),
                    ],
                ))
                .color(tokens.warn_text),
            );
        }
    }

    // ---- Step 2 --------------------------------------------------------------

    fn link_ui(&mut self, ui: &mut Ui, tokens: &Tokens, style: MoneyStyle) {
        let Some(sections) = self.loaded.as_ref().map(|l| l.parsed.sections.clone()) else {
            return;
        };
        if sections.len() > 1 {
            form::card(ui, tokens, |ui| {
                form::heading(ui, tokens, &t("bankimport.section.heading"));
                form::hint(ui, tokens, &t("bankimport.section.hint"));
                let label = |s: &Section| {
                    let mut text = t(kind_name(s.kind));
                    if let Some(id) = &s.account {
                        text.push(' ');
                        text.push_str(&id.mask());
                    }
                    if let Some(n) = &s.account_name {
                        text.push_str(" \u{b7} ");
                        text.push_str(n);
                    }
                    text.push_str(&t_count("bankimport.section.rows", s.rows.len(), &[]));
                    text
                };
                let mut picked = None;
                ui.horizontal(|ui| {
                    let l =
                        ui.add(egui::Label::new(t("bankimport.section.label")).selectable(false));
                    egui::ComboBox::from_id_salt("bankimport/section")
                        .width(360.0)
                        .selected_text(sections.get(self.section).map(label).unwrap_or_default())
                        .show_ui(ui, |ui| {
                            for s in &sections {
                                if ui
                                    .selectable_label(s.index == self.section, label(s))
                                    .clicked()
                                {
                                    picked = Some(s.index);
                                }
                            }
                        })
                        .response
                        .labelled_by(l.id);
                });
                if let Some(i) = picked {
                    self.pick_section(i);
                }
            });
            ui.add_space(10.0);
        }
        self.account_card(ui, tokens);
        if self.csv_mapping.is_some() {
            ui.add_space(10.0);
            self.mapping_card(ui, tokens);
        }
        ui.add_space(10.0);
        self.sign_card(ui, tokens, style);
        if self.reading.is_some() {
            form::progress_line(ui, tokens, &t("bankimport.reading"));
        }
        self.problems_ui(ui, tokens);
    }

    fn account_card(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("bankimport.card.goes_into"));
            let shown = self
                .targets
                .iter()
                .find(|a| Some(a.id) == self.account)
                .map_or_else(|| t("bankimport.account.none"), |a| a.name.clone());
            let mut picked = None;
            ui.horizontal(|ui| {
                let l = ui.add(egui::Label::new(t("bankimport.account.label")).selectable(false));
                egui::ComboBox::from_id_salt("bankimport/account")
                    .width(320.0)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for a in &self.targets {
                            let text = if a.card {
                                t_with("bankimport.account.card", &[("name", &a.name)])
                            } else {
                                a.name.clone()
                            };
                            if ui
                                .selectable_label(Some(a.id) == self.account, text)
                                .clicked()
                            {
                                picked = Some(a.id);
                            }
                        }
                    })
                    .response
                    .labelled_by(l.id);
            });
            if let Some(id) = picked {
                self.pick_account(id);
            }
            if self.targets_task.is_some() {
                form::progress_line(ui, tokens, &t("bankimport.account.loading"));
            }
            if self.targets_failed {
                form::inline_warning(ui, tokens, &t("bankimport.account.failed"));
            }
            match self.linked {
                LinkedHow::Auto => form::hint(ui, tokens, &t("bankimport.linked.auto")),
                LinkedHow::Mask => form::inline_warning(ui, tokens, &t("bankimport.linked.mask")),
                LinkedHow::Owner | LinkedHow::None => {
                    form::hint(ui, tokens, &t("bankimport.linked.pick"));
                }
            }
        });
    }

    fn mapping_card(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(mut m) = self.csv_mapping.clone() else {
            return;
        };
        let names: Vec<String> = self
            .loaded
            .as_ref()
            .and_then(|l| l.parsed.csv_headers.clone())
            .unwrap_or_default();
        let name = |i: usize| {
            names
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("{}", i + 1))
        };
        let before = m.clone();
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("bankimport.csv.heading"));
            form::hint(ui, tokens, &t("bankimport.csv.hint"));
            let column = |ui: &mut Ui, id: &str, label: &str, value: &mut usize| {
                ui.horizontal(|ui| {
                    let l = ui.add(egui::Label::new(label).selectable(false));
                    egui::ComboBox::from_id_salt(id)
                        .width(220.0)
                        .selected_text(name(*value))
                        .show_ui(ui, |ui| {
                            for i in 0..names.len().max(1) {
                                ui.selectable_value(value, i, name(i));
                            }
                        })
                        .response
                        .labelled_by(l.id);
                });
            };
            column(
                ui,
                "bankimport/csv/date",
                &t("bankimport.csv.date"),
                &mut m.date,
            );
            let mut desc = m.description.first().copied().unwrap_or(0);
            column(
                ui,
                "bankimport/csv/desc",
                &t("bankimport.csv.description"),
                &mut desc,
            );
            if m.description.first() != Some(&desc) {
                m.description = vec![desc];
            }
            match &mut m.amount {
                AmountColumns::Signed { column: c } => {
                    column(ui, "bankimport/csv/amount", &t("bankimport.csv.amount"), c);
                }
                AmountColumns::DebitCredit { debit, credit } => {
                    column(
                        ui,
                        "bankimport/csv/debit",
                        &t("bankimport.csv.debit"),
                        debit,
                    );
                    column(
                        ui,
                        "bankimport/csv/credit",
                        &t("bankimport.csv.credit"),
                        credit,
                    );
                }
                AmountColumns::Indicator { amount, indicator } => {
                    column(
                        ui,
                        "bankimport/csv/amount",
                        &t("bankimport.csv.amount"),
                        amount,
                    );
                    column(
                        ui,
                        "bankimport/csv/indicator",
                        &t("bankimport.csv.indicator"),
                        indicator,
                    );
                }
            }
            ui.horizontal(|ui| {
                let l = ui.add(egui::Label::new(t("bankimport.csv.date_order")).selectable(false));
                let text = |o: DateOrder| {
                    t(match o {
                        DateOrder::Mdy => "bankimport.csv.mdy",
                        DateOrder::Dmy => "bankimport.csv.dmy",
                        DateOrder::Ymd => "bankimport.csv.ymd",
                    })
                };
                egui::ComboBox::from_id_salt("bankimport/csv/order")
                    .width(220.0)
                    .selected_text(text(m.date_order))
                    .show_ui(ui, |ui| {
                        for o in [DateOrder::Mdy, DateOrder::Dmy, DateOrder::Ymd] {
                            ui.selectable_value(&mut m.date_order, o, text(o));
                        }
                    })
                    .response
                    .labelled_by(l.id);
            });
            if let Some(dc) = self.current_section().and_then(|s| s.dates) {
                if let Some(line) = dc.day_first_line {
                    form::inline_warning(
                        ui,
                        tokens,
                        &t_with("bankimport.csv.day_first", &[("line", &line.to_string())]),
                    );
                } else if dc.ambiguous {
                    form::hint(ui, tokens, &t("bankimport.csv.ambiguous"));
                }
            }
        });
        if m != before {
            self.set_mapping(m);
        }
    }

    fn sign_card(&mut self, ui: &mut Ui, tokens: &Tokens, style: MoneyStyle) {
        let Some(s) = self.current_section().cloned() else {
            return;
        };
        let needs = self.sign_needs_confirm();
        let card = s.kind == AccountKind::CreditCard || self.card_target();
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("bankimport.sign.heading"));
            let key = if self.sign.suggest_flip {
                "bankimport.sign.suggest"
            } else if card {
                "bankimport.sign.card"
            } else {
                "bankimport.sign.plain"
            };
            form::hint(ui, tokens, &t(key));
            if self.remembered_flip.is_some() {
                form::hint(ui, tokens, &t("bankimport.sign.remembered"));
            }
            egui::Grid::new("bankimport/sign/sample")
                .spacing([16.0, 4.0])
                .show(ui, |ui| {
                    for r in s.rows.iter().take(3) {
                        ui.label(crate::shell::us_date(r.date));
                        // Q-P25r: a row with no name shows its memo.
                        match r.memo.as_deref() {
                            Some(memo) if r.description.is_empty() => {
                                ui.label(RichText::new(memo).color(tokens.text_secondary));
                            }
                            _ => {
                                ui.label(&r.description);
                            }
                        }
                        let m = flow::shown(r.file_amount, self.flipped)
                            .map(|m| m.format(style))
                            .unwrap_or_default();
                        ui.label(
                            RichText::new(format!("{m:>14}"))
                                .text_style(egui::TextStyle::Monospace),
                        );
                        ui.end_row();
                    }
                });
            let mut flipped = self.flipped;
            if ui
                .checkbox(&mut flipped, t("bankimport.sign.flip"))
                .changed()
            {
                self.set_flipped(flipped);
            }
            if needs && ui.button(t("bankimport.sign.looks_right")).clicked() {
                self.set_flipped(self.flipped);
            }
        });
    }

    // ---- Step 3 --------------------------------------------------------------

    fn cards_row(&mut self, ui: &mut Ui, tokens: &Tokens, style: MoneyStyle) {
        let wide = ui.available_width() >= 760.0;
        let third = (ui.available_width() - 28.0) / 3.0;
        let mut change = false;
        let mut cards = |ui: &mut Ui, me: &mut Self| {
            let width = if wide { third } else { ui.available_width() };
            ui.allocate_ui_with_layout(
                egui::vec2(width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| me.file_card(ui, tokens),
            );
            ui.allocate_ui_with_layout(
                egui::vec2(width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    titled_card(ui, tokens, &t("bankimport.card.goes_into"), |ui| {
                        let name = me
                            .targets
                            .iter()
                            .find(|a| Some(a.id) == me.account)
                            .map(|a| a.name.clone())
                            .unwrap_or_default();
                        ui.label(RichText::new(name).strong().color(tokens.text_primary));
                        let key = match me.linked {
                            LinkedHow::Auto => "bankimport.linked.auto",
                            LinkedHow::Mask => "bankimport.linked.mask",
                            LinkedHow::Owner | LinkedHow::None => "bankimport.linked.owner",
                        };
                        ui.add(
                            egui::Label::new(RichText::new(t(key)).color(tokens.text_secondary))
                                .wrap(),
                        );
                        if ui.link(t("bankimport.change_account")).clicked() {
                            change = true;
                        }
                    });
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    me.balance_card(ui, tokens, style);
                },
            );
        };
        if wide {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                cards(ui, self);
            });
        } else {
            ui.vertical(|ui| cards(ui, self));
        }
        if change {
            self.step = Step::Link;
        }
    }

    fn balance_card(&self, ui: &mut Ui, tokens: &Tokens, style: MoneyStyle) {
        titled_card(ui, tokens, &t("bankimport.card.balance"), |ui| {
            let Some(p) = self.preview.as_ref() else {
                return;
            };
            match p.balance_check.as_ref() {
                Some(b) => {
                    pair(
                        ui,
                        tokens,
                        &t_with(
                            "bankimport.balance.bank_says",
                            &[("date", &short_date(b.file_date))],
                        ),
                        &b.file_balance.format(style),
                        true,
                    );
                    pair(
                        ui,
                        tokens,
                        &t_with(
                            "bankimport.balance.books_will",
                            &[("n", &p.counts.new.to_string())],
                        ),
                        &b.projected.format(style),
                        true,
                    );
                    let ok = b.difference.is_zero();
                    let color = if ok { tokens.accent } else { tokens.warn_text };
                    let mut diff = b.difference.format(style);
                    if ok {
                        diff.push_str(" \u{2713}");
                    }
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(t("bankimport.balance.difference"))
                                .strong()
                                .color(color),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(diff)
                                    .strong()
                                    .text_style(egui::TextStyle::Monospace)
                                    .color(color),
                            );
                        });
                    });
                    if b.likely_in_books_count > 0 {
                        form::hint(
                            ui,
                            tokens,
                            &t_with(
                                "bankimport.balance.likely",
                                &[
                                    ("n", &b.likely_in_books_count.to_string()),
                                    ("total", &b.likely_in_books_total.format(style)),
                                ],
                            ),
                        );
                    }
                }
                None if self.link.is_demo() => {
                    form::hint(ui, tokens, &t("bankimport.balance.demo"));
                }
                None => form::hint(ui, tokens, &t("bankimport.balance.none")),
            }
        });
    }

    fn check_ui(&mut self, ui: &mut Ui, tokens: &Tokens, style: MoneyStyle) {
        self.cards_row(ui, tokens, style);
        ui.add_space(12.0);
        if self.preview_task.is_some() {
            form::progress_line(ui, tokens, &t("bankimport.checking"));
        }
        if let Some(tr) = self.preview_trouble.clone() {
            let again = ui
                .horizontal_wrapped(|ui| {
                    ui.label(RichText::new(trouble_text(&tr)).color(tokens.warn_text));
                    ui.button(t("bankimport.try_again")).clicked()
                })
                .inner;
            if again {
                self.run_preview();
            }
        }
        let Some(p) = self.preview.clone() else {
            return;
        };
        if let Some(l) = &p.identity_linked_elsewhere {
            form::inline_warning(
                ui,
                tokens,
                &t_with(
                    "bankimport.linked_elsewhere",
                    &[("mask", &l.mask), ("account", &l.account_name)],
                ),
            );
            let mut relink = self.relink;
            if ui.checkbox(&mut relink, t("bankimport.relink")).changed() {
                self.relink = relink;
            }
        }
        if p.same_file_imported_at.is_some() {
            form::hint(ui, tokens, &t("bankimport.same_file"));
        }
        let problems = self.current_section().map_or(0, |s| s.problems.len());
        if p.counts.new == 0 && p.counts.possible_duplicate == 0 && p.counts.changed == 0 {
            form::inline_warning(
                ui,
                tokens,
                &t_with(
                    "bankimport.nothing_new",
                    &[("n", &p.counts.duplicate.to_string())],
                ),
            );
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(t_count(
                    "bankimport.in_file",
                    usize::try_from(p.counts.in_file).unwrap_or(usize::MAX),
                    &[],
                ))
                .strong()
                .color(tokens.text_primary),
            );
            chip(
                ui,
                tokens,
                &t_with("bankimport.chip.new", &[("n", &p.counts.new.to_string())]),
                true,
            );
            chip(
                ui,
                tokens,
                &t_with(
                    "bankimport.chip.skipped",
                    &[("n", &p.counts.duplicate.to_string())],
                ),
                false,
            );
            if p.counts.possible_duplicate > 0 {
                chip(
                    ui,
                    tokens,
                    &t_count(
                        "bankimport.chip.possible",
                        usize::try_from(p.counts.possible_duplicate).unwrap_or(usize::MAX),
                        &[],
                    ),
                    false,
                );
            }
            if p.counts.changed > 0 {
                chip(
                    ui,
                    tokens,
                    &t_with(
                        "bankimport.chip.changed",
                        &[("n", &p.counts.changed.to_string())],
                    ),
                    false,
                );
            }
            if p.counts.closed_period > 0 {
                chip_with_tip(
                    ui,
                    tokens,
                    &t_with(
                        "bankimport.chip.closed",
                        &[("n", &p.counts.closed_period.to_string())],
                    ),
                    &t("bankimport.status.closed.tip"),
                );
            }
            chip(
                ui,
                tokens,
                &t_count("bankimport.chip.problems", problems, &[]),
                false,
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.checkbox(&mut self.only_skipped, t("bankimport.only_skipped"));
            });
        });
        self.problems_ui(ui, tokens);
        ui.add_space(6.0);
        let choice = self.table_ui(ui, tokens, style, &p);
        if let Some((line, c)) = choice {
            self.choose(line, c);
        }
        if let Some(tr) = &self.send_trouble {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(trouble_text(tr)).color(tokens.warn_text));
            });
        }
    }

    fn table_ui(
        &self,
        ui: &mut Ui,
        tokens: &Tokens,
        style: MoneyStyle,
        p: &BankImportOutcome,
    ) -> Option<(u32, RowChoice)> {
        let section = self.current_section()?;
        let results: BTreeMap<u32, &ImportRowResult> = p.rows.iter().map(|r| (r.line, r)).collect();
        let mut picked = None;
        let rows: Vec<_> = section
            .rows
            .iter()
            .filter_map(|r| {
                let line = u32::try_from(r.line).ok()?;
                let res = results.get(&line).copied();
                // UI review 5: only the rows that are not added. Look-alikes
                // and changed rows are added (Q-P25c, Q-P25x).
                let skipped = res.is_some_and(|x| x.status == ImportRowStatus::Duplicate);
                (!self.only_skipped || skipped).then_some((line, r, res))
            })
            .collect();
        let total = rows.len();
        // The mockup's shaded rows: already imported, not sent again
        // (grid row 0 is the heading).
        let already: std::collections::BTreeSet<usize> = rows
            .iter()
            .take(SHOWN_ROWS)
            .enumerate()
            .filter(|(_, (_, _, res))| res.is_some_and(|x| x.status == ImportRowStatus::Duplicate))
            .map(|(i, _)| i + 1)
            .collect();
        let shade = tokens.locked_row;
        let rounded = rows
            .iter()
            .take(SHOWN_ROWS)
            .any(|(_, r, _)| r.rounded_from.is_some());
        egui::Frame::new()
            .stroke(Stroke::new(1.0, tokens.border_default))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::horizontal()
                    .id_salt("bankimport/table")
                    .show(ui, |ui| {
                        egui::Grid::new("bankimport/rows")
                            .with_row_color(move |row, _| already.contains(&row).then_some(shade))
                            .spacing([24.0, 6.0])
                            .min_col_width(60.0)
                            // Status, the last column, takes the rest of
                            // the width (UI review: the table fills).
                            .num_columns(5)
                            .show(ui, |ui| {
                                for k in [
                                    "bankimport.col.date",
                                    "bankimport.col.description",
                                    "bankimport.col.check",
                                    "bankimport.col.amount",
                                    "bankimport.col.status",
                                ] {
                                    let text = if k == "bankimport.col.amount" {
                                        format!("{:>14}", t(k))
                                    } else {
                                        t(k)
                                    };
                                    let mut rt = RichText::new(text).color(tokens.text_secondary);
                                    if k == "bankimport.col.amount" {
                                        rt = rt.text_style(egui::TextStyle::Monospace);
                                    }
                                    ui.label(rt);
                                }
                                ui.end_row();
                                for (line, r, res) in rows.iter().take(SHOWN_ROWS) {
                                    let dim =
                                        res.is_some_and(|x| x.status == ImportRowStatus::Duplicate);
                                    let ink = if dim {
                                        tokens.locked_text
                                    } else {
                                        tokens.text_primary
                                    };
                                    let mono = |s: String| {
                                        RichText::new(s)
                                            .text_style(egui::TextStyle::Monospace)
                                            .color(ink)
                                    };
                                    ui.label(mono(crate::shell::us_date(r.date)));
                                    // Q-P25r: the bank's memo is its own
                                    // detail, under the description.
                                    if let Some(memo) = r.memo.as_deref() {
                                        ui.vertical(|ui| {
                                            ui.add(
                                                egui::Label::new(mono(r.description.clone()))
                                                    .extend(),
                                            );
                                            ui.label(
                                                RichText::new(memo)
                                                    .size(12.0)
                                                    .color(tokens.text_secondary),
                                            );
                                        });
                                    } else {
                                        // On the row's line, like the date
                                        // and the amount.
                                        ui.add(
                                            egui::Label::new(mono(r.description.clone())).extend(),
                                        );
                                    }
                                    ui.label(mono(
                                        r.check_number.map(|n| n.to_string()).unwrap_or_default(),
                                    ));
                                    let amount = flow::shown(r.file_amount, self.flipped)
                                        .map(|m| m.format(style))
                                        .unwrap_or_default();
                                    ui.label(mono(format!("{amount:>14}")));
                                    ui.horizontal(|ui| {
                                        // Q-P25q: a sub-cent amount was rounded.
                                        if let Some(orig) = r.rounded_from.as_deref() {
                                            chip(
                                                ui,
                                                tokens,
                                                &t_with(
                                                    "bankimport.status.rounded",
                                                    &[("original", orig)],
                                                ),
                                                false,
                                            );
                                        }
                                        if let Some(c) =
                                            Self::status_ui(ui, tokens, *line, *res, &self.choices)
                                        {
                                            picked = Some((*line, c));
                                        }
                                    });
                                    ui.end_row();
                                }
                            });
                    });
            });
        form::hint(
            ui,
            tokens,
            // UI review 6: rounded rows say so in the footnote too.
            &t_with(
                if rounded {
                    "bankimport.footnote.rounded"
                } else {
                    "bankimport.footnote"
                },
                &[
                    ("shown", &total.min(SHOWN_ROWS).to_string()),
                    ("n", &total.to_string()),
                ],
            ),
        );
        if self.flipped {
            form::hint(ui, tokens, &t("bankimport.footnote.flipped"));
        }
        picked
    }

    fn status_ui(
        ui: &mut Ui,
        tokens: &Tokens,
        line: u32,
        res: Option<&ImportRowResult>,
        choices: &BTreeMap<u32, RowChoice>,
    ) -> Option<RowChoice> {
        let res = res?;
        let current = choices.get(&line).copied().unwrap_or_default();
        let mut out = None;
        match res.status {
            ImportRowStatus::New => {
                ui.label(RichText::new(t("bankimport.status.new")).color(tokens.accent));
            }
            ImportRowStatus::Duplicate => {
                let text = res.existing_imported_at.map_or_else(
                    || t("bankimport.status.imported"),
                    |at| {
                        t_with(
                            "bankimport.status.imported_on",
                            &[("date", &local_date(at))],
                        )
                    },
                );
                chip(ui, tokens, &text, false);
            }
            // Q-P25c (owner 2026-11-07): a look-alike is added with a flag,
            // never skipped; the Bank Feed shows it as a possible duplicate.
            ImportRowStatus::PossibleDuplicate => {
                let text = res.existing_imported_at.map_or_else(
                    || t("bankimport.status.possible"),
                    |at| {
                        t_with(
                            "bankimport.status.possible_on",
                            &[("date", &local_date(at))],
                        )
                    },
                );
                chip(ui, tokens, &text, false);
            }
            ImportRowStatus::Changed => {
                chip(ui, tokens, &t("bankimport.status.changed"), false);
                let can_replace = res.existing_pending == Some(true);
                if ui
                    .add_enabled(
                        can_replace,
                        // Framed: they read as buttons next to the chip.
                        egui::Button::new(t("bankimport.replace"))
                            .selected(current.changed == Some(ChangedAction::Replace)),
                    )
                    .clicked()
                {
                    out = Some(super::changed_choice(ChangedAction::Replace));
                }
                if ui
                    .add(
                        egui::Button::new(t("bankimport.add_as_new"))
                            .selected(current.changed == Some(ChangedAction::Add)),
                    )
                    .clicked()
                {
                    out = Some(super::changed_choice(ChangedAction::Add));
                }
            }
        }
        if res.closed_period {
            chip_with_tip(
                ui,
                tokens,
                &t("bankimport.status.closed"),
                &t("bankimport.status.closed.tip"),
            );
        }
        out
    }

    // ---- Step 4 --------------------------------------------------------------

    fn done_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(r) = self.result.clone() else {
            return;
        };
        form::card(ui, tokens, |ui| {
            // UI review 7: after Undo the heading says what is left.
            let text = if let Some(u) = self.undone {
                t_count(
                    "bankimport.done.undone",
                    usize::try_from(u.removed).unwrap_or(usize::MAX),
                    &[],
                )
            } else if r.counts.inserted == 0 {
                t_with(
                    "bankimport.done.nothing",
                    &[("n", &r.counts.duplicate.to_string())],
                )
            } else if r.counts.duplicate == 0 {
                t_count(
                    "bankimport.done.sent_all",
                    usize::try_from(r.counts.inserted).unwrap_or(usize::MAX),
                    &[],
                )
            } else {
                t_with(
                    "bankimport.done.sent",
                    &[
                        ("n", &r.counts.inserted.to_string()),
                        (
                            "skipped",
                            &t_count(
                                "bankimport.done.already",
                                usize::try_from(r.counts.duplicate).unwrap_or(usize::MAX),
                                &[],
                            ),
                        ),
                    ],
                )
            };
            form::heading(ui, tokens, &text);
            if r.replayed {
                form::hint(ui, tokens, &t("bankimport.done.replayed"));
            }
            match r.post_processing {
                Some(cl_api::ImportPostProcessing::Scheduled) if self.prep_gave_up => {
                    form::hint(ui, tokens, &t("bankimport.done.prep_slow"));
                }
                Some(cl_api::ImportPostProcessing::Scheduled) => {
                    form::progress_line(ui, tokens, &t("bankimport.done.preparing"));
                    form::hint(ui, tokens, &t("bankimport.done.prep_wait"));
                }
                Some(cl_api::ImportPostProcessing::Failed) => {
                    form::hint(ui, tokens, &t("bankimport.done.prep_failed"));
                }
                _ => {}
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if crate::widgets::primary_button(ui, &t("bankimport.open_feed")).clicked() {
                    self.link
                        .push(CompanyEvent::OpenWindow(CommandId::new("banking.feed")));
                }
                let can_undo = r.batch.is_some()
                    && r.counts.inserted > 0
                    && self.undone.is_none()
                    && self.undo_task.is_none()
                    && !self.preparing();
                if ui
                    .add_enabled(can_undo, egui::Button::new(t("bankimport.undo")))
                    .clicked()
                {
                    self.undo(false);
                }
                let more = self.loaded.as_ref().is_some_and(|l| {
                    l.parsed
                        .sections
                        .iter()
                        .any(|s| s.index > self.section && !s.rows.is_empty())
                });
                if more && ui.button(t("bankimport.next_account")).clicked() {
                    self.start_over(true);
                }
                if ui.button(t("bankimport.another_file")).clicked() {
                    self.start_over(false);
                }
            });
            if self.undo_task.is_some() {
                form::progress_line(ui, tokens, &t("bankimport.undoing"));
            }
            if let Some(u) = self.undone
                && u.kept > 0
            {
                form::hint(
                    ui,
                    tokens,
                    &t_count(
                        "bankimport.undone_kept",
                        usize::try_from(u.kept).unwrap_or(usize::MAX),
                        &[],
                    ),
                );
            }
            if let Some(u) = self.undone
                && u.restored > 0
            {
                form::hint(
                    ui,
                    tokens,
                    &t_count(
                        "bankimport.undone_restored",
                        usize::try_from(u.restored).unwrap_or(usize::MAX),
                        &[],
                    ),
                );
            }
            if let Some(tr) = self.undo_trouble.clone() {
                ui.label(RichText::new(trouble_text(&tr)).color(tokens.warn_text));
                if matches!(
                    tr,
                    EngineTrouble::Refused(cl_api::BankImportRefusal::RowsAlreadyUsed(_))
                ) && ui.button(t("bankimport.undo.rest")).clicked()
                {
                    self.undo(true);
                }
            }
        });
    }

    // ---- Buttons ---------------------------------------------------------------

    fn buttons_ui(&mut self, ui: &mut Ui, _tokens: &Tokens) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let primary = match self.step {
                Step::Choose => t("bankimport.next"),
                Step::Link => t("bankimport.to_check"),
                Step::Check => {
                    let n = self
                        .preview
                        .as_ref()
                        .map_or(0, |p| flow::rows_to_send(p, &self.choices));
                    // UI review 8: nothing to add says so (the button is
                    // closed); replacing waiting items only is an update.
                    if n > 0 {
                        t_with("bankimport.send", &[("n", &n.to_string())])
                    } else if self.replacing() > 0 {
                        t_count("bankimport.send_updates", self.replacing(), &[])
                    } else {
                        t("bankimport.send_nothing")
                    }
                }
                Step::Done => t("bankimport.close"),
            };
            let enabled = self.step == Step::Done || self.can_next();
            let r = ui
                .add_enabled_ui(enabled, |ui| crate::widgets::primary_button(ui, &primary))
                .inner;
            let r = if self.step == Step::Done {
                r
            } else {
                r.on_hover_text(shortcut_hint("form.save_new"))
            };
            if r.clicked() {
                if self.step == Step::Done {
                    self.link
                        .push(CompanyEvent::CloseWindow(CommandId::new(super::KIND)));
                } else {
                    self.next();
                }
            }
            if self.step != Step::Done {
                if ui.button(t("bankimport.cancel")).clicked() {
                    self.link
                        .push(CompanyEvent::CloseWindow(CommandId::new(super::KIND)));
                }
                if self.step != Step::Choose
                    && ui
                        .button(t("bankimport.back"))
                        .on_hover_text(shortcut_hint("form.step_back"))
                        .clicked()
                {
                    self.back();
                }
            }
        });
    }
}
