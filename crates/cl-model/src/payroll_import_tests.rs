use super::*;
use proptest::prelude::*;

fn day(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("date")
}

fn tag(n: u8) -> TagId {
    TagId::parse(&format!("4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{n:02}")).expect("tag")
}

fn line(entry: usize, line: usize, desc: &str, preview_tag: Option<TagId>) -> PlanLine {
    PlanLine {
        entry,
        line,
        date: day("2025-01-10"),
        description: desc.into(),
        preview_tag,
    }
}

fn headers(h: &[&str]) -> Vec<String> {
    h.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn providers_route_and_round_trip() {
    for p in Provider::ALL {
        assert_eq!(Provider::from_key(p.key()), Some(p));
    }
    assert_eq!(Provider::Rippling.engine_key(), "custom");
    assert!(Provider::Rippling.is_generic());
    assert_eq!(
        Provider::from_engine_key("custom"),
        None,
        "generic is never detected"
    );
    assert_eq!(Provider::from_engine_key("gusto"), Some(Provider::Gusto));
    assert_eq!(Provider::AdpRun.mode(), FileMode::Prebuilt);
    assert_eq!(Provider::QuickBooksPayroll.mode(), FileMode::EmployeeLevel);
}

#[test]
fn steps_skip_map_for_ready_made_and_saved_layouts() {
    assert_eq!(Step::Choose.next(false), Step::Map);
    assert_eq!(Step::Choose.next(true), Step::Accounts);
    assert_eq!(Step::Accounts.back(true), Some(Step::Choose));
    assert_eq!(Step::Accounts.back(false), Some(Step::Map));
    assert_eq!(Step::Done.back(false), None, "posted: no way back");
    assert_eq!(Step::Preview.next(false), Step::Done);
}

#[test]
fn the_period_needs_all_three_dates_in_order() {
    let today = day("2025-02-01");
    let mut p = PeriodInput {
        start: Some(day("2024-12-28")),
        end: Some(day("2025-01-24")),
        check_date: None,
    };
    assert_eq!(p.checked(today), Err(PeriodProblem::Missing));
    p.check_date = Some(day("2025-01-24"));
    assert_eq!(
        p.checked(today),
        Ok((day("2024-12-28"), day("2025-01-24"), day("2025-01-24")))
    );
    p.check_date = Some(day("2025-02-02"));
    assert_eq!(p.checked(today), Err(PeriodProblem::CheckDateInFuture));
    p.check_date = Some(day("2024-12-01"));
    assert_eq!(p.checked(today), Err(PeriodProblem::CheckDateBeforeStart));
    p.end = Some(day("2024-12-01"));
    assert_eq!(p.checked(today), Err(PeriodProblem::EndsBeforeStart));
}

#[test]
fn samples_show_numbers_and_dates_never_text() {
    for ok in [
        "$1,845.35",
        "(12.30)",
        "5.00-",
        "2025-01-10",
        "01/10/2025",
        "80",
    ] {
        assert_eq!(SampleGuard::displayable(ok), Some(ok), "{ok}");
    }
    for no in [
        "Test Worker 01",
        "Kitchen",
        "",
        "Front 2",
        "TST",
        "03/01/2025 - 03/14/2025",
    ] {
        assert_eq!(SampleGuard::displayable(no), None, "{no}");
    }
}

#[test]
fn quickbooks_layout_balances_with_fica_and_no_date_column() {
    let q = builtin_mapping(Provider::QuickBooksPayroll).expect("layout");
    assert_eq!(
        q.column("check_date"),
        None,
        "the wizard's check date applies"
    );
    for f in [
        "social_security_ee",
        "medicare_ee",
        "social_security_er",
        "medicare_er",
    ] {
        assert!(q.column(f).is_some(), "{f}");
    }
    assert_eq!((q.header_row, q.skip_footer_rows), (3, 1));
    let g = builtin_mapping(Provider::Gusto).expect("gusto");
    assert_eq!(g.column("department"), Some("Department"));
    assert!(
        builtin_mapping(Provider::Square)
            .expect("square")
            .column("check_date")
            .is_none()
    );
    assert!(builtin_mapping(Provider::GenericCsv).is_none());
    for p in [
        Provider::Gusto,
        Provider::Square,
        Provider::QuickBooksPayroll,
    ] {
        let l = builtin_mapping(p).expect("layout");
        for f in REQUIRED_FIELDS {
            assert!(l.column(f).is_some(), "{p:?} maps {f}");
        }
        assert!(l.columns.iter().all(|c| MAPPABLE_FIELDS.contains(&c.field)));
    }
}

fn candidate(id: u8, sources: &[&str], updated: &str) -> TemplateCandidate<u8> {
    TemplateCandidate {
        id,
        sources: headers(sources),
        updated_at: Some(updated.into()),
        is_system: false,
    }
}

#[test]
fn saved_layouts_match_by_columns_then_recency() {
    let file = headers(&["Worker", "Paid On", "Wages", "Fed WH", "Take Home"]);
    let small = candidate(1, &["Worker", "Wages", "Take Home"], "2025-07-01T00:00:00Z");
    let big = candidate(
        2,
        &["Worker", "Paid On", "Wages", "Take Home"],
        "2025-06-01T00:00:00Z",
    );
    let missing = candidate(3, &["Worker", "Wages", "Bonus"], "2025-08-01T00:00:00Z");
    let mut system = candidate(4, &["Worker"], "2025-09-01T00:00:00Z");
    system.is_system = true;
    let list = vec![small, big.clone(), missing, system];
    assert_eq!(template_for_headers(&file, &list).map(|t| t.id), Some(2));
    let newer = candidate(
        5,
        &["Worker", "Paid On", "Wages", "Fed WH"],
        "2025-07-20T00:00:00Z",
    );
    let list = vec![big, newer];
    assert_eq!(
        template_for_headers(&file, &list).map(|t| t.id),
        Some(5),
        "tie → most recent"
    );
    assert!(template_for_headers::<u8>(&file, &[]).is_none());
}

proptest! {
    /// Header order and case and spacing never stop a layout from matching.
    #[test]
    fn header_order_and_case_still_match(
        perm in Just(vec![0usize, 1, 2, 3, 4]).prop_shuffle(),
        upper in proptest::collection::vec(any::<bool>(), 5),
    ) {
        let base = ["Worker", "Paid On", "Wages", "Fed WH", "Take Home"];
        let file: Vec<String> = perm
            .iter()
            .zip(&upper)
            .map(|(i, up)| {
                let h = base[*i];
                if *up { format!("  {}  ", h.to_uppercase().replace(' ', "   ")) } else { h.to_lowercase() }
            })
            .collect();
        let t = candidate(1, &base, "2025-07-01T00:00:00Z");
        prop_assert!(template_for_headers(&file, std::slice::from_ref(&t)).is_some());
    }

    /// A file missing any mapped column never matches.
    #[test]
    fn a_removed_mapped_column_never_matches(drop in 0usize..5) {
        let base = ["Worker", "Paid On", "Wages", "Fed WH", "Take Home"];
        let file: Vec<String> = base
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != drop)
            .map(|(_, h)| (*h).to_string())
            .collect();
        let t = candidate(1, &base, "2025-07-01T00:00:00Z");
        prop_assert!(template_for_headers(&file, std::slice::from_ref(&t)).is_none());
    }
}

#[test]
fn the_gate_refuses_an_untagged_line_when_required_and_names_it() {
    let plan = TagPlan::new(
        vec![
            line(0, 0, "Gross Wages Expense", Some(tag(1))),
            line(0, 1, "Federal Income Tax Payable", None),
            line(0, 2, "Payroll Clearing / Cash", None),
        ],
        None,
    );
    let refused = plan.gate(true).expect_err("refused");
    assert_eq!(refused.description, "Federal Income Tax Payable");
    assert_eq!(refused.date, day("2025-01-10"));
    assert_eq!(refused.count, 2);
    // Off: posts, and nothing differs from the preview, so nothing is sent.
    assert_eq!(plan.gate(false), Ok(vec![]));
}

#[test]
fn the_default_tag_fills_untagged_lines_and_only_differences_are_sent() {
    let mut plan = TagPlan::new(
        vec![
            line(0, 0, "Gross Wages Expense", Some(tag(1))),
            line(0, 1, "Federal Income Tax Payable", None),
        ],
        Some(tag(9)),
    );
    let sent = plan.gate(true).expect("default tag fills it");
    assert_eq!(
        sent,
        vec![LineTagOut {
            entry: 0,
            line: 1,
            date: day("2025-01-10"),
            tag: Some(tag(9))
        }],
        "the department-tagged line keeps its tag and is not sent"
    );
    // The owner clears a line: with the gate on it is refused again.
    plan.set(0, 1, None);
    assert!(plan.gate(true).is_err());
    let off = plan.gate(false).expect("off");
    assert!(off.is_empty(), "cleared = same as preview (untagged)");
    // The owner changes a department tag: that line is sent.
    plan.set(0, 0, Some(tag(2)));
    let sent = plan.gate(false).expect("off");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].tag, Some(tag(2)));
}
