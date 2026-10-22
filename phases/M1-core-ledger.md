# M1 — Core ledger (P10–P16)

**Outcome:** a QBD switcher can set up lists and tags, work a two-line register, write checks, make deposits and transfers, record card charges and owner transactions, post journal entries and templates (recurring transactions), and start from Home.

**Milestone exit:** all phases merged; **PR-1** process review (fable) of the core ledger with no unresolved Blocking items; milestone summary in STATE.md. **PR-P10** reviews the grid plan inside P10.

Common to every screen phase: test-writer writes the scenario YAML and unit tests first; ui-verifier checks light/dark screenshots, 200% zoom, and AccessKit labels before the PR; docs-writer drafts the help page.

---

## P10 — Ledger grid widget (`cl-grid`)

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | L | P06, P07 | rust-implementer (opus, effort high) | reviewer (opus); **PR-P10** process-reviewer (fable) on the plan |

**Agents:** architect (opus) → **process-reviewer (fable) plan review** → test-writer → rust-implementer in up to 3 work packages (core/virtualization, editors, keyboard and a11y) → reviewer → ui-verifier.

**Decisions:** Q-12, Q-59, Q-188.

**Crates and files:** `crates/cl-grid/src/{lib.rs,model.rs,layout.rs,virtual_scroll.rs,state.rs,keys.rs,a11y.rs,editors/{text.rs,memo.rs,amount.rs,calc.rs,date.rs,picker.rs,tags.rs,checkbox.rs,dropdown.rs}}`, `crates/cl-grid/benches/scroll.rs`, `crates/cl-automation/scenarios/grid_demo.yaml`, a demo screen behind a dev flag.

**Steps**
1. Spike (throwaway branch allowed): 200k rows with two-line layout in egui; measure frame time; pick the virtualization approach; record results in the plan. — `spike(grid): virtualization`
2. `RowModel` trait: columns, cell kinds, row height mode, editability, validation hook, row state (locked, cleared, reconciled, dirty, error, needs review). — `feat(grid): row model`
3. Virtualized rendering, sticky header, column resize persisted per screen, subtle state colors (Q-188). — `feat(grid): rendering`
4. Editing state machine: cell → row → commit; Esc reverts cell then row; Enter records row; Tab/Shift+Tab traversal; Ctrl+Insert/Delete line; arrows, Page Up/Down, Ctrl+Home/End; clipboard for cell text. — `feat(grid): editing and keys`
5. Editors: amount (exact decimal; `1,234.56`; minus or parentheses; calculator `+ - * /` evaluated exactly, Q-12), date (+ − T M H Y R W K), pickers with type-ahead and inline "Add new", tag chips honoring one-per-group (Q-179), checkbox, dropdown, memo. — `feat(grid): editors`
6. AccessKit roles for grid, row, cell; labels for every editor. — `feat(grid): accessibility`
7. Benchmark and demo scenario. — `test(grid): benchmark and scenario`

**Tests:** proptest for amount parsing and calculator (exactness, rejection messages); date-key tests including month/year edges and leap years; editing state-machine tests; scenario entering a full split line by keyboard only; scroll benchmark recorded in the PR.

**Acceptance**
- [ ] 200k rows scroll at ≥ 60 fps on CI-class hardware.
- [ ] Keyboard-only scenario enters a split line with autofill and a tag; model values exact.
- [ ] Amount parser and calculator property tests pass.
- [ ] PR-P10 report has no unresolved Blocking items.

---

## P11 — Lists, centers, and tags

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | L | P10 | rust-implementer (sonnet) | reviewer (opus), ui-verifier |

**Decisions:** Q-53, Q-58, Q-76–Q-79, Q-90, Q-96, Q-99, Q-128–Q-131, Q-154, Q-178–Q-181, Q-192, Q-208, Q-209. **Engine:** capability check for accounts (hierarchy, unique numbers, sensitive flag), contacts with both roles, ship-to addresses, contact people, notes and to-dos, jobs with status/type/dates, items (two-sided, UOM label, discount, subtotal), price levels, tags with groups and required rules, terms, payment methods, 1099 fields. Gaps → UPSTREAM rows.

**Crates and files:** `crates/cl-ui/src/screens/lists/{accounts.rs,clients.rs,vendors.rs,items.rs,tags.rs,terms.rs,payment_methods.rs,price_levels.rs,detail_pane.rs,csv_import.rs}`, `crates/cl-model/src/lists/*.rs`, scenarios per list.

**Steps**
1. Chart of Accounts: hierarchy (depth per engine, Q-208), unique numbers (Q-209) shown by preference (Q-79), types, active/inactive, balances, Account Report (Q-254), add/edit, merge if supported; masked bank/card numbers with logged Admin reveal (Q-154); sensitive flag (used in P29). — `feat(lists): chart of accounts`
2. Contacts: one record with client and/or vendor roles (Q-77); multiple ship-to addresses with default (Q-128); contact people with roles and invoice/statement recipients (Q-129); dated notes and to-dos (Q-130); no credit limits (Q-131); vendor default expense account (Q-192); vendor 1099 fields (Q-53). — `feat(lists): contacts`
3. Customers & Jobs with jobs (status, type, dates, Q-90) and Vendors (Q-258): list + detail panes (open balance, recent transactions, notes, to-dos, attachments). — `feat(lists): centers`
4. Items: service, non-inventory, other charge, discount (percent/amount), subtotal; two-sided purchase/sales fields (Q-99); UOM label (Q-96); price levels (Q-58). Inventory parts arrive in P24. — `feat(lists): items and price levels`
5. Tags: groups, one-per-group rule, required groups by account type, transaction- and line-level application rules exposed to forms (Q-178–Q-181); rename/merge; usage counts. — `feat(lists): tags`
6. Terms (incl. discount terms), payment methods; CSV import/export per list with mapping. — `feat(lists): terms, methods, csv`
7. Class → tag mapping rule shared with P31 (`Parent:Child` → group + tag). — `feat(model): class-to-tag rule`

**Tests:** keyboard-only create/edit/inactivate per list; unique account number rejection; contact with both roles appears in both centers; one-per-group and required-group validation; class→tag mapping; 20k-record filter benchmark.

**Acceptance**
- [ ] Keyboard-only create/edit/inactivate for every list.
- [ ] Classes in fixtures become tags (groups for `Parent:Child`).
- [ ] 20k-record lists filter under 100 ms.

---

## P12 — Bank and account register

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | L | P10, P11 | rust-implementer (opus) | reviewer (opus), ui-verifier |

**Decisions:** Q-40, Q-186–Q-189, Q-206. **Engine:** register query with true running balance independent of filters, cleared/reconciled status, version for optimistic concurrency, delete with before-image, void.

**Mockup:** `docs/design/mockups/Register.html` and `docs/design/screenshots/Register.png`.

**Crates and files:** `crates/cl-ui/src/screens/register/{view.rs,entry_row.rs,filters.rs,footer.rs}`, `crates/cl-model/src/register.rs`, `crates/cl-model/src/closing.rs`.

**Steps**
1. Register view on `cl-grid`: two-line rows (date · number/type · payee/account·memo · payment · ✓ · deposit · balance); one-line toggle; opens date ascending, last 90 days, range remembered per account (Q-186). — `feat(register): view`
2. Balance column always shows the engine's true per-transaction balance in date order, even when filtered or sorted (Q-187). — `feat(register): true balance`
3. Entry row for simple checks/deposits/transfers; Splits opens the full form; Record with Enter. — `feat(register): entry row`
4. Quick filters: uncleared, payee, tag, amount range (Q-189); Go To; Account Report (Q-254); Edit Transaction. — `feat(register): filters and navigation`
5. Closed-period lock and password flow (shared `cl-model::closing`); Delete and Void everywhere with confirmation, before-image in audit trail (Q-40). — `feat(register): lock, delete, void`
6. Footer: cleared and ending balances.

**Tests:** property test — 1,000 random edits, then register balance = engine account balance; filtered view balances equal unfiltered values per row; closed-period edit refused without password; delete recorded with full lines; scenario for entry row.

**Acceptance**
- [ ] Balance equals the engine account balance after 1,000 random edits.
- [ ] Closed-period edit without the password is refused clearly.
- [ ] Deleted transactions appear in the audit trail with all original lines.
- [ ] Screen matches its mockup per docs/design/README.md (`ui-check --compare` reviewed by ui-verifier); any deviation is listed in the PR and QUESTIONS.md.

---

## P13 — Write Check

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | M | P12 | rust-implementer (sonnet) | reviewer (opus), ui-verifier |

**Decisions:** Q-10, Q-73, Q-85, Q-190–Q-193, Q-212. **Engine:** checks with expense/item lines, billable, tags, attachments, templates (Q-256); rules-based and AI suggestions.

**Mockup:** `docs/design/mockups/WriteCheck.html` and `docs/design/screenshots/WriteCheck.png`.

**Crates and files:** `crates/cl-ui/src/screens/write_check/{form.rs,payee_panel.rs}`, `crates/cl-model/src/forms/check.rs`, `crates/cl-automation/scenarios/write_check.yaml`.

**Steps**
1. Form per DESIGN.md: account + ending balance, number, date, payee (autofill + address), amount + words line, memo, Expenses/Items split grid (account, amount, memo, customer / job, billable, tags). — `feat(check): form`
2. Payee prefill from last transaction except amounts (Q-191); vendor default account (Q-192); header tags default to lines (Q-178). — `feat(check): prefill`
3. Payee panel: open-bill warning ("use Pay Bills instead"), recent checks. — `feat(check): payee panel`
4. Print later, attach, Save as Template (P15, Q-249), history, Prev/Next; no Pay online in beta (Q-10). — `feat(check): toolbar`
5. Suggestions: rule match offline; AI chip only when enabled and online; explicit accept; accepts counted toward rule offers (Q-141). — `feat(check): suggestions`

**Tests:** mockup example scenario (check 4418 to Ozark Feed & Seed, 1,286.40 split 842.15 + 444.25, open-bill warning shown); GL effect test; lines-vs-amount validation; AI-off scenario shows no AI chip.

**Acceptance**
- [ ] The mockup's Write Check example saves; register shows it; GL correct.
- [ ] No save when lines and amount disagree.
- [ ] Screen matches its mockup per docs/design/README.md (`ui-check --compare` reviewed by ui-verifier); any deviation is listed in the PR and QUESTIONS.md.

---

## P14 — Deposits, transfers, sales receipts, card charges, owner and loan helpers

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | L | P13 | rust-implementer (sonnet; work packages allowed) | reviewer (opus), ui-verifier |

**Decisions:** Q-60–Q-62, Q-84–Q-87, Q-202–Q-205. **Engine:** undeposited funds, deposits with cash back and negative lines, transfers, sales receipts, card charges/credits, daily sales summary with over/short, vendor refunds, owner equity defaults by entity type.

**Crates and files:** `crates/cl-ui/src/screens/{deposit.rs,transfer.rs,sales_receipt.rs,card_charge.rs,owner_money.rs,personal_expense.rs,daily_sales.rs,loan_payment.rs}`, matching `cl-model/src/forms/*.rs`, scenarios.

**Steps (work packages A–C may run in parallel worktrees)**
- **A:** Make Deposit (from Undeposited Funds; other lines including negative fee lines; cash back to a chosen account, Q-205; suggested groupings for pending imported deposits; deposit summary printout, Q-203; "Refund from a vendor", Q-87); Transfer Funds. — `feat(banking): deposit and transfer`
- **B:** Sales Receipt (defaults to Undeposited Funds, Q-202); Daily sales summary with tenders and over/short (Q-86, Q-204), can be saved as a template (Q-256). — `feat(sales): sales receipt and daily summary`
- **C:** Record Card Charge/Credit (Q-60); "Pay yourself / Put money in" by entity type (Q-61); "Paid with personal funds" (Q-85); Loan payment helper (Q-62). — `feat(money): card and owner helpers`

**Tests:** deposit grouping clears Undeposited Funds exactly; transfer appears in both registers; loan split matches lender fixture; over/short posts correctly; personal-funds default by entity type; scenarios per form.

**Acceptance**
- [ ] Two grouped payments equal the bank deposit and clear Undeposited Funds.
- [ ] Transfers match in both registers.
- [ ] Loan payment splits exactly; balance matches the lender fixture.

---

## P15 — Journal entries, templates and recurring transactions

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | M | P12 | rust-implementer (opus) | reviewer (opus), ui-verifier |

**Decisions:** Q-52, Q-181, Q-206, Q-207. **Engine:** journal entries (A/R or A/P line with name, one per entry; adjusting flag; reversing), memorized transactions (owner-facing name: Templates, Q-256) with schedules on pg-boss, missed-run catch-up idempotency.

**Crates and files:** `crates/cl-ui/src/screens/{journal_entry.rs,templates.rs}`, `crates/cl-model/src/forms/journal.rs`, `crates/cl-model/src/templates.rs`.

**Steps**
1. Make Journal Entry: debit/credit grid, names, tags, autobalance helper, numbering, reversing option, adjusting flag (Q-207), A/R or A/P rule (Q-206). — `feat(je): form`
2. "Save as Template" from checks, bills, deposits, journal entries (Q-256, Q-249); a Templates list; remind / enter automatically / never; groups; next date; tags carried (Q-181); no recurring invoices (Q-52). — `feat(templates): list and schedules`
3. Catch-up: on launch, post missed occurrences once each and show a summary. — `feat(templates): catch-up`

**Tests:** unbalanced entry refused; A/R line without a name refused; two A/R accounts refused; catch-up idempotency across restarts (simulated clock); reversing entry dates.

**Acceptance**
- [ ] Unbalanced entries cannot save.
- [ ] Missed monthly occurrences post exactly once each.

---

## P15A — Bulk Entry

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | M | P13–P15 (and P10 grid) | rust-implementer (opus) | reviewer (opus), ui-verifier |

**Decisions:** Q-252, Q-256, Q-232, Q-233, Q-238, Q-250. Screen title "Bulk Entry" (Q-256). **Engine:** `/api/v1/batch` (validate, save, suggest-account, parse-csv). Run the capability check first. Invoice, bill, credit memo and customer payment rows may need items (P20) or A/R-A/P rules from P17/P21: rows of types whose forms are not built yet are offered only once their phase is done (record this as a default in QUESTIONS.md when the phase starts).

**Mockup:** none. Follow the grid rules in `docs/DESIGN.md`; note "no mockup" in the PR.

**Crates and files:** `crates/cl-ui/src/screens/batch_entry/`, `crates/cl-model/src/forms/batch.rs`, `crates/cl-automation/scenarios/batch_entry.yaml`.

**Steps**
1. One grid per transaction type with a context account; row validation through `/batch/validate` showing problems per row; nothing is saved until all rows pass. — `feat(batch): grid and validation`
2. Paste from a spreadsheet or CSV via `parse-csv` with column mapping; account suggestions need an accept (AI rules Q-73/Q-141 gating as in P13). — `feat(batch): paste and suggestions`
3. Save all in one call; closing-date password flow; summary; rows appear in the registers. — `feat(batch): save`

**Tests:** a 50-row batch saves exactly (sum of rows equals the register change); one bad row blocks the save and is highlighted; pasted CSV amounts round to the cent (Q-232); a closed-period row asks for the password.

**Acceptance**
- [ ] A mixed batch of checks and deposits saves and every row shows in the register with the right balance.
- [ ] Invalid rows cannot save.
- [ ] Screen verified with a control-channel scenario.

---

## P16 — Home workspace

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| L | M | P13–P15 | rust-implementer (sonnet) | ui-verifier |

**Decisions:** Q-63, Q-71, Q-106, Q-130, Q-149, Q-169, Q-171, Q-217, Q-264, Q-265, Q-266, Q-267, Q-270, Q-272, Q-273, Q-274.

**Mockup:** `docs/design/mockups/Home.html` and `docs/design/screenshots/Home.png` (Home content).

**Crates and files:** `crates/cl-ui/src/screens/home/{lanes.rs,balances.rs,attention.rs,charts.rs,checklist.rs}`.

**Steps**
0. Enter-key preference (Q-264), first: Preferences gets "Enter moves between fields" (off by default). When on, Enter moves to the next field or grid cell like Tab (Shift+Enter moves back; in a grid's last cell it starts a new line) and Ctrl+Enter does Save & New on every form and in Bulk Entry; Alt+S stays Save & Close. When off, today's behavior stays (Enter = Save & New in header fields, Q-190; Enter records a grid row). One switch in the `cl-model::keymap` / form key handling, shown in menus, tooltips, the status bar and KEYBOARD.md; travels in preferences backups. Tests: unit tests for both modes; one form scenario run with the option on. — `feat(forms): Enter moves between fields option`
0b. Persistent headers (Q-265): on every screen the title, toolbar/actions, filters and grid column headers stay fixed while only the content scrolls — list and center screens (Customers & Jobs, Vendors, Chart of Accounts, Items, Tags, Templates), registers, Bulk Entry, Templates Due and every form (header card and Save buttons stay reachable; the lines grid and side panel scroll). One shared screen-layout helper (fixed top and bottom bars, scrolling body) used by all screens; at 960×640 and 200% zoom the fixed parts wrap, and if they would take more than half the window the body still gets at least a few rows. Tests: a scenario per screen kind scrolls to the bottom and asserts the actions are still visible and clickable; forms-zoom extended. — `feat(ui): persistent headers on every screen`
0c. Collapsible detail panel (Q-266): on Customers & Jobs and Vendors, the detailed side panel can be collapsed to a thin edge strip and expanded again (button on the panel edge, keyboard shortcut registered in `cl-model::keymap`, AccessKit label "Show details"/"Hide details"); the list takes the freed width. The choice is remembered per screen in UI preferences and restored on reopen. Built as a shared panel helper so other screens with side panels (Write Check payee panel, registers) can use it later. Tests: unit test for the remembered state; scenario collapses, reopens the screen, sees it still collapsed, expands. — `feat(ui): collapsible detail panel on Customers and Vendors`
0d. Condensed entry layout on wide screens (Q-267): Daily Sales Summary first, then every form with label / account / amount rows (Pay Yourself, Paid with Personal Funds, Loan Payment, Make Deposit's summary). Rows become a compact table with a maximum width (about 760 px, centered or left-aligned with the header card): label left-aligned, account next to it in muted text, amount box right next to them; zebra striping and a hover/focus highlight on the whole row so the eye follows label → amount; section headings span the table; totals and over/short sit in the amount column, right-aligned in the figure font. The header card and buttons use the same width. At narrow widths it behaves as today. Tests: draw tests at 1440 and 1920 wide check the amount box is within the max width of its label; daily_sales scenario screenshots at 1920×1080. — `feat(ui): condensed entry rows on wide screens`
0e. Tag numbers (Q-270): tags get an optional number like accounts. Engine (just in time, Q-230): `tags.tag_number` (text ≤ 20, unique per company when set, like account numbers Q-209), accepted and returned by the tag routes, carried in tenant export/import and the transfer file. UI: Number column in the Tags list and tag form; tags shown as "number · name" wherever a tag is picked or displayed (tag cells, header tags, filters, reports, Bulk Entry), sorted by number then name; typing a number in a tag field finds the tag; duplicates refused in plain words. Tests: contract test for create/edit/duplicate; tag picker sorts and finds by number; tags scenario. — `feat(tags): tag numbers`
0f. Bulk Entry "Copy down" (Q-272): right-click a cell (or Ctrl+D on the focused cell, registered in `cl-model::keymap`) → **Copy down**: every displayed row in that section gets the same value in that column as the row you started from (date, payee/name, account, memo, tag; also Print later). Not offered for Amount, Debit/Credit or check numbers (numbers must stay unique). Empty rows are filled too only if they already have an amount (no new rows are made). One undo step reverts it; changed rows lose their engine check and are re-checked. A second menu item **Copy to rows below** limits it to rows after the clicked one. Context menu also gets AccessKit labels; the grid's right-click menu is a shared cl-grid feature other grids can opt into later. Tests: unit tests for each column kind, undo, re-check; batch_entry scenario step. — `feat(batch): copy down`
0g. Bulk Entry column chooser (Q-273): a "Columns…" button (and right-click on a column header) opens a list of the section type's columns with show/hide checkboxes and move up/down (drag or Alt+↑/↓). Columns required to save that type (e.g. date, account, amount; payee for checks; debit/credit for journal entries — taken from the cl-model batch rules, not hard-coded in the UI) are always shown: their checkbox is ticked and disabled with a tooltip saying why. Order and visibility are remembered per company and section type in UI preferences; "Reset to default" restores them. Hidden columns keep their values (a pasted or copied value in a hidden column still saves); paste/CSV column mapping and Copy down follow the visible order. Tests: required columns can't be hidden; order persists across reopen; hidden values still save; batch_entry scenario step. — `feat(batch): choose and order columns`
0h. Bulk Entry multi-row selection and bulk actions (Q-274): select rows with a checkbox column, Shift+click / Shift+↑↓ for ranges, Ctrl+click to add or remove, Ctrl+A for all displayed rows; a selection bar shows the count and actions: **Set account**, **Set name**, **Set tag**, **Set date**, **Set memo**, **Set Print later**, **Delete rows**, and **Clear selection** (also on the right-click menu). Each Set action opens the same picker as the cell (click opens list, Q-250; suggestions still need accept) and applies to every selected row in one undo step; only columns the section type has are offered; amounts and check numbers are not bulk-set; changed rows are re-checked. Delete asks for confirmation. Works with Copy down (Q-272) and hidden columns (Q-273: setting a hidden column is allowed and says so). The grid's multi-select is a shared cl-grid feature other grids can opt into later. Keyboard and AccessKit labels for every action. Tests: range/toggle selection, each Set action, undo, delete confirm, re-check; batch_entry scenario step. — `feat(batch): select rows and bulk actions`
1. Four lanes from the command registry (later phases light up their actions; placeholders tracked in STATE.md). — `feat(home): lanes`
2. Account balances (books vs. imported bank balance when available). — `feat(home): balances`
3. Needs attention: bank items waiting, overdue invoices, bills overdue or due within 7 days (Q-169), unposted payroll imports, open to-dos (Q-130), reorder alerts (Q-149), weekly health-check findings (Q-171), backup reminder (Q-217). In-app only (Q-71). — `feat(home): needs attention`
4. Charts with a permissive egui plotting crate (Q-106), each drilling into its report. — `feat(home): charts`
5. Setup checklist for new companies (Q-63). — `feat(home): setup checklist`

**Tests:** attention counts equal engine queries; checklist auto-ticks; scenario; Enter-key option in both modes.

**Acceptance**
- [ ] With "Enter moves between fields" on, Enter moves between fields and Ctrl+Enter saves on every form; with it off, behavior is unchanged.
- [ ] On every screen the title, actions and column headers stay visible while scrolling (scenario per screen kind, including 960×640 and 200% zoom).
- [ ] Customers & Jobs and Vendors detail panels collapse and expand, and the choice is remembered.
- [ ] On wide screens, Daily Sales Summary and the other label/amount forms keep each label, account and amount together in a compact, striped table.
- [ ] Tags can carry a unique number, shown and sorted like account numbers everywhere tags appear.
- [ ] Bulk Entry Copy down sets the chosen column on every displayed row in one undoable step.
- [ ] Bulk Entry columns can be shown, hidden and reordered (remembered per company and type); required columns can't be hidden.
- [ ] Bulk Entry rows can be multi-selected and given an account, name, tag, date or memo in one undoable action.
- [ ] Every lane action opens its form or a tracked placeholder.
- [ ] Needs-attention counts match engine queries.
- [ ] Screen matches its mockup per docs/design/README.md (`ui-check --compare` reviewed by ui-verifier); any deviation is listed in the PR and QUESTIONS.md.

---

## Checkpoint PR-1 — Core ledger process review (fable)

Run `process-reviewer` on: register, checks, deposits/transfers, card charges, owner helpers, journal entries, template catch-up, closed-period enforcement, delete/void audit trail. Fix Blocking findings on `fix/PR-1` before M2 starts.
