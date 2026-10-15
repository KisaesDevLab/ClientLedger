# ClientLedger Architecture

## Process model

```
┌──────────────────────────── one PC (single-user or LAN host) ─────────────────────────────┐
│                                                                                            │
│  clientledger (Rust, eframe/egui)                                                          │
│  ├─ cl-ui ── screens, shell, theme                                                         │
│  ├─ cl-grid ── ledger grid widget                                                          │
│  ├─ cl-api ── typed HTTP client ───────────┐ loopback 127.0.0.1:<random>, bearer token     │
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
4. `cl-pg` starts Postgres on a random free loopback port, waits for `pg_isready`.
5. `cl-host` starts `clientledger-engine` with env: `DATABASE_URL`, `CL_MODE=desktop`, `CL_BIND=127.0.0.1:<port>`, `CL_TOKEN=<random 256-bit>`, `CL_OFFLINE=auto`.
6. Engine runs migrations for the open company database, then reports ready on `/health`.
7. UI shows the Open Company window (or reopens the last company).
8. Shutdown: UI asks engine to drain jobs → engine exits → `pg_ctl stop -m fast` → lock released. Crash of either child: supervisor restarts with backoff (max 3), then shows a recovery dialog with log location.

## Company files

A "company" is an engine **tenant** inside the single local database (Q-05), plus its attachments folder. Company-level backups and transfers use the engine's versioned tenant export/import (U-05); whole-cluster cold backups (`cl-pg` `dump`/`restore`, Q-P03a) are reserved for disaster recovery and PostgreSQL upgrades.

| User action | Implementation |
| --- | --- |
| New Company | engine creates a tenant and seeds the chart-of-accounts template |
| Open Company | UI session scoped to the tenant; lists load |
| Back Up | engine tenant export (NDJSON) + attachments + `manifest.json` (app version, engine commit, schema version, trial balance, SHA-256 of parts) → one `.clbackup` zip |
| Restore | verify manifest + checksums → import into a new tenant → compare trial balance → swap names; old tenant kept until the user confirms |
| Portable copy | same as backup, flagged portable |
| Full data backup | whole-database `pg_dump -Fc` of all companies (File › Utilities), for disaster recovery |
| Send to my accountant | `.clxfer` transfer file (P32) for MyBooks Online: lists, transactions, attachments, audit history, manifest, verification totals; secrets excluded |

Automatic backups: on company close (configurable), before every migration, import, restore, and app upgrade. Retention configurable (default: keep last 10 + one per month for 12 months).

## Engine contract

- The engine exposes OpenAPI 3.1 at `/openapi.json`; a copy is committed at `engine/openapi.json` per pinned engine tag.
- `cl-api` is a hand-written typed client (serde + reqwest/rustls) organized by resource, verified by contract tests against a live engine. No GPL/MPL code generators.
- Money crosses the boundary as decimal strings with fixed scale (or integer minor units, whichever the engine uses — record in Q-04).
- All mutating calls carry an idempotency key. The UI retries safely on timeouts.
- Optimistic concurrency: every editable record carries a `version`; saves send it; the engine rejects stale writes with 409, and the UI shows a "changed by another user" merge dialog (critical in LAN mode).

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

- Engine API binds loopback only in single-user mode; random bearer token per launch passed via env, never on the command line.
- Postgres binds loopback only, scram-sha-256, superuser password in the OS keychain (Windows Credential Manager, macOS Keychain, Secret Service on Linux; fallback encrypted file with a user notice).
- Control channel (automation) is off by default; when enabled it uses PdfCraft's safeguards: loopback only, random token in a control file the current user alone can read, refuse symlinked or foreign-owned control files.
- LAN mode: engine binds the chosen interface with TLS (self-signed cert generated on the host); workstations pin the fingerprint at first connect (shown on the host for verification); users authenticate with engine user accounts; brute-force lockout.
- Company data at rest: OS disk encryption is recommended in the setup guide; optional Postgres data-dir encryption is out of scope for v1 (Q-15).

## Crate responsibilities

| Crate | Owns | Must not |
| --- | --- | --- |
| cl-core | Money, ids, dates, fiscal calendars (incl. 4-4-5), shared error types | do I/O |
| cl-pg | pg binaries, initdb, start/stop, whole-database pg_dump/pg_restore for disaster recovery and upgrades | know about accounting |
| cl-host | process supervision, single-instance lock, logs | render UI |
| cl-company | company lifecycle, backup/restore packaging, retention | call egui |
| cl-api | engine HTTP client, idempotency, retries, 409 handling | cache balances beyond a screen's lifetime |
| cl-settings | local user preferences; keychain-backed secrets (AI keys, SMTP password) | write secrets to disk in plain text or into backups |
| cl-import | OFX/QFX/QIF/CSV parsing, CSV mapping profiles, dedupe keys, preview | post transactions (engine does) |
| cl-model | form state machines, validation, keymap, undo | depend on egui |
| cl-grid | ledger grid widget | know resource types (generic over row model) |
| cl-ui | screens, navigation, theme, i18n | talk to Postgres |
| cl-print | check/form/report PDF layout, MICR font placement, print hand-off | depend on the UI |
| cl-automation | control channel, tool table, scenarios | be enabled without explicit flag |
| cl-lan | discovery (mDNS), TLS pinning, host/client mode switching | weaken loopback defaults |

## Licensing boundary

| Component | License | Ships in installer |
| --- | --- | --- |
| ClientLedger Rust code (`crates/`, `apps/`, `xtask/`) | MIT | yes |
| MyBooks engine (`engine/` submodule, public repo) | PolyForm Internal Use 1.0.0 + ClientLedger redistribution grant (Q-31) | yes, as `clientledger-engine` |
| Node.js runtime inside the engine executable | MIT (plus bundled third-party notices) | yes |
| PostgreSQL 16 binaries | PostgreSQL License | yes |
| PDFium binaries | BSD-3-Clause / Apache-2.0 | yes |
| IBM Plex fonts | SIL OFL 1.1 | yes |
| Rust crates | MIT / Apache-2.0 direct; permissive transitive per Q-07 | yes |

The installer and Help › About show all of these. `NOTICE.md` states plainly that the MIT license does not cover `engine/`.
