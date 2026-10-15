# M2 — Money in and out (P17–P24)

**Outcome:** bills and Pay Bills with printed checks, invoices and payments with all the owner-facing edge cases, credit memos, statements, finance charges, sales tax, and average-cost inventory.

**Milestone exit:** all phases merged; process reviews **PR-2** (after P19), **PR-3** (after P22), and **PR-4** (after P24) with no unresolved Blocking items; milestone summary in STATE.md.

---

## P17 — Enter Bill, vendor credits, receipt inbox, attachment viewer

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | L | P13 | rust-implementer (sonnet) | reviewer (opus), ui-verifier |

**Decisions:** Q-13, Q-50, Q-54, Q-73, Q-166, Q-168, Q-191, Q-192. **Engine:** bills and vendor credits (terms, discount date, due date), attachments, duplicate-ref detection, document field reading (AI), vendor statement matching support.

**Mockup:** `docs/design/mockups/EnterBill.html` and `docs/design/screenshots/EnterBill.png`.

**Crates and files:** `crates/cl-ui/src/screens/bill/{form.rs,vendor_panel.rs,statement_match.rs}`, `crates/cl-ui/src/widgets/pdf_viewer.rs`, `crates/cl-ui/src/screens/receipt_inbox.rs`, `crates/cl-model/src/forms/bill.rs`; PDFium binaries via `xtask` and ATTRIBUTION.md.

**Steps**
1. PDF viewer widget on `pdfium-render` with dynamically loaded PDFium (Q-13); zoom, page nav, image files too. — `feat(ui): pdf viewer`
2. Bill/vendor-credit form per DESIGN.md; terms drive discount and due dates; Expenses/Items; bill received; prefill and default account rules. "Select PO" hidden until P41. — `feat(bill): form`
3. Attachments ≤ 10 MB each (Q-50) in the company's attachments folder; included in backups and transfers. — `feat(bill): attachments`
4. Receipt inbox (Q-54): poll the chosen folder (or a watcher crate passing the direct-dependency license rule); inbox pane; attach to new/existing bill, check, or bank item; move into the attachments folder. — `feat(inbox): receipt inbox`
5. Document reading (AI on and online only): fill fields, highlight uncertain lines, totals must reconcile to the document; owner accepts. — `feat(bill): document reading`
6. Vendor snapshot (open bills, unused credits, duplicate-ref warning, 18-month window); no approval step (Q-166). — `feat(bill): vendor panel`
7. Vendor statement match (Q-168): enter or paste statement lines; match to bills, credits, payments; show missing on each side; create missing bills. — `feat(bill): statement match`

**Tests:** bill posts to A/P; duplicate-ref warning; AI-off scenario; statement-match fixture with one missing bill on each side; attachment size limit.

**Acceptance**
- [ ] Bill with expense + item lines posts to A/P; vendor balance updates.
- [ ] Duplicate ref within 18 months warns.
- [ ] AI off/offline leaves manual entry unaffected.
- [ ] Screen matches its mockup per docs/design/README.md (`ui-check --compare` reviewed by ui-verifier); any deviation is listed in the PR and QUESTIONS.md.

---

## P18 — Pay Bills (checks)

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | M | P17 | rust-implementer (sonnet) | reviewer (opus), ui-verifier |

**Decisions:** Q-10, Q-167. **Engine:** bill payments with discounts, credits, partial payments, one check per vendor.

**Mockup:** `docs/design/mockups/PayBills.html` and `docs/design/screenshots/PayBills.png`.

**Crates and files:** `crates/cl-ui/src/screens/pay_bills.rs`, `crates/cl-model/src/forms/bill_payment.rs`.

**Steps**
1. Filters (due on or before / all, vendor, sort) and grid columns per DESIGN.md. — `feat(paybills): selection`
2. Auto-apply eligible discounts (setting); Set discount/credits dialogs; partial payments. — `feat(paybills): discounts and credits`
3. Payment panel: date, check (to print / assign numbers), account, balance before → after; one check per vendor; queue to Print Forms. — `feat(paybills): payment`

**Tests:** mockup example totals to the penny (4 of 5 bills selected: discount 4.37, credits 250.00, pay 6,577.53; ending balance 84,210.55 → 77,633.02); partial payment remaining balance; discount posting.

**Acceptance**
- [ ] The mockup's Pay Bills totals match exactly.
- [ ] Partial payments and discounts post correctly.
- [ ] Screen matches its mockup per docs/design/README.md (`ui-check --compare` reviewed by ui-verifier); any deviation is listed in the PR and QUESTIONS.md.

---

## P19 — Form printing (`cl-print`)

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| L | L | P18 | rust-implementer (sonnet) | ui-verifier |

**Decisions:** Q-14, Q-14b, Q-55, Q-92–Q-95, Q-201, Q-212, Q-213.

**Crates and files:** `crates/cl-print/src/{pdf.rs,layout.rs,checks/{standard.rs,voucher.rs},forms/{invoice.rs,statement.rs,sales_receipt.rs,credit_memo.rs,deposit_summary.rs},numbering.rs,print_queue.rs,os_print.rs}`.

**Steps**
1. PDF engine on a permissive crate; layout spec type with mm coordinates; fonts embedded. — `feat(print): pdf layout engine`
2. Checks on pre-printed stock: standard (3/page) and voucher (1/page) with stubs listing bills paid (Q-213); per-account layout; alignment dialog and test print; no MICR, no signature (Q-212). — `feat(print): checks`
3. Built-in form template: logo, company block, #10 double-window address positions, footer payment instructions (Q-92–Q-95); invoices, statements, sales receipts, credit memos, deposit summary; hide-$0-lines option (Q-201). — `feat(print): forms`
4. Numbering per document type with optional prefix; duplicates warned (Q-94). — `feat(print): numbering`
5. Print Forms queue; "Did all checks print correctly?"; reprint with new numbers and void originals, logged (Q-55). — `feat(print): print queue`
6. OS print hand-off (Windows print verb, `lp`), fallback open PDF (Q-14b). — `feat(print): os print`

**Tests:** PDF coordinate tests (±0.5 mm) for checks and window-envelope addresses; reprint voids and logs; numbering duplicates.

**Acceptance**
- [ ] Field positions within ±0.5 mm at 100% scale.
- [ ] Reprint voids the original number and logs it.

### Checkpoint PR-2 — Purchasing lifecycle (fable)
Bill → vendor credit → Pay Bills (discounts, credits, partials) → check printing → reprint/void → vendor statement match. Fix Blocking findings on `fix/PR-2`.

---

## P20 — Create Invoice

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | L | P11, P19 | rust-implementer (sonnet) | reviewer (opus), ui-verifier |

**Decisions:** Q-34, Q-58, Q-68, Q-94, Q-128, Q-129, Q-162–Q-165, Q-198–Q-201. **Engine:** invoices with billable costs and markup, discount/subtotal items, price levels, service dates, sent tracking, email outbox.

**Mockup:** `docs/design/mockups/CreateInvoice.html` and `docs/design/screenshots/CreateInvoice.png`.

**Crates and files:** `crates/cl-ui/src/screens/invoice/{form.rs,billable_costs.rs,side_panels.rs,email.rs}`, `crates/cl-model/src/forms/invoice.rs`, `crates/cl-settings/src/email_templates.rs`.

**Steps**
1. Form per DESIGN.md (client:job, ship-to choice Q-128, tags, terms/due, P.O. no., lines with optional service date Q-198, multi-line descriptions Q-200, tax, message, totals). — `feat(invoice): form`
2. Price levels and discount/subtotal items applied automatically (Q-58); no line discounts (Q-199). — `feat(invoice): pricing`
3. Add billable costs with client markup; source captions. — `feat(invoice): billable costs`
4. Email via SMTP when online (recipients from contact roles Q-129, invoice PDF only Q-164, editable templates Q-163); otherwise Outbox or save PDF; sent tracking (Q-165); no pay links (Q-68) and no dunning (Q-162). — `feat(invoice): email and sent tracking`

**Tests:** mockup example totals (subtotal 3,080.15; tax 154.72 at the sample 8.100% rate on 1,910.15 taxable; total 3,234.87); markup on billable costs; billable costs consumed; price level and discount math exact; email template field substitution; unsent filter.

**Acceptance**
- [ ] The mockup's invoice totals 3,234.87.
- [ ] Invoiced billable costs no longer appear as unbilled.
- [ ] Screen matches its mockup per docs/design/README.md (`ui-check --compare` reviewed by ui-verifier); any deviation is listed in the PR and QUESTIONS.md.

---

## P21 — Receive Payment

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | L | P20 | rust-implementer (opus) | reviewer (opus), ui-verifier |

**Agents:** architect (opus) required. **Decisions:** Q-80–Q-82, Q-202. **Engine:** payment application, under/overpayment, write-offs, early-payment discounts, customer deposits liability, bounced-check reversal.

**Mockup:** `docs/design/mockups/ReceivePayment.html` and `docs/design/screenshots/ReceivePayment.png`.

**Crates and files:** `crates/cl-ui/src/screens/receive_payment.rs`, `crates/cl-model/src/forms/payment.rs`, `crates/cl-ui/src/screens/bounced_check.rs`.

**Steps**
1. Form per DESIGN.md; auto-apply oldest first; un-apply all; default deposit to Undeposited Funds (Q-202). — `feat(payment): form`
2. Underpayment (leave open / write off) and overpayment (leave credit / refund). — `feat(payment): under and over`
3. Early-payment discount suggestion within terms window (Q-80). — `feat(payment): early discount`
4. Customer deposits held in a Customer Deposits liability until applied (Q-81). — `feat(payment): customer deposits`
5. Bounced check guided action (Q-82): reopen invoices, reverse deposit, NSF bank fee, optional customer fee invoice. — `feat(payment): bounced check`
6. Imported-deposit match banner.

**Tests:** mockup example (4,000.00 applied to invoice 1091, 385.00 left open as an underpayment, client balance 5,645.00 → 1,645.00); write-off; deposit liability lifecycle; bounced check restores balances exactly; idempotent retries.

**Acceptance**
- [ ] The mockup's example balances exactly.
- [ ] Write-off closes the invoice to the chosen account.
- [ ] Customer deposit liability clears on application.
- [ ] Bounced check restores invoice and bank balances exactly.
- [ ] Screen matches its mockup per docs/design/README.md (`ui-check --compare` reviewed by ui-verifier); any deviation is listed in the PR and QUESTIONS.md.

---

## P22 — Credit memos, refunds, statements, finance charges

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| M | M | P20, P21 | rust-implementer (sonnet) | reviewer (opus), ui-verifier |

**Decisions:** Q-88, Q-174–Q-177. **Engine:** credit memos and refunds, statements (open item default), finance-charge assessment with idempotency per period.

**Crates and files:** `crates/cl-ui/src/screens/{credit_memo.rs,refund.rs,statements.rs,finance_charges.rs}`, `crates/cl-model/src/forms/{credit_memo.rs,refund.rs}`.

**Steps**
1. Credit memo (apply or keep as credit); refund by check, cash, or recorded card/ACH/other (Q-88). — `feat(credit): memos and refunds`
2. Statements: open item default (Q-175), balance forward, transaction; batch of clients with open balances filtered by days overdue (Q-176); aging row (Q-177); print/PDF/email. — `feat(statements): statements`
3. Assess Finance Charges with preview (Q-174). — `feat(fincharge): assess`

**Tests:** credit memo reduces A/R exactly; refund clears credit; finance-charge idempotency; statement aging totals equal A/R aging.

**Acceptance**
- [ ] Credit memo and refund flows balance exactly.
- [ ] Finance-charge assessment is idempotent per period.

### Checkpoint PR-3 — Sales lifecycle (fable)
Invoice → payment (discounts, deposits, write-offs, bounced checks) → deposit → credit memos and refunds → statements and finance charges. Fix Blocking findings on `fix/PR-3`.

---

## P23 — Sales tax

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | M | P20 | rust-implementer (opus) | reviewer (opus), ui-verifier |

**Decisions:** Q-17, Q-89, Q-146, Q-147. **Engine:** tax items, groups, codes, agencies, per-invoice rounding, liability by basis, payments with adjustments, exemption certificates.

**Crates and files:** `crates/cl-ui/src/screens/sales_tax/{items.rs,liability.rs,pay.rs,exemptions.rs}`.

**Steps**
1. Tax items, groups, agencies, codes, client defaults; manual rates (Q-17). — `feat(tax): setup`
2. Per-invoice rounding (Q-147) used by invoices, sales receipts, daily sales summaries, credit memos. — `feat(tax): rounding`
3. Liability report and Pay Sales Tax with adjustments; no reminders (Q-146). — `feat(tax): liability and payment`
4. Exemption certificates with warnings and expiring report (Q-89). — `feat(tax): exemptions`

**Tests:** liability = collected − paid; rounding identical across invoice, report, payment; expired certificate warning.

**Acceptance**
- [ ] Liability totals reconcile on fixtures.
- [ ] Rounding consistent across documents and reports.

---

## P24 — Inventory (average cost)

| Tier | Size | Depends on | Implementer | Reviews |
| --- | --- | --- | --- | --- |
| H | M | P17, P20, P23 | rust-implementer (opus) | reviewer (opus), ui-verifier |

**Decisions:** Q-18, Q-148, Q-149. **Engine:** inventory parts with average cost, adjustments, negative stock behavior, reorder points.

**Crates and files:** `crates/cl-ui/src/screens/inventory/{items.rs,adjust.rs,count.rs,reorder.rs}`.

**Steps**
1. Inventory part items (cost, price, on hand, reorder point); use on bills, checks, invoices, sales receipts. — `feat(inventory): items`
2. Quantity and value adjustments; physical count worksheet. — `feat(inventory): adjustments`
3. Negative stock warn-and-allow with cost correction on receipt (Q-148). — `feat(inventory): negative stock`
4. Reorder alerts and report (Q-149). — `feat(inventory): reorder`

**Tests:** average-cost reference sequence; negative stock then receipt; valuation equals GL inventory balance.

**Acceptance**
- [ ] Average cost matches reference calculations, including after negative stock.
- [ ] Inventory valuation equals the GL inventory account.

### Checkpoint PR-4 — Sales tax and inventory (fable)
Rounding consistency end to end; liability vs. GL; average cost through receipts, sales, adjustments, negative stock, voids, and deletes. Fix Blocking findings on `fix/PR-4`.
