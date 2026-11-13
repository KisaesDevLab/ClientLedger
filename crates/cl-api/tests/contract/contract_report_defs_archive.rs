//! Memorized reports survive a tenant archive export and import with every
//! reference remapped to the restored tenant (A09 AC3, Q-A09m; WP-B).
//!
//! A standard and a Report Builder definition name an account, a client,
//! an item, a tag, a tag group and a budget. After export → import as a new
//! tenant, both are there with their name, group and kind, and every id
//! they name is the restored tenant's row of the same name. P32 re-runs
//! this on the real transfer file (the transfer file is this archive).

use std::time::{Duration, Instant};

use cl_api::resources::reports::budgets::NewBudget;
use cl_api::resources::reports::definitions::DefinitionWrite;
use cl_api::{
    AccountFilter, AccountType, ArchiveRequest, Capabilities, ContactFields, ContactFilter,
    ContactType, IdempotencyKey, ImportRequest, ItemFields, ItemFilter, JobState, NewContact,
    Session, TagFields, TagFilter, TagGroupFields,
};
use cl_core::{Money, Secret};
use serde_json::{Value, json};

use crate::common::{account, key, m};
use crate::support::{Harness, unique};

fn finish_job(h: &Harness, s: &Session, job: &cl_api::JobId) -> cl_api::ArchiveJobStatus {
    let deadline = Instant::now() + Duration::from_secs(180);
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

/// The names the definitions refer to, by kind.
struct Named {
    account: String,
    contact: String,
    item: String,
    tag: String,
    tag_group: String,
    budget: String,
}

/// The id strings a definition names at `pointer` (one id or a list).
fn ids_at(v: &Value, pointer: &str) -> Vec<String> {
    match v.pointer(pointer) {
        Some(Value::String(s)) => vec![s.to_lowercase()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_lowercase)
            .collect(),
        other => panic!("{pointer}: {other:?}"),
    }
}

/// The restored tenant's ids of the rows named `n`.
fn restored_ids(h: &Harness, s: &Session, n: &Named) -> [(String, String); 6] {
    let lower = |x: String| x.to_lowercase();
    let account = h
        .run(s.accounts().list_all(&AccountFilter::default()))
        .expect("accounts")
        .into_iter()
        .find(|a| a.fields.name == n.account)
        .expect("account restored");
    let contact = h
        .run(s.contacts().list_all(&ContactFilter {
            search: Some(n.contact.clone()),
            ..ContactFilter::default()
        }))
        .expect("contacts")
        .into_iter()
        .find(|c| c.fields.display_name == n.contact)
        .expect("contact restored");
    let item = h
        .run(s.items().list_all(&ItemFilter::default()))
        .expect("items")
        .into_iter()
        .find(|i| i.fields.name == n.item)
        .expect("item restored");
    let tag = h
        .run(s.tags().list_all(&TagFilter::default()))
        .expect("tags")
        .into_iter()
        .find(|t| t.fields.name == n.tag)
        .expect("tag restored");
    let group = h
        .run(s.tags().groups())
        .expect("groups")
        .into_iter()
        .find(|g| g.fields.name == n.tag_group)
        .expect("tag group restored");
    let budget = h
        .run(s.budgets().list())
        .expect("budgets")
        .into_iter()
        .find(|b| b.name == n.budget)
        .expect("budget restored");
    [
        ("account".into(), lower(account.id.to_string())),
        ("contact".into(), lower(contact.id.to_string())),
        ("item".into(), lower(item.id.to_string())),
        ("tag".into(), lower(tag.id.to_string())),
        ("tag_group".into(), lower(group.id.to_string())),
        ("budget".into(), lower(budget.id)),
    ]
}

pub fn memorized_reports_survive_an_archive_round_trip(h: &Harness) {
    if !h.require_cap(Capabilities::TENANT_ARCHIVE, "U-05")
        || !h.require_cap(Capabilities::REPORT_DEFS_BUILDER, "A07")
    {
        return;
    }
    let s = h.s3();
    let co = h.tenant_b();
    let income = account(h, &s, AccountType::Revenue, "ArchRptIncome");
    let contact = h
        .run(s.contacts().create(
            &NewContact {
                fields: ContactFields::named(ContactType::Customer, unique("ArchRpt Client")),
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        ))
        .expect("client")
        .record;
    let item = h
        .run(s.items().create(
            &ItemFields {
                purchase: cl_api::ItemPurchase::default(),
                name: unique("ArchRpt Mowing"),
                description: None,
                unit_price: Some(m("45.00")),
                income_account_id: income.id,
                is_taxable: false,
                item_type: cl_api::ItemType::Service,
                discount_percent: None,
                tax_code_id: None,
            },
            key(),
        ))
        .expect("item")
        .record;
    let group = h
        .run(s.tags().create_group(
            &TagGroupFields {
                name: unique("ArchRpt Crews"),
                description: None,
                is_single_select: false,
            },
            key(),
        ))
        .expect("tag group");
    let tag = h
        .run(s.tags().create(
            &TagFields {
                name: unique("ArchRpt North"),
                color: None,
                group_id: Some(group.id),
                description: None,
                tag_number: None,
            },
            key(),
        ))
        .expect("tag")
        .record;
    let budget = h
        .run(s.budgets().create(
            &NewBudget {
                name: unique("ArchRpt budget"),
                fiscal_year: 2043,
            },
            key(),
        ))
        .expect("budget");
    let named = Named {
        account: income.fields.name.clone(),
        contact: contact.fields.display_name.clone(),
        item: item.fields.name.clone(),
        tag: tag.fields.name.clone(),
        tag_group: group.fields.name.clone(),
        budget: budget.name.clone(),
    };
    let (a, c, i, t, g, b) = (
        income.id.to_string(),
        contact.id.to_string(),
        item.id.to_string(),
        tag.id.to_string(),
        group.id.to_string(),
        budget.id.clone(),
    );
    let standard = json!({
        "schemaVersion": 1, "kind": "standard", "reportKey": "profit_loss",
        "options": {
            "filters": {"accountIds": [a], "contactIds": [c], "tagIds": [t], "itemIds": [i]},
            "budgetId": b, "columnTagGroupId": g, "tagSplit": {"tagGroupId": g}
        }
    });
    let builder_name = unique("ArchRpt builder");
    let builder = json!({
        "schemaVersion": 1, "kind": "builder", "title": builder_name,
        "dates": {"mode": "range"},
        "columns": {"detailFields": [], "compare": [], "headers": {}},
        "nodes": [{
            "type": "section", "id": "income", "caption": "Income", "source": "gl",
            "measure": "amount",
            "filters": [
                {"field": "account", "op": "in", "ids": [a]},
                {"field": "contact", "op": "in", "ids": [c]},
                {"field": "item", "op": "in", "ids": [i]},
                {"field": "tag", "op": "in", "ids": [t]}
            ],
            "groupBy": [{"key": "tag", "tagGroupId": g}],
            "display": "summary", "defaultExpanded": false, "showTotal": true, "hideZero": false
        }]
    });
    let defs = s.report_definitions();
    let rgroup = h
        .run(defs.create_group(&unique("ArchRpt group")))
        .expect("report group");
    let std_name = unique("ArchRpt standard");
    for (name, kind, body) in [
        (std_name.clone(), "standard", standard),
        (builder_name.clone(), "builder", builder),
    ] {
        h.run(defs.create(
            &DefinitionWrite {
                name,
                group_id: Some(rgroup.id.clone()),
                kind: kind.into(),
                definition: body,
                origin: None,
            },
            IdempotencyKey::new(),
        ))
        .unwrap_or_else(|e| panic!("{kind}: {e}"));
    }
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
    let restored = st.import_result().expect("import result").tenant_id;
    h.run(s.open_company(restored)).expect("open restored");
    let want = restored_ids(h, &s, &named);
    let id_of = |k: &str| {
        want.iter()
            .find(|(x, _)| x == k)
            .map(|(_, id)| id.clone())
            .expect(k)
    };
    // The restored rows are new rows: the ids moved.
    assert_ne!(
        id_of("account"),
        a.to_lowercase(),
        "a new tenant has new ids"
    );
    let defs = s.report_definitions();
    let list = h.run(defs.list()).expect("definitions");
    let groups = h.run(defs.groups()).expect("groups");
    let rg = groups
        .iter()
        .find(|x| x.name == rgroup.name)
        .expect("the report group is restored");
    let std_row = list
        .iter()
        .find(|r| r.name == std_name)
        .expect("standard restored");
    let b_row = list
        .iter()
        .find(|r| r.name == builder_name)
        .expect("builder restored");
    assert_eq!(
        (std_row.kind.as_str(), b_row.kind.as_str()),
        ("standard", "builder")
    );
    assert_eq!(std_row.group_id.as_deref(), Some(rg.id.as_str()));
    assert_eq!(b_row.group_id.as_deref(), Some(rg.id.as_str()));
    let sd = &std_row.definition;
    for (pointer, k) in [
        ("/options/filters/accountIds", "account"),
        ("/options/filters/contactIds", "contact"),
        ("/options/filters/tagIds", "tag"),
        ("/options/filters/itemIds", "item"),
        ("/options/budgetId", "budget"),
        ("/options/columnTagGroupId", "tag_group"),
        ("/options/tagSplit/tagGroupId", "tag_group"),
    ] {
        assert_eq!(ids_at(sd, pointer), [id_of(k)], "standard {pointer}");
    }
    let bd = &b_row.definition;
    for (n, k) in ["account", "contact", "item", "tag"].iter().enumerate() {
        assert_eq!(
            ids_at(bd, &format!("/nodes/0/filters/{n}/ids")),
            [id_of(k)],
            "builder {k}"
        );
    }
    assert_eq!(
        ids_at(bd, "/nodes/0/groupBy/0/tagGroupId"),
        [id_of("tag_group")]
    );
    assert_eq!(bd["title"], json!(builder_name), "the title is kept");
    h.run(s.close_company()).expect("workspace");
}
