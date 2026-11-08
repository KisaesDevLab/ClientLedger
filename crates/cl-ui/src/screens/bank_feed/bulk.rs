//! The bulk bar (P26, mockup): "n selected", Accept selected (Ctrl+Enter),
//! Change account, Exclude and the key hints. Accept selected sends the
//! plan of [`cl_model::bank_feed::plan_accept`]: ready rows with an account
//! and matches with one candidate both ways; transfers, duplicates and rows
//! needing the owner stay selected for review. Rows that changed on
//! another computer leave the selection (plan §5).

use cl_api::{FeedChoice, FeedItemId};
use cl_core::ids::AccountId;
use cl_model::bank_feed::{self as model, PlanRow, Show, Why};
use eframe::egui::{self, RichText, Ui};

use super::BankFeedScreen;
use super::send::Outcome;
use crate::company::demo::feed_review as demo;
use crate::i18n::{t, t_with};
use crate::screens::templates::engine::run;
use crate::theme::Tokens;

/// The plan for `ids` (in list order).
pub(crate) fn plan(s: &BankFeedScreen, ids: &[FeedItemId]) -> model::AcceptPlan<FeedItemId> {
    let rows: Vec<PlanRow<FeedItemId>> = ids
        .iter()
        .filter_map(|id| s.row(*id))
        .map(|r| {
            let why = s.why(r);
            PlanRow {
                id: r.id,
                bucket: r.bucket.map(|b| match b {
                    cl_api::FeedBucket::Ready => model::Bucket::Ready,
                    cl_api::FeedBucket::Review => model::Bucket::Review,
                    cl_api::FeedBucket::Matches => model::Bucket::Matches,
                }),
                has_account: r.assigned_account.is_some() || r.suggested_account.is_some(),
                match_candidates: r.match_candidate_count,
                // The list's count is per row; "both ways" is checked by the
                // engine's accept (E3). One candidate on a row counts here.
                mutual: r.match_candidate_count == 1,
                transfer: why == Why::Transfer,
                possible_duplicate: r.possible_duplicate,
            }
        })
        .collect();
    model::plan_accept(&rows)
}

/// Ctrl+Enter / Accept selected.
pub(crate) fn accept_selected(s: &mut BankFeedScreen, ids: &[FeedItemId]) {
    if s.view_only || s.act.is_some() || s.show != Show::Waiting {
        return;
    }
    let p = plan(s, ids);
    let skipped = p.skipped.len();
    if p.is_empty() {
        s.notice = Some(("bank_feed.notice.skipped", skipped));
        return;
    }
    if !s.accept_route && !s.link.is_demo() {
        // The one accept route (E3) isn't in this engine: nothing is sent
        // row by row (plan §1: not atomic, not undoable).
        s.notice = Some(("bank_feed.notice.accept_route", 0));
        return;
    }
    super::send::accept_selected(s, &p.categorize, &p.matches, skipped);
}

/// Change account on the selected waiting rows (stages it; nothing posts).
pub(crate) fn change_account(s: &mut BankFeedScreen, account: AccountId) {
    let ids = s.list.selected();
    if s.view_only || s.act.is_some() || ids.is_empty() {
        return;
    }
    let choice = FeedChoice {
        account: Some(account),
        ..FeedChoice::default()
    };
    let live_ids = ids.clone();
    let live_choice = choice.clone();
    s.act = Some(run(
        &s.link,
        s.group.as_ref(),
        Some("bank_feed.orphan.assign"),
        move |d| {
            let mut o = Outcome::default();
            for id in ids {
                o.take(id, demo::assign(d, id, &choice));
            }
            Ok(o)
        },
        move |sess| async move {
            let mut o = Outcome::default();
            for id in live_ids {
                o.take(id, sess.feed().assign(id, &live_choice).await);
            }
            Ok(o)
        },
    ));
    s.poll();
}

/// Exclude the selected waiting rows.
pub(crate) fn exclude_selected(s: &mut BankFeedScreen) {
    let ids = s.list.selected();
    if s.view_only || s.act.is_some() || ids.is_empty() {
        return;
    }
    let live_ids = ids.clone();
    s.act = Some(run(
        &s.link,
        s.group.as_ref(),
        Some("bank_feed.orphan.exclude"),
        move |d| {
            let mut o = Outcome::default();
            for id in ids {
                o.take(id, demo::exclude(d, id));
            }
            Ok(o)
        },
        move |sess| async move {
            let mut o = Outcome::default();
            for id in live_ids {
                o.take(id, sess.feed().exclude(id).await);
            }
            Ok(o)
        },
    ));
    s.poll();
}

/// Restore the selected excluded rows.
pub(crate) fn restore_selected(s: &mut BankFeedScreen) {
    let ids = s.list.selected();
    if s.view_only || s.act.is_some() || ids.is_empty() {
        return;
    }
    let live_ids = ids.clone();
    s.act = Some(run(
        &s.link,
        s.group.as_ref(),
        Some("bank_feed.orphan.restore"),
        move |d| {
            let mut o = Outcome::default();
            for id in ids {
                o.take(id, demo::unexclude(d, id));
            }
            Ok(o)
        },
        move |sess| async move {
            let mut o = Outcome::default();
            for id in live_ids {
                o.take(id, sess.feed().unexclude(id).await);
            }
            Ok(o)
        },
    ));
    s.poll();
}

fn key_chip(ui: &mut Ui, tokens: &Tokens, k: &str) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, tokens.border_default))
        .corner_radius(egui::CornerRadius::same(3))
        .inner_margin(egui::Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.label(
                RichText::new(k)
                    .monospace()
                    .size(11.0)
                    .color(tokens.text_secondary),
            );
        });
}

/// The bar over the list.
/// `hints`: the key hints show (a short window leaves them out, QA-9).
pub(crate) fn bar(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens, hints_on: bool) {
    let selected = s.list.selected().len();
    let can_write = !s.view_only && s.act.is_none();
    let why_not = if s.view_only {
        Some(t("bank_feed.view_only.tip"))
    } else {
        None
    };
    let narrow = ui.available_width() < super::view::WRAP_BELOW;
    egui::Frame::new()
        .fill(tokens.accent_tint)
        .inner_margin(egui::Margin::symmetric(16, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let row = |ui: &mut Ui| {
                ui.label(
                    RichText::new(t_with(
                        "bank_feed.bulk.selected",
                        &[("n", &selected.to_string())],
                    ))
                    .strong()
                    .color(tokens.text_primary),
                );
                ui.add_space(8.0);
                match s.show {
                    Show::Waiting => {
                        let accept_ok =
                            can_write && selected > 0 && (s.accept_route || s.link.is_demo());
                        let tip = why_not.clone().unwrap_or_else(|| {
                            if s.accept_route || s.link.is_demo() {
                                t("bank_feed.bulk.accept.tip")
                            } else {
                                t("bank_feed.bulk.accept.unavailable")
                            }
                        });
                        let b = egui::Button::new(
                            RichText::new(format!(
                                "{}  {}",
                                t("bank_feed.bulk.accept"),
                                crate::input::shortcut_text(cl_model::Shortcut::new(
                                    cl_model::Mods::CMD,
                                    cl_model::Key::Enter
                                ))
                            ))
                            .color(tokens.on_accent),
                        )
                        .fill(tokens.accent);
                        if ui
                            .add_enabled(accept_ok, b)
                            .on_hover_text(&tip)
                            .on_disabled_hover_text(&tip)
                            .clicked()
                        {
                            let ids = s.list.selected();
                            accept_selected(s, &ids);
                        }
                        let mut chosen = None;
                        ui.add_enabled_ui(can_write && selected > 0, |ui| {
                            let label = t("bank_feed.bulk.change_account");
                            let r = egui::ComboBox::from_id_salt("bank_feed.change_account")
                                .selected_text(&label)
                                .show_ui(ui, |ui| {
                                    for (id, name) in &s.chart {
                                        if ui.selectable_label(false, name).clicked() {
                                            chosen = Some(*id);
                                        }
                                    }
                                })
                                .response;
                            // Screen readers and the control channel name it
                            // (P26 QA-3).
                            let enabled = ui.is_enabled();
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::ComboBox, enabled, &label)
                            });
                            r.on_disabled_hover_text(why_not.clone().unwrap_or_default())
                        });
                        if let Some(a) = chosen {
                            s.change_to = Some(a);
                            change_account(s, a);
                        }
                        if ui
                            .add_enabled(
                                can_write && selected > 0,
                                egui::Button::new(t("bank_feed.bulk.exclude")),
                            )
                            .on_disabled_hover_text(why_not.clone().unwrap_or_default())
                            .clicked()
                        {
                            exclude_selected(s);
                        }
                    }
                    Show::Excluded => {
                        if ui
                            .add_enabled(
                                can_write && selected > 0,
                                egui::Button::new(t("bank_feed.bulk.restore")),
                            )
                            .on_disabled_hover_text(why_not.clone().unwrap_or_default())
                            .clicked()
                        {
                            restore_selected(s);
                        }
                    }
                    Show::Accepted => {}
                }
                if !narrow && hints_on {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        hints(ui, tokens, true);
                    });
                }
            };
            if narrow {
                ui.horizontal_wrapped(row);
            } else {
                ui.horizontal(row);
            }
            // Narrow or zoomed: the key hints get their own line instead of
            // printing over the buttons (P26 QA-1, QA-23).
            if narrow && hints_on {
                ui.horizontal_wrapped(|ui| hints(ui, tokens, false));
            }
        });
}

/// The key hints, in reading order (`rtl`: laid out right to left).
fn hints(ui: &mut Ui, tokens: &Tokens, rtl: bool) {
    let mut items: Vec<(&[&str], &str)> = vec![
        (&["J", "K"], "bank_feed.keys.move"),
        (&["Space"], "bank_feed.keys.select"),
        (&["A"], "bank_feed.keys.accept"),
        (&["C"], "bank_feed.keys.account"),
        (&["S"], "bank_feed.keys.split"),
        (&["R"], "bank_feed.keys.rule"),
    ];
    if rtl {
        items.reverse();
    }
    for (keys, what) in items {
        let label = |ui: &mut Ui| {
            ui.label(
                RichText::new(t(what))
                    .size(12.0)
                    .color(tokens.text_secondary),
            );
        };
        if rtl {
            label(ui);
            for k in keys.iter().rev() {
                key_chip(ui, tokens, k);
            }
        } else {
            for k in keys {
                key_chip(ui, tokens, k);
            }
            label(ui);
            ui.add_space(6.0);
        }
    }
}
