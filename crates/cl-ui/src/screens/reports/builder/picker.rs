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
    pick_tip(ui, salt, label, None, choices, current, width)
}

/// [`pick`] whose label explains a term (accounting jargon gets a tooltip,
/// non-negotiable 12): on the label, and as the picker's description.
pub(crate) fn pick_tip(
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    tip: Option<&str>,
    choices: &[Choice],
    current: Option<usize>,
    width: f32,
) -> Option<usize> {
    let key = format!("{salt:?}");
    let id = ui.make_persistent_id(("builder.pick", salt));
    let l = caption(ui, label);
    let l = match tip {
        Some(tip) => l.on_hover_text(tip),
        None => l,
    };
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
                    // The reason is part of the text, as drawn ("Expenses
                    // — below this row"), so keyboard and screen-reader
                    // users get it too (UI review M1).
                    Availability::Greyed(why) => {
                        let text = format!("{text} \u{2014} {}", t(why));
                        ui.add_enabled(false, egui::Button::selectable(false, text))
                            .on_disabled_hover_text(t(why));
                    }
                }
            }
        });
    let resp = inner.response.labelled_by(l.id);
    let resp = match tip {
        Some(tip) => resp.on_hover_text(tip),
        None => resp,
    };
    // A control that was just added asks for the keyboard by its salt
    // ([`want_focus`]): ids of widgets after an added row move.
    if ui.data(|d| d.get_temp::<String>(want_id())).as_deref() == Some(key.as_str()) {
        resp.request_focus();
        ui.data_mut(|d| d.remove::<String>(want_id()));
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
        let (down, up) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
            )
        });
        let typed = typed_prefix(ui, id, down || up);
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
        if let Some(prefix) = typed {
            picked = find_prefix(choices, current, &prefix).or(picked);
        }
    }
    picked.filter(|p| Some(*p) != current)
}

/// A picker that adds one of `choices` (names to a filter, rows to a
/// formula, columns): with the keyboard, Up/Down and letters move a
/// highlight shown in the closed box, and Enter adds it, so any choice can
/// be reached, not only the first match (AC6, plan §6); a click on the
/// list adds at once. Returns the index added.
pub(crate) fn pick_add(
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    placeholder: &str,
    choices: &[Choice],
    width: f32,
) -> Option<usize> {
    let id = ui.make_persistent_id(("builder.pick_add", salt));
    let hid = id.with("highlight");
    let mut hl: Option<usize> = ui
        .data(|d| d.get_temp::<usize>(hid))
        .filter(|i| choices.get(*i).is_some_and(|c| c.1.is_available()));
    let l = caption(ui, label);
    let mut added = None;
    let shown = hl
        .and_then(|i| choices.get(i))
        .map_or_else(|| placeholder.to_string(), |c| c.0.clone());
    let inner = egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .width(width)
        .show_ui(ui, |ui| {
            for (i, (text, a)) in choices.iter().enumerate() {
                match a {
                    Availability::Available => {
                        if ui.selectable_label(hl == Some(i), text).clicked() {
                            added = Some(i);
                        }
                    }
                    Availability::Greyed(why) => {
                        let text = format!("{text} \u{2014} {}", t(why));
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
        let (down, up, enter) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Enter),
            )
        });
        let typed = typed_prefix(ui, id, down || up || enter);
        let usable = |i: &usize| choices[*i].1.is_available();
        let n = choices.len();
        if down {
            let from = hl.map_or(0, |c| c + 1);
            hl = (from..n).find(usable).or(hl);
        }
        if up {
            let to = hl.unwrap_or(n);
            hl = (0..to).rev().find(usable).or(hl);
        }
        if let Some(prefix) = typed {
            hl = find_prefix(choices, hl, &prefix).or(hl);
        }
        if enter && let Some(i) = hl {
            added = Some(i);
        }
    }
    if added.is_some() {
        hl = None;
        // The picker keeps the keyboard for the next one.
        resp.request_focus();
    }
    ui.data_mut(|d| match hl {
        Some(i) => {
            d.insert_temp(hid, i);
        }
        None => d.remove::<usize>(hid),
    });
    added
}

/// How long a pause ends a type-ahead: letters typed closer together than
/// this make one prefix ("cu" reaches Custom dates, QA-5).
const TYPE_AHEAD_SECS: f64 = 1.0;

/// The type-ahead prefix after this frame's typing, when something was
/// typed: letters typed within [`TYPE_AHEAD_SECS`] of the last add to it;
/// an arrow or Enter (`reset`) starts a new one.
fn typed_prefix(ui: &Ui, id: egui::Id, reset: bool) -> Option<String> {
    let (text, now) = ui.input(|i| {
        let text: String = i
            .events
            .iter()
            .filter_map(|ev| match ev {
                egui::Event::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        (text, i.time)
    });
    let bid = id.with("type-ahead");
    if reset {
        ui.data_mut(|d| d.remove::<(String, f64)>(bid));
    }
    if text.is_empty() {
        return None;
    }
    let before = ui
        .data(|d| d.get_temp::<(String, f64)>(bid))
        .filter(|(_, at)| now - at <= TYPE_AHEAD_SECS)
        .map(|(s, _)| s)
        .unwrap_or_default();
    let prefix = format!("{before}{}", text.to_lowercase());
    ui.data_mut(|d| d.insert_temp(bid, (prefix.clone(), now)));
    Some(prefix)
}

/// The choice `prefix` reaches from `current`: one letter moves to the
/// next choice starting with it (repeating it cycles); more letters keep
/// the current choice while it still matches, else the first that does.
fn find_prefix(choices: &[Choice], current: Option<usize>, prefix: &str) -> Option<usize> {
    let n = choices.len();
    let Some(first) = prefix.chars().next() else {
        return None;
    };
    if n == 0 {
        return None;
    }
    // "aaa" is "a" three times: it cycles through the A's.
    let same = prefix.chars().all(|ch| ch == first);
    let single = first.to_string();
    let prefix = if same { single.as_str() } else { prefix };
    let matches = |i: &usize| {
        choices[*i].1.is_available() && choices[*i].0.to_lowercase().starts_with(prefix)
    };
    let one = prefix.chars().count() == 1;
    let start = match current {
        Some(c) if one => c + 1,
        Some(c) => c,
        None => 0,
    };
    (0..n).map(|k| (start + k) % n).find(matches)
}

fn want_id() -> egui::Id {
    egui::Id::unique("builder.pick.want_focus")
}

/// Gives the keyboard to the picker drawn with `salt` (the next time it is
/// drawn): the first control of a row just added (AC6).
pub(crate) fn want_focus(ui: &Ui, salt: impl std::fmt::Debug) {
    ui.data_mut(|d| d.insert_temp(want_id(), format!("{salt:?}")));
    ui.ctx().request_repaint();
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
            // A greyed choice names its reason for screen readers (M1).
            let name = match a {
                Availability::Greyed(why) => format!("{label}: {text} \u{2014} {}", t(why)),
                Availability::Available => format!("{label}: {text}"),
            };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<Choice> {
        v.iter().map(|s| on((*s).to_string())).collect()
    }

    /// QA-5: more than one letter reaches a choice by its start; one
    /// letter, typed again, cycles through the choices with it.
    #[test]
    fn type_ahead_matches_more_than_the_first_letter() {
        let c = names(&[
            "This month",
            "Choose each time it runs",
            "Custom dates",
            "Cash",
        ]);
        assert_eq!(find_prefix(&c, Some(0), "c"), Some(1));
        assert_eq!(find_prefix(&c, Some(1), "cu"), Some(2));
        assert_eq!(find_prefix(&c, Some(0), "cu"), Some(2));
        assert_eq!(
            find_prefix(&c, Some(2), "cu"),
            Some(2),
            "stays while it matches"
        );
        assert_eq!(find_prefix(&c, Some(1), "cc"), Some(2), "cycles");
        assert_eq!(find_prefix(&c, Some(2), "ca"), Some(3));
        assert_eq!(find_prefix(&c, Some(0), "x"), None);
    }

    /// QA-5: "cu" typed in a picker picks Custom dates, through egui.
    #[test]
    fn a_picker_takes_two_letters() {
        let ctx = egui::Context::default();
        let c = names(&["This month", "Choose each time it runs", "Custom dates"]);
        let mut current = Some(0);
        let run = |events: Vec<egui::Event>, time: f64, current: &mut Option<usize>| {
            let mut out = ctx.run_ui(
                egui::RawInput {
                    events,
                    time: Some(time),
                    ..Default::default()
                },
                |ui| {
                    if let Some(i) = pick(ui, "dates", "Dates", &c, *current, 160.0) {
                        *current = Some(i);
                    }
                },
            );
            out.textures_delta.clear();
        };
        run(Vec::new(), 0.0, &mut current);
        let tab = egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        run(vec![tab], 0.1, &mut current);
        run(vec![egui::Event::Text("c".into())], 0.2, &mut current);
        assert_eq!(current, Some(1));
        run(vec![egui::Event::Text("u".into())], 0.3, &mut current);
        assert_eq!(current, Some(2), "Custom dates");
        // After a pause a letter starts again.
        run(vec![egui::Event::Text("t".into())], 2.0, &mut current);
        assert_eq!(current, Some(0));
    }
}
