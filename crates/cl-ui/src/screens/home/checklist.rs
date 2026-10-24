//! The setup checklist (P16 step 5, Q-63): "Setup checklist 5 of 6 done ·
//! Set up email for invoices" at the right of Home's title row, and the full
//! list in a card above the lanes when opened (open by itself while fewer
//! than half the steps are done, so a new company sees it first).
//!
//! Every tick comes from a read, never from a click: the company profile
//! (name and address), the chart (a bank account), any posted transaction,
//! any bank-feed item (an imported OFX, QFX or CSV bank file lands in the
//! bank feed), the email settings, and this computer's record
//! of the last backup. The engine keeps no opening-balance mark on an
//! account (verified: `Account` has none), so that step is left out and the
//! list has six steps (Q-P16j). "Hide checklist" sets
//! `CompanyPrefs::setup_checklist_dismissed`; the checklist also hides
//! itself once everything is done.

use cl_api::{ApiError, CompanyProfileUpdate};
use cl_model::home::{Checklist, ChecklistItem};
use cl_model::register::AccountClass;
use eframe::egui::{
    self, Align, Layout, Margin, Pos2, RichText, Role, Sense, Stroke, Ui, Vec2, WidgetInfo,
};

use super::HomeScreen;
use super::balances::{heading, rule};
use crate::i18n::{t, t_with};
use crate::screens::company::form::activated;
use crate::theme::Tokens;

/// The steps Home shows: all but opening balances (no engine signal yet,
/// Q-P16j).
pub(crate) const ITEMS: [ChecklistItem; 6] = [
    ChecklistItem::CompanyInfo,
    ChecklistItem::BankAccount,
    ChecklistItem::FirstTransaction,
    ChecklistItem::BankFile,
    ChecklistItem::Email,
    ChecklistItem::FirstBackup,
];

/// What the checklist reads from the engine (one job; each failed read
/// leaves its step open).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    /// The company has a name and a street address.
    pub company_info: bool,
    /// Any posted transaction exists.
    pub transaction: bool,
    /// A bank file was imported (any bank-feed item).
    pub bank_file: bool,
    /// Email for invoices is set up (a mail server is saved).
    pub email: bool,
}

/// A profile counts as filled in with a business name and a first address
/// line.
#[must_use]
pub(crate) fn profile_filled(p: &CompanyProfileUpdate) -> bool {
    let set = |s: Option<&str>| s.is_some_and(|s| !s.trim().is_empty());
    !p.business_name.trim().is_empty() && set(p.address_line1.as_deref())
}

/// Reads the facts. A read that fails leaves its step open (logged as a
/// count-free warning); the others still tick.
///
/// # Errors
/// Never: each failure is folded into its step.
pub(crate) async fn read(s: &cl_api::Session) -> Result<Facts, ApiError> {
    let ok = |what: &str, r: Result<bool, ApiError>| {
        r.unwrap_or_else(|_| {
            tracing::warn!(read = what, "home checklist read failed");
            false
        })
    };
    let home = s.home();
    Ok(Facts {
        company_info: ok(
            "company",
            s.company_profile()
                .await
                .map(|p| profile_filled(&p.record.profile)),
        ),
        transaction: ok("transactions", home.has_any_transaction().await),
        bank_file: ok("bank_feed", home.bank_feed_any().await),
        email: ok("smtp", s.smtp_settings().await.map(|m| m.configured)),
    })
}

/// The checklist as it stands, or `None` until the reads are in.
#[must_use]
pub(crate) fn checklist(home: &HomeScreen) -> Option<Checklist> {
    let facts = home.data.checklist.value?;
    let accounts = home.data.accounts.value.as_ref()?;
    let bank = accounts
        .iter()
        .any(|a| a.is_active && crate::screens::register::class_of(a) == Some(AccountClass::Bank));
    let mut c = Checklist::new(&ITEMS);
    c.set(ChecklistItem::CompanyInfo, facts.company_info);
    c.set(ChecklistItem::BankAccount, bank);
    c.set(ChecklistItem::FirstTransaction, facts.transaction);
    c.set(ChecklistItem::BankFile, facts.bank_file);
    c.set(ChecklistItem::Email, facts.email);
    c.set(ChecklistItem::FirstBackup, home.local.last_backup.is_some());
    Some(c)
}

/// The step's wording ("Set up email for invoices").
#[must_use]
pub(crate) fn label(item: ChecklistItem) -> String {
    t(&format!("home.checklist.{}", item.key()))
}

/// Whether the full list starts open: fewer than half the steps done.
#[must_use]
pub(crate) fn opens_by_itself(c: &Checklist) -> bool {
    c.done_count() * 2 < c.total()
}

/// What the checklist asks Home to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Open a step's window.
    Open(&'static str),
    /// Show or hide the full list.
    Toggle,
    /// Hide the checklist for this company.
    Dismiss,
}

/// "Setup checklist 5 of 6 done".
fn summary(c: &Checklist) -> String {
    t_with(
        "home.checklist.summary",
        &[
            ("done", &c.done_count().to_string()),
            ("total", &c.total().to_string()),
        ],
    )
}

/// The whole line as it reads (for measuring).
#[must_use]
pub(crate) fn line_text(c: &Checklist) -> String {
    match c.first_open() {
        Some(item) => format!("{} \u{b7} {}", summary(c), label(item)),
        None => summary(c),
    }
}

/// The line at the right end of the title row: "Setup checklist 5 of 6
/// done · Set up email for invoices". Drawn right to left.
pub(super) fn top_line(ui: &mut Ui, tokens: &Tokens, c: &Checklist) -> Option<Action> {
    let mut action = None;
    ui.spacing_mut().item_spacing.x = 4.0;
    if let Some(item) = c.first_open() {
        let r = ui.link(RichText::new(label(item)).size(12.0));
        if activated(&r) {
            action = Some(Action::Open(item.opens()));
        }
        ui.add(
            egui::Label::new(
                RichText::new("\u{b7}")
                    .size(12.0)
                    .color(tokens.text_secondary),
            )
            .selectable(false),
        );
    }
    let summary = summary(c);
    let r = ui
        .add(
            egui::Label::new(
                RichText::new(summary.clone())
                    .size(12.0)
                    .color(tokens.text_secondary),
            )
            .selectable(false)
            .sense(Sense::click()),
        )
        .on_hover_text(t("home.checklist.toggle.tip"));
    r.widget_info(|| WidgetInfo::labeled(Role::Button, true, &summary));
    if activated(&r) {
        action = Some(Action::Toggle);
    }
    action
}

/// The full list: each step with its tick, a link for the open ones, and
/// Hide checklist.
pub(super) fn card(ui: &mut Ui, tokens: &Tokens, c: &Checklist) -> Option<Action> {
    let mut action = None;
    heading(
        ui,
        tokens,
        &t_with(
            "home.checklist.title",
            &[
                ("done", &c.done_count().to_string()),
                ("total", &c.total().to_string()),
            ],
        ),
    );
    for (i, (item, done)) in c.items().enumerate() {
        if i > 0 {
            rule(ui, tokens.border_row);
        }
        egui::Frame::new()
            .inner_margin(Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    tick(ui, tokens, done);
                    let text = label(item);
                    let (rich, name) = if done {
                        (
                            RichText::new(&text).color(tokens.text_secondary),
                            t_with("home.checklist.done_item", &[("item", &text)]),
                        )
                    } else {
                        (
                            RichText::new(&text).color(tokens.text_primary),
                            text.clone(),
                        )
                    };
                    let r = ui.add(egui::Label::new(rich).selectable(false));
                    r.widget_info(|| WidgetInfo::labeled(Role::Label, true, &name));
                    if !done {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let r = ui.link(t("home.checklist.do_it"));
                            r.widget_info(|| WidgetInfo::labeled(Role::Link, true, label(item)));
                            if activated(&r) {
                                action = Some(Action::Open(item.opens()));
                            }
                        });
                    }
                });
            });
    }
    rule(ui, tokens.border_row);
    egui::Frame::new()
        .inner_margin(Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(t("home.checklist.hide"))
                    .on_hover_text(t("home.checklist.hide.tip"))
                    .clicked()
                {
                    action = Some(Action::Dismiss);
                }
                ui.add(
                    egui::Label::new(
                        RichText::new(t("home.checklist.note"))
                            .size(12.0)
                            .color(tokens.text_tertiary),
                    )
                    .wrap(),
                );
            });
        });
    action
}

/// A painted tick (done) or an empty ring (to do): the fonts carry no
/// symbol glyphs.
fn tick(ui: &mut Ui, tokens: &Tokens, done: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    if done {
        p.circle_filled(c, 8.0, tokens.accent);
        p.line(
            vec![
                Pos2::new(c.x - 4.0, c.y),
                Pos2::new(c.x - 1.0, c.y + 3.0),
                Pos2::new(c.x + 4.0, c.y - 3.0),
            ],
            Stroke::new(1.8, tokens.on_accent),
        );
    } else {
        p.circle_stroke(c, 7.0, Stroke::new(1.5, tokens.border_strong));
    }
}

/// The control channel's view: counts and keys only.
#[must_use]
pub(crate) fn state(home: &HomeScreen) -> serde_json::Value {
    let c = checklist(home);
    serde_json::json!({
        "done": c.as_ref().map(Checklist::done_count),
        "total": c.as_ref().map(Checklist::total),
        "first_open": c.as_ref().and_then(Checklist::first_open).map(ChecklistItem::key),
        "dismissed": home.local.checklist_dismissed,
        "visible": c.as_ref().is_some_and(|c| c.visible(home.local.checklist_dismissed)),
        "expanded": home.checklist_expanded(),
    })
}
