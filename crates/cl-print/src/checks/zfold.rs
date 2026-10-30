//! Z-fold stock: the engine's pressure-seal self-mailer (Q-P19ar, owner
//! 2026-10-30). One check per sheet, laid out like the engine's `z_fold`
//! PDF so the same stock prints the same from either app:
//!
//! - the sheet folds at 3.667 in and 7.333 in from the top (light dashed
//!   guides);
//! - the top panel is the remittance stub (the voucher stub's content,
//!   between 0.3 in from the top and 10 pt above the first fold);
//! - the check is the coupon between the tear perforations at 4.0625 in and
//!   6.8335 in (2.771 in tall), with compact field positions and 17 mm /
//!   15 mm side margins;
//! - the bottom panel is the outside mailing face: the company's return
//!   address top left and the payee's delivery address 3 in from the left,
//!   in capitals, clear of the bottom 5/8 in. It always prints; without it
//!   the folded piece has no address.
//!
//! No #10 envelope is involved. As on every stock there is no
//! MICR line and no signature (Q-14, Q-212), and the face prints only the
//! filled-in values: the labels, rules and amount box are on the stock.

use super::voucher::{self, StubGeometry};
use super::{CheckData, CheckFace, CheckFields, CheckOptions, place_check};
use crate::PrintError;
use crate::fonts::{Font, MM_PER_PT};
use crate::layout::{Align, LETTER_H_MM, LETTER_W_MM, Page, Rect, Shape, TextBox, line_height_mm};

const IN: f32 = 25.4;

/// First fold, from the top of the sheet (3.667 in).
pub const FOLD_1: f32 = 3.667 * IN;
/// Second fold (7.333 in).
pub const FOLD_2: f32 = 7.333 * IN;
/// Fold guides run from 0.15 in in from each side.
pub const FOLD_INSET: f32 = 0.15 * IN;
/// Fold guide stroke (0.5 pt), dash and gap length (3 pt each), grey.
pub const FOLD_DASH: f32 = 3.0 * MM_PER_PT;
/// Fold guide thickness.
pub const FOLD_STROKE: f32 = 0.5 * MM_PER_PT;
/// Fold guide grey (0 = black).
pub const FOLD_GRAY: f32 = 0.86;

/// Top of the check coupon: the upper tear perforation (4.0625 in).
pub const COUPON_TOP: f32 = 4.0625 * IN;
/// Height of the check coupon (2.771 in; lower perforation at 6.8335 in).
pub const COUPON_H: f32 = 2.771 * IN;

/// Left margin of the stub, the check face and the mailing panel (17 mm).
pub const LEFT: f32 = 17.0;
/// Right margin (15 mm).
pub const RIGHT_MARGIN: f32 = 15.0;
/// Right edge of the content.
pub const RIGHT: f32 = LETTER_W_MM - RIGHT_MARGIN;

/// Top of the stub panel's content (0.3 in).
pub const STUB_TOP: f32 = 0.3 * IN;
/// The stub's content stays above this: 10 pt above the first fold.
pub const STUB_BOTTOM: f32 = FOLD_1 - 10.0 * MM_PER_PT;
/// The stub: the voucher stub's content in the z-fold margins, ten rows
/// so the totals stay above [`STUB_BOTTOM`].
pub(super) const STUB: StubGeometry = StubGeometry {
    left: LEFT,
    right: RIGHT,
    max_rows: 10,
};

/// Top of the mailing panel (7.5 in).
pub const PANEL_TOP: f32 = 7.5 * IN;
/// Bottom of the mailing panel (0.4 in above the bottom of the sheet).
pub const PANEL_BOTTOM: f32 = LETTER_H_MM - 0.4 * IN;
/// The mailing panel's text is lowered by 10 mm.
const PANEL_DROP: f32 = 10.0;
/// First return-address baseline: 14 pt + 10 mm below the panel top.
pub const RETURN_FIRST_BASELINE: f32 = PANEL_TOP + 14.0 * MM_PER_PT + PANEL_DROP;
/// Return-address line pitch (9 pt).
pub const RETURN_PITCH: f32 = 9.0 * MM_PER_PT;
/// Return-address width (half the sheet).
pub const RETURN_W: f32 = LETTER_W_MM / 2.0;
/// The delivery address block's left edge (3 in).
pub const DELIVERY_X: f32 = 3.0 * IN;
/// Delivery address size (11 pt) and line pitch (13 pt).
pub const DELIVERY_PT: f32 = 11.0;
/// Delivery address line pitch.
pub const DELIVERY_PITCH: f32 = 13.0 * MM_PER_PT;
/// First delivery baseline when it fits: 20 pt above the panel's middle,
/// lowered 10 mm.
pub const DELIVERY_FIRST_BASELINE: f32 =
    f32::midpoint(PANEL_TOP, PANEL_BOTTOM) - 20.0 * MM_PER_PT + PANEL_DROP;
/// The bottom 5/8 in of the panel stays clear (USPS barcode area).
pub const CLEAR_BAND: f32 = 0.625 * IN;
/// The delivery block (its line count times the pitch, from the first
/// baseline) ends above this.
pub const DELIVERY_LIMIT: f32 = PANEL_BOTTOM - CLEAR_BAND;
/// Size of the "no mailing address" note.
pub const NO_ADDRESS_PT: f32 = 7.5;
/// Grey of the "no mailing address" note.
pub const NO_ADDRESS_GRAY: f32 = 0.4;

/// Baselines on the coupon, below its top (engine `drawCheckFace`,
/// compact).
mod face_at {
    use super::{IN, MM_PER_PT};
    /// Company name: 7.2 pt + 7.5 pt.
    pub const COMPANY_NAME: f32 = 14.7 * MM_PER_PT;
    /// Company rows below the name (7 pt pitch).
    pub const COMPANY_PITCH: f32 = 7.0 * MM_PER_PT;
    /// Date: 0.5 in + 8.25 pt.
    pub const DATE: f32 = 0.5 * IN + 8.25 * MM_PER_PT;
    /// Payee name: 0.85 in + 6 pt + 10 pt.
    pub const PAYEE: f32 = 0.85 * IN + 16.0 * MM_PER_PT;
    /// Amount: in the 1.35 in x 22 pt box whose top is 0.82 in down; the
    /// engine's baseline there.
    pub const AMOUNT: f32 = 0.82 * IN + 15.0 * MM_PER_PT;
    /// Amount in words: 1.3 in + 13 pt, 3 pt above its rule.
    pub const WORDS: f32 = 1.3 * IN + 10.0 * MM_PER_PT;
    /// Memo: 2.5 pt above the rule 0.65 in from the coupon's bottom.
    pub const MEMO: f32 = (2.771 - 0.65) * IN - 2.5 * MM_PER_PT;
}

/// The amount box: 1.35 in wide, flush with the right margin.
pub const AMOUNT_BOX_W: f32 = 1.35 * IN;

/// Sizes on the compact face.
const PAYEE_PT: f32 = 9.0;
const MEMO_PT: f32 = 6.75;
const DATE_PT: f32 = 10.0;
const AMOUNT_PT: f32 = 10.0;
const WORDS_TOP_PT: f32 = super::WORDS_PT;
const COMPANY_NAME_PT: f32 = 7.5;
const COMPANY_LINE_PT: f32 = 6.5;
/// Company rows printed under the name (street, street 2, city, phone).
const COMPANY_MAX_LINES: usize = 4;
/// The company logo on the coupon, left of the name.
pub const COMPANY_LOGO: Rect = Rect::new(LEFT, 1.5, 22.0, 9.0);
/// The company name's left edge beside the logo.
pub const COMPANY_TEXT_X_WITH_LOGO: f32 = LEFT + 24.0;

/// A rect whose first baseline is `baseline` for `font` at `size`.
fn on_baseline(x: f32, baseline: f32, w: f32, font: Font, size: f32) -> Rect {
    Rect::new(x, baseline - font.ascent_mm(size), w, line_height_mm(size))
}

/// The compact check face, relative to the coupon's top-left corner.
#[must_use]
pub fn face() -> CheckFace {
    let date_left = RIGHT - 100.0 * MM_PER_PT;
    // The payee rule ends 2.1 in from the right edge; the name stops 6 pt
    // short of it.
    let payee_w = LETTER_W_MM - 2.1 * IN - 6.0 * MM_PER_PT - LEFT;
    // Room is left on the right for DOLLARS (60 pt).
    let words_w = RIGHT - LEFT - 60.0 * MM_PER_PT;
    // The memo starts after the stock's MEMO label (26 pt) and runs 3 in
    // less 30 pt.
    let memo_x = LEFT + 26.0 * MM_PER_PT;
    let memo_w = 3.0 * IN - 30.0 * MM_PER_PT;
    let words = on_baseline(LEFT, face_at::WORDS, words_w, Font::Sans, WORDS_TOP_PT);
    CheckFace {
        fields: CheckFields {
            date: on_baseline(
                date_left,
                face_at::DATE,
                RIGHT - date_left,
                Font::Mono,
                DATE_PT,
            ),
            payee: on_baseline(LEFT, face_at::PAYEE, payee_w, Font::SansBold, PAYEE_PT),
            amount: on_baseline(
                RIGHT - AMOUNT_BOX_W,
                face_at::AMOUNT,
                AMOUNT_BOX_W,
                Font::Mono,
                AMOUNT_PT,
            ),
            // Two lines of the smallest written amount fit.
            words: Rect { h: 6.0, ..words },
            address: None,
            memo: on_baseline(memo_x, face_at::MEMO, memo_w, Font::Sans, MEMO_PT),
        },
        date_align: Align::Center,
        payee_font: Font::SansBold,
        payee_pt: PAYEE_PT,
        memo_pt: MEMO_PT,
        height: COUPON_H,
    }
}

/// One page per check: fold guides, stub, check, mailing panel.
pub(super) fn pages(checks: &[CheckData], opts: &CheckOptions) -> Result<Vec<Page>, PrintError> {
    checks
        .iter()
        .map(|c| {
            let mut page = Page::default();
            place_mailer(&mut page, c, opts);
            voucher::place_stub(&mut page, c, 1, STUB_TOP, STUB, opts)?;
            place_check(&mut page, c, 1, COUPON_TOP, opts)?;
            Ok(page)
        })
        .collect()
}

/// The fold guides and the mailing panel, moved by the offset.
pub(super) fn place_mailer(page: &mut Page, c: &CheckData, opts: &CheckOptions) {
    let (dx, dy) = (opts.offset.dx_mm(), opts.offset.dy_mm());
    for fold in [FOLD_1, FOLD_2] {
        fold_guide(page, fold + dy, dx);
    }
    place_return_address(page, opts, dx, dy);
    place_delivery_address(page, c, opts, dx, dy);
}

/// A light dashed line across the sheet at `y`, drawn as short grey bars
/// so every renderer (PDF and the Windows printer path) shows the dashes.
fn fold_guide(page: &mut Page, y: f32, dx: f32) {
    let end = LETTER_W_MM - FOLD_INSET + dx;
    let mut x = FOLD_INSET + dx;
    while x < end - 0.001 {
        let w = FOLD_DASH.min(end - x);
        page.shape(Shape::Box {
            rect: Rect::new(x, y - FOLD_STROKE / 2.0, w, FOLD_STROKE),
            stroke_pt: None,
            fill_gray: Some(FOLD_GRAY),
        });
        x += 2.0 * FOLD_DASH;
    }
}

/// The company's return address, top left of the panel, in capitals.
fn place_return_address(page: &mut Page, opts: &CheckOptions, dx: f32, dy: f32) {
    let lines = std::iter::once(&opts.company.name)
        .chain(&opts.company.lines)
        .filter(|l| !l.trim().is_empty());
    let mut baseline = RETURN_FIRST_BASELINE;
    for (i, line) in lines.enumerate() {
        let (font, size) = if i == 0 {
            (Font::SansBold, 7.5)
        } else {
            (Font::Sans, 7.0)
        };
        page.text(TextBox::new(
            "mail.return",
            on_baseline(LEFT, baseline, RETURN_W, font, size).moved(dx, dy),
            font,
            size,
            line.trim().to_uppercase(),
        ));
        baseline += RETURN_PITCH;
    }
}

/// The payee's name and address, 3 in from the left, in capitals; a grey
/// note when there is no address on file.
fn place_delivery_address(page: &mut Page, c: &CheckData, opts: &CheckOptions, dx: f32, dy: f32) {
    // The address block is name first; without one, just the payee.
    let mut lines: Vec<&str> = c
        .address
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() && !c.payee.trim().is_empty() {
        lines.push(c.payee.trim());
    }
    let has_address = lines.len() > 1;
    let n = f32::from(u8::try_from(lines.len()).unwrap_or(u8::MAX));
    let mut baseline = DELIVERY_FIRST_BASELINE;
    if baseline + n * DELIVERY_PITCH > DELIVERY_LIMIT {
        baseline = DELIVERY_LIMIT - n * DELIVERY_PITCH;
    }
    let w = RIGHT - DELIVERY_X;
    for (i, line) in lines.iter().enumerate() {
        let font = if i == 0 { Font::SansBold } else { Font::Sans };
        page.text(TextBox::new(
            "mail.to",
            on_baseline(DELIVERY_X, baseline, w, font, DELIVERY_PT).moved(dx, dy),
            font,
            DELIVERY_PT,
            line.to_uppercase(),
        ));
        baseline += DELIVERY_PITCH;
    }
    if !has_address {
        page.text(
            TextBox::new(
                "mail.none",
                on_baseline(DELIVERY_X, baseline, w, Font::Sans, NO_ADDRESS_PT).moved(dx, dy),
                Font::Sans,
                NO_ADDRESS_PT,
                opts.labels.no_address.clone(),
            )
            .gray(NO_ADDRESS_GRAY),
        );
    }
}

/// The company block on the coupon: name in bold, then its rows at a 7 pt
/// pitch, logo (if any) to the left.
pub(super) fn place_company(page: &mut Page, pos: u8, top: f32, opts: &CheckOptions) {
    let (dx, dy) = (opts.offset.dx_mm(), top + opts.offset.dy_mm());
    let x = if opts.logo.is_some() {
        page.elements
            .push(crate::layout::Element::Logo(COMPANY_LOGO.moved(dx, dy)));
        COMPANY_TEXT_X_WITH_LOGO
    } else {
        LEFT
    };
    let w = 90.0;
    let mut baseline = face_at::COMPANY_NAME;
    page.text(TextBox::new(
        format!("check{pos}.company.name"),
        on_baseline(x, baseline, w, Font::SansBold, COMPANY_NAME_PT).moved(dx, dy),
        Font::SansBold,
        COMPANY_NAME_PT,
        opts.company.name.clone(),
    ));
    for line in opts
        .company
        .lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .take(COMPANY_MAX_LINES)
    {
        baseline += face_at::COMPANY_PITCH;
        page.text(TextBox::new(
            format!("check{pos}.company.lines"),
            on_baseline(x, baseline, w, Font::Sans, COMPANY_LINE_PT).moved(dx, dy),
            Font::Sans,
            COMPANY_LINE_PT,
            line.clone(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{assert_check_at, sample};
    use super::super::*;
    use super::*;
    use crate::layout::resolve;
    use crate::pdf::read_back::{content_streams, text_origins};
    use crate::pdf::{pt, render_resolved};

    const TOL: f32 = 0.5;

    fn zfold(off: AlignmentOffset) -> CheckOptions {
        CheckOptions {
            style: CheckStyle::ZFold,
            offset: off,
            company: crate::forms::CompanyInfo {
                name: "Kisaes Garden Supply".into(),
                lines: vec!["12 Elm Street".into(), "Bloomington, IN 47401".into()],
            },
            ..CheckOptions::default()
        }
    }

    fn page_of(c: CheckData, off: AlignmentOffset) -> crate::ResolvedPage {
        let doc = layout_checks(&[c], &zfold(off)).unwrap();
        assert_eq!(doc.pages.len(), 1);
        resolve(&doc).unwrap().remove(0)
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() <= TOL
    }

    fn nth(i: usize) -> f32 {
        f32::from(u8::try_from(i).unwrap())
    }

    /// The fold-guide dashes on the page: (y of the centre line, first x,
    /// last right edge, count).
    fn guides(page: &crate::ResolvedPage) -> Vec<(f32, f32, f32, usize)> {
        let mut out: Vec<(f32, f32, f32, usize)> = Vec::new();
        for s in &page.shapes {
            if let Shape::Box {
                rect,
                stroke_pt: None,
                fill_gray: Some(g),
            } = s
            {
                assert!((g - FOLD_GRAY).abs() < 0.001);
                let y = rect.y + rect.h / 2.0;
                match out.iter_mut().find(|e| (e.0 - y).abs() < 0.01) {
                    Some(e) => {
                        e.2 = rect.right();
                        e.3 += 1;
                    }
                    None => out.push((y, rect.x, rect.right(), 1)),
                }
            }
        }
        out
    }

    #[test]
    fn the_sheet_matches_the_mybooks_pressure_seal_numbers() {
        // Engine check-pdf.service.ts z_fold, in mm from the top.
        assert!((FOLD_1 - 93.142).abs() < 0.01);
        assert!((FOLD_2 - 186.258).abs() < 0.01);
        assert!((COUPON_TOP - 103.188).abs() < 0.01);
        assert!((COUPON_TOP + COUPON_H - 173.571).abs() < 0.01);
        assert!((STUB_TOP - 7.62).abs() < 0.01);
        assert!((STUB_BOTTOM - 89.614).abs() < 0.01);
        assert!((PANEL_TOP - 190.5).abs() < 0.01);
        assert!((PANEL_BOTTOM - 269.24).abs() < 0.01);
        assert!((LEFT - 17.0).abs() < 0.001 && (RIGHT - 200.9).abs() < 0.001);
        // The coupon sits between the folds; the stub between the top and
        // the first fold.
        const { assert!(FOLD_1 < COUPON_TOP && COUPON_TOP + COUPON_H < FOLD_2) };
        assert!(STUB_TOP + STUB.content_height() <= STUB_BOTTOM);
        // Dashed fold guides 0.15 in in from each side.
        let page = page_of(sample(1), AlignmentOffset::NONE);
        let g = guides(&page);
        assert_eq!(g.len(), 2);
        for ((y, x1, x2, n), fold) in g.into_iter().zip([93.142, 186.258]) {
            assert!(near(y, fold), "fold {y}");
            assert!(near(x1, 3.81) && near(x2, 212.09), "{x1}-{x2}");
            assert!(n > 90, "{n} dashes");
        }
    }

    #[test]
    fn the_check_fields_sit_in_the_coupon_where_mybooks_puts_them() {
        let f = face().fields;
        // Inside the 2.771 in coupon and the 17 / 15 mm margins.
        for (name, r) in f.named() {
            assert!(r.y >= 0.0 && r.bottom() <= COUPON_H, "{name} {r:?}");
            assert!(
                r.x >= LEFT - 0.001 && r.right() <= RIGHT + 0.001,
                "{name} {r:?}"
            );
        }
        assert!(f.address.is_none());
        for off in [
            AlignmentOffset::NONE,
            AlignmentOffset::new(-2.5, 3.0).unwrap(),
        ] {
            let doc = layout_checks(&[sample(1), sample(2)], &zfold(off)).unwrap();
            assert_eq!(doc.pages.len(), 2);
            for page in &resolve(&doc).unwrap() {
                assert_check_at(page, 1, COUPON_TOP, off, CheckStyle::ZFold);
                // Engine baselines (mm below the coupon top) and left edges.
                let at = |tag: &str| page.runs_of(&format!("check1.{tag}")).next().unwrap();
                let top = COUPON_TOP + off.dy_mm();
                for (tag, baseline, x) in [
                    ("payee", 27.234, Some(17.0)),
                    ("words", 36.548, Some(17.0)),
                    ("memo", 52.991, Some(26.172)),
                    ("date", 15.610, None),
                    ("amount", 26.119, None),
                ] {
                    let r = at(tag);
                    assert!(
                        near(r.baseline - top, baseline),
                        "{tag} {}",
                        r.baseline - top
                    );
                    if let Some(x) = x {
                        assert!(near(r.x - off.dx_mm(), x), "{tag} x {}", r.x);
                    }
                }
                // Date centred on 165.6-200.9; amount flush right in the
                // 1.35 in box ending at the right margin.
                let d = at("date");
                let mid = d.x + Font::Mono.width_mm(&d.text, 10.0) / 2.0 - off.dx_mm();
                assert!(near(mid, f32::midpoint(165.622, 200.9)), "date {mid}");
                let a = at("amount");
                let right = a.x + Font::Mono.width_mm(&a.text, a.size_pt) - off.dx_mm();
                assert!(near(right, 200.9) && a.x - off.dx_mm() >= 166.61 - 0.001);
                assert!(page.runs_of("check1.address").next().is_none());
            }
        }
    }

    #[test]
    fn the_top_panel_is_the_voucher_stub_and_it_stays_above_the_fold() {
        let mut c = sample(1);
        c.stub.bills = (0..15)
            .map(|i| StubBill {
                ref_no: format!("B-{i}"),
                amount_paid: Decimal::new(10_000, 2),
                ..StubBill::default()
            })
            .collect();
        c.amount = c.stub.totals().unwrap().amount_paid;
        let page = page_of(c, AlignmentOffset::NONE);
        let p = page.runs_of("stub1.payee").next().unwrap();
        assert!(near(p.x, 17.0));
        assert!(near(
            p.baseline,
            7.62 + 6.0 + Font::SansSemiBold.ascent_mm(10.0)
        ));
        // Ten rows: nine bills and an "and 6 more" row.
        assert!(page.runs_of("stub1.row8.ref").next().is_some());
        assert!(page.runs_of("stub1.row9.ref").next().is_none());
        assert_eq!(
            page.runs_of("stub1.more").next().unwrap().text,
            "and 6 more (see the check in ClientLedger)"
        );
        let total = page.runs_of("stub1.check_amount").next().unwrap();
        assert_eq!(total.text, "1,500.00");
        for r in page.runs.iter().filter(|r| r.tag.starts_with("stub")) {
            assert!(r.baseline < STUB_BOTTOM, "{}", r.tag);
            assert!(r.x >= 17.0 - 0.001, "{}", r.tag);
            let right = r.x + r.font.width_mm(&r.text, r.size_pt);
            assert!(right <= RIGHT + 0.01, "{} {right}", r.tag);
        }
        assert!(page.runs_of("stub2.payee").next().is_none());
    }

    #[test]
    fn the_mailing_panel_carries_the_return_and_delivery_addresses() {
        let page = page_of(sample(1), AlignmentOffset::NONE);
        let ret: Vec<_> = page.runs_of("mail.return").collect();
        let texts: Vec<&str> = ret.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "KISAES GARDEN SUPPLY",
                "12 ELM STREET",
                "BLOOMINGTON, IN 47401"
            ]
        );
        for (i, r) in ret.iter().enumerate() {
            assert!(near(r.x, 17.0));
            // 7.5 in + 14 pt + 10 mm, then 9 pt apart.
            let want = 205.439 + 3.175 * nth(i);
            assert!(near(r.baseline, want), "return {i} {}", r.baseline);
            let (font, size) = if i == 0 {
                (Font::SansBold, 7.5)
            } else {
                (Font::Sans, 7.0)
            };
            assert!(r.font == font && (r.size_pt - size).abs() < 0.001);
        }
        let to: Vec<_> = page.runs_of("mail.to").collect();
        let texts: Vec<&str> = to.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(
            texts,
            ["ACME SUPPLY CO.", "100 INDUSTRIAL WAY", "DAYTON, OH 45402"]
        );
        for (i, r) in to.iter().enumerate() {
            assert!(near(r.x, 76.2));
            // Panel middle + 20 pt up + 10 mm down, then 13 pt apart.
            let want = 232.814 + 4.586 * nth(i);
            assert!(near(r.baseline, want), "to {i} {}", r.baseline);
            assert!((r.size_pt - 11.0).abs() < 0.001);
            assert_eq!(r.font, if i == 0 { Font::SansBold } else { Font::Sans });
            // Above the bottom 5/8 in.
            assert!(r.baseline < 269.24 - 15.875);
        }
        assert!(page.runs_of("mail.none").next().is_none());
        // The return address prints whether or not the company block is on
        // the check face.
        assert!(page.runs_of("check1.company.name").next().is_none());
    }

    #[test]
    fn a_long_delivery_address_moves_up_to_stay_clear_of_the_bottom_band() {
        let mut c = sample(1);
        c.address = vec![
            "Acme Supply Co.".into(),
            "Attn: Accounts Payable".into(),
            "Building 4".into(),
            "100 Industrial Way".into(),
            "Suite 200".into(),
            "Dayton, OH 45402".into(),
        ];
        let page = page_of(c, AlignmentOffset::NONE);
        let to: Vec<_> = page.runs_of("mail.to").collect();
        assert_eq!(to.len(), 6);
        // The engine's rule: first baseline + lines x 13 pt = 5/8 in above
        // the panel bottom.
        assert!(
            near(to[0].baseline + 6.0 * 4.586, 253.365),
            "{}",
            to[0].baseline
        );
        assert!(to[5].baseline < 253.365);
    }

    #[test]
    fn no_address_on_file_prints_a_grey_note_under_the_name() {
        for address in [vec!["Acme Supply Co.".to_string()], Vec::new()] {
            let mut c = sample(1);
            c.address = address;
            let page = page_of(c, AlignmentOffset::NONE);
            let to: Vec<_> = page.runs_of("mail.to").collect();
            assert_eq!(to.len(), 1);
            assert_eq!(to[0].text, "ACME SUPPLY CO.");
            let note = page.runs_of("mail.none").next().unwrap();
            assert_eq!(note.text, "\u{2014} no mailing address on file \u{2014}");
            assert!((note.gray - 0.4).abs() < 0.001 && (note.size_pt - 7.5).abs() < 0.001);
            assert!(near(note.x, 76.2) && near(note.baseline, 232.814 + 4.586));
        }
    }

    #[test]
    fn the_offset_moves_everything_on_the_sheet() {
        let mut c = sample(1);
        c.stub.lines = vec![StubLine {
            name: "Rent".into(),
            amount: c.amount,
        }];
        let mut opts = zfold(AlignmentOffset::NONE);
        opts.show_company = true;
        let base = resolve(&layout_checks(&[c.clone()], &opts).unwrap()).unwrap();
        let (dx, dy) = (3.5, -4.25);
        opts.offset = AlignmentOffset::new(dx, dy).unwrap();
        let moved = resolve(&layout_checks(&[c], &opts).unwrap()).unwrap();
        let (before, after) = (&base[0], &moved[0]);
        for t in [
            "mail.return",
            "mail.to",
            "stub1.payee",
            "check1.company.name",
            "check1.payee",
        ] {
            assert!(before.runs_of(t).next().is_some(), "{t}");
        }
        assert_eq!(before.runs.len(), after.runs.len());
        for (r, m) in before.runs.iter().zip(&after.runs) {
            assert_eq!(r.tag, m.tag);
            assert!(
                (m.x - r.x - dx).abs() < 0.001 && (m.baseline - r.baseline - dy).abs() < 0.001,
                "{}",
                r.tag
            );
        }
        assert_eq!(before.shapes.len(), after.shapes.len());
        for (s, m) in before.shapes.iter().zip(&after.shapes) {
            let ((x, y), (mx, my)) = match (s, m) {
                (Shape::Box { rect: r, .. }, Shape::Box { rect: q, .. }) => {
                    ((r.x, r.y), (q.x, q.y))
                }
                (Shape::Line { x1, y1, .. }, Shape::Line { x1: q1, y1: w1, .. }) => {
                    ((*x1, *y1), (*q1, *w1))
                }
                _ => panic!("shape kinds differ"),
            };
            assert!((mx - x - dx).abs() < 0.001 && (my - y - dy).abs() < 0.001);
        }
    }

    #[test]
    fn the_company_block_fits_above_the_date_on_the_coupon() {
        let mut opts = zfold(AlignmentOffset::NONE);
        opts.show_company = true;
        let page = &resolve(&layout_checks(&[sample(1)], &opts).unwrap()).unwrap()[0];
        let n = page.runs_of("check1.company.name").next().unwrap();
        assert!(near(n.x, 17.0) && near(n.baseline - COUPON_TOP, 5.186));
        let lines: Vec<_> = page.runs_of("check1.company.lines").collect();
        assert_eq!(lines.len(), 2);
        assert!(near(lines[1].baseline - lines[0].baseline, 2.469));
        let payee = page.runs_of("check1.payee").next().unwrap();
        assert!(lines[1].baseline < payee.baseline - 5.0);
    }

    #[test]
    fn the_pdf_draws_every_run_where_the_layout_put_it_and_the_note_in_grey() {
        let mut c = sample(1);
        c.address = vec![c.payee.clone()];
        let doc = layout_checks(&[c], &zfold(AlignmentOffset::new(1.0, 1.0).unwrap())).unwrap();
        let pages = resolve(&doc).unwrap();
        let bytes = render_resolved(&pages, None, "Checks").unwrap();
        let streams = content_streams(&bytes);
        assert_eq!(streams.len(), 1);
        let origins = text_origins(&streams[0]);
        assert_eq!(origins.len(), pages[0].runs.len());
        for ((x, y), run) in origins.iter().zip(&pages[0].runs) {
            let (xm, ym) = (x * MM_PER_PT, (pt(LETTER_H_MM) - y) * MM_PER_PT);
            assert!(
                (xm - run.x).abs() < 0.01 && (ym - run.baseline).abs() < 0.01,
                "{}",
                run.tag
            );
        }
        // The note is the only grey text; the fold guides are grey fills.
        assert_eq!(streams[0].matches("0.4 g").count(), 1);
        assert!(streams[0].contains("0.86 g"));
    }
}
