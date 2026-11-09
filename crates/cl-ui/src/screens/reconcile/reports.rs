//! Reports (Q-P27b, Q-P27i): Summary and Detail PDFs drawn by `cl-print`
//! from the engine's report JSON and opened full size (print and save
//! from there, as P22 statements do), Previous reports ("figures as of
//! today") and the Discrepancy report (U-191).

use std::sync::Arc;

use cl_api::{ReconReport, ReconciliationId, RequestGroup};
use cl_model::CommandId;
use cl_print::reports::{Column, Report, Section};
use eframe::egui::{self, Response, Ui};
use serde_json::{Value, json};

use super::{Dialog, ReconcileScreen};
use crate::company::{CompanyEvent, CompanyLink};
use crate::i18n::{t, t_with};
use crate::screens::invoice::engine::{self, Job};

/// Which report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Totals only.
    Summary,
    /// Totals and every line.
    Detail,
}

/// The report reads in flight and what they made.
pub(crate) struct Reports {
    link: Arc<CompanyLink>,
    job: Option<(Kind, Job<ReconReport>)>,
    /// The last report opened (control channel).
    pub(crate) opened: Option<Kind>,
    pub(crate) failed: bool,
}

impl Reports {
    pub(crate) fn new(link: &Arc<CompanyLink>) -> Reports {
        Reports {
            link: Arc::clone(link),
            job: None,
            opened: None,
            failed: false,
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Reads the report of `id` and opens its PDF.
    pub(crate) fn open(
        &mut self,
        link: &Arc<CompanyLink>,
        group: Option<&RequestGroup>,
        id: Option<ReconciliationId>,
        kind: Kind,
    ) {
        let Some(id) = id else {
            return;
        };
        self.failed = false;
        self.job = Some((
            kind,
            engine::read(
                link,
                group,
                move |d| crate::company::demo::reconcile::report(d, id),
                move |s| async move { s.reconciliations().report(id).await },
            ),
        ));
    }

    pub(crate) fn poll(&mut self) {
        let Some((kind, job)) = self.job.as_mut() else {
            return;
        };
        let kind = *kind;
        let Some(r) = job.take() else {
            return;
        };
        self.job = None;
        match r.ok().and_then(|rep| {
            let doc = report(&rep, kind);
            cl_print::reports::render_report(&doc)
                .ok()
                .map(|b| (doc.title, b))
        }) {
            Some((title, bytes)) => {
                self.opened = Some(kind);
                self.link.push(CompanyEvent::OpenWith(
                    CommandId::new(crate::screens::document_view::KIND),
                    Box::new(crate::windows::OpenPayload::Document(
                        crate::widgets::pdf_viewer::DocSource {
                            name: format!("{title}.pdf"),
                            bytes: Arc::from(bytes),
                        },
                    )),
                ));
            }
            None => self.failed = true,
        }
    }

    pub(crate) fn state(&self) -> Value {
        json!({
            "opened": self.opened.map(|k| format!("{k:?}").to_lowercase()),
            "failed": self.failed,
            "busy": self.busy(),
        })
    }
}

fn amount(a: Option<&cl_api::wire::ReportAmount>) -> String {
    a.and_then(|a| a.display(2))
        .map(|s| grouped(&s))
        .unwrap_or_default()
}

/// `1234567.89` as `1,234,567.89` (text only).
pub(crate) fn grouped(s: &str) -> String {
    let (sign, rest) = s.strip_prefix('-').map_or(("", s), |r| ("-", r));
    let (int, frac) = rest.split_once('.').unwrap_or((rest, ""));
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if frac.is_empty() {
        format!("{sign}{out}")
    } else {
        format!("{sign}{out}.{frac}")
    }
}

fn date_text(iso: &str) -> String {
    cl_core::CalendarDate::parse_wire(iso).map_or_else(|_| iso.to_string(), crate::shell::us_date)
}

/// The printable report.
pub(crate) fn report(r: &ReconReport, kind: Kind) -> Report {
    let acct = match &r.account_number {
        Some(n) => format!("{n} {}", r.account_name),
        None => r.account_name.clone(),
    };
    let mut sections = vec![Section::Summary {
        heading: t("reconcile.report.summary_heading"),
        rows: vec![
            (
                t("reconcile.sum.beginning"),
                amount(r.beginning_balance.as_ref()),
                false,
            ),
            (
                t("reconcile.report.cleared_deposits"),
                amount(r.cleared_deposits.as_ref()),
                false,
            ),
            (
                t("reconcile.report.cleared_payments"),
                amount(r.cleared_payments.as_ref()),
                false,
            ),
            (
                t("reconcile.sum.cleared"),
                amount(r.cleared_balance.as_ref()),
                true,
            ),
            (
                t("reconcile.sum.statement_ending"),
                amount(r.statement_ending_balance.as_ref()),
                false,
            ),
            (
                t("reconcile.sum.difference"),
                amount(r.difference.as_ref()),
                true,
            ),
        ],
    }];
    let columns = || {
        vec![
            Column {
                heading: t("reconcile.col.date"),
                share: 1.0,
                right: false,
            },
            Column {
                heading: t("reconcile.col.ref"),
                share: 0.8,
                right: false,
            },
            Column {
                heading: t("reconcile.col.payee"),
                share: 3.0,
                right: false,
            },
            Column {
                heading: t("reconcile.report.payment"),
                share: 1.2,
                right: true,
            },
            Column {
                heading: t("reconcile.report.deposit"),
                share: 1.2,
                right: true,
            },
        ]
    };
    let rows = |lines: &[cl_api::ReportLine]| {
        lines
            .iter()
            .map(|l| {
                vec![
                    date_text(&l.date),
                    l.number.clone().unwrap_or_default(),
                    l.description.clone().unwrap_or_default(),
                    amount(l.payment.as_ref()),
                    amount(l.deposit.as_ref()),
                ]
            })
            .collect()
    };
    if kind == Kind::Detail {
        sections.push(Section::Table {
            heading: t("reconcile.report.cleared"),
            columns: columns(),
            rows: rows(&r.cleared),
            empty: t("reconcile.report.none"),
        });
    }
    sections.push(Section::Table {
        heading: t("reconcile.report.uncleared"),
        columns: columns(),
        rows: rows(&r.uncleared),
        empty: t("reconcile.report.none"),
    });
    let title = match kind {
        Kind::Summary => t("reconcile.report.summary_title"),
        Kind::Detail => t("reconcile.report.detail_title"),
    };
    let mut subtitle = vec![
        acct,
        t_with(
            "reconcile.report.statement",
            &[("date", &date_text(&r.statement_date))],
        ),
    ];
    if let Some(by) = &r.completed_by {
        subtitle.push(t_with("reconcile.report.by", &[("name", by)]));
    }
    Report {
        title,
        subtitle,
        sections,
        footer: t("reconcile.report.as_of_today"),
        page_label: t("reconcile.report.page"),
    }
}

/// The Reports button's menu.
pub(crate) fn menu(s: &mut ReconcileScreen, _ui: &mut Ui, button: &Response) {
    egui::Popup::menu(button).show(|ui| {
        let has = s.report_target().is_some();
        if ui
            .add_enabled(has, egui::Button::new(t("reconcile.reports.summary")))
            .clicked()
        {
            s.reports
                .open(&s.link, s.group.as_ref(), s.report_target(), Kind::Summary);
        }
        if ui
            .add_enabled(has, egui::Button::new(t("reconcile.reports.detail")))
            .clicked()
        {
            s.reports
                .open(&s.link, s.group.as_ref(), s.report_target(), Kind::Detail);
        }
        if ui
            .add_enabled(has, egui::Button::new(t("reconcile.reports.previous")))
            .clicked()
        {
            s.dialog = Some(Dialog::Previous);
        }
        let disc = ui
            .add_enabled(
                s.discrepancy_supported(),
                egui::Button::new(t("reconcile.reports.discrepancy")),
            )
            .on_disabled_hover_text(t("reconcile.reports.needs_update"));
        if disc.clicked() {
            s.open_discrepancy();
        }
        let undo = ui
            .add_enabled(
                has && !s.not_admin,
                egui::Button::new(t("reconcile.undo.button")),
            )
            .on_disabled_hover_text(t("reconcile.undo.admin_only"));
        if undo.clicked() {
            s.undo_mode = cl_api::UndoMode::KeepChecks;
            s.dialog = Some(Dialog::Undo);
        }
    });
}

/// Previous reports: the account's finished reconciliations.
pub(crate) fn previous(s: &mut ReconcileScreen, ui: &Ui) {
    let rows: Vec<_> = s
        .history
        .iter()
        .filter(|r| r.status == cl_api::ReconStatus::Complete)
        .copied()
        .collect();
    egui::Window::new(t("reconcile.previous.title"))
        .id(egui::Id::unique("reconcile_previous"))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            ui.label(t("reconcile.previous.note"));
            ui.add_space(6.0);
            for r in rows {
                ui.horizontal(|ui| {
                    ui.label(crate::shell::us_date(r.statement_date));
                    if ui.button(t("reconcile.reports.summary")).clicked() {
                        s.reports
                            .open(&s.link, s.group.as_ref(), Some(r.id), Kind::Summary);
                    }
                    if ui.button(t("reconcile.reports.detail")).clicked() {
                        s.reports
                            .open(&s.link, s.group.as_ref(), Some(r.id), Kind::Detail);
                    }
                });
            }
            ui.add_space(6.0);
            if ui.button(t("reconcile.close")).clicked() {
                s.dialog = None;
            }
        });
}

/// The discrepancy report's rows (field names only, never memo text).
pub(crate) fn discrepancy(s: &mut ReconcileScreen, ui: &Ui) {
    let tokens = crate::theme::Tokens::of(ui.ctx());
    egui::Window::new(t("reconcile.discrepancy.title"))
        .id(egui::Id::unique("reconcile_discrepancy"))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(true)
        .default_width(720.0)
        .show(ui.ctx(), |ui| {
            match &s.discrepancy {
                None => {
                    ui.spinner();
                }
                Some(d) if d.rows.is_empty() => {
                    ui.label(t("reconcile.discrepancy.none"));
                }
                Some(d) => {
                    egui::Grid::new("reconcile_disc_grid")
                        .striped(true)
                        .show(ui, |ui| {
                            for h in [
                                "what",
                                "statement",
                                "date",
                                "payee",
                                "fields",
                                "effect",
                                "when",
                            ] {
                                ui.label(
                                    egui::RichText::new(t(&format!(
                                        "reconcile.discrepancy.col.{h}"
                                    )))
                                    .color(tokens.text_secondary),
                                );
                            }
                            ui.end_row();
                            for r in &d.rows {
                                let kind_key = format!("reconcile.discrepancy.kind.{}", r.kind);
                                let what = if crate::i18n::has(&kind_key) {
                                    t(&kind_key)
                                } else {
                                    r.kind.clone()
                                };
                                ui.label(what);
                                ui.label(
                                    r.statement_date
                                        .map(crate::shell::us_date)
                                        .unwrap_or_default(),
                                );
                                ui.label(r.txn_date.map(crate::shell::us_date).unwrap_or_default());
                                ui.label(r.payee.clone().unwrap_or_default());
                                ui.label(r.fields.join(", "));
                                ui.label(
                                    egui::RichText::new(
                                        r.effect
                                            .map(cl_model::reconcile::display)
                                            .unwrap_or_default(),
                                    )
                                    .monospace(),
                                );
                                ui.label(r.changed_at.map(super::day_text).unwrap_or_default());
                                ui.end_row();
                            }
                        });
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(t("reconcile.discrepancy.print")).clicked()
                    && let (Some(d), Some(a)) = (&s.discrepancy, s.form.account)
                {
                    print_discrepancy(&s.link, d, &s.account_text(a));
                }
                if ui.button(t("reconcile.close")).clicked() {
                    s.dialog = None;
                }
            });
        });
}

fn print_discrepancy(link: &Arc<CompanyLink>, d: &cl_api::DiscrepancyReport, account: &str) {
    let rows = d
        .rows
        .iter()
        .map(|r| {
            vec![
                r.kind.replace('_', " "),
                r.txn_date.map(crate::shell::us_date).unwrap_or_default(),
                r.payee.clone().unwrap_or_default(),
                r.fields.join(", "),
                r.effect
                    .map(cl_model::reconcile::display)
                    .unwrap_or_default(),
            ]
        })
        .collect();
    let doc = Report {
        title: t("reconcile.discrepancy.title"),
        subtitle: vec![account.to_string()],
        sections: vec![Section::Table {
            heading: String::new(),
            columns: vec![
                Column {
                    heading: t("reconcile.discrepancy.col.what"),
                    share: 1.4,
                    right: false,
                },
                Column {
                    heading: t("reconcile.discrepancy.col.date"),
                    share: 1.0,
                    right: false,
                },
                Column {
                    heading: t("reconcile.discrepancy.col.payee"),
                    share: 2.4,
                    right: false,
                },
                Column {
                    heading: t("reconcile.discrepancy.col.fields"),
                    share: 1.6,
                    right: false,
                },
                Column {
                    heading: t("reconcile.discrepancy.col.effect"),
                    share: 1.0,
                    right: true,
                },
            ],
            rows,
            empty: t("reconcile.discrepancy.none"),
        }],
        footer: t("reconcile.report.as_of_today"),
        page_label: t("reconcile.report.page"),
    };
    if let Ok(bytes) = cl_print::reports::render_report(&doc) {
        link.push(CompanyEvent::OpenWith(
            CommandId::new(crate::screens::document_view::KIND),
            Box::new(crate::windows::OpenPayload::Document(
                crate::widgets::pdf_viewer::DocSource {
                    name: format!("{}.pdf", doc.title),
                    bytes: Arc::from(bytes),
                },
            )),
        ));
    }
}
