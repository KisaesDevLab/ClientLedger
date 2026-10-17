//! Client, sign-in, sessions, token refresh and company scope
//! (P05 plan §2.3).
//!
//! - The access token is a JWT sent as `Authorization: Bearer`. Its `exp` is
//!   read (no signature check, Q-P05p) only to refresh proactively.
//! - The refresh token arrives only as the `kb_refresh` cookie. cl-api keeps
//!   it in a zeroized [`Secret`], never persists it, and sends it by hand as a
//!   `Cookie` header on `/auth/refresh`, `/auth/switch-tenant` and
//!   `/auth/logout` only (Q-P05c). There is no cookie jar.
//! - Refresh rotates the token on every call, so refreshes are single-flight
//!   behind one async mutex and rotation-aware (an auth epoch tells a waiter
//!   that someone else already refreshed).
//! - A session starts in the owner workspace; `open_company` switches the
//!   session to a company tenant, after which company routes carry
//!   `X-Company-Id`. Each switch bumps the scope epoch.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use cl_core::Secret;
use cl_core::Timestamp;
use cl_core::ids::{CompanyId, TenantId, UserId};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::client::{
    AttemptAuth, Call, ClientConfig, EngineReply, Method, TimeoutClass, Transport,
};
use crate::endpoint::EndpointSource;
use crate::errors::{ApiError, AuthFailure};
use crate::idempotency::{
    AttemptCreds, AuthHook, BoxFuture, Executor, Reconciler, StaticAuth, unix_now,
};
use crate::resources::operations::{
    AUTH_ME, COMPANY_GET, CREATE_TENANT, DESKTOP_INFO, HEALTH, LOGIN, LOGOUT, Operation, REFRESH,
    SWITCH_TENANT, TFA_VERIFY,
};
use crate::wire::{Capabilities, DesktopInfo, Health};

/// Refresh when the access token has less than this many seconds left.
const REFRESH_MARGIN_SECS: u64 = 60;

/// The refresh-token cookie name.
const REFRESH_COOKIE: &str = "kb_refresh";

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Parses a JSON body into `T`; a mismatch is `Decode { what }`.
pub(crate) fn decode<T: DeserializeOwned>(
    reply: &EngineReply,
    what: &'static str,
) -> Result<T, ApiError> {
    serde_json::from_slice(&reply.body).map_err(|_| ApiError::Decode { what })
}

pub(crate) fn json_body<T: Serialize>(value: &T) -> Result<Vec<u8>, ApiError> {
    serde_json::to_vec(value).map_err(|_| ApiError::Decode {
        what: "request body",
    })
}

/// The engine API client. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    exec: Executor,
    workspace: Mutex<Option<TenantId>>,
    caps: Mutex<Capabilities>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

impl Client {
    /// Builds a client over `source`.
    ///
    /// # Errors
    /// `Internal` when the HTTP stack cannot be initialised.
    pub fn new(source: Arc<dyn EndpointSource>, cfg: ClientConfig) -> Result<Client, ApiError> {
        let transport = Transport::new(source, cfg).map_err(|_| ApiError::Internal)?;
        Ok(Client {
            inner: Arc::new(ClientInner {
                exec: Executor::new(transport),
                workspace: Mutex::new(None),
                caps: Mutex::new(Capabilities::default()),
            }),
        })
    }

    pub(crate) fn exec(&self) -> &Executor {
        &self.inner.exec
    }

    /// `GET /api/v1/desktop/info`. Also records the workspace tenant and the
    /// engine's capabilities for later requests.
    ///
    /// # Errors
    /// Any [`ApiError`] of the request.
    pub async fn desktop_info(&self) -> Result<DesktopInfo, ApiError> {
        let reply = self
            .exec()
            .run(
                &DESKTOP_INFO,
                Call::at(Method::Get, DESKTOP_INFO.route()).timeout(TimeoutClass::Probe),
                &StaticAuth(AttemptAuth::default()),
                None,
            )
            .await?;
        let info: DesktopInfo = decode(&reply, "desktop info")?;
        if let Some(ws) = info.workspace_tenant_id {
            *lock(&self.inner.workspace) = Some(ws);
        }
        self.exec()
            .set_idempotency_v1(info.capabilities.has(Capabilities::IDEMPOTENCY));
        lock(&self.inner.caps).clone_from(&info.capabilities);
        Ok(info)
    }

    /// `GET /health`.
    ///
    /// # Errors
    /// Any [`ApiError`] of the request.
    pub async fn health(&self) -> Result<Health, ApiError> {
        let reply = self
            .exec()
            .run(
                &HEALTH,
                Call::at(Method::Get, HEALTH.route()).timeout(TimeoutClass::Probe),
                &StaticAuth(AttemptAuth::default()),
                None,
            )
            .await?;
        decode(&reply, "health")
    }

    /// Capabilities from the last successful [`Client::desktop_info`].
    #[must_use]
    pub fn capabilities(&self) -> Capabilities {
        lock(&self.inner.caps).clone()
    }

    async fn workspace_tenant(&self) -> Option<TenantId> {
        if let Some(ws) = *lock(&self.inner.workspace) {
            return Some(ws);
        }
        // Re-read once; an engine without a workspace answers null.
        self.desktop_info()
            .await
            .ok()
            .and_then(|i| i.workspace_tenant_id)
    }

    /// Signs in. The session starts in the owner workspace.
    ///
    /// # Errors
    /// `Unauthorized(InvalidCredentials | AccountLocked | Deactivated)`,
    /// `RateLimited`, or any transport error. Never retried.
    pub async fn login(&self, email: &str, password: &Secret) -> Result<LoginOutcome, ApiError> {
        #[derive(Serialize)]
        struct Body<'a> {
            email: &'a str,
            password: &'a str,
        }
        let _ = self.workspace_tenant().await;
        let body = json_body(&Body {
            email,
            password: password.expose(),
        })?;
        let call = Call::at(Method::Post, LOGIN.route())
            .json_body(body)
            .timeout(TimeoutClass::Auth);
        let reply = self
            .exec()
            .run(&LOGIN, call, &StaticAuth(AttemptAuth::default()), None)
            .await?;
        let wire: AuthWire = decode(&reply, "login answer")?;
        if wire.tfa_required == Some(true) {
            let token = wire
                .tfa_token
                .ok_or(ApiError::Decode { what: "tfa token" })?;
            return Ok(LoginOutcome::TfaRequired(TfaChallenge {
                client: self.clone(),
                tfa_token: Secret::new(token),
                methods: wire
                    .available_methods
                    .unwrap_or_default()
                    .into_iter()
                    .map(TfaMethod)
                    .collect(),
                preferred: wire.preferred_method.map(TfaMethod),
            }));
        }
        Session::from_auth(self.clone(), wire, &reply).map(LoginOutcome::Session)
    }
}

/// Result of [`Client::login`].
#[derive(Debug)]
pub enum LoginOutcome {
    /// Signed in.
    Session(Session),
    /// A second step is needed.
    TfaRequired(TfaChallenge),
}

/// A two-step verification method (`totp`, `email`, `sms`, …).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TfaMethod(String);

impl TfaMethod {
    /// A method by name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> TfaMethod {
        TfaMethod(name.into())
    }

    /// The engine's method name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A pending two-step verification.
pub struct TfaChallenge {
    client: Client,
    tfa_token: Secret,
    methods: Vec<TfaMethod>,
    preferred: Option<TfaMethod>,
}

impl fmt::Debug for TfaChallenge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TfaChallenge")
            .field("methods", &self.methods)
            .field("preferred", &self.preferred)
            .finish_non_exhaustive()
    }
}

impl TfaChallenge {
    /// Methods the user enrolled.
    #[must_use]
    pub fn methods(&self) -> &[TfaMethod] {
        &self.methods
    }

    /// The user's preferred method.
    #[must_use]
    pub fn preferred(&self) -> Option<&TfaMethod> {
        self.preferred.as_ref()
    }

    /// Completes sign-in with a code. On failure the challenge is consumed;
    /// a wrong code (`TfaInvalid`) means signing in again.
    ///
    /// # Errors
    /// `Unauthorized(TfaInvalid | AccountLocked)`, `RateLimited`, transport
    /// errors.
    pub async fn verify(self, code: &Secret, method: &TfaMethod) -> Result<Session, ApiError> {
        #[derive(Serialize)]
        struct Body<'a> {
            code: &'a str,
            method: &'a str,
        }
        let body = json_body(&Body {
            code: code.expose(),
            method: method.as_str(),
        })?;
        let call = Call::at(Method::Post, TFA_VERIFY.route())
            .json_body(body)
            .timeout(TimeoutClass::Auth);
        let auth = StaticAuth(AttemptAuth {
            bearer: Some(self.tfa_token.clone()),
            refresh_cookie: None,
        });
        let reply = self
            .client
            .exec()
            .run(&TFA_VERIFY, call, &auth, None)
            .await?;
        let wire: AuthWire = decode(&reply, "tfa answer")?;
        Session::from_auth(self.client, wire, &reply)
    }
}

/// Login / TFA / refresh / switch answers (the fields cl-api reads).
#[derive(Deserialize)]
struct AuthWire {
    #[serde(default)]
    tfa_required: Option<bool>,
    #[serde(default)]
    tfa_token: Option<String>,
    #[serde(default)]
    available_methods: Option<Vec<String>>,
    #[serde(default)]
    preferred_method: Option<String>,
    #[serde(default)]
    tokens: Option<TokensWire>,
    #[serde(default)]
    user: Option<UserWire>,
}

#[derive(Deserialize)]
struct TokensWire {
    #[serde(rename = "accessToken")]
    access_token: String,
}

#[derive(Deserialize)]
struct UserWire {
    id: UserId,
}

/// Where a session's requests go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The hidden owner workspace: only sign-in and company management.
    Workspace,
    /// An open company.
    Company {
        /// The engine tenant.
        tenant_id: TenantId,
        /// The company row inside it (sent as `X-Company-Id`).
        company_id: CompanyId,
    },
}

/// A company the user can open (`accessibleTenants` minus the workspace).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanySummary {
    /// The engine tenant.
    pub tenant_id: TenantId,
    /// Company name (owner data: never logged).
    pub tenant_name: String,
    /// The user's role there.
    pub role: String,
    /// Last opened, most recent first.
    #[serde(default)]
    pub last_accessed_at: Option<Timestamp>,
}

/// Input of [`Session::create_company`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewCompany {
    /// Company name.
    pub company_name: String,
    /// Chart-of-accounts template (`default`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub business_type: Option<String>,
    /// Legal entity type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,
}

/// Answer of [`Session::create_company`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedCompany {
    /// The new tenant.
    pub tenant_id: TenantId,
    /// Its company row.
    pub company_id: CompanyId,
    /// Its name.
    pub tenant_name: String,
}

/// The company a session switched to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenCompany {
    /// The engine tenant.
    pub tenant_id: TenantId,
    /// The company row.
    pub company_id: CompanyId,
}

/// A signed-in session. Clones share the same tokens and scope.
///
/// A session can be *pinned* to the company open at the moment
/// ([`Session::pinned`]): every company request of the pinned view checks,
/// under the state lock before each attempt, that the company has not been
/// switched since. The request queue pins every job, so a save queued in
/// one company can never be posted into the next one.
#[derive(Clone)]
pub struct Session {
    inner: Arc<SessionInner>,
    pin: Option<ScopePin>,
}

/// The company a pinned session belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScopePin {
    pub(crate) epoch: u64,
    pub(crate) scope: Scope,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("scope", &self.scope())
            .finish_non_exhaustive()
    }
}

struct SessionInner {
    client: Client,
    user_id: Option<UserId>,
    state: Mutex<AuthState>,
    /// Single-flight guard for every call that rotates the refresh token.
    rotate: tokio::sync::Mutex<()>,
}

struct AuthState {
    access: Secret,
    access_exp: Option<u64>,
    refresh: Option<Secret>,
    auth_epoch: u64,
    scope: Scope,
    scope_epoch: u64,
    ended: bool,
}

impl AuthState {
    fn end(&mut self) {
        self.ended = true;
        self.access = Secret::default();
        self.refresh = None;
    }
}

/// What a `Set-Cookie` list says about the refresh token.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CookieUpdate {
    /// No `kb_refresh` cookie.
    Absent,
    /// A new refresh token.
    Set(Secret),
    /// The cookie was cleared.
    Cleared,
}

/// Reads `kb_refresh` from `Set-Cookie` values. Other cookies are ignored.
pub(crate) fn parse_refresh_cookie(set_cookies: &[Secret]) -> Result<CookieUpdate, ApiError> {
    let mut out = CookieUpdate::Absent;
    for raw in set_cookies {
        let raw = raw.expose();
        let mut parts = raw.split(';');
        let Some((name, value)) = parts.next().and_then(|p| p.split_once('=')) else {
            continue;
        };
        if name.trim() != REFRESH_COOKIE {
            continue;
        }
        let value = value.trim();
        let cleared = parts.any(|a| {
            a.split_once('=')
                .is_some_and(|(k, v)| k.trim().eq_ignore_ascii_case("max-age") && v.trim() == "0")
        });
        if cleared || value.is_empty() {
            out = CookieUpdate::Cleared;
            continue;
        }
        let ok = (16..=512).contains(&value.len())
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if !ok {
            return Err(ApiError::Decode {
                what: "refresh cookie",
            });
        }
        out = CookieUpdate::Set(Secret::new(value.to_string()));
    }
    Ok(out)
}

/// Reads `exp` (Unix seconds) from a JWT payload without verifying it
/// (Q-P05p: only the engine verifies; this only schedules refreshes).
pub(crate) fn jwt_exp(token: &str) -> Option<u64> {
    #[derive(Deserialize)]
    struct Claims {
        exp: Option<u64>,
    }
    let payload = token.split('.').nth(1)?;
    let bytes = base64url_decode(payload)?;
    serde_json::from_slice::<Claims>(&bytes).ok()?.exp
}

fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for c in s.bytes().filter(|&c| c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

impl Session {
    fn from_auth(client: Client, wire: AuthWire, reply: &EngineReply) -> Result<Session, ApiError> {
        let tokens = wire.tokens.ok_or(ApiError::Decode { what: "tokens" })?;
        let refresh = match parse_refresh_cookie(&reply.set_cookies)? {
            CookieUpdate::Set(s) => s,
            CookieUpdate::Absent | CookieUpdate::Cleared => {
                return Err(ApiError::Decode {
                    what: "refresh cookie",
                });
            }
        };
        let access = Secret::new(tokens.access_token);
        Ok(Session {
            inner: Arc::new(SessionInner {
                client,
                user_id: wire.user.map(|u| u.id),
                state: Mutex::new(AuthState {
                    access_exp: jwt_exp(access.expose()),
                    access,
                    refresh: Some(refresh),
                    auth_epoch: 0,
                    scope: Scope::Workspace,
                    scope_epoch: 0,
                    ended: false,
                }),
                rotate: tokio::sync::Mutex::new(()),
            }),
            pin: None,
        })
    }

    /// A view of this session pinned to the company open now (or to the one
    /// this view is already pinned to). Its company requests fail with
    /// `StaleScope` (nothing sent) once the company is switched, or with
    /// `OutcomeUnknown` when an earlier attempt of the same save may already
    /// have reached the engine.
    #[must_use]
    pub fn pinned(&self) -> Session {
        Session {
            inner: self.inner.clone(),
            pin: Some(self.pin_now()),
        }
    }

    /// The pin of this view, or the current scope for an unpinned one,
    /// read under one lock.
    pub(crate) fn pin_now(&self) -> ScopePin {
        self.pin.unwrap_or_else(|| {
            let st = lock(&self.inner.state);
            ScopePin {
                epoch: st.scope_epoch,
                scope: st.scope,
            }
        })
    }

    /// The company was switched since this view was pinned.
    pub(crate) fn pin_moved(&self) -> bool {
        self.pin
            .is_some_and(|p| lock(&self.inner.state).scope_epoch != p.epoch)
    }

    /// The client this session belongs to.
    #[must_use]
    pub fn client(&self) -> &Client {
        &self.inner.client
    }

    /// The signed-in user, when the engine said.
    #[must_use]
    pub fn user_id(&self) -> Option<UserId> {
        self.inner.user_id
    }

    /// Workspace or the open company.
    #[must_use]
    pub fn scope(&self) -> Scope {
        lock(&self.inner.state).scope
    }

    /// Increases on every company switch. Results of requests issued under
    /// an older epoch belong to the previous company (P05 plan §2.9).
    #[must_use]
    pub fn scope_epoch(&self) -> u64 {
        lock(&self.inner.state).scope_epoch
    }

    /// Companies the user can open (`/auth/me` `accessibleTenants` minus the
    /// workspace; `GET /api/v1/tenants` replaces this once E5a is pinned).
    ///
    /// # Errors
    /// Any [`ApiError`] of the request.
    pub async fn companies(&self) -> Result<Vec<CompanySummary>, ApiError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Me {
            accessible_tenants: Vec<CompanySummary>,
        }
        let reply = self
            .send(&AUTH_ME, Call::at(Method::Get, AUTH_ME.route()), None)
            .await?;
        let me: Me = decode(&reply, "accessible tenants")?;
        let workspace = self.inner.client.workspace_tenant().await;
        Ok(me
            .accessible_tenants
            .into_iter()
            .filter(|c| Some(c.tenant_id) != workspace)
            .collect())
    }

    /// Creates a company (a new engine tenant). Never retried automatically:
    /// a retry could create a second company. After an error the caller
    /// refreshes [`Session::companies`] before offering "try again".
    ///
    /// # Errors
    /// Any [`ApiError`] of the request.
    pub async fn create_company(&self, company: &NewCompany) -> Result<CreatedCompany, ApiError> {
        let call = Call::at(Method::Post, CREATE_TENANT.route())
            .json_body(json_body(company)?)
            .timeout(TimeoutClass::CreateTenant);
        let reply = self.send(&CREATE_TENANT, call, None).await?;
        // 2xx: the company exists even if its answer cannot be read.
        decode(&reply, "created company").map_err(crate::resources::versioning::saved_unreadable)
    }

    /// Switches this session to a company. Results of reads issued before
    /// the switch are stale afterwards; writes in flight still commit in the
    /// previous company.
    ///
    /// # Errors
    /// `NotFound` for an unknown tenant, `Forbidden` without access, any
    /// transport error. If the company id cannot be read after the switch,
    /// the session switches back to the workspace and returns the error.
    pub async fn open_company(&self, tenant: TenantId) -> Result<OpenCompany, ApiError> {
        self.switch_to(tenant).await?;
        let company = match self.read_company_id().await {
            Ok(c) => c,
            Err(e) => {
                if let Some(ws) = self.inner.client.workspace_tenant().await {
                    let _ = self.switch_to(ws).await;
                }
                self.set_scope(Scope::Workspace);
                return Err(e);
            }
        };
        self.set_scope(Scope::Company {
            tenant_id: tenant,
            company_id: company,
        });
        Ok(OpenCompany {
            tenant_id: tenant,
            company_id: company,
        })
    }

    /// Switches back to the owner workspace.
    ///
    /// # Errors
    /// Any [`ApiError`] of the switch; `Decode` when the engine has no
    /// workspace tenant.
    pub async fn close_company(&self) -> Result<(), ApiError> {
        let ws = self
            .inner
            .client
            .workspace_tenant()
            .await
            .ok_or(ApiError::Decode {
                what: "workspace tenant",
            })?;
        self.switch_to(ws).await?;
        self.set_scope(Scope::Workspace);
        Ok(())
    }

    /// Ends the session on the engine (best effort) and wipes the tokens.
    ///
    /// # Errors
    /// The logout request's error; the local session ends either way.
    pub async fn logout(self) -> Result<(), ApiError> {
        let _g = self.inner.rotate.lock().await;
        let cookie = {
            let st = lock(&self.inner.state);
            if st.ended { None } else { st.refresh.clone() }
        };
        let result = if let Some(cookie) = cookie {
            let auth = StaticAuth(AttemptAuth {
                bearer: None,
                refresh_cookie: Some(cookie),
            });
            let call = Call::at(Method::Post, LOGOUT.route()).timeout(TimeoutClass::Auth);
            self.client()
                .exec()
                .run(&LOGOUT, call, &auth, None)
                .await
                .map(drop)
        } else {
            Ok(())
        };
        lock(&self.inner.state).end();
        result
    }

    pub(crate) fn set_scope(&self, scope: Scope) {
        let mut st = lock(&self.inner.state);
        st.scope = scope;
        st.scope_epoch += 1;
    }

    async fn read_company_id(&self) -> Result<CompanyId, ApiError> {
        #[derive(Deserialize)]
        struct Wrapper {
            company: Row,
        }
        #[derive(Deserialize)]
        struct Row {
            id: CompanyId,
        }
        let reply = self
            .send(
                &COMPANY_GET,
                Call::at(Method::Get, COMPANY_GET.route()),
                None,
            )
            .await?;
        Ok(decode::<Wrapper>(&reply, "company")?.company.id)
    }

    /// `POST /auth/switch-tenant` under the rotation lock, with the cookie so
    /// the engine revokes the previous session.
    async fn switch_to(&self, tenant: TenantId) -> Result<(), ApiError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body {
            tenant_id: TenantId,
        }
        self.inner.ensure_fresh(None).await?;
        let _g = self.inner.rotate.lock().await;
        let (bearer, cookie) = {
            let st = lock(&self.inner.state);
            if st.ended {
                return Err(ApiError::Unauthorized(AuthFailure::SessionExpired));
            }
            (st.access.clone(), st.refresh.clone())
        };
        let cookie = cookie.ok_or(ApiError::Unauthorized(AuthFailure::SessionExpired))?;
        let auth = StaticAuth(AttemptAuth {
            bearer: Some(bearer),
            refresh_cookie: Some(cookie),
        });
        let call = Call::at(Method::Post, SWITCH_TENANT.route())
            .json_body(json_body(&Body { tenant_id: tenant })?)
            .timeout(TimeoutClass::Auth);
        let reply = self
            .client()
            .exec()
            .run(&SWITCH_TENANT, call, &auth, None)
            .await?;
        self.inner.store_rotated(&reply)
    }

    /// Sends one request in this session: checks the scope (company routes
    /// fail locally with `NoCompanyOpen` in the workspace), adds
    /// `X-Company-Id`, and runs it with refresh handling.
    ///
    /// A company request is pinned for its whole life (every attempt) to
    /// this view's pin, or to the scope read once here for an unpinned
    /// session, so the company id and the token never come from two
    /// different companies.
    pub(crate) async fn send(
        &self,
        op: &Operation,
        mut call: Call,
        reconciler: Option<&dyn Reconciler>,
    ) -> Result<EngineReply, ApiError> {
        if !op.company_scoped() {
            return self
                .client()
                .exec()
                .run(op, call, self.inner.as_ref(), reconciler)
                .await;
        }
        let pin = self.pin_now();
        match pin.scope {
            Scope::Company { company_id, .. } => call.company = Some(company_id),
            Scope::Workspace => return Err(ApiError::NoCompanyOpen),
        }
        let auth = PinnedAuth {
            inner: self.inner.as_ref(),
            epoch: pin.epoch,
        };
        self.client().exec().run(op, call, &auth, reconciler).await
    }
}

impl SessionInner {
    /// Credentials for the next attempt. With `pin`, the scope epoch is
    /// checked under the same lock: a moved epoch is `StaleScope`.
    fn snapshot(&self, pin: Option<u64>) -> Result<AttemptCreds, ApiError> {
        let st = lock(&self.state);
        if st.ended {
            return Err(ApiError::Unauthorized(AuthFailure::SessionExpired));
        }
        if pin.is_some_and(|e| e != st.scope_epoch) {
            return Err(ApiError::StaleScope);
        }
        Ok(AttemptCreds {
            auth: AttemptAuth {
                bearer: Some(st.access.clone()),
                refresh_cookie: None,
            },
            epoch: st.auth_epoch,
            access_exp: st.access_exp,
        })
    }

    /// Refreshes first when the access token is about to expire.
    async fn ensure_fresh(&self, pin: Option<u64>) -> Result<AttemptCreds, ApiError> {
        let creds = self.snapshot(pin)?;
        match creds.access_exp {
            Some(exp) if exp.saturating_sub(unix_now()) < REFRESH_MARGIN_SECS => {
                self.refresh_if(creds.epoch).await?;
                self.snapshot(pin)
            }
            _ => Ok(creds),
        }
    }

    /// Single-flight refresh: when another task already refreshed since
    /// `epoch_seen`, its new token is used instead.
    async fn refresh_if(&self, epoch_seen: u64) -> Result<(), ApiError> {
        let _g = self.rotate.lock().await;
        let cookie = {
            let mut st = lock(&self.state);
            if st.ended {
                return Err(ApiError::Unauthorized(AuthFailure::SessionExpired));
            }
            if st.auth_epoch != epoch_seen {
                return Ok(());
            }
            let Some(c) = st.refresh.clone() else {
                st.end();
                return Err(ApiError::Unauthorized(AuthFailure::SessionExpired));
            };
            c
        };
        let auth = StaticAuth(AttemptAuth {
            bearer: None,
            refresh_cookie: Some(cookie),
        });
        let call = Call::at(Method::Post, REFRESH.route()).timeout(TimeoutClass::Auth);
        match self.client.exec().run(&REFRESH, call, &auth, None).await {
            Ok(reply) => {
                tracing::debug!("session refreshed");
                self.store_rotated(&reply)
            }
            Err(ApiError::Unauthorized(_)) => {
                lock(&self.state).end();
                tracing::info!("session expired");
                Err(ApiError::Unauthorized(AuthFailure::SessionExpired))
            }
            Err(e) => Err(e),
        }
    }

    /// Stores the access token and rotated refresh cookie of a refresh or
    /// switch answer and bumps the auth epoch. Call with `rotate` held.
    fn store_rotated(&self, reply: &EngineReply) -> Result<(), ApiError> {
        let wire: AuthWire = decode(reply, "token answer")?;
        let tokens = wire.tokens.ok_or(ApiError::Decode { what: "tokens" })?;
        let cookie = parse_refresh_cookie(&reply.set_cookies)?;
        let access = Secret::new(tokens.access_token);
        let mut st = lock(&self.state);
        st.access_exp = jwt_exp(access.expose());
        st.access = access;
        match cookie {
            CookieUpdate::Set(c) => st.refresh = Some(c),
            // Rotation is unconditional on the engine, so the old cookie is
            // dead; the next refresh will end the session.
            CookieUpdate::Absent | CookieUpdate::Cleared => st.refresh = None,
        }
        st.auth_epoch += 1;
        Ok(())
    }
}

/// Test-only session hooks for the contract harness (feature `test-support`).
#[cfg(feature = "test-support")]
impl Session {
    fn raw_call(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&serde_json::Value>,
    ) -> Result<Call, ApiError> {
        let mut call = Call::new(method, "/raw", path.to_string());
        call.query = query
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        if let Some(b) = body {
            call = call.json_body(json_body(b)?);
        }
        if let Scope::Company { company_id, .. } = self.scope() {
            call.company = Some(company_id);
        }
        Ok(call)
    }

    /// One raw attempt with this session's token: any status comes back as is
    /// (no retry, no refresh, no error mapping). Sends `X-Company-Id` when a
    /// company is open.
    ///
    /// # Errors
    /// Only transport failures and a body that cannot be encoded.
    pub async fn raw(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&serde_json::Value>,
    ) -> Result<crate::testing::RawReply, ApiError> {
        let call = self.raw_call(method, path, query, body)?;
        let creds = self.inner.ensure_fresh(None).await?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        let reply = self
            .client()
            .exec()
            .transport()
            .attempt(&call, &creds.auth, 1, deadline)
            .await
            .map_err(|_| ApiError::EngineUnavailable { permanent: false })?;
        let body = serde_json::from_slice(&reply.body).unwrap_or(serde_json::Value::Null);
        Ok(crate::testing::RawReply {
            status: reply.status,
            body,
        })
    }

    /// A raw request through the normal executor: retry policy, refresh and
    /// the typed error mapping all apply. Returns the 2xx JSON body.
    ///
    /// # Errors
    /// The mapped [`ApiError`].
    pub async fn raw_mapped(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, ApiError> {
        let op = crate::resources::operations::raw(method);
        let call = self.raw_call(method, path, &[], body)?;
        let reply = self.send(&op, call, None).await?;
        Ok(serde_json::from_slice(&reply.body).unwrap_or(serde_json::Value::Null))
    }

    /// Rotate the refresh token now (single-flight like any refresh).
    ///
    /// # Errors
    /// `Unauthorized(SessionExpired)` when the engine refuses the cookie.
    pub async fn force_refresh(&self) -> Result<(), ApiError> {
        let epoch = lock(&self.inner.state).auth_epoch;
        self.inner.refresh_if(epoch).await
    }

    /// A copy of the current refresh cookie (to prove an old one is dead).
    #[must_use]
    pub fn refresh_cookie_for_test(&self) -> Option<Secret> {
        lock(&self.inner.state).refresh.clone()
    }

    /// The current access token (for the logging canary only).
    #[must_use]
    pub fn access_token_for_test(&self) -> Secret {
        lock(&self.inner.state).access.clone()
    }

    /// Mark the access token as expiring now, so the next request refreshes
    /// first (the proactive path).
    pub fn expire_access_for_test(&self) {
        lock(&self.inner.state).access_exp = Some(0);
    }

    /// Replace the access token with one the engine refuses, keeping a
    /// far-future expiry, so the next request meets a 401 `UNAUTHORIZED` (the
    /// reactive path).
    pub fn corrupt_access_for_test(&self) {
        let mut st = lock(&self.inner.state);
        st.access = Secret::new("eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjQxMDI0NDQ4MDB9.invalid");
        st.access_exp = Some(4_102_444_800);
    }
}

#[cfg(feature = "test-support")]
impl Client {
    /// `POST /auth/refresh` with an explicit cookie, through the normal error
    /// mapping (to show a rotated-away cookie is refused).
    ///
    /// # Errors
    /// The mapped [`ApiError`].
    pub async fn refresh_with_cookie_for_test(&self, cookie: &Secret) -> Result<u16, ApiError> {
        let auth = StaticAuth(AttemptAuth {
            bearer: None,
            refresh_cookie: Some(cookie.clone()),
        });
        let call = Call::at(Method::Post, REFRESH.route()).timeout(TimeoutClass::Auth);
        let reply = self.exec().run(&REFRESH, call, &auth, None).await?;
        Ok(reply.status)
    }
}

impl AuthHook for SessionInner {
    fn prepare(&self) -> BoxFuture<'_, Result<AttemptCreds, ApiError>> {
        Box::pin(self.ensure_fresh(None))
    }

    fn rejected<'a>(&'a self, creds: &'a AttemptCreds) -> BoxFuture<'a, Result<(), ApiError>> {
        Box::pin(self.refresh_if(creds.epoch))
    }
}

/// Session credentials for a company request pinned to scope `epoch`.
struct PinnedAuth<'a> {
    inner: &'a SessionInner,
    epoch: u64,
}

impl AuthHook for PinnedAuth<'_> {
    fn prepare(&self) -> BoxFuture<'_, Result<AttemptCreds, ApiError>> {
        Box::pin(self.inner.ensure_fresh(Some(self.epoch)))
    }

    fn rejected<'a>(&'a self, creds: &'a AttemptCreds) -> BoxFuture<'a, Result<(), ApiError>> {
        Box::pin(self.inner.refresh_if(creds.epoch))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::client::tests::{TOKEN, fast_config};
    use crate::endpoint::FixedEndpoint;
    use crate::resources::operations::find;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn b64url(bytes: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
            let chars = chunk.len() + 1;
            for i in 0..chars {
                out.push(char::from(A[((n >> (18 - 6 * i)) & 63) as usize]));
            }
        }
        out
    }

    #[test]
    fn jwt_exp_is_read_without_verification() {
        let payload = b64url(br#"{"userId":"u","exp":1760000000}"#);
        assert_eq!(jwt_exp(&format!("h.{payload}.sig")), Some(1_760_000_000));
        assert_eq!(jwt_exp("not-a-jwt"), None);
        assert_eq!(jwt_exp("a.!!!.c"), None);
        assert_eq!(jwt_exp(&format!("h.{}.s", b64url(b"{}"))), None);
    }

    fn sc(s: &str) -> Secret {
        Secret::new(s)
    }

    #[test]
    fn refresh_cookie_parsing() {
        let tok = "a".repeat(96);
        let set =
            format!("kb_refresh={tok}; HttpOnly; SameSite=Lax; Path=/api/v1/auth; Max-Age=604800");
        assert_eq!(
            parse_refresh_cookie(&[sc("other=1; Path=/"), sc(&set)]).expect("ok"),
            CookieUpdate::Set(Secret::new(tok))
        );
        assert_eq!(
            parse_refresh_cookie(&[sc("kb_refresh=; HttpOnly; Max-Age=0")]).expect("ok"),
            CookieUpdate::Cleared
        );
        assert_eq!(
            parse_refresh_cookie(&[sc(&format!("kb_refresh={}; max-age=0", "b".repeat(20)))])
                .expect("ok"),
            CookieUpdate::Cleared
        );
        assert_eq!(
            parse_refresh_cookie(&[sc("x=y")]).expect("ok"),
            CookieUpdate::Absent
        );
        assert!(parse_refresh_cookie(&[sc("kb_refresh=short")]).is_err());
        assert!(parse_refresh_cookie(&[sc("kb_refresh=aaaaaaaaaa bbbbbbbbbb")]).is_err());
        assert!(parse_refresh_cookie(&[sc(&format!("kb_refresh={}", "x".repeat(513)))]).is_err());
        assert!(
            parse_refresh_cookie(&[sc(&format!("kb_refresh={}.dot", "x".repeat(20)))]).is_err()
        );
    }

    pub(crate) fn session_on(server: &MockServer, scope: Scope) -> Session {
        let src = Arc::new(FixedEndpoint::new(
            server.address().port(),
            Secret::new(TOKEN),
            1,
        ));
        session_with(src, scope)
    }

    pub(crate) fn session_with(src: Arc<dyn EndpointSource>, scope: Scope) -> Session {
        let client = Client::new(src, fast_config()).expect("client");
        Session {
            inner: Arc::new(SessionInner {
                client,
                user_id: None,
                state: Mutex::new(AuthState {
                    access: Secret::new("access"),
                    access_exp: None,
                    refresh: Some(Secret::new("r".repeat(32))),
                    auth_epoch: 0,
                    scope,
                    scope_epoch: 0,
                    ended: false,
                }),
                rotate: tokio::sync::Mutex::new(()),
            }),
            pin: None,
        }
    }

    #[tokio::test]
    async fn company_routes_need_an_open_company_and_send_its_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/accounts"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        let op = *find(Method::Get, "/api/v1/accounts").expect("op");
        let ws = session_on(&server, Scope::Workspace);
        let e = ws
            .send(&op, Call::at(Method::Get, op.route()), None)
            .await
            .expect_err("workspace");
        assert_eq!(e, ApiError::NoCompanyOpen);
        assert_eq!(server.received_requests().await.expect("r").len(), 0);

        let company = CompanyId::parse("0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id");
        let tenant = TenantId::parse("1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id");
        let co = session_on(
            &server,
            Scope::Company {
                tenant_id: tenant,
                company_id: company,
            },
        );
        co.send(&op, Call::at(Method::Get, op.route()), None)
            .await
            .expect("sent");
        let reqs = server.received_requests().await.expect("r");
        assert_eq!(
            reqs[0].headers.get("x-company-id").expect("id"),
            company.to_string().as_str()
        );
        assert_eq!(
            reqs[0].headers.get("authorization").expect("bearer"),
            "Bearer access"
        );
        assert!(reqs[0].headers.get("cookie").is_none());
    }

    #[tokio::test]
    async fn workspace_fence_409_maps_to_no_company_open() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(409).set_body_raw(
                r#"{"error":{"message":"Open a company first","code":"WORKSPACE_TENANT"}}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        let s = session_on(&server, Scope::Workspace);
        let e = s
            .send(&AUTH_ME, Call::at(Method::Get, AUTH_ME.route()), None)
            .await
            .expect_err("fence");
        assert_eq!(e, ApiError::NoCompanyOpen);
    }

    #[tokio::test]
    async fn ended_session_sends_nothing() {
        let server = MockServer::start().await;
        let s = session_on(&server, Scope::Workspace);
        lock(&s.inner.state).end();
        let e = s
            .send(&AUTH_ME, Call::at(Method::Get, AUTH_ME.route()), None)
            .await
            .expect_err("ended");
        assert_eq!(e, ApiError::Unauthorized(AuthFailure::SessionExpired));
        assert_eq!(server.received_requests().await.expect("r").len(), 0);
        assert!(format!("{s:?}").contains("Workspace"));
    }
}
