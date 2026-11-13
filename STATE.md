# STATE.md — ClientLedger

**Current phase:** A09 (ADD-03 Memorized Reports, transfer, export and print) next, then A10, PR-A2 and P29. A08 done 2026-11-12; A07 done 2026-11-12; PR-6 done 2026-11-11; P28 done 2026-11-11; PR-5 done 2026-11-10 (M3 complete); P27 done 2026-11-09; PR-A1 done 2026-11-09; A05, A12 and P26 done 2026-11-08. ADD-03 (A07–A10) right after PR-6; ADD-04 inside P35/P36; ADD-05 (L1–L5) with P35/P36/P39; ADD-06 A11 after P34.
**Status:** A09 next.
**Engine commit:** `e1f125ea` (kisaes-gl-engine `main`, pinned 2026-11-12 in A08 after #50 (U-216, `reports-revenue-net-v1`), #51 (U-217, `report-builder-preview-v1`)); before that `938e60a3` (kisaes-gl-engine `main`, pinned 2026-11-12 in A07 after #44 (U-118 amend, U-119a, migration 0276), #45 (U-119b, migration 0277), #46 (U-119c), #47 (U-119d), #48 (U-119e), #49 (U-119f)); before that `f5ab275c` (kisaes-gl-engine `main`, pinned 2026-11-11 in PR-6 after #42 (U-212), #43 (U-213, U-210, U-211)); before that `154c824d` (kisaes-gl-engine `main`, pinned 2026-11-11 in P28 after #33 (U-14, U-205), #34 (U-118, migration 0274), #35 (U-201, U-202), #36 (U-121 A/P half), #37 (U-203, migration 0275), #38 (U-204, U-207), #39 (U-206), #40 (U-208), #41 (U-209)); before that `e854e797` (kisaes-gl-engine `main`, pinned 2026-11-10 in PR-5 after #32 (U-199, U-200)); before that `4a240f3c` (kisaes-gl-engine `main`, pinned 2026-11-09 in P27 after #25 (U-188–U-190, migrations 0271, 0272), #26 (U-191, U-192), #27 (U-193), #29 (U-194–U-196), #30 (U-197), #31 (U-198)); before that `660fb9d4` (kisaes-gl-engine `main`, pinned 2026-11-09 in PR-A1 after #22 (U-185, U-186), #23 (U-187), #24 (U-99)); before that `88146fbd` (kisaes-gl-engine `main`, pinned 2026-11-08 in A05 after #15 (U-180, U-181, migration 0269) and #16 (U-182, U-183, migration 0270); includes #12 headers and dependency bumps); before that `f5df28d5` (KisaesDevLab/kisaes-gl-engine `main`, pinned 2026-11-08 in A12; packages/api, packages/shared and openapi.json trees identical to Vibe-MyBooks `1096b20d`); before that `1096b20d` (`main`, pinned 2026-11-08 after engine PRs #235 (U-164), #236 (U-166–U-168, U-172–U-174), #237 (U-171, U-77), #238 (U-169, U-170, U-175–U-178; migrations 0266–0268)); before that `f95985c0` (`main`, pinned 2026-11-07 after engine PRs #231 (U-163 FITID per connection, migration 0262), #233 (U-06 bank import batches, migration 0263), #232 (U-165 closing date on Bank Feed posting)); before that `29b31900` (`main`, pinned 2026-11-06 after engine PR #230 (PR-4 fixes, U-162, migration 0261)); before that `31d7ddd5` (`main`, pinned 2026-11-06 after engine PRs #224–#229 (P24 inventory: U-39b, U-156–U-161, U-25 inventory half, U-78; migrations 0258–0260)); before that `1c556ac6` (`main`, pinned 2026-11-02 after engine PR #222 (U-155, cash-basis report performance; migration 0257); before that `9be6da51` (pinned 2026-11-02 after engine PRs #216–#221 (P23 sales tax stack: U-147–U-154, U-25 sales-tax half; migrations 0252–0256); before that `9fe75899` (pinned 2026-11-01 after engine PRs #214 (U-144, U-145: credit memo and sales receipt email; finance-charge invoice keeps its client) and #215 (U-146: invoice client guard, 409 RETRY on receivables deadlocks); before that `bf2daf38` (pinned 2026-11-01 after engine PRs #209–#213 (P22 credit memo, sales receipt, refund, customer statement and finance-charge stack: U-137–U-143, U-112, U-121 A/R half); before that `eaa12bad` (pinned 2026-10-31 after engine PRs #205, #206, #208 and #207 (P21 Receive Payment stack: U-132–U-136); before that `c7b7d61d` (pinned 2026-10-31 after engine PRs #197, #199, #200, #201, #202 (P20 invoice stack: U-122–U-130, U-38, U-39a, U-40 and review fixes; migrations 0242–0246) and #203 (U-131, stored SMTP password; no smtpPass in company responses), and #198 (follow-redirects audit fix); before that `129deff7` (pinned 2026-10-29 after engine PR #196 (U-115–U-117, PR-2 purchasing fixes, migration 0241); before that `12187716` (pinned 2026-10-29 after engine PRs #194 (U-106–U-110, check printing, migration 0239) and #195 (U-111, U-113, document numbering and forms print queue, migration 0240); before that `e45658fb` (pinned 2026-10-28 after engine PR #193 (U-101–U-104, Pay Bills discounts, idempotent pay, check numbers, company scope; migrations 0236–0238); before that `5a9a8700` (pinned 2026-10-27 after engine PR #183 (U-94, document-drafts-statement-v1, migration 0235); before that `97d4d4cf` after engine PR #182 (U-97, restore verify no longer splits multi-byte characters, no migration); before that `4e0593c3` after engine PR #180 (U-95, document-drafts-attach-v1, no migration); before that `38cf97eb` after engine PRs #178 (U-92, document-drafts-v1, idempotency-docs-v1, migration 0233) and #179 (U-92d, document-assist-v1, migration 0234); before that `a88337e3` after engine PR #177 (U-93, intake-settings-v1, migration 0232); before that `b483c02e` after engine PR #176 (U-91, restore verify compares id-keyed jsonb semantically) and engine PRs #171 (idempotency-v1, migration 0229), #172 (run-due-filter-v1), #173 (ap-closing-v1, ap-edit-v1, bill-lines-v1, bill-terms-v1, bill-received-v1, idempotency-ap-v1, migration 0230), #174 (bill-ocr-v2) and #175 (attachments-company-v1, migration 0231); bump per Q-02)
**Last updated:** 2026-11-12
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
| PR-5 | Checkpoint: bank lifecycle process review | — | Done 2026-11-10 (fixes on `fix/PR-5`; engine PR #32 merged) |
| PR-6 | Checkpoint: reports process review | — | Done 2026-11-11 (fixes on `fix/PR-6`; engine PRs #42, #43 merged) |
| P20 | Create Invoice | M | Done 2026-10-31 |
| P21 | Receive Payment | H | Done 2026-11-01 |
| P22 | Credit memos, refunds, statements, finance charges | M | Done 2026-11-01 |
| P23 | Sales tax | H | Done 2026-11-02 |
| P24 | Inventory (average cost) | H | Done 2026-11-06 |
| P25 | Bank file import (QIF, QFX, OFX, CSV) | H | Done 2026-11-07 |
| P26 | Bank feed review | M | Done 2026-11-08 |
| P27 | Reconcile | H | Done 2026-11-09 |
| P28 | Report engine UI | H | Done 2026-11-11 |
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
| A05 | Match document drafts in the Bank Feed | M | Done 2026-11-08 |
| PR-A1 | Checkpoint: document to books (process review) | — | Done 2026-11-09 |

**ADD-02 — Read a vendor statement into Match Vendor Statement** (`addenda/ADD-02-statement-reading.md`; part of the beta; owner request 2026-10-26). Scheduling: A06 runs right after A04 merges (done); P18 started when the owner said go (2026-10-28).

| Phase | Title | Tier | Status |
| --- | --- | --- | --- |
| A06 | Read a vendor statement | M | Done 2026-10-27 |

**ADD-03 — Report Builder and Memorized Reports** (`addenda/ADD-03-report-builder.md`; part of the beta; owner request and Q&A 2026-10-30, Q-305–Q-315; engine U-118–U-121). Scheduling: A07–A10 run right after P28 and its PR-6 checkpoint, before P29; the `ReportBuilder.html` mockup is drafted for the owner's approval before A08; PR-A2 after A10.

| Phase | Title | Tier | Status |
| --- | --- | --- | --- |
| A07 | Report run engine, definitions, run in the report window | H | Done 2026-11-12 |
| A08 | Report Builder editor | M | Done 2026-11-12 |
| A09 | Memorized Reports, transfer, export and print | M | Next |
| A10 | AI drafting of report definitions | M | After A08 |
**ADD-04 — Distribution through winget** (`addenda/ADD-04-winget.md`; part of the beta; owner request and Q&A 2026-10-31, Q-317–Q-323). No A-phases: built inside P35/P36, switched on at P39 stage 2 (first manual submission, then `WINGET_ENABLED`). MSI per-machine only; UpgradeCode {5871208F-8AA3-4EC1-86A7-ABCF603A0765}; package `KisaesLLC.ClientLedger`.
**ADD-05 — VibeMyFirm Community License: tiers, keys and Pro gating** (`addenda/ADD-05-community-license.md`; owner request and Q&A 2026-11-06, Q-336–Q-342). **Draft: license texts in `licensing/`; waiting on the engine addendum** (`GET /license`, host signature). Phases L1–L4 inside P35/P36, L5 with P39 (Q-339). Business needs no key (Q-337); replaces PolyForm (Q-336).
**ADD-06 — MCP bookkeeper moves into the beta** (`addenda/ADD-06-mcp-beta.md`; owner 2026-11-07, Q-345). Phase **A11** (was post-beta P50) runs right after P34, before P35; checkpoint **PR-A3** (process review + security review) right after A11.
**ADD-07 — The engine moves to its own repository** (`addenda/ADD-07-engine-repo.md`; owner 2026-11-07, Q-346–Q-349). Phase **A12** runs right after P26 (its engine PRs merged first), before A05: split `packages/api` + `packages/shared` with history into `KisaesDevLab/kisaes-gl-engine`; MyBooks Online and ClientLedger pin it as a submodule. **A12 done 2026-11-08.**

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

- P06 shell placeholders (Q-P06g): every menu, rail and palette command whose screen is built later opens a placeholder tab. The phase that builds the screen removes its entry: P11 Item List / Tags / Clients & Projects / Vendors, P12 Open Register, P13 Write Check, P14 Make Deposit / Transfer Funds / Sales Receipt / Record Card Charge, P18 Pay Bills, P20 Create Invoice, P21 Receive Payment, P22 Credit Memo or Refund / Statements, P25 Import Bank File, P26 Bank Feed, P29 Set Closing Date / Users and Permissions / Audit Trail, P30 Import Payroll, P32 Send to My Accountant, P38 bundled help (F1 says "not available yet").
- P06 native macOS menu bar (Q-70, muda) is deferred (Q-P06b): in-window menu on every platform until macOS packaging.
- P16 Home lane actions open "arrives in Pxx" placeholders (Q-P16k): P18 Pay Bills, P20 Create Invoice / Sales Receipt, P21 Receive Payment, P26 Review Bank Feed, P29 Set Closing Date, P32 Send to My Accountant. Each phase wires its lane action.
- P16 Home "Bank file" balance column — U-77 / P25 (Q-P16d); "ready to accept" count — P26 (Q-P16f); "invoices more than 60 days past due" — done in P28 (Q-P16g, Q-P28r, Q-P28y); "Send to My Accountant · last sent" — P32 (Q-P16i).
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

## Follow-ups from A07

1. Compare ranges are not sent by the desktop yet (review #13); A08 adds them with the editor.
2. S-L8 (`hiddenNotice` shape) waits for P29 / U-33: add a contract test when the engine hides sensitive accounts.
3. Engine wording stays English as the engine writes it: the " · " separator, Title Case totals and "Customer"; left to the engine.
4. A click on a Total cell under the vertical scrollbar does nothing.
5. Formula-bold rows in Excel exports have no test.
6. Revenue by Category (standard report) shows 150.00 more on account 4000 than the P&L in Q4 2026; the builder starter ties to the P&L (Q-A07u(7)). Resolved in A08 by U-216 (engine #50): the summary now nets debits.
7. Engine: a section with earnings and no grouping adds the earnings lines when detail pages; no starter has that shape.
8. Section drills were not re-timed on the 1.2M-line performance fixture; cash 36-column Balance Sheet plus General Ledger takes 24.4 s of the 30 s limit.
9. After a pin bump, rebuild the engine exe (`cargo xtask engine`) before running the golden-reports tie-out.
10. Load more after a Refresh with the same stamp is not refetched (Q-A07t).
11. The detail tab's date controls are inert (the drill fixes the dates).
12. Memorize is disabled for builder reports until A08.

## Follow-ups from A08

1. Q-A08ai: formulas use the displayed sign (the engine's `valueExpr` applies polarity before formulas; the editor warns). Owner decision pending.
2. The cash 1.2M-line full run and its preview hit the 30 s timeout on this machine (accrual 12.6 s full, 6.9 s preview). Goes to P37.
3. Assets, Liabilities and Equity by Account summaries now show period net change, not ending balance (U-216). The owner may want ending balance.
4. QA-6: the preview pane is too narrow for six aging columns at 1440 wide (it scrolls).
5. QA-15: the cause may be different; re-check on the real engine.
6. At 200% zoom, Memorize can clip at the right edge of the toolbar.
7. The 200-row cap and the denied states are only unit-tested on the screen.
8. Print and Save changes refusals with a second user were not tested by hand.
9. The 24-column cap is not reachable (22 columns are offered).
10. The last commit (f8786f03) was checked with fmt, clippy and the builder tests only; full CI passed at 622fac75.
11. rustc ran out of memory during CI with full parallelism on this machine.

## Follow-ups from PR-6

1. Deferred tests: T5 (4-4-5 retail contract test; `fiscal_year_start` is covered by `report_fixes_v2`) and T6 (the tag filter is gone from Balance Sheet and Trial Balance, so no test).
2. N1 (sensitive accounts, Q-157) goes to P29: nothing is hidden today, so P29 must cover every surface P28 added, preferably engine-side (U-33) — statement rows and section totals, GL / Journal / Transaction List drills, tag split rows, BvA rows, column layout, exports / print / email, memorized definitions naming a hidden account, and Ask (`promptNames` sends up to 500 active account names to the model). List in `target/reviews/PR-6.md` N1.
3. Engine, from the review: N4 (Journal at the 50,000-line cap cuts the last transaction part-way; trim whole transactions) and N6 (default dates on exact routes mix UTC `today()` and local `yearStart()`).
4. PR6-QA-3 root cause not reproduced (a cash-basis drill opening on accrual): fixtures, window flow and the demo all open on Cash; possibly a mis-click on the BS 01/31 accrual tab. Recheck by hand in the next report-touching QA.
5. QA not covered: budget copy, tag split "All others"/untagged drills, 4-4-5 prior period, dark theme, 200% zoom.
6. U-214 (Open): `ap-1099-prep` should say which threshold applies (`reportable_thresholdExact`) so 1099 Preparation can read "Below $2,000"; until then it says "Below" (Q-PR6h).
7. Owner to confirm the 1099 thresholds: $2,000 from 2026 (2025 law), attorney proceeds (MISC-10) kept at $600 (U-212).
8. 1099 Center amounts changed for every MyBooks tenant (U-212: cash paid in the year, checks included).
9. MyBooks behavior changes from U-213 and U-211: budget copy replaces the target's lines, legacy cash Trial Balance retained earnings, legacy recurring checks (U-210).
10. security-reviewer items from the review: the legacy `/reports/ap-1099-prep` route returns the full tax ID (exact route now last four only); report email and Ask need only `reports:read` (view-only users can send report PDFs through the company SMTP; P29 decision).

## Follow-ups from P28

1. The budget editor labels its columns "Period n" while Budget vs. Actual shows month names.
2. The Duplicate Finder note ("what it matches") shows only when rows exist.
3. Sales tax is missing from the golden story, so Sales Tax Liability has no golden (Q-P28ag); covered by the P23 contract goldens.
4. Budget vs. Actual emailed as PDF fails for the wide 52-column table ("file couldn't be saved"); cl-print follow-up (the scenario emails CSV).
5. Demo only: opening A/R Aging seeds the P22 10/01 credit memo, which moves the demo's October P&L until restart; the demo invoice opened from a drill is blank.
6. Legacy (non-exact) A/P aging still shows today's balances (U-121 note); budgets are unique per fiscal year per tenant, not per company; fill-from-actuals uses calendar months on a 4-4-5 calendar (Q-P28ac).
7. View-only users can save memorized reports (the engine follows the report-packs precedent); P29 permissions decide.
8. Disaster-recovery full restore of report definitions is untested end to end (engine PR #34 gap).
9. Not covered by QA: the stale-version budget path, Ask with a local Ollama model, PDF text content, the Journal, Print.

## Follow-ups from PR-5

1. "Look already entered" on Import Bank File counts the whole account (new rows plus rows already waiting); a count for this file only needs an engine per-row flag.
2. Go to Bank Feed puts the cursor on the matched line only when it is on the first loaded page of the Accepted view (account, view and filters are always set).
3. Not done in PR-5: T9 (engine test) and a control-channel scenario for the register's refused delete of a matched entry (unit tests cover the wording).
4. Bank-file payee title-casing (P27 follow-up 3) kept by the owner (Q-P27w).
5. QA round 2 minors: Delete asks "Delete this transaction?" before the matched-entry refusal; at 200% zoom keyboard focus on Skip doesn't scroll it into view; the import check step's count reads "0 new" while Send says 1 after Add as new; Recent checks says "No checks to this payee yet." while editing that payee's check. Not run by hand: Add as new on an accepted row, delete wording for a Bank-Feed-created entry.

## Follow-ups from P27

1. Q-P27s (Undo hidden for non-Admins) is untested on the real engine until P29 builds Users and Permissions; P29 QA checks it. Also untested: demo vs live behaviour of that button, and Tab-from-button pane re-entry.
2. Q-P27v: re-finishing an undone closed-period reconciliation asks no closing password (Undo does). Default: leave it, since re-finishing posts nothing; owner to confirm.
3. Bank-file payees are title-cased ("QA SYNTHETIC DEPOSIT" becomes "Qa Synthetic Deposit") by the engine's bank-name cleaner (`engine/packages/api/src/utils/bank-name-cleaner.ts` `titleCase`); meant for all-caps bank text. Revisit in PR-5.
4. The reconcile shortcuts (Ctrl+Enter, Ctrl+Shift+A/U, Ctrl+E) don't fire while a button has focus; the buttons are reachable by Tab and Space.
5. Migration 0272 kept the newest `updated_at` duplicate (review m1); it ran once, note only.
6. ~~Feed links of entries whose amount changed (review m5 remainder, Q-P27h) go to PR-5.~~ Done in PR-5 (U-199, "Entry changed", Q-PR5a).
7. Vibe-MyBooks #246 (owner review, Q-P27t): the engine pin bump there pulls the engine's dependency bump and license-field change; the web client has no closing-password prompt and no service charge or discrepancy UI.

## Follow-ups from PR-A1

1. ~~P27 hands-on QA runs the whole document-to-books flow end to end (intake → Enter Bill → Pay Bills → bank file → Match → Reconcile), which PR-A1 QA didn't reach~~ Done in P27 QA round 2; "Bring them back" Yes and Discard after Undo by hand is still open (unit-tested).
2. The 10-second notice toast covers Clear and Save & Close on forms (also on main) — clicks land on the toast.
3. Invoice, deposit, journal, transfer, loan, owner-money and personal-funds forms reload their name lists only when they have no unsaved edits (bill, check and card charge fixed in PR-A1).
4. P37 security: the viewer parses PDFs in the desktop process (no decompression cap); the reading worker's no-network rule isn't enforced on Windows/macOS (Q-A03d); right-to-left and control characters reach attachment names and "Save a copy" file names.
5. Not PR-A1 scope, seen in QA: auto-read skipped one PDF; card form refuses a negative discount line; Pay Bills "Due on or before" hides bills with no due date; "qa · Admin" in the status bar; scenario doc_review_flow ordering flake (same-date documents).
6. Engine U-185 vendor-less duplicate matches carry no vendor name (desktop says "any vendor").

## Follow-ups from A05

1. The engine stores a document total of zero or more (`doc_total >= 0`), so credits lose their sign in the draft list; the desktop reads the signed total from the reading (at most 25 full reads per list load). U-184 (planned): store the sign or a credit flag.
2. Not hand-tested on the real engine: a rule's "Accept all" on a line with a receipt (Q-A05e; demo test covers it), Undo after a late attach (contract test covers it), closed-period line with a receipt (P29).
3. The register edit pane shows the split warning and a blank Payee as soon as it opens (seen in A05 QA; not investigated).
4. Contract test `feed_receipts::stale_or_used_receipts_post_nothing` failed twice in one fix pass, then passed (full suite 314/314 after); it now prints the full outcome — watch it.
5. Drafts beyond the newest 2,000 are never offered in the Bank Feed (they attach from Review Documents).
6. Duplicate-warning wording says "This receipt" on Enter Bill too.

## Follow-ups from P26

1. Editing an entry's bank line re-creates the line, so a matched feed row keeps the old line id and the new line can be matched again (review m5) — P27 / checkpoint PR-5. Partly done in P27 (U-192 keeps line ids on edit, U-198 category edits of reconciled entries); feed links of entries whose amount changed done in PR-5 (U-199).
2. `POST /banking/feed/accept`'s "already has a match" skip still uses 3 days, not the owner's transfer window (U-177 note).
3. Not tested by hand on the real engine: closed-period password (Set Closing Date is P29; demo scenario and engine tests cover it), bill/invoice-payment match, Review again, view-only user — rerun in P29 QA / PR-5.
4. Desktop: tag picker in the rule editor; contract tests for the rule routes; password prompt says Delete when undoing a locked entry; Import Bank File reopens the old window as it was (QA-21); import balance box clipped at 1440 wide.
5. Pay from, refunds and the register type still assume detail type `bank` (checking/savings accounts) — outside P26.
6. `feed_accept::locked_rows_need_the_password` failed once in a full contract run (shared closing date on the test company); passed on rerun.

## Follow-ups from P25

1. Engine PR #234 and its desktop mirror, before P26: Cancelled state, processed/total, existingAmount/existingDate, Undo timeout > 20 s.
2. Old-style bank rule matches get no review-bucket entry (engine DB reference error, found in #234); fix in P26.
3. The engine categorization pipeline logs query parameters on failure (privacy); fix in P26.
4. U-164 decimal-exact feed posting, before P26.
5. Untested by hand until P26: undo blocked after a row is used, rule suggestions, closed-period rows, view-only user.
6. A reopened wizard doesn't re-check an earlier import's preparing state.
7. Disabled-button contrast (shared style).
8. Orphaned bank-link-* keychain entries from pre-Q-P25u builds (dev machines only).

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

### M3 — Banking (P25–P27, checkpoint PR-5; ADD-02 A05, ADD-07 A12, checkpoint PR-A1), completed 2026-11-10

- **Shipped:** bank file import of QIF, QFX, OFX and CSV with batches, dedupe and balance check (P25); Bank Feed review with rules, matching, transfers, deposits with fees, Undo and Review again (P26); document drafts matched in the Bank Feed (A05); document-to-books checkpoint fixes (PR-A1); Reconcile with Begin charges, worksheet, Finish at exactly 0.00, Undo modes, discrepancy report and the practice company (P27); engine moved to its own public repo kisaes-gl-engine (A12); bank lifecycle checkpoint fixes: "Entry changed" bank lines, feed guards, edit-form notices, sign hint by resulting sign (PR-5).
- **Screenshots:** `target/ui-check/<screen>/` (light and dark), notably import_bank_file, bank_feed*, reconcile (with `--compare`), register; plus the CI `ui-check` artifacts on each PR.
- **Questions added:** Q-P25a–ag, Q-P26a–z, Q-P27a–w, Q-PR5a–c, Q-A05a–h, Q-PRA1a–d, Q-346–Q-354; owner Q&As at each phase end. Owner to confirm: none open from M3 (Q-P27v and Q-P27w answered 2026-11-10).
- **Engine:** pin e854e797 (Vibe-MyBooks #231–#238, then kisaes-gl-engine #11–#32 merged; U-rows through U-200, migrations through 0272). Pending: Vibe-MyBooks #246 (web client for the reconcile fields, owner review, Q-P27t); U-184 withdrawn; U-98, U-100, U-118–U-121 Planned.
- **Reviews:** combined opus review per phase, process reviews PR-A1 and PR-5 (opus, xhigh), hands-on QA against the real engine at every phase end (P25, P26, A05, PR-A1, P27 three rounds, PR-5 two rounds); no money errors found in PR-5 QA.

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

- 2026-11-12 — A08 Report Builder editor complete on `phase/A08-report-builder-editor`; PR with auto-merge. Tier M.
  - Shipped: the Report Builder editor (outline of six row kinds, Row and Columns tabs, Guided and Advanced start points, live preview through the request queue, problems callouts), Save changes and Memorize for builder reports, compare ranges sent by the desktop (prior period of October = September 1-30), "Edit in Report Builder" beside Customize, demo backend parity, `report_builder` scenario, KEYBOARD.md regenerated.
  - Owner decisions this phase: Q-A08b mockup approved; Q-A08c-g drafted defaults; Q-A08h net every report the account-detail function feeds (engine #50); Q-A08i customer and aging columns; Q-A08j engine preview mode (engine #51); Q-A08o remove-and-say-so.
  - Engine (kisaes-gl-engine, merged; pin e1f125ea): #50 U-216 Revenue by Category and Assets/Liabilities/Equity by Account summaries total the period net (`reports-revenue-net-v1`); #51 U-217 `purpose: 'preview'` cuts detail at 50 rows per line (`report-builder-preview-v1`). No migrations.
  - MyBooks Online output changes (same engine code): Revenue by Category nets debits; Assets/Liabilities/Equity by Account summaries are now period net change; the tax estimator readiness check keeps a gross check. Revenue by Category golden re-blessed (394,895.97 to 394,745.97).
  - Reviews (local, not committed): reviewer opus (target/reviews/A08.md) 0 blocker, 4 major, 9 minor, 3 notes; ui-verifier opus (A08-ui.md) partial: 2 high, 8 medium, about 22 low. Hands-on QA (target/plans/A08-qa-findings.md): 0 high, 5 medium, 11 low. All fixed except as noted in Follow-ups from A08.
  - Acceptance: [x] keyboard: every key in ADD-03's table plus F6 / Shift+F6 and Guided Alt+B / Alt+N, every builder action by keyboard; [x] the preview matches a full run up to the cut (same `RunRef` and engine figures; only detail rows past 50 are cut); [x] mockup compare done, deviations Q-A08z, Q-A08aa-ah; [x] compare ranges sent; [x] Revenue by Category ties to the P&L (Q-A08h).
  - Checks: `cargo xtask ci` green; contract 351 with `CL_API_REQUIRE=1`; goldens (Revenue by Category re-blessed); ui-check report_builder --compare, reports-builder and reports light and dark.
  - Decisions: Q-A08a-Q-A08ai. Rows U-216 and U-217 Merged.
  - Agents: architect (opus); rust-implementer (opus) x2 (A, B) + fresh opus fix pass x2; engine-implementer (opus) #50, #51; reviewer (opus); ui-verifier (opus); general-purpose (opus) QA; bookkeeper (sonnet).
- 2026-11-12 — A07 Report run engine, definitions, run in the report window complete on `phase/A07-report-builder`; PR with auto-merge. Tier H.
  - Shipped: the builder run engine and saved builder definitions (schema v1 mirrored in cl-model and checked against the engine's shared fixtures), the typed client and adapter in cl-api, the 20 starters under "Report Builder" in the report window (expand and collapse, Expand all, Load more paging, detail tabs, drills that open the source form, exports), demo backend parity, `reports-builder` scenario, contract and tie-out tests, user docs in `docs/user/reports.md`.
  - Engine (kisaes-gl-engine, merged; pin 938e60a3): #44 U-118 amend and U-119a schema and kind `builder` (`report-defs-builder-v1`, migration 0276); #45 U-119b foundations (`report-foundations-v1`, migration 0277); #46 U-119c run, run/detail, fields (`report-builder-v1`); #47 U-119d starters (`report-builder-starters-v1`); #48 U-119e drill fixes (`report-builder-drill-v1`); #49 U-119f starter fixes (`report-builder-starters-v2`).
  - MyBooks Online output changes (same engine code): Sales by Client and Sales by Item are natural net, so discounts reduce sales (Q-A07b); A/R Aging Summary buckets are exact sums, customers carry `bucketsExact` and `totalExact` (Q-A07c); the ledger stamp moves when payment, bill payment and vendor credit applications change; starters v2 changed the field lists, Transaction Detail by Account (8 natural-sign sections), Open Invoices and Unpaid Bills (type filter), and range ledgers list opening-only accounts.
  - Review (reviewer opus; target/reviews/A07.md and A07-security.md, local): 1 blocker, 7 major, 11 minor; security-reviewer opus 0 critical, 0 high, 1 medium, 8 low. All fixed except compare ranges (A08) and S-L8 (P29); engine #48 and the review fix pass.
  - Hands-on QA (general-purpose opus, real engine a1ddcc47): 0 high, 5 medium, 6 low; all fixed (engine #49 and the QA fix pass).
  - Acceptance: [x] every starter ties to its standard report to the penny (20-starter tie-out, both bases; Revenue by Category difference in follow-ups, Q-A07u(7)); [x] drill-down from any subtotal sums to it (per-starter drill-sum test); [x] A/R and A/P open items as of a past date match the standard agings; [x] no floating-point money arithmetic in the run path (nofloat gate, exact decimal strings).
  - Checks: `cargo xtask ci` green; contract 347/347 with `CL_API_REQUIRE=1`; goldens unchanged (nothing re-blessed); ui-check reports-builder and reports light and dark (screenshots in `target/ui-check/reports-builder`).
  - Decisions: Q-A07a–Q-A07v. Rows U-119a–U-119f Merged; U-215 present (Open, P29).
  - Agents: engine-implementer (opus) x3 runs (PR-A to PR-D, PR-E, PR-F); rust-implementer (opus) x3 (WP1-3, review fix pass, QA fix pass); architect (opus) plan; reviewer (opus); security-reviewer (opus); general-purpose (opus) QA and the ReportBuilder.html draft; bookkeeper (sonnet).
- 2026-11-11 — Checkpoint PR-6 Reports complete on `fix/PR-6`; PR with auto-merge. Process review (process-reviewer opus, xhigh; target/reviews/PR-6.md, local, not committed): 3 Blocking, 6 Should-fix, 7 Notes, 10 missing tests.
  - Engine (kisaes-gl-engine, merged; pin f5ab275c): #42 U-212 1099 Summary, 1099 Preparation and 1099 Center count cash paid in the year (same payments as `total_paid`, checks included), 1099 thresholds by year ($600 through 2025, $2,000 from 2026), `report-1099-paid-v1`; #43 U-213 cash Trial Balance retained earnings on the report basis (B1), equity drill flags (B2), `fiscal_year_start` on Balance Sheet, Trial Balance and General Ledger (S1), tag filter refused on exact Balance Sheet and Trial Balance (S2), `meta.cashArApResidue` (S5), 1099 Prep tax ID last four only, `report-fixes-v2`; also U-210 (legacy recurring template copy posts a check; the QA-1 item was a card-charge template) and U-211 (budget copy replaces the target's lines). No migrations.
  - Desktop: Prior period compares with the previous period on the fiscal calendar (B3); equity, client, untagged and net income drills sum to their cells (B2, S1-S6); tags off balance reports; demo parity; xlsx dates as dates and bold subtotals, PDF note when sub-cent amounts can miss a total by a cent (N3); cash-basis rules note dropped (Q-P28ai); cash A/R and A/P note with a "See these entries" link; budget copy says it replaces; 2025 year-boundary slice in the golden story with cross-checks (T2, T3, T4, T10).
  - Hands-on QA against the real engine (ui-verifier opus): 4 findings, all fixed with a regression test each (PR6-QA-1 prior/change/% columns clipped by a stale width cache, QA-2 Balance Sheet net income drill per column with Prior period on, QA-3 note link visible, QA-4 1099 filing threshold column).
  - Checks: `cargo xtask ci` green at fb760287; contract 344/344 with `CL_API_REQUIRE=1`; goldens stable over 3 runs (cash Trial Balance and 1099 goldens re-blessed); ui-check reports, budgets, reports-ask, reports-aging, reports-tag-split pass light and dark.
  - Decisions: Q-P28aj and Q-P28ak answered; Q-PR6a–Q-PR6h. Rows U-210, U-211 Done; U-212, U-213 Done; U-214 Open.
  - Agents: process-reviewer (opus, xhigh); engine-implementer (opus) x2 (#42, #43); rust-implementer (opus) x2 (review fix pass, QA fix pass); ui-verifier (opus) QA; bookkeeper (sonnet).
- 2026-11-11 — P28 Report engine UI complete on `phase/P28-reports`; PR with auto-merge.
  - Shipped: WP1 report window (presets incl. 4-4-5, basis, columns, filters, collapse, sort, drill-down, memorize, CSV, xlsx and PDF export) with Home's chart month and 60-day past-due line; WP2 exact statements and ledgers (P&L, Balance Sheet, Trial Balance, GL, Transaction Detail by Account) with % of income, prior period, YTD, by month, client and tag columns and tag rows under accounts; WP3 agings, open items and balances, sales and spending, banking, 1099 Summary by box, Closing Date Exceptions and Duplicate Finder; WP4 budgets, Budget vs. Actual, report email and ask-a-question; WP5 demo report engine, scenarios, golden story (53 goldens) and user docs (`docs/user/reports.md`, `budgets.md`).
  - Engine (kisaes-gl-engine, merged; pin 154c824d): #33 U-14 exact reports and U-205 1099 by box (`reports-exact-v1`); #34 U-118 memorized report definitions, migration 0274; #35 U-201 report options and U-202 drill target (`reports-columns-v1`, `reports-drill-v1`); #36 U-121 A/P as-of aging and open balances; #37 U-203 exact budgets and Budget vs. Actual, migration 0275; #38 U-204 Closing Date Exceptions and audit filters, U-207 report email, desktop rate limit 600/min; #39 U-206 ask-a-question (`reports-ask-v1`); #40 U-208 report POST routes reachable; #41 U-209 QA report fixes (whole-cent cash shares, check numbers, 1099 eligibility, journal debit totals). Open rows: U-210, U-211.
  - Acceptance: [x] golden reports match to the penny (53 goldens, stable over 3 runs; Dec cash foots by hand); [x] drill-down from any P&L amount sums to that amount (each cell equals its GL drill); [x] Excel exports keep numbers numeric; [x] Budget vs. Actual matches the hand fixture (`fixtures/budgets/bva_hand.json`, accrual, cash and tag split); [x] tagged amounts shown separately: account row untagged, tag rows labelled appended or prepended, optional account total equals untagged plus tag rows; [x] screen matches the mockup, deviations listed below and in Q-P28i.
  - Checks: `cargo xtask ci` green at 842f2ca0; contract suite 343 passed with `CL_API_REQUIRE=1`; ui-check reports, budgets, reports-ask, reports-aging, reports-tag-split pass light and dark (re-run on the final commit with the U-209 pin); reports `--compare` reviewed.
  - Mockup deviations: toolbar row above the Ask row; column captions are the engine's ("Total", "YTD"); parent roll-up rows show "—" for % of income; zero rows kept where a parent or YTD is non-zero; catalog lists more reports (Q-P28i); status bar hints generic; demo A/R Aging is the P22 demo story and moves the demo October P&L until restart; sample chart has one income account; BvA PDF email fails for the wide table (CSV works).
  - Reviews: combined review (opus; target/plans/P28-review-findings.md): 3 Blocking (budget editor lost or misfiled rows, old table exported under new labels, step 1 filters missing) and 11 Should-fix, all fixed; hands-on QA against the real engine (opus ui-verifier; target/plans/P28-qa-findings.md): 15 findings, fixed except the deferred ones in Follow-ups from P28 (U-210, U-211, period labels).
  - Decisions: Q-P28d–Q-P28ah (owner review of `cashBasisRules` wording: Q-P28ai). Rows U-14, U-118, U-121 (A/P), U-201–U-209 Done; U-210, U-211 Open (both Done in PR-6).
  - Agents: architect (opus); engine-scout (opus); engine-implementer (opus) x9 PRs; rust-implementer (opus) WP1-4, WP5 and the fix pass; reviewer (opus); ui-verifier (opus) QA; bookkeeper (sonnet). The 2-implementer rule was exceeded (WP split, fix pass).
