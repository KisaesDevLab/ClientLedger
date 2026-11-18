//! The tie-out view (P31): the engine's rows as the screen shows them.
//!
//! - Every amount is the engine's; nothing here adds or subtracts one.
//! - A difference under a cent that is not zero shows its exact digits
//!   (`0.0040`), never `0.00`.
//! - ✓ (and Mark verified) only when the engine says `allZero`.

use cl_core::{Money, MoneyStyle};
use rust_decimal::Decimal;

/// Which part of the tie-out a row is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Section {
    /// Trial balance accounts.
    Accounts,
    /// Open invoices.
    Receivables,
    /// Unpaid bills.
    Payables,
    /// Inventory items.
    Inventory,
    /// The history accounts and the valuation check.
    Checks,
}

impl Section {
    /// A stable key (`imports.tieout.section.<key>`).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Section::Accounts => "accounts",
            Section::Receivables => "receivables",
            Section::Payables => "payables",
            Section::Inventory => "inventory",
            Section::Checks => "checks",
        }
    }
}

/// One row as the engine sent it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowIn {
    /// Its section.
    pub section: Section,
    /// Account, customer, vendor or item name; for a check row its key
    /// (`history_ar`, …), which the UI names.
    pub label: String,
    /// The document number (open items).
    pub number: Option<String>,
    /// The source file row.
    pub source_row: Option<u64>,
    /// Source amount (none for check rows).
    pub source: Option<Money>,
    /// Books amount (none for check rows).
    pub books: Option<Money>,
    /// Books minus source, as the engine computed it.
    pub diff: Money,
    /// Books minus source quantity (inventory rows), as the engine sent it.
    pub quantity_diff: Option<Decimal>,
    /// The engine's id of the row's account, document or item (to point at
    /// a difference the engine named by id).
    pub id: Option<String>,
}

impl RowIn {
    /// A row of the books with no row in the source file (an open item or
    /// an account only the books have).
    #[must_use]
    pub fn books_only(&self) -> bool {
        self.section != Section::Checks && self.source_row.is_none()
    }
}

/// One row as shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The row as sent.
    pub input: RowIn,
    /// Source text.
    pub source_text: String,
    /// Books text.
    pub books_text: String,
    /// Difference text (exact under a cent).
    pub diff_text: String,
    /// The quantity difference, when there is one (the engine's digits).
    pub quantity_text: Option<String>,
    /// The amount and the quantity differences are exactly zero.
    pub zero: bool,
}

/// Difference text: 2 places normally, all 4 when a nonzero difference
/// is under a cent (so it never reads as 0.00).
#[must_use]
pub fn diff_text(diff: Money) -> String {
    let units = diff.to_ten_thousandths();
    if units != 0 && units.abs() < 100 {
        // `to_wire` always has exactly four places (`0.0040`, `-0.0040`).
        return diff.to_wire();
    }
    diff.format(MoneyStyle::default())
}

fn amount_text(m: Option<Money>) -> String {
    m.map_or_else(String::new, |m| m.format(MoneyStyle::default()))
}

/// The tie-out screen's model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    /// Every row, in the engine's order within each section.
    pub rows: Vec<Row>,
    /// The engine says every difference is zero.
    pub all_zero: bool,
    /// Show only rows that differ.
    pub differences_only: bool,
}

impl View {
    /// Builds the view; rows are grouped by section in a fixed order.
    #[must_use]
    pub fn new(mut rows: Vec<RowIn>, all_zero: bool) -> View {
        rows.sort_by_key(|r| r.section);
        View {
            rows: rows
                .into_iter()
                .map(|r| {
                    let quantity = r.quantity_diff.filter(|q| !q.is_zero());
                    Row {
                        source_text: amount_text(r.source),
                        books_text: amount_text(r.books),
                        diff_text: diff_text(r.diff),
                        quantity_text: quantity.map(|q| q.normalize().to_string()),
                        zero: r.diff.is_zero() && quantity.is_none(),
                        input: r,
                    }
                })
                .collect(),
            all_zero,
            differences_only: false,
        }
    }

    /// ✓ is shown (only on the engine's word).
    #[must_use]
    pub fn checkmark(&self) -> bool {
        self.all_zero
    }

    /// Mark verified is on (an Admin also needs the right; the engine
    /// checks again on verify).
    #[must_use]
    pub fn can_mark_verified(&self, admin: bool) -> bool {
        admin && self.all_zero
    }

    /// The rows to show.
    #[must_use]
    pub fn visible(&self) -> Vec<&Row> {
        self.rows
            .iter()
            .filter(|r| !self.differences_only || !r.zero)
            .collect()
    }

    /// The rows that differ, in order (the "first differences" summary).
    #[must_use]
    pub fn differences(&self) -> Vec<&Row> {
        self.rows.iter().filter(|r| !r.zero).collect()
    }

    /// The row the engine named (by section, then source row, else id), for
    /// pointing the owner at a difference verify refused.
    #[must_use]
    pub fn find(&self, section: Section, source_row: Option<u64>, id: Option<&str>) -> Option<usize> {
        let of = |r: &&Row| r.input.section == section;
        let by_row = source_row.and_then(|n| {
            self.rows
                .iter()
                .filter(of)
                .position(|r| r.input.source_row == Some(n))
        });
        by_row
            .or_else(|| {
                let id = id?;
                self.rows
                    .iter()
                    .filter(of)
                    .position(|r| r.input.id.as_deref() == Some(id))
            })
            .map(|i| {
                // The index within the section, back to the whole list.
                self.rows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.input.section == section)
                    .nth(i)
                    .map_or(0, |(k, _)| k)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn row(section: Section, label: &str, diff: &str) -> RowIn {
        RowIn {
            section,
            label: label.into(),
            number: None,
            source_row: Some(3),
            source: Some(m("4311.67")),
            books: Some(m("4311.67")),
            diff: m(diff),
            quantity_diff: None,
            id: None,
        }
    }

    #[test]
    fn a_quantity_difference_is_a_difference() {
        let mut stock = row(Section::Inventory, "Live Oak", "0.00");
        stock.quantity_diff = Some(Decimal::new(-5, 1));
        let mut same = row(Section::Inventory, "Mulch", "0.00");
        same.quantity_diff = Some(Decimal::ZERO);
        let v = View::new(vec![stock, same], false);
        assert!(!v.rows[0].zero, "value ties, quantity does not");
        assert_eq!(v.rows[0].quantity_text.as_deref(), Some("-0.5"));
        assert!(v.rows[1].zero && v.rows[1].quantity_text.is_none());
        assert_eq!(v.differences().len(), 1);
    }

    #[test]
    fn a_difference_is_found_by_row_then_id() {
        let mut a = row(Section::Accounts, "1000", "0.00");
        a.id = Some("acct-1".into());
        let mut b = row(Section::Accounts, "1200", "10.00");
        b.source_row = None;
        b.id = Some("acct-2".into());
        let mut c = row(Section::Receivables, "1005", "5.00");
        c.source_row = Some(9);
        let v = View::new(vec![c, a, b], false);
        assert_eq!(v.find(Section::Receivables, Some(9), None), Some(2));
        assert_eq!(v.find(Section::Accounts, None, Some("acct-2")), Some(1));
        assert_eq!(v.find(Section::Accounts, Some(99), Some("acct-1")), Some(0));
        assert_eq!(v.find(Section::Payables, Some(9), None), None);
        assert!(v.rows[1].input.books_only() && !v.rows[0].input.books_only());
    }

    #[test]
    fn under_a_cent_shows_exact_digits() {
        assert_eq!(diff_text(m("0.0040")), "0.0040");
        assert_eq!(diff_text(m("-0.0040")), "-0.0040");
        assert_eq!(diff_text(m("0.0099")), "0.0099");
        assert_eq!(diff_text(m("0.00")), "0.00");
        assert_eq!(diff_text(m("10.00")), "10.00");
        assert_eq!(diff_text(m("-1234.50")), "-1,234.50");
        assert_eq!(diff_text(m("0.01")), "0.01");
    }

    #[test]
    fn the_checkmark_is_the_engines_word_only() {
        // Rows that all read zero but an engine that says no: no ✓.
        let v = View::new(vec![row(Section::Accounts, "Checking", "0.00")], false);
        assert!(!v.checkmark() && !v.can_mark_verified(true));
        assert_eq!(v.differences().len(), 0);
        let v = View::new(vec![row(Section::Accounts, "Checking", "0.00")], true);
        assert!(v.checkmark() && v.can_mark_verified(true));
        assert!(!v.can_mark_verified(false), "Admin only");
    }

    #[test]
    fn rows_group_by_section_and_filter_to_differences() {
        let mut v = View::new(
            vec![
                row(Section::Checks, "history_ar", "-10.00"),
                row(Section::Receivables, "Brightwater HOA", "0.00"),
                row(Section::Accounts, "Accounts Receivable", "10.00"),
                row(Section::Accounts, "Fuel", "0.0040"),
            ],
            false,
        );
        let order: Vec<_> = v.rows.iter().map(|r| r.input.section).collect();
        assert_eq!(
            order,
            [
                Section::Accounts,
                Section::Accounts,
                Section::Receivables,
                Section::Checks
            ]
        );
        assert_eq!(v.differences().len(), 3);
        assert_eq!(v.rows[1].diff_text, "0.0040");
        assert!(!v.rows[1].zero, "under a cent is still a difference");
        v.differences_only = true;
        assert_eq!(v.visible().len(), 3);
        assert_eq!(v.rows[0].source_text, "4,311.67");
        assert_eq!(Section::Checks.key(), "checks");
    }
}
