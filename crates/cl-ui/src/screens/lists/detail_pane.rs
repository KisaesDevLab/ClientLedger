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

/// A left-aligned caption padded to the label column; returns it for
/// `labelled_by`.
pub fn caption(ui: &mut Ui, text: RichText) -> egui::Response {
    let l = ui.add(egui::Label::new(text).selectable(false));
    let used = l.rect.width();
    if used < LABEL_WIDTH {
        ui.add_space(LABEL_WIDTH - used);
    }
    l
}

/// A section title inside the pane.
pub fn section(ui: &mut Ui, tokens: &Tokens, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong().color(tokens.text_primary));
}

/// A label and its value on one line; an empty value shows a dash.
pub fn row(ui: &mut Ui, tokens: &Tokens, label: &str, value: &str) {
    ui.horizontal(|ui| {
        // Plain text read in order (caption, then value); `labelled_by`
        // would replace the value's own text for screen readers.
        caption(ui, RichText::new(label).color(tokens.text_secondary));
        let shown = if value.trim().is_empty() {
            "\u{2014}"
        } else {
            value
        };
        ui.add(egui::Label::new(shown).wrap());
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
            let r = ui.selectable_label(i == current, l);
            r.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::Tab, true, i == current, l.clone())
            });
            if r.clicked() && i != current {
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
            let l = caption(ui, RichText::new(label));
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
    picker_opt(ui, id, label, options, Some(current), "", focus)
}

/// [`picker`] that may start with nothing chosen: `placeholder` shows until
/// the user picks (a merge target, QA-20). Down or a letter picks from the
/// top; Up does nothing until something is chosen.
pub fn picker_opt(
    ui: &mut Ui,
    id: egui::Id,
    label: &str,
    options: &[String],
    current: Option<usize>,
    placeholder: &str,
    focus: bool,
) -> Option<usize> {
    let mut picked = None;
    ui.horizontal(|ui| {
        let caption = caption(ui, RichText::new(label));
        let shown = current
            .and_then(|c| options.get(c).cloned())
            .unwrap_or_else(|| placeholder.to_owned());
        let inner = egui::ComboBox::from_id_salt(id)
            .selected_text(shown)
            .width(ui.available_width().min(FIELD_WIDTH))
            .show_ui(ui, |ui| {
                for (i, o) in options.iter().enumerate() {
                    if ui.selectable_label(Some(i) == current, o).clicked() {
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
            let next = current.map_or(0, |c| c + 1);
            if down && next < count {
                picked = Some(next);
            }
            if up && let Some(c) = current.filter(|c| *c > 0) {
                picked = Some(c - 1);
            }
            if let Some(ch) = typed
                && count > 0
            {
                let ch = ch.to_lowercase().to_string();
                let start = next;
                picked = (0..count)
                    .map(|k| (start + k) % count)
                    .find(|i| options[*i].to_lowercase().starts_with(&ch))
                    .or(picked);
            }
        }
    });
    picked.filter(|p| Some(*p) != current)
}
