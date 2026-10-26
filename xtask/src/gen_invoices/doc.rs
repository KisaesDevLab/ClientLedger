//! One synthetic document's values (seeded) and its ground truth.

use super::layouts::{Catalog, Family, Layout, Paid, QtyKind, RefShape};
use super::rng::Rng;
use super::values::{Date, extend, milli_wire, percent, wire};
use serde_json::{Value, json};

/// The business the bills are addressed to (the owner's company).
pub const CUSTOMERS: [(&str, &str, &str); 3] = [
    (
        "Ozark Ridge Outfitters LLC",
        "112 Hollow Creek Rd",
        "Fayetteville, AR 72701",
    ),
    (
        "Ozark Ridge Outfitters",
        "Attn: Accounts Payable, 112 Hollow Creek Rd",
        "Fayetteville, AR 72701",
    ),
    (
        "Ozark Ridge Outfitters LLC - Warehouse",
        "2450 Commerce Dr",
        "Springdale, AR 72764",
    ),
];

/// One line of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineItem {
    pub code: String,
    pub description: String,
    /// Thousandths.
    pub qty: i64,
    /// Thousandths of a dollar.
    pub rate: i64,
    /// Cents.
    pub amount: i64,
}

/// How a receipt was paid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payment {
    Card { brand: &'static str, last4: String },
    Cash { tendered: i64 },
    Stamp,
}

/// Utility and statement extras.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Account {
    pub previous: i64,
    pub payments: i64,
    pub period: Option<(Date, Date)>,
}

/// One document.
#[derive(Debug, Clone)]
pub struct Doc {
    pub layout: Layout,
    /// 1-based within the layout.
    pub seq: u32,
    pub date: Date,
    pub time: String,
    pub due: Option<Date>,
    pub terms_days: Option<u16>,
    pub reference: Option<String>,
    pub decoy: Option<String>,
    pub account: Option<String>,
    pub customer: usize,
    pub lines: Vec<LineItem>,
    pub subtotal: i64,
    pub discount: i64,
    pub fee: i64,
    pub tax: i64,
    pub tip: i64,
    pub total: i64,
    pub deposit: i64,
    pub payment: Option<Payment>,
    pub acct: Account,
    /// Whether the PDF's fonts carry `ToUnicode` maps.
    pub tounicode: bool,
}

fn digits(rng: &mut Rng, n: u32) -> String {
    let mut s = String::new();
    for i in 0..n {
        let lo = i64::from(i == 0);
        s.push(char::from(
            b'0' + u8::try_from(rng.range(lo, 9)).unwrap_or(0),
        ));
    }
    s
}

fn reference(rng: &mut Rng, shape: RefShape) -> String {
    match shape {
        RefShape::Prefixed(p, n) => format!("{p}{}", digits(rng, n)),
        RefShape::Digits(n) => digits(rng, n),
        RefShape::Mixed => {
            let a = char::from(b'A' + u8::try_from(rng.range(0, 25)).unwrap_or(0));
            format!("{a}{}-{}", rng.range(1, 9), digits(rng, 5))
        }
    }
}

#[allow(clippy::too_many_lines)]
fn catalog(c: Catalog) -> &'static [(&'static str, i64, i64)] {
    // (description, lowest price, highest price) in cents.
    match c {
        Catalog::Electrical => &[
            ("Service call - diagnose tripping breaker", 8_500, 14_500),
            ("Install GFCI receptacles, kitchen", 6_500, 9_500),
            ("Rough-in wiring, warehouse bay 3", 7_500, 9_500),
            ("Replace 200A panel breaker", 8_000, 12_000),
            ("LED fixture install, showroom", 6_500, 8_500),
            ("After-hours emergency labor", 12_500, 16_500),
        ],
        Catalog::Plumbing => &[
            ("Replace water heater element", 9_500, 18_500),
            ("Clear main line, camera inspection", 18_500, 32_500),
            ("Install frost-free hose bib", 12_500, 21_000),
            ("Repair leak under break-room sink", 8_500, 14_000),
            ("Backflow preventer test", 6_500, 9_500),
        ],
        Catalog::Wholesale => &[
            ("Copper elbow 3/4 in. 90 deg", 189, 389),
            ("PVC pipe 2 in. x 10 ft Sch 40", 1_249, 1_899),
            ("Ball valve 1/2 in. full port", 899, 1_499),
            ("Thread seal tape 1/2 in.", 129, 249),
            ("Pipe hanger 1 in. galvanized", 79, 159),
            ("Brass shutoff valve 1/4 turn", 1_099, 1_699),
            ("Pressure gauge 0-100 psi", 1_450, 2_250),
        ],
        Catalog::AutoParts => &[
            ("BRAKE PAD SET FRONT", 3_499, 7_999),
            ("OIL FILTER", 699, 1_299),
            ("SERPENTINE BELT", 2_499, 4_599),
            ("WIPER BLADE 22IN", 1_199, 2_199),
            ("SPARK PLUG IRIDIUM", 899, 1_499),
            ("CABIN AIR FILTER", 1_599, 2_899),
        ],
        Catalog::Grocery => &[
            ("BANANAS", 59, 189),
            ("COFFEE 12OZ", 799, 1_299),
            ("PAPER TOWELS 6PK", 899, 1_499),
            ("BOTTLED WATER 24PK", 399, 699),
            ("COFFEE CREAMER", 349, 549),
            ("TRASH BAGS 30CT", 999, 1_599),
            ("HAND SOAP", 249, 449),
        ],
        Catalog::Hardware => &[
            ("WOOD SCREWS #8 1-1/4", 599, 999),
            ("DUCT TAPE 60YD", 649, 999),
            ("UTILITY KNIFE", 899, 1_699),
            ("LED BULB 4PK", 799, 1_399),
            ("EXT CORD 25FT", 1_499, 2_499),
            ("WORK GLOVES L", 999, 1_799),
            ("2X4X8 STUD", 389, 549),
        ],
        Catalog::Food => &[
            ("Club Sandwich", 1_195, 1_495),
            ("Iced Tea", 295, 395),
            ("House Salad", 795, 1_095),
            ("Soup of the Day", 595, 795),
            ("Burger, Medium", 1_395, 1_695),
            ("Coffee", 250, 350),
        ],
        Catalog::Handyman => &[
            ("Fix sticking office door", 4_500, 6_500),
            ("Patch drywall, hang shelves", 4_500, 6_000),
            ("Replace faucet in back room", 5_000, 6_500),
            ("Paint trim at front entry", 4_000, 5_500),
        ],
        Catalog::Roofing => &[
            ("Tear off and replace shingles, 18 sq", 650_000, 980_000),
            ("Replace drip edge", 45_000, 85_000),
            ("Flashing repair at chimney", 35_000, 62_500),
            ("Dumpster and haul-off", 37_500, 52_500),
            ("Ice and water shield", 42_000, 61_000),
        ],
        Catalog::Software => &[
            ("Team plan - monthly", 4_900, 19_900),
            ("Additional storage 100 GB", 999, 1_999),
            ("Premium support add-on", 2_500, 4_900),
        ],
        Catalog::Office => &[
            ("Copy paper 8.5x11, case", 4_299, 5_899),
            ("Toner cartridge, black", 7_999, 12_999),
            ("Ballpoint pens, box of 12", 699, 1_199),
            ("File folders, letter, 100 ct", 1_499, 2_299),
            ("Sticky notes 3x3, 12 pads", 899, 1_399),
            ("Desk organizer, mesh", 1_999, 3_499),
        ],
        Catalog::Freight => &[
            ("LTL freight Tulsa OK to Rogers AR", 28_500, 64_000),
            ("Liftgate service", 7_500, 9_500),
            ("Residential delivery", 5_500, 8_500),
        ],
        Catalog::Accounting => &[
            ("Monthly bookkeeping", 15_000, 17_500),
            ("Payroll tax filings, Q3", 12_500, 15_000),
            ("Consultation", 15_000, 17_500),
            ("Year-end tax planning", 17_500, 22_500),
        ],
        Catalog::Consulting => &[
            ("Strategy workshop", 17_500, 22_500),
            ("Process review", 15_000, 19_500),
            ("Travel time", 8_500, 9_500),
            ("Inventory system assessment", 16_000, 19_000),
        ],
        Catalog::AutoRepair => &[
            ("Front brake job, pads and rotors", 18_900, 32_900),
            ("Oil and filter change", 4_999, 8_999),
            ("Diagnostic, check engine light", 9_500, 12_900),
            ("Tire rotation", 2_500, 3_500),
            ("Coolant flush", 8_999, 12_999),
        ],
        Catalog::Landscaping => &[
            ("Weekly mowing and trim", 4_500, 7_500),
            ("Hedge trimming", 8_500, 15_000),
            ("Mulch, installed, per yard", 6_500, 8_500),
            ("Leaf cleanup", 12_000, 22_000),
            ("Irrigation check", 7_500, 9_500),
        ],
        Catalog::Fuel => &[("UNLEADED", 2_899, 3_499), ("DIESEL", 3_299, 3_999)],
        Catalog::Print => &[
            ("Business cards, 500 ct", 3_900, 5_900),
            ("Tri-fold brochures, 250", 14_900, 21_900),
            ("Vinyl banner 3 x 6 ft", 8_900, 12_900),
            ("Window decal", 2_900, 4_900),
        ],
        Catalog::Insurance => &[
            ("Commercial general liability", 42_000, 98_000),
            ("Commercial property", 31_000, 76_000),
            ("Policy fee", 2_500, 2_500),
        ],
        Catalog::Electric => &[
            ("Energy charge", 18_000, 64_000),
            ("Customer charge", 2_500, 2_500),
            ("Fuel cost adjustment", 450, 3_900),
            ("Franchise fee", 300, 1_900),
        ],
        Catalog::Water => &[
            ("Water usage", 2_800, 9_500),
            ("Sewer service", 2_400, 7_800),
            ("Base charge", 1_450, 1_450),
            ("Stormwater fee", 400, 400),
        ],
        Catalog::Telecom => &[
            ("Business internet 300 Mbps", 8_999, 12_999),
            ("Business phone lines (3)", 7_497, 8_997),
            ("Static IP", 1_500, 1_500),
            ("Equipment rental", 1_000, 1_500),
        ],
    }
}

/// The documents' dates: 2025-09-01 .. 2026-09-30.
fn date(rng: &mut Rng) -> Date {
    Date {
        y: 2025,
        m: 9,
        d: 1,
    }
    .plus(rng.range(0, 394))
}

impl Doc {
    /// The `seq`th document of `layout` (seeded by layout and number).
    #[allow(clippy::too_many_lines)]
    pub fn new(layout: &Layout, seq: u32, seed: &Rng) -> Doc {
        let mut rng = seed.fork(&format!("{}/{seq}", layout.id));
        let date = date(&mut rng);
        let time = format!(
            "{:02}:{:02}{}",
            rng.range(1, 12),
            rng.range(0, 59),
            if rng.chance(50) { " PM" } else { " AM" }
        );
        let terms_days = (!layout.terms.is_empty()).then(|| *rng.pick(layout.terms));
        let due = match (layout.due_label, terms_days) {
            (Some(_), Some(d)) => Some(date.plus(i64::from(d))),
            (Some(_), None) => Some(date.plus(30)),
            _ => None,
        };
        let reference = layout
            .ref_label
            .map(|_| reference(&mut rng, layout.ref_shape));
        let decoy = layout.decoy_label.map(|_| {
            let n = u32::try_from(rng.range(4, 7)).unwrap_or(5);
            digits(&mut rng, n)
        });
        let account = layout.account_label.map(|_| {
            format!("{}-{}", digits(&mut rng, 4), {
                let n = u32::try_from(rng.range(4, 6)).unwrap_or(5);
                digits(&mut rng, n)
            })
        });
        let customer = rng.below(CUSTOMERS.len());
        let cat = catalog(layout.catalog);
        let n = match layout.family {
            Family::Slip => 0,
            Family::Utility => i64::try_from(cat.len()).unwrap_or(1).min(4),
            _ => rng.range(layout.lines.0, layout.lines.1),
        };
        let mut lines = Vec::new();
        let mut used = Vec::new();
        for i in 0..n {
            let k = if matches!(layout.family, Family::Utility) {
                usize::try_from(i).unwrap_or(0)
            } else if layout.family == Family::Statement || used.len() >= cat.len() {
                rng.below(cat.len())
            } else {
                let mut k = rng.below(cat.len());
                while used.contains(&k) {
                    k = (k + 1) % cat.len();
                }
                k
            };
            used.push(k);
            let (desc, lo, hi) = cat[k];
            let price_cents = rng.range(lo, hi);
            let (qty, rate) = match layout.qty {
                QtyKind::One => (1000, price_cents * 10),
                QtyKind::Hours => (rng.range(1, 24) * 250, price_cents * 10),
                QtyKind::Gallons => (rng.range(4_000, 24_999), price_cents + rng.range(0, 9)),
                QtyKind::Units => {
                    let q = if matches!(layout.family, Family::Receipt) {
                        if rng.chance(75) { 1 } else { rng.range(2, 4) }
                    } else if price_cents > 100_000 || layout.family == Family::Utility {
                        1
                    } else {
                        rng.range(1, 24)
                    };
                    (q * 1000, price_cents * 10)
                }
            };
            let amount = extend(qty, rate);
            let code = match layout.catalog {
                Catalog::AutoRepair => format!("{}", rng.range(1, 9)),
                _ => format!(
                    "{}{}",
                    char::from(b'A' + u8::try_from(rng.range(0, 25)).unwrap_or(0)),
                    digits(&mut rng, 5)
                ),
            };
            lines.push(LineItem {
                code,
                description: desc.to_string(),
                qty,
                rate,
                amount,
            });
        }
        if layout.family == Family::Statement {
            lines.sort_by_key(|l| l.code.clone());
        }
        let mut subtotal: i64 = lines.iter().map(|l| l.amount).sum();
        if layout.family == Family::Slip {
            subtotal = rng.range(1_200, 9_800);
        }
        let discount = if layout.discount.is_some() && rng.chance(70) {
            percent(subtotal, *rng.pick(&[500, 1000, 1500]))
        } else {
            0
        };
        let fee = if layout.fee.is_some() {
            rng.range(1_500, 12_500)
        } else {
            0
        };
        let tax = if layout.tax_bp > 0 {
            percent(subtotal - discount, layout.tax_bp)
        } else {
            0
        };
        let tip = if layout.tip && rng.chance(85) {
            percent(subtotal, *rng.pick(&[1500, 1800, 2000, 2200]))
        } else {
            0
        };
        let mut acct = Account::default();
        if layout.family == Family::Utility {
            acct.previous = rng.range(4_000, 60_000);
            acct.payments = if rng.chance(80) {
                acct.previous
            } else {
                rng.range(0, acct.previous)
            };
            acct.period = Some((date.plus(-33), date.plus(-3)));
        }
        let charges = subtotal - discount + fee + tax + tip;
        let total = if layout.family == Family::Utility {
            charges + acct.previous - acct.payments
        } else {
            charges
        };
        let deposit = if layout.paid == Paid::Deposit {
            percent(total, *rng.pick(&[2500, 3000, 5000]))
        } else {
            0
        };
        let payment = match layout.paid {
            Paid::Card => Some(card(&mut rng)),
            Paid::CardOrCash => {
                if rng.chance(70) {
                    Some(card(&mut rng))
                } else {
                    let tendered = ((total / 2_000) + 1) * 2_000;
                    Some(Payment::Cash { tendered })
                }
            }
            Paid::Stamp => Some(Payment::Stamp),
            Paid::No | Paid::Deposit => None,
        };
        // A few documents per set have no ToUnicode map (review S15): the
        // last document of every third layout.
        let index = super::layouts::all()
            .iter()
            .position(|l| l.id == layout.id)
            .unwrap_or(0);
        let tounicode = !(seq == 10 && index % 3 == 0);
        Doc {
            layout: *layout,
            seq,
            date,
            time,
            due,
            terms_days,
            reference,
            decoy,
            account,
            customer,
            lines,
            subtotal,
            discount,
            fee,
            tax,
            tip,
            total,
            deposit,
            payment,
            acct,
            tounicode,
        }
    }

    pub fn balance_due(&self) -> Option<i64> {
        (self.deposit > 0).then(|| self.total - self.deposit)
    }

    pub fn last4(&self) -> Option<&str> {
        match &self.payment {
            Some(Payment::Card { last4, .. }) => Some(last4),
            _ => None,
        }
    }

    pub fn terms_text(&self) -> Option<String> {
        self.terms_days.map(|d| {
            if d == 0 {
                "Due on receipt".to_string()
            } else {
                format!("Net {d}")
            }
        })
    }

    /// The ground truth: what a perfect reader returns. `null` marks a
    /// field the document does not print.
    pub fn truth(&self) -> Value {
        let l = &self.layout;
        let opt = |v: Option<i64>| v.map_or(Value::Null, |c| Value::String(wire(c)));
        let amount_paid = match (&self.payment, self.deposit) {
            (_, d) if d > 0 => Some(d),
            (Some(Payment::Card { .. } | Payment::Stamp | Payment::Cash { .. }), _) => {
                Some(self.total)
            }
            (None, _) => None,
        };
        json!({
            "schema": 1,
            "layout": l.id,
            "family": l.family.name(),
            "holdout": l.holdout,
            "font": l.font.family(),
            "tounicode": self.tounicode,
            "seq": self.seq,
            "vendor": { "key": l.vendor.key, "name": l.vendor.name },
            "date": self.date.iso(),
            "reference": self.reference,
            "total": wire(self.total),
            "subtotal": if l.subtotal_label.is_some() && l.family != super::layouts::Family::Utility {
                Value::String(wire(self.subtotal))
            } else {
                Value::Null
            },
            "tax": if l.tax_label.is_some() && l.tax_bp > 0 { Value::String(wire(self.tax)) } else { Value::Null },
            "due_date": self.due.map(Date::iso),
            "terms": l.terms_label.and_then(|_| self.terms_text()),
            "account_number": self.account,
            "amount_paid": opt(amount_paid),
            "balance_due": opt(self.balance_due()),
            "card_last4": self.last4(),
            "lines": self.lines.iter().map(|li| json!({
                "description": li.description,
                "quantity": milli_wire(li.qty),
                "unit_price": milli_wire(li.rate),
                "amount": wire(li.amount),
            })).collect::<Vec<_>>(),
        })
    }
}

fn card(rng: &mut Rng) -> Payment {
    Payment::Card {
        brand: rng.pick(&["VISA", "MASTERCARD", "AMEX", "DISCOVER"]),
        last4: digits(rng, 4).chars().rev().collect(),
    }
}

/// The vendor roster the harness matches against: every layout's vendor
/// and a confusable name for each (review S15), with phone and account
/// numbers as an owner would have them.
pub fn roster(layouts: &[Layout]) -> Value {
    let mut out = Vec::new();
    for l in layouts {
        let v = &l.vendor;
        out.push(json!({
            "key": v.key,
            "name": v.name,
            "phone": v.phone,
            "address": format!("{}\n{}", v.street, v.city),
        }));
        out.push(json!({
            "key": format!("{}~confusable", v.key),
            "name": v.confusable,
            "phone": Value::Null,
            "address": Value::Null,
        }));
    }
    Value::Array(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gen_invoices::layouts::all;

    #[test]
    fn documents_add_up_and_are_reproducible() {
        let seed = Rng::new(42);
        for l in all() {
            for seq in 1..=10 {
                let d = Doc::new(&l, seq, &seed);
                let again = Doc::new(&l, seq, &seed);
                assert_eq!(d.truth(), again.truth(), "{}", l.id);
                let lines: i64 = d.lines.iter().map(|x| x.amount).sum();
                if l.family != Family::Slip {
                    assert_eq!(lines, d.subtotal, "{}", l.id);
                }
                assert!(d.total > 0, "{} {seq}", l.id);
                assert_eq!(d.reference.is_some(), l.ref_label.is_some());
            }
        }
        let seed = &seed;
        let no_unicode = all()
            .iter()
            .flat_map(|l| (1..=10).map(move |s| Doc::new(l, s, seed)))
            .filter(|d| !d.tounicode)
            .count();
        // One of them from the shared layout (review S4).
        assert_eq!(no_unicode, 9);
    }
}
