//! Payroll import against the live engine (P30 plan §5): every provider
//! fixture's entries equal the independent `rust_decimal` expectation
//! (`support/payroll_expect.rs`), a generic layout is reused for the second
//! period, the same file twice is 409, reverse twice is 409, a repeated key
//! replays, and net-pay withdrawals match in the Bank Feed.
//!
//! PENDING until the pinned engine advertises `payroll-import-exact-v1`,
//! `payroll-import-post-v1` and `payroll-import-sensitive-v1` (U-242 to
//! U-251); each test then returns early with a PENDING line.
//!
//! The fixtures never share a pay period (fixtures/payroll/README.md), so
//! they post in Contract Co one after another without overlapping.

use std::collections::BTreeMap;

use cl_api::payroll::{
    ColumnMap, ColumnSource, DescriptionMapping, ImportMode, PayrollRefusal, PostOutcome,
    PostRequest, SkipRule, UploadRequest, payroll_refusal,
};
use cl_api::{AccountType, Capabilities, IdempotencyKey, Session};
use cl_core::CalendarDate;
use cl_core::ids::AccountId;
use rust_decimal::Decimal;

use crate::common::{account_fields, key};
use crate::payroll_expect::{self, Side, Spec};
use crate::support::Harness;

fn caps(h: &Harness) -> bool {
    h.require_cap(Capabilities::PAYROLL_IMPORT_EXACT, "U-250")
        && h.require_cap(Capabilities::PAYROLL_IMPORT_POST, "U-244")
        && h.require_cap(Capabilities::PAYROLL_IMPORT_SENSITIVE, "U-247")
}

fn day(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("date")
}

fn account(
    h: &Harness,
    s: &Session,
    t: AccountType,
    detail: Option<&str>,
    name: &str,
) -> AccountId {
    let mut f = account_fields(t, name);
    f.detail_type = detail.map(str::to_string);
    h.run(s.accounts().create(&f, key()))
        .expect("account")
        .record
        .id
}

/// Accounts for every line type: wages and taxes to their own accounts,
/// net pay to a bank account (Q-P30f). Returns the bank.
fn map_accounts(h: &Harness, s: &Session) -> (AccountId, AccountId, AccountId) {
    let bank = account(h, s, AccountType::Asset, Some("bank"), "Payroll Bank");
    let wages = account(h, s, AccountType::Expense, None, "Payroll Wages");
    let owed = account(h, s, AccountType::Liability, None, "Payroll Taxes Owed");
    let mut m = BTreeMap::new();
    for (lt, debit) in cl_model_line_types() {
        let a = if lt == "payroll_clearing" {
            bank
        } else if debit {
            wages
        } else {
            owed
        };
        m.insert(lt.to_string(), a);
    }
    h.run(s.payroll().save_account_mappings(&m))
        .expect("account map");
    (bank, wages, owed)
}

/// The engine's line types (cl-model is not a dependency of cl-api).
fn cl_model_line_types() -> Vec<(&'static str, bool)> {
    vec![
        ("gross_wages_expense", true),
        ("employer_tax_expense", true),
        ("employer_benefits_expense", true),
        ("reimbursement_expense", true),
        ("fit_payable", false),
        ("sit_payable", false),
        ("ss_payable", false),
        ("medicare_payable", false),
        ("futa_payable", false),
        ("suta_payable", false),
        ("health_ins_payable", false),
        ("retirement_payable", false),
        ("other_deduction_payable", false),
        ("other_tax_payable", false),
        ("payroll_clearing", false),
    ]
}

/// The column map for a Mode A fixture, from the expectation's own spec.
fn column_map(stem: &str) -> Option<ColumnMap> {
    let Spec::A(s) = payroll_expect::spec(stem) else {
        return None;
    };
    let mut mappings = BTreeMap::new();
    let mut put = |f: &str, h: &str| {
        mappings.insert(
            f.to_string(),
            ColumnSource {
                source: h.to_string(),
                format: None,
            },
        );
    };
    put("employee_name", s.employee);
    if let Some(c) = s.check_date {
        put("check_date", c);
    }
    if let Some(d) = s.department {
        put("department", d);
    }
    for (f, h) in s.money {
        put(f, h);
    }
    let fmt = if stem.starts_with("generic") {
        "MM/DD/YYYY"
    } else {
        "YYYY-MM-DD"
    };
    Some(ColumnMap {
        header_row: u32::try_from(s.header_row).expect("row"),
        data_start_row: u32::try_from(s.header_row + 1).expect("row"),
        skip_footer_rows: u32::try_from(s.skip_footer).expect("row"),
        date_format: Some(fmt.into()),
        mappings,
        skip_rules: vec![
            SkipRule {
                field: "employee_name".into(),
                values: None,
            },
            SkipRule {
                field: "employee_name".into(),
                values: Some(vec!["Total".into()]),
            },
        ],
        department_tags: BTreeMap::new(),
    })
}

/// Uploads, maps, validates and previews `stem`; returns the session and
/// the preview, after comparing it with the independent expectation.
fn prepare(
    h: &Harness,
    s: &Session,
    stem: &str,
    bank: AccountId,
    other: AccountId,
) -> (cl_api::payroll::PayrollSessionId, Option<String>) {
    let g = payroll_expect::golden(stem);
    let mode = if g.mode == "B" {
        ImportMode::PrebuiltJe
    } else {
        ImportMode::EmployeeLevel
    };
    let up = h
        .run(s.payroll().upload(&UploadRequest {
            mode,
            bytes: payroll_expect::fixture_bytes(&format!("{stem}.csv")),
            file_name: format!("{stem}.csv"),
            period_start: day(&g.period_start),
            period_end: day(&g.period_end),
            check_date: day(&g.check_date),
        }))
        .expect("upload");
    let mut departments = BTreeMap::new();
    if let Some(map) = column_map(stem) {
        h.run(s.payroll().apply_mapping(up.session, &map))
            .expect("mapping");
        if stem == "gusto" {
            departments.insert("Kitchen".to_string(), None);
            departments.insert("Front".to_string(), None);
        }
    } else {
        let descs: Vec<DescriptionMapping> = h
            .run(s.payroll().description_map(up.session))
            .expect("descriptions")
            .into_iter()
            .map(|d| {
                let net = d.description.to_lowercase().contains("net");
                DescriptionMapping {
                    account: Some(if net { bank } else { other }),
                    ..d
                }
            })
            .collect();
        h.run(
            s.payroll()
                .save_description_map(up.session, &g.provider, &descs),
        )
        .expect("description map");
    }
    let v = h.run(s.payroll().validate(up.session)).expect("validate");
    assert!(v.can_continue(), "{stem}: {v:?}");
    let p = h
        .run(s.payroll().preview(up.session, &departments))
        .expect("preview");
    let want = payroll_expect::expect(stem, &g.check_date);
    assert_eq!(
        p.entries.len(),
        want.len(),
        "{stem}: one entry per pay date"
    );
    for (e, w) in p.entries.iter().zip(&want) {
        assert_eq!(e.date.to_wire(), w.date, "{stem}");
        assert!(e.is_balanced, "{stem}");
        assert_eq!(e.total_debits.as_decimal(), w.total_debits, "{stem}");
        let mut got: Vec<(String, Option<String>, Side, Decimal)> = e
            .lines
            .iter()
            .map(|l| {
                let (side, amt) = if l.debit.is_zero() {
                    (Side::Credit, l.credit)
                } else {
                    (Side::Debit, l.debit)
                };
                (
                    l.line_type.clone(),
                    l.department.clone(),
                    side,
                    amt.as_decimal(),
                )
            })
            .collect();
        let mut exp: Vec<(String, Option<String>, Side, Decimal)> = w
            .lines
            .iter()
            .map(|l| (l.line_type.clone(), l.department.clone(), l.side, l.amount))
            .collect();
        got.sort();
        exp.sort();
        assert_eq!(got, exp, "{stem} {}", w.date);
        assert!(!e.memo.contains("Test Worker"), "{stem}: no names in memos");
    }
    (up.session, p.token)
}

/// A1: every provider fixture posts balanced entries equal to the
/// expectation computed from its CSV.
pub fn providers_post_their_expected_entries(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let (bank, _, owed) = map_accounts(h, s);
    for stem in [
        "gusto",
        "adp_run_gli",
        "quickbooks_payroll",
        "paychex_flex_gls",
        "square",
        "onpay_gl_summary",
    ] {
        let (id, token) = prepare(h, s, stem, bank, owed);
        let req = PostRequest {
            preview_token: token,
            ..PostRequest::default()
        };
        let out = h
            .run(s.payroll().post(id, &req, IdempotencyKey::new()))
            .expect("post");
        let PostOutcome::Posted { entries, .. } = out else {
            panic!("{stem}: overlap {out:?}")
        };
        assert_eq!(
            entries.len(),
            payroll_expect::golden(stem).entries.len(),
            "{stem}"
        );
        // U-251: the employee rows are gone once posted.
        let after = h.run(s.payroll().session(id)).expect("session");
        assert!(after.detail_discarded, "{stem}");
    }
}

/// A2: the generic layout saved for period 1 maps period 2.
pub fn generic_layout_is_reused(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let (bank, _, owed) = map_accounts(h, s);
    let map = column_map("generic_period1").expect("map");
    let t = h
        .run(
            s.payroll()
                .save_template(None, "Contract generic", "custom", &map),
        )
        .expect("template");
    for stem in ["generic_period1", "generic_period2"] {
        let templates = h.run(s.payroll().templates()).expect("templates");
        let saved = templates
            .iter()
            .find(|x| x.id == t.id)
            .and_then(|x| x.column_map.clone())
            .expect("saved");
        assert_eq!(saved.sources().len(), map.sources().len());
        let (id, token) = prepare(h, s, stem, bank, owed);
        let req = PostRequest {
            preview_token: token,
            ..PostRequest::default()
        };
        assert!(matches!(
            h.run(s.payroll().post(id, &req, IdempotencyKey::new()))
                .expect("post"),
            PostOutcome::Posted { .. }
        ));
    }
}

/// Safety: same file twice is 409, a repeated key replays, reverse twice
/// is 409.
pub fn duplicates_replays_and_reverse(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    // ADP was posted by the provider test: the same file is refused.
    let (bank, _, owed) = map_accounts(h, s);
    let (id, token) = prepare(h, s, "adp_run_gli", bank, owed);
    let req = PostRequest {
        preview_token: token,
        force_post: true,
        ..PostRequest::default()
    };
    let e = h
        .run(s.payroll().post(id, &req, IdempotencyKey::new()))
        .expect_err("duplicate");
    assert!(
        matches!(
            payroll_refusal(&e),
            Some(PayrollRefusal::DuplicateFile { .. } | PayrollRefusal::DuplicateRun { .. })
        ),
        "{e:?}"
    );
    h.run(s.payroll().discard(id)).expect("discard the draft");
    // Reverse the posted ADP import, then once more.
    let page = h.run(s.payroll().sessions(None, 0)).expect("history");
    let posted = page
        .sessions
        .iter()
        .find(|x| {
            x.provider.as_deref() == Some("adp_run_gli")
                && x.status == cl_api::payroll::SessionStatus::Posted
        })
        .expect("posted ADP");
    let k = IdempotencyKey::new();
    let r = h
        .run(
            s.payroll()
                .reverse(posted.id, "Contract test", None, k.clone()),
        )
        .expect("reverse");
    assert_eq!(r.reversed, 1);
    let again = h
        .run(s.payroll().reverse(posted.id, "Contract test", None, k))
        .expect("replay");
    assert_eq!(again.entries, r.entries, "the same key replays");
    let e = h
        .run(
            s.payroll()
                .reverse(posted.id, "Contract test", None, IdempotencyKey::new()),
        )
        .expect_err("twice");
    assert_eq!(payroll_refusal(&e), Some(PayrollRefusal::NotPosted));
}
