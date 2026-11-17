//! Company › Users and Permissions (P29 WP3, Admin only): the Users tab
//! (role, on/off, locked, last sign-in, must change password; Add, change
//! role, Turn off/on, Unlock, Set a temporary password, Make a new
//! recovery key) and the Roles tab (built-in roles recreated by name when
//! missing, Q-P29v; custom roles copied from a role and edited on a
//! none/view/full grid grouped as the engine groups rights).
//!
//! - **Fail closed (plan §1).** Add invites view-only, writes the
//!   permission row, then sets the role; a stop at any step leaves the user
//!   view-only and the screen says so. A role change writes the row first.
//! - Temporary passwords are shown once in a copyable box (masked in
//!   control-channel screenshots), never logged, never in `state()`, and
//!   dropped when the box is closed.
//! - Adding or turning on a second active user turns automatic sign-in off
//!   (Q-P29o, Q-P29-D2).
//! - Logs carry ids only (§9).

use std::collections::BTreeMap;
use std::sync::Arc;

use cl_api::{
    Added, ApiError, NewTeamUser, OrphanLabel, PermissionRow, RequestGroup, RolePlan, RoleTemplate,
    TeamFailure, TeamStep, TeamUser,
};
use cl_core::Secret;
use cl_core::ids::UserId;
use cl_model::CommandId;
use cl_model::permissions::{BuiltinRole, Level, Resource};
use cl_model::roles::{
    EngineKeys, Group, RoleKind, builtin_by_name, builtin_wire, classify, group_of,
    levels_from_wire, right_key, wire_map,
};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::company::{CompanyEvent, CompanyLink};
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::widgets::primary_button;
use crate::windows::Screen;

/// The window kind.
pub const KIND: &str = "company.users";

/// U-227's capability: the engine has `financial_reports`.
pub const REPORT_AREAS: &str = "report-areas-v1";
/// U-33's capability: the engine has `sensitive_accounts`.
pub const SENSITIVE_ACCOUNTS: &str = "sensitive-accounts-v1";
/// The engine lets a role carry "See sensitive accounts" (owner
/// 2026-11-17: the built-in Bookkeeper does). With it the Bookkeeper
/// template is put back with the right the next time an Admin opens Users
/// and Roles (the put-back below; built-ins can't be edited in place).
pub const SENSITIVE_GRANT: &str = "sensitive-accounts-grant-v1";

/// The window for `kind`, if it is this one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    _open: Option<(cl_core::ids::TenantId, String)>,
    _today: cl_core::CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND).then(|| Box::new(UsersScreen::new(Arc::clone(link))) as _)
}

/// Everything the window reads.
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    users: Vec<TeamUser>,
    templates: Vec<RoleTemplate>,
    /// Bookkeepers' rows (None: no row).
    rows: BTreeMap<UserId, Option<PermissionRow>>,
}

/// A finished write.
enum Wrote {
    Added(Result<Added, Box<TeamFailure>>),
    Role(Result<(), Box<TeamFailure>>),
    Done(&'static str),
    TempPassword(UserId, Secret),
    Builtins,
    RoleSaved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Users,
    Roles,
}

/// The role being edited on the Roles tab.
#[derive(Clone, Debug)]
struct RoleEdit {
    /// The custom template saved over (None: a new role).
    id: Option<Uuid>,
    name: String,
    levels: Vec<(Resource, Level)>,
    sensitive: Level,
}

/// Security review minor 5: closing the window with a temporary password
/// on screen clears a copy of it from the clipboard.
impl Drop for UsersScreen {
    fn drop(&mut self) {
        if self.shown.is_some() {
            crate::secret_clipboard::clear_soon();
        }
    }
}

/// The screen.
pub struct UsersScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    tab: Tab,
    data: Option<Loaded>,
    read: Option<Job<Loaded>>,
    write: Option<Job<Wrote>>,
    ensured: bool,
    selected: Option<UserId>,
    /// The role chosen in the selected user's panel, and for whom; until
    /// the Admin picks one the panel shows the user's own role.
    pick: Option<(UserId, RolePlan)>,
    new_email: String,
    new_name: String,
    new_role: RolePlan,
    /// The temporary password on show: whose (name) and the password.
    shown: Option<(String, Secret)>,
    copied: bool,
    /// Show was pressed on the temporary password (hidden by default, so
    /// the Admin can read it out without it sitting on screen).
    revealed: bool,
    role_selected: Option<Uuid>,
    edit: Option<RoleEdit>,
    /// i18n key of the last message, its fill-ins, and whether it is good
    /// news.
    message: Option<(&'static str, bool)>,
    error: Option<ApiError>,
}

impl std::fmt::Debug for UsersScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsersScreen")
            .field("tab", &self.tab)
            .field("message", &self.message)
            .finish_non_exhaustive()
    }
}

fn label() -> OrphanLabel {
    OrphanLabel {
        what: "orphan.team",
        key: None,
    }
}

/// The plain words for a refusal from the team routes.
#[must_use]
pub fn refusal(e: &ApiError) -> &'static str {
    let code = match e {
        ApiError::Forbidden { code } | ApiError::NotFound { code } => {
            code.as_ref().map(|c| c.as_str().to_owned())
        }
        ApiError::Validation(v) => v.code().map(|c| c.as_str().to_owned()),
        _ => None,
    };
    match (e, code.as_deref()) {
        (_, Some("CANNOT_TARGET_SELF")) => "users.msg.not_yourself",
        (_, Some("SUPER_ADMIN_TARGET")) => "users.msg.main_admin",
        (_, Some("USER_NOT_IN_TENANT")) => "users.msg.not_on_team",
        (_, Some("TEMPLATE_NAME_TAKEN")) => "roles.msg.name_taken",
        (ApiError::Forbidden { .. }, _) => "users.msg.only_admin",
        (ApiError::Conflict { .. }, _) => "users.msg.already",
        (ApiError::NotFound { .. }, _) => "users.msg.gone",
        (ApiError::Validation(_), _) => "users.msg.refused",
        _ => "users.msg.failed",
    }
}

/// What a stopped Add says (plan §1: the user is left view-only).
#[must_use]
pub fn add_stop_message(step: TeamStep) -> &'static str {
    match step {
        TeamStep::Invite => "users.msg.add_failed",
        TeamStep::Permissions | TeamStep::Role => "users.msg.left_view_only",
    }
}

fn same_email(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn when(iso: Option<&str>) -> String {
    iso.and_then(|s| s.get(..10))
        .and_then(|d| cl_core::CalendarDate::parse_wire(d).ok())
        .map_or_else(|| t("users.never"), crate::shell::us_date)
}

impl UsersScreen {
    /// Reads the team and its roles.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>) -> UsersScreen {
        let group = link.group().and_then(Result::ok);
        let mut s = UsersScreen {
            link,
            group,
            tab: Tab::Users,
            data: None,
            read: None,
            write: None,
            ensured: false,
            selected: None,
            pick: None,
            new_email: String::new(),
            new_name: String::new(),
            new_role: RolePlan::ViewOnly,
            shown: None,
            copied: false,
            revealed: false,
            role_selected: None,
            edit: None,
            message: None,
            error: None,
        };
        s.reload();
        s
    }

    fn keys(&self) -> EngineKeys {
        let caps = self.link.capabilities();
        EngineKeys {
            report_areas: self.link.is_demo() || caps.has(REPORT_AREAS),
            sensitive: caps.has(SENSITIVE_ACCOUNTS),
            sensitive_grant: caps.has(SENSITIVE_GRANT),
        }
    }

    fn reload(&mut self) {
        self.error = None;
        self.read = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| {
                let users = d.auth.team();
                let templates = d.auth.role_templates()?;
                let mut rows = BTreeMap::new();
                for u in users.iter().filter(|u| u.role == "bookkeeper") {
                    rows.insert(u.id, d.auth.permission_row(u.id)?);
                }
                Ok(Loaded {
                    users,
                    templates,
                    rows,
                })
            },
            |s| async move {
                let team = s.team();
                let users = team.users().await?;
                let templates = team.templates().await?;
                let mut rows = BTreeMap::new();
                for u in users.iter().filter(|u| u.role == "bookkeeper") {
                    rows.insert(u.id, team.permissions(u.id).await?);
                }
                Ok(Loaded {
                    users,
                    templates,
                    rows,
                })
            },
        ));
        self.poll();
    }

    fn data(&self) -> Loaded {
        self.data.clone().unwrap_or_default()
    }

    fn me(&self) -> Option<UserId> {
        let email = self.link.signed_in_email()?;
        self.data
            .as_ref()?
            .users
            .iter()
            .find(|u| same_email(&u.email, &email))
            .map(|u| u.id)
    }

    fn template_by_builtin(&self, b: BuiltinRole) -> Option<&RoleTemplate> {
        self.data
            .as_ref()?
            .templates
            .iter()
            .find(|t| t.name == b.template_name())
    }

    fn kind_of(&self, u: &TeamUser) -> RoleKind {
        let data = self.data.as_ref();
        let row = data.and_then(|d| d.rows.get(&u.id)).cloned().flatten();
        let name = row.as_ref().map(|r| {
            r.template.and_then(|id| {
                data.and_then(|d| d.templates.iter().find(|t| t.id == id))
                    .map(|t| t.name.as_str())
            })
        });
        classify(&u.role, name)
    }

    fn kind_text(k: &RoleKind) -> String {
        match k {
            RoleKind::Custom(Some(name)) => name.clone(),
            RoleKind::Other(word) => t_with("roles.kind.other", &[("role", word)]),
            other => t(other.key()),
        }
    }

    fn plan_text(&self, p: RolePlan) -> String {
        match p {
            RolePlan::ViewOnly => t("roles.kind.view_only"),
            RolePlan::Admin => t("roles.kind.admin"),
            RolePlan::Template(id) => self
                .data
                .as_ref()
                .and_then(|d| d.templates.iter().find(|t| t.id == id))
                .map_or_else(
                    || t("roles.kind.custom"),
                    |tpl| {
                        builtin_by_name(&tpl.name)
                            .map_or_else(|| tpl.name.clone(), |b| t(RoleKind::Builtin(b).key()))
                    },
                ),
        }
    }

    /// Every role an Admin can give: View-only, the built-ins that exist,
    /// the custom roles, Admin.
    fn plans(&self) -> Vec<RolePlan> {
        let mut out = vec![RolePlan::ViewOnly];
        for b in BuiltinRole::ALL {
            if let Some(tpl) = self.template_by_builtin(*b) {
                out.push(RolePlan::Template(tpl.id));
            }
        }
        if let Some(d) = &self.data {
            for tpl in &d.templates {
                if builtin_by_name(&tpl.name).is_none() {
                    out.push(RolePlan::Template(tpl.id));
                }
            }
        }
        out.push(RolePlan::Admin);
        out
    }

    fn active_count(&self) -> usize {
        self.data
            .as_ref()
            .map_or(0, |d| d.users.iter().filter(|u| u.active).count())
    }

    fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.read) {
            match r {
                Ok(d) => {
                    self.data = Some(d);
                    // Q-P29-D2: automatic sign-in is for one person only.
                    if self.active_count() > 1 {
                        self.link.clear_auto_sign_in();
                    }
                    self.ensure_builtins();
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.write) {
            match r {
                Ok(w) => self.wrote(w),
                Err(e) => self.message = Some((refusal(&e), false)),
            }
            self.reload();
        }
    }

    fn wrote(&mut self, w: Wrote) {
        match w {
            Wrote::Added(Ok(added)) => {
                // A second user: automatic sign-in ends (Q-P29o).
                self.link.clear_auto_sign_in();
                tracing::info!(user = %added.user, "team member added");
                self.selected = Some(added.user);
                self.new_email.clear();
                self.new_name.clear();
                self.message = Some(("users.msg.added", true));
                self.show_password(added);
            }
            Wrote::Added(Err(f)) => {
                tracing::warn!(step = ?f.step, "adding a team member stopped");
                let TeamFailure { step, added, error } = *f;
                if let Some(added) = added {
                    self.link.clear_auto_sign_in();
                    self.selected = Some(added.user);
                    self.new_email.clear();
                    self.new_name.clear();
                    self.show_password(added);
                    self.message = Some((add_stop_message(step), false));
                } else {
                    self.message = Some((refusal(&error), false));
                }
            }
            Wrote::Role(Ok(())) => {
                tracing::info!(user = ?self.selected, "role changed");
                self.message = Some(("users.msg.role_changed", true));
            }
            Wrote::Role(Err(f)) => {
                tracing::warn!(step = ?f.step, "role change stopped");
                self.message = Some((
                    match f.step {
                        TeamStep::Role if matches!(f.error, ApiError::Validation(_)) => {
                            "users.msg.role_refused"
                        }
                        _ => refusal(&f.error),
                    },
                    false,
                ));
            }
            Wrote::Done(key) => {
                if key == "users.msg.turned_on" {
                    self.link.clear_auto_sign_in();
                }
                self.message = Some((key, true));
            }
            Wrote::TempPassword(id, secret) => {
                tracing::info!(user = %id, "temporary password set");
                let name = self.name_of(id);
                self.shown = Some((name, secret));
                self.copied = false;
                self.revealed = false;
                self.message = Some(("users.msg.temp_set", true));
            }
            Wrote::Builtins => {}
            Wrote::RoleSaved => {
                self.edit = None;
                self.message = Some(("roles.msg.saved", true));
            }
        }
    }

    fn name_of(&self, id: UserId) -> String {
        self.data
            .as_ref()
            .and_then(|d| d.users.iter().find(|u| u.id == id))
            .map_or_else(String::new, |u| u.display_name.clone())
    }

    fn show_password(&mut self, added: Added) {
        if let Some(p) = added.temporary_password {
            self.shown = Some((self.new_name_or(added.user), p));
            self.copied = false;
            self.revealed = false;
        }
    }

    fn new_name_or(&self, id: UserId) -> String {
        let n = self.name_of(id);
        if n.is_empty() {
            t("users.new_person")
        } else {
            n
        }
    }

    /// Q-P29v: built-in roles are found by name and recreated when missing;
    /// one whose map no longer matches this engine is put back.
    fn ensure_builtins(&mut self) {
        if self.ensured || self.write.is_some() {
            return;
        }
        self.ensured = true;
        let keys = self.keys();
        let have = self.data().templates;
        let mut todo: Vec<(Option<Uuid>, &'static str, BTreeMap<String, String>)> = Vec::new();
        for b in BuiltinRole::ALL {
            let want = builtin_wire(*b, keys);
            match have.iter().find(|t| t.name == b.template_name()) {
                None => todo.push((None, b.template_name(), want)),
                Some(t) if t.permissions != want => {
                    todo.push((Some(t.id), b.template_name(), want));
                }
                Some(_) => {}
            }
        }
        if todo.is_empty() {
            return;
        }
        let demo_todo = todo.clone();
        self.write = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label(),
            move |d| {
                for (id, name, map) in &demo_todo {
                    match id {
                        Some(id) => d.auth.update_role_template(*id, name, map).map(drop)?,
                        None => d.auth.create_role_template(name, map).map(drop)?,
                    }
                }
                Ok(Wrote::Builtins)
            },
            move |s| async move {
                let team = s.team();
                for (id, name, map) in &todo {
                    match id {
                        Some(id) => team.update_template(*id, name, map).await.map(drop)?,
                        None => team.create_template(name, None, map).await.map(drop)?,
                    }
                }
                Ok(Wrote::Builtins)
            },
        ));
        tracing::info!("built-in roles created or put back");
    }

    fn add(&mut self) {
        if self.write.is_some() {
            return;
        }
        let email = self.new_email.trim();
        if email.is_empty() || !email.contains('@') || self.new_name.trim().is_empty() {
            self.message = Some(("users.msg.add_missing", false));
            return;
        }
        let new = NewTeamUser {
            email: email.to_owned(),
            display_name: self.new_name.trim().to_owned(),
        };
        let plan = self.new_role;
        let demo_new = new.clone();
        self.message = None;
        self.write = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label(),
            move |d| Ok(Wrote::Added(d.auth.add_user(&demo_new, plan))),
            move |s| async move { Ok(Wrote::Added(s.team().add(&new, plan).await)) },
        ));
        self.poll();
    }

    fn change_role(&mut self, id: UserId, plan: RolePlan) {
        if self.write.is_some() {
            return;
        }
        if Some(id) == self.me() {
            self.message = Some(("users.msg.own_role", false));
            return;
        }
        self.message = None;
        self.write = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label(),
            move |d| Ok(Wrote::Role(d.auth.set_user_role(id, plan))),
            move |s| async move { Ok(Wrote::Role(s.team().set_role(id, plan).await)) },
        ));
        self.poll();
    }

    fn act(&mut self, id: UserId, what: Act) {
        if self.write.is_some() {
            return;
        }
        if Some(id) == self.me() && what != Act::Unlock {
            self.message = Some(("users.msg.not_yourself", false));
            return;
        }
        self.message = None;
        self.write = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label(),
            move |d| match what {
                Act::Off => d
                    .auth
                    .set_active(id, false)
                    .map(|()| Wrote::Done("users.msg.turned_off")),
                Act::On => d
                    .auth
                    .set_active(id, true)
                    .map(|()| Wrote::Done("users.msg.turned_on")),
                Act::Unlock => d
                    .auth
                    .unlock_user(id)
                    .map(|()| Wrote::Done("users.msg.unlocked")),
                Act::TempPassword => d
                    .auth
                    .temporary_password(id)
                    .map(|p| Wrote::TempPassword(id, p)),
            },
            move |s| async move {
                let team = s.team();
                match what {
                    Act::Off => team
                        .turn_off(id)
                        .await
                        .map(|()| Wrote::Done("users.msg.turned_off")),
                    Act::On => team
                        .turn_on(id)
                        .await
                        .map(|()| Wrote::Done("users.msg.turned_on")),
                    Act::Unlock => team
                        .unlock(id)
                        .await
                        .map(|()| Wrote::Done("users.msg.unlocked")),
                    Act::TempPassword => team
                        .temporary_password(id)
                        .await
                        .map(|p| Wrote::TempPassword(id, p)),
                }
            },
        ));
        self.poll();
    }

    fn new_role_from(&mut self, base: Option<Uuid>) {
        let data = self.data();
        let tpl = base
            .and_then(|id| data.templates.iter().find(|t| t.id == id))
            .or_else(|| {
                data.templates
                    .iter()
                    .find(|t| t.name == BuiltinRole::Bookkeeper.template_name())
            });
        let (levels, sensitive) = tpl.map_or_else(
            || (BuiltinRole::Bookkeeper.levels(), Level::None),
            |t| levels_from_wire(&t.permissions),
        );
        self.edit = Some(RoleEdit {
            id: None,
            name: tpl.map_or_else(String::new, |t| {
                t_with(
                    "roles.copy_name",
                    &[("name", &self.plan_text(RolePlan::Template(t.id)))],
                )
            }),
            levels,
            sensitive,
        });
        self.message = None;
    }

    fn edit_role(&mut self, id: Uuid) {
        let Some(tpl) = self.data().templates.into_iter().find(|t| t.id == id) else {
            return;
        };
        if builtin_by_name(&tpl.name).is_some() {
            // Q-P29v: editing a built-in saves a custom copy.
            self.new_role_from(Some(id));
            self.message = Some(("roles.msg.builtin_copy", true));
            return;
        }
        let (levels, sensitive) = levels_from_wire(&tpl.permissions);
        self.edit = Some(RoleEdit {
            id: Some(id),
            name: tpl.name,
            levels,
            sensitive,
        });
    }

    fn save_role(&mut self) {
        let Some(edit) = self.edit.clone() else {
            return;
        };
        let name = edit.name.trim().to_owned();
        if name.is_empty() {
            self.message = Some(("roles.msg.name_needed", false));
            return;
        }
        if builtin_by_name(&name).is_some() {
            self.message = Some(("roles.msg.builtin_name", false));
            return;
        }
        let map = wire_map(&edit.levels, edit.sensitive, self.keys());
        let map2 = map.clone();
        let name2 = name.clone();
        self.message = None;
        self.write = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label(),
            move |d| {
                match edit.id {
                    Some(id) => d.auth.update_role_template(id, &name2, &map2).map(drop)?,
                    None => d.auth.create_role_template(&name2, &map2).map(drop)?,
                }
                Ok(Wrote::RoleSaved)
            },
            move |s| async move {
                let team = s.team();
                match edit.id {
                    Some(id) => team.update_template(id, &name, &map).await.map(drop)?,
                    None => team.create_template(&name, None, &map).await.map(drop)?,
                }
                Ok(Wrote::RoleSaved)
            },
        ));
        tracing::info!("custom role saved");
        self.poll();
    }

    fn holders(&self, id: Uuid) -> usize {
        self.data.as_ref().map_or(0, |d| {
            d.rows
                .values()
                .filter(|r| r.as_ref().is_some_and(|r| r.template == Some(id)))
                .count()
        })
    }

    fn delete_role(&mut self, id: Uuid) {
        if self.holders(id) > 0 {
            // Deleting it would leave its holders with no rights at all.
            self.message = Some(("roles.msg.in_use", false));
            return;
        }
        self.message = None;
        self.write = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label(),
            move |d| {
                d.auth
                    .delete_role_template(id)
                    .map(|()| Wrote::Done("roles.msg.deleted"))
            },
            move |s| async move {
                s.team()
                    .delete_template(id)
                    .await
                    .map(|()| Wrote::Done("roles.msg.deleted"))
            },
        ));
        self.role_selected = None;
        self.poll();
    }

    fn new_recovery_key(&self) {
        self.link.push(CompanyEvent::OpenWindow(CommandId::new(
            crate::screens::company::RECOVERY_KEY,
        )));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Off,
    On,
    Unlock,
    TempPassword,
}

/// The main Admin (the engine's super admin), when the team list shows it:
/// on the desktop the first sign-in made is the owner who set the company
/// up. `None` unless every row has its creation time and the earliest is an
/// Admin; the engine's `SUPER_ADMIN_TARGET` refusal stays the backstop.
#[must_use]
pub fn main_admin(users: &[TeamUser]) -> Option<cl_core::ids::UserId> {
    if users.iter().any(|u| u.created_at.is_none()) {
        return None;
    }
    let first = users
        .iter()
        .min_by(|a, b| a.created_at.cmp(&b.created_at))?;
    let ties = users
        .iter()
        .filter(|u| u.created_at == first.created_at)
        .count();
    (ties == 1 && matches!(first.role.as_str(), "owner" | "super_admin")).then_some(first.id)
}

// ---- drawing ----------------------------------------------------------------

impl UsersScreen {
    fn users_tab(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let users = self.data().users;
        let me = self.me();
        if users.len() <= 1 {
            form::hint(ui, tokens, &t("users.only_you"));
        }
        // P29 QA-15: the page scrolls; the table only sideways when the
        // window is narrow, so no user hides below a small box.
        egui::ScrollArea::horizontal()
            .id_salt("users/list")
            .show(ui, |ui| {
                egui::Grid::new("users/grid")
                    .striped(true)
                    .num_columns(6)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        for k in [
                            "users.col.name",
                            "users.col.email",
                            "users.col.role",
                            "users.col.status",
                            "users.col.last",
                            "users.col.notes",
                        ] {
                            ui.label(RichText::new(t(k)).strong());
                        }
                        ui.end_row();
                        for u in &users {
                            let on = self.selected == Some(u.id);
                            let name = if Some(u.id) == me {
                                t_with("users.you", &[("name", &u.display_name)])
                            } else {
                                u.display_name.clone()
                            };
                            let role = Self::kind_text(&self.kind_of(u));
                            let status = t(if u.active {
                                "users.active"
                            } else {
                                "users.turned_off"
                            });
                            let mut notes = Vec::new();
                            if u.locked {
                                notes.push(t("users.locked"));
                            }
                            if u.must_change {
                                notes.push(t("users.must_change"));
                            }
                            let notes = notes.join(" \u{b7} ");
                            // P29 QA-15: the row's button names the whole
                            // row for screen readers (egui's grid has no
                            // row nodes).
                            let row_text = [
                                name.as_str(),
                                role.as_str(),
                                status.as_str(),
                                notes.as_str(),
                            ]
                            .iter()
                            .filter(|s| !s.is_empty())
                            .copied()
                            .collect::<Vec<_>>()
                            .join(", ");
                            let r = ui.selectable_label(on, name.as_str());
                            r.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::Role::Button,
                                    true,
                                    on,
                                    row_text.clone(),
                                )
                            });
                            if r.clicked() {
                                self.selected = Some(u.id);
                            }
                            ui.label(&u.email);
                            ui.label(role);
                            ui.label(status);
                            ui.label(when(u.last_sign_in.as_deref()));
                            ui.label(notes);
                            ui.end_row();
                        }
                    });
            });
        ui.add_space(8.0);
        if let Some(id) = self.selected
            && let Some(u) = users.iter().find(|u| u.id == id).cloned()
        {
            let main = main_admin(&users) == Some(id);
            self.selected_card(ui, tokens, &u, Some(id) == me, main);
            ui.add_space(8.0);
        }
        self.add_card(ui, tokens);
        if self.link.rights().is_admin() {
            ui.add_space(6.0);
            if ui
                .link(t("users.recovery_key"))
                .on_hover_text(t("users.recovery_key.tip"))
                .clicked()
            {
                self.new_recovery_key();
            }
        }
    }

    fn role_picker(&self, ui: &mut Ui, id: &str, label: &str, value: &mut RolePlan) {
        let plans = self.plans();
        ui.horizontal(|ui| {
            let l = ui.label(label);
            egui::ComboBox::from_id_salt(id)
                .selected_text(self.plan_text(*value))
                .width(260.0)
                .show_ui(ui, |ui| {
                    for p in plans {
                        ui.selectable_value(value, p, self.plan_text(p));
                    }
                })
                .response
                .labelled_by(l.id);
        });
    }

    /// [`Self::role_picker`] that may show nothing chosen (a user whose role
    /// isn't one the Admin can give, such as one with no permission row).
    fn role_picker_opt(&self, ui: &mut Ui, id: &str, label: &str, value: &mut Option<RolePlan>) {
        let plans = self.plans();
        ui.horizontal(|ui| {
            let l = ui.label(label);
            egui::ComboBox::from_id_salt(id)
                .selected_text(value.map_or_else(|| t("users.role.choose"), |p| self.plan_text(p)))
                .width(260.0)
                .show_ui(ui, |ui| {
                    for p in plans {
                        ui.selectable_value(value, Some(p), self.plan_text(p));
                    }
                })
                .response
                .labelled_by(l.id);
        });
    }

    /// The role the user has now, as one the Admin can give: None for a
    /// user with no permission row, a full-but-not-Admin user, overrides
    /// with no role, or an unknown engine word.
    fn current_plan(&self, u: &TeamUser) -> Option<RolePlan> {
        match self.kind_of(u) {
            RoleKind::Admin => Some(RolePlan::Admin),
            RoleKind::ViewOnly => Some(RolePlan::ViewOnly),
            RoleKind::Builtin(_) | RoleKind::Custom(Some(_)) => self
                .data
                .as_ref()?
                .rows
                .get(&u.id)?
                .as_ref()?
                .template
                .map(RolePlan::Template),
            RoleKind::FullNotAdmin
            | RoleKind::NoRow
            | RoleKind::Custom(None)
            | RoleKind::Other(_) => None,
        }
    }

    /// What the selected user's dropdown shows: the Admin's pick for this
    /// user, else the user's own role.
    fn picked_for(&self, u: &TeamUser) -> Option<RolePlan> {
        self.pick
            .filter(|(id, _)| *id == u.id)
            .map(|(_, p)| p)
            .or_else(|| self.current_plan(u))
    }

    /// The change Change role would send: only a role different from the
    /// one the user has.
    fn role_change(current: Option<RolePlan>, pick: Option<RolePlan>) -> Option<RolePlan> {
        pick.filter(|p| Some(*p) != current)
    }

    /// `main`: the row is the main Admin (no temporary password: they use
    /// the recovery key; the engine refuses it with `SUPER_ADMIN_TARGET`).
    fn selected_card(&mut self, ui: &mut Ui, tokens: &Tokens, u: &TeamUser, me: bool, main: bool) {
        let busy = self.write.is_some();
        let mut act: Option<Act> = None;
        let mut change: Option<RolePlan> = None;
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(&u.display_name).strong());
            if matches!(self.kind_of(u), RoleKind::NoRow) {
                form::inline_warning(ui, tokens, &t("users.no_row"));
            }
            if me {
                form::hint(ui, tokens, &t("users.yourself"));
                return;
            }
            let current = self.current_plan(u);
            let mut pick = self.picked_for(u);
            self.role_picker_opt(ui, "users/role", &t("users.role"), &mut pick);
            if let Some(p) = pick {
                self.pick = Some((u.id, p));
            }
            let changed = Self::role_change(current, pick);
            ui.add_enabled_ui(!busy, |ui| {
                ui.horizontal_wrapped(|ui| {
                    // Change role stays off until a different role is chosen.
                    let b = ui
                        .add_enabled(changed.is_some(), egui::Button::new(t("users.change_role")));
                    if b.clicked() {
                        change = changed;
                    }
                    if u.active {
                        if ui.button(t("users.turn_off")).clicked() {
                            act = Some(Act::Off);
                        }
                    } else if ui.button(t("users.turn_on")).clicked() {
                        act = Some(Act::On);
                    }
                    if u.locked && ui.button(t("users.unlock")).clicked() {
                        act = Some(Act::Unlock);
                    }
                    if !main
                        && ui
                            .button(t("users.temp_password"))
                            .on_hover_text(t("users.temp_password.tip"))
                            .clicked()
                    {
                        act = Some(Act::TempPassword);
                    }
                });
                if main {
                    form::hint(ui, tokens, &t("users.msg.main_admin"));
                }
            });
        });
        if let Some(p) = change {
            self.change_role(u.id, p);
        }
        if let Some(a) = act {
            self.act(u.id, a);
        }
    }

    fn add_card(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let busy = self.write.is_some();
        let mut go = false;
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(t("users.add_card")).strong());
            form::hint(ui, tokens, &t("users.add_intro"));
            form::field(ui, &t("users.add_name"), &mut self.new_name);
            form::field(ui, &t("users.add_email"), &mut self.new_email);
            let mut role = self.new_role;
            self.role_picker(ui, "users/new_role", &t("users.add_role"), &mut role);
            self.new_role = role;
            ui.add_enabled_ui(!busy, |ui| {
                go = primary_button(ui, &t("users.add")).clicked();
            });
        });
        if go {
            self.add();
        }
    }

    fn password_box(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some((name, secret)) = &self.shown else {
            return;
        };
        // Screenshots always mask it; otherwise it shows only after Show.
        let shooting = ui.ctx().data(|d| {
            d.get_temp::<std::time::Instant>(crate::automation::hide_secrets_id())
                .is_some_and(|until| std::time::Instant::now() < until)
        });
        let masked = shooting || !self.revealed;
        let mut close = false;
        let mut toggle = false;
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(t_with("users.temp.title", &[("name", name)])).strong());
            form::hint(ui, tokens, &t("users.temp.intro"));
            // Security review minor 6 (not fully fixed): the secret is borrowed
            // here, but egui's label still makes its own text copy each frame
            // while the dialog is open.
            let dots = "\u{2022}".repeat(16);
            let text: &str = if masked { &dots } else { secret.expose() };
            let r = ui
                .add(egui::Label::new(RichText::new(text).monospace().size(18.0)).selectable(true));
            r.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Label, true, t("users.temp.label"))
            });
            ui.horizontal(|ui| {
                let word = if self.revealed {
                    "users.temp.hide"
                } else {
                    "users.temp.show"
                };
                if ui.button(t(word)).clicked() {
                    toggle = true;
                }
                if ui.button(t("users.temp.copy")).clicked() {
                    crate::secret_clipboard::copy(ui.ctx(), secret.expose().to_owned());
                    self.copied = true;
                }
                if ui.button(t("users.temp.done")).clicked() {
                    close = true;
                }
            });
            if self.copied {
                form::hint(ui, tokens, &t("users.temp.copied"));
            }
        });
        if toggle {
            self.revealed = !self.revealed;
        }
        if close {
            self.shown = None;
            self.copied = false;
            self.revealed = false;
            crate::secret_clipboard::clear_soon();
        }
    }

    fn roles_tab(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let data = self.data();
        form::hint(ui, tokens, &t("roles.intro"));
        let mut open: Option<Uuid> = None;
        let mut new_from: Option<Option<Uuid>> = None;
        let mut delete: Option<Uuid> = None;
        ui.horizontal_wrapped(|ui| {
            for tpl in &data.templates {
                let builtin = builtin_by_name(&tpl.name).is_some();
                let text = if builtin {
                    t_with(
                        "roles.builtin_tag",
                        &[("name", &self.plan_text(RolePlan::Template(tpl.id)))],
                    )
                } else {
                    tpl.name.clone()
                };
                if ui
                    .selectable_label(self.role_selected == Some(tpl.id), text)
                    .clicked()
                {
                    self.role_selected = Some(tpl.id);
                }
            }
        });
        if !data
            .templates
            .iter()
            .any(|t| builtin_by_name(&t.name).is_none())
        {
            form::hint(ui, tokens, &t("roles.no_custom"));
        }
        ui.add_enabled_ui(self.write.is_none() && self.edit.is_none(), |ui| {
            ui.horizontal(|ui| {
                if ui.button(t("roles.new")).clicked() {
                    new_from = Some(self.role_selected);
                }
                if let Some(id) = self.role_selected {
                    if ui.button(t("roles.edit")).clicked() {
                        open = Some(id);
                    }
                    let custom = data
                        .templates
                        .iter()
                        .any(|t| t.id == id && builtin_by_name(&t.name).is_none());
                    if custom && ui.button(t("roles.delete")).clicked() {
                        delete = Some(id);
                    }
                }
            });
        });
        if let Some(base) = new_from {
            self.new_role_from(base);
        }
        if let Some(id) = open {
            self.edit_role(id);
        }
        if let Some(id) = delete {
            self.delete_role(id);
        }
        if self.edit.is_some() {
            ui.add_space(8.0);
            self.role_grid(ui, tokens);
        }
    }

    fn role_grid(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let keys = self.keys();
        let busy = self.write.is_some();
        let mut save = false;
        let mut cancel = false;
        let Some(edit) = self.edit.as_mut() else {
            return;
        };
        form::card(ui, tokens, |ui| {
            form::field(ui, &t("roles.name"), &mut edit.name);
            egui::ScrollArea::vertical()
                .id_salt("roles/grid_scroll")
                .max_height(360.0)
                .show(ui, |ui| {
                    egui::Grid::new("roles/grid")
                        .striped(true)
                        .num_columns(4)
                        .show(ui, |ui| {
                            for g in Group::ALL {
                                let rows: Vec<usize> = edit
                                    .levels
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, (r, _))| {
                                        group_of(*r) == Some(*g) && keys.knows(*r)
                                    })
                                    .map(|(i, _)| i)
                                    .collect();
                                if rows.is_empty() {
                                    continue;
                                }
                                ui.label(RichText::new(t(g.key())).strong());
                                ui.end_row();
                                for i in rows {
                                    let (r, level) = &mut edit.levels[i];
                                    let name = t(&right_key(*r));
                                    ui.label(&name);
                                    level_radio(ui, level, Level::None, "roles.level.none", &name);
                                    level_radio(ui, level, Level::View, "roles.level.view", &name);
                                    if r.writable() {
                                        level_radio(
                                            ui,
                                            level,
                                            Level::Full,
                                            "roles.level.full",
                                            &name,
                                        );
                                    } else {
                                        ui.label("");
                                    }
                                    ui.end_row();
                                }
                                if *g == Group::Manage && keys.sensitive {
                                    let name = t("roles.right.sensitive_accounts");
                                    ui.label(&name);
                                    let s = &mut edit.sensitive;
                                    level_radio(ui, s, Level::None, "roles.level.none", &name);
                                    level_radio(ui, s, Level::View, "roles.level.view", &name);
                                    ui.label("");
                                    ui.end_row();
                                }
                            }
                        });
                });
            ui.add_enabled_ui(!busy, |ui| {
                match form::actions(ui, &t("roles.save"), &[&t("roles.cancel")]) {
                    Some(0) => save = true,
                    Some(1) => cancel = true,
                    _ => {}
                }
            });
        });
        if save {
            self.save_role();
        }
        if cancel {
            self.edit = None;
        }
    }
}

/// One none/view/full choice, named "<right>: <level>" for screen readers
/// and the control channel.
fn level_radio(ui: &mut Ui, value: &mut Level, this: Level, key: &str, right: &str) {
    let r = ui.radio_value(value, this, t(key));
    let name = format!("{right}: {}", t(key));
    r.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::RadioButton, true, *value == this, &name)
    });
}

impl Screen for UsersScreen {
    fn title(&self) -> String {
        t("cmd.company.users")
    }

    fn dirty(&self) -> bool {
        self.edit.is_some() || !self.new_email.trim().is_empty() || !self.new_name.trim().is_empty()
    }

    fn can_save(&self) -> bool {
        false
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        let tokens = Tokens::of(ui.ctx());
        if self.read.is_some() || self.write.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        form::heading(ui, &tokens, &t("cmd.company.users"));
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, Tab::Users, t("users.tab.users"));
            ui.selectable_value(&mut self.tab, Tab::Roles, t("users.tab.roles"));
        });
        ui.separator();
        if let Some((k, good)) = self.message {
            if good {
                form::inline_note(ui, &tokens, &t(k));
            } else {
                form::inline_warning(ui, &tokens, &t(k));
            }
        }
        self.password_box(ui, &tokens);
        if self.data.is_none() {
            if let Some(e) = &self.error {
                form::inline_error_details(ui, &tokens, e, "users");
            } else {
                form::progress_line(ui, &tokens, &t("users.loading"));
            }
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("users/page")
            .show(ui, |ui| match self.tab {
                Tab::Users => self.users_tab(ui, &tokens),
                Tab::Roles => self.roles_tab(ui, &tokens),
            });
    }

    fn state(&self) -> Value {
        let data = self.data.as_ref();
        json!({
            "tab": match self.tab { Tab::Users => "users", Tab::Roles => "roles" },
            "users": data.map_or(0, |d| d.users.len()),
            "active": self.active_count(),
            "roles": data.map(|d| d.templates.iter().map(|t| t.name.clone()).collect::<Vec<_>>()),
            "selected_role": self.selected.and_then(|id| {
                data?.users.iter().find(|u| u.id == id).map(|u| self.kind_of(u).key())
            }),
            // The selected user's dropdown, and whether Change role is on.
            "role_pick": self.selected.and_then(|id| {
                let u = data?.users.iter().find(|u| u.id == id)?;
                self.picked_for(u).map(|p| self.plan_text(p))
            }),
            "change_role_ready": self.selected.and_then(|id| {
                let u = data?.users.iter().find(|u| u.id == id)?;
                Some(Self::role_change(self.current_plan(u), self.picked_for(u)).is_some())
            }),
            "selected_locked": self.selected.and_then(|id| {
                data?.users.iter().find(|u| u.id == id).map(|u| u.locked)
            }),
            "selected_active": self.selected.and_then(|id| {
                data?.users.iter().find(|u| u.id == id).map(|u| u.active)
            }),
            "temp_password_shown": self.shown.is_some(),
            "temp_password_revealed": self.shown.is_some() && self.revealed,
            "editing_role": self.edit.is_some(),
            "message": self.message.map(|(k, _)| k),
            "busy": self.read.is_some() || self.write.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::company::demo::team::DEMO_TEMP_PASSWORD;

    #[test]
    fn the_main_admin_is_the_earliest_owner_only_when_known() {
        let u = |n: usize, role: &str, at: Option<&str>| TeamUser {
            id: crate::company::demo::auth::demo_user_id(n),
            email: format!("u{n}@example.com"),
            display_name: format!("U{n}"),
            role: role.into(),
            active: true,
            locked: false,
            failed_tries: 0,
            must_change: false,
            last_sign_in: None,
            created_at: at.map(str::to_owned),
        };
        let a = "2026-01-01T08:00:00.000Z";
        let b = "2026-02-01T08:00:00.000Z";
        let team = [u(1, "owner", Some(a)), u(2, "owner", Some(b))];
        assert_eq!(main_admin(&team), Some(team[0].id));
        let team = [u(1, "bookkeeper", Some(a)), u(2, "owner", Some(b))];
        assert_eq!(main_admin(&team), None, "earliest is not an Admin");
        let team = [u(1, "owner", Some(a)), u(2, "owner", None)];
        assert_eq!(main_admin(&team), None, "a row without its time");
        let team = [u(1, "owner", Some(a)), u(2, "owner", Some(a))];
        assert_eq!(main_admin(&team), None, "a tie");
    }

    fn screen() -> (tempfile::TempDir, UsersScreen) {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let mut s = UsersScreen::new(link);
        // The built-ins were created after the first read; read again.
        s.poll();
        (dir, s)
    }

    fn user(s: &UsersScreen, email: &str) -> TeamUser {
        s.data()
            .users
            .into_iter()
            .find(|u| u.email == email)
            .expect("user")
    }

    #[test]
    fn built_in_roles_are_created_by_name_and_put_back() {
        let (_d, s) = screen();
        let names: Vec<String> = s.data().templates.iter().map(|t| t.name.clone()).collect();
        for b in BuiltinRole::ALL {
            assert!(names.contains(&b.template_name().to_owned()), "{b:?}");
        }
        // A changed built-in is put back on the next open.
        let id = s.template_by_builtin(BuiltinRole::Payables).expect("ap").id;
        s.link
            .demo_state()
            .expect("demo")
            .auth
            .update_role_template(id, "Accounts payable", &BTreeMap::new())
            .expect("changed");
        let mut again = UsersScreen::new(Arc::clone(&s.link));
        again.poll();
        let ap = again
            .template_by_builtin(BuiltinRole::Payables)
            .expect("ap");
        assert_eq!(
            ap.permissions,
            builtin_wire(BuiltinRole::Payables, again.keys())
        );
        assert_eq!(ap.id, id, "put back, not duplicated");
    }

    #[test]
    fn adding_shows_the_temporary_password_once_and_fails_closed() {
        let (_d, mut s) = screen();
        let ap = s.template_by_builtin(BuiltinRole::Payables).expect("ap").id;
        s.new_name = "Rio Payables".into();
        s.new_email = "rio@example.com".into();
        s.new_role = RolePlan::Template(ap);
        s.link.demo_state().expect("demo").auth.fail_at = Some(TeamStep::Role);
        s.add();
        assert_eq!(s.state()["message"], "users.msg.left_view_only");
        assert_eq!(s.state()["temp_password_shown"], true);
        assert!(
            !s.state().to_string().contains(DEMO_TEMP_PASSWORD),
            "never in state"
        );
        let rio = user(&s, "rio@example.com");
        assert_eq!(rio.role, "readonly", "left view-only");
        // The row is there; the role change now finishes the job.
        s.change_role(rio.id, RolePlan::Template(ap));
        assert_eq!(s.state()["message"], "users.msg.role_changed");
        assert_eq!(
            s.kind_of(&user(&s, "rio@example.com")),
            RoleKind::Builtin(BuiltinRole::Payables)
        );
        // Closing the box drops the password.
        s.shown = None;
        // A missing email is caught before sending.
        s.new_name = "No Email".into();
        s.add();
        assert_eq!(s.state()["message"], "users.msg.add_missing");
        // Add with no failure.
        s.new_email = "sky@example.com".into();
        s.new_name = "Sky Viewer".into();
        s.new_role = RolePlan::ViewOnly;
        s.add();
        assert_eq!(s.state()["message"], "users.msg.added");
        assert_eq!(user(&s, "sky@example.com").role, "readonly");
    }

    #[test]
    fn unlock_turn_off_and_temporary_passwords() {
        let (_d, mut s) = screen();
        let kim = user(&s, "kim@example.com");
        assert!(kim.locked);
        s.act(kim.id, Act::Unlock);
        assert_eq!(s.state()["message"], "users.msg.unlocked");
        assert!(!user(&s, "kim@example.com").locked);
        s.act(kim.id, Act::Off);
        assert!(!user(&s, "kim@example.com").active);
        s.act(kim.id, Act::On);
        assert!(user(&s, "kim@example.com").active);
        s.act(kim.id, Act::TempPassword);
        assert_eq!(s.state()["message"], "users.msg.temp_set");
        assert!(
            s.shown
                .as_ref()
                .is_some_and(|(n, p)| n == "Kim Payables" && p.expose() == DEMO_TEMP_PASSWORD)
        );
        assert!(user(&s, "kim@example.com").must_change);
        // Hidden until Show; a new password starts hidden again.
        assert_eq!(s.state()["temp_password_revealed"], false);
        s.revealed = true;
        assert_eq!(s.state()["temp_password_revealed"], true);
        s.act(kim.id, Act::TempPassword);
        assert_eq!(s.state()["temp_password_revealed"], false);
        assert!(
            !s.state().to_string().contains(DEMO_TEMP_PASSWORD),
            "never in state"
        );
        // Not for yourself.
        let me = s.me().expect("me");
        s.act(me, Act::TempPassword);
        assert_eq!(s.state()["message"], "users.msg.not_yourself");
        s.change_role(me, RolePlan::ViewOnly);
        assert_eq!(s.state()["message"], "users.msg.own_role");
        // A bookkeeper without a row reads "Full access, choose a role".
        assert_eq!(s.kind_of(&user(&s, "lee@example.com")), RoleKind::NoRow);
    }

    #[test]
    fn custom_roles_copy_a_role_and_built_ins_save_as_copies() {
        let (_d, mut s) = screen();
        let ar = s
            .template_by_builtin(BuiltinRole::Receivables)
            .expect("ar")
            .id;
        s.edit_role(ar);
        assert_eq!(s.state()["message"], "roles.msg.builtin_copy");
        let edit = s.edit.as_mut().expect("editing");
        assert!(edit.id.is_none(), "a copy");
        edit.name = "Accounts receivable".into();
        s.save_role();
        assert_eq!(s.state()["message"], "roles.msg.builtin_name");
        let edit = s.edit.as_mut().expect("editing");
        edit.name = "Front desk".into();
        if let Some(l) = edit.levels.iter_mut().find(|(r, _)| *r == Resource::Bills) {
            l.1 = Level::View;
        }
        s.save_role();
        assert_eq!(s.state()["message"], "roles.msg.saved");
        let fd = s
            .data()
            .templates
            .into_iter()
            .find(|t| t.name == "Front desk")
            .expect("saved");
        assert_eq!(fd.permissions["bills"], "view");
        assert_eq!(fd.permissions["invoices"], "full");
        assert!(!fd.permissions.contains_key("item_costs"));
        // In use: not deleted.
        let lee = user(&s, "lee@example.com").id;
        s.change_role(lee, RolePlan::Template(fd.id));
        s.delete_role(fd.id);
        assert_eq!(s.state()["message"], "roles.msg.in_use");
        s.change_role(lee, RolePlan::ViewOnly);
        s.delete_role(fd.id);
        assert_eq!(s.state()["message"], "roles.msg.deleted");
    }

    /// P29 QA: the selected user's Role dropdown starts at the role they
    /// have (a built-in or a custom role), never View-only by default, and
    /// Change role waits until a different role is chosen.
    #[test]
    fn the_role_dropdown_starts_at_the_users_role() {
        let (_d, mut s) = screen();
        let bk = s
            .template_by_builtin(BuiltinRole::Bookkeeper)
            .expect("bk")
            .id;
        let lee = user(&s, "lee@example.com").id;
        s.change_role(lee, RolePlan::Template(bk));
        assert_eq!(s.state()["message"], "users.msg.role_changed");
        s.selected = Some(lee);
        let u = user(&s, "lee@example.com");
        assert_eq!(s.current_plan(&u), Some(RolePlan::Template(bk)));
        assert_eq!(s.state()["role_pick"], t("roles.kind.bookkeeper"));
        assert_eq!(s.state()["change_role_ready"], false);
        // Choosing View-only (a different role) turns Change role on;
        // choosing Bookkeeper again turns it off.
        s.pick = Some((lee, RolePlan::ViewOnly));
        assert_eq!(s.state()["change_role_ready"], true);
        s.pick = Some((lee, RolePlan::Template(bk)));
        assert_eq!(s.state()["change_role_ready"], false);
        // Another user's pick doesn't carry over.
        s.pick = Some((user(&s, "kim@example.com").id, RolePlan::ViewOnly));
        assert_eq!(s.state()["role_pick"], t("roles.kind.bookkeeper"));
        // A custom role.
        s.pick = None;
        s.edit_role(bk);
        let edit = s.edit.as_mut().expect("editing");
        edit.name = "Front desk".into();
        s.save_role();
        let fd = s
            .data()
            .templates
            .into_iter()
            .find(|t| t.name == "Front desk")
            .expect("saved")
            .id;
        s.change_role(lee, RolePlan::Template(fd));
        let u = user(&s, "lee@example.com");
        assert_eq!(s.current_plan(&u), Some(RolePlan::Template(fd)));
        assert_eq!(s.state()["role_pick"], "Front desk");
        assert_eq!(s.state()["change_role_ready"], false);
        // Back to View-only; with no role and no pick nothing is sent.
        s.change_role(lee, RolePlan::ViewOnly);
        assert_eq!(s.state()["role_pick"], t("roles.kind.view_only"));
        assert_eq!(UsersScreen::role_change(None, None), None);
    }

    #[test]
    fn refusals_read_in_plain_words() {
        let e = |status: u16, code: &str| {
            cl_api::engine_refusal(
                status,
                &serde_json::json!({"error": {"code": code, "message": "x"}}),
            )
        };
        assert_eq!(
            refusal(&e(403, "SUPER_ADMIN_TARGET")),
            "users.msg.main_admin"
        );
        assert_eq!(
            refusal(&e(403, "CANNOT_TARGET_SELF")),
            "users.msg.not_yourself"
        );
        assert_eq!(refusal(&e(403, "FORBIDDEN")), "users.msg.only_admin");
        assert_eq!(
            refusal(&e(400, "TEMPLATE_NAME_TAKEN")),
            "roles.msg.name_taken"
        );
        assert_eq!(refusal(&e(409, "CONFLICT")), "users.msg.already");
        assert_eq!(
            add_stop_message(TeamStep::Permissions),
            "users.msg.left_view_only"
        );
        for k in [
            "users.msg.main_admin",
            "users.msg.not_yourself",
            "users.msg.only_admin",
            "users.msg.already",
            "users.msg.gone",
            "users.msg.refused",
            "users.msg.failed",
            "users.msg.add_failed",
            "users.msg.left_view_only",
            "users.msg.not_on_team",
            "users.msg.role_refused",
            "users.msg.turned_on",
            "users.msg.turned_off",
            "users.no_row",
            "roles.msg.name_taken",
            "orphan.team",
        ] {
            assert!(crate::i18n::has(k), "{k}");
        }
        for r in Resource::ALL {
            if group_of(*r).is_some() {
                assert!(crate::i18n::has(&right_key(*r)), "{r:?}");
            }
        }
        for g in Group::ALL {
            assert!(crate::i18n::has(g.key()), "{g:?}");
        }
        for k in [
            RoleKind::Admin,
            RoleKind::ViewOnly,
            RoleKind::FullNotAdmin,
            RoleKind::NoRow,
            RoleKind::Builtin(BuiltinRole::Bookkeeper),
            RoleKind::Builtin(BuiltinRole::Receivables),
            RoleKind::Builtin(BuiltinRole::Payables),
            RoleKind::Custom(None),
            RoleKind::Other(String::new()),
        ] {
            assert!(crate::i18n::has(k.key()), "{k:?}");
        }
    }
}
