//! Signs: what the owner sees, what the engine gets, and whether a file
//! looks like it uses the other convention (Q-196, Q-P25b, Q-P25t).
//!
//! - `display = flipped ? −file : file` (money out shows negative).
//! - `wire = −display` (the feed sign: positive means money out).
//!
//! [`to_feed_amount`] holds the only negation in the crate's sign handling.

use crate::normalize::{Balance, Section};
use rust_decimal::Decimal;

/// The amount sent to the engine (feed sign: positive = money out).
#[must_use]
pub fn to_feed_amount(file_amount: Decimal, flipped: bool) -> Decimal {
    if flipped { file_amount } else { -file_amount }
}

/// The amount shown to the owner (money out negative), also used for the
/// file's balance, which the engine takes in the holder's sign.
#[must_use]
pub fn to_display(file_amount: Decimal, flipped: bool) -> Decimal {
    if flipped {
        to_feed_amount(file_amount, false)
    } else {
        file_amount
    }
}

/// The result of the sign checks on one section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SignCheck {
    /// Offer to flip the amounts (the owner confirms once, Q-196).
    pub suggest_flip: bool,
    /// Why the flip is (or is not) suggested; `None` when no check had
    /// anything to go on.
    pub reason: Option<SignReason>,
    /// What the running balance says (CSV).
    pub continuity: Continuity,
}

/// What decided the sign suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignReason {
    /// A card file where most rows are positive: charges are positive, so
    /// money out would show as money in (Q-196).
    CardChargesPositive,
    /// A card file where most rows are negative (the usual convention).
    CardChargesNegative,
    /// A bank OFX file where at least 80% of debit-type rows (`DEBIT`,
    /// `CHECK`, `PAYMENT`, `POS`, `ATM`, `FEE`, `SRVCHG`, `DIRECTDEBIT`) are
    /// positive (review S9).
    DebitTypesPositive,
    /// The running balance settles it (it wins over the other checks).
    RunningBalance,
}

/// What a CSV running-balance column says about the rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Continuity {
    /// No running balance on every row, or fewer than 2 rows.
    #[default]
    NotAvailable,
    /// Each balance follows from the one before and the amount.
    Continuous {
        /// File order.
        order: RowOrder,
        /// The amounts have the opposite sign of the balance changes.
        flipped: bool,
    },
    /// The balances do not follow in any order or sign: a gap or a
    /// missing row. The line is where it first breaks.
    Broken {
        /// 1-based line of the first row that does not follow.
        line: usize,
    },
}

/// The order of rows in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowOrder {
    /// Oldest first.
    OldestFirst,
    /// Newest first.
    NewestFirst,
}

/// The OFX `TRNTYPE`s that mean money out.
const DEBIT_TYPES: [&str; 8] = [
    "DEBIT",
    "CHECK",
    "PAYMENT",
    "POS",
    "ATM",
    "FEE",
    "SRVCHG",
    "DIRECTDEBIT",
];

/// Runs the sign checks and stores the result on the section. For a
/// continuous running balance with no stated balance, the ending balance
/// is taken from it (in the file's sign).
pub(crate) fn analyze(section: &mut Section) {
    let continuity = continuity(section);
    let mut check = SignCheck {
        continuity,
        ..SignCheck::default()
    };
    if let Continuity::Continuous { order, flipped } = continuity {
        check.suggest_flip = flipped;
        check.reason = Some(SignReason::RunningBalance);
        if section.ledger_balance.is_none() {
            let last = match order {
                RowOrder::OldestFirst => section.rows.last(),
                RowOrder::NewestFirst => section.rows.first(),
            };
            if let Some((bal, date)) = last.and_then(|r| r.running_balance.map(|b| (b, r.date))) {
                // Stored in the file's sign: flipping it gives the balance as
                // the bank shows it.
                section.ledger_balance = Some(Balance {
                    amount: if flipped { -bal } else { bal },
                    date: Some(date),
                });
            }
        }
    } else if section.kind.is_card() {
        let positive = section
            .rows
            .iter()
            .filter(|r| r.file_amount.is_sign_positive())
            .count();
        let negative = section.rows.len() - positive;
        if positive > negative {
            check.suggest_flip = true;
            check.reason = Some(SignReason::CardChargesPositive);
        } else if negative > 0 {
            check.reason = Some(SignReason::CardChargesNegative);
        }
    } else {
        let debits: Vec<bool> = section
            .rows
            .iter()
            .filter(|r| {
                r.trn_type
                    .as_deref()
                    .is_some_and(|t| DEBIT_TYPES.contains(&t))
            })
            .map(|r| r.file_amount.is_sign_positive())
            .collect();
        let positive = debits.iter().filter(|p| **p).count();
        // At least 80%: positive / total >= 4/5, in integers.
        if !debits.is_empty() && positive * 5 >= debits.len() * 4 {
            check.suggest_flip = true;
            check.reason = Some(SignReason::DebitTypesPositive);
        }
    }
    section.sign = check;
}

/// The sign checks for a section as if it were for an account of `kind`.
/// CSV files do not say what account they are for; once the owner picks a
/// card account, the wizard runs the card check (Q-196) with this.
#[must_use]
pub fn check_as(section: &Section, kind: crate::AccountKind) -> SignCheck {
    let mut copy = Section::new(section.index, kind);
    copy.rows.clone_from(&section.rows);
    copy.ledger_balance = section.ledger_balance;
    analyze(&mut copy);
    copy.sign
}

/// Checks every row's running balance against the one before it, in file
/// order and reversed, with the amounts as they are and flipped.
fn continuity(section: &Section) -> Continuity {
    let rows = &section.rows;
    if rows.len() < 2 || rows.iter().any(|r| r.running_balance.is_none()) {
        return Continuity::NotAvailable;
    }
    let mut best_break: Option<usize> = None;
    let mut best_ok = 0;
    for order in [RowOrder::OldestFirst, RowOrder::NewestFirst] {
        for flipped in [false, true] {
            let mut ok = 0;
            let mut broke = None;
            for i in 1..rows.len() {
                let (prev, cur) = (&rows[i - 1], &rows[i]);
                let (Some(pb), Some(cb)) = (prev.running_balance, cur.running_balance) else {
                    return Continuity::NotAvailable;
                };
                // Oldest first: this balance = the previous + this amount.
                // Newest first: the previous balance = this + the previous amount.
                let (from, to, amount) = match order {
                    RowOrder::OldestFirst => (pb, cb, cur.file_amount),
                    RowOrder::NewestFirst => (cb, pb, prev.file_amount),
                };
                let amount = if flipped { -amount } else { amount };
                if from.checked_add(amount) == Some(to) {
                    ok += 1;
                } else {
                    broke = Some(cur.line);
                    break;
                }
            }
            match broke {
                None => return Continuity::Continuous { order, flipped },
                Some(line) if ok > best_ok || best_break.is_none() => {
                    best_ok = ok;
                    best_break = Some(line);
                }
                Some(_) => {}
            }
        }
    }
    best_break.map_or(Continuity::NotAvailable, |line| Continuity::Broken { line })
}

#[cfg(test)]
mod tests;
