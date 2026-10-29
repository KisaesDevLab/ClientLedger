//! Voucher check stock: the check on top, then two identical stubs (one
//! for the payee, one for the records) listing the bills paid and the
//! credits used, with exact column totals (Q-213).

use super::{CheckData, CheckLabels, CheckOptions, StubData, place_check};
use crate::PrintError;
use crate::amount::{check_figures, format_amount};
use crate::fonts::Font;
use crate::layout::{Align, Page, Rect, Shape, TextBox};
use rust_decimal::Decimal;

/// Top of each stub on the sheet.
pub const STUB_TOPS: [f32; 2] = [88.9, 184.15];
/// Bill and credit rows a stub can list; more become an "and N more" row.
pub const STUB_MAX_ROWS: usize = 12;
/// Left margin of the stub content.
const LEFT: f32 = 12.7;
/// Right margin of the stub content.
const RIGHT: f32 = 203.2;
/// Row pitch.
const ROW_H: f32 = 4.2;
/// First row, below the stub top.
const FIRST_ROW: f32 = 24.0;

/// A stub column: left edge, width, alignment.
#[derive(Debug, Clone, Copy)]
struct Col {
    x: f32,
    w: f32,
    align: Align,
}

const COLS: [Col; 7] = [
    Col {
        x: LEFT,
        w: 20.0,
        align: Align::Left,
    }, // date
    Col {
        x: 32.7,
        w: 25.0,
        align: Align::Left,
    }, // ref no
    Col {
        x: 57.7,
        w: 30.0,
        align: Align::Left,
    }, // vendor invoice
    Col {
        x: 87.7,
        w: 30.0,
        align: Align::Right,
    }, // bill total
    Col {
        x: 117.7,
        w: 25.0,
        align: Align::Right,
    }, // discount
    Col {
        x: 142.7,
        w: 28.0,
        align: Align::Right,
    }, // credit
    Col {
        x: 170.7,
        w: 32.5,
        align: Align::Right,
    }, // amount paid
];
const COL_NAMES: [&str; 7] = [
    "date",
    "ref",
    "vendor_invoice",
    "bill_total",
    "discount",
    "credit",
    "amount_paid",
];

/// One page per check.
pub(super) fn pages(checks: &[CheckData], opts: &CheckOptions) -> Result<Vec<Page>, PrintError> {
    checks
        .iter()
        .map(|c| {
            let mut page = Page::default();
            place_check(&mut page, c, 1, 0.0, opts.offset)?;
            for (i, top) in STUB_TOPS.iter().enumerate() {
                place_stub(&mut page, c, i + 1, *top, opts)?;
            }
            Ok(page)
        })
        .collect()
}

#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // One linear stub layout.
fn place_stub(
    page: &mut Page,
    c: &CheckData,
    n: usize,
    top: f32,
    opts: &CheckOptions,
) -> Result<(), PrintError> {
    let (dx, dy) = (opts.offset.dx_mm(), opts.offset.dy_mm());
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::new(x + dx, top + y + dy, w, h);
    let l: &CheckLabels = &opts.labels;
    let tag = |name: &str| format!("stub{n}.{name}");

    page.text(TextBox::new(
        tag("payee"),
        at(LEFT, 6.0, 120.0, 5.0),
        Font::SansSemiBold,
        10.0,
        c.payee.clone(),
    ));
    page.text(
        TextBox::new(
            tag("number"),
            at(140.0, 6.0, RIGHT - 140.0, 5.0),
            Font::Sans,
            9.0,
            format!("{} {}", l.check_no, c.number),
        )
        .align(Align::Right),
    );
    page.text(
        TextBox::new(
            tag("date"),
            at(140.0, 11.0, RIGHT - 140.0, 5.0),
            Font::Mono,
            9.0,
            c.date.clone(),
        )
        .align(Align::Right),
    );
    if !c.memo.is_empty() {
        page.text(TextBox::new(
            tag("memo"),
            at(LEFT, 11.0, 120.0, 5.0),
            Font::Sans,
            9.0,
            format!("{}: {}", l.memo, c.memo),
        ));
    }

    let heads = [
        &l.date,
        &l.ref_no,
        &l.vendor_invoice,
        &l.bill_total,
        &l.discount,
        &l.credit,
        &l.amount_paid,
    ];
    for ((col, name), head) in COLS.iter().zip(COL_NAMES).zip(heads) {
        page.text(
            TextBox::new(
                tag(&format!("head.{name}")),
                at(col.x, 18.0, col.w, 4.0),
                Font::SansSemiBold,
                7.5,
                head.clone(),
            )
            .align(col.align),
        );
    }
    page.shape(rule(at(LEFT, 22.5, RIGHT - LEFT, 0.0)));

    let rows = stub_rows(&c.stub, l);
    let shown = if rows.len() > STUB_MAX_ROWS {
        STUB_MAX_ROWS - 1
    } else {
        rows.len()
    };
    let mut y = FIRST_ROW;
    for (i, row) in rows.iter().take(shown).enumerate() {
        for (k, (col, cell)) in COLS.iter().zip(row).enumerate() {
            if cell.is_empty() {
                continue;
            }
            let font = if k >= 3 || k == 0 {
                Font::Mono
            } else {
                Font::Sans
            };
            page.text(
                TextBox::new(
                    tag(&format!("row{i}.{}", COL_NAMES[k])),
                    at(col.x, y, col.w - 1.0, ROW_H),
                    font,
                    8.0,
                    cell.clone(),
                )
                .align(col.align),
            );
        }
        y += ROW_H;
    }
    if shown < rows.len() {
        let more = l
            .more_rows
            .replace("{n}", &(rows.len() - shown).to_string());
        page.text(TextBox::new(
            tag("more"),
            at(LEFT, y, RIGHT - LEFT, ROW_H),
            Font::Sans,
            8.0,
            more,
        ));
    }

    let t = c.stub.totals()?;
    let rule_y = FIRST_ROW + ROW_H * as_f32(STUB_MAX_ROWS) + 1.0;
    page.shape(rule(at(COLS[3].x, rule_y, RIGHT - COLS[3].x, 0.0)));
    let ty = rule_y + 1.0;
    page.text(TextBox::new(
        tag("total.label"),
        at(COLS[2].x, ty, COLS[2].w, ROW_H),
        Font::SansSemiBold,
        8.0,
        l.totals.clone(),
    ));
    for (k, v) in [
        (3, t.bill_total),
        (4, t.discount),
        (5, t.credits),
        (6, t.amount_paid),
    ] {
        let col = COLS[k];
        page.text(
            TextBox::new(
                tag(&format!("total.{}", COL_NAMES[k])),
                at(col.x, ty, col.w - 1.0, ROW_H),
                Font::Mono,
                8.0,
                format_amount(v),
            )
            .align(Align::Right),
        );
    }
    let cy = ty + ROW_H + 1.5;
    page.text(
        TextBox::new(
            tag("check_amount.label"),
            at(COLS[4].x, cy, COLS[5].right_edge() - COLS[4].x, ROW_H),
            Font::SansSemiBold,
            8.5,
            l.check_amount.clone(),
        )
        .align(Align::Right),
    );
    let col = COLS[6];
    page.text(
        TextBox::new(
            tag("check_amount"),
            at(col.x, cy, col.w - 1.0, ROW_H),
            Font::Mono,
            8.5,
            check_figures(c.amount)?,
        )
        .align(Align::Right),
    );
    Ok(())
}

impl Col {
    fn right_edge(self) -> f32 {
        self.x + self.w
    }
}

fn rule(r: Rect) -> Shape {
    Shape::Line {
        x1: r.x,
        y1: r.y,
        x2: r.right(),
        y2: r.y,
        width_pt: 0.5,
    }
}

fn as_f32(n: usize) -> f32 {
    f32::from(u16::try_from(n).unwrap_or(u16::MAX))
}

fn money(v: Decimal) -> String {
    format_amount(v)
}

/// Bill rows, then credit rows, as cell text.
fn stub_rows(s: &StubData, l: &CheckLabels) -> Vec<[String; 7]> {
    let mut rows: Vec<[String; 7]> = s
        .bills
        .iter()
        .map(|b| {
            [
                b.date.clone(),
                b.ref_no.clone(),
                b.vendor_invoice_no.clone(),
                money(b.bill_total),
                money(b.discount),
                String::new(),
                money(b.amount_paid),
            ]
        })
        .collect();
    rows.extend(s.credits.iter().map(|c| {
        [
            c.date.clone(),
            c.ref_no.clone(),
            l.credit_row.clone(),
            String::new(),
            String::new(),
            money(c.amount),
            String::new(),
        ]
    }));
    rows
}

#[cfg(test)]
mod tests {
    use super::super::tests::{origin, sample};
    use super::super::*;
    use super::{STUB_MAX_ROWS, STUB_TOPS};
    use crate::layout::resolve;

    const TOL: f32 = 0.5;

    fn with_stub(bills: usize, credits: usize) -> CheckData {
        let mut c = sample(1);
        c.stub.bills = (0..bills)
            .map(|i| StubBill {
                ref_no: format!("B-{i}"),
                vendor_invoice_no: format!("INV-{i}"),
                date: "10/01/2026".into(),
                bill_total: Decimal::new(50_000 + i64::try_from(i).unwrap(), 2),
                discount: Decimal::new(1_000, 2),
                amount_paid: Decimal::new(49_000, 2),
            })
            .collect();
        c.stub.credits = (0..credits)
            .map(|i| StubCredit {
                ref_no: format!("C-{i}"),
                date: "09/15/2026".into(),
                amount: Decimal::new(2_500, 2),
            })
            .collect();
        c
    }

    fn text_of(page: &crate::ResolvedPage, tag: &str) -> String {
        page.runs_of(tag)
            .map(|r| r.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_check_fields_and_window_address_are_within_half_a_millimetre() {
        for off in [
            AlignmentOffset::NONE,
            AlignmentOffset::new(2.0, -1.5).unwrap(),
        ] {
            let opts = CheckOptions {
                style: CheckStyle::Voucher,
                offset: off,
                ..CheckOptions::default()
            };
            let doc = layout_checks(&[with_stub(2, 1)], &opts).unwrap();
            assert_eq!(doc.pages.len(), 1);
            let page = &resolve(&doc).unwrap()[0];
            for (name, font, size, rect) in [
                ("date", Font::Mono, 10.0, FIELDS.date),
                ("payee", Font::Sans, 10.0, FIELDS.payee),
                ("words", Font::Sans, 9.5, FIELDS.words),
                ("address", Font::Sans, 10.0, FIELDS.address),
                ("memo", Font::Sans, 9.0, FIELDS.memo),
            ] {
                let r = page.runs_of(&format!("check1.{name}")).next().unwrap();
                let (x, y) = origin(rect.moved(off.dx_mm(), off.dy_mm()), font, size);
                assert!(
                    (r.x - x).abs() <= TOL && (r.baseline - y).abs() <= TOL,
                    "{name}"
                );
            }
            let a = page.runs_of("check1.amount").next().unwrap();
            let right = a.x + Font::Mono.width_mm(&a.text, 10.0);
            assert!((right - (FIELDS.amount.right() + off.dx_mm())).abs() <= TOL);
            let window = WINDOW_ON_VOUCHER.moved(off.dx_mm(), off.dy_mm());
            for l in page.runs_of("check1.address") {
                assert!(l.x >= window.x && l.baseline <= window.bottom());
                assert!(l.baseline - Font::Sans.ascent_mm(10.0) >= window.y - 0.001);
            }
            // The stubs start at their tops, moved by the offset.
            for (n, top) in STUB_TOPS.iter().enumerate() {
                let p = page
                    .runs_of(&format!("stub{}.payee", n + 1))
                    .next()
                    .unwrap();
                let (x, y) = origin(
                    crate::Rect::new(12.7, top + 6.0, 0.0, 0.0).moved(off.dx_mm(), off.dy_mm()),
                    Font::SansSemiBold,
                    10.0,
                );
                assert!((p.x - x).abs() <= TOL && (p.baseline - y).abs() <= TOL);
            }
        }
    }

    #[test]
    fn both_stubs_list_bills_and_credits_with_exact_totals() {
        let doc = layout_checks(
            &[with_stub(3, 1)],
            &CheckOptions {
                style: CheckStyle::Voucher,
                ..CheckOptions::default()
            },
        )
        .unwrap();
        let page = &resolve(&doc).unwrap()[0];
        for n in 1..=2 {
            let s = |t: &str| text_of(page, &format!("stub{n}.{t}"));
            assert_eq!(s("row0.ref"), "B-0");
            assert_eq!(s("row0.vendor_invoice"), "INV-0");
            assert_eq!(s("row2.bill_total"), "500.02");
            assert_eq!(s("row0.discount"), "10.00");
            assert_eq!(s("row0.amount_paid"), "490.00");
            assert_eq!(s("row3.vendor_invoice"), "Credit");
            assert_eq!(s("row3.credit"), "25.00");
            assert_eq!(s("total.bill_total"), "1,500.03");
            assert_eq!(s("total.discount"), "30.00");
            assert_eq!(s("total.credit"), "25.00");
            assert_eq!(s("total.amount_paid"), "1,470.00");
            assert_eq!(s("check_amount"), "1,234.56");
            assert_eq!(s("number"), "Check no. 1001");
            assert_eq!(s("memo"), "Memo: Account 7781");
            assert_eq!(s("more"), "");
        }
    }

    #[test]
    fn long_stubs_show_a_more_row_but_total_everything() {
        let doc = layout_checks(
            &[with_stub(15, 2)],
            &CheckOptions {
                style: CheckStyle::Voucher,
                ..CheckOptions::default()
            },
        )
        .unwrap();
        let page = &resolve(&doc).unwrap()[0];
        let last = STUB_MAX_ROWS - 2;
        assert!(
            page.runs_of(&format!("stub1.row{last}.ref"))
                .next()
                .is_some()
        );
        assert!(
            page.runs_of(&format!("stub1.row{}.ref", last + 1))
                .next()
                .is_none()
        );
        assert_eq!(
            text_of(page, "stub1.more"),
            "and 6 more (see the bill payment)"
        );
        assert_eq!(text_of(page, "stub1.total.discount"), "150.00");
        assert_eq!(text_of(page, "stub1.total.credit"), "50.00");
        // The more row and the totals stay inside the stub.
        let total = page.runs_of("stub2.check_amount").next().unwrap();
        assert!(total.baseline < crate::layout::LETTER_H_MM - 5.0);
        let first_total = page.runs_of("stub1.check_amount").next().unwrap();
        assert!(first_total.baseline < STUB_TOPS[1]);
    }

    #[test]
    fn one_page_per_voucher_check() {
        let doc = layout_checks(
            &[sample(1), sample(2)],
            &CheckOptions {
                style: CheckStyle::Voucher,
                ..CheckOptions::default()
            },
        )
        .unwrap();
        assert_eq!(doc.pages.len(), 2);
    }
}
