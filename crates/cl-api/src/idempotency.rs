//! Idempotency keys and the safe-retry policy (P05 plan §2.6, U-18).
//!
//! Without `idempotency-v1` the engine ignores `Idempotency-Key` (except
//! `/batch/save`, `batch-idempotent-v1`), so sending it is harmless and also
//! no protection: a mutation is retried only when it provably never reached
//! a handler, or when outcome reconciliation proves the first attempt did not
//! commit.
//!
//! With `idempotency-v1` the engine replays a repeated key on exactly its
//! route set (the `keyable` rows of `operations.txt`). Those routes, and only
//! those, send the key and are retried like reads with it (`KeyedOnce`): a
//! repeat gets the first result (`Idempotent-Replayed: true`), a key still
//! running gets 409 `IDEMPOTENCY_IN_PROGRESS` (waited for, `Retry-After`).
//! Every other mutation keeps the rules above.

use std::fmt;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cl_core::ids::RequestId;
use tokio::time::Instant;
use tracing::Instrument;

use crate::client::{AttemptAuth, Call, EngineReply, Transport, TransportError};
use crate::endpoint::EndpointState;
use crate::errors::{ApiError, AuthFailure, Mapped, RouteKind, map_failure};
use crate::resources::operations::{BATCH_SAVE, Operation, RetryClass};

/// The engine forgets a key after 24 hours (U-18, Q-U18c); cl-api relies on
/// a key for a replay only while it is younger than this, so clock and
/// timing slack can never turn a "replay" into a second posting.
pub(crate) const KEY_REPLAY_AGE: Duration = Duration::from_hours(20);

/// Opaque key sent as `Idempotency-Key` on mutations. Two keys are equal
/// when their values are; the creation instant only says how old it is.
#[derive(Clone)]
pub struct IdempotencyKey {
    id: uuid::Uuid,
    /// When the key was made (monotonic clock).
    born: std::time::Instant,
}

impl PartialEq for IdempotencyKey {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for IdempotencyKey {}

impl Hash for IdempotencyKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl IdempotencyKey {
    /// A fresh random key (one per logical user action).
    #[must_use]
    pub fn new() -> IdempotencyKey {
        IdempotencyKey {
            id: uuid::Uuid::new_v4(),
            born: std::time::Instant::now(),
        }
    }

    /// Header value (36-character lower-case UUID).
    #[must_use]
    pub fn as_header(&self) -> String {
        self.id.hyphenated().to_string()
    }

    /// The engine still replays this key at `now` with a wide margin (it
    /// is younger than [`KEY_REPLAY_AGE`]).
    pub(crate) fn replayable_at(&self, now: std::time::Instant) -> bool {
        now.saturating_duration_since(self.born) < KEY_REPLAY_AGE
    }

    /// [`Self::replayable_at`] now.
    pub(crate) fn replayable(&self) -> bool {
        self.replayable_at(std::time::Instant::now())
    }

    /// Tests: a key made `age` ago (`None` when the monotonic clock does
    /// not reach that far back, for example shortly after boot).
    #[cfg(test)]
    pub(crate) fn aged(age: Duration) -> Option<IdempotencyKey> {
        Some(IdempotencyKey {
            id: uuid::Uuid::new_v4(),
            born: std::time::Instant::now().checked_sub(age)?,
        })
    }
}

impl Default for IdempotencyKey {
    fn default() -> Self {
        IdempotencyKey::new()
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id.hyphenated())
    }
}

impl fmt::Debug for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IdempotencyKey({})", self.id.hyphenated())
    }
}

// ---------------------------------------------------------------------------
// The executor: one logical request, its attempts and the retry policy.
// ---------------------------------------------------------------------------

/// A boxed, sendable future (object-safe hooks).
pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What outcome reconciliation found out about an ambiguous mutation
/// (P05 plan §2.6.1).
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
#[derive(Clone, Debug)]
pub(crate) struct Ambiguity {
    pub request_id: RequestId,
    /// Generation of the endpoint the attempt used.
    pub generation: u64,
    pub cause: AmbiguityCause,
    /// The idempotency key the attempt carried: the owner's recheck finds
    /// the stored probe by it (B2).
    pub key: Option<IdempotencyKey>,
}

impl Ambiguity {
    /// The old request can no longer commit: the engine process it reached
    /// is gone (`PostgreSQL` rolls back a transaction whose client vanished).
    /// A timeout or a broken connection on the same generation is not
    /// settled, because the engine sets no request or statement timeout.
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

/// Refused connects of one request that may wait for the endpoint to move
/// on (one per generation), so an endpoint that keeps changing to dead ports
/// cannot spin the retry loop.
const MAX_SETTLE_WAITS: u32 = 3;

/// 409 `IDEMPOTENCY_IN_PROGRESS` (U-18): another request with this key holds
/// its lock (`Retry-After: 1`).
pub(crate) const IDEMPOTENCY_IN_PROGRESS: &str = "IDEMPOTENCY_IN_PROGRESS";

/// Waits for a key still in progress, at most, per request (each within the
/// request's deadline).
const MAX_IN_PROGRESS_RETRIES: u32 = 5;

/// The wait when the engine names none.
const IN_PROGRESS_WAIT: Duration = Duration::from_secs(1);

/// The longest single wait for a key in progress.
const MAX_IN_PROGRESS_WAIT: Duration = Duration::from_secs(5);

/// Runs logical requests through the transport with the §2.6 policy.
pub(crate) struct Executor {
    transport: Transport,
    idempotency_v1: AtomicBool,
}

/// Per-request bookkeeping of the retry loop.
#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent once-only flags of one retry loop"
)]
struct Retries {
    transient: u32,
    /// Waits for 409 `IDEMPOTENCY_IN_PROGRESS` so far.
    in_progress: u32,
    gate_replayed: bool,
    auth_replayed: bool,
    same_key_retried: bool,
    /// An earlier attempt may have reached a handler (timeout, broken
    /// connection, 5xx).
    maybe_committed: bool,
    /// Generation whose first refused connect was retried after the ordinary
    /// backoff.
    probed_generation: Option<u64>,
    /// Generation whose refused connect last waited for the endpoint to move
    /// on, and how many such waits this request has had.
    settle_generation: Option<u64>,
    settle_waits: u32,
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
        if !call.method.is_read() && call.idempotency_key.is_none() {
            call.idempotency_key = Some(IdempotencyKey::new());
        }
        let idempotency_v1 = self.idempotency_v1.load(Ordering::Relaxed);
        let class = op.retry().effective(
            idempotency_v1
                && call
                    .idempotency_key
                    .as_ref()
                    .is_some_and(IdempotencyKey::replayable),
        );
        // With `idempotency-v1` the key goes only where the engine replays
        // it: its route set, and `/batch/save` (its own
        // `batch-idempotent-v1`). Without it every mutation sends the key as
        // before (the engine ignores it there).
        call.send_key = !idempotency_v1 || op.is_keyable() || *op == BATCH_SAVE;
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
            let creds = match auth.prepare().await {
                Ok(c) => c,
                // The company was switched (pinned requests): nothing of
                // this attempt was sent, but an earlier one may have
                // committed in the previous company.
                Err(ApiError::StaleScope) if r.maybe_committed && !call.method.is_read() => {
                    tracing::info!(outcome = "unknown", reconciled = false, "mutation outcome");
                    return Err(ApiError::OutcomeUnknown {
                        request_id: call.request_id,
                        idempotency_key: call.idempotency_key.clone(),
                        // The company was switched: never "save again" here.
                        keyed: false,
                    });
                }
                Err(e) => return Err(e),
            };
            let outcome = self
                .transport
                .attempt(call, &creds.auth, attempt, ctx.deadline)
                .await;
            let step = match outcome {
                Ok(reply) if (200..300).contains(&reply.status) => Step::Done(success(reply)),
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
            TransportError::NotSent { .. } | TransportError::Ambiguous { .. } => {
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
            TransportError::NotSent { generation } => {
                if ctx.class != RetryClass::Never
                    && self.retry_refused(generation, r, ctx.deadline).await
                {
                    Step::Again
                } else {
                    Step::Done(Err(keyed_give_up(ctx, r, give_up(e))))
                }
            }
            TransportError::Timeout { generation } | TransportError::Ambiguous { generation } => {
                r.maybe_committed = true;
                match ctx.class {
                    RetryClass::Idempotent | RetryClass::KeyedOnce => {
                        if self.backoff(r, ctx.deadline).await {
                            Step::Again
                        } else {
                            Step::Done(Err(keyed_give_up(ctx, r, give_up(e))))
                        }
                    }
                    RetryClass::Never => Step::Done(Err(give_up(e))),
                    // `Keyable` is never in effect (`effective` maps it);
                    // were it, the unkeyed rule is the safe one.
                    RetryClass::PreHandlerOnly | RetryClass::Keyable => {
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
        match map_failure(
            reply.status,
            &reply.body,
            reply.retry_after,
            ctx.op.kind,
            ctx.op.failure_shape(),
        ) {
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
            Mapped::Final(e) => {
                self.on_final(ctx, e, reply.generation, reply.retry_after, r)
                    .await
            }
        }
    }

    async fn on_final(
        &self,
        ctx: &Ctx<'_>,
        e: ApiError,
        generation: u64,
        retry_after: Option<Duration>,
        r: &mut Retries,
    ) -> Step {
        if matches!(e, ApiError::Server { .. }) {
            r.maybe_committed = true;
        }

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
                    Step::Done(Err(keyed_give_up(ctx, r, e)))
                }
            }
            (
                RetryClass::Idempotent,
                ApiError::RateLimited {
                    retry_after: Some(wait),
                },
            ) if rate_limit_ok => {
                let wait = *wait;
                let in_time = Instant::now()
                    .checked_add(wait)
                    .is_some_and(|t| t < ctx.deadline);
                if r.transient < MAX_TRANSIENT_RETRIES && in_time {
                    r.transient += 1;
                    tokio::time::sleep(wait).await;
                    Step::Again
                } else {
                    Step::Done(Err(e))
                }
            }
            // U-18: the same key is still running (another attempt of this
            // save, whose reply may have been lost). Nothing was posted by
            // this attempt; wait as asked and send the same key again, which
            // the engine replays once the first one finishes.
            (RetryClass::KeyedOnce, ApiError::Conflict { code: Some(c), .. })
                if c.as_str() == IDEMPOTENCY_IN_PROGRESS =>
            {
                r.maybe_committed = true;
                let wait = retry_after
                    .unwrap_or(IN_PROGRESS_WAIT)
                    .clamp(Duration::from_millis(100), MAX_IN_PROGRESS_WAIT);
                let in_time = Instant::now()
                    .checked_add(wait)
                    .is_some_and(|t| t < ctx.deadline);
                if r.in_progress < MAX_IN_PROGRESS_RETRIES && in_time {
                    r.in_progress += 1;
                    tokio::time::sleep(wait).await;
                    Step::Again
                } else {
                    Step::Done(Err(keyed_give_up(ctx, r, e)))
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
            keyed: false,
        };
        let Some(rec) = ctx.reconciler else {
            tracing::info!(outcome = "unknown", reconciled = false, "mutation outcome");
            return Step::Done(Err(unknown()));
        };
        let amb = Ambiguity {
            request_id: ctx.call.request_id,
            generation,
            cause,
            key: ctx.call.idempotency_key.clone(),
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

    /// Whether to retry after a refused connect (nothing was sent, so a retry
    /// is safe). The first refusal on a generation is retried after the
    /// ordinary backoff: a live engine can refuse once (a full listen backlog,
    /// say). A refusal after that is most likely a dead engine, so the request
    /// waits for the endpoint to move on before it falls back to the backoff.
    async fn retry_refused(&self, generation: u64, r: &mut Retries, deadline: Instant) -> bool {
        if r.probed_generation != Some(generation) {
            r.probed_generation = Some(generation);
            if self.backoff(r, deadline).await {
                return true;
            }
        }
        self.endpoint_moved_on(generation, r, deadline).await || self.backoff(r, deadline).await
    }

    /// A connect refused again by an endpoint that is still published as the
    /// same generation: the engine has most likely just died and its
    /// supervisor has not unpublished it yet (it does on its next check).
    /// Wait, within the deadline and `refused_settle`, for the endpoint to
    /// change instead of spending the transient retries against a dead port,
    /// so a request issued while the engine is down waits for the next
    /// generation like one issued while it is `Restarting`. Waits once per
    /// generation and at most `MAX_SETTLE_WAITS` times per request; returns
    /// whether the endpoint moved, never true at or after the deadline.
    async fn endpoint_moved_on(&self, generation: u64, r: &mut Retries, deadline: Instant) -> bool {
        if r.settle_generation == Some(generation) || r.settle_waits >= MAX_SETTLE_WAITS {
            return false;
        }
        r.settle_generation = Some(generation);
        r.settle_waits += 1;
        let timeouts = &self.transport.config().timeouts;
        let until = Instant::now()
            .checked_add(timeouts.refused_settle)
            .map_or(deadline, |t| t.min(deadline));
        loop {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let same = matches!(
                self.transport.current(),
                EndpointState::Ready(ep) if ep.generation == generation
            );
            if !same {
                return true;
            }
            if now >= until {
                return false;
            }
            tokio::time::sleep(timeouts.poll.min(until - now)).await;
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

/// Giving up on a keyed mutation (U-18 `KeyedOnce`) after an attempt that
/// may have reached a handler: the outcome is unknown, so it is
/// `OutcomeUnknown`, not `Timeout` or `EngineUnavailable`. It is `keyed`:
/// sending the same request again with the same key is a replay, never a
/// second posting (Q-PR1a).
fn keyed_give_up(ctx: &Ctx<'_>, r: &Retries, e: ApiError) -> ApiError {
    if ctx.class == RetryClass::KeyedOnce && r.maybe_committed {
        tracing::info!(
            outcome = "unknown",
            reconciled = false,
            keyed = true,
            "mutation outcome"
        );
        ApiError::OutcomeUnknown {
            request_id: ctx.call.request_id,
            idempotency_key: ctx.call.idempotency_key.clone(),
            keyed: true,
        }
    } else {
        e
    }
}

/// A 2xx answer. A replay (`Idempotent-Replayed: true`, U-18) is an
/// ordinary success: the first request's stored answer, or its record as it
/// is now. When that record is gone the engine answers only
/// `{"idempotency":{"state":"committed",…}}`: the save is in the books but
/// there is nothing to show, so it is `SavedUnreadable` ("saved, reload").
fn success(reply: EngineReply) -> Result<EngineReply, ApiError> {
    if reply.replayed {
        tracing::debug!(replayed = true, "the engine replayed an earlier answer");
        if committed_marker(&reply.body) {
            return Err(ApiError::SavedUnreadable {
                what: "replayed save without its record",
            });
        }
    }
    Ok(reply)
}

/// The engine's minimal committed-replay body (U-18 plan §2.6).
fn committed_marker(body: &[u8]) -> bool {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_slice::<serde_json::Value>(body)
    else {
        return false;
    };
    map.len() == 1
        && map
            .get("idempotency")
            .and_then(|i| i.get("state"))
            .and_then(serde_json::Value::as_str)
            == Some("committed")
}

/// An unknown outcome of a call whose key the caller does not hold (the
/// resource made a fresh key for that one call): sending "the same save"
/// again would carry a new key, so it is not `keyed` for the caller.
pub(crate) fn unkeyed(e: ApiError) -> ApiError {
    match e {
        ApiError::OutcomeUnknown {
            request_id,
            idempotency_key,
            ..
        } => ApiError::OutcomeUnknown {
            request_id,
            idempotency_key,
            keyed: false,
        },
        other => other,
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
    use crate::client::tests::{ConnectStub, TOKEN, fast_config, idle_closing_server};
    use crate::client::{ClientConfig, Method, Transport};
    use crate::endpoint::{EndpointSource, EngineEndpoint, FixedEndpoint, Purpose};
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
            keyed: false,
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
        // Generation 1 refuses every connect; generation 2 answers.
        let seq = || -> Arc<dyn EndpointSource> {
            Arc::new(SeqEndpoint::new(vec![
                ready(port, TOKEN, 1),
                ready(port, TOKEN, 2),
            ]))
        };
        let stub = ConnectStub::new(|_, generation| generation == 1);
        let ex = exec_stubbed(seq(), fast_config(), &stub);
        let r = ex
            .run(&post_op(), post_call(), &none(), None)
            .await
            .expect("retried");
        assert_eq!(r.generation, 2);
        assert_eq!(stub.connects(), 2);
        assert_eq!(count(&server).await, 1);

        let stub = ConnectStub::new(|_, generation| generation == 1);
        let ex = exec_stubbed(seq(), fast_config(), &stub);
        let login = Call::at(Method::Post, "/api/v1/auth/login").json_body(b"{}".to_vec());
        let e = ex
            .run(&LOGIN, login, &none(), None)
            .await
            .expect_err("never class");
        assert_eq!(e, ApiError::EngineUnavailable { permanent: false });
        assert_eq!(stub.connects(), 1);
        assert_eq!(count(&server).await, 1);
    }

    /// `before` until `flip_at`, then `after`: an engine that died while its
    /// supervisor still publishes it, then the next generation.
    struct FlipEndpoint {
        before: EndpointState,
        flip_at: Instant,
        after: EndpointState,
    }

    impl EndpointSource for FlipEndpoint {
        fn current(&self) -> EndpointState {
            if Instant::now() < self.flip_at {
                self.before.clone()
            } else {
                self.after.clone()
            }
        }
    }

    /// A port no test connects to: every connect on it is refused by the
    /// [`ConnectStub`] before a socket is opened.
    const UNUSED_PORT: u16 = 9;

    /// An executor whose connects go through `stub` first.
    fn exec_stubbed(
        src: Arc<dyn EndpointSource>,
        cfg: ClientConfig,
        stub: &Arc<ConnectStub>,
    ) -> Executor {
        Executor::new(
            Transport::new(src, cfg)
                .expect("client")
                .with_connect_stub(Arc::clone(stub)),
        )
    }

    /// Wraps a source and records after which connect the executor re-read
    /// the endpoint outside `wait_ready` (`current`, not `current_for`). In a
    /// refused-connect run only the settle wait does that, so the recorded
    /// connect numbers are the settle waits, one entry per wait.
    struct SettleWatch {
        inner: Arc<dyn EndpointSource>,
        stub: Arc<ConnectStub>,
        waits_after: Mutex<Vec<u32>>,
    }

    impl SettleWatch {
        fn new(inner: Arc<dyn EndpointSource>, stub: &Arc<ConnectStub>) -> Arc<SettleWatch> {
            Arc::new(SettleWatch {
                inner,
                stub: Arc::clone(stub),
                waits_after: Mutex::new(Vec::new()),
            })
        }

        fn waits_after(&self) -> Vec<u32> {
            self.waits_after.lock().expect("lock").clone()
        }
    }

    impl EndpointSource for SettleWatch {
        fn current(&self) -> EndpointState {
            let n = self.stub.connects();
            let mut w = self.waits_after.lock().expect("lock");
            if w.last() != Some(&n) {
                w.push(n);
            }
            self.inner.current()
        }
        fn current_for(&self, _: Purpose) -> EndpointState {
            self.inner.current()
        }
    }

    /// Ready on a generation one past the connects made so far: every
    /// connect lands on a new generation.
    struct MovingEndpoint(Arc<ConnectStub>);

    impl EndpointSource for MovingEndpoint {
        fn current(&self) -> EndpointState {
            ready(UNUSED_PORT, TOKEN, u64::from(self.0.connects()) + 1)
        }
    }

    #[tokio::test]
    async fn one_refused_connect_on_a_live_endpoint_is_retried_quickly() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(json(201, "{}"))
            .mount(&server)
            .await;
        let fixed = Arc::new(FixedEndpoint::new(
            server.address().port(),
            Secret::new(TOKEN),
            1,
        ));
        let stub = ConnectStub::new(|n, _| n == 1);
        let src = SettleWatch::new(fixed, &stub);
        let mut cfg = fast_config();
        cfg.timeouts.refused_settle = Duration::from_secs(3);
        let ex = exec_stubbed(src.clone(), cfg, &stub);
        let started = Instant::now();
        let r = ex
            .run(&post_op(), post_call(), &none(), None)
            .await
            .expect("second connect answers");
        let took = started.elapsed();
        assert_eq!(r.generation, 1);
        assert_eq!(stub.connects(), 2);
        assert_eq!(count(&server).await, 1, "sent exactly once");
        assert_eq!(src.waits_after(), Vec::<u32>::new(), "no settle wait");
        // One backoff of at most 200 ms, far below `refused_settle`.
        assert!(took < Duration::from_secs(1), "{took:?}");
    }

    #[tokio::test]
    async fn refused_write_on_a_stale_endpoint_waits_for_the_next_generation() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(json(201, "{}"))
            .mount(&server)
            .await;
        let port = server.address().port();
        // Generation 1 refuses for 800 ms, longer than the whole transient
        // budget (three instant connects, at most 200 + 400 ms of backoff),
        // but within one settle wait (1 s by default) after the quick retry.
        let stale = || -> Arc<dyn EndpointSource> {
            Arc::new(FlipEndpoint {
                before: ready(port, TOKEN, 1),
                flip_at: Instant::now() + Duration::from_millis(800),
                after: ready(port, TOKEN, 2),
            })
        };
        let stub = ConnectStub::new(|_, generation| generation == 1);
        let src = SettleWatch::new(stale(), &stub);
        let ex = exec_stubbed(src.clone(), fast_config(), &stub);
        let r = ex
            .run(&post_op(), post_call(), &none(), None)
            .await
            .expect("sent to the next generation");
        assert_eq!(r.generation, 2);
        assert_eq!(count(&server).await, 1, "sent exactly once");
        // Refused, quick retry refused, one settle wait, then generation 2.
        assert_eq!(stub.connects(), 3);
        assert_eq!(src.waits_after(), vec![2]);

        // A `Never` route stays single-shot: it neither retries nor waits.
        let stub = ConnectStub::new(|_, generation| generation == 1);
        let src = SettleWatch::new(stale(), &stub);
        let ex = exec_stubbed(src.clone(), fast_config(), &stub);
        let login = Call::at(Method::Post, "/api/v1/auth/login").json_body(b"{}".to_vec());
        let e = ex
            .run(&LOGIN, login, &none(), None)
            .await
            .expect_err("never class");
        assert_eq!(e, ApiError::EngineUnavailable { permanent: false });
        assert_eq!(stub.connects(), 1);
        assert_eq!(src.waits_after(), Vec::<u32>::new(), "no settle wait");
        assert_eq!(count(&server).await, 1);
    }

    #[tokio::test]
    async fn refused_connect_on_an_endpoint_that_never_moves_gives_up_after_one_settle_wait() {
        let fixed = Arc::new(FixedEndpoint::new(UNUSED_PORT, Secret::new(TOKEN), 1));
        let stub = ConnectStub::new(|_, _| true);
        let src = SettleWatch::new(fixed, &stub);
        let mut cfg = fast_config();
        cfg.timeouts.refused_settle = Duration::from_millis(300);
        let ex = exec_stubbed(src.clone(), cfg, &stub);
        let started = Instant::now();
        let e = ex
            .run(&post_op(), post_call(), &none(), None)
            .await
            .expect_err("always refused");
        let took = started.elapsed();
        assert_eq!(e, ApiError::EngineUnavailable { permanent: false });
        // Refused; quick retry refused; one full settle wait; last retry
        // refused; no second wait on the same generation.
        assert_eq!(stub.connects(), 3);
        assert_eq!(src.waits_after(), vec![2]);
        assert!(took >= Duration::from_millis(300), "{took:?}");
        // Backstop: well before the 4 s deadline.
        assert!(took < Duration::from_secs(3), "{took:?}");
    }

    #[tokio::test]
    async fn refused_connects_on_ever_new_generations_wait_at_most_three_times() {
        let stub = ConnectStub::new(|_, _| true);
        let src = SettleWatch::new(Arc::new(MovingEndpoint(Arc::clone(&stub))), &stub);
        let ex = exec_stubbed(src.clone(), fast_config(), &stub);
        let e = ex
            .run(&post_op(), post_call(), &none(), None)
            .await
            .expect_err("always refused");
        assert_eq!(e, ApiError::EngineUnavailable { permanent: false });
        // Connects 1 and 2 use the two backoffs; 3, 4 and 5 each see the
        // endpoint move on at once (three settle waits); 6 gives up.
        assert_eq!(stub.connects(), 6);
        assert_eq!(src.waits_after(), vec![3, 4, 5]);
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
            generation: 1,
            cause: AmbiguityCause::Timeout,
            key: None,
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
    async fn a_huge_retry_after_is_clamped_and_never_overflows() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                json(429, r#"{"error":{"message":"x"}}"#)
                    .insert_header("retry-after", u64::MAX.to_string().as_str()),
            )
            .mount(&server)
            .await;
        let ex = exec_at(server.address().port());
        let t = Instant::now();
        let e = ex
            .run(&get_op(), get_call(), &none(), None)
            .await
            .expect_err("limited");
        assert_eq!(
            e,
            ApiError::RateLimited {
                retry_after: Some(crate::client::MAX_RETRY_AFTER)
            }
        );
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "no wait past the deadline"
        );
        assert_eq!(count(&server).await, 1);
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
    async fn keyed_once_giving_up_after_a_timeout_or_5xx_is_outcome_unknown_with_the_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/slow"))
            .respond_with(json(201, "{}").set_delay(Duration::from_secs(3)))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/accounts"))
            .respond_with(json(503, r#"{"error":{"message":"x"}}"#))
            .mount(&server)
            .await;
        let ex = exec_at(server.address().port());
        ex.set_idempotency_v1(true);
        for call in [
            Call::new(Method::Post, "/api/v1/accounts", "/slow").json_body(b"{}".to_vec()),
            post_call(),
        ] {
            let key = IdempotencyKey::new();
            let mut call = call;
            call.idempotency_key = Some(key.clone());
            let e = ex
                .run(&post_op(), call, &none(), None)
                .await
                .expect_err("gave up");
            assert!(
                matches!(e, ApiError::OutcomeUnknown { idempotency_key: Some(ref k), keyed: true, .. } if *k == key),
                "{e:?}"
            );
        }
    }

    fn post_to(route: &'static str) -> (Operation, Call) {
        let op = *find(Method::Post, route).expect("covered");
        (op, Call::at(Method::Post, route).json_body(b"{}".to_vec()))
    }

    /// U-18 plan §10.1: with `idempotency-v1` the key goes only to the
    /// engine's route set (and `/batch/save`, its own capability); a route
    /// outside it is never blind-retried. Without the capability every
    /// mutation sends the key, as before.
    #[tokio::test]
    async fn idempotency_v1_sends_the_key_only_where_the_engine_replays_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(json(503, r#"{"error":{"message":"x"}}"#))
            .mount(&server)
            .await;
        let ex = exec_at(server.address().port());
        ex.set_idempotency_v1(true);
        // Outside the set: no header, one attempt, outcome unknown (unkeyed).
        let (op, call) = post_to("/api/v1/contacts/merge");
        let e = ex.run(&op, call, &none(), None).await.expect_err("5xx");
        assert!(
            matches!(e, ApiError::OutcomeUnknown { keyed: false, .. }),
            "{e:?}"
        );
        let reqs = server.received_requests().await.expect("r");
        assert_eq!(reqs.len(), 1, "never blind-retried");
        assert!(reqs[0].headers.get("idempotency-key").is_none());
        // Batch save keeps its own key mechanism.
        let (op, call) = post_to("/api/v1/batch/save");
        let _ = ex.run(&op, call, &none(), None).await;
        let reqs = server.received_requests().await.expect("r");
        assert!(reqs[1].headers.get("idempotency-key").is_some());
        // In the set: the key, retried with it.
        let (op, call) = post_to("/api/v1/transactions");
        let e = ex.run(&op, call, &none(), None).await.expect_err("5xx");
        assert!(
            matches!(e, ApiError::OutcomeUnknown { keyed: true, .. }),
            "{e:?}"
        );
        let reqs = server.received_requests().await.expect("r");
        let keyed: Vec<_> = reqs[2..]
            .iter()
            .map(|r| r.headers.get("idempotency-key").cloned())
            .collect();
        assert_eq!(keyed.len(), 3, "retried with the key");
        assert!(keyed.iter().all(|k| k.is_some() && *k == keyed[0]));
        // Without the capability: every mutation carries a key, none is
        // retried after it may have reached a handler.
        ex.set_idempotency_v1(false);
        let before = reqs.len();
        let (op, call) = post_to("/api/v1/contacts/merge");
        let _ = ex.run(&op, call, &none(), None).await;
        let (op, call) = post_to("/api/v1/transactions");
        let _ = ex.run(&op, call, &none(), None).await;
        let reqs = server.received_requests().await.expect("r");
        assert_eq!(reqs.len(), before + 2);
        assert!(
            reqs[before..]
                .iter()
                .all(|r| r.headers.get("idempotency-key").is_some())
        );
    }

    /// C3: 409 `IDEMPOTENCY_IN_PROGRESS` on a keyed route is waited for
    /// (`Retry-After`) and sent again with the same key; the replay is an
    /// ordinary success.
    #[tokio::test]
    async fn in_progress_waits_retry_after_and_the_replay_is_a_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                json(
                    409,
                    r#"{"error":{"message":"x","code":"IDEMPOTENCY_IN_PROGRESS"}}"#,
                )
                .insert_header("retry-after", "1"),
            )
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(
                json(201, r#"{"first":true}"#).insert_header("idempotent-replayed", "true"),
            )
            .mount(&server)
            .await;
        let ex = exec_at(server.address().port());
        ex.set_idempotency_v1(true);
        let t = Instant::now();
        let r = ex
            .run(&post_op(), post_call(), &none(), None)
            .await
            .expect("replayed");
        assert!(r.replayed);
        assert_eq!(&r.body[..], br#"{"first":true}"#);
        assert!(
            t.elapsed() >= Duration::from_millis(900),
            "waited Retry-After"
        );
        let reqs = server.received_requests().await.expect("r");
        assert_eq!(reqs.len(), 2);
        assert_eq!(
            reqs[0].headers.get("idempotency-key"),
            reqs[1].headers.get("idempotency-key")
        );
    }

    /// The waits are bounded by the deadline: then the outcome is unknown
    /// but keyed (sending the same key again is safe). Without the
    /// capability a 409 in progress is a plain `Busy`, not retried.
    #[tokio::test]
    async fn in_progress_waits_are_bounded_and_need_the_capability() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                json(
                    409,
                    r#"{"error":{"message":"x","code":"IDEMPOTENCY_IN_PROGRESS"}}"#,
                )
                .insert_header("retry-after", "1"),
            )
            .mount(&server)
            .await;
        let ex = exec_at(server.address().port());
        let e = ex
            .run(&post_op(), post_call(), &none(), None)
            .await
            .expect_err("busy");
        assert!(
            matches!(
                e,
                ApiError::Conflict {
                    reason: crate::errors::ConflictReason::Busy,
                    ..
                }
            ),
            "{e:?}"
        );
        assert_eq!(count(&server).await, 1);
        ex.set_idempotency_v1(true);
        let e = ex
            .run(&post_op(), post_call(), &none(), None)
            .await
            .expect_err("still running");
        assert!(
            matches!(e, ApiError::OutcomeUnknown { keyed: true, .. }),
            "{e:?}"
        );
        let n = count(&server).await - 1;
        assert!(
            (2..=1 + MAX_IN_PROGRESS_RETRIES as usize).contains(&n),
            "{n}"
        );
    }

    /// C4: a replay whose record is gone answers only the committed marker:
    /// the save is in the books, so it is `SavedUnreadable` (reload), never
    /// a failure that invites a second save.
    #[tokio::test]
    async fn a_replay_without_its_record_is_saved_unreadable() {
        let marker = r#"{"idempotency":{"state":"committed","entityType":"transaction","entityId":"5f6e3c1a-8d2b-4c7e-9a10-2b3c4d5e6f70"}}"#;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/transactions"))
            .respond_with(json(200, marker).insert_header("idempotent-replayed", "true"))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(json(200, marker))
            .mount(&server)
            .await;
        let ex = exec_at(server.address().port());
        ex.set_idempotency_v1(true);
        let (op, call) = post_to("/api/v1/transactions");
        let e = ex.run(&op, call, &none(), None).await.expect_err("marker");
        assert!(matches!(e, ApiError::SavedUnreadable { .. }), "{e:?}");
        // The same body without the replay header is an ordinary answer.
        let (op, call) = post_to("/api/v1/accounts");
        ex.run(&op, call, &none(), None).await.expect("plain");
    }

    /// C5: a key older than 20 hours is not relied on (the engine forgets
    /// keys after 24): the save is treated as unkeyed, never blind-retried.
    #[tokio::test]
    async fn an_old_key_is_not_relied_on() {
        let young = IdempotencyKey::new();
        let now = std::time::Instant::now();
        assert!(young.replayable_at(now));
        assert!(young.replayable_at(now + Duration::from_hours(19)));
        assert!(!young.replayable_at(now + KEY_REPLAY_AGE));
        let Some(old) = IdempotencyKey::aged(Duration::from_hours(21)) else {
            eprintln!("SKIPPED: the monotonic clock does not reach 21 hours back");
            return;
        };
        let server = failing_post_server(503).await;
        let ex = exec_at(server.address().port());
        ex.set_idempotency_v1(true);
        let mut call = post_call();
        call.idempotency_key = Some(old.clone());
        let e = ex
            .run(&post_op(), call, &none(), None)
            .await
            .expect_err("unknown");
        assert!(
            matches!(e, ApiError::OutcomeUnknown { keyed: false, idempotency_key: Some(ref k), .. } if *k == old),
            "{e:?}"
        );
        assert_eq!(count(&server).await, 1, "not retried");
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
