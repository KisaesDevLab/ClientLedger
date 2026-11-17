//! Demo payroll parity (P30 WP7): every fixture's demo preview equals its
//! golden, and the demo refuses as the engine does.

use super::*;
use cl_api::payroll::{PayrollRefusal, payroll_refusal};

fn day(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("date")
}

fn acct(n: u8) -> AccountId {
    AccountId::from_uuid(uuid::Uuid::from_u128(0x5a00 + u128::from(n)))
}

/// Uploads `stem` with its golden's period and maps every line.
fn ready(d: &mut DemoPayroll, stem: &str) -> PayrollSessionId {
    let f = FIXTURES.iter().find(|f| f.stem == stem).expect("fixture");
    let g: Value = serde_json::from_str(f.golden).expect("golden");
    let date = |k: &str| day(g[k].as_str().expect("date"));
    let up = d
        .upload(&UploadRequest {
            mode: f.mode,
            bytes: f.bytes.to_vec(),
            file_name: format!("{stem}.csv"),
            period_start: date("periodStart"),
            period_end: date("periodEnd"),
            check_date: date("checkDate"),
        })
        .expect("upload");
    let mut map = BTreeMap::new();
    let mut descs = Vec::new();
    for (i, lt) in cl_model::payroll_import::LINE_TYPES.iter().enumerate() {
        map.insert(lt.0.to_string(), acct(u8::try_from(i).expect("small")));
    }
    d.save_account_mappings(&map);
    if f.mode == ImportMode::PrebuiltJe {
        for m in d.description_map(up.session).expect("descriptions") {
            descs.push(DescriptionMapping {
                account: Some(acct(99)),
                ..m
            });
        }
        let provider = golden_of(f).0;
        d.save_description_map(&provider, &descs);
    }
    d.validate(up.session).expect("validate");
    up.session
}

#[test]
fn every_demo_preview_equals_its_golden() {
    let mut d = DemoPayroll::default();
    for f in &FIXTURES {
        let id = ready(&mut d, f.stem);
        let mut depts = BTreeMap::new();
        if f.stem == "gusto" {
            depts.insert("Kitchen".to_string(), None);
            depts.insert("Front".to_string(), None);
        }
        let p = d.preview(id, &depts, &|_| None).expect("preview");
        let (_, golden) = golden_of(f);
        assert_eq!(p.entries.len(), golden.len(), "{}", f.stem);
        for (e, g) in p.entries.iter().zip(&golden) {
            assert_eq!(e.date, g.date);
            assert!(e.is_balanced);
            assert_eq!(e.total_debits, g.total);
            let mut got: Vec<(String, Option<String>, Money, Money)> = e
                .lines
                .iter()
                .map(|l| (l.line_type.clone(), l.department.clone(), l.debit, l.credit))
                .collect();
            let mut want: Vec<(String, Option<String>, Money, Money)> = g
                .lines
                .iter()
                .map(|l| {
                    let (dr, cr) = if l.debit {
                        (l.amount, Money::ZERO)
                    } else {
                        (Money::ZERO, l.amount)
                    };
                    (l.line_type.clone(), l.department.clone(), dr, cr)
                })
                .collect();
            got.sort();
            want.sort();
            assert_eq!(got, want, "{} {}", f.stem, g.date.to_wire());
            assert!(!e.memo.contains("Test Worker"));
        }
    }
}

#[test]
fn without_a_department_map_net_pay_and_expenses_are_one_line_each() {
    let mut d = DemoPayroll::default();
    let id = ready(&mut d, "gusto");
    let p = d.preview(id, &BTreeMap::new(), &|_| None).expect("preview");
    for e in &p.entries {
        let wages: Vec<_> = e
            .lines
            .iter()
            .filter(|l| l.line_type == "gross_wages_expense")
            .collect();
        assert_eq!(wages.len(), 1);
        assert!(wages[0].department.is_none());
        let debits = e
            .lines
            .iter()
            .try_fold(Money::ZERO, |a, l| a.checked_add(l.debit))
            .expect("sum");
        assert_eq!(debits, e.total_debits, "merging keeps the totals");
    }
}

#[test]
fn the_demo_refuses_as_the_engine_does() {
    let mut d = DemoPayroll::default();
    let first = ready(&mut d, "adp_run_gli");
    let key = IdempotencyKey::new();
    let posted = d.post(first, false, &key, None, None).expect("post");
    assert!(matches!(
        posted,
        PostOutcome::Posted {
            replayed: false,
            ..
        }
    ));
    // The same key replays; nothing posts twice.
    let again = d.post(first, false, &key, None, None).expect("replay");
    assert!(matches!(again, PostOutcome::Posted { replayed: true, .. }));
    // A new key on a posted import.
    let e = d
        .post(first, false, &IdempotencyKey::new(), None, None)
        .expect_err("posted");
    assert_eq!(payroll_refusal(&e), Some(PayrollRefusal::AlreadyPosted));
    // The same file again.
    let second = ready(&mut d, "adp_run_gli");
    let e = d
        .post(second, true, &IdempotencyKey::new(), None, None)
        .expect_err("duplicate");
    assert!(matches!(
        payroll_refusal(&e),
        Some(PayrollRefusal::DuplicateFile { .. })
    ));
    // Reverse twice.
    let k = IdempotencyKey::new();
    d.reverse(first, &k, None, None).expect("reverse");
    let e = d
        .reverse(first, &IdempotencyKey::new(), None, None)
        .expect_err("twice");
    assert_eq!(payroll_refusal(&e), Some(PayrollRefusal::NotPosted));
}

#[test]
fn overlap_asks_first_and_a_closed_period_needs_the_password() {
    let mut d = DemoPayroll::default();
    let p1 = ready(&mut d, "generic_period1");
    d.post(p1, false, &IdempotencyKey::new(), None, None)
        .expect("post");
    // Same period typed for another file: overlap.
    let p2 = ready(&mut d, "generic_period2");
    d.sessions
        .iter_mut()
        .find(|s| s.id == p2)
        .expect("s")
        .period = (day("2025-06-28"), day("2025-07-11"));
    let o = d
        .post(p2, false, &IdempotencyKey::new(), None, None)
        .expect("asks");
    assert!(matches!(o, PostOutcome::NeedsConfirmation { ref overlaps } if overlaps.len() == 1));
    let closing = Some(day("2025-12-31"));
    let e = d
        .post(p2, true, &IdempotencyKey::new(), None, closing)
        .expect_err("locked");
    assert!(matches!(e, ApiError::ClosedPeriod(_)));
    let wrong = Secret::new("nope");
    let e = d
        .post(p2, true, &IdempotencyKey::new(), Some(&wrong), closing)
        .expect_err("wrong");
    assert!(matches!(e, ApiError::ClosingPasswordRejected));
    let right = Secret::new(super::super::register::DEMO_CLOSING_PASSWORD);
    let ok = d
        .post(p2, true, &IdempotencyKey::new(), Some(&right), closing)
        .expect("posted");
    assert!(matches!(ok, PostOutcome::Posted { .. }));
    assert_eq!(d.sessions(None).sessions.len(), 2);
    assert_eq!(d.unposted(), 0);
}

#[test]
fn an_unknown_file_and_an_unmapped_line_are_refused() {
    let mut d = DemoPayroll::default();
    let e = d
        .upload(&UploadRequest {
            mode: ImportMode::EmployeeLevel,
            bytes: b"a,b\n1,2\n".to_vec(),
            file_name: "x.csv".into(),
            period_start: day("2025-01-01"),
            period_end: day("2025-01-14"),
            check_date: day("2025-01-14"),
        })
        .expect_err("unknown");
    assert!(matches!(e, ApiError::Validation(_)));
    let f = &FIXTURES[0];
    let up = d
        .upload(&UploadRequest {
            mode: f.mode,
            bytes: f.bytes.to_vec(),
            file_name: "g.csv".into(),
            period_start: day("2024-12-28"),
            period_end: day("2025-01-24"),
            check_date: day("2025-01-24"),
        })
        .expect("upload");
    d.validate(up.session).expect("validate");
    let e = d
        .post(up.session, false, &IdempotencyKey::new(), None, None)
        .expect_err("unmapped");
    assert!(matches!(
        payroll_refusal(&e),
        Some(PayrollRefusal::Unmapped { .. })
    ));
    assert!(fixture_bytes("square").is_some());
}
