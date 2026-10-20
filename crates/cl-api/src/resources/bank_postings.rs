//! Checks, deposits, transfers and card charges: the postings a register
//! records inline (P12 plan WP2).
//!
//! - A [`BankPosting`] is built from a draft through its checked
//!   constructor, which refuses before anything is sent: no lines or more
//!   than 500, a zero or negative amount, split lines that do not add up to
//!   a check's amount, the same account on both sides, a line contact where
//!   the engine has none. Amounts are `Money` and travel as
//!   `Money::to_wire()` strings, never JSON numbers.
//! - Creates and updates carry the outcome probe (ledger stamp plus the
//!   fingerprint the engine's record will have), so a lost reply is
//!   reconciled instead of entered twice. Updates are fingerprint-guarded
//!   (`Conflict { Stale }`, nothing sent) until U-17.
//! - The closing-date password rides in [`PostingOptions`] (`closingPassword`
//!   in the body, U-15; refused locally without `closing-password-v1`). A
//!   body that carries it asks the endpoint source for a credentials-grade
//!   endpoint (`Call::json_body` marks it sensitive).
//! - The engine stores a check as an `expense` with a check number. Without
//!   `check-edit-v1` a check is changed through `PUT /transactions/{id}` as
//!   an expense: its number, the name printed on it, its address and Print
//!   later stay as they were. With it, `checks().update_check` (P13,
//!   `resources/checks.rs`) changes those too.
//! - Line tags are always sent (`null` for none) so the engine never fills a
//!   default tag the owner did not choose.
//! - A check or card-charge line's customer and billable mark (P13, P14)
//!   are sent only when set and only once the engine advertises
//!   `line-billable-v1`; the caller passes that flag in
//!   ([`BankPosting::check_with`], [`BankPosting::card_charge_with`]) so the
//!   builder stays pure.
//! - A deposit (P14) may also take whole pending payments or a refunded
//!   vendor credit (source lines at the engine's exact amount), fees
//!   (positive here, negative on the wire) and cash back, each behind its
//!   capability ([`BankPosting::deposit_with`], [`DepositCaps`]). The bank
//!   total is computed exactly and refused locally unless it is greater
//!   than zero.

use cl_core::ids::{AccountId, ContactId, TagId, TransactionId};
use cl_core::{CalendarDate, Money};
use serde_json::{Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::attachments::DraftKey;
use crate::resources::common::remember_created;
use crate::resources::operations::{CHECKS_CREATE, TXN_GET, TXN_PUT, TXNS_CREATE};
use crate::resources::posting::{PostingOptions, posting_body};
use crate::resources::reconcile::{OutcomeMatcher, Probe, Search, row_fp};
use crate::resources::transactions::{
    Basis, MAX_LINES, Transaction, Transactions, TxnStatus, TxnType, line_editable, non_empty,
    txn_editable, with_source,
};
use crate::resources::versioning::{
    GuardedUpdate, RecordVersion, Versioned, fingerprint_of, saved_from, update_guarded,
};
use crate::wire::Capabilities;

/// The longest name the engine prints on a check.
pub const PAYEE_NAME_MAX: usize = 255;
/// The longest payee address the engine takes (UTF-16 code units, as the
/// engine's schema counts them).
pub const PAYEE_ADDRESS_MAX: usize = 500;
/// The longest memo printed on a check (UTF-16 code units).
pub const PRINTED_MEMO_MAX: usize = 255;

/// One split line of a posting: the other side of the bank line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitLine {
    /// The account (expense, income, or any other account).
    pub account: AccountId,
    /// The amount (greater than zero).
    pub amount: Money,
    /// Line memo (owner data: never logged).
    pub memo: Option<String>,
    /// Line customer: a deposit's customer, or the customer (job) a check
    /// line is for (`line-billable-v1`).
    pub contact: Option<ContactId>,
    /// The check line is billable to `contact` (`line-billable-v1`; needs
    /// a customer).
    pub billable: bool,
    /// Line tag.
    pub tag: Option<TagId>,
}

/// A check to write (`POST /checks`, or `PUT /checks/{id}` with
/// `check-edit-v1`).
#[derive(Clone, PartialEq, Eq)]
pub struct CheckDraft {
    /// The bank account it is drawn on.
    pub bank: AccountId,
    /// Date.
    pub date: CalendarDate,
    /// Payee contact.
    pub payee: Option<ContactId>,
    /// The name printed on the check (required).
    pub payee_name: String,
    /// A hand-written check's number (the engine assigns the next one
    /// otherwise). Never set together with `print_later`.
    pub check_number: Option<u32>,
    /// Queue the check for printing: no number until it prints (`printStatus`
    /// `queue`).
    pub print_later: bool,
    /// The address printed under the payee's name (lines separated by a
    /// line feed, at most 500 characters).
    pub payee_address: Option<String>,
    /// Memo kept in the books.
    pub memo: Option<String>,
    /// The memo printed on the check (at most 255 characters).
    pub printed_memo: Option<String>,
    /// Header tags. On a create they are the check's tags; an edit
    /// (`PUT /checks/{id}`) always sends them and the engine replaces the
    /// check's header tags with exactly these (empty clears them).
    pub tag_ids: Vec<TagId>,
    /// The draft the form's files were uploaded under before the first
    /// save; the create moves them onto the check (not sent on an edit).
    pub draft_attachment: Option<DraftKey>,
    /// The check amount; the lines must add up to it exactly.
    pub amount: Money,
    /// Where the money went.
    pub lines: Vec<SplitLine>,
}

/// Ids, the date, the flags and counts only: names, addresses, memos and
/// amounts are owner data.
impl std::fmt::Debug for CheckDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckDraft")
            .field("bank", &self.bank)
            .field("date", &self.date)
            .field("payee", &self.payee)
            .field("print_later", &self.print_later)
            .field("tag_ids", &self.tag_ids)
            .field("draft_attachment", &self.draft_attachment)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

/// A deposit (`POST /transactions`, `deposit`).
///
/// The bank gets `Σpayments + Σlines − Σdeductions − cash back`
/// ([`DepositDraft::total`]), which must be greater than zero. The P12
/// plain deposit has only `lines` ([`DepositDraft::plain`]).
#[derive(Clone, PartialEq, Eq)]
pub struct DepositDraft {
    /// The account the money goes into.
    pub account: AccountId,
    /// Date.
    pub date: CalendarDate,
    /// Memo.
    pub memo: Option<String>,
    /// Pending payments (or a refunded vendor credit) taken in whole, at
    /// the engine's exact amount (`deposit-grouping-v1`,
    /// `vendor-refund-v1`). Sent first, in this order.
    pub payments: Vec<GroupedPayment>,
    /// Other money deposited (each amount greater than zero).
    pub lines: Vec<SplitLine>,
    /// Fees taken out of the deposit: each amount is positive here and goes
    /// out negative, posting as a debit (`deposit-cashback-v1`, Q-84).
    pub deductions: Vec<SplitLine>,
    /// Cash kept back from the deposit (`deposit-cashback-v1`).
    pub cash_back: Option<CashBack>,
}

/// Ids, the date and counts only: amounts and memos are owner data.
impl std::fmt::Debug for DepositDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DepositDraft")
            .field("account", &self.account)
            .field("date", &self.date)
            .field("payments", &self.payments)
            .field("lines", &self.lines.len())
            .field("deductions", &self.deductions.len())
            .field("cash_back", &self.cash_back)
            .finish_non_exhaustive()
    }
}

impl DepositDraft {
    /// A deposit of plain lines only (the P12 register deposit).
    #[must_use]
    pub fn plain(
        account: AccountId,
        date: CalendarDate,
        memo: Option<String>,
        lines: Vec<SplitLine>,
    ) -> DepositDraft {
        DepositDraft {
            account,
            date,
            memo,
            payments: Vec::new(),
            lines,
            deductions: Vec::new(),
            cash_back: None,
        }
    }

    /// What the bank gets: `Σpayments + Σlines − Σdeductions − cash back`,
    /// exactly. `None` when the sum overflows. The checked builder
    /// ([`BankPosting::deposit_with`]) refuses a total that is not greater
    /// than zero.
    #[must_use]
    pub fn total(&self) -> Option<Money> {
        let payments = Money::sum(self.payments.iter().map(|p| p.amount)).ok()?;
        let lines = Money::sum(self.lines.iter().map(|l| l.amount)).ok()?;
        let deductions = Money::sum(self.deductions.iter().map(|l| l.amount)).ok()?;
        let cash_back = self.cash_back.as_ref().map_or(Money::ZERO, |c| c.amount);
        payments
            .checked_add(lines)
            .and_then(|t| t.checked_sub(deductions))
            .and_then(|t| t.checked_sub(cash_back))
            .ok()
    }
}

/// What a deposit's source line takes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DepositSource {
    /// A payment (or a daily-sales tender) waiting in Payments Clearing
    /// (`deposit-grouping-v1`): the line is on Payments Clearing and its
    /// amount is exactly what the payment holds there.
    Payment,
    /// A vendor credit refunded in cash (`vendor-refund-v1`): the line is
    /// on Accounts Payable and names the credit's vendor; the amount is at
    /// most the credit's open balance.
    VendorCredit,
}

/// One whole document deposited: its amount is the engine's figure, never
/// typed or rounded (P14 plan §4).
#[derive(Clone, PartialEq, Eq)]
pub struct GroupedPayment {
    /// The payment (or vendor credit) transaction.
    pub source: TransactionId,
    /// Which kind of document it is.
    pub kind: DepositSource,
    /// Payments Clearing for a payment, Accounts Payable for a vendor
    /// credit (the engine checks it).
    pub account: AccountId,
    /// Exactly the engine's undeposited amount (greater than zero).
    pub amount: Money,
    /// Received from (required for a vendor credit: its vendor).
    pub contact: Option<ContactId>,
    /// Line memo (owner data: never logged).
    pub memo: Option<String>,
}

/// Ids and the kind only.
impl std::fmt::Debug for GroupedPayment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroupedPayment")
            .field("source", &self.source)
            .field("kind", &self.kind)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// Cash kept back from a deposit (`deposit-cashback-v1`).
#[derive(Clone, PartialEq, Eq)]
pub struct CashBack {
    /// The account the cash goes to (debited; never the deposit account).
    pub account: AccountId,
    /// How much (greater than zero, less than the rest of the deposit).
    pub amount: Money,
    /// Memo (at most 1000 characters; owner data, never logged).
    pub memo: Option<String>,
}

/// The account only.
impl std::fmt::Debug for CashBack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CashBack")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// The longest cash-back memo the engine takes.
pub const CASH_BACK_MEMO_MAX: usize = 1000;

/// Which deposit parts the engine takes (from its capabilities).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DepositCaps {
    /// `deposit-grouping-v1`: pending payments as source lines.
    pub grouping: bool,
    /// `deposit-cashback-v1`: cash back and fee (negative) lines.
    pub cashback: bool,
    /// `vendor-refund-v1`: a refunded vendor credit as a source line.
    pub vendor_refund: bool,
}

impl DepositCaps {
    /// What `caps` advertises.
    #[must_use]
    pub fn from_caps(caps: &Capabilities) -> DepositCaps {
        DepositCaps {
            grouping: caps.has(Capabilities::DEPOSIT_GROUPING),
            cashback: caps.has(Capabilities::DEPOSIT_CASHBACK),
            vendor_refund: caps.has(Capabilities::VENDOR_REFUND),
        }
    }
}

/// A transfer between two balance-sheet accounts (`transfer`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferDraft {
    /// Money leaves this account.
    pub from: AccountId,
    /// Money arrives in this account.
    pub to: AccountId,
    /// Date.
    pub date: CalendarDate,
    /// Amount.
    pub amount: Money,
    /// Memo.
    pub memo: Option<String>,
}

/// A card charge or any other payment that is not a check (`expense` paid
/// from `pay_from`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardChargeDraft {
    /// The card (or other account) it is paid from.
    pub pay_from: AccountId,
    /// Date.
    pub date: CalendarDate,
    /// Reference number (receipt, confirmation).
    pub number: Option<String>,
    /// Payee.
    pub payee: Option<ContactId>,
    /// Memo.
    pub memo: Option<String>,
    /// Where the money went (the total is their sum).
    pub lines: Vec<SplitLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Check(CheckDraft),
    Deposit(DepositDraft),
    Transfer(TransferDraft),
    CardCharge(CardChargeDraft),
}

/// What kind of bank posting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BankPostingKind {
    /// A check.
    Check,
    /// A deposit.
    Deposit,
    /// A transfer.
    Transfer,
    /// A card charge (or other non-check payment).
    CardCharge,
}

/// A checked bank posting, ready to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BankPosting(Kind);

fn clean(mut lines: Vec<SplitLine>) -> Vec<SplitLine> {
    for l in &mut lines {
        l.memo = non_empty(l.memo.take());
    }
    lines
}

/// The split lines of a check or card charge (the caller has already
/// refused line customers and billable marks the engine can't take): their
/// exact total.
fn check_lines(lines: &[SplitLine], own: AccountId) -> Result<Money, LocalValidation> {
    if lines.is_empty() {
        return Err(LocalValidation::TooFewLines);
    }
    if lines.len() > MAX_LINES {
        return Err(LocalValidation::TooManyLines);
    }
    if lines.iter().any(|l| !l.amount.is_positive()) {
        return Err(LocalValidation::NonPositiveAmount);
    }
    if lines.iter().any(|l| l.account == own) {
        return Err(LocalValidation::SameAccount);
    }
    // Billable always needs the customer it is billable to.
    if lines.iter().any(|l| l.billable && l.contact.is_none()) {
        return Err(LocalValidation::InvalidValue);
    }
    Money::sum(lines.iter().map(|l| l.amount)).map_err(|_| LocalValidation::InvalidValue)
}

/// A text the engine stores as `null` when empty; `None` when it is blank.
fn blank_is_none(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

/// Longer than `max` as the engine's schema counts (UTF-16 code units).
fn too_long(s: Option<&String>, max: usize) -> bool {
    s.is_some_and(|v| v.encode_utf16().count() > max)
}

impl BankPosting {
    /// A check for an engine without `line-billable-v1`: no line customers
    /// and no billable lines ([`BankPosting::check_with`]).
    ///
    /// # Errors
    /// As [`BankPosting::check_with`].
    pub fn check(d: CheckDraft) -> Result<BankPosting, LocalValidation> {
        Self::check_with(d, false)
    }

    /// A check. `line_billable` is whether the engine advertises
    /// `line-billable-v1` (customers and billable marks on lines).
    ///
    /// # Errors
    /// `LineBillableUnavailable` (a line customer or billable mark without
    /// the capability), `TooFewLines`, `TooManyLines`, `NonPositiveAmount`,
    /// `TotalMismatch` (lines ≠ amount, exactly: the engine's own check is
    /// never relied on), `SameAccount` (a line to the bank itself),
    /// `InvalidValue` (blank or too long payee name, check number 0, a
    /// number together with Print later, a billable line without a
    /// customer, an address over 500 or a printed memo over 255
    /// characters, an overflowing total).
    pub fn check_with(
        mut d: CheckDraft,
        line_billable: bool,
    ) -> Result<BankPosting, LocalValidation> {
        if !line_billable && d.lines.iter().any(|l| l.contact.is_some() || l.billable) {
            return Err(LocalValidation::LineBillableUnavailable);
        }
        let total = check_lines(&d.lines, d.bank)?;
        if !d.amount.is_positive() {
            return Err(LocalValidation::NonPositiveAmount);
        }
        if total != d.amount {
            return Err(LocalValidation::TotalMismatch);
        }
        let name = d.payee_name.trim();
        if name.is_empty()
            || name.chars().count() > PAYEE_NAME_MAX
            || d.check_number == Some(0)
            || (d.print_later && d.check_number.is_some())
        {
            return Err(LocalValidation::InvalidValue);
        }
        d.payee_name = name.to_string();
        d.payee_address = blank_is_none(d.payee_address);
        d.printed_memo = blank_is_none(d.printed_memo);
        if too_long(d.payee_address.as_ref(), PAYEE_ADDRESS_MAX)
            || too_long(d.printed_memo.as_ref(), PRINTED_MEMO_MAX)
        {
            return Err(LocalValidation::InvalidValue);
        }
        let mut seen = std::collections::HashSet::new();
        d.tag_ids.retain(|t| seen.insert(*t));
        d.memo = non_empty(d.memo);
        d.lines = clean(d.lines);
        Ok(BankPosting(Kind::Check(d)))
    }

    /// A deposit for an engine without the P14 deposit capabilities: plain
    /// lines only ([`BankPosting::deposit_with`]).
    ///
    /// # Errors
    /// As [`BankPosting::deposit_with`].
    pub fn deposit(d: DepositDraft) -> Result<BankPosting, LocalValidation> {
        Self::deposit_with(d, DepositCaps::default())
    }

    /// A deposit. `caps` is what the engine advertises
    /// ([`DepositCaps::from_caps`]).
    ///
    /// # Errors
    /// Nothing is sent for:
    /// - `DepositGroupingUnavailable`, `VendorRefundUnavailable`,
    ///   `DepositCashBackUnavailable`: a part the engine does not take yet;
    /// - `TooFewLines` (no payment, line or fee), `TooManyLines` (more than
    ///   [`MAX_LINES`] parts, cash back included);
    /// - `NonPositiveAmount`: a part that is not greater than zero, or a
    ///   bank total (`Σpayments + Σlines − Σdeductions − cash back`) that is
    ///   not greater than zero;
    /// - `SameAccount`: a part or the cash back on the deposit account;
    /// - `InvalidValue`: the same source twice, a vendor credit without its
    ///   vendor, a billable line, a cash-back memo over 1000 characters, an
    ///   overflowing total.
    pub fn deposit_with(
        mut d: DepositDraft,
        caps: DepositCaps,
    ) -> Result<BankPosting, LocalValidation> {
        // What the engine takes at all comes first: nothing else matters
        // when a part can't be sent.
        if !caps.grouping && d.payments.iter().any(|p| p.kind == DepositSource::Payment) {
            return Err(LocalValidation::DepositGroupingUnavailable);
        }
        if !caps.vendor_refund
            && d.payments
                .iter()
                .any(|p| p.kind == DepositSource::VendorCredit)
        {
            return Err(LocalValidation::VendorRefundUnavailable);
        }
        if !caps.cashback && (!d.deductions.is_empty() || d.cash_back.is_some()) {
            return Err(LocalValidation::DepositCashBackUnavailable);
        }
        let parts = d.payments.len() + d.lines.len() + d.deductions.len();
        if parts == 0 {
            return Err(LocalValidation::TooFewLines);
        }
        if parts + usize::from(d.cash_back.is_some()) > MAX_LINES {
            return Err(LocalValidation::TooManyLines);
        }
        let split_parts = || d.lines.iter().chain(d.deductions.iter());
        let amounts_positive = d.payments.iter().all(|p| p.amount.is_positive())
            && split_parts().all(|l| l.amount.is_positive())
            && d.cash_back.as_ref().is_none_or(|c| c.amount.is_positive());
        if !amounts_positive {
            return Err(LocalValidation::NonPositiveAmount);
        }
        let own = d.account;
        if d.payments.iter().any(|p| p.account == own)
            || split_parts().any(|l| l.account == own)
            || d.cash_back.as_ref().is_some_and(|c| c.account == own)
        {
            return Err(LocalValidation::SameAccount);
        }
        let mut seen = std::collections::HashSet::new();
        if !d.payments.iter().all(|p| seen.insert(p.source))
            || d.payments
                .iter()
                .any(|p| p.kind == DepositSource::VendorCredit && p.contact.is_none())
            || split_parts().any(|l| l.billable)
        {
            return Err(LocalValidation::InvalidValue);
        }
        if let Some(c) = &mut d.cash_back {
            c.memo = blank_is_none(c.memo.take());
            if c.memo
                .as_ref()
                .is_some_and(|m| m.chars().count() > CASH_BACK_MEMO_MAX)
            {
                return Err(LocalValidation::InvalidValue);
            }
        }
        // The bank side, exactly; the engine refuses it at zero or below
        // (`DEPOSIT_TOTAL_NOT_POSITIVE`), so it is never sent that way.
        let total = d.total().ok_or(LocalValidation::InvalidValue)?;
        if !total.is_positive() {
            return Err(LocalValidation::NonPositiveAmount);
        }
        d.memo = non_empty(d.memo);
        for p in &mut d.payments {
            p.memo = non_empty(p.memo.take());
        }
        d.lines = clean(d.lines);
        d.deductions = clean(d.deductions);
        Ok(BankPosting(Kind::Deposit(d)))
    }

    /// A transfer.
    ///
    /// # Errors
    /// `NonPositiveAmount`, `SameAccount` (from = to).
    pub fn transfer(mut d: TransferDraft) -> Result<BankPosting, LocalValidation> {
        if !d.amount.is_positive() {
            return Err(LocalValidation::NonPositiveAmount);
        }
        if d.from == d.to {
            return Err(LocalValidation::SameAccount);
        }
        d.memo = non_empty(d.memo);
        Ok(BankPosting(Kind::Transfer(d)))
    }

    /// A card charge for an engine without `line-billable-v1`: no line
    /// customers and no billable lines ([`BankPosting::card_charge_with`]).
    ///
    /// # Errors
    /// As [`BankPosting::card_charge_with`].
    pub fn card_charge(d: CardChargeDraft) -> Result<BankPosting, LocalValidation> {
        Self::card_charge_with(d, false)
    }

    /// A card charge (or other payment that is not a check). `line_billable`
    /// is whether the engine advertises `line-billable-v1`: then a line may
    /// name its customer and be billable to it, as on a check.
    ///
    /// # Errors
    /// `LineBillableUnavailable` (a line customer or billable mark without
    /// the capability), `TooFewLines`, `TooManyLines`, `NonPositiveAmount`,
    /// `SameAccount`, `InvalidValue` (a billable line without a customer, a
    /// number over 50 characters, an overflowing total).
    pub fn card_charge_with(
        mut d: CardChargeDraft,
        line_billable: bool,
    ) -> Result<BankPosting, LocalValidation> {
        if !line_billable && d.lines.iter().any(|l| l.contact.is_some() || l.billable) {
            return Err(LocalValidation::LineBillableUnavailable);
        }
        check_lines(&d.lines, d.pay_from)?;
        d.number = non_empty(d.number.map(|n| n.trim().to_string()));
        if d.number.as_ref().is_some_and(|n| n.chars().count() > 50) {
            return Err(LocalValidation::InvalidValue);
        }
        d.memo = non_empty(d.memo);
        d.lines = clean(d.lines);
        Ok(BankPosting(Kind::CardCharge(d)))
    }

    /// Its kind.
    #[must_use]
    pub fn kind(&self) -> BankPostingKind {
        match &self.0 {
            Kind::Check(_) => BankPostingKind::Check,
            Kind::Deposit(_) => BankPostingKind::Deposit,
            Kind::Transfer(_) => BankPostingKind::Transfer,
            Kind::CardCharge(_) => BankPostingKind::CardCharge,
        }
    }

    /// Its date.
    #[must_use]
    pub fn date(&self) -> CalendarDate {
        match &self.0 {
            Kind::Check(d) => d.date,
            Kind::Deposit(d) => d.date,
            Kind::Transfer(d) => d.date,
            Kind::CardCharge(d) => d.date,
        }
    }

    /// The check, for a check posting.
    pub(crate) fn check_draft(&self) -> Option<&CheckDraft> {
        match &self.0 {
            Kind::Check(d) => Some(d),
            _ => None,
        }
    }

    /// The engine's transaction type.
    pub(crate) fn txn_type(&self) -> TxnType {
        match &self.0 {
            Kind::Check(_) | Kind::CardCharge(_) => TxnType::Expense,
            Kind::Deposit(_) => TxnType::Deposit,
            Kind::Transfer(_) => TxnType::Transfer,
        }
    }

    /// The bank-side account (the register's account).
    fn own_account(&self) -> AccountId {
        match &self.0 {
            Kind::Check(d) => d.bank,
            Kind::Deposit(d) => d.account,
            Kind::Transfer(d) => d.from,
            Kind::CardCharge(d) => d.pay_from,
        }
    }

    /// An expense body (`createExpenseSchema`): card charges, and checks on
    /// update.
    fn expense_body(
        date: CalendarDate,
        number: Option<&str>,
        payee: Option<ContactId>,
        pay_from: AccountId,
        memo: Option<&str>,
        lines: &[SplitLine],
    ) -> Value {
        let lines: Vec<Value> = lines
            .iter()
            .map(|l| {
                let mut v = json!({
                    "expenseAccountId": l.account,
                    "amount": l.amount.to_wire(),
                    "tagId": l.tag,
                });
                if let Some(m) = &l.memo {
                    v["description"] = Value::String(m.clone());
                }
                line_extras(&mut v, l);
                v
            })
            .collect();
        let mut v = json!({
            "txnType": "expense",
            "txnDate": date,
            "payFromAccountId": pay_from,
            "lines": lines,
        });
        if let Some(n) = number {
            v["txnNumber"] = Value::String(n.to_string());
        }
        if let Some(c) = payee {
            v["contactId"] = json!(c);
        }
        if let Some(m) = memo {
            v["memo"] = Value::String(m.to_string());
        }
        v
    }

    /// The deposit body (P14 shapes): source lines first (with
    /// `sourceTransactionId`), then the other lines, then the fees as
    /// negative amounts; the cash back in `cashBack`. Every amount is a
    /// `Money::to_wire()` string. The engine numbers the journal lines in
    /// this order after the bank line ([`DepositDraft`] fingerprint).
    fn deposit_body(d: &DepositDraft) -> Value {
        let line = |account: AccountId,
                    amount: String,
                    tag: Option<TagId>,
                    contact: Option<ContactId>,
                    memo: Option<&String>| {
            let mut v = json!({
                "accountId": account,
                "amount": amount,
                "tagId": tag,
                "contactId": contact,
            });
            if let Some(m) = memo {
                v["description"] = Value::String(m.clone());
            }
            v
        };
        let mut lines: Vec<Value> = d
            .payments
            .iter()
            .map(|p| {
                let mut v = line(
                    p.account,
                    p.amount.to_wire(),
                    None,
                    p.contact,
                    p.memo.as_ref(),
                );
                v["sourceTransactionId"] = json!(p.source);
                v
            })
            .collect();
        lines.extend(d.lines.iter().map(|l| {
            line(
                l.account,
                l.amount.to_wire(),
                l.tag,
                l.contact,
                l.memo.as_ref(),
            )
        }));
        lines.extend(d.deductions.iter().map(|l| {
            // The builder refused anything not greater than zero, so the
            // wire text has no sign of its own: the minus is exact.
            line(
                l.account,
                format!("-{}", l.amount.to_wire()),
                l.tag,
                l.contact,
                l.memo.as_ref(),
            )
        }));
        let mut v = json!({
            "txnType": "deposit",
            "txnDate": d.date,
            "depositToAccountId": d.account,
            "lines": lines,
        });
        if let Some(m) = &d.memo {
            v["memo"] = Value::String(m.clone());
        }
        if let Some(c) = &d.cash_back {
            let mut cb = json!({
                "accountId": c.account,
                "amount": c.amount.to_wire(),
            });
            if let Some(m) = &c.memo {
                cb["memo"] = Value::String(m.clone());
            }
            v["cashBack"] = cb;
        }
        v
    }

    fn transfer_body(d: &TransferDraft) -> Value {
        let mut v = json!({
            "txnType": "transfer",
            "txnDate": d.date,
            "fromAccountId": d.from,
            "toAccountId": d.to,
            "amount": d.amount.to_wire(),
        });
        if let Some(m) = &d.memo {
            v["memo"] = Value::String(m.clone());
        }
        v
    }

    /// The `POST /checks` body (`create`), or the `PUT /checks/{id}` body
    /// (the engine ignores a draft attachment on an edit, so none is sent).
    /// An edit always carries `tagIds` (possibly `[]`): the engine replaces
    /// the header tags with what is sent, so a removed tag goes away and
    /// an absent list would leave them as the ledger edit leaves them.
    fn check_body(d: &CheckDraft, create: bool) -> Value {
        let lines: Vec<Value> = d
            .lines
            .iter()
            .map(|l| {
                let mut v = json!({
                    "accountId": l.account,
                    "amount": l.amount.to_wire(),
                    "tagId": l.tag,
                });
                if let Some(m) = &l.memo {
                    v["description"] = Value::String(m.clone());
                }
                line_extras(&mut v, l);
                v
            })
            .collect();
        let mut v = json!({
            "bankAccountId": d.bank,
            "payeeNameOnCheck": d.payee_name,
            "txnDate": d.date,
            "amount": d.amount.to_wire(),
            "printLater": d.print_later,
            "lines": lines,
        });
        if let Some(c) = d.payee {
            v["contactId"] = json!(c);
        }
        if let Some(n) = d.check_number {
            v["checkNumber"] = json!(n);
        }
        if let Some(a) = &d.payee_address {
            v["payeeAddress"] = Value::String(a.clone());
        }
        if let Some(m) = &d.memo {
            v["memo"] = Value::String(m.clone());
        }
        if let Some(m) = &d.printed_memo {
            v["printedMemo"] = Value::String(m.clone());
        }
        if !create || !d.tag_ids.is_empty() {
            v["tagIds"] = json!(d.tag_ids);
        }
        if create && let Some(k) = d.draft_attachment {
            v["draftAttachmentId"] = Value::String(k.to_string());
        }
        v
    }

    /// The `PUT /checks/{id}` body (`check-edit-v1`); `None` for other
    /// kinds.
    pub(crate) fn check_put_value(&self) -> Option<Value> {
        self.check_draft().map(|d| Self::check_body(d, false))
    }

    /// The create body (`POST /checks` for a check, else `POST
    /// /transactions`).
    pub(crate) fn create_value(&self) -> Value {
        match &self.0 {
            Kind::Check(d) => Self::check_body(d, true),
            Kind::Deposit(d) => Self::deposit_body(d),
            Kind::Transfer(d) => Self::transfer_body(d),
            Kind::CardCharge(d) => Self::expense_body(
                d.date,
                d.number.as_deref(),
                d.payee,
                d.pay_from,
                d.memo.as_deref(),
                &d.lines,
            ),
        }
    }

    /// The `PUT /transactions/{id}` body (a check goes as an expense).
    pub(crate) fn update_value(&self) -> Value {
        match &self.0 {
            Kind::Check(d) => {
                Self::expense_body(d.date, None, d.payee, d.bank, d.memo.as_deref(), &d.lines)
            }
            _ => self.create_value(),
        }
    }

    /// The fingerprint of the record the engine stores for this posting
    /// (the [`Transaction`] editable fields). `update` follows the expense
    /// path for a check, where an empty line memo takes the header memo.
    fn fingerprint(&self, update: bool) -> [u8; 32] {
        fingerprint_of(&self.editable_value(update))
    }

    /// The [`Transaction`] editable fields of the record the engine stores
    /// for this posting (see [`BankPosting::fingerprint`]).
    pub(crate) fn editable_value(&self, update: bool) -> Value {
        // A debit split: a check's or a card charge's line (a customer and a
        // billable mark with `line-billable-v1`).
        let split = |l: &SplitLine, description: Option<String>| L {
            account: l.account,
            debit: l.amount,
            credit: Money::ZERO,
            description,
            tag: l.tag,
            contact: l.contact,
            billable: l.billable,
            source: None,
        };
        let total =
            |lines: &[SplitLine]| Money::sum(lines.iter().map(|l| l.amount)).unwrap_or_default();
        let side = L::side;
        // Expense path: an empty line description takes the header memo.
        let expense_lines = |lines: &[SplitLine], memo: Option<&String>, pay_from: AccountId| {
            let mut out: Vec<L> = lines
                .iter()
                .map(|l| split(l, l.memo.clone().or_else(|| memo.cloned())))
                .collect();
            out.push(side(pay_from, Money::ZERO, total(lines)));
            out
        };
        let (memo, contact, lines): (Option<&String>, Option<ContactId>, Vec<L>) = match &self.0 {
            Kind::Check(d) if update => (
                d.memo.as_ref(),
                d.payee,
                expense_lines(&d.lines, d.memo.as_ref(), d.bank),
            ),
            Kind::Check(d) => {
                let mut out: Vec<L> = d.lines.iter().map(|l| split(l, l.memo.clone())).collect();
                out.push(side(d.bank, Money::ZERO, d.amount));
                (d.memo.as_ref(), d.payee, out)
            }
            Kind::CardCharge(d) => (
                d.memo.as_ref(),
                d.payee,
                expense_lines(&d.lines, d.memo.as_ref(), d.pay_from),
            ),
            Kind::Deposit(d) => (d.memo.as_ref(), None, deposit_journal(d)),
            Kind::Transfer(d) => (
                d.memo.as_ref(),
                None,
                vec![
                    side(d.to, d.amount, Money::ZERO),
                    side(d.from, Money::ZERO, d.amount),
                ],
            ),
        };
        let lines: Vec<Value> = lines
            .iter()
            .enumerate()
            .map(|(i, l)| {
                with_source(
                    line_editable(
                        l.account,
                        &l.debit.to_wire(),
                        &l.credit.to_wire(),
                        l.description.as_deref(),
                        l.tag,
                        l.contact,
                        l.billable,
                        i64::try_from(i).unwrap_or(i64::MAX),
                    ),
                    l.source,
                )
            })
            .collect();
        txn_editable(
            self.txn_type(),
            self.date(),
            memo.map(String::as_str),
            Basis::Both,
            TxnStatus::Posted,
            contact,
            &lines,
        )
    }

    /// The fingerprint the record has once this posting is created (for a
    /// check also once `PUT /checks/{id}` stored it: that route writes the
    /// lines as the create does).
    #[must_use]
    pub fn created_fingerprint(&self) -> [u8; 32] {
        self.fingerprint(false)
    }

    /// The fingerprint the record has once an update with this posting
    /// commits.
    #[must_use]
    pub fn updated_fingerprint(&self) -> [u8; 32] {
        self.fingerprint(true)
    }
}

/// One journal line of the record the engine stores for a posting (the
/// fingerprinted fields).
struct L {
    account: AccountId,
    debit: Money,
    credit: Money,
    description: Option<String>,
    tag: Option<TagId>,
    contact: Option<ContactId>,
    billable: bool,
    source: Option<TransactionId>,
}

impl L {
    /// A bare line: an account and its side.
    fn side(account: AccountId, debit: Money, credit: Money) -> L {
        L {
            account,
            debit,
            credit,
            description: None,
            tag: None,
            contact: None,
            billable: false,
            source: None,
        }
    }
}

/// A deposit's journal as the engine writes it (deposit.service
/// planDeposit, PR #159): the bank debit of the exact total first, then each
/// body line in order (a source line credits its amount, a fee debits it),
/// then the cash-back debit described by its memo.
fn deposit_journal(d: &DepositDraft) -> Vec<L> {
    let mut out = vec![L::side(
        d.account,
        d.total().unwrap_or_default(),
        Money::ZERO,
    )];
    out.extend(d.payments.iter().map(|p| L {
        description: p.memo.clone(),
        contact: p.contact,
        source: Some(p.source),
        ..L::side(p.account, Money::ZERO, p.amount)
    }));
    let part = |l: &SplitLine, is_fee: bool| {
        let (debit, credit) = if is_fee {
            (l.amount, Money::ZERO)
        } else {
            (Money::ZERO, l.amount)
        };
        L {
            description: l.memo.clone(),
            tag: l.tag,
            contact: l.contact,
            ..L::side(l.account, debit, credit)
        }
    };
    out.extend(d.lines.iter().map(|l| part(l, false)));
    out.extend(d.deductions.iter().map(|l| part(l, true)));
    if let Some(c) = &d.cash_back {
        out.push(L {
            description: c.memo.clone(),
            ..L::side(c.account, c.amount, Money::ZERO)
        });
    }
    out
}

/// A check or expense line's customer and billable mark: sent only when
/// set (the builder refuses them without `line-billable-v1`).
fn line_extras(v: &mut Value, l: &SplitLine) {
    if let Some(c) = l.contact {
        v["contactId"] = json!(c);
    }
    if l.billable {
        v["isBillable"] = Value::Bool(true);
    }
}

/// Reads a save answer whose record sits under one of `keys` (a check's
/// create answers `{check}`; a reconciled one is the `{transaction}` read).
pub(crate) fn saved_any<T>(body: &[u8], keys: &[&'static str]) -> Result<Versioned<T>, ApiError>
where
    T: serde::de::DeserializeOwned + crate::resources::versioning::EditableFields,
{
    let v: Value = serde_json::from_slice(body).map_err(|_| ApiError::SavedUnreadable {
        what: "transaction",
    })?;
    let key = keys
        .iter()
        .find(|k| v.get(**k).is_some())
        .ok_or(ApiError::SavedUnreadable {
            what: "transaction",
        })?;
    saved_from(body, key)
}

impl Transactions<'_> {
    /// Records a check, deposit, transfer or card charge.
    ///
    /// # Errors
    /// `ClosedPeriod` (with `password_required`), `ClosingPasswordRejected`,
    /// `ClosingPasswordRateLimited`, `Local(ClosedPeriodOverrideUnavailable)`
    /// (nothing sent), `Validation`, `Conflict` (a check number already
    /// used), `OutcomeUnknown` when reconciliation cannot tell, or any
    /// [`ApiError`].
    pub async fn create_bank_posting(
        &self,
        posting: &BankPosting,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<Versioned<Transaction>, ApiError> {
        // One company for the stamp read, the probe and the write.
        let s = &self.0.pinned();
        let body = posting_body(s, posting.create_value(), opts)?;
        let matcher = OutcomeMatcher::Created {
            search: Search::Txn {
                kind: posting.txn_type(),
                date: posting.date(),
                account: Some(posting.own_account()),
            },
            fp: row_fp::<Transaction>,
            fingerprint: posting.created_fingerprint(),
        };
        let probe = Probe::new(s, matcher, true).await;
        let (op, item) = match posting.kind() {
            BankPostingKind::Check => (&CHECKS_CREATE, "check"),
            _ => (&TXNS_CREATE, "transaction"),
        };
        let mut call = Call::at(Method::Post, op.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(op, call, Some(&probe)).await?;
        // A reconciled answer is the `{transaction}` read, whatever the route.
        let keys = [item, "transaction"];
        for k in keys {
            remember_created(s, &reply.body, k);
        }
        saved_any(&reply.body, &keys)
    }

    /// Replaces a check, deposit, transfer or card charge if it is still at
    /// `expected` (fingerprint stopgap until U-17). The engine checks the
    /// closing date for both the old and the new date.
    ///
    /// # Errors
    /// `Conflict { Stale }` (nothing sent), `ClosedPeriod`,
    /// `ClosingPasswordRejected`, or as
    /// [`Transactions::create_bank_posting`].
    pub async fn update_bank_posting(
        &self,
        id: TransactionId,
        expected: &RecordVersion,
        posting: &BankPosting,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<Versioned<Transaction>, ApiError> {
        let body = posting_body(self.0, posting.update_value(), opts)?;
        let u = GuardedUpdate {
            get: &TXN_GET,
            put: &TXN_PUT,
            path: format!("/api/v1/transactions/{id}"),
            key: "transaction",
            body,
            idempotency_key: key,
            requested: posting.updated_fingerprint(),
            fp: row_fp::<Transaction>,
            stamp_covered: true,
        };
        update_guarded(self.0, expected, u).await
    }
}

#[cfg(test)]
#[path = "bank_postings_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "deposit_tests.rs"]
mod deposit_tests;
