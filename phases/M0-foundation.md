# M0 — Foundation (P00–P09)

**Outcome:** the app launches with no network, runs its own PostgreSQL and engine, opens and backs up a company, holds preferences and optional AI keys, and can be driven and screenshotted by Claude Code.

**Milestone exit:** all ten phases merged; `cargo xtask ci` green on Linux, Windows, macOS; a milestone summary written to STATE.md. Process-review checkpoint **PR-P05** happens inside P05 (plan review).

---

## P00 — Repository bootstrap

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| L | M | — | rust-implementer (sonnet) | — |

**Decisions:** Q-01, Q-07, Q-32, Q-72, Q-112–Q-117, Q-131–Q-133, Q-221.

**Crates and files**
- `Cargo.toml` (workspace), `rust-toolchain.toml` (stable, edition 2024), every crate and app in the CLAUDE.md layout as compiling stubs.
- `LICENSE` (MIT, Kisaes LLC dba KisaesDevLab), `NOTICE.md`, `README.md`, `CONTRIBUTING.md`, `SECURITY.md`, `ATTRIBUTION.md`, `.gitignore`, `.editorconfig`, `.github/ISSUE_TEMPLATE/*`, `.github/workflows/{ci.yml,nightly.yml,close-prs.yml}`.
- `deny.toml`, `xtask/src/{main.rs,ci.rs,layering.rs,licenses.rs}`.
- Plan files already present: `CLAUDE.md`, `PHASES.md`, `phases/*.md`, `STATE.md`, `QUESTIONS.md`, `UPSTREAM.md`, `docs/*`, `.claude/*`.

**Steps**
1. Create the workspace with crates `cl-core`, `cl-api`, `cl-pg`, `cl-host`, `cl-company`, `cl-settings`, `cl-import`, `cl-model`, `cl-grid`, `cl-ui`, `cl-automation`, `cl-print`, `cl-lan`; apps `clientledger`, `clientledger-cli`; `xtask`. Each crate has `#![forbid(unsafe_code)]` and one trivial test. — `chore(repo): workspace skeleton`
2. Licensing files: MIT `LICENSE` scoped to ClientLedger code; `NOTICE.md` explains `engine/` is PolyForm Internal Use 1.0.0 (unmodified, no grant), the internal-business-use scope (Q-132), and that forks may not bundle, download, or install the engine (Q-133); README links both. — `docs(license): MIT scope and engine notice`
3. Public-repo hygiene: CONTRIBUTING (issues and discussions welcome; PRs closed automatically with a note — Q-32), SECURITY (licensing@kisaes.com until a security address exists), issue templates (bug, question), `.gitignore` with `target/`, `private-fixtures/` (Q-72). Workflow `close-prs.yml` comments and closes PRs from outside the org. — `chore(repo): community files`
4. `deny.toml`: `[licenses]` allow MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, CC0-1.0, BSL-1.0 (Boost); deny everything else; `[bans]` deny multiple major versions of egui/wgpu; `[advisories]` deny vulnerabilities. — `chore(deny): license and advisory policy`
5. `xtask licenses`: read `cargo metadata`, fail if any **direct** dependency of a workspace crate is not MIT or Apache-2.0 (dual allowed). — `feat(xtask): direct-dependency license gate`
6. `xtask layering`: encode the CLAUDE.md rules as a table; fail with the offending edge. — `feat(xtask): crate layering check`
7. `xtask ci`: fmt check, clippy `-D warnings` (pedantic reviewed), test, `cargo deny check`, licenses, layering. Stubs for `run`, `engine`, `pg-fetch`, `ui-check`, `keymap-doc`, `package` that print "lands in Pxx". — `feat(xtask): ci entry point`
8. CI: `ci.yml` runs `cargo xtask ci` on Ubuntu 22.04 for every push; on pull requests also Windows x64 and macOS arm64 + x64. `nightly.yml` runs all three. Secret scanning with a permissive scanner. Coverage job (`cargo-llvm-cov`) reports only until P05, then enforces 80% (Q-116). — `ci: three-OS matrix and nightly`
9. Branch protection notes in README (owner applies them on GitHub): require PR checks on `main`, allow auto-merge.

**Tests**
- Layering and license gates have unit tests with fake metadata.
- One-off verification (not committed): add a GPL crate → CI fails; add a direct BSD-only crate → CI fails; add a forbidden crate edge → CI fails.

**Acceptance**
- [ ] `cargo xtask ci` passes locally and on all three OSes.
- [ ] GPL crate, direct BSD-only crate, and layering violation each fail CI (verified once, reverted, noted in the PR).
- [ ] Secret scan passes; no real data or private URLs in the repo.

---

## P01 — Engine submodule and single-executable build

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| L | M | P00 | rust-implementer (sonnet) | — |

**Decisions:** Q-02, Q-03, Q-29, Q-35a. **Engine:** none (read-only).

**Crates and files:** `.gitmodules`, `engine/` (submodule → `KisaesDevLab/vibe-mybooks`, tracking `main`), `xtask/src/engine.rs`, `target/engine/<target>/{clientledger-engine[.exe],manifest.json}`.

**Steps**
1. Add the submodule tracking `main`; pin the current commit; record it in STATE.md. — `build(engine): add vibe-mybooks submodule`
2. `xtask engine`: read the engine's Node version (`.nvmrc`/`engines`), run `npm ci` and the engine build; bundle to one JS file; create a Node SEA blob and inject it into a copy of the Node binary for the host target; codesign placeholder hooks for macOS. Cache by engine commit + Node version. — `feat(xtask): engine single executable`
3. Write `manifest.json` (engine commit, Node version, bundle SHA-256, build time).
4. Smoke test `xtask engine --smoke`: start a temporary Postgres (system or Docker in CI), run the executable with minimal env, poll `/health`, stop. — `test(engine): smoke run`
5. CI: build the engine on all three OSes; upload as a workflow artifact for later jobs.

**Tests:** smoke test; a test that the executable runs with `PATH` stripped of Node.

**Acceptance**
- [ ] Executable builds on Windows x64, macOS arm64/x64, Linux x64.
- [ ] Runs without Node or `node_modules` installed.
- [ ] `/health` reports the pinned engine commit.

---

## P02 — Engine desktop profile (upstream)

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | L | P01 | engine-implementer (opus) | reviewer (opus) |

**Agents:** engine-scout → architect (opus) → engine-implementer (opus) → reviewer. **Decisions:** Q-05, Q-11, Q-33, Q-38, Q-136–Q-141. **Engine:** U-01, U-02, U-03, U-04, U-05, U-09, U-11.

**Engine areas:** queue/jobs module, config/bootstrap, health route, OpenAPI generation, tenant management, AI provider layer.

**Steps** (each a commit on `clientledger/desktop-profile` or `clientledger/tenant-portability` in `engine/`)
1. **U-01** Extract a `JobQueue` interface from current BullMQ usage; implement `PgBossQueue`; select by config; port every job type; keep BullMQ default for the server appliance. — `feat(queue): pg-boss adapter`
2. **U-02** `CL_MODE=desktop`: skip Redis, read `CL_BIND`, `CL_TOKEN`, `CL_LOG_FILE`; require bearer token on every route; refuse non-loopback binds unless `CL_LAN=1` (for P33). — `feat(config): desktop mode`
3. **U-03** `/health` → `{version, schemaVersion, online, lastOnlineCheck, mode}`; connectivity probe on demand; online-only modules (AI providers, SMTP, Plaid) throw `OfflineUnavailable` within 2 s. — `feat(health): connectivity and offline errors`
4. **U-04** OpenAPI 3.1 generation from route definitions; `npm run openapi:export`; commit `openapi.json`; CI check that it is current. — `feat(api): openapi export`
5. **U-05** Tenant lifecycle endpoints (list/create/rename/delete) and versioned tenant export/import: NDJSON per table in dependency order + attachments + `manifest.json` (format version, engine commit, schema version, counts, trial balance, SHA-256 per part). Import into a new tenant id; verify counts and trial balance. — `feat(tenant): export/import`
6. **U-09** AI calls accept provider credentials per session from the client; never persisted in desktop mode; redaction pass (account/card numbers, tax IDs, emails, phones) before provider calls; per-call token usage returned. — `feat(ai): desktop credentials, redaction, usage`
7. **U-11** Ollama adapter (base URL, model, no key) if missing; confirm Anthropic and OpenAI adapters. — `feat(ai): ollama provider`
8. Open a PR on the engine repo for owner review (Q-29); point the submodule at the branch commit.

**Tests (engine):** queue adapter parity tests (same jobs on both adapters); desktop-mode auth tests; offline tests with network namespace or blocked DNS; OpenAPI validation; tenant export → import round trip with trial-balance equality; redaction unit tests with fixtures; Ollama adapter test against a stub server.

**Acceptance**
- [ ] Desktop mode runs with no Redis process; recurring and report jobs complete via pg-boss.
- [ ] Network disabled: online-only endpoints return `OfflineUnavailable` within 2 s.
- [ ] `openapi.json` validates and matches the routes.
- [ ] Tenant export → import reproduces the trial balance exactly.
- [ ] Engine test suite green; server-appliance path unchanged (BullMQ tests still pass).

---

## P03 — Bundled PostgreSQL manager (`cl-pg`)

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| L | M | P00 | rust-implementer (sonnet) | security-reviewer (opus) |

**Parallel:** may run alongside P02 (disjoint crates). **Decisions:** Q-05, Q-06, Q-42.

**Crates and files:** `crates/cl-pg/src/{lib.rs,binaries.rs,cluster.rs,ports.rs,keychain.rs,dump.rs,paths.rs}`, `xtask/src/pg_fetch.rs`, `pg-binaries.lock`.

**Steps**
1. `xtask pg-fetch`: download pinned PostgreSQL 16.x archives per target from zonky embedded-postgres-binaries; verify SHA-256 from `pg-binaries.lock`; unpack to `target/pg/<target>/`; trim to `postgres`, `initdb`, `pg_ctl`, `pg_isready`, `pg_dump`, `pg_restore`, `psql`, share files, libs. — `feat(xtask): pinned postgres binaries`
2. `Cluster::ensure(data_dir)`: `initdb` with UTF8, C.UTF-8 collation, scram-sha-256; random superuser password stored via `keyring` (fallback: encrypted file with a user notice). — `feat(pg): cluster init`
3. Data-dir guard: refuse network shares and cloud-sync folders (OneDrive, Dropbox, iCloud Drive, Google Drive markers and known paths) with a clear error (Q-42). — `feat(pg): data dir guard`
4. `start()`: pick a free loopback port, write `postgresql.conf` overrides (listen loopback, port, logging), clear stale `postmaster.pid` safely, `pg_ctl start`, wait `pg_isready`. `stop()`: `pg_ctl stop -m fast`. — `feat(pg): start/stop`
5. `dump`/`restore` wrappers for whole-database disaster recovery (company backups use the engine's tenant export). — `feat(pg): dump and restore`
6. ATTRIBUTION.md: PostgreSQL License and binary source.

**Tests:** fresh init/start/stop; second start reuses cluster; kill -9 recovery; port collision retry; data-dir guard cases; dump→restore row counts and per-table checksums; all with network blocked.

**Acceptance**
- [ ] Fresh `ensure` + `start` under 10 s on CI; second start reuses the cluster.
- [ ] Recovers after kill -9.
- [ ] Dump → restore round trip identical.
- [ ] Postgres listens only on loopback (security-reviewer confirms).

---

## P04 — Supervisor (`cl-host`)

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| L | M | P02, P03 | rust-implementer (sonnet) | security-reviewer (opus) |

**Decisions:** Q-118. **Crates and files:** `crates/cl-host/src/{lib.rs,instance_lock.rs,supervisor.rs,health.rs,logs.rs,paths.rs}`, `apps/clientledger-cli/src/host.rs`.

**Steps**
1. App data paths per OS (`%LOCALAPPDATA%\ClientLedger`, `~/Library/Application Support/ClientLedger`, `$XDG_DATA_HOME/clientledger`). — `feat(host): app paths`
2. Single-instance lock per OS user; second launch sends "focus" over a local socket or named pipe. — `feat(host): single instance`
3. Supervisor: start pg (P03) then engine (P01 executable) with env `DATABASE_URL`, `CL_MODE=desktop`, `CL_BIND=127.0.0.1:<port>`, `CL_TOKEN` (256-bit random, env only), `CL_LOG_FILE`; poll `/health`; stop in reverse; restart each child up to 3 times with backoff; then `HostFault` with log paths. — `feat(host): supervise pg and engine`
4. Log rotation: 30 days or 100 MB, whichever first (Q-118). — `feat(host): log rotation`
5. CLI `clientledger-cli host status|start|stop|logs`. — `feat(cli): host commands`

**Tests:** cold and warm start timings; kill engine → restart; kill pg → restart; three failures → `HostFault`; process listing after shutdown shows no orphans (all OSes); token never appears in process arguments.

**Acceptance**
- [ ] Ready under 15 s cold, 5 s warm on CI.
- [ ] Engine killed while idle restarts; a one-line reconnect notice appears in the UI stub.
- [ ] No orphan processes on any OS; token not visible in process listings.

---

## P05 — Engine API client (`cl-api`)

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | L | P02, P04 | rust-implementer (opus) | reviewer (opus); **PR-P05** process-reviewer (opus) on the plan |

**Agents:** engine-scout → architect (opus) → **process-reviewer (opus) reviews the plan** → test-writer → rust-implementer (opus) → reviewer. **Decisions:** Q-04, Q-116.

**Crates and files:** `crates/cl-core/src/{money.rs,ids.rs,dates.rs,fiscal.rs,errors.rs}`, `crates/cl-api/src/{client.rs,queue.rs,errors.rs,idempotency.rs,resources/*.rs}`, `crates/cl-api/tests/contract_*.rs`, `crates/cl-api/tests/fixtures/openapi.sha256`.

**Steps**
1. Detect the engine's money representation from `openapi.json` and code; record the finding in QUESTIONS.md (Q-04). Implement `cl-core::Money` on `rust_decimal` with exact conversions both ways; property tests. — `feat(core): exact money type`
2. `cl-core` ids, dates (including 4-4-5 fiscal periods), shared errors.
3. HTTP client (reqwest + rustls) with bearer token, timeouts, idempotency keys on mutations, retry with jitter on timeouts/5xx only. — `feat(api): http client`
4. Typed errors: `Validation{field,msg}`, `Conflict{current_version}`, `ClosedPeriod`, `OfflineUnavailable`, `Unauthorized`, `Forbidden`, `NotFound`. — `feat(api): typed errors`
5. Resource modules for every endpoint the beta needs, added incrementally as phases need them; start with companies, accounts, contacts, items, tags, transactions (journal entry). — `feat(api): core resources`
6. UI request queue: tokio runtime off the UI thread; requests return a handle; results polled each frame; cancellation on window close. — `feat(api): ui request queue`
7. Contract harness: start pg + engine via `cl-host`, create a tenant, seed minimal data; contract tests per resource; `openapi.json` hash fixture so spec changes force test updates. — `test(api): contract harness`
8. Turn on the 80% coverage gate in CI (Q-116).

**Tests:** money property tests (round trips, no drift, rejects ambiguous input); contract tests; 409 test using two clients; offline test returning `OfflineUnavailable`.

**Acceptance**
- [ ] Contract tests pass for companies, accounts, contacts, items, tags, and a journal entry round trip.
- [ ] 409 surfaces as `Conflict` with the server version.
- [ ] Full run passes with network blocked; coverage gate active.
- [ ] PR-P05 report has no unresolved Blocking items.

---

## P06 — Application shell (`cl-ui`)

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| L | L | P05 | rust-implementer (sonnet) | ui-verifier (sonnet) |

**Decisions:** Q-30, Q-57, Q-59, Q-66, Q-70, Q-97, Q-98, Q-119, Q-120–Q-123, Q-135, Q-137, Q-161, Q-184, Q-190–Q-193.

**Mockup:** `docs/design/mockups/Home.html` and `docs/design/screenshots/Home.png` (shell chrome only: title bar, menu, rail, tabs, status bar).

**Crates and files:** `crates/cl-ui/src/{app.rs,theme.rs,fonts.rs,shell/{title_bar.rs,menu.rs,rail.rs,tabs.rs,status_bar.rs,palette.rs},windows.rs,errors.rs,about.rs,i18n.rs}`, `crates/cl-model/src/{keymap.rs,commands.rs,forms/mod.rs,undo.rs,date_rules.rs}`, `assets/fonts/`, `assets/icon/`, `apps/clientledger/src/main.rs`, `xtask/src/keymap_doc.rs`, `docs/KEYBOARD.md`.

**Steps**
1. eframe app with theme tokens from DESIGN.md (light/dark/auto); IBM Plex Sans/Mono embedded (ATTRIBUTION.md). — `feat(ui): theme and fonts`
2. Command registry and keymap in `cl-model` (ids, labels, shortcuts, scopes form > window > global); palette (Ctrl+K); `xtask keymap-doc` generates KEYBOARD.md and the printable cheat-sheet source (Q-161); Ctrl+/ overlay. — `feat(model): command registry and keymap`
3. Shell: title bar with company name, period, closing date, and company accent stripe (Q-184); menu bar (native macOS menu via a permissive crate such as muda; in-window on Windows/Linux, Q-70); left rail; tab strip; status bar. — `feat(ui): window anatomy`
4. Window manager: tabs, Ctrl+Tab, dirty dots, close prompts, restore per company, drag-out to OS windows via egui multi-viewport (Q-57). — `feat(ui): window manager`
5. Form framework in `cl-model`: form state machine (clean/dirty/saving/error), Save & New/Save & Close, Enter = Save & New in header fields (Q-190), Esc revert chain, undo/redo stack for unsaved edits (Q-59), date-warning rules (Q-97), payee prefill hooks (Q-191). — `feat(model): form framework`
6. Error presentation (Q-119): plain message, suggested fix, "Copy details" with scrubbed error id and log excerpt. — `feat(ui): error dialogs`
7. App zoom 80–200% (Q-66). About box (license notices, engine license scope Q-137, privacy notice Q-135, "powered by Vibe MyBooks", non-affiliation notice). — `feat(ui): zoom and about`
8. Three original icon drafts, option 1 wired in (Q-122); record in QUESTIONS.md.

**Tests:** keymap conflict test (no duplicate shortcut in one scope); form state machine unit tests; undo stack tests; date-rule tests; screenshots light/dark after P07 lands (ui-verifier runs then).

**Acceptance**
- [x] Every menu and rail entry has a command id, its shortcut, and a palette entry.
- [x] KEYBOARD.md generated and committed.
- [x] Shell renders at minimum window size and 200% zoom without clipping (verified in P07 scenario, `shell.yaml`, Windows; Q-P07f).
- [x] Screen matches its mockup per docs/design/README.md (`ui-check home --compare` reviewed by hand; deviations in Q-P07h and Q-P06j).

---

## P07 — Automation and control channel (`cl-automation`)

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| L | M | P06 | rust-implementer (sonnet) | ui-verifier, security-reviewer (opus) |

**Decisions:** Q-16, Q-72. **Crates and files:** `crates/cl-automation/src/{server.rs,control_file.rs,inspect.rs,actions.rs,screenshot.rs,scenario.rs,tools.rs}`, `crates/cl-automation/scenarios/shell.yaml`, `apps/clientledger-cli/src/ui.rs`, `xtask/src/ui_check.rs`.

**Steps**
1. `--control <path>`: loopback listener on a random port; write port + random token to the control file with current-user-only permissions; refuse symlinks and files owned by or readable by others; app never starts the listener without the flag. — `feat(automation): control channel`
2. Methods: `inspect` (AccessKit tree: id, role, label, bounds, focus, enabled), `click`, `type`, `key`, `focus`, `command`, `screenshot`, `resize`, `state`, `quit` (prompts if dirty; never discards). — `feat(automation): methods`
3. CLI `clientledger-cli ui --control <path> <method> …`. — `feat(cli): ui commands`
4. Scenario runner: YAML steps (actions + assertions: exists, text equals, focused, enabled, screenshot); runs light and dark; saves PNGs to `target/ui-check/<screen>/`. — `feat(automation): scenario runner`
5. `xtask ui-check <screen>`: builds, launches with a temp control file, runs the scenario; Linux CI under Xvfb, Windows/macOS on PRs. `--compare` opens the sample company at the mockup's story date, sizes the window to 1440 px wide, and writes `target/ui-check/<screen>/compare.html` showing the app's light screenshot beside `docs/design/screenshots/<Screen>.png` (Q-226). — `feat(xtask): ui-check`
6. Tool table: every command exported with JSON schema (`clientledger-cli tools`) for a future MCP server (Q-16). — `feat(automation): tool table`
7. Shell scenario `shell.yaml`. — `test(ui): shell scenario`

**Tests:** control-file safety tests (symlink, foreign owner, world-readable); no-socket test without the flag; shell scenario.

**Acceptance**
- [ ] Shell scenario passes in Linux CI and on PR runs for Windows and macOS. (Passes on Windows locally; CI jobs `ui-check-linux` and `ui-check-other` pending a push.)
- [x] Control-file safety tests pass; no listening sockets without `--control`.

---

## P08 — Company lifecycle (`cl-company` + UI)

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | L | P05, P06 | rust-implementer (sonnet) | reviewer (opus), ui-verifier |

**Decisions:** Q-41, Q-44, Q-45, Q-48, Q-49, Q-51, Q-56, Q-127, Q-182–Q-185, Q-210, Q-219. **Engine:** U-05.

**Mockup:** `docs/design/mockups/OpenCompany.html` and `docs/design/screenshots/OpenCompany.png`.

**Crates and files:** `crates/cl-company/src/{lifecycle.rs,backup.rs,restore.rs,retention.rs,export_all.rs,sample.rs,templates.rs}`, `crates/cl-ui/src/screens/company/{open.rs,new_interview.rs,opening_balances.rs,recovery_key.rs,delete.rs}`, `assets/coa/{general_service,contractor,retail,farm}.json`, `fixtures/sample/ozark_ridge/*`, `xtask/src/gen_sample.rs`.

**Steps**
1. Open Company window: recent list, open, new, restore, sample company; one company open at a time; switching only via File › Open (Q-182, Q-183), saving or prompting open forms first. — `feat(company): open window`
2. New Company interview (plain language): names, entity type, fiscal year start, standard or 4-4-5, COA template (four, Q-49, with unique account numbers), sales tax?, inventory?, accrual/cash default (Q-44), closing-date password, company color (Q-184). — `feat(company): setup interview`
3. Recovery key (Q-210): generated at the end of setup, shown once with Print/Save, confirmation required; engine stores only a hash. — `feat(company): recovery key`
4. Guided opening balances (Q-45): start date; bank/card balances, open invoices, unpaid bills, other balances; post; review balance sheet. — `feat(company): opening balances`
5. Back Up `.clbackup` (zip: engine tenant export + attachments + manifest with app version, engine commit, schema version, trial balance, SHA-256 of parts); Restore (verify manifest → import as new tenant → compare trial balance → swap names → keep old tenant until the user confirms); ~~Portable copy~~ (removed, Q-268: Back Up + Restore moves a company); backup-on-close setting; retention (last 10 + monthly for 12). Full-database `pg_dump` under File › Utilities. — `feat(company): backup and restore`
6. New from existing (Q-127); Delete company (Q-185: Admin, typed name, verified backup first); Export All Data zip of CSV/Excel (Q-56). — `feat(company): copy, delete, export all`
7. Year-end (Q-48): verify the engine rolls net income to retained earnings automatically; prompt to set a closing date after year-end. — `feat(company): year-end prompt`
8. Sample company generator (`xtask gen-sample`): Ozark Ridge Landscaping LLC, one year, ~1,500 transactions that reproduce the sample figures in docs/design/README.md and the records shown in the mockups (Q-219, Q-226), with a golden test asserting each figure. — `feat(xtask): sample company`

**Tests:** backup → restore on a clean data dir with TB equality; tampered backup refused; restore failure leaves the live company intact; ten sample-size companies open and switch under 2 s (Q-51); year-boundary retained-earnings test; Export All Data row counts; scenarios for the interview, opening balances, and recovery key.

**Acceptance**
- [ ] Create → add accounts → back up → restore elsewhere → trial balance identical.
- [ ] Bad-checksum backup refused; live company untouched.
- [ ] Restore never removes the existing company before the restored one verifies.
- [ ] Ten companies open and switch under 2 s each.
- [ ] Retained earnings correct after a fiscal year boundary.
- [ ] Screen matches its mockup per docs/design/README.md (`ui-check --compare` reviewed by ui-verifier); any deviation is listed in the PR and QUESTIONS.md.

---

## P09 — Preferences and AI provider keys (`cl-settings` + UI)

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | M | P06, P08 | rust-implementer (sonnet) | reviewer (opus), security-reviewer (opus), ui-verifier |

**Decisions:** Q-33, Q-34, Q-38, Q-54, Q-136, Q-138–Q-141. **Engine:** U-09, U-11.

**Mockup:** `docs/design/mockups/Preferences.html` and `docs/design/screenshots/Preferences.png` (AI panel; other panels follow its layout).

**Crates and files:** `crates/cl-settings/src/{user_prefs.rs,secrets.rs,ai.rs,smtp.rs}`, `crates/cl-ui/src/screens/preferences/{company.rs,user.rs,ai.rs,email.rs,folders.rs}`.

**Steps**
1. User preferences stored locally (theme, date format, negatives style, start screen, zoom, backup on close, date-warning windows Q-97, bills-due days Q-169). Company preferences through the engine (basis, closing date, sales tax, inventory, finance charges, check options, default accounts, 1099 box mapping Q-53). — `feat(settings): preferences`
2. Secrets in the OS keychain (`keyring`): AI keys, SMTP password; redacted in UI after save; never in logs, backups, transfers, screenshots, or error details. — `feat(settings): keychain secrets`
3. AI panel: off by default; provider Anthropic or OpenAI (key, online) or Ollama (base URL, model, offline); cloud disclosure with consent recorded in the audit trail (date, user, provider, disclosure version, Q-136); test button; per-feature toggles (Q-140); usage meter and monthly cap (Q-138). — `feat(settings): ai providers and controls`
4. Email panel: SMTP host, port, TLS, user, password, from address; test send (Q-34). — `feat(settings): smtp`
5. Receipt inbox folder picker (Q-54); update-check setting (manual).

**Tests:** network-capture test that no AI request happens when AI is off or keyless; Ollama-offline test with a local stub; payload redaction test against fixtures; cap enforcement test; secrets-leak scan of logs/backups/screenshots; preferences round trip through backup/restore.

**Acceptance**
- [ ] No AI request when AI is off or no key is set.
- [ ] Ollama works with networking disabled; nothing external is contacted.
- [ ] Keys never appear in logs, backups, transfer files, or screenshots.
- [ ] Captured payloads contain no account numbers, tax IDs, emails, or phone numbers.
- [ ] Monthly cap blocks further cloud AI calls.
- [ ] Company preferences survive backup/restore.
- [ ] Screen matches its mockup per docs/design/README.md (`ui-check --compare` reviewed by ui-verifier); any deviation is listed in the PR and QUESTIONS.md.
