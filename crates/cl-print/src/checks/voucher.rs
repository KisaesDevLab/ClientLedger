//! Voucher check stock: the check on top, then two identical stubs (one
//! for the payee, one for the records). A bill payment's stubs list the
//! bills paid and the credits used, with exact column totals (Q-213); a
//! plain check's stubs list its expense and item lines and their total
//! (owner 2026-10-29).

use super::{CheckData, CheckLabels, CheckOptions, StubData, place_check};
use crate::PrintError;
use crate::amount::{check_figures, format_amount};
use crate::fonts::Font;
use crate::layout::{Align, Page, Rect, Shape, TextBox};
use rust_decimal::Decimal;

/// Top of each stub on the sheet.
pub const STUB_TOPS: [f32; 2] = [88.9, 184.15];
/// Rows a stub can list; more become an "and N more" row.
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

impl Col {
    const fn new(x: f32, w: f32, align: Align) -> Col {
        Col { x, w, align }
    }

    fn right_edge(self) -> f32 {
        self.x + self.w
    }
}

/// Bill-payment columns: date, ref no, vendor invoice, bill total,
/// discount, credit, amount paid.
const BILL_COLS: [Col; 7] = [
    Col::new(LEFT, 20.0, Align::Left),
    Col::new(32.7, 25.0, Align::Left),
    Col::new(57.7, 30.0, Align::Left),
    Col::new(87.7, 30.0, Align::Right),
    Col::new(117.7, 25.0, Align::Right),
    Col::new(142.7, 28.0, Align::Right),
    Col::new(170.7, 32.5, Align::Right),
];
const BILL_NAMES: [&str; 7] = [
    "date",
    "ref",
    "vendor_invoice",
    "bill_total",
    "discount",
    "credit",
    "amount_paid",
];
const BILL_MONO: [bool; 7] = [true, false, false, true, true, true, true];

/// Plain-check columns: account or item, amount.
const LINE_COLS: [Col; 2] = [
    Col::new(LEFT, 150.0, Align::Left),
    Col::new(170.7, 32.5, Align::Right),
];
const LINE_NAMES: [&str; 2] = ["name", "amount"];
const LINE_MONO: [bool; 2] = [false, true];

/// One page per check.
pub(super) fn pages(checks: &[CheckData], opts: &CheckOptions) -> Result<Vec<Page>, PrintError> {
    checks
        .iter()
        .map(|c| {
            let mut page = Page::default();
            place_check(&mut page, c, 1, 0.0, opts)?;
            for (i, top) in STUB_TOPS.iter().enumerate() {
                place_stub(&mut page, c, i + 1, *top, opts)?;
            }
            Ok(page)
        })
        .collect()
}

/// A stub table: columns, tag names, headings, which columns are numbers,
/// rows, and the totals row (label column and text, then value columns).
struct Table<'a> {
    cols: &'a [Col],
    names: &'a [&'a str],
    heads: Vec<String>,
    mono: &'a [bool],
    rows: Vec<Vec<String>>,
    total_label: (usize, String),
    totals: Vec<(usize, Decimal)>,
}

/// Positions within one stub, offset applied.
struct StubFrame {
    n: usize,
    top: f32,
    dx: f32,
    dy: f32,
}

impl StubFrame {
    fn at(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::new(x + self.dx, self.top + y + self.dy, w, h)
    }

    fn tag(&self, name: &str) -> String {
        format!("stub{}.{name}", self.n)
    }
}

#[allow(clippy::too_many_lines)] // One linear stub layout.
fn place_stub(
    page: &mut Page,
    c: &CheckData,
    n: usize,
    top: f32,
    opts: &CheckOptions,
) -> Result<(), PrintError> {
    let fr = StubFrame {
        n,
        top,
        dx: opts.offset.dx_mm(),
        dy: opts.offset.dy_mm(),
    };
    let l: &CheckLabels = &opts.labels;
    page.text(TextBox::new(
        fr.tag("payee"),
        fr.at(LEFT, 6.0, 120.0, 5.0),
        Font::SansSemiBold,
        10.0,
        c.payee.clone(),
    ));
    page.text(
        TextBox::new(
            fr.tag("number"),
            fr.at(140.0, 6.0, RIGHT - 140.0, 5.0),
            Font::Sans,
            9.0,
            format!("{} {}", l.check_no, c.number),
        )
        .align(Align::Right),
    );
    page.text(
        TextBox::new(
            fr.tag("date"),
            fr.at(140.0, 11.0, RIGHT - 140.0, 5.0),
            Font::Mono,
            9.0,
            c.date.clone(),
        )
        .align(Align::Right),
    );
    if !c.memo.is_empty() {
        page.text(TextBox::new(
            fr.tag("memo"),
            fr.at(LEFT, 11.0, 120.0, 5.0),
            Font::Sans,
            9.0,
            format!("{}: {}", l.memo, c.memo),
        ));
    }
    let t = c.stub.totals()?;
    let paid = if c.stub.is_bill_payment() {
        t.amount_paid
    } else {
        t.lines
    };
    let listed = c.stub.is_bill_payment() || !c.stub.lines.is_empty();
    if listed && paid != c.amount {
        return Err(PrintError::StubMismatch);
    }
    let table = if c.stub.is_bill_payment() {
        Table {
            cols: &BILL_COLS,
            names: &BILL_NAMES,
            heads: [
                &l.date,
                &l.ref_no,
                &l.vendor_invoice,
                &l.bill_total,
                &l.discount,
                &l.credit,
                &l.amount_paid,
            ]
            .into_iter()
            .cloned()
            .collect(),
            mono: &BILL_MONO,
            rows: bill_rows(&c.stub, l),
            total_label: (2, l.totals.clone()),
            totals: vec![
                (3, t.bill_total),
                (4, t.discount),
                (5, t.credits),
                (6, t.amount_paid),
            ],
        }
    } else {
        Table {
            cols: &LINE_COLS,
            names: &LINE_NAMES,
            heads: vec![l.account_or_item.clone(), l.amount.clone()],
            mono: &LINE_MONO,
            rows: c
                .stub
                .lines
                .iter()
                .map(|x| vec![x.name.clone(), format_amount(x.amount)])
                .collect(),
            total_label: (0, l.total.clone()),
            totals: vec![(1, t.lines)],
        }
    };
    place_table(page, &fr, &table, l);

    let cy = FIRST_ROW + ROW_H * as_f32(STUB_MAX_ROWS) + 2.0 + ROW_H + 1.5;
    page.text(
        TextBox::new(
            fr.tag("check_amount.label"),
            fr.at(
                BILL_COLS[4].x,
                cy,
                BILL_COLS[5].right_edge() - BILL_COLS[4].x,
                ROW_H,
            ),
            Font::SansSemiBold,
            8.5,
            l.check_amount.clone(),
        )
        .align(Align::Right),
    );
    let col = BILL_COLS[6];
    page.text(
        TextBox::new(
            fr.tag("check_amount"),
            fr.at(col.x, cy, col.w - 1.0, ROW_H),
            Font::Mono,
            8.5,
            check_figures(c.amount)?,
        )
        .align(Align::Right),
    );
    Ok(())
}

fn place_table(page: &mut Page, fr: &StubFrame, t: &Table<'_>, l: &CheckLabels) {
    for ((col, name), head) in t.cols.iter().zip(t.names).zip(&t.heads) {
        page.text(
            TextBox::new(
                fr.tag(&format!("head.{name}")),
                fr.at(col.x, 18.0, col.w, 4.0),
                Font::SansSemiBold,
                7.5,
                head.clone(),
            )
            .align(col.align),
        );
    }
    page.shape(rule(fr.at(LEFT, 22.5, RIGHT - LEFT, 0.0)));
    let shown = if t.rows.len() > STUB_MAX_ROWS {
        STUB_MAX_ROWS - 1
    } else {
        t.rows.len()
    };
    let mut y = FIRST_ROW;
    for (i, row) in t.rows.iter().take(shown).enumerate() {
        for (k, (col, cell)) in t.cols.iter().zip(row).enumerate() {
            if cell.is_empty() {
                continue;
            }
            let font = if t.mono[k] { Font::Mono } else { Font::Sans };
            page.text(
                TextBox::new(
                    fr.tag(&format!("row{i}.{}", t.names[k])),
                    fr.at(col.x, y, col.w - 1.0, ROW_H),
                    font,
                    8.0,
                    cell.clone(),
                )
                .align(col.align),
            );
        }
        y += ROW_H;
    }
    if shown < t.rows.len() {
        let more = l
            .more_rows
            .replace("{n}", &(t.rows.len() - shown).to_string());
        page.text(TextBox::new(
            fr.tag("more"),
            fr.at(LEFT, y, RIGHT - LEFT, ROW_H),
            Font::Sans,
            8.0,
            more,
        ));
    }
    let rule_y = FIRST_ROW + ROW_H * as_f32(STUB_MAX_ROWS) + 1.0;
    let rx = t.cols[t.totals.first().map_or(0, |(k, _)| *k)].x;
    page.shape(rule(fr.at(rx, rule_y, RIGHT - rx, 0.0)));
    let ty = rule_y + 1.0;
    let lc = t.cols[t.total_label.0];
    page.text(TextBox::new(
        fr.tag("total.label"),
        fr.at(lc.x, ty, lc.w, ROW_H),
        Font::SansSemiBold,
        8.0,
        t.total_label.1.clone(),
    ));
    for (k, v) in &t.totals {
        let col = t.cols[*k];
        page.text(
            TextBox::new(
                fr.tag(&format!("total.{}", t.names[*k])),
                fr.at(col.x, ty, col.w - 1.0, ROW_H),
                Font::Mono,
                8.0,
                format_amount(*v),
            )
            .align(Align::Right),
        );
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

/// Bill rows, then credit rows, as cell text.
fn bill_rows(s: &StubData, l: &CheckLabels) -> Vec<Vec<String>> {
    let money = format_amount;
    let mut rows: Vec<Vec<String>> = s
        .bills
        .iter()
        .map(|b| {
            vec![
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
        vec![
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
    use super::super::tests::{assert_check_at, origin, sample};
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
        c.amount = c.stub.totals().unwrap().amount_paid;
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
            let mut c = sample(1);
            c.stub.lines = vec![StubLine {
                name: "Rent".into(),
                amount: c.amount,
            }];
            let doc = layout_checks(&[c], &opts).unwrap();
            assert_eq!(doc.pages.len(), 1);
            let page = &resolve(&doc).unwrap()[0];
            assert_check_at(page, 1, 0.0, off, WINDOW_ON_VOUCHER);
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
        let doc = layout_checks(&[with_stub(3, 1)], &CheckOptions::default()).unwrap();
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
            assert_eq!(s("check_amount"), "1,470.00");
            assert_eq!(s("number"), "Check no. 1001");
            assert_eq!(s("memo"), "Memo: Account 7781");
            assert_eq!(s("more"), "");
        }
    }

    #[test]
    fn a_plain_check_stub_lists_its_lines_and_total() {
        let mut c = sample(1);
        c.stub.lines = vec![
            StubLine {
                name: "Rent".into(),
                amount: Decimal::new(100_000, 2),
            },
            StubLine {
                name: "Utilities".into(),
                amount: Decimal::new(23_456, 2),
            },
        ];
        let doc = layout_checks(&[c], &CheckOptions::default()).unwrap();
        let page = &resolve(&doc).unwrap()[0];
        for n in 1..=2 {
            let s = |t: &str| text_of(page, &format!("stub{n}.{t}"));
            assert_eq!(s("head.name"), "Account or item");
            assert_eq!(s("row0.name"), "Rent");
            assert_eq!(s("row1.amount"), "234.56");
            assert_eq!(s("total.label"), "Total");
            assert_eq!(s("total.amount"), "1,234.56");
            assert_eq!(s("check_amount"), "1,234.56");
            assert_eq!(s("payee"), "Acme Supply Co.");
            assert_eq!(s("memo"), "Memo: Account 7781");
            assert_eq!(s("head.bill_total"), "");
        }
    }

    #[test]
    fn long_stubs_show_a_more_row_but_total_everything() {
        let doc = layout_checks(&[with_stub(15, 2)], &CheckOptions::default()).unwrap();
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
            "and 6 more (see the check in ClientLedger)"
        );
        assert_eq!(text_of(page, "stub1.total.discount"), "150.00");
        assert_eq!(text_of(page, "stub1.total.credit"), "50.00");
        // The totals stay inside their stub.
        let total = page.runs_of("stub2.check_amount").next().unwrap();
        assert!(total.baseline < crate::layout::LETTER_H_MM - 5.0);
        let first_total = page.runs_of("stub1.check_amount").next().unwrap();
        assert!(first_total.baseline < STUB_TOPS[1]);
    }

    #[test]
    fn stub_numbers_at_the_largest_amounts_shrink_but_print_whole() {
        let big = Decimal::new(99_999_999_999, 2); // 999,999,999.99
        let mut c = sample(1);
        c.stub.bills = (0..10)
            .map(|i| StubBill {
                ref_no: format!("B-{i}"),
                bill_total: big,
                discount: big,
                amount_paid: big,
                ..StubBill::default()
            })
            .collect();
        c.stub.credits = vec![StubCredit {
            amount: Decimal::new(999_999_999_999, 2),
            ..StubCredit::default()
        }];
        c.amount = c.stub.totals().unwrap().amount_paid;
        let doc = layout_checks(&[c], &CheckOptions::default()).unwrap();
        let page = &resolve(&doc).unwrap()[0];
        for (tag, want) in [
            ("stub1.total.bill_total", "9,999,999,999.90"),
            ("stub1.total.discount", "9,999,999,999.90"),
            ("stub1.total.credit", "9,999,999,999.99"),
            ("stub1.total.amount_paid", "9,999,999,999.90"),
            ("stub1.check_amount", "9,999,999,999.90"),
            ("stub1.row0.discount", "999,999,999.99"),
        ] {
            let r = page.runs_of(tag).next().unwrap();
            assert_eq!(r.text, want, "{tag}");
        }
        // The narrow discount total had to shrink.
        assert!(page.runs_of("stub1.total.discount").next().unwrap().size_pt < 8.0);
    }

    #[test]
    fn one_page_per_voucher_check() {
        let doc = layout_checks(&[sample(1), sample(2)], &CheckOptions::default()).unwrap();
        assert_eq!(doc.pages.len(), 2);
    }
}
