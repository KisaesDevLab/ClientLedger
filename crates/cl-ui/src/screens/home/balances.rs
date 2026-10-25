//! Account balances (P16 step 2, D9): each active bank and card account
//! with the balance the engine reports, "In your books", and the bank
//! file's balance, a dash until bank-file import (U-77, P25; Q-P16d).
//!
//! The figure is the engine's stored balance: the cash position of
//! `dashboard-exact-v1` (U-76) when the engine has it, else the account's
//! own stored balance (`Account.balance`, read exactly). Both are debits
//! minus credits, so a card's figure is flipped for display the way the
//! Chart of Accounts flips it (`DisplaySign`, Q-P12h, Q-241): an amount
//! owed reads positive. Nothing here adds, subtracts or re-derives a
//! balance (Non-negotiable 7), and no amount is logged or put in the
//! control channel's state.

use cl_api::ApiError;
use cl_core::ids::AccountId;
use cl_core::{Money, OwnerFacing};
use cl_model::register::DisplaySign;
use eframe::egui::{self, Align, Layout, Margin, RichText, Stroke, Ui, Vec2};

use super::{HomeData, HomeScreen, money_accounts};
use crate::i18n::t;
use crate::screens::lists::accounts::model::kind_of;
use crate::theme::{Tokens, styles};

/// Room between columns and around each figure.
const COLUMN_PAD: f32 = 12.0;

/// One row of the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BalanceRow {
    /// The account.
    pub account: AccountId,
    /// "1010 · Operating Checking".
    pub label: String,
    /// The engine's balance (`None` when it sent none).
    pub books: Option<Money>,
}

/// "1010 · Operating Checking", or the name alone.
fn number_name(number: Option<&str>, name: &str) -> String {
    match number.filter(|n| !n.is_empty()) {
        Some(n) => format!("{n} \u{b7} {name}"),
        None => name.to_string(),
    }
}

/// The rows: the cash position when the engine reports it exactly, else
/// the chart's stored balances; `None` while neither has arrived.
#[must_use]
pub(crate) fn rows(data: &HomeData, exact: bool) -> Option<Vec<BalanceRow>> {
    if exact
        && let Some(cash) = data
            .summary
            .value
            .as_ref()
            .and_then(|s| s.cash_position.as_ref())
    {
        let banks = cash.bank_accounts.iter().map(|c| (c, DisplaySign::AsSent));
        let cards = cash.credit_cards.iter().map(|c| (c, DisplaySign::Negated));
        return Some(
            banks
                .chain(cards)
                .map(|(c, sign)| BalanceRow {
                    account: c.id,
                    label: data.account(c.id).map_or_else(
                        || number_name(c.account_number.as_deref(), &c.name),
                        crate::screens::register::account_text,
                    ),
                    books: sign.apply(c.balance).ok(),
                })
                .collect(),
        );
    }
    let accounts = data.accounts.value.as_ref()?;
    Some(
        money_accounts(accounts)
            .into_iter()
            .map(|a| BalanceRow {
                account: a.id,
                label: crate::screens::register::account_text(a),
                books: a.balance.and_then(|b| {
                    DisplaySign::for_stored_balance(kind_of(a.fields.account_type))
                        .apply(b)
                        .ok()
                }),
            })
            .collect(),
    )
}

/// Where the rows came from (the control channel).
#[must_use]
pub(crate) fn source(data: &HomeData, exact: bool) -> &'static str {
    let cash = exact
        && data
            .summary
            .value
            .as_ref()
            .is_some_and(|s| s.cash_position.is_some());
    if cash {
        "cash_position"
    } else if data.accounts.value.is_some() {
        "accounts"
    } else {
        "none"
    }
}

/// A card's heading with the line under it.
pub(super) fn heading(ui: &mut Ui, tokens: &Tokens, text: &str) {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(14, 11))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(
                    RichText::new(text)
                        .text_style(styles::section())
                        .color(tokens.text_primary),
                )
                .selectable(false),
            );
        });
    rule(ui, tokens.border_row);
}

/// A full-width line.
pub(super) fn rule(ui: &mut Ui, color: egui::Color32) {
    let r = ui.available_rect_before_wrap();
    ui.painter()
        .hline(r.x_range(), r.top(), Stroke::new(1.0, color));
}

/// A padded line of muted text.
pub(super) fn note(ui: &mut Ui, tokens: &Tokens, text: &str) {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(14, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(RichText::new(text).size(12.0).color(tokens.text_tertiary)).wrap(),
            );
        });
}

/// "Couldn't load …" with Try again and Copy details (Q-119: an error id
/// and the scrubbed detail, no amounts or names). True when Try again was
/// chosen.
pub(super) fn failed(ui: &mut Ui, tokens: &Tokens, key: &str, error: &ApiError) -> bool {
    let mut retry = false;
    egui::Frame::new()
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(t(key)).color(tokens.warn_text)).wrap());
            ui.horizontal_wrapped(|ui| {
                if ui.button(t("home.try_again")).clicked() {
                    retry = true;
                }
                if ui.button(t("home.copy_details")).clicked() {
                    let detail = crate::errors::scrub_line(&error.scrubbed_detail());
                    ui.ctx().copy_text(crate::errors::details_text(
                        crate::errors::fresh_id(),
                        key,
                        &detail,
                        &[],
                    ));
                }
            });
        });
    retry
}

/// The Account balances card. True when Try again was chosen.
pub(super) fn show(ui: &mut Ui, home: &HomeScreen, tokens: &Tokens) -> bool {
    heading(ui, tokens, &t("home.balances.title"));
    let data = &home.data;
    let Some(rows) = rows(data, home.exact) else {
        if let Some(e) = &data.accounts.error {
            return failed(ui, tokens, "home.balances.failed", e);
        }
        note(ui, tokens, &t("home.balances.loading"));
        return false;
    };
    if rows.is_empty() {
        note(ui, tokens, &t("home.balances.empty"));
        return false;
    }
    let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
    let header = |ui: &mut Ui, text: &str, right: bool| {
        let rich = RichText::new(text).size(13.0).color(tokens.text_secondary);
        let label = egui::Label::new(rich).selectable(false);
        if right {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.add(label));
        } else {
            ui.add(label.truncate());
        }
    };
    // Each figure column is as wide as its header or widest figure (the
    // mockup's table); the account name takes the rest, so it is cut only
    // when the card is truly narrow.
    let figures: Vec<String> = rows
        .iter()
        .map(|r| {
            r.books
                .map_or_else(|| "\u{2014}".to_string(), |m| m.format(style))
        })
        .collect();
    let measure = |s: &str, font: egui::FontId| {
        ui.painter()
            .layout_no_wrap(s.to_owned(), font, tokens.text_primary)
            .size()
            .x
    };
    let head = egui::FontId::proportional(13.0);
    let mono = egui::TextStyle::Monospace.resolve(ui.style());
    let books_w = figures
        .iter()
        .map(|f| measure(f, mono.clone()))
        .fold(measure(&t("home.balances.books"), head.clone()), f32::max)
        + COLUMN_PAD;
    let file_w =
        measure(&t("home.balances.bank_file"), head).max(measure("\u{2014}", mono)) + COLUMN_PAD;
    let widths = [books_w, file_w];
    table_row(ui, widths, 30.0, |ui, col| match col {
        0 => header(ui, &t("home.balances.account"), false),
        1 => header(ui, &t("home.balances.books"), true),
        _ => header(ui, &t("home.balances.bank_file"), true),
    });
    for (row, figure) in rows.iter().zip(&figures) {
        rule(ui, tokens.border_row);
        table_row(ui, widths, 31.0, |ui, col| match col {
            0 => {
                ui.add(
                    egui::Label::new(RichText::new(&row.label).color(tokens.text_primary))
                        .truncate(),
                );
            }
            1 => {
                let text = figure.as_str();
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(text)
                                .family(egui::FontFamily::Monospace)
                                .color(tokens.text_primary),
                        )
                        .selectable(false),
                    );
                });
            }
            _ => {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new("\u{2014}")
                                .family(egui::FontFamily::Monospace)
                                .color(tokens.text_tertiary),
                        )
                        .selectable(false),
                    )
                    .on_hover_text(t("home.balances.bank_file.tip"));
                });
            }
        });
    }
    rule(ui, tokens.border_row);
    note(ui, tokens, &t("home.balances.note"));
    false
}

/// One row of the table: the account column takes what the two figure
/// columns (`figures` wide) leave.
fn table_row(ui: &mut Ui, figures: [f32; 2], h: f32, mut cell: impl FnMut(&mut Ui, usize)) {
    egui::Frame::new()
        .inner_margin(Margin::symmetric(14, 0))
        .show(ui, |ui| {
            let width = ui.available_width();
            let [books_w, file_w] = figures;
            let name_w = (width - books_w - file_w - 8.0).max(80.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (col, w) in [(0, name_w), (1, books_w), (2, file_w)] {
                    let layout = if col == 0 {
                        Layout::left_to_right(Align::Center)
                    } else {
                        Layout::right_to_left(Align::Center)
                    };
                    ui.allocate_ui_with_layout(Vec2::new(w, h), layout, |ui| {
                        ui.set_min_size(Vec2::new(w, h));
                        cell(ui, col);
                    });
                }
            });
        });
}
