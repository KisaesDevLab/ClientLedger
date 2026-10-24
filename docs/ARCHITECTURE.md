# ClientLedger Architecture

## Process model

```
┌──────────────────────────── one PC (single-user or LAN host) ─────────────────────────────┐
│                                                                                            │
│  clientledger (Rust, eframe/egui)                                                          │
│  ├─ cl-ui ── screens, shell, theme                                                         │
│  ├─ cl-grid ── ledger grid widget                                                          │
│  ├─ cl-api ── typed HTTP client ───────────┐ loopback 127.0.0.1:<random>, launch token+JWT │
│  ├─ cl-automation ── control channel (opt-in, loopback + token file)                       │
│  └─ cl-host ── supervisor                  │                                               │
│        │ spawns + health-checks            ▼                                               │
│        ├────────► clientledger-engine  (MyBooks engine, single executable, Node SEA)       │
│        │              │  pg-boss job queue (replaces Redis/BullMQ)                         │
│        │              ▼                                                                    │
│        └────────► postgres 16 (bundled binaries, data dir in app data, port <random>)      │
│                                                                                            │
└────────────────────────────────────────────────────────────────────────────────────────────┘

LAN multi-user (P33):
  workstation clientledger ──TLS (pinned self-signed cert)──► host PC clientledger-engine
  (workstations run no Postgres or engine of their own)
```

### Why this shape

- **Engine reuse.** The MyBooks engine already implements the ledger, posting rules, AI categorization, report packs, payroll import, the QBD report-export importer, and more. ClientLedger is a new client of it, not a fork of its rules.
- **Real PostgreSQL.** The engine's Drizzle schema and migrations run unchanged. Postgres handles concurrent LAN users properly.
- **No Redis.** BullMQ is swapped for pg-boss behind the engine's queue interface (upstream change U-01), so the desktop install has two processes, not three.
- **Rust UI.** Native binary, no webview, cross-platform, permissive licenses, and the PdfCraft patterns (engine-first crates, agent control channel, cargo-deny).

## Startup sequence

1. `clientledger` acquires the single-instance lock (per OS user).
2. `cl-host` resolves the app data dir:
   - Windows `%LOCALAPPDATA%\ClientLedger`
   - macOS `~/Library/Application Support/ClientLedger`
   - Linux `$XDG_DATA_HOME/clientledger`
3. `cl-pg` checks the cluster: `initdb` on first run (locale C.UTF-8, UTF8 encoding, scram-sha-256 auth, random superuser password stored in the OS keychain), else verifies `PG_VERSION` matches the bundled major version.
4. `cl-pg` starts Postgres on a random free loopback port, waits until it accepts connections, then idempotently creates the non-superuser application role `clientledger_app` and the database `clientledger` it owns (Q-P04a). The engine never connects as the superuser.
5. `cl-host` starts `clientledger-engine` with an environment built from scratch (nothing inherited, no command-line arguments): `DATABASE_URL` (application role), `CL_MODE=desktop`, `CL_BIND=127.0.0.1:0` (the real port comes from the engine's ready line), `CL_TOKEN=<random 256-bit, new for every launch>`, `CL_DATA_DIR` and `CL_LOG_FILE` (absolute paths), `CL_REQUIRE_NON_SUPERUSER=1`, `CL_OFFLINE=auto`, plus `JWT_SECRET`, `ENCRYPTION_KEY` and `PLAID_ENCRYPTION_KEY` derived from one keychain secret. Windows runs it in a kill-on-close job object; Unix in its own process group (Q-P04d). Before the token is sent, the host checks that the engine process owns the port (Q-P04e).
6. Engine runs migrations for the open company database, then reports ready on `/health`.
7. UI shows the Open Company window (or reopens the last company).
8. Shutdown: UI asks engine to drain jobs → engine exits → `pg_ctl stop -m fast` → lock released. Crash of either child: supervisor restarts with backoff (max 3, Q-P04f), then shows a recovery dialog with log location. A second launch finds the running instance through the single-instance lock and asks it to focus (Q-P04c).

## Company files

A "company" is an engine **tenant** inside the single local database (Q-05), plus its attachments folder. Company-level backups and transfers use the engine's versioned tenant export/import (U-05); whole-cluster cold backups (`cl-pg` `dump`/`restore`, Q-P03a) are reserved for disaster recovery and PostgreSQL upgrades.

| User action | Implementation |
| --- | --- |
| New Company | engine creates a tenant and seeds the chart-of-accounts template |
| Open Company | UI session scoped to the tenant; lists load |
| Back Up | `.clbackup` = the engine tenant archive byte-for-byte (encrypted with the owner's backup passphrase from the keychain, shown once with Print/Save; manifest with engine commit, schema version, trial balance and part hashes inside), with app metadata (app version, kind, name, colour, local flags) in `manifest.host` (Q-P02i, Q-P08g) |
| Restore | verify manifest + checksums → import into a new tenant → compare trial balance → swap names; old tenant kept until the user confirms |
| Move to another computer | Back Up on this computer, Restore a backup on the other ("Make Portable Copy" was removed, Q-268; older `.clbackup` files flagged portable still restore and are never deleted by retention) |
| Full data backup | whole-database `pg_dump -Fc` of all companies (File › Utilities), for disaster recovery |
| Send to my accountant | `.clxfer` transfer file (P32) for MyBooks Online: lists, transactions, attachments, audit history, manifest, verification totals; secrets excluded |

Automatic backups: on company close (configurable), before every migration, import, restore, and app upgrade. Retention configurable (default: keep last 10 + one per month for 12 months); it only ever deletes regular backups the app recorded, never before-delete copies or older portable copies (Q-268).

## Engine contract

- The engine exposes OpenAPI 3.1 at `/openapi.json`; a copy is committed at `engine/openapi.json` per pinned engine tag.
- `cl-api` is a hand-written typed client (serde + reqwest) organized by resource, verified by contract tests against a live engine. No GPL/MPL code generators. The spec describes request schemas only in part and describes almost no responses, so generated types would carry every amount as a bare `string`; hand-written models are where exactness is kept.
- `cl-api` covers 40 engine operations (P05), listed in `crates/cl-api/operations.txt` and mirrored by `OPERATIONS` in `resources/operations.rs`. Later phases add operations through their own capability checks.
- Money crosses the boundary as decimal strings at scale 4 (Q-04, below).
- Mutations carry an `Idempotency-Key`, but the engine ignores it until U-18, so the client does not rely on it (see Idempotency and outcome reconciliation).
- Optimistic concurrency is `version` plus `If-Match` once U-17 lands. Until then the client runs a pre-save check (see Concurrency).

### What the client sees

**Transport.** Plain HTTP to the constant host `127.0.0.1` on the port the supervisor reports. reqwest is built with `default-features = false`: no TLS, no system proxy (`no_proxy()` is set), no HTTP/2, no redirects, no cookie jar. TLS and certificate pinning arrive with LAN mode (P33), which supplies its own endpoint source (Q-P05d). Nothing in `cl-api` can reach another host.

**Two layers of credentials.**

| Layer | Header | Sent on | Source |
| --- | --- | --- | --- |
| Launch token | `X-CL-Token` | every request, including `/health` and login | the supervisor (new for every engine launch) |
| User session | `Authorization: Bearer <access JWT>` | session routes | login or refresh |
| Two-step sign-in | `Authorization: Bearer <tfa token>` | `/auth/tfa/verify` only | the login answer |
| Refresh token | `Cookie: kb_refresh=…` | `/auth/refresh`, `/auth/switch-tenant`, `/auth/logout` only | the `Set-Cookie` of login, refresh or switch |

The launch token proves "this is the desktop app" and the JWT identifies the user; the engine's roles and limiters apply as usual. `Authorization`, `Cookie` and `X-CL-Token` values are marked sensitive. Every request also carries `Accept: application/json` and an `X-Request-Id`. Mutations carry `Idempotency-Key`; updates with a server version carry `If-Match`. Company routes carry `X-Company-Id`. The client never sends `Origin` (the desktop gate answers 403 `ORIGIN_NOT_ALLOWED`) and reqwest sets `Host` to `127.0.0.1:<port>` (any other host gets 421 `HOST_NOT_ALLOWED`).

**The endpoint snapshot is re-read for every attempt.** `cl-api` defines the trait `EndpointSource { fn current() -> EndpointState }` and never caches the answer. The host side implements it over `Supervisor::snapshot()`, which returns status, endpoint and generation under one lock so they cannot disagree. A new engine process means a new port, a new launch token and a new generation, and the next attempt picks all three up.

| Supervisor snapshot | `EndpointState` |
| --- | --- |
| `Running` with an endpoint | `Ready { port, token, generation }` |
| `Running` without an endpoint (caught mid-transition) | `Restarting` |
| `Starting` | `Starting` |
| `Restarting` | `Restarting` |
| `Fault`, `Stopping`, `Stopped` | `Down { permanent: true }` |

While the state is `Starting` or `Restarting`, a request polls (every 150 ms by default) until its deadline. `Down { permanent: true }` fails at once with `EngineUnavailable { permanent: true }`. A generation change is logged once at info level and nothing else is reset: sessions survive an engine restart because the JWT secret is derived from the stable master secret and sessions live in the database (Q-P04a). The supervisor publishes an endpoint only after it has checked that the engine process owns the port, and `engine_info` and `initialize_engine` run the same check before they send the token, which closes Q-P04n. `cl-api` does not repeat the check.

**Time limits.** One logical request has a per-attempt limit and an overall deadline that includes waiting for the engine and any retries.

| Class | Per attempt | Overall |
| --- | --- | --- |
| `/health`, `/desktop/info` | 3 s | 5 s |
| Reads | 15 s | 30 s |
| Writes | 30 s | 50 s (the deadline bounds only the wait before any byte is sent) |
| Login, refresh, switch-tenant, TFA | 15 s | 20 s |
| Create company | 120 s | 140 s |

Opening a connection is limited to 2 s, below every attempt limit, so a refused connect (slow on Windows) still counts as "not sent". A killed engine stays published until the supervisor's next check, so a connect refused on an endpoint that is still the same generation first waits (up to 5 s, `refused_settle`, within the deadline, once per generation) for the endpoint to change, and only then falls back to the jittered retries; a request issued just after the engine died therefore waits for the next generation instead of using up its retries on the dead port. `Never` routes stay single-shot. Response bodies are read in chunks and capped at 32 MiB. Error bodies are `{ error: { message, code?, details?, feature? } }`; a body that is not that shape counts as having no code. Codes are kept only if they match `[A-Z0-9_]{1,64}`.

**Company scope.** A session starts in the owner's hidden *workspace* tenant. The engine fences the workspace: only `/api/v1/auth/`, `/api/v1/tenants`, `/api/v1/tenant-archives` and `/api/v1/desktop/` pass, and everything else is 409 `WORKSPACE_TENANT`. `cl-api` refuses company routes in the workspace locally with `NoCompanyOpen` and sends no request (a 409 `WORKSPACE_TENANT` maps to the same error). `open_company` posts `/auth/switch-tenant` with the refresh cookie, then reads `GET /api/v1/company` to learn the company id, then sets the scope to `Company { tenant_id, company_id }` and bumps `scope_epoch`. From then on every company route sends `X-Company-Id` explicitly instead of relying on the engine's first-company fallback. If the company id cannot be read, the session switches back to the workspace and returns the error. `close_company` switches back to the workspace. The companies list comes from `GET /auth/me` (`accessibleTenants` minus the workspace id from `/desktop/info`); the `GET /api/v1/tenants` path is used once the engine advertises `tenants-list-v1` (Q-P05y). A ClientLedger company is an engine tenant; `cl-api` does not expose `/company/create` or the tenant delete route (Q-P05n). `create_company` is never retried, because a retry could create a second company.

**Capabilities.** `GET /api/v1/desktop/info` returns `capabilities: string[]` (introduced by U-21; a missing array means none). `Client::desktop_info` records them. The names `cl-api` knows are `record-version-v1`, `idempotency-v1`, `money-strict-v1`, `error-codes-v1`, `tenants-list-v1` and `closing-password-v1`. Only `idempotency-v1` and `closing-password-v1` change client behavior today; the others switch on contract-test branches. The pinned engine advertises none of them yet.

**Errors.** Every failure is an `ApiError` variant. The engine's message text can hold names and amounts (for example "debits (x) != credits (y)"), so it lives in a `UserText` whose `Debug` prints only a length and which has no `Display`. `ApiError::Display` is technical and carries only the status, code and request id; it is the only thing logs and "Copy details" see. `ApiError` implements `OwnerFacing`, mapping each variant to i18n keys. The mapping, one test row per line:

| Status and code | Becomes |
| --- | --- |
| 400 `VALIDATION_ERROR` | `Validation` (field path from `details[]`, joined with `.`) |
| 400 with no code (the balance error), 422 | `Validation` with no code |
| 400 `INVALID_JSON`, 403 `ORIGIN_NOT_ALLOWED`, 422 `IDEMPOTENCY_KEY_REUSED` | `Protocol` (a client bug) |
| 400 on a TFA route with `locked_until` | `Unauthorized(AccountLocked)` |
| 401 `INVALID_CREDENTIALS`, `ADMIN_SESSION_EXPIRED` | `Unauthorized(InvalidCredentials / ReauthRequired)` |
| 401 `DESKTOP_TOKEN_REQUIRED`, 421 | re-read the endpoint and replay once, then `EngineUnavailable` |
| 401 `UNAUTHORIZED` | refresh flow (see Sessions and refresh) |
| 401 with no code or not JSON, any route that is not refresh or TFA | not the engine's `authenticate`: never refresh; see Sessions and refresh |
| 403 `ACCOUNT_LOCKED`, `ACCOUNT_DEACTIVATED` | `Unauthorized(AccountLocked / Deactivated)` |
| 403 other, 404 `NOT_AVAILABLE_IN_DESKTOP`, 404 other | `Forbidden`, `Unsupported`, `NotFound` |
| 409 `WORKSPACE_TENANT` | `NoCompanyOpen` |
| 409 `STALE` | `Conflict { Stale, current_version: Server(n) }` |
| 409 `TAG_IN_USE`, `EMAIL_EXISTS`/`ACCOUNT_NUMBER_EXISTS`, `TENANT_BUSY`/`IDEMPOTENCY_IN_PROGRESS`, other | `Conflict { InUse / AlreadyExists / Busy / Unspecified }` |
| 413 | `PayloadTooLarge` |
| 423 `TB_PERIOD_LOCKED` | `ClosedPeriod` (see Closed periods) |
| 429 | `RateLimited { retry_after }` |
| 503 `OFFLINE_UNAVAILABLE` | `OfflineUnavailable { feature }`, never retried |
| 503 `EGRESS_BLOCKED` | `FeatureNotEnabled { feature }`, never retried |
| other 5xx | `Server` (reads are retried on 502, 503 and 504) |

The queue adds `StaleScope`, `Cancelled` and `Internal`; a failed local check is `Local(..)` and nothing is sent. A save the engine answered with 2xx whose answer cannot be read (model drift, an enum value this version does not know) is `SavedUnreadable` (keys `error.api.saved_unreadable` and `fix.api.reload_to_see`, wording "Saved — reload to see it"), never the generic internal error, because the entry is recorded and re-entering it would duplicate it.

### Money on the wire

`cl-core` owns exact amounts. There are three fixed-scale types over `rust_decimal`, each mirroring one engine column:

| Type | Engine column | Integer digits | Decimals |
| --- | --- | --- | --- |
| `Money` | `decimal(19,4)` | 15 | up to 4 |
| `Quantity` | `decimal(12,4)` | 8 | up to 4 |
| `Rate` | `decimal(5,4)` | 1 | up to 4 |

- **Wire form out:** `to_wire()` always writes the canonical string with exactly four decimals (`"12.5000"`). That is also what the engine returns, so contract tests compare exact strings.
- **Wire form in:** `^-?\d{1,N}(\.\d{1,4})?$`, delivered as a JSON **string**. A JSON number, `NaN`, `Infinity`, an exponent, a `+`, spaces or commas is a `Decode` error, never coerced. Serde accepts only string tokens for these types.
- **Zero never has a sign.** Every constructor and every arithmetic result is rebuilt through one normalizing step, so `to_wire()` never writes `-0.0000`. Input with trailing zeros beyond the fourth place (`1.50000`) is accepted as `1.5`; a non-zero fifth decimal is rejected.
- **Arithmetic is checked and exact.** `checked_add`, `checked_sub`, `checked_neg`, `sum` return an error on leaving the column's range. `allocate` and `split` use largest-remainder so the parts sum exactly to the total. The only rounding is `mul_round` and `mul_rate_round`, which are for previews (half away from zero, the PostgreSQL rule); the engine's value always replaces a preview.
- **Owner input** (`parse_user`) is a separate, stricter grammar. It refuses ambiguous separators (`1,23`, `1.234,56`, `1 234`), more decimals than the field allows (2 for ordinary money, 4 for prices, quantities and rates) and more integer digits than the column holds. It never rounds and never guesses.
- **Display** (`format`) shows at least two decimals for `Money` and adds the third and fourth only when they are non-zero, so an exact value is never hidden or rounded (Q-P05l; DESIGN.md says two decimals). The types have no `Display`, so an amount is not formatted into a log line by accident.
- **Balances** reported by the engine are shown as received and never recomputed or cached beyond a screen's lifetime.

**Engine report numbers** are JSON numbers today (some totals are float sums inside the engine) until U-14. `cl-api` reads them as `ReportAmount`, which keeps the exact JSON text through `serde_json::value::RawValue` (the `raw_value` feature only; `arbitrary_precision` would change number handling for the whole dependency graph). `ReportAmount` has no `PartialEq`, `Ord`, `Hash`, arithmetic or conversion to `Money`, so a report value can never feed a posting or an equality check. `display(dp)` rounds half away from zero and is approximate until U-14 (a text like `0.30000000000000004` shows as `0.30`).

**Journal entries are balanced exactly on the client.** The engine accepts an entry when debits and credits differ by no more than 0.0001 (`ledger.service.ts`, on create and on update), and its money fields are unvalidated strings. So an entry off by 0.0001 would be stored unbalanced. `NewJournalEntry::new` therefore refuses, before anything is sent: fewer than 2 or more than 500 lines, any line amount that is not greater than zero, and any difference at all between total debits and total credits. A line is either a debit or a credit by construction, and the body carries both sides in canonical form (the unused side as `"0.0000"`). The engine stays the authority on posting rules (non-negotiable 7); these are input checks that close its tolerance until U-19 tightens it. The contract suite pins the current engine behavior with a deliberate probe (it accepts a raw entry off by 0.0001, then voids it, in a throwaway second company) and expects a 400 instead once `money-strict-v1` is advertised.

**No floating point anywhere in the money path, enforced three ways.**

1. `#![deny(clippy::float_arithmetic, clippy::float_cmp, clippy::cast_precision_loss)]` in `cl-core` and `cl-api`, enforced by `clippy -D warnings`.
2. A `clippy.toml` in each of those two crates lists the `disallowed-methods` for float conversions (`Decimal::from_f32_retain`, `from_f64_retain`, `as_f64`, `ToPrimitive::to_f32/to_f64`, `FromPrimitive::from_f32/from_f64`, `serde_json::Number::as_f64/from_f64`, `Value::as_f64`). Per-crate placement leaves crates that legitimately use `f32` for egui alone. `tests/float_ban.rs` in each crate calls every banned method under `#[expect(clippy::disallowed_methods)]`, so a path that stops resolving leaves an unfulfilled expectation and fails the build (Q-P05u). Conversions that are trait impls on primitives or `as` casts cannot be listed, which is why the third layer exists.
3. `cargo xtask nofloat` parses every `.rs` file under `crates/cl-core/src` and `crates/cl-api/src` with `syn` and fails on any `f32`/`f64` type, float literal, `to_/from_/as_f32/f64` method name, or `ToPrimitive`/`FromPrimitive` identifier, including inside macro token streams. Comments and strings are not scanned, so "12.50" in a doc comment is fine. The scan has a self-test (a bad fixture that must produce exactly the expected findings, a good fixture that must produce none) and an allowlist file that is empty. It runs in `cargo xtask ci`.

### Sessions and refresh

- The access token is a JWT. `cl-api` reads its `exp` by base64url-decoding the payload without verifying the signature (only the engine verifies; Q-P05p), and uses it only to refresh early and to decide whether a mutation's 401 came from our own expired token. This relies on the client and engine sharing a clock, which is true on one machine; LAN mode (P33) revisits it.
- The refresh token exists only as the `kb_refresh` cookie. `cl-api` parses `Set-Cookie` itself: only that cookie, value `[A-Za-z0-9_-]{16,512}`, and an empty value or `Max-Age=0` clears it. It keeps the value in a zeroized `Secret`, never persists it (restarting the app means signing in again), and sends it by hand only on the three routes in the table above. There is no cookie jar and no change to the engine's `Secure` flag, because the engine reads the cookie by hand and `/auth/switch-tenant` reads the old token only from the cookie (Q-P05c).
- **Refresh rotates on every call and a reused token is rejected, so refresh is single-flight.** One async mutex guards every call that rotates the token. Each session has an `auth_epoch`; a waiter that sees the epoch has moved uses the token someone else just fetched instead of calling refresh again. A refresh happens (a) proactively, when under 60 s remain on the access token, and (b) reactively, on a 401 that is provably from the engine's `authenticate`.
- **A reactive refresh needs all of:** the body is the engine JSON shape with code `UNAUTHORIZED`; the endpoint generation is still the one the 401 came from; and the request is a GET, or our own clock says the token had expired (so a mutation is replayed only when `authenticate` refused it before any handler ran). The request is then replayed once. A second 401 is `Unauthorized(SessionExpired)`. A 401 after a generation change re-reads the endpoint and replays once without refreshing.
- **A 401 with no code, or with a body that is not the engine JSON, means "not the engine".** The engine codes every `authenticate` 401, and emits code-less 401s only from `/auth/refresh` and the TFA routes. Anywhere else, a code-less 401 could come from another program that took over the port after a crash. In that case `cl-api` never sends the refresh cookie and never refreshes; it re-reads the endpoint, replays once only if the endpoint changed and is `Ready`, and otherwise returns `EngineUnavailable { permanent: false }`. Any 401 from `/auth/refresh` itself ends the session as `SessionExpired`, and the UI asks for the password.
- Login, TFA verify, refresh, logout, switch-tenant and create-tenant are never retried (rate limiters, token rotation). The harness budget keeps rate-limited auth calls under the engine's 10 per minute per address. The engine keeps at most 3 sessions per user and evicts the oldest, and the app signs out on exit so LAN sessions are not evicted later.
- A two-step sign-in returns a `TfaChallenge` holding the TFA token; its `verify` consumes it. A wrong code means signing in again. TFA is covered by stub tests only; the live test belongs to P29.

### Concurrency: `Conflict` now, record versions later

Until the engine carries record versions (U-17, capability `record-version-v1`), `cl-api` enforces "changed elsewhere" itself.

- Every `get(id)` returns `Versioned<T> { record, version }`. The version is `RecordVersion::Server(n)` if the engine sent a `version`, otherwise `RecordVersion::Fingerprint`, a SHA-256 of the canonical JSON (sorted keys, no whitespace) of the **editable fields** the record's edit form writes. The field list is a `const` next to each model and unit-tested.
- Left out of every fingerprint: server-maintained values (account `balance`, `updatedAt`, `createdAt`), joined display names, and journal-line ids (the engine deletes and recreates every line on update, so a save with identical lines must give an identical fingerprint). Journal-line `lineOrder`, account, amounts, description, tag and contact are in, and lines are hashed in `lineOrder` order. So a posting to an account does not conflict with a rename of that account.
- `update(id, expected, body)` with a `Fingerprint` re-reads the record first. A different fingerprint returns `Conflict { Stale, current_version: Fingerprint(new) }` and **no PUT is sent**; an equal one sends the PUT without `If-Match`. With a `Server(n)` version it sends `If-Match: "n"` and maps 409 `STALE` to `Conflict { Stale, Server(current) }`.
- **Limits of the interim check.** It costs two round trips (GET then PUT) per update. It has a residual race of milliseconds between the GET and the PUT, acceptable for one desktop user. It cannot see a change that is outside the editable fields, for example a line cleared in an in-progress reconciliation (the engine protects lines cleared in a completed one). LAN mode (P33) is blocked until U-17 merges.
- The UI keeps the `Versioned` with its form. On `Conflict { Stale }` it shows "changed by another window or user" with Reload or Overwrite; Overwrite is an explicit re-save with the new version. The engine takes a row lock inside `PUT /transactions/:id`, so concurrent writers are serialized and the last one wins unless the client check above stops it.
- `GET /api/v1/ledger-version` returns a stamp that moves when the ledger changes. Registers poll it to know when to refetch. It is never a per-record version.

### Idempotency and outcome reconciliation

The engine ignores `Idempotency-Key` until U-18, so sending it is harmless and also no protection: re-sending a save that did commit posts it twice. The client therefore decides how to retry from what it can prove.

| Class | Operations | Automatic retry |
| --- | --- | --- |
| `Idempotent` | GET | timeouts, connect errors and 502/503/504; up to 3 attempts, full-jitter backoff, within the deadline. 429 waits for `Retry-After`. |
| `PreHandlerOnly` | every mutation | only when the request provably never reached a handler: connect refused, the desktop-gate refusal (401 `DESKTOP_TOKEN_REQUIRED` or 421) replayed once on the re-read endpoint, or a 401 on a token our clock says had expired. Anything after bytes may have been written is *ambiguous*. |
| `KeyedOnce` | mutations, once the engine advertises `idempotency-v1` | like `Idempotent`, always with the same key; a reply with `Idempotent-Replayed: true` counts as success. Reconciliation is skipped. |
| `Never` | login, TFA verify, refresh, logout, switch-tenant, create-tenant | none |

One `IdempotencyKey` (a UUIDv4) is made per logical user action, so the form's save keeps its key across retries.

**Why two connection pools.** The engine's listener sets no keep-alive timeout, and Node closes idle sockets after about 5 s. A request written onto a socket the server is closing fails *after* bytes were sent, which would look like an ambiguous save. Reads use a pool that drops idle connections after 2 s (a read that hits the race is simply retried). Every mutation opens a fresh connection, so a post-write connection error on a mutation is a real ambiguity.

**Ambiguous means reconcile, not retry.** A timeout, a connection lost after writing, or a 5xx on a mutation goes to outcome reconciliation (`resources/reconcile.rs`) before the owner hears anything. Every mutation carries a probe built just before it is sent: the ledger-version stamp (postings only, read with one extra GET), the client time, and a matcher that recognizes this write.

- **The stamp is a commit detector for postings.** PostgreSQL triggers bump it inside the writing transaction on every `journal_lines` insert, update or delete and on key `transactions` columns, so it moves exactly when a posting commits. Master-data writes do not move it, so they use matching only.
- **"Settled".** An unchanged stamp proves "not committed" only if the old request can no longer commit. The engine sets no request, statement or idle-transaction timeout, so a slow handler may still commit after the client gave up. The request is *settled* only when the endpoint generation has moved past the one the attempt used (the old process is gone and PostgreSQL rolled back its open transaction). A timeout or reset on the same generation is not settled.
- **Algorithm** (budget 15 s, polled about every 500 ms): wait for a `Ready` endpoint; if settled and the stamp is unchanged, the result is `NotCommitted`; otherwise search with the matcher.
  - *Create:* a journal entry is found by listing entries for its date and reading each candidate created at most 2 s before the send, then comparing editable fingerprints. Master data is found by exact name. Exactly one match is `Committed`. No candidate at all and settled is `NotCommitted`. Any candidate that is not ours, or two identical matches, is `Unresolved`: a candidate whose fingerprint differs (for instance because the engine defaulted a field) must never allow a duplicate (Q-P05z).
  - *Update:* fingerprint equals the requested one is `Committed`; equals the pre-image and settled is `NotCommitted`; anything else is `Unresolved`.
  - *Void or deactivate:* the status or `isActive` field has the new value is `Committed`. *Delete:* a 404 is `Committed`.
  - Not settled with nothing found keeps polling until the budget ends, then `Unresolved`.
- `Committed` returns the found record as the call's success. `NotCommitted` allows **one** automatic retry with the same key (with a fresh wait window); a second ambiguous failure is never retried again. `Unresolved` returns `OutcomeUnknown { request_id, idempotency_key }`. The tracing output carries only the outcome class.
- If the stamp read fails, the write still goes out and only the matcher can resolve it. With `idempotency-v1` the probe reads no stamp and reconciliation does not run. The `createdAt` window compares the local clock with the engine's, which is sound on one machine; LAN mode (P33) uses U-18 instead.

**`OutcomeUnknown` UX contract (for P06 and the forms).** Say, in plain words, that the save could not be confirmed and to check the register before trying again (Q-P05f). The register is refreshed through the ledger-version stamp. The dialog offers **Check again**, which re-runs reconciliation with the stored pre-image, and enables **Try again** (same key) only when that returns `NotCommitted`. Never offer a blind re-send. `cl-api` returns the request id and key for this, but it does not yet expose a standalone "check again" call; the owner-facing keys `error.api.outcome_unknown` and `fix.api.check_again` exist. Once U-18 is advertised, a same-key retry is safe by itself and the restriction lifts. A case that stays unresolved on the same generation is a candidate for an engine-side request deadline (noted under U-18).

### Closed periods

- A save dated on or before the closing date is refused by the engine with 423 `TB_PERIOD_LOCKED`, which becomes `ClosedPeriod { lock_date, can_override, password_required, message }`. `lock_date` is read only from a plain `YYYY-MM-DD`; anything else becomes `None`, so a time zone can never shift the date. `can_override` is information only. When an entry is edited the engine checks its original date and, if the date changes, the new date too, so moving an entry out of a closed period is refused as well as moving one in. The contract suite exercises a new entry and a move *into* the closed period; the move *out* is read from the engine source, not tested.
- **No request type can carry `overrideConfirmed` or `lockDate`, and nothing in the typed API writes the closing date.** The engine reads `overrideConfirmed` from the request body, so the safe design is that the client never writes that key. `tests/api_surface.rs` checks it three ways: a source scan (the override key is named nowhere; the lock date appears only in read models), a compile-time check that `CompanySettings` and `ClosedPeriod` do not implement `Serialize`, and a key scan of every public request type plus a journal-entry proptest. The engine accepts `lockDate` on `PUT /company/settings`, so any future typed settings writer must omit it until U-15. Only the contract tests set it, through the raw path that exists under `test-support`.
- The slot for a future override is `PostingOptions { closed_period: Option<ClosedPeriodAuthorization> }`, where the authorization holds a closing-date password in a `Secret` behind a private field. The body gets `closingPassword` **only** when the engine advertises `closing-password-v1` (U-15). Without that, giving a password fails locally with `Local(ClosedPeriodOverrideUnavailable)` and nothing is sent. Until U-15 lands, edits inside a closed period are impossible through ClientLedger. Whether an override with no password but an explicit confirmation is allowed when no closing password is set is an open owner decision for P08 (Q-P05g).

### The UI request queue

`ApiQueue` (`queue.rs`) is how the UI talks to the engine without ever blocking a frame. It owns a multi-thread tokio runtime (2 worker threads named `cl-api`) off the UI thread and has no egui dependency; `cl-ui` hands in its repaint callback as `notify`.

- `queue.group()` returns a `RequestGroup`, one per window or tab. `group.read(&scope, f)`, `group.prefetch(&scope, f)` and `group.write(&scope, label, f)` spawn the job and return a `Pending<T>` handle at once. The window stores the handle and calls `try_take()` every frame, which never blocks. When a job finishes the queue calls `notify`, so the UI repaints without polling at a high rate. `scope` is a `ScopeSource`: a `Session` (carries `scope_epoch` and the open company) or a `Client` (before sign-in).
- **Limits:** at most 256 outstanding jobs (`QueueFull` beyond), 8 concurrent engine requests, and a separate lane of 2 for prefetch. Request coalescing is not implemented; it is deferred to P12 (Q-P05z).
- **Cancelling.** Closing a window (or `cancel()`, or dropping the group) aborts its reads, which resolve to `Cancelled`. A write is never aborted, because aborting would make the outcome unknown. It finishes, and its result goes to `take_orphans()` as an `OrphanOutcome { label, company, result, stale_scope }`. The label is an i18n key plus the save's idempotency key, never an amount, name or memo, so the shell can say "A save from a closed window failed" and offer "check".
- **`scope_epoch`, pinning and orphans.** The epoch moves on every company switch. Every queued job runs on a session view pinned to the epoch and company open when it was queued (`Session::pinned`; the multi-step writes and the outcome probe pin themselves too). Before *every* attempt of a pinned company request, the epoch is compared under the session state lock: if it moved and nothing of this save can have reached the engine yet, the request fails with `StaleScope` and nothing is sent; if an earlier attempt may have reached the engine (timeout, broken connection, 5xx), it is `OutcomeUnknown`. Outcome reconciliation stops as `Unresolved` at a switch, so it never reads the other company's ledger. A *read* that finishes under an older epoch resolves to `StaleScope`, and the UI discards it. A *write* under an older epoch is never dropped: it resolves for its `Pending` *and* is copied to the orphans with the company it was queued and sent in and `stale_scope = true`. An attempt already on the wire at the moment of the switch carries the old company's token and company id together, so it can only commit in the old company.
- **A panicked write is `OutcomeUnknown`**, never a silently closed handle. A panicked read is `Internal`.
- `pending_writes()` counts writes still running. P08's company switch must refuse or wait while it is above zero.
- **Shutdown order.** `ApiQueue::shutdown(deadline, session)` (call it from a thread that is not inside a tokio runtime): stop intake (`QueueClosed`) and cancel reads; wait for in-flight writes up to `deadline`, with `pending_writes()` available so the shell can show "Finishing 1 save…"; return a `ShutdownReport { completed, outcome_unknown, abandoned }` where `abandoned` lists the labels of writes still running at the deadline; sign the session out (best effort, 2 s); stop the runtime (2 s). Only then does the shell call `Supervisor::stop`. On Windows the supervisor kills the engine immediately (Q-P04d), so stopping it earlier would turn every save in flight into an unknown. Dropping the queue without `shutdown` logs a warning and falls back to stopping the runtime.
- **Wiring (P06).** At startup: `Supervisor::spawn`, then a newtype over it that implements `EndpointSource`, then `Client::new`, then `ApiQueue::start(repaint)`. The status bar reads `HostStatus` plus `Client::health()` for online state; `online: None` means unknown, not offline.

### Testing and drift detection

| Layer | Where | Needs an engine | Covers |
| --- | --- | --- | --- |
| Unit and property | next to the code; `crates/cl-core/tests/*` (`money_props`, `dates_props`, `fiscal_props`, `float_ban`) | no | money round trips, no drift, allocation, input rejection, calendars, JWT and cookie parsing, the error mapping table, retry classes, fingerprints, the queue, reconciliation hooks |
| Stub (wiremock) | `crates/cl-api/tests/stub/` plus unit tests in `src/` | no, all OSes | the public session API; transport headers, endpoint re-read, idle-socket race, 401 handling, retry and reconciliation matrices, the `STALE` contract |
| Surface | `tests/api_surface.rs`, `tests/float_ban.rs` | no | no override or lock-date key; every banned float method fires |
| Spec fixtures | `tests/spec_fixtures.rs`, `cargo xtask api-spec --check` | needs `engine/` checked out | the OpenAPI hash, covered-operations table and engine Zod hashes |
| Contract | `tests/contract/` (one binary), `tests/contract_offline.rs` | yes, real PostgreSQL and engine | every covered resource, money, errors, conflicts, closed periods, sessions, logging, restart |
| Network probe | `tests/network_probe.rs` | no, runs only with `CL_NETWORK_BLOCKED=1` | proves the blocked CI run is really blocked |

**The contract harness** starts the real PostgreSQL and engine through `cl-host` (a **dev**-dependency; `cl-api` also gets `cl-pg` that way). The layering rule in `CLAUDE.md` and the `xtask` layering test allow exactly those two dev edges, and `cl-api`'s runtime dependencies stay `cl-core` only (Q-P05b). The `test-support` cargo feature (raw requests, a request observer that can count attempts and drop a reply, session token overrides) is refused by `xtask` layering on any app, any non-dev edge and any `default` feature.

The contract binaries use their own runner (`harness = false`), so the order, the budgets and the teardown are controlled:

- One engine and one fresh cluster per binary, in a scratch folder (`CL_API_KEEP=1` keeps it). The supervisor is always stopped at the end, even if a test panics, so no engine or postgres process outlives the run.
- A missing engine or PostgreSQL binary prints `SKIPPED` locally and fails when `CL_API_REQUIRE=1` (set in CI).
- Boot has a time budget (`CL_API_BOOT_BUDGET_SECS`, default 90); each test has a budget (`CL_API_TEST_BUDGET_SECS`, default 20) and fails on overrun.
- At most three live sessions per binary (the engine evicts the oldest beyond three), and the observer fails the binary after more than 8 calls to the rate-limited auth routes. Names carry a random suffix and list assertions filter by search or id, so tests share one company safely; tests that change company-wide state (the closing date, the money probe) use a second company.
- `contract_offline` is a second engine started with `CL_OFFLINE=on` (`OfflineSetting::On`). It covers `OFFLINE_UNAVAILABLE` for `test-smtp` in under 2 s with one attempt, core bookkeeping working offline with `/health` reporting `online: false`, reads resuming after a killed engine with no new login, a write waiting for the new generation and being sent once, and the "engine dies after commit" case reconciling to exactly one posting.
- A `logging` test runs with canary amounts, memo, email, password and tokens and asserts none appear in captured `tracing` output or in any `ApiError` `Display` or `Debug`.

**PENDING by capability.** Tests for engine features that do not exist yet are written against the documented contract. `h.require_cap(name, "U-xx")` returns true and the strict branch runs when the engine advertises the capability. Otherwise it prints `PENDING(U-xx)`, records it in `target/<binary>-pending.txt`, and the test asserts the engine's *current* behavior for that capability state (so the probes are deterministic, not "pass either way"). Setting `CL_API_REQUIRE_CAPS=a,b` turns a missing capability into a failure; CI adds a capability to that list when its U-item merges. Today's probes:

| Capability | U-item | Without it, the test asserts |
| --- | --- | --- |
| `record-version-v1` | U-17 | the live server 409 test is skipped; the fingerprint conflict test runs |
| `idempotency-v1` | U-18 | the same key sent twice posts twice |
| `money-strict-v1` | U-19 | a raw entry off by 0.0001 is accepted |
| `error-codes-v1` | U-21 | the unbalanced-entry 400 has no code |
| `je-line-contact-v1` | U-22 | the engine drops a line's `contactId` |
| `exact-money-v1` | U-14 | report amounts are JSON numbers |

**Drift detection.** Three fixtures in `crates/cl-api/tests/fixtures/` are regenerated by `cargo xtask api-spec --update` (which prints what changed) and checked by `--check` inside `cargo xtask ci`: `openapi.sha256` (hash of the canonical, key-sorted spec), `covered_operations.json` (a hash per covered operation's parameters and request schema) and `engine_schemas.sha256` (hashes of the engine Zod source files behind the covered request shapes, listed in `operations.txt`). A mismatch fails until the fixtures are regenerated and the diff is reviewed against the models. The checks need the engine checkout; without it they skip, and fail under `CL_API_REQUIRE=1`. The spec has a known gap: the journal-entry request bodies, the list query parameters and almost all responses are not described in it, so the per-operation hashes are empty for the most money-critical operations. Two guards cover that: the Zod-source hashes, and the **field census** contract test, which fetches the raw JSON of each covered response and asserts every key a model declares is present (a rename cannot silently become `None`) and every money, quantity and rate key is a JSON string. A live test also asserts the live spec's hash equals the fixture. U-21 is meant to register the per-`txnType` request schemas so the per-operation hashes become meaningful.

**CI.** All gates run through `cargo xtask ci`: fmt, clippy `-D warnings`, tests, `cargo deny`, licenses, layering, `nofloat` and `api-spec --check`.

| Job | Runs | Does |
| --- | --- | --- |
| `ci (ubuntu-22.04)` | every push | fetches the pinned PostgreSQL, then `cargo xtask ci` |
| `ci (windows-latest, macos-14, macos-15-intel)` | pull requests | the same |
| `engine (ubuntu-22.04, windows-latest, macos-14, macos-15-intel)` | pull requests and `main` | builds and smoke-tests the engine, runs the real-engine `cl-host` tests, `api-spec --check`, then `cargo test -p cl-api --test contract --test contract_offline --test spec_fixtures` with `CL_API_REQUIRE=1` |
| network-blocked run (steps of the Linux `engine` job) | pull requests and `main` | after every download and build, creates a non-root user `cltest`, adds `iptables` and `ip6tables` owner-match rules that reject all of that user's non-loopback traffic, runs `network_probe` first (it must print `blocked OK`), then runs `contract` and `contract_offline` as `cltest` with `CL_NETWORK_BLOCKED=1`; the rules and user are removed in an `if: always()` step |
| `coverage` | every push and pull request | `cargo llvm-cov --workspace --exclude xtask --summary-only --fail-under-lines 80` (Q-116) |

The block runs as a non-root user because `initdb` and `postgres` refuse to run as root, and it blocks only that user so the runner's own connection survives. DNS (UDP and TCP port 53 on any interface, so also the local stub resolver on 127.0.0.53) is rejected for that user as well. The probe checks that connects to `1.1.1.1:443` and its IPv6 twin are rejected quickly (a silent drop would not prove the rule works), that a public name does not resolve, and that loopback still connects. Checkouts use `persist-credentials: false`. The network block exists on Linux x64 only; Windows and macOS rely on the client design (constant `127.0.0.1` host, `no_proxy()`, no redirects) plus the Linux proof (Q-P05j). Making `coverage` a required status check on `main` is an owner action (Q-P05i). As configured, the `coverage` job fetches PostgreSQL but not the engine, so the contract suites skip there and do not count toward the 80%; the stub, unit, property and `cl-host` tests do.

### Engine items the client waits on

None of these is advertised by the pinned engine. Until each lands, `cl-api` ships against the documented contract and the matching test is PENDING. Each item is an engine pull request on its own `clientledger/<feature>` branch, gated or additive so the server product's defaults do not change, and each advertises a capability string in `/desktop/info`. They are built in the order shown (U-17 starts only after its Revision 2 design is approved, Q-P05r).

| ID | What | Capability | What the client does until then | Needed by | Status |
| --- | --- | --- | --- | --- | --- |
| U-21 | Stable error codes (`TXN_UNBALANCED`, `TXN_ZERO_AMOUNT`, `TXN_VOID_IMMUTABLE`, `TXN_RECONCILED_LOCKED`, `ACCOUNT_NONZERO_BALANCE`, `ACCOUNT_SYSTEM`, `ACCOUNT_OUT_OF_SCOPE`, `UNKNOWN_TXN_TYPE`, `TAG_NAME_EXISTS`, and the refresh/TFA token codes); introduces the `capabilities` array; registers per-`txnType` request and list query schemas in the OpenAPI generator | `error-codes-v1` | maps the code-less 400 to `Validation` with no code | P15 (journal form errors); first in order | Planned |
| U-17 | Optimistic concurrency: a `version` column on `transactions`, `accounts`, `contacts`, `items`, `tags` and `companies`; `If-Match` on updates, 409 `STALE` with `currentVersion`; triggers bump the parent on any `journal_lines` change; explicit bumps where a trigger cannot see (reconcile clearing, tag routes); compare-and-set for master data; `If-Match: *` means "must exist" | `record-version-v1` | fingerprint pre-save check | P12/P13 (strongly), P27, **P33 (hard)** | Planned; design must be approved first |
| U-19 | Money wire hardening: strict money, quantity, rate and date strings; no negative or two-sided journal lines; exact balance after rounding each line to 4 places (replaces the 0.0001 tolerance); bad input is 400 instead of 500 | `money-strict-v1` | refuses these inputs locally | P12 | Planned (after U-17, they touch the same posting code) |
| U-18 | Request idempotency: `Idempotency-Key` on posting and master-data creates and on create-tenant; stored responses replayed with `Idempotent-Replayed: true`; 422 `IDEMPOTENCY_KEY_REUSED`; 409 `IDEMPOTENCY_IN_PROGRESS` | `idempotency-v1` | reconciliation instead of safe retry | before the first posting form (P12/P13) | Planned |
| U-22 | Journal-line `contactId` accepted and stored | `je-line-contact-v1` | the engine drops the field; no line contact | P15 | Planned |
| U-14 | Money as exact decimal strings beyond the core resources (reports, account-balance helpers, 1099 corrections, batch and daily-sales totals); widened from the earlier reports-only item. The scout's U-20 is folded into it | `exact-money-v1` | `ReportAmount` (display only, approximate) | P12 (if register balances use these paths), P16, P28 (blocked on it) | Planned |
| U-15 | Closing-date password: accept `closingPassword` on posting routes; also covers `lockDate` on `PUT /company/settings`, which the engine accepts without a password today | `closing-password-v1` | no override is possible; `Local(ClosedPeriodOverrideUnavailable)` | P08 / P29 | Planned |

`GET /api/v1/tenants` (E5a, `tenants-list-v1`) will replace the `/auth/me` companies list when it is pinned. U-23 is not needed (the refresh cookie is sent by hand, Q-P05c), and U-24 is E5a.

## The ledger grid (cl-grid, P10)

One widget for every line grid: split lines, registers, Pay Bills-style fixed lists. Decisions are Q-P10a-w.

- **Layers.** `core/` is egui-free and pure: row model (`RowModel`, `EditableModel`, `ModelRef`), `RowKey`s and selection, the integer row-anchored viewport, and the editing state machine (`Core::handle(GridInput) -> Effects`). `view/` and `editors/` draw it with egui. `sequencer` is an egui plugin that splits a frame's input at navigation keys, shortcuts and pointer presses, so batched input (remote desktop, scanners) is applied in order.
- **Presets.** `GridSpec::line_entry` (entry line, insert/delete), `browse` (registers), `fixed_edit` (fixed rows, editable cells).
- **Focus.** One egui focus id per grid; cells are click targets with explicit AccessKit nodes. Commits happen on cell exit, each one `FormState::edit_step`; the editor reloads when the model's revision changes (undo, recompute).
- **Money.** Cells hold `Money`; display is `Money::format`; amount entry and the calculator (`cl_model::amount_entry`) are exact and refuse inexact results. The grid never sums; totals come from the screen or the engine.
- **Screens.** cl-ui's `GridForm` keeps a form's `FormState` and its grid together, so every save, leave and record move commits the open cell first. `grid_env` supplies date order, money style, theme and the modal flag per viewport; column widths live in `grid-widths.json`.
- **Checks.** `cl-grid/tests` (state tables, keyboard in consecutive frames, virtualization work counts, AccessKit), `cl-ui/tests/grid_keyboard.rs` (the demo in the real shell), `ui-check grid_demo` (`--demo` only), and `cargo xtask grid-bench` (p95 frame time with 200k rows).

## Online-only features and offline behavior

| Feature | Offline behavior |
| --- | --- |
| Bank data | Bank file import (QIF, QFX, OFX, CSV) only in beta; Plaid is post-beta (P43) |
| AI suggestions (accounts, bill reading, categorization) | Off by default. Anthropic or OpenAI with the owner's key needs internet; a local Ollama model works fully offline (U-11). Keys passed to the engine per session (U-09). Rules engine always categorizes. |
| Email invoices / statements | Through the owner's SMTP settings when online; otherwise queue to Outbox or save PDF |
| Transfer to MyBooks Online | Export file only (no direct upload in v1) |
| Updates | Manual check against GitHub Releases; never auto-download without consent |

The engine reports connectivity on `/health` (`online: bool`, last check). The app makes no outbound calls except the owner-enabled AI and email features (made by the engine) and the user-initiated update check.

## Security

- Engine API binds loopback only in single-user mode. Two layers guard it: a random launch token per launch (`X-CL-Token`, passed via env, never on the command line) that every request must carry, and the signed-in user's session token on every account route (see Engine contract). Refresh tokens and passwords stay in memory in zeroized `Secret` values and are never persisted or logged.
- Postgres binds loopback only, scram-sha-256, superuser password in the OS keychain (Windows Credential Manager, macOS Keychain, Secret Service on Linux; fallback encrypted file with a user notice).
- Control channel (automation) is off by default; when enabled it uses PdfCraft's safeguards: loopback only, random token in a control file the current user alone can read, refuse symlinked or foreign-owned control files. The server starts only from `--control <path>`; details and the Windows location rule are in Q-P07a to Q-P07d.
- LAN mode: engine binds the chosen interface with TLS (self-signed cert generated on the host); workstations pin the fingerprint at first connect (shown on the host for verification); users authenticate with engine user accounts; brute-force lockout.
- Company data at rest: OS disk encryption is recommended in the setup guide; optional Postgres data-dir encryption is out of scope for v1 (Q-15).

## Crate responsibilities

| Crate | Owns | Must not |
| --- | --- | --- |
| cl-core | `Money`/`Quantity`/`Rate`, typed ids, dates, fiscal calendars (incl. 4-4-5), `Secret`, shared error vocabulary | do I/O, read a clock, generate random numbers, or convert to floating point |
| cl-pg | pg binaries, initdb, start/stop, whole-database pg_dump/pg_restore for disaster recovery and upgrades | know about accounting |
| cl-host | process supervision, single-instance lock, logs | render UI |
| cl-company | company lifecycle, backup/restore packaging, retention | call egui |
| cl-api | engine HTTP client, sessions and token refresh, typed errors, retry policy and outcome reconciliation, conflict detection, the UI request queue | cache balances beyond a screen's lifetime, depend on cl-host at runtime (the host implements `EndpointSource`), reach any host but `127.0.0.1` |
| cl-settings | local user preferences; keychain-backed secrets (AI keys, SMTP password) | write secrets to disk in plain text or into backups |
| cl-import | OFX/QFX/QIF/CSV parsing, CSV mapping profiles, dedupe keys, preview | post transactions (engine does) |
| cl-model | form state machines, validation, keymap, undo | depend on egui |
| cl-grid | ledger grid widget: row model, viewport, editing state machine, input sequencer, AccessKit nodes | know resource types (generic over row model), sum or round money |
| cl-ui | screens, navigation, theme, i18n | talk to Postgres |
| cl-print | check/form/report PDF layout, MICR font placement, print hand-off | depend on the UI |
| cl-automation | control channel, tool table, scenarios | be enabled without explicit flag |
| cl-lan | discovery (mDNS), TLS pinning, host/client mode switching | weaken loopback defaults |

## Licensing boundary

| Component | License | Ships in installer |
| --- | --- | --- |
| ClientLedger Rust code (`crates/`, `apps/`, `xtask/`) | MIT | yes |
| MyBooks engine (`engine/` submodule, public repo) | PolyForm Internal Use 1.0.0, unmodified (Q-31) | yes, as `clientledger-engine` |
| Node.js runtime inside the engine executable | MIT (plus bundled third-party notices) | yes |
| PostgreSQL 16 binaries | PostgreSQL License | yes |
| PDFium binaries | BSD-3-Clause / Apache-2.0 | yes |
| IBM Plex fonts | SIL OFL 1.1 | yes |
| Rust crates | MIT / Apache-2.0 direct; permissive transitive per Q-07 | yes |

The installer and Help › About show all of these. `NOTICE.md` states plainly that the MIT license does not cover `engine/`.
