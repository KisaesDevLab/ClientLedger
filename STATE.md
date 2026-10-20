# STATE.md — ClientLedger

**Current phase:** P14 — Deposits, transfers, sales receipts, card charges, owner and loan helpers (Next; Intuit compliance check runs first — see Owner-requested checkpoints)
**Status:** P13 built on `phase/P13-write-check`; PR to open
**Engine commit:** `f78583edabe88929f92c10521fefdd1f287c3d81` (`main`, pinned 2026-10-20 after engine PR #159: deposit-grouping-v1, deposit-cashback-v1, vendor-refund-v1, equity-roles-v1, feed-deposit-groupings-v1, daily-sales-lock-v1, migration 0223; bump per Q-02)
**Last updated:** 2026-10-20
**Tiers:** H = opus implementer + architect + opus review · M = sonnet implementer + opus review · L = sonnet (see docs/AGENTS.md)

## Phase board — beta track

| Phase | Title | Tier | Status |
| --- | --- | --- | --- |
| P00 | Repository bootstrap | L | Done 2026-10-15 |
| P01 | Engine submodule and single-executable build | L | Done 2026-10-15 (deferred criterion closed in P02: smoke requires 200 and the pinned commit, Q-P01a) |
| P02 | Engine desktop profile (upstream) | H | In progress |
| P03 | Bundled PostgreSQL manager | L | Done 2026-10-15 |
| P04 | Supervisor | L | Built, CI green, security-reviewed (PR #5, stacked on #4) |
| P05 | Engine API client | H | Done 2026-10-17 (AC4 real-engine test blocked by U-17) |
| P06 | Application shell | L | Done 2026-10-17 (AC3/AC4 closed in P07: scenario and Home --compare, deviations in Q-P07h) |
| P07 | Automation and control channel | L | Done 2026-10-17 |
| P08 | Company lifecycle | M | Done 2026-10-17 (AC1-AC3 and the closing password live since pin 5c8e9665; delete PENDING by `tenant-delete-v1`; recovery key by `recovery-key-v1`) |
| P09 | Preferences and AI provider keys | M | Done 2026-10-18 (session secrets, desktop AI, SMTP session password and token cap live since pin 5c8e9665) |
| P10 | Ledger grid widget | H | Done 2026-10-18 (reviews pending; CI to confirm grid-bench and both ui-check jobs) |
| P11 | Lists, centers, and tags | M | Done 2026-10-18 (engine gaps PENDING by U-31 to U-46; reviews pending) |
| P12 | Bank and account register | H | Done 2026-10-19 (AC1 register part, password path and delete live since pin 5c8e9665; R5 PENDING by `register-cleared-v1`; reviews pending) |
| P13 | Write Check | M | Done 2026-10-20 |
| P14 | Deposits, transfers, sales receipts, card charges, owner and loan helpers | M | Next |
| P15 | Journal entries, templates and recurring | H | |
| P15A | Bulk Entry | M | |
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

Format: `Pxx — blocked by U-xx/Q-xx — what is pending — workaround in place`.

- P39 (release) — owner decision needed on Q-P03e — the bundled PostgreSQL archives ship LGPL libraries (gettext/libiconv on Windows) and OpenSSL, ICU, libxml2 and others on Linux/macOS; the owner must confirm the licensing position (license texts shipped, any written source offer) before beta release — workaround in place: libraries kept unmodified and listed in ATTRIBUTION.md.

- P05 AC4 — blocked by U-17 — real-engine 409 contract test is PENDING(record-version-v1) — stub contract covers the client (`If-Match` sent, 409 `STALE` mapped to `Conflict { Stale }`, fingerprint check sends nothing).

- P08 Delete Company (Q-185) — blocked by U-05 E5b (`tenant-delete-v1`) — `lifecycle::safe_delete_e5b` is PENDING — in place: the window explains it and keeps the button off; cl-company refuses without a before-delete backup path and SHA-256 (nothing sent) and removes the local record only after the engine's receipt.
- P08 Recovery key (Q-210) — blocked by U-23 (`recovery-key-v1`) — `lifecycle::recovery_key_u23` is PENDING — in place: the screen (show once, copy, save, confirm) runs on the key `/desktop/initialize` returns or a rotated one; without the capability it says so.
- P08 Full Database Backup (File > Utilities) — needs a supervisor-side `pg_dump` API while the cluster runs — placeholder window; P34 (Q-P08l).
- P11 Chart of Accounts merge — blocked by U-31 (engine merge moves no journal lines; `/accounts/:id/ledger` empty) — merge not built — in place: merge hidden (`merge_available: false`), QuickReport uses `/reports/account-report` (Q-P11a, Q-P11i).
- P11 parent-plus-sub-account balance — blocked by U-32 — PENDING — in place: each account's own engine balance only, with a note under the chart; `parentId` checked in `cl_model::lists::accounts::check` (Q-P11b).
- P11 sensitive accounts enforcement — blocked by U-33 (and P29) — PENDING — in place: the mark is stored locally in `CompanyPrefs.sensitive_accounts` and shown (Q-P11c).
- P11 masked bank or card number with logged Admin reveal (Q-154) — blocked by U-34 — PENDING — in place: the field is shown switched off with "not available yet" on bank and credit-card accounts (Q-P11d).
- P11 jobs in the Client center (Q-90) — blocked by U-35 — `contacts::jobs_u35` is PENDING — in place: clients only; the Jobs tab says jobs aren't available yet (Q-P11j).
- P11 several ship-to addresses and contact people with roles (Q-128, Q-129) — blocked by U-36 — `contacts::addresses_u36` is PENDING — in place: one bill-to and one ship-to, single person/email fields, with a note in the pane (Q-P11k).
- P11 dated notes and to-dos (Q-130) — blocked by U-37 — `contacts::notes_u37` is PENDING — in place: the single notes field in the Notes tab, with a note that dated notes and to-dos arrive later (Q-P11l).
- P11 per-contact open balance — blocked by U-38 — `contacts::open_balance_u38` is PENDING — in place: figures from the client balance summary and vendor aging reports, labelled with their source, never summed in the UI (Q-P11m).
- P11 recording a paper W-9 — blocked by U-44 — `contacts::w9_on_file_u44` is PENDING — in place: W-9 on file shown read-only, the form says recording it isn't available yet (Q-P11n).
- P11 item kinds other than service, the purchase side of two-sided items and the unit label (Q-58, Q-96, Q-99) — blocked by U-39 — `items::item_types_u39` is PENDING — in place: service items only, the panel says the rest isn't available yet (Q-P11q).
- P11 price levels (Q-58) — blocked by U-40 — `items::price_levels_u40` is PENDING — in place: a note under the Item List (Q-P11r).
- P11 tag merge for tags used on lines or as defaults, one tag per group in the engine, group reorder — blocked by U-41 — `tags::line_tag_merge_u41`, `tags::single_select_u41`, `tags::group_reorder_u41` are PENDING — in place: merge offered only for header-only tags, the forms enforce one per group, groups keep the engine's order (Q-P11t, Q-P11v).
- P11 required tag groups by account type (Q-180) — blocked by U-42 — `tags::required_groups_u42` is PENDING — in place: kept on the company record and enforced by the forms' rule check (Q-P11u).
- P11/P13 several tags on one line (Q-179, Q-P10t) — blocked by U-45 — `tags::several_line_tags_u45` is PENDING — in place: one tag per line, refused in plain words before saving (Q-P11w).
- P11 your own terms (including discount terms) and payment methods — blocked by U-43 — `list_csv::terms_lists_u43` is PENDING — in place: the engine's fixed lists, read-only, with a note (Q-P11y).
- P11 item CSV import in one call and full contact rows — blocked by U-46 — `list_csv::items_import_u46` is PENDING — in place: items created one at a time; contacts import name, company, email and phone only (Q-P11x).
- P12 toggling cleared marks in the register (R5) — blocked by `register-cleared-v1` — in place: ✓/R read-only (Q-P12k). Also still PENDING for P12: U-17 (server-side version check; fingerprint stopgap) and U-18 (idempotency keys; lost-reply probe).

## Placeholders tracked

Screens or actions that link to a later phase. Remove each line when its phase lands.

- P34/P08 — restore UI must call `Cluster::restore(src, store, allow_unregistered)` with `allow_unregistered = false` by default and set it to `true` only after an explicit owner confirmation that the file came from a trusted source (Q-P03h).

- P06 shell placeholders (Q-P06g): every menu, rail and palette command whose screen is built later opens a placeholder tab. The phase that builds the screen removes its entry: P11 Item List / Tags / Clients & Jobs / Vendors, P12 Open Register, P13 Write Check, P14 Make Deposit / Transfer Funds / Sales Receipt / Record Card Charge, P15 Make Journal Entry, P16 Home (replaces the Home placeholder and the demo rail counts), P17 Enter Bill, P18 Pay Bills, P20 Create Invoice, P21 Receive Payment, P22 Credit Memo or Refund / Statements, P25 Import Bank File, P26 Bank Feed, P27 Reconcile, P28 Reports / Profit & Loss / Balance Sheet / Trial Balance, P29 Set Closing Date / Users and Permissions / Audit Trail, P30 Import Payroll, P32 Send to My Accountant, P38 bundled help (F1 says "not available yet").
- P06 native macOS menu bar (Q-70, muda) is deferred (Q-P06b): in-window menu on every platform until macOS packaging.

## Follow-ups from P03 security review

Findings from the P03 security review that were not fixed in P03. Fixed in P03: 1 (crafted backup runs code on restore), 2 and 15 (stop signalling an unrelated process), 3 (backup file mode on Unix), 5, 8, 12, 13, 14. Each item below names the phase that must pick it up; remove the line when it lands.

| # | Follow-up | Target |
|---|-----------|--------|
| 4 | Limit extraction size and check free disk space before unpacking a backup (a hostile archive can expand without bound). Company restores go through the engine's import, which caps sizes (P02 §2.5); the whole-cluster restore UI is P34. | P34 |
| 6 | A keychain hiccup must not downgrade an already initialized cluster to the file fallback store. Not done in P09 (no `cl-pg` touch; P09 only added keychain entries through `cl-settings`). | next `cl-pg` touch / P37 |
| 7 | Derive the fallback file key from OS APIs, not environment variables (user name, computer name). Not done in P09 (no `cl-pg` touch). | next `cl-pg` touch / P37 |
| 9 | Truncate `postgresql.auto.conf` on every start (restore already resets it). | next `cl-pg` touch (P04) |
| 10 | The engine must connect as a non-superuser application role, not the `postgres` superuser. | P04 |
| 11 | Owner-only ACL on the data directory, the `.secret` file, and backups on Windows (Unix backups are already 0600). Not done in P09. | P35 / P37 |
| 16 | Treat gvfs `smb-share:`, `sftp:`, and `dav:` mounts as network drives in `check_data_dir`. Not done in P08 (no `cl-pg` touch); next `cl-pg` touch or P35. | P35 |
| 17 | Canonicalize the parent folder when checking that a backup is not inside the data directory. | next `cl-pg` touch |
| 18 | `pg-fetch`: pass `--proto-redir =https --tlsv1.2` to curl, and consider verifying the Maven `.asc` signature when the pinned hashes are bumped. | next hash bump / P35 |

Notes kept for later phases:

- `startup.log` (the `pg_ctl -l` file) must stay outside the data directory: on Windows, after a crash the server fsyncs every file in the data directory and fails with a sharing violation on a log file that is still open (stalls startup).
- Pid-file ownership uses the pid, process name, and data directory from `postmaster.pid`; the process start time is not compared (no portable OS call without more dependencies).
- A running server whose pid file shows non-loopback settings is stopped and restarted (unit-tested on the pid file fields; no integration test starts a wider server).

## Follow-ups from P04 security review

No Critical or High findings. Fixed on `phase/P04-supervisor`: M1, M2, L1, L2, L5, L6, L7, L9, L10, L11 (see Q-P04p). Recorded, not fixed:

| # | Follow-up | Target |
|---|-----------|--------|
| M3 | Windows custom or portable data folders inherit "Authenticated Users: Modify": set an owner-only ACL or refuse such roots; the file-store key derivation is not protective on shared folders (Q-P04l, Q-P03c). | P35 / P09 |
| L3 | Fail closed after N failed listener lookups instead of warning forever (Q-P04m). | P37 |
| L4 | `cl-api` re-reads endpoint and generation for every request and verifies the listener before the first request after a restart; host `info`/`initialize` should verify the listener first (Q-P04n). | P05 |
| L8 | Engine exits when stdin closes / `PR_SET_PDEATHSIG` (added to U-16); write `run/engine.pid` atomically with the spawn (Q-P04o). | U-16 / P37 |
| Info | Suspended start under the job object and fail if job creation fails; no Node diagnostic reports; WER exclusion; secrets never rotate, so P34 recovery guidance must cover them (Q-P04o). | P35 / P34 / P37 |

## Follow-ups from P05 reviews

Fixed on `phase/P05-engine-client`: scope pinning (both reviews), Retry-After overflow (L1), Call `Debug` (L2), endpoint unpublished on engine exit (L3), candidate id parsing (L5), test-feature guard (L6), DNS block and checkout credentials (L7), code-review should-fix 1-4 and the nits (Q-P05ad). Recorded, not fixed:

| # | Follow-up | Target |
|---|-----------|--------|
| L4 | Best-effort secret wipe: zeroizing/constant-time `cl_pg::Secret` (or re-export `cl_core::Secret`), pre-sized `Zeroizing` body buffers (Q-P05aa). | P09 / P37 |
| L8 | Lint or review rule against logging `ApiError` payloads, models or amounts with `?value` at info level (Q-P05ab). | P37 |
| L9 | 64 KiB cap for engine error bodies (Q-P05ac). | P37 |
| AC5 | No contract test yet moves a transaction OUT of a closed period (old date closed, new date open); the engine checks both dates, the stub suite covers the client mapping. | P07 (with U-15) |

## Follow-ups from P10

- S14 tag gap (Q-P10t): engine lines hold one `tagId`; the grid allows several tag chips per line. P11 opened U-45 (several tags per line, Q-P11w); until it lands, P13 refuses to save a line with more than one tag through `cl_model::lists::tags::engine_line_tag`.

## Follow-ups from P13

1. A register tab left open shows stale rows after another window saves; needs a cross-window "transactions changed" refresh (the scenario closes the tab as a workaround). Fix before or in P14.
2. Native file picker for attachments (Q-P13k).
3. The `cl-host` cold-start budget test exceeds the 15 s local limit under load (CI uses `CL_HOST_BUDGET_SECS=30`).
4. Engine issues #157 (duplicate check numbers under concurrency) and #158 (float compare in the rules engine).

## Owner-requested checkpoints

- **After P13 merges, before P14 starts (owner, 2026-10-20):** Intuit compliance check — trademarks/names, copied text, icons and assets, layouts and colors vs QuickBooks, feature names, help text, fixtures, non-affiliation notice (non-negotiable 5). Report to the owner; fix findings on a `fix/` branch.

## Log

Newest first. One entry per completed phase: date, phase, summary, acceptance checklist results, notable decisions (Q-xx / U-xx).

Older entries (P00–P07, engine and planning notes) are in [docs/history/STATE-log.md](docs/history/STATE-log.md). Keep at most the last four entries here; the bookkeeper moves older ones there when a phase closes.

- 2026-10-20 — P13 complete on `phase/P13-write-check`; PR to open. Shipped: the Write Check form (check card, amount in words, "To print", Expenses lines with Customer:Job, Billable and Tags, Save & New / Save & Close, Prev/Next with a Save/Discard prompt, History, Attach, payee panel with the open-bill warning and recent checks, prefill from the last transaction except amounts, vendor default account, offline rule suggestions and a gated AI chip with explicit accept); check edit with the printed-check choice; the closing-password flow reused; the shared lines grid and closing prompt; Q-232 typed amounts round to the cent; Q-238 "Customers" wording; Q-250 a click opens list fields; sample data for the mockup story; vendor-center focus fix. Engine PR #156 merged (U-48 check edit, U-49 line customer, U-50 billable + migration 0221, U-51 rules evaluate); pin 454fd23. Contract suite 101/101; `write_check` scenario 254 steps light+dark, `--compare` 43 steps. P15A Batch Enter Transactions added to beta (Q-252).
  Acceptance (phases/M1-core-ledger.md P13): [x] the mockup example saves, the register shows it, the GL is correct (contract `write_check::gl_effect`); [x] no save when lines and amount disagree (unit test + scenario, nothing sent); [x] mockup match via `--compare` reviewed by ui-verifier, deviations Q-P13a–p.
  Agents: scout/opus, architect/opus, impl/opus (Fable out of credits, opus fallback for money packages), engine-impl/opus, security-review/opus, review/opus, ui/opus, bookkeeper/sonnet.
- 2026-10-20 — Engine pin 850e43d2 → 5c8e9665 (engine main with PRs #151 tenant archives, #152 AI desktop/session secrets, #153 register exact money, closing password, soft delete) on `chore/engine-pin-5c8e9665`. Live capabilities: `tenants-list-v1`, `tenant-archive-v1`, `session-secrets-v1`, `ai-desktop-v1`, `smtp-session-v1`, `ai-token-cap-v1`, `register-exact-v1`, `closing-password-v1`, `txn-delete-v1` (names match cl-api). Engine SEA rebuilt and smoke-tested; api-spec fixtures refreshed.
  Results: cl-api contract 91 passed, 0 failed (the formerly PENDING E5a archive round trip, closing password U-15, E6 session secrets/usage summary/SMTP session/token cap, register exact money, audit before-images and delete now run); `register_property::balance_holds_after_random_edits` at 1,000 operations with the register checks and deletes on: green (25 s, seed 54358578820153362); contract_offline 6/6; spec_fixtures 4/4; rest of the workspace green.
  Client fixes (the client follows the engine): closed-period test expects `CLOSING_PASSWORD_INVALID` for a password on a company without one; contract tests hand the Ollama address over session secrets (desktop mode ignores the stored URL) and name the session SMTP relay by LAN address (the engine resolves the host fail-closed); `OUTSIDE_SPEC` trimmed to desktop-only routes; `push_at_open` sends the AI and SMTP session secrets in separate requests so an unresolvable mail server offline no longer drops the Ollama address. No engine bugs found. Still PENDING: E5b, U-14, U-17–U-19, U-21–U-23, U-31–U-47, R5.
  ui-check register, open-company, preferences-ai, chart-of-accounts: pass, light and dark (`--demo` data). Agents: implementer on opus.
- 2026-10-19 — P12 complete on `phase/P12-register` (WP1–WP8, 34 commits; not pushed). Shipped: cl-api register/summary, bank postings (check, deposit, transfer, card charge), audit reader, soft delete and closing-password errors; cl-model register window cache, ranges, entry rules and `closing` flow; cl-ui register screen (two-line/one-line view, true balance, footer, entry row, edit and Splits, quick filters, Go To, QuickReport, lock/void/delete, conflicts); scenario `register.yaml`; WP3 property test `register_property::balance_holds_after_random_edits` (seeded, `CL_PROP_OPS`/`CL_PROP_SEED`, `CL_ENGINE_EXE` for the engine branch).
  Acceptance (phases/M1-core-ledger.md): (1) balance = engine account balance after 1,000 random edits — met for the account balance (oracle == `accounts.balance`, 1,000 ops, two seeds); the register per-line/ending/filtered checks PENDING by `register-exact-v1`; (2) closed-period edit without password refused clearly — met (423 contract + scenario); password path PENDING by `closing-password-v1`; (3) deleted transactions in the audit trail with all lines — PENDING by `txn-delete-v1`; (4) mockup — `ui-check register` light+dark screenshots in `target/ui-check/register/`, `--compare` review by ui-verifier to follow, deviations Q-P12r–w. Capabilities all on engine PR #153.
  Still PENDING: R5 cleared toggle (`register-cleared-v1`), U-17, U-18. Decisions Q-P12a–w added (Q-P12h: `accounts.balance` is debit-positive for every type, found by WP3).
  Agents: all implementers and the engine-side work on opus (Fable unavailable); code review, security review and ui-verifier to follow.
- 2026-10-18 — P11 complete on `phase/P11-lists` (steps 1-7, 21 commits; not pushed). Shipped: Chart of Accounts, client and vendor centers on one contact record, Item List, Tags and tag groups, read-only Terms and Payment Methods, list CSV import/export with column mapping (`cl-import` CSV, `cl-model` mapping, `cl-api` export/import routes), `cl_model::lists::tags` line rules for forms, and `cl_model::lists::class_map` (class → tag, `Parent:Child` → group + tag, Q-P11z) for P31.
  Acceptance: (1) keyboard-only create/edit/inactivate for every list — met for accounts, contacts (both centers), items, tags (scenarios `chart-of-accounts`, `client-center`, `vendor-center`, `items`, `tags`, `import-export`, light+dark); terms and payment methods PENDING by U-43 (read-only); (2) classes in fixtures become tags — met (`class_map` tests on `fixtures/lists/classes.csv` plus a property test); (3) 20k-record filter under 100 ms — met (accounts, contacts, items, tags tests).
  PENDING engine items: U-31 to U-46 (merge with lines, subtree balance, sensitive flag, masked numbers, jobs, addresses/people, notes/to-dos, contact balance, item kinds, price levels, tag merge/single-select, required groups, terms/methods, W-9, several tags per line, list imports); see Blocked items. Q-P11p is now an index of the per-item rows.
  Agents: engine-scout and all implementers on opus (Fable unavailable); code review and ui-verifier to follow.
