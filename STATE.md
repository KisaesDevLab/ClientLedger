# STATE.md — ClientLedger

**Current phase:** P09 — Preferences and AI provider keys (Next); P02 engine PRs E5/E6 pending
**Status:** P08 built on `phase/P08-company-lifecycle`; not pushed, no PR yet
**Engine commit:** `13ebd2ba50db0e3e68dd473329caf96d1f039e4e` (`main`, pinned 2026-10-15; bump per Q-02)
**Last updated:** 2026-10-17
**Tiers:** H = opus implementer + architect + opus review · M = sonnet implementer + opus review · L = sonnet (see docs/AGENTS.md)

## Phase board — beta track

| Phase | Title | Tier | Status |
| --- | --- | --- | --- |
| P00 | Repository bootstrap | L | Done 2026-10-15 |
| P01 | Engine submodule and single-executable build | L | Done 2026-10-15 (deferred criterion closed in P02: smoke requires 200 and the pinned commit, Q-P01a) |
| P02 | Engine desktop profile (upstream) | H | In progress |
| P03 | Bundled PostgreSQL manager | L | Done 2026-10-15 |
| P04 | Supervisor | L | Built, CI green, security-reviewed (PR #5, stacked on #4) |
| P05 | Engine API client | H | Done 2026-10-17 (AC4 real-engine test blocked by U-17) |
| P06 | Application shell | L | Done 2026-10-17 (AC3/AC4 closed in P07: scenario and Home --compare, deviations in Q-P07h) |
| P07 | Automation and control channel | L | Done 2026-10-17 |
| P08 | Company lifecycle | M | Done 2026-10-17 (AC1-AC3 PENDING by `tenant-archive-v1`; delete by `tenant-delete-v1`; recovery key by `recovery-key-v1`; closing password by `closing-password-v1`) |
| P09 | Preferences and AI provider keys | M | |
| P10 | Ledger grid widget | H | |
| P11 | Lists, centers, and tags | M | |
| P12 | Bank and account register | H | |
| P13 | Write Check | M | |
| P14 | Deposits, transfers, sales receipts, card charges, owner and loan helpers | M | |
| P15 | Journal entries, memorized and recurring | H | |
| P16 | Home workspace | L | |
| P17 | Enter Bill, vendor credits, receipt inbox, attachment viewer | M | |
| P18 | Pay Bills (checks) | M | |
| P19 | Form printing | L | |
| P20 | Create Invoice | M | |
| P21 | Receive Payment | H | |
| P22 | Credit memos, refunds, statements, finance charges | M | |
| P23 | Sales tax | H | |
| P24 | Inventory (average cost) | H | |
| P25 | Bank file import (QIF, QFX, OFX, CSV) | H | |
| P26 | Bank feed review | M | |
| P27 | Reconcile | H | |
| P28 | Report engine UI | H | |
| P29 | Users, permissions, closing date, audit trail | M | |
| P30 | Payroll import | M | |
| P31 | QuickBooks Desktop import with tie-out, and spreadsheet import | H | |
| P32 | Transfer file for MyBooks Online, and IIF export | H | |
| P33 | LAN multi-user | H | |
| P34 | Backup scheduling, recovery, and data integrity | H | |
| P35 | Packaging | L | |
| P36 | Updates and migrations | H | |
| P37 | Hardening | L | |
| P38 | Documentation | L | |
| P39 | Beta release (STOP for owner sign-off) | L | |

## Post-beta (owner may reorder, Q-39)

| Phase | Title | Tier | Status |
| --- | --- | --- | --- |
| P40 | Estimates, progress invoicing, sales orders, recurring invoices, retainage | H | |
| P41 | Purchase orders and item receipts | M | |
| P42 | Inventory assemblies | H | |
| P43 | Plaid bank sync | M | |
| P44 | Report Packs | L | |
| P45 | Accountant tools | M | |
| P46 | Workpapers and Tax Estimator | M | |
| P47 | Custom fields, form template designer, job costing | M | |
| P48 | Statement of cash flows and calendar | M | |

## Blocked items

Format: `Pxx — blocked by U-xx/Q-xx — what is pending — workaround in place`.

- P39 (release) — owner decision needed on Q-P03e — the bundled PostgreSQL archives ship LGPL libraries (gettext/libiconv on Windows) and OpenSSL, ICU, libxml2 and others on Linux/macOS; the owner must confirm the licensing position (license texts shipped, any written source offer) before beta release — workaround in place: libraries kept unmodified and listed in ATTRIBUTION.md.

- P05 AC4 — blocked by U-17 — real-engine 409 contract test is PENDING(record-version-v1) — stub contract covers the client (`If-Match` sent, 409 `STALE` mapped to `Conflict { Stale }`, fingerprint check sends nothing).

- P08 AC1 (create, back up, restore elsewhere, TB identical), AC2 (bad-checksum backup refused), AC3 (restore never removes the existing company first) — blocked by U-05 E5a (`tenant-archive-v1`; built on the engine branch, not yet pinned) — the live round trip `lifecycle::archive_export_import_e5a` is PENDING — in place: cl-api types the export job / import / rename contract (stub tests), cl-company refuses non-archives and changed files before anything is sent, restores as a new tenant and keeps the previous one (wiremock flow tests), the Restore panel shows the engine's verification.
- P08 Delete Company (Q-185) — blocked by U-05 E5b (`tenant-delete-v1`) — `lifecycle::safe_delete_e5b` is PENDING — in place: the window explains it and keeps the button off; cl-company refuses without a before-delete backup path and SHA-256 (nothing sent) and removes the local record only after the engine's receipt.
- P08 Recovery key (Q-210) — blocked by U-23 (`recovery-key-v1`) — `lifecycle::recovery_key_u23` is PENDING — in place: the screen (show once, copy, save, confirm) runs on the key `/desktop/initialize` returns or a rotated one; without the capability it says so.
- P08 Closing-date password in the interview — blocked by U-15 (`closing-password-v1`) — `lifecycle::closing_password_u15` is PENDING — in place: the fields appear only with the capability; cl-api refuses to write `lockDate` without it.
- P08 Full Database Backup (File > Utilities) — needs a supervisor-side `pg_dump` API while the cluster runs — placeholder window; P34 (Q-P08l).

## Placeholders tracked

Screens or actions that link to a later phase. Remove each line when its phase lands.

- P34/P08 — restore UI must call `Cluster::restore(src, store, allow_unregistered)` with `allow_unregistered = false` by default and set it to `true` only after an explicit owner confirmation that the file came from a trusted source (Q-P03h).

- P06 shell placeholders (Q-P06g): every menu, rail and palette command whose screen is built later opens a placeholder tab. The phase that builds the screen removes its entry: P09 Preferences (and may move `cl-ui::prefs` into `cl-settings`), P11 Chart of Accounts / Item List / Tags / Clients & Jobs / Vendors, P12 Open Register, P13 Write Check, P14 Make Deposit / Transfer Funds / Sales Receipt / Record Card Charge, P15 Make Journal Entry, P16 Home (replaces the Home placeholder and the demo rail counts), P17 Enter Bill, P18 Pay Bills, P20 Create Invoice, P21 Receive Payment, P22 Credit Memo or Refund / Statements, P25 Import Bank File, P26 Bank Feed, P27 Reconcile, P28 Reports / Profit & Loss / Balance Sheet / Trial Balance, P29 Set Closing Date / Users and Permissions / Audit Trail, P30 Import Payroll, P32 Send to My Accountant, P38 bundled help (F1 says "not available yet").
- P06 native macOS menu bar (Q-70, muda) is deferred (Q-P06b): in-window menu on every platform until macOS packaging.

## Follow-ups from P03 security review

Findings from the P03 security review that were not fixed in P03. Fixed in P03: 1 (crafted backup runs code on restore), 2 and 15 (stop signalling an unrelated process), 3 (backup file mode on Unix), 5, 8, 12, 13, 14. Each item below names the phase that must pick it up; remove the line when it lands.

| # | Follow-up | Target |
|---|-----------|--------|
| 4 | Limit extraction size and check free disk space before unpacking a backup (a hostile archive can expand without bound). Company restores go through the engine's import, which caps sizes (P02 §2.5); the whole-cluster restore UI is P34. | P34 |
| 6 | A keychain hiccup must not downgrade an already initialized cluster to the file fallback store. | P09 |
| 7 | Derive the fallback file key from OS APIs, not environment variables (user name, computer name). | P09 |
| 9 | Truncate `postgresql.auto.conf` on every start (restore already resets it). | next `cl-pg` touch (P04) |
| 10 | The engine must connect as a non-superuser application role, not the `postgres` superuser. | P04 |
| 11 | Owner-only ACL on the data directory, the `.secret` file, and backups on Windows (Unix backups are already 0600). | P04 / P09 |
| 16 | Treat gvfs `smb-share:`, `sftp:`, and `dav:` mounts as network drives in `check_data_dir`. Not done in P08 (no `cl-pg` touch); next `cl-pg` touch or P35. | P35 |
| 17 | Canonicalize the parent folder when checking that a backup is not inside the data directory. | next `cl-pg` touch |
| 18 | `pg-fetch`: pass `--proto-redir =https --tlsv1.2` to curl, and consider verifying the Maven `.asc` signature when the pinned hashes are bumped. | next hash bump / P35 |

Notes kept for later phases:

- `startup.log` (the `pg_ctl -l` file) must stay outside the data directory: on Windows, after a crash the server fsyncs every file in the data directory and fails with a sharing violation on a log file that is still open (stalls startup).
- Pid-file ownership uses the pid, process name, and data directory from `postmaster.pid`; the process start time is not compared (no portable OS call without more dependencies).
- A running server whose pid file shows non-loopback settings is stopped and restarted (unit-tested on the pid file fields; no integration test starts a wider server).

## Follow-ups from P04 security review

No Critical or High findings. Fixed on `phase/P04-supervisor`: M1, M2, L1, L2, L5, L6, L7, L9, L10, L11 (see Q-P04p). Recorded, not fixed:

| # | Follow-up | Target |
|---|-----------|--------|
| M3 | Windows custom or portable data folders inherit "Authenticated Users: Modify": set an owner-only ACL or refuse such roots; the file-store key derivation is not protective on shared folders (Q-P04l, Q-P03c). | P35 / P09 |
| L3 | Fail closed after N failed listener lookups instead of warning forever (Q-P04m). | P37 |
| L4 | `cl-api` re-reads endpoint and generation for every request and verifies the listener before the first request after a restart; host `info`/`initialize` should verify the listener first (Q-P04n). | P05 |
| L8 | Engine exits when stdin closes / `PR_SET_PDEATHSIG` (added to U-16); write `run/engine.pid` atomically with the spawn (Q-P04o). | U-16 / P37 |
| Info | Suspended start under the job object and fail if job creation fails; no Node diagnostic reports; WER exclusion; secrets never rotate, so P34 recovery guidance must cover them (Q-P04o). | P35 / P34 / P37 |

## Follow-ups from P05 reviews

Fixed on `phase/P05-engine-client`: scope pinning (both reviews), Retry-After overflow (L1), Call `Debug` (L2), endpoint unpublished on engine exit (L3), candidate id parsing (L5), test-feature guard (L6), DNS block and checkout credentials (L7), code-review should-fix 1-4 and the nits (Q-P05ad). Recorded, not fixed:

| # | Follow-up | Target |
|---|-----------|--------|
| L4 | Best-effort secret wipe: zeroizing/constant-time `cl_pg::Secret` (or re-export `cl_core::Secret`), pre-sized `Zeroizing` body buffers (Q-P05aa). | P09 / P37 |
| L8 | Lint or review rule against logging `ApiError` payloads, models or amounts with `?value` at info level (Q-P05ab). | P37 |
| L9 | 64 KiB cap for engine error bodies (Q-P05ac). | P37 |
| AC5 | No contract test yet moves a transaction OUT of a closed period (old date closed, new date open); the engine checks both dates, the stub suite covers the client mapping. | P07 (with U-15) |

## Log

Newest first. One entry per completed phase: date, phase, summary, acceptance checklist results, notable decisions (Q-xx / U-xx).

- 2026-10-17 — P08 review round (code review, security review, ui-verifier; PR #9 CI green except coverage 79.83%). Fixed on the branch, one commit per group: archive job shapes matched to the engine E5a branch (`queued|running|succeeded|failed`, `error:{code}`, import as a job, echoed `host`) with one `wait_for_job`; restore renames with the row's `updatedAt`, writes the local record first and treats a failed swap as non-fatal; lifecycle operations run on the write lane, are counted on the link and pinned to their company (flow test: a switch mid-load posts nothing into the other company); AC2 tamper case added to the live round-trip test; passphrase messages; delete tombstones with the receipt; retention from the backup history; retail-calendar year-end; recovery key never rotates by itself, key file outside the app folder and never overwritten, zeroized `cl_pg::Secret`, bounded zip reads, key masked during screenshots; Esc reverts then prompts, Close Company finishes after the prompt, labelled pickers, plain wording; headless render tests for every company screen (local `cargo llvm-cov --workspace --exclude xtask`: 85.25% lines, 86.01% functions, with the engine-checkout fixture test skipped because the sibling E5a checkout changes `openapi.json`). Decisions Q-P08o and amendments to Q-P08a/c/g/j and Q-127. Open after the round: Print/Save show-once passphrase (Q-P08g deviation), AC4 partial.
- 2026-10-17 — P08 complete on `phase/P08-company-lifecycle` (6 commits; not pushed). Built: `cl-api` P08 resources (tenants list/create/rename, archive export job + import + download, safe delete, recovery key, closing-date writer gated on `closing-password-v1`, company-settings writer, COA template options, bulk account import, opening balances, invoices and bills, balance sheet / trial balance (CSV for equality) / P&L, export manifest and files; PATCH method, `Archive` timeout class, `passphrase`/`recoveryKey`/`newPassword` as credential keys; 22 new operations, spec fixtures updated); `cl-settings` (recent-companies store with accent colours, backup history and local flags; keychain `SecretStore`); `cl-company` (four starter charts in `assets/coa`, interview model and `create_company`, list/open/new-from-existing, backup with retention and `manifest.host`, restore with tamper check and name swap, Export All Data zip, delete, year-end prompt rule, practice-company loader); `cl-ui` screens (Open Company with first-run setup and sign-in, interview, recovery key, opening balances, Back Up / Portable / Export, Delete) wired through a shared engine link with a demo mode; `xtask gen-sample` and `fixtures/sample/ozark_ridge` (1,512 transactions, 214 attachments, 37 feed items) with the golden test for every README figure. Acceptance: AC1-AC3 (backup/restore round trip, bad checksum refused, old company kept) PENDING by `tenant-archive-v1` with the client, flow tests and screens in place; AC4 ten companies open and switch under 2 s — partially met (10 switches across 4 near-empty companies, slowest 41 ms; the engine's auth-rate budget caps creation at 8 per test binary, and the practice fixture is loaded through the API, not as a fixture, so a ten-company timing on real-sized books waits for P37's performance pass); AC5 retained earnings after a fiscal-year boundary — met (`lifecycle::year_boundary_retained_earnings`: virtual `30120` row on the balance sheet and TB CSV, net income only for the new year); AC6 mockup match — `ui-check open-company --compare` reviewed by hand (ui-verifier not run), deviations in Q-P08k. Scenarios open-company, interview, recovery-key, opening-balances and shell pass in light and dark on Windows. Contract suite: 45 passed (5 PENDING by capability). Decisions Q-P08a-n; UPSTREAM U-23 to U-28 added, U-15 provenance. Known: `cargo xtask api-spec --check` and the cl-api/xtask spec-fixture tests fail on this machine only because the engine working tree is on the sibling agent's E5a branch (regenerated `openapi.json`); the fixtures match the pinned commit `850e43d2` (verified by hash). Agents: rust-implementer on fable.
- 2026-10-17 — P07 complete (PR #8, merge commit). Built: `cl-automation` (control file created owner-only, refusing symlinks, foreign owners, other-readable and hard-linked files; on Windows it must sit under `%LOCALAPPDATA%` until the P37 ACL check, Q-P07b; loopback JSON-lines server with a random token, started only by `--control`, 5 s first-line deadline, 8 connections, 64 KiB lines; method table with JSON schemas; secret redaction covering descendant text runs and secret-looking labels; scenario runner and client; PNG encoding), `cl-ui::automation::Bridge` (AccessKit tree, inspect/click/type/key/focus/command/screenshot/resize/state/quit/set_theme/set_zoom; theme and zoom set by the channel last for the run only), `clientledger --control`, `clientledger-cli ui …` (incl. `type --stdin`, port-owner pid check before the token is sent) and `tools`, `xtask ui-check [--compare]` (tempfile dir), `scenarios/shell.yaml`, CI jobs `ui-check` on Linux (Xvfb), Windows and macOS. `EndpointSource::current_for(Purpose)`: every call whose body carries a password, code or token re-verifies the engine listener (closes P06 L1 / Q-P06o). Acceptance: control-file safety tests pass on all three OSes - met; no listening socket without `--control` - met; shell scenario passes in Linux CI and on Windows/macOS PR runs - met (64 steps, light and dark). P06 deferred criteria closed: 960 x 640 minimum and 200% zoom without clipping - met (scenario assertion + screenshot review; two shell defects found and fixed: rail under the status bar, rail repeating windows); Home mockup `--compare` - met, reviewed by hand (ui-verifier subagent not run), deviations in Q-P07h. Security review (opus): no critical/high; M1 inspect redaction bypass via child text nodes and M2 over-broad Windows control-file root fixed, L1-L8 fixed (Q-P07j); open: no `openat` no-follow open on Unix, Windows real ACL check (P37). CI fix: `Ctrl` in a chord is the platform command modifier (macOS sent a literal Control key). Decisions Q-P07a-j. Agents: rust-implementer (sonnet), security-reviewer (opus), orchestrator fix on fable.
- 2026-10-17 — P06 complete (PR #7, merge commit). Built: `cl-model` command registry (menus, rail, palette, scopes form > window > global), conflict-checked `Keymap`, form state machine, undo stack, date-key and date-warning rules; `xtask keymap-doc` (+ `--check` in `xtask ci`) generating `docs/KEYBOARD.md` and the cheat-sheet source; `cl-ui` theme tokens (light/dark, contrast-tested), embedded IBM Plex, i18n catalog, shell (title bar with company stripe, menus, rail, tabs with dirty dots and pop-out viewports, status bar, palette, shortcut overlay), window manager with per-company saved layout (hashed file names) and close prompts, error dialog with scrubbed, previewed "Copy details", About and privacy notice, zoom 80-200% and theme remembered; app binary with single-instance lock, supervisor start and rotating `ui.log` through `HostLog`; `VerifiedEndpoint` (P05 L3b); icon drafts (option 1 wired); `cargo xtask run`. Acceptance: (1) every menu and rail entry has a command id, shortcut where DESIGN.md defines one, and a palette entry - met; (2) KEYBOARD.md generated and committed - met; (3) and (4) deferred to and closed in P07. Native macOS menu deferred (Q-P06b). Security review (opus): no critical/high; M1 Copy-details scrubbing (amounts in key=value/JSON, spaced user names, keyless credentials, preview before copy) fixed, L2-L6 fixed, L1 closed in P07. Decisions Q-P06a-o. Agents: rust-implementer (sonnet), security-reviewer (opus).

- 2026-10-17 — P05 complete (PR #6, merge commit). `cl-api`: loopback typed engine client, single-flight refresh, typed errors, retry classes with outcome reconciliation, UI request queue, contract harness with PENDING-by-capability tests, 4-OS engine contract job, network-blocked job, 80% coverage gate enforced (required check `coverage`). Opus code and security reviews found scope-pinning (write running in wrong company after a switch), Retry-After overflow, unreadable 2xx saves, reconciliation claiming own earlier entry; all fixed with tests (Q-P05aa–ad); remaining items in "Follow-ups from P05 reviews". Blocked: AC4 (U-17). Agents: architect/rust-implementer x2/reviewer/security-reviewer (opus), ci-fixer (sonnet).
- 2026-10-15 — Owner: the engine is fully licensed for ClientLedger's use; engine licensing is settled and closed (Q-35, Q-P02a, U-10). Not to be raised again. P04 built on `phase/P04-supervisor` (PR #5 stacked on draft PR #4, all CI green): cl-host supervisor, least-privilege `clientledger_app` DB role, single-instance lock, log rotation, CLI host commands; independent security review fixed M1 (engine search path) and M2 (master-secret replacement) plus lows. Engine PRs: E1 #147 (ready for owner review), E2 #146 (draft), license draft #145 (owner's); E3 (pg-boss) built locally; E4 (offline guard) security fixes in progress; E5/E6 deferred to just before P08/P09 (plan settled).
- 2026-10-15 — Owner decision: engine license changes to straight PolyForm Internal Use 1.0.0 (no ClientLedger grant). The engine is currently PolyForm Small Business 1.0.0, so this is a relicense of the public `Vibe-MyBooks` repo, prepared as engine branch `clientledger/license-internal-use` for the owner to review and merge (Q-29); resolves Q-P02a. NOTICE.md, README, CLAUDE.md, ARCHITECTURE, M0/M6 and Q-31/Q-35/Q-132/Q-133/Q-134/Q-137 updated; the draft grant stays in docs/legal for reference only. Consequences: recipients may not distribute the engine (only official releases from Kisaes LLC carry it); use is limited to internal business operations, so bookkeeping for others needs a separate license; the explicit accountant-review allowance and third-party mirroring right from the draft grant are gone. Attorney review of the NOTICE summary is still advisable before P39.
- 2026-10-15 — P03 complete (PR #3, squash-merged). `cl-pg` + `xtask pg-fetch`: pinned zonky PG16 binaries (SHA-256 lock), cluster init with keychain password, data-dir guard, loopback-only start/stop, cold whole-database dump/restore. Acceptance: fresh ensure+start 3.3 s locally; second start reuses; kill -9 recovery; dump→restore identical; loopback-only confirmed by security-reviewer. Security review (opus): no critical; fixed crafted-backup code execution on restore, stop() signalling unrelated processes, world-readable backups and cheap hardening; remaining findings tracked in "Follow-ups from P03 security review". Decisions: Q-P03a–h (no pg_dump/psql in zonky archives → cold copy; restore takes a registry-override flag the restore UI must gate behind owner confirmation). Open for owner: Q-P03e LGPL/OpenSSL/ICU in bundled archives, before P39. Agents: rust-implementer x2 (sonnet), security-reviewer (opus), ci-fixer (sonnet; pg-fetch layout `share/postgresql/` on Unix archives).
- 2026-10-15 — P01 complete (PR #2, squash-merged), with one acceptance criterion deferred: `/health` reporting the pinned engine commit. The pinned engine has no commit field and returns 503 without Redis, so the smoke test accepts 503 with DB ok until U-02/U-03 land in P02 (Q-P01a). Shipped: `engine/` submodule (Vibe-MyBooks `main`, pinned 13ebd2ba), `cargo xtask engine` (esbuild + Node SEA, manifest.json, cache by commit + Node version), `--smoke`, PATH-stripped run test, 4-OS `engine` CI job. Verified: built on Windows x64, macOS arm64/x64, Linux x64 in CI; runs without Node; smoke ran all 325 migrations. Decisions: Q-P01a–c. Engine untouched; bundling workarounds in Q-P01b (puppeteer stubbed, so engine HTML-to-PDF routes fail; P04 sets CL_ENGINE_RUNTIME_DIR). Agents: rust-implementer (sonnet), ci-fixer (sonnet; Windows runner is elevated, so smoke Postgres starts via pg_ctl).
- 2026-10-15 — P00 complete (PR #1, squash-merged). Workspace skeleton (13 crates, 2 apps, xtask), MIT/NOTICE/community files, deny.toml, `xtask` license + layering + ci gates, three-OS CI with secret scan and report-only coverage. Acceptance: `cargo xtask ci` green on Ubuntu, Windows, macOS arm64 + x64; GPL dep, direct BSD-only dep, and a layering violation each failed their gate (verified once, reverted); gitleaks scan clean. Decisions: Q-P00a–c. Agents: rust-implementer (sonnet), ci-fixer (sonnet; clippy `assert_is_empty` on newer CI toolchain). Not enforced: the rule that only cl-pg/cl-host/cl-company spawn processes (not visible in the dependency graph). Follow-up: consider pinning rust-toolchain.toml to CI's clippy version. Owner enabled branch protection (required: ci x4 + secret scan), auto-merge, and secret scanning with push protection.
- 2026-10-15 — UI mockups added as the visual reference (Q-226–Q-228): 15 screens in docs/design (HTML + PNG + index); each screen phase names its mockup and gains a mockup-match acceptance item; ui-check gets `--compare`; ui-verifier reviews against mockups; sample company reproduces mockup figures; canvas-example tests now use the mockup numbers.
- 2026-10-11 — Detailed build plan written: PHASES.md is now an index; phases/M0–M7 hold per-phase decisions, engine items, crates and files, ordered commit-sized steps, tests, acceptance, and Fable checkpoints.
- 2026-10-11 — Added process-reviewer on Fable: 12 checkpoints (PR-P05, PR-P10, PR-1…PR-10), deep end-to-end reviews; security stays on Opus.
- 2026-10-11 — Multi-agent build added: 12 subagents in .claude/agents/, model routing by phase tier (docs/AGENTS.md), orchestrator on Sonnet.
- 2026-10-11 — Q&A rounds 44–48 (final): deposits, journal entries, chart of accounts, passwords and recovery key, check stubs, updates, backup reminder, QBD versions, sample size, large engine gaps. Q&A complete; only the attorney review of the grant remains.
- 2026-10-11 — Q&A round 43: service dates on lines, no line discounts, plain multi-line descriptions, hide-$0-lines option.
- 2026-10-11 — Q&A round 42: OFX account auto-link, MM/DD default, card sign auto-detect, file balance check.
- 2026-10-11 — Q&A round 41: Enter = Save & New, payee prefill without amounts, vendor default accounts, optional memos.
- 2026-10-11 — Q&A round 40: register defaults, true running balance always shown, state row colors, four quick filters.
- 2026-10-11 — Q&A round 39: one company at a time, File > Open switching, company color stripe, guarded company delete.
- 2026-10-11 — Q&A round 38: tags at transaction and line level, one per group, optional required groups, rules set tags.
- 2026-10-11 — Q&A round 37: owner-run finance charges with preview, open-item statements, open-balance batches, aging row.
- 2026-10-11 — Q&A round 36: Verify Data tool, weekly quick health check, duplicate finder, no condense.
- 2026-10-11 — Q&A round 35: no bill approvals, Positive Pay post-beta, guided vendor statement match, 7-day bills-due on Home.
- 2026-10-11 — Q&A round 34: no dunning, editable email templates, invoice PDF only, sent tracking.
- 2026-10-11 — Q&A round 33: minimum hardware 4 GB/2 cores/2 GB, offline bundled help, videos post-beta, shortcut PDF + overlay.
- 2026-10-11 — Q&A round 32: masked account numbers, admin-only closing date, audit trail export, sensitive account flag.
- 2026-10-11 — Q&A round 31: payroll summary entries, expected-withdrawal matching, departments to tags, generic payroll CSV.
- 2026-10-11 — Q&A round 30: no sales tax reminders, per-invoice rounding, negative inventory warn-and-allow, reorder alerts.
- 2026-10-11 — Q&A round 29: QBD import history depth by owner choice, no memorized or user import, guided spot-check list.
- 2026-10-11 — Q&A round 28: AI usage meter + cap, redacted minimal data, per-feature toggles, rule suggestions from accepts.
- 2026-10-11 — Q&A round 27: installer license acceptance, privacy notice, recorded AI consent, scope note in About/docs.
- 2026-10-11 — Grant placeholders filled: Kisaes LLC (Missouri) dba KisaesDevLab, Missouri law, no venue clause, effective on commit, licensing@kisaes.com, GitHub org KisaesDevLab. Engine repo: KisaesDevLab/vibe-mybooks.
- 2026-10-11 — Engine grant drafted (docs/legal/LICENSE-CLIENTLEDGER.draft.md) for attorney review: own-organization use only; official releases only.
- 2026-10-11 — Q&A round 26: multiple ship-to addresses, contact people with roles, notes and to-dos, no credit limits.
- 2026-10-11 — Q&A round 25: no other-app imports in beta, CSV transaction import, IIF export, New from existing.
- 2026-10-11 — Q&A round 24: desktop app owns the ClientLedger name and site, GitHub Releases downloads, drafted icon options, app name ClientLedger.
- 2026-10-11 — Q&A round 23: 80% workspace coverage, Rust 2024, 30-day/100 MB logs, plain errors with Copy details.
- 2026-10-11 — Q&A round 22: continuous runs, branch + PR per phase with auto-merge, Linux CI per push and all OSes on PR/nightly.
- 2026-10-11 — Q&A round 21: optional host service, LAN only (no remote), no work while host is down, 5 concurrent users.
- 2026-10-11 — Q&A round 20: report columns, fixed aging buckets, Home charts, full report headers.
- 2026-10-11 — Q&A round 19: reconcile pre-check from imports, optional auto-accept rules, transfer detection, no split rules.
- 2026-10-11 — Q&A round 18: UOM labels only, configurable date warnings, no spell check, two-sided items.
- 2026-10-11 — Q&A round 17: logo on built-in template, #10 window envelopes, prefixed auto-numbering, payment-instructions footer.
- 2026-10-11 — Q&A round 16: refund methods, exemption certificates, job status/type/dates, audit trail kept forever.
- 2026-10-11 — Q&A round 15: manual processor deposits, personal-funds expenses, daily sales summary, guided vendor refunds.
- 2026-10-11 — Q&A round 14: customer early-pay discounts, customer deposits as liability, bounced-check action in beta; retainage post-beta.
- 2026-10-11 — Q&A round 13: no tax-line mapping, contacts can be client and vendor, no Other Names list, account numbers on by default.
- 2026-10-11 — Q&A round 12: private-fixtures folder, four AI features incl. ask-a-question reports, MyBooks-in-Docker transfer CI, staged beta with own clients first.
- 2026-10-11 — Q&A round 11: no pay links, optional auto-lock, native macOS menu, in-app notifications only.
- 2026-10-11 — Q&A round 10: simple P&L budgets and app zoom in beta; cash flow statement and calendar post-beta (new P48).
- 2026-10-11 — Q&A round 9: full card flow, owner pay helper, loan split helper, Home setup checklist.
- 2026-10-11 — Q&A round 8: Export All Data, tabs with pop-out windows, discount/price-level/subtotal items, form-level undo.
- 2026-10-11 — Q&A round 7: recurring invoices post-beta, 1099 flag + summary in beta, receipt inbox folder, confirm-after-print check numbering.
- 2026-10-11 — Q&A round 6: automatic year-end, four starter charts of accounts, 10 MB attachments, 10-company target.
- 2026-10-11 — Q&A round 5: deletes allowed and logged, US only, local data with backups anywhere, versioned transfer files, accrual default, guided opening balances, SemVer, GitHub support.
- 2026-10-11 — Q&A round 4: Q-21–Q-23 settled for post-beta; engine repo public before P01. Only Q-35 (grant text, owner) remains.
- 2026-10-11 — Q&A round 3: PDFium viewer, PDF + OS print, branding confirmed. Open: Q-35 (engine grant, STOP before P39); Q-21–Q-23 deferred to post-beta.
- 2026-10-11 — Q&A round 2: engine tracks main; companies stay engine tenants; parsers in Rust; AI providers Anthropic, OpenAI, Ollama; SMTP email; owner merges engine PRs.
- 2026-10-11 — Plan revised after owner Q&A: free MIT public repo, no licensing, owner audience, beta scope set, Rust/egui native UI confirmed. Next: P00.
- 2026-10-11 — Plan created.
