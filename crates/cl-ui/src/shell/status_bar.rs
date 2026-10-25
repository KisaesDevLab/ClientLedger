//! Status bar: who is signed in, the mode, the engine state, the AI line, and
//! context keyboard hints read from the keymap.

use cl_model::commands::{self, CommandId};
use cl_model::{EnterMode, Key, Mods, Shortcut};
use eframe::egui::{self, Align, Frame, Layout, Margin, RichText, Ui};

use super::{HostView, Shell, chip_text};
use crate::i18n::t;
use crate::theme::Tokens;
use crate::widgets::kbd;

/// Status bar height.
pub const HEIGHT: f32 = 26.0;

/// Commands whose shortcut is hinted on the right, with their short labels.
pub const HINTS: [(&str, &str); 4] = [
    ("help.palette", "status.hint.go_to"),
    ("window.next", "status.hint.next_window"),
    ("help.shortcuts", "status.hint.shortcuts"),
    ("help.current", "status.hint.help"),
];

/// The register's hints after Enter and the date keys (mockup
/// Register.html). Shortcuts come from the keymap.
const REGISTER_HINTS: [(&str, &str); 2] = [
    ("form.delete_item", "status.hint.delete"),
    ("edit.edit_transaction", "status.hint.open_form"),
];

/// Write Check's hints after Tab (mockup WriteCheck.html).
const CHECK_HINTS: [(&str, &str); 3] = [
    ("form.insert_line", "status.hint.insert_line"),
    ("edit.open_list", "status.hint.list"),
    ("form.revert", "status.hint.revert"),
];

/// Forms with a name or account list but no lines grid (Ctrl+L, Esc).
const LIST_HINTS: [(&str, &str); 2] = [
    ("edit.open_list", "status.hint.list"),
    ("form.revert", "status.hint.revert"),
];

/// Forms with neither a lines grid nor a name list (Transfer Funds).
const REVERT_HINTS: [(&str, &str); 1] = [("form.revert", "status.hint.revert")];

/// The Templates list's keys (P15): Enter uses the selected template.
const TEMPLATES_HINTS: [(&str, &str); 4] = [
    ("list.new", "status.hint.new_group"),
    ("edit.edit_transaction", "status.hint.edit"),
    ("list.delete", "status.hint.delete"),
    ("form.revert", "status.hint.close"),
];

/// Templates Due's keys (P15): Enter does the row's main action.
const TEMPLATES_DUE_HINTS: [(&str, &str); 2] = [
    ("edit.edit_transaction", "status.hint.open_form"),
    ("form.revert", "status.hint.close"),
];

/// The form hints of a window kind: Write Check's for forms with a lines
/// grid, fewer for the others; `None` for other windows.
fn form_hints(kind: &str) -> Option<&'static [(&'static str, &'static str)]> {
    match kind {
        "vendors.write_check"
        | "vendors.enter_bill"
        | "vendors.card_charge"
        | "vendors.personal_funds"
        | "banking.deposit"
        | "banking.loan_payment"
        | "company.journal_entry" => Some(&CHECK_HINTS),
        "banking.owner_money" | "clients.daily_sales" => Some(&LIST_HINTS),
        "banking.transfer" => Some(&REVERT_HINTS),
        _ => None,
    }
}

/// One hint on the right of the bar: its key chips and short label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    /// Key chips, left to right ("Ctrl F").
    pub chips: Vec<String>,
    /// The short label after the chips.
    pub label: String,
}

fn keymap_hints(list: &[(&str, &str)]) -> Vec<Hint> {
    list.iter()
        .map(|(id, label)| Hint {
            chips: commands::find(id)
                .and_then(|c| c.shortcut)
                .map(chip_text)
                .into_iter()
                .collect(),
            label: t(label),
        })
        .collect()
}

/// The hints for the window in front (`None` when no window is open): the
/// register and the transaction forms show their own, every other screen
/// the general ones. When Enter moves between fields (Q-264) the forms
/// also show Ctrl+Enter, their Save & New.
#[must_use]
pub fn hints_for(active: Option<CommandId>, mode: EnterMode) -> Vec<Hint> {
    let mut out = base_hints(active);
    if mode == EnterMode::MoveBetweenFields
        && active.is_some_and(|k| form_hints(k.as_str()).is_some())
    {
        out.insert(
            1,
            Hint {
                chips: vec![chip_text(Shortcut::new(Mods::CMD, Key::Enter))],
                label: t("status.hint.save_new"),
            },
        );
    }
    out
}

fn base_hints(active: Option<CommandId>) -> Vec<Hint> {
    match active.map(CommandId::as_str) {
        Some("banking.register") => {}
        Some(kind @ ("lists.templates" | "lists.templates_due")) => {
            let (enter, rest): (&str, &[(&str, &str)]) = if kind == "lists.templates" {
                ("status.hint.use", &TEMPLATES_HINTS)
            } else {
                ("status.hint.enter_or_open", &TEMPLATES_DUE_HINTS)
            };
            let mut out = vec![Hint {
                chips: vec!["Enter".to_string()],
                label: t(enter),
            }];
            out.extend(keymap_hints(rest));
            return out;
        }
        Some(kind) if form_hints(kind).is_some() => {
            let mut out = vec![Hint {
                chips: vec!["Tab".to_string()],
                label: t("status.hint.next_field"),
            }];
            out.extend(keymap_hints(form_hints(kind).unwrap_or_default()));
            return out;
        }
        _ => return keymap_hints(&HINTS),
    }
    let mut out = vec![
        Hint {
            chips: vec!["Enter".to_string()],
            label: t("status.hint.record"),
        },
        Hint {
            chips: ["+", "\u{2212}", "T"].map(str::to_string).to_vec(),
            label: t("status.hint.dates"),
        },
    ];
    out.extend(keymap_hints(&REGISTER_HINTS));
    out
}

/// The engine text.
#[must_use]
pub fn engine_text(view: &HostView) -> Option<String> {
    match view {
        HostView::Unavailable => None,
        HostView::Starting => Some(t("status.engine.starting")),
        HostView::Running => Some(t("status.engine.running")),
        HostView::Restarting => Some(t("status.engine.restarting")),
        HostView::Failed(reason) => Some(format!("{} {reason}", t("status.engine.failed"))),
        HostView::Stopped => Some(t("status.engine.stopped")),
    }
}

/// The "template entries need you" chip text (P15), when any dates wait
/// (it counts dates, as Templates Due lists them).
#[must_use]
pub fn templates_chip(n: usize) -> Option<String> {
    match n {
        0 => None,
        1 => Some(t("status.templates_need_you.one")),
        n => Some(crate::i18n::t_with(
            "status.templates_need_you",
            &[("n", &n.to_string())],
        )),
    }
}

/// Draw the bar. True when the owner clicked the "templates need you"
/// chip (the shell opens Templates Due).
pub fn show(ui: &mut Ui, shell: &Shell, narrow: bool, tokens: &Tokens) -> bool {
    let engine = engine_text(&shell.host_view());
    let chip = templates_chip(shell.templates_need_you());
    let mut chip_clicked = false;
    egui::Panel::bottom("status_bar")
        .exact_size(HEIGHT)
        .frame(
            Frame::new()
                .fill(tokens.surface_panel)
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                let small = |s: String| RichText::new(s).size(12.0).color(tokens.text_secondary);
                let user = shell
                    .status
                    .user
                    .clone()
                    .unwrap_or_else(|| t("status.not_signed_in"));
                ui.label(small(user));
                if let Some(mode) = &shell.status.mode {
                    ui.label(small(mode.clone()));
                }
                if let Some(engine) = engine {
                    ui.label(small(engine));
                }
                if let Some(ai) = &shell.status.ai
                    && !narrow
                {
                    ui.label(small(ai.clone()));
                }
                if let Some(chip) = &chip
                    && ui
                        .link(RichText::new(chip).size(12.0).color(tokens.warn_text))
                        .on_hover_text(t("status.templates_need_you.tip"))
                        .clicked()
                {
                    chip_clicked = true;
                }
                if let Some(toast) = shell.toast_text() {
                    // Leave room for the shortcut chips on the right so the
                    // text never runs under them.
                    let room = if narrow {
                        ui.available_width()
                    } else {
                        (ui.available_width() - 460.0).max(0.0)
                    };
                    ui.scope(|ui| {
                        ui.set_max_width(room);
                        ui.add(
                            egui::Label::new(
                                RichText::new(toast)
                                    .size(12.0)
                                    .color(tokens.accent)
                                    .strong(),
                            )
                            .truncate(),
                        );
                    });
                }
                if !narrow {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let active = shell.windows.active().map(|w| w.kind);
                        for hint in hints_for(active, shell.enter_mode()).into_iter().rev() {
                            ui.label(small(hint.label));
                            for chip in hint.chips.iter().rev() {
                                kbd(ui, chip);
                            }
                            ui.add_space(8.0);
                        }
                    });
                }
            });
        });
    chip_clicked
}
