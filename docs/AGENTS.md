# AGENTS.md — multi-agent build and model routing

How Claude Code builds ClientLedger with subagents and the right model for each job. Subagent definitions live in `.claude/agents/`. This file covers development agents only; the in-app AI features (P09) are unrelated.

## Usage rules (owner 2026-10-26, Q-301; these win over the sections below)

1. **One review per phase.** One `reviewer` (opus) covering code and security together, plus `ui-verifier` for screens. No separate plan reviews and no per-PR `security-reviewer`, except: Tier H phases and process checkpoints keep the `process-reviewer` (xhigh) plan review, and engine PRs are reviewed inside the phase's combined review.
2. **At most 2 implementers per phase**, each with a bigger, coherent work package. Fewer handoffs, cold builds and merge conflicts; no memory kills.
3. **Sonnet for routine coding:** control-channel scenarios, demo-backend parity, docs, engine pin bumps, CI fixes. Opus stays on money, posting, engine changes, security-sensitive code, plans and reviews.
4. **Lean orchestration:** plans ≤ 2,000 words; briefs name only the sections and files to read; agents never read whole plans or archives; the orchestrator reports to the owner at milestones or when the owner is needed, not after every event.
5. **Full local `cargo xtask ci` once per phase**, before the PR. Work packages run clippy and targeted tests only; GitHub CI covers the three OSes.
6. **Engine PRs merge when green** (CI passes, npm-audit excepted while it also fails on engine main) and the phase's combined review has no open findings on them. The owner gets a one-line summary.

## Principles

1. **Match the model to the risk, not the size.** Code that touches money, posting, concurrency, imports, migrations, or security gets Opus for planning and review even when the diff is small. Routine UI, plumbing, and docs use Sonnet. Haiku only searches and keeps records.
2. **The orchestrator stays light.** The main session reads STATE.md, picks the phase, delegates, checks results, and opens the pull request. It does not read large files itself; it asks `explorer`.
3. **Fresh eyes review.** `reviewer` and `security-reviewer` never see the implementation conversation, so their review isn't biased by it.
4. **Parallel where independent, sequential where coupled.** Fan out read-only work freely; run writers in parallel only in separate worktrees on disjoint files.
5. **Escalate, don't loop.** Two failed attempts at the same problem moves it up a tier.

## Models

| Model alias | Used by | Why |
| --- | --- | --- |
| `sonnet` | `explorer`, `bookkeeper`, `ci-fixer` (log triage and mechanical fixes; escalate to opus for logic changes), `docs-writer` | Purely mechanical work (owner, 2026-10-18); never production logic, money code or reviews |
| `opus` | default for all non-mechanical work: orchestrator (main session, unless the owner sets another with `/model`), `engine-scout`, `rust-implementer` (all tiers), `engine-implementer` (all sizes, including accounting-rule changes), `test-writer`, `ui-verifier`, `architect` (all tiers), `reviewer` (all tiers), `process-reviewer` (effort `xhigh` for Tier H phases and process checkpoints), `security-reviewer`; any money, posting, concurrency, data-movement, backup/restore or secret work package | Everything that is not purely mechanical (owner, 2026-10-22, Q-277) |
| `fable` | only when the owner explicitly asks for a specific task; never for security review | On-request only (Q-277) |

Owner routing (2026-10-22, Q-277): **Opus is the default for everything not purely mechanical; Sonnet for mechanical jobs; Fable only when the owner explicitly asks for a specific task.** Haiku is not used. Launch the main session on the model the owner chooses with `/model` (Opus by default). The orchestrator passes a per-invocation `model` when it starts `rust-implementer` or `engine-implementer`; that overrides the agent file's default (Claude Code resolves the invocation parameter first, then the frontmatter, then `CLAUDE_CODE_SUBAGENT_MODEL`, then the main model). Do not set `CLAUDE_CODE_SUBAGENT_MODEL_FORCE`, which would disable this routing.

**Effort:** `low` for search and records; `medium` for routine implementation and verification; `high` for architect, reviewers, and Tier H implementation; `xhigh` only for an escalated re-plan.

## Process reviews (process-reviewer)

`process-reviewer` runs on `opus` at effort `xhigh` at the checkpoints below, in addition to the per-phase `reviewer`. It traces a whole process across phases, checks accounting invariants, and builds adversarial scenarios. Blocking findings become fix work on a `fix/<checkpoint-id>` branch before the next milestone closes. Reports go to `target/reviews/<checkpoint-id>.md` and a summary goes into the STATE.md log.

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
| PR-A1 | After A05 (ADD-01; opus, xhigh) | Document to books: folder arrival → reading (text, OCR, template, optional Ollama) → checks and duplicates → draft → accept → bill or expense → Pay Bills or Bank Feed match → reconcile; inbox-folder migration; restore on another computer; hostile file |

Also use `process-reviewer` for the architect plan of any engine change sized L that touches posting, balances, inventory costing, sales tax, or tenant export/import.

Fable is used only when the owner explicitly asks for it on a specific task (Q-277); the orchestrator never selects it on its own.

## Phase tiers

| Tier | Phases | Implementer | Architect plan | Reviews |
| --- | --- | --- | --- | --- |
| **H** — money, posting, concurrency, data movement, migrations, core widget | P02, P05, P10, P12, P15, P21, P23, P24, P25, P27, P28, P31, P32, P33, P34, P36; post-beta P40, P42 | `opus`, effort high | required (`architect` on opus) | `reviewer` (opus) + `ui-verifier` (opus); `security-reviewer` (opus) where listed below |
| **M** — feature screens with accounting effects | P08, P09, P11, P13, P14, P17, P18, P20, P22, P26, P29, P30; post-beta P41, P43, P45, P46, P47, P48 | `opus`, effort medium; effort high for any work package touching money, posting, closing date, backup/restore or secrets | only if engine-scout reports an M/L gap | `reviewer` (opus) + `ui-verifier` |
| **L** — scaffolding, shell, printing, packaging, docs | P00, P01, P03, P04, P06, P07, P16, P19, P35, P37, P38, P39; post-beta P44 | `opus`, effort medium | no | `ui-verifier` for screens; `reviewer` optional |

`security-reviewer` (opus) also runs on P03, P04, P07, P09, P29, P32, P33, P35, P36, and P37, regardless of tier.

`security-reviewer` (opus) also runs on A01, A03, and A04 (ADD-01).

Addendum tiers (ADD-01): **H** A03 · **M** A01, A02, A04, A05; all on opus, stepping up to effort xhigh (and the escalation ladder) when needed (owner 2026-10-25).

Engine changes: `engine-implementer` runs on `opus` for every size, at effort high for M/L changes or anything touching posting, balances, inventory costing, sales tax math, closing dates, secrets, or tenant export/import.

## The loop for one phase

```
orchestrator (opus)
 ├─ in parallel ──────────────────────────────────────────────┐
 │   engine-scout (opus)        capability check              │
 │   test-writer (opus)         failing tests from criteria   │
 │   explorer ×N (sonnet)        locate code/patterns          │
 ├─ architect (opus)            plan → target/plans/Pxx.md    │ Tier H, or M with M/L gap
 ├─ engine-implementer          upstream changes first        │ if gaps
 ├─ rust-implementer            model by tier                 │ may split into work packages
 ├─ in parallel ──────────────────────────────────────────────┤
 │   reviewer (opus)      fresh-eyes diff review        │ also covers security on non-listed phases
 │   security-reviewer (opus)   listed phases only            │
 │   docs-writer (sonnet)       help pages                    │
 ├─ rust-implementer (fresh)    fix all findings, one brief   │ a new agent, not a resume
 ├─ ui-verifier (opus)          screens, once, after fixes    │
 ├─ reviewer re-check           only for Blocking findings in │ otherwise CI + named tests
 │                              money/data/secret code        │
 ├─ bookkeeper (sonnet)          STATE/QUESTIONS/UPSTREAM      │
 └─ PR → auto-merge on green CI; ci-fixer (sonnet) if red     ┘
```

Work packages: Tier H implementers take two or three plan steps per agent (token rule 4). When an architect plan lists independent packages (for example, a grid widget's editor types or several report types), the orchestrator may run up to 3 `rust-implementer` subagents at once, each with `isolation: worktree` (passed per invocation) on disjoint files, then merge their branches into the phase branch in plan order. Packages that touch the same crate's public API run sequentially.

## Parallel phases

The plan is sequential by default. The orchestrator may run **two phases at once** only when:
- both phases' "Depends on" lists are fully merged to `main`, and
- they touch disjoint crates and disjoint engine modules, and
- neither is Tier H with an open UPSTREAM item in the same engine area.

Each runs in its own worktree and branch, with its own PR. Examples that qualify: P02 ∥ P03, P19 ∥ P25, P23 ∥ P25, P30 ∥ P31. STATE.md shows both as `In progress`.

## Escalation

| Situation | Action |
| --- | --- |
| Opus implementer fails the same acceptance criterion twice, or `ci-fixer` fails the same error twice | Re-run that work package with a fresh `opus` implementer at effort `xhigh`, with the `architect` re-planning it first |
| Fresh opus (xhigh) fails twice | Mark only that item blocked in STATE.md, note that the owner may ask for Fable, and continue |
| Still failing (non-security work) | Ask `process-reviewer` (opus, xhigh) to diagnose the root cause; implement its fix on a fresh `opus` |
| Still failing | Mark only that item blocked in STATE.md, add a QUESTIONS.md row with a default, and continue other work (Q-220) |
| Reviewer finds a Blocking issue | Fix on the same branch with a fresh implementer. Re-run `reviewer` on the fix diff only when the Blocking item is in money, posting, closing-date, backup/restore, concurrency or secret handling; otherwise CI plus the tests named for each finding close it |
| Engine gap sized L | `architect` plans it first; `engine-implementer` on `opus`, effort high |

## Cost and context guardrails

Token rules (owner, 2026-10-18) — keep quality, cut waste:

1. **Fresh agents for fix passes.** After reviews, start a new implementer with one combined brief (every finding with file:line, the plan section, the tests to add). Do not resume the original implementer: its history is the largest cost in a phase.
2. **Small startup reads.** Agents read only what CLAUDE.md lists for their task: STATE.md's board and recent entries, the one phase section, the Q-IDs the phase cites (search `docs/history/phase-decisions.md`, never read it whole), U-rows only for engine work, and plan sections by line range. The bookkeeper keeps STATE.md to the last four log entries and moves a closed phase's `Q-Pxx` rows to the archive.
3. **One review round.** Tier L/M phases outside the security list get one `reviewer` that also covers security; listed phases add `security-reviewer`. The `ui-verifier` runs once, after the fix pass. A verification re-review runs only for Blocking findings in money, posting, closing dates, backup/restore, concurrency or secrets.
4. **Small work packages.** Tier H implementers take two or three plan steps per agent; the next package starts a fresh agent from the plan, not a resume. Agents commit after every step so a turn limit loses nothing.
5. **Quiet tools.** `cargo xtask ci` is quiet locally (one line per passing gate, only the explaining lines of a failure); use `--verbose` only to debug. `ui-verifier` opens the `--compare` page and the screenshots of failing or changed screens, not every PNG.
6. **Flaky tests fixed at once.** A test that fails once on CI and passes on re-run is fixed in the same PR (or the next commit to main), never just re-run twice.
7. **Briefs, not transcripts.** Scouts and reviewers write their findings to `target/plans/` or `target/reviews/`; later agents get the file path and the relevant section, not a re-run of the search.


- Sonnet never writes production logic, money code, tests of money code, or reviews.
- Fable is used only when the owner explicitly asks for a specific task, and never for security review (it carries extra cybersecurity safeguards); `security-reviewer` stays on opus.
- Subagents return summaries with file paths, not file dumps; `explorer` caps replies at about 300 words.
- `maxTurns` is set on every agent; hitting it counts as one failed attempt for escalation.
- At most 4 subagents run at once (`.claude/settings.json`), so parallel work stays reviewable.
- Each phase's STATE.md log entry lists the agents used and their models, for example `agents: scout/opus, tests/opus, architect/opus, impl/opus, review/opus, ui/opus`, so the owner can see where effort went.
