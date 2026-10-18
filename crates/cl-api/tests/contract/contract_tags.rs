//! Tags (P11 step 5): tag groups (create, rename, one-per-group flag,
//! delete leaves the tags ungrouped), usage counts, rename, and merge of a
//! tag used only as a header tag (Q-178 to Q-180). The engine gaps are
//! capability probes that assert what the engine does today and print
//! `PENDING(U-xx)` until the capability is advertised: U-41 (merge of line
//! tags, one tag per group on the replace path, the group reorder route),
//! U-42 (required groups by account type) and U-45 (several tags on one
//! line).

use cl_api::{
    Basis, Capabilities, JeLine, Method, NewJournalEntry, Side, Tag, TagFields, TagGroupFields,
    TagUpdate,
};
use serde_json::json;

use crate::common::{date, key, line, m, pair, post};
use crate::support::{Harness, unique};

fn tag(h: &Harness, name: &str, group: Option<cl_core::ids::TagGroupId>) -> Tag {
    let s = &h.s1;
    h.run(s.tags().create(
        &TagFields {
            name: unique(name),
            color: None,
            group_id: group,
            description: None,
        },
        key(),
    ))
    .expect("tag")
    .record
}

fn group(h: &Harness, name: &str, single: bool) -> cl_api::TagGroup {
    h.run(h.s1.tags().create_group(
        &TagGroupFields {
            name: unique(name),
            description: None,
            is_single_select: single,
        },
        key(),
    ))
    .expect("group")
}

/// A posted two-line entry whose first line carries `line_tag`.
fn entry_with_line_tag(h: &Harness, line_tag: Option<cl_core::ids::TagId>) -> cl_api::Transaction {
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let e = NewJournalEntry::new(
        date(2026, 8, 3),
        Some("tag contract".into()),
        Basis::Both,
        vec![
            JeLine {
                tag: line_tag,
                ..line(supplies.id, Side::Debit(m("12.34")))
            },
            line(cash.id, Side::Credit(m("12.34"))),
        ],
    )
    .expect("balanced");
    post(h, s, &e).record
}

/// Groups: create (one per group), list with their tags, rename, delete
/// (the tag stays, ungrouped).
pub fn groups_round_trip(h: &Harness) {
    let s = &h.s1;
    let g = group(h, "Region", true);
    assert!(g.fields.is_single_select);
    let t = tag(h, "East", Some(g.id));
    let groups = h.run(s.tags().groups()).expect("groups");
    let listed = groups.iter().find(|x| x.id == g.id).expect("listed");
    assert!(listed.tags.iter().any(|x| x.id == t.id));

    let renamed = h
        .run(s.tags().update_group(
            g.id,
            &TagGroupFields {
                name: unique("Area"),
                description: Some("where the work is".into()),
                is_single_select: false,
            },
            key(),
        ))
        .expect("rename");
    assert!(!renamed.fields.is_single_select);
    assert_eq!(
        renamed.fields.description.as_deref(),
        Some("where the work is")
    );

    h.run(s.tags().delete_group(g.id, key())).expect("delete");
    let after = h.run(s.tags().get(t.id)).expect("tag");
    assert_eq!(after.record.fields.group_id, None, "ungrouped, not deleted");
    let groups = h.run(s.tags().groups()).expect("groups");
    assert!(groups.iter().all(|x| x.id != g.id));
}

/// Rename keeps the tag; a header-only tag merges (usage moves, the source
/// is deleted).
pub fn rename_usage_and_header_merge(h: &Harness) {
    let s = &h.s1;
    let source = tag(h, "Crew A", None);
    let target = tag(h, "Crew B", None);
    let txn = entry_with_line_tag(h, None);
    let added = h
        .run(s.raw(
            Method::Post,
            &format!("/api/v1/tags/transactions/{}/add", txn.id),
            &[],
            Some(&json!({"tagIds": [source.id.to_string()]})),
        ))
        .expect("raw");
    assert_eq!(added.status, 200, "{}", added.body);
    let usage = h.run(s.tags().usage(source.id)).expect("usage");
    assert_eq!(usage.transactions, 1);
    assert_eq!(usage.transaction_lines, 0);
    assert!(usage.merge_safe());

    let got = h.run(s.tags().get(source.id)).expect("get");
    let mut fields = got.record.fields.clone();
    fields.name = unique("Crew A renamed");
    let renamed = h
        .run(s.tags().update(
            source.id,
            &got.version,
            &TagUpdate {
                fields: fields.clone(),
                is_active: true,
            },
            key(),
        ))
        .expect("rename");
    assert_eq!(renamed.record.fields.name, fields.name);

    h.run(s.tags().merge(source.id, target.id, key()))
        .expect("merge");
    assert!(h.run(s.tags().get(source.id)).is_err(), "source deleted");
    let moved = h.run(s.tags().usage(target.id)).expect("usage");
    assert_eq!(moved.transactions, 1);
}

/// U-41: a tag used on a line can't be merged today (the engine moves
/// header tags only and then fails on the line's reference).
pub fn line_tag_merge_u41(h: &Harness) {
    let s = &h.s1;
    let source = tag(h, "Line tag", None);
    let target = tag(h, "Line target", None);
    entry_with_line_tag(h, Some(source.id));
    let usage = h.run(s.tags().usage(source.id)).expect("usage");
    assert_eq!(usage.transaction_lines, 1);
    assert!(!usage.merge_safe());
    let merged = h.run(s.tags().merge(source.id, target.id, key()));
    if h.require_cap(Capabilities::TAG_MERGE, "U-41") {
        merged.expect("merge moves line tags");
        let moved = h.run(s.tags().usage(target.id)).expect("usage");
        assert_eq!(moved.transaction_lines, 1);
    } else {
        assert!(merged.is_err(), "refused today: {merged:?}");
        let still = h.run(s.tags().usage(source.id)).expect("source kept");
        assert_eq!(still.transaction_lines, 1);
    }
}

/// U-41: `PUT /tags/groups/reorder` is caught by `PUT /tags/groups/{id}`
/// today, so the order never changes.
pub fn group_reorder_u41(h: &Harness) {
    let s = &h.s1;
    let a = group(h, "First", false);
    let b = group(h, "Second", false);
    let order = |h: &Harness| -> Vec<cl_core::ids::TagGroupId> {
        h.run(h.s1.tags().groups())
            .expect("groups")
            .into_iter()
            .filter(|g| g.id == a.id || g.id == b.id)
            .map(|g| g.id)
            .collect()
    };
    let before = order(h);
    let mut wanted = before.clone();
    wanted.reverse();
    let raw = h
        .run(s.raw(
            Method::Put,
            "/api/v1/tags/groups/reorder",
            &[],
            Some(
                &json!({"orderedIds": wanted.iter().map(ToString::to_string).collect::<Vec<_>>()}),
            ),
        ))
        .expect("raw");
    let after = order(h);
    if h.require_cap(Capabilities::TAG_MERGE, "U-41") {
        assert_eq!(raw.status, 200, "{}", raw.body);
        assert_eq!(after, wanted);
    } else {
        assert_eq!(
            after, before,
            "the reorder never runs today (status {})",
            raw.status
        );
    }
    for g in [a, b] {
        h.run(s.tags().delete_group(g.id, key())).expect("clean up");
    }
}

/// U-41: two tags of a one-per-group group sent together are both kept
/// today, on the add path and the replace path (the engine only clears the
/// group's earlier tags); the UI enforces Q-179 itself.
pub fn single_select_u41(h: &Harness) {
    let s = &h.s1;
    let g = group(h, "Job type", true);
    let a = tag(h, "Install", Some(g.id));
    let b = tag(h, "Repair", Some(g.id));
    let both = json!({"tagIds": [a.id.to_string(), b.id.to_string()]});
    let enforced = h.require_cap(Capabilities::TAG_MERGE, "U-41");
    for (method, path) in [(Method::Post, "/add"), (Method::Put, "")] {
        let txn = entry_with_line_tag(h, None);
        let raw = h
            .run(s.raw(
                method,
                &format!("/api/v1/tags/transactions/{}{path}", txn.id),
                &[],
                Some(&both),
            ))
            .expect("raw");
        if enforced {
            assert_ne!(raw.status, 200, "{path}: {}", raw.body);
        } else {
            assert_eq!(raw.status, 200, "{path}: not enforced today: {}", raw.body);
        }
    }
    if !enforced {
        for t in [a.id, b.id] {
            let u = h.run(s.tags().usage(t)).expect("usage");
            assert_eq!(u.transactions, 2, "both kept on both transactions");
        }
    }
}

/// U-42: a group can't be required for account types today (the field is
/// dropped).
pub fn required_groups_u42(h: &Harness) {
    let s = &h.s1;
    let raw = h
        .run(s.raw(
            Method::Post,
            "/api/v1/tags/groups",
            &[],
            Some(&json!({"name": unique("Required"), "requiredForAccountTypes": ["expense"]})),
        ))
        .expect("raw");
    assert_eq!(raw.status, 201, "{}", raw.body);
    let g = &raw.body["group"];
    if h.require_cap(Capabilities::TAG_REQUIRED_GROUPS, "U-42") {
        assert_eq!(g["requiredForAccountTypes"], json!(["expense"]));
    } else {
        assert!(g["requiredForAccountTypes"].is_null(), "dropped today: {g}");
    }
    let id = g["id"].as_str().expect("id").to_string();
    let gone = h
        .run(s.raw(
            Method::Delete,
            &format!("/api/v1/tags/groups/{id}"),
            &[],
            None,
        ))
        .expect("raw");
    assert_eq!(gone.status, 200, "{}", gone.body);
}

/// U-45: a journal line holds one tag today (`tagId`); a list of tags on
/// the line is dropped.
pub fn several_line_tags_u45(h: &Harness) {
    let s = &h.s1;
    let a = tag(h, "Line one", None);
    let b = tag(h, "Line two", None);
    let (cash, supplies) = pair(h, s);
    let raw = h
        .run(s.raw(
            Method::Post,
            "/api/v1/transactions",
            &[],
            Some(&json!({
                "txnType": "journal_entry",
                "txnDate": "2026-08-04",
                "lines": [
                    {"accountId": supplies.id.to_string(), "debit": "5.00", "credit": "0",
                     "tagIds": [a.id.to_string(), b.id.to_string()]},
                    {"accountId": cash.id.to_string(), "debit": "0", "credit": "5.00"}
                ]
            })),
        ))
        .expect("raw");
    assert_eq!(raw.status, 201, "{}", raw.body);
    let lines = raw.body["transaction"]["lines"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let tagged = lines
        .iter()
        .find(|l| l["accountId"] == supplies.id.to_string())
        .cloned()
        .unwrap_or_default();
    if h.require_cap(Capabilities::LINE_TAGS, "U-45") {
        assert_eq!(
            tagged["tagIds"].as_array().map(Vec::len),
            Some(2),
            "{tagged}"
        );
    } else {
        assert!(
            tagged["tagIds"].is_null(),
            "one tag per line today: {tagged}"
        );
    }
}
