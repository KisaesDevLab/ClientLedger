//! The Bank Feed window's layout in mockup order: the header (title,
//! account picker, last file imported, Import Bank File, Rules), the
//! segments (All, Ready, Needs review, Matches) with the Show selector,
//! the "Entry voided" chip and the filter, the bulk bar, the list (select,
//! date, from the bank, amount, will be recorded as, why) and the detail
//! panel, and the footnote (Q-P26h deviations: Show selector, Entry voided
//! chip).

use cl_api::{FeedRow, FeedStatus};
use cl_model::bank_feed::{self as model, Segment, Show, Why};
use eframe::egui::{self, RichText, Sense, Ui, Vec2};

use super::{BankFeedScreen, bulk};
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;

/// The detail panel's width (mockup), narrower on a narrow window.
const DETAIL_W: f32 = 330.0;
const DETAIL_MIN_W: f32 = 270.0;
const ROW_H: f32 = 46.0;
/// Below this many points side by side, the detail panel goes under the
/// list (P26 QA-1, QA-23: 1024 wide, 200% zoom).
const STACK_BELOW: f32 = 700.0;
/// Below this list width, a row puts what it will be recorded as on its
/// second line.
const COMPACT_BELOW: f32 = 620.0;
/// The amount column and the Why badge column.
const AMOUNT_W: f32 = 96.0;
const WHY_W: f32 = 112.0;
/// The header and toolbars wrap below this width.
pub(crate) const WRAP_BELOW: f32 = 980.0;

/// Where the list's columns go in a list `width` points wide (left and
/// right edges from the row's left; P26 QA-1, QA-23).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Cols {
    /// Recorded-as and Why sit on the second line.
    pub compact: bool,
    pub date: f32,
    pub bank: (f32, f32),
    pub amount_right: f32,
    pub recorded: (f32, f32),
    pub why: f32,
}

/// The columns for `width`.
pub(crate) fn columns(width: f32) -> Cols {
    if width < COMPACT_BELOW {
        // Line 1: bank text, amount at the right. Line 2: date, what it
        // will be recorded as, the badge at the right.
        let why = (width - WHY_W + 8.0).max(150.0);
        let amount_right = width - 10.0;
        return Cols {
            compact: true,
            date: 44.0,
            bank: (44.0, (amount_right - AMOUNT_W - 8.0).max(44.0)),
            amount_right,
            recorded: (98.0, (why - 8.0).max(98.0)),
            why,
        };
    }
    let mid = width - 106.0 - WHY_W - AMOUNT_W - 32.0;
    let bank_w = (mid * 0.45).max(110.0);
    let amount_right = 106.0 + bank_w + 12.0 + AMOUNT_W;
    let why = width - WHY_W;
    Cols {
        compact: false,
        date: 44.0,
        bank: (106.0, 106.0 + bank_w),
        amount_right,
        recorded: (amount_right + 20.0, why - 8.0),
        why,
    }
}

/// `text` laid out on one line no wider than `max_w`, cut with "…".
fn fit(
    p: &egui::Painter,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    max_w: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(font, color),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(1.0));
    p.layout_job(job)
}

/// Draws `text` left-aligned at `x`, centered on `y`, cut at `max_w`.
#[allow(clippy::many_single_char_names, reason = "painter and coordinates")]
fn put(
    p: &egui::Painter,
    x: f32,
    y: f32,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    max_w: f32,
) {
    let g = fit(p, text, font, color, max_w);
    let h = g.size().y;
    p.galley(egui::pos2(x, y - h / 2.0), g, color);
}

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
    let side = |s: &mut BankFeedScreen, ui: &mut Ui| {
        if s.rules.open {
            super::rules::draw(s, ui, &tokens);
        } else {
            super::detail::draw(s, ui, &tokens);
        }
    };
    if avail.x < STACK_BELOW {
        // Narrow window or large zoom: the panel goes under the list.
        let list_h = (avail.y * 0.5).max(160.0);
        ui.allocate_ui_with_layout(
            Vec2::new(avail.x, list_h),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_max_height(list_h);
                list(s, ui, &tokens, 52.0);
                footnote(s, ui, &tokens);
            },
        );
        ui.separator();
        // The panel scrolls inside what is left of the window.
        let rest = ui.available_size();
        ui.allocate_ui_with_layout(rest, egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_max_height(rest.y);
            side(s, ui);
        });
        return;
    }
    let detail_w = (avail.x * 0.3).clamp(DETAIL_MIN_W, DETAIL_W);
    let list_w = avail.x - detail_w - 12.0;
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(list_w, avail.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_max_width(list_w);
                // Room for the footnote's two lines.
                list(s, ui, &tokens, 52.0);
                footnote(s, ui, &tokens);
            },
        );
        ui.separator();
        ui.allocate_ui_with_layout(
            Vec2::new(detail_w, avail.y),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_max_width(detail_w);
                side(s, ui);
            },
        );
    });
}

fn header(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens) {
    ui.add_space(8.0);
    let narrow = ui.available_width() < WRAP_BELOW;
    let combo_w = (ui.available_width() * 0.35).clamp(160.0, 260.0);
    let body = |ui: &mut Ui| {
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
            .truncate()
            .width(combo_w)
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
        let buttons = |s: &mut BankFeedScreen, ui: &mut Ui| {
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
        };
        if narrow {
            // Narrow or zoomed: the buttons wrap after the picker instead
            // of covering it (P26 QA-23).
            ui.add_space(12.0);
            buttons(s, ui);
        } else {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                buttons(s, ui);
            });
        }
    };
    if narrow {
        ui.horizontal_wrapped(body);
    } else {
        ui.horizontal(body);
    }
    ui.add_space(6.0);
}

fn account_text(label: &str, waiting: u64) -> String {
    format!("{label} ({waiting})")
}

fn controls(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens) {
    let narrow = ui.available_width() < WRAP_BELOW;
    let filter_w = (ui.available_width() * 0.3).clamp(140.0, 240.0);
    let body = |ui: &mut Ui| {
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
        let mut filter = |ui: &mut Ui| {
            let r = ui.add_sized(
                [filter_w, 24.0],
                egui::TextEdit::singleline(&mut s.filter).hint_text(t("bank_feed.filter.hint")),
            );
            r.on_hover_text(t("bank_feed.filter")).changed()
        };
        let edited = if narrow {
            // The filter wraps to its own place instead of sitting on the
            // Show list (P26 QA-23).
            filter(ui)
        } else {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), filter)
                .inner
        };
        if changed || edited {
            s.offset = 0;
            s.list.clear_selection();
            s.reload();
        }
    };
    if narrow {
        ui.horizontal_wrapped(body);
    } else {
        ui.horizontal(body);
    }
}

fn badge(ui: &Ui, rect: egui::Rect, tokens: &Tokens, why: Why) {
    let (fill, color) = match why {
        Why::Rule | Why::History | Why::Accepted | Why::Excluded => {
            (tokens.neutral_chip, tokens.neutral_chip_text)
        }
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
        // The other side found: name its account (P26 QA-18 note, C-1).
        Why::Transfer => match r
            .transfer_candidate
            .as_ref()
            .and_then(|h| h.account_name.clone())
        {
            Some(other) => (
                t_with(
                    if r.is_money_out() {
                        "bank_feed.recorded.transfer_to"
                    } else {
                        "bank_feed.recorded.transfer_from"
                    },
                    &[("account", &other)],
                ),
                t("bank_feed.recorded.transfer.found"),
            ),
            None => (
                t("bank_feed.recorded.transfer"),
                t("bank_feed.recorded.transfer.sub"),
            ),
        },
        Why::NeedsYou => (t("bank_feed.recorded.choose"), String::new()),
        // A look-alike reads like any row needing a choice (P26 QA-21).
        Why::Duplicate if account.is_none() => (t("bank_feed.recorded.choose"), String::new()),
        Why::Accepted => {
            let how = s.accepted_as.get(&r.id).copied();
            let sub = match (r.status, how) {
                (_, Some("deposit")) => t("bank_feed.recorded.deposited"),
                (_, Some("transfer")) => t("bank_feed.recorded.transferred"),
                (FeedStatus::Matched, _) => t("bank_feed.recorded.matched"),
                _ => account.unwrap_or_default(),
            };
            (
                payee.unwrap_or_else(|| super::bank_text(r).to_string()),
                sub,
            )
        }
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

#[allow(clippy::many_single_char_names, reason = "painter and coordinates")]
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
    let c = columns(width);
    let x0 = rect.left();
    let color = tokens.text_secondary;
    if c.compact {
        // Line 1 holds the bank text and the amount; line 2 the date, what
        // it will be recorded as and why.
        put(
            p,
            x0 + c.bank.0,
            y,
            &t("bank_feed.col.bank"),
            font.clone(),
            color,
            c.bank.1 - c.bank.0,
        );
    } else {
        put(
            p,
            x0 + c.date,
            y,
            &t("bank_feed.col.date"),
            font.clone(),
            color,
            c.bank.0 - c.date - 4.0,
        );
        put(
            p,
            x0 + c.bank.0,
            y,
            &t("bank_feed.col.bank"),
            font.clone(),
            color,
            c.bank.1 - c.bank.0,
        );
        put(
            p,
            x0 + c.recorded.0,
            y,
            &t("bank_feed.col.recorded"),
            font.clone(),
            color,
            c.recorded.1 - c.recorded.0,
        );
        put(
            p,
            x0 + c.why,
            y,
            &t("bank_feed.col.why"),
            font.clone(),
            color,
            WHY_W - 4.0,
        );
    }
    let g = fit(p, &t("bank_feed.col.amount"), font, color, AMOUNT_W);
    let h = g.size().y;
    p.galley(
        egui::pos2(x0 + c.amount_right - g.size().x, y - h / 2.0),
        g,
        color,
    );
    p.hline(rect.x_range(), rect.bottom(), (1.0, tokens.border_row));
}

fn list(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens, reserve: f32) {
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
        .max_height((ui.available_height() - reserve).max(80.0))
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
                let second = top + 17.0;
                let c = columns(width);
                let x0 = rect.left();
                let date = crate::shell::us_date(r.date);
                let date = date.get(..5).unwrap_or(&date);
                // The amount never shrinks: the text beside it is cut first.
                let amount = model::display_amount(r.amount).format(style);
                let ag = p.layout_no_wrap(amount, mono.clone(), tokens.text_primary);
                let amount_left = x0 + c.amount_right - ag.size().x;
                p.galley(
                    egui::pos2(amount_left, top - ag.size().y / 2.0),
                    ag,
                    tokens.text_primary,
                );
                let bank_right = (x0 + c.bank.1).min(amount_left - 10.0);
                let bank_w = bank_right - (x0 + c.bank.0);
                put(
                    p,
                    x0 + c.bank.0,
                    top,
                    super::bank_text(r),
                    mono.clone(),
                    tokens.text_primary,
                    bank_w,
                );
                let (line1, line2) = recorded(s, r, why);
                let first_color = if why == Why::NeedsYou {
                    tokens.text_tertiary
                } else {
                    tokens.text_primary
                };
                let rec_w = c.recorded.1 - c.recorded.0;
                if c.compact {
                    put(
                        p,
                        x0 + c.date,
                        second,
                        date,
                        small.clone(),
                        tokens.text_secondary,
                        50.0,
                    );
                    let text = if line2.is_empty() {
                        line1
                    } else {
                        format!("{line1} \u{b7} {line2}")
                    };
                    put(
                        p,
                        x0 + c.recorded.0,
                        second,
                        &text,
                        small,
                        first_color,
                        rec_w,
                    );
                    let why_rect = egui::Rect::from_min_max(
                        egui::pos2(x0 + c.why, rect.top() + 18.0),
                        rect.max,
                    );
                    badge(ui, why_rect, tokens, why);
                } else {
                    put(p, x0 + c.date, top, date, mono, tokens.text_primary, 60.0);
                    put(p, x0 + c.recorded.0, top, &line1, font, first_color, rec_w);
                    put(
                        p,
                        x0 + c.recorded.0,
                        second,
                        &line2,
                        small,
                        tokens.text_secondary,
                        rec_w,
                    );
                    let mut why_rect = rect;
                    why_rect.set_left(x0 + c.why);
                    badge(ui, why_rect, tokens, why);
                }
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
    // Where accepted lines show in the audit trail (mockup, P26 C-3).
    text.push_str(" \u{b7} ");
    text.push_str(&t("bank_feed.footnote.audit"));
    ui.add(egui::Label::new(RichText::new(text).size(12.5).color(tokens.text_secondary)).wrap());
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

/// How long the Undo toast stays on screen (until the next action at most).
const TOAST_FOR: std::time::Duration = std::time::Duration::from_secs(30);

/// The Undo toast after an accept (Q-P26h): "n accepted · Undo".
fn toast(s: &mut BankFeedScreen, ui: &mut Ui, tokens: &Tokens) {
    let dt = ui.input(|i| i.stable_dt).min(1.0);
    let Some(x) = s.toast.as_mut() else {
        return;
    };
    x.shown += std::time::Duration::from_secs_f32(dt.max(0.0));
    if x.shown >= TOAST_FOR {
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
