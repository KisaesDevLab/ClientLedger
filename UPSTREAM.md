# UPSTREAM.md — changes to the MyBooks engine

ClientLedger consumes the public MyBooks engine repo as the `engine/` git submodule tracking `main` (Q-02; PolyForm Internal Use 1.0.0 plus the ClientLedger redistribution grant, Q-31). Engine changes are made on `clientledger/<feature>` branches, follow the engine's own conventions and tests, and go to the owner as pull requests on the engine repo; the owner reviews and merges (Q-29). Until merged, the submodule may point at the branch commit. Keep changes generic so MyBooks Online benefits.

Status values: `Planned`, `In progress`, `Done (branch)`, `Merged (MyBooks vX.Y)`, `Blocked`.

| ID | Change | Needed by | Branch | Status |
| --- | --- | --- | --- | --- |
| U-01 | Queue abstraction with pg-boss adapter (desktop) alongside BullMQ (server) | P02 | clientledger/desktop-profile | Planned |
| U-02 | `CL_MODE=desktop` profile: no Redis (also no `REDIS_URL` requirement in `env.ts`; `/health` must return 200 without it, Q-P01a), env-provided bind address and launch token (`X-CL-Token`), file logging | P02 | clientledger/desktop-mode (E1) | Done (branch), pushed; PR not yet opened (see entry) |
| U-03 | Connectivity reporting on `/health` plus an `engineCommit` field (Q-P01a); online-only modules return `OfflineUnavailable` fast | P02 | clientledger/desktop-mode (E1: `/health` v2) + clientledger/offline-guard (E4: connectivity) | Done (branch) — E1 `/health` v2; E4 at `abc0293b` (incl. security-review fixes), local only, not pushed, no PR yet |
| U-04 | OpenAPI 3.1 export covering all UI routes; committed `openapi.json` | P02 | clientledger/desktop-profile | Planned |
| U-05 | Tenant lifecycle (list/create/rename/delete) and versioned per-tenant export/import (NDJSON + attachments + manifest) for backups, restores, and transfers | P02, P08 | clientledger/tenant-portability | Planned |
| U-06 | Endpoint accepting normalized bank-import batches (rows, dedupe keys, source-file hash) into the review queue; parsing is in Rust (Q-11) | P25 | clientledger/bank-import | Planned |
| U-07 | Desktop-mode adjustments to the QBD report-export importer (file paths, progress events, tie-out endpoint); report list and mapping unchanged | P31 | clientledger/qbd-import | Planned |
| U-08 | Import of `.clxfer` transfer files in MyBooks Online, showing verification totals; accepts any older `formatVersion`, refuses newer with an update message (Q-43) | P32 | clientledger/transfer | Planned |
| U-09 | AI provider credentials supplied by the desktop app per session (owner's own key); never stored by the engine in desktop mode; AI disabled without key or offline; redaction of bank/card numbers, tax IDs, emails, phones before provider calls; per-call token usage reported for the meter and cap | P02, P09 | clientledger/desktop-profile | Planned |
| U-11 | Ollama provider adapter for the engine's multi-provider AI (local base URL, model choice, no key) if missing; Anthropic and OpenAI adapters confirmed | P09 | clientledger/desktop-profile | Planned |
| U-12 | Integrity check endpoints (full Verify and fast weekly subset) with typed findings and safe automatic fixes | P34 | clientledger/verify | Planned |
| U-10 | Redistribution grant text added to the engine repo (`LICENSE-CLIENTLEDGER.md`) by the owner | P39 | main | Planned — owner (Q-31) |
| U-16 | Engine identity proof (HMAC challenge in `/health`) and graceful shutdown (`POST /api/v1/desktop/shutdown`) for the supervisor | P04 follow-up (P37 hardening) | clientledger/desktop-supervisor | Proposed |

Add rows as capability checks find gaps (credit memos, statements, finance charges, sales tax, average-cost inventory, reconciliation, permissions granularity, optimistic concurrency, change notifications for LAN mode; post-beta: estimates, sales orders, POs, assemblies, custom fields, form templates).

## Entries

### U-02 — Desktop mode (`CL_MODE=desktop`), E1
- Needed by: P02 (P04 supervisor, P01 smoke carry-over Q-P01a)
- Why: the engine must run as a local sidecar with only PostgreSQL. That means no Redis or worker container, a loopback-only listener that requires a launch token, no appliance setup wizard or fallback web apps, structured startup errors, scrubbed and rotated file logs, and a non-superuser database role.
- Branch: `clientledger/desktop-mode` from `13ebd2ba` (engine submodule). 10 commits:
  - `41168df7` feat(config): desktop profile loader and env contract
  - `f79b0f75` feat(config): structured fatal exit and desktop preflight
  - `b563a9fd` feat(auth): desktop token gate
  - `27fd336e` feat(server): loopback bind, port 0 ready line
  - `73886826` feat(desktop): log tee with rotation and secret scrubber
  - `cb1b4244` feat(desktop): redis-free switches and queue backend none
  - `83d16b22` feat(desktop): feature profile
  - `07593670` feat(desktop): initialize endpoint
  - `c1635b2a` feat(health): version, commit, schema, mode
  - `8213777e` docs(desktop): DESKTOP.md env contract and profile tables
- Engine files touched (packages/api/src):
  - New: `desktop/{profile,fatal,log-tee,features}.ts`, `middleware/desktop-gate.ts`, `routes/desktop.routes.ts`, `queue/none.adapter.ts`, `startup/{db-role,schema-info}.ts`, `utils/{secret-scrubber,health}.ts`.
  - Changed: `bootstrap.ts`, `index.ts`, `app.ts`, `config/env.ts`, `startup/preflight.ts`, `services/extraction/queue.ts`, `services/passkey-challenge-store.ts`, `services/share/share-redis.ts`, `services/tb/tb-redis.ts`, `utils/{health-redis,peer-jti-store,rate-limit-store,worker-heartbeat}.ts`.
  - Docs: `docs/DESKTOP.md` (new), `docs/API.md`.
- Schema/migrations: none.
- API changes:
  - Desktop only: `POST /api/v1/desktop/initialize`, `GET /api/v1/desktop/info`.
  - Every desktop request needs `X-CL-Token`. 17 route families answer 404 `NOT_AVAILABLE_IN_DESKTOP`.
  - `/health` v2 in all modes (additive).
  - New env: `CL_MODE`, `CL_BIND`, `CL_TOKEN`, `CL_DATA_DIR`, `CL_LOG_FILE`/`_MAX_BYTES`/`_KEEP`, `CL_OFFLINE`, `CL_LAN`, `CL_REQUIRE_NON_SUPERUSER`, `QUEUE_BACKEND` (bullmq|none), `ENGINE_COMMIT`.
  - `REDIS_URL` is now required only when something uses Redis.
  - OpenAPI: the engine has no `openapi.json` or export script yet; that is U-04 (E2).
- Tests (vitest, packages/api): `desktop/{profile,app-gate,desktop-boot,no-redis,features,log-tee}.test.ts`, `middleware/desktop-gate.test.ts`, `config/env.server-defaults.test.ts` (fixture from the pre-change env.ts), `startup/desktop-preflight.test.ts`, `utils/{secret-scrubber,health.v2}.test.ts`.
  - The boot test spawns the real `bootstrap.ts` with no Redis on a fresh database. It includes a NOSUPERUSER owner role migrating and booting under `CL_REQUIRE_NON_SUPERUSER=1` (P03 follow-up #10).
- Server appliance impact: none by default.
  - With `CL_MODE` unset, `env.ts` parses to the same values (plus `QUEUE_BACKEND=bullmq`).
  - The server Express layer table (1,815 layers, `NODE_ENV=production`) is identical to `13ebd2ba`.
  - BullMQ stays the default backend, and `/health` keeps every v1 field and its 503 logic.
  - Visible change: `/health` gains the v2 fields, which are unauthenticated on the server (reviewer item: version and commit disclosure).
- Status: Done (branch) — 2026-10-15. Local only; not pushed and no engine PR opened yet (per orchestrator instruction). The engine PR is opened with the E-series (plan step 43).
- **Follow-up 8b/8c — workspace tenant and guards (Q-P02h, review P02-E5-rev2 N1/N2), 2026-10-15.** Two more local commits on `clientledger/desktop-mode` (split because the diff exceeded ~600 lines; both must land before E5 steps 33c/34b/35b/36a–c). Not pushed, no PR.
  - `79dd153e` feat(desktop): workspace tenant — migration, `desktop/workspace.ts`, `/initialize` rewrite, `/info.workspaceTenantId`, user homing, register/create-client refusal, `createAdminUser`/`adoptRestoredTenants` guards, desktop create-tenant eligibility, switch-tenant `TENANT_NOT_FOUND`.
  - `7de1e152` feat(desktop): workspace guards — fence in `authenticate` (session JWT, API key, download token), admin `/tenants/:id…` guards (`WORKSPACE_TENANT`, `USE_SAFE_DELETE`, `NOT_AVAILABLE_IN_DESKTOP`), `/api/v1/tenant-export` and `/api/v1/backup` (except `/passphrase-strength`) unmounted, boot invariant warning, docs.
  - Files: new `desktop/workspace.ts`, `db/migrations/0216_tenants_is_workspace{,.rollback}.sql`; changed `db/schema/auth.ts`, journal, `routes/{desktop,auth,admin}.routes.ts`, `services/{auth,setup}.service.ts`, `middleware/auth.ts`, `desktop/features.ts`, `app.ts`, `index.ts`, `docs/DESKTOP.md`, `docs/API.md`. Tests: new `desktop/workspace.test.ts` (G-E1 + MT20, in-process app on fresh template-copied databases), `desktop/users-insert-sites.test.ts` (R19 enumerator); `desktop/desktop-boot.test.ts` initialize/login rewritten.
  - Schema/migration: `0216_tenants_is_workspace` — additive `tenants.is_workspace boolean NOT NULL DEFAULT false` + partial unique index `uniq_tenants_workspace`; rollback drops index then column (tested); passes `migrations:check` with no marker.
  - API changes (desktop only): `/initialize` body `company` optional, response `{workspaceTenantId, userId, companyTenantId|null, warnings}` (was `{tenantId, userId}`); `/info` adds `workspaceTenantId`; new error codes `WORKSPACE_TENANT`, `WORKSPACE_MISSING`, `USE_SAFE_DELETE`, `TENANT_NOT_FOUND`, `COMPANY_NOT_CREATED` (warning). 19 families unmounted (was 17).
  - Server appliance impact: none apart from the additive column. Express layer table unchanged (1,815 layers); register/create-client gain a pass-through handler; every new check is behind `isDesktop()`. `rollbackPartialSetup` was refactored to share `deleteTenantScopedRows` (same behavior).

### U-03 — `/health` v2 and connectivity
- E1 part (done on `clientledger/desktop-mode`, commit `c1635b2a`): `mode`, `version`, `engineCommit`, `schemaVersion`, `schemaTag`, `queueBackend`, `online`/`lastOnlineCheck` (always `null` until E4), `checks.db.superuser`, and `redis: "disabled"`. Returns 200 on desktop with no Redis.
- E4 part (`clientledger/offline-guard`, stacked on E1 `8213777e`; commits `78f329f1`, `35a6e33e`, `80079385`, `62133728`, `7702a6bf`):
  - `utils/offline.ts`: `OfflineUnavailableError` (503 `OFFLINE_UNAVAILABLE` + `feature`), `ensureOnline` (CL_OFFLINE `on` fails at once; `auto` probes only the feature's own host, 1.5 s budget, success cached 30 s, failure 5 s), `classifyNetworkError`; `/health` `online`/`lastOnlineCheck` are now real.
  - `desktop/egress-guard.ts`: default-deny at the socket level (`net.Socket#connect`, all `dns` lookup/resolve forms, `dgram`), installed first in `bootstrap.ts` in desktop mode. Allows loopback, IPC, the `DATABASE_URL` host, new optional env `CL_EGRESS_ALLOW`, and runtime `setEgressAllowance()` (E6 session secrets will register AI/SMTP hosts).
  - `utils/mail-transport.ts`: all 9 `nodemailer.createTransport` sites; desktop adds 10/10/30 s timeouts and the offline fast-fail.
  - AI: Anthropic/OpenAI providers wrapped in desktop mode (incl. vision paths); no retry on offline; fallback skips other cloud providers once offline.
  - Phone-homes: `/api/v1/admin/updates` added to `DESKTOP_UNMOUNTED`; Turnstile skipped in desktop.
- Files: `utils/offline.ts`, `desktop/egress-guard.ts`, `utils/mail-transport.ts`, `bootstrap.ts`, `app.ts`, `middleware/error-handler.ts`, `utils/health.ts`, `utils/retry.ts`, `utils/turnstile.ts`, `desktop/features.ts`, `ai-providers/index.ts`, 8 mail-sending services, `ai-categorization.service.ts`, `ai-config.service.ts`, `docs/DESKTOP.md`; tests `utils/offline.test.ts`, `desktop/egress-guard.test.ts`, `utils/mail-transport.test.ts`, `ai-providers/offline-fast-fail.test.ts`, `desktop/offline-matrix.test.ts`.
- Schema/migrations: none. API: no new routes. New 503 codes `OFFLINE_UNAVAILABLE` and `EGRESS_BLOCKED` (desktop only); the OpenAPI component is added when E2 is integrated (`openapi.json` does not exist on this branch).
- Server appliance impact: none. The guard is never installed, `CL_OFFLINE` is always `off` (no probes, errors unchanged), the mail factory is plain `nodemailer.createTransport`, and providers are not wrapped.
- Tests (Windows, PG16, no Redis): full api suite 4904 passed / 30 failed (the known baseline: Redis tax-estimator, BullMQ queue, one Windows path test) / 16 skipped. Not run: Redis suites, Linux/macOS, SEA runtime, the optional `unshare -rn` CI job.
- Security review (2026-10-15; no Critical, one High), all fixed on the branch, `1ffbd065`..`abc0293b`, one commit per item:
  - H-1: worker threads now run the guard. OCR language data is local only: `OCR_LANG_PATH`, or `<CL_DATA_DIR>/tessdata` on desktop. Missing data gives 503 `OCR_DATA_MISSING`.
  - M-1 and L-1: Node's string-port rules, and non-string hosts are blocked.
  - M-2: SMTP host check runs before the probe. Loopback is never "offline", and an inconclusive probe is not cached.
  - M-3: report PDFs load only `data:`/`about:` and run Chromium with `--disable-background-networking`.
  - M-4: desktop SMTP defaults to `requireTLS` (except loopback).
  - L-2: patches are verified, and `index.ts` exits with `EGRESS_GUARD_MISSING` if the guard is absent.
  - L-3: proxy env vars are removed.
  - L-4: only exact `localhost` counts as loopback.
  - L-5: `CL_OFFLINE=on` allows only loopback and the DB.
  - L-7: no recipient address in the mail error log.
- Recorded, not implemented (docs/DESKTOP.md "Rules for features that open egress"), for E6:
  - L-6: validate Ollama/openai_compat base URLs.
  - AI hosts come from a fixed provider table.
  - SMTP host must pass `assertHostSafe` and use ports 25/465/587/2525 only.
  - Only the owner role may add an allowance, and it is cleared when the feature is turned off.
  - Never widen the guard with `CL_EGRESS_ALLOW`.
  - L-8: open keep-alive sockets can be reused after an allowance is removed (documented).
- Packaging follow-ups for P04/xtask:
  - Ship `eng.traineddata` (Apache-2.0, add to ATTRIBUTION.md) and set `OCR_LANG_PATH`.
  - Ship `tesseract.js` and `tesseract.js-core` on disk and set `OCR_WORKER_PATH` for the SEA build.
  - Verify the worker-thread guard in the 4-OS SEA smoke.
- Status: Done (branch) — 2026-10-15. Local only; not pushed, no engine PR (per orchestrator).

### U-16 — Engine identity proof and graceful shutdown (proposed)
- Needed by: not blocking. P04 ships without it (Q-P04d, Q-P04e); P37 hardening should decide.
- Why: (a) If the engine dies, another local program could bind its port and receive the launch token in the `X-CL-Token` header of the next request (security review L11). P04 already checks which process owns the port before sending the token (`cl-host` `verify_listener`), but that is an operating-system lookup and cannot be done the same way everywhere. A proof from the responder removes the dependency. (b) Windows has no graceful stop for a windowless child, so the supervisor terminates the engine's job object; a shutdown endpoint lets the engine finish in-flight requests and background work first.
- Proposed change, desktop mode only, nothing changes on the appliance:
  - `/health`, `/api/health` and `/api/v1/health` accept an optional `X-CL-Nonce` header (16 to 64 random bytes, hex). The answer then carries `proof = hex(HMAC-SHA256(key = launch token, message = "cl-health-v1" + nonce))`. The token is never echoed. The host computes the same value and accepts the engine only on a match, before it sends the token anywhere else (the first nonce request itself carries no token, so it is answered to an impostor the same way as to the engine, which reveals nothing).
  - The desktop gate must let a request that carries only `X-CL-Nonce` reach `/health` (today every request needs the token): it answers 401 with the proof so the host can tell "the engine, wrong token" from "something else".
  - `POST /api/v1/desktop/shutdown` (gated like every route): stop accepting requests, drain, close the pool, exit 0. The host falls back to terminating the process tree after `stop_grace`.
  - Process lifetime (security review L8): the engine exits when its stdin closes (the host keeps the write end open) and, on Linux, sets `PR_SET_PDEATHSIG`, so a crashed host never leaves an engine behind. The host then also writes `run/engine.pid` atomically with the spawn.
- Engine files touched: `middleware/desktop-gate.ts`, `utils/health.ts`, `routes/desktop.routes.ts`, `desktop/profile.ts` (docs).
- Schema/migrations: none.
- Tests: gate (nonce without token, wrong nonce length), proof vector, shutdown drain; host side: `cl-host` verifies the proof and tests an impostor that answers 200 without it.
- Server appliance impact: none (all behind `isDesktop()`).
- Status: Proposed — 2026-10-15. Not started.

## Entry template

```
### U-xx — <title>
- Needed by: Pxx
- Why: <what the desktop app cannot do without it>
- Engine files touched: <paths>
- Schema/migrations: <yes/no + description>
- API changes: <endpoints added/changed, OpenAPI updated>
- Tests: <engine tests added>
- Server appliance impact: <none / describe>
- Status: <status> — <date>
```
