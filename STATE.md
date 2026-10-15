# STATE.md — ClientLedger

**Current phase:** P01 — Engine submodule and single-executable build
**Status:** Next (P03 may run in parallel)
**Engine commit:** tracks `main`; not pinned yet (Q-02)
**Last updated:** 2026-10-15
**Tiers:** H = opus implementer + architect + opus review · M = sonnet implementer + opus review · L = sonnet (see docs/AGENTS.md)

## Phase board — beta track

| Phase | Title | Tier | Status |
| --- | --- | --- | --- |
| P00 | Repository bootstrap | L | Done 2026-10-15 |
| P01 | Engine submodule and single-executable build | L | Next |
| P02 | Engine desktop profile (upstream) | H | |
| P03 | Bundled PostgreSQL manager | L | |
| P04 | Supervisor | L | |
| P05 | Engine API client | H | |
| P06 | Application shell | L | |
| P07 | Automation and control channel | L | |
| P08 | Company lifecycle | M | |
| P09 | Preferences and AI provider keys | M | |
| P10 | Ledger grid widget | H | |
| P11 | Lists, centers, and tags | M | |
| P12 | Bank and account register | H | |
| P13 | Write Check | M | |
| P14 | Deposits, transfers, sales receipts, card charges, owner and loan helpers | M | |
| P15 | Journal entries, memorized and recurring | H | |
| P16 | Home workspace | L | |
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
| P31 | QuickBooks Desktop import with tie-out, and spreadsheet import | H | |
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

None yet. Format: `Pxx — blocked by U-xx/Q-xx — what is pending — workaround in place`.

## Placeholders tracked

Screens or actions that link to a later phase. Remove each line when its phase lands.

- (none yet)

## Log

Newest first. One entry per completed phase: date, phase, summary, acceptance checklist results, notable decisions (Q-xx / U-xx).

- 2026-10-15 — P00 complete (PR #1, squash-merged). Workspace skeleton (13 crates, 2 apps, xtask), MIT/NOTICE/community files, deny.toml, `xtask` license + layering + ci gates, three-OS CI with secret scan and report-only coverage. Acceptance: `cargo xtask ci` green on Ubuntu, Windows, macOS arm64 + x64; GPL dep, direct BSD-only dep, and a layering violation each failed their gate (verified once, reverted); gitleaks scan clean. Decisions: Q-P00a–c. Agents: rust-implementer (sonnet), ci-fixer (sonnet; clippy `assert_is_empty` on newer CI toolchain). Not enforced: the rule that only cl-pg/cl-host/cl-company spawn processes (not visible in the dependency graph). Follow-up: consider pinning rust-toolchain.toml to CI's clippy version. Owner enabled branch protection (required: ci x4 + secret scan), auto-merge, and secret scanning with push protection.
- 2026-10-15 — UI mockups added as the visual reference (Q-226–Q-228): 15 screens in docs/design (HTML + PNG + index); each screen phase names its mockup and gains a mockup-match acceptance item; ui-check gets `--compare`; ui-verifier reviews against mockups; sample company reproduces mockup figures; canvas-example tests now use the mockup numbers.
- 2026-10-11 — Detailed build plan written: PHASES.md is now an index; phases/M0–M7 hold per-phase decisions, engine items, crates and files, ordered commit-sized steps, tests, acceptance, and Fable checkpoints.
- 2026-10-11 — Added process-reviewer on Fable: 12 checkpoints (PR-P05, PR-P10, PR-1…PR-10), deep end-to-end reviews; security stays on Opus.
- 2026-10-11 — Multi-agent build added: 12 subagents in .claude/agents/, model routing by phase tier (docs/AGENTS.md), orchestrator on Sonnet.
- 2026-10-11 — Q&A rounds 44–48 (final): deposits, journal entries, chart of accounts, passwords and recovery key, check stubs, updates, backup reminder, QBD versions, sample size, large engine gaps. Q&A complete; only the attorney review of the grant remains.
- 2026-10-11 — Q&A round 43: service dates on lines, no line discounts, plain multi-line descriptions, hide-$0-lines option.
- 2026-10-11 — Q&A round 42: OFX account auto-link, MM/DD default, card sign auto-detect, file balance check.
- 2026-10-11 — Q&A round 41: Enter = Save & New, payee prefill without amounts, vendor default accounts, optional memos.
- 2026-10-11 — Q&A round 40: register defaults, true running balance always shown, state row colors, four quick filters.
- 2026-10-11 — Q&A round 39: one company at a time, File > Open switching, company color stripe, guarded company delete.
- 2026-10-11 — Q&A round 38: tags at transaction and line level, one per group, optional required groups, rules set tags.
- 2026-10-11 — Q&A round 37: owner-run finance charges with preview, open-item statements, open-balance batches, aging row.
- 2026-10-11 — Q&A round 36: Verify Data tool, weekly quick health check, duplicate finder, no condense.
- 2026-10-11 — Q&A round 35: no bill approvals, Positive Pay post-beta, guided vendor statement match, 7-day bills-due on Home.
- 2026-10-11 — Q&A round 34: no dunning, editable email templates, invoice PDF only, sent tracking.
- 2026-10-11 — Q&A round 33: minimum hardware 4 GB/2 cores/2 GB, offline bundled help, videos post-beta, shortcut PDF + overlay.
- 2026-10-11 — Q&A round 32: masked account numbers, admin-only closing date, audit trail export, sensitive account flag.
- 2026-10-11 — Q&A round 31: payroll summary entries, expected-withdrawal matching, departments to tags, generic payroll CSV.
- 2026-10-11 — Q&A round 30: no sales tax reminders, per-invoice rounding, negative inventory warn-and-allow, reorder alerts.
- 2026-10-11 — Q&A round 29: QBD import history depth by owner choice, no memorized or user import, guided spot-check list.
- 2026-10-11 — Q&A round 28: AI usage meter + cap, redacted minimal data, per-feature toggles, rule suggestions from accepts.
- 2026-10-11 — Q&A round 27: installer license acceptance, privacy notice, recorded AI consent, scope note in About/docs.
- 2026-10-11 — Grant placeholders filled: Kisaes LLC (Missouri) dba KisaesDevLab, Missouri law, no venue clause, effective on commit, licensing@kisaes.com, GitHub org KisaesDevLab. Engine repo: KisaesDevLab/vibe-mybooks.
- 2026-10-11 — Engine grant drafted (docs/legal/LICENSE-CLIENTLEDGER.draft.md) for attorney review: own-organization use only; official releases only.
- 2026-10-11 — Q&A round 26: multiple ship-to addresses, contact people with roles, notes and to-dos, no credit limits.
- 2026-10-11 — Q&A round 25: no other-app imports in beta, CSV transaction import, IIF export, New from existing.
- 2026-10-11 — Q&A round 24: desktop app owns the ClientLedger name and site, GitHub Releases downloads, drafted icon options, app name ClientLedger.
- 2026-10-11 — Q&A round 23: 80% workspace coverage, Rust 2024, 30-day/100 MB logs, plain errors with Copy details.
- 2026-10-11 — Q&A round 22: continuous runs, branch + PR per phase with auto-merge, Linux CI per push and all OSes on PR/nightly.
- 2026-10-11 — Q&A round 21: optional host service, LAN only (no remote), no work while host is down, 5 concurrent users.
- 2026-10-11 — Q&A round 20: report columns, fixed aging buckets, Home charts, full report headers.
- 2026-10-11 — Q&A round 19: reconcile pre-check from imports, optional auto-accept rules, transfer detection, no split rules.
- 2026-10-11 — Q&A round 18: UOM labels only, configurable date warnings, no spell check, two-sided items.
- 2026-10-11 — Q&A round 17: logo on built-in template, #10 window envelopes, prefixed auto-numbering, payment-instructions footer.
- 2026-10-11 — Q&A round 16: refund methods, exemption certificates, job status/type/dates, audit trail kept forever.
- 2026-10-11 — Q&A round 15: manual processor deposits, personal-funds expenses, daily sales summary, guided vendor refunds.
- 2026-10-11 — Q&A round 14: customer early-pay discounts, customer deposits as liability, bounced-check action in beta; retainage post-beta.
- 2026-10-11 — Q&A round 13: no tax-line mapping, contacts can be client and vendor, no Other Names list, account numbers on by default.
- 2026-10-11 — Q&A round 12: private-fixtures folder, four AI features incl. ask-a-question reports, MyBooks-in-Docker transfer CI, staged beta with own clients first.
- 2026-10-11 — Q&A round 11: no pay links, optional auto-lock, native macOS menu, in-app notifications only.
- 2026-10-11 — Q&A round 10: simple P&L budgets and app zoom in beta; cash flow statement and calendar post-beta (new P48).
- 2026-10-11 — Q&A round 9: full card flow, owner pay helper, loan split helper, Home setup checklist.
- 2026-10-11 — Q&A round 8: Export All Data, tabs with pop-out windows, discount/price-level/subtotal items, form-level undo.
- 2026-10-11 — Q&A round 7: recurring invoices post-beta, 1099 flag + summary in beta, receipt inbox folder, confirm-after-print check numbering.
- 2026-10-11 — Q&A round 6: automatic year-end, four starter charts of accounts, 10 MB attachments, 10-company target.
- 2026-10-11 — Q&A round 5: deletes allowed and logged, US only, local data with backups anywhere, versioned transfer files, accrual default, guided opening balances, SemVer, GitHub support.
- 2026-10-11 — Q&A round 4: Q-21–Q-23 settled for post-beta; engine repo public before P01. Only Q-35 (grant text, owner) remains.
- 2026-10-11 — Q&A round 3: PDFium viewer, PDF + OS print, branding confirmed. Open: Q-35 (engine grant, STOP before P39); Q-21–Q-23 deferred to post-beta.
- 2026-10-11 — Q&A round 2: engine tracks main; companies stay engine tenants; parsers in Rust; AI providers Anthropic, OpenAI, Ollama; SMTP email; owner merges engine PRs.
- 2026-10-11 — Plan revised after owner Q&A: free MIT public repo, no licensing, owner audience, beta scope set, Rust/egui native UI confirmed. Next: P00.
- 2026-10-11 — Plan created.
