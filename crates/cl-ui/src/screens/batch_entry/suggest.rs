//! Account suggestions on Bulk Entry rows (plan WP-D2), with the pieces
//! Write Check uses (`forms::suggest`). For a row with a name picked from
//! the list and no account, in order:
//!
//! 1. the engine's own lookup for the name (its default account, then the
//!    account it was used with last; offline);
//! 2. the owner's rules (`rules-evaluate-v1`; offline, read-only);
//! 3. the AI, only when every switch is on (`AiGate`, Q-73).
//!
//! The suggestion is an account only, never an amount (non-negotiable 4).
//! It shows as a ghost value in the empty Account cell; **Tab or Accept
//! sets it**, and "Accept all shown suggestions" asks first with the
//! count. A row with only a ghost has no account, so it can't be saved.
//! Accepts are counted (Q-141): enough repeats offer a rule.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use cl_api::{AiPreviewRow, Capabilities, RuleQuery};
use cl_core::Money;
use cl_core::ids::{AccountId, ContactId};
use cl_grid::RowKey;
use cl_model::forms::batch::SectionKind;
use eframe::egui::{self, RichText, Ui};

use super::BulkEntryScreen;
use crate::company::Task;
use crate::forms::suggest::{self, Asking, Basis, Suggested, TIMEOUT};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;

/// Suggestions in this window.
#[derive(Default)]
pub(crate) struct BatchSuggest {
    /// The name each row was last asked about.
    pub asked: BTreeMap<RowKey, ContactId>,
    pub asking: Vec<Asking>,
    /// The suggestions shown, by row.
    pub shown: BTreeMap<RowKey, Suggested>,
    /// Suggestions accepted (control channel).
    pub accepted: u32,
    /// A plain hint when a request failed or took too long (i18n key).
    pub hint: Option<&'static str>,
    /// A rule offer (Q-141): (name, account).
    pub offer: Option<(ContactId, AccountId)>,
    /// "Accept all shown suggestions" is asking for confirmation.
    pub confirm_all: bool,
}

/// What the engine's lookup, the rules and the AI suggest for one row.
struct Ask {
    contact: ContactId,
    rule: Option<RuleQuery>,
    ai: AiPreviewRow,
}

impl BulkEntryScreen {
    /// Rows with a picked name and no account get asked once per name.
    pub(crate) fn maybe_suggest(&mut self) {
        if self.busy() {
            return;
        }
        let mut wanted = Vec::new();
        for s in &self.batch().sections {
            for r in s.filled() {
                let Some(c) = r.contact else { continue };
                if r.has_account()
                    || self.suggest.asked.get(&r.key) == Some(&c)
                    || self.suggest.asking.iter().any(|a| a.key == r.key)
                {
                    continue;
                }
                wanted.push((
                    r.key,
                    c,
                    s.kind,
                    s.context,
                    r.memo.clone(),
                    r.amount,
                    r.date,
                ));
            }
        }
        for (key, contact, kind, context, memo, amount, date) in wanted {
            self.suggest.asked.insert(key, contact);
            self.suggest.shown.remove(&key);
            self.grid.adapter_mut().suggested.remove(&key);
            let name = self
                .books
                .lists
                .name_text(contact)
                .unwrap_or_default()
                .to_string();
            // The owner's typed amount with the bank-feed sign (money out
            // negative); zero when not typed yet.
            let typed = amount.unwrap_or(Money::ZERO);
            let signed = if kind == SectionKind::Deposits {
                typed
            } else {
                typed.checked_neg().unwrap_or(typed)
            };
            let rule = context.map(|bank| RuleQuery {
                description: suggest::clip(memo.trim()),
                amount: signed,
                contact: Some(contact),
                bank_account: bank,
                date,
            });
            let ai = AiPreviewRow {
                description: suggest::clip(format!("{name} {}", memo.trim()).trim()),
                amount: signed,
            };
            self.ask(key, Ask { contact, rule, ai });
        }
    }

    fn ask(&mut self, key: RowKey, q: Ask) {
        let started = Instant::now();
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            let mut found = local
                .suggest_account(&link, q.contact)
                .ok()
                .flatten()
                .map(|s| Suggested {
                    account: s.account,
                    basis: Basis::Usual(s.source),
                });
            if found.is_none()
                && let Some(rule) = &q.rule
                && let Some(mut demo) = link.demo_state()
            {
                found =
                    crate::company::demo::checks::evaluate_rule(&mut demo, rule).and_then(|m| {
                        Some(Suggested {
                            account: m.account_id?,
                            basis: Basis::Rule(m.rule_name),
                        })
                    });
            }
            if found.is_none() && suggest::demo_gate(&link).is_open() {
                let mut demo = link.demo_state();
                found = demo
                    .as_deref_mut()
                    .and_then(|d| crate::company::demo::checks::ai_preview(d, &[q.ai]).ok())
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
        let under_cap = suggest::under_cap(&self.link, self.tenant.as_ref().map(|(t, _)| *t));
        let Ask { contact, rule, ai } = q;
        if let Ok(p) = group.read(&session, move |s| async move {
            if let Ok(Some(found)) = s.batch().suggest_account(contact).await {
                return Ok(Some(Suggested {
                    account: found.account,
                    basis: Basis::Usual(found.source),
                }));
            }
            if rules
                && let Some(rule) = &rule
                && let Ok(Some(m)) = s.rules().evaluate(rule).await
                && let Some(account) = m.account_id
            {
                return Ok(Some(Suggested {
                    account,
                    basis: Basis::Rule(m.rule_name),
                }));
            }
            let config = s.ai_config().await.ok();
            let consent = s.company_ai_disclosure().await.ok();
            let gate = suggest::gate_of(config.as_ref(), consent.as_ref(), under_cap);
            if !gate.is_open() {
                // Every switch must be on: nothing is sent otherwise.
                return Ok(None);
            }
            let rows = s.ai_categorize_preview(gate, &[ai]).await?;
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
                        .batch()
                        .find(key)
                        .is_some_and(|(_, r)| !r.has_account());
                    let active = self
                        .books
                        .accounts
                        .iter()
                        .any(|a| a.id == s.account && a.is_active);
                    if open && active {
                        self.grid.adapter_mut().suggested.insert(key, s.account);
                        self.suggest.shown.insert(key, s);
                    }
                }
                Ok(None) => {}
                Err(_) => self.suggest.hint = Some("check.suggest.failed"),
            }
        }
        let accepted = std::mem::take(&mut self.grid.adapter_mut().accepted);
        for (key, account) in accepted {
            self.count_accept(key, account);
        }
        // A row that got an account another way drops its ghost.
        let stale: Vec<RowKey> = self
            .grid
            .adapter()
            .suggested
            .keys()
            .copied()
            .filter(|k| self.batch().find(*k).is_none_or(|(_, r)| r.has_account()))
            .collect();
        for k in stale {
            self.grid.adapter_mut().suggested.remove(&k);
            self.suggest.shown.remove(&k);
        }
    }

    /// The owner accepted a suggestion: count it (Q-141).
    fn count_accept(&mut self, key: RowKey, account: AccountId) {
        let Some(s) = self.suggest.shown.remove(&key) else {
            return;
        };
        self.grid.adapter_mut().suggested.remove(&key);
        self.suggest.accepted += 1;
        let Some(contact) = self.batch().find(key).and_then(|(_, r)| r.contact) else {
            return;
        };
        let tenant = self.tenant.as_ref().map(|(t, _)| *t);
        if let Some(offer) = suggest::count_accept(&self.link, tenant, contact, account, &s.basis) {
            self.suggest.offer = Some(offer);
        }
    }

    /// The suggestions waiting for the owner: rows without an account that
    /// have one, in strip order.
    #[must_use]
    pub(crate) fn open_suggestions(&self) -> Vec<(RowKey, AccountId)> {
        let shown = &self.grid.adapter().suggested;
        self.batch()
            .sections
            .iter()
            .flat_map(|s| s.rows.iter())
            .filter(|r| !r.has_account())
            .filter_map(|r| shown.get(&r.key).map(|a| (r.key, *a)))
            .collect()
    }

    /// Accept one suggestion (one undo step), or every shown one.
    pub(crate) fn accept_suggestions(&mut self, only: Option<RowKey>) -> usize {
        let take: Vec<(RowKey, AccountId)> = self
            .open_suggestions()
            .into_iter()
            .filter(|(k, _)| only.is_none_or(|o| o == *k))
            .collect();
        if take.is_empty() || self.busy() {
            return 0;
        }
        let applied = self.grid.edit_step(|b| {
            for s in &mut b.sections {
                for r in &mut s.rows {
                    if let Some((_, a)) = take.iter().find(|(k, _)| *k == r.key)
                        && !r.has_account()
                    {
                        r.account = Some(*a);
                    }
                }
            }
        });
        if !applied {
            return 0;
        }
        for (k, a) in &take {
            self.count_accept(*k, *a);
        }
        self.last_action = Some("accept_suggestions");
        take.len()
    }

    /// Forget every suggestion (a new batch).
    pub(crate) fn reset_suggestions(&mut self) {
        self.suggest = BatchSuggest::default();
        self.grid.adapter_mut().suggested.clear();
        self.grid.adapter_mut().accepted.clear();
    }

    /// The caption under a row with a ghost account: what suggests it.
    #[must_use]
    pub(crate) fn suggestion_caption(&self, key: RowKey) -> Option<String> {
        let s = self.suggest.shown.get(&key)?;
        if self.batch().find(key).is_none_or(|(_, r)| r.has_account()) {
            return None;
        }
        let why = match &s.basis {
            Basis::Rule(rule) => t_with("check.suggest.rule", &[("rule", rule)]),
            Basis::Ai(Some(basis)) => t_with("check.suggest.ai_basis", &[("basis", basis)]),
            Basis::Ai(None) => t("check.suggest.ai"),
            Basis::Usual(_) => t("batch_entry.suggest.usual"),
        };
        Some(t_with("batch_entry.suggest.caption", &[("why", &why)]))
    }

    /// "Accept all shown suggestions" (asks first, with the count), the
    /// rule offer and the hint.
    pub(crate) fn suggestions_ui(&mut self, ui: &mut Ui, tokens: &Tokens, enabled: bool) {
        let open = self.open_suggestions().len();
        let mut accept_all = false;
        if open > 0 {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(t_with(
                        "batch_entry.suggest.open",
                        &[("n", &open.to_string())],
                    ))
                    .color(tokens.warn_text),
                );
                crate::widgets::kbd(ui, "Tab");
                ui.label(RichText::new(t("check.suggest.to_accept")).color(tokens.warn_text));
                if ui
                    .add_enabled(
                        enabled,
                        egui::Button::new(t("batch_entry.suggest.accept_all")),
                    )
                    .clicked()
                {
                    self.suggest.confirm_all = true;
                }
            });
            if self.suggest.confirm_all {
                ui.horizontal_wrapped(|ui| {
                    ui.label(t_with(
                        "batch_entry.suggest.confirm",
                        &[("n", &open.to_string())],
                    ));
                    accept_all = ui.button(t("batch_entry.suggest.confirm.yes")).clicked();
                    if ui.button(t("batch_entry.suggest.confirm.no")).clicked() {
                        self.suggest.confirm_all = false;
                    }
                });
            }
        } else {
            self.suggest.confirm_all = false;
        }
        if accept_all {
            self.suggest.confirm_all = false;
            self.accept_suggestions(None);
        }
        if let Some(k) = self.suggest.hint {
            crate::screens::company::form::hint(ui, tokens, &t(k));
        }
        if let Some((payee, account)) = self.suggest.offer {
            let payee = self
                .books
                .lists
                .name_text(payee)
                .unwrap_or_default()
                .to_string();
            let account = self
                .books
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

    /// The suggestions' control-channel state (counts only).
    pub(crate) fn suggest_json(&self) -> serde_json::Value {
        serde_json::json!({
            "open": self.open_suggestions().len(),
            "asking": self.suggest.asking.len(),
            "accepted": self.suggest.accepted,
            "offer": self.suggest.offer.is_some(),
            "confirm_all": self.suggest.confirm_all,
        })
    }
}
