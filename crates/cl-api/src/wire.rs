//! Shared wire types: engine identity, capabilities, health, pages and
//! report amounts.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use cl_core::Timestamp;
use cl_core::ids::TenantId;
use rust_decimal::{Decimal, RoundingStrategy};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use serde_json::{Map, Value};

use crate::errors::{ApiError, LocalValidation};

/// Capability strings the engine advertises in `/desktop/info.capabilities`
/// (introduced by U-21; a missing array means none).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Capabilities(BTreeSet<String>);

impl Capabilities {
    /// U-17 record versions and `If-Match`.
    pub const RECORD_VERSION: &'static str = "record-version-v1";
    /// U-18 request idempotency.
    pub const IDEMPOTENCY: &'static str = "idempotency-v1";
    /// U-19 strict money validation.
    pub const MONEY_STRICT: &'static str = "money-strict-v1";
    /// U-21 stable error codes.
    pub const ERROR_CODES: &'static str = "error-codes-v1";
    /// E5a `GET /api/v1/tenants`.
    pub const TENANTS_LIST: &'static str = "tenants-list-v1";
    /// U-15 closing-date password.
    pub const CLOSING_PASSWORD: &'static str = "closing-password-v1";
    /// E5a tenant archives (export project, import, rename).
    pub const TENANT_ARCHIVE: &'static str = "tenant-archive-v1";
    /// E5b safe tenant delete.
    pub const TENANT_DELETE: &'static str = "tenant-delete-v1";
    /// U-23 desktop recovery key.
    pub const RECOVERY_KEY: &'static str = "recovery-key-v1";
    /// E6 per-session secrets (AI keys, SMTP password; U-09, U-13).
    pub const SESSION_SECRETS: &'static str = "session-secrets-v1";
    /// E6 desktop AI: provider allowlist, credential resolution, usage
    /// stamps and the integer usage summary (U-09).
    pub const AI_DESKTOP: &'static str = "ai-desktop-v1";
    /// E6 SMTP password from the session secret (U-13).
    pub const SMTP_SESSION: &'static str = "smtp-session-v1";
    /// U-29 monthly token cap enforced by the engine (`monthlyTokenLimit`).
    pub const AI_TOKEN_CAP: &'static str = "ai-token-cap-v1";
    /// U-31 account merge that moves the journal lines (`/accounts/merge`).
    pub const ACCOUNT_MERGE: &'static str = "account-merge-v1";
    /// U-32 checked sub-accounts (same type, no cycles) and an engine
    /// subtree balance.
    pub const ACCOUNT_TREE: &'static str = "account-tree-v1";
    /// U-33 per-account sensitive flag stored and enforced by the engine.
    pub const ACCOUNT_SENSITIVE: &'static str = "account-sensitive-v1";
    /// U-34 masked bank/card number with an audited Admin reveal.
    pub const ACCOUNT_MASKED_NUMBER: &'static str = "account-masked-number-v1";
    /// U-35 projects under a client (status, type, start and end dates).
    pub const CONTACT_PROJECTS: &'static str = "contact-projects-v1";
    /// U-36 several ship-to addresses (one default) and contact people.
    pub const CONTACT_ADDRESSES: &'static str = "contact-addresses-v1";
    /// U-37 dated notes and to-dos on contacts and projects.
    pub const CONTACT_NOTES: &'static str = "contact-notes-v1";
    /// U-38 the engine's open balance and oldest open date on each contact
    /// read (the name engine PR #197 advertises).
    pub const CONTACT_OPEN_BALANCE: &'static str = "contact-balance-v1";
    /// U-44 "W-9 on file" recorded by the owner (a paper W-9).
    pub const VENDOR_W9_ON_FILE: &'static str = "vendor-w9-on-file-v1";
    /// U-39 item types (non-inventory, other charge, discount, subtotal),
    /// the purchase side of two-sided items and a unit-of-measure label.
    /// Widened by P24 (U-39b, plan §2.4): the `inventory_part` kind, asset
    /// and COGS accounts, preferred vendor, reorder fields, the read-only
    /// `quantityOnHand`, `averageCost`, `assetValue`, and item PUT with
    /// `expectedUpdatedAt`. One of the eight P24 capabilities.
    pub const ITEM_TYPES: &'static str = "item-types-v1";
    /// U-40 price levels.
    pub const PRICE_LEVELS: &'static str = "price-levels-v1";
    /// U-41 tag merge that moves line tags and defaults, audited group
    /// changes, one tag per group on every path, a working group reorder.
    pub const TAG_MERGE: &'static str = "tag-merge-v1";
    /// U-42 tag groups required for chosen account types.
    pub const TAG_REQUIRED_GROUPS: &'static str = "tag-required-groups-v1";
    /// U-75 tag numbers (Q-270): `tagNumber` on tags, unique per company
    /// (409 `TAG_NUMBER_EXISTS`), lists sorted number then name, `search`
    /// matches the number, carried by tenant archives.
    pub const TAG_NUMBER: &'static str = "tag-number-v1";
    /// U-43 editable payment terms (with discount terms) and payment
    /// methods.
    pub const TERMS_LISTS: &'static str = "terms-lists-v1";
    /// U-45 several tags on one transaction line (one per group).
    pub const LINE_TAGS: &'static str = "line-tags-v1";
    /// U-46 the item CSV import resolves the income account by name.
    pub const ITEMS_IMPORT: &'static str = "items-import-v1";
    /// P12 R1–R4, R6: exact register money (`?exact=1`), filter-independent
    /// running balances, the cleared and tag filters, the summary's cleared
    /// and ending balances, and audit before-images with lines.
    pub const REGISTER_EXACT: &'static str = "register-exact-v1";
    /// P12 R5: toggle a register line's cleared mark (assumed name).
    pub const REGISTER_CLEARED: &'static str = "register-cleared-v1";
    /// P12 R7: soft delete of a transaction (`DELETE /transactions/{id}`,
    /// engine PR #153).
    pub const TXN_DELETE: &'static str = "txn-delete-v1";

    // ---- P13 WP2: payee panel, attachments, suggestions -------------------
    /// P13 U-51: rule suggestions on a form (`POST
    /// /practice/conditional-rules/evaluate`, engine PR in progress).
    pub const RULES_EVALUATE: &'static str = "rules-evaluate-v1";

    // ---- P13 WP1: write check ---------------------------------------------
    /// P13 U-48: edit a saved check (`PUT /checks/{id}`: number, name on
    /// the check, address, Print later) and the exact line sum on
    /// `POST`/`PUT /checks`.
    pub const CHECK_EDIT: &'static str = "check-edit-v1";
    /// P13 U-49: a customer (`contactId`) and a billable mark (`isBillable`)
    /// on check and expense lines; `isBillable`/`billedStatus` on read.
    pub const LINE_BILLABLE: &'static str = "line-billable-v1";

    // ---- P14: deposits, daily sales, owner helpers (engine PR #159) -------
    /// P14 U-52: deposit lines with `sourceTransactionId` group pending
    /// payments out of Payments Clearing (409 `ALREADY_DEPOSITED`), and
    /// `GET /payments/pending-deposits?exact=1` reports string amounts.
    pub const DEPOSIT_GROUPING: &'static str = "deposit-grouping-v1";
    /// P14 U-53: deposit `cashBack` and negative (fee) line amounts.
    pub const DEPOSIT_CASHBACK: &'static str = "deposit-cashback-v1";
    /// P14 U-54: a deposit line on A/P with `sourceTransactionId` pointing
    /// at a vendor credit (a vendor refund).
    pub const VENDOR_REFUND: &'static str = "vendor-refund-v1";
    /// P14 U-55: `GET /banking/feed/{id}/deposit-groupings`.
    pub const FEED_DEPOSIT_GROUPINGS: &'static str = "feed-deposit-groupings-v1";
    /// P14 U-56: `GET /company/equity-roles` and engine-seeded owner,
    /// partner and shareholder equity accounts.
    pub const EQUITY_ROLES: &'static str = "equity-roles-v1";
    /// P14 U-57: `closingPassword` on daily-sales post and void.
    pub const DAILY_SALES_LOCK: &'static str = "daily-sales-lock-v1";

    // ---- P15: journal entries, templates, catch-up (client-declared) ------
    /// P15 part A: a name (`contactId`) on journal entry lines; an A/R line
    /// needs a customer and an A/P line a vendor
    /// (400 `JE_AR_AP_NAME_REQUIRED`), any number of such lines per entry
    /// since engine 33a973fa (older engines refused a second one with 400
    /// `JE_AR_AP_ONE_PER_ENTRY`, Q-206).
    pub const JE_NAMES: &'static str = "je-names-v1";
    /// P15 part A: `txnNumber` on journal entries and
    /// `GET /journal-entries/next-number`.
    pub const JE_NUMBER: &'static str = "je-number-v1";
    /// P15 part A: `isAdjusting` on journal entries and the report filter
    /// `adjusting=include|exclude|only`.
    pub const JE_ADJUSTING: &'static str = "je-adjusting-v1";
    /// P15 part A: `reverseOn` (after the date) creates a linked reversing
    /// entry; the read-back gives the link.
    pub const JE_REVERSING: &'static str = "je-reversing-v1";
    /// P15 part A: every posting's debits must equal its credits exactly at
    /// 4 places (each line rounded half away from zero, no 0.0001
    /// tolerance), else 400 "Transaction does not balance".
    pub const BALANCE_EXACT: &'static str = "balance-exact-v1";
    /// P15 part A (U-64): recurring copies keep line names, billable marks,
    /// a journal entry's basis and adjusting mark; `PUT /recurring/{id}`
    /// takes `expectedUpdatedAt` (409 `STALE_SCHEDULE`).
    pub const RECURRING_GUARD: &'static str = "recurring-guard-v1";
    /// P15 U-62: `/templates` (stored drafts, groups, `from-transaction`,
    /// `draft`, `enter`, `skip`, `resolve`).
    pub const TEMPLATES: &'static str = "templates-v1";
    /// P15 U-63: `POST /recurring/run-due` and `GET /recurring/attention`.
    pub const RECURRING_RUN_DUE: &'static str = "recurring-run-due-v1";
    /// U-82: `POST /recurring/run-due` takes `templateIds` (only these) and
    /// `excludeTemplateIds` (all but these); a filtered-out template is not
    /// touched and stays due (Q-PR1d).
    pub const RUN_DUE_FILTER: &'static str = "run-due-filter-v1";

    // ---- P15A: Bulk Entry (`/batch/*`; client-declared capabilities) ------
    /// P15A: `POST /batch/save` posts every row in one database
    /// transaction; a failure names `{group, rowNumber, field, message}`
    /// and saves nothing. The engine stores `source: 'batch'` and a
    /// `batchId`.
    pub const BATCH_ATOMIC: &'static str = "batch-atomic-v1";
    /// P15A: `txn_type: check` rows with `checkNumber` and `printLater`.
    pub const BATCH_CHECK: &'static str = "batch-check-v1";
    /// P15A: one call carries `groups[{txn_type, context_account_id,
    /// rows}]` (several types and accounts).
    pub const BATCH_MIXED: &'static str = "batch-mixed-v1";
    /// P15A: string amounts read as exact decimals; `parse-csv` returns
    /// string amounts and handles a BOM and quoted newlines.
    pub const BATCH_EXACT: &'static str = "batch-exact-v1";
    /// P15A: `Idempotency-Key` on `/batch/save`; a replay returns the first
    /// result.
    pub const BATCH_IDEMPOTENT: &'static str = "batch-idempotent-v1";
    /// P15A: a row's `closingPassword` lets it post on or before the
    /// closing date (audited), with the P12 closed-period errors.
    pub const BATCH_CLOSING: &'static str = "batch-closing-v1";

    // ---- P16 WP6a: Home (engine PR #168) -----------------------------------
    /// P16 U-76: `/dashboard/*` money as 4-place decimal strings with
    /// `exact=1`, `asOf` as "today", one overdue-invoice rule, cash-position
    /// account numbers, payables `dueWithinDays`.
    pub const DASHBOARD_EXACT: &'static str = "dashboard-exact-v1";

    // ---- Checkpoint PR-1 and U-47 (engine PRs #169, #170) ------------------
    /// U-80/U-81: `PUT /transactions/:id` refuses a deposit edit that drops
    /// its links (409 `DEPOSIT_LINKS_DROPPED`) and a printed check's face
    /// change (409 `CHECK_PRINTED`); the check print routes honour the
    /// closing date.
    pub const LEDGER_GUARDS: &'static str = "ledger-guards-v1";
    /// U-47: `POST /contacts/merge` and `/tags/merge` honour the closing
    /// date and take `closingPassword` (423 / 403 / 429); the tag merge
    /// also moves line tags and default-tag references.
    pub const MERGE_CLOSING_DATE: &'static str = "merge-closing-date-v1";

    // ---- P17: Enter Bill, vendor credits (engine PR-A U-83..U-87, PR-B U-89;
    // in the pinned engine f3ce33fd) ------------------------------------------
    /// U-83: bill and vendor credit create, edit, void (and bill-capture
    /// enter) honour the closing date and take `closingPassword` (423
    /// `TB_PERIOD_LOCKED`, 403 `CLOSING_PASSWORD_INVALID`), audited.
    pub const AP_CLOSING: &'static str = "ap-closing-v1";
    /// U-84: `PUT /vendor-credits/{id}` (409 `CREDIT_APPLIED`), bill and
    /// credit delete through `DELETE /transactions/{id}` when unpaid and
    /// unapplied (409 `BILL_HAS_PAYMENTS`), `expectedUpdatedAt` on edit, void
    /// and delete (409 `STALE`, the U-17 shape; plan review B3), vendor
    /// credit rows carry `vendorInvoiceNumber` (S6), vendor summary counts
    /// posted bills only.
    pub const AP_EDIT: &'static str = "ap-edit-v1";
    /// U-85: bill and credit lines with `itemId`, `quantity`, `unitPrice`
    /// (amount = qty × price rounded half away from zero, 400
    /// `LINE_AMOUNT_MISMATCH`), `contactId`, `isBillable`, `tagId`, and
    /// `draftAttachmentId` on create.
    pub const BILL_LINES: &'static str = "bill-lines-v1";
    /// U-86: the shared payment terms (with discount terms), `GET
    /// /bills/terms-dates`, `GET /payment-terms`, calendar-exact due dates.
    pub const BILL_TERMS: &'static str = "bill-terms-v1";
    /// U-87: `billReceived` on bills (false = left out of `/bills/payable`).
    pub const BILL_RECEIVED: &'static str = "bill-received-v1";
    /// U-89: no OCR on upload in desktop mode; `fieldConfidence` and line
    /// `confidence` strings on `POST /bills/extract-from-attachment/{id}`,
    /// amounts as decimal strings or null.
    pub const BILL_OCR_V2: &'static str = "bill-ocr-v2";
    /// U-83 to U-87: the engine also replays a repeated `Idempotency-Key` on
    /// the A/P document mutations (bill edit and void, vendor credit create,
    /// edit and void, bill-payment void), on top of `idempotency-v1`.
    pub const IDEMPOTENCY_AP: &'static str = "idempotency-ap-v1";
    /// U-90 (engine PR #175, pin 16aff64b): attachment routes work in the
    /// request's company (`X-Company-Id`): uploads are stamped with it, and
    /// reads, downloads, links and deletes see only that company's files and
    /// tenant-wide ones.
    pub const ATTACHMENTS_COMPANY: &'static str = "attachments-company-v1";

    // ---- A01: intake folders (engine U-93; client-declared until the pin has it)
    /// U-93: `GET`/`PUT /company/intake-settings` (the company's intake
    /// folders, a full replace guarded by `expectedUpdatedAt`, 409 `STALE`),
    /// paths never in audit rows, and `omitIntakeSettings` on the tenant
    /// archive export.
    pub const INTAKE_SETTINGS: &'static str = "intake-settings-v1";

    // ---- A03: document drafts (engine U-92; client-declared until the pin has it)
    /// U-92, U-92b: `/document-drafts` (claim an uploaded draft file, list,
    /// get, duplicates, store a reading, discard, convert into a bill, card
    /// charge, cash expense or check), `/vendor-layout-templates`,
    /// `cardLast4` on accounts, 409 `ATTACHMENT_IN_USE` on deleting a live
    /// draft's file, `omitDocumentDrafts` / `resetDocumentDrafts` on tenant
    /// archives.
    pub const DOCUMENT_DRAFTS: &'static str = "document-drafts-v1";
    /// U-92a: the engine replays a repeated `Idempotency-Key` on exactly
    /// `POST /document-drafts` and `POST /document-drafts/{id}/convert`.
    pub const IDEMPOTENCY_DOCS: &'static str = "idempotency-docs-v1";
    /// U-92d: `POST /document-reading/assist` (a second reading of the page
    /// text, and page images, by the company's AI provider; nothing stored)
    /// and the company task switches `document_text_cloud` and
    /// `document_images_cloud` (off for every company).
    pub const DOCUMENT_ASSIST: &'static str = "document-assist-v1";

    // ---- A04: attach a document draft (engine U-95)
    /// U-95: `POST /document-drafts/{id}/attach` (file a live draft's
    /// document on a posted transaction; keyed with `idempotency-docs-v1`'s
    /// contract under this name), the live-draft file guard on every plain
    /// draft-key relink with 409 `ATTACHMENT_IN_USE` on
    /// `POST /attachments/{id}/link`, and `GET /transactions?amount=` (an
    /// exact total).
    pub const DOCUMENT_DRAFTS_ATTACH: &'static str = "document-drafts-attach-v1";

    // ---- A06: vendor statements (engine U-94)
    /// U-94: the label-only `statement` document draft kind (it never
    /// converts) and the intake folder kind `statements`.
    pub const DOCUMENT_DRAFTS_STATEMENT: &'static str = "document-drafts-statement-v1";

    // ---- P18: Pay Bills (engine U-101 to U-103; client-declared until the
    // pin has them)
    /// U-101: early-payment discounts on `POST /bill-payments`
    /// (`bills[].discountAmount`, `discountAccountId`, 400
    /// `DISCOUNT_ACCOUNT_REQUIRED`), the company setting
    /// `billDiscountAccountId`, and `/bills/payable?paymentDate=` rows with
    /// the engine's `discountAvailable` and `updatedAt`.
    pub const BILL_DISCOUNTS: &'static str = "bill-discounts-v1";
    /// U-102: `Idempotency-Key` on `POST /bill-payments`.
    pub const IDEMPOTENCY_PAY_BILLS: &'static str = "idempotency-pay-bills-v1";
    /// The engine replays an `Idempotency-Key` on `POST /checks/print`
    /// (U-116, PR-2 B2): a resend after a lost reply returns the first batch.
    pub const IDEMPOTENCY_CHECK_PRINT: &'static str = "idempotency-check-print-v1";
    /// U-103: `checkNumber` (the first number) on a check run that is not
    /// printed later; one consecutive number per vendor, 409
    /// `CHECK_NUMBER_IN_USE`.
    pub const BILL_PAYMENT_CHECK_NUMBER: &'static str = "bill-payment-check-number-v1";
    /// U-106 to U-109: check requeue and reprint with spoiled numbers and
    /// audit rows, the print queue's mailing rows, `bill`/`credit` on a bill
    /// payment's applications and the check style per bank account.
    pub const CHECK_REPRINT_AUDIT: &'static str = "check-reprint-audit-v1";
    /// U-110: `GET /company/logo` streams the uploaded logo.
    pub const COMPANY_LOGO: &'static str = "company-logo-v1";
    /// U-111: document numbering per company and per-type footers.
    pub const DOCUMENT_NUMBERS: &'static str = "document-numbers-v1";
    /// U-113: Print later for invoices, sales receipts and credit memos.
    pub const FORMS_PRINT_QUEUE: &'static str = "forms-print-queue-v1";
    /// U-122: sales forms to the cent; tax once per document, allocated to
    /// the taxable lines by largest remainder.
    pub const SALES_CENTS: &'static str = "sales-cents-v1";
    /// U-124: closing-date password and `expectedUpdatedAt` on invoice
    /// create, edit and void; 409 `INVOICE_HAS_PAYMENTS`.
    pub const INVOICE_EDIT: &'static str = "invoice-edit-v1";
    /// U-124: `Idempotency-Key` on invoice edit, void and sent.
    pub const IDEMPOTENCY_INVOICE: &'static str = "idempotency-invoice-v1";
    /// U-125: invoice terms (invoice, else client, else company).
    pub const INVOICE_TERMS: &'static str = "invoice-terms-v1";
    /// U-123: line item and service date; header P.O. number, customer
    /// message, tags and hide-zero-lines.
    pub const INVOICE_FIELDS: &'static str = "invoice-fields-v1";
    /// U-126: send history and the unsent filter.
    pub const INVOICE_SENT_HISTORY: &'static str = "invoice-sent-history-v1";
    /// U-39a: sales item kinds (service, non-inventory, other charge,
    /// discount, subtotal). Contract per P20 plan §2; verify when E2 lands.
    pub const ITEM_KINDS_SALES: &'static str = "item-kinds-sales-v1";
    /// U-129: `POST /invoices/preview` and engine pricing (price levels,
    /// discounts, subtotals). Contract per P20 plan §2; verify when E2
    /// lands.
    pub const INVOICE_PRICING: &'static str = "invoice-pricing-v1";
    /// An invoice PUT may say its line list is complete
    /// (`pricingLinesComplete: true`): discount and subtotal lines it
    /// leaves out were removed on purpose (no 409 `INVOICE_LINES_MISSING`).
    pub const INVOICE_PRICING_LINES: &'static str = "invoice-pricing-lines-v1";
    /// U-130: invoice print layouts (`/company/invoice-layouts`, invoice
    /// `layoutId`, contact `defaultInvoiceLayoutId`).
    pub const INVOICE_LAYOUTS: &'static str = "invoice-layouts-v1";
    /// U-128: billable costs and markup. Contract per P20 plan §2; verify
    /// when E3 lands.
    pub const BILLABLE_COSTS: &'static str = "billable-costs-v1";
    /// U-127: email an invoice through the company SMTP and the Outbox.
    /// Contract per P20 plan §2; verify when E4 lands.
    pub const INVOICE_EMAIL: &'static str = "invoice-email-v1";
    /// U-132: payment preview, receive, edit, void, list (P21 plan §2.2).
    /// Contract per the P21 plan; verify at the pin bump (D9).
    pub const RECEIVE_PAYMENT: &'static str = "receive-payment-v1";
    /// U-132: `Idempotency-Key` on payment receive, edit, void, unapply,
    /// apply and bounce.
    pub const IDEMPOTENCY_RECEIVE_PAYMENT: &'static str = "idempotency-receive-payment-v1";
    /// U-132: a bounced check as one dated "Bounced payment" entry.
    pub const PAYMENT_BOUNCE: &'static str = "payment-bounce-v1";
    /// U-133: write-offs, unapplied credits, applying a credit later and
    /// refunds linked to a credit.
    pub const PAYMENT_SETTLE: &'static str = "payment-settle-v1";
    /// U-134: early-payment discounts on customer payments.
    pub const PAYMENT_DISCOUNTS: &'static str = "payment-discounts-v1";
    /// U-135: customer deposits held in a liability until applied.
    pub const CUSTOMER_DEPOSITS: &'static str = "customer-deposits-v1";
    /// U-136: a customer payment matched to a bank-file deposit as it is
    /// received (`bankFeedItemId`).
    pub const FEED_CUSTOMER_PAYMENTS: &'static str = "feed-customer-payments-v1";
    /// U-137: a credit memo is an A/R credit source (apply, unapply,
    /// `GET /payments/credits`; P22 plan §2.2). Contract per the P22 plan;
    /// verify at the pin bump (D8).
    pub const CREDIT_MEMO_APPLY: &'static str = "credit-memo-apply-v1";
    /// U-137: `Idempotency-Key` on credit memo create, edit, apply and
    /// unapply, and (a desktop-side assumption, P22 D1) sales receipt
    /// create and edit.
    pub const IDEMPOTENCY_CREDIT_MEMO: &'static str = "idempotency-credit-memo-v1";
    /// U-142: exact A/R open items as of a date.
    pub const AR_OPEN_ITEMS_ASOF: &'static str = "ar-open-items-asof-v1";
    /// U-142: the A/R aging summary's exact string fields and per-client
    /// rows (Q-P22o).
    pub const AR_AGING_EXACT: &'static str = "ar-aging-exact-v1";
    /// U-138: credit memo lines with items and tax, preview and edit.
    pub const CREDIT_MEMO_EDIT: &'static str = "credit-memo-edit-v1";
    /// U-143: sales receipt lines with items and tax, preview and edit.
    pub const SALES_RECEIPT_LINES: &'static str = "sales-receipt-lines-v1";
    /// U-139: refunds of a credit memo's credit.
    pub const REFUND_CREDIT_MEMO: &'static str = "refund-credit-memo-v1";
    /// U-139: refunds paid by check (`printLater`, `checkNumber`).
    pub const REFUND_CHECKS: &'static str = "refund-checks-v1";
    /// U-112: customer statements (three formats).
    pub const CUSTOMER_STATEMENT: &'static str = "customer-statement-v1";
    /// U-140: statement runs, snapshots, send history and email.
    pub const CUSTOMER_STATEMENT_SEND: &'static str = "customer-statement-send-v1";
    /// U-140: `Idempotency-Key` on statement runs and statement email.
    pub const IDEMPOTENCY_CUSTOMER_STATEMENTS: &'static str = "idempotency-customer-statements-v1";
    /// U-141: finance-charge settings, preview, assess and list.
    pub const FINANCE_CHARGES: &'static str = "finance-charges-v1";
    /// U-141: `Idempotency-Key` on finance-charge assess.
    pub const IDEMPOTENCY_FINANCE_CHARGES: &'static str = "idempotency-finance-charges-v1";
    /// U-144: email and email-later for credit memos and sales receipts
    /// (keyed on those four routes), their send history and the Outbox
    /// kinds `credit_memo` and `sales_receipt`.
    pub const SALES_DOC_EMAIL: &'static str = "sales-doc-email-v1";
    /// U-145, U-146: an invoice keeps its client while a payment, credit or
    /// finance charge is on it (409 `INVOICE_HAS_PAYMENTS`,
    /// `FINANCE_CHARGE_CLIENT_CHANGE`).
    pub const INVOICE_CLIENT_GUARD: &'static str = "invoice-client-guard-v1";
    /// U-146: a deadlock on the receivables routes is 409 `RETRY` (nothing
    /// saved; resend with the same key).
    pub const RECEIVABLES_RETRY: &'static str = "receivables-retry-v1";
    // P23 sales tax (plan §2.4-§2.8; contract per the plan, verified at the
    // WP-4 pin bump).
    /// U-147, U-25: agencies, sales tax items and groups, tax codes, the
    /// client and item defaults, document `salesTaxId` and line codes.
    pub const SALES_TAX_SETUP: &'static str = "sales-tax-setup-v1";
    /// U-148: six-place rates on every route (`numeric(7,6)`).
    pub const SALES_TAX_RATE6: &'static str = "sales-tax-rate6-v1";
    /// U-147: `salesTaxEnabled`, `salesTaxBasis`, `defaultSalesTaxId` and
    /// `salesTaxSsuta` on `/company/settings`.
    pub const COMPANY_SALES_TAX_FLAGS: &'static str = "company-sales-tax-flags-v1";
    /// U-147: `Idempotency-Key` on the sales tax setup writes.
    pub const IDEMPOTENCY_SALES_TAX: &'static str = "idempotency-sales-tax-v1";
    /// U-149: the liability report's string columns by agency and item,
    /// `GET /sales-tax/owed` and the payments report.
    pub const SALES_TAX_LIABILITY: &'static str = "sales-tax-liability-v2";
    /// U-152: cash basis per the SPF rules (Unapplied receipts, write-offs
    /// off cash, bill discounts).
    pub const CASH_BASIS_SPF: &'static str = "cash-basis-spf-v1";
    /// Daily sales tax lines linked to a sales tax (`expectedTax`,
    /// `TAX_DIFFERS`).
    pub const DAILY_SALES_TAX: &'static str = "daily-sales-tax-v1";
    /// U-150: Pay Sales Tax (preview, post, link, get, list).
    pub const SALES_TAX_PAY: &'static str = "sales-tax-pay-v1";
    /// U-150: `Idempotency-Key` on Pay Sales Tax and link.
    pub const IDEMPOTENCY_SALES_TAX_PAY: &'static str = "idempotency-sales-tax-pay-v1";
    /// U-151: exemption certificates and the expiring report.
    pub const TAX_EXEMPTIONS: &'static str = "tax-exemptions-v1";
    /// U-153: SSUTA discount and bad-debt shares (`discountTax`,
    /// `writeOffTax` on the payment preview).
    pub const SALES_TAX_SSUTA: &'static str = "sales-tax-ssuta-v1";
    /// U-154: bad-debt recoveries.
    pub const BAD_DEBT_RECOVERY: &'static str = "bad-debt-recovery-v1";
    /// U-154: `Idempotency-Key` on a recovery.
    pub const IDEMPOTENCY_RECOVERY: &'static str = "idempotency-recovery-v1";
    // P24 inventory (plan §2.4; contract per the plan, client-declared
    // until the pin bump).
    /// U-156: the `inventory_asset` account tag; turning inventory on
    /// adopts or creates "Inventory Asset" and a default COGS account.
    pub const INVENTORY_ACCOUNT: &'static str = "inventory-account-v1";
    /// U-25 (inventory half): `inventoryEnabled` on `/company/settings`.
    pub const COMPANY_INVENTORY_FLAG: &'static str = "company-inventory-flag-v1";
    /// U-157, U-158: average costing on every document, `inventoryLines[]`
    /// outside `lines`, `NEGATIVE_STOCK` warnings; documents with inventory dated on or before the closing date are refused (409 `INVENTORY_PERIOD_CLOSED`, owner 2026-11-06).
    pub const INVENTORY_COSTING: &'static str = "inventory-costing-v1";
    /// U-159: inventory adjustments, the count sheet, posting a count and
    /// the opening quantity on item create.
    pub const INVENTORY_ADJUST: &'static str = "inventory-adjust-v1";
    /// U-160: inventory valuation summary and detail, stock status.
    pub const INVENTORY_REPORTS: &'static str = "inventory-reports-v1";
    /// U-78: `GET /items/reorder-alerts` and the reorder report.
    pub const REORDER_ALERTS: &'static str = "reorder-alerts-v1";
    /// U-161: unit sets (`/unit-sets`), `unitId` on item lines, base
    /// quantities, `unitPrices` on items, the reorder report's purchase
    /// unit (Q-P24i). The engine keys the inventory writes (adjustments,
    /// counts) under `inventory-adjust-v1`; there is no separate
    /// idempotency capability for them.
    pub const UNITS_OF_MEASURE: &'static str = "units-of-measure-v1";
    /// U-06 (P25): bank file import batches (`POST /banking/feed/import-batch`
    /// preview and commit, the batch list and undo, the connection lookup).
    /// The commit and the undo are keyed under this capability; a preview
    /// takes no key.
    pub const BANK_IMPORT_BATCH: &'static str = "bank-import-batch-v1";
    /// Bank import preparation progress (U-06 follow-up, engine PR #234):
    /// `processed`/`total` on the batch list, `cancelled` post-processing,
    /// undo while preparing, and `existingAmount`/`existingDate` on changed
    /// rows.
    pub const BANK_IMPORT_PROGRESS: &'static str = "bank-import-progress-v1";
    /// U-165 (P25, used from P26): the single-item bank feed routes take the
    /// closing-date password.
    pub const FEED_CLOSING_DATE: &'static str = "feed-closing-date-v1";
    /// U-164 (P26 E1): categorize and approve post the feed amount exactly;
    /// categorize and assign take `splits` (2-50 lines adding up to the
    /// amount; 400 `SPLITS_DONT_BALANCE {difference}`, `AMOUNT_NOT_CENTS`).
    pub const FEED_EXACT: &'static str = "feed-exact-v1";
    /// U-166, U-172 (P26 E2): matches at the level of a bank line
    /// (`journalLineId`), payroll overlaps per line, the "Entry voided" flag
    /// and Review again.
    pub const FEED_MATCH_V2: &'static str = "feed-match-v2";
    /// U-173 (P26 E2): 409 `FEED_ITEM_NOT_PENDING`; categorize, approve and
    /// match replay an `Idempotency-Key`; duplicate and review-again need one.
    pub const FEED_IDEMPOTENCY: &'static str = "feed-idempotency-v1";
    /// U-174, U-168 (P26 E2): review buckets, `/banking/feed/counts`,
    /// possible-duplicate decisions and the rule that staged a suggestion.
    pub const FEED_REVIEW: &'static str = "feed-review-v1";
    /// U-171 (P26 E4): transfer candidates, one transfer for a bank line
    /// and its other side, and deposits of waiting payments net of fees.
    pub const FEED_MATCH_POST: &'static str = "feed-match-post-v1";
    /// U-77 (P26 E4): books balance next to the latest bank-file balance
    /// per bank and card account.
    pub const ACCOUNT_BALANCES: &'static str = "account-balances-v1";
    /// U-170 (P26 E3): `POST /banking/feed/accept`, one accept route for a
    /// selection or every row of one auto-accept rule.
    pub const FEED_ACCEPT: &'static str = "feed-accept-v1";
    /// U-169 (P26 E3): `POST /banking/feed/undo`, rows or an accept group.
    pub const FEED_UNDO: &'static str = "feed-undo-v1";
    /// U-175 (P26 QA-6): a bank file goes into any bank register account
    /// (`bank`, `checking`, `savings`, `cash`, `petty_cash`) or a card.
    pub const BANK_IMPORT_ACCOUNT_TYPES: &'static str = "bank-import-account-types-v1";
    /// U-177 (P26 QA-16): `transferWindowDays` (1-10) on `GET /banking/feed`
    /// and `/banking/feed/counts`.
    pub const FEED_TRANSFER_WINDOW: &'static str = "feed-transfer-window-v1";
    /// U-178 (P26 QA-9): list rows carry `acceptKind`.
    pub const FEED_ACCEPT_KIND: &'static str = "feed-accept-kind-v1";

    // ---- A05: document drafts in the Bank Feed (engine U-180, U-181)
    /// U-181: an accept item may carry `documentDraft {id,
    /// expectedUpdatedAt}`; the draft's file is attached to the posted or
    /// matched entry and the draft closes in the same transaction. Per-item
    /// failures `DOCUMENT_DRAFT_CLOSED {transactionId}`, `STALE
    /// {updatedAt}`, `DOCUMENT_FILE_MISSING`, `INVALID_KIND`.
    pub const FEED_ACCEPT_DRAFT: &'static str = "feed-accept-draft-v1";
    /// U-180: deleting an entry (feed Undo, `DELETE /transactions/:id`)
    /// returns the drafts converted or attached to it to the review queue
    /// with their files; the answers list them in `reopenedDrafts`.
    pub const DOCUMENT_DRAFTS_REOPEN: &'static str = "document-drafts-reopen-v1";
    /// PR-A1 B1 (Q-PRA1b): a feed accept item carrying a document draft
    /// runs the convert's duplicate check on the posted row and fails the
    /// row `DOCUMENT_POSSIBLE_DUPLICATE {matches}` unless
    /// `documentDraft.acknowledgeDuplicateOf` lists every match.
    pub const FEED_ACCEPT_DRAFT_DUPLICATES: &'static str = "feed-accept-draft-duplicates-v1";
    /// PR-A1 QA-1 (Q-PRA1d): a document with no vendor is checked too (on
    /// convert, on a feed accept carrying it and in `/duplicates`): posted
    /// bills, expenses, card charges and checks with the same reference,
    /// or with the exact total dated within 7 days.
    pub const DOCUMENT_DUPLICATES_NO_VENDOR: &'static str = "document-duplicates-no-vendor-v1";

    // ---- P27: Reconcile (engine U-188 to U-191)
    /// U-188: reconciliation amounts as exact strings, the cleared totals,
    /// a strict zero finish, 409s for status, one in progress per account
    /// and batched ticks with `?view=summary`.
    pub const RECONCILE_EXACT: &'static str = "reconcile-exact-v1";
    /// U-189: service charge and interest at Begin (posted at Finish),
    /// bank-file pre-check, later lines, drift and `postingLocked`.
    pub const RECONCILE_BEGIN: &'static str = "reconcile-begin-v1";
    /// U-190: undo modes (keep checks, discard), Admin only, closing date.
    pub const RECONCILE_UNDO: &'static str = "reconcile-undo-v2";
    /// U-191: `GET /reports/reconciliation-discrepancy`.
    pub const RECONCILE_DISCREPANCY: &'static str = "reconcile-discrepancy-v1";
    /// U-192: editing an entry keeps its in-progress reconciliation tick
    /// and its same-amount bank-file link.
    pub const LINE_IDENTITY: &'static str = "line-identity-v1";
    /// U-193: Finish drops voided rows, edits wait for Finish, statement
    /// dates in order (409 `RECONCILE_STATEMENT_DATE_ORDER`), a voided
    /// posted charge can be entered again.
    pub const RECONCILE_HARDENING: &'static str = "reconcile-hardening-v1";
    /// U-194–U-196: every bank detail type gets the bank entry types, a
    /// reconciled entry's edit or void is 409 `RECONCILED_TRANSACTION`, the
    /// register summary names the latest reconciliation.
    pub const RECONCILE_QA: &'static str = "reconcile-qa-v1";
    /// U-198: a reconciled entry keeps only its reconciled lines; its other
    /// lines (also a balance-sheet one such as A/R, owner's equity or a
    /// loan) can be recategorized.
    pub const RECONCILED_CATEGORY_EDIT: &'static str = "reconciled-category-edit-v1";

    // ---- PR-5: the bank lifecycle (engine U-199, U-200)
    /// U-199 (Q-PR5a): an amount or bank-account edit of an entry matched
    /// to a bank line flags the row `entryChanged`/`entryChangedAt` (list
    /// filter `entryChanged`, `/feed/counts` `entryChanged`); Review again
    /// takes such rows; Undo's reconciled check reads the current lines.
    pub const FEED_ENTRY_CHANGED: &'static str = "feed-entry-changed-v1";
    /// U-200: `transferWindowDays` on `POST /feed/accept`; 409
    /// `FEED_ITEM_POSSIBLE_DUPLICATE` on the single-row routes; 409
    /// `FEED_ITEM_HAS_MATCH` on a transfer (again with `force` and a new
    /// key); import preview rows' `existingReplaceable` and commit rows'
    /// `replaceRefused`.
    pub const FEED_GUARDS: &'static str = "feed-guards-v1";

    // ---- P28: reports (engine PR-A to PR-G; Q-220 until they merge)
    /// U-14 (PR-A): `exact=1` adds `*Exact` decimal strings on every
    /// catalog report route.
    pub const REPORTS_EXACT: &'static str = "reports-exact-v1";
    /// U-118 (PR-F): memorized reports, `kind:'standard'`.
    pub const REPORT_DEFS: &'static str = "report-defs-v1";
    /// U-203 (PR-C): company-scoped, versioned, validated budgets and the
    /// exact Budget vs. Actual.
    pub const BUDGETS_EXACT: &'static str = "budgets-exact-v1";
    /// U-207 (PR-E): `POST /reports/email`.
    pub const REPORTS_EMAIL: &'static str = "reports-email-v1";
    /// U-206 (PR-G): `POST /reports/ask`.
    pub const REPORTS_ASK: &'static str = "reports-ask-v1";

    // ---- A07: Report Builder (engine PR-A to PR-D; Q-220 until they merge)
    /// PR-A: `report_definitions.kind = 'builder'` (migration 0276) and the
    /// 256 KB builder cap.
    pub const REPORT_DEFS_BUILDER: &'static str = "report-defs-builder-v1";
    /// PR-B: the shared foundations (no new endpoint; output changes per
    /// Q-A07b and Q-A07c).
    pub const REPORT_FOUNDATIONS: &'static str = "report-foundations-v1";
    /// PR-C: `POST /report-builder/run`, `/run/detail`, `GET /fields`.
    pub const REPORT_BUILDER: &'static str = "report-builder-v1";
    /// PR-D: `GET /report-builder/starters`.
    pub const REPORT_BUILDER_STARTERS: &'static str = "report-builder-starters-v1";
    /// PR-E (U-119e): section Total, outer-line and earnings drills, one
    /// column's `range` (as-of columns too), open-item rows with paths.
    pub const REPORT_BUILDER_DRILL: &'static str = "report-builder-drill-v1";
    /// PR-F (U-119f): starters v2 (detail fields on every starter, natural
    /// signs in Transaction Detail, Open Invoices/Unpaid Bills by document
    /// type) and opening-balance-only accounts in running-balance runs.
    pub const REPORT_BUILDER_STARTERS_V2: &'static str = "report-builder-starters-v2";
    /// U-216 (engine PR #50): Revenue by Category and the Assets,
    /// Liabilities and Equity by Account summaries total the period net
    /// (credit memos and refunds reduce revenue; it ties to the P&L).
    pub const REPORTS_REVENUE_NET: &'static str = "reports-revenue-net-v1";
    /// U-217 (engine PR #51): `purpose: 'preview'` on `/report-builder/run`
    /// (the screen run cut at 50 detail rows per line, `meta.preview`).
    pub const REPORT_BUILDER_PREVIEW: &'static str = "report-builder-preview-v1";
    /// U-218 (engine PR #52): formulas, their $ / % change and named
    /// percent-of bases use each section's normal sign; "Show as positive"
    /// changes only the section's own rows (Q-A08ai).
    pub const REPORT_BUILDER_FORMULA_SIGN: &'static str = "report-builder-formula-sign-v1";
    /// U-219 (engine PR #53): the Assets, Liabilities and Equity by Account
    /// summaries total each account's balance at the end date (Q-A09b);
    /// Equity by Account shows posted balances only (no computed earnings).
    /// No desktop screen calls these routes yet.
    pub const REPORTS_BY_ACCOUNT_BALANCE: &'static str = "reports-by-account-balance-v1";

    /// True when the engine advertises `name`.
    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        self.0.contains(name)
    }

    /// Every advertised capability.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

impl<S: Into<String>> FromIterator<S> for Capabilities {
    fn from_iter<I: IntoIterator<Item = S>>(iter: I) -> Self {
        Capabilities(iter.into_iter().map(Into::into).collect())
    }
}

/// `GET /api/v1/desktop/info` (the fields cl-api uses; others are ignored).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopInfo {
    /// Engine version string.
    #[serde(default)]
    pub version: Option<String>,
    /// Engine source commit.
    #[serde(default)]
    pub engine_commit: Option<String>,
    /// Whether a user exists (`null` while the database is unavailable).
    #[serde(default)]
    pub initialized: Option<bool>,
    /// The hidden owner workspace tenant (Q-P02h); `null` on older engines.
    #[serde(default)]
    pub workspace_tenant_id: Option<TenantId>,
    /// Advertised capabilities.
    #[serde(default)]
    pub capabilities: Capabilities,
}

/// `GET /health` (desktop fields; others are ignored).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    /// `ok`, `degraded`, …
    #[serde(default)]
    pub status: Option<String>,
    /// Internet reachability; `None` means unknown, not offline (E4).
    #[serde(default)]
    pub online: Option<bool>,
    /// When reachability was last checked.
    #[serde(default)]
    pub last_online_check: Option<Timestamp>,
}

/// One page of a list (`{data, total}`; tags answer `{tags, total, …}` and
/// are adapted into the same shape).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    /// The rows of this page.
    pub data: Vec<T>,
    /// Rows matching the filter in all pages.
    pub total: u64,
}

impl<T: DeserializeOwned> Page<T> {
    /// Reads a page whose rows sit under `key` (`data` for most resources,
    /// `tags` for tags). `total` must be a JSON integer.
    ///
    /// # Errors
    /// `Decode` when the body is not that shape.
    pub fn from_json(body: &[u8], key: &str) -> Result<Page<T>, ApiError> {
        let mut obj: Map<String, Value> =
            serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: "page" })?;
        let total = obj
            .get("total")
            .and_then(Value::as_u64)
            .ok_or(ApiError::Decode { what: "page total" })?;
        let rows = obj
            .remove(key)
            .ok_or(ApiError::Decode { what: "page rows" })?;
        let data =
            serde_json::from_value(rows).map_err(|_| ApiError::Decode { what: "page rows" })?;
        Ok(Page { data, total })
    }
}

/// Paging of a list request: `limit` 1..=500 (the engine's maximum).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRequest {
    limit: u16,
    offset: u64,
}

impl PageRequest {
    /// The largest page the engine serves for core lists.
    pub const MAX_LIMIT: u16 = 500;

    /// A page of `limit` rows starting at `offset`.
    ///
    /// # Errors
    /// `Local(PageLimit)` when `limit` is 0 or above 500.
    pub fn new(limit: u16, offset: u64) -> Result<PageRequest, ApiError> {
        if limit == 0 || limit > Self::MAX_LIMIT {
            return Err(ApiError::Local(LocalValidation::PageLimit));
        }
        Ok(PageRequest { limit, offset })
    }

    /// The first page of the largest size.
    #[must_use]
    pub fn first_max() -> PageRequest {
        PageRequest {
            limit: Self::MAX_LIMIT,
            offset: 0,
        }
    }

    /// Rows per page.
    #[must_use]
    pub fn limit(&self) -> u16 {
        self.limit
    }

    /// Rows skipped.
    #[must_use]
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// The next page.
    #[must_use]
    pub fn next(&self) -> PageRequest {
        PageRequest {
            limit: self.limit,
            offset: self.offset + u64::from(self.limit),
        }
    }

    /// `limit` and `offset` query pairs (same casing for every resource).
    #[must_use]
    pub fn query(&self) -> Vec<(String, String)> {
        vec![
            ("limit".into(), self.limit.to_string()),
            ("offset".into(), self.offset.to_string()),
        ]
    }
}

/// A number from an engine report, kept as the exact JSON text the engine
/// sent (until U-14 makes report money decimal strings).
///
/// Display only: there is deliberately no `PartialEq`, `Eq`, `Ord`, `Hash`,
/// arithmetic or conversion to `Money`, so a report value can never feed a
/// posting or an equality check. P&L totals are float sums in the engine
/// today, so texts like `0.30000000000000004` arrive; [`ReportAmount::display`]
/// is therefore approximate until U-14.
#[derive(Clone)]
pub struct ReportAmount(Box<RawValue>);

impl ReportAmount {
    /// The number text (a JSON number as sent, or the contents of a JSON
    /// string).
    #[must_use]
    pub fn text(&self) -> &str {
        let raw = self.0.get();
        raw.strip_prefix('"')
            .and_then(|r| r.strip_suffix('"'))
            .unwrap_or(raw)
    }

    /// Rounded to `dp` decimals, half away from zero, never `-0`. `None` when
    /// the text is not a number `Decimal` can hold.
    #[must_use]
    pub fn display(&self, dp: u32) -> Option<String> {
        let text = self.text();
        let parsed = if text.contains(['e', 'E']) {
            Decimal::from_scientific(text)
        } else {
            Decimal::from_str(text)
        };
        let mut d = parsed
            .ok()?
            .round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero);
        if d.is_zero() {
            d.set_sign_positive(true);
        }
        let width = usize::try_from(dp).ok()?;
        Some(format!("{d:.width$}"))
    }
}

/// Prints the amount text itself: owner data. Never log a value with
/// `?value` at info level or above.
impl fmt::Debug for ReportAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ReportAmount({})", self.text())
    }
}

impl<'de> Deserialize<'de> for ReportAmount {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(d)?;
        let text = raw.get();
        let body = text
            .strip_prefix('"')
            .and_then(|r| r.strip_suffix('"'))
            .unwrap_or(text);
        let numeric = !body.is_empty()
            && body
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E'));
        if numeric {
            Ok(ReportAmount(raw))
        } else {
            Err(serde::de::Error::invalid_type(
                serde::de::Unexpected::Other("a non-numeric value"),
                &"a JSON number or decimal string",
            ))
        }
    }
}

/// `deserialize_with` for a money field that must be a decimal string
/// (`dashboard-exact-v1`, P16 D9): a JSON number, `null` or any other type
/// is a decode error, so a float amount can never be read. The text is
/// parsed exactly by [`cl_core::Money::parse_wire`].
///
/// # Errors
/// The deserializer's error for a non-string or a string outside the
/// engine money grammar.
pub fn money_str_strict<'de, D: Deserializer<'de>>(d: D) -> Result<cl_core::Money, D::Error> {
    struct StrOnly;
    impl serde::de::Visitor<'_> for StrOnly {
        type Value = cl_core::Money;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a decimal money string")
        }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<cl_core::Money, E> {
            cl_core::Money::parse_wire(v).map_err(E::custom)
        }
    }
    d.deserialize_str(StrOnly)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_money_reads_strings_only() {
        #[derive(Deserialize)]
        struct Row {
            #[serde(deserialize_with = "money_str_strict")]
            amount: cl_core::Money,
        }
        let read = |json: &str| serde_json::from_str::<Row>(json).map(|r| r.amount.to_wire());
        assert_eq!(
            read(r#"{"amount":"12345.6700"}"#).ok().as_deref(),
            Some("12345.6700")
        );
        assert_eq!(
            read(r#"{"amount":"-0.0100"}"#).ok().as_deref(),
            Some("-0.0100")
        );
        assert_eq!(read(r#"{"amount":"0"}"#).ok().as_deref(), Some("0.0000"));
        for bad in [
            r#"{"amount":12345.67}"#,
            r#"{"amount":12}"#,
            r#"{"amount":null}"#,
            r#"{"amount":true}"#,
            r#"{"amount":"1e3"}"#,
            r#"{"amount":"12.34567"}"#,
            r#"{"amount":" 1.00"}"#,
            r#"{"amount":""}"#,
            "{}",
        ] {
            assert!(read(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn desktop_info_tolerates_missing_and_extra_fields() {
        let info: DesktopInfo = serde_json::from_str(
            r#"{"mode":"desktop","version":"1.2.3","engineCommit":null,"offline":"auto",
                "initialized":true,"workspaceTenantId":"6f1c1e0e-1d7f-4c55-9e3e-1b2f7f3f9a10",
                "capabilities":["idempotency-v1","error-codes-v1"]}"#,
        )
        .expect("info");
        assert!(info.capabilities.has(Capabilities::IDEMPOTENCY));
        assert!(!info.capabilities.has(Capabilities::RECORD_VERSION));
        assert_eq!(info.capabilities.iter().count(), 2);
        assert!(info.workspace_tenant_id.is_some());
        let old: DesktopInfo = serde_json::from_str(r#"{"workspaceTenantId":null}"#).expect("old");
        assert_eq!(old.capabilities, Capabilities::default());
        assert_eq!(old.workspace_tenant_id, None);
    }

    #[test]
    fn health_online_null_is_unknown() {
        let h: Health = serde_json::from_str(r#"{"status":"ok","online":null}"#).expect("h");
        assert_eq!(h.online, None);
        let h: Health = serde_json::from_str(
            r#"{"status":"ok","online":false,"lastOnlineCheck":"2026-10-15T08:00:00.000Z"}"#,
        )
        .expect("h");
        assert_eq!(h.online, Some(false));
        assert!(h.last_online_check.is_some());
        let caps: Capabilities = ["a", "b"].into_iter().collect();
        assert!(caps.has("a"));
    }

    #[test]
    fn report_amounts_display_exactly_or_approximately() {
        #[derive(Deserialize)]
        struct Row {
            amount: ReportAmount,
            label: String,
        }
        let cases = [
            ("0.30000000000000004", 2, "0.30"),
            ("1e-7", 2, "0.00"),
            ("-0", 2, "0.00"),
            ("1e21", 2, "1000000000000000000000.00"),
            ("\"1234.5650\"", 2, "1234.57"),
            ("-2.345", 2, "-2.35"),
            ("-0.004", 2, "0.00"),
            ("12", 0, "12"),
        ];
        for (json, dp, want) in cases {
            let a: ReportAmount = serde_json::from_str(json).expect(json);
            assert_eq!(a.display(dp).as_deref(), Some(want), "{json}");
        }
        let big: ReportAmount = serde_json::from_str("1e300").expect("parses");
        assert_eq!(big.display(2), None);
        assert_eq!(big.text(), "1e300");
        for bad in ["null", "true", "{}", "[1]", "\"abc\"", "\"\""] {
            assert!(serde_json::from_str::<ReportAmount>(bad).is_err(), "{bad}");
        }
        let row: Row = serde_json::from_str(r#"{"label":"Sales","amount":1234.5}"#).expect("row");
        assert_eq!(row.amount.text(), "1234.5");
        assert_eq!(row.label, "Sales");
        assert_eq!(format!("{:?}", row.amount), "ReportAmount(1234.5)");
    }

    #[test]
    fn pages_and_page_requests() {
        #[derive(Deserialize, Debug, PartialEq)]
        struct R {
            id: u8,
        }
        let p: Page<R> =
            Page::from_json(br#"{"data":[{"id":1}],"total":7}"#, "data").expect("page");
        assert_eq!(
            p,
            Page {
                data: vec![R { id: 1 }],
                total: 7
            }
        );
        let t: Page<R> =
            Page::from_json(br#"{"tags":[],"total":0,"limit":200,"offset":0}"#, "tags")
                .expect("tags");
        assert_eq!(t.total, 0);
        assert_eq!(t.data.len(), 0);
        for bad in [
            &br#"{"data":[],"total":"3"}"#[..],
            br#"{"total":1}"#,
            b"[]",
            br#"{"data":[{"id":"x"}],"total":1}"#,
        ] {
            assert!(Page::<R>::from_json(bad, "data").is_err());
        }
        assert!(PageRequest::new(0, 0).is_err());
        assert!(PageRequest::new(501, 0).is_err());
        let r = PageRequest::new(500, 1000).expect("ok");
        assert_eq!(r.next().offset(), 1500);
        assert_eq!(r.limit(), 500);
        assert_eq!(
            PageRequest::first_max().query(),
            vec![
                ("limit".to_string(), "500".to_string()),
                ("offset".to_string(), "0".to_string())
            ]
        );
    }
}
