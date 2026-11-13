//! Memorized reports against the real engine (P28, U-118 `report-defs-v1`,
//! Q-P28d): create, list, read, change at the read version, the stale 409,
//! the name 409, groups (deleting a group keeps its reports, ungrouped),
//! an invalid body refused, and soft delete.

use cl_api::Capabilities;
use cl_api::resources::reports::definitions::{
    DefinitionRefusal, DefinitionWrite, definition_refusal,
};
use serde_json::json;

use crate::support::{Harness, unique};

fn standard(report_key: &str, preset: &str) -> serde_json::Value {
    let options = json!({"dates": {"preset": preset}});
    json!({"schemaVersion": 1, "kind": "standard", "reportKey": report_key, "options": options})
}

pub fn crud_conflicts_and_groups(h: &Harness) {
    if !h.require_cap(Capabilities::REPORT_DEFS, "U-118") {
        return;
    }
    let s = &h.s1;
    let defs = s.report_definitions();
    let group = h
        .run(defs.create_group(&unique("Month end")))
        .expect("group");
    let name = unique("Monthly P&L");
    let w = DefinitionWrite {
        name: name.clone(),
        group_id: Some(group.id.clone()),
        kind: "standard".into(),
        definition: standard("profit_loss", "last_month"),
        origin: None,
    };
    let row = h
        .run(defs.create(&w, cl_api::IdempotencyKey::new()))
        .expect("create");
    assert_eq!(row.name, name);
    assert_eq!(row.group_id.as_deref(), Some(group.id.as_str()));
    assert_eq!(row.definition["reportKey"], "profit_loss");
    assert!(
        h.run(defs.list())
            .expect("list")
            .iter()
            .any(|r| r.id == row.id),
        "listed"
    );
    let read = h.run(defs.get(&row.id)).expect("get");
    assert_eq!(read.version, row.version);

    // The same name again: 409 NAME_TAKEN.
    let again = h
        .run(defs.create(&w, cl_api::IdempotencyKey::new()))
        .expect_err("name taken");
    assert_eq!(
        definition_refusal(&again),
        Some(DefinitionRefusal::NameTaken)
    );

    // A change at the read version, then a change at the old one: stale.
    let mut changed = w.clone();
    changed.definition = standard("profit_loss", "this_year");
    let next = h
        .run(defs.update(&row.id, row.version, &changed))
        .expect("update");
    assert!(next.version > row.version);
    let stale = h
        .run(defs.update(&row.id, row.version, &changed))
        .expect_err("stale");
    assert_eq!(definition_refusal(&stale), Some(DefinitionRefusal::Stale));

    // A report key that is not a key is refused.
    let mut bad = w.clone();
    bad.name = unique("Bad");
    bad.definition =
        json!({"schemaVersion": 1, "kind": "standard", "reportKey": "NOT A KEY", "options": {}});
    let e = h
        .run(defs.create(&bad, cl_api::IdempotencyKey::new()))
        .expect_err("invalid");
    assert!(matches!(
        definition_refusal(&e),
        Some(DefinitionRefusal::Invalid { .. })
    ));

    // Deleting the group keeps the report, ungrouped.
    let groups = h.run(defs.groups()).expect("groups");
    let g = groups
        .iter()
        .find(|g| g.id == group.id)
        .expect("group listed");
    h.run(defs.delete_group(&g.id, g.version))
        .expect("delete group");
    let after = h.run(defs.get(&row.id)).expect("still there");
    assert_eq!(after.group_id, None);

    // Soft delete: gone from the list.
    h.run(defs.delete(&row.id, after.version)).expect("delete");
    assert!(
        !h.run(defs.list())
            .expect("list")
            .iter()
            .any(|r| r.id == row.id),
        "deleted"
    );
}
