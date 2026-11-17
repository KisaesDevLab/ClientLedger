//! The demo's team management (P29 WP3), with the engine's rules
//! (`company.routes.ts`, `auth.service.ts`, `permission.service.ts`): only
//! an Admin manages the team; an invite is view-only first; a permission
//! row before a role; no temporary password for yourself or the main
//! Admin; an Admin can't be turned off; template names are unique.
//! `DemoAuth::fail_at` makes one step fail once (tests, scenarios).
//! Every value here is synthetic.

use cl_api::{
    Added, ApiError, NewTeamUser, PermissionMap, PermissionRow, RolePlan, RoleTemplate,
    TeamFailure, TeamStep, TeamUser,
};
use cl_core::Secret;
use cl_core::ids::UserId;
use uuid::Uuid;

use super::auth::{DemoAuth, DemoUser, demo_user_id};

/// The temporary password the demo hands out (synthetic).
pub const DEMO_TEMP_PASSWORD: &str = "TEMP-0000-DEMO-2";

fn refusal(status: u16, code: &str) -> ApiError {
    cl_api::engine_refusal(
        status,
        &serde_json::json!({"error": {"code": code, "message": "refused"}}),
    )
}

fn not_found() -> ApiError {
    refusal(404, "NOT_FOUND")
}

impl DemoAuth {
    fn admin(&self) -> Result<(), ApiError> {
        match self.current() {
            Some(u) if u.role == "owner" || u.super_admin => Ok(()),
            _ => Err(refusal(403, "FORBIDDEN")),
        }
    }

    fn step(&mut self, step: TeamStep) -> Result<(), ApiError> {
        if self.fail_at == Some(step) {
            self.fail_at = None;
            return Err(refusal(400, "VALIDATION_ERROR"));
        }
        Ok(())
    }

    fn user_mut(&mut self, id: UserId) -> Result<&mut DemoUser, ApiError> {
        self.users
            .iter_mut()
            .find(|u| u.id == id)
            .ok_or_else(not_found)
    }

    /// Everyone on the team, as `GET /company/users` answers.
    #[must_use]
    pub fn team(&self) -> Vec<TeamUser> {
        self.users
            .iter()
            .enumerate()
            .map(|(i, u)| TeamUser {
                id: u.id,
                email: u.email.clone(),
                display_name: u.display_name.clone(),
                role: u.role.clone(),
                active: u.active,
                locked: u.locked,
                failed_tries: u32::from(u.failed),
                must_change: u.must_change,
                last_sign_in: u.last_sign_in.clone(),
                // Made in list order: the main Admin first, as on the
                // desktop engine.
                created_at: Some(format!("2026-01-01T08:{:02}:00.000Z", i.min(59))),
            })
            .collect()
    }

    /// A user's permission row.
    ///
    /// # Errors
    /// `Forbidden` for a non-admin.
    pub fn permission_row(&self, id: UserId) -> Result<Option<PermissionRow>, ApiError> {
        self.admin()?;
        Ok(self
            .users
            .iter()
            .find(|u| u.id == id)
            .and_then(|u| u.row)
            .map(|template| PermissionRow {
                template,
                overrides: PermissionMap::new(),
            }))
    }

    /// The fail-closed add, step by step like [`cl_api::Team::add`].
    ///
    /// # Errors
    /// [`TeamFailure`] naming the step; the user stays view-only.
    pub fn add_user(
        &mut self,
        new: &NewTeamUser,
        plan: RolePlan,
    ) -> Result<Added, Box<TeamFailure>> {
        let fail = |step, error, added: Option<Added>| Box::new(TeamFailure { step, added, error });
        self.admin().map_err(|e| fail(TeamStep::Invite, e, None))?;
        self.step(TeamStep::Invite)
            .map_err(|e| fail(TeamStep::Invite, e, None))?;
        let email = new.email.trim();
        if email.is_empty() || !email.contains('@') || new.display_name.trim().is_empty() {
            return Err(fail(
                TeamStep::Invite,
                refusal(400, "VALIDATION_ERROR"),
                None,
            ));
        }
        if self
            .users
            .iter()
            .any(|u| u.email.eq_ignore_ascii_case(email))
        {
            return Err(fail(TeamStep::Invite, refusal(409, "CONFLICT"), None));
        }
        let mut user = DemoUser::invited(email, new.display_name.trim(), DEMO_TEMP_PASSWORD);
        user.id = demo_user_id(self.users.len());
        let added = Added {
            user: user.id,
            temporary_password: Some(Secret::new(DEMO_TEMP_PASSWORD)),
        };
        self.users.push(user);
        match self.apply(added.user, plan) {
            Ok(()) => Ok(added),
            Err((step, e)) => Err(fail(step, e, Some(added))),
        }
    }

    /// A role change: the row before the role.
    ///
    /// # Errors
    /// [`TeamFailure`] naming the step.
    pub fn set_user_role(&mut self, id: UserId, plan: RolePlan) -> Result<(), Box<TeamFailure>> {
        let fail = |(step, error)| {
            Box::new(TeamFailure {
                step,
                added: None,
                error,
            })
        };
        self.admin().map_err(|e| fail((TeamStep::Role, e)))?;
        if self.current().is_some_and(|u| u.id == id) {
            return Err(fail((TeamStep::Role, refusal(400, "VALIDATION_ERROR"))));
        }
        self.apply(id, plan).map_err(fail)
    }

    #[allow(clippy::result_large_err)] // private; the public callers box it
    fn apply(&mut self, id: UserId, plan: RolePlan) -> Result<(), (TeamStep, ApiError)> {
        let role = match plan {
            RolePlan::ViewOnly => "readonly",
            RolePlan::Admin => "owner",
            RolePlan::Template(t) => {
                self.step(TeamStep::Permissions)
                    .map_err(|e| (TeamStep::Permissions, e))?;
                if !self.templates.iter().any(|x| x.id == t) {
                    return Err((TeamStep::Permissions, not_found()));
                }
                self.user_mut(id)
                    .map_err(|e| (TeamStep::Permissions, e))?
                    .row = Some(Some(t));
                "bookkeeper"
            }
        };
        self.step(TeamStep::Role).map_err(|e| (TeamStep::Role, e))?;
        let owners = self
            .users
            .iter()
            .filter(|u| u.role == "owner" && u.active)
            .count();
        let user = self.user_mut(id).map_err(|e| (TeamStep::Role, e))?;
        if user.role == "owner" && role != "owner" && owners <= 1 {
            return Err((TeamStep::Role, refusal(400, "VALIDATION_ERROR")));
        }
        role.clone_into(&mut user.role);
        Ok(())
    }

    /// Turn a user off or on.
    ///
    /// # Errors
    /// `Forbidden`; `Validation` for an Admin ("Cannot deactivate the
    /// owner") or yourself.
    pub fn set_active(&mut self, id: UserId, on: bool) -> Result<(), ApiError> {
        self.admin()?;
        if self.current().is_some_and(|u| u.id == id) {
            return Err(refusal(400, "VALIDATION_ERROR"));
        }
        let user = self.user_mut(id)?;
        if !on && user.role == "owner" {
            return Err(refusal(400, "VALIDATION_ERROR"));
        }
        user.active = on;
        Ok(())
    }

    /// Clear a lockout.
    ///
    /// # Errors
    /// `Forbidden`; `NotFound`.
    pub fn unlock_user(&mut self, id: UserId) -> Result<(), ApiError> {
        self.admin()?;
        let user = self.user_mut(id)?;
        user.locked = false;
        user.failed = 0;
        Ok(())
    }

    /// A temporary password (U-230): never for yourself or the main Admin.
    ///
    /// # Errors
    /// `Forbidden` with `CANNOT_TARGET_SELF` or `SUPER_ADMIN_TARGET`.
    pub fn temporary_password(&mut self, id: UserId) -> Result<Secret, ApiError> {
        self.admin()?;
        if self.current().is_some_and(|u| u.id == id) {
            return Err(refusal(403, "CANNOT_TARGET_SELF"));
        }
        let user = self.user_mut(id)?;
        if user.super_admin {
            return Err(refusal(403, "SUPER_ADMIN_TARGET"));
        }
        user.set_password(DEMO_TEMP_PASSWORD);
        user.must_change = true;
        user.locked = false;
        user.failed = 0;
        Ok(Secret::new(DEMO_TEMP_PASSWORD))
    }

    /// The templates.
    ///
    /// # Errors
    /// `Forbidden` for a non-admin.
    pub fn role_templates(&self) -> Result<Vec<RoleTemplate>, ApiError> {
        self.admin()?;
        let mut out = self.templates.clone();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// Creates a template (names are unique).
    ///
    /// # Errors
    /// `Forbidden`; `Validation` with `TEMPLATE_NAME_TAKEN`.
    pub fn create_role_template(
        &mut self,
        name: &str,
        permissions: &PermissionMap,
    ) -> Result<RoleTemplate, ApiError> {
        self.admin()?;
        if name.trim().is_empty() {
            return Err(refusal(400, "VALIDATION_ERROR"));
        }
        if self.templates.iter().any(|t| t.name == name.trim()) {
            return Err(refusal(400, "TEMPLATE_NAME_TAKEN"));
        }
        let n = u128::try_from(self.templates.len()).unwrap_or(0) + 1;
        let t = RoleTemplate {
            id: Uuid::from_u128(0x7e3a_0000_0000_4000_8000_0000_0000_0000 + n),
            name: name.trim().to_owned(),
            description: None,
            permissions: permissions.clone(),
        };
        self.templates.push(t.clone());
        Ok(t)
    }

    /// Replaces a template's name and map.
    ///
    /// # Errors
    /// `Forbidden`; `NotFound`; `TEMPLATE_NAME_TAKEN`.
    pub fn update_role_template(
        &mut self,
        id: Uuid,
        name: &str,
        permissions: &PermissionMap,
    ) -> Result<RoleTemplate, ApiError> {
        self.admin()?;
        if self
            .templates
            .iter()
            .any(|t| t.id != id && t.name == name.trim())
        {
            return Err(refusal(400, "TEMPLATE_NAME_TAKEN"));
        }
        let t = self
            .templates
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(not_found)?;
        name.trim().clone_into(&mut t.name);
        t.permissions = permissions.clone();
        Ok(t.clone())
    }

    /// Deletes a template; its holders keep a row without one.
    ///
    /// # Errors
    /// `Forbidden`; `NotFound`.
    pub fn delete_role_template(&mut self, id: Uuid) -> Result<(), ApiError> {
        self.admin()?;
        let before = self.templates.len();
        self.templates.retain(|t| t.id != id);
        if self.templates.len() == before {
            return Err(not_found());
        }
        for u in &mut self.users {
            if u.row == Some(Some(id)) {
                u.row = Some(None);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_user() -> NewTeamUser {
        NewTeamUser {
            email: "rio@example.com".into(),
            display_name: "Rio Sales".into(),
        }
    }

    fn template(a: &mut DemoAuth) -> Uuid {
        a.create_role_template("Accounts receivable", &PermissionMap::new())
            .expect("made")
            .id
    }

    /// Each failing step leaves the new user view-only (plan §1).
    #[test]
    fn a_failed_add_leaves_the_user_view_only() {
        for (step, role) in [
            (TeamStep::Permissions, "readonly"),
            (TeamStep::Role, "readonly"),
        ] {
            let mut a = DemoAuth::default();
            let t = template(&mut a);
            a.fail_at = Some(step);
            let f = a
                .add_user(&new_user(), RolePlan::Template(t))
                .expect_err("fails");
            assert_eq!(f.step, step);
            let added = f.added.expect("created");
            assert!(added.temporary_password.is_some());
            let u = a
                .team()
                .into_iter()
                .find(|u| u.id == added.user)
                .expect("u");
            assert_eq!(u.role, role, "{step:?}");
            assert!(u.must_change);
        }
        let mut a = DemoAuth::default();
        a.fail_at = Some(TeamStep::Invite);
        let f = a.add_user(&new_user(), RolePlan::Admin).expect_err("fails");
        assert!(f.added.is_none());
        assert_eq!(a.team().len(), 4);
        // And the happy path.
        let t = template(&mut a);
        let added = a.add_user(&new_user(), RolePlan::Template(t)).expect("ok");
        assert_eq!(
            a.permission_row(added.user).expect("row"),
            Some(PermissionRow {
                template: Some(t),
                overrides: PermissionMap::new()
            })
        );
        assert!(
            a.add_user(&new_user(), RolePlan::ViewOnly).is_err(),
            "already on the team"
        );
    }

    #[test]
    fn temporary_passwords_skip_yourself_and_the_main_admin() {
        let mut a = DemoAuth::default();
        let owner = a.users[0].id;
        let lee = a.users[1].id;
        let kim = a.users[2].id;
        let code = |e: ApiError| match e {
            ApiError::Forbidden { code: Some(c) } => c.as_str().to_owned(),
            other => format!("{other:?}"),
        };
        assert_eq!(
            code(a.temporary_password(owner).expect_err("self")),
            "CANNOT_TARGET_SELF"
        );
        a.users[1].role = "owner".into();
        a.signed_in = Some(a.users[1].email.clone());
        assert_eq!(
            code(a.temporary_password(owner).expect_err("main admin")),
            "SUPER_ADMIN_TARGET"
        );
        let p = a.temporary_password(kim).expect("kim");
        assert_eq!(p.expose(), DEMO_TEMP_PASSWORD);
        let k = a.team().into_iter().find(|u| u.id == kim).expect("kim");
        assert!(k.must_change && !k.locked);
        let _ = lee;
    }

    #[test]
    fn only_an_admin_manages_and_admins_stay_on() {
        let mut a = DemoAuth::default();
        let owner = a.users[0].id;
        let lee = a.users[1].id;
        assert!(a.set_active(owner, false).is_err(), "not yourself");
        a.unlock_user(a.users[2].id).expect("unlock");
        assert!(!a.users[2].locked);
        a.set_active(lee, false).expect("off");
        a.set_active(lee, true).expect("on");
        a.signed_in = Some(a.users[1].email.clone());
        assert!(matches!(
            a.role_templates(),
            Err(ApiError::Forbidden { .. })
        ));
        assert!(a.unlock_user(owner).is_err());
        // Deleting a template leaves its holders on a row without one.
        let mut a = DemoAuth::default();
        let t = template(&mut a);
        a.set_user_role(lee, RolePlan::Template(t)).expect("role");
        a.delete_role_template(t).expect("deleted");
        assert_eq!(a.users[1].row, Some(None));
        assert!(
            a.create_role_template("X", &PermissionMap::new()).is_ok()
                && a.create_role_template("X", &PermissionMap::new()).is_err()
        );
    }
}
