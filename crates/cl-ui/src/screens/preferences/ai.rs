//! The AI panel (P09 step 3; mockup `Preferences.html`): off by default
//! (Q-38); where AI runs (Ollama on this computer, Anthropic or `OpenAI`
//! with the owner's key, Q-33); the cloud notice whose acceptance the
//! engine records in the audit trail (Q-136); per-feature switches
//! (Q-140); the usage meter and monthly cap (Q-138, Q-P02e).
//!
//! Keys go to the OS keychain only. They reach the engine per session once
//! it advertises `session-secrets-v1`; until then cloud providers are
//! saved as "waiting for engine support" and no cloud call can happen
//! (Q-P09a). [`AiSavePlan`] is the pure decision of what the engine is
//! told; its tests prove that AI off, or a cloud provider without a key,
//! produces no request that could reach a model.

use cl_api::{
    AiConfig, AiConfigUpdate, AiTaskToggles, ApiError, Capabilities, CompanyConsent, Session,
    SessionSecrets,
};
use cl_core::Secret;
use cl_settings::ai::{ANTHROPIC_KEY, OLLAMA_DEFAULT_URL, OPENAI_KEY};
use cl_settings::{AiProvider, CompanyPrefs, SecretError, SecretStore};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::{Edits, Loaded, SaveBlock, SaveOutcome};
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;

/// Which secrets the keychain holds (read once when the window opens,
/// updated as the owner saves or removes them). Never the values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SecretsSaved {
    /// An Anthropic key is in the keychain.
    pub anthropic: bool,
    /// An `OpenAI` key is in the keychain.
    pub openai: bool,
    /// The SMTP password is in the keychain.
    pub smtp_password: bool,
}

impl SecretsSaved {
    /// Asks the keychain which entries exist.
    #[must_use]
    pub fn read(store: &dyn SecretStore) -> SecretsSaved {
        let has = |name: &str| {
            store
                .get(name)
                .ok()
                .flatten()
                .is_some_and(|s| !s.is_empty())
        };
        SecretsSaved {
            anthropic: has(ANTHROPIC_KEY),
            openai: has(OPENAI_KEY),
            smtp_password: has(cl_settings::smtp::SMTP_PASSWORD),
        }
    }

    /// Whether `provider`'s key is saved (always true for Ollama, which
    /// needs none).
    #[must_use]
    pub fn key(&self, provider: AiProvider) -> bool {
        match provider {
            AiProvider::Ollama => true,
            AiProvider::Anthropic => self.anthropic,
            AiProvider::OpenAi => self.openai,
        }
    }

    /// Records a key saved or removed.
    pub fn set_key(&mut self, provider: AiProvider, saved: bool) {
        match provider {
            AiProvider::Ollama => {}
            AiProvider::Anthropic => self.anthropic = saved,
            AiProvider::OpenAi => self.openai = saved,
        }
    }
}

/// What the AI panel edits.
#[derive(Clone, PartialEq, Eq)]
pub struct AiEdits {
    /// The big switch.
    pub enabled: bool,
    /// Where AI runs.
    pub provider: AiProvider,
    /// Ollama address.
    pub ollama_url: String,
    /// Ollama model.
    pub ollama_model: String,
    /// A key typed this session (cleared after it is saved).
    pub key_input: String,
    /// The owner ticked the cloud notice.
    pub consent_agreed: bool,
    /// Bank Feed suggestions (`categorization`).
    pub bank_feed: bool,
    /// Read bills and receipts (`receipt_ocr` + `document_classification`).
    pub read_documents: bool,
    /// Ask a question in Reports (`report_summary`).
    pub ask_reports: bool,
    /// Suggest rules (local, Q-141).
    pub suggest_rules: bool,
    /// Monthly token cap as typed (empty = no cap).
    pub cap_text: String,
}

impl std::fmt::Debug for AiEdits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiEdits")
            .field("enabled", &self.enabled)
            .field("provider", &self.provider)
            .field("key_typed", &!self.key_input.is_empty())
            .finish_non_exhaustive()
    }
}

impl Default for AiEdits {
    fn default() -> Self {
        AiEdits {
            enabled: false,
            provider: AiProvider::Ollama,
            ollama_url: OLLAMA_DEFAULT_URL.into(),
            ollama_model: "llama3.1:8b".into(),
            key_input: String::new(),
            consent_agreed: false,
            bank_feed: true,
            read_documents: true,
            ask_reports: true,
            suggest_rules: false,
            cap_text: cl_settings::ai::DEFAULT_MONTHLY_TOKEN_CAP.to_string(),
        }
    }
}

impl AiEdits {
    /// Starts from the company-local preferences.
    #[must_use]
    pub fn from_local(prefs: &CompanyPrefs) -> AiEdits {
        AiEdits {
            suggest_rules: prefs.ai.suggest_rules,
            cap_text: prefs
                .ai
                .monthly_token_cap
                .map(|c| c.to_string())
                .unwrap_or_default(),
            ..AiEdits::default()
        }
    }

    /// Overlays what the engine reported.
    pub fn apply_engine(&mut self, config: Option<&AiConfig>, consent: Option<&CompanyConsent>) {
        if let Some(c) = config {
            self.enabled = c.is_enabled;
            if let Some(p) = c
                .fallback_chain
                .first()
                .and_then(|p| AiProvider::from_wire(p))
            {
                self.provider = p;
            }
            if let Some(u) = c.ollama_base_url.as_deref().filter(|u| !u.is_empty()) {
                self.ollama_url = u.to_string();
            }
            if let Some(m) = c
                .categorization_model
                .as_deref()
                .filter(|m| !m.is_empty())
                .filter(|_| self.provider == AiProvider::Ollama)
            {
                self.ollama_model = m.to_string();
            }
        }
        if let Some(c) = consent {
            let tasks = c.enabled_tasks;
            self.bank_feed = tasks.categorization.unwrap_or(self.bank_feed);
            self.read_documents = tasks.receipt_ocr.unwrap_or(self.read_documents);
            self.ask_reports = tasks.report_summary.unwrap_or(self.ask_reports);
            self.consent_agreed = c.ai_enabled && c.accepted_at.is_some();
        }
    }

    /// The cap as a number (`None` = no cap).
    #[must_use]
    pub fn cap(&self) -> Option<u64> {
        let text = self.cap_text.trim().replace(',', "");
        if text.is_empty() {
            None
        } else {
            text.parse().ok()
        }
    }

    /// A key is typed or saved for the chosen provider.
    #[must_use]
    pub fn has_key(&self, saved: &SecretsSaved) -> bool {
        !self.provider.is_cloud() || saved.key(self.provider) || !self.key_input.trim().is_empty()
    }

    /// Checks the fields before a save.
    ///
    /// # Errors
    /// The first problem.
    pub fn validate(&self, saved: &SecretsSaved) -> Result<(), SaveBlock> {
        if !self.cap_text.trim().is_empty() && self.cap().is_none() {
            return Err(SaveBlock::BadNumber("cap"));
        }
        if self.enabled && self.provider.is_cloud() {
            if !self.has_key(saved) {
                return Err(SaveBlock::AiNeedsKey);
            }
            if !self.consent_agreed {
                return Err(SaveBlock::AiNeedsConsent);
            }
        }
        Ok(())
    }

    /// Saves a typed key to the keychain (nothing else ever stores it).
    ///
    /// # Errors
    /// The keychain's.
    pub fn store_secrets(
        &self,
        store: &dyn SecretStore,
        saved: &mut SecretsSaved,
    ) -> Result<(), SecretError> {
        let typed = self.key_input.trim();
        if typed.is_empty() {
            return Ok(());
        }
        if let Some(name) = self.provider.key_name() {
            store.set(name, &Secret::new(typed))?;
            saved.set_key(self.provider, true);
        }
        Ok(())
    }

    /// Forgets what was typed (after it was saved).
    pub fn clear_typed(&mut self) {
        self.key_input.clear();
    }

    /// Writes the local switches into the company preferences.
    pub fn write_local(&self, prefs: &mut CompanyPrefs) {
        prefs.ai.suggest_rules = self.suggest_rules;
        prefs.ai.monthly_token_cap = self.cap();
        if self.enabled && self.provider.is_cloud() && self.consent_agreed {
            let version = prefs
                .ai_consent
                .as_ref()
                .map_or(1, |c| c.disclosure_version);
            prefs.ai_consent = Some(cl_settings::AiConsentNote {
                provider: self.provider,
                disclosure_version: version,
                accepted_at: Some(cl_settings::recent::now()),
            });
        }
    }

    /// The config to push before a provider test (address and model).
    #[must_use]
    pub fn config_for_test(&self) -> Option<AiConfigUpdate> {
        match self.provider {
            AiProvider::Ollama => Some(AiConfigUpdate {
                ollama_base_url: Some(self.ollama_url.trim().to_string()),
                categorization_provider: Some("ollama".into()),
                categorization_model: Some(self.ollama_model.trim().to_string()),
                ..AiConfigUpdate::default()
            }),
            _ => None,
        }
    }

    /// What the engine gets for this session (keys from the keychain, the
    /// SMTP password likewise). Only for the chosen provider.
    #[must_use]
    pub fn session_secrets(
        &self,
        store: &dyn SecretStore,
        email: &super::email::EmailEdits,
    ) -> SessionSecrets {
        let key = |name: &str| store.get(name).ok().flatten().filter(|s| !s.is_empty());
        let mut s = SessionSecrets::default();
        if self.enabled {
            match self.provider {
                AiProvider::Anthropic => s.anthropic_key = key(ANTHROPIC_KEY),
                AiProvider::OpenAi => s.openai_key = key(OPENAI_KEY),
                AiProvider::Ollama => {
                    s.ollama = Some(cl_api::OllamaTarget {
                        base_url: self.ollama_url.trim().to_string(),
                        model: Some(self.ollama_model.trim().to_string()),
                    });
                }
            }
        }
        if email.form.host.trim().is_empty() {
            return s;
        }
        s.smtp_password = key(cl_settings::smtp::SMTP_PASSWORD);
        s
    }

    /// Control-channel view (no values).
    #[must_use]
    pub fn state(&self, saved: &SecretsSaved, caps: &Capabilities) -> Value {
        json!({
            "enabled": self.enabled,
            "provider": self.provider.wire(),
            "key_saved": saved.key(self.provider) && self.provider.is_cloud(),
            "key_typed": !self.key_input.is_empty(),
            "consent_agreed": self.consent_agreed,
            "bank_feed": self.bank_feed,
            "read_documents": self.read_documents,
            "ask_reports": self.ask_reports,
            "suggest_rules": self.suggest_rules,
            "cap": self.cap(),
            "waiting_engine": self.provider.is_cloud() && !caps.has(Capabilities::SESSION_SECRETS),
        })
    }
}

/// What the engine is told on save: nothing that could start a model call
/// unless AI is on, the provider has its key, and consent is recorded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AiSavePlan {
    /// Accept the system disclosure first (the engine refuses
    /// `isEnabled` before it).
    pub accept_system_disclosure: bool,
    /// The config write (never a key).
    pub config: Option<AiConfigUpdate>,
    /// Accept the company disclosure (audit row, Q-136).
    pub accept_company_consent: bool,
    /// Revoke the company consent (AI turned off).
    pub revoke_company_consent: bool,
    /// Task switches.
    pub tasks: Option<AiTaskToggles>,
    /// Push the session secrets (keys from the keychain).
    pub push_secrets: bool,
    /// Keys stay local: the engine has no session-secrets support.
    pub waiting_engine: bool,
}

impl AiSavePlan {
    /// Decides the plan.
    #[must_use]
    pub fn build(
        e: &AiEdits,
        saved: &SecretsSaved,
        engine: Option<&AiConfig>,
        consent: Option<&CompanyConsent>,
        caps: &Capabilities,
    ) -> AiSavePlan {
        let was_on = engine.is_some_and(|c| c.is_enabled);
        let disclosure_accepted = engine.is_some_and(|c| c.admin_disclosure_accepted_at.is_some());
        let company_consented = consent.is_some_and(|c| c.ai_enabled);
        if !e.enabled {
            return AiSavePlan {
                config: was_on.then(|| AiConfigUpdate {
                    is_enabled: Some(false),
                    ..AiConfigUpdate::default()
                }),
                revoke_company_consent: company_consented,
                ..AiSavePlan::default()
            };
        }
        let cloud = e.provider.is_cloud();
        if cloud && (!saved.key(e.provider) || !e.consent_agreed) {
            // Refused before this point by `validate`; never enable.
            return AiSavePlan::default();
        }
        let secrets_supported = caps.has(Capabilities::SESSION_SECRETS);
        let mut config = AiConfigUpdate::for_provider(e.provider.wire(), true);
        if e.provider == AiProvider::Ollama {
            config.ollama_base_url = Some(e.ollama_url.trim().to_string());
            config.categorization_model = Some(e.ollama_model.trim().to_string());
            config.ocr_model = Some(e.ollama_model.trim().to_string());
            config.document_classification_model = Some(e.ollama_model.trim().to_string());
        }
        AiSavePlan {
            accept_system_disclosure: !disclosure_accepted,
            config: Some(config),
            accept_company_consent: !company_consented,
            revoke_company_consent: false,
            tasks: Some(AiTaskToggles {
                categorization: Some(e.bank_feed),
                receipt_ocr: Some(e.read_documents),
                document_classification: Some(e.read_documents),
                statement_parsing: Some(e.read_documents),
                report_summary: Some(e.ask_reports),
                ..AiTaskToggles::default()
            }),
            push_secrets: secrets_supported,
            waiting_engine: cloud && !secrets_supported,
        }
    }

    /// True when nothing would be sent to the engine.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.accept_system_disclosure
            && self.config.is_none()
            && !self.accept_company_consent
            && !self.revoke_company_consent
            && self.tasks.is_none()
            && !self.push_secrets
    }

    /// Runs the plan in order on the session.
    ///
    /// # Errors
    /// The first engine error.
    pub async fn run(
        self,
        session: &Session,
        secrets: SessionSecrets,
    ) -> Result<SaveOutcome, cl_company::CompanyError> {
        let key = cl_api::IdempotencyKey::new;
        if self.accept_system_disclosure {
            session.accept_ai_disclosure(key()).await?;
        }
        if self.push_secrets && !secrets.is_empty() {
            session.put_session_secrets(&secrets).await?;
        }
        if let Some(c) = &self.config {
            session.update_ai_config(c, key()).await?;
        }
        if self.accept_company_consent {
            session.accept_company_ai_consent(key()).await?;
        }
        if let Some(tasks) = &self.tasks {
            session.set_ai_tasks(tasks, key()).await?;
        }
        if self.revoke_company_consent {
            match session.revoke_company_ai_consent(key()).await {
                Ok(()) | Err(ApiError::NotFound { .. }) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(SaveOutcome {
            ai_waiting_engine: self.waiting_engine,
        })
    }
}

/// What the panel asked the screen to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelAction {
    /// Nothing.
    None,
    /// Remove this provider's key from the keychain.
    RemoveKey(AiProvider),
    /// Test the chosen provider.
    Test,
}

/// A labelled on/off switch row (title, description, checkbox).
pub fn switch_row(ui: &mut Ui, tokens: &Tokens, value: &mut bool, title: &str, description: &str) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 60.0).max(120.0));
            ui.add(egui::Label::new(title).selectable(false));
            form::hint(ui, tokens, description);
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.checkbox(value, "");
            r.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::CheckBox, true, *value, title.to_string())
            });
        });
    });
}

/// Draws the AI panel.
pub fn panel(
    ui: &mut Ui,
    tokens: &Tokens,
    edits: &mut Edits,
    loaded: &Loaded,
    saved: &SecretsSaved,
    caps: &Capabilities,
    testing: bool,
) -> PanelAction {
    let mut action = PanelAction::None;
    let e = &mut edits.ai;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 90.0).max(200.0));
            form::heading(ui, tokens, &t("prefs.ai.title"));
            form::hint(ui, tokens, &t("prefs.ai.intro"));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = if e.enabled {
                t("prefs.on")
            } else {
                t("prefs.off")
            };
            let r = ui.checkbox(&mut e.enabled, label);
            r.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::Role::CheckBox,
                    true,
                    e.enabled,
                    t("prefs.ai.title"),
                )
            });
        });
    });
    ui.add_space(6.0);

    form::card(ui, tokens, |ui| {
        form::heading(ui, tokens, &t("prefs.ai.where"));
        ui.columns(3, |cols| {
            for (i, p) in AiProvider::ALL.iter().enumerate() {
                let desc = match p {
                    AiProvider::Ollama => "prefs.ai.ollama_desc",
                    AiProvider::Anthropic => "prefs.ai.anthropic_desc",
                    AiProvider::OpenAi => "prefs.ai.openai_desc",
                };
                if form::choice(
                    &mut cols[i],
                    tokens,
                    e.provider == *p,
                    &t(p.label_key()),
                    &t(desc),
                )
                .clicked()
                {
                    e.provider = *p;
                    e.key_input.clear();
                }
            }
        });
        ui.add_space(6.0);
        match e.provider {
            AiProvider::Ollama => {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() * 0.55).max(220.0));
                        form::field(ui, &t("prefs.ai.ollama_url"), &mut e.ollama_url);
                    });
                    form::field(ui, &t("prefs.ai.ollama_model"), &mut e.ollama_model);
                });
            }
            cloud => {
                // The key is a secret: masked on screen and in the tree.
                form::password(ui, &t("prefs.ai.key"), &mut e.key_input);
                let status = if saved.key(cloud) {
                    t("prefs.ai.key_saved")
                } else {
                    t("prefs.ai.key_missing")
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(status).color(tokens.text_secondary));
                    if saved.key(cloud) && ui.button(t("prefs.ai.key_remove")).clicked() {
                        action = PanelAction::RemoveKey(cloud);
                    }
                });
                if !caps.has(Capabilities::SESSION_SECRETS) {
                    form::hint(ui, tokens, &t("prefs.ai.waiting_engine"));
                }
            }
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if testing {
                form::progress_line(ui, tokens, &t("prefs.ai.testing"));
            } else if ui.button(t("prefs.ai.test")).clicked() {
                action = PanelAction::Test;
            }
        });
    });

    if e.provider.is_cloud() {
        ui.add_space(6.0);
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("prefs.ai.notice_title"));
            let text = loaded
                .consent
                .as_ref()
                .and_then(|c| c.text.clone())
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| t("prefs.ai.notice_text"));
            form::hint(ui, tokens, &text);
            form::hint(ui, tokens, &t("prefs.ai.notice_images"));
            ui.checkbox(&mut e.consent_agreed, t("prefs.ai.agree"));
            if let Some(c) = loaded.consent.as_ref().filter(|c| c.accepted_at.is_some()) {
                form::hint(
                    ui,
                    tokens,
                    &t_with(
                        "prefs.ai.recorded",
                        &[(
                            "when",
                            &c.accepted_at
                                .map(|t| t.as_utc().format("%m/%d/%Y").to_string())
                                .unwrap_or_default(),
                        )],
                    ),
                );
            }
        });
    }

    ui.add_space(6.0);
    form::card(ui, tokens, |ui| {
        form::heading(ui, tokens, &t("prefs.ai.features"));
        switch_row(
            ui,
            tokens,
            &mut e.bank_feed,
            &t("prefs.ai.feat_feed"),
            &t("prefs.ai.feat_feed_desc"),
        );
        switch_row(
            ui,
            tokens,
            &mut e.read_documents,
            &t("prefs.ai.feat_docs"),
            &t("prefs.ai.feat_docs_desc"),
        );
        switch_row(
            ui,
            tokens,
            &mut e.ask_reports,
            &t("prefs.ai.feat_ask"),
            &t("prefs.ai.feat_ask_desc"),
        );
        switch_row(
            ui,
            tokens,
            &mut e.suggest_rules,
            &t("prefs.ai.feat_rules"),
            &t("prefs.ai.feat_rules_desc"),
        );
    });

    ui.add_space(6.0);
    form::card(ui, tokens, |ui| {
        form::heading(ui, tokens, &t("prefs.ai.usage"));
        let month = {
            let now = cl_settings::recent::now().as_utc();
            cl_settings::ai::month_key(chrono::Datelike::year(&now), chrono::Datelike::month(&now))
        };
        let used = edits.company.ai.used_in(&month);
        let cap = e.cap();
        let percent = cap.map_or(0, |c| {
            used.min(c)
                .saturating_mul(100)
                .checked_div(c)
                .map_or(100, |p| u8::try_from(p).unwrap_or(100))
        });
        ui.horizontal(|ui| {
            ui.add_sized(
                [220.0, 8.0],
                egui::ProgressBar::new(f32::from(percent) / 100.0)
                    .fill(tokens.accent)
                    .corner_radius(egui::CornerRadius::same(4)),
            );
            let text = match cap {
                Some(c) => t_with(
                    "prefs.ai.usage_of",
                    &[("used", &group_digits(used)), ("cap", &group_digits(c))],
                ),
                None => t_with("prefs.ai.usage_nocap", &[("used", &group_digits(used))]),
            };
            ui.label(RichText::new(text).monospace());
            ui.add_space(12.0);
            let l = ui.add(egui::Label::new(t("prefs.ai.cap")).selectable(false));
            ui.add_sized([110.0, 24.0], egui::TextEdit::singleline(&mut e.cap_text))
                .labelled_by(l.id);
        });
        if let Some(u) = loaded.usage.as_ref() {
            let est = u
                .total_cost
                .as_ref()
                .and_then(|c| c.display(2))
                .unwrap_or_else(|| "0.00".into());
            form::hint(
                ui,
                tokens,
                &t_with(
                    "prefs.ai.estimate",
                    &[("calls", &u.total_calls.to_string()), ("dollars", &est)],
                ),
            );
        }
        form::hint(ui, tokens, &t("prefs.ai.usage_note"));
    });
    action
}

/// `1234567` → `1,234,567`.
#[must_use]
pub fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_settings::MemorySecrets;

    fn caps(list: &[&str]) -> Capabilities {
        list.iter().copied().collect()
    }

    fn on_engine(enabled: bool) -> AiConfig {
        AiConfig {
            is_enabled: enabled,
            admin_disclosure_accepted_at: enabled
                .then(|| cl_core::Timestamp::parse_wire("2026-10-01T00:00:00.000Z").expect("ts")),
            ..AiConfig::default()
        }
    }

    #[test]
    fn ai_off_sends_nothing_that_could_reach_a_model() {
        // A loopback listener stands in for "the internet": nothing may
        // connect to it while the plan is built and inspected.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let e = AiEdits::default();
        let plan = AiSavePlan::build(&e, &SecretsSaved::default(), None, None, &caps(&[]));
        assert!(plan.is_empty(), "{plan:?}");
        // Turning off an engine that was on: only the off switch and the
        // consent revoke, never secrets or an enable.
        let plan = AiSavePlan::build(
            &e,
            &SecretsSaved::default(),
            Some(&on_engine(true)),
            None,
            &caps(&[Capabilities::SESSION_SECRETS]),
        );
        assert_eq!(plan.config.as_ref().and_then(|c| c.is_enabled), Some(false));
        assert!(!plan.push_secrets && !plan.accept_company_consent && plan.tasks.is_none());
        assert!(listener.accept().is_err(), "no connection was made");
    }

    #[test]
    fn a_keyless_cloud_provider_is_never_enabled() {
        let mut e = AiEdits {
            enabled: true,
            provider: AiProvider::Anthropic,
            consent_agreed: true,
            ..AiEdits::default()
        };
        let none = SecretsSaved::default();
        assert_eq!(e.validate(&none), Err(SaveBlock::AiNeedsKey));
        let plan = AiSavePlan::build(
            &e,
            &none,
            None,
            None,
            &caps(&[Capabilities::SESSION_SECRETS]),
        );
        assert!(plan.is_empty(), "{plan:?}");
        e.consent_agreed = false;
        e.key_input = "sk-ant-typed".into();
        assert_eq!(e.validate(&none), Err(SaveBlock::AiNeedsConsent));
        let saved = SecretsSaved {
            anthropic: true,
            ..SecretsSaved::default()
        };
        let plan = AiSavePlan::build(&e, &saved, None, None, &caps(&[]));
        assert!(plan.is_empty(), "no consent, no enable: {plan:?}");
    }

    #[test]
    fn a_cloud_provider_with_key_and_consent_plans_the_full_sequence() {
        let e = AiEdits {
            enabled: true,
            provider: AiProvider::OpenAi,
            consent_agreed: true,
            ask_reports: false,
            ..AiEdits::default()
        };
        let saved = SecretsSaved {
            openai: true,
            ..SecretsSaved::default()
        };
        let plan = AiSavePlan::build(&e, &saved, None, None, &caps(&[]));
        assert!(plan.accept_system_disclosure && plan.accept_company_consent);
        let c = plan.config.as_ref().expect("config");
        assert_eq!(c.fallback_chain, Some(vec!["openai".to_string()]));
        assert_eq!(c.is_enabled, Some(true));
        assert!(
            !plan.push_secrets && plan.waiting_engine,
            "no capability: keys stay local"
        );
        assert_eq!(plan.tasks.and_then(|t| t.report_summary), Some(false));
        let with = AiSavePlan::build(
            &e,
            &saved,
            Some(&on_engine(true)),
            None,
            &caps(&[Capabilities::SESSION_SECRETS]),
        );
        assert!(with.push_secrets && !with.waiting_engine && !with.accept_system_disclosure);
        // Ollama needs neither key nor the notice and carries its address.
        let o = AiEdits {
            enabled: true,
            ..AiEdits::default()
        };
        assert_eq!(o.validate(&SecretsSaved::default()), Ok(()));
        let plan = AiSavePlan::build(&o, &SecretsSaved::default(), None, None, &caps(&[]));
        let c = plan.config.expect("config");
        assert_eq!(c.ollama_base_url.as_deref(), Some(OLLAMA_DEFAULT_URL));
        assert!(!plan.waiting_engine);
    }

    #[test]
    fn keys_go_to_the_keychain_and_never_into_debug_or_state() {
        let store = MemorySecrets::new();
        let mut saved = SecretsSaved::read(&store);
        assert!(!saved.anthropic);
        let mut e = AiEdits {
            provider: AiProvider::Anthropic,
            key_input: " sk-ant-canary-77 ".into(),
            ..AiEdits::default()
        };
        e.store_secrets(&store, &mut saved).expect("store");
        assert!(saved.anthropic);
        assert_eq!(
            store
                .get(ANTHROPIC_KEY)
                .expect("get")
                .expect("saved")
                .expose(),
            "sk-ant-canary-77"
        );
        assert!(!format!("{e:?}").contains("canary"));
        assert!(!e.state(&saved, &caps(&[])).to_string().contains("canary"));
        assert_eq!(e.state(&saved, &caps(&[]))["key_saved"], true);
        e.clear_typed();
        assert_eq!(e.key_input, "");
        let email = super::super::email::EmailEdits::default();
        e.enabled = true;
        let s = e.session_secrets(&store, &email);
        assert!(s.anthropic_key.is_some() && s.smtp_password.is_none());
        assert!(!format!("{s:?}").contains("canary"));
        let mut prefs = CompanyPrefs::default();
        e.consent_agreed = true;
        e.cap_text = "250,000".into();
        e.write_local(&mut prefs);
        assert_eq!(prefs.ai.monthly_token_cap, Some(250_000));
        assert_eq!(
            prefs.ai_consent.as_ref().map(|c| c.provider),
            Some(AiProvider::Anthropic)
        );
        assert_eq!(group_digits(1_234_567), "1,234,567");
        assert_eq!(group_digits(12), "12");
    }

    #[test]
    fn engine_values_overlay_the_defaults() {
        let mut e = AiEdits::default();
        let cfg = AiConfig {
            is_enabled: true,
            fallback_chain: vec!["anthropic".into()],
            ollama_base_url: Some("http://10.0.0.5:11434".into()),
            ..AiConfig::default()
        };
        e.apply_engine(Some(&cfg), None);
        assert!(e.enabled);
        assert_eq!(e.provider, AiProvider::Anthropic);
        assert_eq!(e.ollama_url, "http://10.0.0.5:11434");
        assert_eq!(e.cap(), Some(cl_settings::ai::DEFAULT_MONTHLY_TOKEN_CAP));
        e.cap_text = "x".into();
        assert_eq!(
            e.validate(&SecretsSaved::default()),
            Err(SaveBlock::BadNumber("cap"))
        );
        e.cap_text.clear();
        assert_eq!(e.cap(), None);
    }
}
