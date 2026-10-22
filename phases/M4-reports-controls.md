# M4 — Reports and controls (P28–P30)

**Outcome:** a full report engine with drill-down, budgets, and AI question-to-report; users, roles, closing date, sensitive accounts, audit trail; payroll import.

**Milestone exit:** all phases merged; **PR-6** process review (fable) after P28 with no unresolved Blocking items; milestone summary in STATE.md.

---

## P28 — Report engine UI

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | L | P12–P24 | rust-implementer (opus; work packages by report family) | reviewer (opus), ui-verifier |

**Decisions:** Q-44, Q-53, Q-65, Q-73, Q-104, Q-105, Q-107, Q-157, Q-172, Q-207, Q-270, Q-271. **Engine:** report endpoints (filters, columns by period/tag/client/job, basis, drill-down ids), budgets, duplicate finder query.

**Mockup:** `docs/design/mockups/Reports.html` and `docs/design/screenshots/Reports.png`.

**Crates and files:** `crates/cl-ui/src/screens/reports/{window.rs,filters.rs,columns.rs,drill.rs,memorized.rs,export.rs,ask.rs,budgets.rs,catalog.rs}`, `crates/cl-print/src/reports.rs`, `fixtures/golden/*.csv`.

**Steps**
1. Report window: date presets incl. 4-4-5; accrual/cash toggle; columns by period/tag/client/job; filters (account, name, tag, amount, cleared, type, adjusting flag Q-207); sort; collapse/expand. — `feat(reports): window`
2. Extra columns: % of income, prior period $ and %, YTD (Q-104); fixed aging buckets (Q-105). — `feat(reports): columns`
3. Drill-down to transaction detail and source form; sensitive accounts hidden by role (Q-157, enforced in P29). — `feat(reports): drill-down`
4. Memorized reports and groups; export Excel (permissive crate, numbers as numbers), CSV, PDF with full header (Q-107); print. — `feat(reports): memorize and export`
5. Beta catalog (work packages): P&L variants, Balance Sheet variants, Trial Balance, GL, Journal, Transaction Detail by Account, A/R and A/P agings, Open Invoices, Unpaid Bills, Sales by Client/Item, Expenses by Vendor, Sales Tax Liability, Inventory Valuation and Stock Status, 1099 Summary (Q-53), Audit Trail, Closing Date Exceptions, Duplicate Finder (Q-172). — `feat(reports): catalog`
5b. Tag rows under accounts (Q-271): a report option "Show tagged amounts separately" on account-based reports (P&L variants, Balance Sheet, Trial Balance, GL, Budget vs. Actual) with a tag-group picker. The account row (6100 - Repairs) shows only the **untagged** amount; each tag gets its own row with that tag's amount (6100-10 Repairs - Lake Area); an optional setting "Show a total for each account" adds a total row (Total 6100 - Repairs) equal to the untagged amount plus every tag row, exactly. Row label option: tag number appended to the account number (6100-10), prepended (10-6100) or not shown, separator "-" by default. Section and report totals are unchanged whichever way the rows are shown. Amounts come from the engine (tag-split report query; no UI arithmetic beyond display); drill-down from any row lists exactly its lines; memorized reports keep the options; Excel/CSV/PDF export the rows with the chosen codes. Engine: tag-split report query if the existing columns-by-tag path can't return it (just in time, Q-230). Tests: golden CSV with tag rows, with and without the account total; untagged + tag rows equal the account's full amount; append/prepend labels; drill-down sums. — `feat(reports): tag rows under accounts`
6. Simple P&L budgets and Budget vs. Actual (Q-65). — `feat(reports): budgets`
7. Ask-a-question (Q-73): AI maps a question to a catalog report + filters + dates, shows the mapping, runs it via the engine; never states numbers itself; works with Ollama. — `feat(reports): ask a question`

**Tests:** golden CSVs for the sample company (every report); drill-down sums; Excel number formats; budget fixture; ask-a-question mapping tests with a stubbed model, asserting the response contains no numbers not produced by the engine.

**Acceptance**
- [ ] Golden reports match to the penny.
- [ ] Drill-down from any P&L amount sums to that amount.
- [ ] Excel exports keep numbers numeric.
- [ ] Budget vs. Actual matches the hand-computed fixture.
- [ ] With tagged amounts shown separately, the account row shows the untagged amount, each tag has its own row labelled with the tag number appended or prepended as chosen, and the optional account total equals untagged + tag rows exactly.
- [ ] Screen matches its mockup per docs/design/README.md (`ui-check --compare` reviewed by ui-verifier); any deviation is listed in the PR and QUESTIONS.md.

### Checkpoint PR-6 — Reports (fable)
Every report ties to the trial balance; accrual vs. cash consistency; drill-down completeness; sensitive-account hiding; ask-a-question never fabricates numbers. Fix Blocking findings on `fix/PR-6`.

---

## P29 — Users, permissions, closing date, audit trail

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | L | P16 | rust-implementer (sonnet) | reviewer (opus), security-reviewer (opus), ui-verifier |

**Decisions:** Q-20, Q-69, Q-91, Q-154–Q-157, Q-210, Q-211, Q-276. **Engine:** users, roles and rights enforced server-side, password policy and lockout, recovery-key reset, closing date (Admin only), sensitive accounts, audit trail queries and export.

**Crates and files:** `crates/cl-ui/src/screens/{login.rs,users.rs,roles.rs,closing_date.rs,audit_trail.rs,recovery.rs}`, `crates/cl-model/src/permissions.rs`.

**Steps**
1. Login; optional single-user auto-login (Q-20); password policy 12+ characters, no expiry, lockout (Q-211); recovery-key reset flow (Q-210); optional idle auto-lock (Q-69). — `feat(auth): login and recovery`
2. Roles (Admin, Bookkeeper, A/R, A/P, View-only, custom) with per-area rights; UI hides and engine refuses. — `feat(auth): roles and rights`
3. Closing date and password, Admin only (Q-155); exceptions report. — `feat(close): closing date`
4. Sensitive accounts (Q-157) across registers, reports, drill-down; masked account numbers with logged reveals (Q-154). — `feat(auth): sensitive data`
5. Audit trail viewer (filters, before/after, source); export CSV/PDF for Admins (Q-156); kept forever (Q-91). — `feat(audit): viewer and export`
6. Company tag options (Q-276), in Company › Preferences (Admin only, audited): **Require a tag on every line** — every form, Bulk Entry, template "Use" and in-app import refuses to save a transaction with an untagged line, naming the lines in plain words; the header tag still fills empty lines (Q-178), so one header tag satisfies it. **Default tag for new transactions** — prefilled as the header tag on every new form and new Bulk Entry section (owner can change it per transaction); not applied to edits of saved transactions. App-side only (Q-276): postings that don't go through ClientLedger's forms (e.g. the MyBooks web) are not checked. Works with required tag groups (Q-180). Tests: each form refuses an untagged line with the option on and saves with it off; default tag prefilled on new forms only; Bulk Entry and templates honour both. — `feat(settings): company tag options`

**Tests:** role enforcement both in UI and engine; lockout; recovery key works once and is invalidated; sensitive account hidden from Bookkeeper everywhere; closed-period edits in exceptions report.

**Acceptance**
- [ ] A/P role blocked from invoices and restricted reports (UI and engine).
- [ ] Every closed-period edit appears in the exceptions report.
- [ ] Sensitive payroll account hidden from Bookkeeper in P&L, register, drill-down.
- [ ] With "Require a tag on every line" on, no form or Bulk Entry can save an untagged line; the default tag prefills new transactions.

---

## P30 — Payroll import

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | M | P15 | rust-implementer (sonnet) | reviewer (opus), ui-verifier |

**Parallel:** may run alongside P31. **Decisions:** Q-150–Q-153. **Engine:** existing payroll import module; expected withdrawals for matching; generic CSV profiles.

**Crates and files:** `crates/cl-ui/src/screens/payroll_import.rs`, `fixtures/payroll/*`.

**Steps**
1. Wizard: provider (Gusto, ADP Run, QuickBooks Payroll (export file), Paychex, Square, OnPay, Rippling, or generic CSV Q-153), file, pay period, mapping saved per provider, departments → tags (Q-152), preview, post. — `feat(payroll): import wizard`
2. Summary journal entry per pay date, no employee names (Q-150); duplicate pay-period detection. — `feat(payroll): summary posting`
3. Expected withdrawals for bank-feed matching (Q-151). — `feat(payroll): expected withdrawals`

**Tests:** provider fixtures balance and match totals; generic CSV profile reused next period; bank-file withdrawals match expected.

**Acceptance**
- [ ] Each provider fixture posts balanced entries matching totals.
- [ ] Generic CSV re-imports without remapping.
- [ ] Imported bank withdrawals match expected payroll withdrawals.
