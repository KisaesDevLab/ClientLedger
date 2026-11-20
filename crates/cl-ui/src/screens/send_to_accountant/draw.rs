//! Drawing Send to My Accountant (layout of `SendToAccountant.html`).
//!
//! Two columns from [`WIDE`] points of room, else one column in the
//! mockup's order: what's included, check totals, the file card (with the
//! result right under Save), then Sent before (P32 UI B1, S2).

use cl_api::VerificationTotals;
use cl_company::transfer::TransferError;
use cl_core::report::display_amount;
use cl_core::{Money, MoneyStyle};
use eframe::egui::{self, Align, RichText, Ui};

use super::{Ask, SendScreen};
use crate::i18n::{count_text, t, t_count};
use crate::screens::company::form;
use crate::theme::Tokens;

/// The room two columns need.
pub(super) const WIDE: f32 = 1100.0;

fn amount(m: Money) -> String {
    display_amount(m.as_decimal(), MoneyStyle::default())
}

/// Right-aligned amounts: monospace text padded to the widest (S5).
fn right_aligned(values: &[String]) -> Vec<String> {
    let w = values.iter().map(|v| v.chars().count()).max().unwrap_or(0);
    values.iter().map(|v| format!("{v:>w$}")).collect()
}

/// `path` without `.` and `..` parts, as the owner reads it (M2).
fn tidy(path: &std::path::Path) -> String {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out.display().to_string()
}

/// What a draw asked for, run after the frame's borrows end.
enum Act {
    Save,
    MakeOne,
    Copy,
    CopyResult,
    Payroll(bool),
    OpenFolder,
    Refresh,
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
            let wide = ui.available_width() >= WIDE;
            steps(ui, &tokens, wide);
            ui.add_space(10.0);
            if wide {
                ui.columns(2, |cols| {
                    included(s, &mut cols[0], &tokens);
                    cols[0].add_space(10.0);
                    history(s, &mut cols[0], &tokens);
                    totals(s, &mut cols[1], &tokens, true, &mut act);
                    cols[1].add_space(10.0);
                    file_card(s, &mut cols[1], &tokens, &mut act);
                });
            } else {
                included(s, ui, &tokens);
                ui.add_space(10.0);
                totals(s, ui, &tokens, false, &mut act);
                ui.add_space(10.0);
                file_card(s, ui, &tokens, &mut act);
                ui.add_space(10.0);
                history(s, ui, &tokens);
            }
        });
    let ctx = ui.ctx().clone();
    match act {
        Some(Act::Save) => s.save(Some(&ctx)),
        Some(Act::MakeOne) => s.make_one(),
        Some(Act::Copy) => {
            crate::secret_clipboard::copy(&ctx, s.passphrase.text().to_string());
            s.message = Some("send.copied");
            s.recorded = true;
        }
        Some(Act::CopyResult) => {
            crate::secret_clipboard::copy(&ctx, s.result_pass.text().to_string());
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
                    tidy(&dir).replace('\\', "/")
                )));
            }
        }
        Some(Act::OpenRecord(kind, id)) => s.open_record(kind.as_deref(), id.as_deref()),
        Some(Act::Iif) => s.open_iif(),
        Some(Act::Refresh) => s.refresh(),
        None => {}
    }
}

/// A numbered accent badge (the mockup's step marker, M1).
fn badge(ui: &mut Ui, tokens: &Tokens, n: usize) {
    let size = 22.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), size / 2.0, tokens.accent);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        n.to_string(),
        egui::FontId::proportional(13.0),
        tokens.on_accent,
    );
}

fn step(ui: &mut Ui, tokens: &Tokens, i: usize, title: &str, sub: &str) {
    form::card_tight(ui, tokens, |ui| {
        ui.horizontal(|ui| {
            badge(ui, tokens, i + 1);
            ui.vertical(|ui| {
                ui.label(RichText::new(t(title)).strong().color(tokens.text_primary));
                ui.add(egui::Label::new(RichText::new(t(sub)).color(tokens.text_secondary)).wrap());
            });
        });
    });
}

const STEPS: [(&str, &str); 3] = [
    ("send.step1", "send.step1.sub"),
    ("send.step2", "send.step2.sub"),
    ("send.step3", "send.step3.sub"),
];

fn steps(ui: &mut Ui, tokens: &Tokens, wide: bool) {
    if wide {
        ui.columns(3, |cols| {
            for (i, (title, sub)) in STEPS.into_iter().enumerate() {
                step(&mut cols[i], tokens, i, title, sub);
            }
        });
    } else {
        for (i, (title, sub)) in STEPS.into_iter().enumerate() {
            step(ui, tokens, i, title, sub);
            ui.add_space(4.0);
        }
    }
}

fn included(s: &SendScreen, ui: &mut Ui, tokens: &Tokens) {
    form::card(ui, tokens, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.spacing_mut().interact_size.y = 16.0;
        ui.label(RichText::new(t("send.included")).strong());
        let counts = s.preview.as_ref().map(|p| p.counts);
        let n = |key: &str, v: Option<u64>| match v {
            Some(v) => t_count(
                key,
                usize::try_from(v).unwrap_or(usize::MAX),
                &[("n", &count_text(v))],
            ),
            None => crate::i18n::t_with(key, &[("n", "\u{2026}")]),
        };
        let yes = |ui: &mut Ui, text: String, tip: Option<String>| {
            ui.horizontal(|ui| {
                crate::widgets::check_mark(ui, 14.0, tokens.accent, &t("send.included.yes"));
                let r =
                    ui.add(egui::Label::new(RichText::new(text).color(tokens.text_primary)).wrap());
                if let Some(tip) = tip {
                    r.on_hover_text(tip);
                }
            });
        };
        yes(
            ui,
            n("send.inc.lists", counts.map(|c| c.transactions)),
            None,
        );
        yes(
            ui,
            n("send.inc.attachments", counts.map(|c| c.attachments)),
            None,
        );
        yes(ui, t("send.inc.audit"), None);
        yes(ui, t("send.inc.tags"), None);
        yes(ui, t("send.inc.settings"), None);
        yes(ui, t("send.inc.names"), None);
        yes(ui, t("send.inc.hidden"), Some(t("send.inc.hidden.tip")));
        let no = |ui: &mut Ui, text: String| {
            ui.add(
                egui::Label::new(
                    RichText::new(format!("\u{d7}  {text}")).color(tokens.text_secondary),
                )
                .wrap(),
            );
        };
        no(ui, t("send.exc.secrets"));
        if let Some(p) = &s.preflight
            && p.waiting_documents > 0
        {
            no(ui, n("send.exc.documents", Some(p.waiting_documents)));
        }
    });
}

fn totals_grid(ui: &mut Ui, p: &VerificationTotals, id: &str, pairs: bool) {
    let labels = [
        t("send.total.assets"),
        t("send.total.ar"),
        t("send.total.liabilities"),
        t("send.total.ap"),
        t("send.total.equity"),
        t("send.total.transactions"),
        crate::i18n::t_with(
            "send.total.net_income",
            &[
                ("from", &crate::shell::us_date(p.net_income.from)),
                ("to", &crate::shell::us_date(p.net_income.to)),
            ],
        ),
        t("send.total.attachments"),
    ];
    let values = right_aligned(&[
        amount(p.total_assets),
        amount(p.accounts_receivable),
        amount(p.total_liabilities),
        amount(p.accounts_payable),
        amount(p.total_equity),
        count_text(p.counts.transactions),
        amount(p.net_income.amount),
        count_text(p.counts.attachments),
    ]);
    let per_row = if pairs { 2 } else { 1 };
    egui::Grid::new(id)
        .num_columns(2 * per_row)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            for (i, (label, value)) in labels.iter().zip(&values).enumerate() {
                ui.label(label);
                ui.label(RichText::new(value).strong().monospace());
                if (i + 1) % per_row == 0 {
                    ui.end_row();
                }
            }
        });
}

fn totals(s: &SendScreen, ui: &mut Ui, tokens: &Tokens, wide: bool, act: &mut Option<Act>) {
    form::card(ui, tokens, |ui| {
        // Two pairs a row only when the card has room for them.
        let pairs = wide || ui.available_width() >= 560.0;
        let in_file = s.outcome.as_ref().and_then(|o| o.totals.as_ref());
        match (in_file, s.preview.as_ref()) {
            (Some(f), _) => {
                ui.label(RichText::new(t("send.totals.file")).strong());
                if s.totals_differ() {
                    form::inline_warning(ui, tokens, &t("send.totals.changed"));
                }
                totals_grid(ui, f, "send_totals_file", pairs);
            }
            (None, Some(p)) => {
                ui.add(egui::Label::new(RichText::new(t("send.totals")).strong()).wrap());
                totals_grid(ui, p, "send_totals_preview", pairs);
                // QA-4: read again after changes in another window.
                ui.horizontal(|ui| {
                    let busy = s.preview_job.is_some() || s.preflight_job.is_some();
                    if ui
                        .add_enabled(
                            !busy && s.task.is_none(),
                            egui::Button::new(t("send.refresh")),
                        )
                        .on_hover_text(t("send.refresh.tip"))
                        .clicked()
                    {
                        *act = Some(Act::Refresh);
                    }
                    if busy {
                        form::hint(ui, tokens, &t("send.totals.loading"));
                    }
                });
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

/// The generated passphrase, hidden until Show (Q-P32w). Shown, its label
/// is marked secret for the control channel, whatever the text looks like.
fn generated(s: &mut SendScreen, ui: &mut Ui) {
    if !s.generated {
        return;
    }
    ui.checkbox(&mut s.show_generated, t("send.show"))
        .on_hover_text(t("send.show.tip"));
    if s.show_generated {
        let r = ui.label(RichText::new(s.passphrase.text()).monospace().strong());
        ui.ctx().accesskit_node_builder(r.id, |node| {
            node.set_description(cl_automation::inspect::SECRET_MARK);
        });
    }
}

fn file_card(s: &mut SendScreen, ui: &mut Ui, tokens: &Tokens, act: &mut Option<Act>) {
    let running = s.task.is_some() || s.held.is_some();
    form::card(ui, tokens, |ui| {
        ui.add_enabled_ui(!running, |ui| {
            // QA-9: the whole width, so the default name is never cut.
            ui.horizontal(|ui| {
                let l = ui.add(egui::Label::new(t("send.file_name")).selectable(false));
                ui.add_sized(
                    [ui.available_width(), 24.0],
                    egui::TextEdit::singleline(&mut s.file_name),
                )
                .labelled_by(l.id);
            });
            ui.add_space(4.0);
            let a = form::password_secret(ui, &t("send.passphrase"), &mut s.passphrase);
            let b = form::password_secret(ui, &t("send.passphrase_again"), &mut s.again);
            if a.changed() || b.changed() {
                // Typed by the owner: no longer the one Make one for me made.
                s.generated = false;
                s.show_generated = false;
                s.recorded = false;
            }
            form::hint(ui, tokens, &t("send.passphrase.hint"));
            ui.horizontal_wrapped(|ui| {
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
                generated(s, ui);
            });
            // QA-2: a made passphrase must be copied or written down
            // before Save, or nobody can open the file.
            if s.generated {
                ui.checkbox(&mut s.recorded, t("send.recorded"))
                    .on_hover_text(t("send.recorded.tip"));
            }
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
            && !matches!(e.kind(), TransferError::MissingFiles(_))
        {
            form::inline_error(ui, tokens, e);
        }
        // Review S7: the payroll imports are gone even though the file
        // wasn't made; say so, and which backup still holds them.
        if let Some(d) = s.error.as_ref().and_then(TransferError::discarded) {
            ui.label(RichText::new(super::discard_note(d)).color(tokens.warn_text));
        }
        ui.add_space(8.0);
        if running {
            let text = match s.progress.lock().ok().and_then(|p| p.clone()) {
                Some(cl_company::Progress::Working {
                    percent: Some(p), ..
                }) => crate::i18n::t_with("send.working.percent", &[("p", &p.to_string())]),
                Some(cl_company::Progress::Copying) => t("send.working.copy"),
                Some(cl_company::Progress::Verifying) => t("send.working.verify"),
                _ => t("send.working"),
            };
            form::progress_line(ui, tokens, &text);
        } else if s.ask != Ask::Payroll {
            ui.horizontal(|ui| {
                ui.add_enabled_ui(s.supported() && s.can_save_now(), |ui| {
                    let r = crate::widgets::primary_button(ui, &t("send.save"));
                    if r.clicked() {
                        *act = Some(Act::Save);
                    }
                    if !s.can_save_now() {
                        r.on_disabled_hover_text(t("send.recorded.first"));
                    }
                });
            });
        }
        result(s, ui, tokens, act);
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
        ui.label(
            RichText::new(t_count(
                "send.payroll.title",
                usize::try_from(n).unwrap_or(usize::MAX),
                &[("n", &count_text(n))],
            ))
            .strong(),
        );
        form::hint(ui, tokens, &t("send.payroll.body"));
        // Discard is the default (Q-P30c): first, and Enter takes it.
        match form::actions(ui, &t("send.payroll.discard"), &[&t("send.payroll.cancel")]) {
            Some(0) => *act = Some(Act::Payroll(true)),
            Some(_) => *act = Some(Act::Payroll(false)),
            None => {}
        }
    });
}

/// The result or the missing files, right under Save; scrolled into view
/// the frame they arrive (S2).
fn result(s: &mut SendScreen, ui: &mut Ui, tokens: &Tokens, act: &mut Option<Act>) {
    let mut shown: Option<egui::Response> = None;
    if let Some(TransferError::MissingFiles(files)) = s.error.as_ref().map(TransferError::kind) {
        ui.add_space(8.0);
        let (r, ask) =
            crate::widgets::missing_files::panel(ui, tokens, files, "send.missing.body", "send");
        if let Some((kind, id)) = ask {
            *act = Some(Act::OpenRecord(kind, id));
        }
        shown = Some(r);
    }
    if let Some(o) = &s.outcome {
        ui.add_space(8.0);
        let r = form::card_tight(ui, tokens, |ui| {
            ui.label(RichText::new(t("send.done")).strong());
            ui.label(t("send.done.saved_as"));
            ui.add(egui::Label::new(RichText::new(tidy(&o.record.path)).monospace()).wrap());
            if o.discarded_payroll > 0 {
                form::hint(
                    ui,
                    tokens,
                    &t_count(
                        "send.done.discarded",
                        usize::try_from(o.discarded_payroll).unwrap_or(usize::MAX),
                        &[("n", &count_text(o.discarded_payroll))],
                    ),
                );
            }
            if !s.result_pass.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new(t("send.done.passphrase")).strong());
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut s.show_result_pass, t("send.show"))
                        .on_hover_text(t("send.show.tip"));
                    if ui
                        .button(t("send.copy"))
                        .on_hover_text(t("send.copy.tip"))
                        .clicked()
                    {
                        *act = Some(Act::CopyResult);
                    }
                });
                if s.show_result_pass {
                    let l = ui.label(RichText::new(s.result_pass.text()).monospace().strong());
                    ui.ctx().accesskit_node_builder(l.id, |node| {
                        node.set_description(cl_automation::inspect::SECRET_MARK);
                    });
                }
            }
            ui.horizontal(|ui| ui.button(t("send.open_folder"))).inner
        });
        if r.clicked() {
            *act = Some(Act::OpenFolder);
        }
        shown = Some(r);
    }
    if std::mem::take(&mut s.scroll_result)
        && let Some(r) = shown
    {
        r.scroll_to_me(Some(Align::Max));
    }
}

fn history(s: &SendScreen, ui: &mut Ui, tokens: &Tokens) {
    form::card(ui, tokens, |ui| {
        ui.label(RichText::new(t("send.history")).strong());
        if s.history.is_empty() {
            form::hint(ui, tokens, &t("send.history.empty"));
            return;
        }
        let rows: Vec<_> = s.history.iter().rev().collect();
        let incomes = right_aligned(
            &rows
                .iter()
                .map(|r| {
                    r.totals
                        .as_ref()
                        .map(|x| amount(x.net_income))
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>(),
        );
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
                for (r, income) in rows.iter().zip(&incomes) {
                    let local = r.created_at.as_utc().with_timezone(&chrono::Local);
                    ui.label(RichText::new(local.format("%m/%d/%Y").to_string()).monospace());
                    ui.label(t("send.history.kind"));
                    ui.label(
                        r.path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    );
                    ui.label(RichText::new(income).monospace());
                    ui.end_row();
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_line_up_on_the_right_and_paths_read_plainly() {
        let v = right_aligned(&["9,286.90".into(), "412,906.33".into()]);
        assert_eq!(v, ["  9,286.90", "412,906.33"]);
        let p = std::path::Path::new("C:/a/b/../c/./d.clxfer");
        assert_eq!(tidy(p).replace('\\', "/"), "C:/a/c/d.clxfer");
    }
}
