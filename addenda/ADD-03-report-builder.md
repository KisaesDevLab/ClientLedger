# ADD-03 — Report Builder and Memorized Reports

**Status:** Active (added 2026-10-30, during the build) · **Part of the beta** · Phases **A07–A10** · Checkpoint **PR-A2** · Decisions Q-305–Q-315 · Engine U-118–U-121

Owner request, 2026-10-30: "a report builder… simple, similar to the financial builder in MyBooks… either summary, detailed or a combination of both", working with transactions — sales, A/R, A/P and the rest. The owner answered a Q&A the same day; the answers are Q-305–Q-315.

This addendum adds work to a plan that is already being built. It never rewrites completed phases; where it changes planned or shipped behavior (P28 memorized reports, the Reports mockup's Customize button, the M5 transfer file), it says so below.

Models: opus for all work (Q-277); process-reviewer at effort xhigh for PR-A2.

## What it is

A report is **one tree** of rows (Q-305), like MyBooks' Financial Statements builder (`engine/packages/shared/src/financial-statements/*`) but over transactions:

- **Section**: a source (Q-306), filters, up to **3** nested group-bys (account, account type, name, item, transaction type, project, tag — one per row, Q-233 — month/quarter/year, due status, aging bucket) and a display:
  - **summary**: group lines with subtotals and a total;
  - **detail**: every transaction;
  - **expandable**: summary lines that open to show their transactions; print and export keep what is open.
- **Group**: a heading over sections, with a total.
- **Formula**: adds and subtracts earlier rows' totals (e.g. Gross profit = Sales − Cost of goods sold). Only earlier rows, so no loops.
- **Text**, **blank** and **page break** rows for layout.

Columns: pick and order detail fields; periods side by side (month, quarter, year); compare (prior period or year, $ and % change, % of total, Q-104); rename any header or caption.

The engine computes every figure — group totals, subtotals, formulas, percentages, running balances — in exact decimal arithmetic. ClientLedger never adds money up (non-negotiable 7).

## When to run it (scheduling rule)

1. A07–A10 need P28 (report window, filters, export, memorize, drill-down). Run them **right after P28 and its PR-6 checkpoint merge, before P29**.
2. Before A08, draft the mockup `ReportBuilder.html` (outline left, properties panel, live preview) for the owner to approve into `docs/design/` (Q-305). A08 builds to the approved mockup.
3. If U-14 (exact money) has not merged by then, the engine PRs still go ahead and the desktop side builds against the documented contract with its contract tests PENDING (Q-220).
4. PR-A2 runs after A10.

Mark A07–A10 in STATE.md like any other phase. Branches `phase/A07-…`; PRs `A07 — …`; final commit `docs(state): complete A07, next <phase>`.

## What changes for planned and shipped work

| Where | Was | Becomes | Migration |
| --- | --- | --- | --- |
| P28 step 4 (memorized reports) | Stored by P28 (not yet decided where) | Stored in the engine (`report_definitions`, `kind: 'standard'` or `'builder'`) so they travel in backups and the transfer file (Q-312) | One-time move of any memorized reports P28 kept locally |
| Reports mockup › **Customize** | No design | Opens the Report Builder on the current report | None |
| P28 A/R and A/P aging | Today's balance due, filtered by invoice date | True as-of open balance: payments and credits dated on or before the as-of date (Q-307, U-121), same as the builder | None; a past as-of date now shows what was really owed then |
| M5 transfer file, step 1 ("memorized reports") | Unspecified store | The engine tables, carried by the tenant archive with ids remapped | None |

---

## A07 — Engine, model, run in the report window

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | L | P28, U-14 | engine-implementer (opus), rust-implementer (opus) | reviewer (opus), security-reviewer (opus) |

**Decisions:** Q-305–Q-314, Q-44, Q-104, Q-105, Q-157, Q-207. **Engine:** U-118, U-119, U-121 on `clientledger/report-builder` (separate PRs where the size calls for it).

**Crates and files:** engine `packages/shared/src/report-builder/{schemas.ts,fields.ts,starters.ts,fixtures/*.json}`, `packages/api/src/services/report-builder/*`, `packages/api/src/routes/report-builder.routes.ts`, `packages/api/src/db/schema/report-definitions.ts` (migration `0242_report_definitions` or the next free number); `crates/cl-model/src/report_def/{schema.rs,validate.rs,outline.rs,fields.rs}`; `crates/cl-api/src/resources/report_builder.rs`, `crates/cl-api/tests/report_builder_contract.rs`; `crates/cl-ui/src/screens/reports/{window.rs,drill.rs}`.

**Steps**
0. Capability check (engine-scout): transaction types per source; EXPLAIN on a synthetic company with 1 million lines (indexes in the same migration if needed); how the standard agings compute today.
1. **Definition schema v1** (zod in engine shared; serde mirror in cl-model, refusing a newer `schemaVersion` with "update ClientLedger"). Both suites parse the same fixtures. Field catalogue `fields.ts`: per source, its measures, filters, group-bys and detail fields; dimensions that don't exist yet are offered greyed with a reason (projects until U-35, inventory until P24). **One tag per row** (Q-233): every transaction row carries the single tag of its line, the Tag detail field shows that one tag, and grouping or filtering by tag uses it; there is no multi-tag mode (U-45 is withdrawn). Caps: 200 nodes, 24 detail fields, 36 period columns, 2 compare sets, 20 filters, 500 ids per list, 256 KB.
2. **U-118 saved definitions:** `report_definitions` and `report_definition_groups`, scoped to tenant and company, `created_by`, `version` (409 when stale), soft delete, unique name per company; CRUD with author / owner / super-admin edit rule (Q-310); audit rows; tenant portability classifies `definition_json` as `'contains-ids'` (like `report_pack_items.options_json`).
3. **U-119 run engine:** `POST /report-builder/run` → columns and rows (`kind`, `level`, `caption`, `cells` as exact decimal strings, `fields`, `drill` ids, `expandable`/`expanded`, `cursor`); `POST /run/detail` (keyset-paged detail for one group path; also drill-down from a subtotal); `GET /fields`; `GET /starters`. One fact query per source family reusing `cashBasisLinesWith`, `adjustingCond`, `companyFilter`, `tagIn`; grouping with SQL `GROUPING SETS` and `SUM(numeric)`; running balance by window function plus opening balance; periods and compare via `getRangePeriods`; formulas and percentages with `decimal.js`; section rights by source area; sensitive-account facts removed before grouping once U-33 lands (Q-309); limits (Q-311). Capability `report-builder-v1`; openapi regenerated; DESKTOP.md.
4. **U-121 true as-of open items** for A/R and A/P, used by the builder and the standard agings.
5. **Starters:** every P28 report the builder can express, each with a penny tie-out test against the standard report (after U-14). Balance Sheet and Trial Balance reuse the engine's current-year earnings row (Q-313).
6. `cl-api` client with exact decimal strings (not `ReportAmount`) and contract tests against a live local engine.
7. P28 report window renders builder rows: expand and collapse send the open paths to the engine; a transaction row opens its source form; a subtotal pages its detail.

**Tests:** shared fixtures parse identically; for every subtotal, its detail rows sum to it; cash and accrual sections; as-of aging with payments dated after the as-of date, and the standard aging agrees; aging buckets; formulas, including forward-reference refusal; paging at 500 and 5,000; statement timeout; a role without A/P rights gets no A/P figures; sensitive accounts hidden (PENDING until U-33); tenant archive round trip remaps ids in definitions; a grep test for float arithmetic in the run path.

**Acceptance**
- [ ] Every starter ties to its standard report to the penny.
- [ ] Drill-down from any subtotal sums to it.
- [ ] A/R and A/P open items as of a past date match the standard agings for that date.
- [ ] No floating-point money arithmetic in the run path.

## A08 — Report Builder editor

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | L | A07, approved `ReportBuilder.html` | rust-implementer (opus) | reviewer (opus), ui-verifier |

**Mockup:** `docs/design/mockups/ReportBuilder.html` (drafted by Claude, approved by the owner before A08).

**Crates and files:** `crates/cl-ui/src/screens/reports/builder/{mod.rs,outline.rs,properties.rs,preview.rs,columns.rs,formula.rs,sources.rs}`, `crates/cl-model/src/report_def/outline.rs`, `crates/cl-automation/scenarios/report_builder.yaml`.

**Steps**
1. Outline tree (left): add, remove, move, indent and outdent nodes; rename captions.
2. Properties panel: section source, measure, filters, group-bys, display, default expanded, totals, polarity, hide when zero; formula term picker (earlier rows only); text rows.
3. Columns tab: detail fields and their order, periods, compare, % of total, header captions.
4. Live preview: an engine run through the request queue, debounced, detail capped at 50 rows; the UI does no arithmetic.
5. Start blank or from a starter; **Customize** on any P28 report opens the builder on it.
6. Keyboard (in `cl-model::keymap`; KEYBOARD.md regenerated):

   | Key | Action |
   | --- | --- |
   | Insert / Delete | Add or remove a node |
   | Alt+↑ / Alt+↓ | Move a node |
   | Tab / Shift+Tab | Indent or outdent a node |
   | F2 | Rename |
   | Ctrl+Enter | Refresh the preview |
   | Ctrl+M | Memorize |
   | Ctrl+Z / Ctrl+Y | Undo / redo |

7. AccessKit labels on everything; strings through `cl-ui::i18n`.

**Tests:** outline operations as cl-model unit tests; control-channel scenario in light and dark (a P&L with Gross profit, an A/R aging by customer then month, renamed headers, keyboard only); `ui-check --compare` against the mockup.

**Acceptance**
- [ ] Every builder action works by keyboard.
- [ ] The preview always matches a full run.
- [ ] Screen matches its mockup per docs/design/README.md; any deviation is listed in the PR and QUESTIONS.md.

## A09 — Memorized Reports, transfer, export and print

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | M | A08 | rust-implementer (opus) | reviewer (opus), ui-verifier |

**Crates and files:** `crates/cl-ui/src/screens/reports/{memorized.rs,export.rs}`, `crates/cl-print/src/reports.rs`, `fixtures/golden/report_builder/*.csv`.

**Steps**
1. Memorized Reports list with P28's groups: builder and standard reports together, author shown, visible to everyone with report access (Q-310). Edit and Delete only for the author or an admin; others get **Memorize a copy**.
2. Export to Excel, CSV and PDF and print reproduce exactly the rows on screen (open paths plus paged detail). Excel cells are numbers, converted from the exact strings only at export; totals are values, not formulas.
3. Backups and the transfer file carry the definitions with ids remapped (a test proves it).

**Acceptance**
- [ ] Golden CSVs for collapsed, partly expanded and fully expanded states.
- [ ] Excel numbers stay numeric.
- [ ] Transfer and backup round trips keep definitions and their ids.

## A10 — AI drafting

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | S | A08, U-120 | rust-implementer (opus); engine-implementer (opus) for U-120 | reviewer (opus), security-reviewer (opus) |

**Crates and files:** `crates/cl-ui/src/screens/reports/builder/ai_draft.rs` (reusing P28 `ask.rs`, Q-73); engine U-120 `POST /report-builder/draft`.

**Steps**
1. **Describe the report**: the owner types a description; the engine's AI drafts a definition (sections, sources, filters, group-bys, columns). Names only go to the model — never amounts; the engine resolves names to ids and never accepts an id from the model; output is validated and refused if it contains amount-like numbers (Q-315).
2. The draft shows as a suggestion in the builder with unresolved items marked; nothing is saved until the owner accepts.

**Tests:** with a stubbed model the draft contains no numbers; the Ollama path works; with AI consent off, no cloud call is made; invented ids are refused.

**Acceptance**
- [ ] AI drafts a definition and never states or computes a number.
- [ ] Nothing is saved without the owner's accept.

## Checkpoint PR-A2 — Report Builder (opus, xhigh)

Drill-down sums; as-of aging against the standard aging; sensitive-account hiding; author and admin rules; transfer and backup; AI never producing numbers. Fix Blocking findings on `fix/PR-A2`.

## Decisions (added to QUESTIONS.md)

| ID | Topic | Decision |
| --- | --- | --- |
| Q-305 | Report Builder shape | Owner 2026-10-30: FS-builder style over transactions; one tree of sections, groups, formulas, text, blank and page-break rows; section display summary / detail / expandable; up to 3 group-bys; one tag per row (Q-233); columns pick-and-order, periods, compare, rename. Formulas add and subtract earlier rows only (default). Names "Report Builder" and "Memorized Reports". Mockup drafted by Claude and approved by the owner before A08. Runs right after P28. |
| Q-306 | Sources | Owner 2026-10-30: sales & customers, purchases & vendors, accounts/GL, items, tax and bank, all in v1. A/R and A/P: activity in a range and open items as of a date. |
| Q-307 | As-of open items | Owner 2026-10-30: true as-of (payments and credits dated on or before the as-of date), in the builder and the standard agings (U-121). |
| Q-308 | Basis on open-item sources | Default: cash/accrual doesn't apply to open items; the section shows a note. |
| Q-309 | Sensitive accounts | Default: totals for that user leave hidden accounts out, with "Some amounts are hidden for your role"; a definition referring to a hidden account can't be edited by that user. From P29 / U-33. |
| Q-310 | Who sees and edits | Owner 2026-10-30: anyone with report access builds and saves; saved reports are visible to everyone with report access; only the author or an admin edits or deletes; others memorize a copy. |
| Q-311 | Row limits | Default: 500 detail rows per page, 5,000 per run (then "more"), 100,000 for export; 30 s per run. |
| Q-312 | Where memorized reports live | Default: in the engine (standard and builder), so they travel in backups and the transfer file (owner 2026-10-30: included in the transfer file). |
| Q-313 | Balance Sheet and Trial Balance starters | Default: reuse the engine's current-year earnings row; never recomputed. |
| Q-314 | Signs | Default: each section shows amounts positive in its natural direction (income credit-positive, expenses debit-positive), so formulas read naturally. |
| Q-315 | AI drafting | Owner 2026-10-30: AI drafts a definition for the owner to accept; never states or computes numbers; works with Ollama. Default: names only go to the model, never amounts. |

## Notes for later phases

- **P29 (permissions):** turn on U-33 filtering in the builder; source rights follow role areas.
- **P24 (inventory):** enable the inventory activity source.
- **P31 (QBD import):** QuickBooks Desktop memorized reports are not imported (Q-143).
- **P32 (transfer):** MyBooks Online's import (U-08) accepts the new tables.
- **P44 (Report Packs):** packs may include builder reports.
- **U-35:** enable the project group-by when it merges.
- **P38 (docs):** a "Build your own report" help page.
