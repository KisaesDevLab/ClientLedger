//! Synthetic vendor statements (A06 step 5): 12 seen layouts and 3 held
//! out, each with per-line ground truth, for `docread-accuracy`'s
//! statement lines metric.
//!
//! Layouts cover one signed amount column, charges / payments / credits
//! columns, a running balance or none, balance forward as a table row, a
//! header value or a summary box, page totals, aging footers, and
//! multi-page statements with and without a repeated header row. Each
//! document is written as a text PDF, a flatbed scan (a picture-only PDF
//! for a multi-page statement, a JPEG for one page) and, for one-page
//! statements, a phone photo. Everything is invented; held-out layouts and
//! their font (PT Serif) are written only with `--holdout`.
//!
//! Output: `<out>/statements/{seen,holdout}/<layout>/NN.{pdf,json,scan.*,photo.jpg}`.

// Page geometry: short coordinate names read best here.
#![allow(clippy::many_single_char_names, clippy::cast_precision_loss)]

use super::fonts::{FontId, Fonts};
use super::layouts::Vendor;
use super::pdf::{self, Page};
use super::render::{Canvas, LETTER_H, LETTER_W, MARGIN};
use super::rng::Rng;
use super::scan;
use super::values::{Date, DateFmt, MoneyFmt, money, wire};
use anyhow::Result;
use serde_json::{Value, json};
use std::path::Path;

/// Statements per layout.
pub const DOCS_PER_LAYOUT: u32 = 6;

/// How a single amount column prints a credit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Credit {
    /// `-12.00`
    Minus,
    /// `(12.00)`
    Parens,
    /// `12.00CR`
    Cr,
    /// `12.00 CR`
    CrWord,
}

/// The amount columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cols {
    /// One signed amount column.
    Single {
        amount: &'static str,
        credit: Credit,
    },
    /// Charges and payments, and credits when named.
    Split {
        charges: &'static str,
        payments: &'static str,
        credits: Option<&'static str>,
    },
}

/// Where the opening balance is printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forward {
    /// Not printed (an open-item statement): opening is zero.
    None,
    /// The first table row.
    Row(&'static str),
    /// A header label and value.
    Header(&'static str),
    /// A boxed summary row (labels, values under them).
    Summary,
}

/// Where the closing balance is printed (besides an aging footer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closing {
    /// The last table row.
    Row(&'static str),
    /// A header label and value.
    Header(&'static str),
    /// In the summary box ("New Balance").
    Summary,
}

/// How the period is printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    None,
    /// `Label: <from> - <to>`
    Range(&'static str),
    /// `From: <from>  To: <to>`
    FromTo,
    /// `Activity from <from> through <to>`
    Through,
}

/// One statement layout.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub id: &'static str,
    pub holdout: bool,
    pub vendor: Vendor,
    pub font: FontId,
    pub bold: FontId,
    pub title: &'static str,
    pub band: bool,
    pub date_fmt: DateFmt,
    pub money: MoneyFmt,
    pub date_col: &'static str,
    pub ref_col: &'static str,
    pub desc_col: Option<&'static str>,
    pub due_col: bool,
    pub cols: Cols,
    /// The running balance column's header.
    pub running: Option<&'static str>,
    pub forward: Forward,
    pub closing: Closing,
    pub aging: bool,
    pub period: Period,
    pub date_label: &'static str,
    pub account_label: &'static str,
    pub rows: (i64, i64),
    /// Continuation pages repeat the table header row.
    pub repeat_header: bool,
    /// A row at the foot of each full page.
    pub page_total: Option<&'static str>,
    /// Invoice references: prefix and digits.
    pub inv_ref: (&'static str, u32),
}

const fn vendor(
    key: &'static str,
    name: &'static str,
    street: &'static str,
    city: &'static str,
    phone: &'static str,
) -> Vendor {
    Vendor {
        key,
        name,
        street,
        city,
        phone: Some(phone),
        web: None,
        confusable: "",
    }
}

const BASE: Layout = Layout {
    id: "",
    holdout: false,
    vendor: vendor("", "", "", "", ""),
    font: FontId::PlexSans,
    bold: FontId::PlexSansBold,
    title: "STATEMENT",
    band: false,
    date_fmt: DateFmt::Mdy,
    money: MoneyFmt::Plain,
    date_col: "Date",
    ref_col: "Reference",
    desc_col: Some("Description"),
    due_col: false,
    cols: Cols::Single {
        amount: "Amount",
        credit: Credit::Parens,
    },
    running: Some("Balance"),
    forward: Forward::Row("Balance Forward"),
    closing: Closing::Row("Closing Balance"),
    aging: false,
    period: Period::Range("Statement Period"),
    date_label: "Statement Date",
    account_label: "Account No.",
    rows: (4, 14),
    repeat_header: true,
    page_total: None,
    inv_ref: ("INV-", 5),
};

/// Every statement layout (seen first, then held out).
#[allow(clippy::too_many_lines)]
pub fn all() -> Vec<Layout> {
    vec![
        Layout {
            id: "st01-classic-single",
            vendor: vendor(
                "cedar-valley",
                "Cedar Valley Parts Co.",
                "88 Quarry Road",
                "Rogers, AR 72756",
                "(479) 555-0161",
            ),
            ..BASE
        },
        Layout {
            id: "st02-three-columns",
            vendor: vendor(
                "harbor-electric",
                "Harbor Electric Supply",
                "4100 Dock St",
                "Fort Smith, AR 72901",
                "(479) 555-0172",
            ),
            ref_col: "Invoice #",
            cols: Cols::Split {
                charges: "Charges",
                payments: "Payments",
                credits: Some("Credits"),
            },
            forward: Forward::Header("Previous Balance"),
            closing: Closing::Header("Amount Due"),
            period: Period::Range("Billing Period"),
            rows: (5, 16),
            inv_ref: ("", 6),
            ..BASE
        },
        Layout {
            id: "st03-mono-cr",
            font: FontId::PlexMono,
            bold: FontId::PlexMono,
            title: "STATEMENT OF ACCOUNT",
            vendor: vendor(
                "bluestem-feed",
                "Bluestem Feed & Seed",
                "21 Mill Pond Ln",
                "Siloam Springs, AR 72761",
                "(479) 555-0133",
            ),
            ref_col: "Doc No.",
            desc_col: Some("Transaction"),
            cols: Cols::Single {
                amount: "Amount",
                credit: Credit::Cr,
            },
            forward: Forward::Row("Previous Balance"),
            closing: Closing::Row("New Balance"),
            period: Period::FromTo,
            rows: (4, 12),
            inv_ref: ("D", 6),
            ..BASE
        },
        Layout {
            id: "st04-no-running",
            vendor: vendor(
                "summit-office",
                "Summit Office Products",
                "700 Commerce Pkwy",
                "Bentonville, AR 72712",
                "(479) 555-0118",
            ),
            title: "Customer Statement",
            ref_col: "Ref #",
            desc_col: Some("Details"),
            cols: Cols::Single {
                amount: "Amount",
                credit: Credit::Minus,
            },
            running: None,
            forward: Forward::Header("Opening Balance"),
            closing: Closing::Header("Total Due"),
            aging: true,
            rows: (3, 10),
            inv_ref: ("SO", 5),
            ..BASE
        },
        Layout {
            id: "st05-multi-repeat",
            vendor: vendor(
                "riverbend-supply",
                "Riverbend Building Supply",
                "1200 Mill Road",
                "Springfield, MO 65801",
                "(417) 555-0142",
            ),
            ref_col: "Invoice",
            cols: Cols::Split {
                charges: "Debits",
                payments: "Credits",
                credits: None,
            },
            closing: Closing::Row("Ending Balance"),
            rows: (45, 80),
            repeat_header: true,
            page_total: Some("Page Total"),
            ..BASE
        },
        Layout {
            id: "st06-multi-no-repeat",
            font: FontId::CourierPrime,
            bold: FontId::CourierPrimeBold,
            vendor: vendor(
                "ozark-fuel",
                "Ozark Fuel & Lube",
                "3 Industrial Dr",
                "Harrison, AR 72601",
                "(870) 555-0109",
            ),
            period: Period::Through,
            rows: (45, 90),
            repeat_header: false,
            page_total: Some("Carried Forward"),
            inv_ref: ("T", 6),
            ..BASE
        },
        Layout {
            id: "st07-summary-aging",
            vendor: vendor(
                "lakeside-paper",
                "Lakeside Paper & Packaging",
                "55 Shoreline Blvd",
                "Hot Springs, AR 71901",
                "(501) 555-0190",
            ),
            title: "Monthly Statement",
            ref_col: "Invoice #",
            desc_col: None,
            cols: Cols::Single {
                amount: "Amount",
                credit: Credit::Minus,
            },
            forward: Forward::Summary,
            closing: Closing::Summary,
            aging: true,
            rows: (5, 15),
            ..BASE
        },
        Layout {
            id: "st08-open-items",
            vendor: vendor(
                "keystone-tools",
                "Keystone Tool Rental",
                "910 Ridge Ave",
                "Conway, AR 72032",
                "(501) 555-0127",
            ),
            title: "Open Item Statement",
            ref_col: "Invoice #",
            due_col: true,
            cols: Cols::Single {
                amount: "Amount",
                credit: Credit::Minus,
            },
            running: None,
            forward: Forward::None,
            closing: Closing::Row("Total Due"),
            period: Period::None,
            rows: (3, 12),
            ..BASE
        },
        Layout {
            id: "st09-band-split",
            band: true,
            vendor: vendor(
                "prairie-tile",
                "Prairie Tile & Stone",
                "2400 W Walnut St",
                "Rogers, AR 72756",
                "(479) 555-0155",
            ),
            ref_col: "Ref",
            cols: Cols::Split {
                charges: "Charges",
                payments: "Payments",
                credits: None,
            },
            forward: Forward::Row("Previous Balance"),
            closing: Closing::Header("Balance Due"),
            period: Period::FromTo,
            rows: (6, 18),
            money: MoneyFmt::Dollar,
            ..BASE
        },
        Layout {
            id: "st10-short-dates-crword",
            vendor: vendor(
                "northgate-auto",
                "Northgate Auto Parts",
                "640 N College Ave",
                "Fayetteville, AR 72701",
                "(479) 555-0166",
            ),
            date_fmt: DateFmt::MdyShort,
            ref_col: "Number",
            desc_col: Some("Activity"),
            cols: Cols::Single {
                amount: "Amount",
                credit: Credit::CrWord,
            },
            forward: Forward::Header("Last Statement Balance"),
            closing: Closing::Header("Please Pay"),
            period: Period::Range("Period"),
            rows: (5, 14),
            inv_ref: ("", 7),
            ..BASE
        },
        Layout {
            id: "st11-month-names",
            vendor: vendor(
                "greenleaf-nursery",
                "Greenleaf Nursery Wholesale",
                "18 Orchard Rd",
                "Van Buren, AR 72956",
                "(479) 555-0148",
            ),
            date_fmt: DateFmt::MonD,
            ref_col: "Document #",
            cols: Cols::Split {
                charges: "Invoices",
                payments: "Payments",
                credits: Some("Credits"),
            },
            forward: Forward::Row("Beginning Balance"),
            closing: Closing::Row("Ending Balance"),
            rows: (5, 14),
            inv_ref: ("GL", 5),
            ..BASE
        },
        Layout {
            id: "st12-multi-courier-aging",
            font: FontId::CourierPrime,
            bold: FontId::CourierPrimeBold,
            vendor: vendor(
                "delta-freight",
                "Delta Freight Lines",
                "77 Terminal Way",
                "Little Rock, AR 72206",
                "(501) 555-0181",
            ),
            title: "ACCOUNT STATEMENT",
            ref_col: "Pro #",
            desc_col: Some("Details"),
            cols: Cols::Split {
                charges: "Charges",
                payments: "Payments",
                credits: Some("Credits"),
            },
            forward: Forward::Header("Previous Balance"),
            closing: Closing::Header("Total Due"),
            aging: true,
            rows: (35, 60),
            repeat_header: true,
            inv_ref: ("", 8),
            ..BASE
        },
        // Held out: never tuned on (written only with --holdout).
        Layout {
            id: "sx01-holdout-parens",
            holdout: true,
            font: FontId::PtSerif,
            bold: FontId::PtSerifBold,
            vendor: vendor(
                "magnolia-linen",
                "Magnolia Linen Service",
                "300 Laundry Ln",
                "Pine Bluff, AR 71601",
                "(870) 555-0114",
            ),
            date_fmt: DateFmt::MdyDash,
            ref_col: "Trans #",
            desc_col: Some("Memo"),
            forward: Forward::Row("Prior Balance"),
            closing: Closing::Row("Statement Balance"),
            rows: (6, 16),
            inv_ref: ("ML", 5),
            ..BASE
        },
        Layout {
            id: "sx02-holdout-multi-split",
            holdout: true,
            font: FontId::PtSerif,
            bold: FontId::PtSerifBold,
            vendor: vendor(
                "ironhorse-rail",
                "Ironhorse Rail Supply",
                "5 Switchyard Rd",
                "Texarkana, AR 71854",
                "(870) 555-0193",
            ),
            date_fmt: DateFmt::Iso,
            ref_col: "Inv #",
            cols: Cols::Split {
                charges: "Charge",
                payments: "Payment",
                credits: Some("Credit"),
            },
            forward: Forward::Header("Previous Balance"),
            closing: Closing::Header("Total Amount Due"),
            aging: true,
            rows: (35, 60),
            repeat_header: false,
            inv_ref: ("IR", 6),
            ..BASE
        },
        Layout {
            id: "sx03-holdout-cr",
            holdout: true,
            font: FontId::PtSerif,
            bold: FontId::PtSerifBold,
            vendor: vendor(
                "copperline-hvac",
                "Copperline HVAC Supply",
                "41 Furnace St",
                "Jonesboro, AR 72401",
                "(870) 555-0102",
            ),
            date_fmt: DateFmt::MdyNoPad,
            money: MoneyFmt::DollarSpace,
            ref_col: "Invoice No.",
            desc_col: Some("Particulars"),
            cols: Cols::Single {
                amount: "Amt",
                credit: Credit::Cr,
            },
            running: None,
            forward: Forward::None,
            closing: Closing::Row("Total"),
            period: Period::FromTo,
            rows: (4, 10),
            ..BASE
        },
    ]
}

/// The kind of a statement line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Invoice,
    Payment,
    Credit,
}

/// One printed line.
#[derive(Debug, Clone)]
struct Row {
    date: Date,
    reference: Option<String>,
    description: &'static str,
    kind: Kind,
    /// Signed cents: charge +, payment / credit −.
    amount: i64,
    running: i64,
    due: Date,
}

/// One statement.
pub struct Statement {
    pub layout: Layout,
    seq: u32,
    date: Date,
    from: Date,
    to: Date,
    account: String,
    opening: i64,
    closing: i64,
    rows: Vec<Row>,
}

fn digits(rng: &mut Rng, n: u32) -> String {
    (0..n)
        .map(|i| {
            let lo = i64::from(i == 0);
            char::from(b'0' + u8::try_from(rng.range(lo, 9)).unwrap_or(1))
        })
        .collect()
}

impl Statement {
    pub fn new(layout: &Layout, seq: u32, seed: &Rng) -> Statement {
        let mut rng = seed.fork(&format!("statement/{}/{seq}", layout.id));
        let month = u32::try_from(rng.range(1, 9)).unwrap_or(6);
        let from = Date {
            y: 2026,
            m: month,
            d: 1,
        };
        let to = Date {
            y: 2026,
            m: month + 1,
            d: 1,
        }
        .plus(-1);
        let date = to.plus(rng.range(0, 3));
        let opening = if layout.forward == Forward::None {
            0
        } else {
            rng.range(0, 600_000)
        };
        let n = usize::try_from(rng.range(layout.rows.0, layout.rows.1)).unwrap_or(5);
        let span = to.days() - from.days();
        let mut days: Vec<i64> = (0..n).map(|_| rng.range(0, span)).collect();
        days.sort_unstable();
        let mut running = opening;
        let mut rows = Vec::new();
        let (prefix, nd) = layout.inv_ref;
        let mut last_invoice = 0;
        for d in days {
            let roll = rng.range(0, 99);
            let kind = if roll < 62 {
                Kind::Invoice
            } else if roll < 85 {
                Kind::Payment
            } else {
                Kind::Credit
            };
            let (amount, reference, description) = match kind {
                Kind::Invoice => {
                    let c = rng.range(2_500, 400_000);
                    last_invoice = c;
                    (
                        c,
                        Some(format!("{prefix}{}", digits(&mut rng, nd))),
                        *rng.pick(&["Invoice", "Sales", "Materials", "Service call"]),
                    )
                }
                Kind::Payment => {
                    let c = if last_invoice > 0 && rng.chance(40) {
                        last_invoice
                    } else {
                        rng.range(5_000, 300_000)
                    };
                    let r = if rng.chance(12) {
                        None
                    } else if rng.chance(50) {
                        Some(format!("CK{}", digits(&mut rng, 4)))
                    } else {
                        Some(format!("PMT-{}", digits(&mut rng, 5)))
                    };
                    (
                        -c,
                        r,
                        *rng.pick(&["Payment - Thank You", "Payment", "ACH Payment"]),
                    )
                }
                Kind::Credit => (
                    -rng.range(500, 40_000),
                    Some(format!("CM-{}", digits(&mut rng, 4))),
                    *rng.pick(&["Credit Memo", "Return", "Price adjustment"]),
                ),
            };
            running += amount;
            let date = from.plus(d);
            rows.push(Row {
                date,
                reference,
                description,
                kind,
                amount,
                running,
                due: date.plus(30),
            });
        }
        let account = format!("{}-{}", digits(&mut rng, 3), digits(&mut rng, 4));
        Statement {
            layout: *layout,
            seq,
            date,
            from,
            to,
            account,
            opening,
            closing: running,
            rows,
        }
    }

    /// The ground truth (amounts as wire strings, dates ISO).
    pub fn truth(&self, pages: usize) -> Value {
        let l = &self.layout;
        json!({
            "schema": 1,
            "kind": "statement",
            "layout": l.id,
            "holdout": l.holdout,
            "seq": self.seq,
            "pages": pages,
            "vendor": { "key": l.vendor.key, "name": l.vendor.name },
            "account_number": self.account,
            "statement_date": self.date.iso(),
            "period_from": (l.period != Period::None).then(|| self.from.iso()),
            "period_to": (l.period != Period::None).then(|| self.to.iso()),
            "opening": (l.forward != Forward::None).then(|| wire(self.opening)),
            "closing": wire(self.closing),
            "lines": self.rows.iter().map(|r| json!({
                "date": r.date.iso(),
                "reference": r.reference,
                "amount": wire(r.amount),
                "running": l.running.map(|_| wire(r.running)),
            })).collect::<Vec<_>>(),
        })
    }

    fn date_text(&self, d: Date) -> String {
        d.format(self.layout.date_fmt)
    }

    fn signed(&self, cents: i64, credit: Credit) -> String {
        let f = self.layout.money;
        if cents >= 0 {
            return money(cents, f);
        }
        let abs = money(-cents, f);
        match credit {
            Credit::Minus => money(cents, f),
            Credit::Parens => format!("({abs})"),
            Credit::Cr => format!("{abs}CR"),
            Credit::CrWord => format!("{abs} CR"),
        }
    }

    fn credit(&self) -> Credit {
        match self.layout.cols {
            Cols::Single { credit, .. } => credit,
            Cols::Split { .. } => Credit::Minus,
        }
    }
}

const SIZE: f32 = 9.0;
const ROW: f32 = 14.0;
/// Rows stop above this on every page.
const BOTTOM: f32 = LETTER_H - 90.0;

/// x positions of the columns: (left of text columns or right of amount
/// columns).
struct Xs {
    date: f32,
    reference: f32,
    desc: f32,
    due: f32,
    /// (header, right edge, which)
    money: Vec<(&'static str, f32, Slot)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Amount,
    Charges,
    Payments,
    Credits,
    Running,
}

fn xs(l: &Layout) -> Xs {
    let right = LETTER_W - MARGIN;
    let mut money: Vec<(&'static str, f32, Slot)> = Vec::new();
    match l.cols {
        Cols::Single { amount, .. } => money.push((amount, 0.0, Slot::Amount)),
        Cols::Split {
            charges,
            payments,
            credits,
        } => {
            money.push((charges, 0.0, Slot::Charges));
            money.push((payments, 0.0, Slot::Payments));
            if let Some(c) = credits {
                money.push((c, 0.0, Slot::Credits));
            }
        }
    }
    if let Some(r) = l.running {
        money.push((r, 0.0, Slot::Running));
    }
    let n = money.len() as f32;
    for (i, m) in money.iter_mut().enumerate() {
        m.1 = right - 4.0 - 74.0 * (n - 1.0 - i as f32);
    }
    let first_money = right - 4.0 - 74.0 * n;
    Xs {
        date: MARGIN + 4.0,
        reference: MARGIN + 82.0,
        desc: MARGIN + 170.0,
        due: (first_money - 62.0).max(MARGIN + 250.0),
        money,
    }
}

fn header_row(c: &mut Canvas<'_>, s: &Statement, x: &Xs, y: f32) {
    let l = &s.layout;
    let b = l.bold;
    c.shade(MARGIN, y - 11.0, LETTER_W - 2.0 * MARGIN, 16.0, 0.88);
    c.text(x.date, y, SIZE, b, l.date_col);
    c.text(x.reference, y, SIZE, b, l.ref_col);
    if let Some(d) = l.desc_col {
        c.text(x.desc, y, SIZE, b, d);
    }
    if l.due_col {
        c.text(x.due, y, SIZE, b, "Due Date");
    }
    for (h, r, _) in &x.money {
        c.right(*r, y, SIZE, b, h);
    }
}

fn row_money(s: &Statement, row: &Row, slot: Slot) -> Option<String> {
    let f = s.layout.money;
    match (slot, row.kind) {
        (Slot::Amount, _) => Some(s.signed(row.amount, s.credit())),
        (Slot::Charges, Kind::Invoice) => Some(money(row.amount, f)),
        (Slot::Payments, Kind::Payment) | (Slot::Credits, Kind::Credit) => {
            Some(money(-row.amount, f))
        }
        // No credits column: credit memos go with the payments.
        (Slot::Payments, Kind::Credit)
            if matches!(s.layout.cols, Cols::Split { credits: None, .. }) =>
        {
            Some(money(-row.amount, f))
        }
        (Slot::Running, _) => Some(s.signed(row.running, s.credit())),
        _ => None,
    }
}

/// A row with a label in the description column and an amount in the
/// last money column.
fn label_row(c: &mut Canvas<'_>, s: &Statement, x: &Xs, y: f32, label: &str, cents: i64) {
    let l = &s.layout;
    c.text(x.desc, y, SIZE, l.bold, label);
    if let Some((_, r, _)) = x.money.last() {
        c.right(*r, y, SIZE, l.bold, &s.signed(cents, s.credit()));
    }
}

#[allow(clippy::too_many_lines)]
fn render(s: &Statement, fonts: &Fonts) -> Vec<Page> {
    let l = &s.layout;
    let (f, b) = (l.font, l.bold);
    let v = &l.vendor;
    let mut c = Canvas::new(fonts, LETTER_W, LETTER_H);
    let right = LETTER_W - MARGIN;
    let mut y = MARGIN + 18.0;
    if l.band {
        c.shade(0.0, 24.0, LETTER_W, 44.0, 0.86);
        c.text(MARGIN, 54.0, 20.0, b, v.name);
        c.right(right, 52.0, 14.0, b, l.title);
        c.text(
            MARGIN,
            84.0,
            9.0,
            f,
            &format!("{}   {}   {}", v.street, v.city, v.phone.unwrap_or("")),
        );
        y = 110.0;
    } else {
        c.text(MARGIN, y, 16.0, b, v.name);
        c.text(MARGIN, y + 16.0, 9.5, f, v.street);
        c.text(MARGIN, y + 28.0, 9.5, f, v.city);
        c.text(
            MARGIN,
            y + 40.0,
            9.5,
            f,
            &format!("Phone {}", v.phone.unwrap_or("")),
        );
        c.right(right, y + 4.0, 18.0, b, l.title);
        y += 64.0;
    }
    // Header values on the right.
    let mut pairs: Vec<(String, String)> = vec![
        (l.date_label.to_string(), s.date_text(s.date)),
        (l.account_label.to_string(), s.account.clone()),
    ];
    match l.period {
        Period::Range(label) => pairs.push((
            label.to_string(),
            format!("{} - {}", s.date_text(s.from), s.date_text(s.to)),
        )),
        Period::None | Period::FromTo | Period::Through => {}
    }
    if let Forward::Header(label) = l.forward {
        pairs.push((label.to_string(), s.signed(s.opening, s.credit())));
    }
    if let Closing::Header(label) = l.closing {
        pairs.push((label.to_string(), money(s.closing, l.money)));
    }
    let x_label = right - 250.0;
    for (i, (k, val)) in pairs.iter().enumerate() {
        let yy = y + 14.0 * i as f32;
        c.text(x_label, yy, 9.5, b, &format!("{k}:"));
        c.right(right, yy, 9.5, f, val);
    }
    // The customer on the left.
    let (cust, street, city) = super::doc::CUSTOMERS[0];
    c.text(MARGIN, y, 9.0, b, "Statement To");
    c.text(MARGIN, y + 13.0, 9.5, f, cust);
    c.text(MARGIN, y + 25.0, 9.5, f, street);
    c.text(MARGIN, y + 37.0, 9.5, f, city);
    y += (14.0 * pairs.len() as f32).max(50.0) + 12.0;
    match l.period {
        Period::FromTo => {
            c.text(
                MARGIN,
                y,
                9.5,
                f,
                &format!("From: {}   To: {}", s.date_text(s.from), s.date_text(s.to)),
            );
            y += 18.0;
        }
        Period::Through => {
            c.text(
                MARGIN,
                y,
                9.5,
                f,
                &format!(
                    "Activity from {} through {}",
                    s.date_text(s.from),
                    s.date_text(s.to)
                ),
            );
            y += 18.0;
        }
        Period::None | Period::Range(_) => {}
    }
    if l.forward == Forward::Summary {
        let charges: i64 = s
            .rows
            .iter()
            .filter(|r| r.amount > 0)
            .map(|r| r.amount)
            .sum();
        let credits: i64 = s
            .rows
            .iter()
            .filter(|r| r.amount < 0)
            .map(|r| -r.amount)
            .sum();
        let cells = [
            ("Previous Balance", s.opening),
            ("New Charges", charges),
            ("Payments & Credits", credits),
            ("New Balance", s.closing),
        ];
        let w = (LETTER_W - 2.0 * MARGIN) / cells.len() as f32;
        c.boxed(MARGIN, y - 11.0, LETTER_W - 2.0 * MARGIN, 32.0);
        for (i, (k, cents)) in cells.iter().enumerate() {
            let x = MARGIN + w * i as f32;
            c.center(x + w / 2.0, y, 8.5, b, k);
            c.center(x + w / 2.0, y + 15.0, 9.5, f, &money(*cents, l.money));
        }
        y += 40.0;
    }
    y += 10.0;
    let x = xs(l);
    header_row(&mut c, s, &x, y);
    y += 18.0;
    if let Forward::Row(label) = l.forward {
        c.text(x.date, y, SIZE, f, &s.date_text(s.from));
        label_row(&mut c, s, &x, y, label, s.opening);
        y += ROW;
    }
    let mut page_no = 1;
    let mut page_sum = 0i64;
    for (i, row) in s.rows.iter().enumerate() {
        let room = if l.page_total.is_some() { ROW } else { 0.0 };
        if y + room > BOTTOM {
            if let Some(label) = l.page_total {
                let shown = if l.running.is_some() {
                    s.rows[i - 1].running
                } else {
                    page_sum
                };
                label_row(&mut c, s, &x, y, label, shown);
            }
            c.text(
                right - 60.0,
                LETTER_H - 40.0,
                8.0,
                f,
                &format!("Page {page_no}"),
            );
            c.new_page();
            page_no += 1;
            page_sum = 0;
            y = MARGIN + 10.0;
            c.text(MARGIN, y, 10.0, b, v.name);
            c.right(
                right,
                y,
                9.0,
                f,
                &format!("{} {}", l.account_label, s.account),
            );
            y += 24.0;
            if l.repeat_header {
                header_row(&mut c, s, &x, y);
                y += 18.0;
            }
        }
        c.text(x.date, y, SIZE, f, &s.date_text(row.date));
        if let Some(r) = &row.reference {
            c.text(x.reference, y, SIZE, f, r);
        }
        if l.desc_col.is_some() {
            c.text(x.desc, y, SIZE, f, row.description);
        }
        if l.due_col {
            c.text(x.due, y, SIZE, f, &s.date_text(row.due));
        }
        for (_, r, slot) in &x.money {
            if let Some(t) = row_money(s, row, *slot) {
                c.right(*r, y, SIZE, f, &t);
            }
        }
        page_sum += row.amount;
        y += ROW;
    }
    if let Closing::Row(label) = l.closing {
        if y + ROW > BOTTOM {
            c.new_page();
            y = MARGIN + 30.0;
        }
        c.rule(MARGIN, y - 8.0, right, 0.6);
        label_row(&mut c, s, &x, y + 4.0, label, s.closing);
        y += ROW + 8.0;
    }
    if l.aging {
        if y + 60.0 > LETTER_H - 40.0 {
            c.new_page();
            y = MARGIN + 30.0;
        }
        y += 18.0;
        let cells = [
            ("Current", s.closing),
            ("1-30 Days", 0),
            ("31-60 Days", 0),
            ("61-90 Days", 0),
            ("Over 90", 0),
            ("Total Due", s.closing),
        ];
        let w = (LETTER_W - 2.0 * MARGIN) / cells.len() as f32;
        c.boxed(MARGIN, y - 11.0, LETTER_W - 2.0 * MARGIN, 32.0);
        for (i, (k, cents)) in cells.iter().enumerate() {
            let xx = MARGIN + w * i as f32;
            c.center(xx + w / 2.0, y, 8.5, b, k);
            c.center(xx + w / 2.0, y + 15.0, 9.0, f, &money(*cents, l.money));
        }
        y += 34.0;
    }
    if y + 30.0 < LETTER_H - 40.0 {
        c.text(
            MARGIN,
            y + 18.0,
            8.5,
            f,
            "Please include your account number with your payment. Thank you for your business.",
        );
    }
    if page_no > 1 {
        c.text(
            right - 60.0,
            LETTER_H - 40.0,
            8.0,
            f,
            &format!("Page {page_no}"),
        );
    }
    c.pages
}

/// Write one layout's statements; returns the number of files.
pub fn write_layout(
    l: &Layout,
    out: &Path,
    fonts: &Fonts,
    renderer: Option<&scan::Renderer>,
    seed: &Rng,
) -> Result<usize> {
    let dir = out
        .join("statements")
        .join(if l.holdout { "holdout" } else { "seen" })
        .join(l.id);
    std::fs::create_dir_all(&dir)?;
    let mut n = 0;
    for seq in 1..=DOCS_PER_LAYOUT {
        let s = Statement::new(l, seq, seed);
        let pages = render(&s, fonts);
        let bytes = pdf::write(&pages, fonts, true, "Synthetic test statement")?;
        let stem = format!("{seq:02}");
        let mut truth = s.truth(pages.len());
        std::fs::write(dir.join(format!("{stem}.pdf")), &bytes)?;
        n += 1;
        let mut pictures = Vec::new();
        if let Some(r) = renderer {
            let mut rng = seed.fork(&format!("statement/{}/{seq}/pictures", l.id));
            if pages.len() == 1 {
                let page = r.first_page(&bytes, 300.0)?;
                let img = scan::Degrade::scan(&mut rng).apply(&page, &mut rng);
                let q = u8::try_from(rng.range(75, 90)).unwrap_or(80);
                std::fs::write(dir.join(format!("{stem}.scan.jpg")), scan::jpeg(&img, q)?)?;
                let page = r.first_page(&bytes, 200.0)?;
                let img = scan::Degrade::photo(&mut rng).apply(&page, &mut rng);
                let q = u8::try_from(rng.range(40, 70)).unwrap_or(60);
                std::fs::write(dir.join(format!("{stem}.photo.jpg")), scan::jpeg(&img, q)?)?;
                pictures = vec!["scan.jpg", "photo.jpg"];
                n += 2;
            } else {
                // A scanned multi-page statement: one picture per page.
                let mut scanned = Vec::new();
                for p in &pages {
                    let one = pdf::write(std::slice::from_ref(p), fonts, true, "page")?;
                    let grey = r.first_page(&one, 300.0)?;
                    let img = scan::Degrade::scan(&mut rng).apply(&grey, &mut rng);
                    let q = u8::try_from(rng.range(75, 90)).unwrap_or(80);
                    scanned.push(Page {
                        width: p.width,
                        height: p.height,
                        items: Vec::new(),
                        image: Some(pdf::PageImage {
                            jpeg: scan::jpeg(&img, q)?,
                            width: img.width(),
                            height: img.height(),
                        }),
                    });
                }
                std::fs::write(
                    dir.join(format!("{stem}.scan.pdf")),
                    pdf::write(&scanned, fonts, true, "Synthetic test statement")?,
                )?;
                pictures = vec!["scan.pdf"];
                n += 1;
            }
        }
        truth["pictures"] = json!(pictures);
        std::fs::write(
            dir.join(format!("{stem}.json")),
            serde_json::to_vec_pretty(&truth)?,
        )?;
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_set_has_twelve_seen_and_three_held_out_layouts() {
        let all = all();
        assert_eq!(all.iter().filter(|l| !l.holdout).count(), 12);
        assert_eq!(all.iter().filter(|l| l.holdout).count(), 3);
        let mut ids: Vec<&str> = all.iter().map(|l| l.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
        assert!(
            all.iter()
                .filter(|l| l.holdout)
                .all(|l| l.font == FontId::PtSerif)
        );
    }

    #[test]
    fn balances_add_up_and_multi_page_layouts_span_pages() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let fonts = Fonts::load(&root, true).unwrap();
        let seed = Rng::new(super::super::SEED);
        for l in all()
            .iter()
            .filter(|l| fonts.has(l.font) && fonts.has(l.bold))
        {
            let s = Statement::new(l, 1, &seed);
            let sum: i64 = s.rows.iter().map(|r| r.amount).sum();
            assert_eq!(s.opening + sum, s.closing, "{}", l.id);
            let pages = render(&s, &fonts).len();
            if l.rows.0 >= 35 {
                assert!(pages > 1, "{} has {pages} page(s)", l.id);
            } else {
                assert_eq!(pages, 1, "{}", l.id);
            }
            let t = s.truth(pages);
            assert_eq!(t["lines"].as_array().unwrap().len(), s.rows.len());
        }
    }
}
