//! The harness itself: one boot, one login, Contract Co open, loopback only.

use cl_api::{Method, Scope};

use crate::support::{CONTRACT_CO, Harness};

pub fn boots_once_and_opens_contract_co(h: &Harness) {
    // Opened by the harness through switch-tenant.
    match h.s1.scope() {
        Scope::Company { tenant_id, .. } => assert_eq!(tenant_id, h.contract_co),
        Scope::Workspace => panic!("Contract Co is not open"),
    }
    let info = h.run(h.client.desktop_info()).expect("desktop info");
    assert_eq!(info.initialized, Some(true));
    assert!(info.workspace_tenant_id.is_some(), "E1 workspace tenant");
    let profile = h.run(h.s1.company_profile()).expect("company profile");
    assert_eq!(profile.record.profile.business_name, CONTRACT_CO);
    // Exactly one login so far for the whole binary.
    assert_eq!(h.observer.sent(Method::Post, "/api/v1/auth/login"), 1);
}
