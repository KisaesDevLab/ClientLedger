//! Checks on pre-printed stock (Q-14, Q-P19g): standard (three per page)
//! and voucher (one check and two stubs listing the bills paid, Q-213).
//! No MICR line and no signature (Q-14, Q-212): the bank details and the
//! check number are already on the stock.
//!
//! # Geometry (default assumption, Q-P19h)
//!
//! All positions are millimetres on US Letter.
//!
//! - **Standard stock:** three 3.5 in (88.9 mm) checks stacked from the top
//!   of the sheet at a fixed 88.9 mm pitch (the last 0.5 in is the tear
//!   strip). Position 1 starts at 0, position 2 at 88.9, position 3 at
//!   177.8.
//! - **Voucher stock:** the check is the top 88.9 mm; the two stubs are
//!   3.75 in (95.25 mm) each, from 88.9 and 184.15 mm.
//! - **#10 double-window envelope (Q-93):** 9.5 × 4.125 in; the lower
//!   (recipient) window is 4.5 × 1.125 in, 7/8 in from the left and 5/8 in
//!   from the bottom. A standard check sits at the bottom of the envelope,
//!   so the window spans 44.4–73.0 mm from the check's top; a voucher sheet
//!   tri-folds at 93.1 mm with the check panel outermost, so the window
//!   spans 48.6–77.2 mm. The address block (49.5–71.5 mm, from 25.4 mm
//!   across) shows through both, and through the window even if the check
//!   slides up to 12.7 mm sideways.
//!
//! The alignment offset ([`AlignmentOffset`], ±10 mm, saved per bank
//! account) moves everything printed on the page. It corrects a printer
//! that feeds the sheet a little off; it cannot correct stock with a
//! different check pitch.

pub mod standard;
pub mod voucher;

use crate::PrintError;
use crate::amount::{amount_in_words, check_figures};
use crate::fonts::Font;
use crate::layout::{Align, Document, Page, Rect, Shape, TextBox};
use rust_decimal::Decimal;

/// Height of one check.
pub const CHECK_H_MM: f32 = 88.9;
/// Largest alignment offset either way.
pub const MAX_OFFSET_MM: f32 = 10.0;

/// The recipient window of a #10 double-window envelope, in a standard
/// check's coordinates (see the module notes).
pub const WINDOW_ON_STANDARD: Rect = Rect::new(22.2, 44.4, 114.3, 28.6);
/// The recipient window in a tri-folded voucher sheet's coordinates.
pub const WINDOW_ON_VOUCHER: Rect = Rect::new(22.2, 48.6, 114.3, 28.6);

/// Where each check field goes, relative to the check's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CheckFields {
    /// The date, on the stock's date line.
    pub date: Rect,
    /// Pay to the order of.
    pub payee: Rect,
    /// The amount in figures (right-aligned, asterisk fill on the left).
    pub amount: Rect,
    /// The amount in words (asterisk fill on the right).
    pub words: Rect,
    /// The payee's address block (#10 window).
    pub address: Rect,
    /// The memo line.
    pub memo: Rect,
}

/// The one check layout both styles share.
pub const FIELDS: CheckFields = CheckFields {
    date: Rect::new(165.0, 17.0, 38.0, 6.0),
    payee: Rect::new(30.0, 30.0, 125.0, 6.0),
    amount: Rect::new(167.0, 30.0, 36.0, 6.0),
    words: Rect::new(12.0, 40.0, 150.0, 6.0),
    address: Rect::new(25.4, 49.5, 95.0, 22.0),
    memo: Rect::new(20.0, 76.0, 80.0, 6.0),
};

impl CheckFields {
    /// Every field moved by (`dx`, `dy`).
    #[must_use]
    pub fn moved(self, dx: f32, dy: f32) -> CheckFields {
        CheckFields {
            date: self.date.moved(dx, dy),
            payee: self.payee.moved(dx, dy),
            amount: self.amount.moved(dx, dy),
            words: self.words.moved(dx, dy),
            address: self.address.moved(dx, dy),
            memo: self.memo.moved(dx, dy),
        }
    }

    /// The fields with their names (for outlines and tests).
    #[must_use]
    pub fn named(&self) -> [(&'static str, Rect); 6] {
        [
            ("date", self.date),
            ("payee", self.payee),
            ("amount", self.amount),
            ("words", self.words),
            ("address", self.address),
            ("memo", self.memo),
        ]
    }
}

/// Check stock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CheckStyle {
    /// Three checks per page.
    #[default]
    Standard,
    /// One check and two stubs per page.
    Voucher,
}

/// A printer's feed correction, applied to everything printed on the page.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AlignmentOffset {
    dx_mm: f32,
    dy_mm: f32,
}

impl AlignmentOffset {
    /// No correction.
    pub const NONE: AlignmentOffset = AlignmentOffset {
        dx_mm: 0.0,
        dy_mm: 0.0,
    };

    /// A correction: positive `right_mm` moves right, positive `down_mm`
    /// moves down.
    ///
    /// # Errors
    /// [`PrintError::OffsetOutOfRange`] beyond ±[`MAX_OFFSET_MM`] or not a
    /// number.
    pub fn new(right_mm: f32, down_mm: f32) -> Result<AlignmentOffset, PrintError> {
        let in_range = |v: f32| v.is_finite() && v.abs() <= MAX_OFFSET_MM;
        if in_range(right_mm) && in_range(down_mm) {
            Ok(AlignmentOffset {
                dx_mm: right_mm,
                dy_mm: down_mm,
            })
        } else {
            Err(PrintError::OffsetOutOfRange)
        }
    }

    /// Horizontal correction.
    #[must_use]
    pub fn dx_mm(self) -> f32 {
        self.dx_mm
    }

    /// Vertical correction.
    #[must_use]
    pub fn dy_mm(self) -> f32 {
        self.dy_mm
    }
}

/// A bill paid, as a voucher stub lists it (Q-213).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StubBill {
    /// The bill's reference number in the books.
    pub ref_no: String,
    /// The vendor's invoice number.
    pub vendor_invoice_no: String,
    /// The bill date, formatted by the caller.
    pub date: String,
    /// The bill's total.
    pub bill_total: Decimal,
    /// Early-payment discount taken.
    pub discount: Decimal,
    /// Paid by this check.
    pub amount_paid: Decimal,
}

/// A vendor credit used with this payment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StubCredit {
    /// The credit's reference number.
    pub ref_no: String,
    /// The credit date, formatted by the caller.
    pub date: String,
    /// The amount of the credit used.
    pub amount: Decimal,
}

/// What a voucher stub lists.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StubData {
    /// Bills paid.
    pub bills: Vec<StubBill>,
    /// Credits used.
    pub credits: Vec<StubCredit>,
}

/// Column totals of a stub, computed exactly (Q-213).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StubTotals {
    /// Sum of the bill totals.
    pub bill_total: Decimal,
    /// Sum of the discounts.
    pub discount: Decimal,
    /// Sum of the credits used.
    pub credits: Decimal,
    /// Sum of the amounts paid.
    pub amount_paid: Decimal,
}

impl StubData {
    /// The column totals.
    ///
    /// # Errors
    /// [`PrintError::AmountTooLarge`] if a sum overflows.
    pub fn totals(&self) -> Result<StubTotals, PrintError> {
        let add = |a: Decimal, b: Decimal| a.checked_add(b).ok_or(PrintError::AmountTooLarge);
        let mut t = StubTotals::default();
        for b in &self.bills {
            t.bill_total = add(t.bill_total, b.bill_total)?;
            t.discount = add(t.discount, b.discount)?;
            t.amount_paid = add(t.amount_paid, b.amount_paid)?;
        }
        for c in &self.credits {
            t.credits = add(t.credits, c.amount)?;
        }
        Ok(t)
    }
}

/// One check to print. Amounts are parsed by the caller from the engine's
/// strings; dates are formatted by the caller.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CheckData {
    /// The check number (voucher stubs show it; the stock has it printed).
    pub number: String,
    /// The check date.
    pub date: String,
    /// Pay to the order of.
    pub payee: String,
    /// The amount (cents; never negative).
    pub amount: Decimal,
    /// The address block as it should print, name first.
    pub address: Vec<String>,
    /// The memo.
    pub memo: String,
    /// What the voucher stubs list (ignored on standard stock).
    pub stub: StubData,
}

/// Fixed words printed on checks (English defaults; the UI passes its
/// translations).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckLabels {
    /// Stub heading before the check number.
    pub check_no: String,
    /// Column: date.
    pub date: String,
    /// Column: reference number.
    pub ref_no: String,
    /// Column: vendor invoice number.
    pub vendor_invoice: String,
    /// Column: bill total.
    pub bill_total: String,
    /// Column: discount.
    pub discount: String,
    /// Column: credit used.
    pub credit: String,
    /// Column: amount paid.
    pub amount_paid: String,
    /// The word shown in a credit row's vendor-invoice column.
    pub credit_row: String,
    /// Totals row.
    pub totals: String,
    /// The check amount line.
    pub check_amount: String,
    /// Memo prefix on stubs.
    pub memo: String,
    /// Overflow row; `{n}` is replaced by the count of rows not shown.
    pub more_rows: String,
    /// Payee shown on the alignment test page.
    pub sample_payee: String,
    /// Memo shown on the alignment test page.
    pub sample_memo: String,
}

impl Default for CheckLabels {
    fn default() -> CheckLabels {
        CheckLabels {
            check_no: "Check no.".into(),
            date: "Date".into(),
            ref_no: "Ref no.".into(),
            vendor_invoice: "Vendor invoice".into(),
            bill_total: "Bill total".into(),
            discount: "Discount".into(),
            credit: "Credit".into(),
            amount_paid: "Amount paid".into(),
            credit_row: "Credit".into(),
            totals: "Totals".into(),
            check_amount: "Check amount".into(),
            memo: "Memo".into(),
            more_rows: "and {n} more (see the bill payment)".into(),
            sample_payee: "Sample Payee Name".into(),
            sample_memo: "Alignment test".into(),
        }
    }
}

/// How to print a batch of checks.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckOptions {
    /// The stock.
    pub style: CheckStyle,
    /// The chosen printer's saved feed correction (from `cl-settings`).
    pub offset: AlignmentOffset,
    /// Standard stock: the first unused position on the first sheet (1–3).
    pub first_position: u8,
    /// Fixed words.
    pub labels: CheckLabels,
}

impl Default for CheckOptions {
    fn default() -> CheckOptions {
        CheckOptions {
            style: CheckStyle::Standard,
            offset: AlignmentOffset::NONE,
            first_position: 1,
            labels: CheckLabels::default(),
        }
    }
}

/// Lay out checks: one page per voucher check, three per standard sheet.
///
/// # Errors
/// [`PrintError::NoPages`] for no checks, [`PrintError::BadPosition`] for
/// a first position outside 1–3, and the amount errors of
/// [`crate::amount::dollars_cents`].
pub fn layout_checks(checks: &[CheckData], opts: &CheckOptions) -> Result<Document, PrintError> {
    if checks.is_empty() {
        return Err(PrintError::NoPages);
    }
    let pages = match opts.style {
        CheckStyle::Standard => standard::pages(checks, opts)?,
        CheckStyle::Voucher => voucher::pages(checks, opts)?,
    };
    Ok(Document {
        title: "Checks".into(),
        pages,
        logo: None,
    })
}

/// The alignment test page: sample data in every field with the field
/// outlines, on every check position of the sheet, offset applied.
///
/// # Errors
/// Only if the sample amount could not be written (not expected).
pub fn alignment_test(opts: &CheckOptions) -> Result<Document, PrintError> {
    let sample = CheckData {
        number: "0000".into(),
        date: "12/31/2026".into(),
        payee: opts.labels.sample_payee.clone(),
        amount: Decimal::new(123_456, 2),
        address: vec![
            opts.labels.sample_payee.clone(),
            "123 Main Street, Suite 400".into(),
            "Springfield, ST 12345-6789".into(),
        ],
        memo: opts.labels.sample_memo.clone(),
        stub: StubData::default(),
    };
    let mut page = Page::default();
    let tops: &[f32] = match opts.style {
        CheckStyle::Standard => &standard::POSITION_TOPS,
        CheckStyle::Voucher => &[0.0],
    };
    for (i, top) in tops.iter().enumerate() {
        let pos = u8::try_from(i + 1).unwrap_or(1);
        place_check(&mut page, &sample, pos, *top, opts.offset)?;
        let fields = FIELDS.moved(opts.offset.dx_mm(), top + opts.offset.dy_mm());
        for (_, r) in fields.named() {
            page.shape(Shape::Box {
                rect: r,
                stroke_pt: Some(0.3),
                fill_gray: None,
            });
        }
    }
    Ok(Document {
        title: "Check alignment test".into(),
        pages: vec![page],
        logo: None,
    })
}

/// Page coordinates of a check's fields at `top` with `offset`.
#[must_use]
pub fn fields_at(top: f32, offset: AlignmentOffset) -> CheckFields {
    FIELDS.moved(offset.dx_mm(), top + offset.dy_mm())
}

/// Place one check's six fields, tagged `check{pos}.{field}`.
fn place_check(
    page: &mut Page,
    c: &CheckData,
    pos: u8,
    top: f32,
    offset: AlignmentOffset,
) -> Result<(), PrintError> {
    let f = fields_at(top, offset);
    let tag = |name: &str| format!("check{pos}.{name}");
    page.text(TextBox::new(
        tag("date"),
        f.date,
        Font::Mono,
        10.0,
        c.date.clone(),
    ));
    page.text(TextBox::new(
        tag("payee"),
        f.payee,
        Font::Sans,
        10.0,
        c.payee.clone(),
    ));
    page.text(
        TextBox::new(
            tag("amount"),
            f.amount,
            Font::Mono,
            10.0,
            check_figures(c.amount)?,
        )
        .align(Align::Right)
        .filled('*'),
    );
    page.text(
        TextBox::new(
            tag("words"),
            f.words,
            Font::Sans,
            9.5,
            amount_in_words(c.amount)?,
        )
        .filled('*'),
    );
    page.text(TextBox::new(
        tag("address"),
        f.address,
        Font::Sans,
        10.0,
        c.address.join("\n"),
    ));
    page.text(TextBox::new(
        tag("memo"),
        f.memo,
        Font::Sans,
        9.0,
        c.memo.clone(),
    ));
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::layout::resolve;

    pub(crate) fn sample(n: u32) -> CheckData {
        CheckData {
            number: format!("{}", 1000 + n),
            date: "10/29/2026".into(),
            payee: "Acme Supply Co.".into(),
            amount: Decimal::new(123_456, 2),
            address: vec![
                "Acme Supply Co.".into(),
                "100 Industrial Way".into(),
                "Dayton, OH 45402".into(),
            ],
            memo: "Account 7781".into(),
            stub: StubData::default(),
        }
    }

    /// The baseline origin a left-aligned field's first run must have.
    pub(crate) fn origin(r: Rect, font: Font, size: f32) -> (f32, f32) {
        (r.x, r.y + font.ascent_mm(size))
    }

    #[test]
    fn offsets_are_limited_to_ten_millimetres() {
        assert!(AlignmentOffset::new(10.0, -10.0).is_ok());
        assert_eq!(
            AlignmentOffset::new(10.1, 0.0),
            Err(PrintError::OffsetOutOfRange)
        );
        assert_eq!(
            AlignmentOffset::new(0.0, -10.5),
            Err(PrintError::OffsetOutOfRange)
        );
        assert_eq!(
            AlignmentOffset::new(f32::NAN, 0.0),
            Err(PrintError::OffsetOutOfRange)
        );
    }

    #[test]
    fn the_address_block_shows_through_the_envelope_window_on_both_stocks() {
        assert!(WINDOW_ON_STANDARD.contains(FIELDS.address));
        assert!(WINDOW_ON_VOUCHER.contains(FIELDS.address));
        // Sliding 12.7 mm sideways in the envelope keeps it in the window.
        assert!(
            WINDOW_ON_STANDARD
                .moved(-12.7, 0.0)
                .contains(FIELDS.address)
        );
        // Every line the block can hold stays inside the window.
        let lines = crate::layout::max_lines(FIELDS.address.h, 10.0);
        assert_eq!(lines, 5);
    }

    #[test]
    fn stub_totals_are_exact_sums() {
        let s = StubData {
            bills: vec![
                StubBill {
                    bill_total: Decimal::new(10_010, 2),
                    discount: Decimal::new(200, 2),
                    amount_paid: Decimal::new(9_810, 2),
                    ..StubBill::default()
                },
                StubBill {
                    bill_total: Decimal::new(1, 1),
                    discount: Decimal::ZERO,
                    amount_paid: Decimal::new(2, 1),
                    ..StubBill::default()
                },
            ],
            credits: vec![StubCredit {
                amount: Decimal::new(2_500, 2),
                ..StubCredit::default()
            }],
        };
        let t = s.totals().unwrap();
        assert_eq!(t.bill_total, Decimal::new(10_020, 2));
        assert_eq!(t.discount, Decimal::new(200, 2));
        assert_eq!(t.amount_paid, Decimal::new(9_830, 2));
        assert_eq!(t.credits, Decimal::new(2_500, 2));
        let huge = StubData {
            bills: vec![
                StubBill {
                    bill_total: Decimal::MAX,
                    ..StubBill::default()
                };
                2
            ],
            credits: vec![],
        };
        assert_eq!(huge.totals(), Err(PrintError::AmountTooLarge));
    }

    #[test]
    fn the_alignment_page_outlines_every_field_on_every_position_with_the_offset() {
        let off = AlignmentOffset::new(1.5, -2.0).unwrap();
        for (style, positions) in [(CheckStyle::Standard, 3usize), (CheckStyle::Voucher, 1)] {
            let doc = alignment_test(&CheckOptions {
                style,
                offset: off,
                ..CheckOptions::default()
            })
            .unwrap();
            assert_eq!(doc.pages.len(), 1);
            let page = &resolve(&doc).unwrap()[0];
            assert_eq!(page.shapes.len(), positions * 6);
            for pos in 1..=positions {
                let top = standard::POSITION_TOPS[pos - 1];
                let f = fields_at(top, off);
                let r = page.runs_of(&format!("check{pos}.date")).next().unwrap();
                let (x, y) = origin(f.date, Font::Mono, 10.0);
                assert!((r.x - x).abs() < 0.5 && (r.baseline - y).abs() < 0.5);
                assert!((f.date.x - (FIELDS.date.x + 1.5)).abs() < 0.001);
            }
        }
    }

    #[test]
    fn no_checks_is_nothing_to_print() {
        assert_eq!(
            layout_checks(&[], &CheckOptions::default()),
            Err(PrintError::NoPages)
        );
    }
}
