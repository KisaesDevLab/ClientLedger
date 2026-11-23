# STATE.md — ClientLedger

**Current phase:** P34 — Backup scheduling, recovery, and data integrity (next). PR-8 done 2026-11-22; P33 done 2026-11-22; P32 done 2026-11-19; P31 done 2026-11-19; P30 done 2026-11-17; P29 done 2026-11-16; PR-A2 done 2026-11-16; A10 done 2026-11-15; A09 done 2026-11-13; A08 done 2026-11-12; A07 done 2026-11-12; PR-6 done 2026-11-11; P28 done 2026-11-11; PR-5 done 2026-11-10 (M3 complete); P27 done 2026-11-09; PR-A1 done 2026-11-09; A05, A12 and P26 done 2026-11-08. ADD-03 (A07–A10) right after PR-6; ADD-04 inside P35/P36; ADD-05 (L1–L5) with P35/P36/P39; ADD-06 A11 after P34; ADD-08 is P35.1, after P35. ADD-09 R0 done 2026-11-22, R1–R5/R8 with P35/P36 (proposed, Q-ADD9a).
**Status:** PR-8 (LAN multi-user checkpoint) done 2026-11-22 on `fix/PR-8`; P34 next. U-285 (the MyBooks Online import page) is Online-side and owner-pending.
**Engine commit:** `e51d7615` (kisaes-gl-engine `main`, pinned 2026-11-22 in PR-8 after #101 (U-295, `maintenance-sessions-v1`; no migration)); before that `ffb2d750` (kisaes-gl-engine `main`, pinned 2026-11-20 in P33 after #88 (U-288), #89 (U-289), #90 (U-290), #91 (U-291), #92 (U-292), #93 (U-293), #94 (U-294), #95 (stale race fix), #96 (import undo ignores change-feed rows), #97 and #98 (review fixes) and #99 (posting lock order); migrations 0285–0289); before that `401eaeb3` (kisaes-gl-engine `main`, pinned 2026-11-19 in P32 after #82 (U-282, `tenant-archive-v2`), #83 (U-283, `archive-verification-totals-v1`), #84 (U-284, `tenant-archive-transfer-v1`, migration 0283), #85, #86 (migration 0284, schema version 285) and #87); before that `ef87c736` (kisaes-gl-engine `main`, pinned 2026-11-19 in P31 after #72 (U-260), #73 (U-261–U-263, `imports-batch-v1`, migration 0282), #75 (U-264, `qbd-open-items-v1`), #76 (U-265, `qbd-mapping-v1`), #77 (U-266, `qbd-dates-v1`), #78 (U-267, U-271, `imports-tieout-v1`), #79 (U-272, `qbd-retained-earnings-v1`), #80 (U-268, `imports-transactions-v1`) and #81 (U-273, `imports-review-fixes-v1`)); before that `1f2c9bb3` (kisaes-gl-engine `main`, pinned 2026-11-18 in P31 after #79 (U-272; `qbd-retained-earnings-v1`) and #80 (U-268; `imports-transactions-v1`); no migration); before that `d9b2e037` (kisaes-gl-engine `main`, pinned 2026-11-18 in P31 after #78 (U-267, U-271; `imports-tieout-v1`; no migration)); before that `a3d9a509` (kisaes-gl-engine `main`, pinned 2026-11-18 in P31 after #72 (U-260), #73 (U-261–U-263), #75 (U-264), #76 (U-265) and #77 (U-266)); before that `10ed13e4` (kisaes-gl-engine `main`, pinned 2026-11-17 in P30 after #67 (U-250, U-242, U-243, U-246), #68 (U-244, U-245, U-251), #69 (U-247), #70 (U-252) and #71 (U-253, review fixes; `payroll-import-review-v1`)); before that `5cce402d` (kisaes-gl-engine `main`, pinned 2026-11-17 in `fix/P29-owner-answers` after #66 (U-240)); before that `95596786` (pinned 2026-11-17 after #65 (U-238; `sensitive-accounts-grant-v1`)); before that `568b1f69` (kisaes-gl-engine `main`, pinned 2026-11-16 in P29 after #59 (U-33 part 1), #60 (U-23, U-227, U-230), #61 (U-215, U-229, U-232), #62 (U-33 part 2) and #64 (P29 review fixes; `sensitive-accounts-doors-v1`)); before that `03d85104` (kisaes-gl-engine `main`, pinned 2026-11-16 in PR-A2 after #58 (U-225, U-226; `report-builder-ties-v1`, `report-refs-v1`)); before that `9487bfa8` (kisaes-gl-engine `main`, pinned 2026-11-15 in the A10 QA fixes after #57 (U-224)); before that `feb8474b` (kisaes-gl-engine `main`, pinned 2026-11-15 in A10 after #56 (U-120, `report-builder-ai-v1`)); before that `cf0cb239` (kisaes-gl-engine `main`, pinned 2026-11-12 in A09 after #52 (U-218, `report-builder-formula-sign-v1`), #53 (U-219, `reports-by-account-balance-v1`), #54 (U-220, `report-defs-transfer-v1`)); before that `e1f125ea` (kisaes-gl-engine `main`, pinned 2026-11-12 in A08 after #50 (U-216, `reports-revenue-net-v1`), #51 (U-217, `report-builder-preview-v1`)); before that `938e60a3` (kisaes-gl-engine `main`, pinned 2026-11-12 in A07 after #44 (U-118 amend, U-119a, migration 0276), #45 (U-119b, migration 0277), #46 (U-119c), #47 (U-119d), #48 (U-119e), #49 (U-119f)); before that `f5ab275c` (kisaes-gl-engine `main`, pinned 2026-11-11 in PR-6 after #42 (U-212), #43 (U-213, U-210, U-211)); before that `154c824d` (kisaes-gl-engine `main`, pinned 2026-11-11 in P28 after #33 (U-14, U-205), #34 (U-118, migration 0274), #35 (U-201, U-202), #36 (U-121 A/P half), #37 (U-203, migration 0275), #38 (U-204, U-207), #39 (U-206), #40 (U-208), #41 (U-209)); before that `e854e797` (kisaes-gl-engine `main`, pinned 2026-11-10 in PR-5 after #32 (U-199, U-200)); before that `4a240f3c` (kisaes-gl-engine `main`, pinned 2026-11-09 in P27 after #25 (U-188–U-190, migrations 0271, 0272), #26 (U-191, U-192), #27 (U-193), #29 (U-194–U-196), #30 (U-197), #31 (U-198)); before that `660fb9d4` (kisaes-gl-engine `main`, pinned 2026-11-09 in PR-A1 after #22 (U-185, U-186), #23 (U-187), #24 (U-99)); before that `88146fbd` (kisaes-gl-engine `main`, pinned 2026-11-08 in A05 after #15 (U-180, U-181, migration 0269) and #16 (U-182, U-183, migration 0270); includes #12 headers and dependency bumps); before that `f5df28d5` (KisaesDevLab/kisaes-gl-engine `main`, pinned 2026-11-08 in A12; packages/api, packages/shared and openapi.json trees identical to Vibe-MyBooks `1096b20d`); before that `1096b20d` (`main`, pinned 2026-11-08 after engine PRs #235 (U-164), #236 (U-166–U-168, U-172–U-174), #237 (U-171, U-77), #238 (U-169, U-170, U-175–U-178; migrations 0266–0268)); before that `f95985c0` (`main`, pinned 2026-11-07 after engine PRs #231 (U-163 FITID per connection, migration 0262), #233 (U-06 bank import batches, migration 0263), #232 (U-165 closing date on Bank Feed posting)); before that `29b31900` (`main`, pinned 2026-11-06 after engine PR #230 (PR-4 fixes, U-162, migration 0261)); before that `31d7ddd5` (`main`, pinned 2026-11-06 after engine PRs #224–#229 (P24 inventory: U-39b, U-156–U-161, U-25 inventory half, U-78; migrations 0258–0260)); before that `1c556ac6` (`main`, pinned 2026-11-02 after engine PR #222 (U-155, cash-basis report performance; migration 0257); before that `9be6da51` (pinned 2026-11-02 after engine PRs #216–#221 (P23 sales tax stack: U-147–U-154, U-25 sales-tax half; migrations 0252–0256); before that `9fe75899` (pinned 2026-11-01 after engine PRs #214 (U-144, U-145: credit memo and sales receipt email; finance-charge invoice keeps its client) and #215 (U-146: invoice client guard, 409 RETRY on receivables deadlocks); before that `bf2daf38` (pinned 2026-11-01 after engine PRs #209–#213 (P22 credit memo, sales receipt, refund, customer statement and finance-charge stack: U-137–U-143, U-112, U-121 A/R half); before that `eaa12bad` (pinned 2026-10-31 after engine PRs #205, #206, #208 and #207 (P21 Receive Payment stack: U-132–U-136); before that `c7b7d61d` (pinned 2026-10-31 after engine PRs #197, #199, #200, #201, #202 (P20 invoice stack: U-122–U-130, U-38, U-39a, U-40 and review fixes; migrations 0242–0246) and #203 (U-131, stored SMTP password; no smtpPass in company responses), and #198 (follow-redirects audit fix); before that `129deff7` (pinned 2026-10-29 after engine PR #196 (U-115–U-117, PR-2 purchasing fixes, migration 0241); before that `12187716` (pinned 2026-10-29 after engine PRs #194 (U-106–U-110, check printing, migration 0239) and #195 (U-111, U-113, document numbering and forms print queue, migration 0240); before that `e45658fb` (pinned 2026-10-28 after engine PR #193 (U-101–U-104, Pay Bills discounts, idempotent pay, check numbers, company scope; migrations 0236–0238); before that `5a9a8700` (pinned 2026-10-27 after engine PR #183 (U-94, document-drafts-statement-v1, migration 0235); before that `97d4d4cf` after engine PR #182 (U-97, restore verify no longer splits multi-byte characters, no migration); before that `4e0593c3` after engine PR #180 (U-95, document-drafts-attach-v1, no migration); before that `38cf97eb` after engine PRs #178 (U-92, document-drafts-v1, idempotency-docs-v1, migration 0233) and #179 (U-92d, document-assist-v1, migration 0234); before that `a88337e3` after engine PR #177 (U-93, intake-settings-v1, migration 0232); before that `b483c02e` after engine PR #176 (U-91, restore verify compares id-keyed jsonb semantically) and engine PRs #171 (idempotency-v1, migration 0229), #172 (run-due-filter-v1), #173 (ap-closing-v1, ap-edit-v1, bill-lines-v1, bill-terms-v1, bill-received-v1, idempotency-ap-v1, migration 0230), #174 (bill-ocr-v2) and #175 (attachments-company-v1, migration 0231); bump per Q-02)
**Last updated:** 2026-11-22
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
| P29 | Users, permissions, closing date, audit trail | M | Done 2026-11-16 |
| P30 | Payroll import | M | Done 2026-11-17 |
| P31 | QuickBooks Desktop import with tie-out, spreadsheet import, other accounting systems | H | Done 2026-11-19 |
| P32 | Transfer file for MyBooks Online, and IIF export | H | Done 2026-11-19 (U-285, the Online import page, is owner-pending) |
| P33 | LAN multi-user | H | Done 2026-11-22 (PR-8 checkpoint done 2026-11-22) |
| P34 | Backup scheduling, recovery, and data integrity | H | |
| P35 | Packaging | L | |
| P35.1 | QuickBooks Desktop import through the QuickBooks SDK (ADD-08) | H | |
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
| A09 | Memorized Reports, transfer, export and print | M | Done 2026-11-13 |
| A10 | AI drafting of report definitions | M | Done 2026-11-15 |
| PR-A2 | Checkpoint: Report Builder (process review) | — | Done 2026-11-16 (fixes on `fix/PR-A2`; engine PR #58 merged) |
**ADD-04 — Distribution through winget** (`addenda/ADD-04-winget.md`; part of the beta; owner request and Q&A 2026-10-31, Q-317–Q-323). No A-phases: built inside P35/P36, switched on at P39 stage 2 (first manual submission, then `WINGET_ENABLED`). MSI per-machine only; UpgradeCode {5871208F-8AA3-4EC1-86A7-ABCF603A0765}; package `KisaesLLC.ClientLedger`.
**ADD-05 — VibeMyFirm Community License: tiers, keys and Pro gating** (`addenda/ADD-05-community-license.md`; owner request and Q&A 2026-11-06, Q-336–Q-342). **Draft: license texts in `licensing/`; waiting on the engine addendum** (`GET /license`, host signature). Phases L1–L4 inside P35/P36, L5 with P39 (Q-339). Business needs no key (Q-337); replaces PolyForm (Q-336).
**ADD-06 — MCP bookkeeper moves into the beta** (`addenda/ADD-06-mcp-beta.md`; owner 2026-11-07, Q-345). Phase **A11** (was post-beta P50) runs right after P34, before P35; checkpoint **PR-A3** (process review + security review) right after A11.
**ADD-07 — The engine moves to its own repository** (`addenda/ADD-07-engine-repo.md`; owner 2026-11-07, Q-346–Q-349). Phase **A12** runs right after P26 (its engine PRs merged first), before A05: split `packages/api` + `packages/shared` with history into `KisaesDevLab/kisaes-gl-engine`; MyBooks Online and ClientLedger pin it as a submodule. **A12 done 2026-11-08.**
**ADD-08 — Import from QuickBooks Desktop through the SDK** (`addenda/ADD-08-qbd-sdk-import.md`; part of the beta as phase P35.1; owner request 2026-11-18, scheduling 2026-11-19, Q-355–Q-367, Q-ADD8a–g; engine U-274–U-278). Scheduling: **P35.1 runs right after P35 and before P36** (owner 2026-11-19, Q-366); work packages I01–I08 run inside it; I04 lands in the engine repo and needs owner review before I05; PR-I1 after I08. A Windows extractor reads QuickBooks through its SDK into a `.clqb` package; the importer imports into the open company as a P31-style batch (after a verified BeforeImport backup; discard = batch undo) and ties out to QuickBooks' own reports. Reuses P31's tie-out and batch plumbing (P31 already ships the report-based import).

| Phase | Title | Tier | Status |
| --- | --- | --- | --- |
| I01 | SDK connection layer | M | |
| I02 | Extract lists and preferences | M | |
| I03 | Extract transactions, ledger lines and control totals | H | |
| I04 | Engine requests (U-274–U-278) | H | |
| I05 | Mapping and list import | M | |
| I06 | Transaction import | H | |
| I07 | Tie-out and accept | H | |
| I08 | Wizard, extractor packaging, help | M | |
| PR-I1 | Checkpoint: QuickBooks SDK import (process review) | — | |

**ADD-09 — Private development repo, public release repo** (`addenda/ADD-09-private-dev-public-release.md`, revision 2, owner 2026-11-21; Q-ADD9a–Q-ADD9m). Proposed scheduling (Q-ADD9a; default until the owner says otherwise): **R0 done 2026-11-22**; R1–R5 and R8 are built inside P35/P36 with ADD-04 (Q-ADD9b–e, j, l, m answered 2026-11-22 from the TB decisions: Inno Setup per-user plus zip replaces the MSI, local release script, Windows only, browser-open update check, support@kisaes.com); R6 (cutover) and R7 (source requests, collaborators) are owner steps after the beta build. The phase loop continues with P33.

R0 inventory (2026-11-22, `main` at d693a32f):

- **Remote and visibility:** `origin` = `https://github.com/KisaesDevLab/ClientLedger.git`, default branch `main`, **public** today. Engine submodule `KisaesDevLab/kisaes-gl-engine` (public, `.gitmodules:3`).
- **Tags and Releases:** none on `KisaesDevLab/ClientLedger` (`git tag` and `gh release list` empty).
- **Packaging today:** no `packaging/` folder, no installer script (MSI/WiX or Inno Setup); P35 not built. Only the unsigned portable preview archive from `cargo xtask preview` (`xtask/src/preview.rs`), uploaded as a workflow artifact by `.github/workflows/preview.yml` on every merge to `main`, never a GitHub Release. No `[profile.release]` in `Cargo.toml` (only `grid-bench`, `docread-bench`, `dev.package.cl-ui`).
- **Repo URL occurrences** (file:line):
  - `Cargo.toml:10` `repository = "https://github.com/KisaesDevLab/ClientLedger"` (ships in crate metadata) — R5
  - `xtask/preview/README-PREVIEW.txt:75` issues link `https://github.com/KisaesDevLab/ClientLedger/issues` (copied into every preview archive by `xtask/src/preview.rs:25`) — R5
  - `docs/legal/LICENSE-CLIENTLEDGER.draft.md:15` "publishes ClientLedger at https://github.com/KisaesDevLab/ClientLedger under the MIT License" (stale draft, superseded by `LICENSE.md`) — R5, Q-ADD9k
  - `crates/cl-ui/src/i18n.rs:718` and `:848` "report it on GitHub" (no URL) — R5 rewords to the public issues page
  - `crates/cl-ui/src/i18n.rs:1149` "releases are on GitHub" (update placeholder) — R4
  - `README.md:58` "Repository settings (owner applies on GitHub)" — developer docs, stays
  - `NOTICE.md:37` and `.gitmodules:3` — the public engine repo, not the dev repo; stays (Q-ADD9g)
  - `UPSTREAM.md` engine PR links (e.g. lines 525–565) — developer docs, engine repo; stays
  - `CLAUDE.md:10`, `QUESTIONS.md:131/132/146`, `phases/M6-ship-beta.md:120`, `addenda/ADD-04-winget.md:27/56` — ClientLedger.net and Release plans; developer docs
  - About box (`about.*` strings, `i18n.rs:694–706`): no URL. Help menu: no repo link. Installer metadata: none yet. `crates/cl-ui/src/screens/home/support.rs:17` is the Ko-fi link (not the repo).
- **Update check:** none. Preferences shows "Check for updates" with the placeholder "The check itself is not available yet" (`i18n.rs:1148–1149`); no update network code anywhere. P36 step 1 plans a GitHub Releases fetch with installer checksum check (Q-214–Q-216); ADD-09 R4 replaces it with a browser-open (Q-ADD9e).
- **Workflows:** `ci.yml` (PR/main checks; uploads CI artifacts only), `preview.yml` (preview archives as workflow artifacts), `nightly.yml`, `transfer-roundtrip.yml`, `close-prs.yml` (closes outside PRs). None publishes a GitHub Release. ADD-04 and P35 plan a tag-triggered release workflow (Q-ADD9c).
- **Bundled components readable after packaging (R2.7):** the engine Node SEA blob (`clientledger-engine.exe`; JavaScript recoverable, but the engine source is public anyway); PostgreSQL, PDFium and tesseract binaries and models (third-party, public); `clientledger-docread.exe` and `clientledger-cli.exe` (Rust, same hygiene as the main exe); embedded i18n strings, SQL inside the engine, report and invoice templates, fonts (IBM Plex, OFL).
- **Build-plan and agent files:** 36 tracked files (`CLAUDE.md`, `PHASES.md`, `STATE.md`, `QUESTIONS.md`, `UPSTREAM.md`, `phases/`, `addenda/`, `docs/AGENTS.md`, `.claude/`, plus `docs/history/`, `docs/design/`, `target/plans/` is ignored). All are in git history since P00. `KICKOFF_PROMPT.md` never existed. They drive the phase loop and stay tracked (Q-ADD9h).


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

## Owner-scheduled before beta

Set by the owner 2026-11-17. Do not drop these; they are not post-beta.

- U-233: convert the reports and routes that still refuse a restricted user (Q-P29ae).
- U-228: the "see costs" right (Q-P29af). One engine pass (with U-233) plus desktop work, to be done no later than the checkpoint before P39.

## Blocked items

Format: `Pxx — blocked by U-xx/Q-xx — what is pending — workaround in place`.

- P39 (release) — owner decision needed on Q-P03e — the bundled PostgreSQL archives ship LGPL libraries (gettext/libiconv on Windows) and OpenSSL, ICU, libxml2 and others on Linux/macOS; the owner must confirm the licensing position (license texts shipped, any written source offer) before beta release — workaround in place: libraries kept unmodified and listed in ATTRIBUTION.md.

- P32 MyBooks Online import page — blocked by U-285 (Vibe-MyBooks PR: an engine pin bump, then the import page; owner reviews and merges, Q-P32e) — Owner-pending, not built — in place: the engine API and contract ship in P32 and the round trip runs on the pinned engine in server mode (`cargo xtask transfer-roundtrip`, Q-P32d). On a stock Online server firm staff importing get 403 `TENANT_CREATION_NOT_ALLOWED` until an operator turns self-service company creation on (Q-P32x); a ClientLedger release needs Online's pin bumped first (Q-P32s).

- P05 AC4 — blocked by U-17 — real-engine 409 contract test is PENDING(record-version-v1) — stub contract covers the client (`If-Match` sent, 409 `STALE` mapped to `Conflict { Stale }`, fingerprint check sends nothing).

- P08 Delete Company (Q-185) — blocked by U-05 E5b (`tenant-delete-v1`) — `lifecycle::safe_delete_e5b` is PENDING — in place: the window explains it and keeps the button off; cl-company refuses without a before-delete backup path and SHA-256 (nothing sent) and removes the local record only after the engine's receipt.
- P08 Recovery key (Q-210) — blocked by U-23 (`recovery-key-v1`) — `lifecycle::recovery_key_u23` is PENDING — in place: the screen (show once, copy, save, confirm) runs on the key `/desktop/initialize` returns or a rotated one; without the capability it says so.
- P08 Full Database Backup (File > Utilities) — P34 built the cold copy and restore (`cl_host::cluster_ops`, tested on the real engine); the window needs the running app to hand its shared supervisor over and re-attach the engine link, session and LAN host — placeholder window; re-targeted to P37 (Q-P08l, Q-P34y).
- P11 Chart of Accounts merge — blocked by U-31 (engine merge moves no journal lines; `/accounts/:id/ledger` empty) — merge not built — in place: merge hidden (`merge_available: false`), QuickReport uses `/reports/account-report` (Q-P11a, Q-P11i).
- P11 parent-plus-sub-account balance — blocked by U-32 — PENDING — in place: each account's own engine balance only, with a note under the chart; `parentId` checked in `cl_model::lists::accounts::check` (Q-P11b).
- P11 sensitive accounts enforcement — blocked by U-33 (and P29) — PENDING — in place: the mark is stored locally in `CompanyPrefs.sensitive_accounts` and shown (Q-P11c). Done in P29 (U-33, engine `568b1f69`; the engine enforces it and the desktop hides the refused doors).
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

- P37/P08 (was P34; Q-P34y) — restore UI must call `Cluster::restore(src, store, allow_unregistered)` with `allow_unregistered = false` by default and set it to `true` only after an explicit owner confirmation that the file came from a trusted source (Q-P03h).

- P06 shell placeholders (Q-P06g): every menu, rail and palette command whose screen is built later opens a placeholder tab. The phase that builds the screen removes its entry: P11 Item List / Tags / Clients & Projects / Vendors, P12 Open Register, P13 Write Check, P14 Make Deposit / Transfer Funds / Sales Receipt / Record Card Charge, P18 Pay Bills, P20 Create Invoice, P21 Receive Payment, P22 Credit Memo or Refund / Statements, P25 Import Bank File, P26 Bank Feed, P29 Set Closing Date / Users and Permissions / Audit Trail, P30 Import Payroll, P32 Send to My Accountant (built in P32), P38 bundled help (F1 says "not available yet"). P29 Set Closing Date, Users and Permissions and Audit Trail are built in P29. Done in P29.
- P06 native macOS menu bar (Q-70, muda) is deferred (Q-P06b): in-window menu on every platform until macOS packaging.
- P16 Home lane actions open "arrives in Pxx" placeholders (Q-P16k): P18 Pay Bills, P20 Create Invoice / Sales Receipt, P21 Receive Payment, P26 Review Bank Feed, P29 Set Closing Date, P32 Send to My Accountant. Each phase wires its lane action.
- P16 Home "Bank file" balance column — U-77 / P25 (Q-P16d); "ready to accept" count — P26 (Q-P16f); "invoices more than 60 days past due" — done in P28 (Q-P16g, Q-P28r, Q-P28y); "Send to My Accountant · last sent" — P32 (Q-P16i).
- P16 Needs attention placeholder line for to-dos, reorder alerts and the weekly health check — U-37, U-78, U-12 (Q-P16e).

## Follow-ups from P03 security review

Findings from the P03 security review that were not fixed in P03. Fixed in P03: 1 (crafted backup runs code on restore), 2 and 15 (stop signalling an unrelated process), 3 (backup file mode on Unix), 5, 8, 12, 13, 14. Each item below names the phase that must pick it up; remove the line when it lands.

| # | Follow-up | Target |
|---|-----------|--------|
| 4 | Limit extraction size and check free disk space before unpacking a backup (a hostile archive can expand without bound). Company restores go through the engine's import, which caps sizes (P02 §2.5); the whole-cluster restore UI is P37 (Q-P34y); `cl-pg` restore already checks free space and caps the size (Q-P34m). | P37 |
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

## Follow-ups from P34

1. Full Database Backup window (review S5): re-targeted to P37 (Q-P34y). `cluster_ops::full_backup` / `full_restore` take the `Supervisor` by value, while the app shares it (`Arc`) between `HostLink`, `CompanyLink`, the status probe and the LAN host, and a restore restarts the engine on other users and sessions. P37 adds a swappable supervisor slot, then the window (Back Up Now, Restore, typed "I trust this file" only after `Unregistered`, LAN-disconnect warning, host app only) and `has_full_backup` in the crash window.
2. Review S6 / P33 follow-up 5 (QA-10): the QA practice company's good backup still fails the engine restore check (`ARCHIVE_VERIFY_FAILED`); P34 added only `verifyFailureSummary` logging. Reproduce with the job `details`, fix in the engine, and add a synthetic regression test.
3. Review M10: the crash window's "engine failed to start" trigger (`engine_failed`) is never set (Q-P34bn), and Copy Details has no scrubbed log excerpt (Q-119).
4. Crash marker on log-off (Q-P34w): Linux/macOS `SIGTERM`/`SIGHUP` now mark the session clean; a Windows log-off (no `WM_ENDSESSION` in winit) and a macOS log-out (`terminate:`) without a restart can still open the crash window.

## Follow-ups from PR-8

1. B2 has no live step: payroll post and spreadsheet commit on a LAN host are covered by unit tests and the route-set test only.
2. M1: the LAN slot is released only on `finish`, so an abandoned request could leak a slot. M2: `with_maintenance` does not take the company link, so a hold could start against the wrong company after a switch.
3. S6 follow-up: after a 401 a workstation closes the company instead of signing in again in place (a restore, which replaces the books, still closes it, which is correct).
4. The engine keeps `keepSessions` in memory: if the engine restarts mid-hold, the end of the hold signs the others out.
5. CI `transfer-roundtrip` (not required) flakes on a `@kis-books/shared` resolve error; re-run passes. Needs a look at its npm workspace setup.

## Follow-ups from P33

1. Prev on a new invoice loads nothing (QA-14; also on the host, not LAN-specific).
2. No "N people signed in" line on a paired computer's card: the host does not track sign-ins per computer (`DeviceRow.signed_in` is `None`). Needs a per-device session count from the engine (`/desktop/sessions`) or the gate.
3. A fresh workstation still starts the local postgres and engine before it pairs (start-up order untouched).
4. Live coverage is still missing for the invoice and contact conflicts, the payment re-check (Q-P33ao), the silent re-save, and certificate regeneration. Unit, demo and e2e tests cover them.
5. QA-10: restoring the QA practice company's backup fails the engine's restore check (`ARCHIVE_VERIFY_FAILED`, verify.ts:342), with sharing on or off, so it is not P33's. It needs an engine issue with the table list from the job's `details`; the owner adds the UPSTREAM row. The desktop now says it in plain words.
6. Live repaint check while a conflict read is pending (receive payment, tags, the lists and the register) is not done.
7. Import jobs: the maintenance hold covers only the request the engine gates, not the job's whole run (Q-P33ar; hold to job end later through the wizard's completion hook).
8. No file-level restore path calls `maintenance_reset` yet (`cl_pg::Cluster::restore` has no app caller); the supervisor resets at every engine start.
9. macOS headless keychain prompt check needs a Mac (CI or owner). Demo paired dates still read "Nov 3, 2026" / "Today".
10. Collapsed Home checklist line on a workstation shows the first-backup step without the reason text; the checklist branch has no test. Open Company on a workstation says twice that the books are on the host.
11. Conflict dialogs exist only on forms; list deletes and deactivations show the 409 as an error. Engine asks: return the company `updatedAt` from `GET /company/settings` (Q-P33as); `/vendor-layout-templates` and intake null-stamp creates are fixed in #98.
12. Final-lane cl-ui unit tests for the session 8 minor fixes (QA-14 contrast, QA-7 status line, Open Company, Home) could not be compiled locally (rustc out of memory); the full CI run covers them.

## Follow-ups from P30

1. The wizard's account pickers don't filter as you type.
2. The file step says "File chosen (2 KB)" instead of the file's name.
3. A tag can't be created in place on the Departments step.
4. Each voided entry shows as two VOID rows in the register.
5. History has no link to the posted entries.
6. The early duplicate warning on step 2 has no date: the engine's upload answer doesn't name the earlier import (U-254).
7. The "posted" time the engine reports is really the upload time (U-254).
8. Entry memos use ISO dates and say "QuickBooks Online Payroll" (engine text, U-255).
9. The engine pre-maps accounts by account number; the desktop now ignores that unless the name matches.
10. The engine's check-register post path still has unfixed races and `Math.round` (not used by the desktop, U-256).
11. No unique index on the posted file hash; an advisory lock is used instead (U-257).
12. OnPay files may show liabilities with the opposite sign (unverified layout, Q-P30b, Q-P30l).
13. Stored upload files can stay on disk if discard fails (review M7; needs a sweep, U-258).
14. P32 must offer discarding unposted payroll drafts before a transfer export (Q-P30c).
15. Tax impound withdrawals don't match a payroll entry (U-249 deferred, Q-P30e).
16. The Audit Trail evening-date retest from P29 is still owed.
17. Flaky engine test `peer-token.service.test.ts` failed once on #70 and passed on rerun (U-259).
18. The QA-15 fix changed cl-api for every keyed write: a write sent once and then an engine that is down now says "outcome unknown". Watch other screens for wording.
19. Engine #65 widened sensitive-account visibility for MyBooks Online bookkeepers with no permission row; owner to confirm (revert is one line, Q-P30m).
20. Not verified by hand: the "outcome not known" state when the engine stops while a payroll post is in flight (unit tests cover it; target/reviews/P30-qa3-notes.md).
21. QA part 3 minors: while the engine is down the wizard greys Post, Cancel and Back with no message; a duplicate draft that can't be posted still lists as "Ready to post" in History; the Audit Trail Source column says "payroll import" in lowercase; a second Post after an engine restart skipped the closing-password prompt once (cause not known).

## Follow-ups from P29

1. Per-form tag tests (Q-P29r) still missing for Sales receipt, Credit memo, Daily Sales, register entry and template Use; Deposit and Invoice have on/off tests (the default tag on new forms is not checked for them); Loan payment has no extra-line refusal test.
2. The Bank Feed rule accept trusts rows off the listed page to the rule's own tag (an Assigned off-page row would post its staged tag); check the engine's rule row selection. A Bank Feed accept into a closed period is not in the exceptions-report contract cases.
3. Bulk Entry: a row can set its own tag but cannot clear the section tag for itself alone; the Section tag picker is not in the mockup. Card charge and Personal funds fill the default tag when the lists arrive, never on a form already touched.
4. No `docs/user` pages for sign-in, Users and Permissions, Set Closing Date or the Audit Trail (P38).
5. The automatic sign-in switch is only unit-tested (no scenario, no live run).
6. Per-user right overrides can't be edited in the Users screen (roles only).
7. The temporary password text is still copied by egui each frame (its own layout copy); the clipboard is cleared after 30 s, and the clear overwrites whatever is on the clipboard by then.
8. MyBooks Online release notes needed: U-227 report areas (changes behaviour online), U-229 closing date Admin only, U-232 masked tax ID, download tokens of turned-off users stop at once (#64), run-due posting with no viewer.
9. Engine, still refusing for a restricted viewer (U-33, U-233): sales tax liability, 1099 summary and prep, inventory, cash flow, packs and the other reports named in U-33; the desktop hides these doors from `GET /sensitive-doors`. Also U-234 (bulk account edit can put a bank account under a sensitive one), U-235 (legacy 1099 summary returns the full tax ID), U-236 (temporary and invite passwords never expire), U-237 (`GET /company/users` readable by any user).
10. Engine gaps recorded in the fix pass: an automatic template posts as the system even if its owner lost the sensitive right after the account was marked; closing-date-exceptions does not detect a deleted transaction whose lines are gone; Budget performance and Budget vs. Actual compute with JS floats; the Journal totals show the visible lines of a mixed entry (accepted gap); scheduled packs and emails for non-Admin recipients are still refused; dashboard panels carry no `hiddenNotice`; MCP report tools use API-key scopes, not report areas; the 1099 Center compares with `Number()`; `vendor_1099_threshold_no_w9` keeps its own threshold (default 600).
11. Desktop sensitive accounts: the register tab close, the builder read-only reason and the UI unit tests were finished in the leftovers pass (commits 1ee38f37, 06c95d93, 64be69c9). Still open: no demo scenario shows the builder's hidden-reference reason; open report windows in the demo don't rerun when the demo role switches.
12. Smaller desktop gaps: Preferences > AI saves are not separately blocked (the panel is not drawn for a refused user); the intake panels read Admin once per load; Sales Tax setup agency/code New buttons, the Templates list and exemptions have no read-only gate beyond the command level; the Audit Trail viewer cannot be opened from a record (`for_record` has no caller); the audit User filter lists only people seen in rows read so far; a JSON number in an audit image prints as serde reads it (review M6).
13. Review items left: M4 (`tenant-settings.routes.ts` rewritten whole for a 6-line change; already merged), M7 ("Back Up Company" needs only `Read(CompanySettings)` and the backup includes sensitive accounts; no QUESTIONS note written).
14. `reports_exact::columns_drill_and_tag_split` ran over its 20 s budget once in two full contract runs under machine load; `cargo xtask qa-company` ran end to end twice (8 min 17 s, then 4 min 48 s) but the app was not launched on that folder by the implementer; `qa seed` is behind the `qa-seed` feature, so CI's plain `cargo test` no longer runs its tests.
15. Areas neither P29 review examined: from the combined review: cl-api retries of non-idempotent POSTs (checked afterwards, correct), idle lock internals, Users and Closing Date screen logic, engine Report Builder / `.clreport` / AI draft / Ask sensitive handling, search, print, item and price-level lists for hidden accounts, `ledger.service.ts` choke points, dashboard service, sweep test coverage, migrations 0278-0280 rollbacks, demo parity, qa-company, i18n coverage, AccessKit labels, 409 handling in the new screens.
16. Areas the security review did not examine: Report Builder rights and runs, redaction in tenant export and AI prompts, `my_sign_in.rs`, what sign-out leaves in memory, the log grep, audit PDF contents, 1099 thresholds, `xtask/src/qa_company.rs` in full.
17. Q-P29a to Q-P29aa are defaults awaiting the owner, except Q-P29a, Q-P29c (changed), Q-P29k, Q-P29o, Q-P29t, Q-P29x and Q-P29y, answered 2026-11-17; Q-P29ad to Q-P29ag are owner answers and Q-P29ac a default. Hands-on QA results for P29 are pending.
18. Unfixed hands-on QA findings, all pre-existing (target/reviews/P29-qa.md): QA-10 Enter Bill Prev on a new bill says "This is the oldest one." although bills exist; QA-12 payee autofill on Write Check adds an empty line that blocks Save until deleted; QA-19 the Audit Trail list is a short fixed-height box (about 11 of 100 rows show); QA-20 (fixed 2026-11-17 in `fix/P29-owner-answers`) Merge pre-selects an unrelated customer ("Into customer" is the first one alphabetically) and Merge is one click that can't be undone, with no confirm step; QA-21 a memorized report opens under the generic tab title, so two memorized P&Ls look the same in the tab strip.
19. QA-14: signing in with a temporary password and being forced to change it: hand check done 2026-11-17, passed.
20. QA-9: the Closing Date Exceptions column fix: hand check done 2026-11-17, passed.
21. The status bar shows the built-in role name only; a custom role's name needs an engine field (Q-P29ab).
22. The Audit Trail report's timestamp text shape was read from engine source, not captured live (commit e24c2ec6).
23. Git stash "pre-A10: stale Q-PR5d register test edit" still exists.
24. The Audit Trail evening-date fix (QA-8) still needs a retest in the evening.
25. Long user names ending in "…" in Closing Date Exceptions are not tested (the test user's name is short).
26. MyBooks Online release note needed for U-238: once an account is marked sensitive, a bookkeeper with no permission row starts seeing it.
27. The Audit Trail shows engine wording for failed sign-ins (Action "login", Record "user login failed").
28. The QA company's `credentials.txt` is out of date after the hand checks; rebuild with `cargo xtask qa-company --fresh`.
29. The hidden-before-update state of an existing company's Bookkeeper was not observed by hand (unit tests cover the update rule).
30. Replace chrono (Q-P29ah).
31. The Audit Trail password-change item is fixed (engine #66, U-240); the rows still filed under the workspace are U-241.

## Follow-ups from PR-A2

1. P29 must cover 17 surfaces for sensitive accounts in the builder and memorized reports (run facts, earnings rows, drills, captions, saved definitions, `.clreport`, import preview, AI draft, pickers and more): list in `target/reviews/PR-A2.md` Process 3.
2. Account merge does not re-point memorized reports (it moves no lines, U-31), and items have no merge; only contacts and tags are re-pointed.
3. Review Minors left open: 6-M2 (the output word check is an English list), 6-M3 (a fullwidth-digit books name is refused by the desktop), 6-M4 (any active name of the company passes the heading check), 5-M1 (no engine body cap on `.clreport`, U-221), 5-M2 (the Replace safety copy has no ids), 4-M1 (deleting a group changes others' reports), and the engine re-check minors M-2 to M-8.
4. Re-check minor M-a: a Trial Balance total with a zero side shows 0.00 while its drill subtotal is empty.
5. Re-check minor M-b: a Trial Balance line drill reads the whole section on every page.
6. The demo's on-screen expanded balance line still lists earlier transactions without the opening row, and the demo has no Trial Balance.
7. The opening row in an export is tested in cl-print only, not through a drill tab.
8. Hands-on QA round 1 (`target/reviews/PR-A2-qa.md`): a balance in a date range passed with figures, Trial Balance partly (account then Tag not tested); QA-1 minor: a single-month drill keeps the three month headers.
9. QA round 2 spent its budget on setup: merge, report-file import, Trial Balance by tag and the cash-basis starters were not tried by hand on the real engine, and the "uses a name that was deleted" message was never seen there (live contract tests, engine tests and the demo scenario `report_missing_ref` cover them). Rerun these by hand in P29 QA. Found: the engine refuses to delete a tag that is in use ("make it inactive instead"), so the deleted-name message arises from deleting a tag group, an unused tag or a budget, and from records merged earlier. The merge warning now says memorized reports switch to the name kept. Owner 2026-11-16 (Q-PRA2i): P29 builds a scripted seeded QA company and reruns these flows on it.
10. All review runs were accrual only, except the Trial Balance re-check, which ran both bases.
11. U-226's standard-report `missing` was confirmed from engine source, not a live test.
12. Q-PRA2a-Q-PRA2g were confirmed by the owner on 2026-11-16, together with Q-A10l, Q-A10o and Q-A10p.

## Follow-ups from A10

1. ~~The desktop's number-word check is looser than the engine's.~~ Done on `fix/A10-qa`: the desktop uses the engine's list; the only difference left is the engine's NFKC folding.
2. The engine fix pass for the Blocking review finding was not re-reviewed; PR-A2 re-checks "AI never produces numbers".
3. ~~A 503 after names were sent writes no audit line.~~ Done (U-224, Q-A10k): outcome `failed`.
4. The toolbar label is "Describe…", not the approved "Describe the report…" (Q-A10c); the full phrase is in the tooltip and the screen-reader label; not measured with the full label.
5. In the demo the describe panel says "prepared drafts" while the status bar says Ollama.
6. Redrafting an existing report is not built (Q-A10g).
7. UI review minors left: M1 (AI wording in the strings table), M4 (two "Keep" buttons), M5 (focus after picking a name; Go to row does not scroll), M6 (demo draft lists vendors), M7 (scrolling at 200%), M8 (banner token choice vs DESIGN.md), M9 (Add menu open after Remove).
8. Code review minors left: Minor 6 (Roman numerals pass the engine's digit check, allowed on purpose). Minor 4 (demo notes) was fixed on `fix/A10-qa`; demo row ids still differ from the engine's.
9. Not checked by hand: a real AI provider (cloud and Ollama) end to end, a screen reader, macOS and Linux, dark mode at 200%, several unmatched names at once, the "not found" name state.
10. Q-A10a print: after the fix the 423-page demo print test still takes about 20 s (19.7 s on a loaded machine).
11. Hands-on QA (rounds 1 and 2, `target/reviews/A10-qa.md`): round 2 used a stand-in local model on the real engine, no real AI provider. Not reached on the real engine: Discard, draft twice, Ctrl+Z after Keep, closing a tab under review, a late reply after switching report; the Audit Trail lines; keyboard-only use; AI off after on; reopening a memorized draft; Ask a question with the provider down. Unit, frame and demo tests cover them.
12. QA2-1: "401(k) Match by month" gets the "numbers weren't used" note although the digits belong to the account name. QA2-2: a digit name that is not in the books ("Studio 54") is refused by the engine with the general message instead of a name to pick or remove; the in-books case was not tried by hand. QA2-3: a not-found row doesn't say which name the AI wrote; one filter chip reads "Remove Pick a name".
13. A name that is only a number ("3M") can't be drafted: the description's numbers are hidden from the AI (U-224 note).
14. The builder's run and run-detail calls still show the "could not tell whether that was saved" message when a report times out (drafts and Ask a question are fixed). With the local engine itself down, the describe panel says to check Ollama.
15. `reports_p28::ask_off_then_ollama` fails when run alone under the filter `ask` (an earlier test leaves AI on); it passes in the full contract run.
16. QA-5 (not A10): the sidebar's Audit Trail and `company.audit_trail` open the P29 placeholder; the report opens from Reports. P29 wires them. Done in P29.
17. U-224 (engine PR #57) was not reviewed fresh-eyes; PR-A2 covers it. Q-A10o and Q-A10p were added by the fix pass as defaults; the owner confirmed them on 2026-11-16.

## Follow-ups from A09

1. Q-A09x: the goldens use `present` and `to_csv`, not the app's table code.
2. No scenario steps for QA-1, QA-2, QA-3 and QA-5 (unit and frame tests only).
3. The large-export path (rows not loaded, the 100k cap, Cancel mid-fill) was not tested by hand: no big line in the practice data.
4. The edit-conflict refusal and an admin renaming or replacing another user's report were not tested by hand.
5. "% of" under U-218 (normal sign) was not checked by hand.
6. QA-7: an empty group is left if a single import is closed without memorizing (Q-A09aa).
7. Q-A09u type-to-find in the lists goes to P33.
8. The engine contract tests use 4-decimal amount filter values; the desktop now refuses more than 2 decimals.
9. S-L9 surfaces (`.clreport` references, import candidates and status, pickers) are in the P29 N1 list (Follow-ups from PR-6 item 2) and U-33. Engine follow-ups U-221-U-223 are Open (later).

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
2. N1 (sensitive accounts, Q-157) goes to P29: nothing is hidden today, so P29 must cover every surface P28 added, preferably engine-side (U-33) — statement rows and section totals, GL / Journal / Transaction List drills, tag split rows, BvA rows, column layout, exports / print / email, memorized definitions naming a hidden account, and Ask (`promptNames` sends up to 500 active account names to the model). A09 adds three (S-L9): `.clreport` exports whose references name a hidden account (name and number), the import preview's candidates and resolved status (they reveal that an account exists), and the review's full-list pickers (every account); engine side in U-33 (exclude hidden accounts from export references, candidates and lists). List in `target/reviews/PR-6.md` N1.
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
7. View-only users can save memorized reports (the engine follows the report-packs precedent); P29 permissions decide. Kept in P29 (Q-P29u).
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
5. Q-PR4b: cost visibility permission, in P29. Not done in P29: U-228 (see costs) was cut; inventory value reports follow Items.
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

### M4 — Reports and controls (P28–P30, checkpoints PR-6 and PR-A2), completed 2026-11-17

- **Shipped:** P28, the report window with presets, basis, columns, filters, drill-down, memorize, CSV, Excel and PDF export, exact statements and ledgers, agings, sales and spending, banking, 1099 Summary by box, Closing Date Exceptions, Duplicate Finder, budgets and Budget vs. Actual, report email and ask-a-question. P29, sign-in and recovery, Users and Roles, report areas, closing date (Admin only), audit trail with export, sensitive accounts, 1099 thresholds, masked tax ID and tag options. P29 follow-up, the owner's answers (the built-in Bookkeeper sees sensitive accounts) and hand-check fixes (temporary password Show/Hide, password changes and failed sign-ins in the Audit Trail). P30, the Import Payroll wizard (Gusto, ADP, Paychex, QuickBooks Payroll, Square, OnPay, any CSV) with a History tab to reverse an import; unbalanced files are refused and employee detail is thrown away after posting.
- **Screenshots:** `target/ui-check/<screen>/` (light and dark); QA shots in `target/ui-check/P29b/` (P29 follow-up); P30 QA notes in `target/reviews/P30-qa.md` and `target/reviews/P30-qa3-notes.md`; plus the CI `ui-check` artifacts on each PR.
- **Questions awaiting the owner:** Q-P29ac (when Bookkeeper role updates), Q-P29ah (ignore chrono advisory, replace later), Q-P30a (Rippling uses generic CSV), Q-P30b (provider layouts unverified), Q-P30c (worker names in engine rows), Q-P30d (unbalanced pay date refused), Q-P30e (payroll tax withdrawal deferred), Q-P30f (net pay bank or clearing), Q-P30g (who may import payroll), Q-P30h (no backup before posting; flagged), Q-P30i (pay period for several dates), Q-P30j (duplicate refusals become 409), Q-P30k (default accounts in wizard), Q-P30l (OnPay layout unverified), Q-P30m (MyBooks Online bookkeepers, sensitive accounts).
- **Upstream (open or deferred):** U-239 (reset-email audit entry), U-241 (audit rows filed under workspace), U-249 (tax-impound withdrawal, Deferred), U-254 to U-259 (payroll duplicate answer and posted time, memo wording, check-register path, unique index, upload sweep, flaky peer-token test), all Open (later). Owner-scheduled before beta: U-233 (restricted-viewer reports) and U-228 (see costs), to be built together.
- **Reviews:** combined opus review per phase; hands-on QA against the real engine (P28 15 findings, P29 22 findings, P30 three passes).

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

- 2026-11-22 — PR-8 LAN multi-user checkpoint (process review of P33) fixes on `fix/PR-8`; PR with auto-merge. Tier H checkpoint.
  - Review: process-reviewer (opus, xhigh; `target/reviews/PR-8.md`, local): 2 Blocking (B1, B2), 7 Should fix (S1-S7), 3 Minor (M1-M3).
  - Fixed: B1 an edited form after a lost reply could post the same transaction twice (shared `HeldKey` helper `forms/held_key.rs`: the key stays pinned through every failure and after a 422, dropped on success or reset); B2 payroll post and spreadsheet commit/undo now run under the maintenance hold on a sharing host (test `every_maintenance_route_is_called_under_a_hold` checks the call sites against the engine's route list); S1 duplicate-bill check sees saves from other computers; S2 every change-feed kind has a row and refreshes its windows (`FEED_KINDS`); S3 Preferences sends only changed fields and the closing date is refused when it changed elsewhere; S4 no save goes out after a pre-save read that waited for the host; S5 the daily-sales retry no longer takes another computer's draft (`created_by`); S6 routine holds keep workstations signed in (engine #101, U-295), a restore still signs everyone out; S7 the hold is taken whenever the engine runs in LAN mode, even if sharing failed to start; M3 the feed's first page reloads the windows once.
  - Engine (KisaesDevLab/kisaes-gl-engine, merged; pin e51d7615): #101 (U-295, `maintenance-sessions-v1`, no migration).
  - Checks: live `office_network_live` passed (94 s, S6 routine-hold step ran, nothing pending); cl-api lib 852 and contract 430 on the local engine build; cl-ui targeted suites (813 + 154); clippy clean on cl-api, cl-ui, cl-host, cl-lan; full CI on the PR (Q-368).
  - Not done: see Follow-ups from PR-8.
  - Decisions (defaults followed, no Q&A until P36): the held key stays pinned after a 422; payroll post is not on the HeldKey helper (the engine refuses a second post of a session); S5 matches drafts by `created_by` only; `permission_templates` maps to the company-settings notice; the QuickBooks/other-system import hold is routine (signs nobody out); the engine keeps `keepSessions` in memory, not in a migration.
  - Agents and models: process-reviewer opus (xhigh); rust-implementer opus in two packages (WP1: B1, S3, S5; WP2: B2, S1, S2, S4, S6, S7, M3), engine change in WP2; orchestrator merged and pinned. Q-368 tally: 1 review, 2 implementers, full CI once (PR CI).
- 2026-11-22 — P33 LAN multi-user complete on `phase/P33-lan`; PR with auto-merge. Tier H. P34 still follows in M5, so no milestone summary yet. PR-8 (the LAN multi-user checkpoint, "after P33" in PHASES.md) runs next, before P34.
  - Shipped: Office Network (Company menu, and from Open Company with no company open): one computer hosts, the others use its books. A pinned TLS 1.3 proxy (`cl-lan`) on one private interface only, pairing by code (fingerprint prefix plus secret) with an Allow click, per-device sign-ins, device list with Remove and Make a new certificate, host mode that starts at sign-in (Task Scheduler, LaunchAgent, systemd) and a headless `--lan-serve` host. Workstations run no local database or engine; host-only commands are disabled with "Do this on the host computer.", AI, document reading and email settings are read-only. Record stamps on every edit and delete (`expectedUpdatedAt`), a conflict dialog (Keep mine, Use theirs, Back) on every form and list, a silent re-save when nothing editable changed, a host-down overlay that never sends a save on its own, a maintenance hold around restore, import and payroll post, a live refresh from the change feed, plain-words status bar, owner guide `docs/user/office-network.md`.
  - Acceptance: [x] Two clients editing the same record: the second gets a clear conflict and no lost update (engine stale-check suites and race tests; live `office_network_live` and `conflict::stale_all_u291`; the conflict dialog tests on every form; scenario `conflict_dialog`, 77 steps, light and dark; the soak's zero-lost check); [x] Host restart reconnects clients (live `office_network_live`: engine kill, proxy restart and host restart with a new sign-in reconnect with data intact, the host generation moves; scenario `host_down`); [x] A 30-minute soak with 5 clients shows no lost updates (`cargo xtask lan-soak --minutes 30`: 1802 s, 43652 saves, 14697 reads, 142 resolved conflicts, 0 lost, 0 deadlock replays, 0 failures; save round trip p50 39 ms, p95 75 ms, p99 107 ms, max 292 ms; trial balance balances and the A/R and A/P agings equal their control accounts).
  - Engine (KisaesDevLab/kisaes-gl-engine, merged; pin ffb2d750): #88 (U-288, `route-classes-v1`), #89 (U-289, `lan-profile-v1`), #90 (U-290, `lan-sessions-v1`, migration 0285), #91 (U-291, `stale-all-v1`, migrations 0286, 0287), #92 (U-292, `stale-required-v1`), #93 (U-293, `maintenance-v1`, migration 0288), #94 (U-294, `changes-v1`, migration 0289), #95 (the lost update after a deadlock retry, lock order), #96 (import undo ignores change-feed rows), #97 (review fixes), #98 (U-292b: creates need no stamp; daily-sales rows carry `updatedAt`) and #99 (posting lock order, the company-row lock mode, `/health` on its own pool). Capabilities added to `CL_API_REQUIRE_CAPS`.
  - Decisions (QUESTIONS.md, Q-P33a to Q-P33at; defaults in force except the two Open — owner rows): Q-P33ad (one request per connection), Q-P33ae (proxy forwards `/api/` only), Q-P33af (certificate file beside `lan.json`, key in the keychain), Q-P33ag (pairing spends the code before the Allow click), Q-P33ah (engine commit check only when the build knows it), Q-P33ai (default-deny route classes are the engine's), Q-P33aj (no mockup; silent re-save only when nothing editable changed on their side), Q-P33ak (stamps in the engine's ms-UTC shape only), Q-P33al (`LAN_LINK_LOST` never resent), Q-P33am (host-only commands), Q-P33an (live refresh by poll, then the feed), Q-P33ao (payment applications never merge), Q-P33ap (soak clients are one owner on five devices), Q-P33aq (soak replays with the same key), Q-P33ar (import hold covers the gated request only), Q-P33as (company-row writes read `GET /company` for the stamp), Q-P33at (a fresh read before a write where the screen holds no stamp); earlier plan decisions Q-P33l, Q-P33q, Q-P33x, Q-P33y.
  - Reviews and QA (Q-368 tally): the combined review (opus, security in scope) found B1 (a maintenance hold could outlive its holder), S1-S9 and M1-M9, all fixed (desktop and engine #97). Hands-on QA on the real engine (`target/reviews/P33-qa.md`, three layers beyond it: demo scenarios, the live suite and the soak, found none of these): 3 Blocking, 6 Should fix, 5 Minor. Blocking found by QA: QA-1 (a new second computer could not reach the workstation setup), QA-6 (a save pressed while the host is down was sent on its own when it came back; the cause was in cl-api, found only with the real host) and QA-10 (restore on the host fails with `ARCHIVE_VERIFY_FAILED`; reproduced with sharing off, so downgraded to an engine finding, not P33's). QA-1 and QA-6 fixed and rechecked live; QA-2, 3, 8, 9, 11, 12, 13, 4, 5, 7 and 14 (except the signed-in count) fixed. None of the three Blocking items was found by unit tests, the demo scenarios, the live suite or the review.
  - Checks: engine contract 430/430 and offline 6/6 on ffb2d750; live `office_network_live` passes (91 s) with the host-restore, held-save, stamped-removal and removal-ends-sign-ins steps; soak 180 s and 30 min pass (above); scenarios `lan_setup` (107 steps), `conflict_dialog` (77), `host_down` (59) and `maintenance` (24) pass light and dark, screenshots in `target/ui-check/<scenario>/`; cl-lan 42 unit + 13 e2e; full local `cargo xtask ci` was green on 6935b13a, and the later QA and minor-fix commits are covered by the final run and GitHub CI (the session 8 cl-ui tests could not be compiled locally for memory).
  - Not done: see Follow-ups from P33.
  - Open questions (defaults in force; no Q&A until P36): Open — owner: Q-P33f (a system service that runs with nobody signed in, a new Tier H addendum scheduled with P35) and Q-P33x (the CLAUDE.md wording for the cl-lan layering rule); the other Q-P33 rows are defaults in force; also Q-P32d through Q-P32z, Q-P31af and the P31 and P30 open rows already listed in earlier entries. Owner-pending: U-285 (the MyBooks Online import page); an engine issue for QA-10.
  - Agents and models: architect opus; engine-scout opus; engine-implementer opus (U-288 to U-294, the race fix, the review fixes, the lock-order fix); rust-implementer opus in two lanes (A: cl-lan, host, api, shell; B: the forms conflicts) with fresh opus agents for the review fixes, the QA fixes and the minor QA fixes; reviewer opus (combined, security in scope); ui-verifier opus (hands-on QA); final pass: rust-implementer opus, bookkeeper sonnet, orchestrator opus. No Fable subagents.
- 2026-11-22 — ADD-09 registered and R0 (inventory, read-only) done on `addenda/ADD-09-register`. No code, packaging, repo or visibility change. Orchestrator only (inventory by hand, advisor consulted; no subagents). Recorded under ADD-09 in Active addenda; nine conflicts with ADD-04, P35/P36 and CLAUDE.md logged as Q-ADD9a–Q-ADD9m with defaults. R1–R5 and R8 are scheduled with P35/P36; R6 and R7 are owner steps.
- 2026-11-19 — P32 Transfer file for MyBooks Online, and IIF export complete on `phase/P32-transfer-iif`; PR with auto-merge. Tier H. P33 and P34 still follow in M5, so no milestone summary yet.
  - Shipped: Send to My Accountant (File and Company menus, Tools list): a transfer file (`.clxfer`, tenant archive format 2) with its own passphrase (at least 12 characters, never stored, never the backup passphrase, Make one for me hidden by default with Show and Copy), check totals from a live preview and again from the saved file, What's included (names, hidden accounts and waiting documents in plain words), a backup first, a payroll-draft prompt, missing attachment files listed in one run, a result panel, Sent before, and a Refresh. Export IIF File (all transactions as general journal entries, a stop list for what can't be written, limitations shown after Save). Close Company, Sign Out, Restore, Delete and the other archive doors are refused while a transfer runs; Restore refuses a `.clxfer` in plain words. Engine side: archive format 2, verification totals `vt1` (strict on the same definition, backfill-only differences warn), transfer mode (users as names only, omissions and scrubs listed in the manifest), `TENANT_BUSY`, `transferOnly`.
  - Acceptance: [x] A transfer file imported by the pinned engine in server mode matches all verification totals (`cargo xtask transfer-roundtrip` passes locally; CI workflow `transfer-roundtrip.yml`; engine `transfer-external` imports the desktop-made file as a firm-staff user, `matches === true`); [x] No secrets in the file (engine test T5 canaries across every decrypted part; the `transfer-external` destination canary scan of every table and the upload folder, #87; the desktop scan of the plain parts); [x] The screen matches the mockup (ui-verifier `--compare` in light and dark, scenarios `send_to_accountant` and `iif_export`; deviations in Q-P32q plus the review additions: the net income date label, the file name pattern, no status-bar note, one column below 1100 pt).
  - Engine (KisaesDevLab/kisaes-gl-engine, merged; pin 401eaeb3): #82 (U-282, 4efbfe12, `tenant-archive-v2`), #83 (U-283, 661c65f7, `archive-verification-totals-v1`), #84 (U-284, dd11f2b4, `tenant-archive-transfer-v1`, migration 0283), #85 (b1065257, CI round trip and more name readers), #86 (57bb7615, migration 0284, schema version 285, review fixes S1-S3, M6, M7) and #87 (401eaeb3, canary scan). Capabilities added to `CL_API_REQUIRE_CAPS`. U-285 (the Online import page) is Owner-pending, not built; U-286 (transaction-list paging) Open, optional; U-287 (engine guard for a negative inventory bill line with a quantity, Q-P31af) Open.
  - Reviews: plan review (process-reviewer opus xhigh, target/reviews/P32-plan.md) 2 Blocking, all fixed in the plan; UI review (target/reviews/P32-ui.md) 2 Blocking (one-column layout below 1100 pt; the made passphrase shown by default), fixed; combined review (opus, security in scope, target/reviews/P32.md) 2 Blocking, 8 Should fix, 8 Minor, Blocking and Should fix fixed in the desktop and engine #86; real-engine QA (target/reviews/P32-qa.md) 13 findings, 1 Blocking (QA-1, a missing attachment stopped Send with a misleading message) fixed, QA-2, QA-4 and QA-14 fixed, minors QA-6, 9, 10, 11 and 12 fixed.
  - Owner decisions: Q-P32a (users travel as names only), Q-P32b (hidden accounts and their grants travel, and the screen says so), Q-P32c (IIF stops and lists; no plug line); confirmed 2026-11-19: Q-P32o (strict totals, warn only on upgrade steps) and Q-P32s (Online bumps its engine pin before a ClientLedger release; release notes state the minimum Online version). Owner rule 2026-11-19: no Q&A until P36 is complete.
  - Open questions (defaults in force, QUESTIONS.md): Q-P32d (round trip on the pinned engine in server mode, not an appliance image), Q-P32e (U-285 as Vibe-MyBooks PRs), Q-P32f (a separate passphrase per file, zeroized buffers), Q-P32g (format 2), Q-P32h, Q-P32i, Q-P32j (IIF details), Q-P32k (missing files in one run), Q-P32l (preview then file totals), Q-P32m (no paging), Q-P32n (`.clxfer` in Restore), Q-P32p (review shape), Q-P32q (mockup deviations), Q-P32r (second transfer, same-source warning), Q-P32t (what a transfer omits or scrubs), Q-P32u (`TENANT_BUSY`), Q-P32v (IIF rules unconfirmed: the owner should test-import `fixtures/iif/sample.iif` into QuickBooks Desktop), Q-P32w (generated passphrase hidden by default), Q-P32x (Online's self-service company creation is off by default, so firm staff importing get 403 `TENANT_CREATION_NOT_ALLOWED`; the U-285 page explains it), Q-P32y (payroll drafts are discarded before the export; desktop message now, the engine may omit them later), Q-P32z (Close and Sign Out are refused during a transfer); Q-P31af (a negative item line with a quantity is refused in the desktop; an engine guard is a candidate, U-287) still open.
  - Checks: local full `cargo xtask ci` green at fdc8a6f5 (the later QA fix commits through de929150 ran the changed crates' tests; GitHub CI covers the rest); `cargo xtask transfer-roundtrip` passes locally; scenarios `send_to_accountant` (140 steps) and `iif_export` (38 steps) pass light and dark; screenshots `target/ui-check/send_to_accountant/`, `target/ui-check/iif_export/` and `target/ui-check/P32-qa/`.
  - Not done from QA: QA-3 and QA-5 (not verified, harness doubts), QA-7 and QA-8 (IIF content; part of Q-P32v's owner test), QA-13 (Restore screen layout, not P32), and a ui-check scenario for the Back Up Company missing-files panel (unit test only; needs a demo dev command). Also not done: QA-6 naming the fallback account, QA-15 naming the payroll import. About 20 engine name readers still join `users` without `userDisplayNameSql` (names-only people show no name there).
  - Agents and models: architect opus; process-reviewer opus xhigh (plan review); engine-scout opus; engine-implementer opus (E1-E3 and E5) plus a fresh opus for E6; rust-implementer opus (desktop) plus fresh opus agents for the review fixes and the QA fixes; reviewer opus (combined, security in scope); ui-verifier opus (UI and QA); bookkeeper sonnet. No Fable subagents.
