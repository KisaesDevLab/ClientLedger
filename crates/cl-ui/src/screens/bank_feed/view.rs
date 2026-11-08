//! The Bank Feed window's layout in mockup order: the header (title,
//! account picker, last file imported, Import Bank File, Rules), the
//! segments (All, Ready, Needs review, Matches) with the Show selector,
//! the "Entry voided" chip and the filter, the bulk bar, the list (select,
//! date, from the bank, amount, will be recorded as, why) and the detail
//! panel, and the footnote (Q-P26h deviations: Show selector, Entry voided
//! chip).

use cl_api::{FeedRow, FeedStatus};
use cl_model::bank_feed::{self as model, Segment, Show, Why};
use eframe::egui::{self, Align2, RichText, Sense, Ui, Vec2};

use super::{BankFeedScreen, bulk};
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;

/// The detail panel's width (mockup).
const DETAIL_W: f32 = 330.0;
const ROW_H: f32 = 46.0;

/// Draws the whole window.
pub(crate) fn draw(s: &mut BankFeedScreen, ui: &mut Ui) {
    let tokens = Tokens::of(ui.ctx());
    header(s, ui, &tokens);
    if !s.supported {
        form::hint(ui, &tokens, &t("bank_feed.unsupported"));
        return;
    }
    controls(s, ui, &tokens);
    ui.add_space(6.0);
    rule_banner(s, ui, &tokens);
    rule_offer(s, ui, &tokens);
    bulk::bar(s, ui, &tokens);
    toast(s, ui, &tokens);
    undo_changed(s, ui, &tokens);
    closing(s, ui, &tokens);
    if s.load_failed {
        form::inline_warning(ui, &tokens, &t("bank_feed.load_failed"));
    }
    // The Undo toast already says what was recorded.
    if let Some((k, n)) = s
        .notice
        .filter(|(k, _)| !(s.toast.is_some() && *k == "bank_feed.notice.done"))
    {
        form::hint(ui, &tokens, &t_with(k, &[("n", &n.to_string())]));
    }
    let avail = ui.available_size();
    let list_w = (avail.x - DETAIL_W - 8.0).max(320.0);
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(list_w, avail.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                list(s, ui, &tokens);
                footnote(s, ui, &tokens);
            },
        );
        ui.separator();
        ui.allocate_ui_with_layout(
            Vec2::new(DETAIL_W, avail.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                if s.rules.open {
                    super::rules::draw(s, ui, &tokens);
                } else {
                    super::detail::draw(s, ui, &tokens);
                }
            },
        );
    });
}

fn header(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.heading(RichText::new(t("cmd.banking.feed")).strong());
        ui.add_space(12.0);
        let r = ui.label(RichText::new(t("bank_feed.account")).color(tokens.text_secondary));
        let shown = s.current_account().map_or_else(
            || t("bank_feed.account.none"),
            |a| account_text(&a.label, a.waiting),
        );
        let mut pick = None;
        egui::ComboBox::from_id_salt("bank_feed.account")
            .selected_text(shown)
            .width(260.0)
            .show_ui(ui, |ui| {
                for (i, a) in s.accounts.iter().enumerate() {
                    if ui
                        .selectable_label(i == s.account, account_text(&a.label, a.waiting))
                        .clicked()
                    {
                        pick = Some(i);
                    }
                }
            })
            .response
            .labelled_by(r.id);
        if let Some(i) = pick
            && i != s.account
        {
            s.account = i;
            s.offset = 0;
            s.list.clear_selection();
            s.reload();
        }
        if let Some(d) = s.last_import {
            ui.add_space(12.0);
            ui.label(
                RichText::new(t_with(
                    "bank_feed.last_import",
                    &[("date", &crate::shell::us_date(d))],
                ))
                .color(tokens.text_secondary),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Rules is hidden for a view-only user (plan §5).
            if !s.view_only && ui.button(t("bank_feed.rules")).clicked() {
                s.detail_request = None;
                super::rules::open(s);
            }
            if ui.button(t("cmd.banking.import_file")).clicked() {
                s.link.push(crate::company::CompanyEvent::OpenWindow(
                    cl_model::CommandId::new("banking.import_file"),
                ));
            }
        });
    });
    ui.add_space(6.0);
}

fn account_text(label: &str, waiting: u64) -> String {
    format!("{label} ({waiting})")
}

fn controls(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens) {
    ui.horizontal(|ui| {
        let mut changed = false;
        if s.show == Show::Waiting {
            for seg in Segment::ALL {
                let n = match seg {
                    Segment::All => s.counts.waiting,
                    Segment::Ready => s.counts.ready,
                    Segment::Review => s.counts.review,
                    Segment::Matches => s.counts.matches,
                };
                let text = format!("{} {n}", t(seg.key()));
                let on = s.segment == seg;
                let rich = if on {
                    RichText::new(text).strong().color(tokens.on_accent)
                } else {
                    RichText::new(text).color(tokens.text_primary)
                };
                let mut b = egui::Button::new(rich).selected(on);
                if on {
                    b = b.fill(tokens.accent);
                }
                if ui.add(b).clicked() && !on {
                    s.segment = seg;
                    changed = true;
                }
            }
        }
        ui.add_space(12.0);
        let r = ui.label(RichText::new(t("bank_feed.show")).color(tokens.text_secondary));
        let mut show = s.show;
        egui::ComboBox::from_id_salt("bank_feed.show")
            .selected_text(t(s.show.key()))
            .show_ui(ui, |ui| {
                for v in Show::ALL {
                    ui.selectable_value(&mut show, v, t(v.key()));
                }
            })
            .response
            .labelled_by(r.id);
        if show != s.show {
            s.show = show;
            s.voided_only = false;
            changed = true;
        }
        if s.show == Show::Accepted {
            let text = t_with(
                "bank_feed.chip.entry_voided",
                &[("n", &s.counts.entry_voided.to_string())],
            );
            if ui
                .add(egui::Button::new(text).selected(s.voided_only))
                .on_hover_text(t("bank_feed.chip.entry_voided.tip"))
                .clicked()
            {
                s.voided_only = !s.voided_only;
                changed = true;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add_sized(
                [240.0, 24.0],
                egui::TextEdit::singleline(&mut s.filter).hint_text(t("bank_feed.filter.hint")),
            );
            let r = r.on_hover_text(t("bank_feed.filter"));
            if r.changed() {
                changed = true;
            }
        });
        if changed {
            s.offset = 0;
            s.list.clear_selection();
            s.reload();
        }
    });
}

fn badge(ui: &Ui, rect: egui::Rect, tokens: &Tokens, why: Why) {
    let (fill, color) = match why {
        Why::Rule | Why::History => (tokens.neutral_chip, tokens.neutral_chip_text),
        Why::Match | Why::Transfer => (tokens.info_tint, tokens.info),
        Why::Ai { high: true } => (tokens.accent_tint, tokens.accent),
        Why::Ai { high: false } | Why::Duplicate | Why::EntryVoided => {
            (tokens.warn_tint, tokens.warn_text)
        }
        _ => (tokens.warn_surface, tokens.warn_text),
    };
    let p = ui.painter();
    let g = p.layout_no_wrap(t(why.key()), egui::FontId::proportional(12.0), color);
    let r = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.top() + 6.0),
        g.size() + Vec2::new(12.0, 4.0),
    );
    if why != Why::NeedsYou {
        p.rect_filled(r, 4.0, fill);
    }
    p.galley(r.min + Vec2::new(6.0, 2.0), g, color);
}

/// What the row will be recorded as: a payee line and an account line.
fn recorded(s: &BankFeedScreen, r: &FeedRow, why: Why) -> (String, String) {
    if r.status == FeedStatus::Excluded {
        return (t("bank_feed.recorded.excluded"), String::new());
    }
    let payee = r
        .assigned_contact
        .as_ref()
        .or(r.suggested_contact.as_ref())
        .map(|c| c.name.clone())
        .or_else(|| r.assigned_memo.clone());
    let account = r
        .assigned_account
        .as_ref()
        .or(r.suggested_account.as_ref())
        .map(|a| a.name.clone());
    match why {
        Why::Match => (
            payee.unwrap_or_else(|| t("bank_feed.recorded.match")),
            t("bank_feed.recorded.match.sub"),
        ),
        Why::Transfer => (
            t("bank_feed.recorded.transfer"),
            t("bank_feed.recorded.transfer.sub"),
        ),
        Why::NeedsYou => (t("bank_feed.recorded.choose"), String::new()),
        Why::EntryVoided => (
            payee.unwrap_or_default(),
            t("bank_feed.recorded.entry_voided"),
        ),
        _ => {
            let mut sub = account.unwrap_or_default();
            if why == (Why::Ai { high: false }) {
                sub.push('?');
            }
            let _ = s;
            (payee.unwrap_or_else(|| r.description.clone()), sub)
        }
    }
}

fn column_headers(ui: &mut Ui, tokens: &Tokens, s: &mut BankFeedScreen) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::hover());
    let all_selected = !s.list.rows().is_empty() && s.list.selected().len() == s.list.rows().len();
    let mut all = all_selected;
    let cb = ui.put(
        egui::Rect::from_min_size(rect.min + Vec2::new(10.0, 4.0), Vec2::new(18.0, 18.0)),
        egui::Checkbox::without_text(&mut all),
    );
    cb.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::CheckBox,
            true,
            all_selected,
            t("bank_feed.select_all"),
        )
    });
    if cb.clicked() {
        s.list.toggle_all();
    }
    let p = ui.painter();
    let font = egui::FontId::proportional(13.0);
    let y = rect.center().y;
    let amount_right = rect.left() + amount_right_x(width);
    for (x, key) in [
        (44.0, "bank_feed.col.date"),
        (106.0, "bank_feed.col.bank"),
        (amount_right - rect.left() + 20.0, "bank_feed.col.recorded"),
        (width - 112.0, "bank_feed.col.why"),
    ] {
        p.text(
            egui::pos2(rect.left() + x, y),
            Align2::LEFT_CENTER,
            t(key),
            font.clone(),
            tokens.text_secondary,
        );
    }
    p.text(
        egui::pos2(amount_right, y),
        Align2::RIGHT_CENTER,
        t("bank_feed.col.amount"),
        font,
        tokens.text_secondary,
    );
    p.hline(rect.x_range(), rect.bottom(), (1.0, tokens.border_row));
}

fn amount_right_x(width: f32) -> f32 {
    (width * 0.52).clamp(300.0, 520.0)
}

fn list(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens) {
    if s.rows.is_empty() {
        ui.add_space(12.0);
        let key = match s.show {
            Show::Waiting => "bank_feed.empty",
            Show::Accepted => "bank_feed.empty.accepted",
            Show::Excluded => "bank_feed.empty.excluded",
        };
        form::hint(ui, tokens, &t(key));
        return;
    }
    column_headers(ui, tokens, s);
    let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
    let cursor = s.list.current();
    let total = s.rows.len();
    let mut toggled = None;
    let mut clicked = None;
    let rows = s.rows.clone();
    egui::ScrollArea::vertical()
        .auto_shrink([false, true])
        .max_height((ui.available_height() - 28.0).max(80.0))
        .show(ui, |ui| {
            for (i, r) in rows.iter().enumerate() {
                let why = s.why(r);
                let width = ui.available_width();
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, ROW_H), Sense::click());
                let label = t_with(
                    "bank_feed.row",
                    &[("n", &(i + 1).to_string()), ("total", &total.to_string())],
                );
                let is_cur = cursor == Some(r.id);
                resp.widget_info(|| {
                    egui::WidgetInfo::selected(egui::Role::Button, true, is_cur, &label)
                });
                if is_cur {
                    resp.scroll_to_me(None);
                }
                // The row's fill first, so its check box draws on top.
                if ui.is_rect_visible(rect) {
                    let fill = if is_cur {
                        Some(tokens.accent_tint)
                    } else if why == Why::NeedsYou {
                        Some(tokens.warn_surface)
                    } else if resp.hovered() {
                        Some(tokens.surface_subtle)
                    } else {
                        None
                    };
                    if let Some(f) = fill {
                        ui.painter().rect_filled(rect, 0.0, f);
                    }
                }
                let mut sel = s.list.is_selected(r.id);
                let cb = ui.put(
                    egui::Rect::from_min_size(
                        rect.min + Vec2::new(10.0, 12.0),
                        Vec2::new(18.0, 18.0),
                    ),
                    egui::Checkbox::without_text(&mut sel),
                );
                let cb_label = t_with("bank_feed.row.select", &[("n", &(i + 1).to_string())]);
                cb.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::Role::CheckBox,
                        true,
                        s.list.is_selected(r.id),
                        &cb_label,
                    )
                });
                if cb.clicked() {
                    toggled = Some(r.id);
                }
                if resp.clicked() {
                    clicked = Some(r.id);
                }
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let p = ui.painter();

                p.hline(rect.x_range(), rect.bottom(), (1.0, tokens.border_row));
                let mono = egui::FontId::monospace(13.0);
                let font = egui::FontId::proportional(14.0);
                let small = egui::FontId::proportional(12.5);
                let top = rect.top() + 14.0;
                let date = crate::shell::us_date(r.date);
                p.text(
                    egui::pos2(rect.left() + 44.0, top),
                    Align2::LEFT_CENTER,
                    date.get(..5).unwrap_or(&date),
                    mono.clone(),
                    tokens.text_primary,
                );
                let amount_right = rect.left() + amount_right_x(width);
                let amount = model::display_amount(r.amount).format(style);
                let ag = p.layout_no_wrap(amount, mono.clone(), tokens.text_primary);
                let amount_left = amount_right - ag.size().x;
                p.galley(
                    egui::pos2(amount_left, top - ag.size().y / 2.0),
                    ag,
                    tokens.text_primary,
                );
                let mut bank = rect;
                bank.set_left(rect.left() + 106.0);
                bank.set_right(amount_left - 12.0);
                p.with_clip_rect(bank.intersect(p.clip_rect())).text(
                    egui::pos2(bank.left(), top),
                    Align2::LEFT_CENTER,
                    &r.description,
                    mono,
                    tokens.text_primary,
                );
                let (line1, line2) = recorded(s, r, why);
                let mut rec = rect;
                rec.set_left(amount_right + 20.0);
                rec.set_right(rect.right() - 120.0);
                let clip = p.with_clip_rect(rec.intersect(p.clip_rect()));
                let first_color = if why == Why::NeedsYou {
                    tokens.text_tertiary
                } else {
                    tokens.text_primary
                };
                clip.text(
                    egui::pos2(rec.left(), top),
                    Align2::LEFT_CENTER,
                    line1,
                    font,
                    first_color,
                );
                clip.text(
                    egui::pos2(rec.left(), top + 17.0),
                    Align2::LEFT_CENTER,
                    line2,
                    small,
                    tokens.text_secondary,
                );
                let mut why_rect = rect;
                why_rect.set_left(rect.right() - 112.0);
                badge(ui, why_rect, tokens, why);
            }
        });
    if let Some(id) = toggled {
        s.list.toggle(id);
    }
    if let Some(id) = clicked {
        s.list.focus(id);
    }
}

fn footnote(s: &BankFeedScreen, ui: &mut Ui, tokens: &Tokens) {
    ui.add_space(6.0);
    let mut text = t_with(
        "bank_feed.footnote.showing",
        &[
            ("shown", &s.rows.len().to_string()),
            ("total", &s.total.to_string()),
        ],
    );
    // AI off or offline: no AI mention (P26 acceptance).
    let key = if s.ai_on {
        "bank_feed.footnote.ai"
    } else {
        "bank_feed.footnote.no_ai"
    };
    text.push_str(" \u{b7} ");
    text.push_str(&t(key));
    ui.label(RichText::new(text).size(12.5).color(tokens.text_secondary));
}

/// "Accept all from this rule" (Q-P26a, Q-P26n): the cursor's row was
/// staged by an auto-accept rule; one click accepts every waiting row the
/// rule staged (logged, undoable), 500 a call. The rule's change token is
/// sent exactly as listed (409 `RULE_CHANGED` refreshes).
fn rule_banner(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens) {
    if s.show != Show::Waiting || s.view_only {
        return;
    }
    let Some(r) = s.list.current().and_then(|id| s.row(id)) else {
        return;
    };
    let Some(rule) = r.suggested_rule.clone().filter(|x| x.auto_accept) else {
        return;
    };
    let Some(token) = rule.updated_at.clone() else {
        return;
    };
    if !r.status.is_waiting() {
        return;
    }
    let n = s
        .rows
        .iter()
        .filter(|x| {
            x.status.is_waiting() && x.suggested_rule.as_ref().is_some_and(|y| y.id == rule.id)
        })
        .count();
    let connection = s.current_account().map(|a| a.connection);
    let mut go = false;
    egui::Frame::new()
        .fill(tokens.info_tint)
        .inner_margin(egui::Margin::symmetric(16, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(t_with(
                        "bank_feed.rule_banner",
                        &[("rule", &rule.name), ("n", &n.to_string())],
                    ))
                    .color(tokens.text_primary),
                );
                go = ui
                    .add_enabled(
                        s.act.is_none(),
                        egui::Button::new(t("bank_feed.rule_banner.accept")),
                    )
                    .on_hover_text(t("bank_feed.rule_banner.tip"))
                    .clicked();
            });
        });
    if go {
        super::send::start(
            s,
            super::send::Send::Rule(cl_api::RuleAccept {
                rule_id: rule.id,
                kind: rule.kind,
                updated_at: token,
                connection,
                after: None,
            }),
            None,
        );
    }
}

/// How long the Undo toast stays.
const TOAST_FOR: std::time::Duration = std::time::Duration::from_secs(12);

/// The Undo toast after an accept (Q-P26h): "n accepted · Undo".
fn toast(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens) {
    let Some(x) = &s.toast else {
        return;
    };
    if x.at.elapsed() >= TOAST_FOR {
        s.toast = None;
        return;
    }
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(500));
    let (target, rows) = (x.target.clone(), x.rows);
    let mut undo = false;
    egui::Frame::new()
        .fill(tokens.surface_subtle)
        .inner_margin(egui::Margin::symmetric(16, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(t_with("bank_feed.toast", &[("n", &rows.to_string())]));
                undo = ui
                    .add_enabled(
                        s.act.is_none() && !s.view_only,
                        egui::Button::new(t("bank_feed.toast.undo")),
                    )
                    .on_hover_text(t("bank_feed.toast.undo.tip"))
                    .clicked();
            });
        });
    if undo {
        s.toast = None;
        super::send::start(s, super::send::Send::Undo(target, false), None);
    }
}

/// 409 `FEED_UNDO_CHANGED`: the entry was edited after it was accepted.
fn undo_changed(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens) {
    let Some((target, n)) = s.confirm.clone() else {
        return;
    };
    let mut answer = None;
    form::card(ui, tokens, |ui| {
        ui.add(egui::Label::new(t_with("bank_feed.undo_changed", &[("n", &n.to_string())])).wrap());
        ui.horizontal(|ui| {
            if crate::widgets::destructive_button(ui, &t("bank_feed.undo_changed.yes")).clicked() {
                answer = Some(true);
            }
            if ui.button(t("bank_feed.undo_changed.no")).clicked() {
                answer = Some(false);
            }
        });
    });
    match answer {
        Some(true) => {
            s.confirm = None;
            super::send::start(s, super::send::Send::Undo(target, true), None);
        }
        Some(false) => s.confirm = None,
        None => {}
    }
}

/// The closing-date password for locked rows (plan §5).
fn closing(s: &mut BankFeedScreen, ui: &Ui, tokens: &Tokens) {
    use crate::forms::closing_prompt::ClosingClick;
    s.closing.set_ask(
        "bank_feed.closing.ask",
        s.closing.pending().map_or(1, |_| 1),
    );
    match s.closing.window(ui.ctx(), tokens, "bank_feed") {
        ClosingClick::Submit => {
            if let Some((again, pw)) = s.closing.submit() {
                let busy = s.act.is_some();
                super::send::start(s, again, Some(pw));
                if busy {
                    s.closing.nothing_sent();
                }
            }
        }
        ClosingClick::Cancel => s.closing.cancel(),
        ClosingClick::None => {}
    }
}

/// The rule offer (Q-P26k): the engine saw the same choice often enough.
fn rule_offer(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens) {
    let n = s.rules.ideas.len();
    if n == 0 || s.view_only || s.rules.open || s.show != Show::Waiting {
        return;
    }
    let mut open = false;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(crate::i18n::t_count("bank_feed.rule_offer", n, &[]))
                .color(tokens.text_secondary),
        );
        open = ui.small_button(t("bank_feed.rule_offer.open")).clicked();
    });
    if open {
        super::rules::open(s);
    }
}
