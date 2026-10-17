//! Loopback transport: one HTTP attempt against the engine (P05 plan §2.2).
//!
//! - The host is the constant `127.0.0.1`; there is no proxy, no TLS, no
//!   redirect following, no cookie jar and no HTTP/2.
//! - Every attempt re-reads the endpoint (port, launch token, generation)
//!   from the [`EndpointSource`]; nothing is cached.
//! - Two connection pools (B1): reads reuse idle sockets for at most 2 s
//!   (the engine's Node listener closes idle sockets at 5 s), and every
//!   mutation opens a fresh connection, so a mutation never lands on a socket
//!   the server is closing. Any failure after a mutation's bytes may have
//!   been written is therefore a genuine ambiguity.
//! - Bodies are read in chunks with a size cap.
//! - Logs carry the method, route template, request id, attempt, status and
//!   generation only: never bodies, tokens, cookies or engine messages.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use cl_core::Secret;
use cl_core::ids::{CompanyId, RequestId};
use reqwest::header::{self, HeaderMap, HeaderName, HeaderValue};
use tokio::time::Instant;
use zeroize::Zeroizing;

use crate::endpoint::{EndpointSource, EndpointState, EngineEndpoint, Purpose};
use crate::idempotency::IdempotencyKey;

/// The loopback host. A constant, so this client cannot reach the network.
pub const LOOPBACK_HOST: &str = "127.0.0.1";

const USER_AGENT: &str = concat!("ClientLedger/", env!("CARGO_PKG_VERSION"));

/// Per-attempt and overall time limits of one request class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Limit for one HTTP attempt.
    pub attempt: Duration,
    /// Limit for the whole logical request, including waiting for the engine
    /// to become ready and any automatic retries.
    pub overall: Duration,
}

impl Budget {
    const fn secs(attempt: u64, overall: u64) -> Budget {
        Budget {
            attempt: Duration::from_secs(attempt),
            overall: Duration::from_secs(overall),
        }
    }
}

/// Time limits per request class (P05 plan §2.2 "Timeouts").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// `/health` and `/api/v1/desktop/info`.
    pub probe: Budget,
    /// Reads (GET).
    pub read: Budget,
    /// Mutations. Waiting for the engine happens only before any byte is sent.
    pub write: Budget,
    /// Login, refresh, switch-tenant and TFA verification.
    pub auth: Budget,
    /// Company creation (seeds a chart of accounts).
    pub create_tenant: Budget,
    /// Budget of outcome reconciliation after an ambiguous mutation.
    pub reconcile: Duration,
    /// How often a waiting request re-reads the endpoint.
    pub poll: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            probe: Budget::secs(3, 5),
            read: Budget::secs(15, 30),
            write: Budget::secs(30, 50),
            auth: Budget::secs(15, 20),
            create_tenant: Budget::secs(120, 140),
            reconcile: Duration::from_secs(15),
            poll: Duration::from_millis(150),
        }
    }
}

/// Client settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientConfig {
    /// Time limits.
    pub timeouts: Timeouts,
    /// Largest accepted response body in bytes.
    pub max_body_bytes: usize,
    /// Idle time after which a pooled read connection is dropped. Must stay
    /// well under the engine's 5 s keep-alive (B1).
    pub read_pool_idle: Duration,
    /// Limit for opening a connection. Kept below every per-attempt limit so
    /// a refused or unanswered connect is reported as "not sent" (on Windows
    /// a refused loopback connect takes about 2 s of SYN retries).
    pub connect_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            timeouts: Timeouts::default(),
            max_body_bytes: 32 * 1024 * 1024,
            read_pool_idle: Duration::from_secs(2),
            connect_timeout: Duration::from_secs(2),
        }
    }
}

/// HTTP method of an engine operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    /// Read.
    Get,
    /// Create or action.
    Post,
    /// Update.
    Put,
    /// Delete or deactivate.
    Delete,
}

impl Method {
    /// True for GET (safe to repeat).
    #[must_use]
    pub const fn is_read(self) -> bool {
        matches!(self, Method::Get)
    }

    /// Upper-case name as used in `operations.txt`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
        }
    }

    fn to_reqwest(self) -> reqwest::Method {
        match self {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
            Method::Put => reqwest::Method::PUT,
            Method::Delete => reqwest::Method::DELETE,
        }
    }
}

/// Which [`Budget`] applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TimeoutClass {
    Probe,
    Read,
    Write,
    Auth,
    CreateTenant,
}

impl Timeouts {
    pub(crate) fn budget(&self, class: TimeoutClass) -> Budget {
        match class {
            TimeoutClass::Probe => self.probe,
            TimeoutClass::Read => self.read,
            TimeoutClass::Write => self.write,
            TimeoutClass::Auth => self.auth,
            TimeoutClass::CreateTenant => self.create_tenant,
        }
    }
}

/// One logical request, fixed across attempts. Its `Debug` shows only the
/// method, route template, request id and body length: never the query
/// (search text is owner data) or the body.
pub(crate) struct Call {
    pub method: Method,
    /// Route template for logs (`/api/v1/accounts/{id}`); never the query.
    pub route: &'static str,
    /// Concrete path (`/api/v1/accounts/4f…`).
    pub path: String,
    pub query: Vec<(String, String)>,
    /// JSON body. Zeroized on drop: login and TFA bodies carry secrets.
    pub body: Option<Zeroizing<Vec<u8>>>,
    pub timeout: TimeoutClass,
    pub request_id: RequestId,
    pub company: Option<CompanyId>,
    pub idempotency_key: Option<IdempotencyKey>,
    pub if_match: Option<String>,
    /// The body carries a password or similar secret (set by `json_body`):
    /// the endpoint must be verified afresh for this call.
    pub sensitive: bool,
}

impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Call")
            .field("method", &self.method)
            .field("route", &self.route)
            .field("request_id", &self.request_id)
            .field("body_len", &self.body.as_ref().map_or(0, |b| b.len()))
            .finish_non_exhaustive()
    }
}

impl Call {
    pub(crate) fn new(method: Method, route: &'static str, path: impl Into<String>) -> Call {
        Call {
            method,
            route,
            path: path.into(),
            query: Vec::new(),
            body: None,
            timeout: if method.is_read() {
                TimeoutClass::Read
            } else {
                TimeoutClass::Write
            },
            request_id: RequestId::from_uuid(uuid::Uuid::new_v4()),
            company: None,
            idempotency_key: None,
            if_match: None,
            sensitive: false,
        }
    }

    /// The route template doubles as the path for parameterless routes.
    pub(crate) fn at(method: Method, route: &'static str) -> Call {
        Call::new(method, route, route)
    }

    pub(crate) fn json_body(mut self, bytes: Vec<u8>) -> Call {
        self.sensitive = body_has_secret_key(&bytes);
        self.body = Some(Zeroizing::new(bytes));
        self
    }

    pub(crate) fn timeout(mut self, class: TimeoutClass) -> Call {
        self.timeout = class;
        self
    }
}

/// JSON keys that carry a secret (a sign-in, closing-date or SMTP password, a
/// code, a refresh token). Every body goes through [`Call::json_body`], so any
/// request that puts a secret under one of these keys is treated as
/// credential-carrying whichever resource builds it (P07 review L2).
const SECRET_KEYS: [&str; 5] = [
    "password",
    "closingPassword",
    "code",
    "refreshToken",
    "token",
];

fn body_has_secret_key(body: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(body) else {
        return false;
    };
    SECRET_KEYS.iter().any(|k| {
        let needle = format!("\"{k}\"");
        text.match_indices(&needle).any(|(i, _)| {
            // A key is followed by a colon (a value that merely equals the
            // word is not).
            text[i + needle.len()..].trim_start().starts_with(':')
        })
    })
}

/// Credentials of one attempt (they can change between attempts after a
/// token refresh).
#[derive(Clone, Default)]
pub(crate) struct AttemptAuth {
    /// `Authorization: Bearer …`.
    pub bearer: Option<Secret>,
    /// `Cookie: kb_refresh=…` (only on refresh, switch-tenant and logout).
    pub refresh_cookie: Option<Secret>,
}

/// The engine's answer to one attempt (any status).
pub(crate) struct EngineReply {
    pub status: u16,
    /// Zeroized on drop: auth responses carry tokens.
    pub body: Zeroizing<Vec<u8>>,
    /// Raw `Set-Cookie` values (they carry the refresh token).
    pub set_cookies: Vec<Secret>,
    /// From `Retry-After` or `RateLimit-Reset` (whole seconds).
    pub retry_after: Option<Duration>,
    /// `Idempotent-Replayed: true` (U-18).
    #[allow(
        dead_code,
        reason = "read by the keyed-once write path once U-18 lands"
    )]
    pub replayed: bool,
    /// Generation of the endpoint this attempt used.
    pub generation: u64,
}

impl std::fmt::Debug for EngineReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineReply")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// Why an attempt produced no HTTP answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransportError {
    /// The engine did not become ready before the deadline, or is down.
    NotReady { permanent: bool },
    /// The connection could not be opened: nothing reached the engine.
    NotSent,
    /// The attempt timed out after the request may have been written.
    Timeout { generation: u64 },
    /// The connection failed after the request may have been written.
    Ambiguous { generation: u64 },
    /// The response body exceeded the cap.
    TooLarge,
    /// A header value could not be built (a malformed token).
    BadHeader,
}

/// One-attempt HTTP transport over loopback.
pub(crate) struct Transport {
    source: Arc<dyn EndpointSource>,
    read: reqwest::Client,
    write: reqwest::Client,
    cfg: ClientConfig,
    last_generation: AtomicU64,
    /// Test-only request observer (feature `test-support`).
    #[cfg(feature = "test-support")]
    pub(crate) observer: crate::testing::ObserverSlot,
}

impl Transport {
    pub(crate) fn new(
        source: Arc<dyn EndpointSource>,
        cfg: ClientConfig,
    ) -> Result<Transport, reqwest::Error> {
        let base = || {
            reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .http1_only()
                .connect_timeout(cfg.connect_timeout)
                .tcp_nodelay(true)
                .user_agent(USER_AGENT)
        };
        let read = base()
            .pool_idle_timeout(cfg.read_pool_idle)
            .pool_max_idle_per_host(8)
            .build()?;
        let write = base().pool_max_idle_per_host(0).build()?;
        Ok(Transport {
            source,
            read,
            write,
            cfg,
            last_generation: AtomicU64::new(0),
            #[cfg(feature = "test-support")]
            observer: crate::testing::ObserverSlot::default(),
        })
    }

    pub(crate) fn config(&self) -> &ClientConfig {
        &self.cfg
    }

    /// The current endpoint state (never cached).
    pub(crate) fn current(&self) -> EndpointState {
        self.source.current()
    }

    /// Waits until the endpoint is ready, polling until `deadline`.
    pub(crate) async fn wait_ready(
        &self,
        deadline: Instant,
        purpose: Purpose,
    ) -> Result<EngineEndpoint, TransportError> {
        loop {
            match self.source.current_for(purpose) {
                EndpointState::Ready(ep) => {
                    let prev = self.last_generation.swap(ep.generation, Ordering::Relaxed);
                    if prev != 0 && prev != ep.generation {
                        tracing::info!(generation = ep.generation, "engine restarted");
                    }
                    return Ok(ep);
                }
                EndpointState::Down { permanent: true } => {
                    return Err(TransportError::NotReady { permanent: true });
                }
                EndpointState::Starting
                | EndpointState::Restarting
                | EndpointState::Down { permanent: false } => {}
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(TransportError::NotReady { permanent: false });
            }
            tokio::time::sleep(self.cfg.timeouts.poll.min(deadline - now)).await;
        }
    }

    /// One attempt. Waits for a ready endpoint (bounded by `deadline`), then
    /// sends once. For mutations the deadline bounds only the waiting; the
    /// attempt itself always gets its full per-attempt limit.
    pub(crate) async fn attempt(
        &self,
        call: &Call,
        auth: &AttemptAuth,
        attempt_no: u32,
        deadline: Instant,
    ) -> Result<EngineReply, TransportError> {
        // Auth calls carry a password, code or refresh token: the source must
        // verify the engine afresh for them (Q-P06o).
        let purpose = if call.timeout == TimeoutClass::Auth || call.sensitive {
            Purpose::Credentials
        } else {
            Purpose::Ordinary
        };
        let ep = self.wait_ready(deadline, purpose).await?;
        let budget = self.cfg.timeouts.budget(call.timeout);
        let per_attempt = if call.method.is_read() {
            budget
                .attempt
                .min(deadline.saturating_duration_since(Instant::now()))
        } else {
            budget.attempt
        };
        if per_attempt.is_zero() {
            return Err(TransportError::NotReady { permanent: false });
        }
        let url = build_url(ep.port, &call.path, &call.query)?;
        let headers = build_headers(&ep, call, auth)?;
        let client = if call.method.is_read() {
            &self.read
        } else {
            &self.write
        };
        let mut req = client
            .request(call.method.to_reqwest(), url)
            .headers(headers)
            .timeout(per_attempt);
        if let Some(body) = &call.body {
            req = req.body(body.to_vec());
        }
        let started = Instant::now();
        #[cfg(feature = "test-support")]
        self.observer.before_send(call.method, call.route);
        let result = self.send(req, ep.generation).await;
        #[cfg(feature = "test-support")]
        let result = self
            .observer
            .after_reply(call.method, call.route, result, ep.generation);
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match &result {
            Ok((status, ..)) => tracing::debug!(
                method = call.method.as_str(),
                route = call.route,
                request_id = %call.request_id,
                attempt = attempt_no,
                status = *status,
                elapsed_ms,
                generation = ep.generation,
                "engine attempt"
            ),
            Err(e) => tracing::debug!(
                method = call.method.as_str(),
                route = call.route,
                request_id = %call.request_id,
                attempt = attempt_no,
                failure = ?e,
                elapsed_ms,
                generation = ep.generation,
                "engine attempt failed"
            ),
        }
        let (status, headers, body) = result?;
        Ok(EngineReply {
            status,
            set_cookies: headers
                .get_all(header::SET_COOKIE)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .map(|v| Secret::new(v.to_string()))
                .collect(),
            retry_after: retry_after(&headers),
            replayed: headers
                .get("idempotent-replayed")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.eq_ignore_ascii_case("true")),
            body,
            generation: ep.generation,
        })
    }

    async fn send(
        &self,
        req: reqwest::RequestBuilder,
        generation: u64,
    ) -> Result<(u16, HeaderMap, Zeroizing<Vec<u8>>), TransportError> {
        let mut resp = req.send().await.map_err(|e| classify(&e, generation))?;
        let status = resp.status().as_u16();
        let headers = std::mem::take(resp.headers_mut());
        let cap = self.cfg.max_body_bytes;
        let mut body = Zeroizing::new(Vec::new());
        while let Some(chunk) = resp.chunk().await.map_err(|e| classify(&e, generation))? {
            if body.len().saturating_add(chunk.len()) > cap {
                return Err(TransportError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok((status, headers, body))
    }
}

/// Connect failures are the only errors that prove nothing was sent.
fn classify(e: &reqwest::Error, generation: u64) -> TransportError {
    if e.is_connect() {
        TransportError::NotSent
    } else if e.is_timeout() {
        TransportError::Timeout { generation }
    } else {
        TransportError::Ambiguous { generation }
    }
}

fn build_url(
    port: u16,
    path: &str,
    query: &[(String, String)],
) -> Result<reqwest::Url, TransportError> {
    let mut url = reqwest::Url::parse(&format!("http://{LOOPBACK_HOST}:{port}"))
        .map_err(|_| TransportError::BadHeader)?;
    url.set_path(path);
    if !query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (k, v) in query {
            pairs.append_pair(k, v);
        }
    }
    Ok(url)
}

fn sensitive(value: &str) -> Result<HeaderValue, TransportError> {
    let mut v = HeaderValue::from_str(value).map_err(|_| TransportError::BadHeader)?;
    v.set_sensitive(true);
    Ok(v)
}

fn plain(value: &str) -> Result<HeaderValue, TransportError> {
    HeaderValue::from_str(value).map_err(|_| TransportError::BadHeader)
}

fn build_headers(
    ep: &EngineEndpoint,
    call: &Call,
    auth: &AttemptAuth,
) -> Result<HeaderMap, TransportError> {
    let mut h = HeaderMap::new();
    h.insert(
        HeaderName::from_static("x-cl-token"),
        sensitive(ep.token.expose())?,
    );
    h.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
    h.insert(
        HeaderName::from_static("x-request-id"),
        plain(&call.request_id.to_string())?,
    );
    if call.body.is_some() {
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    if let Some(bearer) = &auth.bearer {
        h.insert(
            header::AUTHORIZATION,
            sensitive(&format!("Bearer {}", bearer.expose()))?,
        );
    }
    if let Some(cookie) = &auth.refresh_cookie {
        h.insert(
            header::COOKIE,
            sensitive(&format!("kb_refresh={}", cookie.expose()))?,
        );
    }
    if let Some(company) = &call.company {
        h.insert(
            HeaderName::from_static("x-company-id"),
            plain(&company.to_string())?,
        );
    }
    if let Some(key) = &call.idempotency_key {
        h.insert(
            HeaderName::from_static("idempotency-key"),
            plain(&key.as_header())?,
        );
    }
    if let Some(tag) = &call.if_match {
        h.insert(header::IF_MATCH, plain(tag)?);
    }
    Ok(h)
}

/// The longest wait a `Retry-After` can ask for. Larger values (up to
/// `u64::MAX` from a hostile or broken peer) are clamped here, so no later
/// `Instant + Duration` can overflow.
pub(crate) const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    ["retry-after", "ratelimit-reset"].iter().find_map(|name| {
        headers
            .get(*name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(|secs| Duration::from_secs(secs).min(MAX_RETRY_AFTER))
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::endpoint::FixedEndpoint;
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    pub(crate) const TOKEN: &str = "launch-token-canary-0123456789";

    #[test]
    fn call_debug_shows_no_query_or_body() {
        let mut call = Call::new(
            Method::Post,
            "/api/v1/tags/{id}",
            "/api/v1/tags/x-path-canary",
        )
        .json_body(br#"{"name":"body-canary"}"#.to_vec());
        call.query.push(("search".into(), "query-canary".into()));
        let d = format!("{call:?}");
        assert!(d.contains("/api/v1/tags/{id}"), "{d}");
        assert!(d.contains("body_len: 22"), "{d}");
        for canary in ["body-canary", "query-canary", "x-path-canary"] {
            assert!(!d.contains(canary), "{d}");
        }
    }

    pub(crate) fn fast_config() -> ClientConfig {
        let mut cfg = ClientConfig::default();
        let t = &mut cfg.timeouts;
        t.read = Budget {
            attempt: Duration::from_secs(2),
            overall: Duration::from_secs(4),
        };
        t.write = t.read;
        t.auth = t.read;
        t.probe = t.read;
        t.poll = Duration::from_millis(20);
        t.reconcile = Duration::from_secs(1);
        cfg.connect_timeout = Duration::from_millis(500);
        cfg
    }

    fn transport_to(port: u16) -> (Arc<FixedEndpoint>, Transport) {
        let src = Arc::new(FixedEndpoint::new(port, Secret::new(TOKEN), 1));
        let t = Transport::new(src.clone(), fast_config()).expect("client builds");
        (src, t)
    }

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(3)
    }

    #[tokio::test]
    async fn every_request_carries_the_launch_token_and_no_origin() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        let (_src, t) = transport_to(server.address().port());
        let call = Call::at(Method::Get, "/health");
        let reply = t
            .attempt(&call, &AttemptAuth::default(), 1, soon())
            .await
            .expect("answer");
        assert_eq!(reply.status, 200);
        let reqs = server.received_requests().await.expect("recorded");
        let h = &reqs[0].headers;
        assert_eq!(h.get("x-cl-token").expect("token"), TOKEN);
        assert_eq!(h.get("accept").expect("accept"), "application/json");
        assert_eq!(
            h.get("x-request-id").expect("id").to_str().ok(),
            Some(call.request_id.to_string().as_str())
        );
        assert!(h.get("origin").is_none());
        assert!(h.get("authorization").is_none());
        assert!(h.get("cookie").is_none());
        assert!(h.get("x-company-id").is_none());
        let host = h.get("host").expect("host").to_str().expect("ascii");
        assert_eq!(host, format!("127.0.0.1:{}", server.address().port()));
    }

    /// Records the purpose of each endpoint read.
    struct PurposeSource {
        inner: FixedEndpoint,
        seen: std::sync::Mutex<Vec<Purpose>>,
    }

    impl EndpointSource for PurposeSource {
        fn current(&self) -> EndpointState {
            self.inner.current()
        }
        fn current_for(&self, purpose: Purpose) -> EndpointState {
            self.seen.lock().expect("lock").push(purpose);
            self.inner.current()
        }
    }

    #[tokio::test]
    async fn auth_calls_ask_for_a_credentials_grade_endpoint() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        let src = Arc::new(PurposeSource {
            inner: FixedEndpoint::new(server.address().port(), Secret::new(TOKEN), 1),
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let t = Transport::new(src.clone(), fast_config()).expect("client builds");
        let ordinary = Call::at(Method::Get, "/health");
        let auth = Call::at(Method::Post, "/login").timeout(TimeoutClass::Auth);
        t.attempt(&ordinary, &AttemptAuth::default(), 1, soon())
            .await
            .expect("answer");
        t.attempt(&auth, &AttemptAuth::default(), 1, soon())
            .await
            .expect("answer");
        let seen = src.seen.lock().expect("lock").clone();
        assert_eq!(seen, vec![Purpose::Ordinary, Purpose::Credentials]);
    }

    #[test]
    fn bodies_with_a_secret_key_are_sensitive() {
        for (body, want) in [
            (r#"{"closingPassword":"x","memo":"a"}"#, true),
            (r#"{"host":"h","password": "pw"}"#, true),
            (r#"{"memo":"password"}"#, false),
            (r#"{"memo":"my \"code\" word"}"#, false),
            (r#"{"amount":"12.00"}"#, false),
        ] {
            let call = Call::at(Method::Post, "/x").json_body(body.as_bytes().to_vec());
            assert_eq!(call.sensitive, want, "{body}");
        }
    }

    #[tokio::test]
    async fn a_write_with_a_closing_password_asks_for_a_credentials_grade_endpoint() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        let src = Arc::new(PurposeSource {
            inner: FixedEndpoint::new(server.address().port(), Secret::new(TOKEN), 1),
            seen: std::sync::Mutex::new(Vec::new()),
        });
        let t = Transport::new(src.clone(), fast_config()).expect("client builds");
        let call = Call::at(Method::Post, "/txn")
            .json_body(br#"{"closingPassword":"pw","memo":"m"}"#.to_vec());
        t.attempt(&call, &AttemptAuth::default(), 1, soon())
            .await
            .expect("answer");
        assert_eq!(*src.seen.lock().expect("lock"), vec![Purpose::Credentials]);
    }

    #[tokio::test]
    async fn optional_headers_are_sent_when_set() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201).set_body_string("{}"))
            .mount(&server)
            .await;
        let (_src, t) = transport_to(server.address().port());
        let company = CompanyId::parse("6f1c1e0e-1d7f-4c55-9e3e-1b2f7f3f9a10").expect("id");
        let mut call =
            Call::new(Method::Post, "/api/v1/x/{id}", "/api/v1/x/1").json_body(b"{}".to_vec());
        call.company = Some(company);
        let key = IdempotencyKey::new();
        call.idempotency_key = Some(key.clone());
        call.if_match = Some("\"7\"".into());
        call.query = vec![("search".into(), "a b&c".into())];
        let auth = AttemptAuth {
            bearer: Some(Secret::new("access-abc")),
            refresh_cookie: Some(Secret::new("refresh-def")),
        };
        t.attempt(&call, &auth, 1, soon()).await.expect("answer");
        let reqs = server.received_requests().await.expect("recorded");
        let r = &reqs[0];
        assert_eq!(r.url.query(), Some("search=a+b%26c"));
        let h = &r.headers;
        assert_eq!(h.get("authorization").expect("auth"), "Bearer access-abc");
        assert_eq!(h.get("cookie").expect("cookie"), "kb_refresh=refresh-def");
        assert_eq!(
            h.get("x-company-id").expect("company"),
            "6f1c1e0e-1d7f-4c55-9e3e-1b2f7f3f9a10"
        );
        assert_eq!(
            h.get("idempotency-key").expect("key"),
            key.as_header().as_str()
        );
        assert_eq!(h.get("if-match").expect("if-match"), "\"7\"");
        assert_eq!(h.get("content-type").expect("ct"), "application/json");
    }

    #[tokio::test]
    async fn endpoint_is_reread_for_every_attempt() {
        let a = MockServer::start().await;
        let b = MockServer::start().await;
        for s in [&a, &b] {
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200))
                .mount(s)
                .await;
        }
        let (src, t) = transport_to(a.address().port());
        let call = Call::at(Method::Get, "/health");
        let r1 = t
            .attempt(&call, &AttemptAuth::default(), 1, soon())
            .await
            .expect("a");
        src.set(EndpointState::Ready(EngineEndpoint {
            port: b.address().port(),
            token: Secret::new("second-token"),
            generation: 2,
        }));
        let r2 = t
            .attempt(&call, &AttemptAuth::default(), 2, soon())
            .await
            .expect("b");
        assert_eq!((r1.generation, r2.generation), (1, 2));
        assert_eq!(a.received_requests().await.expect("a").len(), 1);
        let breqs = b.received_requests().await.expect("b");
        assert_eq!(breqs.len(), 1);
        assert_eq!(
            breqs[0].headers.get("x-cl-token").expect("t"),
            "second-token"
        );
    }

    #[tokio::test]
    async fn waits_while_restarting_then_proceeds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let src = Arc::new(FixedEndpoint::with_state(EndpointState::Restarting));
        let t = Transport::new(src.clone(), fast_config()).expect("client");
        let port = server.address().port();
        let setter = src.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            setter.set(EndpointState::Ready(EngineEndpoint {
                port,
                token: Secret::new(TOKEN),
                generation: 5,
            }));
        });
        let reply = t
            .attempt(
                &Call::at(Method::Get, "/health"),
                &AttemptAuth::default(),
                1,
                soon(),
            )
            .await
            .expect("ready later");
        assert_eq!(reply.generation, 5);
    }

    #[tokio::test]
    async fn down_and_starting_end_without_a_request() {
        let src = Arc::new(FixedEndpoint::with_state(EndpointState::Down {
            permanent: true,
        }));
        let t = Transport::new(src.clone(), fast_config()).expect("client");
        let call = Call::at(Method::Get, "/health");
        let started = Instant::now();
        let e = t
            .attempt(&call, &AttemptAuth::default(), 1, soon())
            .await
            .expect_err("down");
        assert_eq!(e, TransportError::NotReady { permanent: true });
        assert!(started.elapsed() < Duration::from_millis(500));
        src.set(EndpointState::Starting);
        let e = t
            .attempt(
                &call,
                &AttemptAuth::default(),
                1,
                Instant::now() + Duration::from_millis(100),
            )
            .await
            .expect_err("never ready");
        assert_eq!(e, TransportError::NotReady { permanent: false });
    }

    pub(crate) fn dead_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        l.local_addr().expect("addr").port()
    }

    #[tokio::test]
    async fn connection_refused_is_not_sent() {
        let (_src, t) = transport_to(dead_port());
        let call = Call::at(Method::Post, "/api/v1/x").json_body(b"{}".to_vec());
        let e = t
            .attempt(&call, &AttemptAuth::default(), 1, soon())
            .await
            .expect_err("refused");
        assert_eq!(e, TransportError::NotSent);
    }

    #[tokio::test]
    async fn slow_answer_is_a_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
            .mount(&server)
            .await;
        let src = Arc::new(FixedEndpoint::new(
            server.address().port(),
            Secret::new(TOKEN),
            1,
        ));
        let mut cfg = fast_config();
        cfg.timeouts.write.attempt = Duration::from_millis(300);
        let t = Transport::new(src, cfg).expect("client");
        let call = Call::at(Method::Post, "/api/v1/x").json_body(b"{}".to_vec());
        let e = t
            .attempt(&call, &AttemptAuth::default(), 1, soon())
            .await
            .expect_err("timeout");
        assert_eq!(e, TransportError::Timeout { generation: 1 });
    }

    #[tokio::test]
    async fn oversized_body_is_refused() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 4096]))
            .mount(&server)
            .await;
        let src = Arc::new(FixedEndpoint::new(
            server.address().port(),
            Secret::new(TOKEN),
            1,
        ));
        let mut cfg = fast_config();
        cfg.max_body_bytes = 1024;
        let t = Transport::new(src, cfg).expect("client");
        let e = t
            .attempt(
                &Call::at(Method::Get, "/x"),
                &AttemptAuth::default(),
                1,
                soon(),
            )
            .await
            .expect_err("too large");
        assert_eq!(e, TransportError::TooLarge);
    }

    #[tokio::test]
    async fn redirects_are_not_followed_and_reply_headers_are_read() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", "http://example.com/")
                    .insert_header("retry-after", "7")
                    .insert_header("idempotent-replayed", "true")
                    .append_header("set-cookie", "a=1; Path=/")
                    .append_header("set-cookie", "kb_refresh=abc; Path=/"),
            )
            .mount(&server)
            .await;
        let (_src, t) = transport_to(server.address().port());
        let r = t
            .attempt(
                &Call::at(Method::Get, "/x"),
                &AttemptAuth::default(),
                1,
                soon(),
            )
            .await
            .expect("reply");
        assert_eq!(r.status, 302);
        assert_eq!(r.retry_after, Some(Duration::from_secs(7)));
        assert!(r.replayed);
        let mut h = HeaderMap::new();
        h.insert(
            "retry-after",
            HeaderValue::from_static("18446744073709551615"),
        );
        assert_eq!(
            retry_after(&h),
            Some(MAX_RETRY_AFTER),
            "u64::MAX is clamped"
        );
        assert_eq!(r.set_cookies.len(), 2);
        assert_eq!(server.received_requests().await.expect("r").len(), 1);
        assert!(!format!("{r:?}").contains("kb_refresh"));
    }

    /// A raw HTTP/1.1 server that keeps connections alive but closes any
    /// connection idle for `idle` (like Node's keep-alive timeout).
    pub(crate) async fn idle_closing_server(idle: Duration) -> (u16, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let c = c.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    loop {
                        let mut chunk = [0u8; 4096];
                        let Ok(Ok(n)) = tokio::time::timeout(idle, sock.read(&mut chunk)).await
                        else {
                            return; // idle: close the socket
                        };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                            continue;
                        };
                        let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if buf.len() < end + 4 + len {
                            continue;
                        }
                        buf.drain(..end + 4 + len);
                        c.fetch_add(1, Ordering::SeqCst);
                        let resp = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: keep-alive\r\n\r\n{}";
                        if sock.write_all(resp).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        (port, count)
    }

    #[tokio::test]
    async fn mutation_after_idle_uses_a_fresh_connection() {
        let (port, count) = idle_closing_server(Duration::from_secs(1)).await;
        let (_src, t) = transport_to(port);
        let make_post = || Call::at(Method::Post, "/api/v1/x").json_body(b"{}".to_vec());
        t.attempt(&make_post(), &AttemptAuth::default(), 1, soon())
            .await
            .expect("first");
        // Inside the race window: the server closed the socket at 1 s.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        t.attempt(&make_post(), &AttemptAuth::default(), 1, soon())
            .await
            .expect("second is not ambiguous");
        tokio::time::sleep(Duration::from_secs(3)).await;
        t.attempt(&make_post(), &AttemptAuth::default(), 1, soon())
            .await
            .expect("third after 3 s idle");
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn defaults_follow_the_plan() {
        let cfg = ClientConfig::default();
        assert_eq!(cfg.max_body_bytes, 32 * 1024 * 1024);
        assert!(cfg.read_pool_idle <= Duration::from_secs(2));
        assert_eq!(cfg.timeouts.write.overall, Duration::from_secs(50));
        assert_eq!(cfg.timeouts.create_tenant.attempt.as_secs(), 120);
        assert_eq!(Method::Delete.as_str(), "DELETE");
        assert!(Method::Get.is_read() && !Method::Put.is_read());
    }
}
