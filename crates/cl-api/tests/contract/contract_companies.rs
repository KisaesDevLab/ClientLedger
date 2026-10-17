//! Companies: list (= accessibleTenants minus the workspace), create, open,
//! close, profile read/update, settings read (AC1).

use cl_api::{AccountFilter, ApiError, Method, PageRequest, Scope};

use crate::common::key;
use crate::support::{CONTRACT_CO, Harness, find_company};

pub fn list_excludes_the_workspace_and_create_is_listed(h: &Harness) {
    let info = h.run(h.client.desktop_info()).expect("info");
    let workspace = info.workspace_tenant_id.expect("workspace tenant");
    let b = h.tenant_b();
    let list = h.run(h.s1.companies()).expect("companies");
    assert!(
        list.iter().all(|c| c.tenant_id != workspace),
        "workspace listed"
    );
    assert!(list.iter().any(|c| c.tenant_name == CONTRACT_CO));
    assert!(list.iter().any(|c| c.tenant_id == b), "new company listed");
    assert_eq!(
        h.observer.sent(Method::Post, "/api/v1/auth/create-tenant"),
        1
    );
}

pub fn open_close_and_scope_epoch(h: &Harness) {
    let s = h.s3();
    let b = h.tenant_b();
    let epoch = s.scope_epoch();
    h.run(s.close_company()).expect("close");
    assert_eq!(s.scope(), Scope::Workspace);
    assert!(s.scope_epoch() > epoch);
    // A company resource in the workspace fails locally: nothing is sent.
    let before = h.observer.total();
    let err = h
        .run(
            s.accounts()
                .list(&AccountFilter::default(), PageRequest::first_max()),
        )
        .expect_err("workspace");
    assert!(matches!(err, ApiError::NoCompanyOpen), "{err:?}");
    assert_eq!(h.observer.total(), before, "a request was sent");
    // The engine's own fence: a raw request in the workspace is a 409
    // WORKSPACE_TENANT, which cl-api maps to NoCompanyOpen.
    let raw = h
        .run(s.raw(Method::Get, "/api/v1/accounts", &[], None))
        .expect("raw");
    assert_eq!(raw.status, 409);
    assert_eq!(raw.body["error"]["code"], "WORKSPACE_TENANT");
    let mapped = h
        .run(s.raw_mapped(Method::Get, "/api/v1/accounts", None))
        .expect_err("mapped");
    assert!(matches!(mapped, ApiError::NoCompanyOpen), "{mapped:?}");
    // Reopen.
    let e2 = s.scope_epoch();
    let open = h.run(s.open_company(b)).expect("open");
    assert_eq!(open.tenant_id, b);
    assert!(matches!(s.scope(), Scope::Company { tenant_id, .. } if tenant_id == b));
    assert!(s.scope_epoch() > e2);
    let co = h.run(find_company(&s, CONTRACT_CO)).expect("listed");
    assert_ne!(co.tenant_id, b);
}

pub fn profile_round_trip_and_settings(h: &Harness) {
    let s = h.s3();
    let got = h.run(s.company_profile()).expect("profile");
    let mut update = got.record.profile.clone();
    update.city = Some("Springfield".into());
    update.phone = Some("555-0100".into());
    let saved = h
        .run(s.update_company_profile(&got.version, &update, key()))
        .expect("update");
    assert_eq!(saved.record.profile.city.as_deref(), Some("Springfield"));
    let again = h.run(s.company_profile()).expect("profile");
    assert_eq!(again.record.profile, update);
    assert_eq!(again.version, saved.version);
    let settings = h.run(s.company_settings()).expect("settings");
    assert!(settings.fiscal_year_start_month.is_some());
}
