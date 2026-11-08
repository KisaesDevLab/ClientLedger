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

use super::{BankFeedScreen, Outcome};
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
    let (categorize, matches) = (p.categorize.clone(), p.matches.clone());
    s.act = Some(run(
        &s.link,
        s.group.as_ref(),
        Some("bank_feed.orphan.accept"),
        move |d| {
            let mut o = Outcome {
                skipped,
                ..Outcome::default()
            };
            for id in categorize {
                o.take(id, demo::categorize(d, id, None).map(|_| ()));
            }
            for id in matches {
                o.take(id, demo::match_one(d, id));
            }
            Ok(o)
        },
        // E3 fills this in with `/banking/feed/accept` (typed in cl-api
        // once its contract is reported); the button is disabled until the
        // engine advertises `feed-accept-v1`.
        move |_s| async move {
            Err(cl_api::ApiError::Local(
                cl_api::LocalValidation::Unsupported,
            ))
        },
    ));
    s.poll();
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
pub(crate) fn bar(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens) {
    let selected = s.list.selected().len();
    let can_write = !s.view_only && s.act.is_none();
    let why_not = if s.view_only {
        Some(t("bank_feed.view_only.tip"))
    } else {
        None
    };
    egui::Frame::new()
        .fill(tokens.accent_tint)
        .inner_margin(egui::Margin::symmetric(16, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
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
                            egui::ComboBox::from_id_salt("bank_feed.change_account")
                                .selected_text(t("bank_feed.bulk.change_account"))
                                .show_ui(ui, |ui| {
                                    for (id, name) in &s.chart {
                                        if ui.selectable_label(false, name).clicked() {
                                            chosen = Some(*id);
                                        }
                                    }
                                })
                                .response
                                .on_disabled_hover_text(why_not.clone().unwrap_or_default())
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
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for (k, what) in [
                        ("R", "bank_feed.keys.rule"),
                        ("S", "bank_feed.keys.split"),
                        ("A", "bank_feed.keys.accept"),
                        ("Space", "bank_feed.keys.select"),
                    ] {
                        ui.label(
                            RichText::new(t(what))
                                .size(12.0)
                                .color(tokens.text_secondary),
                        );
                        key_chip(ui, tokens, k);
                    }
                    ui.label(
                        RichText::new(t("bank_feed.keys.move"))
                            .size(12.0)
                            .color(tokens.text_secondary),
                    );
                    key_chip(ui, tokens, "K");
                    key_chip(ui, tokens, "J");
                });
            });
        });
}
