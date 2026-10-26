//! Label words (plan D3 `fields.rs`): canonical tokens and the label table.
//!
//! A word becomes one or two canonical tokens: lower case, outer
//! punctuation dropped, `no`/`number`/`#` → `#`, `invoice#` → `invoice`
//! `#`, `sub-total` → `subtotal`. Labels are matched on those tokens,
//! longest first, anywhere in a segment.

use crate::model::Word;

/// What a label introduces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Label {
    /// The document total; the number is the priority (0 = strongest).
    Total(u8),
    /// "Amount due": the total when no total is printed, else the balance.
    AmountDue,
    Balance,
    Paid,
    Subtotal,
    Tax,
    Fee,
    Date(u8),
    Due,
    Reference(u8),
    Terms,
    Account,
    Phone,
    /// "Bill to", "Ship to": the customer's block, never the vendor.
    Party,
    Change,
    CheckNumber,
    /// A card brand ("Visa 54.82" is the amount paid by card).
    Card,
}

impl Label {
    /// Whether the value may be the next segment on the line.
    #[must_use]
    pub fn value_may_be_next_segment(self) -> bool {
        !matches!(self, Label::Reference(2) | Label::Party)
    }

    /// Whether the value may be on the line below the label.
    #[must_use]
    pub fn value_may_be_below(self) -> bool {
        matches!(
            self,
            Label::Date(_)
                | Label::Due
                | Label::Reference(0 | 1)
                | Label::Terms
                | Label::Account
                | Label::Total(_)
                | Label::AmountDue
                | Label::Balance
        )
    }

    /// Labels whose value is an amount.
    #[must_use]
    pub fn is_amount(self) -> bool {
        matches!(
            self,
            Label::Total(_)
                | Label::AmountDue
                | Label::Balance
                | Label::Paid
                | Label::Subtotal
                | Label::Tax
                | Label::Fee
                | Label::Change
                | Label::Card
        )
    }
}

const TABLE: &[(&[&str], Label)] = &[
    (&["total"], Label::Total(0)),
    (&["grand", "total"], Label::Total(0)),
    (&["invoice", "total"], Label::Total(0)),
    (&["total", "amount"], Label::Total(0)),
    (&["total", "due"], Label::Total(0)),
    (&["total", "amount", "due"], Label::Total(0)),
    (&["order", "total"], Label::Total(0)),
    (&["receipt", "total"], Label::Total(0)),
    (&["total", "sale"], Label::Total(0)),
    (&["amount", "due"], Label::AmountDue),
    (&["please", "pay"], Label::AmountDue),
    (&["pay", "this", "amount"], Label::AmountDue),
    (&["balance", "due"], Label::Balance),
    (&["balance"], Label::Balance),
    (&["remaining", "balance"], Label::Balance),
    (&["amount", "paid"], Label::Paid),
    (&["paid"], Label::Paid),
    (&["payment"], Label::Paid),
    (&["payments"], Label::Paid),
    (&["payment", "received"], Label::Paid),
    (&["less", "payment"], Label::Paid),
    (&["less", "payments"], Label::Paid),
    (&["payments/credits"], Label::Paid),
    (&["subtotal"], Label::Subtotal),
    (&["sub", "total"], Label::Subtotal),
    (&["tax"], Label::Tax),
    (&["sales", "tax"], Label::Tax),
    (&["tax", "total"], Label::Tax),
    (&["total", "tax"], Label::Tax),
    (&["tax", "amount"], Label::Tax),
    (&["state", "tax"], Label::Tax),
    (&["shipping"], Label::Fee),
    (&["shipping", "&", "handling"], Label::Fee),
    (&["shipping", "and", "handling"], Label::Fee),
    (&["s&h"], Label::Fee),
    (&["freight"], Label::Fee),
    (&["delivery"], Label::Fee),
    (&["delivery", "fee"], Label::Fee),
    (&["handling"], Label::Fee),
    (&["surcharge"], Label::Fee),
    (&["fuel", "surcharge"], Label::Fee),
    (&["service", "fee"], Label::Fee),
    (&["service", "charge"], Label::Fee),
    (&["convenience", "fee"], Label::Fee),
    (&["processing", "fee"], Label::Fee),
    (&["invoice", "date"], Label::Date(0)),
    (&["bill", "date"], Label::Date(0)),
    (&["receipt", "date"], Label::Date(0)),
    (&["issue", "date"], Label::Date(0)),
    (&["date", "issued"], Label::Date(0)),
    (&["date", "of", "issue"], Label::Date(0)),
    (&["transaction", "date"], Label::Date(0)),
    (&["date"], Label::Date(1)),
    (&["order", "date"], Label::Date(2)),
    (&["due", "date"], Label::Due),
    (&["date", "due"], Label::Due),
    (&["payment", "due"], Label::Due),
    (&["due"], Label::Due),
    (&["pay", "by"], Label::Due),
    (&["invoice", "#"], Label::Reference(0)),
    (&["inv", "#"], Label::Reference(0)),
    (&["bill", "#"], Label::Reference(0)),
    (&["receipt", "#"], Label::Reference(0)),
    (&["reference", "#"], Label::Reference(0)),
    (&["reference"], Label::Reference(0)),
    (&["ref", "#"], Label::Reference(0)),
    (&["ref"], Label::Reference(0)),
    (&["document", "#"], Label::Reference(0)),
    (&["transaction", "#"], Label::Reference(1)),
    (&["trans", "#"], Label::Reference(1)),
    (&["order", "#"], Label::Reference(1)),
    (&["invoice"], Label::Reference(2)),
    (&["terms"], Label::Terms),
    (&["payment", "terms"], Label::Terms),
    (&["terms", "of", "payment"], Label::Terms),
    (&["account", "#"], Label::Account),
    (&["account"], Label::Account),
    (&["acct", "#"], Label::Account),
    (&["acct"], Label::Account),
    (&["account", "id"], Label::Account),
    (&["customer", "#"], Label::Account),
    (&["customer", "id"], Label::Account),
    (&["cust", "#"], Label::Account),
    (&["phone"], Label::Phone),
    (&["phone", "#"], Label::Phone),
    (&["tel"], Label::Phone),
    (&["telephone"], Label::Phone),
    (&["ph"], Label::Phone),
    (&["bill", "to"], Label::Party),
    (&["ship", "to"], Label::Party),
    (&["sold", "to"], Label::Party),
    (&["deliver", "to"], Label::Party),
    (&["invoice", "to"], Label::Party),
    (&["customer"], Label::Party),
    (&["change"], Label::Change),
    (&["change", "due"], Label::Change),
    (&["check", "#"], Label::CheckNumber),
    (&["check"], Label::CheckNumber),
    (&["chk", "#"], Label::CheckNumber),
    (&["ck", "#"], Label::CheckNumber),
    (&["visa"], Label::Card),
    (&["mastercard"], Label::Card),
    (&["amex"], Label::Card),
    (&["american", "express"], Label::Card),
    (&["discover"], Label::Card),
    (&["debit"], Label::Card),
    (&["credit", "card"], Label::Card),
];

/// One canonical token and the position of the word it came from.
#[derive(Clone, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    /// Index into the word list the tokens were made from.
    pub word: usize,
}

/// Document text: `Debug` shows the word index and length only.
impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Token")
            .field("word", &self.word)
            .field("chars", &self.text.chars().count())
            .finish()
    }
}

/// A label found in a token list: `tokens[start..end]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelHit {
    pub label: Label,
    pub start: usize,
    pub end: usize,
}

/// Canonical tokens of words.
pub fn tokens<'a>(words: impl Iterator<Item = &'a Word>) -> Vec<Token> {
    let mut out = Vec::new();
    for (i, w) in words.enumerate() {
        for t in canon(&w.text) {
            out.push(Token { text: t, word: i });
        }
    }
    out
}

/// One word's canonical tokens (one or two).
pub fn canon(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let t = lower
        .trim_start_matches(['(', '[', '{', '"', '\''])
        .trim_end_matches([':', ';', ',', '.', ')', ']', '}', '"', '\'']);
    if t.is_empty() {
        return Vec::new();
    }
    if matches!(t, "#" | "no" | "nbr" | "num" | "number" | "#:") {
        return vec!["#".into()];
    }
    if let Some(stem) = t.strip_suffix('#')
        && !stem.is_empty()
        && stem.chars().all(char::is_alphabetic)
    {
        return vec![stem.to_string(), "#".into()];
    }
    if t.contains('-') && t.chars().all(|c| c.is_alphabetic() || c == '-') {
        return vec![t.replace('-', "")];
    }
    vec![t.to_string()]
}

/// The labels in `tokens`, left to right, longest match first at each
/// position, never overlapping.
#[must_use]
pub fn find(tokens: &[Token]) -> Vec<LabelHit> {
    let mut hits = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let best = TABLE
            .iter()
            .filter(|(words, _)| {
                i + words.len() <= tokens.len()
                    && words.iter().zip(&tokens[i..]).all(|(w, t)| *w == t.text)
            })
            .max_by_key(|(words, _)| words.len());
        if let Some((words, label)) = best {
            hits.push(LabelHit {
                label: *label,
                start: i,
                end: i + words.len(),
            });
            i += words.len();
        } else {
            i += 1;
        }
    }
    hits
}

/// Whether a token list starts with a label (a line that begins a new
/// field, not a value).
pub fn starts_with_label(tokens: &[Token]) -> bool {
    find(tokens).first().is_some_and(|h| h.start == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bbox, Confidence};

    fn words(s: &str) -> Vec<Word> {
        s.split(' ')
            .map(|t| Word {
                text: t.into(),
                bbox: Bbox::PAGE,
                confidence: Confidence::CERTAIN,
            })
            .collect()
    }

    fn labels(s: &str) -> Vec<Label> {
        find(&tokens(words(s).iter()))
            .into_iter()
            .map(|h| h.label)
            .collect()
    }

    #[test]
    fn canonical_tokens() {
        assert_eq!(canon("Invoice#:"), ["invoice", "#"]);
        assert_eq!(canon("No."), ["#"]);
        assert_eq!(canon("Sub-Total:"), ["subtotal"]);
        assert_eq!(canon("#20417"), ["#20417"]);
        assert_eq!(canon("(TOTAL)"), ["total"]);
        assert_eq!(canon("::").len(), 0);
    }

    #[test]
    fn longest_label_wins() {
        assert_eq!(labels("Invoice No. 20417"), [Label::Reference(0)]);
        assert_eq!(labels("Invoice Date: 07/14/2026"), [Label::Date(0)]);
        assert_eq!(labels("Subtotal 100.00"), [Label::Subtotal]);
        assert_eq!(labels("Sub Total 100.00"), [Label::Subtotal]);
        assert_eq!(labels("Total Tax 8.25"), [Label::Tax]);
        assert_eq!(labels("Total Amount Due $108.25"), [Label::Total(0)]);
        assert_eq!(labels("Payment Terms: Net 30"), [Label::Terms]);
        assert_eq!(
            labels("Date: 07/14/2026 Invoice #: 1042"),
            [Label::Date(1), Label::Reference(0)]
        );
        assert_eq!(labels("Canvas tarp").len(), 0);
        let t = tokens(words("Riverbend").iter());
        assert!(!format!("{t:?}").contains("riverbend"));
    }
}
