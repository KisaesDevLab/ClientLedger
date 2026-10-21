//! `cargo xtask gen-sample [--check]`: the practice company (Q-219, Q-226).
//!
//! Writes `fixtures/sample/ozark_ridge/` for **Ozark Ridge Landscaping
//! LLC**: one year of synthetic books (1,512 transactions, 214 with an
//! attachment flag) plus a bank feed, generated deterministically from a
//! fixed seed so the files never change between runs. A handful of
//! "plug" transactions at the end make the figures in
//! `docs/design/README.md` come out exactly; `cl-company`'s golden test
//! asserts every one of them from the fixture.
//!
//! The P14 money story rides on top without moving a design figure: two
//! customer payments waiting in Payments Clearing (10150), the 11/04 bank
//! file's deposit of both, the "Plant sale counter" daily-sales layout and
//! the lender's 11/04 statement for the truck loan (`lender_statement.json`).
//! P15 adds three templates (`templates.json`); the automatic one posts
//! two 49.00 card charges on the first open, after the design figures.
//!
//! Amounts are integer cents here and decimal strings in the files; no
//! floating point anywhere.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Story date of the mockups.
const STORY_DATE: (i32, u32, u32) = (2026, 11, 4);
/// Books closed through.
const CLOSING_DATE: &str = "2026-09-30";
/// Target figures (docs/design/README.md), in cents: 84,210.55 and so on.
const ENDING_1010: i64 = 8_421_055;
const BANK_FILE_1010: i64 = 9_243_107;
const LAST_RECONCILED_1010: i64 = 7_143_620;
const OCT_STATEMENT_1010: i64 = 8_690_213;
const OPEN_AR: i64 = 4_891_240;
const BILLS_DUE_7: i64 = 747_190;
const NI_OCT: i64 = 1_386_514;
const NI_JAN_OCT: i64 = 12_598_500;
const TXN_COUNT: usize = 1_512;
const ATTACHMENT_COUNT: usize = 214;
const FEED_READY: usize = 28;
const FEED_REVIEW: usize = 6;
const FEED_MATCH: usize = 3;

/// Deterministic xorshift64* generator.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `lo..=hi`.
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        let span = u64::try_from(hi - lo + 1).unwrap_or(1);
        lo + i64::try_from(self.next() % span).unwrap_or(0)
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        let i = usize::try_from(self.next() % items.len() as u64).unwrap_or(0);
        &items[i]
    }
}

/// A calendar date without a date library: day-of-year arithmetic for 2026.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Day(i32, u32, u32);

const DAYS_IN_MONTH: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

impl Day {
    fn text(self) -> String {
        format!("{:04}-{:02}-{:02}", self.0, self.1, self.2)
    }

    fn add_days(self, n: u32) -> Day {
        let mut d = self;
        for _ in 0..n {
            if d.2 < DAYS_IN_MONTH[(d.1 - 1) as usize] {
                d.2 += 1;
            } else if d.1 < 12 {
                d.1 += 1;
                d.2 = 1;
            } else {
                d.0 += 1;
                d.1 = 1;
                d.2 = 1;
            }
        }
        d
    }
}

#[derive(Clone, Debug)]
struct Line {
    account: &'static str,
    debit: i64,
    credit: i64,
    /// Line memo (the Write Check story lines only).
    memo: Option<&'static str>,
    /// Line tag name (story lines only).
    tag: Option<&'static str>,
    /// `Customer:Job` the line is for (story lines only).
    job: Option<&'static str>,
}

#[derive(Clone, Debug)]
struct Txn {
    date: Day,
    kind: &'static str,
    number: String,
    contact: Option<String>,
    memo: String,
    lines: Vec<Line>,
    /// For bills: the due date.
    due: Option<Day>,
    /// Bank lines cleared on a statement (1010 only).
    cleared: bool,
    attachment: bool,
    /// A bill that was paid (set when a payment references it).
    paid: bool,
}

fn line(account: &'static str, debit: i64, credit: i64) -> Line {
    Line {
        account,
        debit,
        credit,
        memo: None,
        tag: None,
        job: None,
    }
}

/// A Write Check story expense line: a debit with a memo, the Install tag
/// and, optionally, the job it is for.
fn story_line(
    account: &'static str,
    debit: i64,
    memo: &'static str,
    job: Option<&'static str>,
) -> Line {
    Line {
        memo: Some(memo),
        tag: Some(STORY_TAG),
        job,
        ..line(account, debit, 0)
    }
}

fn cents(c: i64) -> String {
    let sign = if c < 0 { "-" } else { "" };
    let a = c.abs();
    format!("{sign}{}.{:02}", a / 100, a % 100)
}

const ACCOUNTS: &[(&str, &str, &str, &str)] = &[
    ("1010", "Operating Checking", "asset", "checking"),
    ("1020", "Savings", "asset", "savings"),
    ("1500", "Equipment", "asset", "fixed_asset"),
    ("1510", "Vehicles", "asset", "fixed_asset"),
    ("2050", "Business Credit Card", "liability", "credit_card"),
    (
        "2200",
        "Vehicle and Equipment Loans",
        "liability",
        "long_term_liability",
    ),
    ("30160", "Owner's Contribution", "equity", ""),
    ("30170", "Owner's Draw", "equity", ""),
    ("4000", "Service Income", "revenue", ""),
    ("5000", "Subcontractors", "cogs", ""),
    ("5090", "Materials", "cogs", ""),
    ("5100", "Plants", "cogs", ""),
    ("5110", "Mulch & stone", "cogs", ""),
    ("5200", "Job Supplies and Materials", "cogs", ""),
    ("6000", "Advertising and Marketing", "expense", ""),
    ("6050", "Bank and Merchant Fees", "expense", ""),
    ("6100", "Fuel", "expense", ""),
    ("6150", "Insurance", "expense", ""),
    ("6250", "Office Supplies and Software", "expense", ""),
    ("6300", "Rent", "expense", ""),
    ("6350", "Repairs and Maintenance", "expense", ""),
    ("6400", "Telephone and Internet", "expense", ""),
    ("6500", "Utilities", "expense", ""),
    ("6550", "Wages", "expense", ""),
    ("6560", "Payroll Taxes", "expense", ""),
    ("6700", "Vehicle Expenses", "expense", ""),
    ("8000", "Interest Expense", "other_expense", ""),
];
/// Sub-accounts: (number, parent number). 5100 and 5110 show as
/// "Materials:Plants" and "Materials:Mulch & stone" (the Write Check story).
const ACCOUNT_PARENTS: &[(&str, &str)] = &[("5100", "5090"), ("5110", "5090")];

// ---- The Write Check story (P13 WP0, docs/design/mockups/WriteCheck.html) --
/// The story's vendor. It takes the list slot of a vendor no scenario uses,
/// so the vendor count stays 38, and has no random history: only the three
/// prior checks and the open bill below.
const STORY_VENDOR: &str = "Ozark Feed & Seed";
/// Random picks that land on the story vendor's slot go here instead.
const STORY_VENDOR_STAND_IN: &str = "Persimmon Hill Plants";
/// The tag on every story line; "Crew A" is the second story tag.
const STORY_TAG: &str = "Install";
const STORY_TAGS: &[(&str, Option<&str>)] = &[(STORY_TAG, None), ("Crew A", Some("Crew"))];
/// The job the story lines are for.
const STORY_CUSTOMER: &str = "Hollister Family Trust";
const STORY_JOB_NAME: &str = "Fall bed install";
const STORY_JOB: &str = "Hollister Family Trust:Fall bed install";
/// Prior checks to the story vendor on 1010: number, date, plants, mulch.
const STORY_CHECKS: &[(u32, Day, i64, i64)] = &[
    (4380, Day(2026, 8, 27), 49_180, 26_450),
    (4392, Day(2026, 9, 18), 123_666, 66_745),
    (4401, Day(2026, 10, 2), 148_735, 83_125),
];
/// The open bill from the story vendor: plants, mulch, dated, due.
const STORY_BILL: (i64, i64, Day, Day) = (84_215, 44_425, Day(2026, 10, 6), Day(2026, 11, 5));
/// The highest check number on 1010; the next check is 4418.
const LAST_CHECK_1010: u32 = 4417;

/// Engine system accounts the fixture also posts to.
const AR: &str = "10200";
const AP: &str = "20100";
const OPENING: &str = "30000";
/// Engine system accounts the P14 story uses: Payments Clearing (payments
/// received but not yet taken to the bank) and Sales Tax Payable.
const PAYMENTS_CLEARING: &str = "10150";
const SALES_TAX_PAYABLE: &str = "20900";

// ---- The P14 money story (deposits, daily sales, loan) ----------------------
/// Payments waiting to be deposited, each paying its own November invoice so
/// no customer balance, open A/R or 1010 figure moves: customer, the
/// customer's check number, invoice date, payment date, cents.
const DEPOSIT_STORY: &[(&str, &str, Day, Day, i64)] = &[
    (
        "Hollister Family Trust",
        "5521",
        Day(2026, 11, 1),
        Day(2026, 11, 2),
        125_000,
    ),
    (
        "Maple Street Bakery",
        "1094",
        Day(2026, 11, 1),
        Day(2026, 11, 3),
        84_250,
    ),
];
/// The bank deposit on 11/04 that takes both payments to 1010 (the bank
/// file after the 11/03 one the design figures use).
const DEPOSIT_STORY_DATE: Day = Day(2026, 11, 4);
const DEPOSIT_STORY_TOTAL: i64 = 209_250;
/// The truck loan: monthly payment and the interest of the 11/04 statement.
const LOAN_ACCOUNT: &str = "2200";
const LOAN_LENDER: &str = "Lakeview Credit Union";
const LOAN_PAYMENT: i64 = 89_200;
const LOAN_STATEMENT_INTEREST: i64 = 20_743;

const CLIENT_NAMES: &[&str] = &[
    "Riverbend HOA",
    "Table Rock Dental",
    "Hollister Family Trust",
    "Brightwater Apartments",
    "Cedar Hollow Church",
    "Maple Street Bakery",
    "Northfork Veterinary",
    "Pine Ridge Storage",
    "Sunset Trail Realty",
    "Lakeview Credit Union",
    "Branson Hills Inn",
    "Juniper Court Condos",
    "Ozark Mountain Daycare",
    "Silver Dollar Rentals",
    "Whitetail Hunting Lodge",
    "Greenway Fitness",
    "Highway 65 Auto",
    "Kimberling Marina",
    "Bluff View Estates",
    "Forsyth Public Library",
    "Walnut Shade Farmhouse",
    "Copper Kettle Coffee",
    "Taney County Title",
    "Shepherd Hills Clinic",
    "Reeds Spring Hardware",
    "Bull Shoals Cabins",
    "Indian Point Resort",
    "Rockaway Beach Diner",
    "Crane Creek Nursery",
    "Galena Gun Club",
    "Lampe Country Store",
    "Blue Eye Feed Co",
    "Protem Lakeside HOA",
    "Powersite Dam Tours",
    "Mincy Hollow Ranch",
    "Ridgedale Chapel",
    "Oak Grove Orchard",
    "Sycamore Log Homes",
    "Cedar Creek Winery",
    "Bear Creek Campground",
    "Dogwood Canyon Rentals",
    "Elm Street Flats",
    "Persimmon Hill Farm",
    "Roark Creek Dentistry",
    "Swan Creek Outfitters",
    "Thunder Ridge Stables",
    "Tri-Lakes Insurance",
    "Wildwood Pediatrics",
    "Mountain Grove Antiques",
    "Stone Mill Bistro",
    "Hickory Ridge Apartments",
    "Lost Canyon Golf",
    "White River Lumber",
    "Compton Ridge Church",
    "Turkey Creek Nursery",
    "Barn Owl Books",
    "Hilltop Veterinary",
    "Cardinal Hill Daycare",
    "Long Creek Marina",
    "Mill Creek Pottery",
    "Spring Branch Law",
    "Sugar Creek Bakery",
    "Timber Lake Lodge",
    "Valley View Assisted Living",
];
const VENDOR_NAMES: &[&str] = &[
    "Ozark Mulch Supply",
    "Tri-Lakes Nursery Wholesale",
    "Branson Equipment Rental",
    "Hollister Fuel Stop",
    "White River Stone Yard",
    "Midwest Irrigation Parts",
    "Taney County Landfill",
    "Table Rock Small Engine",
    "Lakeside Insurance Agency",
    "Ozark Cellular",
    "Empire Electric Cooperative",
    "City of Hollister Water",
    "Shepherd Hills Office Supply",
    "Cedar Hollow Signs",
    "Highway 65 Tire & Lube",
    "Riverbend Hardware",
    "Bluff View Trucking",
    "Forsyth Print Shop",
    "Kimberling Fertilizer",
    "Lampe Seed & Feed",
    "Reeds Spring Welding",
    "Stone County Permits",
    "Pine Ridge Storage Units",
    "Walnut Shade Diesel",
    "Mountain Software Co",
    "Oak Grove Uniforms",
    "Galena Gravel",
    "Powersite Trailer Sales",
    "Thunder Ridge Subcontracting",
    "Crane Creek Tree Service",
    "Bear Creek Hauling",
    "Copper Kettle Catering",
    "Blue Eye Lumber",
    "Rockaway Auto Glass",
    "Sycamore Fence Co",
    STORY_VENDOR,
    "Elm Street Accountants",
    "Persimmon Hill Plants",
];

/// A random vendor. The story vendor's slot stands in for another vendor so
/// the random stream (and every amount and date) is the same as before the
/// story vendor existed.
fn pick_vendor(rng: &mut Rng) -> &'static str {
    let v: &'static str = rng.pick(VENDOR_NAMES);
    if v == STORY_VENDOR {
        STORY_VENDOR_STAND_IN
    } else {
        v
    }
}

/// The Write Check story: three prior checks to the story vendor and one
/// open bill from it, all on 5100/5110 with line memos and the Install tag.
/// Pushed before the plugs, which absorb them, so every design figure stays.
fn push_story(txns: &mut Vec<Txn>, bill_seq: &mut u32) {
    for &(number, date, plants, mulch) in STORY_CHECKS {
        let job = (number == 4401).then_some(STORY_JOB);
        txns.push(Txn {
            date,
            kind: "check",
            number: number.to_string(),
            contact: Some(STORY_VENDOR.into()),
            memo: "Plants and mulch".into(),
            lines: vec![
                story_line("5100", plants, "Perennials", job),
                story_line("5110", mulch, "Hardwood mulch", None),
                line("1010", 0, plants + mulch),
            ],
            due: None,
            cleared: true,
            attachment: false,
            paid: false,
        });
    }
    let (plants, mulch, date, due) = STORY_BILL;
    *bill_seq += 1;
    txns.push(Txn {
        date,
        kind: "bill",
        number: format!("B-{bill_seq}"),
        contact: Some(STORY_VENDOR.into()),
        memo: "Fall bed plants and mulch".into(),
        lines: vec![
            story_line("5100", plants, "Perennials", Some(STORY_JOB)),
            story_line("5110", mulch, "Hardwood mulch", None),
            line(AP, 0, plants + mulch),
        ],
        due: Some(due),
        cleared: false,
        attachment: false,
        paid: false,
    });
}

/// A bill's amount: its credit to A/P.
fn bill_amount(t: &Txn) -> i64 {
    t.lines
        .iter()
        .filter(|l| l.account == AP)
        .map(|l| l.credit)
        .sum()
}

/// Card payments, the truck loan and owner draws go out as bank transfers.
fn is_transfer_payment(t: &Txn) -> bool {
    matches!(
        t.memo.as_str(),
        "Card payment" | "Truck loan payment" | "Owner draw"
    )
}

/// Numbers the 1010 checks in date order so the story holds: the story
/// checks keep 4380, 4392 and 4401, the highest 1010 number is 4417 (a
/// check), and 4418 is free. Payments that do not get a number become
/// unnumbered electronic payments (a check becomes an `expense`). Amounts,
/// dates and lines are untouched, so no figure moves.
fn renumber_checks(txns: &mut [Txn]) {
    let pays_1010 = |t: &Txn| t.lines.iter().any(|l| l.account == "1010" && l.credit > 0);
    let mut cands: Vec<usize> = (0..txns.len())
        .filter(|&i| matches!(txns[i].kind, "check" | "bill_payment") && pays_1010(&txns[i]))
        .collect();
    let unnumber = |t: &mut Txn| {
        t.number.clear();
        if t.kind == "check" {
            t.kind = "expense";
        }
    };
    for &i in &cands {
        if is_transfer_payment(&txns[i]) {
            unnumber(&mut txns[i]);
        }
    }
    cands.retain(|&i| !txns[i].number.is_empty());
    cands.sort_by_key(|&i| (txns[i].date, i));
    let is_anchor = |t: &Txn| t.contact.as_deref() == Some(STORY_VENDOR);
    let anchors: Vec<usize> = (0..cands.len())
        .filter(|&k| is_anchor(&txns[cands[k]]))
        .collect();
    assert_eq!(anchors.len(), STORY_CHECKS.len(), "story checks");
    let numbers: BTreeMap<usize, u32> = anchors
        .iter()
        .map(|&k| (k, txns[cands[k]].number.parse().unwrap_or(0)))
        .collect();
    let anchor_no = |k: usize| -> u32 { numbers[&k] };

    // Before the first anchor: every payment numbered, ending just below it.
    let first = anchors[0];
    let start = anchor_no(first) - u32::try_from(first).unwrap_or(0);
    for (n, &i) in cands[..first].iter().enumerate() {
        txns[i].number = (start + u32::try_from(n).unwrap_or(0)).to_string();
    }
    // Between anchors, and after the last one up to LAST_CHECK_1010: spread
    // the free numbers evenly over the payments; the rest go electronic.
    let mut segments: Vec<(usize, usize, u32, u32)> = anchors
        .windows(2)
        .map(|w| (w[0] + 1, w[1], anchor_no(w[0]) + 1, anchor_no(w[1])))
        .collect();
    let last = anchors[anchors.len() - 1];
    // The last numbered payment is a check, so the next check is 4418.
    let tail_end = (last + 1..cands.len())
        .rev()
        .find(|&k| txns[cands[k]].kind == "check")
        .map_or(last + 1, |k| k + 1);
    segments.push((last + 1, tail_end, anchor_no(last) + 1, LAST_CHECK_1010 + 1));
    for k in tail_end..cands.len() {
        unnumber(&mut txns[cands[k]]);
    }
    for (from, to, lo, hi) in segments {
        let slots = usize::try_from(hi - lo).unwrap_or(0);
        let m = to - from;
        assert!(m >= slots, "{m} payments for {slots} numbers from {lo}");
        let mut next = lo;
        for j in 0..m {
            let i = cands[from + j];
            // Keep j when the even spread crosses a slot boundary there (the
            // last payment of the segment always gets a number).
            if (j + 1) * slots / m > j * slots / m {
                txns[i].number = next.to_string();
                next += 1;
            } else {
                unnumber(&mut txns[i]);
            }
        }
        assert_eq!(next, hi);
    }
}

/// The generated fixture.
struct Fixture {
    txns: Vec<Txn>,
    feed: Vec<Value>,
}

#[allow(clippy::too_many_lines)]
fn generate() -> Fixture {
    let mut rng = Rng::new(0x0A4B_1D6E_2026_0001);
    let mut txns: Vec<Txn> = Vec::new();
    let mut invoice_no = 1001u32;
    let mut check_no = 4001u32;
    let mut bill_seq = 0u32;

    // Opening balances as of 1/1/2026.
    txns.push(Txn {
        date: Day(2026, 1, 1),
        kind: "journal_entry",
        number: "OB-1".into(),
        contact: None,
        memo: "Opening balances".into(),
        lines: vec![
            line("1010", 5_500_000, 0),
            line("1020", 1_200_000, 0),
            line("1500", 4_800_000, 0),
            line("1510", 6_200_000, 0),
            line("2200", 0, 3_800_000),
            line(OPENING, 0, 13_900_000),
        ],
        due: None,
        cleared: true,
        attachment: false,
        paid: false,
    });

    let expense_accounts: &[(&str, i64, i64)] = &[
        ("6100", 4500, 32_000),
        ("6250", 2000, 24_000),
        ("6350", 6000, 90_000),
        ("6700", 4000, 60_000),
        ("6000", 5000, 45_000),
        ("5200", 8000, 140_000),
    ];
    let bill_accounts: &[(&str, i64, i64)] = &[
        ("5000", 40_000, 320_000),
        ("5200", 15_000, 240_000),
        ("6350", 9000, 110_000),
        ("6150", 30_000, 90_000),
        ("6500", 12_000, 42_000),
        ("6400", 9000, 26_000),
        ("6300", 145_000, 145_000),
    ];

    // Open invoices are collected per month so payments can settle them.
    let mut open_invoices: Vec<(usize, i64)> = Vec::new();
    let mut open_bills: Vec<usize> = Vec::new();

    for month in 1..=11u32 {
        let days_this_month = if month == 11 {
            4
        } else {
            DAYS_IN_MONTH[(month - 1) as usize]
        };
        let scale = if month == 11 { 4 } else { 30 };
        let invoices = 34 * scale / 30;
        let bills = 19 * scale / 30;
        let checks = 21 * scale / 30;
        let cards = 11 * scale / 30;
        let day = |rng: &mut Rng| {
            Day(
                2026,
                month,
                u32::try_from(rng.range(1, i64::from(days_this_month))).unwrap_or(1),
            )
        };
        let cleared_cut = Day(2026, 10, 31);

        for _ in 0..invoices {
            let d = day(&mut rng);
            let amount = rng.range(18_000, 480_000);
            let client = rng.pick(CLIENT_NAMES);
            txns.push(Txn {
                date: d,
                kind: "invoice",
                number: format!("INV-{invoice_no}"),
                contact: Some((*client).into()),
                memo: "Landscaping services".into(),
                lines: vec![line(AR, amount, 0), line("4000", 0, amount)],
                due: Some(d.add_days(30)),
                cleared: false,
                attachment: false,
                paid: false,
            });
            invoice_no += 1;
            open_invoices.push((txns.len() - 1, amount));
        }
        // Payments: most invoices older than ~20 days get paid.
        let mut still_open = Vec::new();
        for (idx, amount) in open_invoices.drain(..) {
            let inv_date = txns[idx].date;
            let pay_day = inv_date.add_days(u32::try_from(rng.range(12, 38)).unwrap_or(20));
            if pay_day <= Day(2026, 11, 4) && rng.range(0, 99) < 92 {
                let client = txns[idx].contact.clone();
                txns.push(Txn {
                    date: pay_day,
                    kind: "payment",
                    number: format!("PMT-{}", txns[idx].number),
                    contact: client,
                    memo: "Payment received".into(),
                    lines: vec![line("1010", amount, 0), line(AR, 0, amount)],
                    due: None,
                    cleared: pay_day <= cleared_cut,
                    attachment: false,
                    paid: false,
                });
                txns[idx].paid = true;
            } else {
                still_open.push((idx, amount));
            }
        }
        open_invoices = still_open;

        for _ in 0..bills {
            let d = day(&mut rng);
            let (acct, lo, hi) = *rng.pick(bill_accounts);
            let amount = rng.range(lo, hi);
            let vendor = pick_vendor(&mut rng);
            bill_seq += 1;
            txns.push(Txn {
                date: d,
                kind: "bill",
                number: format!("B-{bill_seq}"),
                contact: Some(vendor.into()),
                memo: "Vendor bill".into(),
                lines: vec![line(acct, amount, 0), line(AP, 0, amount)],
                due: Some(d.add_days(30)),
                cleared: false,
                attachment: bill_seq.is_multiple_of(3),
                paid: false,
            });
            open_bills.push(txns.len() - 1);
        }
        // Bill payments: bills older than ~15 days are paid by check.
        let mut still_open = Vec::new();
        for idx in open_bills.drain(..) {
            let bill_date = txns[idx].date;
            let pay_day = bill_date.add_days(u32::try_from(rng.range(10, 28)).unwrap_or(15));
            if pay_day <= Day(2026, 10, 28) {
                let amount = txns[idx].lines[0].debit;
                let vendor = txns[idx].contact.clone();
                txns.push(Txn {
                    date: pay_day,
                    kind: "bill_payment",
                    number: format!("{check_no}"),
                    contact: vendor,
                    memo: "Bill payment".into(),
                    lines: vec![line(AP, amount, 0), line("1010", 0, amount)],
                    due: None,
                    cleared: pay_day <= cleared_cut,
                    attachment: false,
                    paid: false,
                });
                check_no += 1;
                txns[idx].paid = true;
            } else {
                still_open.push(idx);
            }
        }
        open_bills = still_open;

        for _ in 0..checks {
            let d = day(&mut rng);
            let (acct, lo, hi) = *rng.pick(expense_accounts);
            let amount = rng.range(lo, hi);
            let vendor = pick_vendor(&mut rng);
            txns.push(Txn {
                date: d,
                kind: "check",
                number: format!("{check_no}"),
                contact: Some(vendor.into()),
                memo: "Check".into(),
                lines: vec![line(acct, amount, 0), line("1010", 0, amount)],
                due: None,
                cleared: d <= cleared_cut,
                attachment: check_no.is_multiple_of(4),
                paid: false,
            });
            check_no += 1;
        }
        for _ in 0..cards {
            let d = day(&mut rng);
            let (acct, lo, hi) = *rng.pick(expense_accounts);
            let amount = rng.range(lo, hi.min(40_000));
            let vendor = pick_vendor(&mut rng);
            txns.push(Txn {
                date: d,
                kind: "card_charge",
                number: String::new(),
                contact: Some(vendor.into()),
                memo: "Card charge".into(),
                lines: vec![line(acct, amount, 0), line("2050", 0, amount)],
                due: None,
                cleared: false,
                attachment: false,
                paid: false,
            });
        }
        if month <= 10 {
            // Card statement paid in full on the 20th.
            let card_total: i64 = txns
                .iter()
                .filter(|t| t.kind == "card_charge" && t.date.1 == month)
                .map(|t| t.lines[1].credit)
                .sum();
            let d = Day(2026, month, 20);
            txns.push(Txn {
                date: d,
                kind: "check",
                number: format!("{check_no}"),
                contact: Some("Business Credit Card".into()),
                memo: "Card payment".into(),
                lines: vec![line("2050", card_total, 0), line("1010", 0, card_total)],
                due: None,
                cleared: d <= cleared_cut,
                attachment: false,
                paid: false,
            });
            check_no += 1;
            // Two payrolls.
            for pd in [8u32, 22] {
                let d = Day(2026, month, pd);
                let wages = 618_000 + rng.range(-20_000, 20_000);
                let taxes = wages * 9 / 100;
                txns.push(Txn {
                    date: d,
                    kind: "journal_entry",
                    number: format!("PR-{month}-{pd}"),
                    contact: None,
                    memo: "Payroll".into(),
                    lines: vec![
                        line("6550", wages, 0),
                        line("6560", taxes, 0),
                        line("1010", 0, wages + taxes),
                    ],
                    due: None,
                    cleared: d <= cleared_cut,
                    attachment: true,
                    paid: false,
                });
            }
            // Loan payment and owner draw.
            let d = Day(2026, month, 15);
            txns.push(Txn {
                date: d,
                kind: "check",
                number: format!("{check_no}"),
                contact: Some("Lakeview Credit Union".into()),
                memo: "Truck loan payment".into(),
                lines: vec![
                    line("2200", 68_000, 0),
                    line("8000", 21_200, 0),
                    line("1010", 0, 89_200),
                ],
                due: None,
                cleared: d <= cleared_cut,
                attachment: false,
                paid: false,
            });
            check_no += 1;
            let d = Day(2026, month, 28);
            txns.push(Txn {
                date: d,
                kind: "check",
                number: format!("{check_no}"),
                contact: Some("Kurt (owner)".into()),
                memo: "Owner draw".into(),
                lines: vec![line("30170", 350_000, 0), line("1010", 0, 350_000)],
                due: None,
                cleared: d <= cleared_cut,
                attachment: false,
                paid: false,
            });
            check_no += 1;
        }
    }
    push_story(&mut txns, &mut bill_seq);

    // ---- Plugs that make the design figures exact --------------------------
    let ni = |txns: &[Txn], from: Day, to: Day| -> i64 {
        txns.iter()
            .filter(|t| t.date >= from && t.date <= to)
            .flat_map(|t| t.lines.iter())
            .map(|l| match account_kind(l.account) {
                "revenue" => l.credit - l.debit,
                "cogs" | "expense" | "other_expense" => -(l.debit - l.credit),
                _ => 0,
            })
            .sum()
    };
    // (1) Jan..Sep: income plug on 9/29 (an invoice paid in October) so
    //     Jan..Oct net income lands exactly; (2) October plug on 10/30.
    let oct = (Day(2026, 10, 1), Day(2026, 10, 31));
    let oct_now = ni(&txns, oct.0, oct.1);
    let oct_diff = NI_OCT - oct_now;
    push_income_plug(
        &mut txns,
        Day(2026, 10, 30),
        oct_diff,
        &mut invoice_no,
        "INV",
    );
    let jan_oct_now = ni(&txns, Day(2026, 1, 1), Day(2026, 10, 31));
    let diff = NI_JAN_OCT - jan_oct_now;
    push_income_plug(&mut txns, Day(2026, 9, 29), diff, &mut invoice_no, "INV");
    assert_eq!(ni(&txns, oct.0, oct.1), NI_OCT);
    assert_eq!(ni(&txns, Day(2026, 1, 1), Day(2026, 10, 31)), NI_JAN_OCT);

    // (3) Cleared 1010 through 9/30 and (4) through 10/31: owner equity
    //     deposits or draws, cleared, so the P&L is untouched.
    let cleared_through = |txns: &[Txn], to: Day| -> i64 {
        txns.iter()
            .filter(|t| t.cleared && t.date <= to)
            .flat_map(|t| t.lines.iter())
            .filter(|l| l.account == "1010")
            .map(|l| l.debit - l.credit)
            .sum()
    };
    let d930 = LAST_RECONCILED_1010 - cleared_through(&txns, Day(2026, 9, 30));
    push_equity_plug(&mut txns, Day(2026, 9, 30), d930, true, &mut check_no);
    let d1031 = OCT_STATEMENT_1010 - cleared_through(&txns, Day(2026, 10, 31));
    push_equity_plug(&mut txns, Day(2026, 10, 31), d1031, true, &mut check_no);
    assert_eq!(
        cleared_through(&txns, Day(2026, 9, 30)),
        LAST_RECONCILED_1010
    );
    assert_eq!(
        cleared_through(&txns, Day(2026, 10, 31)),
        OCT_STATEMENT_1010
    );

    // (5) Open A/R: one more open invoice on 11/02.
    let open_ar = |txns: &[Txn]| -> i64 {
        txns.iter()
            .flat_map(|t| t.lines.iter())
            .filter(|l| l.account == AR)
            .map(|l| l.debit - l.credit)
            .sum()
    };
    let ar_diff = OPEN_AR - open_ar(&txns);
    assert!(ar_diff > 0, "open A/R plug must be positive: {ar_diff}");
    push_income_plug(&mut txns, Day(2026, 11, 2), ar_diff, &mut invoice_no, "INV");
    assert_eq!(open_ar(&txns), OPEN_AR);

    // (6) Bills due within 7 days: unpaid bills due on or before 11/11.
    let due_soon = |txns: &[Txn]| -> i64 {
        txns.iter()
            .filter(|t| {
                t.kind == "bill" && !t.paid && t.due.is_some_and(|d| d <= Day(2026, 11, 11))
            })
            .map(bill_amount)
            .sum()
    };
    let bills_diff = BILLS_DUE_7 - due_soon(&txns);
    assert!(bills_diff > 0, "bills plug must be positive: {bills_diff}");
    bill_seq += 1;
    txns.push(Txn {
        date: Day(2026, 11, 1),
        kind: "bill",
        number: format!("B-{bill_seq}"),
        contact: Some("Ozark Mulch Supply".into()),
        memo: "Fall mulch delivery".into(),
        lines: vec![line("5200", bills_diff, 0), line(AP, 0, bills_diff)],
        due: Some(Day(2026, 11, 8)),
        cleared: false,
        attachment: true,
        paid: false,
    });
    assert_eq!(due_soon(&txns), BILLS_DUE_7);

    // (7) Books balance of 1010 on the story date (uncleared owner move).
    let balance_1010 = |txns: &[Txn]| -> i64 {
        txns.iter()
            .flat_map(|t| t.lines.iter())
            .filter(|l| l.account == "1010")
            .map(|l| l.debit - l.credit)
            .sum()
    };
    let d1104 = ENDING_1010 - balance_1010(&txns);
    push_equity_plug(&mut txns, Day(2026, 11, 3), d1104, false, &mut check_no);
    assert_eq!(balance_1010(&txns), ENDING_1010);

    // (8) The P14 deposit story: invoices paid into Payments Clearing. Net
    //     zero on A/R and on every customer; 1010 and Jan-Oct untouched.
    push_deposit_story(&mut txns, &mut invoice_no);
    assert_eq!(open_ar(&txns), OPEN_AR);
    assert_eq!(balance_1010(&txns), ENDING_1010);

    // ---- Pad to the exact count with savings deposits (1020 only) ----------
    assert!(
        txns.len() <= TXN_COUNT,
        "generated {} > {TXN_COUNT}",
        txns.len()
    );
    let mut pad = 0u32;
    while txns.len() < TXN_COUNT {
        pad += 1;
        txns.push(Txn {
            date: Day(2026, 11, 4),
            kind: "deposit",
            number: format!("DEP-{pad}"),
            contact: Some("Kurt (owner)".into()),
            memo: "Owner contribution to savings".into(),
            lines: vec![line("1020", 5000, 0), line("30160", 0, 5000)],
            due: None,
            cleared: false,
            attachment: false,
            paid: false,
        });
    }
    // Attachments: exactly 214.
    let have: usize = txns.iter().filter(|t| t.attachment).count();
    if have > ATTACHMENT_COUNT {
        let mut extra = have - ATTACHMENT_COUNT;
        for t in txns.iter_mut().rev() {
            if extra == 0 {
                break;
            }
            if t.attachment {
                t.attachment = false;
                extra -= 1;
            }
        }
    } else {
        let mut need = ATTACHMENT_COUNT - have;
        for t in &mut txns {
            if need == 0 {
                break;
            }
            if !t.attachment && t.kind == "check" {
                t.attachment = true;
                need -= 1;
            }
        }
    }
    assert_eq!(
        txns.iter().filter(|t| t.attachment).count(),
        ATTACHMENT_COUNT
    );
    renumber_checks(&mut txns);
    txns.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.number.cmp(&b.number)));

    // ---- Bank feed: 37 items from 11/01..11/03 ---------------------------------
    let mut feed = Vec::new();
    let uncleared: Vec<&Txn> = txns
        .iter()
        .filter(|t| t.lines.iter().any(|l| l.account == "1010") && t.date >= Day(2026, 10, 12))
        .collect();
    assert!(
        uncleared.len() >= FEED_READY + FEED_MATCH,
        "{} recent bank items",
        uncleared.len()
    );
    let mut sum = 0i64;
    for (i, t) in uncleared.iter().take(FEED_READY).enumerate() {
        let amt: i64 = t
            .lines
            .iter()
            .filter(|l| l.account == "1010")
            .map(|l| l.debit - l.credit)
            .sum();
        sum += amt;
        feed.push(json!({
            "date": Day(2026, 11, 1 + u32::try_from(i % 3).unwrap_or(0)).text(),
            "description": format!(
                "{} {}",
                t.contact.clone().unwrap_or_else(|| "DEPOSIT".into()).to_uppercase(),
                if t.number.is_empty() { "EFT" } else { t.number.as_str() }
            ),
            "amount": cents(amt),
            "status": "ready",
        }));
    }
    for i in 0..FEED_MATCH {
        let t = uncleared[FEED_READY + i];
        let amt: i64 = t
            .lines
            .iter()
            .filter(|l| l.account == "1010")
            .map(|l| l.debit - l.credit)
            .sum();
        sum += amt;
        feed.push(json!({
            "date": Day(2026, 11, 2).text(),
            "description": format!("POS DEBIT {}", t.contact.clone().unwrap_or_default().to_uppercase()),
            "amount": cents(amt),
            "status": "match",
        }));
    }
    let review = [
        ("SQ *RIDGE TOOL RENTAL", -14_850),
        ("AMZN MKTP US", -6219),
        ("ZELLE FROM T ROCK DENTAL", 92_500),
        ("MO DOR VEHICLE REG", -8425),
        ("CHECK 4502", -31_000),
    ];
    for (desc, amt) in review {
        sum += amt;
        feed.push(json!({"date": Day(2026, 11, 3).text(), "description": desc, "amount": cents(amt), "status": "review"}));
    }
    // The last review item makes the bank file balance exact.
    let last = BANK_FILE_1010 - OCT_STATEMENT_1010 - sum;
    feed.push(json!({"date": Day(2026, 11, 3).text(), "description": "MOBILE DEPOSIT", "amount": cents(last), "status": "review"}));
    assert_eq!(feed.len(), FEED_READY + FEED_REVIEW + FEED_MATCH);
    // The 11/04 bank file: one deposit of both waiting payments. It comes
    // after the 11/03 file, so the design figures (through 11/03) hold.
    feed.push(json!({
        "date": DEPOSIT_STORY_DATE.text(),
        "description": "DEPOSIT",
        "amount": cents(DEPOSIT_STORY_TOTAL),
        "status": "ready",
    }));
    Fixture { txns, feed }
}

/// The P14 deposit story: for each payment, a November invoice and the
/// customer's payment of it into Payments Clearing.
fn push_deposit_story(txns: &mut Vec<Txn>, invoice_no: &mut u32) {
    let mut total = 0;
    for &(customer, check, invoice_date, paid_on, amount) in DEPOSIT_STORY {
        let invoice = format!("INV-{invoice_no}");
        *invoice_no += 1;
        txns.push(Txn {
            date: invoice_date,
            kind: "invoice",
            number: invoice.clone(),
            contact: Some(customer.into()),
            memo: "Landscaping services".into(),
            lines: vec![line(AR, amount, 0), line("4000", 0, amount)],
            due: Some(invoice_date.add_days(30)),
            cleared: false,
            attachment: false,
            paid: true,
        });
        txns.push(Txn {
            date: paid_on,
            kind: "payment",
            number: check.into(),
            contact: Some(customer.into()),
            memo: format!("Payment received for {invoice}"),
            lines: vec![line(PAYMENTS_CLEARING, amount, 0), line(AR, 0, amount)],
            due: None,
            cleared: false,
            attachment: false,
            paid: false,
        });
        total += amount;
    }
    assert_eq!(total, DEPOSIT_STORY_TOTAL);
}

/// The lender's 11/04 statement for the truck loan: the payment splits into
/// interest and principal, and the balance after is the books' balance on
/// 11/04 less the principal, in exact cents.
fn lender_statement(txns: &[Txn]) -> Value {
    // Credit-normal: what is owed.
    let owed: i64 = txns
        .iter()
        .filter(|t| t.date <= DEPOSIT_STORY_DATE)
        .flat_map(|t| t.lines.iter())
        .filter(|l| l.account == LOAN_ACCOUNT)
        .map(|l| l.credit - l.debit)
        .sum();
    let principal = LOAN_PAYMENT - LOAN_STATEMENT_INTEREST;
    assert!(principal > 0 && owed > principal, "loan statement");
    json!({
        "lender": LOAN_LENDER,
        "account": LOAN_ACCOUNT,
        "statementDate": DEPOSIT_STORY_DATE.text(),
        "balanceBefore": cents(owed),
        "payment": cents(LOAN_PAYMENT),
        "interest": cents(LOAN_STATEMENT_INTEREST),
        "principal": cents(principal),
        "balanceAfter": cents(owed - principal),
    })
}

/// The P15 templates (Q-P15r): a check to print each month (Remind me), a
/// 49.00 software charge on the card entered automatically from 09/15 for
/// three months (the first open catches up 09/15 and 10/15; the count
/// stops it), and an owner's insurance journal entry with no schedule.
fn templates() -> Value {
    json!({
        "groups": ["Monthly bills"],
        "templates": [
            {
                "name": "Shop rent",
                "group": "Monthly bills",
                "kind": "check",
                "account": "1010",
                "payee": "Pine Ridge Storage Units",
                "memo": "Shop rent",
                "printLater": true,
                "lines": [{"account": "6300", "debit": cents(145_000)}],
                "schedule": {"mode": "remind", "frequency": "monthly", "interval": 1, "nextDate": "2026-12-01", "remainingCount": null},
            },
            {
                "name": "Accounting software",
                "group": "Monthly bills",
                "kind": "card_charge",
                "account": "2050",
                "payee": "Mountain Software Co",
                "memo": "Accounting software",
                "printLater": false,
                "lines": [{"account": "6250", "debit": cents(4_900)}],
                "schedule": {"mode": "auto", "frequency": "monthly", "interval": 1, "nextDate": "2026-09-15", "remainingCount": 3},
            },
            {
                "name": "Owner health insurance",
                "group": null,
                "kind": "journal_entry",
                "account": null,
                "payee": null,
                "memo": "Owner health insurance",
                "printLater": false,
                "lines": [
                    {"account": "30170", "debit": cents(41_250)},
                    {"account": "1010", "credit": cents(41_250)},
                ],
                "schedule": null,
            },
        ],
    })
}

/// The "Plant sale counter" daily-sales layout (created from the engine's
/// retail preset, then its lines replaced). `fallback` is used when the
/// chart lacks `account` (4100 Product Sales is an inventory-only account).
fn daily_sales_layouts() -> Value {
    json!([{
        "name": "Plant sale counter",
        "preset": "retail",
        "lines": [
            {"section": "sales", "label": "Plant sales", "account": "4100", "fallbackAccount": "4000", "side": "credit"},
            {"section": "tax", "label": "Sales Tax Collected", "account": SALES_TAX_PAYABLE, "side": "credit"},
            {"section": "payment", "label": "Cash", "account": PAYMENTS_CLEARING, "side": "debit"},
            {"section": "payment", "label": "Cards", "account": PAYMENTS_CLEARING, "side": "debit"},
            {"section": "payout", "label": "Paid-outs", "account": "6250", "side": "debit"},
        ],
    }])
}

fn account_kind(number: &str) -> &'static str {
    ACCOUNTS
        .iter()
        .find(|a| a.0 == number)
        .map_or("other", |a| a.2)
}

fn push_income_plug(txns: &mut Vec<Txn>, date: Day, diff: i64, invoice_no: &mut u32, prefix: &str) {
    if diff == 0 {
        return;
    }
    let (lines, memo) = if diff > 0 {
        (
            vec![line(AR, diff, 0), line("4000", 0, diff)],
            "Fall cleanup contract",
        )
    } else {
        (
            vec![line("5200", -diff, 0), line(AR, 0, -diff)],
            "Credit for returned materials",
        )
    };
    txns.push(Txn {
        date,
        kind: "invoice",
        number: format!("{prefix}-{invoice_no}"),
        contact: Some("Riverbend HOA".into()),
        memo: memo.into(),
        lines,
        due: Some(date.add_days(30)),
        cleared: false,
        attachment: false,
        paid: false,
    });
    *invoice_no += 1;
}

fn push_equity_plug(txns: &mut Vec<Txn>, date: Day, diff: i64, cleared: bool, check_no: &mut u32) {
    if diff == 0 {
        return;
    }
    let (kind, number, memo, lines) = if diff > 0 {
        (
            "deposit",
            format!("DEP-OWN-{}", date.text()),
            "Owner contribution",
            vec![line("1010", diff, 0), line("30160", 0, diff)],
        )
    } else {
        *check_no += 1;
        (
            "check",
            format!("{check_no}"),
            "Owner draw",
            vec![line("30170", -diff, 0), line("1010", 0, -diff)],
        )
    };
    txns.push(Txn {
        date,
        kind,
        number,
        contact: Some("Kurt (owner)".into()),
        memo: memo.into(),
        lines,
        due: None,
        cleared,
        attachment: false,
        paid: false,
    });
}

fn render(fx: &Fixture) -> BTreeMap<&'static str, String> {
    let mut out = BTreeMap::new();
    let company = json!({
        "name": "Ozark Ridge Landscaping LLC",
        "legalName": "Ozark Ridge Landscaping LLC",
        "entityType": "single_member_llc",
        "chart": "general_service",
        "fiscalYearStartMonth": 1,
        "basis": "accrual",
        "storyDate": format!("{:04}-{:02}-{:02}", STORY_DATE.0, STORY_DATE.1, STORY_DATE.2),
        "closingDate": CLOSING_DATE,
        "accounts": ACCOUNTS.iter().map(|(n, name, t, d)| {
            let mut a = json!({"number": n, "name": name, "type": t, "detail": if d.is_empty() { Value::Null } else { json!(d) }});
            if let Some((_, parent)) = ACCOUNT_PARENTS.iter().find(|(c, _)| c == n) {
                a["parent"] = json!(parent);
            }
            a
        }).collect::<Vec<_>>(),
        "systemAccounts": {
            "accountsReceivable": AR,
            "accountsPayable": AP,
            "openingBalances": OPENING,
            "paymentsClearing": PAYMENTS_CLEARING,
            "salesTaxPayable": SALES_TAX_PAYABLE
        },
        "dailySalesLayouts": daily_sales_layouts(),
        "clients": CLIENT_NAMES,
        "vendors": VENDOR_NAMES,
        "vendorDetails": [{
            "name": STORY_VENDOR,
            "addressLine1": "1200 Commerce Dr",
            "city": "Aurora",
            "state": "MO",
            "zip": "65605",
            "defaultExpenseAccount": "5110"
        }],
        "tags": STORY_TAGS.iter().map(|(name, group)| json!({"name": name, "group": group})).collect::<Vec<_>>(),
        "jobs": [{"customer": STORY_CUSTOMER, "name": STORY_JOB_NAME}],
        "bank": {
            "lastFileDate": "2026-11-03",
            "lastFileBalance": cents(BANK_FILE_1010),
            "lastReconciliation": {"date": "2026-09-30", "balance": cents(LAST_RECONCILED_1010)},
            "octoberStatement": {"date": "2026-10-31", "endingBalance": cents(OCT_STATEMENT_1010)}
        },
        "figures": {
            "operatingCheckingEnding": cents(ENDING_1010),
            "openAr": cents(OPEN_AR),
            "billsDueThrough": "2026-11-11",
            "billsDueWithin7Days": cents(BILLS_DUE_7),
            "netIncomeOctober": cents(NI_OCT),
            "netIncomeJanToOct": cents(NI_JAN_OCT),
            "transactions": TXN_COUNT,
            "attachments": ATTACHMENT_COUNT,
            "nextCheckNumber1010": LAST_CHECK_1010 + 1,
            "bankFeedWaiting": {"ready": FEED_READY, "review": FEED_REVIEW, "match": FEED_MATCH}
        }
    });
    out.insert("company.json", pretty(&company));
    let txns: Vec<Value> = fx
        .txns
        .iter()
        .enumerate()
        .map(|(i, t)| {
            json!({
                "id": i + 1,
                "date": t.date.text(),
                "type": t.kind,
                "number": t.number,
                "contact": t.contact,
                "memo": t.memo,
                "due": t.due.map(Day::text),
                "cleared": t.cleared,
                "attachment": t.attachment,
                "lines": t.lines.iter().map(|l| {
                    let mut v = json!({"account": l.account, "debit": cents(l.debit), "credit": cents(l.credit)});
                    for (key, value) in [("memo", l.memo), ("tag", l.tag), ("job", l.job)] {
                        if let Some(value) = value {
                            v[key] = json!(value);
                        }
                    }
                    v
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    out.insert("transactions.json", pretty(&json!({"transactions": txns})));
    out.insert(
        "bank_feed.json",
        pretty(&json!({"account": "1010", "items": fx.feed})),
    );
    out.insert("lender_statement.json", pretty(&lender_statement(&fx.txns)));
    out.insert("templates.json", pretty(&templates()));
    out
}

fn pretty(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap_or_default();
    s.push('\n');
    s
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/sample/ozark_ridge")
}

/// Generates the fixture files; `--check` fails when they differ from
/// what is committed.
pub fn gen_sample(args: &[String]) -> Result<()> {
    let check = args.first().map(String::as_str) == Some("--check");
    let files = render(&generate());
    let dir = fixture_dir();
    if check {
        let mut stale = Vec::new();
        for (name, text) in &files {
            let path = dir.join(name);
            let old = std::fs::read_to_string(&path).unwrap_or_default();
            if old.replace("\r\n", "\n") != *text {
                stale.push(name.to_string());
            }
        }
        if !stale.is_empty() {
            bail!("gen-sample: stale fixtures {stale:?}; run `cargo xtask gen-sample`");
        }
        println!("gen-sample: fixtures are current");
        return Ok(());
    }
    std::fs::create_dir_all(&dir).context("fixture folder")?;
    let mut report = String::new();
    for (name, text) in &files {
        std::fs::write(dir.join(name), text).with_context(|| name.to_string())?;
        let _ = writeln!(report, "gen-sample: wrote {name} ({} bytes)", text.len());
    }
    print!("{report}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_is_deterministic_and_counts_match() {
        let a = render(&generate());
        let b = render(&generate());
        assert_eq!(a, b);
        let fx = generate();
        assert_eq!(fx.txns.len(), TXN_COUNT);
        assert_eq!(
            fx.txns.iter().filter(|t| t.attachment).count(),
            ATTACHMENT_COUNT
        );
        // 37 items through 11/03 (the design figures) plus the 11/04 deposit.
        assert_eq!(fx.feed.len(), 38);
        for t in &fx.txns {
            let d: i64 = t.lines.iter().map(|l| l.debit).sum();
            let c: i64 = t.lines.iter().map(|l| l.credit).sum();
            assert_eq!(d, c, "{} unbalanced", t.number);
            assert!(t.lines.iter().all(|l| l.debit >= 0 && l.credit >= 0));
        }
    }

    #[test]
    fn write_check_story_holds() {
        let fx = generate();
        let on_1010 = |t: &&Txn| t.lines.iter().any(|l| l.account == "1010");
        let numbers: Vec<u32> = fx
            .txns
            .iter()
            .filter(on_1010)
            .filter_map(|t| t.number.parse().ok())
            .collect();
        assert_eq!(numbers.iter().max(), Some(&LAST_CHECK_1010));
        assert!(!numbers.contains(&(LAST_CHECK_1010 + 1)));
        let unique: std::collections::BTreeSet<_> = numbers.iter().collect();
        assert_eq!(unique.len(), numbers.len(), "check numbers are unique");
        let top = fx
            .txns
            .iter()
            .find(|t| t.number == LAST_CHECK_1010.to_string())
            .expect("4417");
        assert_eq!(top.kind, "check");
        let story: Vec<&Txn> = fx
            .txns
            .iter()
            .filter(|t| t.contact.as_deref() == Some(STORY_VENDOR))
            .collect();
        assert_eq!(story.len(), 4, "three checks and the open bill only");
        for (n, date, plants, mulch) in STORY_CHECKS {
            let c = story
                .iter()
                .find(|t| t.number == n.to_string())
                .expect("story check");
            assert_eq!((c.kind, c.date), ("check", *date));
            assert_eq!(c.lines[0].debit + c.lines[1].debit, plants + mulch);
        }
        let bill = story.iter().find(|t| t.kind == "bill").expect("bill");
        assert_eq!(bill_amount(bill), 128_640);
        assert_eq!(bill.due, Some(Day(2026, 11, 5)));
    }

    #[test]
    fn deposit_and_loan_story_holds() {
        let fx = generate();
        let clearing: i64 = fx
            .txns
            .iter()
            .flat_map(|t| t.lines.iter())
            .filter(|l| l.account == PAYMENTS_CLEARING)
            .map(|l| l.debit - l.credit)
            .sum();
        assert_eq!(clearing, DEPOSIT_STORY_TOTAL);
        let last = fx.feed.last().expect("feed");
        assert_eq!(last["amount"], "2092.50");
        assert_eq!(last["date"], "2026-11-04");
        let s = lender_statement(&fx.txns);
        assert_eq!(s["payment"], "892.00");
        assert_eq!(s["interest"], "207.43");
        assert_eq!(s["principal"], "684.57");
        assert_eq!(s["balanceBefore"], "31200.00");
        assert_eq!(s["balanceAfter"], "30515.43");
    }

    #[test]
    fn committed_fixtures_are_current() {
        let files = render(&generate());
        for (name, text) in &files {
            let old = std::fs::read_to_string(fixture_dir().join(name)).unwrap_or_default();
            assert_eq!(
                old.replace("\r\n", "\n"),
                *text,
                "{name} is stale: run `cargo xtask gen-sample`"
            );
        }
    }
}
