//! The list reports (P28 WP3): who owes what, sales and spending, banking
//! and 1099, read through `?exact=1` (U-14, U-121). Each route is a
//! declarative [`Layout`]: the array its rows are in, one [`Col`] per
//! column, and where the engine's totals are. Cells copy the engine's
//! exact strings; the total line is the engine's (`totalsExact`,
//! `totalBalanceExact`, ...), never a sum of rows (plan §6).
//!
//! An aging detail opened from an aging cell keeps only that contact's
//! rows and that bucket's (the engine's own bucket, never recomputed); a
//! narrowed list has no total line, since no engine figure covers it.

use std::collections::HashSet;

use cl_core::ids::{ContactId, TransactionId};
use cl_core::report::{
    AgingBucket, Cell, ColumnHead, ColumnKind, Drill, ReportMeta, ReportRequest, ReportTable,
    RowKind, RowPath, Source,
};
use serde_json::Value;

use super::exact::{decimal, meta_basis, require_exact, shape, text};
use super::ledger::type_label;
use super::{col, row};
use crate::errors::{ApiError, LocalValidation};

/// How a cell is read.
#[derive(Clone, Copy, Debug)]
pub(crate) enum F {
    /// Owner text (blank: empty).
    Text(&'static str),
    /// A contact's or item's name; the `@` label when the id field is
    /// `null` (the engine's own "(no client)" words are not shown).
    Name {
        /// The name field.
        name: &'static str,
        /// The id field (`null`: no contact or item).
        id: &'static str,
        /// The `@` label for no contact or item.
        none: &'static str,
    },
    /// `number · name`.
    Account(&'static str, &'static str),
    /// A `YYYY-MM-DD` date (`null`: empty).
    Date(&'static str),
    /// A required exact amount string.
    Amount(&'static str),
    /// A count (a JSON number or numeric text).
    Count(&'static str),
    /// A transaction type, as the `@reports.txn.*` label.
    TxnType(&'static str),
    /// An engine code shown through `@<prefix>.<code>`.
    Code(&'static str, &'static str),
    /// A taxpayer id: the last four digits only (Q-P28p).
    Last4(&'static str),
    /// The engine's "meets the year's 1099 threshold" flag: `false` says
    /// the vendor is below it (PR6-QA-4); `true` or no flag is blank.
    BelowThreshold(&'static str),
}

/// The words for a vendor below the 1099 filing threshold, without the
/// amount (the 1099 Preparation's engine flag doesn't say which).
pub const BELOW_THRESHOLD: &str = "@reports.1099.below_threshold";

/// The words for a vendor below a known threshold: `@key $2,000`.
fn below_threshold_of(threshold: rust_decimal::Decimal) -> String {
    let style = cl_core::MoneyStyle {
        symbol: true,
        ..cl_core::MoneyStyle::default()
    };
    let shown = cl_core::report::display_amount(threshold, style);
    // Whole dollars read "$2,000", not "$2,000.00".
    let shown = if threshold.fract().is_zero() {
        shown.trim_end_matches(".00").to_string()
    } else {
        shown
    };
    format!("@reports.1099.below_threshold_of {shown}")
}

/// One column.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Col {
    key: &'static str,
    caption: &'static str,
    kind: ColumnKind,
    f: F,
    /// JSON pointer of this column's engine total.
    total: Option<&'static str>,
}

const fn c(key: &'static str, caption: &'static str, kind: ColumnKind, f: F) -> Col {
    Col {
        key,
        caption,
        kind,
        f,
        total: None,
    }
}

const fn amt(key: &'static str, caption: &'static str, field: &'static str) -> Col {
    c(key, caption, ColumnKind::Amount, F::Amount(field))
}

const fn tot(mut col: Col, pointer: &'static str) -> Col {
    col.total = Some(pointer);
    col
}

const fn date(key: &'static str, field: &'static str) -> Col {
    c(key, "@reports.col.date", ColumnKind::Date, F::Date(field))
}

const fn due(field: &'static str) -> Col {
    c("due", "@reports.col.due", ColumnKind::Date, F::Date(field))
}

const fn number(field: &'static str) -> Col {
    c(
        "number",
        "@reports.col.number",
        ColumnKind::Text,
        F::Text(field),
    )
}

const fn txn_type(field: &'static str) -> Col {
    c(
        "type",
        "@reports.col.type",
        ColumnKind::Text,
        F::TxnType(field),
    )
}

const fn days(field: &'static str) -> Col {
    c(
        "days",
        "@reports.col.days_overdue",
        ColumnKind::Count,
        F::Count(field),
    )
}

/// What a row's amount opens.
#[derive(Clone, Copy, Debug)]
pub(crate) enum RowDrill {
    /// Nothing.
    None,
    /// The transaction's form: its id field and its type field.
    Txn(&'static str, &'static str),
    /// The transaction's form, of a fixed type.
    TxnOf(&'static str, &'static str),
}

/// Narrowing an aging detail (an aging cell's drill).
#[derive(Clone, Copy, Debug)]
struct Narrow {
    /// The contact id field.
    contact: &'static str,
    /// The bucket field and the engine's codes, in [`AgingBucket`] order
    /// (current, 1-30, 31-60, 61-90, over 90).
    bucket: &'static str,
    codes: [&'static str; 5],
}

/// One list route.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Layout {
    /// The array of rows.
    rows: &'static str,
    /// The first (caption) column's reader; `None`: no caption.
    caption: Option<F>,
    /// The other columns.
    cols: &'static [Col],
    drill: RowDrill,
    narrow: Option<Narrow>,
}

const NO_CLIENT: &str = "@reports.row.no_client";
const NO_VENDOR: &str = "@reports.row.no_vendor";
const NO_ITEM: &str = "@reports.row.no_item";

const AR_AGING_DETAIL: Layout = Layout {
    rows: "details",
    caption: Some(F::Name {
        name: "customer_name",
        id: "contact_id",
        none: NO_CLIENT,
    }),
    cols: &[
        date("date", "txn_date"),
        txn_type("txn_type"),
        number("txn_number"),
        due("due_date"),
        days("daysOverdue"),
        c(
            "bucket",
            "@reports.col.aging",
            ColumnKind::Text,
            F::Code("reports.bucket", "bucket"),
        ),
        tot(amt("open", "@reports.col.open", "openExact"), "/totalExact"),
    ],
    drill: RowDrill::Txn("id", "txn_type"),
    narrow: Some(Narrow {
        contact: "contact_id",
        bucket: "bucket",
        codes: ["current", "1-30", "31-60", "61-90", "90+"],
    }),
};

const AP_AGING_DETAIL: Layout = Layout {
    rows: "details",
    caption: Some(F::Name {
        name: "vendor_name",
        id: "contact_id",
        none: NO_VENDOR,
    }),
    cols: &[
        date("date", "txn_date"),
        txn_type("txn_type"),
        number("txn_number"),
        c(
            "vendor_number",
            "@reports.col.vendor_number",
            ColumnKind::Text,
            F::Text("vendor_invoice_number"),
        ),
        due("due_date"),
        days("days_overdue"),
        c(
            "bucket",
            "@reports.col.aging",
            ColumnKind::Text,
            F::Code("reports.bucket", "bucket"),
        ),
        amt("amount", "@reports.col.amount", "totalExact"),
        tot(
            amt("open", "@reports.col.open", "balanceExact"),
            "/totalsExact/total",
        ),
    ],
    drill: RowDrill::Txn("bill_id", "txn_type"),
    narrow: Some(Narrow {
        contact: "contact_id",
        bucket: "bucket",
        codes: ["current", "1_30", "31_60", "61_90", "over_90"],
    }),
};

const INVOICES: Layout = Layout {
    rows: "data",
    caption: Some(F::Name {
        name: "customer_name",
        id: "contact_id",
        none: NO_CLIENT,
    }),
    cols: &[
        date("date", "txn_date"),
        number("txn_number"),
        due("due_date"),
        c(
            "status",
            "@reports.col.status",
            ColumnKind::Text,
            F::Code("reports.status", "invoice_status"),
        ),
        tot(
            amt("amount", "@reports.col.amount", "totalExact"),
            "/totalsExact/total",
        ),
        tot(
            amt("paid", "@reports.col.paid", "amount_paidExact"),
            "/totalsExact/amount_paid",
        ),
        tot(
            amt("open", "@reports.col.open", "balance_dueExact"),
            "/totalsExact/balance_due",
        ),
    ],
    drill: RowDrill::TxnOf("id", "invoice"),
    narrow: None,
};

const OPEN_INVOICES: Layout = Layout {
    cols: &[
        date("date", "txn_date"),
        number("txn_number"),
        due("due_date"),
        days("days_overdue"),
        tot(
            amt("amount", "@reports.col.amount", "totalExact"),
            "/totalsExact/total",
        ),
        tot(
            amt("paid", "@reports.col.paid", "amount_paidExact"),
            "/totalsExact/amount_paid",
        ),
        tot(
            amt("open", "@reports.col.open", "balance_dueExact"),
            "/totalsExact/balance_due",
        ),
    ],
    ..INVOICES
};

const UNPAID_BILLS: Layout = Layout {
    rows: "data",
    caption: Some(F::Name {
        name: "vendor_name",
        id: "contact_id",
        none: NO_VENDOR,
    }),
    cols: &[
        date("date", "txn_date"),
        number("txn_number"),
        c(
            "vendor_number",
            "@reports.col.vendor_number",
            ColumnKind::Text,
            F::Text("vendor_invoice_number"),
        ),
        due("due_date"),
        days("days_overdue"),
        tot(
            amt("amount", "@reports.col.amount", "totalExact"),
            "/totalsExact/total",
        ),
        tot(
            amt("paid", "@reports.col.paid", "amount_paidExact"),
            "/totalsExact/amount_paid",
        ),
        tot(
            amt("discount", "@reports.col.discount", "discount_amountExact"),
            "/totalsExact/discount_amount",
        ),
        tot(
            amt("credits", "@reports.col.credits", "credits_appliedExact"),
            "/totalsExact/credits_applied",
        ),
        tot(
            amt("open", "@reports.col.open", "balance_dueExact"),
            "/totalsExact/balance_due",
        ),
    ],
    drill: RowDrill::TxnOf("id", "bill"),
    narrow: None,
};

const CUSTOMER_BALANCES: Layout = Layout {
    rows: "data",
    caption: Some(F::Name {
        name: "display_name",
        id: "id",
        none: NO_CLIENT,
    }),
    cols: &[tot(
        amt("balance", "@reports.col.balance", "balanceExact"),
        "/totalsExact/balance",
    )],
    drill: RowDrill::None,
    narrow: None,
};

const VENDOR_BALANCES: Layout = Layout {
    caption: Some(F::Name {
        name: "display_name",
        id: "id",
        none: NO_VENDOR,
    }),
    ..CUSTOMER_BALANCES
};

const BILL_PAYMENTS: Layout = Layout {
    rows: "data",
    caption: Some(F::Text("vendor_name")),
    cols: &[
        date("date", "txn_date"),
        number("txn_number"),
        c(
            "check_number",
            "@reports.col.check_number",
            ColumnKind::Text,
            F::Text("check_number"),
        ),
        c(
            "bills",
            "@reports.col.bills_paid",
            ColumnKind::Count,
            F::Count("bill_count"),
        ),
        tot(
            amt("amount", "@reports.col.amount", "totalExact"),
            "/totalsExact/total",
        ),
    ],
    drill: RowDrill::TxnOf("id", "bill_payment"),
    narrow: None,
};

const BY_VENDOR: Layout = Layout {
    rows: "data",
    caption: None,
    cols: &[
        date("date", "txn_date"),
        txn_type("txn_type"),
        number("txn_number"),
        c(
            "memo",
            "@reports.col.memo",
            ColumnKind::Text,
            F::Text("memo"),
        ),
        tot(
            amt("amount", "@reports.col.amount", "totalExact"),
            "/totalsExact/total",
        ),
    ],
    drill: RowDrill::Txn("id", "txn_type"),
    narrow: None,
};

const SALES_BY_CLIENT: Layout = Layout {
    rows: "data",
    caption: Some(F::Name {
        name: "customer_name",
        id: "contact_id",
        none: NO_CLIENT,
    }),
    cols: &[tot(
        amt("total", "@reports.col.total", "totalExact"),
        "/grandTotalExact",
    )],
    drill: RowDrill::None,
    narrow: None,
};

const SALES_BY_ITEM: Layout = Layout {
    rows: "data",
    caption: Some(F::Name {
        name: "item_name",
        id: "item_id",
        none: NO_ITEM,
    }),
    cols: &[
        c(
            "sku",
            "@reports.col.sku",
            ColumnKind::Text,
            F::Text("item_sku"),
        ),
        c(
            "count",
            "@reports.col.txn_count",
            ColumnKind::Count,
            F::Count("txn_count"),
        ),
        tot(
            amt("total", "@reports.col.total", "totalExact"),
            "/grandTotalExact",
        ),
    ],
    drill: RowDrill::None,
    narrow: None,
};

const EXPENSES_BY_VENDOR: Layout = Layout {
    rows: "data",
    caption: Some(F::Name {
        name: "vendor_name",
        id: "contact_id",
        none: NO_VENDOR,
    }),
    cols: &[tot(
        amt("total", "@reports.col.total", "totalExact"),
        "/totalsExact/total",
    )],
    drill: RowDrill::None,
    narrow: None,
};

const DEPOSITS: Layout = Layout {
    rows: "data",
    caption: None,
    cols: &[
        date("date", "txn_date"),
        number("txn_number"),
        c(
            "memo",
            "@reports.col.memo",
            ColumnKind::Text,
            F::Text("memo"),
        ),
        tot(
            amt("amount", "@reports.col.amount", "totalExact"),
            "/totalsExact/total",
        ),
    ],
    drill: RowDrill::TxnOf("id", "deposit"),
    narrow: None,
};

const CHECK_REGISTER: Layout = Layout {
    rows: "data",
    caption: None,
    cols: &[
        date("date", "txn_date"),
        txn_type("txn_type"),
        c(
            "check_number",
            "@reports.col.check_number",
            ColumnKind::Text,
            F::Text("check_number"),
        ),
        c(
            "payee",
            "@reports.col.payee",
            ColumnKind::Text,
            F::Text("payee"),
        ),
        c(
            "memo",
            "@reports.col.memo",
            ColumnKind::Text,
            F::Text("memo"),
        ),
        tot(
            amt("deposit", "@reports.col.deposit", "debitExact"),
            "/totalsExact/debit",
        ),
        tot(
            amt("payment", "@reports.col.payment", "creditExact"),
            "/totalsExact/credit",
        ),
    ],
    drill: RowDrill::Txn("id", "txn_type"),
    narrow: None,
};

const BANK_BALANCES: Layout = Layout {
    rows: "accounts",
    caption: Some(F::Account("accountNumber", "name")),
    cols: &[tot(
        amt("balance", "@reports.col.balance", "balanceExact"),
        "/totalBalanceExact",
    )],
    drill: RowDrill::None,
    narrow: None,
};

const ACCOUNT_ACTIVITY: Layout = Layout {
    rows: "data",
    caption: Some(F::Account("account_number", "name")),
    cols: &[
        tot(
            amt("debit", "@reports.col.debit", "total_debitExact"),
            "/totalDebitsExact",
        ),
        tot(
            amt("credit", "@reports.col.credit", "total_creditExact"),
            "/totalCreditsExact",
        ),
        tot(amt("net", "@reports.col.net", "netExact"), "/totalNetExact"),
    ],
    drill: RowDrill::None,
    narrow: None,
};

const PREP_1099: Layout = Layout {
    rows: "data",
    caption: Some(F::Text("vendor_name")),
    cols: &[
        c(
            "tin",
            "@reports.col.tin",
            ColumnKind::Text,
            F::Last4("tax_id"),
        ),
        c(
            "address",
            "@reports.col.address",
            ColumnKind::Text,
            F::Text("address"),
        ),
        // U-209 (`report-fixes-v1`): the 1099 amount from the account-to-box
        // mapping, as the 1099 Summary, not every payment (QA-8).
        tot(
            amt(
                "reportable",
                "@reports.col.reportable_1099",
                "reportable_totalExact",
            ),
            "/totalReportableExact",
        ),
        // PR6-QA-4: listed, but marked when below the year's threshold.
        c(
            "threshold",
            "@reports.col.threshold_1099",
            ColumnKind::Text,
            F::BelowThreshold("reportable_meets_threshold"),
        ),
    ],
    drill: RowDrill::None,
    narrow: None,
};

/// The layout of a list source.
#[must_use]
pub(crate) const fn layout(source: Source) -> Option<&'static Layout> {
    Some(match source {
        Source::ArAgingDetail => &AR_AGING_DETAIL,
        Source::ApAgingDetail => &AP_AGING_DETAIL,
        Source::InvoiceList => &INVOICES,
        Source::UnpaidBills => &UNPAID_BILLS,
        Source::CustomerBalanceSummary => &CUSTOMER_BALANCES,
        Source::VendorBalanceSummary => &VENDOR_BALANCES,
        Source::BillPaymentHistory => &BILL_PAYMENTS,
        Source::TransactionListByVendor => &BY_VENDOR,
        Source::SalesByCustomer => &SALES_BY_CLIENT,
        Source::SalesByItem => &SALES_BY_ITEM,
        Source::ExpenseByVendor => &EXPENSES_BY_VENDOR,
        Source::DepositDetail => &DEPOSITS,
        Source::CheckRegister => &CHECK_REGISTER,
        Source::BankBalances => &BANK_BALANCES,
        Source::AccountActivitySummary => &ACCOUNT_ACTIVITY,
        Source::Ap1099Prep => &PREP_1099,
        _ => return None,
    })
}

/// The Open Invoices layout (the invoice list with `open=1`).
#[must_use]
pub(crate) const fn open_invoices() -> &'static Layout {
    &OPEN_INVOICES
}

/// A required exact amount at `key` of `v`, at full scale.
fn exact_at(v: &Value, key: &'static str) -> Result<Cell, ApiError> {
    match v.get(key) {
        Some(Value::String(s)) => decimal(s, key).map(Cell::Amount),
        _ => Err(shape(key)),
    }
}

fn opt_text(v: &Value, key: &str) -> Cell {
    match v.get(key) {
        // A whole number in a text column (the engine's integer
        // `check_number`) shows as its digits; never a fraction, so no
        // amount can arrive here as text.
        Some(Value::Number(n)) if n.is_u64() || n.is_i64() => Cell::Text(n.to_string()),
        _ => text(v, key).map_or(Cell::Empty, |s| Cell::Text(s.to_string())),
    }
}

/// The cell `f` reads from `v`.
pub(crate) fn read(v: &Value, f: F) -> Result<Cell, ApiError> {
    Ok(match f {
        F::Text(k) => opt_text(v, k),
        // Only an explicit `null` id is "no contact"; a route whose rows
        // carry no id (the Invoice List) shows the name.
        F::Name { name, id, none } => match v.get(id) {
            Some(Value::Null) => Cell::Text(none.to_string()),
            _ => {
                text(v, name).map_or_else(|| Cell::Text(none.to_string()), |s| Cell::Text(s.into()))
            }
        },
        F::Account(n, name) => Cell::Text(super::exact::account_caption(text(v, n), text(v, name))),
        F::Date(k) => match v.get(k) {
            None | Some(Value::Null) => Cell::Empty,
            Some(Value::String(s)) if s.is_empty() => Cell::Empty,
            Some(Value::String(s)) => Cell::Date(
                cl_core::CalendarDate::parse_wire(s.get(..10).unwrap_or(s))
                    .map_err(|_| shape(k))?,
            ),
            _ => return Err(shape(k)),
        },
        F::Amount(k) => exact_at(v, k)?,
        F::Count(k) => match v.get(k) {
            None | Some(Value::Null) => Cell::Empty,
            Some(Value::Number(n)) => n.as_u64().map_or_else(
                // A negative count (days before due) shows as given.
                || Cell::Text(n.to_string()),
                Cell::Count,
            ),
            Some(Value::String(s)) => s
                .trim()
                .parse::<u64>()
                .map_or_else(|_| Cell::Text(s.clone()), Cell::Count),
            _ => return Err(shape(k)),
        },
        F::TxnType(k) => text(v, k).map_or(Cell::Empty, |t| Cell::Text(type_label(t))),
        F::Code(prefix, k) => {
            text(v, k).map_or(Cell::Empty, |code| Cell::Text(code_label(prefix, code)))
        }
        F::Last4(k) => text(v, k).map_or(Cell::Empty, |t| Cell::Text(last4(t))),
        F::BelowThreshold(k) => match v.get(k) {
            Some(Value::Bool(false)) => Cell::Text(BELOW_THRESHOLD.into()),
            _ => Cell::Empty,
        },
    })
}

/// The engine codes each `@` prefix names in plain words; any other code
/// shows as the engine's word with spaces (never a raw key).
const CODES: [(&str, &[&str]); 4] = [
    (
        "reports.bucket",
        &["current", "1_30", "31_60", "61_90", "90_up", "over_90"],
    ),
    (
        "reports.status",
        &[
            "draft", "sent", "viewed", "partial", "paid", "overdue", "void", "open", "unpaid",
        ],
    ),
    (
        "reports.audit.action",
        &["create", "update", "void", "delete", "override"],
    ),
    ("reports.closing.method", &["password", "confirmed"]),
];

/// The caption of engine code `code` under `prefix`.
#[must_use]
pub fn code_label(prefix: &str, code: &str) -> String {
    let norm = code.replace('-', "_").replace('+', "_up");
    let known = CODES
        .iter()
        .any(|(p, list)| *p == prefix && list.contains(&norm.as_str()));
    if known {
        format!("@{prefix}.{norm}")
    } else {
        code.replace('_', " ")
    }
}

/// A taxpayer id as `•••• 1234` (Q-P28p): never more than its last four
/// digits.
#[must_use]
pub fn last4(tin: &str) -> String {
    let digits: Vec<char> = tin.chars().filter(char::is_ascii_digit).collect();
    let tail: String = digits[digits.len().saturating_sub(4)..].iter().collect();
    if tail.is_empty() {
        String::new()
    } else {
        format!("\u{2022}\u{2022}\u{2022}\u{2022} {tail}")
    }
}

fn meta(v: &Value, req: &ReportRequest) -> ReportMeta {
    ReportMeta {
        basis: meta_basis(v).or(req.basis),
        dates: Some(req.dates.clone()),
        ..ReportMeta::default()
    }
}

fn drill_of(e: &Value, d: RowDrill) -> Result<Option<Drill>, ApiError> {
    let (id_key, txn_type) = match d {
        RowDrill::None => return Ok(None),
        RowDrill::Txn(id, t) => (id, text(e, t).unwrap_or("").to_string()),
        RowDrill::TxnOf(id, t) => (id, t.to_string()),
    };
    Ok(super::exact::id(e, id_key, TransactionId::parse)?
        .map(|id| Drill::Transaction { id, txn_type }))
}

/// True when the row stays under the request's contact and bucket.
fn kept(
    e: &Value,
    n: &Narrow,
    contacts: &HashSet<ContactId>,
    no_contact: bool,
    bucket: Option<AgingBucket>,
) -> bool {
    if no_contact && !matches!(e.get(n.contact), None | Some(Value::Null)) {
        return false;
    }
    if !contacts.is_empty() {
        let c = text(e, n.contact).and_then(|s| ContactId::parse(s).ok());
        if !c.is_some_and(|c| contacts.contains(&c)) {
            return false;
        }
    }
    let Some(b) = bucket else { return true };
    let i = match b {
        AgingBucket::Current => 0,
        AgingBucket::Days1To30 => 1,
        AgingBucket::Days31To60 => 2,
        AgingBucket::Days61To90 => 3,
        AgingBucket::Over90 => 4,
        AgingBucket::Total => return true,
    };
    text(e, n.bucket) == Some(n.codes[i])
}

/// A list answer as its table.
///
/// # Errors
/// [`ApiError::ReportShape`] when an exact field is missing or unreadable.
pub(crate) fn table(v: &Value, req: &ReportRequest, l: &Layout) -> Result<ReportTable, ApiError> {
    if v.get("meta").is_some() {
        require_exact(v)?;
    }
    let columns: Vec<ColumnHead> = l
        .cols
        .iter()
        .map(|x| col(x.key, x.caption, x.kind))
        .collect();
    let contacts: HashSet<ContactId> = req.filters.contact_ids.iter().copied().collect();
    let bucket = req.filters.bucket;
    let narrowed =
        l.narrow.is_some() && (!contacts.is_empty() || bucket.is_some() || req.filters.no_contact);
    let mut rows = Vec::new();
    let items = match v.get(l.rows) {
        Some(Value::Array(a)) => a.as_slice(),
        None | Some(Value::Null) => &[],
        _ => return Err(shape("rows")),
    };
    for (i, e) in items.iter().enumerate() {
        if let Some(n) = &l.narrow
            && !kept(e, n, &contacts, req.filters.no_contact, bucket)
        {
            continue;
        }
        let caption = match l.caption {
            Some(f) => match read(e, f)? {
                Cell::Text(s) => s,
                _ => String::new(),
            },
            None => String::new(),
        };
        let cells = l
            .cols
            .iter()
            .map(|x| read(e, x.f))
            .collect::<Result<_, _>>()?;
        let mut r = row(
            RowKind::Detail,
            1,
            RowPath::of(["rows", i.to_string().as_str()]),
            caption,
            cells,
        );
        r.drill = drill_of(e, l.drill)?;
        rows.push(r);
    }
    if !narrowed && l.cols.iter().any(|x| x.total.is_some()) {
        let cells = l
            .cols
            .iter()
            .map(|x| match x.total {
                Some(p) => match v.pointer(p) {
                    Some(Value::String(s)) => decimal(s, "total").map(Cell::Amount),
                    _ => Err(shape("total")),
                },
                None => Ok(Cell::Empty),
            })
            .collect::<Result<_, _>>()?;
        rows.push(row(
            RowKind::Grand,
            0,
            RowPath::of(["total"]),
            "@reports.row.total".into(),
            cells,
        ));
    }
    Ok(ReportTable {
        columns,
        rows,
        meta: meta(v, req),
    })
}

/// Customer or Vendor Balance Detail: per contact a heading, its open
/// documents (open amount and the engine's running balance) and its
/// balance; the engine's total last.
///
/// # Errors
/// [`ApiError::ReportShape`] when an exact field is missing.
pub fn balance_detail(v: &Value, req: &ReportRequest, none: &str) -> Result<ReportTable, ApiError> {
    require_exact(v)?;
    let columns = vec![
        col("date", "@reports.col.date", ColumnKind::Date),
        col("type", "@reports.col.type", ColumnKind::Text),
        col("number", "@reports.col.number", ColumnKind::Text),
        col("due", "@reports.col.due", ColumnKind::Date),
        col("amount", "@reports.col.amount", ColumnKind::Amount),
        col("open", "@reports.col.open", ColumnKind::Amount),
        col("balance", "@reports.col.balance", ColumnKind::Amount),
    ];
    let mut rows = Vec::new();
    for (i, e) in super::exact::list(v, "data")?.iter().enumerate() {
        let key = text(e, "id").map_or_else(|| format!("none{i}"), str::to_string);
        let name = match read(
            e,
            F::Name {
                name: "display_name",
                id: "id",
                none: "",
            },
        )? {
            Cell::Text(s) if !s.is_empty() => s,
            _ => none.to_string(),
        };
        let mut head = row(
            RowKind::Header,
            0,
            RowPath::of([key.as_str()]),
            name.clone(),
            vec![Cell::Empty; 7],
        );
        head.expandable = true;
        rows.push(head);
        for (j, it) in super::exact::list(e, "items")?.iter().enumerate() {
            let mut r = row(
                RowKind::Detail,
                1,
                RowPath::of([key.as_str(), j.to_string().as_str()]),
                String::new(),
                vec![
                    read(it, F::Date("date"))?,
                    read(it, F::TxnType("txnType"))?,
                    read(it, F::Text("number"))?,
                    read(it, F::Date("dueDate"))?,
                    exact_at(it, "amountExact")?,
                    exact_at(it, "openExact")?,
                    exact_at(it, "runningBalanceExact")?,
                ],
            );
            r.drill = drill_of(it, RowDrill::Txn("txnId", "txnType"))?;
            rows.push(r);
        }
        let mut cells = vec![Cell::Empty; 6];
        cells.push(exact_at(e, "balanceExact")?);
        rows.push(row(
            RowKind::Subtotal,
            0,
            RowPath::of([format!("{key}~total").as_str()]),
            format!("@reports.row.total_of {name}"),
            cells,
        ));
    }
    let mut cells = vec![Cell::Empty; 6];
    cells.push(match v.pointer("/totalsExact/balance") {
        Some(Value::String(s)) => Cell::Amount(decimal(s, "totalsExact.balance")?),
        _ => return Err(shape("totalsExact.balance")),
    });
    rows.push(row(
        RowKind::Grand,
        0,
        RowPath::of(["total"]),
        "@reports.row.total".into(),
        cells,
    ));
    Ok(ReportTable {
        columns,
        rows,
        meta: meta(v, req),
    })
}

/// A/P Aging Summary (U-121, as of a date): one row per vendor and the
/// "(no vendor)" row, the five buckets and the total; the engine's totals
/// last.
///
/// # Errors
/// [`ApiError::ReportShape`] when an exact field is missing.
pub fn ap_aging_summary(
    v: &Value,
    req: &ReportRequest,
    as_of: cl_core::CalendarDate,
) -> Result<ReportTable, ApiError> {
    const KEYS: [&str; 6] = [
        "currentExact",
        "bucket1to30Exact",
        "bucket31to60Exact",
        "bucket61to90Exact",
        "bucketOver90Exact",
        "totalExact",
    ];
    const TOTALS: [&str; 6] = [
        "current",
        "bucket1to30",
        "bucket31to60",
        "bucket61to90",
        "bucketOver90",
        "total",
    ];
    require_exact(v)?;
    let columns = AgingBucket::KEYS
        .iter()
        .zip(super::ar_aging::CAPTIONS)
        .map(|(k, c)| col(k, c, ColumnKind::Amount))
        .collect();
    let mut rows = Vec::new();
    for (i, e) in super::exact::list(v, "vendors")?.iter().enumerate() {
        let contact = super::exact::id(e, "contact_id", ContactId::parse)?;
        let caption = match read(
            e,
            F::Name {
                name: "vendor_name",
                id: "contact_id",
                none: NO_VENDOR,
            },
        )? {
            Cell::Text(s) => s,
            _ => NO_VENDOR.to_string(),
        };
        let key = contact.map_or_else(|| "none".to_string(), |c| c.to_string());
        let cells = KEYS
            .iter()
            .map(|k| exact_at(e, k))
            .collect::<Result<_, _>>()?;
        let mut r = row(
            RowKind::Detail,
            1,
            RowPath::of(["vendors", key.as_str(), i.to_string().as_str()]),
            caption,
            cells,
        );
        r.drill = Some(Drill::Aging {
            contact,
            bucket: AgingBucket::Total,
            as_of,
        });
        rows.push(r);
    }
    let totals = v.get("totalsExact").ok_or(shape("totalsExact"))?;
    let cells = TOTALS
        .iter()
        .map(|k| match totals.get(*k) {
            Some(Value::String(s)) => decimal(s, "totalsExact").map(Cell::Amount),
            _ => Err(shape("totalsExact")),
        })
        .collect::<Result<_, _>>()?;
    rows.push(row(
        RowKind::Grand,
        0,
        RowPath::of(["total"]),
        "@reports.row.total".into(),
        cells,
    ));
    Ok(ReportTable {
        columns,
        rows,
        meta: meta(v, req),
    })
}

/// The 1099 Summary by box (U-205): one column per box of the engine's
/// catalog, a row per vendor with the TIN's last four digits (Q-P28p),
/// and the engine's totals.
///
/// # Errors
/// [`ApiError::ReportShape`] when an exact field is missing.
pub fn vendor_1099_summary(v: &Value, req: &ReportRequest) -> Result<ReportTable, ApiError> {
    require_exact(v)?;
    let catalog = super::exact::list(v, "boxCatalog")?;
    let mut columns = vec![col("tin", "@reports.col.tin", ColumnKind::Text)];
    let mut boxes = Vec::new();
    // Each box's threshold for the year (PR6-QA-4).
    let mut thresholds = std::collections::HashMap::new();
    for b in catalog {
        let code = text(b, "box").ok_or(shape("boxCatalog.box"))?;
        if let Some(t) = text(b, "thresholdExact") {
            thresholds.insert(code.to_string(), decimal(t, "boxCatalog.thresholdExact")?);
        }
        // `1099-NEC 1 · Nonemployee compensation`: owner-facing, from the
        // engine's catalog.
        let caption = format!(
            "{} {}",
            text(b, "form").unwrap_or(""),
            text(b, "boxNumber").unwrap_or(code)
        );
        columns.push(col(code, caption.trim(), ColumnKind::Amount));
        boxes.push(code.to_string());
    }
    columns.push(col("total", "@reports.col.total", ColumnKind::Amount));
    columns.push(col(
        "threshold",
        "@reports.col.threshold_1099",
        ColumnKind::Text,
    ));
    let box_cell = |m: Option<&Value>, code: &str| -> Result<Cell, ApiError> {
        match m.and_then(|m| m.get(code)) {
            None | Some(Value::Null) => Ok(Cell::Empty),
            Some(Value::String(s)) => decimal(s, "boxes").map(Cell::Amount),
            _ => Err(shape("boxes")),
        }
    };
    let mut rows = Vec::new();
    for (i, e) in super::exact::list(v, "rows")?.iter().enumerate() {
        let mut cells = vec![text(e, "tinLast4").map_or(Cell::Empty, |t| Cell::Text(last4(t)))];
        for b in &boxes {
            cells.push(box_cell(e.get("boxes"), b)?);
        }
        cells.push(exact_at(e, "totalExact")?);
        // Below the threshold: the engine's flag says so; the amount is
        // named when every box the vendor has shares one threshold.
        cells.push(match e.get("meetsThreshold") {
            Some(Value::Bool(false)) => {
                let mut of = boxes
                    .iter()
                    .filter(|b| {
                        e.get("boxes")
                            .and_then(|m| m.get(b.as_str()))
                            .is_some_and(|x| !x.is_null())
                    })
                    .map(|b| thresholds.get(b).copied());
                let first = of.next().flatten();
                match first {
                    Some(t) if of.all(|x| x == Some(t)) => Cell::Text(below_threshold_of(t)),
                    _ => Cell::Text(BELOW_THRESHOLD.into()),
                }
            }
            _ => Cell::Empty,
        });
        rows.push(row(
            RowKind::Detail,
            1,
            RowPath::of(["rows", i.to_string().as_str()]),
            text(e, "name").unwrap_or("").to_string(),
            cells,
        ));
    }
    let totals = v.get("totalsExact");
    let mut cells = vec![Cell::Empty];
    for b in &boxes {
        cells.push(box_cell(totals, b)?);
    }
    cells.push(match totals.and_then(|t| t.get("total")) {
        Some(Value::String(s)) => Cell::Amount(decimal(s, "totalsExact.total")?),
        _ => return Err(shape("totalsExact.total")),
    });
    cells.push(Cell::Empty);
    rows.push(row(
        RowKind::Grand,
        0,
        RowPath::of(["total"]),
        "@reports.row.total".into(),
        cells,
    ));
    Ok(ReportTable {
        columns,
        rows,
        meta: meta(v, req),
    })
}

/// The Vendor Statement: the opening balance, each charge and payment with
/// the engine's running balance, and the engine's totals and closing
/// balance.
///
/// # Errors
/// [`ApiError::ReportShape`] when an exact field is missing.
pub fn vendor_statement(v: &Value, req: &ReportRequest) -> Result<ReportTable, ApiError> {
    require_exact(v)?;
    let columns = vec![
        col("date", "@reports.col.date", ColumnKind::Date),
        col("type", "@reports.col.type", ColumnKind::Text),
        col("number", "@reports.col.number", ColumnKind::Text),
        col(
            "vendor_number",
            "@reports.col.vendor_number",
            ColumnKind::Text,
        ),
        col("memo", "@reports.col.memo", ColumnKind::Text),
        col("charge", "@reports.col.charge", ColumnKind::Amount),
        col("payment", "@reports.col.payment", ColumnKind::Amount),
        col("balance", "@reports.col.balance", ColumnKind::Amount),
    ];
    let mut rows = Vec::new();
    let mut open = vec![Cell::Empty; 7];
    open.push(exact_at(v, "openingBalanceExact")?);
    rows.push(row(
        RowKind::Header,
        0,
        RowPath::of(["opening"]),
        "@reports.row.opening_balance".into(),
        open,
    ));
    for (i, l) in super::exact::list(v, "lines")?.iter().enumerate() {
        let mut r = row(
            RowKind::Detail,
            1,
            RowPath::of(["lines", i.to_string().as_str()]),
            String::new(),
            vec![
                read(l, F::Date("txn_date"))?,
                read(l, F::TxnType("txn_type"))?,
                read(l, F::Text("txn_number"))?,
                read(l, F::Text("vendor_invoice_number"))?,
                read(l, F::Text("memo"))?,
                exact_at(l, "chargeExact")?,
                exact_at(l, "paymentExact")?,
                exact_at(l, "balanceExact")?,
            ],
        );
        r.drill = drill_of(l, RowDrill::Txn("id", "txn_type"))?;
        rows.push(r);
    }
    let mut close = vec![Cell::Empty; 5];
    close.push(exact_at(v, "totalChargesExact")?);
    close.push(exact_at(v, "totalPaymentsExact")?);
    close.push(exact_at(v, "closingBalanceExact")?);
    rows.push(row(
        RowKind::Grand,
        0,
        RowPath::of(["total"]),
        "@reports.row.closing_balance".into(),
        close,
    ));
    Ok(ReportTable {
        columns,
        rows,
        meta: meta(v, req),
    })
}

/// The Duplicate Finder (Q-P28o): each likely pair as a heading and its two
/// transactions; the engine's cap noted when reached. A list only: either
/// transaction opens in its form.
///
/// # Errors
/// [`ApiError::ReportShape`] when an amount is not exact text.
pub fn duplicates(v: &Value, req: &ReportRequest) -> Result<ReportTable, ApiError> {
    let columns = vec![
        col("date", "@reports.col.date", ColumnKind::Date),
        col("type", "@reports.col.type", ColumnKind::Text),
        col("number", "@reports.col.number", ColumnKind::Text),
        col("payee", "@reports.col.payee", ColumnKind::Text),
        col("memo", "@reports.col.memo", ColumnKind::Text),
        col("amount", "@reports.col.amount", ColumnKind::Amount),
    ];
    let pairs = super::exact::list(v, "pairs")?;
    let mut rows = Vec::new();
    for (i, p) in pairs.iter().enumerate() {
        let k = i.to_string();
        let days = p.get("daysApart").and_then(Value::as_u64).unwrap_or(0);
        rows.push(row(
            RowKind::Header,
            0,
            RowPath::of(["pairs", k.as_str()]),
            format!("@reports.row.duplicate_pair {days}"),
            vec![Cell::Empty; 6],
        ));
        for side in ["a", "b"] {
            let t = p.get(side).ok_or(shape("pairs"))?;
            let mut r = row(
                RowKind::Detail,
                1,
                RowPath::of(["pairs", k.as_str(), side]),
                String::new(),
                vec![
                    read(t, F::Date("txnDate"))?,
                    read(t, F::TxnType("txnType"))?,
                    read(t, F::Text("txnNumber"))?,
                    read(t, F::Text("payee"))?,
                    read(t, F::Text("memo"))?,
                    exact_at(t, "total")?,
                ],
            );
            r.drill = drill_of(t, RowDrill::Txn("id", "txnType"))?;
            rows.push(r);
        }
    }
    let limit = v.get("limit").and_then(Value::as_u64);
    let count = v.get("count").and_then(Value::as_u64).unwrap_or(0);
    let mut meta = meta(v, req);
    if let Some(limit) = limit
        && count >= limit
    {
        meta.truncated = Some(cl_core::report::Truncated {
            shown: count,
            total: count,
        });
    }
    Ok(ReportTable {
        columns,
        rows,
        meta,
    })
}

/// Rows the request needs a pick for before it can run.
///
/// # Errors
/// `Local(MissingPick)` when a vendor or bank account must be chosen.
pub fn require_pick(req: &ReportRequest) -> Result<(), ApiError> {
    let missing = match req.source {
        Source::TransactionListByVendor | Source::VendorStatement => {
            req.filters.contact_ids.is_empty()
        }
        Source::CheckRegister => req.filters.account_ids.is_empty(),
        _ => false,
    };
    if missing {
        Err(ApiError::Local(LocalValidation::MissingPick))
    } else {
        Ok(())
    }
}
