//! The editor's small controls: a keyboard picker (Up/Down or a letter
//! change it while it has focus, a click opens the list; greyed choices
//! show their reason and are skipped), segmented buttons, and a text field
//! that commits when it loses focus (one undo step per committed field,
//! Q-A08p) and reverts on Esc.

use cl_model::report_def::fields::Availability;
use eframe::egui::{self, Response, RichText, Ui};

use super::{BuilderScreen, FieldKey};
use crate::i18n::t;
use crate::theme::Tokens;

/// One choice: its text and whether it can be picked.
pub(crate) type Choice = (String, Availability);

/// A choice that can be picked.
pub(crate) fn on(text: String) -> Choice {
    (text, Availability::Available)
}

/// A field label above its control (the accessible name).
pub(crate) fn caption(ui: &mut Ui, text: &str) -> Response {
    let tokens = Tokens::of(ui.ctx());
    ui.add(
        egui::Label::new(RichText::new(text).size(12.0).color(tokens.text_secondary))
            .selectable(false),
    )
}

/// A labelled keyboard picker. Returns the index picked, when it changed.
pub(crate) fn pick(
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    choices: &[Choice],
    current: Option<usize>,
    width: f32,
) -> Option<usize> {
    let id = ui.make_persistent_id(("builder.pick", salt));
    let l = caption(ui, label);
    let mut picked = None;
    let shown = current
        .and_then(|i| choices.get(i))
        .map_or_else(|| t("builder.pick.none"), |c| c.0.clone());
    let inner = egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(width)
        .show_ui(ui, |ui| {
            for (i, (text, a)) in choices.iter().enumerate() {
                match a {
                    Availability::Available => {
                        if ui.selectable_label(current == Some(i), text).clicked() {
                            picked = Some(i);
                        }
                    }
                    Availability::Greyed(why) => {
                        ui.add_enabled(false, egui::Button::selectable(false, text))
                            .on_disabled_hover_text(t(why));
                    }
                }
            }
        });
    let resp = inner.response.labelled_by(l.id);
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
        let usable = |i: &usize| choices[*i].1.is_available();
        let n = choices.len();
        if down {
            let from = current.map_or(0, |c| c + 1);
            picked = (from..n).find(usable).or(picked);
        }
        if up {
            let to = current.unwrap_or(n);
            picked = (0..to).rev().find(usable).or(picked);
        }
        if let Some(ch) = typed
            && n > 0
        {
            let ch = ch.to_lowercase().to_string();
            let start = current.map_or(0, |c| c + 1);
            picked = (0..n)
                .map(|k| (start + k) % n)
                .filter(usable)
                .find(|i| choices[*i].0.to_lowercase().starts_with(&ch))
                .or(picked);
        }
    }
    picked.filter(|p| Some(*p) != current)
}

/// Segmented buttons (one choice of a few). Returns the index picked.
pub(crate) fn segmented(
    ui: &mut Ui,
    label: &str,
    choices: &[Choice],
    current: Option<usize>,
) -> Option<usize> {
    let tokens = Tokens::of(ui.ctx());
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, (text, a)) in choices.iter().enumerate() {
            let selected = current == Some(i);
            let rich = if selected {
                RichText::new(text).color(tokens.on_accent).strong()
            } else {
                RichText::new(text)
            };
            let mut b = egui::Button::new(rich).min_size(egui::vec2(0.0, 28.0));
            if selected {
                b = b.fill(tokens.accent);
            }
            let r = ui.add_enabled(a.is_available(), b);
            let r = match a {
                Availability::Greyed(why) => r.on_disabled_hover_text(t(why)),
                Availability::Available => r,
            };
            let name = format!("{label}: {text}");
            r.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::Role::RadioButton,
                    a.is_available(),
                    selected,
                    &name,
                )
            });
            if r.clicked() && !selected {
                picked = Some(i);
            }
        }
    });
    picked
}

/// A labelled text field that commits to the editor when it loses focus
/// (Enter or Tab), reverts on Esc, and keeps its text while it has focus.
pub(crate) fn text(
    s: &mut BuilderScreen,
    ui: &mut Ui,
    key: FieldKey,
    label: &str,
    current: &str,
    width: f32,
) -> Response {
    let l = caption(ui, label);
    text_bare(s, ui, key, current, width, Some(l.id))
}

/// [`text`] without its own caption (`labelled_by`: the label shown
/// elsewhere).
pub(crate) fn text_bare(
    s: &mut BuilderScreen,
    ui: &mut Ui,
    key: FieldKey,
    current: &str,
    width: f32,
    labelled_by: Option<egui::Id>,
) -> Response {
    let id = text_id(s.id, &key);
    let mut buf = s
        .drafts
        .get(&key)
        .cloned()
        .unwrap_or_else(|| current.to_string());
    let mut r = ui.add(
        egui::TextEdit::singleline(&mut buf)
            .id(id)
            .desired_width(width),
    );
    if let Some(l) = labelled_by {
        r = r.labelled_by(l);
    }
    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if r.has_focus() {
        s.drafts.insert(key.clone(), buf);
        s.pending = Some(key);
    } else if r.lost_focus() {
        s.drafts.remove(&key);
        if s.pending.as_ref() == Some(&key) {
            s.pending = None;
        }
        if !esc && buf != current {
            s.commit(&key, buf);
        }
    }
    r
}

/// A text field's id: stable for the editor and the field, wherever it is
/// drawn (the control channel and tests find it).
pub(crate) fn text_id(editor: super::EditorId, key: &FieldKey) -> egui::Id {
    egui::Id::unique(("builder.text", editor.0, key))
}

/// A checkbox. Returns the new value when it was toggled.
pub(crate) fn check(ui: &mut Ui, value: bool, label: &str) -> Option<bool> {
    let mut v = value;
    ui.checkbox(&mut v, label).changed().then_some(v)
}
