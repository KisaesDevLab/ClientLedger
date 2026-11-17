//! P29 WP3: team management against the live engine (plan §1, §3 WP3).
//!
//! - Add is fail closed: view-only first, then the permission row, then
//!   the role; a stop at the row (a template of no company) leaves the new
//!   user `readonly`.
//! - A non-admin is refused the team routes (403) but may read the list
//!   (Q-P29y).
//! - A temporary password is refused for yourself (U-230).
//!
//! Every email is synthetic (`@example.com`). Positive saves for an A/P
//! user are in `contract_rights` (the A/P built-in runs A/P aging and is
//! refused invoices).

use cl_api::{ApiError, NewTeamUser, PermissionMap, RolePlan, TeamStep};

use crate::support::{Harness, unique};

/// U-227's capability: the engine has `financial_reports`.
const REPORT_AREAS: &str = "report-areas-v1";
/// U-230's capability.
const TEMP_PASSWORD: &str = "temp-password-v1";

/// The A/P built-in, only keys the engine knows (`financial_reports` needs
/// U-227; there is no `item_costs`).
fn ap_map(with_areas: bool) -> PermissionMap {
    let mut m: PermissionMap = [
        ("recurring", "full"),
        ("contacts", "full"),
        ("attachments", "full"),
        ("checks", "full"),
        ("bills", "full"),
        ("pay_bills", "full"),
        ("vendor_credits", "full"),
        ("tags", "view"),
        ("items", "view"),
        ("accounts", "view"),
        ("reports", "view"),
        ("dashboard", "view"),
        ("company_settings", "view"),
        ("invoices", "none"),
        ("transactions", "none"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    if with_areas {
        m.insert("financial_reports".into(), "none".into());
    }
    m
}

fn new_user(prefix: &str) -> NewTeamUser {
    let name = unique(prefix);
    NewTeamUser {
        email: format!("{}@example.com", name.to_lowercase().replace(' ', "-")),
        display_name: name,
    }
}

/// The happy path ends as a `bookkeeper` on the template; a row write that
/// fails leaves the user `readonly`.
pub fn add_is_view_only_until_the_row_is_written(h: &Harness) {
    let team = h.s1.team();
    let tpl = h
        .run(team.create_template(
            &unique("Contract AP"),
            None,
            &ap_map(h.caps.has(REPORT_AREAS)),
        ))
        .unwrap_or_else(|e| panic!("template: {e}"));
    let added = h
        .run(team.add(&new_user("Team AP"), RolePlan::Template(tpl.id)))
        .unwrap_or_else(|f| panic!("add stopped at {:?}: {}", f.step, f.error));
    let users = h.run(team.users()).expect("users");
    let u = users.iter().find(|u| u.id == added.user).expect("listed");
    assert_eq!(u.role, "bookkeeper");
    let row = h
        .run(team.permissions(added.user))
        .expect("row")
        .expect("has a row");
    assert_eq!(row.template, Some(tpl.id));
    assert!(row.overrides.is_empty());

    // A template no company has: the row write fails, the role is never set.
    let f = h
        .run(team.add(
            &new_user("Team Stop"),
            RolePlan::Template(uuid::Uuid::new_v4()),
        ))
        .expect_err("stops at the row");
    assert_eq!(f.step, TeamStep::Permissions, "{}", f.error);
    let stopped = f.added.expect("the user was created");
    let users = h.run(team.users()).expect("users");
    let u = users.iter().find(|u| u.id == stopped.user).expect("listed");
    assert_eq!(u.role, "readonly", "left view-only");
}

/// A bookkeeper (full rights, no Admin) may read the list but not manage.
pub fn non_admin_is_refused_team_changes(h: &Harness) {
    let (_, bk) = h.bookkeeper();
    let team = bk.team();
    assert!(h.run(team.users()).is_ok(), "the list is ungated (Q-P29y)");
    let e = h.run(team.templates()).expect_err("templates refused");
    assert!(matches!(e, ApiError::Forbidden { .. }), "{e:?}");
    let e = h
        .run(team.invite_view_only(&new_user("Team Nope")))
        .expect_err("invite refused");
    assert!(matches!(e, ApiError::Forbidden { .. }), "{e:?}");
}

/// U-230: no temporary password for yourself.
pub fn temporary_password_is_refused_for_yourself(h: &Harness) {
    if !h.require_cap(TEMP_PASSWORD, "U-230") {
        return;
    }
    let me = h.s1.user_id().expect("signed in");
    let e = h
        .run(h.s1.team().temporary_password(me))
        .expect_err("self refused");
    assert!(
        matches!(&e, ApiError::Forbidden { code: Some(c) } if c.as_str() == "CANNOT_TARGET_SELF"),
        "{e:?}"
    );
    // Someone else gets one, shown once.
    let added = h
        .run(h.s1.team().add(&new_user("Team Temp"), RolePlan::ViewOnly))
        .unwrap_or_else(|f| panic!("add: {}", f.error));
    let p = h
        .run(h.s1.team().temporary_password(added.user))
        .expect("temporary password");
    assert_ne!(p.expose(), "", "a password came back");
    let users = h.run(h.s1.team().users()).expect("users");
    assert!(
        users.iter().any(|u| u.id == added.user && u.must_change),
        "must change at the next sign-in"
    );
}
