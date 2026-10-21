//! QuickReport (Ctrl+Shift+Q, P12 WP7): the P11 account report (Q-P11i)
//! for the register's account and shown dates, in a panel above the
//! register. Its lines and the account balance are the engine's; nothing is
//! added up here.

use cl_api::{AccountReport, ApiError};
use cl_core::CalendarDate;
use cl_core::ids::AccountId;
use cl_grid::{Grid, GridHeight, ModelRef, NoLookups};
use cl_model::register::{BalanceSource, DisplaySign};
use eframe::egui::{RichText, Ui};
use serde_json::{Value, json};

use super::RegisterScreen;
use crate::company::Task;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::lists::accounts::model::{ReportModel, report_day, report_spec};
use crate::theme::Tokens;

/// The QuickReport panel.
pub struct QuickReport {
    account: AccountId,
    start: CalendarDate,
    end: CalendarDate,
    task: Option<Task<AccountReport>>,
    grid: Grid,
    model: ReportModel,
    loaded: bool,
    error: Option<ApiError>,
}

impl std::fmt::Debug for QuickReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuickReport")
            .field("account", &self.account)
            .field("start", &self.start)
            .field("end", &self.end)
            .field("loaded", &self.loaded)
            .finish_non_exhaustive()
    }
}

impl QuickReport {
    /// What the control channel reports.
    #[must_use]
    pub fn state(&self) -> Value {
        json!({
            "loaded": self.loaded,
            "lines": self.model.len(),
            "start": self.start.to_wire(),
            "end": self.end.to_wire(),
            "error": self.error.is_some(),
        })
    }
}

impl RegisterScreen {
    /// Ctrl+Shift+Q: the report for this account and the shown dates (or
    /// closes it when it is already shown).
    pub(crate) fn toggle_quick_report(&mut self) {
        if self.report.take().is_some() {
            return;
        }
        let (Some(account), Some(q)) = (self.account, self.query.as_ref()) else {
            return;
        };
        let (start, end) = (q.start, q.end);
        let task = if self.link.is_demo() {
            self.account_record().map(|a| {
                let mut report = crate::company::demo::account_report(a);
                report
                    .data
                    .retain(|l| report_day(l.txn_date).is_some_and(|d| d >= start && d <= end));
                Task::ready(report)
            })
        } else {
            self.link.session().and_then(|session| {
                self.group.as_ref().and_then(|g| {
                    g.read(&session, move |s| async move {
                        s.reports()
                            .account_report(account, Some(start), Some(end))
                            .await
                    })
                    .ok()
                    .map(Task::Pending)
                })
            })
        };
        // The report spec is fixed and covered by the chart's tests.
        let grid = Grid::new("register/quick-report", report_spec())
            .expect("the quick report grid spec is valid");
        self.report = Some(QuickReport {
            account,
            start,
            end,
            task,
            grid,
            model: ReportModel::default(),
            loaded: false,
            error: None,
        });
        self.poll_report();
    }

    /// Takes the report's answer when it arrives.
    pub(super) fn poll_report(&mut self) {
        let Some(r) = self.report.as_mut() else {
            return;
        };
        let Some(result) = r.task.as_mut().and_then(Task::try_take) else {
            return;
        };
        r.task = None;
        r.loaded = true;
        match result {
            Ok(report) => r.model.set(report),
            Err(e) => r.error = Some(e),
        }
    }

    /// The panel, above the register.
    pub(super) fn quick_report_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        // The stored balance is debit-positive for every type: a
        // credit-normal account shows it flipped (Q-P12h, Q-241).
        let sign = DisplaySign::for_source(self.class, BalanceSource::AccountBalance);
        let balance = self
            .account_record()
            .and_then(|a| a.balance)
            .and_then(|b| sign.apply(b).ok());
        let title = self
            .account_record()
            .map(crate::account_names::shown_name)
            .unwrap_or_default();
        let style = self.parts.frame.money_style;
        let Some(r) = self.report.as_mut() else {
            return;
        };
        let mut close = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(t_with("lists.qr.title", &[("account", &title)])).strong());
            ui.label(
                RichText::new(format!(
                    "{} \u{2013} {}",
                    crate::shell::us_date(r.start),
                    crate::shell::us_date(r.end)
                ))
                .color(tokens.text_secondary),
            );
            if ui.button(t("lists.qr.close")).clicked() {
                close = true;
            }
        });
        if let Some(e) = &r.error {
            form::inline_error(ui, tokens, e);
        } else if !r.loaded {
            form::progress_line(ui, tokens, &t("lists.qr.loading"));
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        } else if r.model.is_empty() {
            ui.label(RichText::new(t("lists.qr.empty")).color(tokens.text_secondary));
        } else {
            let label = t("lists.qr.grid_label");
            let env = self
                .parts
                .env(&NoLookups, &label, GridHeight::Rows { min: 3, max: 6 });
            r.grid.show(ui, ModelRef::Browse(&mut r.model), &env);
        }
        // The engine's balance of this account; the lines are never summed.
        let text = balance.map_or_else(
            || t("lists.qr.no_balance"),
            |b| t_with("lists.qr.balance", &[("amount", &b.format(style))]),
        );
        ui.label(RichText::new(text).size(12.0).color(tokens.text_secondary));
        ui.separator();
        if close {
            self.report = None;
        }
    }
}
