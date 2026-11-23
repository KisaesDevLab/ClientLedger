//! Scheduled Backups and History (P34 B4, Q-42, Q-217): two tabs.
//! - **Schedule:** on or off, any folder, daily / weekly / monthly at a
//!   local time, how many to keep, and "test each backup".
//! - **History:** every backup of the company with what made it and its
//!   last test restore, plus scheduled runs that failed or were skipped;
//!   Test restore, Show in folder and Restore on each file.
//!
//! The schedule is kept on this computer (`companies.json`); the backups
//! run here (host only on an office network). Test restore's verdict and
//! trial-balance figures are the engine's.

use std::path::PathBuf;
use std::sync::Arc;

use cl_company::CompanyError;
use cl_company::schedule::{LocalZone, SystemZone, next_slot};
use cl_company::verify::TestOutcome;
use cl_core::ids::TenantId;
use cl_core::{MoneyStyle, Timestamp};
use cl_model::{CommandId, Scope};
use cl_settings::{
    BackupRecord, BackupSchedule, BackupTrigger, CompanyRecord, Frequency, RunOutcome, TimeOfDay,
};
use eframe::egui::{RichText, Ui};
use serde_json::{Value, json};

use super::form;
use crate::company::{CompanyEvent, CompanyLink, Task, spawn_lifecycle};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command that opens it.
pub const KIND: &str = "file.backup_settings";

/// The message shown when no backup passphrase is set (with its button).
const NO_PASSPHRASE: &str = "backups.no_passphrase";

/// Which tab shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Schedule,
    History,
}

/// What the schedule form holds while it is edited.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Draft {
    enabled: bool,
    folder: String,
    frequency: u8,
    weekday: u8,
    day: String,
    at: String,
    keep_last: String,
    keep_months: String,
    test_each: bool,
}

impl Draft {
    fn of(s: Option<&BackupSchedule>, default_folder: &str) -> Draft {
        let s = s.cloned().unwrap_or_default();
        let (frequency, weekday, day) = match s.frequency {
            Frequency::Daily => (0, 0, 1),
            Frequency::Weekly { weekday } => (1, weekday, 1),
            Frequency::Monthly { day } => (2, 0, day),
        };
        Draft {
            enabled: s.enabled,
            folder: if s.folder.as_os_str().is_empty() {
                default_folder.to_string()
            } else {
                s.folder.display().to_string()
            },
            frequency,
            weekday,
            day: day.to_string(),
            at: s.at.text(),
            keep_last: s.keep_last.to_string(),
            keep_months: s.keep_months.to_string(),
            test_each: s.test_each,
        }
    }

    /// The schedule, or the i18n key of what is wrong.
    fn schedule(&self) -> Result<BackupSchedule, &'static str> {
        let folder = PathBuf::from(self.folder.trim());
        if self.enabled && folder.as_os_str().is_empty() {
            return Err("backups.no_folder");
        }
        let at = TimeOfDay::parse(&self.at).ok_or("backups.bad_time")?;
        let frequency = match self.frequency {
            1 => Frequency::Weekly {
                weekday: self.weekday.min(6),
            },
            2 => {
                let day: u8 = self.day.trim().parse().map_err(|_| "backups.bad_day")?;
                if !(1..=28).contains(&day) {
                    return Err("backups.bad_day");
                }
                Frequency::Monthly { day }
            }
            _ => Frequency::Daily,
        };
        // Review M11: bounded so the kept files always fit the history.
        let keep_last: usize = self
            .keep_last
            .trim()
            .parse()
            .ok()
            .filter(|n| (1..=MAX_KEEP_LAST).contains(n))
            .ok_or("backups.bad_keep")?;
        let keep_months: u32 = self
            .keep_months
            .trim()
            .parse()
            .ok()
            .filter(|n| *n <= MAX_KEEP_MONTHS)
            .ok_or("backups.bad_keep")?;
        Ok(BackupSchedule {
            enabled: self.enabled,
            frequency,
            at,
            folder,
            keep_last,
            keep_months,
            test_each: self.test_each,
            since: None,
        })
    }
}

/// Most newest backups a schedule keeps (the history holds 200).
const MAX_KEEP_LAST: usize = 100;
/// Most months a schedule keeps one backup for.
const MAX_KEEP_MONTHS: u32 = 60;

/// How often an open window rereads the history (a scheduled run or a
/// test restore elsewhere shows without reopening, QA-13).
const REFRESH: std::time::Duration = std::time::Duration::from_secs(2);

/// True when turning a schedule on must be refused: it could never run
/// without a backup passphrase (QA-12). The demo has none and runs nothing.
fn passphrase_missing(enabled: bool, demo: bool, secrets: &dyn cl_settings::SecretStore) -> bool {
    enabled && !demo && !cl_company::backup::has_passphrase(secrets)
}

/// The Scheduled Backups and History window.
pub struct BackupSettingsScreen {
    link: Arc<CompanyLink>,
    group: Option<cl_api::RequestGroup>,
    tenant: Option<(TenantId, String)>,
    tab: Tab,
    draft: Draft,
    record: Option<CompanyRecord>,
    message: Option<(&'static str, bool)>,
    testing: Option<(PathBuf, Task<Result<TestOutcome, CompanyError>>)>,
    test_error: Option<CompanyError>,
    browsing: Option<std::sync::mpsc::Receiver<Option<PathBuf>>>,
    refreshed: std::time::Instant,
}

/// Weekday names (i18n keys), Monday first.
const WEEKDAYS: [&str; 7] = [
    "backups.weekday.0",
    "backups.weekday.1",
    "backups.weekday.2",
    "backups.weekday.3",
    "backups.weekday.4",
    "backups.weekday.5",
    "backups.weekday.6",
];

/// A button whose screen-reader name says which file it acts on, while
/// the visible text stays short (QA-3).
fn named_button(ui: &mut Ui, enabled: bool, text: &str, name: &str) -> eframe::egui::Response {
    let r = ui.add_enabled(enabled, eframe::egui::Button::new(text));
    let name = name.to_string();
    r.widget_info(|| eframe::egui::WidgetInfo::labeled(eframe::egui::Role::Button, enabled, &name));
    r
}

/// A tab the screen reader announces as one (QA-8).
fn tab(ui: &mut Ui, current: &mut Tab, value: Tab, text: &str) -> bool {
    let on = *current == value;
    let r = ui.selectable_label(on, text);
    let label = text.to_string();
    r.widget_info(|| eframe::egui::WidgetInfo::selected(eframe::egui::Role::Tab, true, on, &label));
    if r.clicked() {
        *current = value;
        return true;
    }
    false
}

impl BackupSettingsScreen {
    /// The window over the open company.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, tenant: Option<(TenantId, String)>) -> BackupSettingsScreen {
        let mut s = BackupSettingsScreen {
            group: link.group().and_then(Result::ok),
            draft: Draft::of(None, ""),
            link,
            tenant,
            tab: Tab::Schedule,
            record: None,
            message: None,
            testing: None,
            test_error: None,
            browsing: None,
            refreshed: std::time::Instant::now(),
        };
        // The demo: each opening starts from no schedule (light and dark run
        // in one demo app).
        if s.link.is_demo()
            && let Some((id, _)) = s.tenant
        {
            let _ = s.link.store().update(|l| {
                if let Some(c) = l.get_mut(id) {
                    c.schedule = None;
                }
            });
        }
        s.reload();
        s
    }

    fn default_folder(&self) -> String {
        self.link
            .store()
            .load()
            .ok()
            .and_then(|l| l.backup_folder)
            .unwrap_or_else(|| self.link.backup_dir.clone())
            .display()
            .to_string()
    }

    /// Rereads the company's record only (the form being edited stays).
    fn refresh_record(&mut self) {
        self.refreshed = std::time::Instant::now();
        let Some((id, _)) = self.tenant.clone() else {
            return;
        };
        self.record = self
            .link
            .store()
            .load()
            .ok()
            .and_then(|l| l.get(id).cloned());
    }

    fn reload(&mut self) {
        let Some((id, name)) = self.tenant.clone() else {
            return;
        };
        if self.link.is_demo() {
            seed_demo(&self.link, id, &name);
        }
        self.refresh_record();
        let folder = self.default_folder();
        self.draft = Draft::of(
            self.record.as_ref().and_then(|r| r.schedule.as_ref()),
            &folder,
        );
    }

    fn now(&self) -> Timestamp {
        if self.link.is_demo() {
            crate::company::demo::DemoState::now()
        } else {
            cl_settings::recent::now()
        }
    }

    fn save(&mut self) {
        let Some((id, _)) = self.tenant.clone() else {
            return;
        };
        let mut schedule = match self.draft.schedule() {
            Ok(s) => s,
            Err(key) => {
                self.message = Some((key, false));
                return;
            }
        };
        // Q-P34bg: the folder must exist (never made, never replaced).
        if schedule.enabled && !self.link.is_demo() && !schedule.folder.is_dir() {
            self.message = Some(("backups.folder_missing", false));
            return;
        }
        // QA-12: a schedule that could never run is not saved.
        if passphrase_missing(
            schedule.enabled,
            self.link.is_demo(),
            self.link.secrets().as_ref(),
        ) {
            self.message = Some((NO_PASSPHRASE, false));
            return;
        }
        let old = self.record.as_ref().and_then(|r| r.schedule.clone());
        // Q-P34ba: a schedule turned on or changed counts from now.
        let changed = old.as_ref().is_none_or(|o| {
            BackupSchedule {
                since: None,
                ..o.clone()
            } != schedule
        });
        schedule.since = if changed {
            Some(self.now())
        } else {
            old.and_then(|o| o.since)
        };
        let saved = self.link.store().update(|l| {
            if let Some(c) = l.get_mut(id) {
                c.schedule = Some(schedule);
            }
        });
        self.message = Some(if saved.is_ok() {
            ("backups.saved", true)
        } else {
            ("backups.save_failed", false)
        });
        tracing::info!(changed, "backup schedule saved");
        self.reload();
    }

    fn next_text(&self) -> Option<String> {
        let s = self.record.as_ref()?.schedule.as_ref()?;
        if !s.is_active() {
            return None;
        }
        let now = self.now();
        let (at, local) = if self.link.is_demo() {
            let z = Utc;
            let at = next_slot(s, &z, now)?;
            (at, z.to_local(at))
        } else {
            let at = next_slot(s, &SystemZone, now)?;
            (at, SystemZone.to_local(at))
        };
        let _ = at;
        Some(t_with(
            "backups.next",
            &[("when", &local.format("%m/%d/%Y %H:%M").to_string())],
        ))
    }

    fn test(&mut self, path: PathBuf) {
        if self.testing.is_some() {
            return;
        }
        self.test_error = None;
        if self.link.is_demo() {
            let sha = self
                .record
                .as_ref()
                .and_then(|r| r.backups.iter().find(|b| b.path == path))
                .map(|b| b.sha256.clone())
                .unwrap_or_default();
            let at = self.now();
            let _ = self.link.store().update(|l| {
                l.record_test(
                    &sha,
                    cl_settings::BackupTest {
                        at,
                        passed: true,
                        reason: None,
                        tb: Some(demo_tb()),
                    },
                )
            });
            self.reload();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let store = self.link.store().clone();
        let secrets = self.link.secrets();
        let p = path.clone();
        if let Ok(task) = spawn_lifecycle(&self.link, group, move || async move {
            cl_company::verify::test_restore(
                &session,
                &store,
                secrets.as_ref(),
                &p,
                &cl_company::backup::silent(),
            )
            .await
        }) {
            self.testing = Some((path, task));
        }
    }

    fn poll(&mut self) {
        if let Some(chosen) = crate::widgets::file_picker::poll(&mut self.browsing)
            && let Some(dir) = chosen
        {
            self.draft.folder = dir.display().to_string();
        }
        if self.refreshed.elapsed() >= REFRESH {
            self.refresh_record();
        }
        if let Some((_, task)) = self.testing.as_mut()
            && let Some(r) = task.poll_result()
        {
            self.testing = None;
            match r {
                Ok(_) => self.reload(),
                Err(e) => self.test_error = Some(e),
            }
        }
    }

    fn schedule_tab(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.record.as_ref().is_some_and(|r| r.is_sample) {
            form::hint(ui, tokens, &t("backups.sample"));
            return;
        }
        form::card(ui, tokens, |ui| {
            ui.checkbox(&mut self.draft.enabled, t("backups.enabled"));
            form::hint(ui, tokens, &t("backups.enabled.hint"));
            ui.add_space(6.0);
            form::field_wide(ui, &t("backups.folder"), &mut self.draft.folder);
            let browse = ui
                .add_enabled(
                    self.browsing.is_none(),
                    eframe::egui::Button::new(t("backups.browse")),
                )
                .on_hover_text(t("backups.browse.tip"));
            if browse.clicked() {
                let typed = PathBuf::from(self.draft.folder.trim());
                let start = typed.is_dir().then_some(typed);
                self.browsing = Some(crate::widgets::file_picker::pick_folder(
                    ui.ctx(),
                    crate::widgets::file_picker::PickUse::BackupFolder,
                    start.as_deref(),
                ));
            }
            form::hint(ui, tokens, &t("backups.folder.hint"));
            ui.add_space(6.0);
            ui.label(t("backups.frequency"));
            ui.horizontal(|ui| {
                for (i, key) in ["backups.daily", "backups.weekly", "backups.monthly"]
                    .iter()
                    .enumerate()
                {
                    let i = u8::try_from(i).unwrap_or(0);
                    ui.radio_value(&mut self.draft.frequency, i, t(key));
                }
            });
            match self.draft.frequency {
                1 => {
                    eframe::egui::ComboBox::from_label(t("backups.weekday"))
                        .selected_text(t(WEEKDAYS[usize::from(self.draft.weekday.min(6))]))
                        .show_ui(ui, |ui| {
                            for (i, d) in WEEKDAYS.iter().enumerate() {
                                let i = u8::try_from(i).unwrap_or(0);
                                ui.selectable_value(&mut self.draft.weekday, i, t(d));
                            }
                        });
                }
                2 => {
                    form::field(ui, &t("backups.day_of_month"), &mut self.draft.day);
                }
                _ => {}
            }
            form::field(ui, &t("backups.at"), &mut self.draft.at);
            form::field(ui, &t("backups.keep_last"), &mut self.draft.keep_last);
            form::field(ui, &t("backups.keep_months"), &mut self.draft.keep_months);
            ui.checkbox(&mut self.draft.test_each, t("backups.test_each"));
            form::hint(ui, tokens, &t("backups.test_each.hint"));
        });
        if let Some(n) = self.next_text() {
            ui.label(n);
        }
        if let Some((key, ok)) = self.message {
            if ok {
                form::inline_success(ui, tokens, &t(key));
            } else {
                form::inline_warning(ui, tokens, &t(key));
            }
            if key == NO_PASSPHRASE && ui.button(t("backups.set_passphrase")).clicked() {
                self.link
                    .push(CompanyEvent::OpenWindow(CommandId::new("file.backup")));
            }
        }
        if form::actions(ui, &t("backups.save"), &[]).is_some() {
            self.save();
        }
    }

    fn history_tab(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(rec) = self.record.clone() else {
            return;
        };
        if let Some(e) = &self.test_error {
            form::inline_error(ui, tokens, e);
        }
        // QA-15: only failures since the last run that worked.
        let since_done = rec
            .schedule_runs
            .iter()
            .rposition(|r| matches!(r.outcome, RunOutcome::Done { .. }))
            .map_or(0, |i| i + 1);
        for r in rec.schedule_runs[since_done..].iter().rev().take(10) {
            let line = match &r.outcome {
                RunOutcome::Failed { code } => {
                    t_with("backups.run.failed", &[("why", &reason_text(code))])
                }
                RunOutcome::Skipped { reason } => {
                    t_with("backups.run.skipped", &[("why", &reason_text(reason))])
                }
                RunOutcome::Done { .. } => continue,
            };
            form::inline_warning(ui, tokens, &format!("{} — {line}", when_text(r.at)));
        }
        if rec.backups.is_empty() {
            form::hint(ui, tokens, &t("backups.history.empty"));
            return;
        }
        let mut action = None;
        for b in rec.backups.iter().rev() {
            form::card_tight(ui, tokens, |ui| {
                let name = b
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                ui.add(eframe::egui::Label::new(RichText::new(&name).strong()).wrap());
                ui.label(format!(
                    "{} · {}",
                    when_text(b.created_at),
                    t(trigger_key(b))
                ));
                ui.label(test_text(b));
                let testing = self.testing.as_ref().is_some_and(|(p, _)| *p == b.path);
                ui.horizontal(|ui| {
                    if testing {
                        ui.label(t("backups.test.running"));
                    } else if named_button(
                        ui,
                        self.testing.is_none(),
                        &t("backups.test.go"),
                        &t_with("backups.test.go.named", &[("name", &name)]),
                    )
                    .clicked()
                    {
                        action = Some((0, b.path.clone()));
                    }
                    if named_button(
                        ui,
                        true,
                        &t("backups.show"),
                        &t_with("backups.show.named", &[("name", &name)]),
                    )
                    .clicked()
                    {
                        action = Some((1, b.path.clone()));
                    }
                    if named_button(
                        ui,
                        true,
                        &t("backups.restore"),
                        &t_with("backups.restore.named", &[("name", &name)]),
                    )
                    .on_hover_text(t("backups.restore.hint"))
                    .clicked()
                    {
                        action = Some((2, b.path.clone()));
                    }
                });
            });
        }
        match action {
            Some((0, p)) => self.test(p),
            Some((1, p)) => {
                if let Some(dir) = p.parent() {
                    ui.ctx().open_url(eframe::egui::OpenUrl::new_tab(format!(
                        "file:///{}",
                        dir.display().to_string().replace('\\', "/")
                    )));
                }
            }
            Some((_, p)) => {
                crate::screens::company::open::request_restore(p);
                self.link
                    .push(CompanyEvent::OpenWindow(CommandId::new("file.restore")));
            }
            None => {}
        }
    }
}

/// UTC as the demo's zone (screenshots are the same everywhere).
struct Utc;

impl LocalZone for Utc {
    fn to_local(&self, t: Timestamp) -> chrono::NaiveDateTime {
        t.as_utc().naive_utc()
    }

    fn instants(&self, local: chrono::NaiveDateTime) -> cl_company::schedule::Resolved {
        Timestamp::from_unix_millis(local.and_utc().timestamp_millis()).map_or(
            cl_company::schedule::Resolved::Gap,
            cl_company::schedule::Resolved::One,
        )
    }
}

fn when_text(at: Timestamp) -> String {
    at.as_utc()
        .with_timezone(&chrono::Local)
        .format("%m/%d/%Y %H:%M")
        .to_string()
}

fn trigger_key(b: &BackupRecord) -> &'static str {
    match b.trigger {
        Some(BackupTrigger::Manual) => "backups.trigger.manual",
        Some(BackupTrigger::Scheduled) => "backups.trigger.scheduled",
        Some(BackupTrigger::Missed) => "backups.trigger.missed",
        Some(BackupTrigger::OnClose) => "backups.trigger.on_close",
        Some(BackupTrigger::BeforeFix) => "backups.trigger.before_fix",
        None => "backups.trigger.other",
    }
}

fn reason_text(code: &str) -> String {
    let key = format!("backups.reason.{code}");
    if crate::i18n::has(&key) {
        t(&key)
    } else {
        t_with("backups.reason.other", &[("code", code)])
    }
}

fn test_text(b: &BackupRecord) -> String {
    let Some(test) = &b.tested else {
        return t("backups.test.untested");
    };
    let mut s = if test.passed {
        t("backups.test.passed")
    } else {
        let code = test.reason.clone().unwrap_or_default();
        let key = format!("backups.test.why.{code}");
        let why = if crate::i18n::has(&key) {
            t(&key)
        } else {
            t_with("backups.test.why.other", &[("code", &code)])
        };
        t_with("backups.test.failed", &[("why", &why)])
    };
    if let Some(tb) = &test.tb {
        let style = MoneyStyle::default();
        s.push_str(" · ");
        s.push_str(&t_with(
            "backups.test.tb",
            &[
                (
                    "debit",
                    &cl_core::report::display_amount(tb.actual_debit.as_decimal(), style),
                ),
                (
                    "credit",
                    &cl_core::report::display_amount(tb.actual_credit.as_decimal(), style),
                ),
            ],
        ));
    }
    s
}

fn demo_tb() -> cl_settings::TestTrialBalance {
    let m = |s: &str| cl_core::Money::parse_wire(s).unwrap_or_default();
    cl_settings::TestTrialBalance {
        rows_equal: true,
        totals_equal: true,
        expected_debit: m("184210.5500"),
        expected_credit: m("184210.5500"),
        actual_debit: m("184210.5500"),
        actual_credit: m("184210.5500"),
    }
}

/// The demo company's history (synthetic): two backups and a failed
/// scheduled run, written once into the demo's own store.
fn seed_demo(link: &CompanyLink, id: TenantId, name: &str) {
    let _ = link.store().update(|l| {
        let c = l.upsert(id, name);
        if !c.backups.is_empty() {
            return;
        }
        let at = |s: &str| {
            Timestamp::parse_wire(s).unwrap_or_else(|_| crate::company::demo::DemoState::now())
        };
        let dir = link.backup_dir.clone();
        for (when, trigger) in [
            ("2026-11-02T23:00:00.000Z", BackupTrigger::Scheduled),
            ("2026-11-03T17:45:00.000Z", BackupTrigger::Manual),
        ] {
            let created = at(when);
            c.backups.push(BackupRecord {
                path: dir.join(cl_company::backup::backup_file_name(
                    name,
                    created,
                    cl_settings::BackupKind::Backup,
                )),
                sha256: format!("{:064x}", created.unix_millis()),
                archive_id: format!("demo-{}", created.unix_millis()),
                created_at: created,
                kind: cl_settings::BackupKind::Backup,
                trigger: Some(trigger),
                tested: None,
            });
        }
        c.schedule_runs.push(cl_settings::ScheduleRun {
            slot: at("2026-11-03T23:00:00.000Z"),
            at: at("2026-11-03T23:00:05.000Z"),
            outcome: RunOutcome::Failed {
                code: "FOLDER_UNREACHABLE".into(),
            },
        });
    });
}

impl Screen for BackupSettingsScreen {
    fn title(&self) -> String {
        t("backups.title")
    }

    fn scope(&self) -> Scope {
        Scope::Window
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        if self.testing.is_some() || self.browsing.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            ui.ctx().request_repaint_after(REFRESH);
        }
        let tokens = Tokens::of(ui.ctx());
        ui.add_space(8.0);
        ui.heading(t("backups.title"));
        let Some((_, name)) = self.tenant.clone() else {
            form::hint(ui, &tokens, &t("backups.no_company"));
            return;
        };
        ui.label(RichText::new(t_with("backup.company", &[("name", &name)])).strong());
        ui.horizontal(|ui| {
            tab(ui, &mut self.tab, Tab::Schedule, &t("backups.tab.schedule"));
            if tab(ui, &mut self.tab, Tab::History, &t("backups.tab.history")) {
                self.refresh_record();
            }
        });
        ui.separator();
        match self.tab {
            Tab::Schedule => self.schedule_tab(ui, &tokens),
            Tab::History => self.history_tab(ui, &tokens),
        }
    }

    fn command(&mut self, _id: CommandId) -> bool {
        false
    }

    fn state(&self) -> Value {
        let rec = self.record.as_ref();
        json!({
            "screen": "backup_settings",
            "tab": match self.tab { Tab::Schedule => "schedule", Tab::History => "history" },
            "enabled": rec.and_then(|r| r.schedule.as_ref()).is_some_and(|s| s.enabled),
            "frequency": self.draft.frequency,
            "since_set": rec.and_then(|r| r.schedule.as_ref()).is_some_and(|s| s.since.is_some()),
            "backups": rec.map_or(0, |r| r.backups.len()),
            "tested": rec.map_or(0, |r| r.backups.iter().filter(|b| b.tested.is_some()).count()),
            "failed_runs": rec.map_or(0, |r| r.schedule_runs.iter().filter(|x| matches!(x.outcome, RunOutcome::Failed { .. })).count()),
            "testing": self.testing.is_some(),
            "message": self.message.map(|(k, _)| k),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_draft_checks_what_it_saves() {
        let mut d = Draft::of(None, "D:/Backups");
        assert_eq!(d.at, "18:00");
        d.enabled = true;
        d.frequency = 2;
        d.day = "29".into();
        assert_eq!(d.schedule(), Err("backups.bad_day"));
        d.day = "28".into();
        d.at = "7:5".into();
        assert_eq!(d.schedule(), Err("backups.bad_time"));
        d.at = "07:05".into();
        d.keep_last = "0".into();
        assert_eq!(d.schedule(), Err("backups.bad_keep"));
        d.keep_last = "101".into();
        assert_eq!(d.schedule(), Err("backups.bad_keep"), "review M11");
        d.keep_last = "5".into();
        d.keep_months = "61".into();
        assert_eq!(d.schedule(), Err("backups.bad_keep"));
        d.keep_months = "12".into();
        let s = d.schedule().expect("ok");
        assert_eq!(s.frequency, Frequency::Monthly { day: 28 });
        assert_eq!(s.keep_last, 5);
        d.folder = " ".into();
        assert_eq!(d.schedule(), Err("backups.no_folder"));
    }

    #[test]
    fn saving_in_the_demo_sets_since_and_history_has_the_seed() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let id = TenantId::parse("d0000000-0000-4000-8000-000000000001").expect("id");
        let mut s = BackupSettingsScreen::new(link, Some((id, "Ozark".into())));
        assert_eq!(s.state()["backups"], 2);
        assert_eq!(s.state()["failed_runs"], 1);
        s.draft.enabled = true;
        s.save();
        assert_eq!(s.state()["enabled"], true);
        assert_eq!(s.state()["since_set"], true);
        let first = s.draft.clone();
        let p = s.record.as_ref().expect("r").backups[0].path.clone();
        s.test(p);
        assert_eq!(s.state()["tested"], 1);
        assert_eq!(s.draft, first);
    }

    /// QA-12: turning a schedule on without a backup passphrase is refused
    /// (outside the demo); with one, or turned off, it saves.
    #[test]
    fn a_schedule_without_a_passphrase_is_refused() {
        use cl_settings::SecretStore as _;
        let none = cl_settings::MemorySecrets::default();
        assert!(passphrase_missing(true, false, &none));
        assert!(!passphrase_missing(false, false, &none));
        assert!(!passphrase_missing(true, true, &none));
        let set = cl_settings::MemorySecrets::default();
        set.set(
            cl_settings::secrets::BACKUP_PASSPHRASE,
            &cl_core::Secret::new("a long synthetic passphrase"),
        )
        .expect("set");
        assert!(!passphrase_missing(true, false, &set));
    }
}
