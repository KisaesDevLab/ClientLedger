//! Turning what the engine reports into pages (P19): check data and voucher
//! stubs, form pages, envelopes, and the hand-off to the printer on its own
//! thread (`cl_host::OsPrint` starts processes and can wait on the print
//! system, so it never runs on the egui thread). In the demo and under the
//! control channel nothing reaches a printer: the outcome is made up.
//!
//! Amounts arrive as [`Money`] and go to `cl-print` as exact decimals.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use cl_api::{BillPaymentRecord, DocType, QueuedCheck, QueuedForm, Transaction};
use cl_core::Money;
use cl_print::checks::StubLine;
use cl_print::forms::{Column, TotalRow};
use cl_print::{
    CheckData, CheckLabels, CompanyInfo, Document, FormDoc, FormLabels, StubBill, StubCredit,
};

use crate::i18n::t;

/// What the printer hand-off did, worded for the owner (Q-P19al).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sent {
    /// Printed straight to the printer.
    Printed,
    /// Handed to the PDF app or `lp` (it may not have printed).
    Handed,
    /// Printing failed; the PDF is open in the viewer.
    Viewer,
    /// Nothing worked; the PDF was saved.
    Failed,
    /// The printer's name can't be passed on.
    BadPrinter,
}

impl Sent {
    /// The i18n key of the line that says what happened.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Sent::Printed => "print.sent.printed",
            Sent::Handed => "print.sent.sent",
            Sent::Viewer => "print.sent.viewer",
            Sent::Failed => "print.sent.failed",
            Sent::BadPrinter => "print.sent.bad_printer",
        }
    }

    /// The control channel's word.
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Sent::Printed => "printed",
            Sent::Handed => "sent",
            Sent::Viewer => "viewer",
            Sent::Failed => "failed",
            Sent::BadPrinter => "bad_printer",
        }
    }
}

/// Where the hand-off writes its temporary PDF (`<data>/print`), when
/// there is an app data folder.
#[derive(Clone, Debug)]
pub(crate) struct Printer {
    /// The app data folder (the settings folder's parent).
    pub root: Option<PathBuf>,
    /// Make the outcome up (demo, control channel): never print.
    pub fake: bool,
}

/// Prints `doc` (whose bytes are `pdf`) on `printer`, `copies` times, on a
/// thread; the answer arrives on the receiver.
pub(crate) fn print(
    p: &Printer,
    doc: Document,
    pdf: std::sync::Arc<[u8]>,
    kind: &'static str,
    printer: Option<String>,
    copies: u16,
) -> Receiver<Sent> {
    let (tx, rx) = channel();
    if p.fake {
        let _ = tx.send(Sent::Printed);
        return rx;
    }
    let root = p.root.clone();
    let spawned = std::thread::Builder::new()
        .name("cl-print".into())
        .spawn(move || {
            let sent = (|| {
                let paths = cl_host::AppPaths::at(root?).ok()?;
                let path = cl_host::write_temp_pdf(&paths, kind, &pdf).ok()?;
                let os = cl_host::OsPrint::system();
                Some(
                    match os.print_document(&doc, &path, printer.as_deref(), copies) {
                        cl_host::PrintOutcome::Printed { .. } => Sent::Printed,
                        cl_host::PrintOutcome::Sent { .. } => Sent::Handed,
                        cl_host::PrintOutcome::OpenedInViewer { .. } => Sent::Viewer,
                        cl_host::PrintOutcome::Failed { .. } => Sent::Failed,
                        cl_host::PrintOutcome::InvalidPrinter { .. } => Sent::BadPrinter,
                    },
                )
            })()
            .unwrap_or(Sent::Failed);
            let _ = tx.send(sent);
        });
    if spawned.is_err() {
        tracing::warn!("the print hand-off could not start");
    }
    rx
}

/// The installed printers and the default one, read on a thread (the OS
/// commands can take a moment). The demo lists made-up printers.
pub(crate) fn printers(fake: bool) -> Receiver<(Vec<String>, Option<String>)> {
    let (tx, rx) = channel();
    if fake {
        let _ = tx.send((
            vec!["Office Laser".to_string(), "Front Desk Inkjet".to_string()],
            Some("Office Laser".to_string()),
        ));
        return rx;
    }
    let spawned = std::thread::Builder::new()
        .name("cl-printers".into())
        .spawn(move || {
            let os = cl_host::OsPrint::system();
            let _ = tx.send((os.list_printers(), os.default_printer_name()));
        });
    if spawned.is_err() {
        tracing::warn!("the printer list could not be read");
    }
    rx
}

/// Writes `bytes` to `path` on a thread; true when it worked.
pub(crate) fn save(path: PathBuf, bytes: std::sync::Arc<[u8]>) -> Receiver<bool> {
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new()
        .name("cl-save-pdf".into())
        .spawn(move || {
            let ok = path
                .parent()
                .is_none_or(|d| std::fs::create_dir_all(d).is_ok())
                && std::fs::write(&path, &*bytes).is_ok();
            let _ = tx.send(ok);
        });
    if spawned.is_err() {
        tracing::warn!("the PDF could not be saved");
    }
    rx
}

fn dec(m: Money) -> rust_decimal::Decimal {
    m.as_decimal()
}

fn opt_dec(m: Option<Money>) -> rust_decimal::Decimal {
    m.map_or(rust_decimal::Decimal::ZERO, dec)
}

/// A bill payment's voucher stub: the bills paid and the credits used
/// (Q-213, U-108).
#[must_use]
pub(crate) fn stub_of_payment(r: &BillPaymentRecord) -> cl_print::StubData {
    let date = |d: Option<cl_core::CalendarDate>| d.map(crate::shell::us_date).unwrap_or_default();
    cl_print::StubData {
        bills: r
            .bill_applications
            .iter()
            .map(|a| {
                let doc = a.bill.as_ref();
                StubBill {
                    ref_no: doc.and_then(|d| d.txn_number.clone()).unwrap_or_default(),
                    vendor_invoice_no: doc
                        .and_then(|d| d.vendor_invoice_number.clone())
                        .unwrap_or_default(),
                    date: date(doc.and_then(|d| d.txn_date)),
                    bill_total: opt_dec(doc.and_then(|d| d.total)),
                    discount: opt_dec(a.discount_amount),
                    amount_paid: dec(a.amount),
                }
            })
            .collect(),
        credits: r
            .credit_applications
            .iter()
            .map(|c| {
                let doc = c.credit.as_ref();
                StubCredit {
                    ref_no: doc
                        .and_then(|d| d.vendor_invoice_number.clone().or(d.txn_number.clone()))
                        .unwrap_or_default(),
                    date: date(doc.and_then(|d| d.txn_date)),
                    amount: dec(c.amount),
                }
            })
            .collect(),
        lines: Vec::new(),
    }
}

/// Every account's `Parent:Child` name (stubs name accounts the way the
/// lists and the print queue do).
#[must_use]
pub(crate) fn full_names(
    accounts: &[cl_api::Account],
) -> std::collections::BTreeMap<cl_core::ids::AccountId, String> {
    let by_id: std::collections::BTreeMap<_, _> = accounts.iter().map(|a| (a.id, a)).collect();
    accounts
        .iter()
        .map(|a| {
            let mut parts = vec![a.fields.name.clone()];
            let mut parent = a.fields.parent_id;
            // A parent chain never loops in the engine; the bound guards it.
            for _ in 0..16 {
                let Some(p) = parent.and_then(|p| by_id.get(&p)) else {
                    break;
                };
                parts.push(p.fields.name.clone());
                parent = p.fields.parent_id;
            }
            parts.reverse();
            (a.id, parts.join(":"))
        })
        .collect()
}

/// A written check's voucher stub: its expense lines (Q-P19o), each named
/// with its full account name when known.
#[must_use]
pub(crate) fn stub_of_check(
    t: &Transaction,
    names: &std::collections::BTreeMap<cl_core::ids::AccountId, String>,
) -> cl_print::StubData {
    cl_print::StubData {
        lines: t
            .lines
            .iter()
            .filter(|l| l.debit.is_positive())
            .map(|l| StubLine {
                name: names
                    .get(&l.account_id)
                    .cloned()
                    .or_else(|| l.account_name.clone())
                    .unwrap_or_default(),
                amount: dec(l.debit),
            })
            .collect(),
        ..cl_print::StubData::default()
    }
}

/// One check as it prints.
#[must_use]
pub(crate) fn check_data(
    c: &QueuedCheck,
    number: Option<u64>,
    stub: cl_print::StubData,
) -> CheckData {
    // The address may already start with the payee's name (Write Check's
    // address box does); the name is printed once.
    let named = c
        .address
        .first()
        .is_some_and(|l| l.trim().eq_ignore_ascii_case(c.payee.trim()));
    let mut address = if named {
        Vec::new()
    } else {
        vec![c.payee.clone()]
    };
    address.extend(c.address.iter().cloned());
    CheckData {
        number: number.map(|n| n.to_string()).unwrap_or_default(),
        date: crate::shell::us_date(c.date),
        payee: c.payee.clone(),
        amount: dec(c.amount),
        address,
        memo: c.memo.clone(),
        stub,
    }
}

/// The fixed words on checks, from the catalog.
#[must_use]
pub(crate) fn check_labels() -> CheckLabels {
    CheckLabels {
        check_no: t("print.pdf.check_no"),
        date: t("print.pdf.date"),
        ref_no: t("print.pdf.ref_no"),
        vendor_invoice: t("print.pdf.vendor_invoice"),
        bill_total: t("print.pdf.bill_total"),
        discount: t("print.pdf.discount"),
        credit: t("print.pdf.credit"),
        amount_paid: t("print.pdf.amount_paid"),
        credit_row: t("print.pdf.credit"),
        account_or_item: t("print.pdf.account_or_item"),
        amount: t("print.pdf.amount"),
        totals: t("print.pdf.totals"),
        total: t("print.pdf.total"),
        check_amount: t("print.pdf.check_amount"),
        memo: t("print.pdf.memo"),
        more_rows: t("print.pdf.more_rows"),
        sample_payee: t("print.pdf.sample_payee"),
        sample_memo: t("print.pdf.sample_memo"),
        no_address: t("print.pdf.no_address"),
    }
}

/// The fixed words on forms, from the catalog (CLAUDE.md: every
/// user-visible string goes through `i18n`).
#[must_use]
pub(crate) fn form_labels() -> FormLabels {
    // `page_of` keeps its `{page}`/`{pages}` placeholders: cl-print fills
    // them, so the raw catalog text is read rather than `t_with`.
    FormLabels {
        invoice: t("print.kind.invoice"),
        sales_receipt: t("print.kind.sales_receipt"),
        credit_memo: t("print.kind.credit_memo"),
        statement: t("print.kind.statement"),
        deposit_summary: t("print.pdf.deposit_summary"),
        bill_to: t("print.pdf.bill_to"),
        sold_to: t("print.pdf.sold_to"),
        customer: t("print.pdf.customer"),
        number: t("print.pdf.number"),
        date: t("print.pdf.date"),
        due_date: t("print.pdf.due_date"),
        terms: t("print.pdf.terms"),
        po_number: t("print.pdf.po_number"),
        ship_to: t("print.pdf.ship_to"),
        service_date: t("print.pdf.service_date"),
        payment_method: t("print.pdf.payment_method"),
        reference: t("print.pdf.reference"),
        statement_date: t("print.pdf.statement_date"),
        deposit_to: t("print.pdf.deposit_to"),
        description: t("print.pdf.description"),
        quantity: t("print.pdf.quantity"),
        rate: t("print.pdf.rate"),
        amount: t("print.pdf.amount"),
        received_from: t("print.pdf.received_from"),
        memo: t("print.pdf.memo"),
        check_no: t("print.pdf.check_no"),
        transaction: t("print.pdf.transaction"),
        balance: t("print.pdf.balance"),
        balance_forward: t("print.pdf.balance_forward"),
        subtotal: t("print.pdf.subtotal"),
        tax: t("print.pdf.tax"),
        total: t("print.pdf.total"),
        payments: t("print.pdf.payments"),
        balance_due: t("print.pdf.balance_due"),
        total_credit: t("print.pdf.total_credit"),
        remaining_credit: t("print.pdf.remaining_credit"),
        deposit_subtotal: t("print.pdf.deposit_subtotal"),
        cash_back: t("print.pdf.cash_back"),
        deposit_total: t("print.pdf.deposit_total"),
        amount_due: t("print.pdf.amount_due"),
        current: t("print.pdf.current"),
        days_1_30: t("print.pdf.days_1_30"),
        days_31_60: t("print.pdf.days_31_60"),
        days_61_90: t("print.pdf.days_61_90"),
        over_90: t("print.pdf.over_90"),
        continued: t("print.pdf.continued"),
        page_of: t("print.pdf.page_of"),
    }
}

/// The check style the PDF uses for the engine's check format.
#[must_use]
pub(crate) fn style_of(format: cl_api::CheckFormat) -> cl_print::CheckStyle {
    use cl_api::CheckFormat;
    match format {
        CheckFormat::Standard => cl_print::CheckStyle::Standard,
        CheckFormat::ZFold => cl_print::CheckStyle::ZFold,
        CheckFormat::Voucher | CheckFormat::CheckMiddle => cl_print::CheckStyle::Voucher,
    }
}

/// The form's title word.
#[must_use]
pub(crate) fn kind_word(kind: DocType) -> String {
    t(match kind {
        DocType::Invoice => "print.kind.invoice",
        DocType::SalesReceipt => "print.kind.sales_receipt",
        DocType::CreditMemo => "print.kind.credit_memo",
        DocType::Statement => "print.kind.statement",
    })
}

/// One form's page content: its lines are the income lines of the
/// transaction (description or account, amount), the total from the
/// engine. `lines` empty when the transaction could not be read.
#[must_use]
pub(crate) fn form_doc(f: &QueuedForm, address: &[String], lines: &[(String, Money)]) -> FormDoc {
    let rows = lines
        .iter()
        .map(|(name, amount)| vec![name.clone(), cl_print::format_amount(dec(*amount))])
        .collect();
    let total = f
        .total
        .unwrap_or_else(|| Money::sum(lines.iter().map(|(_, a)| *a)).unwrap_or(Money::ZERO));
    FormDoc {
        title: kind_word(f.kind),
        number: f.number.clone().unwrap_or_default(),
        address_label: t(if f.kind == DocType::Invoice {
            "print.pdf.bill_to"
        } else {
            "print.pdf.sold_to"
        }),
        address: address.to_vec(),
        fields: f
            .number
            .clone()
            .filter(|n| !n.trim().is_empty())
            .map(|n| (t("print.pdf.number"), n))
            .into_iter()
            .chain([(t("print.pdf.date"), crate::shell::us_date(f.date))])
            .collect(),
        columns: vec![
            Column::text("description", &t("print.pdf.description"), 140.0),
            Column::number("amount", &t("print.pdf.amount"), 50.5),
        ],
        rows,
        totals: vec![TotalRow::new(&t("print.pdf.total"), dec(total), true)],
        message: String::new(),
        boxes: Vec::new(),
        ..FormDoc::default()
    }
}

/// A form's income lines from its transaction: credits other than the
/// receivable or the deposit account.
#[must_use]
pub(crate) fn form_lines(t: &Transaction) -> Vec<(String, Money)> {
    t.lines
        .iter()
        .filter(|l| l.credit.is_positive())
        .map(|l| {
            (
                l.description
                    .clone()
                    .filter(|d| !d.trim().is_empty())
                    .or_else(|| l.account_name.clone())
                    .unwrap_or_default(),
                l.credit,
            )
        })
        .collect()
}

/// A saved deposit's summary as the printed page lists it: the payments,
/// then the other lines (fees negative), cash back and the engine's total.
/// The subtotal is the exact sum of the rows.
#[must_use]
pub(crate) fn deposit_data(
    s: &cl_model::forms::deposit::DepositSummary,
) -> cl_print::forms::deposit_summary::DepositSummaryData {
    use cl_print::forms::deposit_summary::{DepositLine, DepositSummaryData};
    let rows: Vec<_> = s.items.iter().chain(&s.other).collect();
    let subtotal = Money::sum(rows.iter().map(|r| r.amount))
        .map_or_else(|_| dec(s.total) + opt_dec(s.cash_back), dec);
    DepositSummaryData {
        account: s.account.clone(),
        date: crate::shell::us_date(s.date),
        lines: rows
            .iter()
            .map(|r| DepositLine {
                received_from: r.label.clone(),
                memo: r.detail.clone(),
                check_no: r.check_no.clone(),
                payment_method: r.method.clone(),
                amount: dec(r.amount),
            })
            .collect(),
        subtotal,
        cash_back: s.cash_back.map(dec),
        total: dec(s.total),
        memo: s.memo.clone(),
    }
}

/// Several documents as one (forms print as one job, Q-P19t).
#[must_use]
pub(crate) fn join(mut docs: Vec<Document>, title: &str) -> Option<Document> {
    let mut first = docs.drain(..1.min(docs.len())).next()?;
    for d in docs {
        first.pages.extend(d.pages);
    }
    first.title = title.to_string();
    Some(first)
}

/// The company block from its name and address rows.
#[must_use]
pub(crate) fn company(name: &str, lines: Vec<String>) -> CompanyInfo {
    CompanyInfo {
        name: name.to_string(),
        lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::ids::TransactionId;

    fn m(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn check() -> QueuedCheck {
        QueuedCheck {
            id: TransactionId::from_uuid(uuid::Uuid::from_u128(1)),
            date: cl_core::CalendarDate::from_ymd(2026, 11, 4).expect("d"),
            amount: m("1286.40"),
            payee: "Feed & Seed".into(),
            memo: "Acct 7712".into(),
            kind: cl_api::QueuedCheckKind::Check,
            bank_account: None,
            address: vec!["1200 Commerce Dr".into(), "Aurora, MO 65605".into()],
        }
    }

    #[test]
    fn a_check_prints_its_exact_amount_and_the_payee_above_the_address() {
        let c = check_data(&check(), Some(1001), cl_print::StubData::default());
        assert_eq!(c.amount, m("1286.40").as_decimal());
        assert_eq!(c.number, "1001");
        assert_eq!(c.address[0], "Feed & Seed");
        assert_eq!(c.address.len(), 3);
        // An address that already starts with the name prints it once.
        let mut named = check();
        named.address.insert(0, "Feed & Seed".into());
        let c = check_data(&named, Some(1001), cl_print::StubData::default());
        assert_eq!(c.address.len(), 3);
        assert_eq!(c.address[0], "Feed & Seed");
        assert_eq!(c.date, "11/04/2026");
    }

    #[test]
    fn a_voucher_check_lays_out_with_a_stub_that_adds_up() {
        let stub = cl_print::StubData {
            lines: vec![
                StubLine {
                    name: "Plants".into(),
                    amount: m("842.15").as_decimal(),
                },
                StubLine {
                    name: "Mulch".into(),
                    amount: m("444.25").as_decimal(),
                },
            ],
            ..cl_print::StubData::default()
        };
        let c = check_data(&check(), Some(1001), stub);
        let opts = cl_print::CheckOptions {
            labels: check_labels(),
            ..cl_print::CheckOptions::default()
        };
        let doc = cl_print::layout_checks(&[c], &opts).expect("layout");
        assert_eq!(doc.pages.len(), 1);
        let pdf = cl_print::render(&doc).expect("pdf");
        assert!(pdf.starts_with(b"%PDF"));
    }

    #[test]
    fn form_labels_come_from_the_catalog() {
        let l = form_labels();
        // Every word is a catalog text, not a missing key.
        assert_eq!(l.deposit_summary, "Deposit Summary");
        assert_eq!(l.cash_back, "Less cash back");
        assert!(!l.over_90.starts_with("print."));
        // cl-print fills the page placeholders itself.
        assert!(l.page_of.contains("{page}") && l.page_of.contains("{pages}"));
        // English matches cl-print's defaults word for word.
        assert_eq!(l, FormLabels::default());
    }

    #[test]
    fn styles_follow_the_engine_format() {
        assert_eq!(
            style_of(cl_api::CheckFormat::ZFold),
            cl_print::CheckStyle::ZFold
        );
        assert_eq!(
            style_of(cl_api::CheckFormat::CheckMiddle),
            cl_print::CheckStyle::Voucher
        );
    }

    #[test]
    fn forms_join_into_one_document() {
        let f = QueuedForm {
            id: TransactionId::from_uuid(uuid::Uuid::from_u128(2)),
            kind: DocType::Invoice,
            number: Some("INV-1040".into()),
            date: cl_core::CalendarDate::from_ymd(2026, 11, 2).expect("d"),
            customer: Some("Hollister".into()),
            total: Some(m("2450.00")),
        };
        let style = cl_print::FormStyle {
            company: company("Ozark Landscaping", vec!["Branson, MO".into()]),
            logo: None,
            footer: "Thanks".into(),
            labels: cl_print::FormLabels::default(),
        };
        let address = [
            "Hollister Family Trust".to_string(),
            "200 Riverbend Pkwy".into(),
        ];
        let doc = form_doc(&f, &address, &[("Design".into(), m("2450.00"))]);
        // The invoice prints its number and its bill-to address.
        assert_eq!(doc.fields[0].1, "INV-1040");
        assert_eq!(doc.address.len(), 2);
        let one = cl_print::layout_form(&doc, &style).expect("form");
        let two = cl_print::layout_form(&form_doc(&f, &[], &[]), &style).expect("form");
        let joined = join(vec![one, two], "Forms").expect("joined");
        assert_eq!(joined.pages.len(), 2);
        assert!(join(Vec::new(), "x").is_none());
        assert_eq!(Sent::Printed.key(), "print.sent.printed");
    }

    #[test]
    fn a_payment_stub_lists_bills_and_credits() {
        let body = r#"{"id":"00000000-0000-4000-8000-000000000001","txnDate":"2026-11-04","status":"posted",
          "billApplications":[{"billId":"00000000-0000-4000-8000-000000000002","amount":"100.00","discountAmount":"2.00",
             "bill":{"txnNumber":"B-7","vendorInvoiceNumber":"20931","txnDate":"2026-10-01","total":"102.00"}}],
          "creditApplications":[{"creditId":"00000000-0000-4000-8000-000000000003","billId":"00000000-0000-4000-8000-000000000002",
             "amount":"20.00","credit":{"vendorInvoiceNumber":"CM-1","txnDate":"2026-10-02","total":"20.00"}}]}"#;
        let r: BillPaymentRecord = serde_json::from_str(body).expect("record");
        let s = stub_of_payment(&r);
        assert_eq!(s.bills[0].vendor_invoice_no, "20931");
        assert_eq!(s.bills[0].amount_paid, m("100.00").as_decimal());
        assert_eq!(s.credits[0].ref_no, "CM-1");
        assert!(s.is_bill_payment());
    }
}
