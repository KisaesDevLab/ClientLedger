# UPSTREAM.md — changes to the MyBooks engine

ClientLedger consumes the public MyBooks engine repo as the `engine/` git submodule tracking `main` (Q-02; PolyForm Internal Use 1.0.0 plus the ClientLedger redistribution grant, Q-31). Engine changes are made on `clientledger/<feature>` branches, follow the engine's own conventions and tests, and go to the owner as pull requests on the engine repo; the owner reviews and merges (Q-29). Until merged, the submodule may point at the branch commit. Keep changes generic so MyBooks Online benefits.

Status values: `Planned`, `In progress`, `Done (branch)`, `Merged (MyBooks vX.Y)`, `Blocked`.

| ID | Change | Needed by | Branch | Status |
| --- | --- | --- | --- | --- |
| U-01 | Queue abstraction with pg-boss adapter (desktop) alongside BullMQ (server) | P02 | clientledger/desktop-profile | Planned |
| U-02 | `CL_MODE=desktop` profile: no Redis (also no `REDIS_URL` requirement in `env.ts`; `/health` must return 200 without it, Q-P01a), env-provided bind address and launch token (`X-CL-Token`), file logging | P02 | clientledger/desktop-mode (E1) | Done (branch), pushed; PR not yet opened (see entry) |
| U-03 | Connectivity reporting on `/health` plus an `engineCommit` field (Q-P01a); online-only modules return `OfflineUnavailable` fast | P02 | clientledger/desktop-mode (E1: `/health` v2) + clientledger/offline-guard (E4: connectivity) | In progress — `/health` v2 done on E1; connectivity/`OfflineUnavailable` pending E4 |
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

### U-03 — `/health` v2 and connectivity
- E1 part (done on `clientledger/desktop-mode`, commit `c1635b2a`): `mode`, `version`, `engineCommit`, `schemaVersion`, `schemaTag`, `queueBackend`, `online`/`lastOnlineCheck` (always `null` until E4), `checks.db.superuser`, and `redis: "disabled"`. Returns 200 on desktop with no Redis.
- E4 part (pending, `clientledger/offline-guard`): connectivity probe, `OfflineUnavailable` (503 `OFFLINE_UNAVAILABLE`), egress guard, SMTP timeouts.
- Status: In progress — 2026-10-15.

### U-16 — Engine identity proof and graceful shutdown (proposed)
- Needed by: not blocking. P04 ships without it (Q-P04d, Q-P04e); P37 hardening should decide.
- Why: (a) If the engine dies, another local program could bind its port and receive the launch token in the `X-CL-Token` header of the next request (security review L11). P04 already checks which process owns the port before sending the token (`cl-host` `verify_listener`), but that is an operating-system lookup and cannot be done the same way everywhere. A proof from the responder removes the dependency. (b) Windows has no graceful stop for a windowless child, so the supervisor terminates the engine's job object; a shutdown endpoint lets the engine finish in-flight requests and background work first.
- Proposed change, desktop mode only, nothing changes on the appliance:
  - `/health`, `/api/health` and `/api/v1/health` accept an optional `X-CL-Nonce` header (16 to 64 random bytes, hex). The answer then carries `proof = hex(HMAC-SHA256(key = launch token, message = "cl-health-v1" + nonce))`. The token is never echoed. The host computes the same value and accepts the engine only on a match, before it sends the token anywhere else (the first nonce request itself carries no token, so it is answered to an impostor the same way as to the engine, which reveals nothing).
  - The desktop gate must let a request that carries only `X-CL-Nonce` reach `/health` (today every request needs the token): it answers 401 with the proof so the host can tell "the engine, wrong token" from "something else".
  - `POST /api/v1/desktop/shutdown` (gated like every route): stop accepting requests, drain, close the pool, exit 0. The host falls back to terminating the process tree after `stop_grace`.
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
