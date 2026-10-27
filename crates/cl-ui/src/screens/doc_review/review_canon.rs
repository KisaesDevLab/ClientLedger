//! The canonical text of what posts (A04 AC1, plan §4 N4, review S2).
//!
//! Two independent paths reach the same [`Canon`]:
//!
//! - **The form side** ([`Canon::of_bill`], [`Canon::of_card`],
//!   [`Canon::of_check`]) reads the form's checked values (what the owner
//!   reviewed, after its exact gate), field by field.
//! - **The posted side** ([`Canon::of_wire`]) reads the convert request's
//!   `body` exactly as cl-api puts it on the wire
//!   ([`cl_api::ConvertBody::wire_body`]), never the form's own engine
//!   draft. Neither side calls the forms' builders.
//!
//! So their [`digest`](super::digest)s are equal exactly when the request
//! carries every value the owner reviewed: date, payee, account paid from,
//! reference or check number, memo, terms, received, print later, address,
//! header tags and, per line, account, exact amount, memo, customer,
//! billable, tag, item, quantity and cost. Totals are not added up here
//! (review L6): a bill's and a card charge's total is their lines' on the
//! wire (the form's gate made them equal to the cent); a check's amount is
//! sent and compared as sent.
//!
//! Money and quantities are their exact wire text; ids their text. Owner
//! data: never logged, never in `Debug`.

use cl_api::DraftKind;
use cl_core::{CalendarDate, Money, Quantity};
use cl_model::forms::bill::{CheckedBill, CheckedTerms};
use cl_model::forms::card::CheckedCard;
use cl_model::forms::check::CheckedCheck;
use cl_model::forms::lines::PostingLine;
use serde::Serialize;
use serde_json::Value;

#[cfg(test)]
#[path = "review_canon_tests.rs"]
mod tests;

/// One line of the canonical text.
#[derive(Clone, PartialEq, Eq, Serialize, Default)]
pub(crate) struct CanonLine {
    account: String,
    amount: String,
    memo: Option<String>,
    contact: Option<String>,
    billable: bool,
    tag: Option<String>,
    item: Option<String>,
    quantity: Option<String>,
    unit_price: Option<String>,
}

/// Payment terms as sent.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CanonTerms {
    code: String,
    days: Option<u64>,
    discount_percent: Option<String>,
    discount_days: Option<u64>,
}

/// The canonical text (see the module comment).
#[derive(Clone, PartialEq, Eq, Serialize, Default)]
pub(crate) struct Canon {
    kind: &'static str,
    date: String,
    due: Option<String>,
    payee: Option<String>,
    paid_from: Option<String>,
    reference: Option<String>,
    number: Option<u64>,
    /// A check's amount only (see the module comment).
    total: Option<String>,
    memo: Option<String>,
    terms: Option<CanonTerms>,
    received: Option<bool>,
    print_later: Option<bool>,
    address: Option<String>,
    header_tags: Vec<String>,
    lines: Vec<CanonLine>,
}

impl std::fmt::Debug for Canon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Canon")
            .field("kind", &self.kind)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

/// Trimmed text, `None` when empty (the engine keeps a blank as none).
fn text(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn id<T: ToString>(v: Option<T>) -> Option<String> {
    v.map(|v| v.to_string())
}

/// A percentage's exact value as one text (`2`, `1.5`), whichever way it
/// was written.
fn percent(s: &str) -> Option<String> {
    cl_api::Percent::parse(s).ok().map(cl_api::Percent::to_wire)
}

fn kind_name(kind: DraftKind) -> &'static str {
    kind.as_str()
}

impl Canon {
    /// The canonical bytes.
    #[must_use]
    pub(crate) fn bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Its [`digest`](super::digest).
    #[must_use]
    pub(crate) fn digest(&self) -> String {
        super::digest(&self.bytes())
    }

    fn posting_line(l: &PostingLine) -> CanonLine {
        CanonLine {
            account: l.account.to_string(),
            amount: l.amount.to_wire(),
            memo: text(l.memo.as_deref()),
            contact: id(l.contact),
            billable: l.billable,
            tag: id(l.tag),
            ..CanonLine::default()
        }
    }

    fn terms(t: &CheckedTerms) -> CanonTerms {
        CanonTerms {
            code: t.code.clone(),
            days: t.days.map(u64::from),
            discount_percent: t.discount_percent.and_then(|p| percent(&p.to_wire())),
            discount_days: t.discount_days.map(u64::from),
        }
    }

    /// A checked bill (the form side).
    #[must_use]
    pub(crate) fn of_bill(c: &CheckedBill) -> Canon {
        let mut lines: Vec<CanonLine> = c.expense_lines.iter().map(Self::posting_line).collect();
        lines.extend(c.item_lines.iter().map(|l| CanonLine {
            account: l.account.to_string(),
            amount: l.amount.to_wire(),
            memo: text(l.description.as_deref()),
            contact: id(l.contact),
            billable: l.billable,
            tag: id(l.tag),
            item: id(l.item),
            quantity: l.qty.map(Quantity::to_wire),
            unit_price: l.rate.map(Money::to_wire),
        }));
        Canon {
            kind: "bill",
            date: c.date.to_wire(),
            due: c.due_date.map(CalendarDate::to_wire),
            payee: Some(c.vendor.to_string()),
            reference: text(c.reference.as_deref()),
            memo: text(c.memo.as_deref()),
            terms: c.terms.as_ref().map(Self::terms),
            // No answer is the engine's default: received.
            received: Some(c.bill_received.unwrap_or(true)),
            lines,
            ..Canon::default()
        }
    }

    /// A checked card charge or cash expense (the form side).
    #[must_use]
    pub(crate) fn of_card(kind: DraftKind, c: &CheckedCard) -> Canon {
        Canon {
            kind: kind_name(kind),
            date: c.date.to_wire(),
            payee: id(c.payee),
            paid_from: Some(c.card.to_string()),
            reference: text(c.ref_no.as_deref()),
            memo: text(c.memo.as_deref()),
            // Header tags are each line's default tag (Q-178).
            lines: c.lines.iter().map(Self::posting_line).collect(),
            ..Canon::default()
        }
    }

    /// A checked check (the form side).
    #[must_use]
    pub(crate) fn of_check(c: &CheckedCheck) -> Canon {
        Canon {
            kind: "check",
            date: c.date.to_wire(),
            payee: Some(c.payee.to_string()),
            paid_from: Some(c.bank.to_string()),
            number: c.number.map(u64::from),
            total: Some(c.amount.to_wire()),
            memo: text(c.memo.as_deref()),
            print_later: Some(c.print_later),
            address: text(c.address.as_deref()),
            header_tags: c.header_tags.iter().map(ToString::to_string).collect(),
            lines: c.lines.iter().map(Self::posting_line).collect(),
            ..Canon::default()
        }
    }

    /// The posted side: a convert request's `body` as sent for `kind`
    /// (`None` when it isn't one this version reads: the digests then
    /// differ and the scenario fails).
    #[must_use]
    pub(crate) fn of_wire(kind: DraftKind, v: &Value) -> Option<Canon> {
        let s = |k: &str| v.get(k).and_then(Value::as_str);
        let date = s("txnDate")?.to_string();
        Some(match kind {
            DraftKind::Bill => Canon {
                kind: "bill",
                date,
                due: s("dueDate").map(str::to_string),
                payee: s("contactId").map(str::to_string),
                reference: text(s("vendorInvoiceNumber")),
                memo: text(s("memo")),
                terms: s("paymentTerms").map(|code| CanonTerms {
                    code: code.to_string(),
                    days: v.get("termsDays").and_then(Value::as_u64),
                    discount_percent: s("discountPercent").and_then(percent),
                    discount_days: v.get("discountDays").and_then(Value::as_u64),
                }),
                received: Some(
                    v.get("billReceived")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                ),
                lines: wire_lines(v, "accountId")?,
                ..Canon::default()
            },
            DraftKind::CardCharge | DraftKind::CashExpense => Canon {
                kind: kind_name(kind),
                date,
                payee: s("contactId").map(str::to_string),
                paid_from: s("payFromAccountId").map(str::to_string),
                reference: text(s("txnNumber")),
                memo: text(s("memo")),
                lines: wire_lines(v, "expenseAccountId")?,
                ..Canon::default()
            },
            DraftKind::Check => Canon {
                kind: "check",
                date,
                payee: s("contactId").map(str::to_string),
                paid_from: s("bankAccountId").map(str::to_string),
                number: v.get("checkNumber").and_then(Value::as_u64),
                total: s("amount").map(str::to_string),
                memo: text(s("memo")),
                print_later: v.get("printLater").and_then(Value::as_bool),
                address: text(s("payeeAddress")),
                header_tags: v
                    .get("tagIds")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                lines: wire_lines(v, "accountId")?,
                ..Canon::default()
            },
            DraftKind::Statement | DraftKind::Unknown => return None,
        })
    }
}

/// The wire lines (`account` under `account_key`).
fn wire_lines(v: &Value, account_key: &str) -> Option<Vec<CanonLine>> {
    v.get("lines")?
        .as_array()?
        .iter()
        .map(|l| {
            let s = |k: &str| l.get(k).and_then(Value::as_str).map(str::to_string);
            Some(CanonLine {
                account: s(account_key)?,
                amount: s("amount")?,
                memo: text(l.get("description").and_then(Value::as_str)),
                contact: s("contactId"),
                billable: l.get("isBillable").and_then(Value::as_bool) == Some(true),
                tag: s("tagId"),
                item: s("itemId"),
                quantity: s("quantity"),
                unit_price: s("unitPrice"),
            })
        })
        .collect()
}
