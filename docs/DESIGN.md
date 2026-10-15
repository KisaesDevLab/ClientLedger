# ClientLedger Design Spec

Source of truth for tokens, behavior, and keyboard. The visual reference is the set of owner-approved mockups in `docs/design/` (15 screens, HTML plus PNG). Read `docs/design/README.md` for how to use them and which source wins when they disagree (Q-226).

**Principle:** QuickBooks Desktop users should feel at home in the *workflow* — forms, registers, keys — while the visual design is ClientLedger's own. Never copy Intuit icons, artwork, colors, or layouts.

---

## Tokens

### Color — light

| Token | Hex | Use |
| --- | --- | --- |
| `chrome.title` | #1E2421 | title bar background |
| `chrome.title.text` | #E9E6E0 | title bar primary text |
| `chrome.title.muted` | #A9B0AB | title bar secondary text |
| `surface.app` | #E9E6E0 | app background behind windows |
| `surface.panel` | #F7F5F1 | menu bar, left rail, tab strip, status bar |
| `surface.window` | #FFFFFF | active window/form body |
| `surface.subtle` | #FBFAF8 | table headers, section cards |
| `surface.form` | #F2F6F5 | check/bill header card, selected rows |
| `border.strong` | #C9C4BA | inputs, window borders |
| `border.default` | #DAD6CE | cards, table outer borders |
| `border.row` | #EEEBE5 | table row separators |
| `text.primary` | #1E2421 | body text |
| `text.secondary` | #4F5752 | labels, captions |
| `text.tertiary` | #6B726D | section eyebrows, hints |
| `text.placeholder` | #8C928E | placeholders, empty-row hints |
| `accent` | #0F5E5A | primary buttons, focus ring, active rail item, links |
| `accent.hover` | #0A4441 | hover/pressed |
| `accent.tint` | #E6F0EF | selected rows, info banners |
| `accent.border` | #B9C9C6 | info banner and form card borders |
| `warn` | #B5651D | count badges, needs-attention, low-confidence marker |
| `warn.tint` | #FFF8EF | rows needing review, entry row |
| `warn.surface` | #FFF4E5 | warning callouts |
| `warn.text` | #6B3A0C | text on warning callouts |
| `info` | #23406E | "match" badges text |
| `info.tint` | #E8EEF8 | "match" badges, bank-link banner |
| `neutral.chip` | #EEEBE5 | tags, rule badges |
| `neutral.chip.text` | #3A403C | tag text |
| `locked.row` | #F6F5F2 | closed-period rows |
| `locked.text` | #5C625E | closed-period row text |

### Color — dark

Derive from light with the same roles; starting values (tune for 4.5:1 text contrast):

| Token | Hex |
| --- | --- |
| `surface.app` | #121513 |
| `surface.panel` | #1A1E1C |
| `surface.window` | #202523 |
| `surface.subtle` | #262B29 |
| `surface.form` | #1F2B29 |
| `border.strong` | #3D4541 |
| `border.default` | #343B38 |
| `border.row` | #2B312E |
| `text.primary` | #E6E9E7 |
| `text.secondary` | #B4BBB7 |
| `text.tertiary` | #949C97 |
| `accent` | #3FA39C |
| `accent.tint` | #1C3533 |
| `warn` | #D98A45 |
| `warn.tint` | #2E2418 |
| `info.tint` | #1D2638 |

### Typography

- UI: **IBM Plex Sans** 400/500/600/700. Numbers, dates, check numbers, amounts: **IBM Plex Mono** 400/500. Both SIL OFL 1.1 — embed and list in ATTRIBUTION.md.
- Sizes (logical px): window title 22/600 · section heading 14/600 · eyebrow 11–13/600 uppercase +0.06em · body 13 · form inputs 14 (payee/amount 15) · captions 12 · kbd chips 11 mono.
- Amounts are right-aligned, mono, thousands separators, two decimals; negatives with leading minus (setting: parentheses).

### Spacing, radius, sizing

- Spacing scale: 2, 4, 6, 8, 10, 12, 14, 16, 20, 24 px.
- Radius: inputs 3–4, cards 6, form header card 8, chips 10 (pill).
- Minimum target 44 px for primary actions (Save buttons, Home lane actions); dense grid rows 28–32 px (two-line rows ~44 px).
- Focus: 2 px `accent` outline on focused input/cell.

---

## Window anatomy

1. **Title bar** (36 px, `chrome.title`): app name · company name · period and "Books closed through MM/DD/YYYY".
2. **Menu bar** (`surface.panel`): File, Edit, Lists, Clients, Vendors, Banking, Payroll (import only), Reports, Company (preferences, closing date, users, send to accountant), Window, Help; right side: Go to search (Ctrl+K).
3. **Left rail** (200–220 px): Workspace (Home, Clients & Jobs, Vendors, Bank Feed with count badge (fed by bank file imports in beta), Chart of Accounts, Reports), Tools (Import Bank File, Import Payroll, Send to My Accountant, Audit Trail), Open windows. Accountant tools (Workpapers, Report Packs, Tax Estimator) are post-beta and have no rail entry in beta.
4. **Tab strip:** one tab per open window; dirty tabs show "●"; Ctrl+Tab cycles.
5. **Window body** (`surface.window`): form toolbar → content → action row.
6. **Status bar:** signed-in user and role, mode (single-user / multi-user N users), server, context keyboard hints.

## Screen specs (mockups in docs/design/mockups/)

| Screen | Key elements |
| --- | --- |
| Home | Four lanes: Money in (Create Invoice, Receive Payment, Sales Receipt, Make Deposit, Open A/R total) · Money out (Enter Bill, Pay Bills, Write Check, Record Card Charge, bills due ≤ 7 days) · Banking (Review Bank Feed + count, Open Register, Reconcile, Transfer Funds, last reconciled) · Close & report (Make Journal Entry, Profit & Loss, Send to My Accountant, Set Closing Date, sales tax owed). Below: Account balances (books vs bank) and Needs attention list. |
| Write Check | Toolbar (Prev/Next, Find, Print, Attach, Memorize, History; Print later — Pay online is post-beta) · bank account + ending balance · check card (company name, No., Date, Pay to the order of, Amount, amount in words, Address, Memo) · Expenses/Items tabs with totals · split grid (Account, Amount, Memo, Client:Job, Billable, Tags) · AI suggestion row styling (`warn.tint`, caption in `warn.text`) · Clear / Save & New / Save & Close · side panel (payee summary + open bill warning, recent checks, attachments). |
| Bank Register | Account, Show range, Sort by, one-line toggle, Go To, QuickReport, Edit Transaction · two-line rows (Date · Number/Type · Payee/Account·Memo · Payment · ✓ · Deposit · Balance) · locked closed-period rows · entry row pinned below the grid (`warn.tint`, accent top border) with Record and Splits · footer cleared/ending balance. |
| Bank Feed | Header with last import date and an Import Bank File button · filter segmented control (All, High confidence, Needs review, Matches) · bulk bar (Accept selected Ctrl+Enter, Change account, Exclude, key hints) · grid (select, date, bank description + sub, amount, proposed payee/account, basis badge, action) · detail panel (callout, Payee, Account, Client:Job, Create rule, Accept, Split) · footnote "AI suggests accounts only; amounts always come from the bank." |
| Enter Bill | Toolbar (Prev/Next, Find, Attach, Select PO (post-beta), Memorize, History; Bill/Vendor credit; Bill received) · source-document banner · header card (Vendor, Date, Ref no., Address, Terms, Amount due, Discount date, Bill due, Memo) · Expenses/Items tabs with "Lines total = amount due ✓" · split grid with Tags · side panel (document preview, vendor snapshot with duplicate check). |
| Pay Bills | Filter row (due on or before / all, date, vendor, sort, take discounts) · grid (select, due, vendor+note, ref, disc. date, amt due, discount, credits, amt to pay input) · totals row · selected bill panel (terms, discount, credits, Set discount/credits) · payment panel (date, method, check numbering, account, ending balance before → after) · Pay selected bills button showing total. |
| Create Invoice | Toolbar (Prev/Next, Find, Preview, Print, Email, Add billable costs, Progress bill (post-beta); Template) · billable costs banner · header card (Client:Job, Date, Invoice #, Tags, Bill to, Job site, Terms, Due, P.O. no.) · line grid (Item, Description + source caption, Qty, Rate, Amount, Tax) · message + totals block · Email after saving checkbox (no online payment links in v1) · side panels (client balance, job progress bar vs estimate, unbilled costs). |
| Receive Payment | Toolbar · bank-feed link banner · header card (Received from, Date, Payment amount, Method, Check/reference no., Deposit to, Memo) · open invoices grid with payment inputs and totals · underpayment callout (leave open / write off) · amounts summary · side panels (balance before/after, next step Make Deposit). |

Sample data in screenshots and the seeded sample company: "Ozark Ridge Landscaping LLC" (synthetic).

---

## Keyboard map (initial)

Global

| Keys | Command |
| --- | --- |
| Ctrl+K | Go to / command palette |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous window |
| Alt+H | Home |
| Ctrl+W | Write Check |
| Ctrl+I | Create Invoice |
| Ctrl+R | Receive Payment |
| Ctrl+Shift+R | Sales Receipt |
| Ctrl+D | Make Deposit (in forms/registers: delete line or transaction per context — see note) |
| Ctrl+B | Enter Bill |
| Ctrl+Shift+B | Pay Bills |
| Ctrl+Shift+C | Record Card Charge |
| Ctrl+G | Open Register |
| Ctrl+Shift+E | Reconcile |
| Ctrl+T | Transfer Funds |
| Ctrl+J | Make Journal Entry |
| Ctrl+Shift+T | Trial Balance |
| Ctrl+P | Print (current form or report) |
| Ctrl+A | Chart of Accounts |
| Ctrl+F | Find / Go To |
| Ctrl+E | Edit transaction (from register/report) |
| Ctrl+L | Open list for the focused field |
| F1 | Help for current screen |
| Ctrl+/ | Shortcut overlay for current screen |

Note: context overrides (Ctrl+D, Ctrl+P) are resolved by the keymap's scope rules (form > window > global). Keep the keymap the single registry; menus and tooltips read from it.

Forms and grids

| Keys | Action |
| --- | --- |
| Alt+N | Save & New (also Enter in header fields) |
| Alt+S | Save & Close |
| Esc | Revert field → revert form → close (prompt if dirty) |
| Tab / Shift+Tab | Next / previous field or cell |
| Enter | Record row (registers, grids) |
| Ctrl+Insert / Ctrl+Delete | Insert / delete line |
| Page Up / Page Down | Previous / next record (forms), page (grids) |

Date fields

| Key | Result |
| --- | --- |
| + / − | Next / previous day |
| T | Today |
| M / H | First / last day of month |
| Y / R | First / last day of year |
| W / K | First / last day of week |

Bank feed: J/K move, Space select, A accept, S split, R make rule, Ctrl+Enter accept selected.

---

## Performance budgets (P37)

| Measure | Budget |
| --- | --- |
| Cold start to Open Company window | ≤ 6 s (SSD, mid-range laptop) |
| Open sample company | ≤ 2 s |
| Open 100k-transaction register | ≤ 1 s to first paint |
| Register scroll | ≥ 60 fps |
| P&L, 5 years by month, 100k transactions | ≤ 3 s |
| Form save round trip | ≤ 150 ms p95 (local) |
| List filter (20k records) | ≤ 100 ms |
