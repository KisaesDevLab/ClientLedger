//! Idempotency keys and the safe-retry policy (P05 plan §2.6).
//!
//! The engine ignores `Idempotency-Key` until U-18, so sending it is harmless
//! and also no protection: a mutation is retried only when it provably never
//! reached a handler, or when outcome reconciliation proves the first attempt
//! did not commit.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cl_core::ids::RequestId;
use tokio::time::Instant;
use tracing::Instrument;

use crate::client::{AttemptAuth, Call, EngineReply, Transport, TransportError};
use crate::endpoint::EndpointState;
use crate::errors::{ApiError, AuthFailure, Mapped, RouteKind, map_failure};
use crate::resources::operations::{Operation, RetryClass};

/// Opaque key sent as `Idempotency-Key` on mutations.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(uuid::Uuid);

impl IdempotencyKey {
    /// A fresh random key (one per logical user action).
    #[must_use]
    pub fn new() -> IdempotencyKey {
        IdempotencyKey(uuid::Uuid::new_v4())
    }

    /// Header value (36-character lower-case UUID).
    #[must_use]
    pub fn as_header(&self) -> String {
        self.0.hyphenated().to_string()
    }
}

impl Default for IdempotencyKey {
    fn default() -> Self {
        IdempotencyKey::new()
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.hyphenated())
    }
}

impl fmt::Debug for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IdempotencyKey({})", self.0.hyphenated())
    }
}

// ---------------------------------------------------------------------------
// The executor: one logical request, its attempts and the retry policy.
// ---------------------------------------------------------------------------

/// A boxed, sendable future (object-safe hooks).
pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What outcome reconciliation found out about an ambiguous mutation
/// (P05 plan §2.6.1).
#[allow(
    dead_code,
    reason = "constructed by the per-operation reconcilers (P05 plan §4 step 20)"
)]
pub(crate) enum Reconciled<T> {
    /// The first attempt committed; this is its result.
    Committed(T),
    /// The first attempt provably did not commit; one same-key retry is safe.
    NotCommitted,
    /// It cannot be told; the owner must check (`OutcomeUnknown`).
    Unresolved,
}

/// Why a mutation's outcome is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AmbiguityCause {
    /// The attempt timed out after it may have been written.
    Timeout,
    /// The connection broke after the request may have been written.
    ConnectionLost,
    /// The engine answered 5xx after the handler may have run.
    ServerError(u16),
}

/// Facts about an ambiguous attempt, handed to the reconciler.
#[allow(
    dead_code,
    reason = "constructed by the per-operation reconcilers (P05 plan §4 step 20)"
)]
#[derive(Clone, Debug)]
pub(crate) struct Ambiguity {
    pub request_id: RequestId,
    pub idempotency_key: Option<IdempotencyKey>,
    /// Generation of the endpoint the attempt used.
    pub generation: u64,
    pub cause: AmbiguityCause,
}

impl Ambiguity {
    /// The old request can no longer commit: the engine process it reached
    /// is gone (`PostgreSQL` rolls back a transaction whose client vanished).
    /// A timeout or a broken connection on the same generation is not
    /// settled, because the engine sets no request or statement timeout.
    #[allow(
        dead_code,
        reason = "constructed by the per-operation reconcilers (P05 plan §4 step 20)"
    )]
    pub(crate) fn settled(&self, now: &EndpointState) -> bool {
        match now {
            EndpointState::Ready(ep) => ep.generation > self.generation,
            EndpointState::Down { .. } | EndpointState::Starting | EndpointState::Restarting => {
                false
            }
        }
    }
}

/// Resolves an ambiguous mutation (implemented per operation in step 20).
pub(crate) trait Reconciler: Send + Sync {
    fn reconcile<'a>(&'a self, ambiguity: &'a Ambiguity) -> BoxFuture<'a, Reconciled<EngineReply>>;
}

/// Credentials used for one attempt.
#[derive(Clone, Default)]
pub(crate) struct AttemptCreds {
    pub auth: AttemptAuth,
    /// Session auth epoch the bearer token belongs to.
    pub epoch: u64,
    /// The access token's `exp` (Unix seconds), when known.
    pub access_exp: Option<u64>,
}

impl AttemptCreds {
    /// Our own clock says the access token had expired (same machine as the
    /// engine; P33 revisits for LAN).
    pub(crate) fn expired_now(&self) -> bool {
        self.access_exp.is_some_and(|exp| exp <= unix_now())
    }
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Supplies credentials per attempt and handles the session refresh.
pub(crate) trait AuthHook: Send + Sync {
    /// Credentials for the next attempt (may refresh proactively). An error
    /// here means nothing was sent.
    fn prepare(&self) -> BoxFuture<'_, Result<AttemptCreds, ApiError>>;
    /// The engine's `authenticate` refused `creds` (401 `UNAUTHORIZED`).
    /// Refresh (single-flight); `Ok` means replay once.
    fn rejected<'a>(&'a self, creds: &'a AttemptCreds) -> BoxFuture<'a, Result<(), ApiError>>;
}

/// No user credentials (public routes) or fixed ones (TFA token, refresh
/// cookie). A 401 is never refreshed.
pub(crate) struct StaticAuth(pub AttemptAuth);

impl AuthHook for StaticAuth {
    fn prepare(&self) -> BoxFuture<'_, Result<AttemptCreds, ApiError>> {
        let creds = AttemptCreds {
            auth: self.0.clone(),
            ..AttemptCreds::default()
        };
        Box::pin(async move { Ok(creds) })
    }

    fn rejected<'a>(&'a self, _: &'a AttemptCreds) -> BoxFuture<'a, Result<(), ApiError>> {
        Box::pin(async { Err(ApiError::Unauthorized(AuthFailure::SessionExpired)) })
    }
}

/// Up to three attempts for transient failures of retryable requests.
const MAX_TRANSIENT_RETRIES: u32 = 2;

/// Runs logical requests through the transport with the §2.6 policy.
pub(crate) struct Executor {
    transport: Transport,
    idempotency_v1: AtomicBool,
}

/// Per-request bookkeeping of the retry loop.
#[derive(Default)]
struct Retries {
    transient: u32,
    gate_replayed: bool,
    auth_replayed: bool,
    same_key_retried: bool,
}

enum Step {
    Again,
    Done(Result<EngineReply, ApiError>),
}

/// What one attempt is about, for the failure handlers.
struct Ctx<'a> {
    op: &'a Operation,
    class: RetryClass,
    call: &'a Call,
    reconciler: Option<&'a dyn Reconciler>,
    deadline: Instant,
}

impl Executor {
    pub(crate) fn new(transport: Transport) -> Executor {
        Executor {
            transport,
            idempotency_v1: AtomicBool::new(false),
        }
    }

    #[allow(
        dead_code,
        reason = "constructed by the per-operation reconcilers (P05 plan §4 step 20)"
    )]
    pub(crate) fn transport(&self) -> &Transport {
        &self.transport
    }

    /// Record whether the engine advertises `idempotency-v1` (U-18).
    pub(crate) fn set_idempotency_v1(&self, on: bool) {
        self.idempotency_v1.store(on, Ordering::Relaxed);
    }

    /// Runs one logical request. Returns the 2xx answer or a typed error.
    pub(crate) async fn run(
        &self,
        op: &Operation,
        mut call: Call,
        auth: &dyn AuthHook,
        reconciler: Option<&dyn Reconciler>,
    ) -> Result<EngineReply, ApiError> {
        let class = op
            .retry()
            .effective(self.idempotency_v1.load(Ordering::Relaxed));
        if !call.method.is_read() && call.idempotency_key.is_none() {
            call.idempotency_key = Some(IdempotencyKey::new());
        }
        let span = tracing::debug_span!(
            "engine_request",
            method = call.method.as_str(),
            route = call.route,
            request_id = %call.request_id,
        );
        self.run_inner(op, class, &call, auth, reconciler)
            .instrument(span)
            .await
    }

    async fn run_inner(
        &self,
        op: &Operation,
        class: RetryClass,
        call: &Call,
        auth: &dyn AuthHook,
        reconciler: Option<&dyn Reconciler>,
    ) -> Result<EngineReply, ApiError> {
        let budget = self.transport.config().timeouts.budget(call.timeout);
        let mut ctx = Ctx {
            op,
            class,
            call,
            reconciler,
            deadline: Instant::now() + budget.overall,
        };
        let mut r = Retries::default();
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let creds = auth.prepare().await?;
            let outcome = self
                .transport
                .attempt(call, &creds.auth, attempt, ctx.deadline)
                .await;
            let step = match outcome {
                Ok(reply) if (200..300).contains(&reply.status) => Step::Done(Ok(reply)),
                Ok(reply) => self.on_status(&ctx, auth, &creds, reply, &mut r).await,
                Err(e) => self.on_transport(&ctx, e, &mut r).await,
            };
            match step {
                Step::Done(result) => return result,
                Step::Again => {
                    if r.same_key_retried && Instant::now() >= ctx.deadline {
                        // The single post-reconciliation retry gets a fresh
                        // window to wait for the engine.
                        ctx.deadline = Instant::now() + budget.attempt;
                    }
                }
            }
        }
    }

    async fn on_transport(&self, ctx: &Ctx<'_>, e: TransportError, r: &mut Retries) -> Step {
        let give_up = |e: TransportError| match e {
            TransportError::NotReady { permanent } => ApiError::EngineUnavailable { permanent },
            TransportError::Timeout { .. } => ApiError::Timeout,
            TransportError::NotSent | TransportError::Ambiguous { .. } => {
                ApiError::EngineUnavailable { permanent: false }
            }
            TransportError::TooLarge => ApiError::Decode {
                what: "response too large",
            },
            TransportError::BadHeader => ApiError::Decode {
                what: "request header",
            },
        };
        match e {
            TransportError::NotReady { .. }
            | TransportError::TooLarge
            | TransportError::BadHeader => Step::Done(Err(give_up(e))),
            // Nothing reached the engine: safe to retry for every class but
            // `Never` (rate-limited and rotating routes stay single-shot).
            TransportError::NotSent => {
                if ctx.class != RetryClass::Never && self.backoff(r, ctx.deadline).await {
                    Step::Again
                } else {
                    Step::Done(Err(give_up(e)))
                }
            }
            TransportError::Timeout { generation } | TransportError::Ambiguous { generation } => {
                match ctx.class {
                    RetryClass::Idempotent | RetryClass::KeyedOnce => {
                        if self.backoff(r, ctx.deadline).await {
                            Step::Again
                        } else {
                            Step::Done(Err(give_up(e)))
                        }
                    }
                    RetryClass::Never => Step::Done(Err(give_up(e))),
                    RetryClass::PreHandlerOnly => {
                        let cause = if matches!(e, TransportError::Timeout { .. }) {
                            AmbiguityCause::Timeout
                        } else {
                            AmbiguityCause::ConnectionLost
                        };
                        self.ambiguous(ctx, generation, cause, r).await
                    }
                }
            }
        }
    }

    async fn on_status(
        &self,
        ctx: &Ctx<'_>,
        auth: &dyn AuthHook,
        creds: &AttemptCreds,
        reply: EngineReply,
        r: &mut Retries,
    ) -> Step {
        match map_failure(reply.status, &reply.body, reply.retry_after, ctx.op.kind) {
            // The desktop gate runs before every handler: replay once on the
            // re-read endpoint.
            Mapped::DesktopGate => {
                if r.gate_replayed {
                    Step::Done(Err(ApiError::EngineUnavailable { permanent: false }))
                } else {
                    r.gate_replayed = true;
                    Step::Again
                }
            }
            // Not the engine's authenticate (B4): never refresh; replay only
            // when the endpoint has changed and is ready.
            Mapped::NotEngine => {
                let changed = matches!(
                    self.transport.current(),
                    EndpointState::Ready(ep) if ep.generation != reply.generation
                );
                if changed && !r.gate_replayed {
                    r.gate_replayed = true;
                    Step::Again
                } else {
                    Step::Done(Err(ApiError::EngineUnavailable { permanent: false }))
                }
            }
            Mapped::SessionRejected => {
                if r.auth_replayed {
                    return Step::Done(Err(ApiError::Unauthorized(AuthFailure::SessionExpired)));
                }
                let same_generation = matches!(
                    self.transport.current(),
                    EndpointState::Ready(ep) if ep.generation == reply.generation
                );
                if !same_generation {
                    // Re-read the endpoint first; never refresh against a
                    // generation we did not talk to.
                    r.auth_replayed = true;
                    return Step::Again;
                }
                if !(ctx.call.method.is_read() || creds.expired_now()) {
                    return Step::Done(Err(ApiError::Unauthorized(AuthFailure::SessionExpired)));
                }
                match auth.rejected(creds).await {
                    Ok(()) => {
                        r.auth_replayed = true;
                        Step::Again
                    }
                    Err(e) => Step::Done(Err(e)),
                }
            }
            Mapped::Final(e) => self.on_final(ctx, e, reply.generation, r).await,
        }
    }

    async fn on_final(&self, ctx: &Ctx<'_>, e: ApiError, generation: u64, r: &mut Retries) -> Step {
        let transient_status = matches!(
            e,
            ApiError::Server {
                status: 502..=504,
                ..
            }
        );
        let rate_limit_ok = matches!(ctx.op.kind, RouteKind::Session | RouteKind::Public);
        match (ctx.class, &e) {
            (RetryClass::Idempotent | RetryClass::KeyedOnce, _) if transient_status => {
                if self.backoff(r, ctx.deadline).await {
                    Step::Again
                } else {
                    Step::Done(Err(e))
                }
            }
            (
                RetryClass::Idempotent,
                ApiError::RateLimited {
                    retry_after: Some(wait),
                },
            ) if rate_limit_ok => {
                let wait = *wait;
                if r.transient < MAX_TRANSIENT_RETRIES && Instant::now() + wait < ctx.deadline {
                    r.transient += 1;
                    tokio::time::sleep(wait).await;
                    Step::Again
                } else {
                    Step::Done(Err(e))
                }
            }
            // A 5xx after a mutation's handler may have run is ambiguous.
            (RetryClass::PreHandlerOnly, ApiError::Server { status, .. }) => {
                let cause = AmbiguityCause::ServerError(*status);
                self.ambiguous(ctx, generation, cause, r).await
            }
            _ => Step::Done(Err(e)),
        }
    }

    /// Outcome reconciliation for an ambiguous mutation (B1). Only the
    /// outcome class is logged.
    async fn ambiguous(
        &self,
        ctx: &Ctx<'_>,
        generation: u64,
        cause: AmbiguityCause,
        r: &mut Retries,
    ) -> Step {
        let unknown = || ApiError::OutcomeUnknown {
            request_id: ctx.call.request_id,
            idempotency_key: ctx.call.idempotency_key.clone(),
        };
        let Some(rec) = ctx.reconciler else {
            tracing::info!(outcome = "unknown", reconciled = false, "mutation outcome");
            return Step::Done(Err(unknown()));
        };
        let amb = Ambiguity {
            request_id: ctx.call.request_id,
            idempotency_key: ctx.call.idempotency_key.clone(),
            generation,
            cause,
        };
        match rec.reconcile(&amb).await {
            Reconciled::Committed(reply) => {
                tracing::info!(outcome = "committed", reconciled = true, "mutation outcome");
                Step::Done(Ok(reply))
            }
            // One same-key retry; a second ambiguity is never retried again.
            Reconciled::NotCommitted if !r.same_key_retried => {
                tracing::info!(
                    outcome = "not_committed",
                    reconciled = true,
                    "mutation outcome"
                );
                r.same_key_retried = true;
                Step::Again
            }
            Reconciled::NotCommitted | Reconciled::Unresolved => {
                tracing::info!(outcome = "unknown", reconciled = true, "mutation outcome");
                Step::Done(Err(unknown()))
            }
        }
    }

    /// Full-jitter backoff `rand(0, min(2000, 200 * 2^n))` ms within the
    /// deadline. Returns false when no retry is left or time is up.
    async fn backoff(&self, r: &mut Retries, deadline: Instant) -> bool {
        if r.transient >= MAX_TRANSIENT_RETRIES {
            return false;
        }
        let cap_ms = 2000u64.min(200u64 << r.transient);
        r.transient += 1;
        let wait = Duration::from_millis(jitter(cap_ms));
        if Instant::now() + wait >= deadline {
            return false;
        }
        tokio::time::sleep(wait).await;
        true
    }
}

/// A random value in `0..=max` (uniform enough for jitter).
fn jitter(max: u64) -> u64 {
    let low = uuid::Uuid::new_v4().as_u128() & u128::from(u64::MAX);
    u64::try_from(low).unwrap_or(0) % (max + 1)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::client::tests::{TOKEN, dead_port, fast_config, idle_closing_server};
    use crate::client::{Method, Transport};
    use crate::endpoint::{EndpointSource, EngineEndpoint, FixedEndpoint};
    use crate::resources::operations::{AUTH_ME, LOGIN, find};
    use cl_core::Secret;
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Returns the states in order; the last one repeats.
    pub(crate) struct SeqEndpoint(Mutex<Vec<EndpointState>>);

    impl SeqEndpoint {
        pub(crate) fn new(states: Vec<EndpointState>) -> SeqEndpoint {
            SeqEndpoint(Mutex::new(states))
        }
    }

    impl EndpointSource for SeqEndpoint {
        fn current(&self) -> EndpointState {
            let mut v = self.0.lock().expect("lock");
            if v.len() > 1 {
                v.remove(0)
            } else {
                v[0].clone()
            }
        }
    }

    pub(crate) fn ready(port: u16, token: &str, generation: u64) -> EndpointState {
        EndpointState::Ready(EngineEndpoint {
            port,
            token: Secret::new(token),
            generation,
        })
    }

    pub(crate) fn exec_on(src: Arc<dyn EndpointSource>) -> Executor {
        Executor::new(Transport::new(src, fast_config()).expect("client"))
    }

    fn exec_at(port: u16) -> Executor {
        exec_on(Arc::new(FixedEndpoint::new(port, Secret::new(TOKEN), 1)))
    }

    fn post_op() -> Operation {
        *find(Method::Post, "/api/v1/accounts").expect("covered")
    }

    fn get_op() -> Operation {
        *find(Method::Get, "/api/v1/accounts").expect("covered")
    }

    fn post_call() -> Call {
        Call::at(Method::Post, "/api/v1/accounts").json_body(b"{}".to_vec())
    }

    fn get_call() -> Call {
        Call::at(Method::Get, "/api/v1/accounts")
    }

    fn none() -> StaticAuth {
        StaticAuth(AttemptAuth::default())
    }

    async fn count(server: &MockServer) -> usize {
        server.received_requests().await.expect("recorded").len()
    }

    fn json(status: u16, body: &str) -> ResponseTemplate {
        ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json")
    }

    /// Scripted reconciler: answers in order and counts calls.
    struct Script {
        answers: Mutex<Vec<&'static str>>,
        calls: AtomicUsize,
        settled_seen: Mutex<Vec<bool>>,
        src: Option<Arc<dyn EndpointSource>>,
    }

    impl Script {
        fn new(answers: Vec<&'static str>) -> Script {
            Script {
                answers: Mutex::new(answers),
                calls: AtomicUsize::new(0),
                settled_seen: Mutex::new(Vec::new()),
                src: None,
            }
        }
    }

    impl Reconciler for Script {
        fn reconcile<'a>(&'a self, amb: &'a Ambiguity) -> BoxFuture<'a, Reconciled<EngineReply>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(src) = &self.src {
                self.settled_seen
                    .lock()
                    .expect("lock")
                    .push(amb.settled(&src.current()));
            }
            let next = {
                let mut a = self.answers.lock().expect("lock");
                if a.is_empty() {
                    "unresolved"
                } else {
                    a.remove(0)
                }
            };
            Box::pin(async move {
                match next {
                    "committed" => Reconciled::Committed(EngineReply {
                        status: 201,
                        body: zeroize::Zeroizing::new(b"{\"found\":true}".to_vec()),
                        set_cookies: Vec::new(),
                        retry_after: None,
                        replayed: false,
                        generation: amb.generation,
                    }),
                    "not_committed" => Reconciled::NotCommitted,
                    _ => Reconciled::Unresolved,
                }
            })
        }
    }

    #[tokio::test]
    async fn reads_retry_transient_5xx_then_succeed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(json(200, "{}"))
            .mount(&server)
            .await;
        let ex = exec_at(server.address().port());
        let r = ex
            .run(&get_op(), get_call(), &none(), None)
            .await
            .expect("ok");
        assert_eq!(r.status, 200);
        assert_eq!(count(&server).await, 3);
    }

    #[tokio::test]
    async fn reads_give_up_after_three_attempts_and_500_is_not_retried() {
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/accounts"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&server)
            .await;
        Mock::given(path("/api/v1/auth/me"))
            .respond_with(json(
                500,
                r#"{"error":{"message":"x","code":"INTERNAL_ERROR"}}"#,
            ))
            .mount(&server)
            .await;
        let ex = exec_at(server.address().port());
        let e = ex
            .run(&get_op(), get_call(), &none(), None)
            .await
            .expect_err("502");
        assert!(matches!(e, ApiError::Server { status: 502, .. }));
        assert_eq!(count(&server).await, 3);
        let e = ex
            .run(
                &AUTH_ME,
                Call::at(Method::Get, "/api/v1/auth/me"),
                &none(),
                None,
            )
            .await
            .expect_err("500");
        assert!(matches!(e, ApiError::Server { status: 500, .. }));
        assert_eq!(count(&server).await, 4);
    }

    #[tokio::test]
    async fn offline_and_egress_are_never_retried() {
        let server = MockServer::start().await;
        Mock::given(path("/api/v1/accounts"))
            .respond_with(json(
                503,
                r#"{"error":{"message":"x","code":"OFFLINE_UNAVAILABLE","feature":"smtp"}}"#,
            ))
            .mount(&server)
            .await;
        let ex = exec_at(server.address().port());
        let started = Instant::now();
        let e = ex
            .run(&get_op(), get_call(), &none(), None)
            .await
            .expect_err("offline");
        assert_eq!(
            e,
            ApiError::OfflineUnavailable {
                feature: Some("smtp".into())
            }
        );
        let rec = Script::new(vec!["committed"]);
        let e = ex
            .run(&post_op(), post_call(), &none(), Some(&rec))
            .await
            .expect_err("offline");
        assert_eq!(
            e,
            ApiError::OfflineUnavailable {
                feature: Some("smtp".into())
            }
        );
        assert_eq!(rec.calls.load(Ordering::SeqCst), 0);
        assert_eq!(count(&server).await, 2);
        assert!(started.elapsed() < Duration::from_secs(2));

        let egress = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(json(
                503,
                r#"{"error":{"message":"x","code":"EGRESS_BLOCKED","feature":"ai"}}"#,
            ))
            .mount(&egress)
            .await;
        let ex = exec_at(egress.address().port());
        let e = ex
            .run(&get_op(), get_call(), &none(), None)
            .await
            .expect_err("egress");
        assert_eq!(
            e,
            ApiError::FeatureNotEnabled {
                feature: Some("ai".into())
            }
        );
        assert_eq!(count(&egress).await, 1);
    }

    #[tokio::test]
    async fn mutation_timeout_without_reconciler_is_outcome_unknown_and_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(json(201, "{}").set_delay(Duration::from_secs(3)))
            .mount(&server)
            .await;
        let src = Arc::new(FixedEndpoint::new(
            server.address().port(),
            Secret::new(TOKEN),
            1,
        ));
        let mut cfg = fast_config();
        cfg.timeouts.write.attempt = Duration::from_millis(300);
        let ex = Executor::new(Transport::new(src, cfg).expect("client"));
        let call = post_call();
        let rid = call.request_id;
        let e = ex
            .run(&post_op(), call, &none(), None)
            .await
            .expect_err("unknown");
        let ApiError::OutcomeUnknown {
            request_id,
            idempotency_key,
        } = e
        else {
            panic!("{e:?}")
        };
        assert_eq!(request_id, rid);
        let key = idempotency_key.expect("a key is always sent");
        let reqs = server.received_requests().await.expect("r");
        assert_eq!(reqs.len(), 1);
        assert_eq!(
            reqs[0].headers.get("idempotency-key").expect("key"),
            key.as_header().as_str()
        );
    }

    #[tokio::test]
    async fn refused_connect_is_retried_for_mutations_but_not_for_login() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(json(201, "{}"))
            .mount(&server)
            .await;
        let port = server.address().port();
        let src = Arc::new(SeqEndpoint::new(vec![
            ready(dead_port(), TOKEN, 1),
            ready(port, TOKEN, 2),
        ]));
        let ex = exec_on(src);
        let r = ex
            .run(&post_op(), post_call(), &none(), None)
            .await
            .expect("retried");
        assert_eq!(r.generation, 2);
        assert_eq!(count(&server).await, 1);

        let src = Arc::new(SeqEndpoint::new(vec![
            ready(dead_port(), TOKEN, 1),
            ready(port, TOKEN, 2),
        ]));
        let ex = exec_on(src);
        let login = Call::at(Method::Post, "/api/v1/auth/login").json_body(b"{}".to_vec());
        let e = ex
            .run(&LOGIN, login, &none(), None)
            .await
            .expect_err("never class");
        assert_eq!(e, ApiError::EngineUnavailable { permanent: false });
        assert_eq!(count(&server).await, 1);
    }

    #[tokio::test]
    async fn desktop_gate_refusal_replays_once_on_the_reread_endpoint() {
        let server = MockServer::start().await;
        Mock::given(header("x-cl-token", "new-token"))
            .respond_with(json(201, "{}"))
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(json(
                401,
                r#"{"error":{"message":"x","code":"DESKTOP_TOKEN_REQUIRED"}}"#,
            ))
            .mount(&server)
            .await;
        let port = server.address().port();
        let src = Arc::new(SeqEndpoint::new(vec![
            ready(port, "old-token", 1),
            ready(port, "new-token", 2),
        ]));
        let ex = exec_on(src);
        ex.run(&post_op(), post_call(), &none(), None)
            .await
            .expect("replayed");
        assert_eq!(count(&server).await, 2);

        // Still refused after the replay: the engine is not reachable.
        let ex = exec_at(port);
        let e = ex
            .run(&post_op(), post_call(), &none(), None)
            .await
            .expect_err("gate");
        assert_eq!(e, ApiError::EngineUnavailable { permanent: false });
        assert_eq!(count(&server).await, 4);
    }

    #[tokio::test]
    async fn misdirected_421_is_a_gate_refusal() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(json(421, r#"{"error":{"code":"HOST_NOT_ALLOWED"}}"#))
            .mount(&server)
            .await;
        let ex = exec_at(server.address().port());
        let e = ex
            .run(&get_op(), get_call(), &none(), None)
            .await
            .expect_err("421");
        assert_eq!(e, ApiError::EngineUnavailable { permanent: false });
        assert_eq!(count(&server).await, 2);
    }

    /// Records refresh attempts; `prepare` hands out a bearer and a fixed
    /// expiry.
    struct CountingAuth {
        refreshes: AtomicUsize,
        exp: Option<u64>,
    }

    impl AuthHook for CountingAuth {
        fn prepare(&self) -> BoxFuture<'_, Result<AttemptCreds, ApiError>> {
            let creds = AttemptCreds {
                auth: AttemptAuth {
                    bearer: Some(Secret::new("access")),
                    refresh_cookie: None,
                },
                epoch: self.refreshes.load(Ordering::SeqCst) as u64,
                access_exp: self.exp,
            };
            Box::pin(async move { Ok(creds) })
        }

        fn rejected<'a>(&'a self, _: &'a AttemptCreds) -> BoxFuture<'a, Result<(), ApiError>> {
            self.refreshes.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    fn counting(exp: Option<u64>) -> CountingAuth {
        CountingAuth {
            refreshes: AtomicUsize::new(0),
            exp,
        }
    }

    const UNAUTHORIZED: &str = r#"{"error":{"message":"Token expired","code":"UNAUTHORIZED"}}"#;

    #[tokio::test]
    async fn b4_code_less_or_non_json_401_never_refreshes() {
        for body in [
            "",
            "Unauthorized",
            r#"{"error":{"message":"Unauthorized"}}"#,
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(401).set_body_string(body))
                .mount(&server)
                .await;
            let ex = exec_at(server.address().port());
            let auth = counting(None);
            let e = ex
                .run(&get_op(), get_call(), &auth, None)
                .await
                .expect_err("401");
            assert_eq!(
                e,
                ApiError::EngineUnavailable { permanent: false },
                "{body:?}"
            );
            assert_eq!(auth.refreshes.load(Ordering::SeqCst), 0, "{body:?}");
            assert_eq!(count(&server).await, 1, "{body:?}");
        }
    }

    #[tokio::test]
    async fn b4_code_less_401_replays_only_on_a_changed_endpoint() {
        let foreign = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&foreign)
            .await;
        let engine = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(json(200, "{}"))
            .mount(&engine)
            .await;
        // Attempt 1 reads gen 1 (foreign); the 401 check sees gen 2; the
        // replay goes to gen 2.
        let src = Arc::new(SeqEndpoint::new(vec![
            ready(foreign.address().port(), TOKEN, 1),
            ready(engine.address().port(), TOKEN, 2),
        ]));
        let ex = exec_on(src);
        let auth = counting(None);
        let r = ex
            .run(&get_op(), get_call(), &auth, None)
            .await
            .expect("replayed");
        assert_eq!(r.generation, 2);
        assert_eq!(auth.refreshes.load(Ordering::SeqCst), 0);
        let reqs = foreign.received_requests().await.expect("r");
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].headers.get("cookie").is_none());
    }

    #[tokio::test]
    async fn engine_unauthorized_refreshes_and_replays_a_read_once() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(json(401, UNAUTHORIZED))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(json(200, "{}"))
            .mount(&server)
            .await;
        let ex = exec_at(server.address().port());
        let auth = counting(None);
        ex.run(&get_op(), get_call(), &auth, None)
            .await
            .expect("refreshed");
        assert_eq!(auth.refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(count(&server).await, 2);

        // Rejected again after the refresh: the session is over.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(json(401, UNAUTHORIZED))
            .mount(&server)
            .await;
        let ex = exec_at(server.address().port());
        let auth = counting(None);
        let e = ex
            .run(&get_op(), get_call(), &auth, None)
            .await
            .expect_err("expired");
        assert_eq!(e, ApiError::Unauthorized(AuthFailure::SessionExpired));
        assert_eq!(auth.refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(count(&server).await, 2);
    }

    #[tokio::test]
    async fn mutation_401_replays_only_when_our_token_had_expired() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(json(401, UNAUTHORIZED))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(json(201, "{}"))
            .mount(&server)
            .await;
        let ex = exec_at(server.address().port());
        // Token still valid by our clock: the session was revoked; no replay.
        let valid = counting(Some(unix_now() + 600));
        let e = ex
            .run(&post_op(), post_call(), &valid, None)
            .await
            .expect_err("revoked");
        assert_eq!(e, ApiError::Unauthorized(AuthFailure::SessionExpired));
        assert_eq!(valid.refreshes.load(Ordering::SeqCst), 0);
        // Expired by our clock: authenticate refused before any handler ran.
        let expired = counting(Some(unix_now().saturating_sub(5)));
        Mock::given(method("POST"))
            .respond_with(json(401, UNAUTHORIZED))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        ex.run(&post_op(), post_call(), &expired, None)
            .await
            .expect("replayed");
        assert_eq!(expired.refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(count(&server).await, 3);
    }

    #[tokio::test]
    async fn unauthorized_after_a_generation_change_rereads_without_refresh() {
        let old = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(json(401, UNAUTHORIZED))
            .mount(&old)
            .await;
        let new = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(json(200, "{}"))
            .mount(&new)
            .await;
        let src = Arc::new(SeqEndpoint::new(vec![
            ready(old.address().port(), TOKEN, 1),
            ready(new.address().port(), TOKEN, 2),
        ]));
        let ex = exec_on(src);
        let auth = counting(None);
        ex.run(&get_op(), get_call(), &auth, None)
            .await
            .expect("new engine");
        assert_eq!(auth.refreshes.load(Ordering::SeqCst), 0);
    }

    async fn failing_post_server(status: u16) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(json(
                status,
                r#"{"error":{"message":"x","code":"INTERNAL_ERROR"}}"#,
            ))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(json(201, r#"{"second":true}"#))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn reconciliation_committed_returns_success_without_a_second_post() {
        let server = failing_post_server(500).await;
        let ex = exec_at(server.address().port());
        let rec = Script::new(vec!["committed"]);
        let r = ex
            .run(&post_op(), post_call(), &none(), Some(&rec))
            .await
            .expect("found");
        assert_eq!(&r.body[..], b"{\"found\":true}");
        assert_eq!(count(&server).await, 1);
    }

    #[tokio::test]
    async fn reconciliation_not_committed_retries_once_with_the_same_key() {
        let server = failing_post_server(502).await;
        let ex = exec_at(server.address().port());
        let rec = Script::new(vec!["not_committed"]);
        let r = ex
            .run(&post_op(), post_call(), &none(), Some(&rec))
            .await
            .expect("retried");
        assert_eq!(&r.body[..], b"{\"second\":true}");
        let reqs = server.received_requests().await.expect("r");
        assert_eq!(reqs.len(), 2);
        let k0 = reqs[0].headers.get("idempotency-key").expect("k0");
        let k1 = reqs[1].headers.get("idempotency-key").expect("k1");
        assert_eq!(k0, k1);
    }

    #[tokio::test]
    async fn reconciliation_never_loops_and_unresolved_is_outcome_unknown() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let ex = exec_at(server.address().port());
        let rec = Script::new(vec!["not_committed", "not_committed"]);
        let e = ex
            .run(&post_op(), post_call(), &none(), Some(&rec))
            .await
            .expect_err("unknown");
        assert!(matches!(
            e,
            ApiError::OutcomeUnknown {
                idempotency_key: Some(_),
                ..
            }
        ));
        assert_eq!(count(&server).await, 2);
        assert_eq!(rec.calls.load(Ordering::SeqCst), 2);

        let rec = Script::new(vec!["unresolved"]);
        let e = ex
            .run(&post_op(), post_call(), &none(), Some(&rec))
            .await
            .expect_err("unknown");
        assert!(matches!(e, ApiError::OutcomeUnknown { .. }));
        assert_eq!(count(&server).await, 3);
    }

    #[tokio::test]
    async fn reconciler_sees_a_settled_request_after_a_restart() {
        let slow = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(json(201, "{}").set_delay(Duration::from_secs(3)))
            .mount(&slow)
            .await;
        let src = Arc::new(FixedEndpoint::new(
            slow.address().port(),
            Secret::new(TOKEN),
            1,
        ));
        let mut cfg = fast_config();
        cfg.timeouts.write.attempt = Duration::from_millis(200);
        let ex = Executor::new(Transport::new(src.clone(), cfg).expect("client"));
        let mut rec = Script::new(vec!["unresolved"]);
        rec.src = Some(src.clone());
        // Same generation: not settled.
        let _ = ex.run(&post_op(), post_call(), &none(), Some(&rec)).await;
        src.set(ready(slow.address().port(), TOKEN, 2));
        let amb = Ambiguity {
            request_id: RequestId::from_uuid(uuid::Uuid::nil()),
            idempotency_key: None,
            generation: 1,
            cause: AmbiguityCause::Timeout,
        };
        assert!(amb.settled(&src.current()));
        assert!(!amb.settled(&EndpointState::Restarting));
        assert_eq!(*rec.settled_seen.lock().expect("lock"), vec![false]);
    }

    #[tokio::test]
    async fn read_after_idle_close_is_retried_transparently() {
        let (port, count) = idle_closing_server(Duration::from_secs(1)).await;
        let ex = exec_at(port);
        ex.run(&get_op(), get_call(), &none(), None)
            .await
            .expect("first");
        tokio::time::sleep(Duration::from_millis(1500)).await;
        ex.run(&get_op(), get_call(), &none(), None)
            .await
            .expect("second");
        tokio::time::sleep(Duration::from_secs(3)).await;
        ex.run(&post_op(), post_call(), &none(), None)
            .await
            .expect("mutation after idle");
        assert!(count.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn rate_limited_read_waits_but_login_does_not() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                json(429, r#"{"error":{"message":"x"}}"#).insert_header("retry-after", "1"),
            )
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(json(200, "{}"))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(
                json(429, r#"{"error":{"message":"x"}}"#).insert_header("retry-after", "1"),
            )
            .mount(&server)
            .await;
        let ex = exec_at(server.address().port());
        ex.run(&get_op(), get_call(), &none(), None)
            .await
            .expect("waited");
        let login = Call::at(Method::Post, "/api/v1/auth/login").json_body(b"{}".to_vec());
        let e = ex
            .run(&LOGIN, login, &none(), None)
            .await
            .expect_err("limited");
        assert_eq!(
            e,
            ApiError::RateLimited {
                retry_after: Some(Duration::from_secs(1))
            }
        );
        assert_eq!(count(&server).await, 3);
    }

    #[tokio::test]
    async fn keyed_once_retries_mutations_with_the_same_key_once_u18_is_advertised() {
        let server = failing_post_server(503).await;
        let ex = exec_at(server.address().port());
        ex.set_idempotency_v1(true);
        ex.run(&post_op(), post_call(), &none(), None)
            .await
            .expect("keyed retry");
        let reqs = server.received_requests().await.expect("r");
        assert_eq!(reqs.len(), 2);
        assert_eq!(
            reqs[0].headers.get("idempotency-key"),
            reqs[1].headers.get("idempotency-key")
        );
    }

    #[tokio::test]
    async fn down_permanent_fails_at_once() {
        let ex = exec_on(Arc::new(FixedEndpoint::with_state(EndpointState::Down {
            permanent: true,
        })));
        let e = ex
            .run(&get_op(), get_call(), &none(), None)
            .await
            .expect_err("down");
        assert_eq!(e, ApiError::EngineUnavailable { permanent: true });
    }

    #[test]
    fn jitter_stays_in_range_and_keys_are_unique() {
        for cap in [0, 1, 200, 2000] {
            for _ in 0..50 {
                assert!(jitter(cap) <= cap);
            }
        }
        assert_ne!(IdempotencyKey::new(), IdempotencyKey::new());
        assert_eq!(IdempotencyKey::new().as_header().len(), 36);
    }
}
