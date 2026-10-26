//! A reading made on this computer, as the forms take it (A03 step 17).
//!
//! Every value is the document's own printed one (non-negotiable 4): the
//! header fields, the line amounts, the total. Nothing is computed except
//! the one total line of Q-A03m, which *is* the printed total. The
//! reading's checks travel alongside to be shown; they never change a value
//! (AC3). The vendor comes from the desktop's match (Q-A03o); a printed
//! name that matched no vendor is kept so the form can offer a new vendor.

use cl_api::{Contact, ContactType};
use cl_core::Money;
use cl_docread::model::{Check, CheckCode, ConfidenceBucket, Found, Reading};
use cl_docread::vendor_match::VendorCandidate;
use cl_model::forms::doc_read::{Confidence, DocReading, FieldConfidence, ReadLine};

use super::LocalReading;

/// A reading mapped for a form. `Debug` shows shapes only.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Mapped {
    /// The values to fill, each *read, not accepted*.
    pub doc: DocReading,
    /// The vendor's name as printed, when it matched no vendor.
    pub unknown_vendor: Option<String>,
    /// The reading's checks, to show (never applied).
    pub checks: Vec<Check>,
    /// The lines couldn't be read: one line of the printed total (Q-A03m).
    pub one_total_line: bool,
    /// The balance due printed, when the document also prints a different
    /// total (a deposit taken): the banner shows both (review nit).
    pub balance_due: Option<Money>,
}

impl std::fmt::Debug for Mapped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mapped")
            .field("doc", &self.doc)
            .field("unknown_vendor", &self.unknown_vendor.is_some())
            .field(
                "checks",
                &self.checks.iter().map(|c| c.code).collect::<Vec<_>>(),
            )
            .field("one_total_line", &self.one_total_line)
            .field("balance_due", &self.balance_due.is_some())
            .finish()
    }
}

/// The form's confidence for a reader's confidence (the engine's buckets).
#[must_use]
pub fn level(c: cl_docread::model::Confidence) -> Confidence {
    match c.bucket() {
        ConfidenceBucket::High => Confidence::High,
        ConfidenceBucket::Medium => Confidence::Medium,
        ConfidenceBucket::Low => Confidence::Low,
    }
}

fn level_of<T>(f: Option<&Found<T>>) -> Option<Confidence> {
    f.map(|f| level(f.confidence))
}

/// The company's active vendors as matching candidates. No layouts: the
/// engine's templates arrive with `document-drafts-v1` (WP-F).
#[must_use]
pub fn vendor_candidates(contacts: &[Contact]) -> Vec<VendorCandidate> {
    contacts
        .iter()
        .filter(|c| {
            c.is_active
                && matches!(
                    c.fields.contact_type,
                    ContactType::Vendor | ContactType::Both
                )
        })
        .map(|c| {
            let f = &c.fields;
            let city_line = [
                f.billing_city.as_deref(),
                f.billing_state.as_deref(),
                f.billing_zip.as_deref(),
            ]
            .into_iter()
            .flatten()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
            let address = [
                f.billing_line1.clone(),
                f.billing_line2.clone(),
                Some(city_line),
            ]
            .into_iter()
            .flatten()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
            VendorCandidate {
                contact: c.id,
                display_name: f.display_name.clone(),
                company_name: f.company_name.clone(),
                account_number: f.vendor_account_number.clone(),
                phone: f.phone.clone(),
                address: (!address.is_empty()).then_some(address),
                layouts: Vec::new(),
            }
        })
        .collect()
}

/// The reading as the form model takes it.
#[must_use]
pub fn to_doc(local: &LocalReading) -> Mapped {
    let r: &Reading = &local.reading;
    let h = &r.header;
    // The bill's amount is the document's total; a document that prints
    // only a balance due gives that.
    let total = h.total.as_ref().or(h.balance_due.as_ref());
    let mut lines: Vec<ReadLine> = r
        .lines
        .iter()
        .map(|l| ReadLine {
            description: l
                .description
                .as_ref()
                .map(|d| d.value.trim().to_string())
                .unwrap_or_default(),
            amount: Some(l.amount.value),
            account: None,
            confidence: Some(level(l.amount.confidence)),
        })
        .collect();
    let lines_not_read = r.checks.iter().any(|c| c.code == CheckCode::LinesNotRead);
    let mut one_total_line = false;
    if lines.is_empty()
        && lines_not_read
        && let Some(t) = total
    {
        // Q-A03m: one line of the printed total, to the vendor's default
        // account (the form's ghost).
        lines.push(ReadLine {
            description: String::new(),
            amount: Some(t.value),
            account: None,
            confidence: Some(level(t.confidence)),
        });
        one_total_line = true;
    }
    let unknown_vendor = if local.vendor.is_none() {
        h.vendor
            .as_ref()
            .map(|v| v.value.trim().to_string())
            .filter(|n| !n.is_empty())
    } else {
        None
    };
    Mapped {
        doc: DocReading {
            vendor: local.vendor.map(|v| v.contact),
            bill_date: h.date.as_ref().map(|d| d.value),
            due_date: h.due_date.as_ref().map(|d| d.value),
            reference: h
                .reference
                .as_ref()
                .map(|f| f.value.trim().to_string())
                .filter(|s| !s.is_empty()),
            subtotal: h.subtotal.as_ref().map(|f| f.value),
            tax: h.tax.as_ref().map(|f| f.value),
            total: total.map(|f| f.value),
            lines,
            confidence: FieldConfidence {
                vendor: local.vendor.map(|v| level(v.confidence)),
                bill_date: level_of(h.date.as_ref()),
                due_date: level_of(h.due_date.as_ref()),
                reference: level_of(h.reference.as_ref()),
                total: level_of(total),
                terms: level_of(h.terms.as_ref()),
                amount_paid: level_of(h.amount_paid.as_ref()),
                payment: level_of(h.payment.as_ref()),
            },
        },
        unknown_vendor,
        checks: r.checks.clone(),
        one_total_line,
        balance_due: match (&h.total, &h.balance_due) {
            (Some(t), Some(b)) if t.value != b.value => Some(b.value),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::ids::ContactId;
    use cl_core::{CalendarDate, Money};
    use cl_docread::model::{Bbox, Confidence as Bp, Line, Reader, RuleId, Spot};
    use cl_docread::vendor_match::{VendorMatch, VendorTier};

    fn found<T>(value: T, bp: u16) -> Found<T> {
        Found {
            value,
            spot: Spot {
                page: 0,
                bbox: Bbox::new(10, 10, 20, 20).unwrap(),
            },
            rule: RuleId::new("test"),
            confidence: Bp::new(bp).unwrap(),
        }
    }

    fn money(s: &str) -> Money {
        Money::parse_wire(s).unwrap()
    }

    fn check(code: CheckCode) -> Check {
        Check {
            code,
            rule: RuleId::new("test"),
            field: None,
            line: None,
            expected: None,
            found: None,
            difference: None,
            spot: None,
        }
    }

    fn contact() -> ContactId {
        ContactId::parse("00000000-0000-0000-0000-000000000007").unwrap()
    }

    fn local() -> LocalReading {
        let mut r = Reading::empty(Reader::TextPdf, 1);
        r.header.vendor = Some(found("Midwest Equipment Rental".to_string(), 10_000));
        r.header.date = Some(found(CalendarDate::from_ymd(2026, 11, 3).unwrap(), 9_000));
        r.header.reference = Some(found(" INV-55871 ".to_string(), 7_000));
        r.header.total = Some(found(money("1480.00"), 10_000));
        r.lines = vec![
            Line {
                description: Some(found("Mini excavator".to_string(), 10_000)),
                quantity: None,
                unit_price: None,
                amount: found(money("1330.00"), 10_000),
            },
            Line {
                description: None,
                quantity: None,
                unit_price: None,
                amount: found(money("150.00"), 4_000),
            },
        ];
        r.checks.push(check(CheckCode::LinesVsSubtotal));
        LocalReading {
            reading: r,
            vendor: Some(VendorMatch {
                contact: contact(),
                tier: VendorTier::Name(1),
                confidence: Bp::new(9_000).unwrap(),
                layout_of: None,
            }),
            card: None,
        }
    }

    /// Review nit: a deposit invoice's total and balance due both reach the
    /// banner; the bill's amount stays the total.
    #[test]
    fn a_different_balance_due_is_shown_with_the_total() {
        let mut l = local();
        assert_eq!(to_doc(&l).balance_due, None);
        l.reading.header.balance_due = Some(found(money("1480.00"), 10_000));
        assert_eq!(to_doc(&l).balance_due, None, "the same figure");
        l.reading.header.balance_due = Some(found(money("980.00"), 10_000));
        let m = to_doc(&l);
        assert_eq!(m.balance_due, Some(money("980")));
        assert_eq!(m.doc.total, Some(money("1480")));
    }

    #[test]
    fn every_value_is_the_documents_own() {
        let m = to_doc(&local());
        assert_eq!(m.doc.vendor, Some(contact()));
        assert_eq!(m.doc.reference.as_deref(), Some("INV-55871"));
        assert_eq!(m.doc.total, Some(money("1480.00")));
        let amounts: Vec<_> = m.doc.lines.iter().map(|l| l.amount).collect();
        assert_eq!(amounts, vec![Some(money("1330.00")), Some(money("150.00"))]);
        assert_eq!(m.doc.lines[1].confidence, Some(Confidence::Low));
        assert_eq!(m.doc.confidence.reference, Some(Confidence::Medium));
        assert_eq!(m.doc.confidence.vendor, Some(Confidence::High));
        assert_eq!(m.unknown_vendor, None);
        // Checks are carried to be shown, never applied.
        assert_eq!(m.checks.len(), 1);
        assert!(!m.one_total_line);
    }

    #[test]
    fn unread_lines_become_one_line_of_the_printed_total() {
        let mut l = local();
        l.reading.lines.clear();
        l.reading.checks = vec![check(CheckCode::LinesNotRead)];
        l.vendor = None;
        let m = to_doc(&l);
        assert!(m.one_total_line);
        assert_eq!(m.doc.lines.len(), 1);
        assert_eq!(m.doc.lines[0].amount, Some(money("1480.00")));
        assert_eq!(m.doc.vendor, None);
        assert_eq!(
            m.unknown_vendor.as_deref(),
            Some("Midwest Equipment Rental")
        );
        // No total, no line: nothing is made up.
        l.reading.header.total = None;
        assert_eq!(to_doc(&l).doc.lines.len(), 0);
    }

    #[test]
    fn debug_hides_the_values() {
        let text = format!("{:?}", to_doc(&local()));
        assert!(
            !text.contains("Midwest") && !text.contains("1480"),
            "{text}"
        );
    }
}
