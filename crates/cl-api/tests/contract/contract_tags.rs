//! Tags (P11 step 5): tag groups (create, rename, one-per-group flag,
//! delete leaves the tags ungrouped), usage counts, rename, and merge of a
//! tag used only as a header tag (Q-178 to Q-180). The engine gaps are
//! capability probes that assert what the engine does today and print
//! `PENDING(U-xx)` until the capability is advertised: U-41 (merge of line
//! tags, one tag per group on the replace path, the group reorder route),
//! U-42 (required groups by account type) and U-45 (several tags on one
//! line). Tag numbers (Q-270) are U-75 (`tag-number-v1`): written against
//! the documented contract and PENDING until the pin has it.

use std::time::{Duration, Instant};

use cl_api::{
    ApiError, ArchiveRequest, Basis, Capabilities, ConflictReason, ImportRequest, JeLine, JobState,
    LocalValidation, Method, NewJournalEntry, PageRequest, Side, Tag, TagFields, TagFilter,
    TagGroupFields, TagUpdate,
};
use cl_core::Secret;
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
            tag_number: None,
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

    h.run(s.tags().merge(source.id, target.id, None, key()))
        .expect("merge");
    assert!(h.run(s.tags().get(source.id)).is_err(), "source deleted");
    let moved = h.run(s.tags().usage(target.id)).expect("usage");
    assert_eq!(moved.transactions, 1);
}

/// U-41 / U-47: a tag used on a line. Without `merge-closing-date-v1` the
/// engine moves header tags only and then fails on the line's reference;
/// with it (Q-P11t) the line tag moves and the source is deleted.
pub fn line_tag_merge_u41(h: &Harness) {
    let s = &h.s1;
    let source = tag(h, "Line tag", None);
    let target = tag(h, "Line target", None);
    entry_with_line_tag(h, Some(source.id));
    let usage = h.run(s.tags().usage(source.id)).expect("usage");
    assert_eq!(usage.transaction_lines, 1);
    assert!(!usage.merge_safe());
    let merged = h.run(s.tags().merge(source.id, target.id, None, key()));
    if h.caps.has(Capabilities::MERGE_CLOSING_DATE) {
        merged.expect("merge moves line tags (merge-closing-date-v1)");
        assert!(h.run(s.tags().get(source.id)).is_err(), "source deleted");
        let moved = h.run(s.tags().usage(target.id)).expect("usage");
        assert_eq!(moved.transaction_lines, 1);
    } else if h.require_cap(Capabilities::TAG_MERGE, "U-41") {
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

/// A tag number no other run used: `9` and nine digits (≤ 20 characters).
fn fresh_number() -> String {
    let digits: String = unique("")
        .trim()
        .bytes()
        .map(|b| char::from(b'0' + b % 10))
        .collect();
    format!("9{}", &digits[..9])
}

fn numbered(name: &str, number: Option<&str>) -> TagFields {
    TagFields {
        name: name.to_string(),
        color: None,
        group_id: None,
        description: None,
        tag_number: number.map(str::to_string),
    }
}

fn search(h: &Harness, text: &str) -> Vec<Tag> {
    h.run(h.s1.tags().list(
        &TagFilter {
            search: Some(text.to_string()),
            ..TagFilter::default()
        },
        PageRequest::first_max(),
    ))
    .expect("list")
    .data
}

/// U-75 (Q-270): create with a number (sent trimmed), a used number is 409
/// `TAG_NUMBER_EXISTS` on create and on update, edit, clear with `""`, the
/// list sorts number then name (unnumbered last), and `search` matches the
/// number. Without `tag-number-v1` a number is refused before sending and
/// the engine drops the key.
pub fn numbers_u75(h: &Harness) {
    let s = &h.s1;
    let number = fresh_number();
    if !h.require_cap(Capabilities::TAG_NUMBER, "U-75") {
        let before = h.observer.total();
        let e = h
            .run(
                s.tags()
                    .create(&numbered(&unique("Numbered"), Some(&number)), key()),
            )
            .expect_err("refused locally");
        assert_eq!(e, ApiError::Local(LocalValidation::InvalidValue));
        assert_eq!(h.observer.total(), before, "nothing may be sent");
        let raw = h
            .run(s.raw(
                Method::Post,
                "/api/v1/tags",
                &[],
                Some(&json!({"name": unique("Numbered raw"), "tagNumber": number})),
            ))
            .expect("raw");
        assert_eq!(raw.status, 201, "{}", raw.body);
        assert!(
            raw.body["tag"]["tagNumber"].is_null(),
            "dropped today: {}",
            raw.body
        );
        let id = cl_core::ids::TagId::parse(raw.body["tag"]["id"].as_str().expect("id"))
            .expect("tag id");
        h.run(s.tags().delete(id, key())).expect("clean up");
        return;
    }

    // Create (trimmed) and read back.
    let first = h
        .run(s.tags().create(
            &numbered(&unique("Lake Area"), Some(&format!(" {number} "))),
            key(),
        ))
        .expect("create")
        .record;
    assert_eq!(first.fields.tag_number.as_deref(), Some(number.as_str()));
    let got = h.run(s.tags().get(first.id)).expect("get");
    assert_eq!(
        got.record.fields.tag_number.as_deref(),
        Some(number.as_str())
    );

    // A used number: create and update are both refused, nothing created.
    let dup_name = unique("Lake Area again");
    let dup = h
        .run(s.tags().create(&numbered(&dup_name, Some(&number)), key()))
        .expect_err("used number");
    assert!(
        matches!(
            dup,
            ApiError::Conflict {
                reason: ConflictReason::TagNumberTaken,
                ..
            }
        ),
        "{dup:?}"
    );
    assert!(search(h, &dup_name).is_empty(), "nothing created");
    let other = h
        .run(s.tags().create(&numbered(&unique("Hill"), None), key()))
        .expect("other");
    let mut taken = other.record.fields.clone();
    taken.tag_number = Some(number.clone());
    let e = h
        .run(s.tags().update(
            other.record.id,
            &other.version,
            &TagUpdate {
                fields: taken,
                is_active: true,
            },
            key(),
        ))
        .expect_err("used number on update");
    assert!(
        matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::TagNumberTaken,
                ..
            }
        ),
        "{e:?}"
    );

    // Edit, then clear with "".
    let edited_number = fresh_number();
    let mut fields = got.record.fields.clone();
    fields.tag_number = Some(edited_number.clone());
    let edited = h
        .run(s.tags().update(
            first.id,
            &got.version,
            &TagUpdate {
                fields: fields.clone(),
                is_active: true,
            },
            key(),
        ))
        .expect("edit");
    assert_eq!(
        edited.record.fields.tag_number.as_deref(),
        Some(edited_number.as_str())
    );
    // Leaving the number out of a PUT keeps it.
    let mut kept_fields = fields.clone();
    kept_fields.tag_number = None;
    kept_fields.description = Some("kept".into());
    let kept = h
        .run(s.tags().update(
            first.id,
            &edited.version,
            &TagUpdate {
                fields: kept_fields,
                is_active: true,
            },
            key(),
        ))
        .expect("keep");
    assert_eq!(
        kept.record.fields.tag_number.as_deref(),
        Some(edited_number.as_str())
    );
    fields.tag_number = Some(String::new());
    let cleared = h
        .run(s.tags().update(
            first.id,
            &kept.version,
            &TagUpdate {
                fields,
                is_active: true,
            },
            key(),
        ))
        .expect("clear");
    assert_eq!(cleared.record.fields.tag_number, None);
    // The old number is free again.
    let reused = h
        .run(
            s.tags()
                .create(&numbered(&unique("Reused"), Some(&number)), key()),
        )
        .expect("number free after the edit");

    // List order: number, then name; unnumbered last. Search by number.
    let tok = unique("Order");
    let base = fresh_number();
    let (n1, n2) = (format!("{}1", &base[..9]), format!("{}2", &base[..9]));
    for (suffix, n) in [("c", None), ("b", Some(&n1)), ("a", Some(&n2))] {
        h.run(s.tags().create(
            &numbered(&format!("{tok} {suffix}"), n.map(String::as_str)),
            key(),
        ))
        .expect("order tag");
    }
    let names: Vec<String> = search(h, &tok).into_iter().map(|t| t.fields.name).collect();
    assert_eq!(
        names,
        [format!("{tok} b"), format!("{tok} a"), format!("{tok} c")],
        "number then name, unnumbered last"
    );
    let by_number = search(h, &n2);
    assert!(
        by_number
            .iter()
            .any(|t| t.fields.tag_number.as_deref() == Some(n2.as_str())),
        "search matches the number"
    );
    for id in [first.id, other.record.id, reused.record.id] {
        h.run(s.tags().delete(id, key())).expect("clean up");
    }
}

/// Polls an archive job to its end.
fn finish_job(h: &Harness, s: &cl_api::Session, job: &cl_api::JobId) -> cl_api::ArchiveJobStatus {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let st = h.run(s.tenants().archive_job(job)).expect("job");
        match st.status {
            JobState::Succeeded | JobState::Failed => return st,
            JobState::Queued | JobState::Running => {
                assert!(Instant::now() < deadline, "job did not finish");
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
}

/// U-75: a tag number survives a tenant archive export and import.
pub fn number_archive_round_trip_u75(h: &Harness) {
    let numbers = h.require_cap(Capabilities::TAG_NUMBER, "U-75");
    let archives = h.require_cap(Capabilities::TENANT_ARCHIVE, "U-05");
    if !(numbers && archives) {
        return;
    }
    // Tenant B round-trips: a company made here would be one more
    // rate-limited create-tenant call than the run's budget allows.
    let s = h.s3();
    let co = h.tenant_b();
    let number = fresh_number();
    let name = unique("Archived");
    h.run(s.tags().create(&numbered(&name, Some(&number)), key()))
        .expect("tag");
    h.run(s.close_company()).expect("workspace");
    let passphrase = Secret::new(format!("contract-{}", unique("pass")));
    let job = h
        .run(s.tenants().export_archive(
            co,
            &ArchiveRequest {
                include_attachments: false,
                include_audit: true,
                omit_intake_settings: false,
                omit_document_drafts: false,
                host: json!({"app": "ClientLedger", "appVersion": "test", "kind": "backup"}),
                passphrase: passphrase.clone(),
            },
        ))
        .expect("export job");
    let st = finish_job(h, &s, &job.job_id);
    assert_eq!(st.status, JobState::Succeeded, "{:?}", st.error);
    let path = st
        .export_result()
        .expect("export result")
        .path
        .clone()
        .expect("path");
    let job = h
        .run(s.tenants().import_archive(&ImportRequest {
            path,
            passphrase,
            scratch: false,
            reset_document_drafts: false,
        }))
        .expect("import job");
    let st = finish_job(h, &s, &job.job_id);
    assert_eq!(st.status, JobState::Succeeded, "{:?}", st.error);
    let imported = st.import_result().expect("import result").tenant_id;
    h.run(s.open_company(imported)).expect("open restored");
    let found = h
        .run(s.tags().list_all(&TagFilter {
            search: Some(name.clone()),
            ..TagFilter::default()
        }))
        .expect("tags");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].fields.tag_number.as_deref(), Some(number.as_str()));
    h.run(s.close_company()).expect("workspace");
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
