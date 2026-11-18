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

fn saved(id: u8, provider: &str, cols: &[(&'static str, &str)], updated: &str) -> SavedLayout<u8> {
    SavedLayout {
        id,
        name: format!("Layout {id}"),
        provider_key: provider.into(),
        is_system: false,
        updated_at: Some(updated.into()),
        layout: manual_layout(
            &cols
                .iter()
                .map(|(f, h)| (*f, (*h).to_string()))
                .collect::<Vec<_>>(),
            None,
        ),
        department_tags: BTreeMap::new(),
        left_out: None,
    }
}

fn rows(r: &[&[&str]]) -> Vec<Vec<String>> {
    r.iter()
        .map(|x| x.iter().map(|c| (*c).to_string()).collect())
        .collect()
}

/// S4: a saved layout applied to a file with new amount columns names
/// them; columns it left out, text columns and an unchanged file don't.
#[test]
fn new_amount_columns_are_named_before_a_layout_applies() {
    let mut layout = saved(
        1,
        "custom",
        &[
            ("employee_name", "Worker"),
            ("gross_pay", "Wages"),
            ("net_pay", "Take Home"),
        ],
        "2025-07-12T00:00:00Z",
    );
    layout.left_out = Some(vec!["Fed WH".into()]);
    let h = headers(&[
        "Worker",
        "Wages",
        "Fed WH",
        "Take Home",
        "Notes",
        "SS ER",
        "medicare  er",
    ]);
    let r = rows(&[&[
        "Test Worker 01",
        "1,000.00",
        "100.05",
        "800.00",
        "on leave",
        "$62.00",
        "(14.50)",
    ]]);
    assert_eq!(
        new_amount_columns(&layout, &h, &r),
        vec!["SS ER".to_string(), "medicare  er".to_string()]
    );
    // The same columns as when it was saved: nothing new.
    let same = headers(&["Worker", "Wages", "Fed WH", "Take Home"]);
    assert_eq!(new_amount_columns(&layout, &same, &r), Vec::<String>::new());
    // A layout saved before left-out columns were kept: every unmatched
    // amount column is new (stop once on Match columns).
    layout.left_out = None;
    assert_eq!(
        new_amount_columns(&layout, &same, &r),
        vec!["Fed WH".to_string()]
    );
    assert_eq!(
        unmatched_columns(&layout.layout, &h),
        vec!["Fed WH", "Notes", "SS ER", "medicare  er"]
    );
}

/// M9: taxpayer-number-shaped cells are never shown, nor read as amounts.
#[test]
fn ssn_shaped_cells_are_never_shown() {
    for c in ["123-45-6789", " 123 45 6789 ", "123456789"] {
        assert_eq!(SampleGuard::displayable(c), None, "{c}");
        assert!(!SampleGuard::is_amount(c), "{c}");
    }
    assert_eq!(SampleGuard::displayable("1,234.56"), Some("1,234.56"));
    assert_eq!(SampleGuard::displayable("2025-07-11"), Some("2025-07-11"));
    assert!(SampleGuard::is_amount("-50.00"));
    assert!(!SampleGuard::is_amount("07/11/2025"));
    assert!(!SampleGuard::is_amount("2025-07-11"));
}

#[test]
fn layouts_are_remembered_per_provider_and_by_headers_for_generic_files() {
    let generic = headers(&["Worker", "Paid On", "Wages", "Take Home"]);
    let mine = saved(
        1,
        "custom",
        &[
            ("employee_name", "Worker"),
            ("gross_pay", "Wages"),
            ("net_pay", "Take Home"),
        ],
        "2025-07-12T00:00:00Z",
    );
    let gusto_own = saved(
        2,
        "gusto",
        &[
            ("employee_name", "Employee Name"),
            ("gross_pay", "Gross Pay"),
            ("net_pay", "Net Pay"),
        ],
        "2025-01-11T00:00:00Z",
    );
    let list = vec![mine, gusto_own];
    // A2: the second generic file of the same shape needs no Map step.
    assert_eq!(
        choose_layout(Provider::GenericCsv, &generic, &list),
        LayoutChoice::Saved(1)
    );
    assert_eq!(
        choose_layout(Provider::Rippling, &generic, &list),
        LayoutChoice::Saved(1)
    );
    assert_eq!(
        choose_layout(Provider::GenericCsv, &headers(&["Other"]), &list),
        LayoutChoice::Manual
    );
    let gusto_file = headers(&["Employee Name", "Gross Pay", "Net Pay", "Department"]);
    assert_eq!(
        choose_layout(Provider::Gusto, &gusto_file, &list),
        LayoutChoice::Saved(2)
    );
    assert_eq!(
        choose_layout(Provider::Gusto, &headers(&["Employee Name"]), &list),
        LayoutChoice::Builtin,
        "a saved layout that doesn't fit falls back to the built-in one"
    );
    assert_eq!(
        choose_layout(Provider::AdpRun, &gusto_file, &list),
        LayoutChoice::NotNeeded
    );
    assert_eq!(mappable_field("gross_pay"), Some("gross_pay"));
    assert_eq!(mappable_field("employee_ssn"), None);
    assert_eq!(guess_date_format(["07/11/2025"]), "MM/DD/YYYY");
    assert_eq!(guess_date_format(["2025-07-11"]), "YYYY-MM-DD");
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

// ---- Accounts step defaults (P30 QA-1) ------------------------------------

/// The seeded QA company's chart shape: the engine's default chart (10100
/// Cash, 6000/6200/6400 numbered for other things) plus the sample's own.
fn seeded_chart() -> Vec<ChartRow<u32>> {
    use ChartKind::{Bank, Expense, Liability, Other};
    [
        (10100, "Cash", Bank, false),
        (1010, "Operating Checking", Bank, true),
        (1020, "Savings", Bank, false),
        (1200, "Accounts Receivable", Other, false),
        (2000, "Accounts Payable", Liability, false),
        (2050, "Business Credit Card", Liability, false),
        (2200, "Vehicle and Equipment Loans", Liability, false),
        (2500, "Payroll Liabilities", Liability, false),
        (20900, "Sales Tax Payable", Liability, false),
        (4000, "Service Income", Other, false),
        (6000, "Advertising and Marketing", Expense, false),
        (6150, "Insurance", Expense, false),
        (6200, "Legal and Professional Fees", Expense, false),
        (6400, "Telephone and Internet", Expense, false),
        (6550, "Wages", Expense, false),
        (6560, "Payroll Taxes", Expense, false),
        (6700, "Vehicle Expenses", Expense, false),
    ]
    .into_iter()
    .map(|(id, name, kind, checking)| ChartRow {
        id,
        name: name.into(),
        kind,
        checking,
    })
    .collect()
}

const GUSTO_LINES: [&str; 11] = [
    "gross_wages_expense",
    "employer_tax_expense",
    "reimbursement_expense",
    "fit_payable",
    "sit_payable",
    "ss_payable",
    "medicare_payable",
    "futa_payable",
    "suta_payable",
    "health_ins_payable",
    "retirement_payable",
];

#[test]
fn seeded_chart_defaults_are_payroll_accounts_or_empty() {
    let chart = seeded_chart();
    // The engine's number matcher on this chart (QA-1).
    let engine: BTreeMap<String, u32> = [
        ("gross_wages_expense", 6000),
        ("employer_tax_expense", 6200),
        ("reimbursement_expense", 6400),
        ("retirement_payable", 2200),
        ("payroll_clearing", 10100),
        ("other_tax_payable", 2500),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let mut lines = GUSTO_LINES.to_vec();
    lines.push(NET_PAY_LINE);
    let d = default_accounts(&lines, &chart, &engine, &BTreeMap::new());
    assert_eq!(d.get("gross_wages_expense"), Some(&6550));
    assert_eq!(d.get("employer_tax_expense"), Some(&6560));
    assert_eq!(
        d.get("reimbursement_expense"),
        None,
        "no reimbursement account"
    );
    for p in [
        "fit_payable",
        "sit_payable",
        "ss_payable",
        "medicare_payable",
        "futa_payable",
        "suta_payable",
        "health_ins_payable",
        "retirement_payable",
    ] {
        assert_eq!(d.get(p), Some(&2500), "{p}");
    }
    // The only checking account, never the first bank account ("Cash").
    assert_eq!(d.get(NET_PAY_LINE), Some(&1010));
    for wrong in [6000, 6200, 6400, 2200, 10100] {
        assert!(!d.values().any(|v| *v == wrong), "{wrong}: {d:?}");
    }
}

#[test]
fn nothing_matching_leaves_the_line_empty() {
    use ChartKind::{Bank, Expense, Liability};
    let chart: Vec<ChartRow<u32>> = [
        (1, "Cash", Bank, false),
        (2, "Petty Cash", Bank, false),
        (3, "Advertising", Expense, false),
        (4, "Real Estate Loan", Liability, false),
        (5, "Employee Benefits Program", Expense, false),
    ]
    .into_iter()
    .map(|(id, name, kind, checking)| ChartRow {
        id,
        name: name.into(),
        kind,
        checking,
    })
    .collect();
    let mut lines = GUSTO_LINES.to_vec();
    lines.push(NET_PAY_LINE);
    let engine: BTreeMap<String, u32> = [("gross_wages_expense".to_string(), 3)].into();
    let d = default_accounts(&lines, &chart, &engine, &BTreeMap::new());
    assert!(d.is_empty(), "{d:?}");
    // "state" never matches "Real Estate"; "fit" never matches "Benefits".
    assert_eq!(confident_account("sit_payable", &chart), None);
    assert_eq!(confident_account("fit_payable", &chart), None);
    assert_eq!(
        confident_account("employer_benefits_expense", &chart),
        Some(5)
    );
}

#[test]
fn ready_made_descriptions_read_as_line_types() {
    for (desc, debit, lt) in [
        ("Wages and Salary", true, "gross_wages_expense"),
        ("Social Security Expense", true, "employer_tax_expense"),
        ("Employer 401(k) Match", true, "employer_benefits_expense"),
        ("Net Pay", false, NET_PAY_LINE),
        ("Federal Withholding", false, "fit_payable"),
        ("FUTA Liability", false, "futa_payable"),
        ("State Unemployment Payable", false, "suta_payable"),
        ("Real Estate Garnishment", false, "other_deduction_payable"),
    ] {
        assert_eq!(line_for_description(desc, debit), lt, "{desc}");
    }
}

#[test]
fn a_remembered_account_wins_and_two_equal_matches_pick_none() {
    let mut chart = seeded_chart();
    chart.push(ChartRow {
        id: 6551,
        name: "Salaries".into(),
        kind: ChartKind::Expense,
        checking: false,
    });
    chart.push(ChartRow {
        id: 1030,
        name: "Payroll Checking".into(),
        kind: ChartKind::Bank,
        checking: true,
    });
    // Wages and Salaries tie: the owner picks.
    assert_eq!(confident_account("gross_wages_expense", &chart), None);
    // A bank account named for payroll beats two checking accounts.
    assert_eq!(confident_account(NET_PAY_LINE, &chart), Some(1030));
    let remembered: BTreeMap<String, u32> = [
        ("gross_wages_expense".to_string(), 6551),
        (NET_PAY_LINE.to_string(), 10100),
        // An account no longer in the chart is not pre-selected.
        ("fit_payable".to_string(), 9999),
    ]
    .into();
    let d = default_accounts(
        &["gross_wages_expense", NET_PAY_LINE, "fit_payable"],
        &chart,
        &BTreeMap::new(),
        &remembered,
    );
    assert_eq!(d.get("gross_wages_expense"), Some(&6551));
    assert_eq!(d.get(NET_PAY_LINE), Some(&10100));
    assert_eq!(d.get("fit_payable"), Some(&2500));
}
