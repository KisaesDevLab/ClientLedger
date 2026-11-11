//! The demo report store: memorized reports, budgets, Budget vs. Actual,
//! email and the stub mapper.

use super::*;
use cl_core::report::{Cell, Source};

fn day(y: i32, m: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, d).expect("date")
}

#[test]
fn the_mockups_memorized_report_and_a_budget_are_there_once() {
    let mut d = DemoState::new();
    let (list, _) = definitions(&mut d);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "Monthly P&L for the bank");
    let def: cl_model::reports::StandardDefinition =
        serde_json::from_value(list[0].definition.clone()).expect("definition");
    assert!(def.check().is_ok());
    assert_eq!(budgets(&mut d).len(), 1);
    assert_eq!(definitions(&mut d).0.len(), 1, "seeded once");
}

#[test]
fn memorize_saves_renames_and_refuses_a_taken_name_or_stale_version() {
    let mut d = DemoState::new();
    let w = DefinitionWrite {
        name: "October for the bank".into(),
        group_id: None,
        kind: "standard".into(),
        definition: json!({}),
    };
    let row = save_definition(&mut d, None, &w).expect("new");
    assert_eq!(row.version, 1);
    let again = save_definition(&mut d, None, &w).expect_err("taken");
    assert_eq!(
        cl_api::resources::reports::definitions::definition_refusal(&again),
        Some(cl_api::resources::reports::definitions::DefinitionRefusal::NameTaken)
    );
    let saved = save_definition(&mut d, Some((row.id.clone(), 1)), &w).expect("changes");
    assert_eq!(saved.version, 2);
    let stale = save_definition(&mut d, Some((row.id.clone(), 1)), &w).expect_err("stale");
    assert_eq!(
        cl_api::resources::reports::definitions::definition_refusal(&stale),
        Some(cl_api::resources::reports::definitions::DefinitionRefusal::Stale)
    );
    assert_eq!(definition(&mut d, &row.id).expect("get").version, 2);
}

#[test]
fn budget_lines_save_adjust_copy_fill_and_refuse_a_stale_version() {
    let mut d = DemoState::new();
    let id = budgets(&mut d)[0].id.clone();
    let lines = budget_lines(&mut d, &id).expect("lines");
    assert!(!lines.lines.is_empty());
    let v = lines.budget.version;
    let a = lines.lines[0].account_id;
    let saved = save_budget_lines(
        &mut d,
        &id,
        v,
        &[BudgetLineWrite {
            account_id: a,
            periods: [money("10.00"); PERIODS],
        }],
    )
    .expect("save");
    assert_eq!(
        saved.lines[0].total,
        "120.00".parse::<Decimal>().expect("d")
    );
    let stale = save_budget_lines(&mut d, &id, v, &[]).expect_err("stale");
    assert_eq!(
        cl_api::resources::reports::budgets::budget_refusal(&stale),
        Some(cl_api::resources::reports::budgets::BudgetRefusal::Stale)
    );
    let up = whole_budget(
        &mut d,
        &id,
        saved.budget.version,
        &WholeBudget::Adjust(Decimal::TEN),
    )
    .expect("adjust");
    assert_eq!(
        up.lines[0].periods[0],
        "11.00".parse::<Decimal>().expect("d")
    );
    let next = create_budget(
        &mut d,
        &NewBudget {
            name: "2027 budget".into(),
            fiscal_year: 2027,
        },
    )
    .expect("create");
    let taken = create_budget(
        &mut d,
        &NewBudget {
            name: "again".into(),
            fiscal_year: 2027,
        },
    )
    .expect_err("year taken");
    assert!(cl_api::resources::reports::budgets::budget_refusal(&taken).is_some());
    let copied = whole_budget(&mut d, &next.id, 1, &WholeBudget::Copy(&id)).expect("copy");
    assert_eq!(copied.lines.len(), up.lines.len());
    let filled = whole_budget(&mut d, &next.id, 2, &WholeBudget::Fill).expect("fill");
    assert!(!filled.lines.is_empty(), "2026 actuals fill 2027");
}

#[test]
fn budget_vs_actual_runs_through_the_live_adapter() {
    let mut d = DemoState::new();
    let id = budgets(&mut d)[0].id.clone();
    let oct = DateRange {
        from: day(2026, 10, 1),
        to: day(2026, 10, 31),
    };
    let mut req = ReportRequest::new(Source::BudgetVsActual, ReportDates::Ranges(vec![oct]));
    req.budget_id = Some(id);
    req.first_period = Some(10);
    let t = budget_vs_actual(&mut d, &req).expect("bva");
    assert_eq!(t.columns.len(), 8, "October and Total, four each");
    let income = t
        .rows
        .iter()
        .find(|r| r.caption.contains("4000"))
        .expect("4000 row");
    assert_eq!(
        income.cells[1],
        Cell::Amount("85000.00".parse().expect("d")),
        "October's budget"
    );
    let net = t.rows.last().expect("net");
    let (Cell::Amount(a), Cell::Amount(b), Cell::Amount(o)) =
        (&net.cells[0], &net.cells[1], &net.cells[2])
    else {
        panic!("net cells");
    };
    assert_eq!(*a, "13865.14".parse::<Decimal>().expect("d"));
    assert_eq!(a - b, *o);
}

#[test]
fn email_sends_nothing_and_counts() {
    let mut d = DemoState::new();
    let m = ReportEmail {
        to: vec!["bank@example.com".into()],
        cc: vec![],
        subject: "P&L".into(),
        body: String::new(),
        filename: "pl.pdf".into(),
        file: vec![1, 2, 3],
        report_key: Some("profit_loss".into()),
    };
    let sent = email(&mut d, &m).expect("sent");
    assert!(sent.sent);
    assert_eq!(sent.recipient_count, 1);
    assert_eq!(d.report_store.emails_sent, 1);
    let none = ReportEmail { to: vec![], ..m };
    assert!(email(&mut d, &none).is_err());
}

#[test]
fn the_stub_mapper_maps_the_mockups_question_and_refuses_others() {
    let req = crate::screens::reports::ask_request_for_test(
        "How did October compare with the year so far?",
        day(2026, 11, 4),
    );
    let m = ask(&req).expect("mapped");
    assert_eq!(m.report_key, "profit_loss");
    assert_eq!(m.extras, ["pct_of_income", "ytd"]);
    let other =
        crate::screens::reports::ask_request_for_test("What's the weather?", day(2026, 11, 4));
    assert!(matches!(ask(&other), Err(ApiError::ReportShape { .. })));
    let owe = crate::screens::reports::ask_request_for_test("Who owes me money?", day(2026, 11, 4));
    assert_eq!(ask(&owe).expect("aging").report_key, "ar_aging_summary");
}
