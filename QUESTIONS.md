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
| Q-122 | **Claude Code drafts original icon options**; option 1 used until the owner picks. Owner picked option 1 (Q-257). | P06 |
| Q-123 | App and installer name: **ClientLedger**. | P06, P35 |
| Q-124 | **No imports from other apps** (QBO, Wave, Xero) in beta. | P31 |
| Q-125 | **Mapped CSV import** of invoices, bills, and journal entries. | P31 |
| Q-126 | **IIF export** of lists and journal entries for QuickBooks Desktop. | P32 |
| Q-127 | **"New from existing"** company copies setup without transactions or contacts. P08 copies the chart as a flat list (sub-accounts lose their parent; P11's chart screen restores hierarchy on copy), settings and the legal form. | P08, P11 |
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

## Owner Q&A 2026-10-19

Answers from the owner after P11. They replace the defaults named below; later phases must follow them.

| ID | Question | Owner answer | Applies to |
| --- | --- | --- | --- |
| Q-230 | When is pending engine work (U-31…U-47, U-15 closing-date password, U-23 recovery key, E5b safe delete) built? | **Just-in-time:** each engine item is built and opened as an engine PR right before the first phase that needs it (for example U-15 before P12/P13, U-39 item types before P20). Nothing moves past beta. | Orchestrator sequencing from P12 on |
| Q-231 | Merges (contacts, tags) that rewrite transactions in a closed period. | **Require the closing-date password**, recorded in the audit trail (confirms Q-P11aa; engine U-47). | P11 merges, U-47, P29 |
| Q-232 | An amount typed directly with more than two decimals (e.g. `12.345`) in a money field. | **Round to the cent, half away from zero** (12.345 → 12.35), the same rule as calculator results (Q-P10x). Replaces the refusal. Fields with a wider scale (item prices, 4 decimals) round to their own scale. | `cl_model::amount_entry` plain parse; all money fields |
| Q-233 | Several tags on one transaction line (one per group)? | **No: one tag per line**, matching the engine's single `journal_lines.tag_id`. Header (transaction-level) tags still allow one per group. U-45 is withdrawn; the "one tag per line" rule in Q-P10t / Q-P11w becomes permanent, not a stopgap. | cl-grid tag cell, forms P13+, U-45 |
| Q-234 | Should a company backup made by a company admin (not the owner) include other users' password hashes? | **Always include** every user's password hash in company backups, whoever makes the backup, so all users can sign in after a restore on another PC. Replaces the H4 default on engine branch `clientledger/tenant-portability` (hashes only for a super-admin caller); that engine fix must be changed before PR #151 merges. The backup passphrase protects the archive; the backup screen should say that the backup contains everyone's sign-in data. | Engine E5a (U-05), P08 backup screen text |
| Q-235 | How do jobs work in the Client center? | **A separate jobs list** linked to a client (not QuickBooks-style sub-customers), with status, type and start/end dates. U-35 is built as a jobs table. QBD import (P31) maps `Customer:Job` to client + job. | U-35, P11 Client center Jobs tab, P20 invoices, P31 |
| Q-236 | Finish the unpushed engine security fixes (E5a #151, E6 #152) now on Opus, or wait for Fable? | **Opus now**, before the first phase that needs them (Fable is out of credits). | Engine PRs #151, #152 |
| Q-237 | Show a hint when an amount is rounded to the cent? | **No hint**; round silently. | Q-P10x, Q-232 |
| Q-238 | Owner-facing word for the people a business sells to: "Clients" or "Customers"? | **Customers.** Every owner-facing label says Customer/Customers, never Client/Clients: the menu ("Customers"), the left rail and list window ("Customers & Jobs"; amended by Q-258: never "Customer Center"), "Customer / Job" (Q-255), form fields ("Customer", "Received from" unchanged), reports ("Customer Balance Summary", A/R aging by customer), CSV column names, help text and error messages. This wins over the mockups and DESIGN.md (Q-226). Code identifiers (`client_center`, command ids such as `lists.clients`) may keep their names until touched, but i18n keys and all displayed strings change; the product name ClientLedger is unaffected. Apply in one pass at the start of P12: i18n catalog, DESIGN.md wording, KEYBOARD.md (regenerate), scenarios' label assertions, docs/user. | cl-ui i18n, DESIGN.md, scenarios, P12+ |
| Q-239 | Forgotten closing-date password: how is it reset? | **The owner (super admin) can clear or replace it** after re-entering their own sign-in password; the reset is recorded in the audit trail. Engine change on PR #153 (U-15). | U-15, P12 closing flow, P29 |
| Q-240 | Transaction delete in P12 (needs engine soft delete R7). | **Build it in P12**: engine soft delete (reversing lines, `deleted` status hidden from registers and reports, audit before-image with all lines, closing-date and reconciliation guards, closing password) and the register Delete with confirmation. | Engine R7 on PR #153, P12 WP8, AC3 |
| Q-241 | Credit-card register running balance sign. | **Amount owed, shown positive**: charges raise it, payments lower it (confirms Q-P12h). | P12 register, cards |
| Q-242 | Suggest the next check number in the register entry row? | **Yes**: for a Check, propose the next number after the highest check in the account; the owner can overwrite it (confirms Q-P12q). | P12 entry row, P13 Write Check |
| Q-243 | Who merges engine (Vibe-MyBooks) pull requests? | **The orchestrator merges** each engine PR itself once its security review is clean (Blocking/High findings fixed) and the engine repo's CI passes, in migration order, then bumps ClientLedger's engine pin in a normal PR. Replaces the owner-review step of Q-29 for engine PRs; the owner can still review any PR before it merges. | Engine PRs #151, #152, #153 and later; pin bumps |
| Q-244 | Should the owner (super admin) see every company on this computer in Open Company, even without an access row? | **Yes, show all** companies to the owner/super admin on a desktop install. | Engine `GET /api/v1/tenants` (E5a), P08 Open Company |
| Q-245 | Which transaction types can be deleted (engine R7)? | Default (engine agent's call, owner may change): only journal entries, checks/expenses, transfers, deposits and sales receipts. Invoices, bills and payments are refused (`TXN_DELETE_UNSUPPORTED`) and deposits that group payments are refused (`TXN_DELETE_LINKED`), because deleting them would leave linked records inconsistent; those are voided instead. | P12 Delete, P17–P21 forms |
| Q-246 | Check number for a "Print later" check before printing. | **Shows "To print"**; the number is assigned at print time in print order (P19). | P13 Write Check, P18, P19 |
| Q-247 | Items tab on Write Check before item types exist (P20). | **Hidden until P20**; P13 has the Expenses tab only. | P13, P20 |
| Q-248 | Strength of the open-bill warning on Write Check. | **Side panel only**: lists the open bills with "use Pay Bills instead"; saving is not interrupted. | P13 |
| Q-250 | How do list fields (payee, account, customer, tag, class…) open their list? | **Clicking the field opens the dropdown list** right away (owner, 2026-10-20, from exploring the beta); typing still filters, and Ctrl+L still opens the full list. Applies to every autofill field in forms, grids, the register entry row and filters. | All forms and grids |
| Q-251 | Fill-in-the-amounts templates for journal entries and daily sales (engine has /api/v1/je-templates)? | **Keep the plan**: Templates only (formerly "memorized transactions", Q-256; P15; P14's daily sales summary can be saved as a template). No engine fill-in-the-amounts JE or daily sales templates (`/api/v1/je-templates`) in beta. | P14, P15 |
| Q-252 | Bulk entry of transactions (engine /api/v1/batch: validate, save, suggest-account, parse-csv; types expense, credit_card_charge, deposit, credit_card_credit, invoice, bill, credit_memo, journal_entry, customer_payment) in beta? | **Yes**: a new beta phase **P15A — Bulk Entry** (renamed by Q-256) right after P15, Tier M, on the P10 grid; moved out of post-beta P45. | P15A, P45 |
| Q-249 | "Save as Template" on Write Check (formerly Memorize, Q-256). | **Skipped in P13**; the Save as Template button arrives with the Templates work in P15. | P13, P15 |
| Q-254 | "QuickReport" is an Intuit feature name (`Quick*` family; compliance review 2026-10-20, M-1). What do we call it? | **"Account Report"** from registers and the Chart of Accounts (button, window title "Account Report: {account}"); **"Transaction Report"** from Customers & Jobs and Vendors. The shared command (Ctrl+Shift+Q, id `list.quick_report`) reads "Account or Transaction Report" in the palette and KEYBOARD.md; command ids and Rust identifiers keep their names. Supersedes the label in Q-P11h, Q-P11i, Q-P12w. Deviation from the Register mockup (`docs/design/mockups/Register.html`, "QuickReport" button). | P11, P12, all later registers and lists |
| Q-255 | "Customer:Job" (QBD colon notation, review L-1). | **"Customer / Job"** in all owner-facing text: column headers, labels, help, AccessKit labels, scenarios. A billable line error reads "needs a customer or job". QBD import (P31) still reads the QBD `Customer:Job` field. Supersedes the wording in Q-238 and Q-P13h. | P13 lines grid, P14+, P20 invoices, DESIGN.md |
| Q-256 | "Memorized transactions" and "Batch Enter Transactions" (QBD feature names, review L-3/L-4). | Memorized transactions are called **Templates**: "Save as Template" on forms, a "Templates" list (P15). Batch entry phase P15A is renamed **"P15A — Bulk Entry"**. Decision IDs (Q-249, Q-251, Q-252) keep their numbers; their wording is updated. QBD import text may still say "QuickBooks memorized transactions" (Q-143) because it describes the source file. Memorized reports (P28) are not covered by this decision. | P13, P15, P15A, P40, P48 |
| Q-257 | App icon (Q-122; review L-10 flagged option 2's ring shape). | **Option 1** (teal square with ledger rows), already the shipped app and window icon; options 2 and 3 stay as unused drafts in `assets/icon/`. | P06 icon, packaging (P36) |
| Q-258 | "Customer Center"/"Vendor Center" and "QB Payroll" (review L-2, L-7). | Owner-facing text says **"Customers & Jobs"** and **"Vendors"**, never "Customer center"/"Vendor center" (Q-238 amended). The payroll provider picker spells out **"QuickBooks Payroll (export file)"**, never "QB Payroll". Trademark notice (About, Open Company footer, NOTICE.md, README) reads "Intuit and QuickBooks are trademarks of Intuit Inc. … Other product names are trademarks of their owners." (review M-2, M-3). | P11 lists, P30 payroll import, About, README |
| Q-259 | Sales Receipt lines before items exist (P20). | **Sales Receipt moves to P20** and is built with items there; P14 builds the rest (deposits, transfers, daily sales summary, card charges, owner, personal-funds and loan helpers). | P14, P20 |
| Q-260 | Sales tax on the daily sales summary before P23. | **Manual "Sales tax collected" line**: the owner types the tax to a sales-tax liability account; P23 replaces it with calculated tax. | P14, P20, P23 |
| Q-261 | Look of P14 screens (no mockups). | **Follow the approved Write Check pattern** (tokens, header + lines grid + side panel); screenshots in the PR for owner review. | P14 |
| Q-262 | Open registers don't refresh after a save in another window (P13 follow-up). | **Fix first in P14**: every save tells open registers and lists to reload. | P14 |
| Q-253 | Early test builds before P35 packaging? | **Yes**: a GitHub workflow builds unsigned portable preview archives (Windows, macOS, Linux) automatically after each phase merges to main, and on demand, as workflow artifacts (kept 30 days), not public releases; P35 still builds the installers. | Preview build, P35 |

## Open

Phase-specific decisions (`Q-Pxx…`) of completed phases (P00–P09) are in [docs/history/phase-decisions.md](docs/history/phase-decisions.md); look up an ID there with a search, do not read the file whole. Rows for the phase in progress stay here until it closes.

| ID | Question | Raised | Default assumption | Status |
| --- | --- | --- | --- | --- |
| Q-35 | Engine redistribution grant. | P01 | **Closed 2026-10-15:** the owner states the engine is fully licensed for ClientLedger's use (Internal Use license, no grant); engine licensing is settled and is not to be raised again. `docs/legal/LICENSE-CLIENTLEDGER.draft.md` is kept for reference only. | Closed |
| Q-P10a | Grid scrolling at 200k+ rows | P10 | Row-anchored integer viewport, not `ScrollArea::show_rows` (f32 offsets drift: 0.72 pt at 200k, 35 pt at 10M; plan §12 spike). | Default in force |
| Q-P10b | Amount entry and calculator | P10 | `cl_model::amount_entry`: plain parse first, then `+ - * / ( )` on exact decimals; calculator results rounded to the cent, half away from zero (Q-P10x, replacing the earlier refusal); Debug output has no digits. Reconstructed from the shipped design: the Revision 1 text was replaced. | Default in force |
| Q-P10c | Autofill ranking | P10 | `cl_model::lookup` tiers: exact number or name, number prefix (ties by number), name prefix, `Parent:Child` segment prefix, word prefix, substring; inactive entries only on an exact match; Tab takes the highlighted entry. Reconstructed (see Q-P10b). | Default in force |
| Q-P10d | Grid keys vs the keymap | P10 | Grid screens use `Scope::Form`. Esc, Ctrl+Insert/Delete, Ctrl+L, Ctrl+Z/Y, Page Up/Down and Alt+N/S reach the grid through `GridForm::command`; key repeats per plan §2.4.4; pop-outs run their own shortcuts; Enter in an open list accepts and does not record. | Default in force |
| Q-P10e | Commits and undo | P10 | Commit on cell exit as one `edit_step` (never merged); the editor resyncs when the form revision changes; `commit_pending` runs before every save, leave and record move (`GridForm`), and on focus loss before the list closes (Tab rule). | Default in force |
| Q-P10f | Date entry | P10 | DESIGN.md date keys; `M/D/YY` and `M/D/YYYY` (`YYYY/M/D` only in Y-M-D order); a two-digit year shows four digits as soon as it parses; no interception during IME composition. | Default in force |
| Q-P10g | Tags in a cell | P10 | Chips from the tag list only; one tag per group (a second replaces the first with "Only one tag per group"); duplicates refused. Reconstructed (see Q-P10b). | Default in force |
| Q-P10h | Demo screen | P10 | `dev.grid_demo` lives in `DEV_COMMANDS`, built only with `--demo`, never saved in layouts; its `state` has no amounts, names or memo text (exactness: displayed values and in-process tests). | Default in force |
| Q-P10i | Frame-time gate | P10 | Primary gate: deterministic work counts (`virtualization.rs`). `cargo xtask grid-bench` (bench in `cl-ui/benches/grid_scroll.rs`, real shell, 200k rows, 600 frames) fails at p95 > 8 ms (AccessKit off) / 12 ms (on), one retry; profile `grid-bench` keeps debug assertions because cl-ui's dev-dependencies enable cl-api `test-support`. | Default in force |
| Q-P10j | Paste | P10 | One value per paste; a second field is refused ("Paste one amount at a time"); one trailing line break trimmed; NBSP, U+202F and U+2212 normalized. | Default in force |
| Q-P10k | Column widths | P10 | `grid-widths.json` through `UiStore`, each entry checked on its own, size-capped, never written during a controlled run; `UserPrefs` untouched. | Default in force |
| Q-P10l | Locked rows | P10 | Rows on or before the closing date are read-only; edit, insert or delete emits `LockedRowEditAttempt` for the screen's closing-date flow. Reconstructed (see Q-P10b). | Default in force |
| Q-P10m | Money in the grid | P10 | `CellValue::Money` shown with `Money::format` only; the grid never rounds or sums; totals come from the screen or the engine. Reconstructed (see Q-P10b). | Default in force |
| Q-P10n | Selection | P10 | By RowKey (`Selection{keys, anchor}`); Shift ranges stop at Loading rows; vanished keys are dropped. | Default in force |
| Q-P10o | Accessibility | P10 | Grid, row and cell nodes with row/column indexes; row labels end in "row n of N"; relations (`controls`, `active_descendant`) only to nodes present in that frame. Reconstructed (see Q-P10b). | Default in force |
| Q-P10p | Input ordering | P10 | `cl_grid::sequencer` splits every frame at boundary events and holds navigation keys for one frame after a focus-lock widget gains focus; installed after the automation bridge. | Default in force |
| Q-P10q | Focus model | P10 | One egui focus id per grid; cells non-focusable with explicit AccessKit nodes; the grid answers AccessKit Focus/Click/Scroll; `Grid::focus` retries for three frames when the grid is first shown. | Default in force |
| Q-P10r | Grid presets | P10 | `line_entry`, `browse`, `fixed_edit` (flags: editable, entry_line, insert_delete, selection, async_record). | Default in force |
| Q-P10s | RowKeys | P10 | Screen-allocated, monotonic, stored in line values, never engine line ids; remapped by line order after `save_ok` (P13 verifies the engine keeps line order). | Default in force |
| Q-P10t | Several tags per line vs one engine `tagId` | P10 | P11 opens an UPSTREAM row; until it lands, P13 refuses to save a line with more than one tag, with a plain message. | Answered by owner: Q-233 (one tag per line, permanent) |
| Q-P10u | Modals | P10 | Grids give up focus and ignore input while a shell modal is open; the close prompt focuses Save. | Default in force |
| Q-P10v | Enter on an empty entry line | P10 | `EnterOnEmptyEntry`; the screen treats it as Save & New (Q-190); saved records never include the blank entry line. | Default in force |
| Q-P10w | AI suggestion acceptance | P10 | Only Tab on an empty, unchanged suggested cell accepts; never Enter, a click, other keys or a save. | Default in force |
| Q-P10x | Calculator rounding | P10 | Calculator results round to the cent, half away from zero (owner 2026-10-18); replaces the refusal design. Exact rational math, one rounding of the final result to the field's unit; plainly typed amounts are not rounded (`12.345` is still refused). | Owner decision |
| Q-P11a | Account merge | P11 | Hidden in the Chart of Accounts until U-31: the engine's merge only deactivates the source and moves no journal lines, so it would hide history. `state.merge_available` is `false`. | Default in force |
| Q-P11b | Parent and sub-account balances | P11 | Each row shows the engine's own balance of that account only; the UI never adds sub-accounts into a parent (NN7). A parent's total is PENDING on U-32. Until U-32 validates `parentId`, `cl_model::lists::accounts::check` refuses a missing parent, a parent of another type, a loop, or an inactive parent; depth is unlimited (Q-208). | Default in force |
| Q-P11c | Sensitive accounts (Q-157) | P11 | The mark is kept on this computer in `CompanyPrefs.sensitive_accounts` (travels in backups) and shown in the Status column and form; nothing is hidden yet. Enforcement waits for U-33 and P29. | Default in force |
| Q-P11d | Masked bank or card number (Q-154) | P11 | For bank and credit-card accounts the form shows the field switched off with "Keeping the full account number, shown masked, is not available yet." until U-34 (encrypted storage, Admin reveal with audit entry). | Default in force |
| Q-P11e | Used account numbers (Q-209) | P11 | Checked against the loaded chart before sending; the engine's 409 `ACCOUNT_NUMBER_EXISTS` (another window got there first) shows the same plain message on the number field. | Default in force |
| Q-P11f | Making an account inactive | P11 | The screen refuses system accounts and accounts whose engine-reported balance is not zero before calling the engine; any engine refusal shows a plain message. Reactivating sends the unchanged fields with `isActive: true`. | Default in force |
| Q-P11g | Add/edit layout | P11 | A side panel inside the Chart of Accounts window (no separate window): Save & Close closes the panel, Save & New starts another account of the same type, Esc reverts and then closes the panel. While the panel is open the chart shows number, name, type and balance only and leaves the focus in the form. | Default in force |
| Q-P11h | List keys | P11 | Ctrl+A opens the chart; in list windows Ctrl+N new, Ctrl+E edit, Ctrl+Shift+I make inactive or active again, Ctrl+Shift+H include inactive, Ctrl+Shift+Q Account Report (Transaction Report in Customers & Jobs and Vendors, Q-254), Ctrl+F find; letters typed within one second build one find prefix. The other P11 lists reuse the `list.*` commands. | Default in force |
| Q-P11i | Account Report (was "QuickReport", Q-254) | P11 | Shown under the chart from `/reports/account-report`, this year to date (default) or all dates; it prints the engine's account balance and never sums the lines (`/accounts/:id/ledger` is empty until U-31). | Default in force |
| Q-P11j | Jobs in the Client center (Q-90) | P11 | Until U-35 the engine has no jobs (a contact's `parentId` is dropped): the Client center lists clients only, its Jobs tab says "Jobs aren't available yet", `state.jobs_available` is `false`, and `contacts::jobs_u35` is PENDING by capability. | Answered by owner: Q-235 (separate jobs list) |
| Q-P11k | Several ship-to addresses and contact people (Q-128, Q-129) | P11 | Until U-36 a contact keeps one bill-to and one ship-to address and the single person/email fields; the pane and form say several ship-tos and contact people with roles arrive later. `contacts::addresses_u36` is PENDING by capability. | Default in force |
| Q-P11l | Dated notes, to-dos and attachments on contacts (Q-130) | P11 | Until U-37 the Notes tab shows the engine's single notes field and says dated notes and to-dos aren't available yet (`contacts::notes_u37` PENDING). Attaching files to a contact waits for the attachments work (the engine's generic attachments were not checked for the contact type); the tab says so. | Default in force |
| Q-P11m | Open balance per contact | P11 | Until U-38 the centers show each contact's open balance from the engine's client balance summary and vendor aging reports (labelled as such), matched by contact; the UI never adds transactions up (NN7). An inactive client shows "inactive" rather than a figure. `contacts::open_balance_u38` is PENDING by capability. | Default in force |
| Q-P11n | Recording a paper W-9 (Q-53) | P11 | Until U-44 the 1099 profile update drops `w9OnFile`: the vendor pane shows the engine's W-9 value read-only and the form says recording a paper W-9 isn't available yet (`contacts::w9_on_file_u44` PENDING). The tax ID is only ever shown masked (last four digits). | Default in force |
| Q-P11o | Contact names | P11 | A name must be unique among active clients and vendors (one list, Q-77), ignoring case and outer spaces; an inactive contact does not hold its name, because a merge leaves the merged-away duplicate inactive under its old name. Merge asks for a confirmation that it can't be undone and uses the engine's merge, which moves the transactions. | Default in force |
| Q-P11p | Lists without engine support (steps 4–6) | P11 | Split per item: item kinds U-39 → Q-P11q; price levels U-40 → Q-P11r; tag merge and single-select U-41 → Q-P11t; required tag groups U-42 → Q-P11u; terms and payment methods U-43 → Q-P11y; several tags per line U-45 → Q-P11w; list imports U-46 → Q-P11x. Each row's PENDING test runs once its capability appears. | Split into per-item rows |
| Q-P11q | Item kinds, purchase side, unit of measure (Q-58, Q-96, Q-99) | P11 | Until U-39 the engine keeps service items only: the Item List offers the Service type, refuses any other kind before sending (`ItemProblem::KindNotAvailable`), and the panel says non-inventory parts, other charges, discounts, subtotals, a purchase price with its expense account and a unit label aren't available yet. `items::item_types_u39` is PENDING by capability (`item-types-v1`). | Default in force |
| Q-P11r | Price levels (Q-58) | P11 | Until U-40 there are no price levels: the Item List says so under the list (no `price_levels.rs` screen yet); `items::price_levels_u40` is PENDING by capability (`price-levels-v1`). | Default in force |
| Q-P11s | Item names and prices | P11 | Names are unique over every item, inactive ones included (the engine's unique index), checked ignoring case before sending; the engine's 409 shows the same words on the name field. Prices are typed as exact money up to four decimals, never below zero, calculations allowed, and shown back from the engine's decimal string (`12.50`, `0.045`). Scenarios name what they create after the theme (`{theme}` in typed text) because an inactive item keeps its name. | Default in force |
| Q-P11t | Tag merge (U-41) | P11 | The engine's merge moves header tags only and then fails on a tag used on a line, a budget, an item or contact default or a bank rule. The Tags window reads `/tags/{id}/usage` for the selected tag and shows Merge only when every use is a header tag (`TagUsage::merge_safe`); otherwise it says merging isn't available yet and suggests making the tag inactive. Delete is offered for unused tags only (the engine refuses others with 409 `TAG_IN_USE`). | Default in force |
| Q-P11u | Required tag groups (Q-180, U-42) | P11 | The group form keeps "required on lines for these account types" in `CompanyPrefs.required_tag_groups` (engine account-type names, travels in backups); `cl_model::lists::tags::check_line_tags` reports a missing required group for P13 onward. Deleting a group drops its rule. | Default in force |
| Q-P11v | Tag application rules for forms (Q-178, Q-179, Q-181) | P11 | `cl_model::lists::tags`: header tags are every line's default and a line's own tag wins in a one-per-group group (`effective_line_tags`); one tag per one-per-group group per line, ungrouped unlimited (`check_line_tags`); a rule's or memorized transaction's tags replace the same one-per-group group and add to the rest (`apply_rule_tags`). The engine's single-select check only clears earlier tags, so the forms enforce Q-179 themselves. | Default in force |
| Q-P11w | Several tags on one line (Q-P10t) | P11 | Opened as U-45. Until it lands `engine_line_tag` refuses a line with more than one tag ("A line can have only one tag for now"), and P13 uses it before saving. | Answered by owner: Q-233 (U-45 withdrawn) |
| Q-P11x | List CSV import and export (P11 step 6) | P11 | Lists > Import or Export a List. Export writes the whole list to `<list>.csv` in a folder the owner picks (the backup folder by default; a number is added rather than overwrite; UTF-8 with a byte-order mark): accounts, contacts and items from the engine's export routes, tags written by ClientLedger. Import reads a CSV, guesses the column mapping from the header, lets the owner change it, lists rows left out with their line and reason, and only adds records (existing account numbers and item names are skipped, never updated), so it takes no backup first (non-negotiable 9 covers restores, company imports and migrations). Accounts and contacts go through the engine's import routes; items are created one at a time (U-46). Contacts take name, company, email and phone only until U-46. Tags are export-only. | Default in force |
| Q-P11y | Terms and payment methods (U-43) | P11 | Until U-43 both are the engine's fixed lists (`cl_model::lists::terms`), shown read-only in Lists > Terms and Lists > Payment Methods with a note that adding your own (and discount terms) isn't available yet; `list_csv::terms_lists_u43` is PENDING by capability (`terms-lists-v1`). | Default in force |
| Q-P11z | Classes to tags (P11 step 7, shared with P31) | P11 | `cl_model::lists::class_map`: a class becomes a tag; `Parent:Child` becomes the tag `Child` in the group `Parent` (deeper levels keep the rest, `Region:North:Lake` is `North:Lake` in `Region`); parts are trimmed and empty parts dropped; a parent-only class is the group alone, plus an ungrouped tag of its name when transactions use it directly; class groups take one tag per line; group names match ignoring case (first spelling wins); a tag name clash, with existing tags or another class, gets the group in brackets and then a number; names over 100 characters are reported, not cut; inactive classes become inactive tags. Fixture `fixtures/lists/classes.csv`. | Default in force |
| Q-P11aa | Merges that touch closed periods (contacts, tags) | P11 | A contact or tag merge rewrites transactions; when the source has transactions dated on or before the closing date, the merge requires the closing-date password flow and is recorded in the audit trail (non-negotiable 8); the engine enforces it (U-47). Until U-47 lands the engine merges without the check, so both merge confirmations warn that closed-period transactions move too. | Confirmed by owner: Q-231 |
| Q-P12a | Capability names for the register engine work | P12 | One `register-exact-v1` covers R1–R4 and R6; R5 is `register-cleared-v1`; R7 is `txn-delete-v1` (the plan's `transaction-delete-v1` was shortened by the engine agent); constants in `wire.rs`. All on engine PR #153, PENDING on the pinned engine. | Default in force |
| Q-P12b | Register before R2 (amounts are JSON numbers) | P12 | Strict decimal-text parse (never f64), `exact: false`, display only; edits seed from `GET /transactions/:id`; exact tests PENDING. | Default in force |
| Q-P12c | A filter on before R1 | P12 | Balance cells show "—" with "Balances show when no filter is on"; the footer's ending balance comes from an unfiltered companion query. | Default in force |
| Q-P12d | Uncleared and Tag filters before R3 | P12 | Switched off, with the tooltip "This filter isn't available yet." | Default in force |
| Q-P12e | Footer content (the mockup shows the last-reconciled amount) | P12 | "Cleared balance" (R4) + "Ending balance"; "Last reconciled <date>" without an amount; the DESIGN row wins (Q-226). | Default in force |
| Q-P12f | What "Splits opens the full form" means before P13 | P12 | A Splits panel in the register (lines grid), reused by P13–P15. | Default in force |
| Q-P12g | Entry kind and column labels | P12 | Kind inferred from the side and the other account (bank: Payment→Check, Deposit→Deposit, other bank/card account→Transfer; card: Charge→Card charge, Payment→Transfer or card credit if allowed; other balance-sheet: two-line journal entry). Labels: bank Payment/Deposit; card Charge/Payment; other asset Decrease/Increase; liability and equity Increase/Decrease. | Default in force |
| Q-P12h | Balance sign for credit-normal registers | P12 | Every account shows its normal balance positive (a card's amount owed reads positive). **Corrected in the P12 fix pass:** the engine uses two conventions. Register figures (running balances, balance forward, the summary's ending and cleared balances) come in the account's natural sign (credit − debit on card, A/P, other liability, equity) and are shown **as sent**. Only the stored `accounts.balance` (summary `currentBalance`, chart balance) is debit-positive for every type, so only it is flipped (`Money::checked_neg`, display only) for credit-normal accounts (`DisplaySign::for_source`, `for_stored_balance`). The engine's `payment` / `deposit` names follow the normal side too: on a credit-normal register `deposit` is the credit (a card charge) and lands in the first column (Charge / Increase), `payment` the debit (`AccountClass::engine_payment_side`). Verified by `register::card_register_columns_and_sign` on the pinned engine. | Answered by owner: Q-241 |
| Q-P12i | Delete before R7 | P12 | Ctrl+D / Delete explains and offers Void without `txn-delete-v1`; with it, Delete with confirmation (Q-240), only for the types of Q-245. | Superseded by Q-240 |
| Q-P12j | Closing password reuse | P12 | Asked for every save, never cached; a wrong password retries in the same dialog; lockout is the engine's. | Default in force |
| Q-P12k | Cleared column | P12 | Read-only ✓/R; legend "✓ cleared · R reconciled"; toggling waits for R5 (`register-cleared-v1`, PENDING). | Default in force |
| Q-P12l | Range presets and remembered range | P12 | Presets per plan WP4; All dates = 1900-01-01…2999-12-31; relative presets stored relative per tenant and account in `UiStore`. | Default in force |
| Q-P12m | Kinds the register can't edit yet | P12 | Read-only lines view: "You'll be able to change this kind of transaction in its own form." | Default in force |
| Q-P12n | Refresh after changes elsewhere | P12 | Ledger stamp read after own writes, on focus, and every 5 s while visible. | Default in force |
| Q-P12o | Go To | P12 | Number, payee, memo or exact amount within the shown dates; date-sort offset lookup, other sorts scan at most 50 pages. See Q-P12v for the "Look in" choice. | Default in force |
| Q-P12p | A/R and A/P registers (Q-206) | P12 | The entry row posts a two-line journal entry only with a name chosen; otherwise view-only. See Q-P12t. | Default in force |
| Q-P12q | Next check number | P12 | Highest numeric check number in the account + 1, from an engine query; editable. | Answered by owner: Q-242 |
| Q-P12r | Mockup deviations | P12 | No Print button (DESIGN row lacks it; printing is a later phase); quick filters per Q-189 instead of "This month · Checks · Deposits"; masked "••••2208" omitted until U-34 (Q-P11d); footer per Q-P12e; legend "✓ cleared" (not "by bank file" until P26); rail says "Customers" (Q-238). | Default in force |
| Q-P12s | Where an edit of a recorded row happens | P12 | In the pinned entry row below the grid (loaded from `GET /transactions/:id`), not inline in the grid row: inline editing needs an editable paged grid, which `cl-grid` does not have yet. Revisit when the grid gains editable windowed rows. | Deviation from plan WP6 |
| Q-P12t | Journal-entry lines with a customer or vendor | P12 | Refused before sending with a plain message: the client's `JeLine` has no contact field yet (engine side: U-22). A/R and A/P registers stay view-only for those entries. | Deviation from plan WP6 / Q-P12p |
| Q-P12u | Customer on Splits lines | P12 | The Customer column is offered on deposit splits only; check and card-charge splits have no customer until the P13 forms. | Deviation from plan WP6 |
| Q-P12v | Go To "Look in" | P12 | Go To has a "Look in" picker (Any field, Number, Payee, Memo, Amount) so an amount or a number can be searched on its own. | Addition to plan WP7 |
| Q-P12w | Account Report range | P12 | Account Report (Ctrl+Shift+Q, Q-254) uses the register's shown dates, not the P11 year-to-date default. | Deviation from plan WP7 / Q-P11i |
| Q-P13a | Items tab (mockup shows "Items $0.00") | P13 | Hidden until P20; deviation from the mockup. | Answered by owner: Q-247 |
| Q-P13b | Save as Template button (was Memorize, Q-256) | P13 | Not shown until P15; deviation from the mockup. | Answered by owner: Q-249 |
| Q-P13c | Print button and Ctrl+P before P19 | P13 | The Print button is disabled (with a tooltip) until P19; Print later is the way to queue a check. | Default in force |
| Q-P13d | Mockup shows Print later ticked and No. 4418 | P13 | They are exclusive: ticked shows "To print" instead of a number. | Answered by owner: Q-246 |
| Q-P13e | "Go to Pay Bills" link | P13 | No link until P18; the open-bill warning text only, saving is never interrupted. | Answered by owner: Q-248 |
| Q-P13f | Mockup line 1 has two tags (Install, Crew A) | P13 | One tag per line; "Crew A" can go in the header Tags field. | Answered by owner: Q-233 |
| Q-P13g | Header Tags field, not in the mockup | P13 | An extra Tags field in the check header; it defaults the lines that have no tag of their own. | Answered by owner: Q-178 |
| Q-P13h | "Customer / Job" wording; the demo company has no jobs | P13 | Column says "Customer / Job" (Q-255; was "Customer:Job" under Q-238); the sample data has no jobs until U-35 lands. | Default in force |
| Q-P13i | Editing a printed check where a printed field changes | P13 | Asks "Reprint / Keep number N as handwritten / Cancel" (engine answers 409 `CHECK_PRINTED`, client `printedCheckChange`). | Default in force |
| Q-P13j | Void or delete of a check with an already-billed billable line | P13 | Refused with the engine's 409 `LINE_BILLED` (engine PR #156 default); no effect until invoicing bills lines. | Default in force |
| Q-P13k | Attachments: no native file picker yet | P13 | Type a path or drag and drop; "Save a copy" writes to Downloads without overwriting. Follow-up: a native file dialog with a permissive crate. | Default in force |
| Q-P13l | AI suggestion display | P13 | A caption strip below the grid instead of under the line; the suggested account shows as grey text; account suggestions on checks follow the "Bank Feed suggestions" preference (the help page says so). | Default in force |
| Q-P13m | Next check number | P13 | Sent explicitly as max(engine counter, highest check number including bill payments + 1), unless Print later is ticked; editable (Q-242). | Default in force |
| Q-P13n | Status bar shortcut text | P13 | Shows "Ctrl Insert" (keymap text), not the mockup's "Ctrl Ins". | Default in force |
| Q-P13o | Prefill source | P13 | Copies from the payee's newest check, expense or bill (e.g., the bill for Ozark Feed & Seed); amounts are never copied. | Default in force |
| Q-P13p | Find and History | P13 | Find opens the account register (Ctrl+F there for Go To); History shows date and action, not the user. | Default in force |
| Q-P13w | Scenario repeatability | P13 | The `write_check` scenario voids or deletes what it posts so it can run again. | Default in force |
| Q-P13x | Checks with line customers | P13 | Read-only when the engine lacks `line-billable-v1` (live since pin 454fd23, so editable now). | Default in force |
| Q-P14a | "Undeposited Funds" vs the engine's "Payments Clearing" | P14 | Owners see "Undeposited Funds" wherever account names show (display mapping by the payments_clearing system tag); heading "Payments to deposit"; tooltip "Payments you've received but haven't taken to the bank yet." | Default in force |
| Q-P14b | Grouping before `deposit-grouping-v1` | P14 | Read-only pending list with a note; plain lines only (capability is live since pin f78583e). | Default in force |
| Q-P14c | Fees and cash back before `deposit-cashback-v1` | P14 | Negative amounts refused; cash back hidden; tooltip (capability is live since pin f78583e). | Default in force |
| Q-P14d | Deposit summary (Q-203) | P14 | `DepositSummary` model and an on-screen Summary tab; the PDF is P19, Print disabled until then. | Default in force |
| Q-P14e | Suggested groupings | P14 | From bank-file deposits waiting for this account; a click ticks them; feed matching is P26. | Default in force |
| Q-P14f | Guided vendor refund | P14 | (1) "Something I already paid for" gives an expense line; (2) vendor credit or overpayment uses `vendor-refund-v1`. The vendor statement shows refunds. | Default in force |
| Q-P14g | Editing a grouped deposit | P14 | Read-only except Void. | Default in force |
| Q-P14h | Transfer accounts | P14 | Bank, card, other current asset/liability, loans; not A/R, A/P, system accounts or equity. | Default in force |
| Q-P14i | Card credit | P14 | Deposit into the card account. | Default in force |
| Q-P14j | Customer / Job and Billable on card charges | P14 | Yes, as on Write Check (Q-255 wording). | Default in force |
| Q-P14k | Rule/AI suggestions on P14 forms | P14 | None in P14. | Default in force |
| Q-P14l | Pay yourself by entity type | P14 | Sole prop/SMLLC: owner draw/contribution; partnership: partner draws/contributions (per partner); S/C corp: shareholder distributions/paid-in capital. Neutral sentence and "Ask your accountant..."; no tax advice. Equity role accounts (30160/30170) come from the engine; starter charts no longer add owner equity rows. | Default in force |
| Q-P14m | Without `equity-roles-v1` | P14 | The owner picks the equity account once; remembered per company. | Default in force |
| Q-P14n | Personal-funds default (Q-85) | P14 | Sole prop/SMLLC/partnership: "Money I put into the business"; S/C corp: "The business will pay me back". | Default in force |
| Q-P14o | Personal-funds posting | P14 | An expense paid from the equity or liability account. "Who paid" is stored as a memo prefix "Paid by <name> — <memo>" because the engine has no contact on the pay-from line. | Default in force |
| Q-P14p | Loan method and prefill | P14 | Check or Online/automatic; accounts prefilled, never amounts (Q-191); optional extra lines. | Default in force |
| Q-P14q | Loan balance check | P14 | Optional lender balance vs the engine balance after save; no amortization. | Default in force |
| Q-P14r | Daily sales "saved as a template" | P14 | The engine daily-sales template is the layout, never amounts; not in the P15 Templates list. | Default in force |
| Q-P14s | Debit/credit on daily lines | P14 | Fixed by section; never shown to owners. | Default in force |
| Q-P14t | Negative daily-sales amounts | P14 | Refused. | Default in force |
| Q-P14u | Daily-sales drafts | P14 | Save posts in one action; a failed post leaves a draft the form reopens; no "Save draft" button. The demo stores no tags on daily-sales days. | Default in force |
| Q-P14v | Closed-period daily sales | P14 | Refused with a plain message without `daily-sales-lock-v1`; with it the closing-password flow applies (live since pin f78583e). | Default in force |
| Q-P14w | Over/short before saving | P14 | Local exact figure; the engine preview confirms before posting; a mismatch posts nothing. | Default in force |
| Q-P14x | Menus and shortcuts | P14 | Daily Sales Summary (Customers), Personal Funds (Vendors), Owner Money and Loan Payment (Banking); no new shortcuts. | Default in force |
| Q-P14y | Look | P14 | No mockups: Write Check pattern (Q-261); PR screenshots; no `--compare`. | Default in force |
| Q-P14z | Demo and sample additions | P14 | 10150/20900/60900 in the demo chart; 1,250.00 + 842.50 payments; feed deposit 2,092.50; "Plant sale counter"; lender statement. Dropdown scenario steps choose Blue Eye Feed Co (refund) and 1020 (cash back) because the control channel can't click options scrolled out of view. | Default in force |
| Q-P14aa | Register routing for P14 types | P14 | Ctrl+E on a P14-type row opens its form. Enter or double-click edits simple rows in the register entry row (P12 behavior); grouped deposits, daily-sales days and non-editable rows open the form. | Default in force |
| Q-P14ab | Sales Receipt | P14 | Built in P20, not P14 (Q-259). | Default in force |
| Q-P14ac | Sales tax on daily sales | P14 | A manual sales tax line (Q-260); computed tax arrives with P23. | Default in force |
| Q-P14ad | Engine main CI | P14 | The npm-audit job failing on engine main is pre-existing and unrelated to PR #159. | Default in force |
