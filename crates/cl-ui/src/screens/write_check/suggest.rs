//! Account suggestions for a line without an account (plan WP7.3).
//!
//! Once the payee is set and the owner leaves a line's Memo or Amount with
//! no account chosen, the screen asks, in order:
//!
//! 1. the owner's rules (`rules-evaluate-v1`; offline, read-only), then
//! 2. the AI preview, only when AI is on, the company agreed to the AI
//!    notice, the account-suggestion task is on, and the provider is the
//!    local model or the monthly cap allows a cloud call ([`AiGate`]). With
//!    any switch off nothing is sent (non-negotiable 3).
//!
//! The answer is an account only (non-negotiable 4): the AI reply is never
//! read for an amount, and the amount sent is the owner's own typed one.
//! It shows as a ghost value in the empty Account cell, on a `warn.tint`
//! row with a `warn.text` caption; **Tab or Accept sets it**, anything else
//! leaves it. A line with only a ghost has no account, so saving it is
//! refused as "Choose an account for each line".
//!
//! One request runs per line, given up after 5 seconds; a failure is
//! silent (a hint only). Accepts are counted per (payee, account, source)
//! in a small local file (Q-141); enough repeats offer a rule when the
//! owner turned on Suggest rules.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use cl_api::{AiConfig, AiGate, AiPreviewRow, Capabilities, CompanyConsent, RuleQuery};
use cl_core::Money;
use cl_core::ids::{AccountId, ContactId};
use cl_grid::RowKey;
use cl_settings::{AiProvider, CapCheck};
use eframe::egui::{self, RichText, Ui};

use super::WriteCheckScreen;
use crate::company::Task;
use crate::forms::lines_grid::{L_AMOUNT, L_MEMO};
use crate::i18n::{t, t_with};
use crate::prefs::SuggestionAccepts;
use crate::theme::Tokens;

/// A suggestion request is given up after this long.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(5);
/// Accepts of the same suggestion before a rule is offered (Q-141).
pub(crate) const OFFER_AFTER: u32 = 3;
/// The longest text sent (the AI preview's limit; rules take more).
const DESCRIPTION_MAX: usize = cl_api::AI_PREVIEW_DESCRIPTION_MAX_CHARS;

/// Why an account is suggested.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Basis {
    /// The owner's rule, by name.
    Rule(String),
    /// The AI, with what it based it on.
    Ai(Option<String>),
}

impl Basis {
    pub(crate) fn source(&self) -> &'static str {
        match self {
            Basis::Rule(_) => "rule",
            Basis::Ai(_) => "ai",
        }
    }
}

/// One suggested account (never an amount).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Suggested {
    pub account: AccountId,
    pub basis: Basis,
}

/// Ids only: rule names and the AI's basis may name a vendor.
impl std::fmt::Debug for Suggested {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Suggested")
            .field("account", &self.account)
            .field("source", &self.basis.source())
            .finish()
    }
}

/// One line's request in flight.
pub(crate) struct Asking {
    pub key: RowKey,
    pub started: Instant,
    pub task: Task<Option<Suggested>>,
}

/// Suggestions on this check.
#[derive(Default)]
pub(crate) struct SuggestState {
    /// What each line was last asked about (payee, memo and amount hashed).
    pub asked: BTreeMap<RowKey, u64>,
    pub asking: Vec<Asking>,
    /// The suggestions shown, by line.
    pub shown: BTreeMap<RowKey, Suggested>,
    /// Suggestions accepted on this check (control channel).
    pub accepted: u32,
    /// A plain hint when a request failed or took too long (i18n key).
    pub hint: Option<&'static str>,
    /// A rule offer for the payee (Q-141): (payee, account).
    pub offer: Option<(ContactId, AccountId)>,
}

impl SuggestState {
    pub(crate) fn busy(&self) -> bool {
        !self.asking.is_empty()
    }
}

/// Whether the company's AI consent is for the current notice.
fn consent_current(c: &CompanyConsent) -> bool {
    c.ai_enabled
        && c.accepted_at.is_some()
        && !c.is_stale
        && c.accepted_version.is_some()
        && c.accepted_version == c.system_version
}

/// The AI switches as the engine reports them, with the local monthly cap
/// for a cloud provider. Every provider the engine may use is checked: the
/// whole fallback chain and the categorization provider. A cloud provider
/// anywhere needs the cap to allow the call; a provider this version does
/// not know closes the gate.
#[must_use]
pub(crate) fn gate_of(
    config: Option<&AiConfig>,
    consent: Option<&CompanyConsent>,
    under_cap: bool,
) -> AiGate {
    let named: Vec<&str> = config
        .map(|c| {
            c.fallback_chain
                .iter()
                .map(String::as_str)
                .chain(
                    c.categorization_provider
                        .as_deref()
                        .filter(|p| !p.is_empty()),
                )
                .collect()
        })
        .unwrap_or_default();
    let providers: Option<Vec<AiProvider>> =
        named.iter().map(|p| AiProvider::from_wire(p)).collect();
    let reachable = match providers {
        Some(p) if !p.is_empty() => p.iter().all(|p| !p.is_cloud()) || under_cap,
        _ => false,
    };
    AiGate {
        ai_enabled: config.is_some_and(|c| c.is_enabled),
        consented: consent.is_some_and(consent_current),
        categorization_on: consent.is_some_and(|c| c.enabled_tasks.categorization == Some(true)),
        provider_reachable: reachable,
    }
}

fn signature(payee: ContactId, memo: &str, amount: Option<Money>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    payee.hash(&mut h);
    memo.trim().hash(&mut h);
    amount.map(Money::to_wire).hash(&mut h);
    h.finish()
}

fn clip(s: &str) -> String {
    s.chars().take(DESCRIPTION_MAX).collect()
}

impl WriteCheckScreen {
    /// The AI may be asked (demo: its switches; an engine: the monthly cap
    /// first, the rest is read with the request). When the local store
    /// can't be read the cap is unknown, so a cloud call is not made.
    pub(crate) fn under_cap(&self) -> bool {
        let Some((tenant, _)) = &self.tenant else {
            return true;
        };
        let month = crate::screens::preferences::ai::this_month();
        let Ok(list) = self.link.store().load() else {
            return false;
        };
        list.get(*tenant)
            .map(|r| r.prefs.ai.check(&month))
            .is_none_or(|c| matches!(c, CapCheck::Allowed))
    }

    /// The demo's AI gate (a local model; consent given in the demo).
    fn demo_gate(&self) -> AiGate {
        let ai = self.link.demo_state().map(|d| d.ai).unwrap_or_default();
        AiGate {
            ai_enabled: ai.enabled,
            consented: ai.enabled,
            categorization_on: ai.categorization,
            provider_reachable: true,
        }
    }

    /// Lines the owner left with a memo or an amount and no account: ask.
    pub(crate) fn maybe_suggest(&mut self) {
        let Some(payee) = self.header.current().payee else {
            return;
        };
        if self.editing.as_ref().is_some_and(|e| e.read_only) || self.save.task.is_some() {
            return;
        }
        let cursor = self
            .lines
            .has_focus()
            .then(|| self.lines.cursor())
            .flatten();
        let lines = self.lines.value().lines.clone();
        for l in lines {
            if l.account.is_some() || (l.memo.trim().is_empty() && l.amount.is_none()) {
                continue;
            }
            if cursor.is_some_and(|c| c.row == l.key && (c.col == L_MEMO || c.col == L_AMOUNT)) {
                continue;
            }
            let sig = signature(payee, &l.memo, l.amount);
            if self.suggest.asked.get(&l.key) == Some(&sig)
                || self.suggest.asking.iter().any(|a| a.key == l.key)
            {
                continue;
            }
            self.suggest.asked.insert(l.key, sig);
            self.suggest.shown.remove(&l.key);
            self.lines.set_suggestion(l.key, None);
            self.ask(l.key, payee, &l.memo, l.amount);
        }
    }

    fn ask(&mut self, key: RowKey, payee: ContactId, memo: &str, amount: Option<Money>) {
        let Some(bank) = self.bank() else { return };
        let header = self.header.current();
        let payee_name = self.lists.name_text(payee).unwrap_or_default().to_string();
        let text = if memo.trim().is_empty() {
            header.memo.trim().to_string()
        } else {
            memo.trim().to_string()
        };
        // The owner's typed amount, money out (the bank-feed sign).
        let typed = amount.or(header.amount).unwrap_or(Money::ZERO);
        let signed = typed.checked_neg().unwrap_or(typed);
        let rule = RuleQuery {
            description: clip(&text),
            amount: signed,
            contact: Some(payee),
            bank_account: bank,
            date: header.date,
        };
        let ai_row = AiPreviewRow {
            description: clip(format!("{payee_name} {text}").trim()),
            amount: signed,
        };
        let started = Instant::now();
        let link = std::sync::Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let mut found =
                crate::company::demo::checks::evaluate_rule(&mut demo, &rule).and_then(|m| {
                    Some(Suggested {
                        account: m.account_id?,
                        basis: Basis::Rule(m.rule_name),
                    })
                });
            drop(demo);
            let gate = self.demo_gate();
            if found.is_none() && gate.is_open() {
                let mut demo = self.link.demo_state();
                found = demo
                    .as_deref_mut()
                    .and_then(|d| crate::company::demo::checks::ai_preview(d, &[ai_row]).ok())
                    .and_then(|rows| rows.into_iter().find(|r| r.index == 0))
                    .and_then(|r| {
                        Some(Suggested {
                            account: r.account_id?,
                            basis: Basis::Ai(r.basis),
                        })
                    });
            }
            self.suggest.asking.push(Asking {
                key,
                started,
                task: Task::ready(found),
            });
            self.poll_suggest();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let rules = self.link.capabilities().has(Capabilities::RULES_EVALUATE);
        let under_cap = self.under_cap();
        if let Ok(p) = group.read(&session, move |s| async move {
            if rules
                && let Ok(Some(m)) = s.rules().evaluate(&rule).await
                && let Some(account) = m.account_id
            {
                return Ok(Some(Suggested {
                    account,
                    basis: Basis::Rule(m.rule_name),
                }));
            }
            let config = s.ai_config().await.ok();
            let consent = s.company_ai_disclosure().await.ok();
            let gate = gate_of(config.as_ref(), consent.as_ref(), under_cap);
            if !gate.is_open() {
                // Every switch must be on: nothing is sent otherwise.
                return Ok(None);
            }
            let rows = s.ai_categorize_preview(gate, &[ai_row]).await?;
            Ok(rows.into_iter().find(|r| r.index == 0).and_then(|r| {
                Some(Suggested {
                    account: r.account_id?,
                    basis: Basis::Ai(r.basis),
                })
            }))
        }) {
            self.suggest.asking.push(Asking {
                key,
                started,
                task: Task::Pending(p),
            });
        }
    }

    /// Answers in, slow ones given up, accepts counted.
    pub(crate) fn poll_suggest(&mut self) {
        let mut done = Vec::new();
        let mut i = 0;
        while i < self.suggest.asking.len() {
            let a = &mut self.suggest.asking[i];
            if let Some(r) = a.task.try_take() {
                done.push((a.key, r));
                self.suggest.asking.remove(i);
            } else if a.started.elapsed() > TIMEOUT {
                self.suggest.asking.remove(i);
                self.suggest.hint = Some("check.suggest.slow");
            } else {
                i += 1;
            }
        }
        for (key, r) in done {
            match r {
                Ok(Some(s)) => {
                    let open = self
                        .lines
                        .value()
                        .lines
                        .iter()
                        .any(|l| l.key == key && l.account.is_none());
                    if open
                        && self
                            .accounts
                            .iter()
                            .any(|a| a.id == s.account && a.is_active)
                    {
                        self.lines.set_suggestion(key, Some(s.account));
                        self.suggest.shown.insert(key, s);
                    }
                }
                Ok(None) => {}
                Err(_) => self.suggest.hint = Some("check.suggest.failed"),
            }
        }
        for (key, account) in self.lines.take_accepted() {
            self.count_accept(key, account);
        }
    }

    /// The owner accepted a suggestion: count it (Q-141).
    fn count_accept(&mut self, key: RowKey, account: AccountId) {
        let Some(s) = self.suggest.shown.remove(&key) else {
            return;
        };
        self.lines.set_suggestion(key, None);
        self.suggest.accepted += 1;
        let (Some(payee), Some((tenant, _))) = (self.header.current().payee, &self.tenant) else {
            return;
        };
        let Some(dir) = self
            .link
            .store()
            .path()
            .parent()
            .map(std::path::Path::to_path_buf)
        else {
            return;
        };
        let mut book = SuggestionAccepts::load(&dir);
        let n = book.bump(&SuggestionAccepts::key(
            &tenant.to_string(),
            &payee.to_string(),
            &account.to_string(),
            s.basis.source(),
        ));
        if book.save(&dir).is_err() {
            tracing::debug!("the suggestion count was not written");
        }
        let offer_on = self
            .link
            .store()
            .load()
            .ok()
            .and_then(|l| l.get(*tenant).map(|r| r.prefs.ai.suggest_rules))
            .unwrap_or(false);
        if offer_on && n >= OFFER_AFTER {
            self.suggest.offer = Some((payee, account));
        }
    }

    /// Accept clicked for line `key`.
    pub(crate) fn accept_suggestion(&mut self, key: RowKey) {
        if self.lines.accept_suggestion(key) {
            self.poll_suggest();
        }
    }

    /// Forget every suggestion (a new or another check).
    pub(crate) fn reset_suggestions(&mut self) {
        self.suggest = super::suggest::SuggestState::default();
        self.lines.clear_suggestions();
    }

    /// The captions under the lines: what suggests the ghost account, with
    /// Tab and Accept.
    pub(crate) fn suggestions_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let open = self.lines.open_suggestions();
        let mut accept = None;
        for (key, account) in &open {
            let Some(s) = self.suggest.shown.get(key) else {
                continue;
            };
            let name = self
                .accounts
                .iter()
                .find(|a| a.id == *account)
                .map(crate::screens::register::account_text)
                .unwrap_or_default();
            let caption = match &s.basis {
                Basis::Rule(rule) => t_with("check.suggest.rule", &[("rule", rule)]),
                Basis::Ai(Some(basis)) => t_with("check.suggest.ai_basis", &[("basis", basis)]),
                Basis::Ai(None) => t("check.suggest.ai"),
            };
            egui::Frame::new()
                .fill(tokens.warn_tint)
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&name).strong().color(tokens.warn_text));
                        ui.label(RichText::new(caption).color(tokens.warn_text));
                        crate::widgets::kbd(ui, "Tab");
                        ui.label(
                            RichText::new(t("check.suggest.to_accept")).color(tokens.warn_text),
                        );
                        if ui.button(t("check.suggest.accept")).clicked() {
                            accept = Some(*key);
                        }
                    });
                });
            ui.add_space(4.0);
        }
        if let Some(key) = accept {
            self.accept_suggestion(key);
        }
        if let Some((payee, account)) = self.suggest.offer {
            let payee = self.lists.name_text(payee).unwrap_or_default().to_string();
            let account = self
                .accounts
                .iter()
                .find(|a| a.id == account)
                .map(crate::screens::register::account_text)
                .unwrap_or_default();
            let mut close = false;
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(t_with(
                        "check.suggest.offer",
                        &[("payee", &payee), ("account", &account)],
                    ))
                    .color(tokens.text_secondary),
                );
                close = ui.button(t("check.suggest.offer.ok")).clicked();
            });
            if close {
                self.suggest.offer = None;
            }
        }
    }

    /// The suggestions' control-channel state (flags and counts only).
    pub(crate) fn suggest_json(&self) -> serde_json::Value {
        let open = self.lines.open_suggestions();
        let source = |src: &str| {
            open.iter().any(|(k, _)| {
                self.suggest
                    .shown
                    .get(k)
                    .is_some_and(|s| s.basis.source() == src)
            })
        };
        serde_json::json!({
            "chip": source("ai"),
            "rule": source("rule"),
            "open": open.len(),
            "asking": self.suggest.asking.len(),
            "accepted": self.suggest.accepted,
            "offer": self.suggest.offer.is_some(),
        })
    }
}
