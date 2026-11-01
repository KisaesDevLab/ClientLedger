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
//!   record (bills-due days, AI switches and cap, consent note, SMTP
//!   security, this computer's intake-folder confirmations) and the setup
//!   flags (Q-P08c);
//! - *This company*, intake folders → `/company/intake-settings` (U-93,
//!   A01), see [`intake_folders`];
//! - secrets (AI keys, SMTP password) → the OS keychain only, pushed to
//!   the engine per session once `session-secrets-v1` exists (Q-P09a).
//!
//! Every engine call runs on the request queue and is polled each frame.

pub mod ai;
pub mod ai_cloud_reading;
pub mod company;
pub mod email;
pub mod intake_folders;
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
    /// Reading documents with an online service (A03, Q-A03k).
    AiCloudReading,
    /// SMTP.
    Email,
    /// Document intake: the company's intake folders (A01; shown as
    /// "Document intake", key `inbox` kept for P17 callers).
    ReceiptInbox,
    /// Backup folder and backup on close.
    Backups,
    /// Update checks.
    Updates,
}

impl Category {
    /// Every category in list order.
    pub const ALL: [Category; 13] = [
        Category::LookAndFeel,
        Category::DatesNumbers,
        Category::Reminders,
        Category::Accounting,
        Category::ChecksForms,
        Category::SalesTax,
        Category::Inventory,
        Category::Ai,
        Category::AiCloudReading,
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
            Category::AiCloudReading => "prefs.cat.cloud_reading",
            Category::Email => "prefs.cat.email",
            Category::ReceiptInbox => "prefs.cat.intake",
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
            Category::AiCloudReading => "ai_cloud_reading",
            Category::Email => "email",
            Category::ReceiptInbox => "inbox",
            Category::Backups => "backups",
            Category::Updates => "updates",
        }
    }

    /// Words the search box matches besides the label.
    fn search_terms(self) -> &'static str {
        match self {
            Category::LookAndFeel => "theme dark light zoom colour color enter keyboard tab",
            Category::DatesNumbers => "date format negative parentheses warning",
            Category::Reminders => "bills due backup remind",
            Category::Accounting => "basis cash accrual fiscal year closing date account numbers",
            Category::ChecksForms => "check number print form",
            Category::SalesTax => "tax rate",
            Category::Inventory => "stock items cost",
            Category::Ai => "ai anthropic openai ollama key suggestions cap usage",
            Category::AiCloudReading => {
                "ai online cloud document reading text pictures images consent anthropic openai"
            }
            Category::Email => "smtp email send password server",
            Category::ReceiptInbox => {
                "receipt inbox folder photo pdf intake document bills scan scanner share network"
            }
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
    /// Active income accounts (the markup account picker, Q-P20b).
    pub incomes: Vec<(cl_core::ids::AccountId, String)>,
    /// Active expense accounts (write-off and bank-fee pickers, P21).
    pub expenses: Vec<(cl_core::ids::AccountId, String)>,
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
            .field("incomes", &self.incomes.len())
            .field("expenses", &self.expenses.len())
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
    /// Reading documents online (A03): the company's two consent switches.
    pub cloud_reading: ai_cloud_reading::CloudReadingEdits,
    /// The Email panel.
    pub email: email::EmailEdits,
    /// The Document intake panel (the engine's folder list and this
    /// window's pending confirmations).
    pub intake: intake_folders::IntakeEdits,
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
    /// The intake folders can't be saved as they are.
    Intake(intake_folders::IntakeBlock),
    /// The P17 receipt-inbox folder is not usable (an engine without
    /// intake folders, Q-A01q).
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
            SaveBlock::Intake(b) => b.message_key(),
            SaveBlock::Folder(e) => intake_folders::legacy::folder_error_key(*e),
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
    /// The engine accepted this intake-folder list (U-93).
    pub intake: Option<intake_folders::source::Stored>,
    /// Reading documents online as the engine stores it after the save
    /// (read back, A03 review B1); `None` when it couldn't be read.
    pub cloud_reading: Option<ai_cloud_reading::CloudReadingEdits>,
}

/// Reading documents online after a save, and whether the owner is told
/// it was turned off (A03 review B1). `prev` is what the engine held
/// before, `sent` the owner's own change in this save, `stored` what the
/// engine holds now. Without a read-back the engine's known rules stand:
/// turning AI off clears both switches (revoke); otherwise the owner's
/// change, or what was there. The message shows only when the engine went
/// from on to off without the owner asking.
#[must_use]
pub fn cloud_after_save(
    prev: ai_cloud_reading::CloudReadingEdits,
    sent: Option<ai_cloud_reading::CloudReadingEdits>,
    stored: Option<ai_cloud_reading::CloudReadingEdits>,
    ai_on: bool,
) -> (ai_cloud_reading::CloudReadingEdits, bool) {
    let off = ai_cloud_reading::CloudReadingEdits::default();
    let now = stored.unwrap_or_else(|| if ai_on { sent.unwrap_or(prev) } else { off });
    (now, sent.is_none() && prev != off && now == off)
}

/// The document switches the engine stores now; `None` when the read
/// fails (the save itself succeeded).
pub async fn stored_cloud_reading(
    session: &cl_api::Session,
) -> Option<ai_cloud_reading::CloudReadingEdits> {
    let consent = session.company_ai_disclosure().await.ok()?;
    Some(ai_cloud_reading::CloudReadingEdits::from_consent(Some(
        &consent,
    )))
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
    /// The owner's own change to Reading documents online in the running
    /// save (`None`: unchanged). An engine that turned them off otherwise
    /// is said so, on that screen (review B1).
    cloud_sent: Option<ai_cloud_reading::CloudReadingEdits>,
    /// The Document intake panel's reads, tests and dialogs.
    pub(crate) intake: intake_folders::IntakePanel,
    /// The fixed bars' sizes last frame (Q-265).
    layout: crate::layout::FrameReport,
    /// The Cancel and Save pair's width last frame (they sit right-aligned
    /// in screen and Tab order).
    footer_w: f32,
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
            cloud_reading: ai_cloud_reading::CloudReadingEdits::default(),
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
            intake: intake_folders::IntakeEdits::default(),
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
            cloud_sent: None,
            intake: intake_folders::IntakePanel::default(),
            layout: crate::layout::FrameReport::default(),
            footer_w: 220.0,
        };
        screen.start_load();
        screen.load_intake();
        screen
    }

    /// Reads the intake folders and the user's role (Document intake).
    fn load_intake(&mut self) {
        let tenant = self.tenant.as_ref().map(|(id, _)| *id);
        if let Some(f) = self
            .intake
            .start_load(&self.link, tenant, self.group.as_ref())
        {
            self.intake
                .apply(f, &mut self.edits.intake, &mut self.committed.intake);
        }
    }

    /// Reload after a 409: the engine's list replaces the typed changes.
    fn reload_intake(&mut self) {
        self.intake.replace_next_load();
        self.load_intake();
    }

    /// ClientLedger's own folders, refused as intake folders: the app data
    /// folder and the backup folders.
    /// The engine keeps no intake folders: the P17 single receipt-inbox
    /// folder is still chosen here (Q-A01q).
    fn legacy_folder(&self) -> bool {
        self.intake.load == intake_folders::LoadState::NeedsEngine
    }

    fn app_dirs(&self) -> Vec<std::path::PathBuf> {
        let mut dirs = self.link.app_dirs();
        let typed = self.edits.backup_folder.trim();
        if !typed.is_empty() {
            dirs.push(std::path::PathBuf::from(typed));
        }
        dirs
    }

    fn start_load(&mut self) {
        if self.tenant.is_none() {
            self.loaded.done = true;
            return;
        }
        let demo_ai = self.link.demo_state().map(|d| d.ai);
        if let Some(switches) = demo_ai {
            self.apply_loaded(crate::company::demo::preferences_loaded_with(switches));
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
                incomes: income_accounts(&s).await,
                expenses: expense_accounts(&s).await,
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
        base.cloud_reading =
            ai_cloud_reading::CloudReadingEdits::from_consent(loaded.consent.as_ref());
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
        self.intake
            .validate(&e.intake, &self.committed.intake, &self.app_dirs())
            .map_err(SaveBlock::Intake)?;
        // An engine without intake folders keeps P17's single folder
        // (Q-A01q), checked as P17 checked it.
        if self.legacy_folder()
            && let Some(p) = e
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
            let legacy = self.legacy_folder();
            if let Err(e) = self.link.store().update(|l| {
                if let Some(rec) = l.get_mut(id) {
                    // Only the fields this screen edits: lists, the AI
                    // usage count and the consent note write theirs while
                    // Preferences is open (B3).
                    write_owned_prefs(&prefs, &mut rec.prefs, legacy);
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
        let markup = (edits.accounting.markup_account
            != self
                .loaded
                .settings
                .as_ref()
                .and_then(|s| s.billable_markup_account_id))
        .then_some(edits.accounting.markup_account);
        if let (Some(m), Some(mut demo)) = (markup, self.link.demo_state()) {
            demo.invoices.markup_account = m;
        }
        // P21: Receive Payment's settings go by their own write.
        let receivables = edits
            .accounting
            .receivables
            .update_for(self.loaded.settings.as_ref())
            .ok()
            .flatten();
        if let (Some(u), Some(mut demo)) = (receivables.as_ref(), self.link.demo_state()) {
            let s = &mut demo.receivables.settings;
            if let Some(v) = u.sales_discount_account_id {
                s.discount_account = v;
            }
            if let Some(v) = u.write_off_account_id {
                s.write_off_account = v;
            }
            if let Some(v) = u.write_off_threshold {
                s.write_off_threshold = v;
            }
            if let Some(v) = u.nsf_bank_fee_account_id {
                s.nsf_bank_fee_account = v;
            }
            if let Some(v) = u.nsf_fee_income_account_id {
                s.nsf_fee_income_account = v;
            }
            if let Some(v) = u.nsf_customer_fee {
                s.nsf_customer_fee = v;
            }
        }
        let smtp = edits.email.update_for(self.loaded.smtp.as_ref());
        // Intake folders: the engine takes the whole list against the stamp
        // it was read with. It goes last, so an earlier failure never leaves
        // a list the engine took without the panel knowing its new stamp.
        // This computer's confirmations are written only once it is
        // accepted (finish_save).
        let intake_put = (edits.intake != self.committed.intake)
            .then(|| (edits.intake.folders.clone(), self.intake.updated_at.clone()));
        // Reading documents online (A03): only the owner's change, sent
        // after the AI panel's own (whose consent accept it needs). Turning
        // AI off clears both, as the engine's revoke does.
        let cloud_ready = self.cloud_reading_available() == ai_cloud_reading::Availability::Ready;
        let cloud_put = (cloud_ready && edits.cloud_reading != self.committed.cloud_reading)
            .then(|| edits.cloud_reading.toggles());
        let mut after = edits.clone();
        after.ai.clear_typed();
        after.email.clear_typed();
        after.company = company_prefs;
        // The document switches shown after the save are what the engine
        // stores, read back once it answers (review B1): the engine turns
        // them off on AI off, and on a newer AI notice only. A change sent
        // in this same save goes after the consent, so it stands.
        after.cloud_reading = self.committed.cloud_reading;
        self.cloud_sent = cloud_put.is_some().then_some(edits.cloud_reading);
        if let Some(mut demo) = self.link.demo_state() {
            // The demo engine keeps the AI switches Write Check reads, and
            // the document switches by the engine's rules.
            let (cloud, _) = cloud_after_save(
                self.committed.cloud_reading,
                self.cloud_sent,
                None,
                edits.ai.enabled,
            );
            demo.ai = crate::company::demo::DemoAi {
                enabled: edits.ai.enabled,
                categorization: edits.ai.bank_feed,
                reading: edits.ai.read_documents,
                assist: demo.ai.assist,
                text_cloud: cloud.text,
                images_cloud: cloud.text && cloud.images,
            };
        }
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
            let mut intake = None;
            if let (Some((folders, expected)), Some((id, _))) = (intake_put, self.tenant.as_ref()) {
                match intake_folders::source::demo_put(
                    &self.link,
                    *id,
                    folders,
                    expected.as_deref(),
                ) {
                    Ok(stored) => intake = Some(stored),
                    Err(intake_folders::source::DemoPutError::Stale) => {
                        // Nothing of the intake list is written here.
                        self.intake.stale = true;
                        return false;
                    }
                    Err(_) => {
                        self.block = Some(SaveBlock::Intake(intake_folders::IntakeBlock::Refused));
                        return false;
                    }
                }
            }
            self.finish_save(
                &SaveOutcome {
                    ai_waiting_engine: plan.waiting_engine,
                    cap_reached: plan.cap_reached,
                    consent,
                    intake,
                    // The demo engine follows the engine's rules
                    // (`cloud_after_save` without a read-back).
                    cloud_reading: None,
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
            if let Some(m) = markup {
                session
                    .set_billable_markup_account(m, cl_api::IdempotencyKey::new())
                    .await?;
            }
            if let Some(u) = receivables {
                session
                    .set_receivables_settings(&u, cl_api::IdempotencyKey::new())
                    .await?;
            }
            if let Some(u) = smtp {
                session
                    .update_smtp_settings(&u, cl_api::IdempotencyKey::new())
                    .await?;
            }
            let mut outcome = plan.run(&session, secrets_for_engine).await?;
            if let Some(toggles) = cloud_put {
                // The engine records the change in its audit trail.
                session
                    .set_ai_tasks(&toggles, cl_api::IdempotencyKey::new())
                    .await?;
            }
            // What the engine now stores, after the consent and the change
            // (review B1).
            outcome.cloud_reading = stored_cloud_reading(&session).await;
            if let Some((folders, expected)) = intake_put {
                outcome.intake = Some(
                    intake_folders::source::engine_put(&session, &folders, expected.as_deref())
                        .await?,
                );
            }
            Ok(outcome)
        })
        .ok();
        true
    }

    fn finish_save(&mut self, outcome: &SaveOutcome, mut after: Edits) {
        let (cloud, cloud_reset) = cloud_after_save(
            self.committed.cloud_reading,
            self.cloud_sent.take(),
            outcome.cloud_reading,
            after.ai.enabled,
        );
        after.cloud_reading = cloud;
        let typed_intake = after.intake.clone();
        match &outcome.intake {
            Some(stored) => self.finish_intake_save(stored, &mut after),
            // Not sent (unchanged, or no engine to take it): the baseline
            // stays and any typed change stays dirty.
            None => after.intake = self.committed.intake.clone(),
        }
        self.committed = after;
        self.edits = self.committed.clone();
        if outcome.intake.is_none() {
            self.edits.intake = typed_intake;
        }
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
        if cloud_reset {
            // Shown on its own screen, off (#179 review).
            self.category = Category::AiCloudReading;
        }
        self.status = Some(if cloud_reset {
            t("prefs.cloud_reading.reset")
        } else if outcome.cap_reached {
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

    /// The engine took the intake list (`stored`): this computer's new
    /// confirmations are written now, in one store update, with those of
    /// removed folders dropped; then the inbox service is told (A01
    /// "Preferences save ordering").
    fn finish_intake_save(&mut self, stored: &intake_folders::source::Stored, after: &mut Edits) {
        let pending = std::mem::take(&mut after.intake.confirm);
        let mut here = after.company.intake_here.clone();
        here.extend(pending.clone());
        if let Some((id, name)) = &self.tenant {
            let id = *id;
            match self.link.store().update(|l| {
                // A company with no record on this computer yet (the demo,
                // or one opened before its record was written) gets one, so
                // "Set up on this computer" survives a reopen (UI check M2).
                if l.get(id).is_none() {
                    l.upsert(id, name);
                }
                l.get_mut(id).map(|rec| {
                    rec.prefs.intake_here.extend(pending);
                    rec.prefs.retain_intake_for(&stored.folders);
                    rec.prefs.intake_here.clone()
                })
            }) {
                Ok(Some(written)) => here = written,
                Ok(None) => {}
                Err(e) => self.error = Some(e.into()),
            }
        }
        here.retain(|id, _| stored.folders.iter().any(|f| f.id == *id));
        after.company.intake_here = here;
        after.intake = intake_folders::IntakeEdits {
            folders: stored.folders.clone(),
            confirm: std::collections::BTreeMap::new(),
        };
        self.intake.saved(stored);
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
        self.intake
            .poll(&mut self.edits.intake, &mut self.committed.intake);
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
                    // A 409 on the intake list is the owner's to resolve:
                    // Reload (which replaces the typed folder changes).
                    if intake_folders::source::is_stale(&e) {
                        self.intake.stale = true;
                    } else {
                        self.error = Some(e);
                    }
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

    /// Whether the Reading documents online switches can change: the
    /// engine's assist (the demo engine keeps the switches too), the owner,
    /// and AI on.
    #[must_use]
    pub fn cloud_reading_available(&self) -> ai_cloud_reading::Availability {
        ai_cloud_reading::Availability::of(
            self.link.is_demo()
                || self
                    .link
                    .capabilities()
                    .has(cl_api::Capabilities::DOCUMENT_ASSIST),
            self.intake.is_admin(),
            self.edits.ai.enabled,
        )
    }

    /// The engine sends page pictures to an online provider at all.
    fn cloud_vision(&self) -> bool {
        self.loaded
            .ai
            .as_ref()
            .is_some_and(AiConfig::cloud_vision_allowed)
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
            Category::SalesTax => {
                company::sales_tax(ui, tokens, &mut self.edits);
                if self.link.is_demo()
                    || self
                        .link
                        .capabilities()
                        .has(cl_api::Capabilities::BILLABLE_COSTS)
                {
                    company::billable(ui, tokens, &mut self.edits, &self.loaded.incomes);
                }
                if self.link.is_demo()
                    || self
                        .link
                        .capabilities()
                        .has(cl_api::Capabilities::PAYMENT_SETTLE)
                {
                    company::receivables(
                        ui,
                        tokens,
                        &mut self.edits,
                        &self.loaded.incomes,
                        &self.loaded.expenses,
                    );
                }
            }
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
            Category::AiCloudReading => {
                let available = self.cloud_reading_available();
                let vision = self.cloud_vision();
                ai_cloud_reading::panel(
                    ui,
                    tokens,
                    &mut self.edits.cloud_reading,
                    available,
                    vision,
                );
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
            Category::ReceiptInbox => {
                let app_dirs = self.app_dirs();
                let action = intake_folders::show(
                    ui,
                    tokens,
                    &mut intake_folders::View {
                        panel: &mut self.intake,
                        edits: &mut self.edits.intake,
                        committed: &self.committed.intake,
                        here: &self.committed.company.intake_here,
                        legacy_dir: &mut self.edits.company.receipt_inbox_dir,
                        app_dirs: &app_dirs,
                    },
                );
                if action == intake_folders::PanelAction::Reload {
                    self.reload_intake();
                }
            }
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
            ai::PanelAction::OpenCloudReading => self.category = Category::AiCloudReading,
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
        if let Some(c) = take_requested_category() {
            self.select(c);
        }
        self.poll();
        // The title and Search stay at the top, Cancel and Save at the
        // bottom; the list and the panel scroll between them (Q-265).
        let mut frame =
            crate::layout::FixedFrame::begin(ui, COMMAND, crate::layout::FrameOpts::default());
        frame.top(|ui| {
            ui.add_space(6.0);
            // Narrow (960 px at 200%): the search box takes what is left of
            // the title row, without its visible label (its accessible name
            // stays "Find a setting"), so it isn't cut off and the body keeps
            // its room (UI check M1).
            let narrow = ui.available_width() < 560.0;
            ui.horizontal(|ui| {
                ui.heading(t("prefs.title"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let w = if narrow {
                        (ui.available_width() - 4.0).clamp(80.0, 220.0)
                    } else {
                        220.0
                    };
                    // Right to left: the input sits at the edge, its label
                    // to the left of it (the label is the accessible name).
                    let input = ui.add_sized(
                        [w, 24.0],
                        egui::TextEdit::singleline(&mut self.search).hint_text(t("prefs.search")),
                    );
                    if narrow {
                        let name = t("prefs.search");
                        input.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::TextInput, true, name.clone())
                        });
                    } else {
                        let l = ui.add(egui::Label::new(t("prefs.search")).selectable(false));
                        input.labelled_by(l.id);
                    }
                });
            });
        });
        frame.body(crate::layout::Body::Scroll, |ui| {
            // The category list gives way on a narrow window so the panel
            // keeps room (960 px at 200%).
            let nav_w = (ui.available_width() * 0.3).clamp(130.0, 190.0);
            ui.horizontal_top(|ui| {
                egui::Frame::new()
                    .fill(tokens.surface_subtle)
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(8, 10))
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(nav_w);
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
        // An email field problem goes away once the field is corrected.
        // (Host also stands for "no test request", so it stays until Save.)
        if let Some(SaveBlock::Smtp(e)) = &self.block
            && *e != cl_settings::SmtpFieldError::Host
            && self.edits.email.form.validate().err().as_ref() != Some(e)
        {
            self.block = None;
        }
        frame.bottom(|ui| {
            ui.separator();
            if let Some(b) = &self.block {
                form::hint(ui, &tokens, &t(b.message_key()));
            }
            if let Some(e) = &self.error {
                form::inline_error(ui, &tokens, e);
            }
            // Right-aligned, but laid out left to right so Tab goes Cancel
            // then Save, as they read on screen (UI check): the pair's
            // width from the last frame places them. "Saved." sits on the
            // same line, at the left, so the bar stays one row.
            ui.horizontal(|ui| {
                if let Some(s) = &self.status {
                    ui.add(
                        egui::Label::new(RichText::new(s).color(tokens.text_secondary)).truncate(),
                    );
                }
                ui.add_space((ui.available_width() - self.footer_w).max(0.0));
                let cancel =
                    ui.add(egui::Button::new(t("prefs.cancel")).min_size(Vec2::new(0.0, 44.0)));
                if cancel.clicked() {
                    self.revert();
                    self.link
                        .push(CompanyEvent::CloseWindow(CommandId::new(COMMAND)));
                }
                let last = if self.save_task.is_some() {
                    ui.scope(|ui| form::progress_line(ui, &tokens, &t("prefs.saving")))
                        .response
                } else {
                    let can_save = self.loaded.done;
                    let save = ui
                        .add_enabled_ui(can_save, |ui| primary_button(ui, &t("prefs.save")))
                        .inner;
                    if save.clicked() {
                        self.save();
                    }
                    save
                };
                let w = last.rect.right() - cancel.rect.left();
                if (w - self.footer_w).abs() > 0.5 {
                    self.footer_w = w;
                    ui.ctx().request_repaint();
                }
            });
        });
        self.layout = frame.end();
    }

    fn fixed_layout(&self) -> bool {
        true
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
            // Esc closes the Add folder row (or a rename) before the
            // window (DESIGN: Esc reverts the focused field first).
            "form.revert" => self.intake.escape() || self.revert(),
            _ => false,
        }
    }

    fn save(&mut self) -> bool {
        PreferencesScreen::save(self)
    }

    fn state(&self) -> Value {
        json!({
            "screen": "preferences",
            "layout": self.layout.json(),
            "category": self.category.key(),
            "company_open": self.tenant.is_some(),
            "dirty": self.dirty(),
            "loaded": self.loaded.done,
            "saving": self.save_task.is_some(),
            "blocked": self.block.as_ref().map(|b| format!("{b:?}")),
            "error": self.error.is_some(),
            "status": self.status.is_some(),
            "ai": self.edits.ai.state(&self.secrets_saved, &self.link.capabilities()),
            "cloud_reading": ai_cloud_reading::state_json(
                &self.edits.cloud_reading,
                self.cloud_reading_available(),
                self.cloud_vision(),
            ),
            "email": self.edits.email.state(&self.secrets_saved),
            "account_numbers": self.edits.company.account_numbers,
            "intake": self.intake.state(
                &self.edits.intake,
                &self.committed.intake,
                &self.committed.company.intake_here,
            ),
            "user": {
                "theme": format!("{:?}", self.edits.user.theme).to_ascii_lowercase(),
                "zoom": self.edits.user.zoom_percent,
                "date_format": format!("{:?}", self.edits.user.date_format).to_ascii_lowercase(),
                "negatives": format!("{:?}", self.edits.user.negatives).to_ascii_lowercase(),
                "enter_moves_fields": self.edits.user.enter_moves_fields,
            },
        })
    }
}

/// The panel another window asked Preferences to show next (P17: the
/// receipt inbox's "Choose a folder").
static REQUESTED: std::sync::Mutex<Option<Category>> = std::sync::Mutex::new(None);

/// Show `category` when Preferences opens next, or at once when it is
/// already open (the caller then opens or brings forward the window).
pub fn open_at(category: Category) {
    *REQUESTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(category);
}

fn take_requested_category() -> Option<Category> {
    REQUESTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
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
    let category = take_requested_category().unwrap_or(if open.is_some() {
        Category::Ai
    } else {
        Category::LookAndFeel
    });
    Some(Box::new(PreferencesScreen::new(
        Arc::clone(link),
        open,
        user,
        category,
    )))
}

/// Copies the company-local fields Preferences edits from `from` onto
/// `to`, leaving every other field (sensitive accounts, required tag
/// groups, the AI usage count, the consent note) as `to` has it.
///
/// This computer's intake confirmations are not written here: they are
/// written only after the engine accepted the intake list
/// ([`PreferencesScreen::finish_save`]). The P17 folder is written only
/// when `legacy` (an engine without intake folders, where it is still the
/// inbox's folder, Q-A01q); otherwise the migration owns it, and a window
/// left open across the migration must not bring it back.
fn write_owned_prefs(
    from: &cl_settings::CompanyPrefs,
    to: &mut cl_settings::CompanyPrefs,
    legacy: bool,
) {
    if legacy {
        to.receipt_inbox_dir.clone_from(&from.receipt_inbox_dir);
    }
    to.bills_due_days = from.bills_due_days;
    to.ai.suggest_rules = from.ai.suggest_rules;
    to.ai.monthly_token_cap = from.ai.monthly_token_cap;
    to.smtp_security = from.smtp_security;
    to.account_numbers = from.account_numbers;
    // P20: the email templates and BCC-self (Q-P20j).
    to.email.clone_from(&from.email);
    to.email.normalize();
}

/// The active expense accounts, for the write-off and bank-fee pickers
/// (empty when the read fails).
async fn expense_accounts(s: &cl_api::Session) -> Vec<(cl_core::ids::AccountId, String)> {
    let filter = cl_api::AccountFilter {
        types: vec![
            cl_api::AccountType::Expense,
            cl_api::AccountType::OtherExpense,
        ],
        is_active: Some(true),
        ..cl_api::AccountFilter::default()
    };
    match s
        .accounts()
        .list(&filter, cl_api::PageRequest::first_max())
        .await
    {
        Ok(p) => p.data.into_iter().map(|a| (a.id, a.fields.name)).collect(),
        Err(_) => Vec::new(),
    }
}

/// The active income accounts, for the markup account picker (empty when
/// the read fails).
async fn income_accounts(s: &cl_api::Session) -> Vec<(cl_core::ids::AccountId, String)> {
    let filter = cl_api::AccountFilter {
        types: vec![
            cl_api::AccountType::Revenue,
            cl_api::AccountType::OtherRevenue,
        ],
        is_active: Some(true),
        ..cl_api::AccountFilter::default()
    };
    match s
        .accounts()
        .list(&filter, cl_api::PageRequest::first_max())
        .await
    {
        Ok(p) => p.data.into_iter().map(|a| (a.id, a.fields.name)).collect(),
        Err(_) => Vec::new(),
    }
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

    /// A03 step 21: the switches start off, the owner's change is stored,
    /// and AI turned off (as a re-consent) turns them off again, shown on
    /// their own screen.
    #[test]
    fn reading_documents_online_is_stored_and_reset() {
        let (_d, mut s) = demo_screen(true);
        assert_eq!(
            s.cloud_reading_available(),
            ai_cloud_reading::Availability::Ready
        );
        assert_eq!(
            s.edits().cloud_reading,
            ai_cloud_reading::CloudReadingEdits::default()
        );
        s.edits_mut().cloud_reading.text = true;
        assert!(s.save());
        let demo = s.link.demo_state().expect("demo").ai;
        assert!(demo.text_cloud && !demo.images_cloud);
        assert!(s.committed.cloud_reading.text);
        // AI off: the engine forgets them.
        s.select(Category::Ai);
        s.edits_mut().ai.enabled = false;
        assert!(s.save());
        assert_eq!(
            s.edits().cloud_reading,
            ai_cloud_reading::CloudReadingEdits::default()
        );
        assert_eq!(s.category, Category::AiCloudReading);
        assert_eq!(s.status, Some(t("prefs.cloud_reading.reset")));
        assert!(!s.link.demo_state().expect("demo").ai.text_cloud);
        // AI off: the switches can't change.
        assert_eq!(
            s.cloud_reading_available(),
            ai_cloud_reading::Availability::AiOff
        );
    }

    /// Review B1: the switches after a save are the engine's, and "turned
    /// off" is said only when the engine went from on to off by itself.
    #[test]
    fn cloud_switches_after_a_save_follow_the_engine() {
        use ai_cloud_reading::CloudReadingEdits as C;
        let on = C {
            text: true,
            images: true,
        };
        let off = C::default();
        // A re-consent the engine keeps them on through: on, no message.
        assert_eq!(cloud_after_save(on, None, Some(on), true), (on, false));
        // The engine turned them off (a newer notice): off, said so.
        assert_eq!(cloud_after_save(on, None, Some(off), true), (off, true));
        // The owner turned them off: no message.
        assert_eq!(
            cloud_after_save(on, Some(off), Some(off), true),
            (off, false)
        );
        // Off before: nothing to say.
        assert_eq!(cloud_after_save(off, None, Some(off), false), (off, false));
        // No read-back: AI off clears them (revoke), else they stand.
        assert_eq!(cloud_after_save(on, None, None, false), (off, true));
        assert_eq!(cloud_after_save(on, None, None, true), (on, false));
        let text = C {
            text: true,
            images: false,
        };
        assert_eq!(cloud_after_save(off, Some(text), None, true), (text, false));
    }

    /// Review B1: an engine that keeps the document switches on through a
    /// re-accepted AI notice (same version) is shown as it is.
    #[tokio::test]
    async fn a_reconsent_the_engine_keeps_switches_through_shows_them_on() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};
        let server = wiremock::MockServer::start().await;
        let company = "0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/ai/consent/{company}/disclosure")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "companyId": company, "systemVersion": 2, "acceptedVersion": 2,
                "acceptedAt": "2026-10-17T10:00:00.000Z", "aiEnabled": true,
                "enabledTasks": {"receipt_ocr": true, "document_text_cloud": true,
                    "document_images_cloud": true},
            })))
            .mount(&server)
            .await;
        let session = cl_api::testing::stub_session(server.address().port());
        let stored = stored_cloud_reading(&session).await;
        let on = ai_cloud_reading::CloudReadingEdits {
            text: true,
            images: true,
        };
        assert_eq!(stored, Some(on));
        assert_eq!(cloud_after_save(on, None, stored, true), (on, false));
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
        assert!(Category::LookAndFeel.matches("enter"));
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
    fn the_enter_option_is_a_just_for_me_preference() {
        let (_d, mut s) = demo_screen(true);
        assert_eq!(s.state()["user"]["enter_moves_fields"], false);
        s.edits_mut().user.enter_moves_fields = true;
        assert!(s.dirty());
        assert_eq!(s.state()["user"]["enter_moves_fields"], true);
        assert!(Screen::save(&mut s));
        assert!(
            s.link
                .drain()
                .iter()
                .any(|e| matches!(e, CompanyEvent::UserPrefs(p) if p.enter_moves_fields))
        );
        assert!(crate::i18n::has("prefs.user.enter_moves"));
        assert!(crate::i18n::has("prefs.user.enter_moves.tip"));
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
            .get(s.tenant.as_ref().expect("tenant").0)
            .expect("rec")
            .clone();
        assert_eq!(rec.prefs.bills_due_days, 14);
        // Q-79: the account-number preference is saved on the company.
        assert!(rec.prefs.account_numbers);
        assert_eq!(s.state()["account_numbers"], true);
        s.edits_mut().company.account_numbers = false;
        assert!(s.dirty());
        assert!(Screen::save(&mut s));
        let rec = s
            .link
            .store()
            .load()
            .expect("load")
            .get(s.tenant.as_ref().expect("tenant").0)
            .expect("rec")
            .clone();
        assert!(!rec.prefs.account_numbers);
    }

    /// B3: a list writes its company-local fields while Preferences is
    /// open; saving Preferences afterwards keeps them.
    #[test]
    fn saving_keeps_what_lists_wrote_while_open() {
        let (_d, mut s) = demo_screen(true);
        let id = s.tenant.as_ref().expect("tenant").0;
        let account =
            cl_core::ids::AccountId::parse("a0000000-0000-4000-8000-000000000007").expect("id");
        let group =
            cl_core::ids::TagGroupId::parse("b0000000-0000-4000-8000-000000000003").expect("id");
        s.link
            .store()
            .update(|l| {
                if let Some(rec) = l.get_mut(id) {
                    rec.prefs.sensitive_accounts.insert(account);
                    rec.prefs
                        .required_tag_groups
                        .insert(group, std::iter::once("expense".to_string()).collect());
                    rec.prefs.ai.usage.tokens = 42;
                }
            })
            .expect("list write");
        s.edits_mut().bills_due_days = "21".into();
        assert!(Screen::save(&mut s));
        let rec = s
            .link
            .store()
            .load()
            .expect("load")
            .get(id)
            .expect("rec")
            .clone();
        assert_eq!(rec.prefs.bills_due_days, 21, "Preferences saved");
        assert!(
            rec.prefs.sensitive_accounts.contains(&account),
            "sensitive kept"
        );
        assert!(
            rec.prefs.required_tag_groups.contains_key(&group),
            "required kept"
        );
        assert_eq!(rec.prefs.ai.usage.tokens, 42, "usage kept");
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
                cl_grid::sequencer::install(&ctx);
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
