# UPSTREAM.md — changes to the MyBooks engine

ClientLedger consumes the public MyBooks engine repo as the `engine/` git submodule tracking `main` (Q-02; PolyForm Internal Use 1.0.0 plus the ClientLedger redistribution grant, Q-31). Engine changes are made on `clientledger/<feature>` branches, follow the engine's own conventions and tests, and go to the owner as pull requests on the engine repo; the owner reviews and merges (Q-29). Until merged, the submodule may point at the branch commit. Keep changes generic so MyBooks Online benefits.

Status values: `Planned`, `In progress`, `Done (branch)`, `Merged (MyBooks vX.Y)`, `Blocked`.

| ID | Change | Needed by | Branch | Status |
| --- | --- | --- | --- | --- |
| U-01 | Queue abstraction with pg-boss adapter (desktop) alongside BullMQ (server) | P02 | clientledger/desktop-profile | Planned |
| U-02 | `CL_MODE=desktop` profile: no Redis (also no `REDIS_URL` requirement in `env.ts`; `/health` must return 200 without it, Q-P01a), env-provided bind address and bearer token, file logging | P02 | clientledger/desktop-profile | Planned |
| U-03 | Connectivity reporting on `/health` plus an `engineCommit` field (Q-P01a); online-only modules return `OfflineUnavailable` fast | P02 | clientledger/desktop-profile | Planned |
| U-04 | OpenAPI 3.1 export covering all UI routes; committed `openapi.json` | P02 | clientledger/desktop-profile | Planned |
| U-05 | Tenant lifecycle (list/create/rename/delete) and versioned per-tenant export/import (NDJSON + attachments + manifest) for backups, restores, and transfers | P02, P08 | clientledger/tenant-portability | Planned |
| U-06 | Endpoint accepting normalized bank-import batches (rows, dedupe keys, source-file hash) into the review queue; parsing is in Rust (Q-11) | P25 | clientledger/bank-import | Planned |
| U-07 | Desktop-mode adjustments to the QBD report-export importer (file paths, progress events, tie-out endpoint); report list and mapping unchanged | P31 | clientledger/qbd-import | Planned |
| U-08 | Import of `.clxfer` transfer files in MyBooks Online, showing verification totals; accepts any older `formatVersion`, refuses newer with an update message (Q-43) | P32 | clientledger/transfer | Planned |
| U-09 | AI provider credentials supplied by the desktop app per session (owner's own key); never stored by the engine in desktop mode; AI disabled without key or offline; redaction of bank/card numbers, tax IDs, emails, phones before provider calls; per-call token usage reported for the meter and cap | P02, P09 | clientledger/desktop-profile | Planned |
| U-11 | Ollama provider adapter for the engine's multi-provider AI (local base URL, model choice, no key) if missing; Anthropic and OpenAI adapters confirmed | P09 | clientledger/desktop-profile | Planned |
| U-12 | Integrity check endpoints (full Verify and fast weekly subset) with typed findings and safe automatic fixes | P34 | clientledger/verify | Planned |
| U-10 | Redistribution grant text added to the engine repo (`LICENSE-CLIENTLEDGER.md`) by the owner | P39 | main | Planned — owner (Q-31) |

Add rows as capability checks find gaps (credit memos, statements, finance charges, sales tax, average-cost inventory, reconciliation, permissions granularity, optimistic concurrency, change notifications for LAN mode; post-beta: estimates, sales orders, POs, assemblies, custom fields, form templates).

## Entry template

```
### U-xx — <title>
- Needed by: Pxx
- Why: <what the desktop app cannot do without it>
- Engine files touched: <paths>
- Schema/migrations: <yes/no + description>
- API changes: <endpoints added/changed, OpenAPI updated>
- Tests: <engine tests added>
- Server appliance impact: <none / describe>
- Status: <status> — <date>
```
