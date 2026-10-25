//! The demo engine's Bulk Entry (P15A lane C): the `--demo` twin of
//! `/api/v1/batch/*`, mirroring the grouped-batch contract in
//! `target/plans/P15A-engine-shapes.md` (capabilities `batch-atomic-v1`,
//! `batch-check-v1`, `batch-mixed-v1`, `batch-exact-v1`,
//! `batch-idempotent-v1`, `batch-closing-v1`). Synthetic.
//!
//! - [`validate`] resolves names and accounts like the engine (an id wins;
//!   a name prefers the row's contact type; an account by exact name, then
//!   number, then name ignoring case) and reports every problem per row.
//!   It posts nothing.
//! - [`save`] is all or nothing: it works on a clone of [`DemoState`],
//!   posts every group through the demo's own postings (Write Check,
//!   Make Deposits, journal entries and the ledger for expenses and card
//!   lines) and swaps the clone in only when every row posted. A refusal
//!   leaves the books exactly as they were.
//! - An idempotency key replays the first result for the same body and is
//!   refused (409 `IDEMPOTENCY_KEY_REUSED`) for a different one. A failed
//!   save stores nothing, so a corrected retry may reuse the key.
//! - Rows dated on or before the closing date need the closing-date
//!   password ("demo-close"), carried on each locked row; it is checked
//!   once, before anything posts.
//! - [`parse_csv`] reads text exactly: amounts come back as strings (never
//!   rounded), a byte-order mark is dropped, quoted values may hold the
//!   delimiter, doubled quotes and line breaks.
//!
//! Every amount is exact `Money`; nothing here is logged.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use cl_api::{
    Account, AccountType, ApiError, BatchField, BatchGroup, BatchKind, BatchRowIn, BatchRowProblem,
    BatchSaved, BatchValidation, CheckDraft, ColumnMapping, ConflictReason, Contact, ContactFields,
    ContactType, CsvField, DepositDraft, ENGINE_MAX_ROWS, FieldProblem, IdempotencyKey, JeLine,
    JeOptions, LocalValidation, NewJournalEntry, PARSE_CSV_MAX_BYTES, ParsedRow, ParsedRows,
    ProblemKind, RowCheck, SavedTxn, Side, SplitLine, SuggestedAccount, Tag, UserText,
};
use cl_core::ids::{AccountId, BatchId, ContactId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};

use super::DemoState;
use super::register::{DemoPosting, check_closing};

/// Most groups in one request (the engine's schema).
pub const MAX_GROUPS: usize = 50;
/// The longest memo the engine takes (characters).
const MEMO_MAX_CHARS: usize = 2000;
/// The longest Ref no. (characters); a journal Entry No. is shorter.
const REF_MAX_CHARS: usize = 50;
/// The longest journal entry number (characters).
const JE_REF_MAX_CHARS: usize = 20;
/// The engine's 409 code for a key sent again with another body.
pub const IDEMPOTENCY_KEY_REUSED: &str = "IDEMPOTENCY_KEY_REUSED";
/// The first demo id of a name added by a batch.
const NEW_CONTACT_BASE: u32 = 90_000;

/// The demo's Bulk Entry saves: each idempotency key with the hash of the
/// body it was first sent with and what that save returned.
#[derive(Clone, Debug, Default)]
pub struct DemoBatches {
    /// Replays by key: (body hash, first result).
    pub replays: BTreeMap<String, (u64, BatchSaved)>,
    /// Save requests that reached the demo engine (replays included).
    /// Tests use it to prove that a refused save sent nothing.
    pub saves: u32,
    /// Batches posted, in order: the batch and its transactions (the
    /// engine's `source = 'batch'`, `source_id = batchId`).
    pub posted: Vec<(BatchId, Vec<TransactionId>)>,
}

// ------------------------------------------------------------ the lists

/// The lists a batch is checked against (built on first use, as in the
/// rest of the demo).
struct Lists {
    chart: Vec<Account>,
    contacts: Vec<Contact>,
    tags: Vec<Tag>,
    tags_gone: BTreeSet<TagId>,
    ar_ap: BTreeSet<AccountId>,
    closing: Option<CalendarDate>,
}

impl Lists {
    fn of(state: &DemoState) -> Lists {
        let chart = state.chart.clone().unwrap_or_else(super::chart);
        let (ar, ap) = super::journal::ar_ap(&chart);
        Lists {
            contacts: state
                .contacts
                .clone()
                .unwrap_or_else(super::contacts::contacts),
            tags: state.tags.clone().unwrap_or_else(super::lists::tags),
            tags_gone: state.tags_gone.clone(),
            ar_ap: ar.union(&ap).copied().collect(),
            closing: super::journal::closing(state),
            chart,
        }
    }

    fn account(&self, id: AccountId) -> Option<&Account> {
        self.chart.iter().find(|a| a.id == id)
    }
}

fn problem(field: BatchField, kind: ProblemKind, message: &str) -> FieldProblem {
    FieldProblem::new(field, kind, message)
}

fn invalid() -> ApiError {
    ApiError::Local(LocalValidation::InvalidValue)
}

fn text(s: Option<&String>) -> Option<&str> {
    s.map(|v| v.trim()).filter(|v| !v.is_empty())
}

// ------------------------------------------------------ the whole request

/// A bank account as the engine takes it for a section: an asset with
/// detail type `bank`, `checking`, `savings`, `cash` or `petty_cash`.
fn is_bank(a: &Account) -> bool {
    a.fields.account_type == AccountType::Asset
        && matches!(
            a.fields.detail_type.as_deref(),
            Some("bank" | "checking" | "savings" | "cash" | "petty_cash")
        )
}

/// A credit card account: a liability with detail type `credit_card`.
fn is_card(a: &Account) -> bool {
    a.fields.account_type == AccountType::Liability
        && a.fields.detail_type.as_deref() == Some("credit_card")
}

/// The section account a group of `kind` may use: checks and deposits a
/// bank account, card charges and credits a card, expenses either.
fn context_fits(kind: BatchKind, a: &Account) -> bool {
    match kind {
        BatchKind::Check | BatchKind::Deposit => is_bank(a),
        BatchKind::CardCharge | BatchKind::CardCredit => is_card(a),
        BatchKind::Expense => is_bank(a) || is_card(a),
        BatchKind::JournalEntry => false,
    }
}

/// A refused group's section account (400
/// `BATCH_CONTEXT_ACCOUNT_REQUIRED` / `_INVALID` with `groupIndex`).
fn bad_context(group: usize) -> ApiError {
    ApiError::BatchGroup {
        group: u32::try_from(group).unwrap_or(u32::MAX),
        problem: cl_api::BatchGroupProblem::ContextAccount,
    }
}

/// The schema and context-account checks: any failure refuses the whole
/// request (400 `VALIDATION_ERROR`, `BATCH_CONTEXT_ACCOUNT_*`).
fn check_request(lists: &Lists, groups: &[BatchGroup]) -> Result<(), ApiError> {
    if groups.is_empty() || groups.len() > MAX_GROUPS {
        return Err(invalid());
    }
    let rows: usize = groups.iter().map(|g| g.rows.len()).sum();
    if rows > ENGINE_MAX_ROWS || groups.iter().any(|g| g.rows.is_empty()) {
        return Err(invalid());
    }
    let mut numbers = BTreeSet::new();
    let unique = groups
        .iter()
        .flat_map(|g| &g.rows)
        .all(|r| r.row_number >= 1 && numbers.insert(r.row_number));
    if !unique {
        return Err(invalid());
    }
    // The schema first (a journal entry group never has an account), then
    // each group's account, the first refused group named.
    if groups
        .iter()
        .any(|g| g.kind == BatchKind::JournalEntry && g.context.is_some())
    {
        return Err(invalid());
    }
    for (gi, g) in groups.iter().enumerate() {
        if g.kind == BatchKind::JournalEntry {
            continue;
        }
        let fits = g
            .context
            .and_then(|id| lists.account(id))
            .is_some_and(|a| a.is_active && context_fits(g.kind, a));
        if !fits {
            return Err(bad_context(gi));
        }
    }
    Ok(())
}

// ------------------------------------------------------------- one row

/// One row as checked: what it resolved to and what is wrong.
#[derive(Clone, Debug)]
struct Checked {
    group: u32,
    row_number: u32,
    contact: Option<ContactId>,
    account: Option<AccountId>,
    /// A name not in the lists (added on save with "Add new names").
    new_contact: Option<String>,
    problems: Vec<FieldProblem>,
}

/// The contact types a name on a row of `kind` prefers.
fn prefers(kind: BatchKind, t: ContactType) -> bool {
    match kind {
        BatchKind::Deposit => matches!(t, ContactType::Customer | ContactType::Both),
        BatchKind::JournalEntry => true,
        _ => matches!(t, ContactType::Vendor | ContactType::Both),
    }
}

/// The type a name added for a row of `kind` gets.
fn new_type(kind: BatchKind) -> ContactType {
    match kind {
        BatchKind::Deposit => ContactType::Customer,
        _ => ContactType::Vendor,
    }
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// The row's name: the id when given, else the typed name (the row's
/// preferred type first, then any).
fn resolve_contact(lists: &Lists, kind: BatchKind, row: &BatchRowIn, c: &mut Checked) {
    let found = if let Some(id) = row.contact {
        lists.contacts.iter().find(|x| x.id == id)
    } else if let Some(name) = text(row.contact_name.as_ref()) {
        let named = |x: &&Contact| same_name(&x.fields.display_name, name);
        let hit = lists
            .contacts
            .iter()
            .filter(|x| prefers(kind, x.fields.contact_type))
            .find(named)
            .or_else(|| lists.contacts.iter().find(named));
        if hit.is_none() {
            c.new_contact = Some(name.to_string());
            return;
        }
        hit
    } else {
        return;
    };
    match found {
        Some(x) if x.is_active => c.contact = Some(x.id),
        Some(_) => c.problems.push(problem(
            BatchField::Contact,
            ProblemKind::Inactive,
            "This name is inactive",
        )),
        None => c.problems.push(problem(
            BatchField::Contact,
            ProblemKind::NotInLists,
            "Name not found",
        )),
    }
}

/// The row's account: the id when given, else the typed text (exact name,
/// then account number, then name ignoring case).
fn resolve_account(lists: &Lists, context: Option<AccountId>, row: &BatchRowIn, c: &mut Checked) {
    let found = if let Some(id) = row.account {
        lists.account(id)
    } else if let Some(name) = text(row.account_name.as_ref()) {
        lists
            .chart
            .iter()
            .find(|a| a.fields.name == name)
            .or_else(|| {
                lists
                    .chart
                    .iter()
                    .find(|a| a.fields.account_number.as_deref() == Some(name))
            })
            .or_else(|| lists.chart.iter().find(|a| same_name(&a.fields.name, name)))
    } else {
        c.problems.push(problem(
            BatchField::Account,
            ProblemKind::Required,
            "Account is required",
        ));
        return;
    };
    match found {
        Some(a) if !a.is_active => c.problems.push(problem(
            BatchField::Account,
            ProblemKind::Inactive,
            "This account is inactive",
        )),
        Some(a) if Some(a.id) == context => c.problems.push(problem(
            BatchField::Account,
            ProblemKind::Invalid,
            "The account must differ from the section's account",
        )),
        Some(a) => c.account = Some(a.id),
        None => c.problems.push(problem(
            BatchField::Account,
            ProblemKind::NotInLists,
            "Account not found",
        )),
    }
}

/// A check number as the engine reads it (`^\d{1,9}$`, at least 1).
fn check_number(s: &str) -> Option<u32> {
    let ok = (1..=9).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| s.parse::<u32>().ok())
        .flatten()
        .filter(|n| *n >= 1)
}

/// The number, print-later, amount, memo and tag rules of one row.
fn row_fields(lists: &Lists, kind: BatchKind, row: &BatchRowIn, c: &mut Checked) {
    let number = text(row.number.as_ref());
    let mut push = |field, kind, message: &str| c.problems.push(problem(field, kind, message));
    match (kind, number) {
        (BatchKind::Check, Some(n)) if check_number(n).is_none() => push(
            BatchField::Number,
            ProblemKind::Invalid,
            "A check number is 1 to 9 digits",
        ),
        (BatchKind::JournalEntry, Some(n)) if n.chars().count() > JE_REF_MAX_CHARS => push(
            BatchField::Number,
            ProblemKind::Invalid,
            "An entry number is at most 20 characters",
        ),
        (_, Some(n)) if n.chars().count() > REF_MAX_CHARS => push(
            BatchField::Number,
            ProblemKind::Invalid,
            "A reference is at most 50 characters",
        ),
        _ => {}
    }
    if row.print_later {
        if kind != BatchKind::Check {
            push(
                BatchField::Row,
                ProblemKind::Invalid,
                "Only checks can be printed later",
            );
        } else if number.is_some() {
            push(
                BatchField::Number,
                ProblemKind::Invalid,
                "A check to print later has no number yet",
            );
        }
        let named = row.contact.is_some() || text(row.contact_name.as_ref()).is_some();
        if !named {
            push(
                BatchField::Contact,
                ProblemKind::Required,
                "A check to print later needs a payee",
            );
        }
    }
    if kind == BatchKind::JournalEntry {
        // Never negative (`VALIDATION_ERROR` on the field). `BatchAmount`
        // already refuses one on this side; the engine's rule stands here
        // for any other caller.
        for (field, side) in [
            (BatchField::Debit, row.debit),
            (BatchField::Credit, row.credit),
        ] {
            if side.is_some_and(|a| !a.money().is_positive()) {
                push(
                    field,
                    ProblemKind::Invalid,
                    "Debits and credits can't be negative",
                );
            }
        }
        if row.amount.is_some() {
            push(
                BatchField::Amount,
                ProblemKind::Invalid,
                "Journal entry lines use Debit or Credit",
            );
        }
        if row.debit.is_some() == row.credit.is_some() {
            push(
                BatchField::Debit,
                ProblemKind::Required,
                "Enter a debit or a credit",
            );
        }
    } else {
        if row.amount.is_none() {
            push(
                BatchField::Amount,
                ProblemKind::Required,
                "Amount is required",
            );
        }
        if row.debit.is_some() || row.credit.is_some() {
            push(
                BatchField::Debit,
                ProblemKind::Invalid,
                "Only journal entry lines use Debit and Credit",
            );
        }
    }
    if row
        .memo
        .as_ref()
        .is_some_and(|m| m.chars().count() > MEMO_MAX_CHARS)
    {
        push(
            BatchField::Memo,
            ProblemKind::Invalid,
            "A memo is at most 2000 characters",
        );
    }
    if let Some(t) = row.tag {
        match lists.tags.iter().find(|x| x.id == t) {
            Some(x) if x.is_active && !lists.tags_gone.contains(&t) => {}
            Some(_) => push(
                BatchField::Tag,
                ProblemKind::Inactive,
                "This tag is inactive",
            ),
            None => push(BatchField::Tag, ProblemKind::NotInLists, "Tag not found"),
        }
    }
}

// --------------------------------------------------------- across rows

/// Check numbers used twice in the batch, or already used on the bank
/// account (`CHECK_NUMBER_DUPLICATE`).
fn duplicate_numbers(state: &DemoState, lists: &Lists, groups: &[BatchGroup], out: &mut [Checked]) {
    let mut seen: BTreeMap<(AccountId, u32), usize> = BTreeMap::new();
    let mut books: BTreeMap<AccountId, BTreeSet<u32>> = BTreeMap::new();
    let mut at = Vec::new();
    let mut i = 0;
    for g in groups {
        for r in &g.rows {
            if g.kind == BatchKind::Check
                && let Some(bank) = g.context
                && let Some(n) = text(r.number.as_ref()).and_then(check_number)
            {
                *seen.entry((bank, n)).or_default() += 1;
                books.entry(bank).or_insert_with(|| {
                    state
                        .ledger
                        .check_numbers(&lists.chart, bank)
                        .into_iter()
                        .filter(|(_, _, void)| !*void)
                        .map(|(_, n, _)| n)
                        .collect()
                });
                at.push((i, bank, n));
            }
            i += 1;
        }
    }
    for (i, bank, n) in at {
        let twice = seen.get(&(bank, n)).is_some_and(|c| *c > 1);
        let used = books.get(&bank).is_some_and(|s| s.contains(&n));
        if twice || used {
            out[i].problems.push(problem(
                BatchField::Number,
                ProblemKind::Other,
                "That check number is already used",
            ));
        }
    }
}

/// The key that makes journal entry rows one entry: the date and Entry
/// No. A row without a number is an entry by itself.
fn entry_key(row: &BatchRowIn, index: usize) -> (CalendarDate, String, usize) {
    match text(row.number.as_ref()) {
        Some(n) => (row.date, n.to_string(), 0),
        None => (row.date, String::new(), index + 1),
    }
}

/// The journal entries of a group: each entry's row indexes (into the
/// group) in first-appearance order.
fn entries(g: &BatchGroup) -> Vec<Vec<usize>> {
    let mut order: Vec<(CalendarDate, String, usize)> = Vec::new();
    let mut by: BTreeMap<(CalendarDate, String, usize), Vec<usize>> = BTreeMap::new();
    for (i, r) in g.rows.iter().enumerate() {
        let k = entry_key(r, i);
        if !by.contains_key(&k) {
            order.push(k.clone());
        }
        by.entry(k).or_default().push(i);
    }
    order.into_iter().filter_map(|k| by.remove(&k)).collect()
}

/// The entry rules: at least two lines, debits equal credits exactly
/// (`TXN_UNBALANCED`), a name on every A/R or A/P line.
fn journal_rules(lists: &Lists, g: &BatchGroup, out: &mut [Checked]) {
    for rows in entries(g) {
        if rows.len() < 2 {
            for &i in &rows {
                out[i].problems.push(problem(
                    BatchField::Number,
                    ProblemKind::Invalid,
                    "A journal entry needs at least two lines with the same date and Entry No.",
                ));
            }
            continue;
        }
        let side = |pick: fn(&BatchRowIn) -> Option<cl_api::BatchAmount>| {
            Money::sum(
                rows.iter()
                    .filter_map(|&i| pick(&g.rows[i]).map(cl_api::BatchAmount::money)),
            )
        };
        let debits = side(|r| r.debit);
        let credits = side(|r| r.credit);
        if !matches!((debits, credits), (Ok(d), Ok(c)) if d == c) {
            for &i in &rows {
                out[i].problems.push(problem(
                    BatchField::Row,
                    ProblemKind::Unbalanced,
                    "Debits and credits must be equal",
                ));
            }
        }
        // Any number of A/R or A/P lines, each with a name (Q-206, engine
        // 33a973fa).
        for &i in &rows {
            if out[i].account.is_some_and(|a| lists.ar_ap.contains(&a))
                && out[i].contact.is_none()
                && out[i].new_contact.is_none()
            {
                out[i].problems.push(problem(
                    BatchField::Contact,
                    ProblemKind::Required,
                    "An Accounts Receivable or Accounts Payable line needs a name",
                ));
            }
        }
    }
}

/// Every row's result, in request order.
fn check_rows(state: &DemoState, lists: &Lists, groups: &[BatchGroup]) -> Vec<Checked> {
    let mut out = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        let group = u32::try_from(gi).unwrap_or(u32::MAX);
        for r in &g.rows {
            let mut c = Checked {
                group,
                row_number: r.row_number,
                contact: None,
                account: None,
                new_contact: None,
                problems: Vec::new(),
            };
            resolve_contact(lists, g.kind, r, &mut c);
            resolve_account(lists, g.context, r, &mut c);
            // `BATCH_ACCOUNT_AR_AP`: only a journal entry line may use
            // Accounts Receivable or Accounts Payable.
            if g.kind != BatchKind::JournalEntry
                && c.account.is_some_and(|a| lists.ar_ap.contains(&a))
            {
                c.problems.push(problem(
                    BatchField::Account,
                    ProblemKind::Invalid,
                    "Accounts Receivable and Accounts Payable can't be used here. Use an invoice, a bill or a journal entry.",
                ));
            }
            row_fields(lists, g.kind, r, &mut c);
            out.push(c);
        }
    }
    duplicate_numbers(state, lists, groups, &mut out);
    let mut start = 0;
    for g in groups {
        let end = start + g.rows.len();
        if g.kind == BatchKind::JournalEntry {
            journal_rules(lists, g, &mut out[start..end]);
        }
        start = end;
    }
    out
}

/// `POST /batch/validate`: resolves names and accounts, posts nothing.
///
/// # Errors
/// `Local(InvalidValue)` for what the engine refuses as a whole (no or
/// too many groups or rows, a row number used twice, an account on a
/// journal entry group); `BatchGroup` (`ContextAccount`) for a group whose
/// bank account or card is missing, inactive or of the wrong kind.
pub fn validate(state: &DemoState, groups: &[BatchGroup]) -> Result<BatchValidation, ApiError> {
    let lists = Lists::of(state);
    check_request(&lists, groups)?;
    let rows = check_rows(state, &lists, groups)
        .into_iter()
        .map(|c| RowCheck {
            group: c.group,
            row_number: c.row_number,
            ok: c.problems.is_empty(),
            contact: c.contact,
            account: c.account,
            problems: c.problems,
            new_contact: c.new_contact.is_some(),
        })
        .collect();
    Ok(BatchValidation { rows })
}

// ------------------------------------------------------------------ save

/// The hash of everything the engine stores the body by: every row and
/// the "add new names" choice, never the closing password.
fn body_hash(groups: &[BatchGroup], add_new_names: bool) -> u64 {
    let mut h = DefaultHasher::new();
    add_new_names.hash(&mut h);
    let amount = |a: Option<cl_api::BatchAmount>| a.map(cl_api::BatchAmount::to_wire);
    for g in groups {
        g.kind.wire().hash(&mut h);
        g.context.map(|a| a.to_string()).hash(&mut h);
        g.rows.len().hash(&mut h);
        for r in &g.rows {
            r.row_number.hash(&mut h);
            r.date.to_wire().hash(&mut h);
            r.number.hash(&mut h);
            r.print_later.hash(&mut h);
            r.contact.map(|c| c.to_string()).hash(&mut h);
            r.contact_name.hash(&mut h);
            r.account.map(|a| a.to_string()).hash(&mut h);
            r.account_name.hash(&mut h);
            amount(r.amount).hash(&mut h);
            amount(r.debit).hash(&mut h);
            amount(r.credit).hash(&mut h);
            r.memo.hash(&mut h);
            r.tag.map(|t| t.to_string()).hash(&mut h);
        }
    }
    h.finish()
}

fn refused(c: &Checked, p: FieldProblem) -> BatchRowProblem {
    BatchRowProblem {
        group: c.group,
        row_number: c.row_number,
        problem: p,
    }
}

/// The closed-period rule, once for the whole batch, before anything
/// posts: every row dated on or before the closing date carries the
/// password.
fn check_closed(closing: Option<CalendarDate>, groups: &[BatchGroup]) -> Result<(), ApiError> {
    let locked: Vec<&BatchRowIn> = groups
        .iter()
        .flat_map(|g| &g.rows)
        .filter(|r| closing.is_some_and(|c| r.date <= c))
        .collect();
    if let Some(r) = locked.iter().find(|r| r.closing_password.is_none()) {
        return check_closing(closing, &[r.date], None);
    }
    for r in locked {
        check_closing(closing, &[r.date], r.closing_password.as_ref())?;
    }
    Ok(())
}

/// A posting's own refusal, named by the row (the engine prefixes its
/// message with `Row n: `). A closing-date refusal stays itself.
fn posting_failed(group: u32, row: &BatchRowIn, e: ApiError) -> ApiError {
    let field = match &e {
        ApiError::ClosedPeriod(_)
        | ApiError::ClosingPasswordRejected
        | ApiError::ClosingPasswordRateLimited { .. } => return e,
        ApiError::Conflict { .. } => BatchField::Number,
        _ => BatchField::Row,
    };
    let message = if field == BatchField::Number {
        format!("Row {}: That check number is already used", row.row_number)
    } else {
        format!("Row {}: This row could not be saved", row.row_number)
    };
    BatchRowProblem::into_error(vec![BatchRowProblem {
        group,
        row_number: row.row_number,
        problem: problem(field, ProblemKind::Other, &message),
    }])
}

/// Adds the names the batch does not know (one per name, ignoring case)
/// and returns how many were added.
fn add_names(state: &mut DemoState, groups: &[BatchGroup], checked: &mut [Checked]) -> u32 {
    let mut added: BTreeMap<String, ContactId> = BTreeMap::new();
    let kinds: Vec<BatchKind> = groups
        .iter()
        .flat_map(|g| g.rows.iter().map(move |_| g.kind))
        .collect();
    for (c, kind) in checked.iter_mut().zip(kinds) {
        let Some(name) = c.new_contact.take() else {
            continue;
        };
        let key = name.to_lowercase();
        let id = if let Some(id) = added.get(&key) {
            *id
        } else {
            let contacts = state.contacts_mut();
            let n = NEW_CONTACT_BASE + u32::try_from(contacts.len()).unwrap_or(0);
            let id = super::contacts::contact_id(n);
            contacts.push(Contact {
                id,
                fields: ContactFields::named(new_type(kind), name.clone()),
                is_active: true,
                opening_balance: Some(Money::ZERO),
                opening_balance_date: None,
                created_at: Some(DemoState::now()),
                updated_at: Some(DemoState::now()),
            });
            state.contact_names.insert(id, name);
            added.insert(key, id);
            id
        };
        c.contact = Some(id);
    }
    u32::try_from(added.len()).unwrap_or(u32::MAX)
}

fn contact_name(state: &mut DemoState, id: Option<ContactId>) -> Option<String> {
    let id = id?;
    state
        .contacts_mut()
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.fields.display_name.clone())
}

fn tag_name(state: &mut DemoState, id: Option<TagId>) -> Option<String> {
    let id = id?;
    state
        .tags_mut()
        .iter()
        .find(|t| t.id == id)
        .map(|t| t.fields.name.clone())
}

/// What one non-journal row posts as.
struct RowPost<'a> {
    kind: BatchKind,
    context: AccountId,
    row: &'a BatchRowIn,
    contact: Option<ContactId>,
    account: AccountId,
    amount: Money,
    /// The password, on a row dated on or before the closing date.
    password: Option<&'a Secret>,
}

/// The next check number on `bank`: one past the highest of any live
/// check on it (each check posted in the batch counts at once).
fn next_check_number(state: &mut DemoState, bank: AccountId) -> u32 {
    super::checks::numbers(state, bank)
        .1
        .map_or(1, |n| n.saturating_add(1))
}

/// Posts one check, expense, card line or deposit row.
fn post_row(state: &mut DemoState, p: &RowPost<'_>) -> Result<TransactionId, ApiError> {
    let memo = text(p.row.memo.as_ref()).map(str::to_string);
    let line = SplitLine {
        account: p.account,
        amount: p.amount,
        memo: memo.clone(),
        contact: None,
        billable: false,
        tag: p.row.tag,
    };
    match p.kind {
        BatchKind::Check => {
            let number = match text(p.row.number.as_ref()) {
                _ if p.row.print_later => None,
                Some(n) => check_number(n),
                None => Some(next_check_number(state, p.context)),
            };
            let draft = CheckDraft {
                bank: p.context,
                date: p.row.date,
                payee: p.contact,
                payee_name: contact_name(state, p.contact).unwrap_or_default(),
                check_number: number,
                print_later: p.row.print_later,
                payee_address: None,
                memo,
                printed_memo: None,
                tag_ids: Vec::new(),
                draft_attachment: None,
                amount: p.amount,
                lines: vec![line],
            };
            super::checks::create(state, &draft, p.password)
        }
        BatchKind::Deposit => {
            let line = SplitLine {
                contact: p.contact,
                ..line
            };
            let d = DepositDraft::plain(p.context, p.row.date, memo, vec![line]);
            super::deposits::create(state, &d, p.password)
        }
        BatchKind::Expense | BatchKind::CardCharge | BatchKind::CardCredit => {
            let closing = super::journal::closing(state);
            check_closing(closing, &[p.row.date], p.password)?;
            let chart = state.chart.get_or_insert_with(super::chart).clone();
            // A card credit posts as a deposit into the card: DR card, CR
            // the category.
            let (kind, own_debit) = match p.kind {
                BatchKind::Expense => ("expense", false),
                BatchKind::CardCharge => ("card_charge", false),
                _ => ("deposit", true),
            };
            let posting = DemoPosting {
                kind,
                date: p.row.date,
                number: text(p.row.number.as_ref()).unwrap_or_default().to_string(),
                contact: contact_name(state, p.contact),
                memo: memo.clone().unwrap_or_default(),
                own: p.context,
                own_debit,
                lines: vec![(p.account, p.amount)],
            };
            let id = state
                .ledger
                .save(&chart, &posting, None)
                .ok_or_else(invalid)?;
            let details = [(memo, tag_name(state, p.row.tag))];
            state
                .ledger
                .set_line_details(&chart, id, p.context, &details);
            Ok(id)
        }
        BatchKind::JournalEntry => Err(invalid()),
    }
}

/// Posts one journal entry (its rows' indexes into the group).
fn post_entry(
    state: &mut DemoState,
    g: &BatchGroup,
    rows: &[usize],
    checked: &[Checked],
    closing: Option<CalendarDate>,
) -> Result<TransactionId, ApiError> {
    let first = &g.rows[rows[0]];
    let mut lines = Vec::with_capacity(rows.len());
    for &i in rows {
        let r = &g.rows[i];
        let side = match (r.debit, r.credit) {
            (Some(d), None) => Side::Debit(d.money()),
            (None, Some(c)) => Side::Credit(c.money()),
            _ => return Err(invalid()),
        };
        lines.push(JeLine {
            account: checked[i].account.ok_or_else(invalid)?,
            side,
            description: text(r.memo.as_ref()).map(str::to_string),
            tag: r.tag,
            contact: checked[i].contact,
        });
    }
    let chart = state.chart.get_or_insert_with(super::chart).clone();
    let (ar, ap) = super::journal::ar_ap(&chart);
    let roles = cl_api::AccountRoles {
        receivable: ar,
        payable: ap,
    };
    let opts = JeOptions {
        number: text(first.number.as_ref()).map(str::to_string),
        ..JeOptions::default()
    };
    let entry = NewJournalEntry::new_with(
        first.date,
        text(first.memo.as_ref()).map(str::to_string),
        cl_api::Basis::Both,
        lines,
        opts,
        &roles,
    )
    .map_err(ApiError::Local)?;
    let password = closing
        .filter(|c| first.date <= *c)
        .and(first.closing_password.as_ref());
    super::journal::save(state, &entry, None, password).map(|(id, _)| id)
}

/// Posts every group into `work`; the transaction of each row, in request
/// order.
fn post_all(
    work: &mut DemoState,
    groups: &[BatchGroup],
    checked: &[Checked],
) -> Result<Vec<SavedTxn>, ApiError> {
    let closing = super::journal::closing(work);
    let mut saved = Vec::with_capacity(checked.len());
    let mut start = 0;
    for (gi, g) in groups.iter().enumerate() {
        let group = u32::try_from(gi).unwrap_or(u32::MAX);
        let rows = &checked[start..start + g.rows.len()];
        start += g.rows.len();
        if g.kind == BatchKind::JournalEntry {
            let mut ids: BTreeMap<usize, TransactionId> = BTreeMap::new();
            for entry in entries(g) {
                let id = post_entry(work, g, &entry, rows, closing)
                    .map_err(|e| posting_failed(group, &g.rows[entry[0]], e))?;
                ids.extend(entry.iter().map(|&i| (i, id)));
            }
            for (i, r) in g.rows.iter().enumerate() {
                let id = *ids.get(&i).ok_or_else(invalid)?;
                saved.push(SavedTxn {
                    group,
                    row_number: r.row_number,
                    id,
                });
            }
            continue;
        }
        let context = g.context.ok_or_else(invalid)?;
        for (r, c) in g.rows.iter().zip(rows) {
            let post = RowPost {
                kind: g.kind,
                context,
                row: r,
                contact: c.contact,
                account: c.account.ok_or_else(invalid)?,
                amount: r.amount.ok_or_else(invalid)?.money(),
                password: closing
                    .filter(|d| r.date <= *d)
                    .and(r.closing_password.as_ref()),
            };
            let id = post_row(work, &post).map_err(|e| posting_failed(group, r, e))?;
            saved.push(SavedTxn {
                group,
                row_number: r.row_number,
                id,
            });
        }
    }
    Ok(saved)
}

/// `POST /batch/save`: every row or nothing (`BatchRows` names the refused
/// rows), in this order:
/// 1. the idempotency key: the same body replays the first result, another
///    body is refused (409 `IDEMPOTENCY_KEY_REUSED`);
/// 2. the request as a whole (as [`validate`]);
/// 3. every invalid row (`BATCH_INVALID`), then names not in the lists
///    without `add_new_names` (`BATCH_CONTACT_NOT_FOUND`);
/// 4. the closed period (one check over every locked row);
/// 5. posting, on a clone that replaces the books only when every row
///    posted.
///
/// # Errors
/// `Conflict` (key reused), `Local(InvalidValue)` (the request as a
/// whole), `BatchGroup` (a group's account), `BatchRows`, `ClosedPeriod`, `ClosingPasswordRejected`. Nothing
/// posts for any of them.
pub fn save(
    state: &mut DemoState,
    groups: &[BatchGroup],
    key: &IdempotencyKey,
    add_new_names: bool,
) -> Result<BatchSaved, ApiError> {
    state.batch.saves += 1;
    let hash = body_hash(groups, add_new_names);
    let key = key.as_header();
    if let Some((stored, first)) = state.batch.replays.get(&key) {
        if *stored == hash {
            return Ok(first.clone());
        }
        return Err(ApiError::Conflict {
            reason: ConflictReason::Unspecified,
            current_version: None,
            code: cl_api::ErrorCode::parse(IDEMPOTENCY_KEY_REUSED),
            message: UserText::default(),
        });
    }
    let lists = Lists::of(state);
    check_request(&lists, groups)?;
    let mut checked = check_rows(state, &lists, groups);
    let bad: Vec<BatchRowProblem> = checked
        .iter()
        .flat_map(|c| c.problems.iter().map(|p| refused(c, p.clone())))
        .collect();
    if !bad.is_empty() {
        return Err(BatchRowProblem::into_error(bad));
    }
    if !add_new_names {
        let unknown: Vec<BatchRowProblem> = checked
            .iter()
            .filter(|c| c.new_contact.is_some())
            .map(|c| {
                let p = problem(
                    BatchField::Contact,
                    ProblemKind::NotInLists,
                    "Name not found",
                );
                refused(c, p)
            })
            .collect();
        if !unknown.is_empty() {
            return Err(BatchRowProblem::into_error(unknown));
        }
    }
    check_closed(lists.closing, groups)?;

    let mut work = state.clone();
    let created_contacts = add_names(&mut work, groups, &mut checked);
    let saved = post_all(&mut work, groups, &checked)?;
    let n = u32::try_from(work.batch.posted.len() + 1).unwrap_or(u32::MAX);
    let batch_id = BatchId::parse(&format!("d0000000-0000-4000-8000-e{n:011}")).ok();
    let result = BatchSaved {
        batch_id,
        saved,
        created_contacts,
    };
    if let Some(id) = batch_id {
        let mut txns: Vec<TransactionId> = Vec::new();
        for s in &result.saved {
            if !txns.contains(&s.id) {
                txns.push(s.id);
            }
        }
        work.batch.posted.push((id, txns));
    }
    work.batch.replays.insert(key, (hash, result.clone()));
    *state = work;
    Ok(result)
}

/// `GET /batch/suggest-account`: the name's default account, then the
/// account of its most recent transaction. Never an amount.
///
/// # Errors
/// None in the demo (an unknown name has no suggestion).
pub fn suggest_account(
    state: &DemoState,
    contact: ContactId,
) -> Result<Option<SuggestedAccount>, ApiError> {
    // The demo's lookups build lists on first use, so they work on a copy.
    let mut s = state.clone();
    Ok(super::checks::suggest_account(&mut s, contact))
}

// ------------------------------------------------------------------- CSV

/// The engine's header aliases, per field (lower case).
const ALIASES: [(CsvField, &[&str]); 8] = [
    (
        CsvField::Date,
        &[
            "date",
            "txn_date",
            "transaction date",
            "trans date",
            "posted date",
        ],
    ),
    (
        CsvField::RefNo,
        &[
            "ref",
            "ref no",
            "ref_no",
            "reference",
            "check no",
            "check #",
            "num",
        ],
    ),
    (
        CsvField::Payee,
        &[
            "payee",
            "vendor",
            "paid to",
            "name",
            "customer",
            "received from",
        ],
    ),
    (
        CsvField::Account,
        &[
            "account",
            "category",
            "expense account",
            "revenue account",
            "gl account",
        ],
    ),
    (
        CsvField::Memo,
        &["memo", "description", "note", "notes", "detail"],
    ),
    (
        CsvField::Amount,
        &["amount", "total", "payment", "deposit", "charge"],
    ),
    (CsvField::Debit, &["debit", "dr"]),
    (CsvField::Credit, &["credit", "cr"]),
];

/// The engine's mapping when none is sent: each field's first header
/// that matches one of its aliases.
#[must_use]
pub fn auto_mapping(headers: &[String]) -> ColumnMapping {
    let mut m = ColumnMapping::default();
    for (field, names) in ALIASES {
        let hit = headers
            .iter()
            .position(|h| names.contains(&h.trim().to_lowercase().as_str()));
        if let Some(i) = hit.and_then(|i| u16::try_from(i).ok()) {
            m.0.insert(field, i);
        }
    }
    m
}

/// A date as the engine normalizes it: `YYYY-MM-DD`, `M/D/YYYY` and
/// `YYYY/M/D` (with `/` or `-`) become `YYYY-MM-DD`; anything else comes
/// back unchanged.
#[must_use]
pub fn normalize_date(raw: &str) -> String {
    let parts: Vec<&str> = raw.split(['/', '-']).collect();
    let digits = |s: &str, lo: usize, hi: usize| {
        (lo..=hi).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
    };
    if let [a, b, c] = parts.as_slice() {
        if digits(a, 4, 4) && digits(b, 1, 2) && digits(c, 1, 2) {
            return format!("{a}-{b:0>2}-{c:0>2}");
        }
        if digits(a, 1, 2) && digits(b, 1, 2) && digits(c, 4, 4) {
            return format!("{c}-{a:0>2}-{b:0>2}");
        }
    }
    raw.to_string()
}

/// An amount as `batch-exact-v1` returns it: `$`, `£`, `€`, commas and
/// spaces removed; `(12.50)` and `12.50-` become `-12.50`; `.5` becomes
/// `0.5`; leading zeros dropped; every decimal kept (never rounded). Text
/// that is not a number comes back unchanged.
#[must_use]
pub fn normalize_amount(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !matches!(c, '$' | '£' | '€' | ',') && !c.is_whitespace())
        .collect();
    let (negative, body) =
        if let Some(inner) = cleaned.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
            (true, inner)
        } else if let Some(inner) = cleaned.strip_suffix('-') {
            (true, inner)
        } else if let Some(inner) = cleaned.strip_prefix('-') {
            (true, inner)
        } else {
            (false, cleaned.as_str())
        };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if int.len() + frac.len() == 0 || !all_digits(int) || !all_digits(frac) {
        return raw.to_string();
    }
    let int = int.trim_start_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    let sign = if negative { "-" } else { "" };
    if frac.is_empty() {
        format!("{sign}{int}")
    } else {
        format!("{sign}{int}.{frac}")
    }
}

/// `POST /batch/parse-csv`: the file's rows as text. The first record is
/// the header; the delimiter is a tab when the header has one, else a
/// comma; the mapping (field → column) is used as sent, or worked out from
/// the headers when empty.
///
/// # Errors
/// `Local(AttachmentTooLarge)` for an empty file or one over 10 MB;
/// `Local(InvalidValue)` for a file with no data rows (`CSV_EMPTY`) or an
/// unterminated quote (`CSV_MALFORMED`); `Local(TooManyLines)` for more
/// than 10,000 rows (`CSV_TOO_MANY_ROWS`).
pub fn parse_csv(
    _kind: BatchKind,
    bytes: &[u8],
    mapping: &ColumnMapping,
) -> Result<ParsedRows, ApiError> {
    if bytes.is_empty() || bytes.len() > PARSE_CSV_MAX_BYTES {
        return Err(ApiError::Local(LocalValidation::AttachmentTooLarge));
    }
    let text = cl_import::csv::decode(bytes);
    let first_line = text.lines().next().unwrap_or_default();
    let sep = if first_line.contains('\t') { '\t' } else { ',' };
    let records = cl_import::csv::parse_with(&text, sep).map_err(|e| match e {
        cl_import::csv::CsvError::TooManyRows => ApiError::Local(LocalValidation::TooManyLines),
        _ => invalid(),
    })?;
    let Some((header, data)) = records.split_first() else {
        return Err(invalid());
    };
    if data.is_empty() {
        return Err(invalid());
    }
    if data.len() > ENGINE_MAX_ROWS {
        return Err(ApiError::Local(LocalValidation::TooManyLines));
    }
    let mapping = if mapping.0.is_empty() {
        auto_mapping(&header.fields)
    } else {
        mapping.clone()
    };
    let rows = data
        .iter()
        .enumerate()
        .map(|(i, rec)| {
            let cell = |f: CsvField| {
                let i = usize::from(*mapping.0.get(&f)?);
                let v = rec.fields.get(i)?.trim();
                (!v.is_empty()).then(|| v.to_string())
            };
            let money = |f: CsvField| cell(f).map(|v| normalize_amount(&v));
            ParsedRow {
                row_number: u32::try_from(i + 1).unwrap_or(u32::MAX),
                date: cell(CsvField::Date).map(|v| normalize_date(&v)),
                ref_no: cell(CsvField::RefNo),
                contact_name: cell(CsvField::Payee),
                account_name: cell(CsvField::Account),
                memo: cell(CsvField::Memo),
                amount: money(CsvField::Amount),
                debit: money(CsvField::Debit),
                credit: money(CsvField::Credit),
            }
        })
        .collect();
    Ok(ParsedRows { rows })
}

#[cfg(test)]
#[path = "batch_tests.rs"]
mod tests;
