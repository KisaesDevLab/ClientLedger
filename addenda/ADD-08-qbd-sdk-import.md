# ADD-08 — Import from QuickBooks Desktop through the SDK

**Status:** Active (added 2026-11-19, during the build) · **Part of the beta: phase P35.1** (after P35, before P36) · Work packages **I01–I08** · Checkpoint **PR-I1** · Decisions Q-355–Q-367 (was QB-01–QB-13) · Engine requests U-274–U-278 (was ER-QB1–ER-QB5) · Open questions Q-ADD8a–g

Owner request, 2026-11-18; scheduling decided 2026-11-19 (first "at the end of the project", then "Change ADD-08 to P35.1") (Written first as "ADD-05"; ADD-05 is the community license addendum, so this is ADD-08.)

ADD-08 adds Import from QuickBooks Desktop: a new ClientLedger company built from a QuickBooks Desktop company file, with full transaction history, accepted only after it ties to QuickBooks' own reports. QuickBooks itself opens the file and answers queries through its SDK. ClientLedger never parses the file's bytes.

Models: opus for all work; architect plan required for I03, I04, I06 and I07 (Tier H); engine-implementer on opus at effort high for I04; combined reviewer per phase; process review at PR-I1 (opus, xhigh). Sonnet only for mechanical parts (fixtures, docs).

## When to run it (scheduling rule)

1. **ADD-08 is phase P35.1 on the beta track: it runs right after P35 (Packaging) merges and before P36 (Updates and migrations).** Its work packages I01–I08 keep their IDs and run in order inside P35.1. Owner 2026-11-19 (Q-366). This supersedes the source document's default of "after P39" and the earlier "after P48".
2. **I04 lands in the engine repo** (`KisaesDevLab/kisaes-gl-engine`) and needs owner review before I05 starts.
3. **Checkpoint PR-I1** (opus, xhigh) runs right after I08 merges.
4. Branches `phase/P35.1-qbd-sdk-import` (or `phase/I0x-short-name` per work package); PRs `I0x — <title>`; final commit `docs(state): complete P35.1, next PR-I1`. STATE.md carries one board row for P35.1; the I01–I08 and PR-I1 sub-table is under ADD-08 in Active addenda.
5. Claude Code cannot run QuickBooks. I01–I03 are built against recorded qbXML fixtures, and each ends with a short manual check on a Windows PC that has QuickBooks installed.

## Relation to P31

P31 (beta) already ships the report-based QuickBooks Desktop import with tie-out, built on the engine capabilities `imports-batch-v1`, `qbd-open-items-v1`, `qbd-mapping-v1`, `imports-tieout-v1` and `imports-transactions-v1`, with the BeforeImport backup and undo. ADD-08 adds the SDK route (full history straight from QuickBooks, no report exports).

- When I04 and I05 start, the architect must check which of U-274–U-278 are already covered by those P31 capabilities and **reuse P31's tie-out and batch plumbing rather than duplicate it**; any U-row fully covered is closed as "covered by P31" instead of built.
- Q-359 (QB-05: an SDK import always creates a new, empty company) differs from P31, which imports into the open company. Both routes may coexist. Open question Q-ADD8g; default: the SDK route creates a new company as written.

## Scope

In scope:

- A .QBW open in an installed QuickBooks Desktop for Windows (Pro, Premier or Enterprise).
- .QBB and .QBM files, through a guided restore in QuickBooks first.
- Lists, posting transactions, cleared status, open AR and AP, inventory on hand.
- A tie-out report and an accept-or-discard step.

Out of scope:

- Reading a file with no QuickBooks installed (dead-file recovery).
- QuickBooks Online and QuickBooks for Mac.
- Merging into an existing ClientLedger company.
- Paycheck detail, attachments, memorized transactions, budgets, users and passwords.
- Multi-currency files.

## Locked decisions

Thirteen decisions fixed for the build (owner 2026-11-18/19). Q-365 (QB-11) is the owner's assumption and is repeated under Open questions. Full rows are in QUESTIONS.md.

| Q | Was | Decision | Why |
| --- | --- | --- | --- |
| Q-355 | QB-01 | Talk to QuickBooks through the SDK request processor (QBXMLRP2 over COM) with raw qbXML. No QBFC. | Transaction-complete and supported. The request processor should install with QuickBooks, so nothing from Intuit is redistributed; I01 verifies this. |
| Q-356 | QB-02 | No direct file parsing and no OpenQBW in this addendum. | Incomplete coverage, and Intuit's license bars reverse engineering. |
| Q-357 | QB-03 | Two pieces: a standalone Windows extractor and a cross-platform importer, joined by a package file. | QuickBooks is Windows-only; ClientLedger is not. The books may move to another computer. |
| Q-358 | QB-04 | The extractor is read-only. It sends only query and report requests. | The source file must never change. A test fails the build if any Add, Mod, Delete or Void request exists. |
| Q-359 | QB-05 | An import always creates a new, empty company. Discard deletes that company whole. | No merge logic and a clean rollback. |
| Q-360 | QB-06 | Full history is the default. An optional cutoff date brings in detail from that date on. | Owners expect their history; very large files need an exit. |
| Q-361 | QB-07 | Amounts come from QuickBooks and are never recomputed. Each transaction posts the ledger lines QuickBooks recorded, including cost of goods sold and sales tax. | Recalculation would break the tie-out. |
| Q-362 | QB-08 | Ledger-only fallback: a transaction with no ClientLedger type, or one the typed queries miss, imports as a journal entry tagged with its original type, number and name. | Covers paychecks and payroll liability payments, and guarantees the trial balance ties. |
| Q-363 | QB-09 | Estimates, sales orders and purchase orders are extracted into the package but not imported yet. | Those features come in P40/P41. No second extraction needed later. |
| Q-364 | QB-10 | No sensitive personal data is requested: no Social Security numbers, no card numbers. Employees arrive as names only. | Nothing to protect that was never read. |
| Q-365 | QB-11 | The import is free under the Community License, not Pro-gated. | It is how an owner gets their own books in. |
| Q-366 | QB-12 | The work runs as phase P35.1, right after P35 and before P36, inside the beta. | Owner 2026-11-19: "Change ADD-08 to P35.1". Supersedes the source doc's "after P39" default and the earlier "after P48". |
| Q-367 | QB-13 | Menu name: Import from QuickBooks Desktop. No Intuit logos. Footer line on the first wizard page: "QuickBooks is a registered trademark of Intuit Inc. ClientLedger is not affiliated with or endorsed by Intuit." | Plain naming of the source product only. |

Cutoff mode (Q-360). One opening journal entry dated the day before the cutoff carries every account except AR, AP and inventory. Open invoices, open bills and an opening inventory adjustment bring those three in, all offset to Opening Balance Equity. The import fails if Opening Balance Equity does not net to zero.

## Architecture

The import is two programs joined by one file: a Windows-only extractor that asks QuickBooks for the data, and a cross-platform importer that posts it through the engine. The package is the only thing that crosses between the two sides, so both can sit on one Windows PC or on two different machines. The tie-out compares the new company to QuickBooks' own totals, which travel inside the package.

| Part | Where it lives | Language |
| --- | --- | --- |
| Extractor (`qbd-extract`) | ClientLedger repo, Windows-only crate, also shipped as a standalone executable | Rust, COM, qbXML |
| Package reader and mapper | ClientLedger shell | Rust |
| Import session and historical posting | Kisaes GL Engine (U-274–U-278) | Engine API |
| Wizard and tie-out screens | ClientLedger shell | Rust, egui |

Layering note: process spawning and COM stay in the Windows-only extractor crate; the architect settles at I08 which existing crate may launch it (cl-host) and updates the layering rule in CLAUDE.md if needed.

## Interchange package

One SQLite file with the extension `.clqb`, written by the extractor and read by the importer on any operating system.

| Table | Holds |
| --- | --- |
| `meta` | Format version, extractor version, QuickBooks product and release, qbXML version used, company name, extraction time, date range, completion flag |
| `prefs` | Company preferences that change mapping: accrual or cash reports, sales tax on or off, inventory on or off, class tracking, multi-currency, fiscal year start |
| `list_object` | One row per list entry: list type, ListID, name, active flag, parent ListID, and the raw qbXML |
| `txn` | One row per transaction: TxnID, type, date, number, entity ListID, total, and the raw qbXML |
| `txn_link` | Applied-to pairs: payment to invoice, bill payment to bill, credit to document, with amounts |
| `gl_line` | Every ledger posting line from the transaction-detail report: TxnID, account, debit, credit, name, class, memo, cleared status |
| `control_total` | QuickBooks' own figures for the tie-out: trial balance by account at each date, AR by customer, AP by vendor, inventory quantity and value by item, transaction counts by type |
| `extract_log` | Each request sent, row counts, timing, warnings |

Rules:

- Raw qbXML is kept for every object, so a mapping fix never requires a second extraction.
- `meta.format_version` starts at 1. The importer refuses a newer major version with a plain message.
- `gl_line` must balance per transaction and in total before the package is marked complete.
- The file is not encrypted and holds the full books. The wizard offers to delete it after the import is accepted.

## Build phases

Eight phases, I01 to I08, run in order inside P35.1; I04 lands in the Kisaes GL Engine repo and needs owner review before I05 starts. A process review (PR-I1) follows I08.

### I01: SDK connection layer (Tier M)

- New crate `qbd-extract` (Windows only) with a COM wrapper for the request processor: open connection, begin session, process request, end session, close.
- Host and company queries; pick the highest qbXML version the installed QuickBooks supports.
- Confirm the request processor is present on a plain QuickBooks install with no SDK, and settle 32-bit versus 64-bit builds. Record the answer as a decision.
- Plain-language errors: QuickBooks not installed, no file open, a different file open, access denied by the QuickBooks admin, QuickBooks busy with a dialog.
- Accept: on the manual check, the tool prints product, release and company name from a sample company. The read-only test from Q-358 is in place.
- Commits: `feat(qbd-extract): ...`

### I02: Extract lists and preferences (Tier M)

- Queries for accounts, classes, customers and jobs, vendors, employees (names only), other names, all item types, sales tax codes, sales tax items and groups, terms, payment methods.
- Iterators on every list that supports them, 500 rows a page.
- Package writer for `meta`, `prefs`, `list_object`.
- Accept: fixture tests parse every list type; list counts on the manual check match the QuickBooks list windows.

### I03: Extract transactions, ledger lines and control totals (Tier H)

- Typed queries with line items and linked transactions for the types in the mapping table.
- Transaction-detail report with TxnID, account, debit, credit, name, class, memo and cleared status into `gl_line`. Confirm in this phase that the report exposes TxnID and cleared status; if not, record the gap in QUESTIONS.md.
- Control totals: trial balance at each fiscal year-end and at the last transaction date, AR and AP aging by name, inventory valuation by item.
- Resumable: a stopped extraction continues from the last completed request.
- Accept: `gl_line` balances per transaction and in total; its account totals equal the extracted trial balance at the final date.

### I04: Engine requests (Kisaes GL Engine repo) (Tier H)

- U-274 (was ER-QB1): import session on a new tenant, with discard that removes the tenant.
- U-275 (was ER-QB2): external ID map (source system, source ID) on list entries and transactions.
- U-276 (was ER-QB3): historical posting mode that accepts supplied ledger lines, original dates and document numbers, and skips recalculation, closed-period checks and auto-numbering.
- U-277 (was ER-QB4): set cleared status on imported lines.
- U-278 (was ER-QB5): batch endpoints sized for 100,000 or more transactions.
- First step: the architect compares these with the P31 capabilities (see Relation to P31) and reuses them where they cover a request.
- Accept: engine tests cover each request; historical mode is rejected outside an import session.

### I05: Mapping and list import (Tier M)

- Importer module in the shell (Rust) that reads the package and builds a mapping: account types, item types, names, tax codes.
- Preflight report that blocks or warns per the Risks table: multi-currency, advanced inventory, units of measure.
- Create lists through the engine API in dependency order, preserving names, numbers, hierarchy and inactive flags.
- Accept: importing the fixture package creates every list entry once; running it twice in one session creates no duplicates.

### I06: Transaction import (Tier H)

- Post transactions by date using the historical posting mode (U-276), each with QuickBooks' own ledger lines.
- Rebuild applications from `txn_link`: payments to invoices, bill payments to bills, credits to documents.
- Ledger-only fallback (Q-362) and cutoff mode (Q-360).
- Accept: every `gl_line` TxnID exists in ClientLedger exactly once, as a typed transaction or a fallback journal entry.

### I07: Tie-out and accept (Tier H)

- Run the checks in the Tie-out section and show differences by account, name or item, with drill-down to the transactions behind a difference.
- "Accept this import" is enabled only when the required checks pass. Discard removes the company.
- Save the tie-out report as a PDF in the new company's attachments.
- Accept: a fixture with a deliberately altered control total blocks acceptance and points to the right account.

### I08: Wizard, extractor packaging, help (Tier M)

- Wizard under File, Import from QuickBooks Desktop: choose source, extract or open a package, preflight, options, import progress, tie-out, accept.
- Restore instructions for .QBB and .QBM, shown when the user picks one. Exact copy: "This is a backup or portable file. Open QuickBooks, choose File, then Open or Restore Company, restore it, and leave the company open. Then come back and choose Continue."
- On Windows the wizard launches the extractor. Elsewhere it explains how to run the standalone extractor on the QuickBooks PC and bring the .clqb file over.
- Extractor shipped as a signed standalone executable (the architect confirms the current signing decision, Q-27 and ADD-05 L5) and bundled in the Windows installer.
- Help guide page, including the QuickBooks access prompt and the advice to work on a copy of the company file.
- Accept: manual end-to-end run on two real company files of different QuickBooks years, both accepted with a clean tie-out.

### PR-I1: Checkpoint (process review, no tier)

Opus at xhigh: read-only extractor test still enforced, tie-out blocks on altered totals, historical mode refused outside an import session, no secrets or personal data in packages or logs, P31 plumbing reused.

## Mapping reference

Each QuickBooks object imports as the matching ClientLedger type where one exists, and as a ledger-only journal entry (Q-362) where it does not. I05 checks this table against the types the beta actually shipped and records any change.

### Lists

| QuickBooks | Imports as | Notes |
| --- | --- | --- |
| Chart of accounts | Accounts | Type, number, sub-account hierarchy and inactive flag kept. Opening balances ignored; history carries them. |
| Classes | Classes | Hierarchy kept. |
| Customers and jobs | Customers and jobs | Jobs stay under their customer. |
| Vendors | Vendors | 1099 flag kept; tax ID not requested (Q-364). |
| Employees | Names only | Needed so fallback entries show who was paid. |
| Other names | Other names | |
| Service, non-inventory, other charge items | Items | Income and expense accounts kept. |
| Inventory part items | Inventory items | Average cost going forward; history uses QuickBooks' recorded cost. |
| Assemblies, groups | Assemblies as inventory items; groups expanded to their lines | Builds import through the fallback. |
| Discount, subtotal, payment items | Line types on the documents that use them | |
| Sales tax items, groups, codes | Sales tax rates, combined rates, taxable codes | |
| Terms, payment methods | Terms, payment methods | |

### Transactions

| QuickBooks | Imports as |
| --- | --- |
| Invoice, sales receipt, credit memo, statement charge | Invoice, sales receipt, credit memo, statement charge |
| Receive payment, deposit | Customer payment, deposit |
| Bill, vendor credit, item receipt | Bill, vendor credit, item receipt |
| Bill payment by check or credit card | Bill payment |
| Check, credit card charge, credit card credit | Check, card charge, card credit |
| Journal entry, transfer | Journal entry, transfer |
| Inventory adjustment | Inventory adjustment |
| Sales tax payment | Sales tax payment |
| Paycheck, payroll liability payment, liability adjustment, year-to-date adjustment | Ledger-only journal entry |
| Build assembly, customer refund by card | Ledger-only journal entry |
| Estimate, sales order, purchase order | Kept in the package, not imported (Q-363) |

Every imported entry keeps its QuickBooks ID in the external ID map, plus original date, number, memo and cleared status.

## Tie-out and acceptance

An import can be accepted only when the first four checks pass to the cent; the last two are shown for information.

| Check | Compared against | Pass rule |
| --- | --- | --- |
| Trial balance, every account, at each fiscal year-end and at the last transaction date | QuickBooks trial balance in `control_total` | Equal to the cent, every account, every date |
| Open AR by customer | QuickBooks AR aging | Equal to the cent per customer |
| Open AP by vendor | QuickBooks AP aging | Equal to the cent per vendor |
| Inventory on hand by item at the last transaction date | QuickBooks inventory valuation | Quantity exact; value equal to the cent |
| Transaction counts by type | Counts in the package | Shown; any difference listed |
| Ledger-only entries | None | Count and total by original type, so the owner sees what arrived without document detail |

In cutoff mode the trial balance check runs from the cutoff date forward, and Opening Balance Equity must be zero after the opening entries.

A failed check names the account, customer, vendor or item and lists the transactions behind the difference. The report is saved with the company whether the import is accepted or discarded for another try. Reuse P31's tie-out plumbing (see Relation to P31).

## Risks

The largest risk is that no automated test can run QuickBooks, so three manual checks and the I08 end-to-end run carry the proof.

| Risk | Handling |
| --- | --- |
| No QuickBooks in CI | Parser and mapper tests run on recorded qbXML. Manual checks close I01, I02, I03 and I08. |
| Recorded responses from Intuit's sample companies are Intuit's content | Keep them in a private fixtures repo. The public repo holds only hand-written synthetic qbXML (non-negotiable 10). |
| 32-bit versus 64-bit request processor | Settled in I01; ship both builds if needed and pick at run time. |
| QuickBooks access prompt confuses owners, and warns on unsigned apps | Signed extractor, screenshots in the help guide, and a specific error when access is denied. |
| Detail report lacks TxnID or cleared status on some versions | I03 confirms. Fallback: cleared status is dropped with a warning and noted in the tie-out report. |
| Very large files | Iterators, resumable extraction, batch posting (U-278), cutoff mode. |
| Enterprise advanced inventory (FIFO, multiple sites), units of measure | Preflight warns. History still ties because costs come from QuickBooks; going forward ClientLedger uses average cost. |
| Multi-currency file | Preflight blocks the import with a plain message. |
| Older file gets upgraded when a newer QuickBooks opens it | Help text and wizard both say to restore or copy first. The original backup is not altered. |
| Fewer working QuickBooks installs over time | Per the earlier research, Enterprise is the only edition still sold new in the US and a 30-day trial exists. A lapsed subscription is view-only for a year, then locked. |
| Intuit SDK terms | The extractor uses only the request processor that ships with QuickBooks. Have counsel read the current SDK and Desktop license terms before release. |

Not available through the SDK, so not imported: reconciliation reports, attachments, memorized transactions, the audit trail, users and the closing-date password.

## Open questions

Each has a default the plan already assumes. Rows are in QUESTIONS.md (Q-ADD8a–g).

| ID | Question | Default / answer |
| --- | --- | --- |
| Q-ADD8a | Is ADD-08 the next free addendum number? | Answered: ADD-08 (the source's "ADD-05" was taken by the community license addendum). |
| Q-ADD8b | Where does it run? | Answered, owner 2026-11-19: phase P35.1, after P35 and before P36, in the beta (Q-366). |
| Q-ADD8c | Free for everyone, or part of the Pro tools? | Free (Q-365). |
| Q-ADD8d | Which QuickBooks versions will you have for the manual checks? | Enterprise 24.0 trial plus one older Pro or Premier file. |
| Q-ADD8e | Are U-274–U-278 acceptable as engine pull requests, given that only the owner merges engine changes? | Yes; per Q-301 engine PRs merge when green. |
| Q-ADD8f | Should dead-file recovery (reading a .QBW with no QuickBooks) get its own later addendum after legal review? | Not planned. |
| Q-ADD8g | The SDK route always creates a new company (Q-359) but P31 imports into the open company. Keep both? | Both coexist; the SDK route creates a new company as written. |

## Notes for later phases

- **P31:** unchanged; ADD-08 reuses its capabilities (see Relation to P31).
- **P40/P41 (estimates, sales orders, POs):** may later read the estimates, sales orders and purchase orders already held in `.clqb` packages (Q-363).
- **P35/P36 (packaging):** the extractor is bundled in the Windows installer built in P35, so I08 adds it to the installer components (P35.1 runs right after P35, before P36).
- **P38 (documentation):** the QuickBooks switcher guide may mention the SDK route once I08 ships.
