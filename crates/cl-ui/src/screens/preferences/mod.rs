//! Preferences (P09): one window, a category list on the left ("Just for
//! me" / "This company") and one panel on the right, Cancel and Save at
//! the bottom, as in `docs/design/mockups/Preferences.html`.
//!
//! Where each setting lives (Q-P09c):
//! - *Just for me* → `cl_settings::UserPrefs` (`ui-prefs.json`), handed
//!   back to the shell through [`CompanyEvent::UserPrefs`];
//! - *This company*, engine-owned → `/company/settings` (basis, fiscal
//!   year, default sales tax rate), `/ai/admin/config` and the consent
//!   routes, `/company/smtp`;
//! - *This company*, local → `cl_settings::CompanyPrefs` on the company
//!   record (receipt inbox, bills-due days, AI switches and cap, consent
//!   note, SMTP security) and the setup flags (Q-P08c);
//! - secrets (AI keys, SMTP password) → the OS keychain only, pushed to
//!   the engine per session once `session-secrets-v1` exists (Q-P09a).
//!
//! Every engine call runs on the request queue and is polled each frame.

pub mod ai;
pub mod company;
pub mod email;
pub mod folders;
pub mod user;

use std::sync::Arc;

use cl_api::{AiConfig, AiUsage, CompanyConsent, CompanySettings, SmtpSettings};
use cl_company::CompanyError;
use cl_core::ids::TenantId;
use cl_model::{CommandId, Scope};
use cl_settings::{CompanyFlags, CompanyPrefs, UserPrefs};
use eframe::egui::{self, RichText, Ui, Vec2};
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink, Task, spawn_lifecycle};
use crate::i18n::t;
use crate::screens::company::form;
use crate::theme::Tokens;
use crate::widgets::{eyebrow, primary_button};
use crate::windows::Screen;

/// The command that opens the window.
pub const COMMAND: &str = "edit.preferences";

/// Which side of the list a category sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// Per user, this computer.
    Me,
    /// The open company.
    Company,
}

/// The categories of the left list, in mockup order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    /// Theme and zoom.
    LookAndFeel,
    /// Date format, negatives, date warnings.
    DatesNumbers,
    /// Bills-due window, backup reminder.
    Reminders,
    /// Basis, fiscal year, closing date (shown).
    Accounting,
    /// Check options and form defaults (later phases).
    ChecksForms,
    /// Sales tax on/off and default rate.
    SalesTax,
    /// Inventory on/off.
    Inventory,
    /// The AI panel.
    Ai,
    /// SMTP.
    Email,
    /// Receipt inbox folder.
    ReceiptInbox,
    /// Backup folder and backup on close.
    Backups,
    /// Update checks.
    Updates,
}

impl Category {
    /// Every category in list order.
    pub const ALL: [Category; 12] = [
        Category::LookAndFeel,
        Category::DatesNumbers,
        Category::Reminders,
        Category::Accounting,
        Category::ChecksForms,
        Category::SalesTax,
        Category::Inventory,
        Category::Ai,
        Category::Email,
        Category::ReceiptInbox,
        Category::Backups,
        Category::Updates,
    ];

    /// Which group it belongs to.
    #[must_use]
    pub fn group(self) -> Group {
        match self {
            Category::LookAndFeel | Category::DatesNumbers | Category::Reminders => Group::Me,
            _ => Group::Company,
        }
    }

    /// i18n key of the list label.
    #[must_use]
    pub fn label_key(self) -> &'static str {
        match self {
            Category::LookAndFeel => "prefs.cat.look",
            Category::DatesNumbers => "prefs.cat.dates",
            Category::Reminders => "prefs.cat.reminders",
            Category::Accounting => "prefs.cat.accounting",
            Category::ChecksForms => "prefs.cat.checks",
            Category::SalesTax => "prefs.cat.sales_tax",
            Category::Inventory => "prefs.cat.inventory",
            Category::Ai => "prefs.cat.ai",
            Category::Email => "prefs.cat.email",
            Category::ReceiptInbox => "prefs.cat.inbox",
            Category::Backups => "prefs.cat.backups",
            Category::Updates => "prefs.cat.updates",
        }
    }

    /// Short name for the control channel.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Category::LookAndFeel => "look",
            Category::DatesNumbers => "dates",
            Category::Reminders => "reminders",
            Category::Accounting => "accounting",
            Category::ChecksForms => "checks",
            Category::SalesTax => "sales_tax",
            Category::Inventory => "inventory",
            Category::Ai => "ai",
            Category::Email => "email",
            Category::ReceiptInbox => "inbox",
            Category::Backups => "backups",
            Category::Updates => "updates",
        }
    }

    /// Words the search box matches besides the label.
    fn search_terms(self) -> &'static str {
        match self {
            Category::LookAndFeel => "theme dark light zoom colour color",
            Category::DatesNumbers => "date format negative parentheses warning",
            Category::Reminders => "bills due backup remind",
            Category::Accounting => "basis cash accrual fiscal year closing date",
            Category::ChecksForms => "check number print form",
            Category::SalesTax => "tax rate",
            Category::Inventory => "stock items cost",
            Category::Ai => "ai anthropic openai ollama key suggestions cap usage",
            Category::Email => "smtp email send password server",
            Category::ReceiptInbox => "receipt folder photo pdf inbox",
            Category::Backups => "backup folder close",
            Category::Updates => "update version check",
        }
    }

    /// True when the label or its terms contain `query` (case-insensitive;
    /// an empty query matches everything).
    #[must_use]
    pub fn matches(self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        q.is_empty()
            || t(self.label_key()).to_lowercase().contains(&q)
            || self.search_terms().contains(&q)
    }
}

/// What the engine reported when the window opened (each part `None`
/// when that read failed or there is no engine).
#[derive(Default)]
pub struct Loaded {
    /// `/company/settings`.
    pub settings: Option<CompanySettings>,
    /// `/ai/admin/config`.
    pub ai: Option<AiConfig>,
    /// `/ai/consent/{company}/disclosure`.
    pub consent: Option<CompanyConsent>,
    /// `/company/smtp`.
    pub smtp: Option<SmtpSettings>,
    /// `/ai/usage?months=1`.
    pub usage: Option<AiUsage>,
    /// The reads finished (with or without answers).
    pub done: bool,
}

impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loaded")
            .field("settings", &self.settings.is_some())
            .field("ai", &self.ai.is_some())
            .field("consent", &self.consent.is_some())
            .field("smtp", &self.smtp.is_some())
            .field("usage", &self.usage.is_some())
            .field("done", &self.done)
            .finish()
    }
}

/// Everything the owner can change. Compared whole for the dirty dot.
#[derive(Clone, PartialEq, Eq)]
pub struct Edits {
    /// Just for me.
    pub user: UserPrefs,
    /// Date-warning windows and the bills-due window as typed.
    pub past_days: String,
    /// See `past_days`.
    pub future_days: String,
    /// See `past_days`.
    pub bills_due_days: String,
    /// Company-local preferences.
    pub company: CompanyPrefs,
    /// Setup flags (Q-P08c).
    pub flags: CompanyFlags,
    /// Back up when the company closes (app-wide).
    pub backup_on_close: bool,
    /// Default backup folder.
    pub backup_folder: String,
    /// Engine company settings.
    pub accounting: company::AccountingEdits,
    /// The AI panel.
    pub ai: ai::AiEdits,
    /// The Email panel.
    pub email: email::EmailEdits,
}

impl std::fmt::Debug for Edits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Edits").finish_non_exhaustive()
    }
}

/// Why the edits cannot be saved yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveBlock {
    /// A number field is not a number in range.
    BadNumber(&'static str),
    /// The SMTP form has a bad field.
    Smtp(cl_settings::SmtpFieldError),
    /// A cloud provider needs its key first.
    AiNeedsKey,
    /// A provider needs the owner's agreement to the notice.
    AiNeedsConsent,
    /// AI was already on: the changed panel needs the notice ticked again.
    AiNeedsConsentAgain,
    /// The tax rate is not a rate.
    BadRate,
    /// The Ollama address is not local.
    OllamaUrl(cl_settings::OllamaUrlError),
    /// The receipt-inbox folder is not usable.
    Folder(cl_settings::FolderError),
    /// The engine's settings have not been read yet.
    NotLoaded,
}

impl SaveBlock {
    /// i18n key of the owner message.
    #[must_use]
    pub fn message_key(&self) -> &'static str {
        match self {
            SaveBlock::BadNumber(_) => "prefs.err.number",
            SaveBlock::Smtp(e) => e.message_key(),
            SaveBlock::AiNeedsKey => "prefs.ai.needs_key",
            SaveBlock::AiNeedsConsent => "prefs.ai.needs_consent",
            SaveBlock::AiNeedsConsentAgain => "prefs.ai.needs_consent_again",
            SaveBlock::BadRate => "prefs.err.rate",
            SaveBlock::OllamaUrl(_) => "prefs.ai.err_ollama_url",
            SaveBlock::Folder(e) => folders::folder_error_key(*e),
            SaveBlock::NotLoaded => "prefs.err.not_loaded",
        }
    }
}

/// What a save did on the engine side.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SaveOutcome {
    /// Cloud keys stayed in the keychain because the engine has no
    /// session-secrets support yet.
    pub ai_waiting_engine: bool,
    /// The monthly cap is reached: the online provider stays off.
    pub cap_reached: bool,
    /// The engine recorded the company's consent in this save.
    pub consent: Option<ai::ConsentRecorded>,
}

/// The Preferences window.
pub struct PreferencesScreen {
    link: Arc<CompanyLink>,
    group: Option<cl_api::RequestGroup>,
    tenant: Option<(TenantId, String)>,
    category: Category,
    search: String,
    edits: Edits,
    committed: Edits,
    loaded: Loaded,
    load_task: Option<Task<Result<Loaded, CompanyError>>>,
    save_task: Option<Task<Result<SaveOutcome, CompanyError>>>,
    /// The edits a running save will commit when the engine answers.
    pending: Option<Edits>,
    test_task: Option<Task<Result<String, CompanyError>>>,
    /// Last provider-test result, shown next to the Test button.
    ai_test_status: Option<String>,
    /// A session-secret delete running after "Remove key".
    forget_task: Option<Task<Result<(), CompanyError>>>,
    /// Which cloud keys and the SMTP password are in the keychain.
    pub(crate) secrets_saved: ai::SecretsSaved,
    block: Option<SaveBlock>,
    error: Option<CompanyError>,
    status: Option<String>,
    close_after_save: bool,
}

impl PreferencesScreen {
    /// A window over the open company (or no company: only "Just for me").
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        user: UserPrefs,
        category: Category,
    ) -> PreferencesScreen {
        let group = link.group().and_then(Result::ok);
        let secrets_saved = ai::SecretsSaved::read(link.secrets().as_ref());
        let list = link.store().load().unwrap_or_default();
        let (company_prefs, flags) = tenant
            .as_ref()
            .and_then(|(id, _)| list.get(*id))
            .map(|rec| (rec.prefs.clone(), rec.flags))
            .unwrap_or_default();
        let edits = Edits {
            past_days: user.date_warnings.past_days.to_string(),
            future_days: user.date_warnings.future_days.to_string(),
            bills_due_days: company_prefs.bills_due_days.to_string(),
            user,
            ai: ai::AiEdits::from_local(&company_prefs),
            company: company_prefs,
            flags,
            backup_on_close: list.backup_on_close,
            backup_folder: list
                .backup_folder
                .clone()
                .unwrap_or_else(|| link.backup_dir.clone())
                .display()
                .to_string(),
            accounting: company::AccountingEdits::default(),
            email: email::EmailEdits::default(),
        };
        let mut screen = PreferencesScreen {
            link,
            group,
            tenant,
            category,
            search: String::new(),
            committed: edits.clone(),
            edits,
            loaded: Loaded::default(),
            load_task: None,
            save_task: None,
            pending: None,
            test_task: None,
            ai_test_status: None,
            forget_task: None,
            secrets_saved,
            block: None,
            error: None,
            status: None,
            close_after_save: false,
        };
        screen.start_load();
        screen
    }

    fn start_load(&mut self) {
        if self.tenant.is_none() {
            self.loaded.done = true;
            return;
        }
        if self.link.is_demo() {
            self.apply_loaded(crate::company::demo::preferences_loaded());
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.loaded.done = true;
            return;
        };
        let pending = group.read(&session, |s| async move {
            let mut l = Loaded {
                settings: s.company_settings().await.ok(),
                ai: s.ai_config().await.ok(),
                consent: s.company_ai_disclosure().await.ok(),
                smtp: s.smtp_settings().await.ok(),
                usage: s.ai_usage(1).await.ok(),
                done: true,
            };
            l.done = true;
            Ok(Ok::<Loaded, CompanyError>(l))
        });
        match pending {
            Ok(p) => self.load_task = Some(Task::Pending(p)),
            Err(_) => self.loaded.done = true,
        }
    }

    /// Overlays what the engine reported. With unsaved edits (a re-read
    /// after a failed save), only the committed baseline is refreshed and
    /// the edits are left as the owner typed them (S2).
    fn apply_loaded(&mut self, loaded: Loaded) {
        let dirty = self.edits != self.committed;
        let mut base = if dirty {
            self.committed.clone()
        } else {
            self.edits.clone()
        };
        base.accounting = company::AccountingEdits::from_settings(loaded.settings.as_ref());
        base.ai.apply_engine(
            loaded.ai.as_ref(),
            loaded.consent.as_ref(),
            base.company.ai_consent.as_ref(),
        );
        base.email
            .apply_engine(loaded.smtp.as_ref(), base.company.smtp_security);
        self.committed = base;
        if !dirty {
            self.edits = self.committed.clone();
        }
        self.loaded = loaded;
        self.loaded.done = true;
    }

    /// The edits as the control channel and tests see them.
    #[must_use]
    pub fn edits(&self) -> &Edits {
        &self.edits
    }

    /// Mutable edits (tests).
    pub fn edits_mut(&mut self) -> &mut Edits {
        &mut self.edits
    }

    /// Switch the panel.
    pub fn select(&mut self, category: Category) {
        self.category = category;
    }

    /// Parses the typed numbers into the preference structs.
    ///
    /// # Errors
    /// The first field that is wrong.
    pub fn validate(&self) -> Result<Edits, SaveBlock> {
        let mut e = self.edits.clone();
        let num = |text: &str, name: &'static str, max: u16| -> Result<u16, SaveBlock> {
            text.trim()
                .parse::<u16>()
                .ok()
                .filter(|n| *n <= max)
                .ok_or(SaveBlock::BadNumber(name))
        };
        e.user.date_warnings.past_days = num(
            &e.past_days,
            "past_days",
            cl_settings::user_prefs::DATE_WARNING_MAX_DAYS,
        )?;
        e.user.date_warnings.future_days = num(
            &e.future_days,
            "future_days",
            cl_settings::user_prefs::DATE_WARNING_MAX_DAYS,
        )?;
        let bills = num(
            &e.bills_due_days,
            "bills_due_days",
            cl_settings::company_prefs::BILLS_DUE_DAYS_MAX,
        )?;
        if bills == 0 {
            return Err(SaveBlock::BadNumber("bills_due_days"));
        }
        e.company.bills_due_days = bills;
        e.company.normalize();
        e.user.normalize();
        if let Some(p) = e
            .company
            .receipt_inbox_dir
            .as_ref()
            .filter(|p| !p.as_os_str().is_empty())
        {
            cl_settings::check_inbox_folder(p).map_err(SaveBlock::Folder)?;
        }
        e.accounting.validate()?;
        // A save that leaves the AI panel alone is never blocked by a
        // stale or unknown consent read; a changed panel is checked, and
        // the message says so when AI was already on.
        if e.ai != self.committed.ai {
            match e.ai.validate(&self.secrets_saved) {
                Err(SaveBlock::AiNeedsConsent) if self.committed.ai.enabled => {
                    return Err(SaveBlock::AiNeedsConsentAgain);
                }
                other => other?,
            }
        }
        e.email.validate()?;
        Ok(e)
    }

    /// Saves everything: local stores and keychain now, engine writes on
    /// the queue. Returns false (and shows why) when a field is wrong.
    pub fn save(&mut self) -> bool {
        self.block = None;
        self.error = None;
        self.status = None;
        if !self.loaded.done || self.save_task.is_some() {
            self.block = Some(SaveBlock::NotLoaded);
            return false;
        }
        let edits = match self.validate() {
            Ok(e) => e,
            Err(b) => {
                self.block = Some(b);
                return false;
            }
        };
        // Keys and the SMTP password: keychain only (Q-P09a).
        let secrets = self.link.secrets();
        if let Err(e) = edits
            .ai
            .store_secrets(secrets.as_ref(), &mut self.secrets_saved)
        {
            self.error = Some(e.into());
            return false;
        }
        if let Err(e) = edits
            .email
            .store_password(secrets.as_ref(), &mut self.secrets_saved)
        {
            self.error = Some(e.into());
            return false;
        }
        // Just for me.
        if edits.user != self.committed.user {
            self.link.push(CompanyEvent::UserPrefs(edits.user));
        }
        // Company-local and app-wide local settings.
        let mut company_prefs = edits.company.clone();
        edits.ai.write_local(&mut company_prefs);
        company_prefs.smtp_security = edits.email.form.security;
        if let Some((id, _)) = self.tenant {
            let flags = edits.flags;
            let prefs = company_prefs.clone();
            if let Err(e) = self.link.store().update(|l| {
                if let Some(rec) = l.get_mut(id) {
                    rec.prefs = prefs;
                    rec.flags = flags;
                }
            }) {
                self.error = Some(e.into());
                return false;
            }
        }
        let folder = std::path::PathBuf::from(edits.backup_folder.trim());
        let on_close = edits.backup_on_close;
        if let Err(e) = self.link.store().update(|l| {
            l.backup_on_close = on_close;
            l.backup_folder = (!folder.as_os_str().is_empty()).then(|| folder.clone());
        }) {
            self.error = Some(e.into());
            return false;
        }
        // Engine writes.
        let plan = ai::AiSavePlan::build(
            &edits.ai,
            &self.secrets_saved,
            self.loaded.ai.as_ref(),
            self.loaded.consent.as_ref(),
            &self.link.capabilities(),
            company_prefs.ai.check(&ai::this_month()),
        );
        let settings = edits.accounting.update_for(self.loaded.settings.as_ref());
        let smtp = edits.email.update_for(self.loaded.smtp.as_ref());
        let mut after = edits.clone();
        after.ai.clear_typed();
        after.email.clear_typed();
        after.company = company_prefs;
        if self.link.is_demo() || self.tenant.is_none() {
            // No engine: the consent is "recorded" with the version shown.
            let consent = plan.accept_company_consent.then(|| ai::ConsentRecorded {
                provider: plan.provider,
                version: self
                    .loaded
                    .consent
                    .as_ref()
                    .and_then(|c| c.system_version)
                    .unwrap_or(1),
                at: Some(cl_settings::recent::now()),
            });
            self.finish_save(
                &SaveOutcome {
                    ai_waiting_engine: plan.waiting_engine,
                    cap_reached: plan.cap_reached,
                    consent,
                },
                after,
            );
            return true;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.finish_save(&SaveOutcome::default(), after);
            return true;
        };
        let secrets_for_engine = edits.ai.session_secrets(secrets.as_ref(), &edits.email);
        // The edits stay dirty until the engine answers (S2).
        self.pending = Some(after);
        self.save_task = spawn_lifecycle(&self.link, group, move || async move {
            if let Some(u) = settings {
                session
                    .update_company_settings(&u, cl_api::IdempotencyKey::new())
                    .await?;
            }
            if let Some(u) = smtp {
                session
                    .update_smtp_settings(&u, cl_api::IdempotencyKey::new())
                    .await?;
            }
            plan.run(&session, secrets_for_engine).await
        })
        .ok();
        true
    }

    fn finish_save(&mut self, outcome: &SaveOutcome, after: Edits) {
        self.committed = after;
        self.edits = self.committed.clone();
        // The consent note is written only once the engine recorded the
        // accept, with the version it reports (S4).
        if let Some(c) = &outcome.consent {
            let note = cl_settings::AiConsentNote {
                provider: c.provider,
                disclosure_version: c.version,
                accepted_at: c.at.or_else(|| Some(cl_settings::recent::now())),
            };
            self.committed.company.ai_consent = Some(note.clone());
            self.edits.company.ai_consent = Some(note.clone());
            if let Some((id, _)) = self.tenant {
                let _ = self.link.store().update(|l| {
                    if let Some(rec) = l.get_mut(id) {
                        rec.prefs.ai_consent = Some(note);
                    }
                });
            }
        }
        self.status = Some(if outcome.cap_reached {
            t("prefs.saved_cap")
        } else if outcome.ai_waiting_engine {
            t("prefs.saved_waiting")
        } else {
            t("prefs.saved")
        });
        self.link.push(CompanyEvent::Toast("prefs.toast"));
        if self.close_after_save {
            self.close_after_save = false;
            self.link
                .push(CompanyEvent::CloseWindow(CommandId::new(COMMAND)));
        }
    }

    /// Throws the edits away.
    pub fn revert(&mut self) -> bool {
        if self.edits == self.committed {
            return false;
        }
        self.edits = self.committed.clone();
        self.block = None;
        true
    }

    fn poll(&mut self) {
        if let Some(task) = self.load_task.as_mut()
            && let Some(r) = task.poll_result()
        {
            self.load_task = None;
            match r {
                Ok(l) => self.apply_loaded(l),
                Err(e) => {
                    self.loaded.done = true;
                    self.error = Some(e);
                }
            }
        }
        if let Some(task) = self.save_task.as_mut()
            && let Some(r) = task.poll_result()
        {
            self.save_task = None;
            let after = self.pending.take();
            match (r, after) {
                (Ok(o), Some(after)) => self.finish_save(&o, after),
                (Ok(_), None) => {}
                (Err(e), _) => {
                    // The edits stay dirty; the engine's view is re-read
                    // so the next save starts from what it holds (S2).
                    self.error = Some(e);
                    self.start_load();
                }
            }
        }
        if let Some(task) = self.test_task.as_mut()
            && let Some(r) = task.poll_result()
        {
            self.test_task = None;
            match r {
                Ok(text) => {
                    if self.category == Category::Ai {
                        self.ai_test_status = Some(text);
                    } else {
                        self.status = Some(text);
                    }
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(task) = self.forget_task.as_mut()
            && let Some(r) = task.poll_result()
        {
            self.forget_task = None;
            if let Err(e) = r {
                self.error = Some(e);
            }
        }
    }

    fn nav(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let has_company = self.tenant.is_some();
        let mut last_group = None;
        for c in Category::ALL {
            if !c.matches(&self.search) {
                continue;
            }
            if c.group() == Group::Company && !has_company {
                continue;
            }
            if last_group != Some(c.group()) {
                last_group = Some(c.group());
                ui.add_space(6.0);
                eyebrow(
                    ui,
                    &t(match c.group() {
                        Group::Me => "prefs.group.me",
                        Group::Company => "prefs.group.company",
                    }),
                );
            }
            let selected = self.category == c;
            let label = t(c.label_key());
            let text = if selected {
                RichText::new(&label).color(tokens.on_accent)
            } else {
                RichText::new(&label).color(tokens.text_primary)
            };
            let r = ui.add_sized(
                [ui.available_width(), 30.0],
                egui::Button::new(text)
                    .fill(if selected {
                        tokens.accent
                    } else {
                        egui::Color32::TRANSPARENT
                    })
                    .stroke(egui::Stroke::NONE),
            );
            if r.clicked() {
                self.category = c;
            }
        }
    }

    fn panel(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let caps = self.link.capabilities();
        let demo = self.link.is_demo();
        match self.category {
            Category::LookAndFeel => user::look_and_feel(ui, tokens, &mut self.edits),
            Category::DatesNumbers => user::dates_numbers(ui, tokens, &mut self.edits),
            Category::Reminders => user::reminders(ui, tokens, &mut self.edits),
            Category::Accounting => company::accounting(ui, tokens, &mut self.edits, &self.loaded),
            Category::ChecksForms => company::checks_forms(ui, tokens),
            Category::SalesTax => company::sales_tax(ui, tokens, &mut self.edits),
            Category::Inventory => company::inventory(ui, tokens, &mut self.edits),
            Category::Ai => {
                let action = ai::panel(
                    ui,
                    tokens,
                    &mut self.edits,
                    &self.loaded,
                    &self.secrets_saved,
                    &caps,
                    self.test_task.is_some(),
                    self.ai_test_status.as_deref(),
                );
                self.handle_ai_action(action);
            }
            Category::Email => {
                let action = email::panel(
                    ui,
                    tokens,
                    &mut self.edits,
                    &self.secrets_saved,
                    self.test_task.is_some(),
                    demo,
                );
                self.handle_email_action(action);
            }
            Category::ReceiptInbox => folders::receipt_inbox(ui, tokens, &mut self.edits),
            Category::Backups => user::backups(ui, tokens, &mut self.edits),
            Category::Updates => user::updates(ui, tokens, &mut self.edits),
        }
    }

    fn handle_ai_action(&mut self, action: ai::PanelAction) {
        match action {
            ai::PanelAction::None => {}
            ai::PanelAction::RemoveKey(provider) => {
                if let Some(name) = provider.key_name() {
                    match self.link.secrets().delete(name) {
                        Ok(()) => self.secrets_saved.set_key(provider, false),
                        Err(e) => self.error = Some(e.into()),
                    }
                }
                // The engine forgets it for this session too (S3).
                self.forget_slot(match provider {
                    cl_settings::AiProvider::Anthropic => cl_api::SecretSlot::AiAnthropic,
                    cl_settings::AiProvider::OpenAi => cl_api::SecretSlot::AiOpenai,
                    cl_settings::AiProvider::Ollama => cl_api::SecretSlot::AiOllamaBaseUrl,
                });
            }
            ai::PanelAction::Test => self.test_ai(),
        }
    }

    /// Asks the engine to forget one session secret, when it holds them.
    fn forget_slot(&mut self, slot: cl_api::SecretSlot) {
        if !self
            .link
            .capabilities()
            .has(cl_api::Capabilities::SESSION_SECRETS)
        {
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        self.forget_task = spawn_lifecycle(&self.link, group, move || async move {
            match session.delete_session_secret(slot).await {
                Ok(()) | Err(cl_api::ApiError::NotFound { .. }) => Ok(()),
                Err(e) => Err(e.into()),
            }
        })
        .ok();
    }

    fn test_ai(&mut self) {
        self.error = None;
        self.block = None;
        self.ai_test_status = None;
        let provider = self.edits.ai.provider;
        let update = match self.edits.ai.config_for_test() {
            Ok(u) => u,
            Err(b) => {
                self.block = Some(b);
                return;
            }
        };
        if self.link.is_demo() {
            self.ai_test_status = Some(
                if provider.is_cloud() && !self.secrets_saved.key(provider) {
                    t("prefs.ai.test_needs_key")
                } else {
                    t("prefs.ai.test_ok")
                },
            );
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        self.test_task = spawn_lifecycle(&self.link, group, move || async move {
            if let Some(u) = update {
                session
                    .update_ai_config(&u, cl_api::IdempotencyKey::new())
                    .await?;
            }
            let r = session.test_ai_provider(provider.wire()).await?;
            Ok(if r.success {
                t("prefs.ai.test_ok")
            } else {
                // The engine's reason, once scrubbed of anything personal.
                match r.error.as_deref().map(crate::errors::scrub_line) {
                    Some(reason) if !reason.is_empty() => {
                        format!("{} ({reason})", t("prefs.ai.test_failed"))
                    }
                    _ => t("prefs.ai.test_failed"),
                }
            })
        })
        .ok();
    }

    fn handle_email_action(&mut self, action: email::PanelAction) {
        match action {
            email::PanelAction::None => {}
            email::PanelAction::RemovePassword => {
                match self.link.secrets().delete(cl_settings::smtp::SMTP_PASSWORD) {
                    Ok(()) => self.secrets_saved.smtp_password = false,
                    Err(e) => self.error = Some(e.into()),
                }
            }
            email::PanelAction::Test => self.test_email(),
        }
    }

    fn test_email(&mut self) {
        self.error = None;
        self.status = None;
        if let Err(e) = self.edits.email.form.validate() {
            self.block = Some(SaveBlock::Smtp(e));
            return;
        }
        if self.link.is_demo() {
            self.status = Some(t("prefs.email.test_ok"));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let Some(test) = self.edits.email.test_request(self.link.secrets().as_ref()) else {
            self.block = Some(SaveBlock::Smtp(cl_settings::SmtpFieldError::Host));
            return;
        };
        self.test_task = spawn_lifecycle(&self.link, group, move || async move {
            let r = session.test_company_smtp(&test).await?;
            Ok(if r.success {
                t("prefs.email.test_ok")
            } else {
                t("prefs.email.test_failed")
            })
        })
        .ok();
    }
}

impl Screen for PreferencesScreen {
    fn title(&self) -> String {
        t("prefs.title")
    }

    fn scope(&self) -> Scope {
        Scope::Form
    }

    fn dirty(&self) -> bool {
        self.edits != self.committed
    }

    fn ui(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        self.poll();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.heading(t("prefs.title"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Right to left: the input sits at the edge, its label to
                // the left of it (the label is the accessible name).
                let input = ui.add_sized(
                    [220.0, 24.0],
                    egui::TextEdit::singleline(&mut self.search).hint_text(t("prefs.search")),
                );
                let l = ui.add(egui::Label::new(t("prefs.search")).selectable(false));
                input.labelled_by(l.id);
            });
        });
        ui.add_space(6.0);
        // The list and the panel scroll inside the visible part of the
        // window, so Cancel and Save stay in view at any size: the height
        // is what is visible below this point minus the footer.
        // The footer: separator, buttons, and one line of block/error/status
        // text when there is one.
        let lines = u8::from(self.block.is_some())
            + u8::from(self.error.is_some())
            + u8::from(self.status.is_some());
        let footer_h = 66.0 + 24.0 * f32::from(lines);
        let body_h = (ui.clip_rect().bottom() - ui.cursor().top() - footer_h).max(160.0);
        egui::ScrollArea::vertical()
            .id_salt("prefs-body")
            .max_height(body_h)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    egui::Frame::new()
                        .fill(tokens.surface_subtle)
                        .corner_radius(egui::CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(8, 10))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.set_width(190.0);
                                self.nav(ui, &tokens);
                            });
                        });
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - 8.0).max(200.0));
                        if !self.loaded.done {
                            form::progress_line(ui, &tokens, &t("prefs.loading"));
                        }
                        self.panel(ui, &tokens);
                    });
                });
            });
        ui.add_space(4.0);
        ui.separator();
        if let Some(b) = &self.block {
            form::hint(ui, &tokens, &t(b.message_key()));
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        if let Some(s) = &self.status {
            ui.label(RichText::new(s).color(tokens.text_secondary));
        }
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.save_task.is_some() {
                    form::progress_line(ui, &tokens, &t("prefs.saving"));
                } else {
                    let can_save = self.loaded.done;
                    if ui
                        .add_enabled_ui(can_save, |ui| primary_button(ui, &t("prefs.save")))
                        .inner
                        .clicked()
                    {
                        self.save();
                    }
                }
                if ui
                    .add(egui::Button::new(t("prefs.cancel")).min_size(Vec2::new(0.0, 44.0)))
                    .clicked()
                {
                    self.revert();
                    self.link
                        .push(CompanyEvent::CloseWindow(CommandId::new(COMMAND)));
                }
            });
        });
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            "form.save_close" => {
                self.close_after_save = true;
                if !self.save() {
                    self.close_after_save = false;
                }
                true
            }
            "form.save_new" => {
                self.save();
                true
            }
            "form.revert" => self.revert(),
            _ => false,
        }
    }

    fn save(&mut self) -> bool {
        PreferencesScreen::save(self)
    }

    fn state(&self) -> Value {
        json!({
            "screen": "preferences",
            "category": self.category.key(),
            "company_open": self.tenant.is_some(),
            "dirty": self.dirty(),
            "loaded": self.loaded.done,
            "saving": self.save_task.is_some(),
            "blocked": self.block.as_ref().map(|b| format!("{b:?}")),
            "error": self.error.is_some(),
            "status": self.status.is_some(),
            "ai": self.edits.ai.state(&self.secrets_saved, &self.link.capabilities()),
            "email": self.edits.email.state(&self.secrets_saved),
            "user": {
                "theme": format!("{:?}", self.edits.user.theme).to_ascii_lowercase(),
                "zoom": self.edits.user.zoom_percent,
                "date_format": format!("{:?}", self.edits.user.date_format).to_ascii_lowercase(),
                "negatives": format!("{:?}", self.edits.user.negatives).to_ascii_lowercase(),
            },
        })
    }
}

/// Builds the screen for the Preferences command, or `None`.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    user: UserPrefs,
) -> Option<Box<dyn Screen>> {
    if kind.as_str() != COMMAND {
        return None;
    }
    let category = if open.is_some() {
        Category::Ai
    } else {
        Category::LookAndFeel
    };
    Some(Box::new(PreferencesScreen::new(
        Arc::clone(link),
        open,
        user,
        category,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_settings::AiProvider;

    fn demo_screen(open: bool) -> (tempfile::TempDir, PreferencesScreen) {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let tenant = open.then(|| {
            (
                TenantId::parse("d0000000-0000-4000-8000-000000000001").expect("id"),
                "Ozark".to_string(),
            )
        });
        if let Some((id, name)) = &tenant {
            link.store()
                .update(|l| {
                    l.upsert(*id, name);
                })
                .expect("store");
        }
        let s = PreferencesScreen::new(link, tenant, UserPrefs::default(), Category::Ai);
        (dir, s)
    }

    #[test]
    fn categories_have_labels_keys_and_search_terms() {
        for c in Category::ALL {
            assert!(crate::i18n::has(c.label_key()), "{c:?}");
            assert_ne!(c.key(), "");
            assert!(c.matches(""));
            assert!(c.matches(&t(c.label_key())));
        }
        assert!(Category::Ai.matches("ollama"));
        assert!(!Category::Backups.matches("ollama"));
        assert_eq!(Category::LookAndFeel.group(), Group::Me);
        assert_eq!(Category::Email.group(), Group::Company);
    }

    #[test]
    fn make_builds_only_the_preferences_command() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        assert!(
            make(
                CommandId::new("nav.home"),
                &link,
                None,
                UserPrefs::default()
            )
            .is_none()
        );
        let s = make(CommandId::new(COMMAND), &link, None, UserPrefs::default()).expect("screen");
        assert_eq!(s.title(), "Preferences");
        assert_eq!(s.state()["category"], "look");
        assert!(!s.dirty());
    }

    #[test]
    fn edits_mark_dirty_save_clears_and_bad_numbers_block() {
        let (_d, mut s) = demo_screen(true);
        assert!(s.loaded.done, "demo loads at once");
        assert!(!s.dirty());
        s.edits_mut().user.theme = cl_settings::ThemeChoice::Dark;
        assert!(s.dirty());
        assert!(Screen::save(&mut s));
        assert!(!s.dirty());
        let events = s.link.drain();
        assert!(events.iter().any(
            |e| matches!(e, CompanyEvent::UserPrefs(p) if p.theme == cl_settings::ThemeChoice::Dark)
        ));
        s.edits_mut().past_days = "lots".into();
        assert!(!Screen::save(&mut s));
        assert_eq!(s.block, Some(SaveBlock::BadNumber("past_days")));
        assert!(s.revert());
        assert!(!s.revert(), "nothing left to revert");
        s.edits_mut().bills_due_days = "14".into();
        assert!(Screen::save(&mut s));
        let rec = s
            .link
            .store()
            .load()
            .expect("load")
            .get(s.tenant.expect("tenant").0)
            .expect("rec")
            .clone();
        assert_eq!(rec.prefs.bills_due_days, 14);
    }

    #[test]
    fn a_cloud_provider_without_a_key_or_agreement_cannot_be_saved_on() {
        let (_d, mut s) = demo_screen(true);
        s.edits_mut().ai.enabled = true;
        s.edits_mut().ai.choose(AiProvider::Anthropic);
        assert!(!Screen::save(&mut s));
        assert_eq!(s.block, Some(SaveBlock::AiNeedsKey));
        s.edits_mut().ai.key_input =
            crate::screens::company::form::SecretText::from("sk-ant-canary-xyz");
        assert!(!Screen::save(&mut s));
        // AI was already on in the demo: the message says "again".
        assert_eq!(s.block, Some(SaveBlock::AiNeedsConsentAgain));
        s.edits_mut().ai.consent_agreed = true;
        assert!(Screen::save(&mut s));
        assert!(s.secrets_saved.key(AiProvider::Anthropic));
        assert!(
            s.edits().ai.key_input.is_empty(),
            "the typed key is cleared"
        );
        let state = s.state().to_string();
        assert!(!state.contains("canary"), "{state}");
        assert_eq!(
            s.state()["ai"]["waiting_engine"],
            true,
            "no session-secrets capability in demo"
        );
        let note = s
            .edits()
            .company
            .ai_consent
            .clone()
            .expect("the consent note is written once the accept is recorded");
        assert_eq!(note.provider, AiProvider::Anthropic);
        assert!(note.accepted_at.is_some());
        let saved = s
            .link
            .secrets()
            .get(cl_settings::ai::ANTHROPIC_KEY)
            .expect("get")
            .expect("saved");
        assert_eq!(saved.expose(), "sk-ant-canary-xyz");
        assert!(!format!("{:?}", s.edits()).contains("canary"));
    }

    /// Every panel draws headlessly (both themes, a wide and the minimum
    /// window) without a secret in the accessibility tree or the state.
    #[test]
    fn every_category_renders_headlessly_without_leaking_a_secret() {
        let (_d, mut s) = demo_screen(true);
        s.edits_mut().ai.provider = AiProvider::Anthropic;
        s.edits_mut().ai.key_input =
            crate::screens::company::form::SecretText::from("sk-ant-render-canary");
        s.edits_mut().email.password_input =
            crate::screens::company::form::SecretText::from("smtp-render-canary");
        for size in [egui::vec2(1440.0, 940.0), egui::vec2(960.0, 640.0)] {
            for c in Category::ALL {
                s.select(c);
                let ctx = egui::Context::default();
                crate::shell::Shell::install(&ctx);
                ctx.enable_accesskit();
                let mut texts = Vec::new();
                for _ in 0..2 {
                    let mut out = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                            ..egui::RawInput::default()
                        },
                        |ui| s.ui(ui),
                    );
                    // Nothing paints here; egui insists the texture updates
                    // are consumed.
                    out.textures_delta.clear();
                    if let Some(tree) = out.platform_output.accesskit_update {
                        for (_, node) in tree.nodes {
                            if let Some(v) = node.value() {
                                texts.push(v.to_string());
                            }
                            if let Some(l) = node.label() {
                                texts.push(l.to_string());
                            }
                        }
                    }
                }
                let all = texts.join(
                    "
",
                );
                assert!(
                    !all.contains("canary"),
                    "{c:?} at {size:?} leaks a secret: {all}"
                );
                assert_eq!(s.state()["category"], c.key());
            }
        }
        assert!(!s.state().to_string().contains("canary"));
    }

    /// A re-read after a failed save refreshes the baseline but keeps the
    /// owner's unsaved edits (S2).
    #[test]
    fn a_reload_keeps_unsaved_edits_and_refreshes_the_baseline() {
        let (_d, mut s) = demo_screen(true);
        s.edits_mut().user.theme = cl_settings::ThemeChoice::Dark;
        s.edits_mut().ai.ollama_model = "qwen2.5:7b".into();
        assert!(s.dirty());
        s.apply_loaded(crate::company::demo::preferences_loaded());
        assert_eq!(s.edits().user.theme, cl_settings::ThemeChoice::Dark);
        assert_eq!(s.edits().ai.ollama_model, "qwen2.5:7b");
        assert!(s.dirty(), "the edits survived the reload");
        assert_eq!(
            s.committed.ai.ollama_model, "llama3.1:8b",
            "the baseline is the engine's"
        );
        assert!(s.revert());
        assert_eq!(s.edits().ai.ollama_model, "llama3.1:8b");
        // Clean: the reload refreshes both.
        s.apply_loaded(crate::company::demo::preferences_loaded());
        assert!(!s.dirty());
    }

    /// A save that does not touch the AI panel is not blocked by its
    /// consent state; a changed panel on an already-on AI says "again".
    #[test]
    fn untouched_ai_panel_never_blocks_a_save() {
        let (_d, mut s) = demo_screen(true);
        s.edits_mut().ai.consent_agreed = false;
        s.committed.ai.consent_agreed = false;
        s.edits_mut().user.theme = cl_settings::ThemeChoice::Dark;
        assert!(Screen::save(&mut s), "theme-only save: {:?}", s.block);
        s.edits_mut().ai.ollama_model = "qwen2.5:7b".into();
        assert!(!Screen::save(&mut s));
        assert_eq!(s.block, Some(SaveBlock::AiNeedsConsentAgain));
        assert!(crate::i18n::has(
            SaveBlock::AiNeedsConsentAgain.message_key()
        ));
    }

    #[test]
    fn save_close_and_cancel_close_the_window() {
        let (_d, mut s) = demo_screen(false);
        assert_eq!(s.state()["company_open"], false);
        assert!(Screen::command(&mut s, CommandId::new("form.save_close")));
        let events = s.link.drain();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, CompanyEvent::CloseWindow(k) if k.as_str() == COMMAND))
        );
        assert!(
            !Screen::command(&mut s, CommandId::new("form.revert")),
            "nothing to revert"
        );
        assert!(!Screen::command(&mut s, CommandId::new("edit.undo")));
    }
}
