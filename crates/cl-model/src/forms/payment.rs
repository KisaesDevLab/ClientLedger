//! Receive Payment's form state (P21 D3): what the owner chose, and the
//! engine's preview of it. No egui and no money arithmetic (Q-P20n
//! extended): every figure the screen shows, and every amount a save
//! sends, is copied from the engine's preview ([`PaymentFigures`]).
//!
//! - **Auto** (the default): the request carries the amount and no
//!   applications; the engine fills invoices oldest first.
//! - **Explicit**: the owner ticked, typed or un-applied. The request
//!   carries the rows the owner touched; a ticked row with no amount is a
//!   fill (`None`).
//! - Every edit that changes what the engine computes bumps
//!   [`PaymentForm::revision`]; a preview answer for an older revision is
//!   dropped, and Save waits for the current one (Q-P21s).

use std::collections::BTreeMap;
use std::fmt;

use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{BalanceToken, CalendarDate, Money, Timestamp};

/// How long the request must stand before it is sent (P20's debounce).
pub const PREVIEW_DEBOUNCE_MS: u64 = 250;

/// The payment amount the owner typed.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum AmountInput {
    /// Nothing typed: with ticked rows the engine sums them.
    #[default]
    Empty,
    /// The owner's amount.
    Typed(Money),
}

impl fmt::Debug for AmountInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AmountInput::Empty => "Empty",
            AmountInput::Typed(_) => "Typed(..)",
        })
    }
}

/// Who decides the allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    /// The engine, oldest first.
    #[default]
    Auto,
    /// The rows the owner touched.
    Explicit,
}

/// One row's choice in Explicit mode.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RowChoice {
    /// Not applied.
    Off,
    /// Ticked: the engine fills it.
    Fill,
    /// The owner's amount.
    Typed(Money),
}

impl fmt::Debug for RowChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RowChoice::Off => "Off",
            RowChoice::Fill => "Fill",
            RowChoice::Typed(_) => "Typed(..)",
        })
    }
}

/// An underpayment: leave the rest open (default, Q-P21a) or write it off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Under {
    /// The invoice stays open for the rest.
    #[default]
    LeaveOpen,
    /// The rest is written off to `account` (else the company's).
    WriteOff {
        /// The account picked on save, if any.
        account: Option<AccountId>,
    },
}

/// An overpayment: keep it as a credit (default, Q-P21b) or refund it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Over {
    /// A credit for the client's next invoice.
    #[default]
    KeepCredit,
    /// Refund it after saving (Q-P21m).
    Refund,
}

/// The answer to "Hold as a customer deposit?" (Q-P21e).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NothingApplied {
    /// Hold it in Customer Deposits.
    Hold,
    /// Keep it as a client credit.
    KeepCredit,
}

/// A credit or held deposit the owner chose to use.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CreditChoice {
    /// The credit or deposit.
    pub source: TransactionId,
    /// The invoice.
    pub invoice: TransactionId,
    /// The owner's amount, or `None`: the engine fills it.
    pub amount: Option<Money>,
}

impl fmt::Debug for CreditChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreditChoice")
            .field("source", &self.source)
            .field("invoice", &self.invoice)
            .finish_non_exhaustive()
    }
}

// ---- The engine's figures ----------------------------------------------------------

/// One open invoice as the engine's preview has it.
#[derive(Clone, PartialEq, Eq)]
pub struct FigureRow {
    /// The invoice.
    pub invoice: TransactionId,
    /// Its number.
    pub number: String,
    /// Its date.
    pub date: CalendarDate,
    /// Due.
    pub due: Option<CalendarDate>,
    /// Original amount.
    pub original: Money,
    /// Amount due before this payment.
    pub balance: Money,
    /// The stored balance to echo back.
    pub token: BalanceToken,
    /// Cash this payment puts on it.
    pub payment: Money,
    /// Left after it.
    pub remaining: Money,
    /// Discount taken.
    pub discount: Money,
    /// Written off.
    pub write_off: Money,
    /// Credits and deposits applied.
    pub credit: Money,
    /// The discount its terms allow today.
    pub discount_available: Money,
    /// The last day of the discount window.
    pub discount_date: Option<CalendarDate>,
    /// The terms' percent as the engine wrote it.
    pub discount_percent: Option<String>,
    /// The engine suggests the discount (Q-P21c).
    pub suggest_discount: bool,
}

impl fmt::Debug for FigureRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FigureRow")
            .field("invoice", &self.invoice)
            .field("date", &self.date)
            .field("suggest_discount", &self.suggest_discount)
            .finish_non_exhaustive()
    }
}

/// A partly paid invoice.
#[derive(Clone, PartialEq, Eq)]
pub struct FigureShortfall {
    /// The invoice.
    pub invoice: TransactionId,
    /// What stays open.
    pub amount: Money,
    /// Within the write-off limit: suggest writing it off (Q-P21h).
    pub within_limit: bool,
}

impl fmt::Debug for FigureShortfall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FigureShortfall")
            .field("invoice", &self.invoice)
            .field("within_limit", &self.within_limit)
            .finish_non_exhaustive()
    }
}

/// An open credit or a held deposit of the client.
#[derive(Clone, PartialEq, Eq)]
pub struct FigureCredit {
    /// The credit or deposit.
    pub source: TransactionId,
    /// Its date.
    pub date: CalendarDate,
    /// Its reference.
    pub ref_no: Option<String>,
    /// What is left.
    pub remaining: Money,
    /// A held deposit (else an overpayment credit).
    pub deposit: bool,
}

impl fmt::Debug for FigureCredit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FigureCredit")
            .field("source", &self.source)
            .field("deposit", &self.deposit)
            .finish_non_exhaustive()
    }
}

/// A credit or deposit used, with the engine's amount.
#[derive(Clone, PartialEq, Eq)]
pub struct FigureCreditUse {
    /// The credit or deposit.
    pub source: TransactionId,
    /// The invoice.
    pub invoice: TransactionId,
    /// How much.
    pub amount: Money,
}

impl fmt::Debug for FigureCreditUse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FigureCreditUse")
            .field("source", &self.source)
            .field("invoice", &self.invoice)
            .finish_non_exhaustive()
    }
}

/// Every figure of one preview (copied from the engine, never computed).
#[derive(Clone, PartialEq, Eq)]
pub struct PaymentFigures {
    /// The client's balance before.
    pub balance_before: Money,
    /// And after.
    pub balance_after: Money,
    /// The payment amount (typed, or the filled rows' sum).
    pub amount: Money,
    /// The amount is the filled rows' sum.
    pub amount_from_rows: bool,
    /// Every open invoice, oldest first.
    pub rows: Vec<FigureRow>,
    /// Totals: amount due.
    pub amount_due: Money,
    /// Applied.
    pub applied: Money,
    /// Discounts and credits.
    pub discounts_and_credits: Money,
    /// Unapplied.
    pub unapplied: Money,
    /// Written off.
    pub write_offs: Money,
    /// Partly paid invoices.
    pub shortfalls: Vec<FigureShortfall>,
    /// The overpayment.
    pub overpayment: Option<Money>,
    /// The client's credits and held deposits.
    pub credits: Vec<FigureCredit>,
    /// Credits and deposits used.
    pub credit_uses: Vec<FigureCreditUse>,
    /// Ask "Hold as a customer deposit?".
    pub hold_offered: bool,
    /// Payments waiting to be deposited after saving: (count, total).
    pub pending: Option<(u32, Money)>,
    /// Days the client usually takes to pay.
    pub average_days: Option<u32>,
    /// The engine's problem codes (with the invoice when it named one).
    pub problems: Vec<(String, Option<TransactionId>)>,
}

impl fmt::Debug for PaymentFigures {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentFigures")
            .field("rows", &self.rows.len())
            .field("shortfalls", &self.shortfalls.len())
            .field("problems", &self.problems)
            .finish_non_exhaustive()
    }
}

impl PaymentFigures {
    /// The row of `invoice`.
    #[must_use]
    pub fn row(&self, invoice: TransactionId) -> Option<&FigureRow> {
        self.rows.iter().find(|r| r.invoice == invoice)
    }
}

// ---- Requests the screen sends -----------------------------------------------------

/// One application of a preview request.
#[derive(Clone, PartialEq, Eq)]
pub struct RequestRow {
    /// The invoice.
    pub invoice: TransactionId,
    /// The owner's amount, `None` to fill.
    pub amount: Option<Money>,
    /// A discount.
    pub discount: Option<Money>,
    /// A write-off.
    pub write_off: Option<Money>,
}

impl fmt::Debug for RequestRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestRow")
            .field("invoice", &self.invoice)
            .finish_non_exhaustive()
    }
}

/// What the engine previews (the screen turns it into the API request).
#[derive(Clone, PartialEq, Eq)]
pub struct PaymentRequest {
    /// The client.
    pub client: ContactId,
    /// The date.
    pub date: CalendarDate,
    /// The typed amount.
    pub amount: Option<Money>,
    /// Where it goes.
    pub deposit_to: Option<AccountId>,
    /// `None` in Auto mode.
    pub rows: Option<Vec<RequestRow>>,
    /// Credits and deposits to use.
    pub credits: Vec<CreditChoice>,
    /// Hold as a customer deposit.
    pub hold: bool,
    /// The saved payment being edited.
    pub editing: Option<TransactionId>,
}

impl fmt::Debug for PaymentRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentRequest")
            .field("client", &self.client)
            .field("date", &self.date)
            .field("rows", &self.rows.as_ref().map(Vec::len))
            .finish_non_exhaustive()
    }
}

/// One application of a save, copied from the preview.
#[derive(Clone, PartialEq, Eq)]
pub struct SaveRow {
    /// The invoice.
    pub invoice: TransactionId,
    /// Cash.
    pub amount: Money,
    /// The preview's token for it.
    pub token: BalanceToken,
    /// Discount.
    pub discount: Option<Money>,
    /// Write-off.
    pub write_off: Option<Money>,
}

impl fmt::Debug for SaveRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SaveRow")
            .field("invoice", &self.invoice)
            .finish_non_exhaustive()
    }
}

/// A credit use of a save, copied from the preview.
#[derive(Clone, PartialEq, Eq)]
pub struct SaveCreditUse {
    /// The credit or deposit.
    pub source: TransactionId,
    /// The invoice.
    pub invoice: TransactionId,
    /// How much.
    pub amount: Money,
    /// The preview's token for the invoice.
    pub token: BalanceToken,
}

impl fmt::Debug for SaveCreditUse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SaveCreditUse")
            .field("source", &self.source)
            .field("invoice", &self.invoice)
            .finish_non_exhaustive()
    }
}

/// What a save sends: exactly what the owner saw (Q-P21s).
#[derive(Clone, PartialEq, Eq)]
pub struct PaymentSave {
    /// The client.
    pub client: ContactId,
    /// The date.
    pub date: CalendarDate,
    /// The amount (the preview's).
    pub amount: Money,
    /// Where it goes.
    pub deposit_to: AccountId,
    /// Method.
    pub method: Option<String>,
    /// Check or reference number.
    pub ref_no: Option<String>,
    /// Memo.
    pub memo: Option<String>,
    /// Applications.
    pub rows: Vec<SaveRow>,
    /// Credits and deposits used.
    pub credits: Vec<SaveCreditUse>,
    /// The write-off account picked (else the company's).
    pub write_off_account: Option<AccountId>,
    /// The discount account picked (else the company's).
    pub discount_account: Option<AccountId>,
    /// Hold as a customer deposit.
    pub hold: bool,
    /// The bank-file deposit it matches.
    pub feed_item: Option<String>,
}

impl fmt::Debug for PaymentSave {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentSave")
            .field("client", &self.client)
            .field("date", &self.date)
            .field("rows", &self.rows.len())
            .field("credits", &self.credits.len())
            .field("hold", &self.hold)
            .finish_non_exhaustive()
    }
}

/// Why Save is not possible yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaymentProblem {
    /// Nobody chosen (Received from).
    NoClient,
    /// No date.
    NoDate,
    /// No deposit account.
    NoDepositTo,
    /// Nothing received and nothing to settle.
    NoAmount,
    /// The preview of the current form is not back yet.
    Updating,
    /// The preview failed ("Can't calculate").
    CantCalculate,
    /// "Hold as a customer deposit?" is unanswered (Q-P21e).
    AskDeposit,
    /// A write-off with no account and no company default.
    WriteOffAccount,
    /// A discount with no account and no company default.
    DiscountAccount,
    /// The engine's problem code.
    Engine(String),
}

/// The company defaults a save falls back on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaymentDefaults {
    /// Write-offs post here.
    pub write_off_account: Option<AccountId>,
    /// Discounts post here.
    pub discount_account: Option<AccountId>,
}

/// The saved payment being edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Editing {
    /// The payment.
    pub id: TransactionId,
    /// Its `updatedAt` as read.
    pub updated_at: Timestamp,
}

/// The preview slot.
#[derive(Clone, PartialEq, Eq, Default)]
enum Slot {
    #[default]
    None,
    Pending(u64),
    Ready(u64, Box<PaymentFigures>),
    Failed(u64),
}

/// What the figures area shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FiguresView<'a> {
    /// Nothing to compute yet (no client or date).
    Nothing,
    /// "Updating…": never an old figure.
    Updating,
    /// "Can't calculate" (Save disabled).
    CantCalculate,
    /// The engine's figures for the form as it is.
    Ready(&'a PaymentFigures),
}

/// Which callouts the screen shows (from preview flags only).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Callouts {
    /// Underpayments: the shortfalls (Leave open / Write off).
    pub underpaid: Vec<FigureShortfall>,
    /// Write off is chosen and the rows carry it.
    pub writing_off: bool,
    /// The overpayment to keep or refund.
    pub overpayment: Option<Money>,
    /// Ask "Hold as a customer deposit?".
    pub ask_deposit: bool,
    /// Rows with a discount suggestion.
    pub discounts: Vec<TransactionId>,
}

/// Receive Payment's form.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct PaymentForm {
    revision: u64,
    /// The revision waiting to be sent and since when.
    asked: Option<(u64, u64)>,
    /// Received from.
    pub client: Option<ContactId>,
    /// Date.
    pub date: Option<CalendarDate>,
    amount: AmountInput,
    /// Method (`check`, `cash`, `card`, `bank_transfer`).
    pub method: String,
    /// Check or reference number.
    pub ref_no: String,
    deposit_to: Option<AccountId>,
    /// Memo.
    pub memo: String,
    mode: Mode,
    rows: BTreeMap<TransactionId, RowChoice>,
    discounts: BTreeMap<TransactionId, Money>,
    write_offs: BTreeMap<TransactionId, Money>,
    credits: Vec<CreditChoice>,
    under: Under,
    over: Over,
    nothing: Option<NothingApplied>,
    feed_item: Option<String>,
    preview: Slot,
    editing: Option<Editing>,
    /// The discount account picked on save.
    pub discount_account: Option<AccountId>,
}

impl fmt::Debug for PaymentForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentForm")
            .field("revision", &self.revision)
            .field("client", &self.client)
            .field("date", &self.date)
            .field("mode", &self.mode)
            .field("rows", &self.rows.len())
            .field("editing", &self.editing)
            .finish_non_exhaustive()
    }
}

impl PaymentForm {
    /// A new form dated `date`, deposited to `deposit_to` (Undeposited
    /// Funds by default, Q-202), method check.
    #[must_use]
    pub fn new(date: Option<CalendarDate>, deposit_to: Option<AccountId>) -> PaymentForm {
        PaymentForm {
            date,
            deposit_to,
            method: "check".into(),
            ..PaymentForm::default()
        }
    }

    fn bump(&mut self) {
        self.revision += 1;
    }

    /// The revision of what the engine computes.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Auto or Explicit.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The typed amount.
    #[must_use]
    pub fn amount(&self) -> AmountInput {
        self.amount
    }

    /// Where it goes.
    #[must_use]
    pub fn deposit_to(&self) -> Option<AccountId> {
        self.deposit_to
    }

    /// The underpayment choice.
    #[must_use]
    pub fn under(&self) -> Under {
        self.under
    }

    /// The overpayment choice.
    #[must_use]
    pub fn over(&self) -> Over {
        self.over
    }

    /// The deposit answer.
    #[must_use]
    pub fn nothing_applied(&self) -> Option<NothingApplied> {
        self.nothing
    }

    /// The saved payment being edited.
    #[must_use]
    pub fn editing(&self) -> Option<Editing> {
        self.editing
    }

    /// The linked bank-file deposit.
    #[must_use]
    pub fn feed_item(&self) -> Option<&str> {
        self.feed_item.as_deref()
    }

    /// The owner's choice on `invoice` (Explicit mode).
    #[must_use]
    pub fn row_choice(&self, invoice: TransactionId) -> RowChoice {
        self.rows.get(&invoice).copied().unwrap_or(RowChoice::Off)
    }

    /// Credits and deposits chosen.
    #[must_use]
    pub fn credits(&self) -> &[CreditChoice] {
        &self.credits
    }

    /// The owner chose who paid: everything else starts over (Auto).
    pub fn set_client(&mut self, client: Option<ContactId>) {
        if self.client == client {
            return;
        }
        self.client = client;
        self.mode = Mode::Auto;
        self.rows.clear();
        self.discounts.clear();
        self.write_offs.clear();
        self.credits.clear();
        self.under = Under::LeaveOpen;
        self.over = Over::KeepCredit;
        self.nothing = None;
        self.feed_item = None;
        self.bump();
    }

    /// The date.
    pub fn set_date(&mut self, date: Option<CalendarDate>) {
        if self.date != date {
            self.date = date;
            self.reset_choices();
            self.bump();
        }
    }

    /// The deposit account.
    pub fn set_deposit_to(&mut self, a: Option<AccountId>) {
        if self.deposit_to != a {
            self.deposit_to = a;
            self.bump();
        }
    }

    /// The amount (the mode stays).
    pub fn set_amount(&mut self, amount: AmountInput) {
        if self.amount != amount {
            self.amount = amount;
            self.reset_choices();
            self.bump();
        }
    }

    /// The write-off copies and the deposit answer belong to the figures
    /// they were made from: a new amount or date asks again.
    fn reset_choices(&mut self) {
        self.write_offs.clear();
        self.under = Under::LeaveOpen;
        self.nothing = None;
    }

    /// Explicit mode, keeping what the engine allocated so far: each row
    /// with cash in the ready preview becomes a typed row with that amount
    /// (a byte copy, nothing computed).
    fn go_explicit(&mut self) {
        if self.mode == Mode::Explicit {
            return;
        }
        let copies: Vec<(TransactionId, Money)> = match self.figures() {
            FiguresView::Ready(f) => f
                .rows
                .iter()
                .filter(|r| r.payment.is_positive())
                .map(|r| (r.invoice, r.payment))
                .collect(),
            _ => Vec::new(),
        };
        self.mode = Mode::Explicit;
        self.rows = copies
            .into_iter()
            .map(|(i, m)| (i, RowChoice::Typed(m)))
            .collect();
    }

    /// Space on a row: Off becomes Fill, anything else Off.
    pub fn toggle(&mut self, invoice: TransactionId) {
        self.go_explicit();
        let next = match self.row_choice(invoice) {
            RowChoice::Off => RowChoice::Fill,
            RowChoice::Fill | RowChoice::Typed(_) => RowChoice::Off,
        };
        self.rows.insert(invoice, next);
        if next == RowChoice::Off {
            self.discounts.remove(&invoice);
            self.write_offs.remove(&invoice);
        }
        self.reset_choices();
        self.bump();
    }

    /// The owner typed a Payment cell.
    pub fn type_payment(&mut self, invoice: TransactionId, amount: Money) {
        self.go_explicit();
        self.rows.insert(invoice, RowChoice::Typed(amount));
        self.reset_choices();
        self.bump();
    }

    /// Un-apply all (Alt+U): every row off.
    pub fn unapply_all(&mut self) {
        self.mode = Mode::Explicit;
        self.rows.clear();
        self.discounts.clear();
        self.write_offs.clear();
        self.credits.clear();
        self.reset_choices();
        self.bump();
    }

    /// Back to Auto (oldest first).
    pub fn auto_apply(&mut self) {
        self.mode = Mode::Auto;
        self.rows.clear();
        self.discounts.clear();
        self.write_offs.clear();
        self.reset_choices();
        self.bump();
    }

    /// Apply the engine's suggested discount on `invoice` (Q-P21c): a copy
    /// of its `discountAvailable`. False when the ready preview has none.
    pub fn apply_discount(&mut self, invoice: TransactionId) -> bool {
        let available = match self.figures() {
            FiguresView::Ready(f) => f
                .row(invoice)
                .map(|r| r.discount_available)
                .filter(|m| m.is_positive()),
            _ => None,
        };
        let Some(d) = available else {
            return false;
        };
        self.go_explicit();
        self.rows.entry(invoice).or_insert(RowChoice::Fill);
        self.discounts.insert(invoice, d);
        self.reset_choices();
        self.bump();
        true
    }

    /// The owner's own discount on `invoice` (`None` removes it).
    pub fn set_discount(&mut self, invoice: TransactionId, amount: Option<Money>) {
        self.go_explicit();
        self.rows.entry(invoice).or_insert(RowChoice::Fill);
        match amount {
            Some(m) => self.discounts.insert(invoice, m),
            None => self.discounts.remove(&invoice),
        };
        self.reset_choices();
        self.bump();
    }

    /// The discount on `invoice`, if any.
    #[must_use]
    pub fn discount(&self, invoice: TransactionId) -> Option<Money> {
        self.discounts.get(&invoice).copied()
    }

    /// Use a credit or held deposit on `invoice` (`amount: None`: the engine
    /// fills it).
    pub fn use_credit(
        &mut self,
        source: TransactionId,
        invoice: TransactionId,
        amount: Option<Money>,
    ) {
        self.credits
            .retain(|c| !(c.source == source && c.invoice == invoice));
        self.credits.push(CreditChoice {
            source,
            invoice,
            amount,
        });
        self.nothing = None;
        self.bump();
    }

    /// Stop using a credit.
    pub fn drop_credit(&mut self, source: TransactionId) {
        let before = self.credits.len();
        self.credits.retain(|c| c.source != source);
        if self.credits.len() != before {
            self.bump();
        }
    }

    /// Leave the shortfalls open (the default).
    pub fn leave_open(&mut self) {
        if self.under != Under::LeaveOpen || !self.write_offs.is_empty() {
            self.under = Under::LeaveOpen;
            self.write_offs.clear();
            self.bump();
        }
    }

    /// Write off every shortfall of the ready preview: copies of its
    /// amounts, the rows kept as the engine allocated them. False without
    /// a ready preview with shortfalls.
    pub fn write_off(&mut self, account: Option<AccountId>) -> bool {
        let shortfalls: Vec<(TransactionId, Money)> = match self.figures() {
            FiguresView::Ready(f) => f.shortfalls.iter().map(|s| (s.invoice, s.amount)).collect(),
            _ => Vec::new(),
        };
        if shortfalls.is_empty() {
            if let Under::WriteOff { .. } = self.under {
                self.under = Under::WriteOff { account };
                return true;
            }
            return false;
        }
        self.go_explicit();
        for (i, m) in shortfalls {
            self.rows.entry(i).or_insert(RowChoice::Fill);
            self.write_offs.insert(i, m);
        }
        self.under = Under::WriteOff { account };
        self.bump();
        true
    }

    /// Keep or refund an overpayment.
    pub fn set_over(&mut self, over: Over) {
        self.over = over;
    }

    /// Answer "Hold as a customer deposit?".
    pub fn answer_deposit(&mut self, a: NothingApplied) {
        if self.nothing != Some(a) {
            self.nothing = Some(a);
            self.bump();
        }
    }

    /// Link the bank-file deposit this payment matches (banner), with the
    /// bank it goes straight to.
    pub fn link_feed(&mut self, item: Option<String>, bank: Option<AccountId>) {
        self.feed_item = item;
        if let Some(b) = bank {
            self.set_deposit_to(Some(b));
        }
    }

    /// Start editing a saved payment: the form shows it in Explicit mode
    /// with its own live applications, the credits and deposits it used
    /// (D-B1: an unchanged save sends them back) and the accounts its
    /// write-offs and discounts went to.
    pub fn load_saved(&mut self, editing: Editing, loaded: LoadedPayment) {
        *self = PaymentForm::new(Some(loaded.date), loaded.deposit_to);
        self.client = Some(loaded.client);
        self.amount = AmountInput::Typed(loaded.amount);
        self.method = loaded.method;
        self.ref_no = loaded.ref_no;
        self.memo = loaded.memo;
        self.mode = Mode::Explicit;
        for (i, cash, discount, write_off) in loaded.rows {
            self.rows.insert(i, RowChoice::Typed(cash));
            if let Some(d) = discount {
                self.discounts.insert(i, d);
            }
            if let Some(w) = write_off {
                self.write_offs.insert(i, w);
                self.under = Under::WriteOff {
                    account: loaded.write_off_account,
                };
            }
        }
        for (source, invoice, amount) in loaded.credits {
            self.credits.push(CreditChoice {
                source,
                invoice,
                amount: Some(amount),
            });
        }
        self.discount_account = loaded.discount_account;
        if loaded.hold {
            self.nothing = Some(NothingApplied::Hold);
        }
        self.editing = Some(editing);
        self.bump();
    }

    /// The request for the engine (`None` without a client or date).
    #[must_use]
    pub fn to_preview_request(&self) -> Option<PaymentRequest> {
        let rows = match self.mode {
            Mode::Auto => None,
            Mode::Explicit => Some(
                self.rows
                    .iter()
                    .filter(|(_, c)| **c != RowChoice::Off)
                    .map(|(i, c)| RequestRow {
                        invoice: *i,
                        amount: match c {
                            RowChoice::Typed(m) => Some(*m),
                            RowChoice::Fill | RowChoice::Off => None,
                        },
                        discount: self.discounts.get(i).copied(),
                        write_off: self.write_offs.get(i).copied(),
                    })
                    .collect(),
            ),
        };
        Some(PaymentRequest {
            client: self.client?,
            date: self.date?,
            amount: match self.amount {
                AmountInput::Typed(m) => Some(m),
                AmountInput::Empty => None,
            },
            deposit_to: self.deposit_to,
            rows,
            credits: self.credits.clone(),
            hold: self.nothing == Some(NothingApplied::Hold),
            editing: self.editing.map(|e| e.id),
        })
    }

    /// The request to send now, if any: the current revision has stood for
    /// [`PREVIEW_DEBOUNCE_MS`] and was not sent yet. Marks it pending.
    pub fn poll_preview(&mut self, now_ms: u64) -> Option<(u64, PaymentRequest)> {
        let Some(req) = self.to_preview_request() else {
            self.preview = Slot::None;
            return None;
        };
        if let Slot::Pending(r) | Slot::Ready(r, _) | Slot::Failed(r) = &self.preview
            && *r == self.revision
        {
            return None;
        }
        match self.asked {
            Some((r, since)) if r == self.revision => {
                if now_ms.saturating_sub(since) < PREVIEW_DEBOUNCE_MS {
                    return None;
                }
            }
            _ => {
                self.asked = Some((self.revision, now_ms));
                // The first preview of a form goes at once.
                if self.preview != Slot::None {
                    return None;
                }
            }
        }
        self.preview = Slot::Pending(self.revision);
        Some((self.revision, req))
    }

    /// Applies an answer; an answer for an older revision is dropped.
    /// Returns true when applied.
    pub fn apply_preview(&mut self, revision: u64, figures: PaymentFigures) -> bool {
        if revision != self.revision {
            return false;
        }
        self.preview = Slot::Ready(revision, Box::new(figures));
        true
    }

    /// The preview of `revision` failed ("Can't calculate").
    pub fn fail_preview(&mut self, revision: u64) {
        if revision == self.revision {
            self.preview = Slot::Failed(revision);
        }
    }

    /// Asks for a new preview of the same form (Reload after 409
    /// `INVOICE_BALANCE_CHANGED`).
    pub fn refresh(&mut self) {
        self.bump();
    }

    /// True while a preview is in flight.
    #[must_use]
    pub fn in_flight(&self) -> bool {
        matches!(self.preview, Slot::Pending(r) if r == self.revision)
    }

    /// What the figures area shows: the engine's figures only for the form
    /// as it is now.
    #[must_use]
    pub fn figures(&self) -> FiguresView<'_> {
        if self.client.is_none() || self.date.is_none() {
            return FiguresView::Nothing;
        }
        match &self.preview {
            Slot::Ready(r, f) if *r == self.revision => FiguresView::Ready(f),
            Slot::Failed(r) if *r == self.revision => FiguresView::CantCalculate,
            _ => FiguresView::Updating,
        }
    }

    /// The callouts to show (preview flags only).
    #[must_use]
    pub fn callouts(&self) -> Callouts {
        let FiguresView::Ready(f) = self.figures() else {
            return Callouts {
                writing_off: !self.write_offs.is_empty(),
                ..Callouts::default()
            };
        };
        Callouts {
            underpaid: f.shortfalls.clone(),
            writing_off: !self.write_offs.is_empty(),
            overpayment: f.overpayment.filter(|_| !self.is_hold()),
            ask_deposit: f.hold_offered,
            discounts: f
                .rows
                .iter()
                .filter(|r| r.suggest_discount)
                .map(|r| r.invoice)
                .collect(),
        }
    }

    fn is_hold(&self) -> bool {
        self.nothing == Some(NothingApplied::Hold)
    }

    /// What Save sends, copied from the ready preview of this revision, or
    /// why it can't.
    ///
    /// # Errors
    /// Every [`PaymentProblem`] that applies.
    #[allow(clippy::too_many_lines)]
    pub fn save_plan(&self, defaults: PaymentDefaults) -> Result<PaymentSave, Vec<PaymentProblem>> {
        let mut problems = Vec::new();
        if self.client.is_none() {
            problems.push(PaymentProblem::NoClient);
        }
        if self.date.is_none() {
            problems.push(PaymentProblem::NoDate);
        }
        if self.deposit_to.is_none() {
            problems.push(PaymentProblem::NoDepositTo);
        }
        let f = match self.figures() {
            FiguresView::Ready(f) => Some(f),
            FiguresView::CantCalculate => {
                problems.push(PaymentProblem::CantCalculate);
                None
            }
            FiguresView::Updating => {
                problems.push(PaymentProblem::Updating);
                None
            }
            FiguresView::Nothing => None,
        };
        if let Some(f) = f {
            // Credits alone are not a receipt: the engine refuses a payment
            // of 0 that only uses credits (400 AMOUNT_REQUIRED); they go
            // through Apply on the credit (review D should-fix 3).
            let settles = f
                .rows
                .iter()
                .any(|r| r.discount.is_positive() || r.write_off.is_positive());
            if !f.amount.is_positive() && !settles {
                problems.push(PaymentProblem::NoAmount);
            }
            if f.hold_offered && self.nothing.is_none() {
                problems.push(PaymentProblem::AskDeposit);
            }
            let writes_off = f.rows.iter().any(|r| r.write_off.is_positive());
            let account = match self.under {
                Under::WriteOff { account } => account,
                Under::LeaveOpen => None,
            };
            if writes_off && account.is_none() && defaults.write_off_account.is_none() {
                problems.push(PaymentProblem::WriteOffAccount);
            }
            let discounts = f.rows.iter().any(|r| r.discount.is_positive());
            if discounts && self.discount_account.is_none() && defaults.discount_account.is_none() {
                problems.push(PaymentProblem::DiscountAccount);
            }
            problems.extend(
                f.problems
                    .iter()
                    .map(|(c, _)| PaymentProblem::Engine(c.clone())),
            );
        }
        let (Some(f), Some(client), Some(date), Some(deposit_to), true) = (
            f,
            self.client,
            self.date,
            self.deposit_to,
            problems.is_empty(),
        ) else {
            return Err(problems);
        };
        let token_of = |i: TransactionId| f.row(i).map(|r| r.token.clone());
        let mut credits = Vec::new();
        for c in &f.credit_uses {
            let Some(token) = token_of(c.invoice) else {
                return Err(vec![PaymentProblem::Engine(
                    "INVOICE_NOT_FOR_CUSTOMER".into(),
                )]);
            };
            credits.push(SaveCreditUse {
                source: c.source,
                invoice: c.invoice,
                amount: c.amount,
                token,
            });
        }
        let text = |s: &str| {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        };
        Ok(PaymentSave {
            client,
            date,
            amount: f.amount,
            deposit_to,
            method: text(&self.method),
            ref_no: text(&self.ref_no),
            memo: text(&self.memo),
            rows: f
                .rows
                .iter()
                .filter(|r| {
                    r.payment.is_positive() || r.discount.is_positive() || r.write_off.is_positive()
                })
                .map(|r| SaveRow {
                    invoice: r.invoice,
                    amount: r.payment,
                    token: r.token.clone(),
                    discount: r.discount.is_positive().then_some(r.discount),
                    write_off: r.write_off.is_positive().then_some(r.write_off),
                })
                .collect(),
            credits,
            write_off_account: match self.under {
                Under::WriteOff { account } => account,
                Under::LeaveOpen => None,
            },
            discount_account: self.discount_account,
            hold: self.is_hold(),
            feed_item: self.feed_item.clone(),
        })
    }
}

/// A saved payment as the form loads it.
#[derive(Clone, PartialEq, Eq)]
pub struct LoadedPayment {
    /// The client.
    pub client: ContactId,
    /// Date.
    pub date: CalendarDate,
    /// Amount.
    pub amount: Money,
    /// Where it went (`None` for a payment of 0 that only wrote off).
    pub deposit_to: Option<AccountId>,
    /// Method.
    pub method: String,
    /// Reference.
    pub ref_no: String,
    /// Memo.
    pub memo: String,
    /// Its own live applications: invoice, cash, discount, write-off.
    pub rows: Vec<(TransactionId, Money, Option<Money>, Option<Money>)>,
    /// Credits and held deposits applied inside it: source, invoice,
    /// amount.
    pub credits: Vec<(TransactionId, TransactionId, Money)>,
    /// The account its write-offs went to.
    pub write_off_account: Option<AccountId>,
    /// The account its discounts went to.
    pub discount_account: Option<AccountId>,
    /// A held deposit.
    pub hold: bool,
}

impl fmt::Debug for LoadedPayment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoadedPayment")
            .field("client", &self.client)
            .field("date", &self.date)
            .field("rows", &self.rows.len())
            .field("credits", &self.credits.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[path = "payment_tests.rs"]
mod tests;
