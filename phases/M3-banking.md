# M3 — Banking (P25–P27)

**Outcome:** owners import bank and card files, review and match them with rules (and optional AI), detect transfers, and reconcile with discrepancy reporting.

**Milestone exit:** all phases merged; **PR-5** process review (fable) with no unresolved Blocking items; milestone summary in STATE.md.

---

## P25 — Bank file import (QIF, QFX, OFX, CSV)

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | L | P12 | rust-implementer (opus); engine-implementer for U-06 | reviewer (opus), ui-verifier |

**Parallel:** may run alongside P19–P24 (disjoint crates). **Decisions:** Q-11, Q-154, Q-194–Q-197. **Engine:** U-06.

**Mockup:** `docs/design/mockups/ImportBankFile.html` and `docs/design/screenshots/ImportBankFile.png`.

**Crates and files:** `crates/cl-import/src/{lib.rs,detect.rs,ofx/{sgml.rs,xml.rs},qfx.rs,qif.rs,csv/{mapping.rs,profile.rs,dates.rs},dedupe.rs,normalize.rs}`, `crates/cl-import/fuzz/fuzz_targets/{ofx.rs,qif.rs,csv.rs}`, `crates/cl-ui/src/screens/import_bank_file.rs`, `fixtures/bank/*` (≥ 10 synthetic files).

**Steps**
1. Format detection and normalized row type (date, amount as exact decimal, payee/description, memo, check no., FITID, source line). — `feat(import): normalized rows`
2. OFX 1.x SGML and 2.x XML parsers; QFX; account and bank IDs extracted. — `feat(import): ofx and qfx`
3. QIF parser (bank and card types, date-format detection). — `feat(import): qif`
4. CSV mapping profiles (date, amount or debit/credit, description, check no., sign, date format); US MM/DD default with impossible-date flags (Q-195). — `feat(import): csv profiles`
5. Dedupe keys (FITID, else date+amount+description(+balance)). — `feat(import): dedupe keys`
6. **U-06** engine endpoint accepting a normalized batch into the review queue with server-side fuzzy duplicate rejection and source-file hash. — engine PR
7. Wizard: pick file → detect → link account (OFX account auto-link after first import, masked IDs, Q-194, Q-154) → card sign auto-detect and confirm once (Q-196) → preview with duplicate flags → confirm → show ending-balance difference when the file has one (Q-197). — `feat(import): wizard`
8. Fuzz targets per parser (CI smoke mode). — `test(import): fuzz targets`

**Tests:** fixture suite counts and sums; re-import adds zero rows; malformed files give line-numbered errors; fuzz smoke; ambiguous-date and impossible-date cases.

**Acceptance**
- [ ] ≥ 10 fixture files import with correct counts and sums.
- [ ] Re-importing the same file adds zero rows.
- [ ] Malformed files give line-numbered errors; fuzz smoke finds no crashes.
- [ ] Screen matches its mockup per docs/design/README.md (`ui-check --compare` reviewed by ui-verifier); any deviation is listed in the PR and QUESTIONS.md.

---

## P26 — Bank feed review

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | L | P25 | rust-implementer (sonnet) | reviewer (opus), ui-verifier |

**Decisions:** Q-73, Q-101–Q-103, Q-141, Q-151, Q-181. **Engine:** review queue, rules (conditions, tags, auto-accept flag), matching to existing transactions and Undeposited Funds groups, transfer detection, payroll expected withdrawals, AI categorization.

**Mockup:** `docs/design/mockups/BankFeed.html` and `docs/design/screenshots/BankFeed.png`.

**Crates and files:** `crates/cl-ui/src/screens/bank_feed/{view.rs,detail.rs,rules.rs,bulk.rs}`, `crates/cl-model/src/bank_feed.rs`.

**Steps**
1. Review screen per DESIGN.md with filters and bulk bar; keyboard triage (J/K, Space, A, S, R, Ctrl+Enter). — `feat(feed): review screen`
2. Rules: conditions on description, amount, account; set payee, account, tags (Q-181); one account per rule (Q-103); auto-accept off by default (Q-101); suggest a rule after repeated AI accepts (Q-141). — `feat(feed): rules`
3. Matching: existing transactions, Undeposited Funds groups (one deposit), payroll expected withdrawals (Q-151). — `feat(feed): matching`
4. Transfer detection between own accounts within a date window (Q-102). — `feat(feed): transfers`
5. AI badges only when enabled and online; accepted items post with source "Bank file import". — `feat(feed): ai badges and posting`

**Tests:** offline/AI-off categorizes by rules with no AI badges; matching two undeposited payments creates one deposit; transfer pair produces one transfer; auto-accept logged and reversible.

**Acceptance**
- [ ] Offline and AI off: rules work, no AI badges, no errors.
- [ ] Deposit matching clears both undeposited payments with one deposit.
- [ ] Screen matches its mockup per docs/design/README.md (`ui-check --compare` reviewed by ui-verifier); any deviation is listed in the PR and QUESTIONS.md.

---

## P27 — Reconcile

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | M | P12, P26 | rust-implementer (opus) | reviewer (opus), ui-verifier |

**Decisions:** Q-60, Q-100. **Engine:** reconciliation sessions, statement balances, discrepancy tracking, undo last reconciliation, card reconcile with pay step.

**Mockup:** `docs/design/mockups/Reconcile.html` and `docs/design/screenshots/Reconcile.png`.

**Crates and files:** `crates/cl-ui/src/screens/reconcile/{begin.rs,window.rs,reports.rs,undo.rs}`.

**Steps**
1. Begin: statement date, ending balance, service charge, interest; beginning-balance-changed warning. — `feat(reconcile): begin`
2. Reconcile window: deposits and payments panes; imported/accepted items pre-checked for review (Q-100); difference; hide after statement date; leave/finish. — `feat(reconcile): window`
3. Reports: summary and detail PDFs, previous reports, discrepancy report; undo last (Admin). — `feat(reconcile): reports and undo`
4. Card reconciliation with pay-now (check) or enter-bill step (Q-60). — `feat(reconcile): cards`

**Tests:** finish requires 0.00 difference; editing a reconciled transaction appears in the discrepancy report; undo restores cleared states exactly.

**Acceptance**
- [ ] Finish requires 0.00 (or a permitted adjustment).
- [ ] Reconciled-transaction edits appear in the discrepancy report.
- [ ] Screen matches its mockup per docs/design/README.md (`ui-check --compare` reviewed by ui-verifier); any deviation is listed in the PR and QUESTIONS.md.

### Checkpoint PR-5 — Bank lifecycle (fable)
File import → dedupe → rules and matching → transfers → accept/post → reconcile → discrepancies → undo. Adversarial cases: edited re-import, overlapping files, deleted matched transaction. Fix Blocking findings on `fix/PR-5`.
