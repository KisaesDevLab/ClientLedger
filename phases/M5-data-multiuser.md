# M5 — Data movement and multi-user (P31–P34)

**Outcome:** QuickBooks Desktop conversions that tie out, spreadsheet imports, transfer files for the accountant's MyBooks, IIF export, LAN multi-user, scheduled backups, and data-integrity tools.

**Milestone exit:** all phases merged; **PR-7** (after P32) and **PR-8** (after P33) process reviews with no unresolved Blocking items; milestone summary in STATE.md.

---

## P31 — QuickBooks Desktop import with tie-out, spreadsheet import, and import from other accounting systems

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | L | P11, P28 | rust-implementer (opus); engine-implementer for U-07 | reviewer (opus), ui-verifier |

**Decisions:** Q-24, Q-77, Q-78, Q-124, Q-125, Q-142–Q-145, Q-218, Q-275. **Engine:** U-07 (desktop adjustments to the MyBooks Online report-export importer; report list and mapping unchanged); the MyBooks Bulk Import pipeline (`/imports`: kinds coa, contacts, trial_balance, gl_transactions; source systems accounting_power, quickbooks_online, quickbooks_desktop, generic) for step 7 — capability check first, gaps just in time (Q-230).

**Mockup:** `docs/design/mockups/QBDImport.html` and `docs/design/screenshots/QBDImport.png` (tie-out step; other wizard steps follow its layout).

**Crates and files:** `crates/cl-ui/src/screens/qbd_import/{wizard.rs,mapping.rs,tieout.rs,spot_check.rs}`, `crates/cl-ui/src/screens/csv_transactions.rs`, `crates/cl-ui/src/screens/system_import/{wizard.rs,mapping.rs,tieout.rs}`, `fixtures/qbd/*` (synthetic exports), `docs/user/qbd-switch.md` draft.

**Steps**
1. engine-scout reads the shared importer: required reports, supported QBD versions (Q-218), history options, Other Names mapping; record findings in QUESTIONS.md. — engine notes
2. **U-07**: progress events, local file paths, tie-out endpoint returning per-account and per-open-item differences. — engine PR
3. Wizard (owner language): per-report export steps; drag-and-drop; validation; mapping review (accounts, names merged across lists Q-77, items, classes → tags); history depth choice (Q-142); conversion date; automatic backup; one-batch import with rollback. Not imported: memorized transactions, users (Q-143, Q-145). — `feat(qbd): import wizard`
4. Tie-out report vs. QBD trial balance and A/R/A/P agings; 0.00 required to mark verified. — `feat(qbd): tie-out`
5. Guided spot-check list (Q-144) saved with the conversion record. — `feat(qbd): spot check`
6. Spreadsheet transaction import (Q-125): invoices, bills, journal entries from CSV; saved mappings; row-level validation preview; balanced-entry checks; one-batch rollback. — `feat(import): csv transactions`
7. Import from another accounting system (Q-275), on the engine's Bulk Import pipeline: source **Accounting Power**, **QuickBooks Online**, **QuickBooks Desktop GL** or **Generic CSV**; what to import — chart of accounts, customers and vendors, trial balance (opening balances as of a date), GL transactions; per-step file pick with the exact report/export to make in the source system (plain language); preview with row-level problems; mapping review (accounts by number then name, account types, names merged across lists Q-77, tags); automatic verified backup first; one-batch import with rollback; tie-out of the imported trial balance against the source file to 0.00 (same tie-out screen as step 4); imported transactions marked with their source; re-importing the same file is detected and refused. Synthetic fixtures for each source (never real client files; private fixtures Q-72 optional). — `feat(import): other accounting systems`

**Tests:** synthetic export set ties out to 0.00; altered fixture points to the right account; rollback restores the pre-import state exactly; CSV import validation; optional private-fixtures tests (Q-72) when present.

**Acceptance**
- [ ] Synthetic QBD exports import and tie out to 0.00 on TB and both agings.
- [ ] An altered fixture shows a difference on the right account.
- [ ] Rollback restores the exact pre-import state.
- [ ] Synthetic Accounting Power, QuickBooks Online, QuickBooks Desktop GL and generic CSV files import and tie out to 0.00; re-importing the same file is refused.
- [ ] Screen matches its mockup per docs/design/README.md (`ui-check --compare` reviewed by ui-verifier); any deviation is listed in the PR and QUESTIONS.md.

---

## P32 — Transfer file for MyBooks Online, and IIF export

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | M | P08, P31 | rust-implementer (opus); engine-implementer for U-08 | reviewer (opus), security-reviewer (opus) |

**Decisions:** Q-25, Q-43, Q-74, Q-91, Q-126. **Engine:** U-05 (tenant export), U-08 (Online import of `.clxfer`).

**Mockup:** `docs/design/mockups/SendToAccountant.html` and `docs/design/screenshots/SendToAccountant.png` (excluding the PROPOSED "Send directly" panel, Q-227).

**Crates and files:** `crates/cl-company/src/{transfer.rs,iif.rs}`, `crates/cl-ui/src/screens/send_to_accountant.rs`, `.github/workflows/transfer-roundtrip.yml`.

**Steps**
1. `.clxfer` built on the tenant export: lists, transactions, attachments, audit history (Q-91), tags, memorized reports, settings; manifest with `formatVersion` (Q-43) and verification totals; secrets excluded. — `feat(transfer): clxfer export`
2. "Send to my accountant" screen with plain instructions and the totals the accountant should see (export only, Q-25). — `feat(transfer): screen`
3. **U-08**: MyBooks Online import showing verification totals; accepts older versions, refuses newer with an update message. — engine PR
4. IIF export (Q-126): accounts, clients, vendors, items, journal entries (other transactions summarized as journal entries); documented limitations. — `feat(export): iif`
5. CI round trip (Q-74): start the MyBooks appliance container, import the sample `.clxfer`, compare totals (main and release branches). — `ci: transfer round trip`

**Tests:** secret scan of the package; version compatibility tests; IIF validated by a parser test; round-trip CI.

**Acceptance**
- [ ] Round trip matches all verification totals.
- [ ] Package contains no secrets.
- [ ] Screen matches its mockup per docs/design/README.md (`ui-check --compare` reviewed by ui-verifier); any deviation is listed in the PR and QUESTIONS.md.

### Checkpoint PR-7 — Data movement (fable)
Tenant export/import, backups and restore, QBD import and tie-out, CSV import rollback, transfer files, IIF. Adversarial cases: restore during an import, transfer from an older version, partial attachment loss. Fix Blocking findings on `fix/PR-7`.

---

## P33 — LAN multi-user (`cl-lan`)

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | L | P29 | rust-implementer (opus) | reviewer (opus), security-reviewer (opus), ui-verifier |

**Decisions:** Q-108–Q-111. **Engine:** LAN bind mode (`CL_LAN=1`), TLS, sessions, optimistic concurrency on all records, change notifications (SSE or polling), single-user maintenance mode.

**Crates and files:** `crates/cl-lan/src/{host.rs,service.rs,tls.rs,pinning.rs,discovery.rs,client.rs,maintenance.rs}`, `crates/cl-ui/src/screens/{lan_setup.rs,conflict_dialog.rs}`.

**Steps**
1. Host mode: choose interface; private ranges only, refuse public (Q-109); self-signed cert and fingerprint display; firewall guidance. — `feat(lan): host mode`
2. Optional background service: Windows service, launchd, systemd (Q-108). — `feat(lan): host service`
3. Workstation mode: mDNS discovery (permissive crate) or manual address; fingerprint pinning; login. — `feat(lan): workstation`
4. Live refresh and 409 conflict dialog with field-level diff. — `feat(lan): conflicts`
5. Host-down behavior: block work, keep the open form's unsaved state locally, offer it after reconnecting (Q-110). — `feat(lan): host down`
6. Single-user maintenance mode for restore, import, rollback. — `feat(lan): maintenance mode`

**Tests:** two-workstation conflict; host restart reconnect; five workstations for 30 minutes within the save budget (Q-111); public-interface refusal; pinning mismatch refusal.

**Acceptance**
- [ ] Conflicting edits get the conflict dialog; no lost update.
- [ ] Workstations reconnect after a host restart; saved work intact.
- [ ] Five concurrent users for 30 minutes meet the save budget.

### Checkpoint PR-8 — LAN multi-user (fable)
Concurrency on every form type, host loss mid-save, maintenance mode during active sessions, notification gaps. Fix Blocking findings on `fix/PR-8`.

---

## P34 — Backup scheduling, recovery, and data integrity

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | M | P08, P33 | rust-implementer (opus); engine-implementer for U-12 | reviewer (opus) |

**Decisions:** Q-42, Q-170–Q-173, Q-217. **Engine:** U-12.

**Crates and files:** `crates/cl-company/src/{schedule.rs,verify.rs,health.rs}`, `crates/cl-ui/src/screens/{backup_settings.rs,verify_data.rs,crash_recovery.rs}`.

**Steps**
1. Scheduled backups to any folder including external drives and cloud-sync folders (Q-42); post-write verification; retention; missed schedules at next launch. — `feat(backup): schedules`
2. Test restore into a temp tenant with TB comparison. — `feat(backup): test restore`
3. **U-12** integrity endpoints; Verify Data screen with fixes after automatic backup (Q-170); weekly quick check on open (Q-171); duplicate finder (Q-172, report in P28). — `feat(integrity): verify data`
4. Crash recovery dialog; backup history; 7-day backup reminder (Q-217). No condense (Q-173). — `feat(backup): recovery and reminder`

**Tests:** schedules with simulated clock; test restore pass/fail; seeded corruption detected and fixed by Verify; reminder after 7 days.

**Acceptance**
- [ ] Scheduled backups run while open; missed ones run at next launch.
- [ ] Test restore reports pass/fail with TB comparison.
- [ ] Verify Data finds and fixes seeded integrity faults after a backup.
