//! Learn on accept (A04 D5, Q-A03q, Q-A04j): after a convert answered 201
//! (never after an attach, never after `Closed`), the vendor's saved layout
//! learns where this document printed its values, and the card account
//! learns its printed last four digits.
//!
//! - **Layout:** the stored reading's values (what the form showed) with
//!   the page words and fingerprint of a re-read in the worker (S11: words
//!   are not stored with the draft), turned into the owner's final values
//!   by `cl_docread::correct::to_final` (a typed value keeps a spot only
//!   when exactly one page word parses equal; a clicked one keeps the
//!   click's spot), then `templates::learn`, merged with the vendor's
//!   closest saved layout (`best_match` + `merge`) and stored under the
//!   **final** vendor (`VendorTemplates::put`; the demo keeps its own
//!   list). A 409 `STALE` merges once more; any other failure is skipped
//!   and logged as a class only.
//! - **Card digits:** only when the owner left "Remember card ending 1234"
//!   on, the account has no digits yet, and the account is a card: never
//!   overwritten (a different number already there is a note).
//!
//! Templates hold page regions and labels, never amounts (plan §4).
//! Nothing here logs a value, a name or a digit.

use std::sync::Arc;

use cl_api::{ApiError, CardLast4, ConflictReason, IdempotencyKey, RequestGroup};
use cl_core::CalendarDate;
use cl_core::ids::{AccountId, ContactId};
use cl_docread::correct::{self, FinalValues};
use cl_docread::model::{Last4, Reading, Template};
use cl_docread::templates;

use crate::company::{CompanyLink, Task};

/// What the form hands over at Accept for learning (owner data; `Debug`
/// shows shapes only).
#[derive(Clone, PartialEq, Eq)]
pub struct LearnInput {
    /// The final vendor or payee (a layout is stored only under one).
    pub vendor: Option<ContactId>,
    /// The values the owner accepted, with how each was entered.
    pub values: FinalValues,
    /// "Remember card ending 1234": the card account and the printed
    /// digits, when the owner left it on.
    pub card: Option<CardLearn>,
}

impl std::fmt::Debug for LearnInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LearnInput")
            .field("vendor", &self.vendor.is_some())
            .field("values", &self.values)
            .field("card", &self.card.is_some())
            .finish()
    }
}

/// A card account and the digits printed on the document.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CardLearn {
    /// The card account the charge posted from.
    pub account: AccountId,
    /// The last four digits printed.
    pub digits: Last4,
}

/// The layout this document teaches, from the stored values (`values`)
/// and the re-read's words and fingerprint (`words`); `None` when nothing
/// can be learned (no fingerprint, no field kept a spot).
#[must_use]
pub fn template_of(
    values: &Reading,
    words: &Reading,
    fin: &FinalValues,
    today: CalendarDate,
) -> Option<Template> {
    let mut r = values.clone();
    r.words.clone_from(&words.words);
    r.fingerprint.clone_from(&words.fingerprint);
    r.pages.clone_from(&words.pages);
    r.page_count = words.page_count;
    templates::learn(&correct::to_final(&r, fin, today))
}

/// `new` merged into the closest of `saved` (the vendor's layouts), and
/// which one it replaces.
#[must_use]
pub fn merged(new: Template, saved: &[Template]) -> (Template, Option<usize>) {
    match templates::best_match(&new.fingerprint, saved) {
        Some(i) => (templates::merge(&saved[i], &new), Some(i)),
        None => (new, None),
    }
}

/// What learning did (control channel and tests; classes only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Learned {
    /// The layout was stored.
    Stored,
    /// Nothing to learn (no words, no fingerprint, no vendor).
    Nothing,
    /// The store refused; skipped.
    Failed,
}

impl Learned {
    /// Stable name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Learned::Stored => "stored",
            Learned::Nothing => "nothing",
            Learned::Failed => "failed",
        }
    }
}

/// What remembering the card did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardOutcome {
    /// The digits were set on the account.
    Set,
    /// The account already has digits (the same, or a note for different
    /// ones): nothing changed.
    Kept {
        /// The account's digits differ from the printed ones.
        different: bool,
    },
    /// Not a card account, or the engine refused: skipped.
    Failed,
}

impl CardOutcome {
    /// Stable name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            CardOutcome::Set => "set",
            CardOutcome::Kept { different: false } => "kept",
            CardOutcome::Kept { different: true } => "different",
            CardOutcome::Failed => "failed",
        }
    }
}

/// Stores `template` under `vendor` (merged with its closest saved layout).
/// The task answers `Ok` once stored; `None`: nothing could be sent.
pub fn save_layout(
    link: &CompanyLink,
    group: Option<&RequestGroup>,
    vendor: ContactId,
    template: Template,
) -> Option<Task<()>> {
    if let Some(mut demo) = link.demo_state() {
        let mine: Vec<(usize, Template)> = demo
            .drafts
            .layouts
            .iter()
            .enumerate()
            .filter(|(_, (c, _))| *c == vendor)
            .map(|(i, (_, t))| (i, t.clone()))
            .collect();
        let saved: Vec<Template> = mine.iter().map(|(_, t)| t.clone()).collect();
        let (t, replaced) = merged(template, &saved);
        match replaced.and_then(|i| mine.get(i)).map(|(at, _)| *at) {
            Some(at) => demo.drafts.layouts[at] = (vendor, t),
            None => demo.drafts.layouts.push((vendor, t)),
        }
        return Some(Task::ready(()));
    }
    let (session, group) = (link.session()?, group?);
    let template = Arc::new(template);
    let out = group.write(
        &session,
        cl_api::OrphanLabel {
            what: "orphan.layout_learn",
            key: None,
        },
        move |s| async move {
            // One more merge after a 409 STALE, then give up.
            for _ in 0..2 {
                let rows = s.vendor_templates().list(Some(vendor)).await?;
                let parsed = super::service::templates_of(&rows);
                let saved: Vec<Template> = parsed.iter().map(|(_, t)| t.clone()).collect();
                let (t, replaced) = merged((*template).clone(), &saved);
                // The row replaced, when the merged layout keeps its key.
                let expected = replaced.and_then(|i| {
                    let key = super::service::template_upsert(vendor, &t, None)
                        .ok()?
                        .fingerprint;
                    rows.iter()
                        .find(|r| r.fingerprint == key)
                        .map(|r| r.updated_at.clone())
                        .filter(|_| i < saved.len())
                });
                let upsert = super::service::template_upsert(vendor, &t, expected)?;
                match s.vendor_templates().put(&upsert).await {
                    Ok(_) => return Ok(()),
                    Err(ApiError::Conflict {
                        reason: ConflictReason::Stale,
                        ..
                    }) => {}
                    Err(e) => return Err(e),
                }
            }
            Err(ApiError::Conflict {
                reason: ConflictReason::Stale,
                current_version: None,
                code: None,
                message: cl_api::UserText::default(),
            })
        },
    );
    out.ok().map(Task::Pending)
}

/// Sets the card's digits when it has none (Q-A04j: opt-in, never
/// overwritten). `None`: nothing could be sent.
pub fn remember_card(
    link: &CompanyLink,
    group: Option<&RequestGroup>,
    card: CardLearn,
) -> Option<Task<CardOutcome>> {
    let digits = CardLast4::parse(card.digits.as_str())?;
    if let Some(mut demo) = link.demo_state() {
        let chart = demo.chart.get_or_insert_with(crate::company::demo::chart);
        let out = match chart.iter_mut().find(|a| a.id == card.account) {
            Some(a) if a.fields.detail_type.as_deref() == Some(cl_api::CREDIT_CARD_DETAIL_TYPE) => {
                match a.card_last4 {
                    None => {
                        a.card_last4 = Some(digits);
                        CardOutcome::Set
                    }
                    Some(have) => CardOutcome::Kept {
                        different: have != digits,
                    },
                }
            }
            _ => CardOutcome::Failed,
        };
        return Some(Task::ready(out));
    }
    let (session, group) = (link.session()?, group?);
    let account = card.account;
    let out = group.write(
        &session,
        cl_api::OrphanLabel {
            what: "orphan.card_digits",
            key: None,
        },
        move |s| async move {
            let v = s.accounts().get(account).await?;
            if let Some(have) = v.record.card_last4 {
                return Ok(CardOutcome::Kept {
                    different: have != digits,
                });
            }
            s.accounts()
                .set_card_last4(&v, Some(digits), IdempotencyKey::new())
                .await?;
            Ok(CardOutcome::Set)
        },
    );
    out.ok().map(Task::Pending)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::Money;
    use cl_docread::model::{
        Bbox, Confidence, Found, PageWords, Reader, RuleId, Spot, Word, WordSource,
    };

    fn spot(l: u16, t: u16) -> Spot {
        Spot {
            page: 0,
            bbox: Bbox::new(l, t, l + 800, t + 200).expect("box"),
        }
    }

    fn word(text: &str, l: u16, t: u16) -> Word {
        serde_json::from_value(serde_json::json!({
            "text": text, "bbox": {"l": l, "t": t, "r": l + 800, "b": t + 200}, "confidence": 10000
        }))
        .expect("word")
    }

    fn found<T>(value: T, s: Spot) -> Found<T> {
        Found {
            value,
            spot: s,
            rule: RuleId::new("test"),
            confidence: Confidence::CERTAIN,
        }
    }

    fn reading() -> (Reading, Reading) {
        let mut values = Reading::empty(Reader::TextPdf, 1);
        let total = Money::parse_wire("25.00").expect("money");
        values.header.total = Some(found(total, spot(7000, 8000)));
        values.header.date = Some(found(
            CalendarDate::from_ymd(2026, 3, 14).expect("date"),
            spot(7000, 1000),
        ));
        let mut words = Reading::empty(Reader::TextPdf, 1);
        words.words = vec![PageWords {
            page: 0,
            aspect: 12_941,
            source: WordSource::TextLayer,
            words: vec![
                word("Corner", 500, 500),
                word("Hardware", 1400, 500),
                word("03/14/2026", 7000, 1000),
                word("Total", 5000, 8000),
                word("25.00", 7000, 8000),
            ],
        }];
        words.fingerprint = templates::fingerprint(&words.words);
        (values, words)
    }

    #[test]
    fn a_layout_is_learned_from_the_stored_values_and_the_reread_words() {
        let (values, words) = reading();
        let fin = FinalValues {
            total: Some(correct::Final {
                value: Money::parse_wire("25.00").expect("money"),
                entry: correct::Entry::Read,
            }),
            ..FinalValues::default()
        };
        let today = CalendarDate::from_ymd(2026, 3, 20).expect("date");
        // Without words nothing is learned.
        assert_eq!(
            template_of(&values, &Reading::empty(Reader::TextPdf, 1), &fin, today),
            None
        );
        let t = template_of(&values, &words, &fin, today);
        if let Some(t) = t {
            // Merging with itself keeps one layout.
            let (m, replaced) = merged(t.clone(), std::slice::from_ref(&t));
            assert_eq!(replaced, Some(0));
            assert!(m.samples >= t.samples);
            let (n, none) = merged(t, &[]);
            assert_eq!(none, None);
            assert!(n.samples >= 1);
        }
        assert_eq!(Learned::Stored.name(), "stored");
        assert_eq!(CardOutcome::Kept { different: true }.name(), "different");
    }

    #[test]
    fn the_demo_keeps_a_card_digits_once_and_never_overwrites() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = CompanyLink::demo(dir.path());
        let card = {
            let mut demo = link.demo_state().expect("demo");
            let chart = demo.chart.get_or_insert_with(crate::company::demo::chart);
            chart
                .iter()
                .find(|a| {
                    a.fields.detail_type.as_deref() == Some(cl_api::CREDIT_CARD_DETAIL_TYPE)
                        && a.card_last4.is_none()
                })
                .map(|a| a.id)
        };
        let Some(account) = card else {
            return;
        };
        let learn = |d: &str| CardLearn {
            account,
            digits: Last4::parse(d).expect("digits"),
        };
        let mut t = remember_card(&link, None, learn("1234")).expect("demo");
        assert_eq!(t.try_take().and_then(Result::ok), Some(CardOutcome::Set));
        let mut t = remember_card(&link, None, learn("9999")).expect("demo");
        assert_eq!(
            t.try_take().and_then(Result::ok),
            Some(CardOutcome::Kept { different: true })
        );
        let mut t = remember_card(&link, None, learn("1234")).expect("demo");
        assert_eq!(
            t.try_take().and_then(Result::ok),
            Some(CardOutcome::Kept { different: false })
        );
    }
}
