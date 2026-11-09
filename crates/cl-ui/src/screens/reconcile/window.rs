//! The Reconcile window (step 2), per the `Reconcile` mockup: header,
//! the bank-file banner (Q-100), "Hide transactions after the statement
//! date" (Q-P27c), two panes, the summary and difference cards, Reconcile
//! now only at 0.00.
//!
//! Every figure is the engine's; the panes count rows but never add.

use cl_api::{HeldCharge, ReconLine};
use cl_core::ids::JournalLineId;
use cl_model::reconcile::{Pane, ReconKind, can_finish, display, hint};
use eframe::egui::{self, Color32, RichText, Sense, Stroke, Ui, Vec2};

use super::ReconcileScreen;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;

const ROW_H: f32 = 32.0;

/// The hint under the difference ("One unchecked item equals …"), if any.
pub(crate) fn hint_text(s: &ReconcileScreen) -> Option<String> {
    let rec = s.rec.as_ref()?;
    let unchecked: Vec<&ReconLine> = s
        .shown()
        .filter(|l| !s.queue.ticked(l.journal_line))
        .collect();
    let id = hint(
        unchecked.iter().map(|l| (l.journal_line, l.amount)),
        rec.difference,
    )?;
    let l = unchecked.iter().find(|l| l.journal_line == id)?;
    let payee = l.payee.clone().unwrap_or_default();
    Some(match (&l.number, l.txn_type.as_str()) {
        (Some(n), "check") => t_with("reconcile.hint.check", &[("number", n), ("payee", &payee)]),
        _ => t_with(
            "reconcile.hint.other",
            &[("payee", &payee), ("date", &short_date(l.date))],
        ),
    })
}

fn short_date(d: cl_core::CalendarDate) -> String {
    let full = crate::shell::us_date(d);
    full.get(..5).unwrap_or(&full).to_string()
}

fn mono(text: &str, color: Color32) -> RichText {
    RichText::new(text).monospace().color(color)
}

/// Draws the window.
pub(crate) fn draw(s: &mut ReconcileScreen, ui: &mut Ui) {
    let tokens = Tokens::of(ui.ctx());
    let Some(rec) = s.rec.clone() else {
        return;
    };
    let kind = s.kind();
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(20, 16))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 12.0;
            header(s, ui, &tokens, &rec, kind);
            banner(ui, &tokens);
            if let Some(d) = rec.drift {
                form::inline_warning(
                    ui,
                    &tokens,
                    &t_with(
                        "reconcile.drift",
                        &[("amount", &display(kind.shown(d.difference)))],
                    ),
                );
            }
            if let Some(c) = rec.continuity {
                form::inline_warning(
                    ui,
                    &tokens,
                    &t_with(
                        "reconcile.continuity",
                        &[("amount", &display(kind.shown(c.delta)))],
                    ),
                );
            }
            if let Some(n) = s.notice {
                form::hint(ui, &tokens, &t(n));
            }
            if let Some(e) = &s.error {
                form::inline_error(ui, &tokens, e);
                if ui.button(t("reconcile.retry")).clicked() {
                    s.error = None;
                    let id = rec.id;
                    s.refresh_after_error(id);
                }
            }
            controls(s, ui);
            let bottom = 150.0;
            let pane_h = (ui.available_height() - bottom).max(160.0);
            ui.allocate_ui(Vec2::new(ui.available_width(), pane_h), |ui| {
                panes(s, ui, &tokens, &rec, kind, pane_h);
            });
            cards(s, ui, &tokens, &rec, kind);
            ui.horizontal(|ui| {
                for (k, what) in [
                    ("Space", "reconcile.keys.check"),
                    ("Tab", "reconcile.keys.pane"),
                    ("Ctrl+E", "reconcile.keys.open"),
                ] {
                    ui.label(mono(k, tokens.text_secondary).size(11.0));
                    ui.label(
                        RichText::new(t(what))
                            .size(12.0)
                            .color(tokens.text_secondary),
                    );
                    ui.add_space(10.0);
                }
            });
        });
}

fn header(
    s: &mut ReconcileScreen,
    ui: &mut Ui,
    tokens: &Tokens,
    rec: &cl_api::Reconciliation,
    kind: ReconKind,
) {
    ui.horizontal_wrapped(|ui| {
        let acct = s.account_text(rec.account);
        ui.label(
            RichText::new(t_with("reconcile.title", &[("account", &acct)]))
                .size(22.0)
                .strong(),
        );
        ui.add_space(12.0);
        ui.label(RichText::new(t("reconcile.statement_ending")).color(tokens.text_secondary));
        ui.label(mono(
            &crate::shell::us_date(rec.statement_date),
            tokens.text_primary,
        ));
        ui.add_space(8.0);
        ui.label(RichText::new(t("reconcile.ending_balance")).color(tokens.text_secondary));
        ui.label(mono(
            &display(kind.shown(rec.statement_ending_balance)),
            tokens.text_primary,
        ));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(t("reconcile.change_info")).clicked() {
                s.change_info();
            }
        });
    });
}

fn banner(ui: &mut Ui, tokens: &Tokens) {
    egui::Frame::new()
        .fill(tokens.info_tint)
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.5, tokens.info);
                ui.label(RichText::new(t("reconcile.banner")).color(tokens.info));
            });
        });
}

fn controls(s: &mut ReconcileScreen, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut s.hide_later, t("reconcile.hide_later"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(t("reconcile.goto")).clicked()
                && let Some(a) = s.form.account
            {
                s.link.push(crate::company::CompanyEvent::OpenRegister(a));
            }
            if ui.button(t("reconcile.unmark_all")).clicked() {
                s.mark_all(false);
            }
            if ui.button(t("reconcile.mark_all")).clicked() {
                s.mark_all(true);
            }
        });
    });
}

/// A pending charge row in a pane ("INT/SVC … (from statement info)").
struct PendingRow {
    code: &'static str,
    label: String,
    held: HeldCharge,
}

fn pending_rows(rec: &cl_api::Reconciliation, kind: ReconKind, p: Pane) -> Vec<PendingRow> {
    let mut out = Vec::new();
    if p == Pane::MoneyIn
        && let Some(h) = rec.interest.filter(|h| h.posted.is_none())
    {
        out.push(PendingRow {
            code: "INT",
            label: t("reconcile.pending.interest"),
            held: h,
        });
    }
    if p == Pane::MoneyOut
        && let Some(h) = rec.service_charge.filter(|h| h.posted.is_none())
    {
        let label = if kind == ReconKind::Card {
            t("reconcile.pending.finance")
        } else {
            t("reconcile.pending.service")
        };
        out.push(PendingRow {
            code: if kind == ReconKind::Card {
                "FIN"
            } else {
                "SVC"
            },
            label,
            held: h,
        });
    }
    out
}

fn panes(
    s: &mut ReconcileScreen,
    ui: &mut Ui,
    tokens: &Tokens,
    rec: &cl_api::Reconciliation,
    kind: ReconKind,
    height: f32,
) {
    let hinted = {
        let unchecked: Vec<(JournalLineId, cl_core::Money)> = s
            .shown()
            .filter(|l| !s.queue.ticked(l.journal_line))
            .map(|l| (l.journal_line, l.amount))
            .collect();
        hint(unchecked, rec.difference)
    };
    let gap = 14.0;
    let w = ((ui.available_width() - gap) / 2.0).max(300.0);
    ui.horizontal_top(|ui| {
        for p in [kind.left(), kind.right()] {
            ui.allocate_ui(Vec2::new(w, height), |ui| {
                pane(s, ui, tokens, rec, kind, p, hinted);
            });
            ui.add_space(gap - ui.spacing().item_spacing.x);
        }
    });
}

#[allow(clippy::too_many_lines)]
fn pane(
    s: &mut ReconcileScreen,
    ui: &mut Ui,
    tokens: &Tokens,
    rec: &cl_api::Reconciliation,
    kind: ReconKind,
    p: Pane,
    hinted: Option<JournalLineId>,
) {
    let lines: Vec<ReconLine> = s.pane_lines(p).into_iter().cloned().collect();
    let pending = pending_rows(rec, kind, p);
    let side = match p {
        Pane::MoneyIn => rec.cleared_deposits,
        Pane::MoneyOut => rec.cleared_payments,
    };
    let focused = s.pane == p;
    egui::Frame::new()
        .stroke(Stroke::new(
            1.0,
            if focused {
                tokens.accent_border
            } else {
                tokens.border_default
            },
        ))
        .corner_radius(6.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(ui.available_height());
            ui.spacing_mut().item_spacing.y = 0.0;
            // Pane header: title and "n of m · total" from the engine.
            egui::Frame::new()
                .fill(tokens.surface_subtle)
                .inner_margin(egui::Margin::symmetric(12, 9))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(t(kind.pane_title(p))).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let n = lines.len() + pending.len();
                            ui.label(mono(
                                &t_with(
                                    "reconcile.pane.count",
                                    &[
                                        ("n", &side.count.to_string()),
                                        ("m", &n.to_string()),
                                        ("total", &display(side.total)),
                                    ],
                                ),
                                tokens.text_primary,
                            ));
                        });
                    });
                });
            ui.separator();
            let cols = column_widths(ui.available_width());
            header_row(ui, tokens, &cols);
            if lines.is_empty() && pending.is_empty() {
                ui.add_space(12.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(t("reconcile.pane.empty")).color(tokens.text_secondary));
                });
                return;
            }
            let rows = lines.len() + pending.len();
            egui::ScrollArea::vertical()
                .id_salt(("reconcile_pane", p))
                .auto_shrink([false, false])
                .show_rows(ui, ROW_H, rows, |ui, range| {
                    for i in range {
                        if let Some(l) = lines.get(i) {
                            let on = s.queue.ticked(l.journal_line);
                            let cursor = s.cursor == Some(l.journal_line) && focused;
                            if line_row(
                                ui,
                                tokens,
                                &cols,
                                l,
                                on,
                                hinted == Some(l.journal_line),
                                cursor,
                            ) {
                                s.pane = p;
                                s.toggle(l.journal_line);
                            }
                        } else if let Some(pr) = pending.get(i - lines.len()) {
                            pending_row(ui, tokens, &cols, pr);
                        }
                    }
                });
        });
}

/// Check, Date, Ref, Payee, Amount widths.
fn column_widths(w: f32) -> [f32; 5] {
    let fixed = 34.0 + 64.0 + 56.0 + 100.0;
    [34.0, 64.0, 56.0, (w - fixed - 8.0).max(80.0), 100.0]
}

fn header_row(ui: &mut Ui, tokens: &Tokens, cols: &[f32; 5]) {
    ui.horizontal(|ui| {
        ui.set_height(26.0);
        ui.spacing_mut().item_spacing.x = 0.0;
        let texts = [
            "\u{2713}",
            "reconcile.col.date",
            "reconcile.col.ref",
            "reconcile.col.payee",
            "reconcile.col.amount",
        ];
        for (i, (w, key)) in cols.iter().zip(texts).enumerate() {
            let text = if i == 0 { key.to_string() } else { t(key) };
            let layout = if i == 4 {
                egui::Layout::right_to_left(egui::Align::Center)
            } else {
                egui::Layout::left_to_right(egui::Align::Center)
            };
            ui.allocate_ui_with_layout(Vec2::new(*w, 26.0), layout, |ui| {
                ui.add_space(10.0);
                ui.label(RichText::new(text).size(12.0).color(tokens.text_secondary));
            });
        }
    });
}

/// One line; true when its box was clicked.
fn line_row(
    ui: &mut Ui,
    tokens: &Tokens,
    cols: &[f32; 5],
    l: &ReconLine,
    on: bool,
    hinted: bool,
    cursor: bool,
) -> bool {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::hover());
    let fill = if hinted {
        tokens.warn_surface
    } else if on {
        tokens.surface_form
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, 0.0, fill);
    ui.painter().hline(
        rect.x_range(),
        rect.top(),
        Stroke::new(1.0, tokens.border_row),
    );
    if cursor {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            2.0,
            Stroke::new(1.5, tokens.accent),
            egui::StrokeKind::Inside,
        );
    }
    let mut clicked = false;
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing.x = 0.0;
    let payee = l.payee.clone().unwrap_or_default();
    let number = l.number.clone().unwrap_or_default();
    let label = t_with(
        "reconcile.a11y.cleared",
        &[
            ("date", &short_date(l.date)),
            ("ref", &number),
            ("payee", &payee),
        ],
    );
    ui.allocate_ui_with_layout(
        Vec2::new(cols[0], ROW_H),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add_space(10.0);
            let mut v = on;
            let r = ui.add(egui::Checkbox::without_text(&mut v));
            let r = r.on_hover_text(&label);
            r.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, &label));
            if hinted {
                ui.painter().rect_stroke(
                    r.rect.expand(1.0),
                    2.0,
                    Stroke::new(2.0, tokens.warn),
                    egui::StrokeKind::Outside,
                );
            }
            if r.clicked() {
                clicked = true;
            }
        },
    );
    cell(
        ui,
        cols[1],
        mono(&short_date(l.date), tokens.text_primary),
        false,
    );
    cell(ui, cols[2], mono(&number, tokens.text_primary), false);
    ui.allocate_ui_with_layout(
        Vec2::new(cols[3], ROW_H),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add_space(10.0);
            ui.add(egui::Label::new(RichText::new(&payee).color(tokens.text_primary)).truncate());
            if l.from_bank_feed {
                ui.add_space(6.0);
                let (r, resp) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.5, tokens.info);
                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Label,
                        true,
                        t("reconcile.a11y.from_bank_file"),
                    )
                });
                resp.on_hover_text(t("reconcile.a11y.from_bank_file"));
            }
            if l.likely_duplicate {
                ui.add_space(6.0);
                ui.label(RichText::new("!").strong().color(tokens.warn))
                    .on_hover_text(t("reconcile.likely_duplicate"));
            }
            if l.after_statement_date {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(t("reconcile.after_date"))
                        .size(11.0)
                        .color(tokens.text_secondary),
                );
            }
        },
    );
    cell(
        ui,
        cols[4],
        mono(&display(l.amount.abs()), tokens.text_primary),
        true,
    );
    clicked
}

fn pending_row(ui: &mut Ui, tokens: &Tokens, cols: &[f32; 5], p: &PendingRow) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, tokens.surface_form);
    ui.painter().hline(
        rect.x_range(),
        rect.top(),
        Stroke::new(1.0, tokens.border_row),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let ui = &mut child;
    ui.spacing_mut().item_spacing.x = 0.0;
    ui.allocate_ui_with_layout(
        Vec2::new(cols[0], ROW_H),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add_space(10.0);
            let mut v = true;
            let r = ui.add_enabled(false, egui::Checkbox::without_text(&mut v));
            let label = t_with("reconcile.a11y.pending", &[("what", &p.label)]);
            r.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, false, true, &label));
        },
    );
    cell(
        ui,
        cols[1],
        mono(&short_date(p.held.charge.date), tokens.text_primary),
        false,
    );
    cell(ui, cols[2], mono(p.code, tokens.text_primary), false);
    cell(
        ui,
        cols[3],
        RichText::new(&p.label).color(tokens.text_primary),
        false,
    );
    cell(
        ui,
        cols[4],
        mono(&display(p.held.charge.amount), tokens.text_primary),
        true,
    );
}

fn cell(ui: &mut Ui, w: f32, text: RichText, right: bool) {
    let layout = if right {
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    };
    ui.allocate_ui_with_layout(Vec2::new(w, ROW_H), layout, |ui| {
        ui.add_space(10.0);
        ui.add(egui::Label::new(text).truncate());
    });
}

fn sum_row(ui: &mut Ui, tokens: &Tokens, label: &str, amount: &str, color: Color32, big: bool) {
    ui.horizontal(|ui| {
        let size = if big { 16.0 } else { 13.0 };
        let mut l = RichText::new(label).size(size).color(color);
        let mut a = mono(amount, if big { color } else { tokens.text_primary }).size(size);
        if big {
            l = l.strong();
            a = a.strong();
        }
        ui.label(l);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(a);
        });
    });
}

#[allow(clippy::too_many_lines)]
fn cards(
    s: &mut ReconcileScreen,
    ui: &mut Ui,
    tokens: &Tokens,
    rec: &cl_api::Reconciliation,
    kind: ReconKind,
) {
    let w = ui.available_width();
    let action_w = 260.0;
    let card_w = ((w - action_w - 28.0) / 2.0).max(240.0);
    let h = 120.0;
    ui.horizontal_top(|ui| {
        // Summary.
        ui.allocate_ui(Vec2::new(card_w, h), |ui| {
            egui::Frame::new()
                .fill(tokens.surface_subtle)
                .stroke(Stroke::new(1.0, tokens.border_default))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(h - 24.0);
                    ui.spacing_mut().item_spacing.y = 5.0;
                    let beginning_label = match s.last_finished() {
                        Some(r) => t_with(
                            "reconcile.sum.beginning_on",
                            &[("date", &short_date(r.statement_date))],
                        ),
                        None => t("reconcile.sum.beginning"),
                    };
                    sum_row(
                        ui,
                        tokens,
                        &beginning_label,
                        &display(kind.shown(rec.beginning_balance)),
                        tokens.text_secondary,
                        false,
                    );
                    let (dep_key, pay_key) = match kind {
                        ReconKind::Bank => ("reconcile.sum.deposits", "reconcile.sum.payments"),
                        ReconKind::Card => {
                            ("reconcile.sum.card_payments", "reconcile.sum.card_charges")
                        }
                    };
                    let dep = t_with(dep_key, &[("n", &rec.cleared_deposits.count.to_string())]);
                    let pay = t_with(pay_key, &[("n", &rec.cleared_payments.count.to_string())]);
                    // A side's total as it moves the shown balance.
                    let signed = |m: cl_core::Money, out: bool| {
                        let books = if out { m.checked_neg().unwrap_or(m) } else { m };
                        display(kind.shown(books))
                    };
                    match kind {
                        ReconKind::Bank => {
                            sum_row(
                                ui,
                                tokens,
                                &dep,
                                &signed(rec.cleared_deposits.total, false),
                                tokens.text_secondary,
                                false,
                            );
                            sum_row(
                                ui,
                                tokens,
                                &pay,
                                &signed(rec.cleared_payments.total, true),
                                tokens.text_secondary,
                                false,
                            );
                        }
                        ReconKind::Card => {
                            sum_row(
                                ui,
                                tokens,
                                &pay,
                                &signed(rec.cleared_payments.total, true),
                                tokens.text_secondary,
                                false,
                            );
                            sum_row(
                                ui,
                                tokens,
                                &dep,
                                &signed(rec.cleared_deposits.total, false),
                                tokens.text_secondary,
                                false,
                            );
                        }
                    }
                    ui.separator();
                    sum_row(
                        ui,
                        tokens,
                        &t("reconcile.sum.cleared"),
                        &display(kind.shown(rec.cleared_balance)),
                        tokens.text_secondary,
                        false,
                    );
                });
        });
        // Difference.
        ui.allocate_ui(Vec2::new(card_w, h), |ui| {
            let zero = rec.difference.is_zero();
            let (fill, stroke, text) = if zero {
                (tokens.accent_tint, tokens.accent_border, tokens.accent)
            } else {
                (tokens.warn_surface, tokens.warn_tint, tokens.warn_text)
            };
            egui::Frame::new()
                .fill(fill)
                .stroke(Stroke::new(1.0, stroke))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(h - 24.0);
                    ui.spacing_mut().item_spacing.y = 5.0;
                    sum_row(
                        ui,
                        tokens,
                        &t("reconcile.sum.statement_ending"),
                        &display(kind.shown(rec.statement_ending_balance)),
                        text,
                        false,
                    );
                    sum_row(
                        ui,
                        tokens,
                        &t("reconcile.sum.difference"),
                        &display(kind.shown(rec.difference)),
                        text,
                        true,
                    );
                    if let Some(h) = hint_text(s) {
                        ui.add(egui::Label::new(RichText::new(h).size(12.0).color(text)).wrap());
                    }
                });
        });
        // Actions.
        ui.allocate_ui(Vec2::new(action_w, h), |ui| {
            ui.vertical(|ui| {
                ui.set_width(action_w);
                let ready = can_finish(rec.difference, &s.queue);
                let b = egui::Button::new(
                    RichText::new(t("reconcile.finish"))
                        .strong()
                        .color(tokens.on_accent),
                )
                .fill(if ready {
                    tokens.accent
                } else {
                    tokens.border_strong
                })
                .min_size(Vec2::new(action_w, 44.0));
                if ui.add_enabled(ready, b).clicked() {
                    s.finish();
                }
                ui.horizontal(|ui| {
                    let half = (action_w - 8.0) / 2.0;
                    if ui
                        .add(
                            egui::Button::new(t("reconcile.leave")).min_size(Vec2::new(half, 44.0)),
                        )
                        .clicked()
                    {
                        s.leave();
                    }
                    let r = ui.add(
                        egui::Button::new(t("reconcile.reports")).min_size(Vec2::new(half, 44.0)),
                    );
                    super::reports::menu(s, ui, &r);
                });
                if !ready {
                    ui.add(
                        egui::Label::new(
                            RichText::new(t("reconcile.finish_help"))
                                .size(12.0)
                                .color(tokens.text_tertiary),
                        )
                        .wrap(),
                    );
                }
            });
        });
    });
}
