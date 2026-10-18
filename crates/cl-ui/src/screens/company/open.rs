//! Open Company (P08 step 1): the recent list with colour stripes, Open,
//! New company, Switch from QuickBooks Desktop (P31), Restore a backup,
//! Open on the office network (P33), Try the practice company; and, when
//! the engine needs it, first-run setup or sign-in. One company at a time
//! (Q-182); switching happens only here (Q-183). Follows
//! `docs/design/mockups/OpenCompany.html`.

use std::sync::Arc;

use cl_api::LoginOutcome;
use cl_company::{CompanyEntry, CompanyError, RestoreOutcome, RestoreRequest};
use cl_core::Secret;
use cl_core::ids::TenantId;
use cl_host::{AdminAccount, CompanySeed};
use cl_model::{CommandId, Scope};
use cl_settings::Accent;
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke, Ui, Vec2};
use serde_json::{Value, json};

use super::form::{self, DateInput};
use crate::company::{CompanyEvent, CompanyLink, Task, spawn_lifecycle, user_text};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;
use crate::widgets::{eyebrow, kbd, primary_button};
use crate::windows::Screen;

/// Which engine step the window is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Asking the engine whether first-run setup happened.
    Probing,
    /// The engine is not ready yet (status bar says why).
    EngineNotReady,
    /// No owner account yet: show the setup panel.
    FirstRun,
    /// Needs a sign-in.
    SignIn,
    /// Signed in: list and open.
    Ready,
}

/// The Open Company window.
pub struct OpenCompanyScreen {
    link: Arc<CompanyLink>,
    group: Option<cl_api::RequestGroup>,
    stage: Stage,
    companies: Vec<Row>,
    selected: usize,
    list_task: Option<Task<Result<Vec<CompanyEntry>, CompanyError>>>,
    open_task: Option<Task<Result<Opened, CompanyError>>>,
    probe_task: Option<Task<Result<bool, CompanyError>>>,
    signin_task: Option<Task<Result<SignedIn, CompanyError>>>,
    restore_task: Option<Task<Result<RestoreOutcome, CompanyError>>>,
    sample_task: Option<Task<Result<cl_company::Created, CompanyError>>>,
    // First run / sign-in fields.
    name: String,
    email: String,
    password: String,
    confirm: String,
    // Restore panel.
    restore_open: bool,
    restore_path: String,
    restore_passphrase: String,
    restore_result: Option<RestoreOutcome>,
    busy: Option<&'static str>,
    error: Option<CompanyError>,
    notice: Option<String>,
    loaded_once: bool,
    /// A switch refused because of unsaved windows; retried once they are
    /// saved or discarded.
    retry_open: Option<usize>,
}

#[derive(Clone, Debug)]
struct Row {
    tenant_id: TenantId,
    name: String,
    accent: Accent,
    meta: String,
    is_sample: bool,
    closed_through: Option<cl_core::CalendarDate>,
}

/// What opening a company learned (tenant, name, colour, closing date).
type Opened = (TenantId, String, Accent, Option<cl_core::CalendarDate>);

#[derive(Clone, Debug)]
struct SignedIn {
    session: cl_api::Session,
    user: String,
    /// From first-run setup on an engine with U-23.
    recovery_key: Option<Secret>,
}

impl OpenCompanyScreen {
    /// A new window over the link.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>) -> OpenCompanyScreen {
        let group = link.group().and_then(Result::ok);
        let stage = if link.is_demo() || link.session().is_some() {
            Stage::Ready
        } else {
            Stage::Probing
        };
        OpenCompanyScreen {
            link,
            group,
            stage,
            companies: Vec::new(),
            selected: 0,
            list_task: None,
            open_task: None,
            probe_task: None,
            signin_task: None,
            restore_task: None,
            sample_task: None,
            name: String::new(),
            email: String::new(),
            password: String::new(),
            confirm: String::new(),
            restore_open: false,
            restore_path: String::new(),
            restore_passphrase: String::new(),
            restore_result: None,
            busy: None,
            error: None,
            notice: None,
            loaded_once: false,
            retry_open: None,
        }
    }

    /// Open with the restore panel showing (File > Restore a Backup).
    #[must_use]
    pub fn with_restore(mut self) -> OpenCompanyScreen {
        self.restore_open = true;
        self
    }

    fn load_list(&mut self) {
        self.loaded_once = true;
        if let Some(demo) = self.link.demo_state() {
            self.companies = demo
                .companies
                .iter()
                .map(|c| Row {
                    tenant_id: c.tenant_id,
                    name: c.name.clone(),
                    accent: c.accent,
                    meta: c.meta.clone(),
                    is_sample: c.is_sample,
                    closed_through: c.closed_through,
                })
                .collect();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let link = Arc::clone(&self.link);
        let store = link.store().clone();
        self.list_task = spawn_lifecycle(&link, group, move || async move {
            cl_company::list_companies(&session, &store).await
        })
        .ok();
    }

    fn probe(&mut self) {
        let (Some(group), Some(sup)) = (self.group.as_ref(), self.link.supervisor().cloned())
        else {
            return;
        };
        self.probe_task = spawn_lifecycle(&self.link, group, move || async move {
            tokio::task::spawn_blocking(move || match sup.engine_info() {
                Ok(info) => Ok(info.initialized),
                Err(e) => Err(CompanyError::Io {
                    what: "engine info",
                    source: std::io::Error::other(e.to_string()),
                }),
            })
            .await
            .unwrap_or(Err(CompanyError::Metadata))
        })
        .ok();
    }

    fn start_first_run(&mut self) {
        let (Some(group), Some(sup), Some(client)) = (
            self.group.as_ref(),
            self.link.supervisor().cloned(),
            self.link.client().cloned(),
        ) else {
            return;
        };
        if self.password.chars().count() < cl_api::PASSWORD_MIN_CHARS {
            self.error = Some(CompanyError::Setup(vec![]));
            self.notice = Some(t("open.setup.password_short"));
            return;
        }
        if self.password != self.confirm {
            self.notice = Some(t("open.setup.password_mismatch"));
            return;
        }
        let admin = AdminAccount {
            email: self.email.trim().to_string(),
            display_name: self.name.trim().to_string(),
            password: cl_host::Secret::new(self.password.clone()),
        };
        let email = admin.email.clone();
        let password = Secret::new(std::mem::take(&mut self.password));
        self.confirm.clear();
        let display = admin.display_name.clone();
        self.busy = Some("open.setup.working");
        self.signin_task = spawn_lifecycle(&self.link, group, move || async move {
            let seed: Option<CompanySeed> = None;
            let made =
                tokio::task::spawn_blocking(move || sup.initialize_engine(&admin, seed.as_ref()))
                    .await
                    .map_err(|_| CompanyError::Metadata)?
                    .map_err(|e| CompanyError::Io {
                        what: "first-run setup",
                        source: std::io::Error::other(e.to_string()),
                    })?;
            client.desktop_info().await?;
            match client.login(&email, &password).await? {
                LoginOutcome::Session(session) => Ok(SignedIn {
                    session,
                    user: user_text(&display, "owner"),
                    recovery_key: made
                        .recovery_key
                        .map(|k| Secret::new(k.expose().to_string())),
                }),
                LoginOutcome::TfaRequired(_) => Err(CompanyError::Unavailable("tfa")),
            }
        })
        .ok();
    }

    fn start_sign_in(&mut self) {
        let (Some(group), Some(client)) = (self.group.as_ref(), self.link.client().cloned()) else {
            return;
        };
        let email = self.email.trim().to_string();
        let password = Secret::new(std::mem::take(&mut self.password));
        self.busy = Some("open.signin.working");
        self.signin_task = spawn_lifecycle(&self.link, group, move || async move {
            match client.login(&email, &password).await? {
                LoginOutcome::Session(session) => {
                    let display = email.split('@').next().unwrap_or("Owner").to_string();
                    Ok(SignedIn {
                        session,
                        user: user_text(&display, "owner"),
                        recovery_key: None,
                    })
                }
                LoginOutcome::TfaRequired(_) => Err(CompanyError::Unavailable("tfa")),
            }
        })
        .ok();
    }

    fn open_selected(&mut self) {
        let Some(row) = self.companies.get(self.selected).cloned() else {
            return;
        };
        if self.link.switch_blocked() {
            self.notice = Some(t("open.wait_for_saves"));
            return;
        }
        if self.link.dirty_windows() > 0 {
            self.notice = Some(t("open.save_first"));
            self.retry_open = Some(self.selected);
            self.link.push(CompanyEvent::PromptDirty);
            return;
        }
        self.retry_open = None;
        self.notice = None;
        if let Some(mut demo) = self.link.demo_state() {
            demo.open = Some(row.tenant_id);
            drop(demo);
            self.link.push(CompanyEvent::Opened {
                tenant_id: row.tenant_id,
                name: row.name,
                accent: row.accent,
                closed_through: row.closed_through,
            });
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let store = self.link.store().clone();
        let secrets = self.link.secrets();
        self.busy = Some("open.opening");
        self.open_task = spawn_lifecycle(&self.link, group, move || async move {
            cl_company::open_company(&session, &store, row.tenant_id).await?;
            // The session's secrets (SMTP password, the AI key or Ollama
            // address) go to the engine now, from the keychain (Q-P09a).
            let prefs = store
                .load()
                .ok()
                .and_then(|l| l.get(row.tenant_id).map(|r| r.prefs.clone()))
                .unwrap_or_default();
            crate::screens::preferences::ai::push_at_open(&session, secrets.as_ref(), &prefs).await;
            let settings = session.company_settings().await.ok();
            Ok((
                row.tenant_id,
                row.name,
                row.accent,
                settings.and_then(|s| s.lock_date),
            ))
        })
        .ok();
    }

    fn start_restore(&mut self) {
        let path = std::path::PathBuf::from(self.restore_path.trim());
        let passphrase = (!self.restore_passphrase.is_empty())
            .then(|| Secret::new(std::mem::take(&mut self.restore_passphrase)));
        if self.link.is_demo() {
            if self.restore_path.trim().is_empty() {
                self.notice = Some(t("open.restore.need_file"));
                return;
            }
            let id = self.link.demo_state().map_or_else(
                || TenantId::parse("d0000000-0000-4000-8000-0000000000ff").expect("id"),
                |d| d.fresh_id(),
            );
            self.restore_result = Some(RestoreOutcome {
                new_tenant: id,
                name: "Restored company".into(),
                verification: None,
                kept_previous: None,
                file_known: false,
                names_swapped: false,
            });
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let store = self.link.store().clone();
        let secrets = self.link.secrets();
        self.busy = Some("open.restore.working");
        self.restore_task = spawn_lifecycle(&self.link, group, move || async move {
            cl_company::restore(
                &session,
                &store,
                secrets.as_ref(),
                &RestoreRequest { path, passphrase },
                &cl_company::backup::silent(),
            )
            .await
        })
        .ok();
    }

    /// Builds the practice company (live engine) and opens it.
    fn load_sample(&mut self) {
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.notice = Some(t("open.practice_later"));
            return;
        };
        let store = self.link.store().clone();
        self.busy = Some("open.practice_loading");
        self.sample_task = spawn_lifecycle(&self.link, group, move || async move {
            cl_company::sample::load_into(&session, &store, &cl_company::backup::silent()).await
        })
        .ok();
    }

    fn poll(&mut self) {
        if let Some(t) = self.probe_task.as_mut()
            && let Some(r) = t.poll_result()
        {
            self.probe_task = None;
            match r {
                Ok(true) => self.stage = Stage::SignIn,
                Ok(false) => self.stage = Stage::FirstRun,
                Err(_) => self.stage = Stage::EngineNotReady,
            }
        }
        if let Some(t) = self.signin_task.as_mut()
            && let Some(r) = t.poll_result()
        {
            self.signin_task = None;
            self.busy = None;
            match r {
                Ok(s) => {
                    self.link.set_session(Some(s.session));
                    self.link.push(CompanyEvent::SignedIn { user: s.user });
                    if let Some(key) = s.recovery_key {
                        // Shown once by the recovery-key window (Q-210).
                        self.link.stash_recovery_key(key);
                        self.link.push(CompanyEvent::OpenWindow(CommandId::new(
                            super::RECOVERY_KEY,
                        )));
                    }
                    self.stage = Stage::Ready;
                    self.loaded_once = false;
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(t) = self.list_task.as_mut()
            && let Some(r) = t.poll_result()
        {
            self.list_task = None;
            match r {
                Ok(list) => {
                    self.companies = list.into_iter().map(row_from_entry).collect();
                    self.selected = self.selected.min(self.companies.len().saturating_sub(1));
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(t) = self.open_task.as_mut()
            && let Some(r) = t.poll_result()
        {
            self.open_task = None;
            self.busy = None;
            match r {
                Ok((tenant_id, name, accent, closed_through)) => {
                    self.link.push(CompanyEvent::Opened {
                        tenant_id,
                        name,
                        accent,
                        closed_through,
                    });
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(t) = self.sample_task.as_mut()
            && let Some(r) = t.poll_result()
        {
            self.sample_task = None;
            self.busy = None;
            match r {
                Ok(c) => {
                    self.link.push(CompanyEvent::Opened {
                        tenant_id: c.tenant_id,
                        name: cl_company::SAMPLE_NAME.into(),
                        accent: Accent([0xC9, 0xC4, 0xBA]),
                        closed_through: None,
                    });
                    self.loaded_once = false;
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(t) = self.restore_task.as_mut()
            && let Some(r) = t.poll_result()
        {
            self.restore_task = None;
            self.busy = None;
            match r {
                Ok(out) => {
                    self.restore_result = Some(out);
                    self.loaded_once = false;
                }
                Err(e) => self.error = Some(e),
            }
        }
    }

    fn header(ui: &mut Ui, tokens: &Tokens) {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(48.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, CornerRadius::same(10), tokens.accent);
            let inner = rect.shrink(12.0);
            ui.painter().rect_stroke(
                inner,
                CornerRadius::same(2),
                Stroke::new(2.0, tokens.on_accent),
                egui::StrokeKind::Inside,
            );
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(t("open.title"))
                            .size(22.0)
                            .family(crate::fonts::bold())
                            .color(tokens.text_primary),
                    )
                    .selectable(false),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(t("open.subtitle")).color(tokens.text_secondary),
                    )
                    .selectable(false),
                );
            });
        });
    }

    fn company_row(ui: &mut Ui, tokens: &Tokens, row: &Row, selected: bool) -> egui::Response {
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 64.0), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, true, selected, row.name.clone())
        });
        if ui.is_rect_visible(rect) {
            let fill = if selected {
                tokens.accent_tint
            } else {
                tokens.surface_window
            };
            let stroke = if selected || response.hovered() {
                tokens.accent
            } else {
                tokens.border_default
            };
            let p = ui.painter();
            p.rect(
                rect,
                CornerRadius::same(6),
                fill,
                Stroke::new(1.0, stroke),
                egui::StrokeKind::Inside,
            );
            let stripe = egui::Rect::from_min_size(rect.min, Vec2::new(6.0, rect.height()));
            p.rect_filled(
                stripe,
                CornerRadius {
                    nw: 6,
                    sw: 6,
                    ne: 0,
                    se: 0,
                },
                Color32::from_rgb(row.accent.0[0], row.accent.0[1], row.accent.0[2]),
            );
            p.text(
                egui::pos2(rect.left() + 20.0, rect.top() + 14.0),
                egui::Align2::LEFT_TOP,
                &row.name,
                egui::FontId::new(15.0, crate::fonts::bold()),
                tokens.text_primary,
            );
            p.text(
                egui::pos2(rect.left() + 20.0, rect.top() + 38.0),
                egui::Align2::LEFT_TOP,
                &row.meta,
                egui::FontId::proportional(12.0),
                tokens.text_secondary,
            );
            if selected {
                let chip = egui::Rect::from_min_size(
                    egui::pos2(rect.right() - 60.0, rect.center().y - 10.0),
                    Vec2::new(46.0, 20.0),
                );
                let mut chip_ui = ui.new_child(egui::UiBuilder::new().max_rect(chip));
                kbd(&mut chip_ui, "Enter");
            }
        }
        response
    }

    fn side_button(ui: &mut Ui, tokens: &Tokens, title: &str, sub: &str) -> egui::Response {
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 52.0), egui::Sense::click());
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, title.to_string()));
        if ui.is_rect_visible(rect) {
            let stroke = if response.hovered() || response.has_focus() {
                tokens.accent
            } else {
                tokens.border_default
            };
            let p = ui.painter();
            p.rect(
                rect,
                CornerRadius::same(6),
                tokens.surface_window,
                Stroke::new(1.0, stroke),
                egui::StrokeKind::Inside,
            );
            p.circle_stroke(
                egui::pos2(rect.left() + 22.0, rect.center().y),
                7.0,
                Stroke::new(1.8, tokens.accent),
            );
            p.text(
                egui::pos2(rect.left() + 44.0, rect.top() + 9.0),
                egui::Align2::LEFT_TOP,
                title,
                egui::FontId::proportional(13.0),
                tokens.text_primary,
            );
            p.text(
                egui::pos2(rect.left() + 44.0, rect.top() + 28.0),
                egui::Align2::LEFT_TOP,
                sub,
                egui::FontId::proportional(12.0),
                tokens.text_secondary,
            );
        }
        response
    }

    fn setup_panel(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("open.setup.title"));
            form::hint(ui, tokens, &t("open.setup.intro"));
            ui.add_space(6.0);
            form::field(ui, &t("open.setup.name"), &mut self.name);
            form::field(ui, &t("open.setup.email"), &mut self.email);
            form::password(ui, &t("open.setup.password"), &mut self.password);
            form::password(ui, &t("open.setup.confirm"), &mut self.confirm);
            form::hint(ui, tokens, &t("open.setup.password_rule"));
            ui.add_space(6.0);
            if self.busy.is_some() {
                form::progress_line(ui, tokens, &t("open.setup.working"));
            } else if primary_button(ui, &t("open.setup.go")).clicked() {
                self.start_first_run();
            }
        });
    }

    fn signin_panel(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("open.signin.title"));
            form::field(ui, &t("open.setup.email"), &mut self.email);
            let r = form::password(ui, &t("open.setup.password"), &mut self.password);
            ui.add_space(6.0);
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if self.busy.is_some() {
                form::progress_line(ui, tokens, &t("open.signin.working"));
            } else if primary_button(ui, &t("open.signin.go")).clicked() || enter {
                self.start_sign_in();
            }
        });
    }

    fn restore_panel(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("open.restore.title"));
            form::hint(ui, tokens, &t("open.restore.intro"));
            form::field(ui, &t("open.restore.file"), &mut self.restore_path);
            form::password(
                ui,
                &t("open.restore.passphrase"),
                &mut self.restore_passphrase,
            );
            form::hint(ui, tokens, &t("open.restore.passphrase_hint"));
            if let Some(out) = &self.restore_result {
                let verified = out
                    .verification
                    .as_ref()
                    .is_some_and(|v| v.trial_balance_equal == Some(true));
                let text = if verified || self.link.is_demo() {
                    t_with("open.restore.done", &[("name", &out.name)])
                } else {
                    t_with("open.restore.done_unverified", &[("name", &out.name)])
                };
                ui.add(egui::Label::new(RichText::new(text).color(tokens.text_primary)).wrap());
                if out.kept_previous.is_some() {
                    form::hint(ui, tokens, &t("open.restore.kept_previous"));
                }
            }
            ui.add_space(6.0);
            // A restore runs from the workspace only: closing the company
            // first saves or prompts for open forms and waits for writes.
            let company_open = self.link.demo_state().map_or_else(
                || {
                    self.link
                        .session()
                        .is_some_and(|s| s.scope() != cl_api::Scope::Workspace)
                },
                |d| d.open.is_some(),
            );
            if company_open {
                form::hint(ui, tokens, &t("open.restore.close_first"));
            }
            if self.busy == Some("open.restore.working") {
                form::progress_line(ui, tokens, &t("open.restore.working"));
            } else {
                let mut clicked = None;
                ui.add_enabled_ui(!company_open, |ui| {
                    clicked = form::actions(ui, &t("open.restore.go"), &[]);
                });
                if ui.button(t("open.restore.cancel")).clicked() {
                    clicked = Some(1);
                }
                match clicked {
                    Some(0) => self.start_restore(),
                    Some(_) => {
                        self.restore_open = false;
                        self.restore_result = None;
                    }
                    None => {}
                }
            }
        });
    }

    fn handle_keys(&mut self, ui: &mut Ui) {
        if self.restore_open && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.restore_open = false;
            self.restore_result = None;
            return;
        }
        if ui.ctx().egui_wants_keyboard_input() || self.companies.is_empty() {
            return;
        }
        let (down, up, enter) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Enter),
            )
        });
        if down {
            self.selected = (self.selected + 1).min(self.companies.len() - 1);
        }
        if up {
            self.selected = self.selected.saturating_sub(1);
        }
        if enter && self.stage == Stage::Ready {
            self.open_selected();
        }
    }
}

fn row_from_entry(e: CompanyEntry) -> Row {
    let mut parts: Vec<String> = Vec::new();
    if let Some(opened) = e.last_opened {
        parts.push(t_with("open.meta.opened", &[("when", &when_text(opened))]));
    }
    if let Some(b) = e.last_backup {
        parts.push(t_with("open.meta.backed_up", &[("when", &when_text(b))]));
    } else {
        parts.push(t("open.meta.never_backed_up"));
    }
    if let Some(n) = e.transaction_count {
        parts.push(t_with("open.meta.transactions", &[("n", &group_digits(n))]));
    }
    if e.is_sample {
        parts = vec![t("open.meta.sample")];
    }
    Row {
        tenant_id: e.tenant_id,
        name: e.name,
        accent: e.accent,
        meta: parts.join(" \u{b7} "),
        is_sample: e.is_sample,
        closed_through: None,
    }
}

fn when_text(ts: cl_core::Timestamp) -> String {
    use chrono::Datelike as _;
    let d = ts.as_utc();
    format!("{:02}/{:02}/{}", d.month(), d.day(), d.year())
}

fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl Screen for OpenCompanyScreen {
    fn title(&self) -> String {
        t("open.tab")
    }

    fn scope(&self) -> Scope {
        Scope::Window
    }

    fn ui(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        self.poll();
        if self.stage == Stage::Probing && self.probe_task.is_none() {
            self.probe();
        }
        if self.stage == Stage::Ready && !self.loaded_once && self.list_task.is_none() {
            self.load_list();
        }
        if self.stage == Stage::Ready {
            self.handle_keys(ui);
            if let Some(i) = self.retry_open
                && self.link.dirty_windows() == 0
                && self.busy.is_none()
            {
                self.retry_open = None;
                self.selected = i.min(self.companies.len().saturating_sub(1));
                self.open_selected();
            }
        }
        let max = ui.available_width().min(980.0);
        let offset = ((ui.available_width() - max) / 2.0).max(0.0);
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_space(offset);
            ui.vertical(|ui| {
                ui.set_max_width(max);
                egui::Frame::new()
                    .fill(tokens.surface_window)
                    .stroke(Stroke::new(1.0, tokens.border_strong))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(32, 24))
                    .show(ui, |ui| {
                        Self::header(ui, &tokens);
                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(12.0);
                        if let Some(e) = &self.error {
                            form::inline_error(ui, &tokens, e);
                            ui.add_space(8.0);
                        }
                        if let Some(n) = &self.notice {
                            form::hint(ui, &tokens, n);
                            ui.add_space(8.0);
                        }
                        match self.stage {
                            Stage::Probing => form::progress_line(ui, &tokens, &t("open.probing")),
                            Stage::EngineNotReady => {
                                form::hint(ui, &tokens, &t("open.engine_not_ready"));
                            }
                            Stage::FirstRun => self.setup_panel(ui, &tokens),
                            Stage::SignIn => self.signin_panel(ui, &tokens),
                            Stage::Ready => {}
                        }
                        if self.stage == Stage::Ready {
                            self.body(ui, &tokens);
                        }
                        ui.add_space(14.0);
                        ui.separator();
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(t("open.footer.offline"))
                                    .size(12.0)
                                    .color(tokens.text_secondary),
                            );
                            ui.label(
                                RichText::new(t("open.footer.not_affiliated"))
                                    .size(12.0)
                                    .color(tokens.text_secondary),
                            );
                        });
                    });
            });
        });
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            "file.restore" => {
                self.restore_open = true;
                true
            }
            _ => false,
        }
    }

    fn state(&self) -> Value {
        json!({
            "screen": "open_company",
            "stage": format!("{:?}", self.stage).to_ascii_lowercase(),
            "companies": self.companies.len(),
            "selected": self.selected,
            "restore_open": self.restore_open,
            "restored": self.restore_result.is_some(),
            "busy": self.busy.is_some(),
            "error": self.error.is_some(),
        })
    }
}

impl OpenCompanyScreen {
    fn body(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let narrow = ui.available_width() < 760.0;
        let mut open_clicked = false;
        let mut action: Option<CommandId> = None;
        let mut selected_now: Option<usize> = None;
        let mut practice = false;
        let mut left = |this: &mut Self, ui: &mut Ui| {
            eyebrow(ui, &t("open.recent"));
            ui.add_space(6.0);
            if this.companies.is_empty() && this.list_task.is_none() {
                form::hint(ui, tokens, &t("open.empty"));
            }
            for (i, row) in this.companies.iter().enumerate() {
                let r = Self::company_row(ui, tokens, row, i == this.selected);
                if r.clicked() {
                    selected_now = Some(i);
                }
                if r.double_clicked() {
                    selected_now = Some(i);
                    open_clicked = true;
                }
                ui.add_space(6.0);
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if this.busy == Some("open.opening") {
                    form::progress_line(ui, tokens, &t("open.opening"));
                } else if this.busy == Some("open.practice_loading") {
                    form::progress_line(ui, tokens, &t("open.practice_loading"));
                } else if primary_button(ui, &t("open.open")).clicked() {
                    open_clicked = true;
                }
                if ui
                    .add(egui::Button::new(t("open.browse")).min_size(Vec2::new(0.0, 44.0)))
                    .clicked()
                {
                    this.restore_open = true;
                }
            });
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    RichText::new(t_with(
                        "open.saved_in",
                        &[("folder", &this.link.backup_dir.display().to_string())],
                    ))
                    .size(12.0)
                    .color(tokens.text_tertiary),
                )
                .wrap(),
            );
        };
        let right = |ui: &mut Ui,
                     action: &mut Option<CommandId>,
                     practice: &mut bool,
                     restore: &mut bool| {
            eyebrow(ui, &t("open.start"));
            ui.add_space(6.0);
            if Self::side_button(ui, tokens, &t("open.new"), &t("open.new_sub")).clicked() {
                *action = Some(CommandId::new("file.new_company"));
            }
            ui.add_space(6.0);
            if Self::side_button(ui, tokens, &t("open.qbd"), &t("open.qbd_sub")).clicked() {
                *action = Some(CommandId::new("banking.import_file"));
            }
            ui.add_space(6.0);
            if Self::side_button(ui, tokens, &t("open.restore"), &t("open.restore_sub")).clicked() {
                *restore = true;
            }
            ui.add_space(6.0);
            if Self::side_button(ui, tokens, &t("open.lan"), &t("open.lan_sub")).clicked() {
                *action = Some(CommandId::new("help.current"));
            }
            ui.add_space(6.0);
            if Self::side_button(ui, tokens, &t("open.practice"), &t("open.practice_sub")).clicked()
            {
                *practice = true;
            }
        };
        let mut restore = self.restore_open;
        if narrow {
            left(self, ui);
            ui.add_space(14.0);
            right(ui, &mut action, &mut practice, &mut restore);
        } else {
            let total = ui.available_width();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(total * 0.58);
                    left(self, ui);
                });
                ui.add_space(28.0);
                ui.vertical(|ui| {
                    ui.set_width(total * 0.38);
                    right(ui, &mut action, &mut practice, &mut restore);
                });
            });
        }
        self.restore_open = restore;
        if let Some(i) = selected_now {
            self.selected = i;
        }
        if open_clicked {
            self.open_selected();
        }
        if practice {
            if let Some(i) = self.companies.iter().position(|c| c.is_sample) {
                self.selected = i;
                self.open_selected();
            } else {
                self.load_sample();
            }
        }
        if let Some(id) = action {
            self.link.push(CompanyEvent::OpenWindow(id));
        }
        if self.restore_open {
            ui.add_space(14.0);
            self.restore_panel(ui, tokens);
        }
        let _ = DateInput::new; // date helpers are shared with the other screens
    }
}
