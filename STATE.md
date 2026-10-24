# STATE.md — ClientLedger

**Current phase:** Checkpoint PR-1 (end of M1), then P17 — Enter Bill, vendor credits, receipt inbox, attachment viewer (Next)
**Status:** P16 built on `phase/P16-home-workspace`; PR open with auto-merge. Engine PR #168 (U-75 tag numbers, U-76 dashboard exact) awaits the owner's merge; then bump the pin (Q-02)
**Engine commit:** `c217bfd7e1f72b03a5e5ad3aaa21120da3d36246` (`main`, pinned 2026-10-21 after engine PR #163: batch-atomic-v1, batch-check-v1, batch-mixed-v1, batch-exact-v1, batch-idempotent-v1, batch-closing-v1, migration 0226; bump per Q-02)
**Last updated:** 2026-10-24
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
| P08 | Company lifecycle | M | Done 2026-10-17 (AC1-AC3 and the closing password live since pin 5c8e9665; delete PENDING by `tenant-delete-v1`; recovery key by `recovery-key-v1`) |
| P09 | Preferences and AI provider keys | M | Done 2026-10-18 (session secrets, desktop AI, SMTP session password and token cap live since pin 5c8e9665) |
| P10 | Ledger grid widget | H | Done 2026-10-18 (reviews pending; CI to confirm grid-bench and both ui-check jobs) |
| P11 | Lists, centers, and tags | M | Done 2026-10-18 (engine gaps PENDING by U-31 to U-46; reviews pending) |
| P12 | Bank and account register | H | Done 2026-10-19 (AC1 register part, password path and delete live since pin 5c8e9665; R5 PENDING by `register-cleared-v1`; reviews pending) |
| P13 | Write Check | M | Done 2026-10-20 |
| P14 | Deposits, transfers, sales receipts, card charges, owner and loan helpers | M | Done 2026-10-20 |
| P15 | Journal entries, templates and recurring | H | Done 2026-10-21 |
| P15A | Bulk Entry | M | Done 2026-10-21 |
| P16 | Home workspace | L | Done 2026-10-24 (tag numbers and exact dashboard figures PENDING by U-75/U-76, engine PR #168) |
| PR-1 | Checkpoint: core ledger process review | — | Next |
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
| P31 | QuickBooks Desktop import with tie-out, spreadsheet import, other accounting systems | H | |
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

- P08 Delete Company (Q-185) — blocked by U-05 E5b (`tenant-delete-v1`) — `lifecycle::safe_delete_e5b` is PENDING — in place: the window explains it and keeps the button off; cl-company refuses without a before-delete backup path and SHA-256 (nothing sent) and removes the local record only after the engine's receipt.
- P08 Recovery key (Q-210) — blocked by U-23 (`recovery-key-v1`) — `lifecycle::recovery_key_u23` is PENDING — in place: the screen (show once, copy, save, confirm) runs on the key `/desktop/initialize` returns or a rotated one; without the capability it says so.
- P08 Full Database Backup (File > Utilities) — needs a supervisor-side `pg_dump` API while the cluster runs — placeholder window; P34 (Q-P08l).
- P11 Chart of Accounts merge — blocked by U-31 (engine merge moves no journal lines; `/accounts/:id/ledger` empty) — merge not built — in place: merge hidden (`merge_available: false`), QuickReport uses `/reports/account-report` (Q-P11a, Q-P11i).
- P11 parent-plus-sub-account balance — blocked by U-32 — PENDING — in place: each account's own engine balance only, with a note under the chart; `parentId` checked in `cl_model::lists::accounts::check` (Q-P11b).
- P11 sensitive accounts enforcement — blocked by U-33 (and P29) — PENDING — in place: the mark is stored locally in `CompanyPrefs.sensitive_accounts` and shown (Q-P11c).
- P11 masked bank or card number with logged Admin reveal (Q-154) — blocked by U-34 — PENDING — in place: the field is shown switched off with "not available yet" on bank and credit-card accounts (Q-P11d).
- P11 jobs in the Client center (Q-90) — blocked by U-35 — `contacts::jobs_u35` is PENDING — in place: clients only; the Jobs tab says jobs aren't available yet (Q-P11j).
- P11 several ship-to addresses and contact people with roles (Q-128, Q-129) — blocked by U-36 — `contacts::addresses_u36` is PENDING — in place: one bill-to and one ship-to, single person/email fields, with a note in the pane (Q-P11k).
- P11 dated notes and to-dos (Q-130) — blocked by U-37 — `contacts::notes_u37` is PENDING — in place: the single notes field in the Notes tab, with a note that dated notes and to-dos arrive later (Q-P11l).
- P11 per-contact open balance — blocked by U-38 — `contacts::open_balance_u38` is PENDING — in place: figures from the client balance summary and vendor aging reports, labelled with their source, never summed in the UI (Q-P11m).
- P11 recording a paper W-9 — blocked by U-44 — `contacts::w9_on_file_u44` is PENDING — in place: W-9 on file shown read-only, the form says recording it isn't available yet (Q-P11n).
- P11 item kinds other than service, the purchase side of two-sided items and the unit label (Q-58, Q-96, Q-99) — blocked by U-39 — `items::item_types_u39` is PENDING — in place: service items only, the panel says the rest isn't available yet (Q-P11q).
- P11 price levels (Q-58) — blocked by U-40 — `items::price_levels_u40` is PENDING — in place: a note under the Item List (Q-P11r).
- P11 tag merge for tags used on lines or as defaults, one tag per group in the engine, group reorder — blocked by U-41 — `tags::line_tag_merge_u41`, `tags::single_select_u41`, `tags::group_reorder_u41` are PENDING — in place: merge offered only for header-only tags, the forms enforce one per group, groups keep the engine's order (Q-P11t, Q-P11v).
- P11 required tag groups by account type (Q-180) — blocked by U-42 — `tags::required_groups_u42` is PENDING — in place: kept on the company record and enforced by the forms' rule check (Q-P11u).
- P11/P13 several tags on one line (Q-179, Q-P10t) — blocked by U-45 — `tags::several_line_tags_u45` is PENDING — in place: one tag per line, refused in plain words before saving (Q-P11w).
- P11 your own terms (including discount terms) and payment methods — blocked by U-43 — `list_csv::terms_lists_u43` is PENDING — in place: the engine's fixed lists, read-only, with a note (Q-P11y).
- P11 item CSV import in one call and full contact rows — blocked by U-46 — `list_csv::items_import_u46` is PENDING — in place: items created one at a time; contacts import name, company, email and phone only (Q-P11x).
- P12 toggling cleared marks in the register (R5) — blocked by `register-cleared-v1` — in place: ✓/R read-only (Q-P12k). Also still PENDING for P12: U-17 (server-side version check; fingerprint stopgap) and U-18 (idempotency keys; lost-reply probe).
- P16 tag numbers — blocked by U-75 (`tag-number-v1`, engine PR #168, owner to merge) — `tags::numbers_u75`, `tags::number_archive_round_trip_u75` are PENDING — in place: the Number field and "number · name" display are built and gated on the capability.
- P16 exact Home figures (lane footers, overdue invoices, cash position) — blocked by U-76 (`dashboard-exact-v1`, engine PR #168) — `home::exact_strings_u76` is PENDING — in place: "—" for lane figures, balances from each account's exact stored balance, overdue invoices hidden (Q-P16g, Q-P16i). After the pin bump run `CL_API_REQUIRE_CAPS=tag-number-v1,dashboard-exact-v1`; the engine schema hash changes then.

## Placeholders tracked

Screens or actions that link to a later phase. Remove each line when its phase lands.

- P34/P08 — restore UI must call `Cluster::restore(src, store, allow_unregistered)` with `allow_unregistered = false` by default and set it to `true` only after an explicit owner confirmation that the file came from a trusted source (Q-P03h).

- P06 shell placeholders (Q-P06g): every menu, rail and palette command whose screen is built later opens a placeholder tab. The phase that builds the screen removes its entry: P11 Item List / Tags / Clients & Jobs / Vendors, P12 Open Register, P13 Write Check, P14 Make Deposit / Transfer Funds / Sales Receipt / Record Card Charge, P17 Enter Bill, P18 Pay Bills, P20 Create Invoice, P21 Receive Payment, P22 Credit Memo or Refund / Statements, P25 Import Bank File, P26 Bank Feed, P27 Reconcile, P28 Reports / Profit & Loss / Balance Sheet / Trial Balance, P29 Set Closing Date / Users and Permissions / Audit Trail, P30 Import Payroll, P32 Send to My Accountant, P38 bundled help (F1 says "not available yet").
- P06 native macOS menu bar (Q-70, muda) is deferred (Q-P06b): in-window menu on every platform until macOS packaging.
- P16 Home lane actions open "arrives in Pxx" placeholders (Q-P16k): P17 Enter Bill, P18 Pay Bills, P20 Create Invoice / Sales Receipt, P21 Receive Payment, P26 Review Bank Feed, P27 Reconcile, P28 Profit & Loss, P29 Set Closing Date, P32 Send to My Accountant. Each phase wires its lane action.
- P16 Home chart: a month click opens the Profit & Loss placeholder; P28 must open the report for that month's range (Q-P16h).
- P16 Home "Bank file" balance column — U-77 / P25 (Q-P16d); "ready to accept" count — P26 (Q-P16f); "invoices more than 60 days past due" — P28 aging (Q-P16g); "Send to My Accountant · last sent" — P32 (Q-P16i).
- P16 Needs attention placeholder line for to-dos, reorder alerts and the weekly health check — U-37, U-78, U-12 (Q-P16e).

## Follow-ups from P03 security review

Findings from the P03 security review that were not fixed in P03. Fixed in P03: 1 (crafted backup runs code on restore), 2 and 15 (stop signalling an unrelated process), 3 (backup file mode on Unix), 5, 8, 12, 13, 14. Each item below names the phase that must pick it up; remove the line when it lands.

| # | Follow-up | Target |
|---|-----------|--------|
| 4 | Limit extraction size and check free disk space before unpacking a backup (a hostile archive can expand without bound). Company restores go through the engine's import, which caps sizes (P02 §2.5); the whole-cluster restore UI is P34. | P34 |
| 6 | A keychain hiccup must not downgrade an already initialized cluster to the file fallback store. Not done in P09 (no `cl-pg` touch; P09 only added keychain entries through `cl-settings`). | next `cl-pg` touch / P37 |
| 7 | Derive the fallback file key from OS APIs, not environment variables (user name, computer name). Not done in P09 (no `cl-pg` touch). | next `cl-pg` touch / P37 |
| 9 | Truncate `postgresql.auto.conf` on every start (restore already resets it). | next `cl-pg` touch (P04) |
| 10 | The engine must connect as a non-superuser application role, not the `postgres` superuser. | P04 |
| 11 | Owner-only ACL on the data directory, the `.secret` file, and backups on Windows (Unix backups are already 0600). Not done in P09. | P35 / P37 |
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

## Follow-ups from P10

- S14 tag gap (Q-P10t): engine lines hold one `tagId`; the grid allows several tag chips per line. P11 opened U-45 (several tags per line, Q-P11w); until it lands, P13 refuses to save a line with more than one tag through `cl_model::lists::tags::engine_line_tag`.

## Follow-ups from P13

1. ~~A register tab left open shows stale rows after another window saves.~~ Fixed in P14 by the cross-window refresh bus (Q-262).
2. Native file picker for attachments (Q-P13k).
3. The `cl-host` cold-start budget test exceeds the 15 s local limit under load (CI uses `CL_HOST_BUDGET_SECS=30`).
4. Engine issues #157 (duplicate check numbers under concurrency) and #158 (float compare in the rules engine).

## Follow-ups from P14

1. The control channel can't click dropdown options scrolled out of view (automation limitation); scenarios choose options that are visible (Q-P14z).
2. The demo stores no tags on daily-sales days.
3. Engine issues #154, #155, #157 and #158 are still open.
4. Engine main CI (npm-audit) is failing; pre-existing, not caused by PR #159.
5. The vendor A/P aging summary lists bills only; vendor refunds are not reflected there (the vendor statement shows them).

## Follow-ups from P15

1. Engine issue #162: flaky pgboss-owner-role test (not caused by PRs #160 or #161).
2. Make Journal Entry has no side panel and no History yet (Q-P15z).
3. The demo ledger doesn't store basis.
4. A `--demo` launch skips the catch-up (File > Open Company runs it).

## Follow-ups from P15A

1. Engine issue #164: the engine accepts postings to inactive accounts (Q-P15Ar).
2. The `BATCH_BUSY` path is not exercised live (Q-P15Aq).
3. The `cl-host` cold-start budget still exceeds 15 s locally under load (CI uses 30/60 s).
4. The `cl-automation` control-port test flakes under parallel local load.

## Follow-ups from P16

1. Tag CSV export (`csv_import.rs`) doesn't carry tag numbers yet.
2. `cl-pg fresh_start` exceeds its 10 s budget under parallel local builds (passes alone).
3. This Windows dev machine has Smart App Control enforcing; it sometimes blocks a freshly built test binary (os error 4551). Re-running or touching a source file works; the owner is turning it off for development (2026-10-24).
4. The `recovery-key` scenario fails on this machine on `main` too: the long temp path is wider than the "Save to file" box (`recovery-key.yaml:44`, `assert_no_clipping`). Widen the box or let it scroll.
5. `cargo xtask keymap-doc` (shortcuts cheatsheet) omits the fixed grid keys that KEYBOARD.md lists (Shift+F10, Shift+↑/↓, Alt+↑/↓).
6. U-79 (proposed): `/bills` status filter / leave voided bills out; Home counts posted bills client-side until then (Q-P16r).
7. U-77 bank-file balance and U-78 reorder alerts proposed; U-37 and U-12 amended for Home.
8. Home's Refresh issues many engine reads per data-bus event (one per reconciled account batch plus lanes); measure before P33 LAN mode.

## Owner-requested checkpoints

- **After P13 merges, before P14 starts (owner, 2026-10-20):** Intuit compliance check — trademarks/names, copied text, icons and assets, layouts and colors vs QuickBooks, feature names, help text, fixtures, non-affiliation notice (non-negotiable 5). Report to the owner; fix findings on a `fix/` branch.

## Log

Newest first. One entry per completed phase: date, phase, summary, acceptance checklist results, notable decisions (Q-xx / U-xx).

Older entries (P00–P07, engine and planning notes) are in [docs/history/STATE-log.md](docs/history/STATE-log.md). Keep at most the last four entries here; the bookkeeper moves older ones there when a phase closes.

- 2026-10-24 — P16 complete on `phase/P16-home-workspace`; PR with auto-merge. Work moved to a new Windows 11 dev machine (toolchain reinstalled; PowerShell added to the agent definitions). Shipped:
  - Steps 0–0h:
    - "Enter moves between fields" preference (both modes on every form, Bulk Entry and list forms).
    - Fixed top and bottom bars on every screen (`layout::FixedFrame`).
    - Collapsible detail panel on Customers & Jobs and Vendors.
    - Condensed entry rows on wide screens.
    - Tag numbers ("10 · Lake Area", sorted like account numbers).
    - Bulk Entry Copy down, column chooser, multi-row selection with Set and Delete actions, and a shared cl-grid context menu.
  - Steps 1–6, the Home workspace:
    - Four lanes from the command registry.
    - Account balances.
    - Needs attention: bills, payroll imports, bank items, reconciliations, backup reminder.
    - Profit-and-loss chart drawn in-house.
    - Setup checklist.
    - Ko-fi support button (Q-278).
    - Rail counts from engine reads.
  - Control channel `hover` and `scroll` steps.
  - Engine and CI:
    - Engine PR #168 (U-75 `tag-number-v1`, U-76 `dashboard-exact-v1`, migration 0228) is open for the owner; its contract tests are PENDING.
    - CI fixes merged first: PR #32 (output drain race, cargo-deny advisories) and PR #33 (refused-connect retry race in cl-api).
  - Reviews: two fresh-eyes reviews (part 1 Bulk Entry and forms; part 2 Home) and a ui-verifier pass. Each was followed by a fresh fix pass, among them:
    - voided bills counted on Home
    - Ctrl+A never reaching the grid
    - Copy down from the blank row
    - `number_cmp` not being a total order (proptest added)
    - journal entries merging
    - a flaky scroll scenario
  - The card-balance sign on Home now matches Q-241.
  - Help pages: home, enter-key, tag-numbers, bulk-entry.
  - Decisions Q-P16a to Q-P16y; U-77, U-78 and U-79 proposed.

  Acceptance (phases/M1-core-ledger.md P16):
  - [x] Enter-moves option in both modes (unit tests + `forms-enter.yaml`).
  - [x] Headers stay visible while scrolling, at 960×640 and 200% (`persistent-headers.yaml`, stable 6/6).
  - [x] Detail panels collapse and stay remembered (`client-center.yaml`).
  - [x] Condensed striped rows on wide screens (draw tests + daily_sales screenshots).
  - [x] Unique tag numbers shown and sorted everywhere (unit + tags scenario; live contract PENDING U-75).
  - [x] Copy down in one undo step (model + UI tests, batch_entry).
  - [x] Columns shown, hidden and reordered, remembered; required columns locked.
  - [x] Multi-select with Set actions in one undo step.
  - [x] Every lane action opens its form or a tracked placeholder (registry test + `home.yaml`).
  - [x] Needs-attention counts match engine queries (`contract_home.rs` live on the pin with seeded payroll, feed and a voided bill; dashboard part PENDING U-76).
  - [x] Ko-fi opens only when clicked (unit tests; the scenario hovers only).
  - [x] `ui-check home --compare` reviewed by ui-verifier, deviations in Q-P16d to Q-P16u.

  Agents: scout/opus, architect/opus, impl/opus (WP0–WP8 and two fix passes; the phase is tier L, but opus was used per Q-277), engine-impl/opus, review/opus ×2, ui-verifier/opus, docs-writer/sonnet.
- 2026-10-21 — P15A complete on `phase/P15A-bulk-entry`; PR to open. Shipped: the Bulk Entry window (sections per type and account; checks, expenses, card charges, card credits, deposits and journal entries; other types wait for their phases, Q-263); local exact checks (cent rounding, journal entry balance, duplicate check numbers, closed-period marks); engine validate per row with highlights and F8 navigation; paste from a spreadsheet and CSV import with a remembered column mapping; account suggestions with accept; Save all in one atomic call with an idempotency key and one closing-password prompt; a summary with register links; data-bus refresh. Grid block paste and a control-channel paste step. Engine PR #163 (U-67 to U-74, migration 0226) merged after security review and fixes; pin c217bfd. Contract suite 149/149 live; scenario batch_entry 178 steps plus forms-zoom coverage; help page. Decisions Q-P15Aa to Q-P15As.
  Acceptance (phases/M1-core-ledger.md P15A): [x] a mixed batch of checks and deposits saves and every row shows in the register with the right balance (live contract + scenario); [x] invalid rows cannot save (atomic rollback contract, unit + scenario); [x] control-channel scenario.
  Agents: scout/opus, architect/opus, impl/opus (Fable out of credits, Opus fallback for money lanes), engine-impl/opus, security-review/opus, review/opus, ui/opus, bookkeeper/sonnet.
- 2026-10-21 — P15 complete on `phase/P15-journal-templates`; PR to open. Shipped (2026-10-20/21): Make Journal Entry (names on lines, A/R-A/P rule, entry numbering, adjusting, reversing, autobalance, closing-password flow, edit/void/delete); Templates (store without posting, groups, remind/auto/never, Save as Template on Write Check, Deposit, Transfer, Card Charge, Journal Entry and Daily Sales, Use, Enter now); the catch-up runner and the Templates Due window. Engine PRs #160 (U-58 to U-61, U-64 to U-66, migration 0224, exact balance for all postings, rounding fixes in invoices, bills, vendor credits and estimates) and #161 (U-62, U-63, migration 0225, per-kind permissions, 31-per-run cap, month-end anchoring) merged after security reviews and fixes; pin 886c6b5; U-19 and U-22 delivered by it. Contract suite 136/136 live; practice company loads with templates and the first catch-up posts once; scenarios journal_entry 169, templates 339, templates_due 235 steps plus forms-zoom coverage; help pages x3. Decisions Q-P15a to Q-P15aa.
  Acceptance (phases/M1-core-ledger.md P15): [x] unbalanced entries cannot save (unit + contract + scenario, nothing sent); [x] missed monthly occurrences post exactly once each (contract run_due across engine restart + practice_live).
  Agents: scout/opus, architect/opus (Fable out of credits, Opus fallback for Tier H), impl/opus, engine-impl/opus, security-review/opus x2, review/opus, ui/opus, bookkeeper/sonnet.
- 2026-10-20 — P14 complete on `phase/P14-money-forms`; PR to open. Shipped: Make Deposit (grouping clears Undeposited Funds exactly, fee lines, cash back, vendor refunds, bank-deposit suggestions, on-screen summary), Transfer Funds, Daily Sales Summary (engine templates, over/short exact against the engine, manual tax line), Card Charge/Credit (header tags, payee panel, prefill), Pay Yourself/Put Money In and Paid with Personal Funds by entity type (equity roles), Loan Payment helper (lender fixture exact), cross-window refresh bus (Q-262), shared form header with navigation prompt, tab strip overflow, Undeposited Funds naming, jargon tooltips. Engine PR #159 (U-52 to U-57, migration 0223) merged after security review and fixes; pin f78583e. Contract suite 116/116 live; scenarios deposit 190, transfer 152, daily_sales 126, card_charge 141, owner_money 126, personal_funds 101, loan_payment 108 steps, plus forms-zoom; help pages x7. The preview build workflow (Q-253) and the Intuit compliance fixes (Q-254 to Q-258) merged between P13 and P14. Decisions Q-P14a to Q-P14ad.
  Acceptance (phases/M1-core-ledger.md P14): [x] two grouped payments equal the bank deposit and clear Undeposited Funds (contract `deposits::grouping_clears_payments_clearing` + scenario); [x] transfers match in both registers (contract + scenario); [x] loan payment splits exactly and the balance matches the lender fixture (contract + model proptest).
  Agents: scout/opus, architect/opus, impl/opus (Fable out of credits, Opus fallback for money lanes), engine-impl/opus, security-review/opus, review/opus, ui/opus, bookkeeper/sonnet.
