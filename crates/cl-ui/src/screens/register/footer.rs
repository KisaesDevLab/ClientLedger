//! The register footer (P12 WP5, S6): "Last reconciled <date>" and
//! "N uncleared" on the left; "Cleared balance" and the bold "Ending
//! balance" on the right (mockup `Register.html`).
//!
//! Every figure is one the engine reported: the cleared and ending balances
//! from `GET /accounts/{id}/register/summary` (R4), else the unfiltered
//! `endingBalance` of a register page, else (an engine without either) the
//! account's stored balance when the range is "All dates" (the stored
//! balance counts every posted line, future-dated ones too). When a filter is
//! on before R1 a companion one-line page without filters supplies the
//! ending balance. Nothing here is ever added up (non-negotiable 7); a
//! figure that is missing shows "—". The register figures are shown as
//! sent (they are in the account's natural sign); only the stored balance
//! of a credit-normal account is flipped for display (Q-P12h, Q-241).

use cl_api::{RegisterAmount, RegisterPage, RegisterQuery, RegisterSummary};
use cl_core::CalendarDate;
use cl_core::ids::AccountId;
use cl_model::register::{BalanceSource, DisplaySign};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::RegisterScreen;
use crate::company::Task;
use crate::i18n::{t, t_with};
use crate::screens::lists::center::model::layout_plain;
use crate::theme::Tokens;

/// The dash shown for a figure that is missing.
const DASH: &str = "\u{2014}";

/// Height kept below the grid for the footer.
pub const HEIGHT: f32 = 52.0;

/// What the footer was loaded for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Key {
    pub(super) account: AccountId,
    pub(super) end: CalendarDate,
    /// A companion page without filters supplies the ending balance.
    pub(super) companion: bool,
}

/// The footer's figures and the requests behind them.
#[derive(Debug, Default)]
pub struct Footer {
    pub(super) key: Option<Key>,
    pub(super) summary_task: Option<Task<RegisterSummary>>,
    pub(super) companion_task: Option<Task<RegisterPage>>,
    pub(super) summary: Option<RegisterSummary>,
    pub(super) summary_failed: bool,
    /// The unfiltered ending balance a register page reported.
    pub(super) page_ending: Option<RegisterAmount>,
}

impl Footer {
    /// The ledger moved: load again (the old figures stay until then).
    pub fn invalidate(&mut self) {
        self.key = None;
    }

    /// A register page arrived for the shown query. Its `endingBalance` is
    /// unfiltered whenever the engine sends it (R1), so it is kept unless a
    /// companion page supplies the figure.
    pub fn page_arrived(&mut self, page: &RegisterPage) {
        if !self.key.is_some_and(|k| k.companion) && page.ending_balance.is_some() {
            self.page_ending.clone_from(&page.ending_balance);
        }
    }

    /// True while a request is in flight.
    #[must_use]
    pub fn loading(&self) -> bool {
        self.summary_task.is_some() || self.companion_task.is_some()
    }

    /// True when a figure the engine sent was display-only.
    #[must_use]
    pub fn display_only(&self) -> bool {
        self.summary.as_ref().is_some_and(|s| !s.exact)
            || self.page_ending.as_ref().is_some_and(|a| !a.is_exact())
    }

    fn poll(&mut self) {
        if let Some(task) = self.summary_task.as_mut()
            && let Some(result) = task.try_take()
        {
            self.summary_task = None;
            self.summary = result.ok();
            self.summary_failed = self.summary.is_none();
        }
        if let Some(task) = self.companion_task.as_mut()
            && let Some(result) = task.try_take()
        {
            self.companion_task = None;
            if let Ok(page) = result {
                self.page_ending = page.ending_balance;
            }
        }
    }
}

/// The footer's figures as shown text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// "Last reconciled 09/30/2026", when the account was ever reconciled.
    pub last_reconciled: Option<String>,
    /// "3 uncleared".
    pub uncleared: Option<String>,
    /// Cleared balance text (R4), else none.
    pub cleared: Option<String>,
    /// Ending balance text, or a dash.
    pub ending: String,
    /// The summary could not be loaded.
    pub failed: bool,
}

impl RegisterScreen {
    fn amount_text(&self, a: &RegisterAmount, source: BalanceSource) -> Option<String> {
        a.display(DisplaySign::for_source(self.class, source).negates())
            .map(|p| layout_plain(&p, self.model.style))
    }

    /// Starts the footer requests the shown register needs.
    pub(super) fn footer_load(&mut self) {
        self.footer.poll();
        let (Some(account), Some(q)) = (self.account, self.query.as_ref()) else {
            return;
        };
        let want = Key {
            account,
            end: q.end,
            companion: self.filters.is_active() && !self.register_exact(),
        };
        if self.footer.key == Some(want) {
            return;
        }
        if self
            .footer
            .key
            .is_some_and(|k| k.account != account || k.end != want.end)
        {
            self.footer.summary = None;
            self.footer.page_ending = None;
        }
        self.footer.key = Some(want);
        self.footer.summary_failed = false;
        match self.fetch_summary(account, want.end) {
            Some(task) => self.footer.summary_task = Some(task),
            None => self.footer.summary_failed = true,
        }
        if want.companion {
            let mut unfiltered = RegisterQuery::new(q.start, q.end);
            (unfiltered.sort, unfiltered.dir) = (q.sort, q.dir);
            self.footer.companion_task = self.fetch_one_line(account, &unfiltered);
        }
        self.footer.poll();
    }

    fn fetch_summary(
        &self,
        account: AccountId,
        end: CalendarDate,
    ) -> Option<Task<RegisterSummary>> {
        if let Some(mut demo) = self.link.demo_state() {
            let chart = demo
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            return crate::company::demo::register::summary(
                &demo.ledger,
                &chart,
                account,
                Some(end),
            )
            .map(Task::ready);
        }
        let session = self.link.session()?;
        let group = self.group.as_ref()?;
        group
            .read(&session, move |s| async move {
                s.register().summary(account, Some(end)).await
            })
            .ok()
            .map(Task::Pending)
    }

    /// The first page, one line long, of the register without filters.
    fn fetch_one_line(
        &self,
        account: AccountId,
        query: &RegisterQuery,
    ) -> Option<Task<RegisterPage>> {
        if let Some(mut demo) = self.link.demo_state() {
            let chart = demo
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            return crate::company::demo::register::page(
                &demo.ledger,
                &chart,
                account,
                query,
                1,
                1,
            )
            .map(Task::ready);
        }
        let session = self.link.session()?;
        let group = self.group.as_ref()?;
        let q = query.clone();
        group
            .read(&session, move |s| async move {
                s.register().page(account, &q, 1, 1).await
            })
            .ok()
            .map(Task::Pending)
    }

    /// The footer's text, from the engine's figures only.
    #[must_use]
    pub(super) fn footer_shown(&self) -> Shown {
        let f = &self.footer;
        let s = f.summary.as_ref();
        // The stored balance counts every posted line, future-dated ones
        // too: it is the ending balance only of an "All dates" range.
        let all_dates = self
            .query
            .as_ref()
            .is_some_and(|q| q.end >= CalendarDate::max());
        let ending = s
            .and_then(|s| s.ending_balance.as_ref())
            .or(f.page_ending.as_ref())
            .and_then(|a| self.amount_text(a, BalanceSource::Register))
            // An engine without R1/R4: the account's stored balance stands
            // in only for "All dates".
            .or_else(|| {
                s.filter(|_| all_dates).and_then(|s| {
                    self.amount_text(&s.current_balance, BalanceSource::AccountBalance)
                })
            })
            .unwrap_or_else(|| DASH.to_string());
        Shown {
            last_reconciled: s.and_then(|s| s.last_reconciled).map(|d| {
                t_with(
                    "register.footer.last_reconciled",
                    &[("date", &crate::shell::us_date(d))],
                )
            }),
            uncleared: s.map(|s| {
                t_with(
                    "register.footer.uncleared",
                    &[("count", &s.uncleared_count.to_string())],
                )
            }),
            cleared: s
                .and_then(|s| s.cleared_balance.as_ref())
                .and_then(|a| self.amount_text(a, BalanceSource::Register)),
            ending,
            failed: f.summary_failed,
        }
    }

    /// The footer strip below the grid.
    pub(super) fn footer_ui(&self, ui: &mut Ui, tokens: &Tokens) {
        let shown = self.footer_shown();
        let small = |text: String| {
            egui::Label::new(RichText::new(text).color(tokens.text_secondary)).selectable(false)
        };
        ui.separator();
        ui.horizontal(|ui| {
            if shown.failed {
                ui.add(small(t("register.footer.unavailable")));
            }
            if let Some(text) = shown.last_reconciled.clone() {
                ui.add(small(text));
            }
            if let Some(text) = shown.uncleared.clone() {
                ui.add(small(text));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(&shown.ending)
                            .monospace()
                            .strong()
                            .size(15.0)
                            .color(tokens.text_primary),
                    )
                    .selectable(false),
                )
                .on_hover_text(t("register.footer.ending_help"));
                ui.add(small(t("register.footer.ending")));
                if let Some(cleared) = &shown.cleared {
                    ui.add_space(16.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(cleared)
                                .monospace()
                                .color(tokens.text_primary),
                        )
                        .selectable(false),
                    )
                    .on_hover_text(t("register.footer.cleared_help"));
                    ui.add(small(t("register.footer.cleared")));
                }
            });
        });
    }

    /// The footer as the control channel reports it.
    pub(super) fn footer_state(&self) -> Value {
        let s = self.footer_shown();
        json!({
            "ending": s.ending,
            "cleared": s.cleared,
            "last_reconciled": s.last_reconciled,
            "uncleared": self.footer.summary.as_ref().map(|s| s.uncleared_count),
            "failed": s.failed,
            "loading": self.footer.loading(),
        })
    }
}
