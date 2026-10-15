# docs/design — reference mockups

These are the owner-approved visual references for ClientLedger's screens. Each screen exists twice:

- `mockups/<Screen>.html` — a static HTML mockup (open it in a browser; links between screens work).
- `screenshots/<Screen>.png` — the same screen rendered at 1440 px wide, for side-by-side comparison.

All data in them is synthetic (Ozark Ridge Landscaping LLC, Shoal Creek Farm). The mockups were drawn on the owner's private design canvas; the copies here are the version of record for the build. Never commit the canvas link.

## How to use them

1. **Before building a screen**, open its mockup and screenshot (table below) along with the phase in `phases/` and the screen spec in `docs/DESIGN.md`.
2. **Build it in egui, not HTML.** The mockups show layout, grouping, field order, labels, helper text, states, and sample figures. Translate them into egui widgets and the `cl-grid` widget. Do not port CSS, pixel offsets, or browser behavior.
3. **Precedence when sources disagree:**
   1. CLAUDE.md non-negotiables and decisions in QUESTIONS.md
   2. `docs/DESIGN.md`: tokens, keyboard map, behavior, performance budgets
   3. the mockup: layout, wording, and what appears on the screen
   4. your own judgment, recorded in QUESTIONS.md with a default
4. **Do not edit files in `docs/design/`.** If a mockup conflicts with the spec or can't be built as drawn, follow the precedence above, add a QUESTIONS.md row, list the deviation in the pull request, and continue.
5. **Elements marked PROPOSED are not built.** They wait for an owner decision (see Q-227).
6. **Window sizes.** Mockups are drawn at 1440 × ~940. The app must also work at the minimum window size and at 200% zoom (DESIGN.md). Side panels move below the main content or collapse, the same way the mockups do at narrow widths.
7. **Verification.** `cargo xtask ui-check <screen> --compare` saves `target/ui-check/<screen>/compare.html`, which puts the app's light-theme screenshot next to the reference PNG. `ui-verifier` reviews the two by structure, not pixel diff: missing or extra elements, order, labels, token colors, and states.

## Screen index

| Mockup | Phase(s) | Notes |
| --- | --- | --- |
| `Home` | P06 (title bar, menu, left rail, tab strip, status bar on every screen), P16 (Home content) | Four lanes, setup checklist link, balances with "In your books" / "Bank file" columns, 6-month income/expense chart, Needs attention list |
| `OpenCompany` | P08 | Shown when no company is open: recent companies with color stripe, New, Switch from QuickBooks Desktop, Restore, Open on office network (P33), Practice company |
| `Preferences` | P09 | AI panel: off by default, Ollama/Anthropic/OpenAI, per-feature switches, usage cap, keychain note |
| `Register` | P12 | Two-line rows, locked closed-period rows, legend, quick filters, **entry row pinned below the grid** |
| `WriteCheck` | P13 | Check card, Expenses/Items tabs, split grid with Client:Job, Billable, Tags; AI-suggested account row; open-bill warning side panel (link to Pay Bills arrives with P18) |
| `EnterBill` | P17 | Source-document banner from the receipt inbox, header card, lines-total check, document preview, vendor snapshot with duplicate ref check |
| `PayBills` | P18 | Filters, bill grid with discount and credits, selected-bill panel, payment panel with ending balance before → after |
| `CreateInvoice` | P20 (sales tax line completes in P23) | Billable-cost banner, header card with Tags, line grid with service dates, totals block, email-after-save |
| `ReceivePayment` | P21 | Open-invoice grid, underpayment choice, summary, next-step Make Deposit panel |
| `ImportBankFile` | P25 | 4-step wizard, file/account/balance-check cards, new vs. skipped counts, preview grid |
| `BankFeed` | P26 | Account picker, filter segments, bulk bar with key hints, "Why" badges (Rule, Match, Transfer, AI), detail panel |
| `Reconcile` | P27 | Two panes, bank-file items pre-checked with a marker, summary and difference, Reconcile now disabled until 0.00 |
| `Reports` | P28 | Ask-a-question bar showing how the question was mapped, report catalog, filters, basis toggle, % of income and YTD columns, drill-down links |
| `QBDImport` | P31 | Step list, three tie-out tiles, per-account comparison, Mark verified |
| `SendToAccountant` | P32 | Three-step explainer, what's included, check totals, file save, history. **The "Send directly" panel is PROPOSED — do not build (Q-227).** |

Screens without a mockup (Chart of Accounts, journal entries, lists, the setup interview, and others) follow `docs/DESIGN.md` and the shared shell from the Home mockup.

## Sample figures

The P08 sample-company generator must reproduce these figures so screenshots line up with the mockups (story date 11/04/2026; books closed through 09/30/2026). A golden test asserts them.

| Figure | Value |
| --- | --- |
| 1010 Operating Checking, ending balance in books | 84,210.55 |
| Operating Checking, last bank file balance (11/03) | 92,431.07 |
| Last reconciliation (09/30/2026) | 71,436.20 |
| October statement ending balance (10/31/2026) | 86,902.13 |
| Open A/R | 48,912.40 |
| Bills due within 7 days (through 11/11) | 7,471.90 |
| Bank Feed items waiting (Operating) | 37 (28 ready, 6 need review, 3 matches) |
| P&L October 2026, net income | 13,865.14 |
| P&L Jan–Oct 2026, net income | 125,985.00 |
| Transactions / attachments | 1,512 / 214 |
