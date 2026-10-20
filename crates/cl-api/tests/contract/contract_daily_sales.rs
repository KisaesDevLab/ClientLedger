//! Daily Sales Summary against the real engine (P14 WP-B1.3, plan §6.2).
//!
//! - `over_short`: the plan's day (sales 1,000.00, tax 80.00, Cash 500.00,
//!   Cards 577.65) previews **2.35 short** and posts Cr income 1,000.00,
//!   Cr sales tax 80.00, Dr Payments Clearing 1,077.65 and Dr Cash
//!   Over/Short 2.35, balanced; Cash 503.00 posts **0.65 over** (Cr); an
//!   unmapped line with an amount is refused; void reverses the posting.
//! - `preview_property`: 64 days of random cents; the engine's preview
//!   over/short equals Σdebit-side − Σcredit-side computed here exactly
//!   (the cl-model twin's rule).
//! - `closed_period`: a day on or before the closing date is refused with
//!   423; with `daily-sales-lock-v1` the closing-date password posts and
//!   voids it (PENDING until the pin bump).

use cl_api::{
    AccountType, ApiError, Capabilities, ClosedPeriodAuthorization, DailyEntryDraft,
    DailyEntryQuery, DailyLineInput, DailyPostOptions, DailyTemplate, DailyValue, EntryStatus,
    Method, NormalSide, Preset, Section, Session, TxnStatus,
};
use cl_core::ids::{AccountId, DailyLineId};
use cl_core::{CalendarDate, Money, Secret};
use serde_json::json;

use crate::common::{account, date, key, m};
use crate::support::{Harness, unique};

/// A layout with: Plant sales (mapped to a fresh income account), Dept 2
/// (unmapped), Sales Tax Collected, Cash and Cards (the engine's own
/// accounts from the retail preset).
struct Layout {
    t: DailyTemplate,
    income: AccountId,
    tax_account: AccountId,
    clearing: AccountId,
    sales: DailyLineId,
    dept2: DailyLineId,
    tax: DailyLineId,
    cash: DailyLineId,
    cards: DailyLineId,
}

fn layout(h: &Harness, s: &Session) -> Layout {
    let ds = s.daily_sales();
    let made = h
        .run(ds.create_template(&unique("Plant sale counter"), Some(Preset::Retail)))
        .unwrap_or_else(|e| panic!("create layout: {e}"));
    let made = h.run(ds.template(made.id)).expect("layout");
    let find = |sec: Section| {
        made.lines
            .iter()
            .find(|l| l.section == sec && l.account.is_some())
            .unwrap_or_else(|| panic!("preset line {sec:?}"))
    };
    let tax = find(Section::Tax);
    let tender = find(Section::Payment);
    let (tax_account, clearing) = (tax.account.expect("tax"), tender.account.expect("tender"));
    let income = account(h, s, AccountType::Revenue, "Plant sales").id;
    let line =
        |section: Section, label: &str, acct: Option<AccountId>, side: NormalSide| DailyLineInput {
            id: None,
            section,
            label: label.into(),
            account: acct,
            tag: None,
            side,
            required: false,
            active: true,
        };
    let t = h
        .run(ds.replace_lines(
            made.id,
            &[
                line(
                    Section::Sales,
                    "Plant sales",
                    Some(income),
                    NormalSide::Credit,
                ),
                line(Section::Sales, "Dept 2", None, NormalSide::Credit),
                line(
                    Section::Tax,
                    "Sales tax collected",
                    Some(tax_account),
                    NormalSide::Credit,
                ),
                line(Section::Payment, "Cash", Some(clearing), NormalSide::Debit),
                line(Section::Payment, "Cards", Some(clearing), NormalSide::Debit),
            ],
        ))
        .unwrap_or_else(|e| panic!("replace lines: {e}"));
    let active: Vec<_> = t.lines.iter().filter(|l| l.active).collect();
    assert_eq!(active.len(), 5, "the preset's other lines are removed");
    let id = |label: &str| {
        active
            .iter()
            .find(|l| l.label == label)
            .unwrap_or_else(|| panic!("line {label}"))
            .id
    };
    Layout {
        income,
        tax_account,
        clearing,
        sales: id("Plant sales"),
        dept2: id("Dept 2"),
        tax: id("Sales tax collected"),
        cash: id("Cash"),
        cards: id("Cards"),
        t,
    }
}

fn v(line: DailyLineId, amount: &str) -> DailyValue {
    DailyValue {
        line,
        amount: m(amount),
        tag: None,
    }
}

fn day(l: &Layout, on: CalendarDate, cash: &str) -> DailyEntryDraft {
    DailyEntryDraft {
        template: l.t.id,
        date: on,
        tag: None,
        notes: Some(unique("register Z")),
        values: vec![
            v(l.sales, "1000.00"),
            v(l.tax, "80.00"),
            v(l.cash, cash),
            v(l.cards, "577.65"),
        ],
    }
}

/// Σ debit-side − Σ credit-side over lines with an account (the twin).
fn local_over_short(t: &DailyTemplate, values: &[DailyValue]) -> Money {
    let mut out = Money::ZERO;
    for val in values {
        let Some(line) = t.lines.iter().find(|l| l.id == val.line) else {
            continue;
        };
        if !line.active || line.account.is_none() {
            continue;
        }
        out = match line.side {
            NormalSide::Debit => out.checked_add(val.amount),
            NormalSide::Credit => out.checked_sub(val.amount),
        }
        .expect("in range");
    }
    out
}

/// Σ debit and Σ credit on `account` of a posted transaction.
fn on_account(
    h: &Harness,
    s: &Session,
    txn: cl_core::ids::TransactionId,
    a: AccountId,
) -> (Money, Money) {
    let t = h.run(s.transactions().get(txn)).expect("txn").record;
    let lines = t.lines.iter().filter(|l| l.account_id == a);
    let dr = Money::sum(lines.clone().map(|l| l.debit)).expect("sum");
    let cr = Money::sum(lines.map(|l| l.credit)).expect("sum");
    (dr, cr)
}

fn post(h: &Harness, s: &Session, d: &DailyEntryDraft) -> cl_api::DailyEntry {
    let ds = s.daily_sales();
    let draft = h
        .run(ds.create_draft(d, key()))
        .unwrap_or_else(|e| panic!("draft: {e}"));
    assert_eq!(draft.status, EntryStatus::Draft);
    h.run(ds.post(draft.id, &DailyPostOptions::default(), key()))
        .unwrap_or_else(|e| panic!("post: {e}"))
}

/// The over/short of the plan's day, short and over, posted exactly.
pub fn over_short(h: &Harness) {
    let s = &h.s1;
    let ds = s.daily_sales();
    let l = layout(h, s);
    let on = date(2026, 11, 4);

    // Short: credits 1,080.00, debits 1,077.65.
    let short = day(&l, on, "500.00");
    let p = h.run(ds.preview(l.t.id, &short.values)).expect("preview");
    assert_eq!(p.over_short.to_wire(), "-2.3500");
    assert_eq!(p.over_short, local_over_short(&l.t, &short.values));
    assert_eq!((p.debits, p.credits), (m("1077.65"), m("1080.00")));
    assert_eq!(
        (p.sales, p.tax, p.payments),
        (m("1000"), m("80"), m("1077.65"))
    );
    assert!(!p.balanced && p.unmapped.is_empty());

    let posted = post(h, s, &short);
    assert_eq!(posted.status, EntryStatus::Posted);
    assert_eq!(posted.over_short, m("-2.35"));
    let txn = posted.transaction.expect("journal entry");
    assert_eq!(on_account(h, s, txn, l.income), (Money::ZERO, m("1000")));
    assert_eq!(on_account(h, s, txn, l.tax_account), (Money::ZERO, m("80")));
    assert_eq!(
        on_account(h, s, txn, l.clearing),
        (m("1077.65"), Money::ZERO)
    );
    let record = h.run(s.transactions().get(txn)).expect("txn").record;
    let others: Vec<_> = record
        .lines
        .iter()
        .filter(|x| ![l.income, l.tax_account, l.clearing].contains(&x.account_id))
        .collect();
    assert_eq!(others.len(), 1, "one over/short line");
    assert_eq!(
        (others[0].debit, others[0].credit),
        (m("2.35"), Money::ZERO)
    );
    let over_short_account = others[0].account_id;
    let a = h
        .run(s.accounts().get(over_short_account))
        .expect("acct")
        .record;
    assert_eq!(a.system_tag.as_deref(), Some("cash_over_short"));
    let dr = Money::sum(record.lines.iter().map(|x| x.debit)).expect("dr");
    let cr = Money::sum(record.lines.iter().map(|x| x.credit)).expect("cr");
    assert_eq!(dr, cr, "the posting balances");

    // The tenders wait in Payments Clearing.
    if h.require_cap(Capabilities::DEPOSIT_GROUPING, "U-52 deposit-grouping-v1") {
        let r = h
            .run(s.raw(
                Method::Get,
                "/api/v1/payments/pending-deposits",
                &[("exact", "1")],
                None,
            ))
            .expect("pending");
        let text = r.body.to_string();
        assert!(
            text.contains(&txn.to_string()),
            "the day's tenders are pending"
        );
    }

    // Over: Cash 503.00 → Cr 0.65.
    let over = day(&l, on, "503.00");
    let p = h.run(ds.preview(l.t.id, &over.values)).expect("preview");
    assert_eq!(p.over_short.to_wire(), "0.6500");
    let posted_over = post(h, s, &over);
    let txn_over = posted_over.transaction.expect("txn");
    assert_eq!(
        on_account(h, s, txn_over, over_short_account),
        (Money::ZERO, m("0.65"))
    );

    // An unmapped line with an amount: the preview names it, post refuses.
    let mut unmapped = day(&l, on, "500.00");
    unmapped.values.push(v(l.dept2, "10.00"));
    let p = h
        .run(ds.preview(l.t.id, &unmapped.values))
        .expect("preview");
    assert_eq!(p.unmapped, vec!["Dept 2".to_string()]);
    let draft = h.run(ds.create_draft(&unmapped, key())).expect("draft");
    let e = h
        .run(ds.post(draft.id, &DailyPostOptions::default(), key()))
        .expect_err("unmapped");
    assert!(matches!(e, ApiError::Validation(_)), "{e:?}");
    // The draft is still there (a failed post keeps it, Q-P14u).
    let drafts = h
        .run(ds.list(&DailyEntryQuery {
            status: Some(EntryStatus::Draft),
            template: Some(l.t.id),
            date: Some(on),
            limit: None,
        }))
        .expect("drafts");
    assert!(drafts.iter().any(|d| d.id == draft.id));
    // A draft can be updated, then deleted.
    let fixed = h
        .run(ds.update_draft(draft.id, &day(&l, on, "500.00"), key()))
        .expect("update");
    assert_eq!(fixed.values.len(), 4);
    h.run(ds.delete_draft(draft.id)).expect("delete");
    assert!(matches!(
        h.run(ds.entry(draft.id)),
        Err(ApiError::NotFound { .. })
    ));

    // Void reverses the posting.
    let voided = h
        .run(ds.void(posted.id, &DailyPostOptions::default(), key()))
        .expect("void");
    assert_eq!(voided.status, EntryStatus::Void);
    let t = h.run(s.transactions().get(txn)).expect("txn").record;
    assert_eq!(t.status, TxnStatus::Void);
    h.run(ds.void(posted_over.id, &DailyPostOptions::default(), key()))
        .expect("void over");
    h.run(ds.delete_template(l.t.id)).expect("delete layout");
    let left = h.run(ds.templates()).expect("layouts");
    assert!(left.iter().all(|t| t.id != l.t.id));
}

/// 64 random days: the engine preview equals the exact local figure.
pub fn preview_property(h: &Harness) {
    let s = &h.s1;
    let ds = s.daily_sales();
    let l = layout(h, s);
    let mut seed: u64 = 0x5eed_da11;
    let mut cents = || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // Up to 99,999,999.99, sometimes zero.
        let c = i64::try_from((seed >> 20) % 10_000_000_000).expect("fits");
        if c % 7 == 0 { 0 } else { c }
    };
    for _ in 0..64 {
        let values: Vec<DailyValue> = [l.sales, l.tax, l.cash, l.cards]
            .into_iter()
            .map(|line| DailyValue {
                line,
                amount: Money::from_cents(cents()).expect("cents"),
                tag: None,
            })
            .collect();
        let p = h.run(ds.preview(l.t.id, &values)).expect("preview");
        assert_eq!(p.over_short, local_over_short(&l.t, &values));
        assert_eq!(p.debits.checked_sub(p.credits).expect("sub"), p.over_short);
    }
    h.run(ds.delete_template(l.t.id)).expect("delete layout");
}

/// A day in a closed period (second company).
pub fn closed_period(h: &Harness) {
    let s = h.s3();
    let ds = s.daily_sales();
    let l = layout(h, &s);
    let settings = |lock: serde_json::Value| {
        h.run(s.raw_mapped(
            Method::Put,
            "/api/v1/company/settings",
            Some(&json!({ "lockDate": lock })),
        ))
        .expect("closing date");
    };
    settings(json!("2026-09-30"));
    let closed = day(&l, date(2026, 9, 15), "500.00");
    let draft = h.run(ds.create_draft(&closed, key())).expect("draft");
    let posted = h.run(ds.post(draft.id, &DailyPostOptions::default(), key()));
    match posted {
        Err(ApiError::ClosedPeriod(c)) => {
            assert_eq!(c.lock_date, Some(date(2026, 9, 30)));
            if h.require_cap(Capabilities::DAILY_SALES_LOCK, "U-57 daily-sales-lock-v1") {
                let pw = Secret::new(unique("closing-daily"));
                assert!(
                    h.run(s.set_closing_password(None, Some(&pw), key()))
                        .expect("set password")
                );
                let with = |p: &Secret| DailyPostOptions {
                    closed_period: Some(ClosedPeriodAuthorization::with_password(p.clone())),
                };
                let e = h
                    .run(ds.post(draft.id, &with(&Secret::new("not-it".to_string())), key()))
                    .expect_err("wrong password");
                assert_eq!(e, ApiError::ClosingPasswordRejected);
                let done = h.run(ds.post(draft.id, &with(&pw), key())).expect("posted");
                let e = h
                    .run(ds.void(done.id, &DailyPostOptions::default(), key()))
                    .expect_err("closed void");
                assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
                h.run(ds.void(done.id, &with(&pw), key())).expect("void");
                assert!(
                    !h.run(s.set_closing_password(Some(&pw), None, key()))
                        .expect("clear password")
                );
            } else {
                h.run(ds.delete_draft(draft.id)).expect("delete");
            }
        }
        Ok(day) => {
            // E3: the engine posted a day in a closed period.
            h.pending(
                "E3",
                "daily-sales post does not enforce the closing date at this pin",
            );
            settings(serde_json::Value::Null);
            h.run(ds.void(day.id, &DailyPostOptions::default(), key()))
                .expect("void");
        }
        Err(e) => panic!("closed-period post: {e:?}"),
    }
    settings(serde_json::Value::Null);
    h.run(ds.delete_template(l.t.id)).expect("delete layout");
}
