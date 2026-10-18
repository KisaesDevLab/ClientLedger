//! Building blocks of a list's detail pane and its add/edit panel (P11):
//! labelled read-only rows, section titles, "not available yet" notes for
//! the parts that wait on the engine, a row of tabs, and keyboard-friendly
//! fields and choices. Every widget carries an AccessKit label.

use eframe::egui::{self, RichText, Ui};

use crate::theme::Tokens;

/// Width of a row's label column.
const LABEL_WIDTH: f32 = 130.0;
/// Widest a field gets.
const FIELD_WIDTH: f32 = 260.0;

/// A section title inside the pane.
pub fn section(ui: &mut Ui, tokens: &Tokens, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong().color(tokens.text_primary));
}

/// A label and its value on one line; an empty value shows a dash.
pub fn row(ui: &mut Ui, tokens: &Tokens, label: &str, value: &str) {
    ui.horizontal(|ui| {
        let l = ui.add_sized(
            [LABEL_WIDTH, 18.0],
            egui::Label::new(RichText::new(label).color(tokens.text_secondary)).selectable(false),
        );
        let shown = if value.trim().is_empty() {
            "\u{2014}"
        } else {
            value
        };
        ui.add(egui::Label::new(shown).wrap()).labelled_by(l.id);
    });
}

/// A plain note for a part that waits on the engine.
pub fn pending(ui: &mut Ui, tokens: &Tokens, text: &str) {
    ui.label(RichText::new(text).color(tokens.text_tertiary).italics());
}

/// A row of tabs; returns the index clicked. Each tab is a focusable
/// button (Tab moves to it, Space or Enter picks it).
pub fn tabs(ui: &mut Ui, labels: &[String], current: usize) -> Option<usize> {
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        for (i, l) in labels.iter().enumerate() {
            if ui.selectable_label(i == current, l).clicked() && i != current {
                picked = Some(i);
            }
        }
    });
    picked
}

/// A labelled single-line field.
pub fn text_row(
    ui: &mut Ui,
    id: egui::Id,
    label: &str,
    value: &mut String,
    focus: bool,
) -> egui::Response {
    let r = ui
        .horizontal(|ui| {
            let l = ui.add_sized(
                [LABEL_WIDTH, 24.0],
                egui::Label::new(label).selectable(false),
            );
            ui.add_sized(
                [ui.available_width().min(FIELD_WIDTH), 24.0],
                egui::TextEdit::singleline(value).id(id),
            )
            .labelled_by(l.id)
        })
        .inner;
    if focus {
        r.request_focus();
    }
    r
}

/// A labelled multi-line field (Tab leaves it).
pub fn text_area(
    ui: &mut Ui,
    id: egui::Id,
    label: &str,
    value: &mut String,
    focus: bool,
) -> egui::Response {
    let l = ui.add(egui::Label::new(label).selectable(false));
    let r = ui
        .add(
            egui::TextEdit::multiline(value)
                .id(id)
                .desired_rows(3)
                .desired_width(ui.available_width().min(LABEL_WIDTH + FIELD_WIDTH)),
        )
        .labelled_by(l.id);
    if focus {
        r.request_focus();
    }
    r
}

/// A labelled choice that works from the keyboard: Up/Down (or a letter)
/// change it while it has focus; a click opens the list. Returns the new
/// index when it changed.
pub fn picker(
    ui: &mut Ui,
    id: egui::Id,
    label: &str,
    options: &[String],
    current: usize,
    focus: bool,
) -> Option<usize> {
    let mut picked = None;
    ui.horizontal(|ui| {
        let caption = ui.add_sized(
            [LABEL_WIDTH, 24.0],
            egui::Label::new(label).selectable(false),
        );
        let shown = options.get(current).cloned().unwrap_or_default();
        let inner = egui::ComboBox::from_id_salt(id)
            .selected_text(shown)
            .width(ui.available_width().min(FIELD_WIDTH))
            .show_ui(ui, |ui| {
                for (i, o) in options.iter().enumerate() {
                    if ui.selectable_label(i == current, o).clicked() {
                        picked = Some(i);
                    }
                }
            });
        let resp = inner.response.labelled_by(caption.id);
        if focus {
            resp.request_focus();
        }
        if resp.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    resp.id,
                    egui::EventFilter {
                        vertical_arrows: true,
                        ..egui::EventFilter::default()
                    },
                );
            });
            let count = options.len();
            let (down, up, typed) = ui.input(|i| {
                let typed = i.events.iter().find_map(|ev| match ev {
                    egui::Event::Text(s) => s.chars().next(),
                    _ => None,
                });
                (
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::ArrowUp),
                    typed,
                )
            });
            if down && current + 1 < count {
                picked = Some(current + 1);
            }
            if up && current > 0 {
                picked = Some(current - 1);
            }
            if let Some(ch) = typed
                && count > 0
            {
                let ch = ch.to_lowercase().to_string();
                let start = current + 1;
                picked = (0..count)
                    .map(|k| (start + k) % count)
                    .find(|i| options[*i].to_lowercase().starts_with(&ch))
                    .or(picked);
            }
        }
    });
    picked.filter(|p| *p != current)
}
