# ADD-02 — Read a vendor statement into Match Vendor Statement

**Status:** Active (added 2026-10-26, during the build) · **Part of the beta** · Phase **A06** · Decisions Q-295–Q-300 · Engine U-94 (only if the capability check finds a gap)

Owner request, 2026-10-26: "allow uploading and OCR of a vendor statement into the Match a vendor statement view".

This addendum adds work to a plan that is already being built. It never rewrites completed phases; where it changes shipped behavior (P17 Match Vendor Statement), it says so below.

Models: as ADD-01 (Q-277, Q-294): opus for all work, stepping up to effort xhigh and the escalation ladder in docs/AGENTS.md when needed.

## When to run it (scheduling rule)

The build was in A03 (ADD-01) when this was added, with P18 on hold by the owner.

1. A06 needs A03 (local reading: worker, OCR, line tables) and A04 (the compare tools and document-beside-form layout). Run **A06 right after A04 merges**, before the beta track resumes.
2. The owner's P18 hold applied until the owner said go on 2026-10-28 (P18 started then).
3. Before PR-A1 (ADD-01's checkpoint) runs, A06 is included in its scope ("Document to books" covers statements too).

Mark A06 in STATE.md like any other phase. Branch `phase/A06-statement-reading`; PR `A06 — Read a vendor statement`; final commit `docs(state): complete A06, next <phase>`.

## What changes for shipped work

| Shipped in | Was | Becomes | Migration |
| --- | --- | --- | --- |
| P17 step 7 (Match Vendor Statement) | Statement lines typed or pasted into a text box (Q-P17q) | Also **Read a statement file** (PDF or picture): lines are read locally and placed in the same box for the owner to check, with the document shown beside them | None; typing and pasting work as before |
| ADD-01 A01 intake folders | Folder kinds: Bills / Receipts / Let ClientLedger decide | Adds the kind **Vendor statements**: files from that folder open in Match Vendor Statement instead of becoming bill drafts | Existing folders keep their kind |

---

## A06 — Read a vendor statement

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | M | A03, A04 | rust-implementer (opus); engine-implementer (opus) only for U-94 | reviewer (opus), security-reviewer (opus) if the engine changes, ui-verifier |

**Decisions:** Q-P17q, Q-283, Q-284, Q-285, Q-A03j, Q-A03k, Q-295–Q-300. **Mockup:** none; follow the shipped Match Vendor Statement screen and DESIGN.md › Document viewer until a mockup is drawn (Q-293).

**Crates and files:** `crates/cl-docread/src/statement.rs` (new: statement header and line table), `crates/cl-docread/src/kind.rs` (statement kind), `crates/cl-ui/src/screens/bill/statement_match.rs` (changed: Read a statement, document pane, line spots), `crates/cl-ui/src/reading/` (statement job), `crates/cl-company/src/intake/` (Vendor statements kind), `xtask/src/gen_invoices/` (synthetic statements), `fixtures/docs/read/statements/`, `crates/cl-automation/scenarios/statement_match.yaml`.

**Steps**
0. **Capability check** (engine-scout): can a statement file be kept with the vendor (an attachment on the contact, or a document draft of kind `statement`)? Can `vendor_layout_templates` hold a statement layout separate from the vendor's bill layout? Does the engine report the vendor's balance as of a date? Record gaps as U-94 (Q-220 rules if large). — `docs(upstream): A06 capability check`
1. **Statement reading** in `cl-docread`, run in the sandboxed worker like every other read (no parsing in the desktop process):
   - header: vendor (name, address, account number), statement date, period from/to, opening balance, closing balance / amount due;
   - line table: date, ref. no., description, and the amount from a single amount column or from separate charges / credits / payments columns (sign by column), plus a running-balance column when printed;
   - multi-page tables (header repeated or not), "balance forward" lines, page subtotals and aging summaries recognized and kept out of the lines (Q-296);
   - every value keeps its page and spot. — `feat(docread): read vendor statements`
2. **Checks, flagged, never adjusted:** lines add up to closing − opening; each running balance follows from the one before; dates fall in the period; the vendor on the statement differs from the vendor picked in the form; duplicate lines. A failed check is shown beside the line; no value is changed and nothing is filled in to make it balance (non-negotiable 6, Q-A03 M7). — `feat(docread): statement checks`
3. **Match Vendor Statement, "Read a statement"** (Browse, drop a file, or open one from the inbox):
   - read locally (works offline, AI off); the vendor, period and lines fill the form as suggestions, in the same line format the box accepts today, so the owner can still edit any line;
   - the statement opens in the document pane beside the lines (pop-out works, A02); selecting a line highlights its spot on the page, and the reverse;
   - read-low-confidence values are marked like Enter Bill's "Read from the document — check it"; the owner accepts the lines (one Accept for all, or line by line) before **Match** runs;
   - "Ask AI to help" only as in A03: behind the cloud consents (Q-A03k), with the AI amount gate (Q-A03j) — an AI amount is kept only when the same figure is printed on the page;
   - if nothing can be read: "Couldn't read this statement — type or paste the lines instead." — `feat(statement): read a statement file`
4. **Balance line** (changes Q-P17q, owner may refuse, Q-297): when the statement's closing balance was read and the engine can report the vendor's balance on the statement date, show "Statement says you owe X; your books say Y" with the difference. Shown only; nothing is posted. — `feat(statement): balance comparison`
5. **Keep the statement** (Q-298): after Match, "Save this statement with the vendor" stores the file (and the period) so it can be opened again later; default on. Uses the step 0 result (U-94 if needed). Without engine support the button is hidden and the file is only held while the window is open. — `feat(statement): keep the statement`
6. **Statement layouts** (Q-299): when the owner accepts a read statement, the vendor's statement layout is learned (separate from the vendor's bill layout) and used next time, as A03 templates are. — `feat(docread): statement layouts`
7. **Intake folder kind "Vendor statements"** (A01): a file arriving in such a folder appears in the inbox as a statement; opening it starts Match Vendor Statement with the file read. — `feat(inbox): vendor statement folders`
8. **Synthetic statements** in `xtask gen-invoices`: 12+ layouts (single amount column, charges/credits/payments columns, running balance, multi-page, balance forward, aging footer), with scanned and photo variants and held-out layouts, plus ground truth. — `feat(xtask): synthetic statements`

**Tests:** per-line accuracy on the synthetic set (date, ref. no. and amount all right counts as one correct line), seen and held-out; multi-page tables; balance-forward and aging rows excluded; sum and running-balance checks flagged, never adjusted (property test with injected errors); wrong-vendor warning; offline read with AI off; AI amount gate with an invented amount; consent off → no cloud call; statement kept and reopened; intake folder of kind Vendor statements; control-channel scenario (read a PDF statement, a scan, an unreadable file; accept; Match; Create bill from a missing line) in light and dark.

**Acceptance** (targets follow Q-285)
- [ ] AI off and networking disabled: statement lines read with date, ref. no. and amount all correct for ≥ 95% of lines on text PDFs and ≥ 85% on scans and photos in the synthetic set, held-out layouts included.
- [ ] Statement totals that don't reconcile are flagged and never adjusted.
- [ ] The read lines go through the existing Match exactly as typed lines do; nothing is posted without the owner's action.
- [ ] The statement is shown beside its lines, with each line linked to its spot on the page.
- [ ] A malformed or hostile file can't crash or hang the desktop app (the A03 sandbox).

---

## Decisions (add to QUESTIONS.md, "Default in force" until the owner answers)

| ID | Topic | Default |
| --- | --- | --- |
| Q-295 | Where A06 runs | Right after A04, before the beta track resumes; P18 stays on hold until the owner says go |
| Q-296 | Lines left out | "Balance forward", page subtotals, totals and aging summaries are read as header values, never as lines |
| Q-297 | Balance comparison | Show the statement's closing balance against the books' balance on the statement date when the engine can report it; display only. Changes Q-P17q's "no balance comparison" |
| Q-298 | Keep the statement | "Save this statement with the vendor" after Match, default on; needs engine support (step 0 / U-94), otherwise hidden |
| Q-299 | Statement layouts | Learned per vendor on accept, separate from the bill layout; travel like A03 templates (backups yes, transfer files no) |
| Q-300 | Page limit | Up to 20 pages per statement (A03 reads 10 for bills; statements run longer); same per-page time limits |

## Notes for later phases

- **P29 (permissions):** reading a statement needs the same right as Match Vendor Statement; saving it with the vendor needs the vendor-edit right.
- **P32 (transfer file):** saved statements travel as attachments of the vendor; statement layouts are left out like other layouts (Q-A03g).
- **P38 (docs):** update the "Match a vendor statement" help page and the switcher guide.
