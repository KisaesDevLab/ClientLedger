//! The command palette (Ctrl+K, "Go to"): type a few letters, press Enter.
//! Every registered command is listed, with its shortcut.

use cl_model::commands::DEV_COMMANDS;
use cl_model::{COMMANDS, Command, CommandId, Keymap, Scope};
use eframe::egui::{self, Align2, Context, Key, Modifiers, RichText};

use crate::i18n::t;
use crate::input;
use crate::theme::Tokens;
use crate::widgets::kbd;

/// Most rows shown at once.
pub const MAX_ROWS: usize = 12;

/// Palette state.
#[derive(Debug, Default, Clone)]
pub struct State {
    /// Showing.
    pub open: bool,
    /// What the owner typed.
    pub query: String,
    /// Highlighted row.
    pub selected: usize,
    /// Also list the developer-only commands (`--demo` only, S8).
    pub include_dev: bool,
    just_opened: bool,
}

impl State {
    /// Open or close.
    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open = true;
            self.query.clear();
            self.selected = 0;
            self.just_opened = true;
        }
    }

    /// Close and forget the query.
    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.selected = 0;
    }
}

/// How many result rows fit below the text box in a window `height` points
/// tall (each row is 26 points plus spacing, and the box and margins take
/// about 150): at 200% zoom in a small window only a few fit.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn rows_that_fit(height: f32) -> usize {
    (((height - 150.0) / 32.0).floor().max(0.0) as usize).clamp(3, MAX_ROWS)
}

/// How well `label` matches `query` (lower is better); `None` is no match.
/// Prefix beats word start beats substring beats scattered letters.
#[must_use]
pub fn score(label: &str, query: &str) -> Option<u32> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    let l = label.to_lowercase();
    if l.starts_with(&q) {
        return Some(0);
    }
    if l.split(|c: char| !c.is_alphanumeric())
        .any(|w| w.starts_with(&q))
    {
        return Some(1);
    }
    if l.contains(&q) {
        return Some(2);
    }
    let mut letters = l.chars();
    q.chars()
        .filter(|c| !c.is_whitespace())
        .all(|qc| letters.any(|lc| lc == qc))
        .then_some(3)
}

/// The commands that match, best first. With no query, commands that open
/// screens or run app-wide actions come before form-only keys.
#[must_use]
pub fn matches(query: &str) -> Vec<&'static Command> {
    matches_in(query, false)
}

/// [`matches`], with the developer-only commands when `include_dev`.
#[must_use]
pub fn matches_in(query: &str, include_dev: bool) -> Vec<&'static Command> {
    let dev: &'static [Command] = if include_dev { DEV_COMMANDS } else { &[] };
    let mut scored: Vec<(u32, bool, &'static Command)> = COMMANDS
        .iter()
        .chain(dev)
        .filter(|c| c.palette)
        .filter_map(|c| {
            score(&t(&format!("cmd.{}", c.id)), query)
                .map(|s| (s, matches!(c.scope, Scope::Form | Scope::Grid), c))
        })
        .collect();
    if query.trim().is_empty() {
        // No query: menu order, form-only keys last (the sort is stable).
        scored.sort_by_key(|(_, form_only, _)| *form_only);
    } else {
        scored.sort_by_key(|(s, form_only, c)| (*s, *form_only, t(&format!("cmd.{}", c.id))));
    }
    scored.into_iter().map(|(_, _, c)| c).collect()
}

/// Draw the palette. Returns the command the owner chose.
pub fn show(ctx: &Context, state: &mut State, _keymap: &Keymap) -> Option<CommandId> {
    if !state.open {
        return None;
    }
    let tokens = Tokens::of(ctx);
    let results = matches_in(&state.query, state.include_dev);
    let rows = rows_that_fit(ctx.content_rect().height());
    let shown = results.len().min(rows);
    if shown > 0 {
        state.selected = state.selected.min(shown - 1);
    }

    // Keys act before the text box sees them.
    let (down, up, enter, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if down && shown > 0 {
        state.selected = (state.selected + 1) % shown;
    }
    if up && shown > 0 {
        state.selected = (state.selected + shown - 1) % shown;
    }
    if esc {
        state.close();
        return None;
    }
    if enter {
        let chosen = results.get(state.selected).map(|c| c.id);
        if chosen.is_some() {
            state.close();
            return chosen;
        }
    }

    let mut chosen: Option<CommandId> = None;
    egui::Area::new(egui::Id::unique("command_palette"))
        .anchor(Align2::CENTER_TOP, [0.0, 80.0])
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(tokens.surface_window)
                .stroke(egui::Stroke::new(1.0, tokens.border_strong))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(460.0_f32.min(ctx.content_rect().width() - 40.0));
                    let label = ui.label(
                        RichText::new(t("shell.go_to"))
                            .size(12.0)
                            .color(tokens.text_secondary),
                    );
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut state.query)
                            .hint_text(t("palette.hint"))
                            .desired_width(f32::INFINITY)
                            .font(crate::theme::styles::input()),
                    );
                    // A screen reader announces the box by this label.
                    let edit = edit.labelled_by(label.id);
                    if state.just_opened {
                        edit.request_focus();
                        state.just_opened = false;
                    }
                    if edit.changed() {
                        state.selected = 0;
                    }
                    ui.add_space(6.0);
                    if results.is_empty() {
                        ui.label(RichText::new(t("palette.none")).color(tokens.text_secondary));
                    }
                    for (i, c) in results.iter().take(rows).enumerate() {
                        let selected = i == state.selected;
                        let label = t(&format!("cmd.{}", c.id));
                        let row = ui.add_sized(
                            [ui.available_width(), 26.0],
                            egui::Button::new(RichText::new(&label).color(if selected {
                                tokens.on_accent
                            } else {
                                tokens.text_primary
                            }))
                            .fill(if selected {
                                tokens.accent
                            } else {
                                egui::Color32::TRANSPARENT
                            })
                            .stroke(egui::Stroke::NONE)
                            .right_text(c.shortcut.map(input::shortcut_text).unwrap_or_default()),
                        );
                        if row.clicked() {
                            chosen = Some(c.id);
                        }
                    }
                    let _ = kbd;
                });
        });
    if chosen.is_some() {
        state.close();
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_commands_are_listed_only_when_asked() {
        assert!(
            matches("")
                .iter()
                .all(|c| !c.id.as_str().starts_with("dev."))
        );
        assert!(
            matches("grid demo").is_empty()
                || matches("grid demo")
                    .iter()
                    .all(|c| !c.id.as_str().starts_with("dev."))
        );
        assert!(
            matches_in("grid demo", true)
                .iter()
                .any(|c| c.id.as_str() == "dev.grid_demo")
        );
    }

    fn ids(q: &str) -> Vec<&'static str> {
        matches(q).iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn empty_query_lists_everything_with_form_keys_last() {
        let all = ids("");
        assert_eq!(all.len(), COMMANDS.len());
        let first_form = all.iter().position(|id| id.starts_with("form.")).unwrap();
        assert!(all[..first_form].iter().all(|id| !id.starts_with("form.")));
    }

    #[test]
    fn prefix_beats_word_start_beats_substring() {
        assert_eq!(score("Write Check", "wri"), Some(0));
        assert_eq!(score("Write Check", "che"), Some(1));
        assert_eq!(score("Write Check", "eck"), Some(2));
        assert_eq!(score("Write Check", "wck"), Some(3));
        assert_eq!(score("Write Check", "zzz"), None);
    }

    #[test]
    fn typing_finds_screens() {
        assert_eq!(ids("write ch").first(), Some(&"vendors.write_check"));
        assert_eq!(ids("pay b").first(), Some(&"vendors.pay_bills"));
        assert!(ids("bank").contains(&"banking.feed"));
        assert_eq!(ids("zzzzzz"), Vec::<&str>::new());
    }

    #[test]
    fn every_command_is_findable_by_its_own_label() {
        for c in COMMANDS {
            assert!(
                ids(c.label).contains(&c.id.as_str()),
                "{} not found by its label",
                c.id
            );
        }
    }

    #[test]
    fn few_rows_fit_in_a_small_window() {
        assert_eq!(rows_that_fit(320.0), 5);
        assert_eq!(rows_that_fit(100.0), 3, "never fewer than three");
        assert_eq!(rows_that_fit(940.0), MAX_ROWS);
    }

    #[test]
    fn toggle_opens_clean_and_closes() {
        let mut s = State {
            query: "old".into(),
            ..State::default()
        };
        s.toggle();
        assert!(s.open && s.query.is_empty());
        s.toggle();
        assert!(!s.open);
    }
}
