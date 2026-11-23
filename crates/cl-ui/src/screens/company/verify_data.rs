//! Verify Data (Utilities) (P34 B3, U-12, Q-170–Q-172): Quick check and
//! Full check, the findings errors first, "Fix selected…" behind a
//! confirmation that names the automatic backup, and the way to the
//! Duplicate Finder.
//!
//! The fix runs `cl_company::health::fix_after_backup` (verified before-fix
//! backup, fix with the run's ledger version, check again) under a routine
//! maintenance hold on a sharing host (Q-P34d). On an office-network
//! workstation the checks run and the fix is the host's.
//!
//! Every figure is the engine's exact string; nothing here compares or
//! computes an amount. Labels are shown, never logged.

use std::sync::Arc;

use cl_api::{ApiError, IntegrityLevel, IntegrityRun};
use cl_company::CompanyError;
use cl_company::health::FixOutcome;
use cl_core::MoneyStyle;
use cl_core::ids::TenantId;
use cl_model::verify_data::{
    Figures, FindingView, OpenTarget, Severity, SubjectKind, VerifyModel, code_key, open_target,
};
use cl_model::{CommandId, Scope};
use eframe::egui::{RichText, Ui};
use serde_json::{Value, json};

use super::form;
use crate::company::{CompanyEvent, CompanyLink, Task, spawn_lifecycle};
use crate::i18n::{t, t_count, t_with};
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command that opens it.
pub const KIND: &str = "file.verify_data";

/// "Check My Books" (crash recovery): the next Verify Data window runs a
/// quick check as it opens (QA-17).
static QUICK_ON_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Ask the next Verify Data window to start with a quick check.
pub fn request_quick_check_on_open() {
    QUICK_ON_OPEN.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// The engine's finding as the model shows it.
#[must_use]
pub fn view_of(f: &cl_api::Finding) -> FindingView {
    FindingView {
        id: f.id.as_str().to_string(),
        code: f.code.clone(),
        severity: match f.severity {
            cl_api::Severity::Error => Severity::Error,
            cl_api::Severity::Warning => Severity::Warning,
        },
        subject: match f.subject.kind {
            cl_api::SubjectKind::Account => SubjectKind::Account,
            cl_api::SubjectKind::Transaction => SubjectKind::Transaction,
            cl_api::SubjectKind::DocumentDraft => SubjectKind::DocumentDraft,
            cl_api::SubjectKind::Item => SubjectKind::Item,
            cl_api::SubjectKind::Other => SubjectKind::Other,
        },
        subject_id: f.subject.id.clone(),
        label: f.subject.label.clone(),
        figures: f.amounts.as_ref().map(|a| Figures {
            expected: a.expected_exact,
            actual: a.actual_exact,
            difference: a.difference_exact,
        }),
        fixable: f.fixable(),
    }
}

/// "Found 1 problem and 2 things to look at." with each count's own
/// singular (QA-5).
#[must_use]
pub fn summary_text(errors: usize, warnings: usize) -> String {
    t_with(
        "verify.summary",
        &[
            ("errors", &t_count("verify.summary.errors", errors, &[])),
            (
                "warnings",
                &t_count("verify.summary.warnings", warnings, &[]),
            ),
        ],
    )
}

/// How the last fix ended.
#[derive(Clone, Debug, PartialEq, Eq)]
enum FixEnd {
    /// Fixed this many; the re-check is shown.
    Fixed(usize),
    /// The books changed since the check (`INTEGRITY_STALE`).
    Stale,
    /// The backup or the fix failed (nothing fixed without a backup).
    Failed(String),
}

/// The Verify Data window.
pub struct VerifyDataScreen {
    link: Arc<CompanyLink>,
    group: Option<cl_api::RequestGroup>,
    tenant: Option<(TenantId, String)>,
    check: Option<Job<IntegrityRun>>,
    checking: Option<IntegrityLevel>,
    run: Option<IntegrityRun>,
    model: VerifyModel,
    confirm: bool,
    fix: Option<Task<Result<FixOutcome, CompanyError>>>,
    fix_end: Option<FixEnd>,
    error: Option<ApiError>,
}

impl VerifyDataScreen {
    /// The window over the open company.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, tenant: Option<(TenantId, String)>) -> VerifyDataScreen {
        VerifyDataScreen {
            group: link.group().and_then(Result::ok),
            link,
            tenant,
            check: None,
            checking: None,
            run: None,
            model: VerifyModel::default(),
            confirm: false,
            fix: None,
            fix_end: None,
            error: None,
        }
    }

    fn may_fix(&self) -> bool {
        !self.link.is_workstation() && self.link.rights().is_admin()
    }

    fn start_check(&mut self, level: IntegrityLevel) {
        if self.check.is_some() || self.fix.is_some() {
            return;
        }
        self.error = None;
        self.confirm = false;
        self.checking = Some(level);
        self.check = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| d.integrity.check(level),
            move |s| async move { s.integrity().check(level).await },
        ));
        self.poll();
    }

    fn start_fix(&mut self) {
        let Some(run) = self.run.clone() else {
            return;
        };
        let ids: Vec<String> = self.model.selected_ids();
        if ids.is_empty() || !self.may_fix() {
            return;
        }
        self.confirm = false;
        self.fix_end = None;
        // The demo "backup" first, as the real flow does.
        let demo = self
            .link
            .demo_state()
            .map(|mut d| d.integrity.fix(&run, "demo-before-fix"));
        if let Some(r) = demo {
            match r {
                Ok(f) => {
                    self.fix_end = Some(FixEnd::Fixed(f.fixed.len()));
                    self.link.push(CompanyEvent::Toast("verify.fix.toast"));
                    let level = run.level;
                    self.start_check(level);
                }
                Err(e) if cl_api::is_integrity_stale(&e) => self.fix_end = Some(FixEnd::Stale),
                Err(e) => self.fix_end = Some(FixEnd::Failed(e.to_string())),
            }
            return;
        }
        let (Some(session), Some(group), Some((tenant_id, _))) = (
            self.link.session(),
            self.group.as_ref(),
            self.tenant.clone(),
        ) else {
            return;
        };
        let store = self.link.store().clone();
        let secrets = self.link.secrets();
        let backup_dir = store
            .load()
            .ok()
            .and_then(|l| l.backup_folder)
            .unwrap_or_else(|| self.link.backup_dir.clone());
        let finding_ids: Vec<cl_api::IntegrityId> = run
            .findings
            .iter()
            .filter(|f| ids.contains(&f.id.as_str().to_string()))
            .map(|f| f.id.clone())
            .collect();
        // Review S1: the archive gate first, so the LAN hold below never
        // waits behind a test restore or another backup.
        let Some(gate) = cl_company::backup::try_gate() else {
            self.fix_end = Some(FixEnd::Failed(t("verify.gate_busy")));
            return;
        };
        // Q-P34d: gate → routine hold → verified backup → fix → re-check.
        self.fix = spawn_lifecycle(&self.link, group, move || async move {
            let req = cl_company::health::FixRequest {
                tenant_id,
                backup_dir: &backup_dir,
                run: &run,
                finding_ids: &finding_ids,
            };
            crate::lan_status::with_maintenance(
                &session,
                crate::lan_status::Hold::Routine,
                cl_company::health::fix_after_backup(
                    gate,
                    &session,
                    &store,
                    secrets.as_ref(),
                    &req,
                    &cl_company::backup::silent(),
                ),
            )
            .await
        })
        .ok();
    }

    fn set_run(&mut self, run: IntegrityRun) {
        self.model = VerifyModel::new(run.findings.iter().map(view_of).collect());
        self.run = Some(run);
    }

    fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.check) {
            self.checking = None;
            match r {
                Ok(run) => self.set_run(run),
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(task) = self.fix.as_mut()
            && let Some(r) = task.poll_result()
        {
            self.fix = None;
            match r {
                Ok(out) => {
                    self.fix_end = Some(FixEnd::Fixed(out.result.fixed.len()));
                    self.link.push(CompanyEvent::Toast("verify.fix.toast"));
                    self.link
                        .bus()
                        .publish(crate::data_bus::Change::Ledger(None));
                    match out.recheck {
                        Ok(run) => self.set_run(run),
                        Err(_) => self.run = None,
                    }
                }
                Err(CompanyError::Api(e)) if cl_api::is_integrity_stale(&e) => {
                    self.fix_end = Some(FixEnd::Stale);
                }
                Err(e) => {
                    let m = cl_core::OwnerFacing::owner_message(&e);
                    self.fix_end = Some(FixEnd::Failed(t(m.key)));
                }
            }
        }
    }

    fn open(&self, f: &FindingView) {
        match open_target(f) {
            Some(OpenTarget::Register(id)) => {
                if let Ok(a) = cl_core::ids::AccountId::parse(&id) {
                    self.link.push(CompanyEvent::OpenRegister(a));
                }
            }
            Some(OpenTarget::Transaction(id)) => {
                if let Ok(tx) = cl_core::ids::TransactionId::parse(&id) {
                    self.link.push(CompanyEvent::OpenRecord(
                        CommandId::new("company.journal_entry"),
                        tx,
                    ));
                }
            }
            Some(OpenTarget::ReviewDocuments) => self.link.push(CompanyEvent::OpenWindow(
                CommandId::new("vendors.review_documents"),
            )),
            Some(OpenTarget::Items) => self
                .link
                .push(CompanyEvent::OpenWindow(CommandId::new("lists.items"))),
            None => {}
        }
    }

    fn open_duplicates(&self) {
        self.link.push(CompanyEvent::OpenWith(
            CommandId::new(crate::screens::reports::KIND),
            Box::new(crate::windows::OpenPayload::Report(Box::new(
                crate::screens::reports::ReportOpen {
                    key: "duplicate_finder".into(),
                    options: None,
                },
            ))),
        ));
    }

    fn finding_row(&mut self, ui: &mut Ui, tokens: &Tokens, f: &FindingView) {
        let style = MoneyStyle::default();
        ui.horizontal_top(|ui| {
            if f.fixable && self.may_fix() {
                let mut on = self.model.is_selected(&f.id);
                // QA-9: the name is said once on screen; the box's
                // screen-reader name still says which account it fixes.
                let r = ui.checkbox(&mut on, "");
                let name = t_with("verify.select", &[("name", &f.label)]);
                r.widget_info(|| {
                    eframe::egui::WidgetInfo::selected(
                        eframe::egui::Role::CheckBox,
                        true,
                        on,
                        &name,
                    )
                });
                if r.changed() {
                    self.model.toggle(&f.id);
                }
            }
            // QA-4: everything wraps in the row's width; the Open button
            // sits under the text, never past the window's edge.
            ui.vertical(|ui| {
                ui.add(eframe::egui::Label::new(RichText::new(&f.label).strong()).wrap());
                ui.add(eframe::egui::Label::new(t(code_key(&f.code))).wrap());
                if let Some(fig) = &f.figures {
                    ui.label(
                        RichText::new(t_with(
                            "verify.figures",
                            &[
                                (
                                    "expected",
                                    &cl_core::report::display_amount(
                                        fig.expected.as_decimal(),
                                        style,
                                    ),
                                ),
                                (
                                    "actual",
                                    &cl_core::report::display_amount(
                                        fig.actual.as_decimal(),
                                        style,
                                    ),
                                ),
                                (
                                    "difference",
                                    &cl_core::report::display_amount(
                                        fig.difference.as_decimal(),
                                        style,
                                    ),
                                ),
                            ],
                        ))
                        .monospace()
                        .color(tokens.text_secondary),
                    );
                }
                if !f.fixable {
                    form::hint(ui, tokens, &t("verify.report_only"));
                }
                if open_target(f).is_some()
                    && ui
                        .add(
                            eframe::egui::Button::new(t_with("verify.open", &[("name", &f.label)]))
                                .wrap(),
                        )
                        .clicked()
                {
                    self.open(f);
                }
            });
        });
        ui.add_space(4.0);
    }
}

impl Screen for VerifyDataScreen {
    fn title(&self) -> String {
        t("verify.title")
    }

    fn scope(&self) -> Scope {
        Scope::Window
    }

    #[allow(clippy::too_many_lines, reason = "one screen, top to bottom")]
    fn ui(&mut self, ui: &mut Ui) {
        // QA-17: taken on a frame, so a window already open (or restored
        // at launch) answers "Check My Books" too.
        if QUICK_ON_OPEN.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.start_check(IntegrityLevel::Quick);
        }
        self.poll();
        let tokens = Tokens::of(ui.ctx());
        if self.check.is_some() || self.fix.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        ui.add_space(8.0);
        ui.heading(t("verify.title"));
        form::hint(ui, &tokens, &t("verify.intro"));
        ui.add_space(8.0);
        let busy = self.check.is_some() || self.fix.is_some();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, eframe::egui::Button::new(t("verify.quick")))
                .on_hover_text(t("verify.quick.tip"))
                .clicked()
            {
                self.start_check(IntegrityLevel::Quick);
            }
            if ui
                .add_enabled(!busy, eframe::egui::Button::new(t("verify.full")))
                .on_hover_text(t("verify.full.tip"))
                .clicked()
            {
                self.start_check(IntegrityLevel::Full);
            }
        });
        if self.checking.is_some() {
            form::progress_line(ui, &tokens, &t("verify.checking"));
        }
        if self.fix.is_some() {
            form::progress_line(ui, &tokens, &t("verify.fixing"));
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        match &self.fix_end {
            Some(FixEnd::Fixed(n)) => {
                form::inline_success(ui, &tokens, &t_count("verify.fixed", *n, &[]));
            }
            Some(FixEnd::Stale) => form::inline_warning(ui, &tokens, &t("verify.stale")),
            Some(FixEnd::Failed(why)) => {
                form::inline_warning(ui, &tokens, &t_with("verify.fix_failed", &[("why", why)]));
            }
            None => {}
        }
        ui.add_space(6.0);
        if self.run.is_none() {
            if self.check.is_none() {
                form::hint(ui, &tokens, &t("verify.empty"));
            }
        } else if self.model.is_clean() {
            form::inline_success(ui, &tokens, &t("verify.clean"));
        } else {
            let (errors, warnings) = self.model.counts();
            ui.label(RichText::new(summary_text(errors, warnings)).strong());
            if self.run.as_ref().is_some_and(|r| r.truncated) {
                form::hint(ui, &tokens, &t("verify.truncated"));
            }
            let findings: Vec<FindingView> = self.model.findings().to_vec();
            for (sev, key) in [
                (Severity::Error, "verify.errors"),
                (Severity::Warning, "verify.warnings"),
            ] {
                let group: Vec<&FindingView> =
                    findings.iter().filter(|f| f.severity == sev).collect();
                if group.is_empty() {
                    continue;
                }
                form::card(ui, &tokens, |ui| {
                    form::heading(ui, &tokens, &t(key));
                    for f in group {
                        self.finding_row(ui, &tokens, f);
                    }
                });
            }
            if self.model.fixable_count() > 0 {
                ui.add_space(6.0);
                if self.may_fix() {
                    let n = self.model.selected_ids().len();
                    if !self.confirm
                        && ui
                            .add_enabled(
                                n > 0 && !busy,
                                eframe::egui::Button::new(t("verify.fix_selected")),
                            )
                            .clicked()
                    {
                        self.confirm = true;
                    }
                } else if self.link.is_workstation() {
                    form::hint(ui, &tokens, &t("verify.host_fixes"));
                } else {
                    form::hint(ui, &tokens, &t("verify.admin_fixes"));
                }
            }
        }
        if self.confirm {
            let n = self.model.selected_ids().len();
            form::card(ui, &tokens, |ui| {
                form::heading(ui, &tokens, &t("verify.confirm.title"));
                form::hint(ui, &tokens, &t_count("verify.confirm.body", n, &[]));
                match form::actions(ui, &t("verify.confirm.go"), &[&t("verify.confirm.cancel")]) {
                    Some(0) => self.start_fix(),
                    Some(_) => self.confirm = false,
                    None => {}
                }
            });
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(t("verify.duplicates"));
            if ui.button(t("verify.duplicates.open")).clicked() {
                self.open_duplicates();
            }
        });
    }

    fn command(&mut self, _id: CommandId) -> bool {
        false
    }

    fn state(&self) -> Value {
        let (errors, warnings) = self.model.counts();
        json!({
            "screen": "verify_data",
            "checking": self.checking.is_some(),
            "ran": self.run.is_some(),
            "level": self.run.as_ref().map(|r| match r.level {
                IntegrityLevel::Quick => "quick",
                IntegrityLevel::Full => "full",
            }),
            "errors": errors,
            "warnings": warnings,
            "fixable": self.model.fixable_count(),
            "selected": self.model.selected_ids().len(),
            "confirm_open": self.confirm,
            "fixing": self.fix.is_some(),
            "fixed": match &self.fix_end { Some(FixEnd::Fixed(n)) => json!(n), _ => Value::Null },
            "stale": self.fix_end == Some(FixEnd::Stale),
            "fix_failed": matches!(self.fix_end, Some(FixEnd::Failed(_))),
            "may_fix": self.may_fix(),
            "error": self.error.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> (tempfile::TempDir, VerifyDataScreen) {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let tenant = TenantId::parse("d0000000-0000-4000-8000-000000000001").expect("id");
        let s = VerifyDataScreen::new(link, Some((tenant, "Ozark".into())));
        (dir, s)
    }

    #[test]
    fn check_select_confirm_fix_and_check_again_in_the_demo() {
        let (_d, mut s) = screen();
        assert_eq!(s.state()["ran"], false);
        s.start_check(IntegrityLevel::Full);
        let st = s.state();
        assert_eq!(st["ran"], true);
        assert_eq!(st["errors"], 1);
        assert_eq!(st["warnings"], 1);
        assert_eq!(st["selected"], 1, "fixable ones start selected");
        s.confirm = true;
        s.start_fix();
        let st = s.state();
        assert_eq!(st["fixed"], 1);
        assert_eq!(st["warnings"], 0, "checked again");
        assert_eq!(st["errors"], 1, "report-only stays");
        let ctx = eframe::egui::Context::default();
        crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
        crate::fonts::install(&ctx);
        let mut out = ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
            eframe::egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
        });
        out.textures_delta.clear();
    }

    #[test]
    fn a_stale_fix_says_check_again() {
        let (_d, mut s) = screen();
        s.start_check(IntegrityLevel::Quick);
        if let Some(mut d) = s.link.demo_state() {
            d.integrity.stale_next = true;
        }
        s.start_fix();
        assert_eq!(s.state()["stale"], true);
    }
}
