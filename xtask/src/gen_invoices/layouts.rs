//! The 24 synthetic layouts as data (plan D6): 18 seen and 6 held out.
//! Vendors, addresses and phone numbers are invented; phone numbers use the
//! 555-01xx fiction range.
//!
//! Held-out layouts (`holdout: true`) and the held-out font (PT Serif) are
//! written only with `--holdout`, so nobody tunes the rules on them
//! (review S15).

use super::fonts::FontId;
use super::values::{DateFmt, MoneyFmt};

/// What kind of paper it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// A vendor invoice on letter paper.
    Invoice,
    /// A utility or telecom bill (account number, service period, amount due).
    Utility,
    /// A statement of account (several invoices, balance due).
    Statement,
    /// A thermal register receipt.
    Receipt,
    /// A card terminal slip.
    Slip,
}

impl Family {
    pub fn name(self) -> &'static str {
        match self {
            Family::Invoice => "invoice",
            Family::Utility => "utility",
            Family::Statement => "statement",
            Family::Receipt => "receipt",
            Family::Slip => "slip",
        }
    }
}

/// Where the vendor's name and address sit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Header {
    /// Vendor block top left, title top right.
    LeftTitleRight,
    /// Vendor block centred, title below it.
    Centered,
    /// Title top left, vendor block top right.
    TitleLeftVendorRight,
    /// A shaded band across the top with the vendor name, address under it.
    Band,
}

/// How the date, number, terms and the like are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Meta {
    /// "Label   value" pairs in a block on the right.
    Pairs,
    /// "Label: value" lines on the left under the vendor.
    Inline,
    /// A boxed row of labels with the values in the row below.
    Grid,
}

/// A column of the line table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Col {
    Qty,
    Code,
    Description,
    Rate,
    Amount,
}

/// Which list line descriptions come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Catalog {
    Electrical,
    Plumbing,
    Wholesale,
    AutoParts,
    Grocery,
    Hardware,
    Food,
    Handyman,
    Roofing,
    Software,
    Office,
    Freight,
    Accounting,
    Consulting,
    AutoRepair,
    Landscaping,
    Fuel,
    Print,
    Insurance,
    Electric,
    Water,
    Telecom,
}

/// Quantities as the vendor counts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QtyKind {
    /// Whole units.
    Units,
    /// Hours in quarters.
    Hours,
    /// Gallons to three places (fuel).
    Gallons,
    /// Always one.
    One,
}

/// How the document says it was paid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paid {
    /// Not paid (a bill).
    No,
    /// A card; the brand varies per document.
    Card,
    /// Card or cash, per document.
    CardOrCash,
    /// A deposit was paid; "Balance Due" follows the total.
    Deposit,
    /// "PAID" stamped, method not printed.
    Stamp,
}

/// The reference number's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefShape {
    /// `prefix` then `n` digits.
    Prefixed(&'static str, u32),
    /// `n` digits.
    Digits(u32),
    /// Letters-digits-digits like `A7-20931`.
    Mixed,
}

/// The vendor printed on every document of a layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vendor {
    /// Stable key into the roster.
    pub key: &'static str,
    pub name: &'static str,
    pub street: &'static str,
    pub city: &'static str,
    pub phone: Option<&'static str>,
    pub web: Option<&'static str>,
    /// A name in the roster that is easy to confuse with this one.
    pub confusable: &'static str,
}

/// One layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub id: &'static str,
    pub holdout: bool,
    pub family: Family,
    pub vendor: Vendor,
    pub font: FontId,
    pub bold: FontId,
    pub header: Header,
    pub title: Option<&'static str>,
    pub meta: Meta,
    /// `None`: the document has no reference number.
    pub ref_label: Option<&'static str>,
    pub ref_shape: RefShape,
    pub date_label: &'static str,
    pub due_label: Option<&'static str>,
    pub terms_label: Option<&'static str>,
    /// Terms in days to choose from (0 = "Due on receipt").
    pub terms: &'static [u16],
    /// The owner's account number with the vendor.
    pub account_label: Option<&'static str>,
    /// A second number that is not the reference (PO, order, customer id).
    pub decoy_label: Option<&'static str>,
    pub date_fmt: DateFmt,
    pub money: MoneyFmt,
    /// The total's own format (often with a dollar sign).
    pub total_money: MoneyFmt,
    pub bill_to: Option<&'static str>,
    pub ship_to: Option<&'static str>,
    pub columns: &'static [Col],
    pub col_labels: &'static [&'static str],
    pub lines: (i64, i64),
    pub catalog: Catalog,
    pub qty: QtyKind,
    pub subtotal_label: Option<&'static str>,
    pub tax_label: Option<&'static str>,
    /// Tax rate in basis points (0: no tax line).
    pub tax_bp: i64,
    pub fee: Option<&'static str>,
    pub discount: Option<&'static str>,
    pub total_label: &'static str,
    pub paid: Paid,
    pub tip: bool,
    pub footer: &'static [&'static str],
    /// Receipt paper width in points (thermal and slips).
    pub paper_width: f32,
}

const BASE: Layout = Layout {
    id: "",
    holdout: false,
    family: Family::Invoice,
    vendor: Vendor {
        key: "",
        name: "",
        street: "",
        city: "",
        phone: None,
        web: None,
        confusable: "",
    },
    font: FontId::PlexSans,
    bold: FontId::PlexSansBold,
    header: Header::LeftTitleRight,
    title: Some("INVOICE"),
    meta: Meta::Pairs,
    ref_label: Some("Invoice #"),
    ref_shape: RefShape::Digits(5),
    date_label: "Date",
    due_label: Some("Due Date"),
    terms_label: Some("Terms"),
    terms: &[30],
    account_label: None,
    decoy_label: None,
    date_fmt: DateFmt::Mdy,
    money: MoneyFmt::Plain,
    total_money: MoneyFmt::Dollar,
    bill_to: Some("Bill To"),
    ship_to: None,
    columns: &[Col::Description, Col::Qty, Col::Rate, Col::Amount],
    col_labels: &["Description", "Qty", "Rate", "Amount"],
    lines: (1, 6),
    catalog: Catalog::Electrical,
    qty: QtyKind::Units,
    subtotal_label: Some("Subtotal"),
    tax_label: None,
    tax_bp: 0,
    fee: None,
    discount: None,
    total_label: "Total",
    paid: Paid::No,
    tip: false,
    footer: &["Thank you for your business."],
    paper_width: 612.0,
};

const RECEIPT: Layout = Layout {
    family: Family::Receipt,
    font: FontId::PlexMono,
    bold: FontId::PlexMono,
    header: Header::Centered,
    title: None,
    meta: Meta::Inline,
    ref_label: Some("TRANS #"),
    due_label: None,
    terms_label: None,
    terms: &[],
    bill_to: None,
    columns: &[Col::Description, Col::Amount],
    col_labels: &[],
    lines: (2, 9),
    qty: QtyKind::Units,
    subtotal_label: Some("SUBTOTAL"),
    tax_label: Some("TAX"),
    tax_bp: 825,
    total_label: "TOTAL",
    paid: Paid::CardOrCash,
    money: MoneyFmt::Plain,
    total_money: MoneyFmt::Plain,
    date_fmt: DateFmt::Mdy,
    footer: &["THANK YOU FOR SHOPPING WITH US"],
    paper_width: 226.0,
    ..BASE
};

/// Every layout, seen first. `holdout` ones are only written with
/// `--holdout`.
#[allow(clippy::too_many_lines)]
pub fn all() -> Vec<Layout> {
    vec![
        // ---- seen ------------------------------------------------------
        Layout {
            id: "s01-service-classic",
            vendor: Vendor {
                key: "riverbend-electric",
                name: "Riverbend Electric Co.",
                street: "4180 Mill Creek Rd",
                city: "Rogers, AR 72756",
                phone: Some("(479) 555-0142"),
                web: Some("riverbendelectric.example"),
                confusable: "Riverbend Electrical Supply",
            },
            ref_shape: RefShape::Prefixed("INV-", 5),
            catalog: Catalog::Electrical,
            qty: QtyKind::Hours,
            col_labels: &["Description", "Hours", "Rate", "Amount"],
            tax_label: Some("Sales Tax"),
            tax_bp: 650,
            ..BASE
        },
        Layout {
            id: "s02-service-centered",
            header: Header::Centered,
            title: Some("Invoice"),
            meta: Meta::Inline,
            vendor: Vendor {
                key: "copperline-plumbing",
                name: "Copperline Plumbing & Heating",
                street: "77 Orchard Lane, Suite B",
                city: "Springdale, AR 72762",
                phone: Some("479-555-0118"),
                web: None,
                confusable: "Copperline Plumbing Supply",
            },
            ref_label: Some("Invoice Number:"),
            date_label: "Invoice Date:",
            due_label: Some("Payment Due:"),
            terms_label: None,
            terms: &[15],
            date_fmt: DateFmt::MonthD,
            catalog: Catalog::Plumbing,
            total_label: "Amount Due",
            money: MoneyFmt::Dollar,
            ..BASE
        },
        Layout {
            id: "s03-wholesale-grid",
            meta: Meta::Grid,
            vendor: Vendor {
                key: "harbor-point-supply",
                name: "Harbor Point Supply Inc.",
                street: "2200 Industrial Pkwy",
                city: "Fort Smith, AR 72901",
                phone: Some("(479) 555-0177"),
                web: Some("harborpointsupply.example"),
                confusable: "Harbor Point Marine",
            },
            ref_label: Some("Invoice No."),
            ref_shape: RefShape::Digits(6),
            decoy_label: Some("P.O. Number"),
            terms: &[30, 45],
            ship_to: Some("Ship To"),
            columns: &[
                Col::Code,
                Col::Description,
                Col::Qty,
                Col::Rate,
                Col::Amount,
            ],
            col_labels: &["Item", "Description", "Qty", "Unit Price", "Extended"],
            lines: (2, 8),
            catalog: Catalog::Wholesale,
            fee: Some("Freight"),
            tax_label: Some("Tax"),
            tax_bp: 975,
            ..BASE
        },
        Layout {
            id: "s04-parts-counter",
            font: FontId::PlexMono,
            bold: FontId::PlexSansBold,
            header: Header::TitleLeftVendorRight,
            title: Some("PARTS INVOICE"),
            vendor: Vendor {
                key: "tri-county-auto-parts",
                name: "Tri-County Auto Parts",
                street: "915 W Walnut St",
                city: "Bentonville, AR 72712",
                phone: Some("479.555.0163"),
                web: None,
                confusable: "Tri-County Auto Body",
            },
            ref_label: Some("Invoice"),
            ref_shape: RefShape::Mixed,
            decoy_label: Some("Customer #"),
            date_fmt: DateFmt::MdyShort,
            terms_label: Some("Terms"),
            terms: &[0, 10],
            due_label: None,
            columns: &[
                Col::Qty,
                Col::Code,
                Col::Description,
                Col::Rate,
                Col::Amount,
            ],
            col_labels: &["Qty", "Part #", "Description", "Price", "Total"],
            catalog: Catalog::AutoParts,
            tax_label: Some("Sales Tax"),
            tax_bp: 975,
            total_label: "Grand Total",
            money: MoneyFmt::Bare,
            total_money: MoneyFmt::Bare,
            ..BASE
        },
        Layout {
            id: "s05-utility-electric",
            family: Family::Utility,
            title: Some("Electric Service Statement"),
            vendor: Vendor {
                key: "ozark-valley-electric",
                name: "Ozark Valley Electric Cooperative",
                street: "PO Box 1180",
                city: "Huntsville, AR 72740",
                phone: Some("1-800-555-0199"),
                web: Some("ozarkvalley.example"),
                confusable: "Ozark Valley Propane",
            },
            ref_label: Some("Bill Number"),
            ref_shape: RefShape::Digits(8),
            account_label: Some("Account Number"),
            date_label: "Statement Date",
            due_label: Some("Due Date"),
            terms_label: None,
            terms: &[21],
            catalog: Catalog::Electric,
            total_label: "Total Amount Due",
            subtotal_label: Some("Current Charges"),
            ..BASE
        },
        Layout {
            id: "s06-utility-water",
            family: Family::Utility,
            header: Header::Band,
            title: Some("Utility Bill"),
            meta: Meta::Grid,
            vendor: Vendor {
                key: "millbrook-water",
                name: "City of Millbrook Water Dept.",
                street: "300 E Main St",
                city: "Millbrook, AR 72633",
                phone: Some("(870) 555-0104"),
                web: None,
                confusable: "City of Millbrook Sanitation",
            },
            ref_label: None,
            account_label: Some("Account No."),
            date_label: "Bill Date",
            due_label: Some("Amount Due By"),
            terms_label: None,
            terms: &[20],
            date_fmt: DateFmt::MdyNoPad,
            catalog: Catalog::Water,
            total_label: "Amount Due",
            subtotal_label: Some("Current Charges"),
            ..BASE
        },
        Layout {
            id: "s07-telecom",
            family: Family::Utility,
            header: Header::TitleLeftVendorRight,
            title: Some("Your Monthly Bill"),
            meta: Meta::Inline,
            vendor: Vendor {
                key: "summit-ridge-telecom",
                name: "Summit Ridge Telecom",
                street: "8 Commerce Plaza",
                city: "Little Rock, AR 72201",
                phone: Some("888-555-0131"),
                web: Some("summitridge.example"),
                confusable: "Summit Ridge Wireless",
            },
            ref_label: Some("Invoice No:"),
            ref_shape: RefShape::Prefixed("SR", 7),
            account_label: Some("Account #:"),
            date_label: "Billing Date:",
            due_label: Some("Due By:"),
            terms_label: None,
            terms: &[25],
            date_fmt: DateFmt::MonD,
            catalog: Catalog::Telecom,
            total_label: "Total Due",
            subtotal_label: Some("New Charges"),
            ..BASE
        },
        Layout {
            id: "s08-statement",
            family: Family::Statement,
            title: Some("STATEMENT"),
            vendor: Vendor {
                key: "prairie-lumber",
                name: "Prairie Lumber & Hardware",
                street: "1601 S 8th St",
                city: "Rogers, AR 72756",
                phone: Some("(479) 555-0185"),
                web: None,
                confusable: "Prairie Lumber Co-op",
            },
            ref_label: None,
            account_label: Some("Account"),
            date_label: "Statement Date",
            due_label: None,
            terms_label: None,
            terms: &[],
            lines: (3, 26),
            catalog: Catalog::Hardware,
            total_label: "Balance Due",
            subtotal_label: None,
            ..BASE
        },
        Layout {
            id: "s09-grocery-thermal",
            vendor: Vendor {
                key: "freshway-market",
                name: "FreshWay Market #0412",
                street: "2810 W Pleasant Grove Rd",
                city: "Rogers AR 72758",
                phone: Some("479-555-0150"),
                web: None,
                confusable: "FreshWay Pharmacy",
            },
            ref_shape: RefShape::Digits(4),
            catalog: Catalog::Grocery,
            ..RECEIPT
        },
        Layout {
            id: "s10-hardware-thermal",
            font: FontId::CourierPrime,
            bold: FontId::CourierPrimeBold,
            vendor: Vendor {
                key: "ironwood-hardware",
                name: "IRONWOOD HARDWARE",
                street: "455 N Thompson St",
                city: "Springdale, AR 72764",
                phone: Some("(479) 555-0127"),
                web: None,
                confusable: "Ironwood Lumber",
            },
            ref_label: Some("RECEIPT#"),
            ref_shape: RefShape::Prefixed("R", 6),
            date_fmt: DateFmt::MdyShort,
            columns: &[Col::Description, Col::Qty, Col::Amount],
            catalog: Catalog::Hardware,
            tax_bp: 950,
            paper_width: 204.0,
            ..RECEIPT
        },
        Layout {
            id: "s11-card-slip",
            family: Family::Slip,
            font: FontId::CourierPrime,
            bold: FontId::CourierPrimeBold,
            vendor: Vendor {
                key: "bluebird-cafe",
                name: "BLUEBIRD CAFE",
                street: "120 W Emma Ave",
                city: "Springdale AR",
                phone: None,
                web: None,
                confusable: "Bluebird Bakery",
            },
            ref_label: Some("INVOICE"),
            ref_shape: RefShape::Digits(6),
            date_fmt: DateFmt::MonDUpper,
            subtotal_label: Some("AMOUNT"),
            tax_label: None,
            tax_bp: 0,
            tip: true,
            paid: Paid::Card,
            footer: &["CUSTOMER COPY"],
            paper_width: 180.0,
            ..RECEIPT
        },
        Layout {
            id: "s12-restaurant",
            vendor: Vendor {
                key: "stone-creek-grill",
                name: "Stone Creek Grill",
                street: "88 Razorback Rd",
                city: "Fayetteville, AR 72701",
                phone: Some("479.555.0110"),
                web: None,
                confusable: "Stone Creek Catering",
            },
            ref_label: Some("Check #"),
            ref_shape: RefShape::Digits(4),
            catalog: Catalog::Food,
            subtotal_label: Some("Subtotal"),
            tax_label: Some("Tax"),
            tax_bp: 1125,
            total_label: "Total",
            tip: true,
            paid: Paid::Card,
            footer: &["Thanks for dining with us!"],
            ..RECEIPT
        },
        Layout {
            id: "s13-hand-style",
            font: FontId::Kalam,
            bold: FontId::Kalam,
            header: Header::LeftTitleRight,
            title: Some("Invoice"),
            meta: Meta::Inline,
            vendor: Vendor {
                key: "hals-handyman",
                name: "Hal's Handyman Service",
                street: "19 Pinecrest Dr",
                city: "Lowell, AR 72745",
                phone: Some("479-555-0191"),
                web: None,
                confusable: "Hal's Hauling",
            },
            ref_label: Some("No."),
            ref_shape: RefShape::Digits(3),
            date_label: "Date:",
            due_label: None,
            terms_label: None,
            terms: &[0],
            date_fmt: DateFmt::MdyNoPad,
            catalog: Catalog::Handyman,
            qty: QtyKind::Hours,
            col_labels: &["Work done", "Hrs", "Rate", "Amount"],
            lines: (1, 4),
            total_label: "Total",
            subtotal_label: None,
            money: MoneyFmt::Dollar,
            footer: &["Pay by check to Hal Brewer. Thanks!"],
            ..BASE
        },
        Layout {
            id: "s14-contractor-deposit",
            header: Header::Band,
            vendor: Vendor {
                key: "northgate-roofing",
                name: "Northgate Roofing LLC",
                street: "5021 Highway 412",
                city: "Siloam Springs, AR 72761",
                phone: Some("(479) 555-0158"),
                web: Some("northgateroofing.example"),
                confusable: "Northgate Gutters",
            },
            ref_label: Some("Invoice #"),
            ref_shape: RefShape::Prefixed("NR-", 4),
            decoy_label: Some("Job #"),
            date_fmt: DateFmt::MonD,
            terms: &[0, 15],
            catalog: Catalog::Roofing,
            lines: (2, 5),
            paid: Paid::Deposit,
            total_label: "Total",
            ..BASE
        },
        Layout {
            id: "s15-saas-card",
            header: Header::TitleLeftVendorRight,
            title: Some("Receipt"),
            meta: Meta::Pairs,
            vendor: Vendor {
                key: "cloudnest-software",
                name: "Cloudnest Software, Inc.",
                street: "500 Market St, Floor 4",
                city: "Austin, TX 78701",
                phone: None,
                web: Some("cloudnest.example"),
                confusable: "Cloudnest Hosting",
            },
            ref_label: Some("Invoice ID"),
            ref_shape: RefShape::Prefixed("CN-2026-", 5),
            date_label: "Date paid",
            due_label: None,
            terms_label: None,
            terms: &[],
            date_fmt: DateFmt::MonthD,
            bill_to: Some("Billed to"),
            catalog: Catalog::Software,
            qty: QtyKind::One,
            lines: (1, 3),
            total_label: "Amount paid",
            paid: Paid::Card,
            money: MoneyFmt::Dollar,
            ..BASE
        },
        Layout {
            id: "s16-office-discount",
            meta: Meta::Grid,
            vendor: Vendor {
                key: "paperwise-office",
                name: "Paperwise Office Supply",
                street: "3300 N College Ave",
                city: "Fayetteville, AR 72703",
                phone: Some("(479) 555-0136"),
                web: None,
                confusable: "Paperwise Printing",
            },
            ref_label: Some("Invoice"),
            ref_shape: RefShape::Digits(7),
            decoy_label: Some("Order No."),
            date_fmt: DateFmt::MdyDash,
            catalog: Catalog::Office,
            columns: &[Col::Qty, Col::Description, Col::Rate, Col::Amount],
            col_labels: &["Qty", "Description", "Unit Price", "Amount"],
            lines: (2, 7),
            discount: Some("Discount"),
            tax_label: Some("Sales Tax"),
            tax_bp: 975,
            total_label: "Invoice Total",
            ..BASE
        },
        Layout {
            id: "s17-freight",
            font: FontId::PlexMono,
            bold: FontId::PlexMono,
            title: Some("FREIGHT BILL"),
            vendor: Vendor {
                key: "redline-freight",
                name: "Redline Freight Lines",
                street: "1 Terminal Way",
                city: "Van Buren, AR 72956",
                phone: Some("800-555-0172"),
                web: None,
                confusable: "Redline Courier",
            },
            ref_label: Some("Freight Bill No"),
            ref_shape: RefShape::Digits(9),
            decoy_label: Some("PRO Number"),
            date_label: "Ship Date",
            date_fmt: DateFmt::Iso,
            terms: &[30],
            catalog: Catalog::Freight,
            lines: (1, 3),
            fee: Some("Fuel Surcharge"),
            total_label: "TOTAL CHARGES",
            money: MoneyFmt::Plain,
            total_money: MoneyFmt::Usd,
            ..BASE
        },
        Layout {
            id: "s18-professional",
            header: Header::Centered,
            title: Some("INVOICE"),
            vendor: Vendor {
                key: "whitfield-moore",
                name: "Whitfield & Moore CPAs, PLLC",
                street: "210 N Second St",
                city: "Rogers, AR 72756",
                phone: Some("(479) 555-0123"),
                web: None,
                confusable: "Whitfield Legal Group",
            },
            ref_label: Some("Invoice No."),
            ref_shape: RefShape::Digits(5),
            decoy_label: Some("Client ID"),
            date_label: "Invoice Date",
            terms: &[15, 30],
            catalog: Catalog::Accounting,
            qty: QtyKind::Hours,
            col_labels: &["Services", "Hours", "Rate", "Amount"],
            lines: (1, 5),
            total_label: "Total Due",
            ..BASE
        },
        // ---- held out (only with --holdout) -----------------------------
        Layout {
            id: "h01-serif-letterhead",
            holdout: true,
            font: FontId::PtSerif,
            bold: FontId::PtSerifBold,
            header: Header::Centered,
            title: Some("Statement of Services"),
            meta: Meta::Pairs,
            vendor: Vendor {
                key: "larkspur-consulting",
                name: "Larkspur Consulting Group",
                street: "14 Courthouse Square",
                city: "Bentonville, AR 72712",
                phone: Some("479-555-0166"),
                web: Some("larkspur.example"),
                confusable: "Larkspur Staffing",
            },
            ref_label: Some("Reference"),
            ref_shape: RefShape::Prefixed("LCG-", 4),
            date_label: "Date of Invoice",
            date_fmt: DateFmt::MonthD,
            terms: &[30],
            catalog: Catalog::Consulting,
            qty: QtyKind::Hours,
            col_labels: &["Engagement", "Hours", "Rate", "Fee"],
            total_label: "Total Fees",
            ..BASE
        },
        Layout {
            id: "h02-auto-repair",
            holdout: true,
            header: Header::Band,
            title: Some("REPAIR ORDER"),
            meta: Meta::Grid,
            vendor: Vendor {
                key: "precision-auto-care",
                name: "Precision Auto Care",
                street: "2701 W Sunset Ave",
                city: "Springdale, AR 72762",
                phone: Some("(479) 555-0149"),
                web: None,
                confusable: "Precision Auto Glass",
            },
            ref_label: Some("RO #"),
            ref_shape: RefShape::Digits(6),
            decoy_label: Some("Tag"),
            date_fmt: DateFmt::MdyShort,
            terms: &[0],
            due_label: None,
            columns: &[
                Col::Code,
                Col::Description,
                Col::Qty,
                Col::Rate,
                Col::Amount,
            ],
            col_labels: &["Op", "Description", "Qty/Hrs", "Price", "Ext"],
            catalog: Catalog::AutoRepair,
            tax_label: Some("Tax on Parts"),
            tax_bp: 975,
            fee: Some("Shop Supplies"),
            total_label: "Total Due",
            paid: Paid::Stamp,
            ..BASE
        },
        Layout {
            id: "h03-landscape-serif",
            holdout: true,
            font: FontId::PtSerif,
            bold: FontId::PtSerifBold,
            header: Header::LeftTitleRight,
            title: Some("BILL"),
            meta: Meta::Inline,
            vendor: Vendor {
                key: "greenleaf-lawn",
                name: "Greenleaf Lawn & Landscape",
                street: "PO Box 552",
                city: "Cave Springs, AR 72718",
                phone: Some("479 555 0114"),
                web: None,
                confusable: "Greenleaf Nursery",
            },
            ref_label: Some("Bill #"),
            ref_shape: RefShape::Digits(4),
            date_label: "Date",
            due_label: Some("Please pay by"),
            terms_label: None,
            terms: &[10],
            date_fmt: DateFmt::DMonY,
            catalog: Catalog::Landscaping,
            lines: (2, 6),
            total_label: "Please Pay",
            money: MoneyFmt::Dollar,
            ..BASE
        },
        Layout {
            id: "h04-fuel-thermal",
            holdout: true,
            vendor: Vendor {
                key: "quickstop-fuel",
                name: "QUICKSTOP FUEL #22",
                street: "6100 S Old Missouri Rd",
                city: "Springdale AR 72764",
                phone: None,
                web: None,
                confusable: "QuickStop Car Wash",
            },
            ref_label: Some("SEQ#"),
            ref_shape: RefShape::Digits(5),
            catalog: Catalog::Fuel,
            qty: QtyKind::Gallons,
            columns: &[Col::Description, Col::Qty, Col::Rate, Col::Amount],
            lines: (1, 2),
            tax_label: None,
            tax_bp: 0,
            subtotal_label: None,
            total_label: "TOTAL SALE",
            paid: Paid::Card,
            ..RECEIPT
        },
        Layout {
            id: "h05-print-shop",
            holdout: true,
            font: FontId::PtSerif,
            bold: FontId::PtSerifBold,
            meta: Meta::Grid,
            header: Header::TitleLeftVendorRight,
            title: Some("Invoice"),
            vendor: Vendor {
                key: "inkwell-print",
                name: "Inkwell Print Shop",
                street: "17 E Center St",
                city: "Fayetteville, AR 72701",
                phone: Some("(479) 555-0107"),
                web: Some("inkwellprint.example"),
                confusable: "Inkwell Signs & Graphics",
            },
            ref_label: Some("Invoice #"),
            ref_shape: RefShape::Digits(5),
            decoy_label: Some("Order #"),
            date_fmt: DateFmt::MonD,
            terms: &[0, 30],
            catalog: Catalog::Print,
            columns: &[Col::Qty, Col::Description, Col::Rate, Col::Amount],
            col_labels: &["Qty", "Item", "Each", "Total"],
            tax_label: Some("Tax"),
            tax_bp: 975,
            total_label: "Balance Due",
            ..BASE
        },
        Layout {
            id: "h06-insurance-notice",
            holdout: true,
            family: Family::Utility,
            title: Some("Premium Notice"),
            meta: Meta::Pairs,
            vendor: Vendor {
                key: "heartland-mutual",
                name: "Heartland Mutual Insurance",
                street: "PO Box 8800",
                city: "Des Moines, IA 50306",
                phone: Some("800-555-0188"),
                web: Some("heartlandmutual.example"),
                confusable: "Heartland Bank",
            },
            ref_label: Some("Notice No."),
            ref_shape: RefShape::Digits(8),
            account_label: Some("Policy Number"),
            date_label: "Notice Date",
            due_label: Some("Payment Due"),
            terms_label: None,
            terms: &[30],
            date_fmt: DateFmt::MonthD,
            catalog: Catalog::Insurance,
            total_label: "Minimum Due",
            subtotal_label: Some("Premium"),
            money: MoneyFmt::DollarSpace,
            total_money: MoneyFmt::DollarSpace,
            ..BASE
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twenty_four_layouts_six_held_out_with_their_own_font() {
        let all = all();
        assert_eq!(all.len(), 24);
        assert_eq!(all.iter().filter(|l| l.holdout).count(), 6);
        let mut ids: Vec<&str> = all.iter().map(|l| l.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 24);
        for l in all.iter().filter(|l| !l.holdout) {
            assert!(
                !matches!(l.font, FontId::PtSerif | FontId::PtSerifBold)
                    && !matches!(l.bold, FontId::PtSerif | FontId::PtSerifBold),
                "{} uses the held-out font",
                l.id
            );
        }
        for l in &all {
            assert_eq!(
                l.columns.len(),
                if l.col_labels.is_empty() {
                    l.columns.len()
                } else {
                    l.col_labels.len()
                },
                "{}",
                l.id
            );
            assert!(!l.vendor.confusable.is_empty() && !l.vendor.key.is_empty());
        }
    }
}
