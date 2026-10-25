# ADD-01 — Document intake, local reading, and pop-out viewer

**Status:** Active (added 2026-10-25, during the build) · **Part of the beta** · Phases **A01–A05** · Decisions Q-279–Q-293 · Engine U-92, U-93 · Process review PR-A1 (opus, xhigh)

IDs renumbered on apply, 2026-10-25: Q-230→Q-279 … Q-244→Q-293, U-14→U-92, U-15→U-93 (the bundle's numbers were already taken).

Models (owner 2026-10-25, Q-294): opus for all A-phase work and checkpoint PR-A1, stepping up to effort xhigh (and the escalation ladder in docs/AGENTS.md) when needed; Q-277 stands.

This addendum adds work to a plan that is already being built. It never rewrites completed phases; where it changes behavior that has shipped (P09, P17), it says so and includes the migration.

## When to run it (scheduling rule)

The build was between P17 and P26 when this was added.

1. Finish the phase currently in progress and merge it.
2. Run **A01 → A02 → A03 → A04** next (A02 may run alongside A01 under the parallel rules in docs/AGENTS.md). Then resume the beta track where it left off.
3. Run **A05** immediately after **P26** merges (or right after A04 if P26 is already merged).
4. Run checkpoint **PR-A1** after A05.
5. Before starting any later phase, read **Notes for later phases** below; those requirements apply to that phase.

Mark each A-phase in STATE.md like any other phase. Branches: `phase/Axx-short-name`; PR titles: `Axx — <title>`; final commit `docs(state): complete Axx, next <phase>`.

## What changes for shipped work

| Shipped in | Was | Becomes | Migration |
| --- | --- | --- | --- |
| P09 step 5 | One receipt inbox folder picked in preferences (stored locally) | Mapped intake folders, a **setting of each company** (A01) | On first launch after A01, the old folder becomes this company's first intake folder named "Receipts", kind "Let ClientLedger decide", auto-read off; the old preference is removed; nothing in the folder moves until the owner handles it |
| P17 step 1 | PDF viewer docked in forms | Viewer with images/TIFF, pop-out window, compare tools (A02, A04) | None; existing attachments open in the new viewer |
| P17 step 4 | Inbox polling one folder; files moved into attachments | Watches all of the company's folders; copies into attachments; originals move to `Processed/` (A01) | Files already moved into attachments stay where they are |
| P17 step 5 | Document reading with AI, online only | Local reading that works with AI off and offline (A03); the existing cloud reading becomes an optional reader behind its own consent (Q-284) and goes through the same checks | Existing AI setting carries over; cloud reading stays off until the owner consents on the new screen |

---

## A01 — Mapped intake folders, a setting of each company

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | M | P17 (shipped) | rust-implementer (opus); engine-implementer (opus) for U-93 | reviewer (opus), security-reviewer (opus), ui-verifier |

**Decisions:** Q-54, Q-287, Q-288, Q-289, Q-292. **Engine:** U-93.

**Crates and files:** `crates/cl-company/src/intake.rs`, `crates/cl-ui/src/screens/preferences/intake_folders.rs`, `crates/cl-ui/src/screens/receipt_inbox.rs` (changed), `crates/cl-settings/src/migrate_inbox.rs`, `fixtures/intake/`.

**Steps**
1. **U-93** engine: company preference for intake folders (name, path, default kind Bills / Receipts / Let ClientLedger decide, auto-read on/off), saved with the company, included in backups, excluded from transfer files and IIF. — engine PR
2. **Company preferences › Document intake** (Admin only): add, rename, remove, and **Test** folders — local folder, mapped drive, UNC network share, or cloud-sync folder (OneDrive, Dropbox, Google Drive for desktop). Only the open company's folders are watched; switching companies switches folders. — `feat(settings): intake folders`
3. Migration from the P09 single folder (table above). — `feat(settings): migrate receipt inbox folder`
4. Watching: file watcher if a crate passes the direct-dependency license rule, otherwise polling every 10 s. A file counts as arrived only after its size is stable for 5 s and it can be opened; hidden, temporary, and zero-byte files ignored; symlinks leading outside the folder refused; files over 10 MB listed as too large (Q-50); HEIC files listed with "Save as JPG or PDF instead" (Q-292). — `feat(inbox): watch intake folders`
5. On arrival: copy into the company's attachments store, hash for duplicate checks, list in the receipt inbox with the folder name; if auto-read is on, queue for reading (A03/A04). — `feat(inbox): arrival`
6. After handling (Q-288): move the original to `Processed/YYYY-MM/` or `Discarded/` inside the mapped folder; never delete; option to leave originals untouched. — `feat(inbox): processed and discarded`
7. Folder health on the inbox and Home: unreachable or missing folder (unplugged drive, share offline, sync paused), permission denied, folder mapped by two companies, and after a restore on another computer "Set this company's bill folder for this computer". — `feat(inbox): folder health`

**Tests:** half-written file waits; hidden/temp ignored; symlink escape refused; unreachable share reported; same file twice flagged once; migration from the P09 setting; folders survive backup/restore and are absent from a transfer file; two companies with different folders.

**Acceptance**
- [ ] A file saved into a company's mapped folder (local, mapped drive, or share) appears in that company's inbox within 15 s and is copied into attachments.
- [ ] Originals move to `Processed/` or `Discarded/` and are never deleted.
- [ ] Folders are per company, carried by backups, and excluded from transfer files.
- [ ] The old single-folder setting migrates without losing or moving any file.

---

## A02 — Pop-out document viewer

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | M | P17 (shipped); may run alongside A01 | rust-implementer (opus) | reviewer (opus), ui-verifier |

**Decisions:** Q-57, Q-290, Q-292, Q-293. **Mockup:** Enter Bill (docked); popped-out window follows DESIGN.md › Document viewer until a mockup is drawn (Q-293).

**Crates and files:** `crates/cl-ui/src/widgets/pdf_viewer.rs` (changed), `crates/cl-ui/src/windows/doc_viewer.rs`, `crates/cl-model/src/keymap.rs` (two shortcuts), `docs/KEYBOARD.md` (regenerated), `crates/cl-automation/scenarios/doc_viewer.yaml`.

**Steps**
1. Viewer formats: PDFs via PDFium; JPG, PNG, and multi-page TIFF via the `image` crate; zoom (fit page, fit width, %), rotate, pan, page thumbnails. — `feat(ui): viewer formats`
2. **Pop out / Dock** (button and `Ctrl+Shift+D`): the viewer moves to its own OS window (egui multi-viewport) that can sit on a second monitor; one viewer window is reused and follows the active form or queue item; title shows the form's name with the company color stripe; monitor, position, size, and zoom remembered per user; closing the viewer never affects the form. — `feat(ui): pop-out viewer`
3. `F6` moves focus between the form and the viewer; both shortcuts in the keymap, menus, and KEYBOARD.md. — `feat(model): viewer shortcuts`
4. Works for every attachment: bills, checks, card charges, register items. — `feat(ui): viewer everywhere`

**Tests:** control-channel scenario — pop out, switch forms, viewer follows, dock, restart restores placement; multi-monitor coordinates clamped when a monitor is gone; keymap conflict test.

**Acceptance**
- [ ] The document pops out to its own window, follows the active form, docks back, and restores its placement after restart.
- [ ] JPG, PNG, PDF, and multi-page TIFF all display.

---

## A03 — Local document reading engine

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | L | A01 | rust-implementer (opus); engine-implementer (opus) for U-92 | architect (opus), reviewer (opus), security-reviewer (opus) |

**Decisions:** Q-279, Q-280, Q-281, Q-283, Q-284, Q-285. **Engine:** U-92 (document drafts, vendor layout templates); engine-scout reports what field reading the engine already has (P17's cloud reading) so it is reused, not duplicated.

**Crates and files:** `crates/cl-docread/src/{lib.rs,pdf_text.rs,ocr.rs,preprocess.rs,layout.rs,fields.rs,lines.rs,vendor_match.rs,templates.rs,validate.rs,dupes.rs}`, `apps/clientledger-docread/src/main.rs`, `crates/cl-host/src/docread_worker.rs`, `xtask/src/{gen_invoices.rs,ocr_models.rs}`, `fixtures/docs/`, `ATTRIBUTION.md`.

**Pipeline** — a PDF or photo becomes a draft **on the owner's computer, with no internet and no AI required**:
1. **Text:** PDF text layer → words with positions (PDFium). Scans and photos → clean-up (rotate, deskew, contrast, crop) → offline OCR → words with positions and confidence.
2. **Fields:** vendor, invoice date, reference, terms, due date, subtotal, tax, total, amount paid, payment method (card last 4, "PAID", check no.), line items from column alignment — by label/pattern rules checked against layout, and by **vendor layout templates** learned when the owner accepts a vendor's document.
3. **Match:** vendor to the vendor list (name, address, phone, past layouts) or offer a new vendor; accounts from the vendor's default and history.
4. **Check:** lines = subtotal; subtotal + tax = total; due date agrees with terms; mismatches flagged, never forced. Duplicates: same file hash; same vendor + reference; same vendor + amount + date within 60 days.
5. **Kind:** unpaid invoice → draft bill; paid receipt → draft card charge, check, or cash expense (card last 4 picks the card account).

**Steps**
1. Spike: compare OCR engines on synthetic and private-fixture documents; check code **and model-file** licenses against the allowlist; record the choice in Q-283. — `docs(questions): ocr engine choice`
2. Text layer, clean-up, and OCR; models fetched and checksum-verified by `xtask ocr-models` and bundled (no run-time download). — `feat(docread): text and ocr`
3. Sandboxed worker `clientledger-docread`, started by `cl-host` per batch: no network, 30 s per page, memory cap; crash or timeout → "Couldn't read, enter by hand". Untrusted files never parse inside the desktop process. — `feat(host): docread worker`
4. Fields, line-item tables, vendor matching, checks, duplicates. — `feat(docread): fields and checks`
5. **U-92** engine: store drafts (fields, positions, confidence, source attachment) and vendor layout templates; convert a draft into a bill, card charge, check, or expense on accept; drafts excluded from every balance and report. — engine PR
6. Optional readers through the same checks: local Ollama (text or vision model) when the company's AI uses Ollama; P17's cloud reading kept as an option behind its own consent screen (images can't be redacted, Q-284). — `feat(docread): optional readers`
7. `xtask gen-invoices`: synthetic bills and receipts in 20+ layouts rendered with `cl-print`, plus "scanned" versions (rotation, blur, noise, phone perspective). — `feat(xtask): synthetic documents`

**Tests:** per-field accuracy report per path (text PDF, OCR, template, Ollama) on the synthetic set and on `private-fixtures/` when present (never committed); validation and duplicate fixtures; worker timeout, crash, and oversized-file cases; network-capture test proving no outbound calls with AI off; drafts never change a balance.

**Acceptance** (Q-285)
- [ ] AI off and networking disabled: header fields (vendor, date, reference, total) ≥ 95% on text PDFs and ≥ 85% on scans/photos in the synthetic set.
- [ ] Second document from a vendor with a saved template: ≥ 98% on header fields.
- [ ] Non-reconciling totals are flagged, never adjusted.
- [ ] Text PDF ≤ 1 s per page and OCR ≤ 5 s per page on the minimum hardware (4 GB RAM, 2 cores).
- [ ] A malformed or hostile file can't crash or hang the desktop app.

---

## A04 — Review queue and compare tools

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | M | A02, A03 | rust-implementer (opus) | reviewer (opus), security-reviewer (opus), ui-verifier |

**Decisions:** Q-281, Q-282, Q-286, Q-291. **Mockup:** Enter Bill ("Filled from <file>" banner, document beside the form); queue and check list follow DESIGN.md until mockups are drawn (Q-286, Q-293).

**Crates and files:** `crates/cl-ui/src/screens/doc_review/{queue.rs,review.rs,highlight.rs,checklist.rs}`, `crates/cl-model/src/doc_draft.rs`, `crates/cl-ui/src/screens/receipt_inbox.rs` (Read all).

**Steps**
1. **Read all** in the receipt inbox, plus automatic reading of folders with auto-read on (A01). — `feat(inbox): read all and auto-read`
2. Review queue with status (Ready, Check these fields, Possible duplicate, Couldn't read) and keyboard review (J/K move, Enter open, Alt+S accept, Del discard); count shown on Home's Needs attention. — `feat(ui): review queue`
3. Drafts open in the normal Enter Bill, card charge, check, or expense form with the "Filled from <file>" banner; nothing posts until the owner accepts; accepting or discarding moves the original per A01. — `feat(ui): drafts into forms`
4. Receipts that match an existing register transaction (amount, date window, payee) are attached to it instead of creating a new one (Q-282). — `feat(docread): attach to existing`
5. **Compare tools** in the viewer, docked or popped out (Q-291): field ↔ page highlights both ways with confidence colors (sure / check this / not found); highlights toggle; **click to fill** — with a field focused, click a word or drag a box to put that text in the field (amounts parsed exactly; a dragged box is re-read by OCR); **check list** of each value beside a cropped snippet of its source. — `feat(ui): compare tools`
6. Corrections on accept update the vendor's layout template; the banner says "Filled from <vendor>'s saved layout" next time. — `feat(docread): learn on accept`

**Tests:** queue scenarios for each status; accept posts exactly the reviewed values; discard leaves no transaction; receipt for an existing transaction attaches instead of duplicating; click-to-fill on amounts keeps exact cents; template improves the second document.

**Acceptance**
- [ ] A folder of mixed bills and receipts can be read and worked through by keyboard, each accepted draft posting exactly what was reviewed.
- [ ] Every filled field links to its spot on the page in both the docked and popped-out viewer; click to fill works.
- [ ] No duplicate expense is created for a receipt already in the register.

---

## A05 — Match document drafts in the Bank Feed

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | S | A04, P26 | rust-implementer (opus) | reviewer (opus), ui-verifier |

**Decisions:** Q-282.

**Steps**
1. A draft whose amount, date window, and payee match a Bank Feed item is offered as that item's attachment and coding instead of a new transaction; the Bank Feed shows a "Receipt" badge and the document in the viewer. — `feat(feed): match document drafts`
2. A bank item accepted first and a receipt arriving later attach to the posted transaction (via A04 step 4). — `test(feed): late receipt`

**Acceptance**
- [ ] Receipt first then bank file, and bank file first then receipt, both end with one transaction and the receipt attached.

### Checkpoint PR-A1 — Document to books (opus, xhigh)
Folder arrival → reading (text, OCR, template, optional Ollama) → checks and duplicates → draft → accept → bill or expense → Pay Bills or Bank Feed match → reconcile; plus migration from the old inbox folder, restore on another computer, and a hostile file.

---

## Notes for later phases (apply when those phases run)

- **P32 Transfer file / IIF:** leave intake folder paths out (U-93); include attachments that came through intake as normal attachments.
- **P33 LAN multi-user:** the LAN host watches the open company's intake folders and runs the reading worker; paths must be valid on the host (Q-289); workstations see the shared inbox, queue, and viewer.
- **P34 Backups:** intake folder settings and vendor layout templates are part of the company backup; Verify Data checks drafts never affect balances.
- **P38 Documentation:** owner guide sections for intake folders, reading, the review queue, and the pop-out viewer.
- **Remote add-on (separate plan):** phone receipts land in the company's inbox and are read by A03/A04.
