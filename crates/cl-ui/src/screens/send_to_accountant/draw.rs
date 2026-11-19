//! Drawing Send to My Accountant (layout of `SendToAccountant.html`).

use cl_api::VerificationTotals;
use cl_company::transfer::TransferError;
use cl_core::report::display_amount;
use cl_core::{Money, MoneyStyle};
use eframe::egui::{self, RichText, Ui};

use super::{Ask, SendScreen};
use crate::i18n::{count_text, t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;

fn amount(m: Money) -> String {
    display_amount(m.as_decimal(), MoneyStyle::default())
}

/// What a draw asked for, run after the frame's borrows end.
enum Act {
    Save,
    MakeOne,
    Copy,
    Payroll(bool),
    OpenFolder,
    OpenRecord(Option<String>, Option<String>),
    Iif,
}

pub(super) fn screen(s: &mut SendScreen, ui: &mut Ui) {
    let tokens = Tokens::of(ui.ctx());
    let mut act: Option<Act> = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            form::heading(ui, &tokens, &t("cmd.company.send_to_accountant"));
            form::hint(ui, &tokens, &t("send.subtitle"));
            ui.add_space(8.0);
            if s.tenant.is_none() {
                form::inline_warning(ui, &tokens, &t("send.no_company"));
                return;
            }
            if !s.supported() {
                form::inline_warning(ui, &tokens, &t("send.unsupported"));
            }
            steps(ui, &tokens);
            ui.add_space(10.0);
            ui.columns(2, |cols| {
                included(s, &mut cols[0], &tokens);
                cols[0].add_space(10.0);
                history(s, &mut cols[0], &tokens);
                totals(s, &mut cols[1], &tokens);
                cols[1].add_space(10.0);
                file_card(s, &mut cols[1], &tokens, &mut act);
            });
            result(s, ui, &tokens, &mut act);
        });
    let ctx = ui.ctx().clone();
    match act {
        Some(Act::Save) => s.save(Some(&ctx)),
        Some(Act::MakeOne) => s.make_one(),
        Some(Act::Copy) => {
            crate::secret_clipboard::copy(&ctx, s.passphrase.text().to_string());
            s.message = Some("send.copied");
        }
        Some(Act::Payroll(discard)) => s.answer_payroll(discard, Some(&ctx)),
        Some(Act::OpenFolder) => {
            if let Some(dir) = s
                .outcome
                .as_ref()
                .and_then(|o| o.record.path.parent().map(std::path::Path::to_path_buf))
            {
                ctx.open_url(egui::OpenUrl::new_tab(format!(
                    "file:///{}",
                    dir.display().to_string().replace('\\', "/")
                )));
            }
        }
        Some(Act::OpenRecord(kind, id)) => s.open_record(kind.as_deref(), id.as_deref()),
        Some(Act::Iif) => s.open_iif(),
        None => {}
    }
}

fn steps(ui: &mut Ui, tokens: &Tokens) {
    ui.columns(3, |cols| {
        for (i, (title, sub)) in [
            ("send.step1", "send.step1.sub"),
            ("send.step2", "send.step2.sub"),
            ("send.step3", "send.step3.sub"),
        ]
        .into_iter()
        .enumerate()
        {
            form::card_tight(&mut cols[i], tokens, |ui| {
                ui.label(
                    RichText::new(format!("{}  {}", i + 1, t(title)))
                        .strong()
                        .color(tokens.text_primary),
                );
                ui.label(RichText::new(t(sub)).color(tokens.text_secondary));
            });
        }
    });
}

fn included(s: &SendScreen, ui: &mut Ui, tokens: &Tokens) {
    form::card(ui, tokens, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.spacing_mut().interact_size.y = 16.0;
        ui.label(RichText::new(t("send.included")).strong());
        let counts = s.preview.as_ref().map(|p| p.counts);
        let n = |v: Option<u64>| v.map_or_else(|| "\u{2026}".to_string(), count_text);
        let yes = |ui: &mut Ui, text: String, tip: Option<String>| {
            ui.horizontal(|ui| {
                crate::widgets::check_mark(ui, 14.0, tokens.accent, &t("send.included.yes"));
                let r = ui.label(RichText::new(text).color(tokens.text_primary));
                if let Some(tip) = tip {
                    r.on_hover_text(tip);
                }
            });
        };
        yes(
            ui,
            t_with(
                "send.inc.lists",
                &[("n", &n(counts.map(|c| c.transactions)))],
            ),
            None,
        );
        yes(
            ui,
            t_with(
                "send.inc.attachments",
                &[("n", &n(counts.map(|c| c.attachments)))],
            ),
            None,
        );
        yes(ui, t("send.inc.audit"), None);
        yes(ui, t("send.inc.tags"), None);
        yes(ui, t("send.inc.settings"), None);
        yes(ui, t("send.inc.names"), None);
        yes(ui, t("send.inc.hidden"), Some(t("send.inc.hidden.tip")));
        let no = |ui: &mut Ui, text: String| {
            ui.label(RichText::new(format!("\u{d7}  {text}")).color(tokens.text_secondary));
        };
        no(ui, t("send.exc.secrets"));
        if let Some(p) = &s.preflight
            && p.waiting_documents > 0
        {
            no(
                ui,
                t_with(
                    "send.exc.documents",
                    &[("n", &count_text(p.waiting_documents))],
                ),
            );
        }
    });
}

fn totals_grid(ui: &mut Ui, p: &VerificationTotals, id: &str) {
    let rows = [
        (t("send.total.assets"), amount(p.total_assets)),
        (t("send.total.ar"), amount(p.accounts_receivable)),
        (t("send.total.liabilities"), amount(p.total_liabilities)),
        (t("send.total.ap"), amount(p.accounts_payable)),
        (t("send.total.equity"), amount(p.total_equity)),
        (
            t("send.total.transactions"),
            count_text(p.counts.transactions),
        ),
        (
            t_with(
                "send.total.net_income",
                &[
                    ("from", &crate::shell::us_date(p.net_income.from)),
                    ("to", &crate::shell::us_date(p.net_income.to)),
                ],
            ),
            amount(p.net_income.amount),
        ),
        (
            t("send.total.attachments"),
            count_text(p.counts.attachments),
        ),
    ];
    egui::Grid::new(id)
        .num_columns(4)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            for pair in rows.chunks(2) {
                for (label, value) in pair {
                    ui.label(label);
                    ui.label(RichText::new(value).strong().monospace());
                }
                ui.end_row();
            }
        });
}

fn totals(s: &SendScreen, ui: &mut Ui, tokens: &Tokens) {
    form::card(ui, tokens, |ui| {
        let in_file = s.outcome.as_ref().and_then(|o| o.totals.as_ref());
        match (in_file, s.preview.as_ref()) {
            (Some(f), _) => {
                ui.label(RichText::new(t("send.totals.file")).strong());
                if s.totals_differ() {
                    form::inline_warning(ui, tokens, &t("send.totals.changed"));
                }
                totals_grid(ui, f, "send_totals_file");
            }
            (None, Some(p)) => {
                ui.label(RichText::new(t("send.totals")).strong());
                totals_grid(ui, p, "send_totals_preview");
            }
            (None, None) => {
                ui.label(RichText::new(t("send.totals")).strong());
                if s.preview_job.is_some() {
                    form::progress_line(ui, tokens, &t("send.totals.loading"));
                } else if let Some(e) = &s.preview_error {
                    form::inline_error_details(ui, tokens, e, "send");
                } else {
                    form::hint(ui, tokens, &t("send.totals.unavailable"));
                }
            }
        }
    });
}

fn file_card(s: &mut SendScreen, ui: &mut Ui, tokens: &Tokens, act: &mut Option<Act>) {
    let running = s.task.is_some();
    form::card(ui, tokens, |ui| {
        ui.add_enabled_ui(!running, |ui| {
            form::field(ui, &t("send.file_name"), &mut s.file_name);
            ui.add_space(4.0);
            form::password_secret(ui, &t("send.passphrase"), &mut s.passphrase);
            form::password_secret(ui, &t("send.passphrase_again"), &mut s.again);
            form::hint(ui, tokens, &t("send.passphrase.hint"));
            ui.horizontal(|ui| {
                if ui
                    .button(t("send.make_one"))
                    .on_hover_text(t("send.make_one.tip"))
                    .clicked()
                {
                    *act = Some(Act::MakeOne);
                }
                if ui
                    .add_enabled(!s.passphrase.is_empty(), egui::Button::new(t("send.copy")))
                    .on_hover_text(t("send.copy.tip"))
                    .clicked()
                {
                    *act = Some(Act::Copy);
                }
                if s.generated {
                    ui.label(RichText::new(s.passphrase.text()).monospace().strong());
                }
            });
            if let Some(k) = s.message {
                form::inline_note(ui, tokens, &t(k));
            }
            ui.add_space(6.0);
            ui.label(RichText::new(t("send.how")).strong());
            form::hint(ui, tokens, &t("send.how.body"));
            if s.backup_needed {
                ui.add_space(6.0);
                s.backup_ask.ui(ui, tokens, "send.backup_passphrase_why");
            }
        });
        if s.ask == Ask::Payroll {
            payroll_prompt(s, ui, tokens, act);
        }
        if let Some(e) = &s.error
            && !matches!(e, TransferError::MissingFiles(_))
        {
            form::inline_error(ui, tokens, e);
        }
        ui.add_space(8.0);
        if running {
            let text = match s.progress.lock().ok().and_then(|p| p.clone()) {
                Some(cl_company::Progress::Working {
                    percent: Some(p), ..
                }) => t_with("send.working.percent", &[("p", &p.to_string())]),
                Some(cl_company::Progress::Copying) => t("send.working.copy"),
                Some(cl_company::Progress::Verifying) => t("send.working.verify"),
                _ => t("send.working"),
            };
            form::progress_line(ui, tokens, &text);
        } else if s.ask != Ask::Payroll {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        s.supported(),
                        egui::Button::new(RichText::new(t("send.save")).strong()),
                    )
                    .clicked()
                {
                    *act = Some(Act::Save);
                }
            });
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(t("send.backup_first")).color(tokens.text_secondary));
            ui.label(RichText::new(t("send.need_iif")).color(tokens.text_secondary));
            if ui.link(t("send.iif_link")).clicked() {
                *act = Some(Act::Iif);
            }
        });
    });
}

fn payroll_prompt(s: &SendScreen, ui: &mut Ui, tokens: &Tokens, act: &mut Option<Act>) {
    let n = s.preflight.as_ref().map_or(0, |p| p.unposted_payroll);
    ui.add_space(6.0);
    form::card_tight(ui, tokens, |ui| {
        ui.label(RichText::new(t_with("send.payroll.title", &[("n", &count_text(n))])).strong());
        form::hint(ui, tokens, &t("send.payroll.body"));
        // Discard is the default (Q-P30c): first, and Enter takes it.
        match form::actions(ui, &t("send.payroll.discard"), &[&t("send.payroll.cancel")]) {
            Some(0) => *act = Some(Act::Payroll(true)),
            Some(_) => *act = Some(Act::Payroll(false)),
            None => {}
        }
    });
}

fn result(s: &SendScreen, ui: &mut Ui, tokens: &Tokens, act: &mut Option<Act>) {
    if let Some(TransferError::MissingFiles(files)) = &s.error {
        ui.add_space(8.0);
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(t("send.missing.title")).strong());
            form::hint(ui, tokens, &t("send.missing.body"));
            egui::Grid::new("send_missing")
                .num_columns(3)
                .show(ui, |ui| {
                    for f in files {
                        ui.label(
                            f.file_name
                                .clone()
                                .unwrap_or_else(|| t("send.missing.unknown")),
                        );
                        ui.label(t(&format!("send.missing.reason.{}", f.reason)));
                        if ui
                            .add_enabled(
                                f.record_id.is_some(),
                                egui::Button::new(t("send.open_record")),
                            )
                            .clicked()
                        {
                            *act =
                                Some(Act::OpenRecord(f.record_type.clone(), f.record_id.clone()));
                        }
                        ui.end_row();
                    }
                });
        });
    }
    if let Some(o) = &s.outcome {
        ui.add_space(8.0);
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(t("send.done")).strong());
            ui.label(t_with(
                "send.done.path",
                &[("path", &o.record.path.display().to_string())],
            ));
            if o.discarded_payroll > 0 {
                form::hint(
                    ui,
                    tokens,
                    &crate::i18n::t_count(
                        "send.done.discarded",
                        usize::try_from(o.discarded_payroll).unwrap_or(usize::MAX),
                        &[],
                    ),
                );
            }
            form::hint(ui, tokens, &t("send.how.body"));
            if ui.button(t("send.open_folder")).clicked() {
                *act = Some(Act::OpenFolder);
            }
        });
    }
}

fn history(s: &SendScreen, ui: &mut Ui, tokens: &Tokens) {
    form::card(ui, tokens, |ui| {
        ui.label(RichText::new(t("send.history")).strong());
        if s.history.is_empty() {
            form::hint(ui, tokens, &t("send.history.empty"));
            return;
        }
        egui::Grid::new("send_history")
            .num_columns(4)
            .striped(true)
            .spacing([18.0, 4.0])
            .show(ui, |ui| {
                for h in [
                    "send.history.date",
                    "send.history.how",
                    "send.history.file",
                    "send.history.net_income",
                ] {
                    ui.label(RichText::new(t(h)).color(tokens.text_secondary));
                }
                ui.end_row();
                for r in s.history.iter().rev() {
                    let local = r.created_at.as_utc().with_timezone(&chrono::Local);
                    ui.label(local.format("%m/%d/%Y").to_string());
                    ui.label(t("send.history.kind"));
                    ui.label(
                        r.path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    );
                    ui.label(
                        RichText::new(
                            r.totals
                                .as_ref()
                                .map(|x| amount(x.net_income))
                                .unwrap_or_default(),
                        )
                        .monospace(),
                    );
                    ui.end_row();
                }
            });
    });
}
