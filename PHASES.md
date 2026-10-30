# PHASES.md — ClientLedger build plan (index)

This is the index. Each phase's full detail — decisions, engine changes, crates and files, ordered commit-sized steps, tests, and acceptance criteria — lives in the milestone files under `phases/`. Read this index plus the milestone file for the phase marked **Next** in STATE.md.

**Beta track:** P00–P39 in seven milestones. **Post-beta:** P40–P49. **Process reviews:** PR-P05, PR-P10, PR-1 to PR-10 (opus; see docs/AGENTS.md).

## Milestones

| Milestone | File | Phases | Outcome |
| --- | --- | --- | --- |
| M0 Foundation | [phases/M0-foundation.md](phases/M0-foundation.md) | P00–P09 | Offline app runs its own Postgres and engine, opens and backs up a company, holds preferences and AI keys, drivable by Claude Code |
| M1 Core ledger | [phases/M1-core-ledger.md](phases/M1-core-ledger.md) | P10–P16 | Grid, lists and tags, register, checks, deposits, card and owner helpers, journal entries, Home |
| M2 Money in and out | [phases/M2-money-in-out.md](phases/M2-money-in-out.md) | P17–P24 | Bills, Pay Bills, printing, invoices, payments, credit memos, statements, sales tax, inventory |
| M3 Banking | [phases/M3-banking.md](phases/M3-banking.md) | P25–P27 | Bank file import, review and rules, reconcile |
| M4 Reports and controls | [phases/M4-reports-controls.md](phases/M4-reports-controls.md) | P28–P30 | Reports, budgets, users and permissions, audit trail, payroll import |
| M5 Data movement and multi-user | [phases/M5-data-multiuser.md](phases/M5-data-multiuser.md) | P31–P34 | QBD import with tie-out, transfer file, IIF, LAN multi-user, backups and integrity |
| M6 Ship beta | [phases/M6-ship-beta.md](phases/M6-ship-beta.md) | P35–P39 | Installers, updates, hardening, help, beta (STOP) |
| M7 Post-beta | [phases/M7-post-beta.md](phases/M7-post-beta.md) | P40–P49 | Estimates and sales orders, POs, assemblies, Plaid, Report Packs, accountant tools, workpapers, templates, cash flow, work copies |

## All phases

Tier and agent routing: docs/AGENTS.md. Size: S ≈ under a day of agent work, M ≈ a few days, L ≈ a week or more (rough guide for planning only).

| Phase | Title | Tier | Size | Depends on |
| --- | --- | --- | --- | --- |
| P00 | Repository bootstrap | L | M | — |
| P01 | Engine submodule and single-executable build | L | M | P00 |
| P02 | Engine desktop profile (upstream) | H | L | P01 |
| P03 | Bundled PostgreSQL manager | L | M | P00 |
| P04 | Supervisor | L | M | P02, P03 |
| P05 | Engine API client | H | L | P02, P04 |
| P06 | Application shell | L | L | P05 |
| P07 | Automation and control channel | L | M | P06 |
| P08 | Company lifecycle | M | L | P05, P06 |
| P09 | Preferences and AI provider keys | M | M | P06, P08 |
| P10 | Ledger grid widget | H | L | P06, P07 |
| P11 | Lists, centers, and tags | M | L | P10 |
| P12 | Bank and account register | H | L | P10, P11 |
| P13 | Write Check | M | M | P12 |
| P14 | Deposits, transfers, sales receipts, card charges, owner and loan helpers | M | L | P13 |
| P15 | Journal entries, templates and recurring | H | M | P12 |
| P15A | Bulk Entry | M | M | P13–P15 |
| P16 | Home workspace | L | M | P13–P15 |
| P17 | Enter Bill, vendor credits, receipt inbox, attachment viewer | M | L | P13 |
| P18 | Pay Bills (checks) | M | M | P17 |
| P19 | Form printing | L | L | P18 |
| P20 | Create Invoice | M | L | P11, P19 |
| P21 | Receive Payment | H | L | P20 |
| P22 | Credit memos, refunds, statements, finance charges | M | M | P20, P21 |
| P23 | Sales tax | H | M | P20 |
| P24 | Inventory (average cost) | H | M | P17, P20, P23 |
| P25 | Bank file import (QIF, QFX, OFX, CSV) | H | L | P12 |
| P26 | Bank feed review | M | L | P25 |
| P27 | Reconcile | H | M | P12, P26 |
| P28 | Report engine UI | H | L | P12–P24 |
| P29 | Users, permissions, closing date, audit trail | M | L | P16 |
| P30 | Payroll import | M | M | P15 |
| P31 | QuickBooks Desktop import with tie-out, spreadsheet import, other accounting systems | H | L | P11, P28 |
| P32 | Transfer file for MyBooks Online, and IIF export | H | M | P08, P31 |
| P33 | LAN multi-user | H | L | P29 |
| P34 | Backup scheduling, recovery, and data integrity | H | M | P08, P33 |
| P35 | Packaging | L | L | P00–P34 |
| P36 | Updates and migrations | H | M | P35 |
| P37 | Hardening | L | L | P35 |
| P38 | Documentation | L | M | P35 |
| P39 | Beta release (STOP) | L | S | P35–P38 |
| P40–P49 | Post-beta | — | — | see M7 |

## Addenda (added during the build)

| Addendum | File | Phases | Status | When it runs |
| --- | --- | --- | --- | --- |
| ADD-01 Document intake, local reading, pop-out viewer | [addenda/ADD-01-document-intake.md](addenda/ADD-01-document-intake.md) | A01–A05 | Active, part of the beta | A01–A04 right after P17 merges, before P18; A05 right after P26; checkpoint PR-A1 after A05 |

## Critical path and parallel lanes

Critical path: P00 → P01 → P02 → P04 → P05 → P06 → P07 → P10 → P11 → P12 → P13 → P17 → P18 → P19 → P20 → P21 → P22 → P28 → P31 → P32 → P33 → P34 → P35 → P36 → P39.

Phases that may run in parallel with the critical path (rules in docs/AGENTS.md):
- P03 alongside P01–P02
- P08–P09 alongside P07–P10 once P05 and P06 are merged
- P15 alongside P13–P14
- P25–P27 alongside P19–P24
- P23–P24 alongside P21–P22
- P29–P30 alongside P25–P28
- P37–P38 alongside P36

## Process-review checkpoints (opus)

| ID | Where | Reviews |
| --- | --- | --- |
| PR-P05 | inside P05 | engine contract plan |
| PR-P10 | inside P10 | grid design plan |
| PR-1 | after P16 | core ledger |
| PR-2 | after P19 | purchasing lifecycle |
| PR-3 | after P22 | sales lifecycle |
| PR-4 | after P24 | sales tax and inventory |
| PR-5 | after P27 | bank lifecycle |
| PR-6 | after P28 | reports |
| PR-7 | after P32 | data movement |
| PR-8 | after P33 | LAN multi-user |
| PR-9 | after P36 | upgrades and migrations |
| PR-10 | in P39 | whole-app owner journeys |
| PR-A1 | after A05 | document to books (ADD-01; opus, xhigh) |

## Phase template (every detailed phase follows it)

1. **Header table:** tier, size, dependencies, implementer agent and model, reviews.
2. **Decisions:** the QUESTIONS.md ids that govern the phase. **Engine:** UPSTREAM ids and capability-check items. **Mockup:** the reference screen in `docs/design/` for phases that build a screen.
3. **Crates and files:** the modules, screens, scenarios, and fixtures the phase creates or changes.
4. **Steps:** ordered, each about one commit, with a suggested conventional-commit message.
5. **Tests:** unit, property, contract, golden, fuzz, and control-channel scenarios.
6. **Acceptance:** the checklist that defines done (copied into the PR and the STATE.md log).

## Beta scope summary

Small business owners in the US; offline; Rust/egui native app on the MyBooks engine; companies as engine tenants; checks only on pre-printed stock; average-cost inventory without assemblies; bank file import (no Plaid); optional AI with the owner's key or local Ollama; QBD import via the shared report-export importer with tie-out; transfer to MyBooks Online by file; LAN multi-user up to five users; free app under PolyForm Internal Use 1.0.0 with the engine licensed to ClientLedger (Q-303). Full decision list: QUESTIONS.md.
