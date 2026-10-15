# QUESTIONS.md — ClientLedger

Decisions and open questions. **Follow the default until the owner answers.** New questions get the next number, the phase that raised them, and a default that keeps the build moving. Irreversible decisions are marked **STOP** — wait for the owner.

Owner Q&A held 2026-10-11 answered most of the original list; those rows record the decision.

## Answered

| ID | Decision | Affects |
| --- | --- | --- |
| Q-01 | ClientLedger is **free and MIT-licensed**. Public GitHub repo. Outside pull requests are not accepted. | P00, P35, P39 |
| Q-03 | Engine packaged as a **Node single executable application (SEA)**. | P01 |
| Q-07 | Direct Rust deps: MIT or Apache-2.0 only. Transitive deps may also use BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, CC0-1.0, Boost 1.0, Apache-2.0 WITH LLVM-exception. Bundled fonts: SIL OFL 1.1. | P00 onward |
| Q-08 | **No licensing system.** No keys, activation, entitlements, or grace periods. (Original P09 removed.) | — |
| Q-09 | **No paid tiers or seat limits** in ClientLedger, including LAN multi-user. | P33 |
| Q-10 | **Checks only** in v1. No ACH. | P13, P18 |
| Q-14 | **Pre-printed check stock only** in v1; no MICR line. PDF generation with a permissive crate; OS print hand-off (default below remains for the print mechanism). | P19 |
| Q-15 | **No app-level encryption at rest**; setup guide recommends BitLocker, FileVault, or LUKS. | P38 |
| Q-18 | **Average-cost inventory only** in beta; assemblies post-beta (P42); no lots, serials, bins, or FIFO. | P24, P42 |
| Q-19 | **No Plaid in beta**; bank file import only. Revisit post-beta (P43). | P25–P27, P43 |
| Q-24 | QBD import uses **the same reports and mapping as the MyBooks Online importer**. | P31 |
| Q-25 | MyBooks Online = **the existing browser-based MyBooks**. Transfer is **one-way** (ClientLedger → Online) and **export file only**. | P32 |
| Q-26 | **Windows x64 only** for beta; ARM64 later. | P35 |
| Q-27 | **No signing certificates yet.** Beta builds are unsigned; CI signing steps run only when secrets exist. | P35 |
| Q-31 | The MyBooks engine stays **PolyForm Internal Use 1.0.0** with a **special written grant** allowing redistribution inside ClientLedger. The engine repo will be **public**. | P00, P01, P39 |
| Q-36 | Audience is **small business owners**. Accountant tools, workpapers, Report Packs, and Tax Estimator move post-beta. | All UI phases |
| Q-37 | Beta includes credit memos + refunds, statements + finance charges, sales tax, average-cost inventory, LAN multi-user, and payroll import. Estimates, progress invoicing, sales orders, and POs are post-beta. | Phase list |
| Q-38 | AI is off by default; cloud AI needs the owner's own key and internet; rules categorize offline. | P09, P13, P17, P26 |
| Q-02 | Engine submodule **tracks `main`**: pin the current main commit, bump at the start of each milestone after contract tests pass. | P01, every milestone |
| Q-04 | Money format unknown; **Claude Code detects it in P05** and records the finding here. `cl-core::Money` wraps `rust_decimal`; never f64. | P05 |
| Q-05 | Companies stay **engine tenants in one database**. Backups, restores, and transfers use a versioned per-tenant export/import (U-05); whole-database `pg_dump` only for disaster recovery and upgrades. | P02, P08, P32 |
| Q-06 | PostgreSQL 16 from **zonky embedded-postgres-binaries**, pinned with a SHA-256 lockfile. | P03 |
| Q-11 | Bank-file parsers live **in Rust** (`cl-import`); the engine accepts normalized batches (U-06). MyBooks Online does not share this parser. | P25, P37 |
| Q-12 | Amount fields **act as calculators** with exact decimal math. | P10 |
| Q-16 | MCP server is **post-beta**; the P07 tool table prepares for it. | P07 |
| Q-17 | Sales tax rates are **entered by the owner**; no rate service. | P23 |
| Q-20 | Auto-login is **optional and off by default**; closing-date password still required. | P29 |
| Q-28 | Minimum OS: **Windows 10 22H2+/11 (x64), macOS 13+, Ubuntu 22.04+**. | P35 |
| Q-29 | **Owner reviews and merges** engine pull requests; Claude Code works on `clientledger/*` branches. | All engine work |
| Q-32 | Public repo: **issues and discussions on; PRs auto-closed** with a note. | P00 |
| Q-33 | AI providers: **Anthropic and OpenAI** (owner's key, online) and **Ollama** (local, offline). No Gemini. | P09, U-11 |
| Q-34 | Email via the **owner's SMTP settings**; otherwise Outbox or save PDF. | P09, P20, P22 |
| Q-39 | Post-beta order **as listed** (P40 estimates and sales orders first). | M7 |
| Q-13 | PDF viewer: **PDFium** via `pdfium-render` (MIT/Apache) with prebuilt PDFium binaries (BSD/Apache), loaded dynamically. | P17 |
| Q-14b | Printing: **PDF sent to the OS print path** (Windows print verb, macOS/Linux `lp`), fallback opens the PDF. | P19 |
| Q-30 | Branding: **"ClientLedger — powered by Vibe MyBooks"**; About shows license notices and the QuickBooks non-affiliation notice. | P06 |
| Q-21 | Working trial balance export to Vibe Trial Balance as **CSV in its import format** (read its importer to match). | P45 |
| Q-22 | Fixed assets: **user-entered depreciation amounts**; the register generates journal entries. No tax depreciation. | P45 |
| Q-23 | 1099 data exported as **CSV in Vibe 1099's import format** (vendor flags + account-to-box mapping). | P45 |
| Q-35a | The engine repo goes **public before P01**, so the submodule and public CI work from the start. | P01 |
| Q-40 | Posted transactions can be **deleted or voided**; every delete is kept in the audit trail with its before-image; closed periods need the password. | P12, P29 |
| Q-41 | **US only** for beta: USD, US formats, US sales tax and 1099 concepts. | P08, all |
| Q-42 | Live database stays on a **local disk** (network and cloud-sync folders refused); **backups may go anywhere**, including OneDrive, Dropbox, and external drives. | P03, P34 |
| Q-43 | Transfer files are **versioned**; MyBooks Online accepts older versions and refuses newer ones with an update message. | P32, U-08 |
| Q-44 | Reports default to **accrual**, with a cash toggle on every report and a company preference. | P08, P28 |
| Q-45 | New companies get **guided opening balances** as of a start date. | P08 |
| Q-46 | **SemVer**: 0.x during beta, 1.0 at GA; release when a milestone lands. | P39 |
| Q-47 | Support through **GitHub issues/discussions and docs**; no paid support. | P38, P39 |
| Q-48 | Year-end is **automatic**: net income rolls to retained earnings; owner sets a closing date afterward. | P08, P28 |
| Q-49 | Starter charts of accounts: **General service, Contractor / trades, Retail / product sales, Farm / ranch (Schedule F)**. | P08 |
| Q-50 | Attachments limited to **10 MB per file**, no total cap; included in backups and transfers. | P17, P32 |
| Q-51 | Performance and test target: **up to 10 companies** per install. | P08, P37 |
| Q-52 | Recurring invoices are **post-beta** (P40). | P15, P40 |
| Q-53 | Beta includes a **1099 vendor flag, tax ID, W-9 on file, account-to-box mapping, and a 1099 Summary report** for the CPA. | P11, P28 |
| Q-54 | **Receipt inbox folder**: new photos/PDFs appear for attaching to bills, checks, or bank items. | P09, P17 |
| Q-55 | Check printing **confirms after print**; reprints get new numbers and void the originals, logged. | P19 |
| Q-56 | **Export All Data** to a CSV + Excel zip (lists, transactions with lines, standard reports). | P08 |
| Q-57 | **Tabs with optional pop-out** to separate OS windows. | P06 |
| Q-58 | Beta invoicing includes **discount items, price levels, and subtotal items**; group items are post-beta. | P11, P20 |
| Q-59 | **Undo within the open form** only; saved changes use Edit/Delete/Void, all logged. | P06, P10 |
| Q-60 | **Full credit card flow** in beta: charge/credit form, card register, card reconcile, pay by check, bill, or transfer. | P14, P27 |
| Q-61 | **"Pay yourself / Put money in"** guided by entity type; no tax advice. | P14 |
| Q-62 | **Loan payment helper**: owner enters interest from the statement, principal fills in. Amortization schedules not in beta. | P14 |
| Q-63 | **Setup checklist on Home** for new companies, dismissible. | P16 |
| Q-64 | Statement of Cash Flows is **post-beta** (P48). | P48 |
| Q-65 | **Simple P&L budgets** in beta with a Budget vs. Actual report. | P28 |
| Q-66 | **App-wide zoom 80–200%**, remembered per user. | P06 |
| Q-67 | Calendar view is **post-beta** (P48); Home's Needs attention covers due items in beta. | P16, P48 |
| Q-68 | **No online invoice payment links** in v1; payments recorded manually. | P20 |
| Q-69 | **Optional idle auto-lock**, off by default. | P29 |
| Q-70 | **Native macOS menu bar**; in-window menus on Windows and Linux. | P06 |
| Q-71 | **In-app notifications only**; no OS notifications in v1. | P16 |
| Q-72 | A **git-ignored `private-fixtures/` folder** may hold the owner's anonymized real files for local-only tests. | All test phases |
| Q-73 | Beta AI features (when enabled): **account/payee suggestions, bill and receipt reading, bank feed categorization, and ask-a-question reports** (AI picks the report; the engine computes the numbers). | P13, P17, P26, P28 |
| Q-74 | Transfer round-trip tested in **CI against the MyBooks appliance in Docker**. | P32 |
| Q-75 | **Staged beta**: the owner's clients first, then public pre-release. | P39 |
| Q-76 | **No tax-line mapping** on accounts. | P11 |
| Q-77 | **One contact can be both client and vendor.** | P11, P31 |
| Q-78 | **No "Other names" list**; use vendors for owners, lenders, and one-off payees. | P11, P31 |
| Q-79 | **Account numbers**: preference, on by default. | P08, P11 |
| Q-80 | **Customer early-payment discounts** suggested in Receive Payment. | P21 |
| Q-81 | **Customer deposits** held in a Customer Deposits liability until applied to an invoice. | P21 |
| Q-82 | **Guided bounced-check action** in beta. | P21 |
| Q-83 | **Retainage is post-beta**, with progress invoicing (P40). | P40 |
| Q-84 | Card processor payouts are **entered manually** (negative fee line allowed); no processor imports. | P14 |
| Q-85 | **"Paid with personal funds"** expense entry, defaulting by entity type. | P14 |
| Q-86 | **Daily sales summary form** in beta. | P14, P23 |
| Q-87 | **Guided vendor refunds** in Make Deposit. | P14 |
| Q-88 | Customer refunds by **check, cash, or recorded card/ACH/other** refunds. | P22 |
| Q-89 | **Exemption certificates** tracked with number, state, expiry; invoices warn when missing or expired. | P23 |
| Q-90 | **Jobs have status, type, and dates.** | P11, P28 |
| Q-91 | **Audit trail kept forever.** | P29, P32 |
| Q-92 | **Company logo** on the built-in form template in beta. | P19 |
| Q-93 | Forms fit **#10 double-window envelopes**. | P19 |
| Q-94 | **Auto-increment numbering with optional prefix**; duplicates warned. | P19, P20 |
| Q-95 | **Footer text field** for payment instructions; no remittance stub in beta. | P19 |
| Q-96 | **Units of measure are labels only**; no conversions. | P11 |
| Q-97 | **Date warnings**: >90 days past or >30 days future, configurable. | P06, P09 |
| Q-98 | **No spell check** in v1. | — |
| Q-99 | **Two-sided items** (cost and price, expense and income accounts) for pass-through purchases. | P11, P17, P20 |
| Q-100 | Reconcile **pre-checks imported items**; owner reviews before finishing. | P27 |
| Q-101 | **Auto-accept is optional per rule**, off by default, always logged. | P26 |
| Q-102 | **Transfers between own accounts** detected and matched as one transfer. | P26 |
| Q-103 | **No split rules**; one account per rule. | P26 |
| Q-104 | Report columns: **% of income, prior period $ and % change, year-to-date**. | P28 |
| Q-105 | **Fixed 30/60/90 aging buckets.** | P28 |
| Q-106 | **Small chart set on Home.** | P16 |
| Q-107 | Report headers show **company name, logo, basis, dates, page numbers, run time**. | P28 |
| Q-108 | LAN host can run as an **optional background service**. | P33 |
| Q-109 | **Remote access over the internet is not supported**; LAN only. | P33, P38 |
| Q-110 | **Host down = no work** until reconnected; unsaved form kept locally. | P33 |
| Q-111 | Test and support **up to 5 concurrent LAN users**. | P33, P37 |
| Q-112 | Claude Code **runs continuously**; only STOP items pause it. Milestone summaries go in STATE.md for asynchronous review. | All |
| Q-113 | **Branch and pull request per phase** (`phase/Pxx-name`). | All |
| Q-114 | CI: **Linux every push; Windows and macOS on PRs and nightly.** | P00 |
| Q-115 | **Auto-merge when CI is green.** Engine PRs still wait for the owner (Q-29). | All |
| Q-116 | **≥ 80% line coverage across the workspace**, enforced in CI (from P05). | All |
| Q-117 | **Rust edition 2024.** | P00 |
| Q-118 | Logs kept **30 days or 100 MB**. | P04 |
| Q-119 | Errors: **plain message + "Copy details"** with a scrubbed error ID. | P06 onward |
| Q-120 | **The desktop app owns the ClientLedger name** and ClientLedger.net; MyBooks markets under its own name. | P06, P35, P39 |
| Q-121 | Downloads from **GitHub Releases**, linked from ClientLedger.net; update check uses GitHub. | P35, P36 |
| Q-122 | **Claude Code drafts original icon options**; option 1 used until the owner picks. | P06 |
| Q-123 | App and installer name: **ClientLedger**. | P06, P35 |
| Q-124 | **No imports from other apps** (QBO, Wave, Xero) in beta. | P31 |
| Q-125 | **Mapped CSV import** of invoices, bills, and journal entries. | P31 |
| Q-126 | **IIF export** of lists and journal entries for QuickBooks Desktop. | P32 |
| Q-127 | **"New from existing"** company copies setup without transactions or contacts. | P08 |
| Q-128 | **Multiple ship-to addresses** per client with a default. | P11, P20 |
| Q-129 | **Multiple contact people with roles**; choose invoice/statement recipients. | P11, P20, P22 |
| Q-130 | **Dated notes and simple to-dos** on contacts and jobs; to-dos feed Needs attention. | P11, P16 |
| Q-131 | **No client credit limits.** | P11 |
| Q-132 | Grant scope: ClientLedger users may use the bundled engine to keep **their own organization's books only** (their outside accountant may review/adjust/receive them). Bookkeeping for others needs a MyBooks commercial license. | Grant, P38 docs |
| Q-133 | Only **Licensor's official releases** (and personal builds from unmodified source for own use) may include the engine. Forks of the MIT code may not bundle, download, or install the engine. | Grant, NOTICE.md, P35 |
| Q-134 | Installers show a **license acceptance screen** (MIT + engine grant + PolyForm). | P35 |
| Q-135 | **Short privacy notice** in the app and on ClientLedger.net. | P06, P38 |
| Q-136 | Cloud AI enablement records **dated consent in the audit trail**. | P09 |
| Q-137 | The engine's own-books scope appears **in About and docs only**. | P06, P38 |
| Q-138 | **AI usage meter and owner-set monthly cap.** | P09 |
| Q-139 | **Minimum data, redacted** before cloud AI calls. | P09, U-09 |
| Q-140 | **Per-feature AI toggles.** | P09 |
| Q-141 | Repeated accepted suggestions **offer a local rule**; no model training. | P09, P26 |
| Q-142 | QBD import history depth is **the owner's choice** (all, or two years plus opening balances). | P31 |
| Q-143 | **QBD memorized transactions are not imported.** | P31 |
| Q-144 | **Guided spot-check list** after the tie-out report. | P31 |
| Q-145 | **QBD users and passwords are not imported.** | P31 |
| Q-146 | **No sales tax filing reminders.** | P23 |
| Q-147 | Sales tax **rounded per invoice total**. | P23 |
| Q-148 | Negative inventory: **warn and allow**. | P24 |
| Q-149 | **Reorder alerts** in Needs attention and a reorder report. | P16, P24 |
| Q-150 | Payroll imports post a **summary entry per pay date**; no employee detail. | P30 |
| Q-151 | Payroll creates **expected withdrawals** matched in the bank feed. | P26, P30 |
| Q-152 | Provider **departments map to tags**. | P30 |
| Q-153 | **Generic CSV mapping** for unsupported payroll providers. | P30 |
| Q-154 | **Bank/card numbers masked**; Admin reveal is logged. | P11, P29 |
| Q-155 | **Only Admin changes the closing date.** | P29 |
| Q-156 | **Audit trail export** (CSV, PDF) for Admins. | P29 |
| Q-157 | **Per-account sensitive flag** hides balances and detail from roles without permission. | P28, P29 |
| Q-158 | Minimum hardware: **4 GB RAM, 2 cores, 2 GB disk**; budgets measured there. | P35, P37 |
| Q-159 | **Help bundled offline**; also published online. | P38 |
| Q-160 | **How-to videos post-beta.** | — |
| Q-161 | **Printable shortcut PDF + Ctrl+/ overlay.** | P06, P38 |
| Q-162 | **No overdue reminders/dunning** in v1. | P20 |
| Q-163 | **Editable email templates** per document type. | P20, P22 |
| Q-164 | Emails attach **the invoice PDF only**. | P20 |
| Q-165 | **Sent date, method, and recipients** tracked on invoices. | P20 |
| Q-166 | **No bill approval step.** | P17, P18 |
| Q-167 | **Positive Pay export is post-beta.** | Deferred |
| Q-168 | **Guided vendor statement match** in beta. | P17 |
| Q-169 | Home shows **bills overdue or due within 7 days** (configurable). | P16 |
| Q-170 | **Verify Data** integrity check with owner-applied fixes after backup. | P34, U-12 |
| Q-171 | **Weekly quick health check** on company open. | P34 |
| Q-172 | **Duplicate finder report.** | P28, P34 |
| Q-173 | **No condense tool.** | — |
| Q-174 | Finance charges **assessed by the owner with a preview**. | P22 |
| Q-175 | Statements default to **open item**. | P22 |
| Q-176 | Statement batch defaults to **clients with open balances**. | P22 |
| Q-177 | Statements show an **aging row**. | P22 |
| Q-178 | Tags at **transaction and line level**; header defaults to lines. | P11 |
| Q-179 | **One tag per group per line**; ungrouped tags unlimited. | P11 |
| Q-180 | Tag groups can be **required** for chosen account types. | P11 |
| Q-181 | **Rules and memorized transactions can set tags.** | P15, P26 |
| Q-182 | **One company open at a time.** | P08 |
| Q-183 | Switch companies via **File › Open only**. | P08 |
| Q-184 | **Per-company accent stripe** in the title bar. | P06, P08 |
| Q-185 | Delete company: **Admin, typed-name confirm, backup first**. | P08 |
| Q-186 | Registers open **date ascending, last 90 days**. | P12 |
| Q-187 | **Balance always shown** as the true per-transaction balance in date order. | P12 |
| Q-188 | **Subtle row colors by state.** | P10, P12 |
| Q-189 | Register filters: **uncleared, payee, tag, amount range**. | P12 |
| Q-190 | **Enter = Save & New** in form header fields. | P06, P13 |
| Q-191 | Payee **prefills from last transaction** except amounts. | P13, P17 |
| Q-192 | **Vendor default expense account.** | P11, P13 |
| Q-193 | **Memos optional.** | All forms |
| Q-194 | OFX/QFX accounts **auto-linked after first import**. | P25 |
| Q-195 | CSV dates **assume MM/DD**, flag impossible values. | P25 |
| Q-196 | Card sign **auto-detected, confirmed once**. | P25 |
| Q-197 | **File ending balance compared** to the books. | P25 |
| Q-198 | **Optional service date** per invoice line. | P20 |
| Q-199 | **No line-level discounts**; use discount items. | P20 |
| Q-200 | **Multi-line plain-text** line descriptions. | P20 |
| Q-201 | **Hide $0 lines** per invoice, off by default. | P19, P20 |
| Q-202 | Payments and sales receipts default to **Undeposited Funds**. | P14, P21 |
| Q-203 | **Deposit summary printout only**; no bank slip alignment. | P14 |
| Q-204 | **Cash over/short line** on daily sales summaries. | P14 |
| Q-205 | **Cash back on deposits allowed.** | P14 |
| Q-206 | Journal entries may use **A/R or A/P with a name, one per entry**. | P15 |
| Q-207 | **Adjusting-entry flag** with report filter. | P15, P28 |
| Q-208 | Sub-account depth **matches the MyBooks engine**. | P11 |
| Q-209 | **Account numbers must be unique.** | P11 |
| Q-210 | **Recovery key** created at setup resets the Admin password. | P08, P29 |
| Q-211 | Passwords **12+ characters, no forced expiry, lockout**. | P29 |
| Q-212 | **No signature images** on checks. | P19 |
| Q-213 | Voucher stubs list **bills paid with details**. | P19 |
| Q-214 | **Changelog link only**; no What's New screen. | P36 |
| Q-215 | **Single release channel.** | P36 |
| Q-216 | **No update reminders.** | P36 |
| Q-217 | **Backup reminder after 7 days** without a backup. | P16, P34 |
| Q-218 | QBD versions **match the MyBooks Online importer**. | P31 |
| Q-219 | Sample company: **one year, ~1,500 transactions**. | P08 |
| Q-220 | Large engine gaps: **build upstream and keep going**; block only the affected screen. | All engine work |
| Q-221 | **Multi-agent build** with model routing by phase tier: orchestrator on Sonnet; Haiku for search and records; Sonnet for routine implementation and verification; Opus for architecture, reviews, and Tier H implementation (docs/AGENTS.md). | All |
| Q-222 | **Fresh-eyes reviews**: `reviewer` (all H/M phases) and `security-reviewer` (listed phases) never see the implementation transcript. | All |
| Q-223 | **Escalation after two failed attempts**: sonnet → opus → architect re-plan → block the item and continue. | All |
| Q-224 | **Parallelism limits**: up to 4 concurrent subagents; up to 3 implementer work packages in worktrees; at most two phases at once when dependencies and crates allow. | All |
| Q-225 | **Fable for process reviews**: `process-reviewer` runs on `fable` at 12 checkpoints (two plan reviews, eight lifecycle reviews, upgrades, pre-beta journeys), for L-sized accounting-rule engine plans, and as the last escalation step for non-security work; never for security review or code. Falls back to opus at xhigh effort if unavailable. | All |
| Q-226 | **Mockups are the visual reference.** `docs/design/` holds 15 owner-approved screens (HTML + PNG) with an index mapping each to its phase. Precedence on conflict: non-negotiables and QUESTIONS decisions → DESIGN.md → mockup → judgment (logged here). Build in egui, not HTML; compare by structure via `ui-check --compare`; the P08 sample company reproduces the mockup figures. Claude Code never edits `docs/design/`. | P06–P32 screens, P07, P08 |
| Q-227 | **PROPOSED elements are not built.** The "Send directly" panel on the Send to My Accountant mockup waits for an owner decision; Q-25 (export file only) stands until then. | P32 |
| Q-228 | **Mockup updates:** the owner revises screens on the private design canvas; a session then re-exports them into `docs/design/` (HTML + PNG) in a `docs(design):` commit. The canvas link is never committed (public repo). | All screens |

## Open

| ID | Question | Raised | Default assumption | Status |
| --- | --- | --- | --- | --- |
| Q-35 | Final wording of the engine redistribution grant. | P01 | Draft in `docs/legal/LICENSE-CLIENTLEDGER.draft.md` (2026-10-11) awaiting attorney review; final text goes in the engine repo as `LICENSE-CLIENTLEDGER.md` (U-10). Claude Code must not edit the legal text. | Open — **STOP before the first public release (P39)** |
| Q-P00a | Layering for crates CLAUDE.md does not constrain (`cl-host`, `cl-company`, `cl-print`, `cl-automation`, `cl-lan`), and egui usage. | P00 | Each may depend on any library crate below `cl-ui` (never `cl-ui` or apps). Only `cl-grid`, `cl-ui`, and apps may depend directly on egui/eframe, per "nothing below `cl-ui` depends on egui except `cl-grid`". If `cl-automation` needs egui for widget inspection (P07), amend the table in `xtask/src/layering.rs` and CLAUDE.md together. Table lives in `xtask/src/layering.rs`. | Default in force |
| Q-P00b | `cargo xtask test` sets `CL_OFFLINE_TESTS=1` but cannot block the network itself. | P00 | The variable is a convention: test helpers that could open sockets must check it and refuse. Real network blocking (sandbox or firewall in CI) is deferred to when the first networked test exists (P05). | Default in force |
| Q-P00c | Secret scanner choice (permissive license required). | P00 | gitleaks (MIT) pinned binary v8.18.4 run directly in `ci.yml`, not the licensed GitHub Action. Not run locally; first CI run confirms the download URL and a clean scan. | Default in force |
| Q-P01a | The pinned engine (13ebd2b) cannot satisfy "`/health` reports the pinned engine commit" or run without Redis: `/health` returns 503 `degraded` without Redis and its body has no commit field. | P01 | Not an engine edit in P01. U-02 (Redis-free desktop mode) and U-03 (`/health`) in P02 must also add `engineCommit` (set at build time from the submodule commit, e.g. `CL_ENGINE_COMMIT`) to the `/health` body. Until then `manifest.json` is the source of the pinned commit and `xtask engine --smoke` accepts HTTP 503 when `"db":"ok"` (Redis degraded, expected) and prints a note. The smoke test must be tightened to require 200 and the commit in P02. | Default in force; P01 acceptance item 3 completes in P02 |
| Q-P01b | The engine is not SEA-ready as written (native `bcrypt` addon, migrations/knowledge/fonts read from disk via `import.meta.url`, static `puppeteer` imports, ESM). | P01 | `xtask/engine-tools/bundle.mjs` works around it without editing the engine: esbuild bundle to CJS, `import.meta.url` defined to a per-bundle runtime dir, assets and the host-platform bcrypt prebuild embedded as SEA assets and extracted on first start (`CL_ENGINE_RUNTIME_DIR` overrides the location; P04 should set it inside the app data dir), `puppeteer` stubbed (PDF output is `cl-print`, P19; any engine route that renders HTML to PDF fails with a clear message), optional native accelerators (`@napi-rs/canvas`, `msgpackr-extract`, etc.) left out. If this proves brittle, propose an upstream change (generic `ENGINE_ASSET_DIR` and lazy native loading) as a new U item. | Default in force |
| Q-P01c | Build inputs for `xtask engine`: engine dependencies are installed with `npm ci --ignore-scripts` (skips Chromium download and husky), the Node binary is the host's official build (must satisfy the engine's `engines.node`), and macOS/Windows signing is a placeholder. | P01 | macOS: ad-hoc `codesign` (or `CL_CODESIGN_IDENTITY`) after injection; any OS: `CL_SIGN_HOOK=<program>` is run with the exe path. Real Developer ID / Authenticode signing and notarization land in P35. Node's runtime license text goes into ATTRIBUTION.md in P35. CI smoke on macOS/Windows installs or finds PostgreSQL 16 itself; not yet run on those runners (first CI run confirms). | Default in force |
| Q-P03a | The pinned zonky archives (Q-06) contain only `postgres`, `initdb`, and `pg_ctl`: no `pg_isready`, `pg_dump`, `pg_restore`, or `psql`. The P03 spec names all four. | P03 | `cl-pg` does not need a client program: readiness is a startup-packet probe in Rust (what `pg_isready` does), and tests use the `postgres` crate (dev-dependency only). Whole-cluster "dump/restore" is a **cold physical copy** (stop, tar.gz the data directory, verify by reading it back, restart), valid for the same PostgreSQL major version, OS, and CPU; restore unpacks and checks first and keeps the replaced data directory. Company backups still use the engine's tenant export (Q-05). A logical `pg_dump` is not available; if P34/P36 need cross-version moves, add a second pinned source then. Considered and rejected: theseus-rs full builds (the Linux client tools link system OpenSSL, readline, and Kerberos, so they are not self-contained). | Default in force; revisit in P34/P36 |
| Q-P03b | Cluster locale: the spec says C.UTF-8. PostgreSQL 16 on Windows rejects the name `C.UTF-8`. | P03 | `initdb` is tried with `--locale=C.UTF-8` first and falls back to `--locale=C` (database encoding is UTF8 either way). On Windows this means byte-order (`C`) collation, so SQL `ORDER BY` on text sorts uppercase before lowercase and `upper()/lower()` are ASCII-only. P04/P02 should confirm the engine does not rely on locale-aware SQL sorting; list screens sort in the UI. | Default in force |
| Q-P03c | Where the database password lives and what the fallback protects. | P03 | OS keychain via the `keyring` crate (v4, v1 API; Windows Credential Manager, macOS Keychain, Secret Service), keyed by service `ClientLedger` and a hash of the data directory. `default_store` probes the keychain with a real write/read/delete; if that fails it uses an encrypted file `<data dir>.secret` beside the data directory (ChaCha20-Poly1305, key derived from user name, computer name, and the Linux machine id) and returns a `StoreNotice` for the UI to show (text through `cl-ui::i18n` in P09). The file fallback only stops the file being useful on another computer or account; it does not stop other programs running as the same user. A cluster that already has the fallback file keeps using it. The password is also passed to `initdb` through a 0600 temporary file that is deleted immediately, and to clients through the connection URL only. | Default in force |
| Q-P03d | Windows administrator accounts (hosted CI runners are elevated): `postgres.exe` refuses to run with administrator rights. | P03 | The server is always started with `pg_ctl start` (which drops rights); `initdb` already works elevated. `Cluster::reset_password` runs `postgres --single` directly and so cannot work elevated on Windows; its test reports a skip in that case (it runs on Linux and macOS CI). The per-user installer (P35) runs unelevated, so this does not affect owners. | Default in force |
| Q-P03e | The zonky Windows archive ships `libintl-9.dll` and `libiconv-2.dll` (GNU gettext/libiconv, LGPL) and the Linux/macOS archives bundle OpenSSL, ICU, libxml2 and others. CLAUDE.md non-negotiable 2 is written for Rust dependencies. | P03 | Keep them: they are separate dynamically loaded libraries inside the unmodified upstream archive, replaceable by the owner, and listed in ATTRIBUTION.md. The P35 packaging audit must confirm each library's license text ships with the installer and decide whether any LGPL library needs a written source offer; owner to confirm before the beta release. | Open: owner confirmation before P39 |
| Q-P03f | `xtask engine --smoke` still looks for a system PostgreSQL (P01). | P03 | Left as is: xtask may depend on no internal crate, and the smoke run needs a trust-auth throwaway server that `cl-pg` deliberately cannot produce. When P04 lands, the smoke run can use `target/pg` from `pg-fetch` (binary path only) to drop the Docker/brew/apt setup in CI. | Default in force |
| Q-P03g | Test and CI conventions for PostgreSQL-backed tests. | P03 | Tests find binaries in `target/pg/<target>` (written by `cargo xtask pg-fetch`, the only networked step) or `CL_PG_ROOT`. Without them a test prints `SKIPPED` and passes locally; `CL_PG_REQUIRE=1` (set in CI) turns the skip into a failure. The "fresh ensure + start" test asserts under 10 s (`CL_PG_BUDGET_SECS` overrides); initdb dominates (about 3 s alone on a Windows laptop). `CL_PG_KEEP=1` keeps the scratch folder of a failing test. | Default in force |
