//! The company's team (P29 WP3): users, the permission templates behind
//! roles, and each user's permission row (`company.routes.ts` team
//! management; owner or super admin only, except the user list, which any
//! signed-in user may read, Q-P29y).
//!
//! - **Fail closed (plan §1).** The engine's invite default is
//!   `accountant` (full rights) and a `bookkeeper` without a permission row
//!   has full rights too. [`Team::add`] therefore invites as `readonly`,
//!   then writes the permission row, then changes the role; a failure at
//!   any step leaves the user view-only. [`Team::set_role`] writes the row
//!   before the role for the same reason.
//! - Temporary passwords are [`Secret`]s: never logged, never in `Debug`.
//!   The invite and the temporary-password route are never retried.
//! - Permission maps travel as `key -> level` words; which keys the engine
//!   knows is the caller's business (`cl-model::permissions::template_wire`),
//!   because the engine refuses unknown keys.

use std::collections::BTreeMap;
use std::fmt;

use cl_core::Secret;
use cl_core::ids::UserId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::resources::operations::{
    TEAM_DEACTIVATE, TEAM_INVITE, TEAM_REACTIVATE, TEAM_TEMP_PASSWORD, TEAM_TEMPLATE_DELETE,
    TEAM_TEMPLATE_PUT, TEAM_TEMPLATES, TEAM_TEMPLATES_CREATE, TEAM_UNLOCK, TEAM_USER_PATCH,
    TEAM_USER_PERMISSIONS, TEAM_USER_PERMISSIONS_PUT, TEAM_USERS,
};
use crate::session::{Session, decode, json_body};

/// A permission map: engine key → `none` | `view` | `full`.
pub type PermissionMap = BTreeMap<String, String>;

/// One person on the company's team.
#[derive(Clone, PartialEq, Eq)]
pub struct TeamUser {
    /// The user.
    pub id: UserId,
    /// Sign-in email (owner data; never logged).
    pub email: String,
    /// Name shown in the app (owner data; never logged).
    pub display_name: String,
    /// The engine role in this company (`owner`, `accountant`,
    /// `bookkeeper`, `readonly`).
    pub role: String,
    /// Turned on.
    pub active: bool,
    /// Locked after too many wrong passwords.
    pub locked: bool,
    /// Wrong tries since the last good sign-in.
    pub failed_tries: u32,
    /// Must choose a new password at the next sign-in (U-230).
    pub must_change: bool,
    /// The last sign-in, as the engine wrote it (ISO 8601).
    pub last_sign_in: Option<String>,
    /// When the sign-in was made (ISO 8601): on the desktop the earliest
    /// is the main Admin, who set the company up.
    pub created_at: Option<String>,
}

impl fmt::Debug for TeamUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TeamUser")
            .field("id", &self.id)
            .field("role", &self.role)
            .field("active", &self.active)
            .field("locked", &self.locked)
            .finish_non_exhaustive()
    }
}

/// A named permission template (the engine's; a role in the app).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleTemplate {
    /// Template id.
    pub id: Uuid,
    /// Name (how built-in roles are found again, Q-P29v).
    pub name: String,
    /// Description.
    pub description: Option<String>,
    /// The key map.
    pub permissions: PermissionMap,
}

/// A user's permission row: the template and per-user overrides.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PermissionRow {
    /// The template (None: overrides only).
    pub template: Option<Uuid>,
    /// Per-user overrides.
    pub overrides: PermissionMap,
}

/// Who to add.
#[derive(Clone)]
pub struct NewTeamUser {
    /// Sign-in email.
    pub email: String,
    /// Name shown in the app.
    pub display_name: String,
}

impl fmt::Debug for NewTeamUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewTeamUser").finish_non_exhaustive()
    }
}

/// The role a user should end with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RolePlan {
    /// Engine `readonly`, no row.
    ViewOnly,
    /// Engine `owner`.
    Admin,
    /// Engine `bookkeeper` with this template and no overrides.
    Template(Uuid),
}

/// The step of [`Team::add`] or [`Team::set_role`] that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeamStep {
    /// Creating the user (as view-only).
    Invite,
    /// Writing the permission row.
    Permissions,
    /// Changing the role.
    Role,
}

/// A user added by [`Team::add`].
#[derive(Clone)]
pub struct Added {
    /// The new user.
    pub user: UserId,
    /// Their temporary password, shown once (None when the person already
    /// had a sign-in elsewhere in this workspace).
    pub temporary_password: Option<Secret>,
}

impl fmt::Debug for Added {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Added")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// A sequence that stopped part way. Every stop leaves the user view-only
/// (or, for a role change, as they were).
#[derive(Clone, Debug)]
pub struct TeamFailure {
    /// The step that failed.
    pub step: TeamStep,
    /// The user, when the invite got that far.
    pub added: Option<Added>,
    /// What the engine said.
    pub error: ApiError,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireUser {
    id: UserId,
    #[serde(default)]
    email: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    role: String,
    #[serde(default = "yes")]
    is_active: bool,
    #[serde(default)]
    is_locked: bool,
    #[serde(default)]
    login_failed_attempts: Option<u32>,
    #[serde(default)]
    password_change_required: bool,
    #[serde(default)]
    last_login_at: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
}

fn yes() -> bool {
    true
}

impl From<WireUser> for TeamUser {
    fn from(w: WireUser) -> TeamUser {
        TeamUser {
            id: w.id,
            display_name: w
                .display_name
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| w.email.clone()),
            email: w.email,
            role: w.role,
            active: w.is_active,
            locked: w.is_locked,
            failed_tries: w.login_failed_attempts.unwrap_or(0),
            must_change: w.password_change_required,
            last_sign_in: w.last_login_at,
            created_at: w.created_at,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireTemplate {
    id: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    permissions: PermissionMap,
}

fn uuid_of(s: &str, what: &'static str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s).map_err(|_| ApiError::Decode { what })
}

impl TryFrom<WireTemplate> for RoleTemplate {
    type Error = ApiError;

    fn try_from(w: WireTemplate) -> Result<RoleTemplate, ApiError> {
        Ok(RoleTemplate {
            id: uuid_of(&w.id, "role template")?,
            name: w.name,
            description: w.description,
            permissions: w.permissions,
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TemplateBody<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    permissions: &'a PermissionMap,
}

fn user_path(id: UserId, tail: &str) -> String {
    format!("/api/v1/company/users/{id}{tail}")
}

/// The team of the open company.
#[derive(Debug, Clone, Copy)]
pub struct Team<'a>(&'a Session);

impl Session {
    /// The team of the open company (P29 WP3).
    #[must_use]
    pub fn team(&self) -> Team<'_> {
        Team(self)
    }
}

impl Team<'_> {
    /// Everyone on the team (any signed-in user may read it, Q-P29y).
    ///
    /// # Errors
    /// `Decode`; any [`ApiError`].
    pub async fn users(&self) -> Result<Vec<TeamUser>, ApiError> {
        #[derive(Deserialize)]
        struct Body {
            users: Vec<WireUser>,
        }
        let reply = self
            .0
            .send(&TEAM_USERS, Call::at(Method::Get, TEAM_USERS.route()), None)
            .await?;
        Ok(decode::<Body>(&reply, "team users")?
            .users
            .into_iter()
            .map(TeamUser::from)
            .collect())
    }

    /// Creates a user as view-only (always `readonly`: the engine's own
    /// default is full rights). Never retried.
    ///
    /// # Errors
    /// `Validation` (a bad email, a person already on the team);
    /// `Forbidden` for a non-admin; any [`ApiError`].
    pub async fn invite_view_only(&self, new: &NewTeamUser) -> Result<Added, ApiError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            email: &'a str,
            display_name: &'a str,
            role: &'static str,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Reply {
            user: ReplyUser,
            #[serde(default)]
            temporary_password: Option<String>,
        }
        #[derive(Deserialize)]
        struct ReplyUser {
            id: UserId,
        }
        let call = Call::at(Method::Post, TEAM_INVITE.route()).json_body(json_body(&Body {
            email: new.email.trim(),
            display_name: new.display_name.trim(),
            role: "readonly",
        })?);
        let reply = self.0.send(&TEAM_INVITE, call, None).await?;
        let r: Reply = decode(&reply, "invite")?;
        Ok(Added {
            user: r.user.id,
            temporary_password: r
                .temporary_password
                .filter(|p| !p.is_empty())
                .map(Secret::new),
        })
    }

    /// Changes a user's engine role (`owner`, `bookkeeper`, `readonly`).
    ///
    /// # Errors
    /// `Validation` (the last Admin, your own role); `Forbidden`; any
    /// [`ApiError`].
    pub async fn patch_role(&self, user: UserId, role: &str) -> Result<(), ApiError> {
        let call = Call::new(Method::Patch, TEAM_USER_PATCH.route(), user_path(user, ""))
            .json_body(json_body(&serde_json::json!({ "role": role }))?);
        self.0.send(&TEAM_USER_PATCH, call, None).await.map(drop)
    }

    /// Writes a user's permission row: the template and no overrides (both
    /// always sent: an absent `overrides` keeps the old ones).
    ///
    /// # Errors
    /// `NotFound` for a template of another company; any [`ApiError`].
    pub async fn put_permissions(&self, user: UserId, row: &PermissionRow) -> Result<(), ApiError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            template_id: Option<String>,
            overrides: &'a PermissionMap,
        }
        let call = Call::new(
            Method::Put,
            TEAM_USER_PERMISSIONS_PUT.route(),
            user_path(user, "/permissions"),
        )
        .json_body(json_body(&Body {
            template_id: row.template.map(|u| u.to_string()),
            overrides: &row.overrides,
        })?);
        self.0
            .send(&TEAM_USER_PERMISSIONS_PUT, call, None)
            .await
            .map(drop)
    }

    /// A user's permission row (None: no row; a `bookkeeper` without one
    /// has full rights).
    ///
    /// # Errors
    /// `Decode`; any [`ApiError`].
    pub async fn permissions(&self, user: UserId) -> Result<Option<PermissionRow>, ApiError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Row {
            #[serde(default)]
            template_id: Option<String>,
            #[serde(default)]
            overrides: PermissionMap,
        }
        #[derive(Deserialize)]
        struct Body {
            permissions: Option<Row>,
        }
        let call = Call::new(
            Method::Get,
            TEAM_USER_PERMISSIONS.route(),
            user_path(user, "/permissions"),
        );
        let reply = self.0.send(&TEAM_USER_PERMISSIONS, call, None).await?;
        decode::<Body>(&reply, "permission row")?
            .permissions
            .map(|r| {
                Ok(PermissionRow {
                    template: r
                        .template_id
                        .as_deref()
                        .map(|s| uuid_of(s, "permission row"))
                        .transpose()?,
                    overrides: r.overrides,
                })
            })
            .transpose()
    }

    /// Adds a user the fail-closed way (plan §1): view-only first, then the
    /// permission row, then the role.
    ///
    /// # Errors
    /// [`TeamFailure`] naming the step; the user (when created) stays
    /// view-only and their temporary password is still in `added`.
    pub async fn add(&self, new: &NewTeamUser, plan: RolePlan) -> Result<Added, Box<TeamFailure>> {
        let added = self.invite_view_only(new).await.map_err(|error| {
            Box::new(TeamFailure {
                step: TeamStep::Invite,
                added: None,
                error,
            })
        })?;
        match self.apply(added.user, plan).await {
            Ok(()) => Ok(added),
            Err(mut f) => {
                f.added = Some(added);
                Err(f)
            }
        }
    }

    /// Changes a user's role: for a template role the row is written
    /// before the role, so a failure never leaves more rights than before.
    ///
    /// # Errors
    /// [`TeamFailure`] naming the step (`added` is None).
    pub async fn set_role(&self, user: UserId, plan: RolePlan) -> Result<(), Box<TeamFailure>> {
        self.apply(user, plan).await
    }

    async fn apply(&self, user: UserId, plan: RolePlan) -> Result<(), Box<TeamFailure>> {
        let fail = |step, error| {
            Box::new(TeamFailure {
                step,
                added: None,
                error,
            })
        };
        let role = match plan {
            RolePlan::ViewOnly => "readonly",
            RolePlan::Admin => "owner",
            RolePlan::Template(id) => {
                let row = PermissionRow {
                    template: Some(id),
                    overrides: PermissionMap::new(),
                };
                self.put_permissions(user, &row)
                    .await
                    .map_err(|e| fail(TeamStep::Permissions, e))?;
                "bookkeeper"
            }
        };
        self.patch_role(user, role)
            .await
            .map_err(|e| fail(TeamStep::Role, e))
    }

    /// Turns a user off (they can no longer sign in).
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn turn_off(&self, user: UserId) -> Result<(), ApiError> {
        let call = Call::new(
            Method::Post,
            TEAM_DEACTIVATE.route(),
            user_path(user, "/deactivate"),
        );
        self.0.send(&TEAM_DEACTIVATE, call, None).await.map(drop)
    }

    /// Turns a user back on.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn turn_on(&self, user: UserId) -> Result<(), ApiError> {
        let call = Call::new(
            Method::Post,
            TEAM_REACTIVATE.route(),
            user_path(user, "/reactivate"),
        );
        self.0.send(&TEAM_REACTIVATE, call, None).await.map(drop)
    }

    /// Clears a lockout.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn unlock(&self, user: UserId) -> Result<(), ApiError> {
        let call = Call::new(
            Method::Post,
            TEAM_UNLOCK.route(),
            user_path(user, "/unlock"),
        );
        self.0.send(&TEAM_UNLOCK, call, None).await.map(drop)
    }

    /// Sets a temporary password (U-230, `temp-password-v1`), returned
    /// once. Never retried.
    ///
    /// # Errors
    /// `Forbidden` with `CANNOT_TARGET_SELF`, `SUPER_ADMIN_TARGET` or
    /// `USER_NOT_IN_TENANT`; `NotFound` on an engine without U-230; any
    /// [`ApiError`].
    pub async fn temporary_password(&self, user: UserId) -> Result<Secret, ApiError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Reply {
            temporary_password: String,
        }
        let call = Call::new(
            Method::Post,
            TEAM_TEMP_PASSWORD.route(),
            user_path(user, "/temporary-password"),
        );
        let reply = self.0.send(&TEAM_TEMP_PASSWORD, call, None).await?;
        Ok(Secret::new(
            decode::<Reply>(&reply, "temporary password")?.temporary_password,
        ))
    }

    /// The company's permission templates (roles).
    ///
    /// # Errors
    /// `Forbidden` for a non-admin; any [`ApiError`].
    pub async fn templates(&self) -> Result<Vec<RoleTemplate>, ApiError> {
        #[derive(Deserialize)]
        struct Body {
            templates: Vec<WireTemplate>,
        }
        let reply = self
            .0
            .send(
                &TEAM_TEMPLATES,
                Call::at(Method::Get, TEAM_TEMPLATES.route()),
                None,
            )
            .await?;
        decode::<Body>(&reply, "role templates")?
            .templates
            .into_iter()
            .map(RoleTemplate::try_from)
            .collect()
    }

    /// Creates a template.
    ///
    /// # Errors
    /// `Validation` (`TEMPLATE_NAME_TAKEN`, an unknown key); any
    /// [`ApiError`].
    pub async fn create_template(
        &self,
        name: &str,
        description: Option<&str>,
        permissions: &PermissionMap,
    ) -> Result<RoleTemplate, ApiError> {
        #[derive(Deserialize)]
        struct Body {
            template: WireTemplate,
        }
        let call = Call::at(Method::Post, TEAM_TEMPLATES_CREATE.route()).json_body(json_body(
            &TemplateBody {
                name,
                description,
                permissions,
            },
        )?);
        let reply = self.0.send(&TEAM_TEMPLATES_CREATE, call, None).await?;
        decode::<Body>(&reply, "role template")?.template.try_into()
    }

    /// Replaces a template's name and map (last write wins).
    ///
    /// # Errors
    /// `NotFound`; `Validation`; any [`ApiError`].
    pub async fn update_template(
        &self,
        id: Uuid,
        name: &str,
        permissions: &PermissionMap,
    ) -> Result<RoleTemplate, ApiError> {
        #[derive(Deserialize)]
        struct Body {
            template: WireTemplate,
        }
        let call = Call::new(
            Method::Put,
            TEAM_TEMPLATE_PUT.route(),
            format!("/api/v1/company/permission-templates/{id}"),
        )
        .json_body(json_body(&TemplateBody {
            name,
            description: None,
            permissions,
        })?);
        let reply = self.0.send(&TEAM_TEMPLATE_PUT, call, None).await?;
        decode::<Body>(&reply, "role template")?.template.try_into()
    }

    /// Deletes a template (its holders keep their overrides only).
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn delete_template(&self, id: Uuid) -> Result<(), ApiError> {
        let call = Call::new(
            Method::Delete,
            TEAM_TEMPLATE_DELETE.route(),
            format!("/api/v1/company/permission-templates/{id}"),
        );
        self.0
            .send(&TEAM_TEMPLATE_DELETE, call, None)
            .await
            .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, requests};
    use serde_json::Value;
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer};

    const UID: &str = "7d0e8f43-1f5c-4d4e-9a55-2d0f2b1c0a01";
    const TID: &str = "3b8d1c55-6a0e-4f21-b0c2-6a1e0f9d4c11";

    async fn invite_ok(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/api/v1/company/invite-user"))
            .respond_with(json(
                201,
                &format!(
                    r#"{{"user":{{"id":"{UID}","email":"x@example.com","role":"readonly"}},"temporaryPassword":"TEMP-CANARY","existingUser":false}}"#
                ),
            ))
            .mount(server)
            .await;
    }

    fn new_user() -> NewTeamUser {
        NewTeamUser {
            email: " x@example.com ".into(),
            display_name: "X Person".into(),
        }
    }

    fn template() -> RolePlan {
        RolePlan::Template(TID.parse().expect("uuid"))
    }

    #[tokio::test]
    async fn users_read_the_lock_and_must_change_flags() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/company/users"))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"users":[{{"id":"{UID}","email":"a@example.com","displayName":"A","role":"bookkeeper","isActive":true,"isLocked":true,"loginFailedAttempts":5,"passwordChangeRequired":true,"lastLoginAt":"2026-11-01T10:00:00Z","shareAllowed":null}}]}}"#
                ),
            ))
            .mount(&server)
            .await;
        let users = company_session(&server).team().users().await.expect("ok");
        assert_eq!(users.len(), 1);
        let u = &users[0];
        assert!(u.locked && u.must_change && u.active);
        assert_eq!(u.failed_tries, 5);
        assert_eq!(u.last_sign_in.as_deref(), Some("2026-11-01T10:00:00Z"));
        assert!(!format!("{u:?}").contains("a@example.com"));
    }

    /// The happy path sends readonly, then the row (template and empty
    /// overrides), then the role, and keeps the temporary password secret.
    #[tokio::test]
    async fn add_invites_view_only_then_row_then_role() {
        let server = MockServer::start().await;
        invite_ok(&server).await;
        Mock::given(method("PUT"))
            .and(path_regex(r"/permissions$"))
            .respond_with(json(200, r#"{"permissions":{}}"#))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .respond_with(json(200, r#"{"user":{}}"#))
            .mount(&server)
            .await;
        let added = company_session(&server)
            .team()
            .add(&new_user(), template())
            .await
            .expect("added");
        assert_eq!(
            added.temporary_password.as_ref().map(Secret::expose),
            Some("TEMP-CANARY")
        );
        assert!(!format!("{added:?}").contains("CANARY"));
        let posts = requests(&server, "POST").await;
        let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
        assert_eq!(body["role"], "readonly");
        assert_eq!(body["email"], "x@example.com");
        let puts = requests(&server, "PUT").await;
        let row: Value = serde_json::from_slice(&puts[0].body).expect("json");
        assert_eq!(row["templateId"], TID);
        assert_eq!(row["overrides"], serde_json::json!({}));
        let patches = requests(&server, "PATCH").await;
        let role: Value = serde_json::from_slice(&patches[0].body).expect("json");
        assert_eq!(role["role"], "bookkeeper");
    }

    /// A failed row write stops before the role: the user stays view-only
    /// and the temporary password is still handed back.
    #[tokio::test]
    async fn add_stops_at_a_failed_row_and_never_raises_the_role() {
        let server = MockServer::start().await;
        invite_ok(&server).await;
        Mock::given(method("PUT"))
            .respond_with(json(
                400,
                r#"{"error":{"message":"bad","code":"VALIDATION_ERROR"}}"#,
            ))
            .mount(&server)
            .await;
        let f = company_session(&server)
            .team()
            .add(&new_user(), template())
            .await
            .expect_err("stops");
        assert_eq!(f.step, TeamStep::Permissions);
        assert!(
            f.added
                .as_ref()
                .is_some_and(|a| a.temporary_password.is_some())
        );
        assert!(
            requests(&server, "PATCH").await.is_empty(),
            "role untouched"
        );
    }

    #[tokio::test]
    async fn add_reports_each_failing_step() {
        // The invite itself.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(json(
                403,
                r#"{"error":{"message":"no","code":"FORBIDDEN"}}"#,
            ))
            .mount(&server)
            .await;
        let f = company_session(&server)
            .team()
            .add(&new_user(), RolePlan::Admin)
            .await
            .expect_err("refused");
        assert_eq!(f.step, TeamStep::Invite);
        assert!(f.added.is_none());
        assert_eq!(requests(&server, "POST").await.len(), 1, "never retried");
        // The role step.
        let server = MockServer::start().await;
        invite_ok(&server).await;
        Mock::given(method("PATCH"))
            .respond_with(json(
                400,
                r#"{"error":{"message":"no","code":"VALIDATION_ERROR"}}"#,
            ))
            .mount(&server)
            .await;
        let f = company_session(&server)
            .team()
            .add(&new_user(), RolePlan::Admin)
            .await
            .expect_err("role refused");
        assert_eq!(f.step, TeamStep::Role);
        assert!(
            requests(&server, "PUT").await.is_empty(),
            "Admin has no row"
        );
        // View-only: only the invite.
        let server = MockServer::start().await;
        invite_ok(&server).await;
        Mock::given(method("PATCH"))
            .respond_with(json(200, r#"{"user":{}}"#))
            .mount(&server)
            .await;
        company_session(&server)
            .team()
            .add(&new_user(), RolePlan::ViewOnly)
            .await
            .expect("ok");
    }

    #[tokio::test]
    async fn temporary_password_is_secret_and_sent_once() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"/temporary-password$"))
            .respond_with(json(200, r#"{"temporaryPassword":"0123456789CANARY"}"#))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(json(
                403,
                r#"{"error":{"message":"no","code":"SUPER_ADMIN_TARGET"}}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let uid: UserId = UID.parse().expect("id");
        let p = s.team().temporary_password(uid).await.expect("ok");
        assert_eq!(p.expose(), "0123456789CANARY");
        assert!(!format!("{p:?}").contains("CANARY"));
        let e = s.team().temporary_password(uid).await.expect_err("refused");
        assert!(
            matches!(&e, ApiError::Forbidden { code: Some(c) } if c.as_str() == "SUPER_ADMIN_TARGET"),
            "{e:?}"
        );
        assert_eq!(requests(&server, "POST").await.len(), 2);
    }

    #[tokio::test]
    async fn templates_and_rows_round_trip() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/company/permission-templates"))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"templates":[{{"id":"{TID}","tenantId":"t","name":"Accounts payable","description":null,"permissions":{{"bills":"full"}},"createdAt":"x","updatedAt":"x"}}]}}"#
                ),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"/permissions$"))
            .respond_with(json(200, r#"{"permissions":null}"#))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/company/permission-templates"))
            .respond_with(json(
                201,
                &format!(
                    r#"{{"template":{{"id":"{TID}","name":"Front desk","permissions":{{"invoices":"view"}}}}}}"#
                ),
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let t = s.team().templates().await.expect("list");
        assert_eq!(t[0].name, "Accounts payable");
        assert_eq!(t[0].permissions["bills"], "full");
        let uid: UserId = UID.parse().expect("id");
        assert_eq!(s.team().permissions(uid).await.expect("row"), None);
        let mut map = PermissionMap::new();
        map.insert("invoices".into(), "view".into());
        let made = s
            .team()
            .create_template("Front desk", None, &map)
            .await
            .expect("made");
        assert_eq!(made.permissions, map);
        let posts = requests(&server, "POST").await;
        let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
        assert_eq!(body["name"], "Front desk");
        assert!(body.get("description").is_none());
    }
}
