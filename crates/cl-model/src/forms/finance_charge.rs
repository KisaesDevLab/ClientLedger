//! Assess Finance Charges' form state (P22 D3): the assessment date, the
//! engine's preview of it, which clients are ticked and the assess plan.
//! No egui and no money arithmetic: every charge, the minimum, the period
//! and what is skipped come from the engine; a ticked client's
//! `expectedCharge` is a byte copy of the preview's charge (409
//! `FINANCE_CHARGE_CHANGED` when it moved).
//!
//! Chargeable rows start ticked, except clients with open credits (Q-P22s:
//! "Apply them first, or tick to charge anyway"). Already-assessed and
//! skipped rows can't be ticked.

use std::collections::BTreeSet;
use std::fmt;

use cl_core::ids::ContactId;
use cl_core::{CalendarDate, Money};

use crate::text::TextKey;

/// Clients per assess call (the engine's limit).
pub const ASSESS_CHUNK: usize = 500;

/// One client row of the preview.
#[derive(Clone, PartialEq, Eq)]
pub struct ChargeRowFigure {
    /// The client.
    pub client: ContactId,
    /// The charge, as the engine wrote it.
    pub charge: Money,
    /// The engine would charge it: above zero, not already assessed this
    /// month, not skipped.
    pub chargeable: bool,
    /// The client has open credits (the hint; starts unticked).
    pub has_open_credits: bool,
}

impl fmt::Debug for ChargeRowFigure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChargeRowFigure")
            .field("client", &self.client)
            .field("chargeable", &self.chargeable)
            .field("has_open_credits", &self.has_open_credits)
            .finish_non_exhaustive()
    }
}

/// What the engine's finance-charge preview answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChargeFigures {
    /// The month (`YYYY-MM`).
    pub period: String,
    /// One per client.
    pub rows: Vec<ChargeRowFigure>,
    /// `RATE_NOT_SET` or `INCOME_ACCOUNT_NOT_SET`: show the setup.
    pub not_set_up: bool,
}

/// One assess call: each ticked client with the preview's charge.
#[derive(Clone, PartialEq, Eq)]
pub struct AssessChunk {
    /// Its place in the plan.
    pub index: usize,
    /// The save attempt (the UI keys each chunk as (attempt, index)).
    pub attempt: u64,
    /// (client, expected charge).
    pub clients: Vec<(ContactId, Money)>,
}

impl fmt::Debug for AssessChunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssessChunk")
            .field("index", &self.index)
            .field("attempt", &self.attempt)
            .field("clients", &self.clients.len())
            .finish()
    }
}

/// The assessment to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssessPlan {
    /// The assessment date.
    pub date: CalendarDate,
    /// The calls.
    pub chunks: Vec<AssessChunk>,
}

/// Why Assess can't go yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FinanceChargeProblem {
    /// No assessment date.
    NoDate,
    /// The rate or income account is not set: the setup first (Q-P22a).
    NotSetUp,
    /// The preview is being recalculated.
    PreviewPending,
    /// Nothing is ticked.
    NothingTicked,
}

impl FinanceChargeProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            FinanceChargeProblem::NoDate => TextKey("finance_charges.problem.no_date"),
            FinanceChargeProblem::NotSetUp => TextKey("finance_charges.problem.not_set_up"),
            FinanceChargeProblem::PreviewPending => TextKey("finance_charges.problem.updating"),
            FinanceChargeProblem::NothingTicked => {
                TextKey("finance_charges.problem.nothing_ticked")
            }
        }
    }
}

/// The Assess Finance Charges window.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct FinanceChargeForm {
    date: Option<CalendarDate>,
    figures: Option<(CalendarDate, ChargeFigures)>,
    ticked: BTreeSet<ContactId>,
    attempt: u64,
}

impl fmt::Debug for FinanceChargeForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FinanceChargeForm")
            .field("date", &self.date)
            .field("rows", &self.figures.as_ref().map(|(_, f)| f.rows.len()))
            .field("ticked", &self.ticked.len())
            .finish_non_exhaustive()
    }
}

impl FinanceChargeForm {
    /// Assess on `date` (today by default).
    #[must_use]
    pub fn new(date: Option<CalendarDate>) -> FinanceChargeForm {
        FinanceChargeForm {
            date,
            ..FinanceChargeForm::default()
        }
    }

    /// The assessment date; the figures of another date are dropped.
    pub fn set_date(&mut self, date: Option<CalendarDate>) {
        if self.date != date {
            self.date = date;
            self.figures = None;
            self.ticked.clear();
            self.attempt += 1;
        }
    }

    /// The assessment date.
    #[must_use]
    pub fn date(&self) -> Option<CalendarDate> {
        self.date
    }

    /// Stores the engine's preview of `date` and ticks the chargeable rows
    /// without open credits. An answer for another date is dropped
    /// (false). A re-preview (after 409 CHANGED) re-ticks.
    pub fn apply_preview(&mut self, date: CalendarDate, figures: ChargeFigures) -> bool {
        if self.date != Some(date) {
            return false;
        }
        self.ticked = figures
            .rows
            .iter()
            .filter(|r| r.chargeable && !r.has_open_credits)
            .map(|r| r.client)
            .collect();
        self.figures = Some((date, figures));
        self.attempt += 1;
        true
    }

    /// The figures for the date shown.
    #[must_use]
    pub fn figures(&self) -> Option<&ChargeFigures> {
        let d = self.date?;
        self.figures
            .as_ref()
            .filter(|(fd, _)| *fd == d)
            .map(|(_, f)| f)
    }

    /// Ticks or unticks a chargeable client (others can't be ticked).
    pub fn toggle(&mut self, client: ContactId) {
        let chargeable = self
            .figures()
            .and_then(|f| f.rows.iter().find(|r| r.client == client))
            .is_some_and(|r| r.chargeable);
        if !chargeable {
            return;
        }
        if !self.ticked.remove(&client) {
            self.ticked.insert(client);
        }
        self.attempt += 1;
    }

    /// True when `client` is ticked.
    #[must_use]
    pub fn is_ticked(&self, client: ContactId) -> bool {
        self.ticked.contains(&client)
    }

    /// The assessment: ticked clients in preview order with their charge
    /// strings, at most [`ASSESS_CHUNK`] per call.
    ///
    /// # Errors
    /// One [`FinanceChargeProblem`].
    pub fn assess_plan(&self) -> Result<AssessPlan, FinanceChargeProblem> {
        let date = self.date.ok_or(FinanceChargeProblem::NoDate)?;
        let f = self.figures().ok_or(FinanceChargeProblem::PreviewPending)?;
        if f.not_set_up {
            return Err(FinanceChargeProblem::NotSetUp);
        }
        let picked: Vec<(ContactId, Money)> = f
            .rows
            .iter()
            .filter(|r| r.chargeable && self.ticked.contains(&r.client))
            .map(|r| (r.client, r.charge))
            .collect();
        if picked.is_empty() {
            return Err(FinanceChargeProblem::NothingTicked);
        }
        Ok(AssessPlan {
            date,
            chunks: picked
                .chunks(ASSESS_CHUNK)
                .enumerate()
                .map(|(index, c)| AssessChunk {
                    index,
                    attempt: self.attempt,
                    clients: c.to_vec(),
                })
                .collect(),
        })
    }
}

#[cfg(test)]
#[path = "finance_charge_tests.rs"]
mod tests;
