//! Lays a [`Doc`] out on pages following its [`Layout`].

use super::doc::{CUSTOMERS, Doc, Payment};
use super::fonts::{FontId, Fonts};
use super::layouts::{Col, Family, Header, Layout, Meta, Paid};
use super::pdf::{Item, Page};
use super::values::{DateFmt, money, quantity, rate};

pub(super) const LETTER_W: f32 = 612.0;
pub(super) const LETTER_H: f32 = 792.0;
pub(super) const MARGIN: f32 = 48.0;
/// Rows below this on a letter page go to the next page.
const PAGE_BOTTOM: f32 = LETTER_H - 120.0;

pub(super) struct Canvas<'f> {
    fonts: &'f Fonts,
    pub(super) pages: Vec<Page>,
    width: f32,
    height: f32,
}

impl<'f> Canvas<'f> {
    pub(super) fn new(fonts: &'f Fonts, width: f32, height: f32) -> Canvas<'f> {
        Canvas {
            fonts,
            pages: vec![Page {
                width,
                height,
                items: Vec::new(),
                image: None,
            }],
            width,
            height,
        }
    }

    fn push(&mut self, item: Item) {
        if let Some(p) = self.pages.last_mut() {
            p.items.push(item);
        }
    }

    pub(super) fn new_page(&mut self) {
        self.pages.push(Page {
            width: self.width,
            height: self.height,
            items: Vec::new(),
            image: None,
        });
    }

    pub(super) fn text(&mut self, x: f32, y: f32, size: f32, font: FontId, s: &str) {
        if s.is_empty() {
            return;
        }
        self.push(Item::Text {
            x,
            y,
            size,
            font,
            text: s.to_string(),
        });
    }

    pub(super) fn right(&mut self, right: f32, y: f32, size: f32, font: FontId, s: &str) {
        let w = self.fonts.width(font, s, size);
        self.text(right - w, y, size, font, s);
    }

    pub(super) fn center(&mut self, cx: f32, y: f32, size: f32, font: FontId, s: &str) {
        let w = self.fonts.width(font, s, size);
        self.text(cx - w / 2.0, y, size, font, s);
    }

    pub(super) fn rule(&mut self, x1: f32, y: f32, x2: f32, width: f32) {
        self.push(Item::Line {
            x1,
            y1: y,
            x2,
            y2: y,
            width,
        });
    }

    pub(super) fn boxed(&mut self, x: f32, y: f32, w: f32, h: f32) {
        for (a, b, c, d) in [
            (x, y, x + w, y),
            (x, y + h, x + w, y + h),
            (x, y, x, y + h),
            (x + w, y, x + w, y + h),
        ] {
            self.push(Item::Line {
                x1: a,
                y1: b,
                x2: c,
                y2: d,
                width: 0.6,
            });
        }
    }

    pub(super) fn shade(&mut self, x: f32, y: f32, w: f32, h: f32, gray: f32) {
        self.push(Item::Fill { x, y, w, h, gray });
    }
}

/// The document's pages.
pub fn pages(doc: &Doc, fonts: &Fonts) -> Vec<Page> {
    match doc.layout.family {
        Family::Receipt | Family::Slip => receipt(doc, fonts),
        Family::Invoice | Family::Utility | Family::Statement => letter(doc, fonts),
    }
}

fn date(doc: &Doc, d: super::values::Date) -> String {
    d.format(doc.layout.date_fmt)
}

/// The label/value pairs of the header (reference, date, ...), in order.
fn meta_pairs(doc: &Doc) -> Vec<(String, String)> {
    let l = &doc.layout;
    let strip = |s: &str| s.trim_end_matches(':').to_string();
    let mut out = Vec::new();
    if let (Some(label), Some(r)) = (l.ref_label, &doc.reference) {
        out.push((strip(label), r.clone()));
    }
    out.push((strip(l.date_label), date(doc, doc.date)));
    if let (Some(label), Some(a)) = (l.account_label, &doc.account) {
        out.push((strip(label), a.clone()));
    }
    if let (Some(label), Some(t)) = (l.terms_label, doc.terms_text()) {
        out.push((strip(label), t));
    }
    if let (Some(label), Some(d)) = (l.due_label, doc.due) {
        out.push((strip(label), date(doc, d)));
    }
    if let (Some(label), Some(d)) = (l.decoy_label, &doc.decoy) {
        out.push((strip(label), d.clone()));
    }
    if let Some((from, to)) = doc.acct.period {
        out.push((
            "Service Period".to_string(),
            format!("{} - {}", date(doc, from), date(doc, to)),
        ));
    }
    out
}

#[allow(clippy::too_many_lines)]
fn letter(doc: &Doc, fonts: &Fonts) -> Vec<Page> {
    let l = &doc.layout;
    let mut c = Canvas::new(fonts, LETTER_W, LETTER_H);
    let (f, b) = (l.font, l.bold);
    let v = &l.vendor;
    let mut vendor_lines: Vec<String> = vec![v.street.to_string(), v.city.to_string()];
    if let Some(p) = v.phone {
        vendor_lines.push(format!("Phone {p}"));
    }
    if let Some(w) = v.web {
        vendor_lines.push(w.to_string());
    }
    let title_size = 22.0;
    let mut y = MARGIN + 18.0;
    match l.header {
        Header::LeftTitleRight => {
            c.text(MARGIN, y, 17.0, b, v.name);
            for (i, s) in vendor_lines.iter().enumerate() {
                c.text(MARGIN, y + 16.0 + 12.0 * i as f32, 9.5, f, s);
            }
            if let Some(t) = l.title {
                c.right(LETTER_W - MARGIN, y + 4.0, title_size, b, t);
            }
            y += 18.0 + 12.0 * vendor_lines.len() as f32 + 18.0;
        }
        Header::Centered => {
            c.center(LETTER_W / 2.0, y, 18.0, b, v.name);
            let joined = vendor_lines.join("  |  ");
            c.center(LETTER_W / 2.0, y + 16.0, 9.0, f, &joined);
            c.rule(MARGIN, y + 26.0, LETTER_W - MARGIN, 0.8);
            if let Some(t) = l.title {
                c.center(LETTER_W / 2.0, y + 52.0, 16.0, b, t);
            }
            y += 72.0;
        }
        Header::TitleLeftVendorRight => {
            if let Some(t) = l.title {
                c.text(MARGIN, y + 4.0, title_size, b, t);
            }
            let right = LETTER_W - MARGIN;
            c.right(right, y, 14.0, b, v.name);
            for (i, s) in vendor_lines.iter().enumerate() {
                c.right(right, y + 15.0 + 11.5 * i as f32, 9.0, f, s);
            }
            y += 16.0 + 11.5 * vendor_lines.len() as f32 + 20.0;
        }
        Header::Band => {
            c.shade(0.0, 24.0, LETTER_W, 44.0, 0.86);
            c.text(MARGIN, 54.0, 20.0, b, v.name);
            if let Some(t) = l.title {
                c.right(LETTER_W - MARGIN, 52.0, 14.0, b, t);
            }
            let joined = vendor_lines.join("   ");
            c.text(MARGIN, 84.0, 9.0, f, &joined);
            y = 110.0;
        }
    }

    // Header values.
    let pairs = meta_pairs(doc);
    let left_y;
    match l.meta {
        Meta::Pairs => {
            let x_label = LETTER_W - MARGIN - 230.0;
            for (i, (k, val)) in pairs.iter().enumerate() {
                let yy = y + 14.0 * i as f32;
                c.text(x_label, yy, 9.5, b, k);
                c.right(LETTER_W - MARGIN, yy, 9.5, f, val);
            }
            left_y = y;
            y += 14.0 * pairs.len() as f32 + 10.0;
        }
        Meta::Inline => {
            for (i, (k, val)) in pairs.iter().enumerate() {
                let yy = y + 14.0 * i as f32;
                c.text(MARGIN, yy, 10.0, f, &format!("{k}: {val}"));
            }
            y += 14.0 * pairs.len() as f32 + 12.0;
            left_y = y;
        }
        Meta::Grid => {
            let w = (LETTER_W - 2.0 * MARGIN) / pairs.len().max(1) as f32;
            c.shade(MARGIN, y - 11.0, LETTER_W - 2.0 * MARGIN, 15.0, 0.9);
            c.boxed(MARGIN, y - 11.0, LETTER_W - 2.0 * MARGIN, 32.0);
            for (i, (k, val)) in pairs.iter().enumerate() {
                let x = MARGIN + w * i as f32;
                c.center(x + w / 2.0, y, 8.5, b, k);
                c.center(x + w / 2.0, y + 15.0, 9.5, f, val);
            }
            y += 38.0;
            left_y = y;
        }
    }

    // Bill to / ship to.
    let (cname, cstreet, ccity) = CUSTOMERS[doc.customer];
    let mut party_y = left_y;
    if let Some(bt) = l.bill_to {
        let x = MARGIN;
        c.text(x, party_y, 9.0, b, bt);
        c.text(x, party_y + 13.0, 10.0, f, cname);
        c.text(x, party_y + 25.0, 10.0, f, cstreet);
        c.text(x, party_y + 37.0, 10.0, f, ccity);
        if let Some(st) = l.ship_to {
            let x = MARGIN + 200.0;
            c.text(x, party_y, 9.0, b, st);
            c.text(x, party_y + 13.0, 10.0, f, cname);
            c.text(x, party_y + 25.0, 10.0, f, "2450 Commerce Dr");
            c.text(x, party_y + 37.0, 10.0, f, "Springdale, AR 72764");
        }
        party_y += 52.0;
    }
    y = y.max(party_y) + 8.0;

    match l.family {
        Family::Utility => y = utility_body(&mut c, doc, y),
        Family::Statement => y = statement_body(&mut c, doc, y),
        _ => y = table(&mut c, doc, y),
    }
    y = totals(&mut c, doc, y + 6.0);
    if l.paid == Paid::Stamp {
        c.text(MARGIN + 40.0, y + 10.0, 28.0, b, "PAID");
    }
    for (i, s) in l.footer.iter().enumerate() {
        c.text(MARGIN, LETTER_H - 70.0 + 12.0 * i as f32, 9.0, f, s);
    }
    if let (Paid::No | Paid::Deposit, Some(r)) = (l.paid, &doc.reference)
        && l.family != Family::Statement
    {
        c.text(
            MARGIN,
            LETTER_H - 52.0,
            8.0,
            f,
            &format!("Please write {r} on your check."),
        );
    }
    let n = c.pages.len();
    if n > 1 {
        for (i, p) in c.pages.iter_mut().enumerate() {
            p.items.push(Item::Text {
                x: LETTER_W - MARGIN - 50.0,
                y: LETTER_H - 30.0,
                size: 8.0,
                font: f,
                text: format!("Page {} of {n}", i + 1),
            });
        }
    }
    c.pages
}

/// Column positions: (left, right, right-aligned) per column.
fn columns(cols: &[Col]) -> Vec<(f32, f32, bool)> {
    let total = LETTER_W - 2.0 * MARGIN;
    let widths: Vec<f32> = cols
        .iter()
        .map(|c| match c {
            Col::Qty => 52.0,
            Col::Code => 70.0,
            Col::Rate | Col::Amount => 80.0,
            Col::Description => 0.0,
        })
        .collect();
    let fixed: f32 = widths.iter().sum();
    let desc = (total - fixed).max(120.0);
    let mut x = MARGIN;
    cols.iter()
        .zip(widths)
        .map(|(c, w)| {
            let w = if *c == Col::Description { desc } else { w };
            let r = (x, x + w, !matches!(c, Col::Description | Col::Code));
            x += w;
            r
        })
        .collect()
}

fn table(c: &mut Canvas<'_>, doc: &Doc, mut y: f32) -> f32 {
    let l = &doc.layout;
    let (f, b) = (l.font, l.bold);
    let pos = columns(l.columns);
    let header = |c: &mut Canvas<'_>, y: f32| {
        c.shade(MARGIN, y - 11.0, LETTER_W - 2.0 * MARGIN, 16.0, 0.88);
        for ((left, right, ra), label) in pos.iter().zip(l.col_labels) {
            if *ra {
                c.right(right - 4.0, y, 9.0, b, label);
            } else {
                c.text(left + 4.0, y, 9.0, b, label);
            }
        }
    };
    header(c, y);
    y += 18.0;
    for li in &doc.lines {
        if y > PAGE_BOTTOM {
            c.new_page();
            y = MARGIN + 20.0;
            header(c, y);
            y += 18.0;
        }
        for (col, (left, right, ra)) in l.columns.iter().zip(&pos) {
            let s = match col {
                Col::Qty => quantity(li.qty),
                Col::Code => li.code.clone(),
                Col::Description => li.description.clone(),
                Col::Rate => rate(li.rate, l.money),
                Col::Amount => money(li.amount, l.money),
            };
            if *ra {
                c.right(right - 4.0, y, 9.5, f, &s);
            } else {
                c.text(left + 4.0, y, 9.5, f, &s);
            }
        }
        y += 15.0;
    }
    c.rule(MARGIN, y - 6.0, LETTER_W - MARGIN, 0.6);
    y + 8.0
}

fn utility_body(c: &mut Canvas<'_>, doc: &Doc, mut y: f32) -> f32 {
    let l = &doc.layout;
    let (f, b) = (l.font, l.bold);
    let right = LETTER_W - MARGIN;
    c.text(MARGIN, y, 10.0, b, "Account Summary");
    y += 16.0;
    let rows = [
        ("Previous Balance", doc.acct.previous),
        ("Payment Received - Thank You", -doc.acct.payments),
        ("Balance Forward", doc.acct.previous - doc.acct.payments),
    ];
    for (k, v) in rows {
        c.text(MARGIN + 10.0, y, 9.5, f, k);
        c.right(right, y, 9.5, f, &money(v, l.money));
        y += 14.0;
    }
    y += 8.0;
    c.text(MARGIN, y, 10.0, b, "Current Charges");
    y += 16.0;
    for li in &doc.lines {
        c.text(MARGIN + 10.0, y, 9.5, f, &li.description);
        c.right(right, y, 9.5, f, &money(li.amount, l.money));
        y += 14.0;
    }
    y
}

fn statement_body(c: &mut Canvas<'_>, doc: &Doc, mut y: f32) -> f32 {
    let l = &doc.layout;
    let (f, b) = (l.font, l.bold);
    let right = LETTER_W - MARGIN;
    let head = |c: &mut Canvas<'_>, y: f32| {
        c.shade(MARGIN, y - 11.0, LETTER_W - 2.0 * MARGIN, 16.0, 0.88);
        c.text(MARGIN + 4.0, y, 9.0, b, "Date");
        c.text(MARGIN + 90.0, y, 9.0, b, "Reference");
        c.text(MARGIN + 200.0, y, 9.0, b, "Description");
        c.right(right - 90.0, y, 9.0, b, "Charges");
        c.right(right - 4.0, y, 9.0, b, "Balance");
    };
    head(c, y);
    y += 18.0;
    let mut running = 0;
    for (i, li) in doc.lines.iter().enumerate() {
        if y > PAGE_BOTTOM {
            c.new_page();
            y = MARGIN + 20.0;
            head(c, y);
            y += 18.0;
        }
        running += li.amount;
        let d = doc.date.plus(-28 + i64::try_from(i).unwrap_or(0));
        c.text(MARGIN + 4.0, y, 9.0, f, &d.format(DateFmt::Mdy));
        c.text(MARGIN + 90.0, y, 9.0, f, &format!("INV {}", li.code));
        c.text(MARGIN + 200.0, y, 9.0, f, &li.description);
        c.right(right - 90.0, y, 9.0, f, &money(li.amount, l.money));
        c.right(right - 4.0, y, 9.0, f, &money(running, l.money));
        y += 14.0;
    }
    c.rule(MARGIN, y - 6.0, right, 0.6);
    y + 8.0
}

/// The totals block on the right; returns the y below it.
fn totals(c: &mut Canvas<'_>, doc: &Doc, mut y: f32) -> f32 {
    let l = &doc.layout;
    let (f, b) = (l.font, l.bold);
    if y > LETTER_H - 150.0 {
        c.new_page();
        y = MARGIN + 20.0;
    }
    let right = LETTER_W - MARGIN;
    let label_x = right - 230.0;
    let row = |c: &mut Canvas<'_>, label: &str, value: String, bold: bool, y: &mut f32| {
        let font = if bold { b } else { f };
        let size = if bold { 11.0 } else { 9.5 };
        c.text(label_x, *y, size, font, label);
        c.right(right, *y, size, font, &value);
        *y += if bold { 18.0 } else { 14.0 };
    };
    if let Some(s) = l.subtotal_label {
        row(c, s, money(doc.subtotal, l.money), false, &mut y);
    }
    if let Some(d) = l.discount
        && doc.discount > 0
    {
        row(c, d, money(-doc.discount, l.money), false, &mut y);
    }
    if let Some(fee) = l.fee {
        row(c, fee, money(doc.fee, l.money), false, &mut y);
    }
    if let Some(t) = l.tax_label
        && l.tax_bp > 0
    {
        let pct = format!("{t} ({}.{:02}%)", l.tax_bp / 100, l.tax_bp % 100);
        row(c, &pct, money(doc.tax, l.money), false, &mut y);
    }
    if l.family == Family::Utility {
        row(
            c,
            "Balance Forward",
            money(doc.acct.previous - doc.acct.payments, l.money),
            false,
            &mut y,
        );
    }
    c.rule(label_x, y - 9.0, right, 0.6);
    y += 4.0;
    row(
        c,
        l.total_label,
        money(doc.total, l.total_money),
        true,
        &mut y,
    );
    if doc.deposit > 0 {
        row(
            c,
            "Less Deposit Received",
            money(-doc.deposit, l.money),
            false,
            &mut y,
        );
        row(
            c,
            "Balance Due",
            money(doc.total - doc.deposit, l.total_money),
            true,
            &mut y,
        );
    }
    if let Some(Payment::Card { brand, last4 }) = &doc.payment {
        let brand = title_case(brand);
        c.text(
            label_x,
            y + 4.0,
            9.0,
            f,
            &format!("Paid with {brand} ending in {last4}"),
        );
        y += 18.0;
    }
    y
}

fn title_case(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut ch = lower.chars();
    ch.next()
        .map(|f| f.to_uppercase().collect::<String>() + ch.as_str())
        .unwrap_or_default()
}

/// Thermal receipts and card slips: one narrow page as long as needed.
#[allow(clippy::too_many_lines)]
fn receipt(doc: &Doc, fonts: &Fonts) -> Vec<Page> {
    let l: &Layout = &doc.layout;
    let w = l.paper_width;
    let (f, b) = (l.font, l.bold);
    let size = if w < 200.0 { 7.5 } else { 8.5 };
    let lh = size + 3.5;
    let m = 10.0;
    let right = w - m;
    // Lay out against a tall page, then trim the height.
    let mut c = Canvas::new(fonts, w, 4000.0);
    let mut y = 24.0;
    c.center(w / 2.0, y, size + 3.0, b, l.vendor.name);
    y += lh + 3.0;
    c.center(w / 2.0, y, size, f, l.vendor.street);
    y += lh;
    c.center(w / 2.0, y, size, f, l.vendor.city);
    y += lh;
    if let Some(p) = l.vendor.phone {
        c.center(w / 2.0, y, size, f, p);
        y += lh;
    }
    y += lh * 0.5;
    let d = doc.date.format(l.date_fmt);
    c.text(m, y, size, f, &d);
    c.right(right, y, size, f, &doc.time);
    y += lh;
    if let (Some(label), Some(r)) = (l.ref_label, &doc.reference) {
        c.text(m, y, size, f, &format!("{label} {r}"));
        y += lh;
    }
    if l.family == Family::Slip {
        c.text(m, y, size, f, "MERCHANT ID: 4471002");
        y += lh;
        c.text(m, y, size, f, "TERMINAL ID: 0031");
        y += lh * 1.5;
        c.center(w / 2.0, y, size + 1.0, b, "SALE");
        y += lh * 1.5;
    } else {
        c.rule(m, y - lh * 0.4, right, 0.5);
        y += lh * 0.6;
        for li in &doc.lines {
            let amt = money(li.amount, l.money);
            match (
                l.columns.contains(&Col::Qty),
                l.columns.contains(&Col::Rate),
            ) {
                (true, true) => {
                    c.text(m, y, size, f, &li.description);
                    y += lh;
                    c.text(
                        m + 8.0,
                        y,
                        size,
                        f,
                        &format!("{} GAL @ {}/GAL", quantity(li.qty), rate(li.rate, l.money)),
                    );
                    c.right(right, y, size, f, &amt);
                }
                (true, false) => {
                    c.text(
                        m,
                        y,
                        size,
                        f,
                        &format!("{} {}", quantity(li.qty), li.description),
                    );
                    c.right(right, y, size, f, &amt);
                }
                _ => {
                    if li.qty == 1000 {
                        c.text(m, y, size, f, &li.description);
                    } else {
                        c.text(m, y, size, f, &li.description);
                        y += lh;
                        c.text(
                            m + 8.0,
                            y,
                            size,
                            f,
                            &format!("{} @ {}", quantity(li.qty), rate(li.rate, l.money)),
                        );
                    }
                    c.right(right, y, size, f, &amt);
                }
            }
            y += lh;
        }
        c.rule(m, y - lh * 0.4, right, 0.5);
        y += lh * 0.6;
    }
    let row = |c: &mut Canvas<'_>, label: &str, v: i64, bold: bool, y: &mut f32| {
        let font = if bold { b } else { f };
        let sz = if bold { size + 1.5 } else { size };
        c.text(m + 30.0, *y, sz, font, label);
        c.right(right, *y, sz, font, &money(v, l.total_money));
        *y += lh + if bold { 2.0 } else { 0.0 };
    };
    if let Some(s) = l.subtotal_label {
        row(&mut c, s, doc.subtotal, false, &mut y);
    }
    if let Some(t) = l.tax_label
        && l.tax_bp > 0
    {
        row(&mut c, t, doc.tax, false, &mut y);
    }
    if l.tip {
        row(&mut c, "TIP", doc.tip, false, &mut y);
    }
    row(&mut c, l.total_label, doc.total, true, &mut y);
    y += lh * 0.5;
    match &doc.payment {
        Some(Payment::Card { brand, last4 }) => {
            c.text(m, y, size, f, &format!("{brand} ************{last4}"));
            c.right(right, y, size, f, &money(doc.total, l.total_money));
            y += lh;
            c.text(m, y, size, f, "APPROVED");
            y += lh;
            c.text(m, y, size, f, "ENTRY METHOD: CHIP");
            y += lh;
        }
        Some(Payment::Cash { tendered }) => {
            row(&mut c, "CASH", *tendered, false, &mut y);
            row(&mut c, "CHANGE", tendered - doc.total, false, &mut y);
        }
        Some(Payment::Stamp) | None => {}
    }
    if l.family == Family::Slip {
        y += lh;
        c.text(m, y, size, f, "X______________________");
        y += lh;
    }
    y += lh;
    for s in l.footer {
        c.center(w / 2.0, y, size, f, s);
        y += lh;
    }
    let height = (y + 24.0).max(200.0);
    let mut page = c.pages.remove(0);
    page.height = height;
    vec![page]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gen_invoices::layouts::all;
    use crate::gen_invoices::rng::Rng;

    fn texts(pages: &[Page]) -> String {
        pages
            .iter()
            .flat_map(|p| &p.items)
            .filter_map(|i| match i {
                Item::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_seen_layout_prints_its_truth() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let fonts = Fonts::load(&root, true).unwrap();
        let seed = Rng::new(42);
        for l in all()
            .into_iter()
            .filter(|l| [l.font, l.bold].iter().all(|f| fonts.has(*f)))
        {
            let d = crate::gen_invoices::doc::Doc::new(&l, 3, &seed);
            let pages = pages(&d, &fonts);
            let t = texts(&pages);
            assert!(t.contains(l.vendor.name), "{}", l.id);
            assert!(t.contains(&money(d.total, l.total_money)), "{}", l.id);
            assert!(t.contains(&d.date.format(l.date_fmt)), "{}", l.id);
            if let Some(r) = &d.reference {
                assert!(t.contains(r.as_str()), "{}", l.id);
            }
            for p in &pages {
                for i in &p.items {
                    if let Item::Text { x, y, .. } = i {
                        assert!(
                            *x >= 0.0 && *x < p.width && *y > 0.0 && *y < p.height,
                            "{}",
                            l.id
                        );
                    }
                }
            }
        }
    }
}
