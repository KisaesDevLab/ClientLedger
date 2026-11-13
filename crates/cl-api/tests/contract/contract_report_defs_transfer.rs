//! `.clreport` export, import preview and audit against a live engine (A09
//! AC4, U-220 `report-defs-transfer-v1`, engine cf0cb239; WP-A). CI requires
//! the capability (`CL_API_REQUIRE_CAPS`).
//!
//! - Export then preview into the same company gives the stored
//!   definitions back, ready, with the group and the name clash found; the
//!   export holds no uuid and is audited as `download` per report.
//! - Preview into tenant B (other ids) remaps by account number and tag
//!   group name; an account whose number B doesn't have is unresolved with
//!   the same-name account as its candidate, then picked, then left out.
//! - A Replace (update with `origin: 'import'`) is refused for a non-author
//!   and for a kind change; create and update with the origin are audited
//!   with `origin: 'import'` in `after`.

use cl_api::resources::reports::definitions::{
    Choices, DefinitionPatch, DefinitionRefusal, DefinitionRow, DefinitionWrite, ExportWhat,
    PreviewReport, TransferReport, choice_key, definition_refusal,
};
use cl_api::{Account, AccountType, Capabilities, Method, Session, TagGroupFields};
use serde_json::{Value, json};

use crate::common::{account_fields, key};
use crate::support::{Harness, unique};

fn caps(h: &Harness) -> bool {
    h.require_cap(Capabilities::REPORT_DEFS_BUILDER, "U-119a")
        && h.require_cap(Capabilities::REPORT_DEFS_TRANSFER, "U-220")
}

/// An account named `name` with `number` (both shared by the tests' two
/// tenants), in the session's company.
fn numbered(h: &Harness, s: &Session, t: AccountType, name: &str, number: &str) -> Account {
    let mut f = account_fields(t, name);
    f.name = name.to_string();
    f.account_number = Some(number.to_string());
    h.run(s.accounts().create(&f, key()))
        .unwrap_or_else(|e| panic!("create account: {e}"))
        .record
}

fn tag_group(h: &Harness, s: &Session, name: &str) -> String {
    h.run(s.tags().create_group(
        &TagGroupFields {
            name: name.to_string(),
            description: None,
            is_single_select: false,
        },
        key(),
    ))
    .expect("tag group")
    .id
    .to_string()
}

/// A builder P&L-like section over `accounts`, grouped by a tag group.
fn builder(accounts: &[String], tag_group: &str) -> Value {
    json!({
        "schemaVersion": 1, "kind": "builder",
        "dates": {"mode": "range"},
        "columns": {"detailFields": [], "compare": [], "headers": {}},
        "nodes": [{
            "type": "section", "id": "s1", "caption": "Shop costs", "source": "gl",
            "measure": "amount", "polarity": "debit",
            "filters": [{"field": "account", "op": "in", "ids": accounts},
                        {"field": "amount", "op": "gte", "value": "1250.5000"}],
            "groupBy": [{"key": "tag", "tagGroupId": tag_group}], "display": "summary",
            "defaultExpanded": false, "showTotal": true, "hideZero": false
        }]
    })
}

fn standard(accounts: &[String]) -> Value {
    json!({"schemaVersion": 1, "kind": "standard", "reportKey": "profit_loss",
           "options": {"dates": {"preset": "last_month"},
                       "filters": {"accountIds": accounts}}})
}

fn memorize(h: &Harness, s: &Session, name: &str, kind: &str, def: Value) -> DefinitionRow {
    let w = DefinitionWrite {
        name: name.to_string(),
        group_id: None,
        kind: kind.into(),
        definition: def,
        origin: None,
    };
    h.run(s.report_definitions().create(&w, key()))
        .expect("memorize")
}

/// No uuid-shaped run (8-4-4-4-12 hex digits) anywhere in `v`'s text.
fn uuid_free(v: &Value) -> bool {
    let t = v.to_string();
    let b = t.as_bytes();
    let dash = |i: usize| [8, 13, 18, 23].contains(&i);
    !b.windows(36).any(|w| {
        w.iter().enumerate().all(|(i, c)| {
            if dash(i) {
                *c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
    })
}

/// The audit rows of report `id` (newest first, as the engine lists them).
fn audit_rows(h: &Harness, s: &Session, id: &str) -> Vec<Value> {
    let reply = h
        .run(s.raw(
            Method::Get,
            "/api/v1/audit-log",
            &[("entity_type", "report_definition"), ("entity_id", id)],
            None,
        ))
        .expect("audit log");
    assert_eq!(reply.status, 200, "{}", reply.body);
    reply.body["data"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r["entity_id"].as_str() == Some(id))
        .collect()
}

/// An audit image, stored as JSON or as JSON text.
fn image(v: &Value) -> Value {
    match v {
        Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
        v => v.clone(),
    }
}

fn preview(h: &Harness, s: &Session, r: &[TransferReport], c: &Choices) -> Vec<PreviewReport> {
    h.run(s.report_definitions().import_preview(r, c))
        .expect("preview")
}

pub fn export_and_preview_in_the_same_company(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let defs = s.report_definitions();
    let supplies = numbered(h, s, AccountType::Expense, &unique("Shop supplies"), "6187");
    let region = tag_group(h, s, &unique("Region"));
    let group = h
        .run(defs.create_group(&unique("For the bank")))
        .expect("g");
    let b = memorize(
        h,
        s,
        &unique("Shop costs"),
        "builder",
        builder(&[supplies.id.to_string()], &region),
    );
    let st = memorize(
        h,
        s,
        &unique("Monthly P&L"),
        "standard",
        standard(&[supplies.id.to_string()]),
    );
    let moved = h
        .run(defs.patch(
            &b.id,
            b.version,
            "builder",
            &cl_api::resources::reports::definitions::DefinitionPatch {
                group_id: Some(Some(group.id.clone())),
                ..Default::default()
            },
        ))
        .expect("move");

    let out = h
        .run(defs.export(&ExportWhat::Ids(vec![b.id.clone(), st.id.clone()])))
        .expect("export");
    assert_eq!(out.len(), 2);
    let whole = serde_json::to_value(
        out.iter()
            .map(|r| {
                json!({"name": r.name, "group": r.group_name, "kind": r.kind,
                       "definition": r.definition, "references": r.references})
            })
            .collect::<Vec<_>>(),
    )
    .expect("json");
    assert!(uuid_free(&whole), "no uuid anywhere in an export");
    assert_eq!(out[0].group_name.as_deref(), Some(group.name.as_str()));
    let ids = &out[0].definition["nodes"][0]["filters"][0]["ids"];
    assert_eq!(ids, &json!(["$ref:0"]));
    assert_eq!(
        out[0].definition["nodes"][0]["filters"][1]["value"], "1250.5000",
        "an amount filter is kept as typed"
    );
    assert_eq!(out[0].references[0]["number"], "6187");
    assert_eq!(out[0].references[1]["kind"], "tagGroup");

    // The group export: the same report.
    let by_group = h
        .run(defs.export(&ExportWhat::Group(group.id.clone())))
        .expect("group export");
    assert_eq!(by_group.len(), 1);
    assert_eq!(by_group[0].name, b.name);

    // Back into the same company: identical, ready, clash and group found.
    let back = preview(h, s, &out, &Choices::new());
    assert!(back.iter().all(|r| r.ready && r.unresolved.is_empty()));
    assert_eq!(back[0].definition, b.definition);
    assert_eq!(back[1].definition, st.definition);
    assert_eq!(back[0].group_id.as_deref(), Some(group.id.as_str()));
    let taken = back[0].name_taken.as_ref().expect("name taken");
    assert_eq!(
        (taken.id.as_str(), taken.version),
        (b.id.as_str(), moved.version)
    );
    assert!(taken.can_edit);

    // One download row per report, naming nothing.
    for id in [&b.id, &st.id] {
        let rows = audit_rows(h, s, id);
        let dl = rows
            .iter()
            .find(|r| r["action"] == "download")
            .expect("download audited");
        let after = image(&dl["after_data"]);
        assert_eq!(after["format"], "clreport");
        assert!(after.get("name").is_none());
    }
}

pub fn a_second_company_remaps_and_resolves_references(h: &Harness) {
    if !caps(h) {
        return;
    }
    let (a, bs) = (&h.s1, h.s3());
    let sales_name = unique("Repair sales");
    let fees_name = unique("Bank fees");
    let region_name = unique("Region");
    let sales_a = numbered(h, a, AccountType::Revenue, &sales_name, "4187");
    let fees_a = numbered(h, a, AccountType::Expense, &fees_name, "6287");
    let region_a = tag_group(h, a, &region_name);
    let def = builder(&[sales_a.id.to_string(), fees_a.id.to_string()], &region_a);
    let row = memorize(h, a, &unique("Two accounts"), "builder", def);
    let out = h
        .run(
            a.report_definitions()
                .export(&ExportWhat::Ids(vec![row.id.clone()])),
        )
        .expect("export");

    // Tenant B: the same number and name for sales, the fees account under
    // another number, the tag group by name.
    let sales_b = numbered(h, &bs, AccountType::Revenue, &sales_name, "4187");
    let fees_b = numbered(h, &bs, AccountType::Expense, &fees_name, "6299");
    let region_b = tag_group(h, &bs, &region_name);

    let first = preview(h, &bs, &out, &Choices::new());
    assert!(!first[0].ready);
    assert_eq!(first[0].unresolved.len(), 1, "the fees account");
    let u = &first[0].unresolved[0];
    assert_eq!(u.reference["number"], "6287");
    assert!(
        u.candidates.iter().any(|c| c.id == fees_b.id.to_string()),
        "the same-name account is a candidate"
    );

    // A pick: ready, every id tenant B's own.
    let mut choices = Choices::new();
    choices.insert(
        choice_key(0, &u.reference_token),
        Some(fees_b.id.to_string()),
    );
    let picked = preview(h, &bs, &out, &choices);
    assert!(picked[0].ready, "{:?}", picked[0]);
    assert_eq!(
        picked[0].definition,
        builder(&[sales_b.id.to_string(), fees_b.id.to_string()], &region_b)
    );
    let text = picked[0].definition.to_string();
    for theirs in [sales_a.id.to_string(), fees_a.id.to_string(), region_a] {
        assert!(!text.contains(&theirs), "no id of tenant A");
    }

    // A leave-out: the id leaves its list.
    choices.insert(choice_key(0, &u.reference_token), None);
    let left = preview(h, &bs, &out, &choices);
    assert!(left[0].ready);
    assert_eq!(left[0].dropped.len(), 1);
    assert_eq!(left[0].dropped[0].effect, "removed_from_list");
    assert_eq!(
        left[0].definition["nodes"][0]["filters"][0]["ids"],
        json!([sales_b.id.to_string()])
    );

    // Memorized in tenant B as an import: audited with the origin.
    let w = DefinitionWrite {
        name: picked[0].name.clone(),
        group_id: None,
        kind: "builder".into(),
        definition: picked[0].definition.clone(),
        origin: Some("import".into()),
    };
    let made = h
        .run(bs.report_definitions().create(&w, key()))
        .expect("create as import");
    let rows = audit_rows(h, &bs, &made.id);
    let create = rows
        .iter()
        .find(|r| r["action"] == "create")
        .expect("create audited");
    assert_eq!(image(&create["after_data"])["origin"], "import");
}

pub fn replace_is_refused_for_a_non_author_and_a_kind_change(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let acct = numbered(h, s, AccountType::Expense, &unique("Fuel"), "6387");
    let row = memorize(
        h,
        s,
        &unique("Fuel costs"),
        "builder",
        builder(&[acct.id.to_string()], &tag_group(h, s, &unique("Region"))),
    );
    let replace = DefinitionWrite {
        name: row.name.clone(),
        group_id: None,
        kind: "builder".into(),
        definition: row.definition.clone(),
        origin: Some("import".into()),
    };

    // Another user: 403 NOT_AUTHOR (Q-310). The bookkeeper is shared by the
    // whole run and other tests leave their own permission overrides on it,
    // so this test sets the one it needs (report access) first.
    let (user, bk) = h.bookkeeper();
    let p = h
        .run(s.raw(
            Method::Put,
            &format!("/api/v1/company/users/{user}/permissions"),
            &[],
            Some(&json!({ "overrides": { "reports": "view" } })),
        ))
        .expect("permissions");
    assert_eq!(p.status, 200, "{}", p.body);
    let e = h
        .run(
            bk.report_definitions()
                .update(&row.id, row.version, &replace),
        )
        .expect_err("not the author");
    assert_eq!(
        definition_refusal(&e),
        Some(DefinitionRefusal::NotAuthor),
        "{e:?}"
    );

    // The author, with another kind: refused, the row unchanged.
    let standard_body = DefinitionWrite {
        kind: "standard".into(),
        definition: standard(&[acct.id.to_string()]),
        ..replace.clone()
    };
    h.run(
        s.report_definitions()
            .update(&row.id, row.version, &standard_body),
    )
    .expect_err("a kind change is refused");
    let read = h.run(s.report_definitions().get(&row.id)).expect("get");
    assert_eq!(read.kind, "builder");
    assert_eq!(read.version, row.version);

    // The author's Replace of the same kind, as the desktop sends it (A09
    // #1): the body and the origin only, so a grouped report stays in its
    // group under its own name; audited as an import.
    let group = h
        .run(s.report_definitions().create_group(&unique("Kept group")))
        .expect("group");
    let grouped = h
        .run(s.report_definitions().patch(
            &row.id,
            row.version,
            "builder",
            &DefinitionPatch {
                group_id: Some(Some(group.id.clone())),
                ..DefinitionPatch::default()
            },
        ))
        .expect("move");
    let mut body = row.definition.clone();
    body["nodes"][0]["caption"] = json!("Fuel, replaced");
    let done = h
        .run(s.report_definitions().patch(
            &row.id,
            grouped.version,
            "builder",
            &DefinitionPatch {
                definition: Some(body.clone()),
                origin: Some("import".into()),
                ..DefinitionPatch::default()
            },
        ))
        .expect("replace");
    assert!(done.version > grouped.version);
    assert_eq!(done.group_id.as_deref(), Some(group.id.as_str()), "kept");
    assert_eq!(done.name, row.name, "kept");
    assert_eq!(done.definition["nodes"][0]["caption"], "Fuel, replaced");
    // The full write (name, group, kind, body) is refused by nobody either.
    let full = h
        .run(
            s.report_definitions()
                .update(&row.id, done.version, &replace),
        )
        .expect("full replace");
    assert!(full.version > done.version);
    let rows = audit_rows(h, s, &row.id);
    let imports = rows
        .iter()
        .filter(|r| r["action"] == "update" && image(&r["after_data"])["origin"] == "import")
        .count();
    assert_eq!(imports, 2, "both Replaces audited as an import");
}
