# STATE.md — ClientLedger

**Current phase:** P25 — Bank file import (QIF, QFX, OFX, CSV) (next). PR-4 done 2026-11-06; M2 complete. A05 runs right after P26; ADD-03 (A07–A10) right after P28/PR-6; ADD-04 inside P35/P36; ADD-05 (L1–L5) with P35/P36/P39 (license swapped early, #76).
**Status:** PR-4 PR open with auto-merge.
**Engine commit:** `29b31900` (`main`, pinned 2026-11-06 after engine PR #230 (PR-4 fixes, U-162, migration 0261)); before that `31d7ddd5` (`main`, pinned 2026-11-06 after engine PRs #224–#229 (P24 inventory: U-39b, U-156–U-161, U-25 inventory half, U-78; migrations 0258–0260)); before that `1c556ac6` (`main`, pinned 2026-11-02 after engine PR #222 (U-155, cash-basis report performance; migration 0257); before that `9be6da51` (pinned 2026-11-02 after engine PRs #216–#221 (P23 sales tax stack: U-147–U-154, U-25 sales-tax half; migrations 0252–0256); before that `9fe75899` (pinned 2026-11-01 after engine PRs #214 (U-144, U-145: credit memo and sales receipt email; finance-charge invoice keeps its client) and #215 (U-146: invoice client guard, 409 RETRY on receivables deadlocks); before that `bf2daf38` (pinned 2026-11-01 after engine PRs #209–#213 (P22 credit memo, sales receipt, refund, customer statement and finance-charge stack: U-137–U-143, U-112, U-121 A/R half); before that `eaa12bad` (pinned 2026-10-31 after engine PRs #205, #206, #208 and #207 (P21 Receive Payment stack: U-132–U-136); before that `c7b7d61d` (pinned 2026-10-31 after engine PRs #197, #199, #200, #201, #202 (P20 invoice stack: U-122–U-130, U-38, U-39a, U-40 and review fixes; migrations 0242–0246) and #203 (U-131, stored SMTP password; no smtpPass in company responses), and #198 (follow-redirects audit fix); before that `129deff7` (pinned 2026-10-29 after engine PR #196 (U-115–U-117, PR-2 purchasing fixes, migration 0241); before that `12187716` (pinned 2026-10-29 after engine PRs #194 (U-106–U-110, check printing, migration 0239) and #195 (U-111, U-113, document numbering and forms print queue, migration 0240); before that `e45658fb` (pinned 2026-10-28 after engine PR #193 (U-101–U-104, Pay Bills discounts, idempotent pay, check numbers, company scope; migrations 0236–0238); before that `5a9a8700` (pinned 2026-10-27 after engine PR #183 (U-94, document-drafts-statement-v1, migration 0235); before that `97d4d4cf` after engine PR #182 (U-97, restore verify no longer splits multi-byte characters, no migration); before that `4e0593c3` after engine PR #180 (U-95, document-drafts-attach-v1, no migration); before that `38cf97eb` after engine PRs #178 (U-92, document-drafts-v1, idempotency-docs-v1, migration 0233) and #179 (U-92d, document-assist-v1, migration 0234); before that `a88337e3` after engine PR #177 (U-93, intake-settings-v1, migration 0232); before that `b483c02e` after engine PR #176 (U-91, restore verify compares id-keyed jsonb semantically) and engine PRs #171 (idempotency-v1, migration 0229), #172 (run-due-filter-v1), #173 (ap-closing-v1, ap-edit-v1, bill-lines-v1, bill-terms-v1, bill-received-v1, idempotency-ap-v1, migration 0230), #174 (bill-ocr-v2) and #175 (attachments-company-v1, migration 0231); bump per Q-02)
**Last updated:** 2026-11-06
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
| P16 | Home workspace | L | Done 2026-10-24 |
| PR-1 | Checkpoint: core ledger process review | — | Done 2026-10-25 (fixes on `fix/PR-1`; U-18 and U-82 engine PRs open) |
| P17 | Enter Bill, vendor credits, receipt inbox, attachment viewer | M | Done 2026-10-25 |
| P18 | Pay Bills (checks) | M | Done 2026-10-28 |
| P19 | Form printing | L | Done 2026-10-29 |
| PR-2 | Checkpoint: purchasing lifecycle process review | — | Done 2026-10-29 (fixes on `fix/PR-2`; engine PR #196 merged) |
| PR-3 | Checkpoint: sales lifecycle process review | — | Done 2026-11-01 (fixes on `fix/PR-3`; engine PRs #214, #215 merged) |
| PR-4 | Checkpoint: sales tax and inventory process review | — | Done 2026-11-06 (fixes on `fix/PR-4`; engine PR #230 merged) |
| P20 | Create Invoice | M | Done 2026-10-31 |
| P21 | Receive Payment | H | Done 2026-11-01 |
| P22 | Credit memos, refunds, statements, finance charges | M | Done 2026-11-01 |
| P23 | Sales tax | H | Done 2026-11-02 |
| P24 | Inventory (average cost) | H | Done 2026-11-06 |
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

## Active addenda

**ADD-01 — Document intake, local reading, pop-out viewer** (`addenda/ADD-01-document-intake.md`; part of the beta). Scheduling: A01–A04 run right after P17 merges, before P18; then the beta track resumes; A05 runs right after P26; PR-A1 after A05.

| Phase | Title | Tier | Status |
| --- | --- | --- | --- |
| A01 | Mapped intake folders, a setting of each company | M | Done 2026-10-26 (manual mapped-drive / UNC share check pending the owner) |
| A02 | Pop-out document viewer | M | Done 2026-10-26 |
| A03 | Local document reading engine | H | Done 2026-10-26 |
| A04 | Review queue and compare tools | M | Done 2026-10-27 |
| A05 | Match document drafts in the Bank Feed | M | After P26 |

**ADD-02 — Read a vendor statement into Match Vendor Statement** (`addenda/ADD-02-statement-reading.md`; part of the beta; owner request 2026-10-26). Scheduling: A06 runs right after A04 merges (done); P18 started when the owner said go (2026-10-28).

| Phase | Title | Tier | Status |
| --- | --- | --- | --- |
| A06 | Read a vendor statement | M | Done 2026-10-27 |

**ADD-03 — Report Builder and Memorized Reports** (`addenda/ADD-03-report-builder.md`; part of the beta; owner request and Q&A 2026-10-30, Q-305–Q-315; engine U-118–U-121). Scheduling: A07–A10 run right after P28 and its PR-6 checkpoint, before P29; the `ReportBuilder.html` mockup is drafted for the owner's approval before A08; PR-A2 after A10.

| Phase | Title | Tier | Status |
| --- | --- | --- | --- |
| A07 | Report run engine, definitions, run in the report window | H | After P28 / PR-6 |
| A08 | Report Builder editor | M | After A07 (mockup approved first) |
| A09 | Memorized Reports, transfer, export and print | M | After A08 |
| A10 | AI drafting of report definitions | M | After A08 |
**ADD-04 — Distribution through winget** (`addenda/ADD-04-winget.md`; part of the beta; owner request and Q&A 2026-10-31, Q-317–Q-323). No A-phases: built inside P35/P36, switched on at P39 stage 2 (first manual submission, then `WINGET_ENABLED`). MSI per-machine only; UpgradeCode {5871208F-8AA3-4EC1-86A7-ABCF603A0765}; package `KisaesLLC.ClientLedger`.
**ADD-05 — VibeMyFirm Community License: tiers, keys and Pro gating** (`addenda/ADD-05-community-license.md`; owner request and Q&A 2026-11-06, Q-336–Q-342). **Draft: license texts in `licensing/`; waiting on the engine addendum** (`GET /license`, host signature). Phases L1–L4 inside P35/P36, L5 with P39 (Q-339). Business needs no key (Q-337); replaces PolyForm (Q-336).

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
| P47 | Custom fields, form template designer, project costing | M | |
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
- P11 projects in the Client center (Q-90, Q-302) — blocked by U-35 — `contacts::projects_u35` is PENDING — in place: clients only; the Projects tab says projects aren't available yet (Q-P11j).
- P11 several ship-to addresses and contact people with roles (Q-128, Q-129) — blocked by U-36 — `contacts::addresses_u36` is PENDING — in place: one bill-to and one ship-to, single person/email fields, with a note in the pane (Q-P11k).
- P11 dated notes and to-dos (Q-130) — blocked by U-37 — `contacts::notes_u37` is PENDING — in place: the single notes field in the Notes tab, with a note that dated notes and to-dos arrive later (Q-P11l).
- P11 per-contact open balance — blocked by U-38 — `contacts::open_balance_u38` is PENDING — in place: figures from the client balance summary and vendor aging reports, labelled with their source, never summed in the UI (Q-P11m).
- P11 recording a paper W-9 — blocked by U-44 — `contacts::w9_on_file_u44` is PENDING — in place: W-9 on file shown read-only, the form says recording it isn't available yet (Q-P11n).
- P11 item kinds other than service, the purchase side of two-sided items and the unit label (Q-58, Q-96, Q-99) — blocked by U-39 — `items::item_types_u39` is PENDING — in place: service items only, the panel says the rest isn't available yet (Q-P11q).
- P11 price levels (Q-58) — blocked by U-40 — `items::price_levels_u40` is PENDING — in place: a note under the Item List (Q-P11r).
- P11 one tag per group in the engine, group reorder — blocked by U-41 — `tags::single_select_u41`, `tags::group_reorder_u41` are PENDING — in place: the forms enforce one per group, groups keep the engine's order (Q-P11v). Merging tags used on lines or as defaults works since pin 7b267e48 (U-47, Q-P11t).
- P11 required tag groups by account type (Q-180) — blocked by U-42 — `tags::required_groups_u42` is PENDING — in place: kept on the company record and enforced by the forms' rule check (Q-P11u).
- P11/P13 several tags on one line (Q-179, Q-P10t) — blocked by U-45 — `tags::several_line_tags_u45` is PENDING — in place: one tag per line, refused in plain words before saving (Q-P11w).
- P11 your own terms (including discount terms) and payment methods — blocked by U-43 — `list_csv::terms_lists_u43` is PENDING — in place: the engine's fixed lists, read-only, with a note (Q-P11y).
- P11 item CSV import in one call and full contact rows — blocked by U-46 — `list_csv::items_import_u46` is PENDING — in place: items created one at a time; contacts import name, company, email and phone only (Q-P11x).
- P12 toggling cleared marks in the register (R5) — blocked by `register-cleared-v1` — in place: ✓/R read-only (Q-P12k). Also still PENDING for P12: U-17 (server-side version check; fingerprint stopgap); U-18 (idempotency keys) is live since pin f3ce33fd.

## Placeholders tracked

Screens or actions that link to a later phase. Remove each line when its phase lands.

- P34/P08 — restore UI must call `Cluster::restore(src, store, allow_unregistered)` with `allow_unregistered = false` by default and set it to `true` only after an explicit owner confirmation that the file came from a trusted source (Q-P03h).

- P06 shell placeholders (Q-P06g): every menu, rail and palette command whose screen is built later opens a placeholder tab. The phase that builds the screen removes its entry: P11 Item List / Tags / Clients & Projects / Vendors, P12 Open Register, P13 Write Check, P14 Make Deposit / Transfer Funds / Sales Receipt / Record Card Charge, P18 Pay Bills, P20 Create Invoice, P21 Receive Payment, P22 Credit Memo or Refund / Statements, P25 Import Bank File, P26 Bank Feed, P27 Reconcile, P28 Reports / Profit & Loss / Balance Sheet / Trial Balance, P29 Set Closing Date / Users and Permissions / Audit Trail, P30 Import Payroll, P32 Send to My Accountant, P38 bundled help (F1 says "not available yet").
- P06 native macOS menu bar (Q-70, muda) is deferred (Q-P06b): in-window menu on every platform until macOS packaging.
- P16 Home lane actions open "arrives in Pxx" placeholders (Q-P16k): P18 Pay Bills, P20 Create Invoice / Sales Receipt, P21 Receive Payment, P26 Review Bank Feed, P27 Reconcile, P28 Profit & Loss, P29 Set Closing Date, P32 Send to My Accountant. Each phase wires its lane action.
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
2. ~~Native file picker for attachments (Q-P13k).~~ Done in A01 (Browse on Enter Bill and Write Check, `rfd`).
3. The `cl-host` cold-start budget test exceeds the 15 s local limit under load (CI uses `CL_HOST_BUDGET_SECS=30`).
4. Engine issues #157 (duplicate check numbers under concurrency) and #158 (float compare in the rules engine).

## Follow-ups from P14

1. The control channel can't click dropdown options scrolled out of view (automation limitation); scenarios choose options that are visible (Q-P14z).
2. The demo stores no tags on daily-sales days.
3. Engine issues #154, #155, #157 and #158 are still open.
4. Engine main CI (npm-audit) is failing; pre-existing, not caused by PR #159.
5. The vendor A/P aging summary lists bills only; vendor refunds are not reflected there (the vendor statement shows them).

## Follow-ups from PR-4

1. QA-13 Quick Add: no form has it yet.
2. QA-16: an On hand column in the Item List.
3. Q-PR4c: setup interview agency question (QA-6); needs an engine company default tax item.
4. Q-P24y: re-costing performance on long histories, before P33.
5. Q-PR4b: cost visibility permission, in P29.
6. Migration 0261: corrections stored in the old order stay until the next walk of that item.

## Follow-ups from P24

1. Re-costing time on long histories is about 14.5 s against the 3 s budget (Q-P24y, accepted default); address before P33.
2. The property test is not extended to closing-date moves and units.
3. A live bill void drops NEGATIVE_STOCK warnings (the demo shows them).
4. The engine pg-boss round-trip test was flaky once on PR #225.
5. One unexplained local test failure on the first full CI run, which passed on re-run (watch).
6. The Item panel is cut off at 960×640 (pre-P24).
7. Adjust Inventory: default account before the demo seed.

## Follow-ups from P23

1. ~~`ANALYZE` after company create, import and restore~~ Done 2026-11-02 on `fix/db-statistics` (Q-335): runs as the app role after create, restore, new-from-existing and the practice company; a company import flow will need the same call when it exists.
2. Contract tests not written: sales receipt tax (golden 5), daily sales tax, cash golden 8b and 8d (covered by engine tests and the demo only).
3. Q-P23z: the cash-basis SPF corrections change cash reports of closed periods (no posted rows change); owner confirmation recommended. P28's basis note records the version.
4. A dev database that applied the pre-review 0254 or 0256 migrations must be recreated.
5. No sales tax mockup exists, so no `--compare` run; the tax picker sits above the totals on Create Invoice.

## Follow-ups from PR-3

1. N1: cash basis books a credit applied later (or a prepayment) on the applied or payment date even when that is before the invoice date; open items use the latest of applied, source and invoice dates. Decide which cash-basis date rule to use (owner).
2. N3: open items and client balances scope `company_id = X OR NULL`, reports `= X`; hidden with one company per tenant. Unify before multi-company use.
3. N4: a statement run holds the company counter row for up to 500 statements, blocking other numbered documents meanwhile.
4. N5: P32 transfer file must build on the tenant-portability archive format (the legacy export lacks payment_applications, finance-charge and statement tables).
5. N6: as-of aging uses today's applications (Q-P22i); an unapply changes earlier as-of aging after its applied date.
6. Missing test 3 (monthly cash P&L across the lifecycle) not written.
7. updateAccountBalances now applies one net change per account in id order (U-146 S1): watch for any posting-order regressions.
## Follow-ups from P22

1. ~~Emailing credit memos and sales receipts~~ Done in PR-3 (Q-P22u, U-144).
2. ~~Finance-charge invoice client change~~ Done in PR-3: refused (Q-P22v, U-145).
3. ~~Loose cross-company assertions~~ Done in PR-3 (exact 404).
4. A statement email is sent before the database commit (as U-127): a failed commit and retry could send twice (known limit).
5. Sales by Item has no cash-basis test.
6. The demo tax rate is 8.1% against the golden file's 8.25% per-line rate.
7. Statements has no Alt+N / Alt+S (batch screen).
8. Scenarios don't yet check the Client Center balance after a memo or the Print Checks listing after a refund; no unnamed_widgets checks in the YAML scenarios.
9. Receive Payment --compare remaining deviations: extra Void button, "Unsaved changes" note, no teal tint on the header card, "Write off the rest", Next-step count from demo data, grid without a bordered card.
10. A dev database that ran the old 0250_finance_charges migration must be reset.

## Follow-ups from P21

1. Contract tests not written: `receive_matches_feed_item_atomically` (needs a seeded bank-file item) and `balance_token_round_trips` (partly covered).
2. The open-invoice grid on Receive Payment is hand-rolled, not `cl-grid`.
3. ~~The bounced-check and other dialogs are inline on Receive Payment, not modal; a bounce is reachable only from Receive Payment (open the payment from the register or client history first).~~ Done in P22.
4. ~~No dedicated Find dialog for payments.~~ Done in P22.
5. Demo: the sample ledger's PMT rows aren't receivables, so opening one from the register shows "not found" in the demo.
6. The engine reports the Customer Deposits liability balance debit-positive (-500.00); check the display on Chart of Accounts and the balance sheet in P28.
7. `returned_payment` was added to the attachable types; engine attachment support for it is unverified.
8. Engine: the GET /payments route description isn't updated for the new list fields.
9. The count of legacy sub-cent invoices (Q-P21g) was not run against MyBooks Online data; engine PR #205 has the query.
10. ~~Mockup deviations accepted (Q-P21w): labels beside header fields; toolbar lacks Find/Attach/History; Prev/Next without arrows; summary below the callout; "Write off the rest"; Next-step panel count from demo data.~~ Done in P22.
11. ~~A/R aging nit (from the design review) deferred to P22.~~ Done in P22.

## Follow-ups from P20

1. A billable line's "From <vendor> bill N" caption shows only for costs added in the open window: the engine's saved `billableCosts[]` has no vendor name or source number (engine follow-up).
2. Save & New / Save & Close / Clear tab order is the reverse of the screen order (shared button row; fix across forms).
3. Outbox and Price Levels lists: plain headers, amounts not right-aligned.
4. Print Forms (P19) doesn't use the invoice's print layout; layout accent colour and logo don't print yet.
5. Engine: the PDF ignores showShipTo and takes branding from the tenant's first company; `/transactions` invoice routes don't take layoutId; MCP create_invoice passes no companyId; encrypted Stripe key columns are still in company responses; the legacy JSON export doesn't carry invoice_templates, price levels, billable links or layout ids.
6. Engine: recording a send bumps updatedAt (the app reloads after send); a PUT on a pre-U-122 invoice recomputes under the cent rule; the email send holds a DB connection during SMTP (fine for desktop, note for servers).
7. U-35 projects and U-36 contact people/ship-to/roles deferred (Q-P20r): those invoice parts are blocked, tests PENDING.
8. Contract tests: failed closing-password attempts are limited per user (10/15 min) across the whole suite; new tests must not add wrong-password probes.

## Follow-ups from the 2026-10-30 session (PRs #52–#64)

1. Screen checks that failed on `main`: `card_charge` step 44 ("Account, line 1" too narrow for "5200 · Project Supplies and Materials") and `bill` (document reading needs local OCR on this PC). Fixed on `fix/ui-check-card-bill`: the shared lines grid gives Account more room than Memo, and local OCR is built here (`cargo xtask ocr-build`, `ocr-models`).
2. Engine `main` (and the pin `129deff7`) is still **PolyForm Small Business**; the owner's Internal Use relicense is on the unmerged engine branch `clientledger/license-internal-use` (commit `0b558243`). ClientLedger's NOTICE.md describes the engine as licensed to ClientLedger without naming its license (Q-303). Merge the engine branch when the owner decides.
3. macOS 14 runners retire 2026-11-02; CI, nightly and preview moved to macOS 15 and `main`'s required check is `ci (macos-15)` (#61).
4. Auto-merge sometimes leaves a PR open after its required checks pass (#61, #64); re-applying `gh pr merge --auto --merge` merges it.

## Follow-ups from P19

1. A deposit opened later prints blank Check no. and Payment method columns on its summary (the saved deposit record doesn't carry them).
2. If a save stops at the closing-date password, Print after save is dropped; the owner clicks Print again.
3. Two rare lock-order deadlocks in the engine (printChecks vs updateCheck; document-number claim vs check edits): PostgreSQL fails one request.
4. U-114: no engine route to delete a company logo; Remove hides it locally.
5. Open Company keeps plain circles until an icon set exists (Q-P19aq).
6. Old PR #50 commits with the owner's path may be reachable by SHA on GitHub until purged.
7. Engine `payBills` with method check, Print later off and no check number saves isCheck=false, while migration 0241 marks such voided rows as checks. API-only path (the app always sends Print later or a number); align later.

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

## Follow-ups from P18

1. U-105: per-bill `expectedUpdatedAt` on pay so a stale Pay Bills screen gets 409 STALE instead of 400 (Q-P18g).
2. A vendor paid only by credits with "To be printed": the engine queues a $0.00 check, the demo engine doesn't. Decide in P19 (skip $0 checks when printing) or upstream.
3. A04 drafts saved before P18 with `billReceived:false` never match the form (always true now) and keep a "differs" mark; pre-release data only.
4. Amt. to pay is shown as text in the grid, not as input boxes (mockup); typing goes through the row editor.
5. Live contract tests in a worktree need `CL_ENGINE_EXE` set to the worktree's `target\engine\windows-x64\clientledger-engine.exe`: the harness doesn't find the engine since the per-worktree `build-dir` change. Teach it to ask `cargo metadata`.
6. The full local `cargo xtask ci` is killed by low memory on this 16 GB machine (twice on 2026-10-28). Page file or a lighter CI profile needed.

## Follow-ups from A06

1. Photos of statements read 79.6 % (seen) / 73.5 % (held-out) of lines right; most misses are OCR letter errors in reference numbers. Revisit at PR-A1 with `private-fixtures/`.
2. A read still overwrites typed box text without asking; typing in the box waives the read marks.
3. No test yet that a 20-page, 2,000-line statement fits the worker's 8 MiB reply cap; the read table isn't virtualized (redraw cost fixed, S7).
4. Statement layouts learn header fields only (the template model has no statement column types).
5. Live contract test `drafts::statement_drafts` ran only its PENDING path locally (engine build before U-94); CI runs it on pin 5a9a8700.
6. Region re-read on a statement still stops at page 10.

7. CI `docread-netcheck`: strace 5.16 on ubuntu-22.04 sometimes dies with "ptrace(PTRACE_LISTEN...): Input/output error" while a test kills a worker (PR #45, #46); the step retries that error only. Move the job to a newer strace (ubuntu-24.04) when convenient.

## Follow-ups from A04

1. Pointer drags (draw a box to re-read a region) can't be scripted: the automation tools click and move but can't press, drag and release. Unit tests only.
2. "Enter this as" is checked on screen only; the "Ask AI to help" row in drafts isn't exercised in the demo; Alt+E (Enter as new) unchecked in a scenario; AccessKit inspect of the compare overlay skipped.
3. `doc_review_forms` fails in dark at step 19 (draft not loaded in time, likely state left by the light run); `doc_compare`, `doc_review` and `receipt_inbox` not re-run after the fix pass.
4. A Write Check opened from an unread draft shows today's date but has none until the owner types one; Save refuses. Fill the date or show the field empty.
5. Engine follow-ups U-98 (reopen an attached draft), U-99 (no-vendor duplicate guard), U-100 (Read again).

## Follow-ups from A03

1. One seen document type (PDFs without a text layer, 7 docs) clears its 85 % gate by one document; the hand-style font confuses OCR digits. Re-measure on `private-fixtures/` at the milestone review.
2. Every Preferences save with an online AI provider re-accepts the AI notice (P09 behaviour); harmless now that the document switches are read back from the engine, but worth a P29 look.
3. macOS has no hard memory cap for the worker (RLIMIT_DATA doesn't cover mmap); tesseract on Linux/macOS has the watchdog and CPU limit only (Q-A03c). A Rust abort on Windows can still reach WER (P37).
4. Too-large page pictures (> 4096 px) are not sent to AI; the text goes alone and the banner says so (no downscaling yet).
5. Held-out vendor accuracy dipped one document on scan/template-scan after the "printed name beats layout" fix; watch it.
6. "Network off" in the bill scenario is shown by zero AI requests, not an enforced block; enforced sandboxes in P37.
7. `omit_document_drafts` / `reset_document_drafts` are unused until P32 (transfer files).
8. Account form create-then-set card digits has no live-engine test (the save path itself is tested live).
9. Flaky on slow macOS Intel runners: `automation::bridge_tests::a_closed_window_is_forgotten` (A02) failed once on PR #44 (`before.is_ok()`); passed on re-run. Make its first inspect wait for the child window's first frame.

## Follow-ups from A02

1. Screen readers don't see the pop-out window: eframe 0.37 connects AccessKit only to the main window (Q-A02n). Revisit on an eframe upgrade.
2. eframe gives no monitor origin for a child window; placement assumes monitors side by side from the origin. A second-monitor user who docks and pops before dragging loses the saved place once per session (Q-A02c).
3. No scenario for the viewer following a register item (needs a saved transaction with a file in the demo); shell and unit tests only.
4. Untested edge cases: PDFium missing while popped out, a reloaded form getting a new document key (409), following a P06 popped-out tab.
5. A TIFF page can briefly hold its decoded buffer and the RGBA copy together (up to 512 MiB at the limit); the viewer window's keymap copy refreshes only on a mode change; a popped P06 tab shows "open in its own window" for a document the viewer isn't showing.
6. Write Check's Preview still opens the document tab (document-view.yaml depends on it).

## Follow-ups from A01

1. **Owner manual check pending:** add a real mapped drive and a real UNC share (`\\server\share`) as intake folders, copy a PDF into each (appears within 15 s), New bill → original in `Processed\<year-month>\`, Discard → `Discarded\`, disconnect → "unreachable" without freezing, reconnect → recovers. Automated tests cover local folders live and shares through a simulated file system only.
2. LAN mode (P33): each workstation watches its own confirmed folders (owner, Q-289). Duplicate prevention must become engine-wide (an attachment content hash and a claim on arrival) before P33; plan it with A03/A04 (U-92) or a separate U-row.
3. Transfer files: P32 must set `omitIntakeSettings` on export and `resetIntakeSettings` on import (Q-A01l).
4. Polling every 8 s (no license-clean watcher crate) lists network shares about 25% more often than the planned 10 s (Q-A01d); measure on slow shares.
5. The inbox list is rebuilt from files × entries each frame; fine at demo size, revisit at 20 folders × 500 files.
6. The engine's attachment delete audit row includes the file name in `before` (pre-existing); Discard now creates such rows. Engine follow-up.
7. On Linux the Browse dialog uses the desktop portal over the system `libdbus-1` and falls back to `zenity`; check on the AppImage/deb targets (P35).
8. HEIC: research a permissive decoder in A02 (Q-292).
9. Restore had refused every real engine backup since P08 (manifest past the first 64 archive entries); fixed in A01 (c9f77a5). Only mocked archives had been tested before; the live intake test now restores a real backup.
10. Disk: each parallel worktree keeps its own Cargo target (130–300 GB each); four filled the 932 GB drive on 2026-10-26 (CI failed with os error 112). Delete idle worktree target folders after each phase; sccache keeps rebuilds cheaper.

## Follow-ups from P17

1. Scanned (image-only) PDFs can't be read with AI: no permissive rasterizer, poppler is GPL. The app says so (Q-P17o). ADD-01 A03 (local OCR) is the fix.
2. "Add as a new vendor?" after a reading opens the vendor list, not a prefilled vendor form (Q-P17u).
3. Done in P18: a bill-payment row opens the read-only Bill Payment view with Void.
4. No automatic backup is taken before the engine runs its database migrations on an engine upgrade (non-negotiable 9). Build it with the upgrade flow (P36) or sooner; no release before then.
5. Engine download tokens can be replayed for 60 seconds (stateless signed tokens; U-90 review L1). Revisit before P33 LAN mode.
6. Bill templates: the engine has no `bill` template kind (Save as Template disabled, Q-P17a). Propose a U-row when bill templates are wanted.
7. PDFium renders in the desktop process on a worker thread (P17 plan R3); a sandboxed render helper through cl-host is a P37 follow-up. PDFium and its bundled libraries (FreeType under FTL) go into the P39 licensing review with Q-P03e.
8. Statement match reads at most 20 × 50 bills/credits and 10 × 100 payments and says so when capped; measure on large vendors.
9. Walkthrough harness: a light run that failed while zoomed left the dark run's window at 2× (fixed in P17: zoom reset before each theme). Watch for other state carried between themes.
10. Dev machine: Smart App Control is off (2026-10-25); Bitdefender is the active antivirus, so build exclusions belong in Bitdefender. The user-level Cargo config uses sccache, rust-lld and line-tables-only debug info (not in the repo).
11. Two engine test files also fail locally on Windows on engine main (portal bill-captures email timing, portal tax-estimate options); not caused by ClientLedger PRs.
12. Write Check, Card Charge and Personal Funds tab their bottom buttons in reverse screen order, and Write Check's Esc leaves nothing focused after putting a field back (fixed on Enter Bill in P17; same fix to apply there).
13. The shell's "Navigation" button overlaps the tab strip at 200% zoom in a 960×640 window (older shell code; zoom steps skip the tab strip as forms-zoom does).

## Follow-ups from checkpoint PR-1

1. The unknown-save block lasts for the app session; an app restart forgets it (Q-PR1a). U-18 closes this.
2. Security note from the review: `GET /transactions/:id` and `POST /transactions/:id/void` don't scope by company (moot while one tenant is one company); security-review before P33 LAN mode.
3. With a menu open, Escape closed the Register tab instead of only the menu (seen while writing the shell scenario for PR #35). Check and fix.
4. Not covered by the PR-1 review (cover in a later checkpoint): Daily Sales end to end, LAN concurrency (U-17), Undeposited Funds and A/P subledger tie-out, template CRUD audit rows, Bulk Entry engine validate/plan, printed-form rounding (P19).
5. Contact merge in closed periods (review S1) is fixed by U-47 (engine PR #170, pinned).

## Milestone summaries

### M2 — Money in and out (P17–P24, checkpoints PR-2, PR-3, PR-4; ADD-01 A01–A04, ADD-02 A06), completed 2026-11-06

- **Shipped:** Enter Bill, vendor credits and receipt inbox (P17); document intake, pop-out viewer, local reading and review queue (A01–A04); vendor statement reading (A06); Pay Bills (P18); form printing (P19); Create Invoice (P20); Receive Payment (P21); credit memos, refunds, statements and finance charges (P22); sales tax (P23); inventory with average cost, units of measure and reports (P24); checkpoint fixes PR-2, PR-3, PR-4 (PR-4 with hands-on QA against the real engine).
- **Screenshots:** `target/ui-check/<screen>/` (light and dark) for every screen, plus the CI `ui-check` artifacts on each phase PR.
- **Questions added:** Q-P17a–v, Q-P18a–i, Q-P19a–ar, Q-P20a–aj, Q-P21a–w, Q-P22a–w, Q-P23a–aa, Q-P24a–y, Q-PR2a–e, Q-PR3a–c, Q-PR4a–c, Q-SPF1–7, Q-A01a–r, Q-A02a–n, Q-A03a–u, Q-A04a–o, Q-A06a–i; owner Q&As at each phase end; also Q-335 and Q-336–Q-344 (ADD-05, license swap).
- **Engine:** pin 29b31900 (engine PRs #183 through #230 merged; U-rows through U-162, migrations through 0261). Not done: U-105 and U-114 Proposed; U-98 Planned; U-118–U-120 Planned (ADD-03, A07–A10); several UPSTREAM.md M2 rows still read "PR open" and need a status refresh.
- **Reviews:** opus combined review per phase; process reviews PR-2, PR-3, PR-4 at xhigh; hands-on QA at PR-4 (3 Must-fix, 9 Should-fix, money math matched by hand).

### M1 — Core ledger (P10–P16, checkpoint PR-1), completed 2026-10-25

- **Shipped:** ledger grid (P10); lists, centers and tags (P11); bank and account register (P12); Write Check (P13); deposits, transfers, card charges, daily sales, owner and loan helpers (P14); journal entries, templates and catch-up (P15); Bulk Entry (P15A); Home workspace with the Enter option, fixed headers, condensed rows, tag numbers, Bulk Entry copy down / columns / bulk set, and the Ko-fi button (P16); checkpoint PR-1 fixes (unknown-save block on every posting form, register edits routed to their forms, catch-up confirmation above 3 dates, template link retry, closed-period move-out contract test).
- **Screenshots:** `target/ui-check/<screen>/` (light and dark) for every screen, plus the CI `ui-check` artifacts on each phase PR; Home `--compare` in `target/ui-check/home/compare.html`.
- **Questions added:** Q-P10a–x, Q-P11a–aa, Q-P12a–w, Q-P13a–x, Q-P14a–ad, Q-P15a–aa, Q-P15Aa–As, Q-P16a–y, Q-PR1a–g; owner answers 2026-10-24/25 recorded on Q-P16 rows, Q-PR1b/d and Q-206 (any number of named A/R or A/P lines).
- **Engine:** pin 7b267e48 (PRs #153, #156, #159–#161, #163, #168–#170 merged). Pending owner merge: #171 (U-18 request idempotency, security-reviewed) and #172 (U-82 run-due filter). Proposed: U-77, U-78, U-79.
- **Reviews:** opus fresh-eyes reviews and fix passes on every phase; process review PR-1 (opus, xhigh) with two fix passes and a fresh-eyes review of the fixes.

## Owner-requested checkpoints

- **After P13 merges, before P14 starts (owner, 2026-10-20):** Intuit compliance check — trademarks/names, copied text, icons and assets, layouts and colors vs QuickBooks, feature names, help text, fixtures, non-affiliation notice (non-negotiable 5). Report to the owner; fix findings on a `fix/` branch.

## Log

Newest first. One entry per completed phase: date, phase, summary, acceptance checklist results, notable decisions (Q-xx / U-xx).

Older entries (P00–P07, engine and planning notes) are in [docs/history/STATE-log.md](docs/history/STATE-log.md). Keep at most the last four entries here; the bookkeeper moves older ones there when a phase closes.

- 2026-11-06 — Checkpoint PR-4 Sales tax and inventory complete on `fix/PR-4`; PR #77 with auto-merge. Process review (opus, xhigh; target/reviews/PR-4.md): 1 Blocking, 5 Should-fix, 9 Notes.
  - Hands-on QA against the real engine (target/reviews/PR-4-qa.md): 3 Must-fix, 9 Should-fix, 5 minor; money math matched by hand.
  - Engine PR #230 (merged; pin 29b31900; U-162, migration 0261): B1 one total order for stock rows in every reader (linked returns follow their sale's stored position; re-walk after tenant import); S1 negative-stock warnings on void, delete, edit and repeated sales below zero; S2 `INVENTORY_OFF` on every path; S3 cash-basis COGS and inventory relief on settlement, in proportion (Q-PR4a); S5 linked return of a sale sold below zero at corrected cost; QA-3 COGS account on inventory turn-on; QA-9 `expenseKind`; notes N3–N6; audit rows for correction changes. Fresh-eyes re-review found 1 more Blocking (import ordering) and 3 Should-fix; all fixed and verified; a tied-position residual also fixed.
  - Desktop: Must-fix QA-1, QA-2, QA-4 (pickers never show an unset account; Adjust and Count never default to an unrelated account; no stray Expenses lines on card charges and checks, also QA-5; problems name the tab and line); QA-6, QA-7 agency setup links; QA-8, QA-10–12, QA-14–17; S4 negative-stock warnings on bill, vendor credit, check, card charge and register voids and deletes; demo mirrors every engine rule (golden byte-identical, new cases V2–V4, R2–R5).
  - Not done: QA-13 Quick Add; QA-16 On hand column; QA-6 interview agency question (Q-PR4c); Q-P24y performance; Q-PR4b cost permission (P29). See Follow-ups from PR-4.
  - Checks: full local `cargo xtask ci` green; 6 live inventory contracts pass against the new engine; ui-check light and dark inventory_items, _adjust, _count, _forms, _negative, _reports, _closed, pay_sales_tax, sales_tax_owed, write_check, card_charge, bill.
  - Decisions: Q-PR4a (cash COGS on settlement), Q-PR4b (cost permission in P29), Q-PR4c (agency in interview, default). Row U-162. The license swap (#76, Q-336) merged the same day.
  - Agents: process-reviewer (opus, xhigh), ui-verifier (opus) hands-on QA, engine-implementer (opus) ×2 (+ xhigh second round), rust-implementer (opus) ×2, reviewer (opus) re-review and re-check, bookkeeper (sonnet).
- 2026-11-06 — P24 Inventory (average cost) complete on `phase/P24-inventory`; PR with auto-merge. Shipped:
  - Engine PRs (KisaesDevLab/Vibe-MyBooks, all merged; pin 31d7ddd5): #224 U-39b/U-156/U-25 inventory part items, inventory asset account, inventory switch (migration 0258); #225 U-157 costing core: inventory_movements, average-cost walk from the first affected position under the item lock, COGS/relief lines kept in `inventoryLines` outside `lines`, cached balances, valuation = GL (migration 0259); #226 U-157/U-158 checks, card charges, cash sales, sales receipts, credit memos with linked returns at the sale's cost, negative-stock warnings; #227 U-159 adjustments, physical counts, opening quantity; #228 U-160/U-78 valuation summary/detail, stock status, reorder; #229 U-161 units of measure (unit sets; migration 0260). Review fix pass on all six. Also: 409 RETRY on deadlocks/lock timeouts is engine-wide; $0 documents that move stock post; duplicates keep item fields.
  - Desktop: Lists › Items inventory panel with opening quantity and unit sets; Lists › Unit Sets; Adjust Inventory; Physical Count (409 STALE reload, printable count sheet PDF); Items to Reorder (also from Home Needs attention); reports Inventory Value (with GL check and cash-basis note), Value Detail, Stock Status; parts on bills, vendor credits, Write Checks and card charges (new Items tab), invoices, sales receipts, credit memos ("Return of" picker); Unit column; NEGATIVE_STOCK warning; closed-period refusal message (INVENTORY_PERIOD_CLOSED, no password override); Track inventory switch moved from the local flag to the engine; demo costing parity; print filter skips engine cost lines; `cl_core::UnitCost`, `UnitFactor`.
  - Acceptance: [x] average cost matches the reference calculations including after negative stock (goldens A, B, V, X, M, D, N, E, R, F, U byte-identical engine/demo/cl-api; live replay of every non-engineOnly case); [x] inventory valuation equals the GL inventory account (engine I2 test after every step; live contract valuation = GL; Inventory Value GL check line).
  - Checks: full live contract suite 309 passed at the E5 head (content-identical to pin 31d7ddd5); ui-check inventory_items, inventory_adjust, inventory_count, inventory_reorder, inventory_reports, inventory_negative, inventory_closed, inventory_forms, inventory_units, write_check, card_charge, bill, create_invoice, credit_memo, sales_receipt, preferences-user (fixed a pre-P24 scenario break), home — light and dark; full local `cargo xtask ci` green at the phase head.
  - Owner Q&A mid-phase (2026-11-06): closed periods refused for inventory (Q-P24b), returns at original sale cost when linked (Q-P24e), full units of measure now (Q-P24i), all report screens in P24 (Q-P24n); plan Revision 4.
  - Not done / follow-ups: re-costing time on long histories ~14.5 s vs 3 s budget (Q-P24y, accepted default, before P33); property test not extended to closing-date moves and units; live bill void drops NEGATIVE_STOCK warnings (demo shows them); engine pg-boss round-trip test flaky once on PR #225; one unexplained local test failure on the first full CI run that passed on re-run (watch); Item panel cut off at 960×640 (pre-P24); Adjust default account before the demo seed.
  - Decisions: Q-P24a–y. Rows U-39b, U-78, U-156–U-161, U-25 (inventory half).
  - Agents: engine-scout (opus), architect (opus) ×4 revisions, process-reviewer (opus, xhigh) design review + re-check, engine-implementer (opus) ×4 (E1+E2a, E2b+E3, E4+E5, review fix pass), rust-implementer (opus) ×5 (WP-1, WP-2, WP-3, WP-4, review fix pass), rust-implementer (sonnet) UI fix pass, reviewer (opus) combined review (engine + desktop), ui-verifier (opus), ci-fixer (sonnet), bookkeeper (sonnet). The 2-implementer rule (Q-301) was exceeded: the owner's mid-phase answers added units of measure and all report screens; every agent hit the 150-turn limit at least once and was resumed. The plan ran ~6,100 words against the ≤ 2,000 rule.
- 2026-11-02 — P23 Sales tax complete on `phase/P23-sales-tax`; PR with auto-merge. Shipped:
  - Engine PRs (KisaesDevLab/Vibe-MyBooks, all merged; pin 1c556ac6): #216 U-147 setup (agencies, items, groups, codes; 6-place rates U-148; migration 0252), #217 tax on invoices, credit memos and sales receipts (one rounding per document, group members by largest remainder; recurring and MCP posting paths), #218 U-149 liability by agency/item, accrual and cash (running total), U-152 cash SPF rules (Unapplied receipts, no write-offs or deposits on cash, bill-discount fix), #219 U-150 Pay Sales Tax as a check, link outside payments, mark paid through with undo (0254), #220 U-151 exemption certificates, #221 U-153/U-154 SSUTA discounts, bad debts and recoveries, forward-only switch (0256), #222 U-155 cash-basis report performance (0257).
  - Desktop: Sales Tax setup (agencies, items, groups, codes), tax on Create Invoice / Credit Memo / Sales Receipt, Sales Tax Owed (liability by agency and item, accrual or cash per Preferences), Pay Sales Tax (check via the print queue, adjustments, link, mark paid through and undo), client exemptions with warnings, bad-debt recovery, daily sales tax, Preferences > Sales tax (basis, SSUTA switch); `cl_core::TaxRate`.
  - Acceptance: [x] liability totals reconcile on fixtures (`sales_tax_liability::liability_reconciles_to_gl`, goldens 7 and 9); [x] rounding consistent across documents and reports (goldens 1–4, 12–15; sales tax golden byte-identical to the engine's).
  - Checks: 262 live contract tests at 1c556ac6 (13 new incl. `contract_cash_basis_spf`); invoice and sales tax goldens byte-identical; ui-check sales_tax_setup, sales_tax_invoice, sales_tax_owed, pay_sales_tax, tax_exemptions, preferences_sales_tax, recover_bad_debt, daily_sales_tax, create_invoice; full local `cargo xtask ci` green at 395b57f.
  - Found and fixed: cash-basis reports took 30 s+ on a few dozen transactions (bad plans without statistics, then JIT on CI) — U-155.
  - Decisions: Q-P23a–aa, Q-SPF1–7, Q-PR3b. Rows U-147–U-155, U-25 (sales tax half).
  - Agents: engine-scout (opus), architect (opus), process-reviewer (opus, xhigh) design and cash-basis SPF reviews, engine-implementer (opus) ×4 (stack, review fix pass, cash performance ×2 rounds), rust-implementer (opus) ×3 (desktop, fix pass, WP-4), reviewer (opus) combined review, bookkeeper (sonnet). The 2-implementer rule (Q-301) was exceeded by the review fix pass and the U-155 fix.
- 2026-11-01 — Checkpoint PR-3 Sales lifecycle complete on `fix/PR-3`; PR with auto-merge. Process review (opus, xhigh; target/reviews/PR-3.md): 2 Blocking, 4 Should-fix, 6 Notes, 9 missing tests.
  - Engine PRs (merged; pin 9fe75899): #214 U-144 email credit memos and sales receipts (routes per document, Outbox kinds, send history; Q-P22u) + U-145 finance-charge invoice client change 409 FINANCE_CHARGE_CLIENT_CHANGE (Q-P22v); #215 U-146 PR-3 fixes: B1 invoice client change refused while paid/applied (409 INVOICE_HAS_PAYMENTS), B2 cash-basis applications only for posted invoices, S1 consistent lock order + 409 RETRY on receivables deadlocks, S3 bounce audit carries discount/write-off, S4 cash Sales by Customer/Item after a discounted bounce, N2 invoice edited down to the amount paid becomes paid.
  - Desktop: Email on Credit Memo and Sales Receipt (send now or Outbox, per-document templates, Send all); invoice Client read-only when paid/applied or a finance charge; one automatic resend on 409 RETRY with the same key; demo refuses what the engine refuses (void with payments, unapply with adjustments or draws); demo invoice list ordered as the engine (fixed a pre-existing create_invoice scenario failure on main); stray control characters in doc comments removed.
  - Checks: 249 live contract tests at 9fe75899 (incl. A/R aging = Balance Sheet A/R on 3 dates through payment, memo, bounce, void, refund); cl-ui 1404; ui-check create_invoice (+ --compare), credit_memo, sales_receipt, receive_payment, statements, finance_charges, preferences-email, invoice_email, client-center, home; full local `cargo xtask ci` green at 7276fdb.
  - Not done: review Notes N1, N3–N6 (owner decisions; see Follow-ups from PR-3); missing test 3 (monthly cash P&L).
  - Decisions: Q-P22u, Q-P22v, Q-P22w, Q-PR3a. Rows U-144–U-146.
  - Agents: process-reviewer (opus, xhigh), engine-implementer (opus), rust-implementer (opus) ×2 (fixes + email; create_invoice/aging test).