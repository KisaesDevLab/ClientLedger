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
use std::time::Instant;

use cl_api::{AiGate, AiPreviewRow, Capabilities, RuleQuery};
use cl_core::Money;
use cl_core::ids::{AccountId, ContactId};
use cl_grid::RowKey;
use eframe::egui::{self, RichText, Ui};

use super::WriteCheckScreen;
use crate::company::Task;
use crate::forms::lines_grid::{L_AMOUNT, L_MEMO};
// Shared with Bulk Entry (P15A): lifted into `forms::suggest`, no change in
// behaviour.
use crate::forms::suggest::clip;
pub(crate) use crate::forms::suggest::{Asking, Basis, Suggested, TIMEOUT, gate_of};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;

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

fn signature(payee: ContactId, memo: &str, amount: Option<Money>) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    payee.hash(&mut h);
    memo.trim().hash(&mut h);
    amount.map(Money::to_wire).hash(&mut h);
    h.finish()
}

impl WriteCheckScreen {
    /// The AI may be asked (demo: its switches; an engine: the monthly cap
    /// first, the rest is read with the request). When the local store
    /// can't be read the cap is unknown, so a cloud call is not made.
    pub(crate) fn under_cap(&self) -> bool {
        crate::forms::suggest::under_cap(&self.link, self.tenant.as_ref().map(|(t, _)| *t))
    }

    /// The demo's AI gate (a local model; consent given in the demo).
    fn demo_gate(&self) -> AiGate {
        crate::forms::suggest::demo_gate(&self.link)
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
        let Some(payee) = self.header.current().payee else {
            return;
        };
        let tenant = self.tenant.as_ref().map(|(t, _)| *t);
        if let Some(offer) =
            crate::forms::suggest::count_accept(&self.link, tenant, payee, account, &s.basis)
        {
            self.suggest.offer = Some(offer);
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
                .lists
                .account_text(*account)
                .unwrap_or_default()
                .to_string();
            let caption = match &s.basis {
                Basis::Rule(rule) => t_with("check.suggest.rule", &[("rule", rule)]),
                Basis::Ai(Some(basis)) => t_with("check.suggest.ai_basis", &[("basis", basis)]),
                Basis::Ai(None) => t("check.suggest.ai"),
                Basis::Usual(_) => t("batch_entry.suggest.usual"),
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
                .lists
                .account_text(account)
                .unwrap_or_default()
                .to_string();
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
