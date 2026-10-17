//! Stub-server tests of the public session API (no engine needed).
//!
//! A wiremock server plays the engine's auth routes as they behave at the
//! pinned engine (`engine/packages/api/src/routes/auth.routes.ts`): the
//! refresh token only ever travels in the `kb_refresh` cookie, refresh
//! rotates it, and TFA errors carry no code.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cl_api::{
    ApiError, AuthFailure, Client, ClientConfig, FixedEndpoint, LoginOutcome, NewCompany, Scope,
    Session, TfaMethod,
};
use cl_core::Secret;
use cl_core::ids::{CompanyId, TenantId};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const LAUNCH: &str = "launch-token-canary-0123456789";
const WS: &str = "00000000-0000-4000-8000-0000000000a1";
const T1: &str = "00000000-0000-4000-8000-0000000000b1";
const C1: &str = "00000000-0000-4000-8000-0000000000c1";
const PASSWORD: &str = "pw-canary-Secret-123";
const EMAIL: &str = "owner-canary@example.test";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn b64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(char::from(A[((n >> (18 - 6 * i)) & 63) as usize]));
        }
    }
    out
}

/// A JWT-shaped access token expiring `secs` from now, tagged by `tag`.
fn jwt(tag: &str, secs: u64) -> String {
    let payload = format!(r#"{{"userId":"u","tag":"{tag}","exp":{}}}"#, now() + secs);
    format!("e30.{}.sig-{tag}", b64url(payload.as_bytes()))
}

fn refresh_value(tag: char) -> String {
    std::iter::repeat_n(tag, 96).collect()
}

fn set_cookie(tag: char) -> String {
    format!(
        "kb_refresh={}; HttpOnly; SameSite=Lax; Path=/api/v1/auth; Max-Age=604800",
        refresh_value(tag)
    )
}

fn cookie_header(tag: char) -> String {
    format!("kb_refresh={}", refresh_value(tag))
}

fn json(status: u16, body: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json")
}

fn tokens(access: &str, cookie: char) -> ResponseTemplate {
    json(
        200,
        &format!(r#"{{"tokens":{{"accessToken":"{access}"}}}}"#),
    )
    .append_header("set-cookie", "other=1; Path=/")
    .append_header("set-cookie", set_cookie(cookie).as_str())
}

fn tenants_body() -> String {
    format!(
        r#"{{"user":{{"id":"{WS}"}},"companies":[],"accessibleTenants":[
            {{"tenantId":"{WS}","tenantName":"Workspace","role":"owner","lastAccessedAt":null}},
            {{"tenantId":"{T1}","tenantName":"Canary Co","role":"owner","lastAccessedAt":"2026-10-01T10:00:00.000Z"}}]}}"#
    )
}

fn config() -> ClientConfig {
    let mut cfg = ClientConfig::default();
    let t = &mut cfg.timeouts;
    for b in [
        &mut t.read,
        &mut t.write,
        &mut t.auth,
        &mut t.probe,
        &mut t.create_tenant,
    ] {
        b.attempt = Duration::from_secs(2);
        b.overall = Duration::from_secs(4);
    }
    t.poll = Duration::from_millis(20);
    cfg.connect_timeout = Duration::from_millis(500);
    cfg
}

async fn engine() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/desktop/info"))
        .respond_with(json(
            200,
            &format!(r#"{{"mode":"desktop","initialized":true,"workspaceTenantId":"{WS}","capabilities":["error-codes-v1"]}}"#),
        ))
        .mount(&server)
        .await;
    server
}

fn client(server: &MockServer) -> Client {
    let src = Arc::new(FixedEndpoint::new(
        server.address().port(),
        Secret::new(LAUNCH),
        1,
    ));
    Client::new(src, config()).expect("client")
}

async fn mount_login(server: &MockServer, access: &str, cookie: char) {
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(
            json(
                200,
                &format!(
                    r#"{{"user":{{"id":"{WS}"}},"tokens":{{"accessToken":"{access}"}},"accessibleTenants":[]}}"#
                ),
            )
            .append_header("set-cookie", set_cookie(cookie).as_str()),
        )
        .mount(server)
        .await;
}

async fn login(c: &Client) -> Session {
    match c.login(EMAIL, &Secret::new(PASSWORD)).await.expect("login") {
        LoginOutcome::Session(s) => s,
        LoginOutcome::TfaRequired(_) => panic!("unexpected tfa"),
    }
}

async fn requests(server: &MockServer, p: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .filter(|r| r.url.path() == p)
        .collect()
}

fn hdr<'a>(r: &'a Request, name: &str) -> Option<&'a str> {
    r.headers.get(name).and_then(|v| v.to_str().ok())
}

fn tid(s: &str) -> TenantId {
    TenantId::parse(s).expect("id")
}

#[tokio::test]
async fn login_starts_in_the_workspace_with_only_the_launch_token() {
    let server = engine().await;
    mount_login(&server, &jwt("a", 3600), 'A').await;
    let c = client(&server);
    let s = login(&c).await;
    assert_eq!(s.scope(), Scope::Workspace);
    assert_eq!(s.scope_epoch(), 0);
    assert!(s.user_id().is_some());
    for p in ["/api/v1/desktop/info", "/api/v1/auth/login"] {
        let reqs = requests(&server, p).await;
        assert_eq!(reqs.len(), 1, "{p}");
        assert_eq!(hdr(&reqs[0], "x-cl-token"), Some(LAUNCH));
        assert_eq!(hdr(&reqs[0], "authorization"), None, "{p}");
        assert_eq!(hdr(&reqs[0], "cookie"), None, "{p}");
        assert_eq!(hdr(&reqs[0], "origin"), None, "{p}");
    }
    let body: serde_json::Value =
        serde_json::from_slice(&requests(&server, "/api/v1/auth/login").await[0].body)
            .expect("json");
    assert_eq!(body["email"], EMAIL);
    assert_eq!(body["password"], PASSWORD);
    assert!(c.capabilities().has("error-codes-v1"));
}

#[tokio::test]
async fn bad_password_is_invalid_credentials_and_not_retried() {
    let server = engine().await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(json(
            401,
            r#"{"error":{"message":"Invalid email or password","code":"INVALID_CREDENTIALS"}}"#,
        ))
        .mount(&server)
        .await;
    let c = client(&server);
    let e = c
        .login(EMAIL, &Secret::new("wrong"))
        .await
        .expect_err("refused");
    assert_eq!(e, ApiError::Unauthorized(AuthFailure::InvalidCredentials));
    assert_eq!(requests(&server, "/api/v1/auth/login").await.len(), 1);
}

async fn tfa_challenge(c: &Client) -> cl_api::TfaChallenge {
    match c.login(EMAIL, &Secret::new(PASSWORD)).await.expect("login") {
        LoginOutcome::TfaRequired(ch) => ch,
        LoginOutcome::Session(_) => panic!("expected tfa"),
    }
}

#[tokio::test]
async fn tfa_branch_verifies_with_the_tfa_token() {
    let server = engine().await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(json(
            200,
            r#"{"tfa_required":true,"tfa_token":"tfa-token-canary","available_methods":["totp","email"],
                "preferred_method":"totp","phone_masked":null,"email_masked":"o***@example.test"}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/tfa/verify"))
        .and(header("authorization", "Bearer tfa-token-canary"))
        .and(body_partial_json(serde_json::json!({"code":"123456","method":"totp"})))
        .respond_with(
            json(
                200,
                &format!(
                    r#"{{"user":{{"id":"{WS}"}},"tokens":{{"accessToken":"{}"}},"accessibleTenants":[]}}"#,
                    jwt("t", 3600)
                ),
            )
            .append_header("set-cookie", set_cookie('T').as_str()),
        )
        .mount(&server)
        .await;
    let c = client(&server);
    let ch = tfa_challenge(&c).await;
    assert_eq!(
        ch.methods(),
        &[TfaMethod::new("totp"), TfaMethod::new("email")]
    );
    assert_eq!(ch.preferred().map(TfaMethod::as_str), Some("totp"));
    assert!(!format!("{ch:?}").contains("tfa-token-canary"));
    let s = ch
        .verify(&Secret::new("123456"), &TfaMethod::new("totp"))
        .await
        .expect("verified");
    assert_eq!(s.scope(), Scope::Workspace);
}

#[tokio::test]
async fn tfa_failures_are_typed_and_never_refresh() {
    let server = engine().await;
    Mock::given(path("/api/v1/auth/login"))
        .respond_with(json(
            200,
            r#"{"tfa_required":true,"tfa_token":"tfa-token-canary","available_methods":["totp"]}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/tfa/verify"))
        .and(body_partial_json(serde_json::json!({"code":"000000"})))
        .respond_with(json(
            400,
            r#"{"error":{"message":"Account temporarily locked","remaining_attempts":0,"locked_until":"2026-10-15T12:30:00.000Z"}}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/tfa/verify"))
        .and(body_partial_json(serde_json::json!({"code":"111111"})))
        .respond_with(json(
            401,
            r#"{"error":{"message":"Invalid or expired TFA token"}}"#,
        ))
        .mount(&server)
        .await;
    let c = client(&server);
    let totp = TfaMethod::new("totp");
    let e = tfa_challenge(&c)
        .await
        .verify(&Secret::new("000000"), &totp)
        .await
        .expect_err("locked");
    assert!(matches!(
        e,
        ApiError::Unauthorized(AuthFailure::AccountLocked { until: Some(_) })
    ));
    let e = tfa_challenge(&c)
        .await
        .verify(&Secret::new("111111"), &totp)
        .await
        .expect_err("invalid");
    assert_eq!(e, ApiError::Unauthorized(AuthFailure::TfaInvalid));
    assert_eq!(requests(&server, "/api/v1/auth/refresh").await.len(), 0);
    assert_eq!(requests(&server, "/api/v1/auth/tfa/verify").await.len(), 2);
}

async fn mount_me(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/me"))
        .respond_with(json(200, &tenants_body()))
        .mount(server)
        .await;
}

#[tokio::test]
async fn refresh_is_single_flight_and_lists_companies_without_the_workspace() {
    let server = engine().await;
    mount_login(&server, &jwt("old", 30), 'A').await;
    let fresh = jwt("new", 3600);
    Mock::given(path("/api/v1/auth/refresh"))
        .and(header("cookie", cookie_header('A').as_str()))
        .respond_with(tokens(&fresh, 'B'))
        .mount(&server)
        .await;
    mount_me(&server).await;
    let c = client(&server);
    let s = login(&c).await;
    let mut tasks = Vec::new();
    for _ in 0..10 {
        let s = s.clone();
        tasks.push(tokio::spawn(async move { s.companies().await }));
    }
    for t in tasks {
        let list = t.await.expect("join").expect("companies");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].tenant_id, tid(T1));
        assert_eq!(list[0].tenant_name, "Canary Co");
    }
    let refreshes = requests(&server, "/api/v1/auth/refresh").await;
    assert_eq!(refreshes.len(), 1);
    assert_eq!(hdr(&refreshes[0], "authorization"), None);
    assert_eq!(hdr(&refreshes[0], "x-cl-token"), Some(LAUNCH));
    let bearer = format!("Bearer {fresh}");
    for r in requests(&server, "/api/v1/auth/me").await {
        assert_eq!(hdr(&r, "authorization"), Some(bearer.as_str()));
        assert_eq!(hdr(&r, "cookie"), None);
    }
}

#[tokio::test]
async fn refresh_rotation_uses_the_newest_cookie() {
    let server = engine().await;
    mount_login(&server, &jwt("a", 30), 'A').await;
    Mock::given(path("/api/v1/auth/refresh"))
        .and(header("cookie", cookie_header('A').as_str()))
        .respond_with(tokens(&jwt("b", 30), 'B'))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/refresh"))
        .and(header("cookie", cookie_header('B').as_str()))
        .respond_with(tokens(&jwt("c", 3600), 'C'))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    // Reusing a rotated token is refused, as the engine does.
    Mock::given(path("/api/v1/auth/refresh"))
        .respond_with(json(
            401,
            r#"{"error":{"message":"Invalid refresh token","code":"UNAUTHORIZED"}}"#,
        ))
        .mount(&server)
        .await;
    mount_me(&server).await;
    let s = login(&client(&server)).await;
    s.companies().await.expect("first refresh");
    s.companies().await.expect("second refresh");
    s.companies().await.expect("no refresh needed");
    let cookies: Vec<String> = requests(&server, "/api/v1/auth/refresh")
        .await
        .iter()
        .filter_map(|r| hdr(r, "cookie").map(str::to_string))
        .collect();
    assert_eq!(cookies, vec![cookie_header('A'), cookie_header('B')]);
}

#[tokio::test]
async fn failed_refresh_ends_the_session_before_a_mutation_is_sent() {
    let server = engine().await;
    mount_login(&server, &jwt("a", 30), 'A').await;
    Mock::given(path("/api/v1/auth/refresh"))
        .respond_with(json(
            401,
            r#"{"error":{"message":"Refresh token expired","code":"UNAUTHORIZED"}}"#,
        ))
        .mount(&server)
        .await;
    let s = login(&client(&server)).await;
    let e = s
        .create_company(&NewCompany {
            company_name: "Canary Co".into(),
            business_type: Some("default".into()),
            entity_type: None,
        })
        .await
        .expect_err("expired");
    assert_eq!(e, ApiError::Unauthorized(AuthFailure::SessionExpired));
    assert_eq!(
        requests(&server, "/api/v1/auth/create-tenant").await.len(),
        0
    );
    let before = server.received_requests().await.expect("r").len();
    let e = s.companies().await.expect_err("ended");
    assert_eq!(e, ApiError::Unauthorized(AuthFailure::SessionExpired));
    assert_eq!(server.received_requests().await.expect("r").len(), before);
}

#[tokio::test]
async fn engine_unauthorized_on_a_read_refreshes_and_replays() {
    let server = engine().await;
    mount_login(&server, &jwt("a", 3600), 'A').await;
    Mock::given(path("/api/v1/auth/refresh"))
        .respond_with(tokens(&jwt("b", 3600), 'B'))
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/me"))
        .respond_with(json(
            401,
            r#"{"error":{"message":"Token revoked","code":"UNAUTHORIZED"}}"#,
        ))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    mount_me(&server).await;
    let s = login(&client(&server)).await;
    assert_eq!(s.companies().await.expect("replayed").len(), 1);
    assert_eq!(requests(&server, "/api/v1/auth/refresh").await.len(), 1);
    assert_eq!(requests(&server, "/api/v1/auth/me").await.len(), 2);
}

#[tokio::test]
async fn b4_codeless_401_never_sends_the_refresh_cookie() {
    for body in ["", r#"{"error":{"message":"Unauthorized"}}"#] {
        let server = engine().await;
        mount_login(&server, &jwt("a", 3600), 'A').await;
        Mock::given(path("/api/v1/auth/refresh"))
            .respond_with(tokens(&jwt("b", 3600), 'B'))
            .mount(&server)
            .await;
        Mock::given(path("/api/v1/auth/me"))
            .respond_with(ResponseTemplate::new(401).set_body_string(body))
            .mount(&server)
            .await;
        let s = login(&client(&server)).await;
        let e = s.companies().await.expect_err("not the engine");
        assert_eq!(e, ApiError::EngineUnavailable { permanent: false });
        assert_eq!(requests(&server, "/api/v1/auth/refresh").await.len(), 0);
        for r in server.received_requests().await.expect("r") {
            assert_eq!(hdr(&r, "cookie"), None, "{}", r.url.path());
        }
    }
}

async fn mount_switch(server: &MockServer, tenant: &str, need_cookie: char, gives: char) {
    Mock::given(path("/api/v1/auth/switch-tenant"))
        .and(header("cookie", cookie_header(need_cookie).as_str()))
        .and(body_partial_json(serde_json::json!({ "tenantId": tenant })))
        .respond_with(tokens(&jwt(&format!("sw-{gives}"), 3600), gives))
        .mount(server)
        .await;
}

#[tokio::test]
async fn open_and_close_company_switch_tenants_and_bump_the_scope_epoch() {
    let server = engine().await;
    let first = jwt("a", 3600);
    mount_login(&server, &first, 'A').await;
    mount_switch(&server, T1, 'A', 'B').await;
    mount_switch(&server, WS, 'B', 'C').await;
    Mock::given(method("GET"))
        .and(path("/api/v1/company"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"company":{{"id":"{C1}","tenantId":"{T1}","businessName":"Canary Co"}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = login(&client(&server)).await;
    let open = s.open_company(tid(T1)).await.expect("open");
    let company = CompanyId::parse(C1).expect("id");
    assert_eq!(open.company_id, company);
    assert_eq!(
        s.scope(),
        Scope::Company {
            tenant_id: tid(T1),
            company_id: company
        }
    );
    assert_eq!(s.scope_epoch(), 1);
    let sw = requests(&server, "/api/v1/auth/switch-tenant").await;
    let bearer = format!("Bearer {first}");
    assert_eq!(hdr(&sw[0], "authorization"), Some(bearer.as_str()));
    let get = requests(&server, "/api/v1/company").await;
    assert_eq!(hdr(&get[0], "x-company-id"), None);
    assert_eq!(hdr(&get[0], "cookie"), None);
    s.close_company().await.expect("close");
    assert_eq!(s.scope(), Scope::Workspace);
    assert_eq!(s.scope_epoch(), 2);
    assert_eq!(
        requests(&server, "/api/v1/auth/switch-tenant").await.len(),
        2
    );
}

#[tokio::test]
async fn open_company_switches_back_when_the_company_cannot_be_read() {
    let server = engine().await;
    mount_login(&server, &jwt("a", 3600), 'A').await;
    mount_switch(&server, T1, 'A', 'B').await;
    mount_switch(&server, WS, 'B', 'C').await;
    Mock::given(path("/api/v1/company"))
        .respond_with(json(
            404,
            r#"{"error":{"message":"Company not found","code":"NOT_FOUND"}}"#,
        ))
        .mount(&server)
        .await;
    let s = login(&client(&server)).await;
    let e = s.open_company(tid(T1)).await.expect_err("no company");
    assert!(matches!(e, ApiError::NotFound { .. }));
    assert_eq!(s.scope(), Scope::Workspace);
    assert_eq!(
        requests(&server, "/api/v1/auth/switch-tenant").await.len(),
        2
    );

    let unknown = engine().await;
    mount_login(&unknown, &jwt("a", 3600), 'A').await;
    Mock::given(path("/api/v1/auth/switch-tenant"))
        .respond_with(json(
            404,
            r#"{"error":{"message":"That company does not exist.","code":"TENANT_NOT_FOUND"}}"#,
        ))
        .mount(&unknown)
        .await;
    let s = login(&client(&unknown)).await;
    let e = s.open_company(tid(T1)).await.expect_err("unknown");
    assert!(matches!(e, ApiError::NotFound { code: Some(c) } if c.as_str() == "TENANT_NOT_FOUND"));
    assert_eq!(s.scope_epoch(), 0);
}

#[tokio::test]
async fn create_company_is_sent_once() {
    let server = engine().await;
    mount_login(&server, &jwt("a", 3600), 'A').await;
    Mock::given(path("/api/v1/auth/create-tenant"))
        .and(body_partial_json(
            serde_json::json!({"companyName":"Canary Co","businessType":"default"}),
        ))
        .respond_with(json(
            201,
            &format!(r#"{{"tenantId":"{T1}","companyId":"{C1}","tenantName":"Canary Co"}}"#),
        ))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(path("/api/v1/auth/create-tenant"))
        .respond_with(json(
            500,
            r#"{"error":{"message":"x","code":"INTERNAL_ERROR"}}"#,
        ))
        .mount(&server)
        .await;
    let s = login(&client(&server)).await;
    let new = NewCompany {
        company_name: "Canary Co".into(),
        business_type: Some("default".into()),
        entity_type: None,
    };
    let created = s.create_company(&new).await.expect("created");
    assert_eq!(created.tenant_id, tid(T1));
    let e = s.create_company(&new).await.expect_err("server");
    assert!(matches!(e, ApiError::Server { status: 500, .. }));
    let reqs = requests(&server, "/api/v1/auth/create-tenant").await;
    assert_eq!(reqs.len(), 2);
    let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).expect("json");
    assert!(body.get("entityType").is_none());
}

#[tokio::test]
async fn logout_sends_the_cookie_and_ends_the_session() {
    let server = engine().await;
    mount_login(&server, &jwt("a", 3600), 'A').await;
    Mock::given(path("/api/v1/auth/logout"))
        .and(header("cookie", cookie_header('A').as_str()))
        .respond_with(json(200, r#"{"message":"Logged out"}"#).append_header(
            "set-cookie",
            "kb_refresh=; HttpOnly; Path=/api/v1/auth; Max-Age=0",
        ))
        .mount(&server)
        .await;
    let s = login(&client(&server)).await;
    let other = s.clone();
    s.logout().await.expect("logout");
    let before = server.received_requests().await.expect("r").len();
    let e = other.companies().await.expect_err("ended");
    assert_eq!(e, ApiError::Unauthorized(AuthFailure::SessionExpired));
    assert_eq!(server.received_requests().await.expect("r").len(), before);
    assert_eq!(requests(&server, "/api/v1/auth/logout").await.len(), 1);
}

#[tokio::test]
async fn health_and_desktop_info() {
    let server = engine().await;
    Mock::given(path("/health"))
        .respond_with(json(
            200,
            r#"{"status":"ok","online":null,"lastOnlineCheck":null}"#,
        ))
        .mount(&server)
        .await;
    let c = client(&server);
    let h = c.health().await.expect("health");
    assert_eq!((h.status.as_deref(), h.online), (Some("ok"), None));
    let info = c.desktop_info().await.expect("info");
    assert_eq!(info.workspace_tenant_id, Some(tid(WS)));
}

/// Shared log buffer for the canary test.
#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("lock").extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn canary_secrets_never_reach_logs_or_errors() {
    // A global subscriber (a scoped one can miss events whose callsite
    // interest was cached by tests running in parallel); it captures the
    // logs of every test in this binary, which only widens the check.
    static LOGS: std::sync::OnceLock<Buf> = std::sync::OnceLock::new();
    let buf = LOGS
        .get_or_init(|| {
            let buf = Buf::default();
            let writer = buf.clone();
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_writer(move || writer.clone())
                .finish();
            tracing::subscriber::set_global_default(subscriber).expect("one global subscriber");
            buf
        })
        .clone();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let shown = {
        rt.block_on(async {
            let server = engine().await;
            mount_login(&server, &jwt("canary", 30), 'Z').await;
            Mock::given(path("/api/v1/auth/refresh"))
                .respond_with(tokens(&jwt("canary2", 3600), 'Y'))
                .mount(&server)
                .await;
            Mock::given(path("/api/v1/auth/me"))
                .respond_with(json(
                    400,
                    r#"{"error":{"message":"Canary Co memo 1234.5600 failed","code":"VALIDATION_ERROR",
                        "details":[{"path":["memo"],"message":"Canary Co memo 1234.5600"}]}}"#,
                ))
                .mount(&server)
                .await;
            let c = client(&server);
            let s = login(&c).await;
            let e = s.companies().await.expect_err("validation");
            format!("{e} {e:?} {s:?} {c:?}")
        })
    };
    let logs = String::from_utf8(buf.0.lock().expect("lock").clone()).expect("utf8");
    assert!(logs.contains("engine attempt"), "tracing captured nothing");
    let all = format!("{logs}\n{shown}");
    for canary in [
        LAUNCH,
        PASSWORD,
        EMAIL,
        "canary2",
        "sig-canary",
        &refresh_value('Z'),
        &refresh_value('Y'),
        "Canary Co",
        "1234.56",
    ] {
        assert!(!all.contains(canary), "{canary} leaked:\n{all}");
    }
}
