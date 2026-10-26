//! The AI amount gate (WP-A, step 11; plan D5 "AI amount gate", S18,
//! Q-A03j, non-negotiable 4). Runs in the desktop **before any
//! `DocReading` exists**: an AI amount, quantity or unit price, header or
//! line, is kept only when a page word parses to an equal value, and then
//! takes that word's spot; otherwise it is reported as an `AiDisagrees`
//! check and never used. Other AI values stay suggestions the owner must
//! accept. An Ollama answer that agrees raises confidence; one that
//! disagrees adds a check.
//!
//! **Consumer:** the desktop optional readers (WP-G, step 21); the
//! signatures here are fixed.

use crate::model::{Confidence, Reading};
use cl_core::{CalendarDate, Money, Quantity};
use std::fmt;

/// Which optional reader answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiSource {
    /// A local Ollama model.
    Ollama,
    /// The company's cloud provider, with the owner's consent (Q-A03k).
    Cloud,
}

/// One AI line item; amounts already parsed from the engine's strings.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct AiLine {
    pub description: Option<String>,
    pub quantity: Option<Quantity>,
    pub unit_price: Option<Money>,
    pub amount: Option<Money>,
}

/// What an optional reader returned (the engine's `BillOcrResult` shape,
/// `ai-bill-ocr.service.ts:243-272`).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct AiReading {
    pub vendor: Option<String>,
    pub reference: Option<String>,
    pub date: Option<CalendarDate>,
    pub due_date: Option<CalendarDate>,
    pub subtotal: Option<Money>,
    pub tax: Option<Money>,
    pub total: Option<Money>,
    pub amount_paid: Option<Money>,
    pub lines: Vec<AiLine>,
    /// The reader's overall confidence.
    pub confidence: Confidence,
}

impl fmt::Debug for AiReading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiReading")
            .field("lines", &self.lines.len())
            .field("confidence", &self.confidence)
            .finish_non_exhaustive()
    }
}

/// Merge an optional reader's answer into the local reading under the
/// gate. Stub (step 2): the reading unchanged.
#[must_use]
pub fn merge(reading: Reading, ai: &AiReading, source: AiSource) -> Reading {
    let _ = (ai, source);
    reading
}
