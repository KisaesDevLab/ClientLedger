//! Checks on pre-printed stock (Q-14, Q-P19g): standard (three per page),
//! voucher (one check and two stubs; the default style, owner 2026-10-29)
//! and z-fold (the engine's pressure-seal self-mailer: stub, check and
//! mailing panel on one sheet, Q-P19ar). No MICR line and no signature
//! (Q-14, Q-212): the bank
//! details and the check number are already on the stock. An optional
//! company block (logo, name, address) prints on the check, off by
//! default.
//!
//! # Geometry (Q-P19h)
//!
//! Original numbers derived from common 8.5 × 11 in business check stock,
//! whose checks are 8.5 in wide and 3.5 in (88.9 mm) tall. Millimetres on
//! US Letter, from the top-left corner.
//!
//! - **Standard:** three checks stacked from the top of the sheet at an
//!   88.9 mm pitch (tops 0, 88.9, 177.8; the last 0.5 in is a tear strip).
//!   Always starts at position 1; positions left over on the last sheet stay
//!   blank.
//! - **Voucher:** the check is the top 88.9 mm; the two 3.75 in (95.25 mm)
//!   stubs start at 88.9 and 184.15 mm.
//! - **Z-fold (Q-P19ar):** the engine's pressure-seal self-mailer,
//!   number for number ([`zfold`]): folds at 93.1 and 186.3 mm (3.667 and
//!   7.333 in); the remittance stub in the top panel (from 7.6 mm, ending
//!   10 pt above the first fold); the check is the 70.4 mm (2.771 in)
//!   coupon between the tear perforations at 103.2 and 173.6 mm, with its
//!   own compact field positions inside 17 mm / 15 mm margins; the bottom
//!   panel (from 190.5 mm) is the mailing face with the return address and
//!   the payee's delivery address, clear of the bottom 5/8 in. No envelope.
//! - **#10 double-window envelope (Q-93), standard and voucher:**
//!   9.5 × 4.125 in; the recipient window is 4.5 × 1.125 in, 7/8 in from
//!   the left and 5/8 in from the bottom. A standard check, its bottom edge
//!   on the envelope's bottom, sees the window 44.4–73.0 mm below the
//!   check's top; the voucher sheet's top panel (folded at 93.1 mm, fold at
//!   the bottom) sees it 48.6–77.2 mm down. The address block (49.5–71.5 mm
//!   down, from 25.4 mm across) shows through both, even if the check
//!   slides 12.7 mm sideways.
//!
//! The alignment offset ([`AlignmentOffset`], ±10 mm) is saved per printer
//! on this computer by `cl-settings` (owner 2026-10-29; the stock style
//! stays per bank account in the engine) and moves everything printed on
//! the page. It corrects a printer that feeds the sheet a little off; it
//! cannot correct stock with a different check pitch.

pub mod standard;
pub mod voucher;
pub mod zfold;

use crate::PrintError;
use crate::amount::{amount_in_words, check_figures};
use crate::fonts::Font;
use crate::forms::CompanyInfo;
use crate::image::Logo;
use crate::layout::{Align, Document, Element, LETTER_W_MM, Page, Rect, Shape, TextBox};
use rust_decimal::Decimal;

/// Height of one check.
pub const CHECK_H_MM: f32 = 88.9;
/// Largest alignment offset either way.
pub const MAX_OFFSET_MM: f32 = 10.0;

/// The recipient window of a #10 double-window envelope, in the
/// coordinates of a check whose bottom edge sits on the envelope's bottom
/// (standard checks).
pub const WINDOW_ON_STANDARD: Rect = Rect::new(22.2, 44.4, 114.3, 28.6);
/// The recipient window over a tri-folded voucher sheet's check.
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
    /// The payee's address block (#10 window); none on the z-fold, whose
    /// address prints on the mailing panel.
    pub address: Option<Rect>,
    /// The memo line.
    pub memo: Rect,
}

/// The check layout standard and voucher stock share (the z-fold's
/// compact coupon has its own, [`zfold::face`]).
pub const FIELDS: CheckFields = CheckFields {
    date: Rect::new(165.0, 17.0, 38.0, 6.0),
    payee: Rect::new(30.0, 30.0, 125.0, 6.0),
    amount: Rect::new(167.0, 30.0, 36.0, 6.0),
    words: Rect::new(12.0, 40.0, 150.0, 6.0),
    address: Some(Rect::new(25.4, 49.5, 95.0, 22.0)),
    memo: Rect::new(20.0, 76.0, 80.0, 6.0),
};

/// How one stock's check face prints: where the fields go and the type in
/// them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CheckFace {
    /// Field positions, relative to the check's top-left corner.
    pub fields: CheckFields,
    /// How the date sits in its box.
    pub date_align: Align,
    /// The payee's face.
    pub payee_font: Font,
    /// The payee's size.
    pub payee_pt: f32,
    /// The memo's size.
    pub memo_pt: f32,
    /// Height of the check (the alignment ruler covers it).
    pub height: f32,
}

/// Standard and voucher checks.
pub const STANDARD_FACE: CheckFace = CheckFace {
    fields: FIELDS,
    date_align: Align::Left,
    payee_font: Font::Sans,
    payee_pt: 10.0,
    memo_pt: 9.0,
    height: CHECK_H_MM,
};

/// The optional company block on a check (top left, above the date line).
pub const COMPANY_LOGO: Rect = Rect::new(10.0, 4.0, 22.0, 11.0);
/// Company name and address, beside the logo.
pub const COMPANY_TEXT_X_WITH_LOGO: f32 = 34.0;
/// Company name and address without a logo.
pub const COMPANY_TEXT_X: f32 = 12.0;

impl CheckFields {
    /// Every field moved by (`dx`, `dy`).
    #[must_use]
    pub fn moved(self, dx: f32, dy: f32) -> CheckFields {
        CheckFields {
            date: self.date.moved(dx, dy),
            payee: self.payee.moved(dx, dy),
            amount: self.amount.moved(dx, dy),
            words: self.words.moved(dx, dy),
            address: self.address.map(|r| r.moved(dx, dy)),
            memo: self.memo.moved(dx, dy),
        }
    }

    /// The fields with their names (for outlines and tests).
    #[must_use]
    pub fn named(&self) -> Vec<(&'static str, Rect)> {
        let mut out = vec![
            ("date", self.date),
            ("payee", self.payee),
            ("amount", self.amount),
            ("words", self.words),
        ];
        if let Some(a) = self.address {
            out.push(("address", a));
        }
        out.push(("memo", self.memo));
        out
    }
}

/// Check stock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CheckStyle {
    /// One check and two stubs per page (the default for an account with
    /// no setting).
    #[default]
    Voucher,
    /// Three checks per page.
    Standard,
    /// The engine's pressure-seal self-mailer: stub, check coupon and
    /// mailing panel on one sheet (Q-P19ar).
    ZFold,
}

impl CheckStyle {
    /// Tops of the check positions on a sheet.
    #[must_use]
    pub fn check_tops(self) -> &'static [f32] {
        match self {
            CheckStyle::Standard => &standard::POSITION_TOPS,
            CheckStyle::Voucher => &[0.0],
            CheckStyle::ZFold => &[zfold::COUPON_TOP],
        }
    }

    /// The check face: field positions and type.
    #[must_use]
    pub fn face(self) -> CheckFace {
        match self {
            CheckStyle::Standard | CheckStyle::Voucher => STANDARD_FACE,
            CheckStyle::ZFold => zfold::face(),
        }
    }

    /// The #10 envelope window over the check, in the check's
    /// coordinates; none for the z-fold self-mailer, which needs no
    /// envelope.
    #[must_use]
    pub fn window(self) -> Option<Rect> {
        match self {
            CheckStyle::Voucher => Some(WINDOW_ON_VOUCHER),
            CheckStyle::Standard => Some(WINDOW_ON_STANDARD),
            CheckStyle::ZFold => None,
        }
    }

    /// True when the checks go out in a #10 envelope (Envelopes is
    /// offered); the z-fold mails itself.
    #[must_use]
    pub fn uses_envelope(self) -> bool {
        self.window().is_some()
    }
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
    /// Paid by this check (the bills add up to the check amount).
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

/// An expense or item line of a plain check (not a bill payment).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StubLine {
    /// Account or item name.
    pub name: String,
    /// The line amount (the lines add up to the check amount).
    pub amount: Decimal,
}

/// What a voucher stub lists: bills paid and credits used for a bill
/// payment; expense and item lines for a plain check (owner 2026-10-29).
/// A stub with bills or credits shows the bills table; otherwise the lines.
/// On voucher stock the amounts paid (bill payments) or the line amounts
/// (plain checks) must add up to the check amount, else
/// [`PrintError::StubMismatch`]; credits used are listed beside them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StubData {
    /// Bills paid.
    pub bills: Vec<StubBill>,
    /// Credits used.
    pub credits: Vec<StubCredit>,
    /// Expense and item lines (plain checks).
    pub lines: Vec<StubLine>,
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
    /// Sum of the expense and item lines.
    pub lines: Decimal,
}

impl StubData {
    /// True when the stub lists bills (a bill payment).
    #[must_use]
    pub fn is_bill_payment(&self) -> bool {
        !self.bills.is_empty() || !self.credits.is_empty()
    }

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
        for l in &self.lines {
            t.lines = add(t.lines, l.amount)?;
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
    /// What the voucher stubs list (ignored on other stock).
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
    /// Column: account or item (plain checks).
    pub account_or_item: String,
    /// Column: amount (plain checks).
    pub amount: String,
    /// Totals row.
    pub totals: String,
    /// Plain-check total row.
    pub total: String,
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
    /// The z-fold mailing panel's note when the payee has no address.
    pub no_address: String,
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
            account_or_item: "Account or item".into(),
            amount: "Amount".into(),
            totals: "Totals".into(),
            total: "Total".into(),
            check_amount: "Check amount".into(),
            memo: "Memo".into(),
            more_rows: "and {n} more (see the check in ClientLedger)".into(),
            sample_payee: "Sample Payee Name".into(),
            sample_memo: "Alignment test".into(),
            no_address: "\u{2014} no mailing address on file \u{2014}".into(),
        }
    }
}

/// How to print a batch of checks.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckOptions {
    /// The stock (per bank account, from the engine).
    pub style: CheckStyle,
    /// The chosen printer's feed correction (from `cl-settings`).
    pub offset: AlignmentOffset,
    /// Print the company block on each check (off by default).
    pub show_company: bool,
    /// The company block's text.
    pub company: CompanyInfo,
    /// The company logo for the block, if any.
    pub logo: Option<Logo>,
    /// Fixed words.
    pub labels: CheckLabels,
}

impl Default for CheckOptions {
    fn default() -> CheckOptions {
        CheckOptions {
            style: CheckStyle::Voucher,
            offset: AlignmentOffset::NONE,
            show_company: false,
            company: CompanyInfo::default(),
            logo: None,
            labels: CheckLabels::default(),
        }
    }
}

/// Lay out checks: three per standard sheet from position 1, one page
/// per voucher or z-fold check. Voucher and z-fold stubs whose listed
/// amounts don't add up to the check amount are refused
/// ([`PrintError::StubMismatch`]).
///
/// # Errors
/// [`PrintError::NoPages`] for no checks, and the amount errors of
/// [`crate::amount::dollars_cents`].
pub fn layout_checks(checks: &[CheckData], opts: &CheckOptions) -> Result<Document, PrintError> {
    if checks.is_empty() {
        return Err(PrintError::NoPages);
    }
    let pages = match opts.style {
        CheckStyle::Standard => standard::pages(checks, opts)?,
        CheckStyle::Voucher => voucher::pages(checks, opts)?,
        CheckStyle::ZFold => zfold::pages(checks, opts)?,
    };
    Ok(Document {
        size: crate::layout::PageSize::LETTER,
        title: "Checks".into(),
        pages,
        logo: logo_for(opts),
    })
}

fn logo_for(opts: &CheckOptions) -> Option<Logo> {
    if opts.show_company {
        opts.logo.clone()
    } else {
        None
    }
}

/// The alignment test page, printed on plain paper: a filled-in sample
/// check (offset applied) with its field outlines, over a millimetre ruler
/// grid at fixed paper positions, for every check position of the stock.
/// On z-fold stock the fold guides and the mailing panel print too. The
/// owner holds it over a blank check to see how far to move.
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
    let height = opts.style.face().height;
    if opts.style == CheckStyle::ZFold {
        zfold::place_mailer(&mut page, &sample, opts);
    }
    for (i, top) in opts.style.check_tops().iter().enumerate() {
        ruler_grid(&mut page, *top, height, i + 1);
        let pos = u8::try_from(i + 1).unwrap_or(1);
        place_check(&mut page, &sample, pos, *top, opts)?;
        for (_, r) in fields_at(opts.style, *top, opts.offset).named() {
            page.shape(Shape::Box {
                rect: r,
                stroke_pt: Some(0.6),
                fill_gray: None,
            });
        }
    }
    Ok(Document {
        size: crate::layout::PageSize::LETTER,
        title: "Check alignment test".into(),
        pages: vec![page],
        logo: logo_for(opts),
    })
}

/// Grid lines every 5 mm (heavier every 10 mm) over one check panel
/// `height` tall, with millimetre numbers every 10 mm along its top and
/// left edges, measured from the panel's top-left corner. Never offset.
fn ruler_grid(page: &mut Page, top: f32, height: f32, pos: usize) {
    let bottom = top + height;
    let mut step = 0u16;
    loop {
        let d = f32::from(step) * 5.0;
        if d > LETTER_W_MM {
            break;
        }
        let major = step.is_multiple_of(2);
        page.shape(Shape::Line {
            x1: d,
            y1: top,
            x2: d,
            y2: bottom,
            width_pt: if major { 0.25 } else { 0.1 },
        });
        if major && step > 0 {
            page.text(
                TextBox::new(
                    format!("ruler{pos}.x{}", step * 5),
                    Rect::new(d + 0.3, top + 0.3, 8.0, 2.5),
                    Font::Mono,
                    5.0,
                    (step * 5).to_string(),
                )
                .unclipped(),
            );
        }
        step += 1;
    }
    let mut step = 0u16;
    loop {
        let d = f32::from(step) * 5.0;
        if d > height {
            break;
        }
        let major = step.is_multiple_of(2);
        page.shape(Shape::Line {
            x1: 0.0,
            y1: top + d,
            x2: LETTER_W_MM,
            y2: top + d,
            width_pt: if major { 0.25 } else { 0.1 },
        });
        if major && step > 0 {
            page.text(
                TextBox::new(
                    format!("ruler{pos}.y{}", step * 5),
                    Rect::new(0.5, top + d + 0.3, 8.0, 2.5),
                    Font::Mono,
                    5.0,
                    (step * 5).to_string(),
                )
                .unclipped(),
            );
        }
        step += 1;
    }
}

/// Size of the written amount when it fits on one line.
pub const WORDS_PT: f32 = 9.5;
/// The smallest size the written amount shrinks to.
pub const WORDS_MIN_PT: f32 = 7.0;

/// The written amount is never cut off: it prints at [`WORDS_PT`] with
/// asterisk fill, shrinking in half-point steps to [`WORDS_MIN_PT`] when
/// it is long; amounts over a billion that still do not fit wrap onto two
/// lines at the smallest size (no fill).
fn words_box(tag: String, r: Rect, words: String) -> TextBox {
    let mut size = WORDS_PT;
    while size >= WORDS_MIN_PT {
        if Font::Sans.width_mm(&words, size) <= r.w {
            return TextBox::new(tag, r, Font::Sans, size, words).filled('*');
        }
        size -= 0.5;
    }
    TextBox::new(tag, r, Font::Sans, WORDS_MIN_PT, words).wrapped()
}

/// Page coordinates of a `style` check's fields at `top` with `offset`.
#[must_use]
pub fn fields_at(style: CheckStyle, top: f32, offset: AlignmentOffset) -> CheckFields {
    style
        .face()
        .fields
        .moved(offset.dx_mm(), top + offset.dy_mm())
}

/// Place one check's fields, tagged `check{pos}.{field}`, and the company
/// block when it is on.
fn place_check(
    page: &mut Page,
    c: &CheckData,
    pos: u8,
    top: f32,
    opts: &CheckOptions,
) -> Result<(), PrintError> {
    let off = opts.offset;
    let face = opts.style.face();
    let f = face.fields.moved(off.dx_mm(), top + off.dy_mm());
    let tag = |name: &str| format!("check{pos}.{name}");
    page.text(
        TextBox::new(tag("date"), f.date, Font::Mono, 10.0, c.date.clone()).align(face.date_align),
    );
    page.text(TextBox::new(
        tag("payee"),
        f.payee,
        face.payee_font,
        face.payee_pt,
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
    page.text(words_box(tag("words"), f.words, amount_in_words(c.amount)?));
    if let Some(address) = f.address {
        page.text(TextBox::new(
            tag("address"),
            address,
            Font::Sans,
            10.0,
            c.address.join("\n"),
        ));
    }
    page.text(TextBox::new(
        tag("memo"),
        f.memo,
        Font::Sans,
        face.memo_pt,
        c.memo.clone(),
    ));
    if opts.show_company && opts.style == CheckStyle::ZFold {
        zfold::place_company(page, pos, top, opts);
    } else if opts.show_company {
        let (dx, dy) = (off.dx_mm(), top + off.dy_mm());
        let x = if opts.logo.is_some() {
            page.elements
                .push(Element::Logo(COMPANY_LOGO.moved(dx, dy)));
            COMPANY_TEXT_X_WITH_LOGO
        } else {
            COMPANY_TEXT_X
        };
        page.text(TextBox::new(
            tag("company.name"),
            Rect::new(x, 4.0, 90.0, 4.5).moved(dx, dy),
            Font::SansBold,
            9.0,
            opts.company.name.clone(),
        ));
        page.text(TextBox::new(
            tag("company.lines"),
            Rect::new(x, 8.5, 90.0, 8.0).moved(dx, dy),
            Font::Sans,
            7.5,
            opts.company.lines.join("\n"),
        ));
    }
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

    /// Every check field's first run against the style's spec + top +
    /// offset (±0.5 mm), and every address line inside the envelope window
    /// (no address on the face when the style has no window).
    pub(crate) fn assert_check_at(
        page: &crate::ResolvedPage,
        pos: usize,
        top: f32,
        off: AlignmentOffset,
        style: CheckStyle,
    ) {
        const TOL: f32 = 0.5;
        let face = style.face();
        let f = face.fields;
        let spec = |r: Rect| r.moved(off.dx_mm(), top + off.dy_mm());
        let mut fields = vec![
            ("payee", face.payee_font, face.payee_pt, f.payee),
            ("words", Font::Sans, 9.5, f.words),
            ("memo", Font::Sans, face.memo_pt, f.memo),
        ];
        if face.date_align == Align::Left {
            fields.push(("date", Font::Mono, 10.0, f.date));
        }
        if let Some(a) = f.address {
            fields.push(("address", Font::Sans, 10.0, a));
        }
        for (name, font, size, rect) in fields {
            let r = page
                .runs_of(&format!("check{pos}.{name}"))
                .next()
                .unwrap_or_else(|| panic!("{pos}.{name}"));
            let (x, y) = origin(spec(rect), font, size);
            assert!((r.x - x).abs() <= TOL, "check{pos}.{name} x {} vs {x}", r.x);
            assert!(
                (r.baseline - y).abs() <= TOL,
                "check{pos}.{name} y {} vs {y}",
                r.baseline
            );
        }
        if face.date_align == Align::Center {
            let d = page.runs_of(&format!("check{pos}.date")).next().unwrap();
            let s = spec(f.date);
            let mid = d.x + Font::Mono.width_mm(&d.text, 10.0) / 2.0;
            assert!((mid - (s.x + s.w / 2.0)).abs() <= TOL, "date centre {mid}");
            assert!((d.baseline - (s.y + Font::Mono.ascent_mm(10.0))).abs() <= TOL);
        }
        let a = page.runs_of(&format!("check{pos}.amount")).next().unwrap();
        let right = a.x + Font::Mono.width_mm(&a.text, 10.0);
        let s = spec(f.amount);
        assert!(
            (right - s.right()).abs() <= TOL && a.x >= s.x - 0.001,
            "amount {right}"
        );
        assert!((a.baseline - (s.y + Font::Mono.ascent_mm(10.0))).abs() <= TOL);
        assert!(a.text.ends_with("1,234.56") && a.text.starts_with('*'));
        let lines: Vec<_> = page.runs_of(&format!("check{pos}.address")).collect();
        let Some(window) = style.window() else {
            assert!(lines.is_empty(), "no address on a {style:?} check face");
            return;
        };
        let w = spec(window);
        assert_eq!(lines.len(), 3);
        for l in lines {
            assert!(l.x >= w.x && l.baseline <= w.bottom());
            assert!(l.baseline - Font::Sans.ascent_mm(10.0) >= w.y - 0.001);
        }
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
    fn voucher_is_the_default_style() {
        assert_eq!(CheckStyle::default(), CheckStyle::Voucher);
        assert_eq!(CheckOptions::default().style, CheckStyle::Voucher);
        assert!(!CheckOptions::default().show_company);
    }

    #[test]
    fn the_address_block_shows_through_the_envelope_window_on_envelope_stock() {
        let address = FIELDS.address.unwrap();
        assert!(WINDOW_ON_STANDARD.contains(address));
        assert!(WINDOW_ON_VOUCHER.contains(address));
        // Sliding 12.7 mm sideways in the envelope keeps it in the window.
        assert!(WINDOW_ON_STANDARD.moved(-12.7, 0.0).contains(address));
        assert_eq!(crate::layout::max_lines(address.h, 10.0), 5);
        // Standard and voucher keep their shared fields; the z-fold mails
        // itself, so it has no window, no envelope and no face address.
        assert_eq!(CheckStyle::Standard.face().fields, FIELDS);
        assert_eq!(CheckStyle::Voucher.face().fields, FIELDS);
        assert!(CheckStyle::Standard.uses_envelope() && CheckStyle::Voucher.uses_envelope());
        assert!(!CheckStyle::ZFold.uses_envelope());
        assert!(CheckStyle::ZFold.face().fields.address.is_none());
    }

    #[test]
    fn the_company_block_prints_only_when_on() {
        let company = CompanyInfo {
            name: "Kisaes Garden Supply".into(),
            lines: vec!["12 Elm Street".into(), "Bloomington, IN 47401".into()],
        };
        let off = AlignmentOffset::new(1.0, 1.0).unwrap();
        let mut opts = CheckOptions {
            style: CheckStyle::Standard,
            offset: off,
            company,
            ..CheckOptions::default()
        };
        let page = &resolve(&layout_checks(&[sample(1)], &opts).unwrap()).unwrap()[0];
        assert!(page.runs_of("check1.company.name").next().is_none());
        opts.show_company = true;
        let page = &resolve(&layout_checks(&[sample(1)], &opts).unwrap()).unwrap()[0];
        let n = page.runs_of("check1.company.name").next().unwrap();
        assert!((n.x - (COMPANY_TEXT_X + 1.0)).abs() < 0.001);
        assert_eq!(page.runs_of("check1.company.lines").count(), 2);
        // Above the date line; never over a field.
        let last = page.runs_of("check1.company.lines").last().unwrap();
        assert!(last.baseline < FIELDS.date.y + 1.0);
        opts.logo = Some(Logo::from_bytes(&crate::image::tests::png_rgba(4, 2)).unwrap());
        let doc = layout_checks(&[sample(1)], &opts).unwrap();
        assert!(doc.logo.is_some());
        let page = &resolve(&doc).unwrap()[0];
        assert_eq!(page.logos.len(), 1);
        assert!(
            (page.runs_of("check1.company.name").next().unwrap().x
                - (COMPANY_TEXT_X_WITH_LOGO + 1.0))
                .abs()
                < 0.001
        );
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
            lines: vec![
                StubLine {
                    name: "Rent".into(),
                    amount: Decimal::new(1, 2),
                },
                StubLine {
                    name: "Fuel".into(),
                    amount: Decimal::new(2, 2),
                },
            ],
        };
        let t = s.totals().unwrap();
        assert_eq!(t.bill_total, Decimal::new(10_020, 2));
        assert_eq!(t.discount, Decimal::new(200, 2));
        assert_eq!(t.amount_paid, Decimal::new(9_830, 2));
        assert_eq!(t.credits, Decimal::new(2_500, 2));
        assert_eq!(t.lines, Decimal::new(3, 2));
        let huge = StubData {
            bills: vec![
                StubBill {
                    bill_total: Decimal::MAX,
                    ..StubBill::default()
                };
                2
            ],
            ..StubData::default()
        };
        assert_eq!(huge.totals(), Err(PrintError::AmountTooLarge));
    }

    #[test]
    fn the_alignment_page_has_a_sample_check_and_a_fixed_ruler_on_every_position() {
        let off = AlignmentOffset::new(1.5, -2.0).unwrap();
        for style in [CheckStyle::Standard, CheckStyle::Voucher, CheckStyle::ZFold] {
            let opts = CheckOptions {
                style,
                offset: off,
                ..CheckOptions::default()
            };
            let doc = alignment_test(&opts).unwrap();
            assert_eq!(doc.pages.len(), 1);
            let page = &resolve(&doc).unwrap()[0];
            for (i, top) in style.check_tops().iter().enumerate() {
                let pos = i + 1;
                assert_check_at(page, pos, *top, off, style);
                // The ruler is not offset: "50" sits just right of x = 50 mm.
                let mark = page.runs_of(&format!("ruler{pos}.x50")).next().unwrap();
                assert!((mark.x - 50.3).abs() < 0.001);
                let mark = page.runs_of(&format!("ruler{pos}.y40")).next().unwrap();
                assert!((mark.baseline - (top + 40.3 + Font::Mono.ascent_mm(5.0))).abs() < 0.001);
            }
            let outlines = page
                .shapes
                .iter()
                .filter(|s| {
                    matches!(
                        s,
                        Shape::Box {
                            stroke_pt: Some(_),
                            ..
                        }
                    )
                })
                .count();
            let per_check = style.face().fields.named().len();
            assert_eq!(per_check, if style == CheckStyle::ZFold { 5 } else { 6 });
            assert_eq!(outlines, style.check_tops().len() * per_check);
        }
    }

    #[test]
    fn the_written_amount_is_in_capitals_with_fill_and_never_cut_off() {
        for (amount, single_line) in [
            (Decimal::new(123_456, 2), true),
            (Decimal::new(1_999_999_999, 2), true),
            (Decimal::new(777_777_777_777, 2), false),
        ] {
            let mut c = sample(1);
            c.amount = amount;
            let doc = layout_checks(&[c], &CheckOptions::default()).unwrap();
            let page = &resolve(&doc).unwrap()[0];
            let runs: Vec<_> = page.runs_of("check1.words").collect();
            let words = amount_in_words(amount).unwrap();
            let printed: Vec<String> = runs
                .iter()
                .map(|r| r.text.trim_end_matches('*').trim_end().to_string())
                .collect();
            assert_eq!(printed.join(" "), words);
            assert_eq!(runs.len() == 1, single_line, "{amount}");
            for r in &runs {
                assert!(
                    r.x + Font::Sans.width_mm(&r.text, r.size_pt) <= FIELDS.words.right() + 0.01
                );
            }
            if single_line {
                assert!(
                    runs[0].text.ends_with("/100 *") || runs[0].text.ends_with("**"),
                    "{}",
                    runs[0].text
                );
            }
        }
    }

    #[test]
    fn the_largest_check_amount_prints_whole_at_full_size() {
        let mut c = sample(1);
        c.amount = Decimal::new(999_999_999_999, 2);
        for style in [CheckStyle::Standard, CheckStyle::Voucher, CheckStyle::ZFold] {
            let doc = layout_checks(
                &[c.clone()],
                &CheckOptions {
                    style,
                    ..CheckOptions::default()
                },
            )
            .unwrap();
            let page = &resolve(&doc).unwrap()[0];
            let a = page.runs_of("check1.amount").next().unwrap();
            assert!(a.text.ends_with("9,999,999,999.99"), "{}", a.text);
            assert!((a.size_pt - 10.0).abs() < 0.001);
            let box_ = style.face().fields.amount;
            assert!(a.x >= box_.x - 0.001, "{style:?}");
            assert!(a.x + Font::Mono.width_mm(&a.text, 10.0) <= box_.right() + 0.001);
        }
        c.amount = Decimal::new(1_000_000_000_000, 2);
        assert_eq!(
            layout_checks(&[c], &CheckOptions::default()),
            Err(PrintError::AmountTooLarge)
        );
    }

    #[test]
    fn a_stub_total_that_differs_from_the_check_amount_is_refused() {
        let mut c = sample(1);
        c.stub.lines = vec![StubLine {
            name: "Rent".into(),
            amount: Decimal::new(100, 2),
        }];
        assert_eq!(
            layout_checks(&[c.clone()], &CheckOptions::default()),
            Err(PrintError::StubMismatch)
        );
        c.stub.lines.clear();
        c.stub.bills = vec![StubBill {
            amount_paid: Decimal::new(123_455, 2),
            ..StubBill::default()
        }];
        assert_eq!(
            layout_checks(&[c.clone()], &CheckOptions::default()),
            Err(PrintError::StubMismatch)
        );
        // The z-fold's top-panel stub follows the same rule.
        let zfold = CheckOptions {
            style: CheckStyle::ZFold,
            ..CheckOptions::default()
        };
        assert_eq!(
            layout_checks(&[c.clone()], &zfold),
            Err(PrintError::StubMismatch)
        );
        // Standard stock prints no stub, so nothing to compare.
        let opts = CheckOptions {
            style: CheckStyle::Standard,
            ..CheckOptions::default()
        };
        assert!(layout_checks(&[c], &opts).is_ok());
    }

    #[test]
    fn no_checks_is_nothing_to_print() {
        assert_eq!(
            layout_checks(&[], &CheckOptions::default()),
            Err(PrintError::NoPages)
        );
    }
}
