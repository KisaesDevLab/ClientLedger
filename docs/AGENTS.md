# AGENTS.md — multi-agent build and model routing

How Claude Code builds ClientLedger with subagents and the right model for each job. Subagent definitions live in `.claude/agents/`. This file covers development agents only; the in-app AI features (P09) are unrelated.

## Principles

1. **Match the model to the risk, not the size.** Code that touches money, posting, concurrency, imports, migrations, or security gets Opus for planning and review even when the diff is small. Routine UI, plumbing, and docs use Sonnet. Haiku only searches and keeps records.
2. **The orchestrator stays light.** The main session reads STATE.md, picks the phase, delegates, checks results, and opens the pull request. It does not read large files itself; it asks `explorer`.
3. **Fresh eyes review.** `reviewer` and `security-reviewer` never see the implementation conversation, so their review isn't biased by it.
4. **Parallel where independent, sequential where coupled.** Fan out read-only work freely; run writers in parallel only in separate worktrees on disjoint files.
5. **Escalate, don't loop.** Two failed attempts at the same problem moves it up a tier.

## Models

| Model alias | Used by | Why |
| --- | --- | --- |
| `opus` | orchestrator (main session, unless the owner sets another with `/model`), `explorer`, `bookkeeper`, `engine-scout`, `rust-implementer` (Tier L/M), `engine-implementer` (S changes), `test-writer`, `ui-verifier`, `ci-fixer`, `docs-writer`, `architect` (Tier M), `reviewer` (Tier L/M), `security-reviewer` | Everything that is not complex, crucial or financial code (owner routing, 2026-10-18) |
| `fable` | `architect` (Tier H), `reviewer` (Tier H), `process-reviewer`, `rust-implementer` (Tier H), `engine-implementer` (M/L or accounting-rule changes, tenant export/import, posting, balances, closing date, sales tax, inventory costing), any work package the architect marks crucial (money, posting, concurrency, data movement, backup/restore, secret handling) | Complex code, crucial functions and financial calculations |

Owner routing (2026-10-18): **Fable for complex code, crucial functions and financial calculations; Opus for other areas.** Sonnet and Haiku are no longer used. Launch the main session on the model the owner chooses with `/model` (Opus by default). The orchestrator passes a per-invocation `model` when it starts `rust-implementer` or `engine-implementer`; that overrides the agent file's default (Claude Code resolves the invocation parameter first, then the frontmatter, then `CLAUDE_CODE_SUBAGENT_MODEL`, then the main model). Do not set `CLAUDE_CODE_SUBAGENT_MODEL_FORCE`, which would disable this routing.

**Effort:** `low` for search and records; `medium` for routine implementation and verification; `high` for architect, reviewers, and Tier H implementation; `xhigh` only for an escalated re-plan.

## Process reviews (Fable)

`process-reviewer` runs on `fable` at the checkpoints below, in addition to the per-phase `reviewer`. It traces a whole process across phases, checks accounting invariants, and builds adversarial scenarios. Blocking findings become fix work on a `fix/<checkpoint-id>` branch before the next milestone closes. Reports go to `target/reviews/<checkpoint-id>.md` and a summary goes into the STATE.md log.

| ID | When | What it reviews |
| --- | --- | --- |
| PR-P05 | Architect plan for P05, before implementation | Engine contract: money representation, idempotency, 409 concurrency, offline errors |
| PR-P10 | Architect plan for P10, before implementation | Ledger grid design: editing state machine, exact amount parsing, virtualization |
| PR-1 | After P16 (end of M1) | Core ledger: register, checks, deposits, journal entries, recurring postings, closed periods |
| PR-2 | After P19 | Purchasing lifecycle: bill → Pay Bills → discounts and credits → check printing → reprint and void |
| PR-3 | After P22 | Sales lifecycle: invoice → payment (discounts, deposits, bounced checks) → credit memos and refunds → statements and finance charges |
| PR-4 | After P24 (end of M2) | Sales tax and inventory: rounding consistency, liability, average cost, negative stock |
| PR-5 | After P27 (end of M3) | Bank lifecycle: file import → dedupe → rules and matching → transfers → reconcile → discrepancies |
| PR-6 | After P28 | Reports: every report ties to the trial balance; drill-down sums; accrual vs. cash; sensitive accounts |
| PR-7 | After P32 | Data movement: tenant export/import, backups and restore, QBD import tie-out, transfer files, IIF export |
| PR-8 | After P33 | LAN multi-user: conflicts, host loss, single-user maintenance mode, live refresh |
| PR-9 | After P36 | Upgrades and migrations: interrupted migration, rollback, schema/version compatibility of backups and transfers |
| PR-10 | Before P39 sign-off | Whole-app owner journeys on the sample company: setup → a month of work → close → send to accountant → restore |

Also use `process-reviewer` for the architect plan of any engine change sized L that touches posting, balances, inventory costing, sales tax, or tenant export/import.

If `fable` is unavailable on the account, run `process-reviewer` with `model: opus` and `effort: xhigh` per invocation, and note it in STATE.md.

## Phase tiers

| Tier | Phases | Implementer | Architect plan | Reviews |
| --- | --- | --- | --- | --- |
| **H** — money, posting, concurrency, data movement, migrations, core widget | P02, P05, P10, P12, P15, P21, P23, P24, P25, P27, P28, P31, P32, P33, P34, P36; post-beta P40, P42 | `fable`, effort high | required (`architect` on fable) | `reviewer` (fable) + `ui-verifier` (opus); `security-reviewer` (opus) where listed below |
| **M** — feature screens with accounting effects | P08, P09, P11, P13, P14, P17, P18, P20, P22, P26, P29, P30; post-beta P41, P43, P45, P46, P47, P48 | `opus`, effort medium; `fable` for any work package touching money, posting, closing date, backup/restore or secrets | only if engine-scout reports an M/L gap | `reviewer` (opus; fable for the money packages) + `ui-verifier` |
| **L** — scaffolding, shell, printing, packaging, docs | P00, P01, P03, P04, P06, P07, P16, P19, P35, P37, P38, P39; post-beta P44 | `opus`, effort medium | no | `ui-verifier` for screens; `reviewer` optional |

`security-reviewer` (opus) also runs on P03, P04, P07, P09, P29, P32, P33, P35, P36, and P37, regardless of tier.

Engine changes: `engine-implementer` runs on `opus` for S-sized changes and on `fable` for M/L changes or anything touching posting, balances, inventory costing, sales tax math, closing dates, secrets, or tenant export/import.

## The loop for one phase

```
orchestrator (opus)
 ├─ in parallel ──────────────────────────────────────────────┐
 │   engine-scout (opus)        capability check              │
 │   test-writer (opus)         failing tests from criteria   │
 │   explorer ×N (opus)         locate code/patterns          │
 ├─ architect (fable)            plan → target/plans/Pxx.md    │ Tier H, or M with M/L gap
 ├─ engine-implementer          upstream changes first        │ if gaps
 ├─ rust-implementer            model by tier                 │ may split into work packages
 ├─ in parallel ──────────────────────────────────────────────┤
 │   ui-verifier (opus)         screens                       │
 │   reviewer (opus/fable)      fresh-eyes diff review        │
 │   security-reviewer (opus)   listed phases                 │
 │   docs-writer (opus)         help pages                    │
 ├─ rust-implementer            fix Blocking findings         │
 ├─ bookkeeper (opus)           STATE/QUESTIONS/UPSTREAM      │
 └─ PR → auto-merge on green CI; ci-fixer (opus)   if red     ┘
```

Work packages: when an architect plan lists independent packages (for example, a grid widget's editor types or several report types), the orchestrator may run up to 3 `rust-implementer` subagents at once, each with `isolation: worktree` (passed per invocation) on disjoint files, then merge their branches into the phase branch in plan order. Packages that touch the same crate's public API run sequentially.

## Parallel phases

The plan is sequential by default. The orchestrator may run **two phases at once** only when:
- both phases' "Depends on" lists are fully merged to `main`, and
- they touch disjoint crates and disjoint engine modules, and
- neither is Tier H with an open UPSTREAM item in the same engine area.

Each runs in its own worktree and branch, with its own PR. Examples that qualify: P02 ∥ P03, P19 ∥ P25, P23 ∥ P25, P30 ∥ P31. STATE.md shows both as `In progress`.

## Escalation

| Situation | Action |
| --- | --- |
| Opus implementer fails the same acceptance criterion twice, or `ci-fixer` fails the same error twice | Re-run that work package on `fable`, effort high |
| Fable implementer fails twice | `architect` (fable) re-plans that package on effort `xhigh`; implement the new plan on `fable` |
| Still failing (non-security work) | Ask `process-reviewer` (fable) to diagnose the root cause; implement its fix on `fable` |
| Still failing | Mark only that item blocked in STATE.md, add a QUESTIONS.md row with a default, and continue other work (Q-220) |
| Reviewer finds a Blocking issue | Fix on the same branch; re-run `reviewer` on the new diff before opening the PR |
| Engine gap sized L | `architect` plans it first; `engine-implementer` on `fable` |

## Cost and context guardrails

- Haiku never writes production code, tests, or reviews.
- Opus is used only by the agents and tiers above; the orchestrator never switches itself to Opus or Fable.
- Fable runs only at the 12 checkpoints, L-sized accounting-rule engine plans, and the last escalation step. Never per PR, never for security, never to write code.
- Subagents return summaries with file paths, not file dumps; `explorer` caps replies at about 300 words.
- `maxTurns` is set on every agent; hitting it counts as one failed attempt for escalation.
- At most 4 subagents run at once (`.claude/settings.json`), so parallel work stays reviewable.
- Each phase's STATE.md log entry lists the agents used and their models, for example `agents: scout/opus, tests/opus, architect/fable, impl/fable, review/fable, ui/opus`, so the owner can see where effort went.
