//! The demo engine's form printing (P19): the check print queue (two
//! synthetic checks plus the demo's own Print later checks and bill
//! payments), numbers recorded at Print, requeue with spoiled numbers, the
//! check style per bank account, the logo, footers, document numbers and
//! the forms Print later queue. Every answer is immediate; the closing date
//! asks for the demo closing-date password as the engine would (U-106).

use std::collections::{BTreeMap, BTreeSet};

use cl_api::{
    ApiError, CheckLayout, DocType, DocumentNumber, FormFooters, LocalValidation, MarkedPrinted,
    PrintChecks, PrintedBatch, QueuedCheck, QueuedCheckKind, QueuedForm, RequeuedCheck,
    SpoiledNumber, SpoiledReason,
};
use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use serde_json::json;

use super::DemoState;

/// The demo's printing state.
#[derive(Clone, Debug, Default)]
pub struct DemoPrinting {
    layouts: BTreeMap<AccountId, CheckLayout>,
    /// Checks the print queue numbered: bank, number, batch.
    printed: BTreeMap<TransactionId, (AccountId, u64, uuid::Uuid)>,
    spoiled: Vec<SpoiledNumber>,
    next: BTreeMap<AccountId, u64>,
    footers: FormFooters,
    numbers: BTreeMap<DocType, (String, u64)>,
    /// The uploaded logo; `hidden` after Remove (Q-P19an).
    logo: Option<Vec<u8>>,
    /// Forms whose Print later mark was cleared.
    forms_printed: BTreeSet<TransactionId>,
    /// `POST /checks/print` calls that reached the demo (tests).
    pub print_calls: u32,
    /// `POST /checks/requeue` calls that reached the demo (tests).
    pub requeue_calls: u32,
}

/// The demo's closing-date password.
fn closing(state: &DemoState) -> Option<CalendarDate> {
    super::journal::closing(state)
}

fn refusal(status: u16, code: &str, message: &str) -> ApiError {
    cl_api::engine_refusal(
        status,
        &json!({ "error": { "message": message, "code": code } }),
    )
}

fn id(n: u128) -> TransactionId {
    TransactionId::from_uuid(uuid::Uuid::from_u128(
        0x5019_0000_0000_4000_8000_0000_0000_0000 | n,
    ))
}

fn d(y: i32, m: u32, day: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, day).unwrap_or_else(|_| DemoState::today())
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).unwrap_or(Money::ZERO)
}

/// The demo's checking account (the first bank account of the chart).
#[must_use]
pub fn demo_bank(state: &mut DemoState) -> Option<AccountId> {
    let chart = state.chart.get_or_insert_with(super::chart).clone();
    chart
        .iter()
        .find(|a| {
            crate::screens::register::class_of(a) == Some(cl_model::register::AccountClass::Bank)
        })
        .map(|a| a.id)
}

/// The two synthetic checks waiting to print (with their stub lines).
fn seeded(bank: AccountId) -> Vec<(QueuedCheck, Vec<(&'static str, &'static str)>)> {
    vec![
        (
            QueuedCheck {
                id: id(1),
                date: d(2026, 11, 4),
                amount: m("1286.40"),
                payee: "Feed & Seed Supply".into(),
                memo: "Acct 7712".into(),
                kind: QueuedCheckKind::Check,
                bank_account: Some(bank),
                address: vec!["1200 Commerce Dr".into(), "Aurora, MO 65605".into()],
            },
            vec![
                ("Materials:Plants", "842.15"),
                ("Materials:Mulch & stone", "444.25"),
            ],
        ),
        (
            QueuedCheck {
                id: id(2),
                date: d(2026, 11, 4),
                amount: m("318.72"),
                payee: "Ozark Electric Co-op".into(),
                memo: "November service".into(),
                kind: QueuedCheckKind::Check,
                bank_account: Some(bank),
                address: vec!["PO Box 1188".into(), "Springfield, MO 65801".into()],
            },
            vec![("Utilities", "318.72")],
        ),
    ]
}

/// `GET /checks/print-queue?bank_account_id=`.
#[must_use]
pub fn check_queue(state: &mut DemoState, bank: AccountId) -> Vec<QueuedCheck> {
    let printed: BTreeSet<TransactionId> = state.printing.printed.keys().copied().collect();
    let is_printed = |id: TransactionId| printed.contains(&id);
    let mut out: Vec<QueuedCheck> = Vec::new();
    if demo_bank(state) == Some(bank) {
        out.extend(
            seeded(bank)
                .into_iter()
                .map(|(c, _)| c)
                .filter(|c| !is_printed(c.id)),
        );
    }
    out.extend(super::checks::print_queue(state, bank, &is_printed));
    out.extend(super::pay_bills::print_queue(state, bank, &is_printed));
    out
}

/// What a check's voucher stub lists: bills paid for a bill payment, the
/// expense lines for a written check.
#[must_use]
pub fn stub(state: &mut DemoState, check: &QueuedCheck) -> cl_print::StubData {
    if let Some(bank) = check.bank_account
        && let Some((_, lines)) = seeded(bank).into_iter().find(|(c, _)| c.id == check.id)
    {
        return cl_print::StubData {
            lines: lines
                .into_iter()
                .map(|(name, amount)| cl_print::checks::StubLine {
                    name: name.into(),
                    amount: m(amount).as_decimal(),
                })
                .collect(),
            ..cl_print::StubData::default()
        };
    }
    if check.kind == QueuedCheckKind::BillPayment {
        return super::pay_bills::get(state, check.id)
            .map(|r| crate::screens::print_forms::job::stub_of_payment(&r))
            .unwrap_or_default();
    }
    cl_print::StubData {
        lines: super::checks::stub_lines(state, check.id)
            .into_iter()
            .map(|(name, amount)| cl_print::checks::StubLine {
                name,
                amount: amount.as_decimal(),
            })
            .collect(),
        ..cl_print::StubData::default()
    }
}

/// The next check number and the check style of `bank`.
#[must_use]
pub fn layout(state: &mut DemoState, bank: AccountId) -> (Option<u64>, CheckLayout) {
    let next = state.printing.next.get(&bank).copied().or_else(|| {
        let (_, n) = super::checks::numbers(state, bank);
        n.map(u64::from)
    });
    let layout = state
        .printing
        .layouts
        .get(&bank)
        .copied()
        .unwrap_or_default();
    (next.or(Some(1001)), layout)
}

/// `PUT /checks/settings` with one account's style.
pub fn set_layout(state: &mut DemoState, bank: AccountId, layout: CheckLayout) {
    state.printing.layouts.insert(
        bank,
        CheckLayout {
            own: true,
            ..layout
        },
    );
}

fn date_of(state: &mut DemoState, bank: AccountId, check: TransactionId) -> Option<CalendarDate> {
    check_queue(state, bank)
        .into_iter()
        .find(|c| c.id == check)
        .map(|c| c.date)
}

/// `POST /checks/print`.
///
/// # Errors
/// As the engine: 409 `CHECK_NUMBER_IN_USE`, 404 for a check not in the
/// queue, the closed period.
pub fn print(
    state: &mut DemoState,
    req: &PrintChecks,
    password: Option<&Secret>,
) -> Result<PrintedBatch, ApiError> {
    state.printing.print_calls += 1;
    if req.checks.is_empty() || req.starting_number == 0 {
        return Err(ApiError::Local(LocalValidation::InvalidValue));
    }
    let mut dates = Vec::new();
    for c in &req.checks {
        dates.push(
            date_of(state, req.bank_account, *c)
                .ok_or_else(|| refusal(404, "NOT_FOUND", "Check not found"))?,
        );
    }
    let last = req.starting_number + req.checks.len() as u64 - 1;
    let used = |n: u64| {
        state
            .printing
            .printed
            .values()
            .any(|(b, num, _)| *b == req.bank_account && *num == n)
    };
    if let Some(n) = (req.starting_number..=last).find(|n| used(*n)) {
        return Err(cl_api::engine_refusal(
            409,
            &json!({ "error": { "message": format!("Check #{n} already exists"),
                "code": "CHECK_NUMBER_IN_USE", "details": { "checkNumber": n.to_string() } } }),
        ));
    }
    super::register::check_closing(closing(state), &dates, password)?;
    let batch = uuid::Uuid::new_v4();
    for (i, c) in req.checks.iter().enumerate() {
        state.printing.printed.insert(
            *c,
            (req.bank_account, req.starting_number + i as u64, batch),
        );
    }
    let next = state
        .printing
        .next
        .entry(req.bank_account)
        .or_insert(last + 1);
    *next = (*next).max(last + 1);
    Ok(PrintedBatch {
        batch_id: batch,
        checks_printed: u32::try_from(req.checks.len()).unwrap_or(u32::MAX),
    })
}

/// `POST /checks/requeue`: the numbers are spoiled and the checks wait to
/// print again.
///
/// # Errors
/// 400 `CHECK_NOT_PRINTED` for a check the queue never numbered; the
/// closed period.
pub fn requeue(
    state: &mut DemoState,
    checks: &[TransactionId],
    dates: &[CalendarDate],
    password: Option<&Secret>,
) -> Result<Vec<RequeuedCheck>, ApiError> {
    state.printing.requeue_calls += 1;
    if checks.is_empty() {
        return Err(ApiError::Local(LocalValidation::InvalidValue));
    }
    if checks
        .iter()
        .any(|c| !state.printing.printed.contains_key(c))
    {
        return Err(refusal(400, "CHECK_NOT_PRINTED", "Check was not printed"));
    }
    super::register::check_closing(closing(state), dates, password)?;
    let mut out = Vec::new();
    for c in checks {
        if let Some((bank, number, _)) = state.printing.printed.remove(c) {
            state.printing.spoiled.push(SpoiledNumber {
                number,
                check: *c,
                bank_account: bank,
                reason: SpoiledReason::Requeue,
                date: dates.first().copied(),
            });
            out.push(RequeuedCheck {
                id: *c,
                spoiled_number: Some(number),
                bank_account: Some(bank),
            });
        }
    }
    Ok(out)
}

/// The number a printed check carries (demo).
#[must_use]
pub fn printed_number(state: &DemoState, check: TransactionId) -> Option<u64> {
    state.printing.printed.get(&check).map(|(_, n, _)| *n)
}

/// Spoiled numbers so far.
#[must_use]
pub fn spoiled(state: &DemoState) -> Vec<SpoiledNumber> {
    state.printing.spoiled.clone()
}

/// The logo bytes (none until one is uploaded).
#[must_use]
pub fn logo(state: &DemoState) -> Option<Vec<u8>> {
    state.printing.logo.clone()
}

/// Upload a logo.
pub fn set_logo(state: &mut DemoState, bytes: Option<Vec<u8>>) {
    state.printing.logo = bytes;
}

/// The footers (the engine's default until set).
#[must_use]
pub fn footers(state: &DemoState) -> FormFooters {
    let f = &state.printing.footers;
    let company = f
        .footer_text
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "Thank you for your business!".to_string());
    FormFooters {
        footer_text: f.footer_text.clone(),
        overrides: f.overrides.clone(),
        resolved: DocType::ALL
            .into_iter()
            .map(|t| {
                (
                    t,
                    f.overrides
                        .get(&t)
                        .filter(|o| !o.trim().is_empty())
                        .cloned()
                        .unwrap_or_else(|| company.clone()),
                )
            })
            .collect(),
    }
}

/// Save the footers.
pub fn set_footers(state: &mut DemoState, text: &str, overrides: &BTreeMap<DocType, String>) {
    state.printing.footers = FormFooters {
        footer_text: Some(text.trim().to_string()).filter(|t| !t.is_empty()),
        overrides: overrides
            .iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .map(|(k, v)| (*k, v.trim().to_string()))
            .collect(),
        resolved: BTreeMap::new(),
    };
}

/// Prefix and next number of every type.
#[must_use]
pub fn numbers(state: &DemoState) -> Vec<DocumentNumber> {
    DocType::ALL
        .into_iter()
        .map(|t| {
            let (prefix, next) =
                state
                    .printing
                    .numbers
                    .get(&t)
                    .cloned()
                    .unwrap_or_else(|| match t {
                        DocType::Invoice => ("INV-".to_string(), 1042),
                        _ => (String::new(), 1001),
                    });
            DocumentNumber {
                kind: t,
                next: format!("{prefix}{next}"),
                prefix,
                next_number: next,
            }
        })
        .collect()
}

/// Set one type's prefix and next number.
pub fn set_number(state: &mut DemoState, kind: DocType, prefix: &str, next: u64) {
    state
        .printing
        .numbers
        .insert(kind, (prefix.to_string(), next));
}

/// The synthetic forms marked Print later, and their lines for the page.
fn seeded_forms() -> Vec<QueuedForm> {
    vec![
        QueuedForm {
            id: id(11),
            kind: DocType::Invoice,
            number: Some("INV-1040".into()),
            date: d(2026, 11, 2),
            customer: Some("Hollister Family Trust".into()),
            total: Some(m("2450.00")),
        },
        QueuedForm {
            id: id(12),
            kind: DocType::Invoice,
            number: Some("INV-1041".into()),
            date: d(2026, 11, 3),
            customer: Some("Table Rock Garden Club".into()),
            total: Some(m("615.50")),
        },
        QueuedForm {
            id: id(13),
            kind: DocType::SalesReceipt,
            number: Some("1001".into()),
            date: d(2026, 11, 4),
            customer: Some("Walk-in customer".into()),
            total: Some(m("86.25")),
        },
    ]
}

/// `GET /forms/print-queue`.
#[must_use]
pub fn form_queue(state: &DemoState) -> Vec<QueuedForm> {
    seeded_forms()
        .into_iter()
        .filter(|f| !state.printing.forms_printed.contains(&f.id))
        .collect()
}

/// `POST /forms/mark-printed`.
pub fn mark_printed(state: &mut DemoState, ids: &[TransactionId]) -> MarkedPrinted {
    let mut r = MarkedPrinted::default();
    for i in ids {
        if state.printing.forms_printed.insert(*i) {
            r.marked += 1;
        } else {
            r.already_printed += 1;
        }
    }
    r
}

/// Put the demo's printing back (the scenarios run light, then dark).
pub fn reset(state: &mut DemoState) {
    state.printing = DemoPrinting::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_api::CheckFormat;

    fn bank(s: &mut DemoState) -> AccountId {
        demo_bank(s).expect("bank")
    }

    #[test]
    #[allow(clippy::many_single_char_names)]
    fn printing_records_numbers_and_a_requeue_spoils_a_reusable_one() {
        let mut s = DemoState::new();
        let b = bank(&mut s);
        let q = check_queue(&mut s, b);
        assert_eq!(q.len(), 2);
        let ids: Vec<_> = q.iter().map(|c| c.id).collect();
        let req = PrintChecks {
            bank_account: b,
            checks: ids.clone(),
            starting_number: 1001,
            format: CheckFormat::Voucher,
        };
        let batch = print(&mut s, &req, None).expect("print");
        assert_eq!(batch.checks_printed, 2);
        assert_eq!(check_queue(&mut s, b).len(), 0);
        assert_eq!(printed_number(&s, ids[1]), Some(1002));
        assert_eq!(layout(&mut s, b).0, Some(1003));
        let r = requeue(&mut s, &ids[1..], &[d(2026, 11, 4)], None).expect("requeue");
        assert_eq!(r[0].spoiled_number, Some(1002));
        assert_eq!(spoiled(&s)[0].number, 1002);
        assert_eq!(check_queue(&mut s, b).len(), 1);
        // 1002 may be used again.
        let again = PrintChecks {
            checks: ids[1..].to_vec(),
            starting_number: 1002,
            ..req.clone()
        };
        print(&mut s, &again, None).expect("reuse");
        let e = print(
            &mut s,
            &PrintChecks {
                checks: vec![ids[0]],
                ..req
            },
            None,
        )
        .expect_err("printed already");
        assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
        assert!(requeue(&mut s, &[id(99)], &[], None).is_err());
    }

    #[test]
    #[allow(clippy::many_single_char_names)]
    fn stubs_add_up_to_the_check_and_settings_round_trip() {
        let mut s = DemoState::new();
        let b = bank(&mut s);
        for c in check_queue(&mut s, b) {
            let st = stub(&mut s, &c);
            assert_eq!(st.totals().expect("t").lines, c.amount.as_decimal());
        }
        assert_eq!(layout(&mut s, b).1.format, CheckFormat::Voucher);
        set_layout(
            &mut s,
            b,
            CheckLayout {
                format: CheckFormat::Standard,
                print_company_block: true,
                own: false,
            },
        );
        assert!(layout(&mut s, b).1.own);
        let mut o = BTreeMap::new();
        o.insert(DocType::Invoice, "Net 30".to_string());
        set_footers(&mut s, "Thanks", &o);
        let f = footers(&s);
        assert_eq!(f.resolved_for(DocType::Invoice), "Net 30");
        assert_eq!(f.resolved_for(DocType::Statement), "Thanks");
        set_number(&mut s, DocType::SalesReceipt, "SR-", 2001);
        assert_eq!(numbers(&s)[1].next, "SR-2001");
        let q = form_queue(&s);
        assert_eq!(q.len(), 3);
        let r = mark_printed(&mut s, &[q[0].id, q[0].id]);
        assert_eq!((r.marked, r.already_printed), (1, 1));
        assert_eq!(form_queue(&s).len(), 2);
    }
}
