//! Account suggestions every form shares (P13 Write Check, lifted for P15A
//! Bulk Entry): why an account is suggested ([`Basis`]), one suggestion
//! ([`Suggested`]), a request in flight ([`Asking`]), the AI gate
//! ([`gate_of`], [`demo_gate`], [`under_cap`]) and the accept count that
//! offers a rule (Q-141, [`count_accept`]).
//!
//! A suggestion is an account only (non-negotiable 4): never an amount. It
//! shows as a ghost value until the owner accepts it with Tab or Accept.
//! The AI is asked only when every switch is on (Q-73); otherwise nothing
//! is sent (non-negotiable 3).

use std::time::{Duration, Instant};

use cl_api::{AiConfig, AiGate, CompanyConsent, SuggestSource};
use cl_core::ids::{AccountId, ContactId, TenantId};
use cl_grid::RowKey;
use cl_settings::{AiProvider, CapCheck};

use crate::company::{CompanyLink, Task};
use crate::prefs::SuggestionAccepts;

/// A suggestion request is given up after this long.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(5);
/// Accepts of the same suggestion before a rule is offered (Q-141).
pub(crate) const OFFER_AFTER: u32 = 3;
/// The longest text sent (the AI preview's limit; rules take more).
pub(crate) const DESCRIPTION_MAX: usize = cl_api::AI_PREVIEW_DESCRIPTION_MAX_CHARS;

/// Why an account is suggested.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Basis {
    /// The owner's rule, by name.
    Rule(String),
    /// The AI, with what it based it on.
    Ai(Option<String>),
    /// The engine's own lookup for the name: its default account, or the
    /// account it was used with last (offline).
    Usual(SuggestSource),
}

impl Basis {
    pub(crate) fn source(&self) -> &'static str {
        match self {
            Basis::Rule(_) => "rule",
            Basis::Ai(_) => "ai",
            Basis::Usual(_) => "usual",
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

/// The demo's AI gate (a local model; consent given in the demo).
#[must_use]
pub(crate) fn demo_gate(link: &CompanyLink) -> AiGate {
    let ai = link.demo_state().map(|d| d.ai).unwrap_or_default();
    AiGate {
        ai_enabled: ai.enabled,
        consented: ai.enabled,
        categorization_on: ai.categorization,
        provider_reachable: true,
    }
}

/// The AI may be asked (an engine: the monthly cap first, the rest is
/// read with the request). When the local store can't be read the cap is
/// unknown, so a cloud call is not made.
#[must_use]
pub(crate) fn under_cap(link: &CompanyLink, tenant: Option<TenantId>) -> bool {
    let Some(tenant) = tenant else {
        return true;
    };
    let month = crate::screens::preferences::ai::this_month();
    let Ok(list) = link.store().load() else {
        return false;
    };
    list.get(tenant)
        .map(|r| r.prefs.ai.check(&month))
        .is_none_or(|c| matches!(c, CapCheck::Allowed))
}

/// The text sent, cut to the AI preview's limit.
#[must_use]
pub(crate) fn clip(s: &str) -> String {
    s.chars().take(DESCRIPTION_MAX).collect()
}

/// Counts an accepted suggestion (Q-141) in the small local file. Returns
/// the rule offer (payee, account) when the owner turned on Suggest rules
/// and the same suggestion was accepted often enough.
pub(crate) fn count_accept(
    link: &CompanyLink,
    tenant: Option<TenantId>,
    payee: ContactId,
    account: AccountId,
    basis: &Basis,
) -> Option<(ContactId, AccountId)> {
    let tenant = tenant?;
    let dir = link
        .store()
        .path()
        .parent()
        .map(std::path::Path::to_path_buf)?;
    let mut book = SuggestionAccepts::load(&dir);
    let n = book.bump(&SuggestionAccepts::key(
        &tenant.to_string(),
        &payee.to_string(),
        &account.to_string(),
        basis.source(),
    ));
    if book.save(&dir).is_err() {
        tracing::debug!("the suggestion count was not written");
    }
    let offer_on = link
        .store()
        .load()
        .ok()
        .and_then(|l| l.get(tenant).map(|r| r.prefs.ai.suggest_rules))
        .unwrap_or(false);
    (offer_on && n >= OFFER_AFTER).then_some((payee, account))
}
