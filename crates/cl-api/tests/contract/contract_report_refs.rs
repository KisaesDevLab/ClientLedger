//! PR-A2 checkpoint, engine PR #58, against a live engine. PENDING without
//! the capabilities (CI requires them with `CL_API_REQUIRE_CAPS`).
//!
//! `report-builder-ties-v1` (U-225, Q-PRA2a):
//! - a balance in a range report drills to an opening row plus its rows,
//!   which sum to the line exactly; by-month balances have no Total;
//! - a two-level Trial Balance section's total equals the one-level one.
//!
//! `report-refs-v1` (U-226, Q-PRA2b–d):
//! - a contact merge keeps a memorized report's figure (the definition
//!   holds the survivor);
//! - a deleted tag group is listed in `missing`, and a run of the
//!   definition is refused with the list (inline 400, saved 422);
//! - a `.clreport` account with this company's number but another name is
//!   a candidate, never a match.
//!
//! Postings live in 2033, a year no other contract test posts in.

use cl_api::resources::report_builder::{
    BuilderRefusal, BuilderRun, DetailRequest, RunDates, RunPurpose, RunRef, RunRequest,
    builder_refusal,
};
use cl_api::resources::reports::definitions::{
    Choices, DefinitionRow, DefinitionWrite, TransferReport,
};
use cl_api::{
    AccountType, ApiError, Basis, Capabilities, ContactFields, ContactType, JeLine, NewContact,
    NewJournalEntry, Session, Side, TagGroupFields,
};
use cl_core::Money;
use cl_core::ids::ContactId;
use cl_core::report::{Cell, ColumnKind, DateRange, Drill, RowKind};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::common::{account, account_fields, date, je2, key, line, m, post};
use crate::support::{Harness, unique};

const YEAR: i32 = 2033;

fn ties(h: &Harness) -> bool {
    h.require_cap(Capabilities::REPORT_BUILDER, "U-119c")
        && h.require_cap(Capabilities::REPORT_BUILDER_DRILL, "U-119e")
        && h.require_cap(Capabilities::REPORT_BUILDER_TIES, "U-225")
}

fn refs(h: &Harness) -> bool {
    h.require_cap(Capabilities::REPORT_BUILDER, "U-119c")
        && h.require_cap(Capabilities::REPORT_DEFS_BUILDER, "U-119a")
        && h.require_cap(Capabilities::REPORT_REFS, "U-226")
}

fn months() -> Vec<DateRange> {
    (1..=12)
        .map(|mo| {
            let from = date(YEAR, mo, 1);
            let next = if mo == 12 {
                date(YEAR + 1, 1, 1)
            } else {
                date(YEAR, mo + 1, 1)
            };
            DateRange {
                from,
                to: next.add_days(-1).expect("day"),
            }
        })
        .collect()
}

fn reference(definition: Option<Value>, id: Option<String>, by_month: bool, as_of: bool) -> RunRef {
    RunRef {
        definition,
        definition_id: id,
        version: None,
        dates: if as_of {
            RunDates::as_of(date(YEAR, 12, 31))
        } else {
            RunDates::range(DateRange {
                from: date(YEAR, 1, 1),
                to: date(YEAR, 12, 31),
            })
        },
        period_ranges: if by_month { months() } else { Vec::new() },
        compare_ranges: Vec::new(),
        fiscal_year_starts: vec![date(YEAR - 1, 1, 1), date(YEAR, 1, 1)],
        basis: None,
        adjusting: None,
    }
}

fn run(h: &Harness, s: &Session, reference: RunRef) -> Result<BuilderRun, ApiError> {
    let req = RunRequest {
        reference,
        open: Vec::new(),
        collapsed: Vec::new(),
        expand_all: false,
        purpose: RunPurpose::Screen,
    };
    for _ in 0..10 {
        match h.run(s.report_builder().run(&req)) {
            Err(ApiError::RateLimited { .. }) => {
                std::thread::sleep(std::time::Duration::from_secs(15));
            }
            other => return other,
        }
    }
    h.run(s.report_builder().run(&req))
}

fn amount(c: &Cell) -> Decimal {
    match c {
        Cell::Amount(a) => *a,
        _ => Decimal::ZERO,
    }
}

#[allow(clippy::needless_pass_by_value, reason = "json! reads it in place")]
fn section(id: &str, measure: &str, ids: &[String], group_by: Value) -> Value {
    json!({
        "type": "section", "id": id, "caption": "Section", "source": "gl",
        "measure": measure, "polarity": "debit",
        "filters": [{"field": "account", "op": "in", "ids": ids}],
        "groupBy": group_by, "display": "summary",
        "defaultExpanded": false, "showTotal": true, "hideZero": false
    })
}

#[allow(clippy::needless_pass_by_value, reason = "json! reads it in place")]
fn builder(mode: &str, periods: Option<&str>, nodes: Value) -> Value {
    let mut columns = json!({"detailFields": ["date", "amount"], "compare": [], "headers": {}});
    if let Some(p) = periods {
        columns["periods"] = json!(p);
    }
    json!({"schemaVersion": 1, "kind": "builder", "dates": {"mode": mode},
           "columns": columns, "nodes": nodes})
}

/// (a) A bank balance by month: no Total column; the February drill starts
/// with the balance before Feb 1 and its rows add up to the line exactly.
pub fn a_balance_drill_adds_up_with_its_opening_row(h: &Harness) {
    if !ties(h) {
        return;
    }
    let s = &h.s1;
    let bank = account(h, s, AccountType::Asset, "TieBank");
    let sales = account(h, s, AccountType::Revenue, "TieSales");
    post(
        h,
        s,
        &je2(date(YEAR, 1, 15), "tie", bank.id, sales.id, "1000.0050"),
    );
    post(
        h,
        s,
        &je2(date(YEAR, 2, 10), "tie", bank.id, sales.id, "249.9950"),
    );
    let def = builder(
        "range",
        Some("month"),
        json!([section(
            "banks",
            "balance",
            &[bank.id.to_string()],
            json!([{"key": "account"}])
        )]),
    );
    let reference = reference(Some(def), None, true, false);
    let r = run(h, s, reference.clone()).expect("run");
    let cols = &r.table.columns;
    assert!(
        cols.iter().all(|c| c.key != "total"),
        "balances only: no summed Total (Q-PRA2a)"
    );
    let feb = cols
        .iter()
        .position(|c| c.range.is_some_and(|x| x.from == date(YEAR, 2, 1)))
        .expect("February");
    let line = r
        .table
        .rows
        .iter()
        .find(|x| x.kind == RowKind::Group)
        .expect("the bank line");
    assert_eq!(amount(&line.cells[feb]), Decimal::new(12_500_000, 4));
    let Some(Drill::BuilderDetail { node_id, path, .. }) = line.drill.clone() else {
        panic!("the line drills: {:?}", line.drill);
    };
    let page = h
        .run(s.report_builder().detail(
            &DetailRequest {
                reference,
                node_id,
                path,
                range: cols[feb].range,
                cursor: None,
                fingerprint: r.fingerprint.clone(),
                stamp: r.stamp,
            },
            cols,
            &r.layout,
        ))
        .expect("February drill");
    let opening = &page.rows[0];
    assert_eq!(opening.kind, RowKind::Opening);
    assert_eq!(
        opening.caption, "@reports.builder.opening_before 02/01/2033",
        "dated by the drill's first day"
    );
    assert_eq!(opening.drill, None);
    assert_eq!(amount(&opening.cells[feb]), Decimal::new(10_000_050, 4));
    let rows: Decimal = page
        .rows
        .iter()
        .filter(|x| matches!(x.kind, RowKind::Detail | RowKind::Opening))
        .map(|x| amount(&x.cells[feb]))
        .sum();
    assert_eq!(rows, amount(&line.cells[feb]), "opening + rows = the line");
    assert_eq!(
        page.subtotal[feb], line.cells[feb],
        "the footer is the line"
    );
}

/// (b) A Trial Balance section grouped by type then account totals the
/// same debit and credit as one grouped by account (one asset account
/// with a debit and one with a credit balance in the same type).
pub fn a_two_level_trial_balance_totals_once(h: &Harness) {
    if !ties(h) {
        return;
    }
    let s = &h.s1;
    let bank = account(h, s, AccountType::Asset, "TieTbBank");
    let over = account(h, s, AccountType::Asset, "TieTbOverdrawn");
    let sales = account(h, s, AccountType::Revenue, "TieTbSales");
    post(
        h,
        s,
        &je2(date(YEAR, 3, 1), "tb", bank.id, sales.id, "1460.00"),
    );
    post(
        h,
        s,
        &je2(date(YEAR, 3, 2), "tb", sales.id, over.id, "30.00"),
    );
    let ids = [
        bank.id.to_string(),
        over.id.to_string(),
        sales.id.to_string(),
    ];
    let total = |group_by: Value| {
        let def = builder(
            "asOf",
            None,
            json!([section("tb", "tb_net", &ids, group_by)]),
        );
        let r = run(h, s, reference(Some(def), None, false, true)).expect("TB run");
        let row = r
            .table
            .rows
            .iter()
            .find(|x| x.kind == RowKind::Total)
            .expect("the section total")
            .clone();
        let amounts: Vec<Cell> = r
            .table
            .columns
            .iter()
            .zip(row.cells)
            .filter(|(c, _)| c.kind == ColumnKind::Amount)
            .map(|(_, x)| x)
            .collect();
        amounts
    };
    let one = total(json!([{"key": "account"}]));
    let two = total(json!([{"key": "account_type"}, {"key": "account"}]));
    assert_eq!(two, one, "two group-bys total once (1-B2)");
    assert!(
        one.iter().any(|c| amount(c) == Decimal::new(146_000, 2)),
        "debits {one:?}"
    );
}

fn customer(h: &Harness, s: &Session, prefix: &str) -> ContactId {
    h.run(s.contacts().create(
        &NewContact {
            fields: ContactFields::named(ContactType::Customer, unique(prefix)),
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("contact: {e}"))
    .record
    .id
}

fn memorize(h: &Harness, s: &Session, def: Value) -> DefinitionRow {
    h.run(s.report_definitions().create(
        &DefinitionWrite {
            name: unique("Refs"),
            group_id: None,
            kind: "builder".into(),
            definition: def,
            origin: None,
        },
        key(),
    ))
    .expect("memorize")
}

fn total_of(r: &BuilderRun) -> Vec<Cell> {
    r.table
        .rows
        .iter()
        .find(|x| x.kind == RowKind::Total)
        .map(|x| x.cells.clone())
        .unwrap_or_default()
}

/// (c) A memorized report filtered on a customer keeps its figure after
/// that customer is merged; its definition then names the survivor.
pub fn a_contact_merge_keeps_a_memorized_figure(h: &Harness) {
    if !refs(h) {
        return;
    }
    let s = &h.s1;
    let bank = account(h, s, AccountType::Asset, "RefBank");
    let sales = account(h, s, AccountType::Revenue, "RefSales");
    let (dup, kept) = (customer(h, s, "Ref dup"), customer(h, s, "Ref kept"));
    let entry = NewJournalEntry::new(
        date(YEAR, 4, 2),
        Some(unique("ref")),
        Basis::Both,
        vec![
            line(bank.id, Side::Debit(m("312.3400"))),
            JeLine {
                contact: Some(dup),
                ..line(sales.id, Side::Credit(m("312.3400")))
            },
        ],
    )
    .expect("balanced");
    post(h, s, &entry);
    let mut sec = section("rev", "amount", &[sales.id.to_string()], json!([]));
    sec["polarity"] = json!("credit");
    sec["filters"]
        .as_array_mut()
        .expect("filters")
        .push(json!({"field": "contact", "op": "in", "ids": [dup.to_string()]}));
    let saved = memorize(h, s, builder("range", None, json!([sec])));
    let saved_run = || run(h, s, reference(None, Some(saved.id.clone()), false, false));
    let before = total_of(&saved_run().expect("before the merge"));
    assert!(
        before
            .iter()
            .any(|c| amount(c) == Decimal::new(3_123_400, 4))
    );
    h.run(s.contacts().merge(dup, kept, None, key()))
        .expect("merge");
    let after = total_of(&saved_run().expect("after the merge"));
    assert_eq!(after, before, "the figure is kept (Q-PRA2b)");
    let row = h
        .run(s.report_definitions().get(&saved.id))
        .expect("definition");
    assert!(row.missing.is_empty(), "{:?}", row.missing);
    assert!(row.version > saved.version, "the merge rewrote it");
    let text = row.definition.to_string();
    assert!(text.contains(&kept.to_string()), "the survivor");
    assert!(!text.contains(&dup.to_string()), "not the merged-away id");
}

/// (d) A deleted tag group: the definition lists it in `missing`, an
/// inline run is refused with the list (400), a saved run too (422).
pub fn a_deleted_tag_group_is_missing(h: &Harness) {
    if !refs(h) {
        return;
    }
    let s = &h.s1;
    let costs = account(h, s, AccountType::Expense, "RefCosts");
    let group = h
        .run(s.tags().create_group(
            &TagGroupFields {
                name: unique("Ref jobs"),
                description: None,
                is_single_select: false,
            },
            key(),
        ))
        .expect("tag group")
        .id;
    let sec = section(
        "costs",
        "amount",
        &[costs.id.to_string()],
        json!([{"key": "tag", "tagGroupId": group.to_string()}]),
    );
    let saved = memorize(h, s, builder("range", None, json!([sec])));
    h.run(s.tags().delete_group(group, key()))
        .expect("delete the group");
    let row = h
        .run(s.report_definitions().get(&saved.id))
        .expect("definition");
    assert_eq!(row.missing.len(), 1, "{:?}", row.missing);
    let m0 = &row.missing[0];
    assert_eq!(m0.kind, "tagGroup");
    assert_eq!(m0.reason, "deleted");
    assert_eq!(m0.node_id.as_deref(), Some("costs"));
    assert_eq!(m0.ids, [group.to_string()]);
    let inline = run(
        h,
        s,
        reference(Some(row.definition.clone()), None, false, false),
    )
    .expect_err("inline run");
    assert!(
        matches!(builder_refusal(&inline), Some(BuilderRefusal::MissingReference(l)) if l.len() == 1),
        "inline: {inline:?}"
    );
    let saved_err =
        run(h, s, reference(None, Some(saved.id.clone()), false, false)).expect_err("saved run");
    assert!(
        matches!(builder_refusal(&saved_err), Some(BuilderRefusal::MissingReference(l)) if l.len() == 1),
        "saved: {saved_err:?}"
    );
}

/// (e) A report file's account 6xxx · <other name>, where this company's
/// 6xxx has another name: a candidate to pick, never a match.
pub fn a_same_number_other_name_account_is_a_candidate(h: &Harness) {
    if !refs(h) || !h.require_cap(Capabilities::REPORT_DEFS_TRANSFER, "U-220") {
        return;
    }
    let s = &h.s1;
    let number = format!("6{:03}", std::process::id() % 1000);
    let mut f = account_fields(AccountType::Expense, "RefPayroll");
    f.account_number = Some(number.clone());
    let here = h
        .run(s.accounts().create(&f, key()))
        .unwrap_or_else(|e| panic!("account: {e}"))
        .record;
    let report = TransferReport {
        name: unique("Refs file"),
        group_name: None,
        kind: "builder".into(),
        definition: builder(
            "range",
            None,
            json!([section("s1", "amount", &["$ref:0".into()], json!([]))]),
        ),
        references: vec![json!({"kind": "account", "name": unique("Rent"), "number": number})],
    };
    let out = h
        .run(
            s.report_definitions()
                .import_preview(&[report], &Choices::new()),
        )
        .expect("preview");
    assert!(!out[0].ready, "not matched");
    let u = &out[0].unresolved;
    assert_eq!(u.len(), 1, "{u:?}");
    assert_eq!(u[0].reference_token, "$ref:0");
    assert_eq!(
        u[0].candidates.first().map(|c| c.id.as_str()),
        Some(here.id.to_string().as_str()),
        "the same-number account first (Q-PRA2d)"
    );
}
