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
| Q-31 | The MyBooks engine stays **PolyForm Internal Use 1.0.0** **unmodified** (no ClientLedger grant; owner decision 2026-10-15, supersedes the grant plan). Only the licensor, Kisaes LLC, distributes the engine, inside official ClientLedger releases. The engine repo will be **public**. | P00, P01, P39 |
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
| Q-132 | Scope follows PolyForm Internal Use: ClientLedger users may use the bundled engine for the **internal business operations of their own organization** (their own books). Bookkeeping for others is not internal use and needs a separate license from Kisaes LLC. The earlier explicit allowance for outside accountants is gone with the grant; ordinary file transfer to the accountant is unaffected. | NOTICE.md, P38 docs |
| Q-133 | Only **Licensor's official releases** may include the engine, because recipients may not distribute it. Forks of the MIT code may not bundle, download, or install the engine. | NOTICE.md, P35 |
| Q-134 | Installers show a **license acceptance screen** (MIT + PolyForm Internal Use). | P35 |
| Q-135 | **Short privacy notice** in the app and on ClientLedger.net. | P06, P38 |
| Q-136 | Cloud AI enablement records **dated consent in the audit trail**. | P09 |
| Q-137 | The engine's internal-business-use scope (Q-132) appears **in About and docs only**. | P06, P38 |
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
| Q-35 | Engine redistribution grant. | P01 | **Closed 2026-10-15:** the owner states the engine is fully licensed for ClientLedger's use (Internal Use license, no grant); engine licensing is settled and is not to be raised again. `docs/legal/LICENSE-CLIENTLEDGER.draft.md` is kept for reference only. | Closed |
| Q-P00a | Layering for crates CLAUDE.md does not constrain (`cl-host`, `cl-company`, `cl-print`, `cl-automation`, `cl-lan`), and egui usage. | P00 | Each may depend on any library crate below `cl-ui` (never `cl-ui` or apps). Only `cl-grid`, `cl-ui`, and apps may depend directly on egui/eframe, per "nothing below `cl-ui` depends on egui except `cl-grid`". If `cl-automation` needs egui for widget inspection (P07), amend the table in `xtask/src/layering.rs` and CLAUDE.md together. Table lives in `xtask/src/layering.rs`. | Default in force |
| Q-P00b | `cargo xtask test` sets `CL_OFFLINE_TESTS=1` but cannot block the network itself. | P00 | The variable is a convention: test helpers that could open sockets must check it and refuse. Real network blocking (sandbox or firewall in CI) is deferred to when the first networked test exists (P05). | Default in force |
| Q-P00c | Secret scanner choice (permissive license required). | P00 | gitleaks (MIT) pinned binary v8.18.4 run directly in `ci.yml`, not the licensed GitHub Action. Not run locally; first CI run confirms the download URL and a clean scan. | Default in force |
| Q-P01a | The pinned engine (13ebd2b) cannot satisfy "`/health` reports the pinned engine commit" or run without Redis: `/health` returns 503 `degraded` without Redis and its body has no commit field. | P01 | Not an engine edit in P01. U-02 (Redis-free desktop mode) and U-03 (`/health`) in P02 must also add `engineCommit` (set at build time from the submodule commit, e.g. `CL_ENGINE_COMMIT`) to the `/health` body. Until then `manifest.json` is the source of the pinned commit and `xtask engine --smoke` accepts HTTP 503 when `"db":"ok"` (Redis degraded, expected) and prints a note. The smoke test must be tightened to require 200 and the commit in P02. | Resolved 2026-10-15: E1 (engine `clientledger/desktop-mode`) adds `engineCommit` to `/health` and runs without Redis in desktop mode; `ENGINE_COMMIT` is baked into the SEA banner by `bundle.mjs`; `xtask engine --smoke` now runs the engine in desktop mode with a launch token and a non-superuser role, and requires HTTP 200 and `engineCommit` equal to the manifest commit (no 503 allowance). |
| Q-P01b | The engine is not SEA-ready as written (native `bcrypt` addon, migrations/knowledge/fonts read from disk via `import.meta.url`, static `puppeteer` imports, ESM). | P01 | `xtask/engine-tools/bundle.mjs` works around it without editing the engine: esbuild bundle to CJS, `import.meta.url` defined to a per-bundle runtime dir, assets and the host-platform bcrypt prebuild embedded as SEA assets and extracted on first start (`CL_ENGINE_RUNTIME_DIR` overrides the location; P04 should set it inside the app data dir), `puppeteer` stubbed (PDF output is `cl-print`, P19; any engine route that renders HTML to PDF fails with a clear message), optional native accelerators (`@napi-rs/canvas`, `msgpackr-extract`, etc.) left out. If this proves brittle, propose an upstream change (generic `ENGINE_ASSET_DIR` and lazy native loading) as a new U item. | Default in force |
| Q-P01c | Build inputs for `xtask engine`: engine dependencies are installed with `npm ci --ignore-scripts` (skips Chromium download and husky), the Node binary is the host's official build (must satisfy the engine's `engines.node`), and macOS/Windows signing is a placeholder. | P01 | macOS: ad-hoc `codesign` (or `CL_CODESIGN_IDENTITY`) after injection; any OS: `CL_SIGN_HOOK=<program>` is run with the exe path. Real Developer ID / Authenticode signing and notarization land in P35. Node's runtime license text goes into ATTRIBUTION.md in P35. CI smoke on macOS/Windows installs or finds PostgreSQL 16 itself; not yet run on those runners (first CI run confirms). | Default in force |
| Q-P03a | The pinned zonky archives (Q-06) contain only `postgres`, `initdb`, and `pg_ctl`: no `pg_isready`, `pg_dump`, `pg_restore`, or `psql`. The P03 spec names all four. | P03 | `cl-pg` does not need a client program: readiness is a startup-packet probe in Rust (what `pg_isready` does), and tests use the `postgres` crate (dev-dependency only). Whole-cluster "dump/restore" is a **cold physical copy** (stop, tar.gz the data directory, verify by reading it back, restart), valid for the same PostgreSQL major version, OS, and CPU; restore unpacks and checks first and keeps the replaced data directory. Company backups still use the engine's tenant export (Q-05). A logical `pg_dump` is not available; if P34/P36 need cross-version moves, add a second pinned source then. Considered and rejected: theseus-rs full builds (the Linux client tools link system OpenSSL, readline, and Kerberos, so they are not self-contained). | Default in force; revisit in P34/P36 |
| Q-P03b | Cluster locale: the spec says C.UTF-8. PostgreSQL 16 on Windows rejects the name `C.UTF-8`. | P03 | `initdb` is tried with `--locale=C.UTF-8` first and falls back to `--locale=C` (database encoding is UTF8 either way). On Windows this means byte-order (`C`) collation, so SQL `ORDER BY` on text sorts uppercase before lowercase and `upper()/lower()` are ASCII-only. P04/P02 should confirm the engine does not rely on locale-aware SQL sorting; list screens sort in the UI. | Default in force |
| Q-P03c | Where the database password lives and what the fallback protects. | P03 | OS keychain via the `keyring` crate (v4, v1 API; Windows Credential Manager, macOS Keychain, Secret Service), keyed by service `ClientLedger` and a hash of the data directory. `default_store` probes the keychain with a real write/read/delete; if that fails it uses an encrypted file `<data dir>.secret` beside the data directory (ChaCha20-Poly1305, key derived from user name, computer name, and the Linux machine id) and returns a `StoreNotice` for the UI to show (text through `cl-ui::i18n` in P09). The file fallback only stops the file being useful on another computer or account; it does not stop other programs running as the same user. A cluster that already has the fallback file keeps using it. The password is also passed to `initdb` through a 0600 temporary file that is deleted immediately, and to clients through the connection URL only. | Default in force |
| Q-P03d | Windows administrator accounts (hosted CI runners are elevated): `postgres.exe` refuses to run with administrator rights. | P03 | The server is always started with `pg_ctl start` (which drops rights); `initdb` already works elevated. `Cluster::reset_password` runs `postgres --single` directly and so cannot work elevated on Windows; its test reports a skip in that case (it runs on Linux and macOS CI). The per-user installer (P35) runs unelevated, so this does not affect owners. | Default in force |
| Q-P03e | The zonky Windows archive ships `libintl-9.dll` and `libiconv-2.dll` (GNU gettext/libiconv, LGPL) and the Linux/macOS archives bundle OpenSSL, ICU, libxml2 and others. CLAUDE.md non-negotiable 2 is written for Rust dependencies. | P03 | Keep them: they are separate dynamically loaded libraries inside the unmodified upstream archive, replaceable by the owner, and listed in ATTRIBUTION.md. The P35 packaging audit must confirm each library's license text ships with the installer and decide whether any LGPL library needs a written source offer; owner to confirm before the beta release. | Open: owner confirmation before P39 |
| Q-P03f | `xtask engine --smoke` still looks for a system PostgreSQL (P01). | P03 | Left as is: xtask may depend on no internal crate, and the smoke run needs a trust-auth throwaway server that `cl-pg` deliberately cannot produce. When P04 lands, the smoke run can use `target/pg` from `pg-fetch` (binary path only) to drop the Docker/brew/apt setup in CI. | Default in force |
| Q-P03g | Test and CI conventions for PostgreSQL-backed tests. | P03 | Tests find binaries in `target/pg/<target>` (written by `cargo xtask pg-fetch`, the only networked step) or `CL_PG_ROOT`. Without them a test prints `SKIPPED` and passes locally; `CL_PG_REQUIRE=1` (set in CI) turns the skip into a failure. The "fresh ensure + start" test asserts under 10 s (`CL_PG_BUDGET_SECS` overrides); initdb dominates (about 3 s alone on a Windows laptop). `CL_PG_KEEP=1` keeps the scratch folder of a failing test. | Default in force |
| Q-P03h | A cluster backup is a copy of a live data directory, so a crafted archive could run code on restore (security review of P03). Which archives may `Cluster::restore` accept? | P03 | Default: only archives this install wrote. `dump` records each backup's SHA-256 in `<data dir>.backups`; `restore(src, store, allow_unregistered)` requires a match unless the caller passes `allow_unregistered = true`, which the UI (P34/P08) sets only after the owner confirms the file came from a source they trust (a backup moved from an old computer, for example). Even then restore accepts only the known PostgreSQL 16 top-level entries, rebuilds `postgresql.conf` (keeping only harmless initdb choices), `postgresql.auto.conf`, `pg_ident.conf`, `pg_hba.conf` and `clientledger.conf`, always replaces the superuser password (and undoes the swap if that fails), and the server is started with the security settings pinned on the command line. Residual risk when `allow_unregistered` is true: a hostile archive can still carry crafted catalog data (for example a C function pointing at a library placed inside the data folder). `Cluster::restore` is documented as trusted-input-only. | Default in force |
| Q-P02h | Where do users live, given the engine gives every user a NOT NULL home tenant and a ClientLedger company is one engine tenant? | P02, P08, P33, P34 | **Owner decision 2026-10-15 (option A):** setup (`/desktop/initialize`) creates one hidden, never-deletable, never-exported owner workspace tenant that holds all user rows (owner and LAN users); every real company is an ordinary tenant reached through user access rows. Deleting a company never touches user rows other than its access rows. Deleting the last company is allowed (app shows Open Company). Company archives include password hashes of users with access, protected by the owner's backup passphrase (Q-P02c); on import, created users get the destination workspace as home tenant and users matched by email are never modified. | Decided |
| Q-P04a | What database role does the engine use, and where do its secrets live? (Security review: the engine must not connect as the cluster superuser.) | P04 | `cl-pg` creates, on every start and idempotently (`Cluster::ensure_app_database`), the role `clientledger_app` (`LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS`, no role memberships, so none of `pg_execute_server_program`, `pg_read_server_files`, `pg_write_server_files`) that owns the database `clientledger`; a widened role, an extra membership or a changed owner is repaired on the next start. `PUBLIC` loses all rights on `clientledger`, `postgres` and `template1`, so the role can connect to nothing else. The engine runs with `CL_REQUIRE_NON_SUPERUSER=1` and the host also refuses an engine whose `/health` does not report `superuser:false`. Secrets: one random 256-bit master secret per cluster in the OS keychain (or the encrypted-file fallback, `<data dir>.host-master.secret`, Q-P03c); the role password, `JWT_SECRET`, `ENCRYPTION_KEY` and `PLAID_ENCRYPTION_KEY` are SHA-256 derivations of it with distinct labels (`Secret::derive`), so they are stable across runs and are never written to disk, to the engine's data folder, to logs, or to a command line. Losing the keychain entry means the engine cannot decrypt anything it encrypted with `ENCRYPTION_KEY`; desktop mode keeps few such values (AI and SMTP secrets are per session, U-09), and P34 should include the master secret in its recovery guidance. | Default in force |
| Q-P04b | Which paths may the engine receive? | P04 | Only absolute ones: `AppPaths::at` refuses a relative root, so `CL_DATA_DIR`, `CL_LOG_FILE` and `CL_ENGINE_RUNTIME_DIR` can never be relative, and `build_env` re-checks them and the engine program path. The engine unpacks its embedded assets (including the native `bcrypt` addon, Q-P01b) into `<app data>/engine-runtime`; that folder is writable by the owner's own programs, which is inside the same trust boundary as the app data folder (P35 may add a checksum check of the unpacked files). The engine also gets private `TEMP` and `HOME` folders under the app data folder, so it never reads the owner's own files. | Default in force |
| Q-P04c | How does a second launch reach the first instance (focus, CLI status and stop)? | P04 | An exclusive OS file lock on `run/instance.lock` (released by the OS if the process dies) plus a loopback-only TCP listener whose port and a random 256-bit key are published in `run/instance.ctl` (mode 0600 inside a 0700 folder on Unix; the per-user `%LOCALAPPDATA%` ACL on Windows). A request is one line `<key> focus`, `<key> status` or `<key> stop`; a wrong key gets `denied` and changes nothing. Chosen over named pipes and Unix sockets because it is one safe code path on every OS with no `unsafe` code and no extra dependency. A program that grabbed the port after a crash learns the key only if a second launch connects to it, and the key opens nothing but these three harmless commands; `status` never includes ports or tokens. | Default in force |
| Q-P04d | How are children started and stopped, and what do they inherit? | P04 | The engine's environment is built from scratch (`env_clear`): the desktop contract variables, the secrets, private `TEMP` and `HOME`, a minimal `PATH` (`%SystemRoot%\System32`, or `/usr/bin:/bin`), and on Windows `SystemRoot` and `windir`; nothing else is inherited. It has no command-line arguments at all, so no secret can show in a process listing (tested through the OS process list). Windows: no console window, and the engine runs in a job object with kill-on-close (the whole tree dies with the host, even after a host crash; the `win32job` crate wraps the unsafe calls, so `cl-host` keeps `forbid(unsafe_code)`). Unix: own process group, `SIGTERM` to the group, `SIGKILL` after `stop_grace` (10 s) (`nix` crate). Windows has no graceful stop for a windowless process and the engine has no shutdown endpoint, so there the job is terminated at once (PostgreSQL transactions are atomic and the database is stopped cleanly afterwards); a drain endpoint is part of the engine follow-up U-16. The host records the engine's pid, start time and path in `run/engine.pid`; a later start kills a leftover engine only when all three match (never a recycled pid). PostgreSQL is deliberately not tied to the host's life: after a host crash it keeps running and the next start adopts it (`cl-pg`). Stop order is engine, then database. | Default in force |
| Q-P04e | What if another program takes the engine's port (security review L11)? | P04 | Before the launch token is sent anywhere, `verify_listener` asks the OS (`listeners` crate) who listens on the port: every listener must be the engine process we started and bound to loopback. It is checked after start and every 15 s, and a `401` to our token is treated as "not our engine". A failure counts as an engine failure (restart on a new port with a new token; the token is fresh for every launch). Residual risk, accepted: the window between the engine dying and the next poll (250 ms for exit, 2 s for health), and the fact that any process of the same OS user can read the child's environment or probe the port. Stronger proof (an HMAC challenge over a host nonce in `/health`, so the host can verify the responder knows the token without sending it) is filed as engine follow-up U-16 (proposed). The database port is not verified this way: its login is SCRAM, which does not reveal the password to an impostor. | Default in force |
| Q-P04f | Restart rules. | P04 | Each component may be restarted 3 times (4 start attempts in all) with backoff 0.5 s, 1 s, 2 s, then `HostFault` with the host, engine and database log paths and the last scrubbed output. Counters reset after `stable_after` (default 120 s) of healthy running. Engine exit code 78 (`EX_CONFIG`), a missing program, a superuser role or a wrong engine commit are permanent: no retries. A dead database restarts first and does not use up the engine's budget; the engine is restarted afterwards because the database comes back on a new port. Health is polled every 2 s (3 s timeout) and 3 failed polls in a row count as a hung engine. The UI shows the one-line reconnect notice from `HostState::Restarting`. | Default in force |
| Q-P04g | The pinned engine build (E1 `8213777e`) predates the workspace tenant (Q-P02h): its `/initialize` answers `{tenantId, userId}` and `/info` has no `workspaceTenantId`. | P04 | `initialize_engine` understands both shapes: `Initialized.workspace_tenant_id` is `None` for the old answer and its `tenantId` is returned as `company_tenant_id`. `/info` with `"initialized": null` (the engine cannot read its database) is an error, not "false". The real-engine test accepts either shape. Remove the legacy branch when the engine pin moves past the workspace commits. | Default in force |
| Q-P04h | Is the time budget (15 s cold, 5 s warm) realistic? | P04 | Measured on a Windows laptop: cold 12.6 s (new cluster about 5 s, engine migrations about 8 s), warm 2.2 s. Cold is close to the limit; `CL_HOST_BUDGET_SECS` overrides it (warm is a third of it) and the two real-engine tests run one at a time so they do not compete. If hosted CI runners miss 15 s, raise the CI value rather than weaken the local check, and ask the engine for a faster first migration (squashed baseline) as a follow-up. | Default in force |
| Q-P04i | Test and CI conventions for `cl-host`. | P04 | Supervisor logic (ordering, restarts, backoff, faults, token and environment handling, first-run calls) is tested against `examples/fake_engine.rs` and a fake database, on every OS, with no extra downloads. Tests with the real PostgreSQL and the real engine executable (`target/engine/<target>/clientledger-engine[.exe]`, built by `cargo xtask engine`) print `SKIPPED` when either is missing, and fail when `CL_HOST_REQUIRE=1`; CI sets it only in the `engine` job (which builds the executable and fetches PostgreSQL). Tests keep secrets in `MemoryStore`s so they never touch the owner's keychain; `CL_HOST_KEEP=1` keeps a failing test's folder. | Default in force |
| Q-P04j | Files and dependencies beyond the P04 file list. | P04 | Besides `lib.rs`, `instance_lock.rs`, `supervisor.rs`, `health.rs`, `logs.rs`, `paths.rs` and `apps/clientledger-cli/src/host.rs`, `cl-host` has `database.rs` (trait plus the `cl-pg` implementation), `engine.rs` (environment and launch), `procs.rs` (job object, process groups, leftover cleanup), `secrets.rs`, `http.rs` and `desktop.rs` (a tiny loopback HTTP client for `/health`, `/info`, `/initialize`; `cl-api` in P05 is the real client). New permissive dependencies: in `cl-host` `serde_json`, `sysinfo`, `listeners` (MIT), `win32job` (Windows, MIT or Apache-2.0) and `nix` (Unix, MIT); in the CLI `ctrlc`; in `cl-pg` the `postgres` crate moved from dev to normal dependency. `cl-host -> cl-pg` is allowed by the existing layering table (Q-P00a), so the table is unchanged. | Default in force |
| Q-P04k | Log retention. | P04 | `host.log` rotates at 5 MiB keeping 3 files; the engine rotates `engine.log` itself (10 MiB, 4 files). The host prunes the logs folder at start and hourly: closed rotated files (`*.log.N`) older than 30 days go, then the oldest go while the folder exceeds 100 MB; active `*.log` files are never deleted. Files are opened the way the Rust standard library does on Windows (`FILE_SHARE_DELETE`), tested with a handle held open. PostgreSQL's own logs (`pgdata/log`, one file per weekday, truncated on rotation) and `<data dir>.startup.log` stay where `cl-pg` puts them and are small by construction. | Default in force |
| Q-P04l | Security review M3: on Windows, a data folder chosen by the owner (portable or custom location, P08/P09) inherits the parent's ACL, which on most volumes grants Authenticated Users Modify. The default `%LOCALAPPDATA%\ClientLedger` is per-user and safe. | P04 review | Recorded, not implemented. Before any custom or portable folder reaches users (P35 installer, P09 preferences, P08 company folders), either set an owner-only ACL on the root (protected, no inheritance) or refuse roots whose ACL grants other users write access, with a plain message. Note, as in Q-P03c: the encrypted-file fallback key is derived from user and computer names, so it protects against copying the file elsewhere, not against another local user who can read it; on a folder others can write, that is no protection. | Open (target P35 / P09) |
| Q-P04m | Security review L3: when the operating system cannot list sockets (`verify_listener` reports an error rather than "not ours"), the host logs a warning and carries on. | P04 review | Default today: fail open with a warning, because the check is defense in depth and a platform quirk must not brick the app. Follow-up: after N (suggest 3) consecutive failed lookups, fail closed (treat as an engine failure) and show the owner a plain message. Target: P04 follow-up or P37 hardening. | Open (target P37) |
| Q-P04n | Security review L4: `info`/`initialize` and every later `cl-api` request send the launch token to whatever answers on the recorded port. | P04 review | Recorded for P05 (`cl-api`): re-read `Supervisor::endpoint()` (port, token and `generation`) for every request, never cache it across a restart; treat a generation change as "reconnect"; and call `verify_listener(port, engine pid)` before the first request after a (re)start. Host side, `info`/`initialize` should run `verify_listener` first; today only the poll loop and start-up do, so that is a documented gap (window: one poll interval). | Open (target P05) |
| Q-P04o | Security review L8 and Info items. | P04 review | Recorded, not implemented: (1) engine lifetime tied to the host: the engine exits when its stdin closes (the host keeps the write end) and, on Linux, sets `PR_SET_PDEATHSIG` (engine change, added to U-16); the host should write `run/engine.pid` atomically with the spawn, not just after it, so a crash in between cannot orphan an unrecorded engine; (2) start the engine suspended and assign it to the job object before it runs (std has no suspended start; needs a crate or a small launcher), and fail the start if the job object cannot be created instead of continuing with a plain kill; (3) the engine must not enable Node diagnostic reports (they can contain environment variables): keep `--report-*` off and `NODE_OPTIONS` out of the environment (already refused, Q-P04p); (4) exclude the engine from Windows Error Reporting dumps (`WerAddExcludedApplication` at install time, P35); (5) secrets never rotate: the master secret, the role password and the derived keys stay for the life of the data folder; P34 recovery guidance must say so and offer rotation only through an engine re-encryption path. | Open (targets P35, P34, U-16) |
| Q-P04p | Decisions made when fixing the P04 security review (M1, M2, L1, L2, L5-L7, L9-L11). | P04 review | **M1** `find_engine_exe`: explicit path, `CL_ENGINE_EXE`, the program's own folder (and its `engine/` subfolder), and in debug builds only the `target/engine/<host>/` of the cargo `target` folder that contains the program; the current folder and arbitrary parents are never searched; on Unix the file and its folder must be owned by the user or root and not group/world-writable (Windows has no equivalent check here, see Q-P04l). **M2** a master secret is created only when the cluster does not exist yet; otherwise a missing secret is `MasterSecretMissing` (a `HostFault` with recovery guidance), and a non-secret fingerprint in `run/master.fp` detects a different secret (`MasterSecretMismatch`). **L1** the role password reaches PostgreSQL as a SCRAM verifier computed in the host, and the server is started with `log_min_error_statement=panic` pinned. **L2** `pg_hba.conf` allows only `postgres` to database `postgres` and `clientledger_app` to database `clientledger` (so the superuser can never open the books, and `ensure_app_database` with other names would not be able to log in); role repair revokes each membership under its own grantor and fails if any remains, resets role and database settings, and clears an expiry. **L5** chunk sizes are overflow-checked, a panic in the supervisor thread becomes a `HostFault` and the engine is terminated when its handle is dropped. **L6** engine output is read as bytes with lossy UTF-8 and a 64 KiB line cap. **L7** at most 8 control connections at a time, each on its own thread; replies and log lines are stripped of control characters before printing. **L9** `HostConfig::with_engine_env` exists only with the `test-support` feature (a self dev-dependency, which `xtask` layering now allows) and accepts only `FAKE_*` names. **L10** `AdminAccount` Debug shows nothing personal. **L11** `Supervisor::spawn/start` take and keep the `InstanceGuard`, so one user cannot run two supervisors. | Default in force |
| Q-P05t | `cl-core` money details the P05 plan leaves open. | P05 | (1) `Money`/`Quantity`/`Rate` have no `Display` (use `format` for owners, `to_wire` for the engine) so an amount is never formatted into a log line by accident; `Debug` shows the wire value for tests and is never passed to `tracing`. (2) Arithmetic runs on exact `i128` ten-thousandth units and every result is rebuilt through one normalizing constructor, so zero never carries a sign. (3) "No `to_wire()` output starts with `-0`" (plan P10) is read as "never `-0.0000`": `-0.5000` is a real negative amount; the property asserts a leading `-` appears exactly when the value is negative. (4) Display minimum decimals: 2 for `Money`, 0 for `Quantity` and `Rate`; more digits only when non-zero (Q-P05l). (5) Added `Money::mul_rate_round` (sales tax previews, same half-away-from-zero rule as `mul_round`), `Money::split(n)`, `to_ten_thousandths()`, and `MoneyError::NotWholeUnit` (a sub-cent total cannot be split in cents). (6) `parse_user` accepts `-` and `$` in either order (`-$5`, `$-5`), and leading zeros. (7) cl-core has no random source: the `uuid` `v4` feature is not enabled there; cl-api generates request ids and wraps them with `RequestId::from_uuid`. | Default in force |
| Q-P05u | How is it proven that each `clippy.toml` `disallowed-methods` entry really fires (plan R19)? | P05 | `crates/cl-core/tests/float_ban.rs` calls every banned method once under `#[expect(clippy::disallowed_methods)]`; an entry that stops resolving leaves the expectation unfulfilled, which fails `clippy -D warnings`. Verified by hand: removing `ToPrimitive::to_f64` and misspelling `FromPrimitive::from_f32` each produced "this lint expectation is unfulfilled" errors (a misspelled path alone is only a non-fatal config warning, which is exactly the R19 hole). `rust_decimal::Decimal::as_f64` (an inherent method) was added to the list. Conversions through `TryFrom` on primitives and `as` casts cannot be listed; the float lints and the xtask `nofloat` AST scan (step 10) cover them. cl-api copies the same mechanism in step 13. | Default in force |
