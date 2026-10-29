//! The Enter Bill form model (P17, plan §3 WP3): one checked form for a bill
//! and a vendor credit.
//!
//! The Bill / Vendor credit radio ([`BillKind`]) switches the posting route
//! and the labels, not the screen. [`check`] is the one exact gate before
//! anything is sent:
//!
//! - the lines (expenses + items) must add up to the amount due **exactly**
//!   (one cent off refuses the save; no tolerance);
//! - a blank amount due takes the lines total (as Write Check);
//! - item amounts are `rate × qty` rounded half away from zero to cents
//!   ([`crate::forms::item_lines::item_amount`], D6);
//! - fields read from a document must be accepted first (non-negotiable 4);
//! - a credit carries no terms, due date or discount: the
//!   values stay on the form (switching back restores them) but
//!   [`CheckedBill`] leaves them out.
//!
//! Due and discount dates: the engine's `terms-dates` (with
//! `bill-terms-v1`) or the local calendar preview ([`preview_dates`])
//! fill them only while the owner has not overridden them; the override
//! flags (process review S3) say which dates the owner typed, and only those
//! are sent.
//!
//! Duplicate refs (D3, Q-P17n): [`duplicate_window`] is the bill date ± 18
//! calendar months (inclusive, the day clamped to the month end) and
//! [`ref_matches`] compares trimmed, case-folded text; [`find_duplicates`]
//! filters engine rows with both.
//!
//! Money is exact (`cl_core::Money`); no floats in this crate.

use std::fmt;

use cl_core::ids::{AccountId, ContactId, ItemId, TagId, TransactionId};
use cl_core::money::UserInput;
use cl_core::{CalendarDate, Money, Quantity};

use crate::forms::PayeePrefill;
use crate::forms::check::{AttachmentRef, line_tag_default};
use crate::forms::doc_read::{ReadState, ReadTarget};
use crate::forms::item_lines::{ItemAmountError, ItemLine, ItemLines, items_total};
use crate::forms::lines::{FormLine, FormLines, PostingLine, TotalsStatus, lines_total, trimmed};
use crate::lists::tags::TagList;
use crate::lists::terms::TERMS;
use crate::rowkey::{RowKey, RowKeyGen};
use crate::text::TextKey;

/// Most lines one bill takes, expenses and items together (the engine's
/// maximum).
pub const MAX_LINES: usize = 500;
/// Longest Ref. no. (vendor invoice number), in characters.
pub const REF_MAX: usize = 100;
/// Most days a custom term may run.
pub const TERMS_DAYS_MAX: u16 = 999;
/// Most discount days (U-86: 0..=365, and fewer than the net days).
pub const DISCOUNT_DAYS_MAX: u16 = 365;
/// The duplicate-ref window on each side of the bill date (Q-P17n).
pub const DUPLICATE_WINDOW_MONTHS: i32 = 18;
/// The engine code of the custom term.
pub const CUSTOM_TERMS: &str = "custom";

/// What the form saves: a bill or a vendor credit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BillKind {
    /// A bill (`POST /bills`): A/P goes up.
    #[default]
    Bill,
    /// A vendor credit (`POST /vendor-credits`): no terms or due date.
    Credit,
}

/// An early-payment discount percentage: more than 0 and less than 100, at
/// most four decimals (`2` is 2%). Exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Percent(Quantity);

impl Percent {
    /// `value` when it is more than 0 and less than 100.
    #[must_use]
    pub fn new(value: Quantity) -> Option<Percent> {
        let hundred = Quantity::from_ten_thousandths(100 * 10_000).ok()?;
        (value.is_positive() && value < hundred).then_some(Percent(value))
    }

    /// What the owner typed (`2`, `1.5`, at most four decimals; no `%`).
    #[must_use]
    pub fn parse_user(text: &str) -> Option<Percent> {
        let t = text.trim();
        let t = t.strip_suffix('%').unwrap_or(t);
        Quantity::parse_user(t, UserInput::FULL)
            .ok()
            .and_then(Percent::new)
    }

    /// The exact value.
    #[must_use]
    pub fn value(self) -> Quantity {
        self.0
    }

    /// Canonical engine text (`2.0000`).
    #[must_use]
    pub fn to_wire(self) -> String {
        self.0.to_wire()
    }
}

/// One payment term as the engine lists it (`GET /payment-terms` with
/// `bill-terms-v1`, otherwise [`PaymentTerm::fixed`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentTerm {
    /// Engine code (`net_30`, `2_10_net_30`, `custom`).
    pub code: String,
    /// Days until due; `None` for the custom term (the form gives them).
    pub net_days: Option<u16>,
    /// Discount percentage of a fixed discount term.
    pub discount_percent: Option<Percent>,
    /// Discount days of a fixed discount term.
    pub discount_days: Option<u16>,
}

impl PaymentTerm {
    /// The terms the engine accepts before `bill-terms-v1` (the P11 fixed
    /// list, no discount terms).
    #[must_use]
    pub fn fixed() -> Vec<PaymentTerm> {
        TERMS
            .iter()
            .map(|t| PaymentTerm {
                code: t.wire.to_string(),
                net_days: t.due_days,
                discount_percent: None,
                discount_days: None,
            })
            .collect()
    }

    /// True for the custom term.
    #[must_use]
    pub fn is_custom(&self) -> bool {
        self.code == CUSTOM_TERMS
    }
}

/// Due and discount dates for a term.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TermsDates {
    /// Due date.
    pub due: Option<CalendarDate>,
    /// Last day for the early-payment discount.
    pub discount: Option<CalendarDate>,
}

/// The local preview of the due and discount dates (before `bill-terms-v1`
/// the engine's figure replaces it after the save): plain calendar
/// arithmetic, `date + days`, no time zone. For the custom term the form's
/// days and discount days are used.
#[must_use]
pub fn preview_dates(
    date: CalendarDate,
    term: &PaymentTerm,
    custom_days: Option<u16>,
    custom_discount_days: Option<u16>,
) -> TermsDates {
    let (net, disc) = if term.is_custom() {
        (custom_days, custom_discount_days)
    } else {
        (term.net_days, term.discount_days)
    };
    let add = |d: u16| date.add_days(i64::from(d)).ok();
    TermsDates {
        due: net.and_then(add),
        discount: disc.and_then(add),
    }
}

/// The Enter Bill form as typed. `Debug` shows which fields are filled,
/// never their contents.
#[derive(Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent owner choices and S3 override flags
pub struct BillForm {
    /// Bill or vendor credit.
    pub kind: BillKind,
    /// Vendor.
    pub vendor: Option<ContactId>,
    /// Bill date.
    pub date: Option<CalendarDate>,
    /// Ref. no. (the vendor's invoice number).
    pub reference: String,
    /// Payment term code; `None` uses the vendor's or the company's default
    /// in the engine.
    pub terms: Option<String>,
    /// Days of a custom term.
    pub terms_days: Option<u16>,
    /// Discount percentage of a custom term (`bill-terms-v1`).
    pub discount_percent: Option<Percent>,
    /// Discount days of a custom term (`bill-terms-v1`).
    pub discount_days: Option<u16>,
    /// Amount due as typed (or the lines total, see
    /// [`BillForm::leave_lines`]).
    pub amount_due: Option<Money>,
    /// `amount_due` was taken from the lines total, not typed.
    pub amount_from_lines: bool,
    /// Bill due (from the terms, or the owner's).
    pub due_date: Option<CalendarDate>,
    /// The owner typed the due date (S3): it is sent and terms no longer
    /// move it.
    pub due_overridden: bool,
    /// Discount date (from the terms, or the owner's).
    pub discount_date: Option<CalendarDate>,
    /// The owner typed the discount date (S3).
    pub discount_overridden: bool,
    /// Memo.
    pub memo: String,
    /// Header tags, one per one-per-group group (Q-P13g, Q-178).
    pub header_tags: Vec<TagId>,
    /// The Expenses tab.
    pub expenses: FormLines,
    /// The Items tab.
    pub items: ItemLines,
    /// Files uploaded before the first save (drafts).
    pub attachments: Vec<AttachmentRef>,
    /// Fields read from a document and whether they were accepted.
    pub read: ReadState,
}

impl Default for BillForm {
    fn default() -> Self {
        BillForm {
            kind: BillKind::Bill,
            vendor: None,
            date: None,
            reference: String::new(),
            terms: None,
            terms_days: None,
            discount_percent: None,
            discount_days: None,
            amount_due: None,
            amount_from_lines: false,
            due_date: None,
            due_overridden: false,
            discount_date: None,
            discount_overridden: false,
            memo: String::new(),
            header_tags: Vec::new(),
            expenses: FormLines::default(),
            items: ItemLines::default(),
            attachments: Vec::new(),
            read: ReadState::default(),
        }
    }
}

impl fmt::Debug for BillForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillForm")
            .field("kind", &self.kind)
            .field("vendor", &self.vendor.is_some())
            .field("date", &self.date.is_some())
            .field("reference", &!self.reference.is_empty())
            .field("terms", &self.terms.is_some())
            .field("terms_days", &self.terms_days.is_some())
            .field("discount_percent", &self.discount_percent.is_some())
            .field("discount_days", &self.discount_days.is_some())
            .field("amount_due", &self.amount_due.is_some())
            .field("amount_from_lines", &self.amount_from_lines)
            .field("due_date", &self.due_date.is_some())
            .field("due_overridden", &self.due_overridden)
            .field("discount_date", &self.discount_date.is_some())
            .field("discount_overridden", &self.discount_overridden)
            .field("memo", &!self.memo.is_empty())
            .field("header_tags", &self.header_tags.len())
            .field("expenses", &self.expenses.lines.len())
            .field("items", &self.items.lines.len())
            .field("attachments", &self.attachments.len())
            .field("read", &!self.read.is_empty())
            .finish()
    }
}

impl BillForm {
    /// A blank bill dated `date`, with one blank entry line on
    /// each tab.
    #[must_use]
    pub fn new(date: Option<CalendarDate>, keys: &mut RowKeyGen) -> BillForm {
        BillForm {
            date,
            expenses: FormLines::from_lines(Vec::new(), keys),
            items: ItemLines::from_lines(Vec::new(), keys),
            ..BillForm::default()
        }
    }

    /// The exact lines total, expenses plus items. `None` when an item's
    /// quantity × rate cannot be computed or the sum leaves the money range.
    #[must_use]
    pub fn lines_total(&self) -> Option<Money> {
        lines_total(&self.expenses)?
            .checked_add(items_total(&self.items)?)
            .ok()
    }

    /// The owner typed the amount due; it is no longer "from the lines".
    pub fn set_amount_due(&mut self, amount: Option<Money>) {
        self.amount_due = amount;
        self.amount_from_lines = false;
    }

    /// The owner left the lines (as Write Check, Q-P13r): a blank amount due
    /// takes the lines total and is flagged; an amount that came from the
    /// lines follows them. A typed amount never changes on its own.
    pub fn leave_lines(&mut self) {
        if self.amount_due.is_some() && !self.amount_from_lines {
            return;
        }
        let any = self.expenses.lines.iter().any(|l| l.amount.is_some())
            || self.items.lines.iter().any(|l| l.amount.is_some());
        if any {
            if let Some(total) = self.lines_total() {
                self.amount_due = Some(total);
                self.amount_from_lines = true;
            }
        } else if self.amount_from_lines {
            self.amount_due = None;
            self.amount_from_lines = false;
        }
    }

    /// The footer ("Lines total = amount due ✓"): the amount due against the
    /// lines total. `None` without an amount due or when the total does not
    /// fit.
    #[must_use]
    pub fn totals_status(&self) -> Option<TotalsStatus> {
        let left = self.amount_due?.checked_sub(self.lines_total()?).ok()?;
        Some(if left.is_zero() {
            TotalsStatus::Balanced
        } else if left.is_positive() {
            TotalsStatus::Remaining(left)
        } else {
            TotalsStatus::Over(left.abs())
        })
    }

    /// Dates from the terms (the engine's `terms-dates` or
    /// [`preview_dates`]): each fills only while the owner has not typed it.
    pub fn apply_terms_dates(&mut self, dates: TermsDates) {
        if !self.due_overridden {
            self.due_date = dates.due;
        }
        if !self.discount_overridden {
            self.discount_date = dates.discount;
        }
    }

    /// The owner typed the due date. Clearing it hands the field back to
    /// the terms (the next [`BillForm::apply_terms_dates`] fills it).
    pub fn set_due_by_owner(&mut self, date: Option<CalendarDate>) {
        self.due_date = date;
        self.due_overridden = date.is_some();
    }

    /// The owner typed the discount date (cleared: back to the terms).
    pub fn set_discount_by_owner(&mut self, date: Option<CalendarDate>) {
        self.discount_date = date;
        self.discount_overridden = date.is_some();
    }

    /// True while the form is untouched enough for vendor prefill (Q-P13q):
    /// no line used on either tab and no memo typed.
    #[must_use]
    pub fn can_prefill(&self) -> bool {
        self.expenses.used().next().is_none()
            && self.items.used().next().is_none()
            && self.memo.trim().is_empty()
    }

    /// The vendor's default expense account and tag (Q-192) fill the first
    /// expense line only when no line is used; a typed amount due follows
    /// onto it. Returns true when it filled.
    pub fn apply_vendor_default(
        &mut self,
        account: AccountId,
        tag: Option<TagId>,
        keys: &mut RowKeyGen,
    ) -> bool {
        if self.expenses.used().next().is_some() || self.items.used().next().is_some() {
            return false;
        }
        if self.expenses.lines.is_empty() {
            self.expenses.lines.push(FormLine::blank(keys.next_key()));
        }
        let Some(first) = self.expenses.lines.first_mut() else {
            return false;
        };
        first.account = Some(account);
        first.tag = tag;
        first.amount = if self.amount_from_lines {
            None
        } else {
            self.amount_due
        };
        self.expenses.ensure_entry(keys);
        true
    }
}

/// Copies the vendor's newest bill (Q-191, plan WP4): expense and item lines
/// with their accounts, items, descriptions, customers, billable and tags,
/// plus the memo, terms and header tags. Every amount, quantity and rate is
/// cleared and the amount due is not touched (non-negotiable 4). `last` must
/// have been built with this form's [`RowKeyGen`].
impl PayeePrefill for BillForm {
    fn prefill_from_last(&mut self, last: &BillForm) {
        let entry = self.expenses.lines.last().filter(|l| l.is_blank()).cloned();
        let mut lines: Vec<FormLine> = last
            .expenses
            .lines
            .iter()
            .map(|l| FormLine {
                amount: None,
                ..l.clone()
            })
            .filter(|l| !l.is_blank())
            .collect();
        lines.extend(entry);
        self.expenses = FormLines { lines };
        let entry = self.items.lines.last().filter(|l| l.is_blank()).cloned();
        let mut items: Vec<ItemLine> = last
            .items
            .lines
            .iter()
            .map(|l| ItemLine {
                qty: None,
                rate: None,
                amount: None,
                ..l.clone()
            })
            .filter(|l| !l.is_blank())
            .collect();
        items.extend(entry);
        self.items = ItemLines { lines: items };
        self.memo.clone_from(&last.memo);
        self.terms.clone_from(&last.terms);
        self.terms_days = last.terms_days;
        self.discount_percent = last.discount_percent;
        self.discount_days = last.discount_days;
        self.header_tags.clone_from(&last.header_tags);
    }
}

/// Engine capabilities the checks depend on (cl-api capability names).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct BillCaps {
    /// `ap-closing-v1`: closed-period bills save with the closing password.
    pub closing: bool,
    /// `bill-lines-v1`: item lines, customer, billable and credit line tags.
    pub lines: bool,
    /// `bill-terms-v1`: discount terms and the discount date.
    pub terms: bool,
    /// `bill-received-v1`: the engine takes `billReceived` (always sent
    /// true: "Not received yet" was removed, Q-P18h).
    pub received: bool,
}

/// What a bill is checked against.
#[derive(Debug, Clone, Copy)]
pub struct BillContext<'a> {
    /// The tags (for the header-tag defaults).
    pub tags: &'a TagList,
    /// The payment terms the engine accepts.
    pub terms: &'a [PaymentTerm],
    /// The company's closing date, when set.
    pub closing_date: Option<CalendarDate>,
    /// The saved date of the bill being edited (moving a bill out of a
    /// closed period needs the password too).
    pub original_date: Option<CalendarDate>,
    /// Engine capabilities.
    pub caps: BillCaps,
}

/// The field a problem belongs to (the screen focuses it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BillField {
    /// Vendor.
    Vendor,
    /// Date.
    Date,
    /// Ref. no.
    Reference,
    /// Terms (and the custom days).
    Terms,
    /// The discount fields.
    Discount,
    /// Bill due.
    DueDate,
    /// Discount date.
    DiscountDate,
    /// Amount due.
    AmountDue,
    /// The lines as a whole.
    Lines,
    /// The Items tab.
    Items,
    /// One line's account.
    LineAccount(RowKey),
    /// One line's amount.
    LineAmount(RowKey),
    /// One line's customer (and billable).
    LineCustomer(RowKey),
    /// One item line's quantity.
    LineQty(RowKey),
    /// One item line's rate (cost).
    LineRate(RowKey),
    /// The read-from-document banner (Accept these values).
    ReadBanner,
}

/// Why a bill or credit cannot be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BillProblem {
    /// No vendor.
    VendorMissing,
    /// No date.
    DateMissing,
    /// The date is on or before the closing date and the engine cannot take
    /// the closing password yet (no `ap-closing-v1`).
    ClosedPeriodNeedsUpdate,
    /// Ref. no. is longer than [`REF_MAX`] characters (UTF-16 units, as the
    /// engine counts them).
    RefTooLong,
    /// The term is not one the engine accepts.
    TermsUnknown,
    /// A custom term needs its days (0 to [`TERMS_DAYS_MAX`]).
    TermsDaysInvalid,
    /// Discount percentage and days go together, the days are at most
    /// [`DISCOUNT_DAYS_MAX`] and fewer than the net days.
    DiscountInvalid,
    /// Discount terms and dates need `bill-terms-v1`.
    DiscountNeedsUpdate,
    /// The due date is before the bill date.
    DueBeforeDate,
    /// The discount date is before the bill date or after the due date.
    DiscountDateInvalid,
    /// No amount due and no lines.
    AmountMissing,
    /// The amount due is zero or less.
    AmountNotPositive,
    /// No line is used.
    NoLines,
    /// More than [`MAX_LINES`] lines.
    TooManyLines,
    /// The lines total leaves the money range.
    LinesTotalTooLarge,
    /// The lines don't add up to the amount due; `difference` is the amount
    /// due minus the lines total (negative when the lines are more).
    LinesDisagree {
        /// Amount due − lines total, exact.
        difference: Money,
    },
    /// A used line has no account.
    LineAccountMissing(RowKey),
    /// A used line's amount is blank, zero or less.
    LineAmountNotPositive(RowKey),
    /// A billable line has no customer.
    BillableNeedsCustomer(RowKey),
    /// A line has a customer, billable or (on a credit) a tag, which needs
    /// `bill-lines-v1`.
    LineDetailNeedsUpdate(RowKey),
    /// Item lines need `bill-lines-v1`.
    ItemsNeedUpdate,
    /// An item line has a rate but no quantity.
    ItemRateNeedsQty(RowKey),
    /// An item line's quantity is zero or less.
    ItemQtyNotPositive(RowKey),
    /// An item line's rate is below zero.
    ItemRateNegative(RowKey),
    /// An item line's quantity × rate is too large.
    ItemAmountTooLarge(RowKey),
    /// Fields read from the document are not accepted yet
    /// (non-negotiable 4).
    ReadNotAccepted,
}

impl BillProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            BillProblem::VendorMissing => TextKey("bill.problem.vendor_missing"),
            BillProblem::DateMissing => TextKey("bill.problem.date_missing"),
            BillProblem::ClosedPeriodNeedsUpdate => TextKey("bill.problem.closed_needs_update"),
            BillProblem::RefTooLong => TextKey("bill.problem.ref_too_long"),
            BillProblem::TermsUnknown => TextKey("bill.problem.terms_unknown"),
            BillProblem::TermsDaysInvalid => TextKey("bill.problem.terms_days_invalid"),
            BillProblem::DiscountInvalid => TextKey("bill.problem.discount_invalid"),
            BillProblem::DiscountNeedsUpdate => TextKey("bill.problem.discount_needs_update"),
            BillProblem::DueBeforeDate => TextKey("bill.problem.due_before_date"),
            BillProblem::DiscountDateInvalid => TextKey("bill.problem.discount_date_invalid"),
            BillProblem::AmountMissing => TextKey("bill.problem.amount_missing"),
            BillProblem::AmountNotPositive => TextKey("bill.problem.amount_not_positive"),
            BillProblem::NoLines => TextKey("bill.problem.no_lines"),
            BillProblem::TooManyLines => TextKey("bill.problem.too_many_lines"),
            BillProblem::LinesTotalTooLarge => TextKey("bill.problem.lines_total_too_large"),
            BillProblem::LinesDisagree { .. } => TextKey("bill.problem.lines_disagree"),
            BillProblem::LineAccountMissing(_) => TextKey("bill.problem.line_account_missing"),
            BillProblem::LineAmountNotPositive(_) => {
                TextKey("bill.problem.line_amount_not_positive")
            }
            BillProblem::BillableNeedsCustomer(_) => {
                TextKey("bill.problem.billable_needs_customer")
            }
            BillProblem::LineDetailNeedsUpdate(_) => {
                TextKey("bill.problem.line_detail_needs_update")
            }
            BillProblem::ItemsNeedUpdate => TextKey("bill.problem.items_need_update"),
            BillProblem::ItemRateNeedsQty(_) => TextKey("bill.problem.item_rate_needs_qty"),
            BillProblem::ItemQtyNotPositive(_) => TextKey("bill.problem.item_qty_not_positive"),
            BillProblem::ItemRateNegative(_) => TextKey("bill.problem.item_rate_negative"),
            BillProblem::ItemAmountTooLarge(_) => TextKey("bill.problem.item_amount_too_large"),
            BillProblem::ReadNotAccepted => TextKey("bill.problem.read_not_accepted"),
        }
    }

    /// The field to focus.
    #[must_use]
    pub const fn field(self) -> BillField {
        match self {
            BillProblem::VendorMissing => BillField::Vendor,
            BillProblem::DateMissing | BillProblem::ClosedPeriodNeedsUpdate => BillField::Date,
            BillProblem::RefTooLong => BillField::Reference,
            BillProblem::TermsUnknown | BillProblem::TermsDaysInvalid => BillField::Terms,
            BillProblem::DiscountInvalid | BillProblem::DiscountNeedsUpdate => BillField::Discount,
            BillProblem::DueBeforeDate => BillField::DueDate,
            BillProblem::DiscountDateInvalid => BillField::DiscountDate,
            BillProblem::AmountMissing | BillProblem::AmountNotPositive => BillField::AmountDue,
            BillProblem::NoLines
            | BillProblem::TooManyLines
            | BillProblem::LinesTotalTooLarge
            | BillProblem::LinesDisagree { .. } => BillField::Lines,
            BillProblem::ItemsNeedUpdate => BillField::Items,
            BillProblem::LineAccountMissing(k) => BillField::LineAccount(k),
            BillProblem::LineAmountNotPositive(k) | BillProblem::ItemAmountTooLarge(k) => {
                BillField::LineAmount(k)
            }
            BillProblem::BillableNeedsCustomer(k) | BillProblem::LineDetailNeedsUpdate(k) => {
                BillField::LineCustomer(k)
            }
            BillProblem::ItemRateNeedsQty(k) | BillProblem::ItemQtyNotPositive(k) => {
                BillField::LineQty(k)
            }
            BillProblem::ItemRateNegative(k) => BillField::LineRate(k),
            BillProblem::ReadNotAccepted => BillField::ReadBanner,
        }
    }
}

/// The term as checked: what is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedTerms {
    /// Engine code.
    pub code: String,
    /// Days of a custom term.
    pub days: Option<u16>,
    /// Discount percentage of a custom term.
    pub discount_percent: Option<Percent>,
    /// Discount days of a custom term.
    pub discount_days: Option<u16>,
}

/// One checked item line. `Debug` is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedItemLine {
    /// The item.
    pub item: Option<ItemId>,
    /// Description, when typed (trimmed).
    pub description: Option<String>,
    /// Quantity (more than zero).
    pub qty: Option<Quantity>,
    /// Rate (zero or more); set only together with `qty`.
    pub rate: Option<Money>,
    /// Amount, more than zero: `rate × qty` rounded to cents when both are
    /// set, otherwise as typed.
    pub amount: Money,
    /// Account.
    pub account: AccountId,
    /// Customer.
    pub contact: Option<ContactId>,
    /// Billable.
    pub billable: bool,
    /// Tag.
    pub tag: Option<TagId>,
}

impl fmt::Debug for CheckedItemLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedItemLine")
            .field("item", &self.item.is_some())
            .field("qty", &self.qty.is_some())
            .field("billable", &self.billable)
            .finish_non_exhaustive()
    }
}

/// A bill or credit that passed [`check`], trimmed, ready to map field by
/// field onto the engine draft (no arithmetic left to do). `Debug` is
/// redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedBill {
    /// Bill or credit.
    pub kind: BillKind,
    /// Vendor.
    pub vendor: ContactId,
    /// Date.
    pub date: CalendarDate,
    /// Ref. no., when typed (trimmed).
    pub reference: Option<String>,
    /// Terms (bills only; `None` uses the engine default).
    pub terms: Option<CheckedTerms>,
    /// The due date, sent only when the owner typed it (bills only).
    pub due_date: Option<CalendarDate>,
    /// The discount date, sent only when the owner typed it (bills only,
    /// `bill-terms-v1`).
    pub discount_date: Option<CalendarDate>,
    /// Amount due, more than zero, equal to the sum of the lines.
    pub amount: Money,
    /// Memo, when typed (trimmed).
    pub memo: Option<String>,
    /// Header tags.
    pub header_tags: Vec<TagId>,
    /// Expense lines, each with its effective tag.
    pub expense_lines: Vec<PostingLine>,
    /// Item lines, each with its effective tag.
    pub item_lines: Vec<CheckedItemLine>,
    /// Bill received: always `Some(true)` for a bill when the engine has
    /// the flag (Q-P18h), `None` for a credit or an engine without it.
    pub bill_received: Option<bool>,
    /// Draft files to link.
    pub attachments: Vec<AttachmentRef>,
    /// The date (or the saved date) is in a closed period: the screen asks
    /// for the closing password before sending.
    pub needs_closing_password: bool,
}

impl fmt::Debug for CheckedBill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedBill")
            .field("kind", &self.kind)
            .field("expense_lines", &self.expense_lines.len())
            .field("item_lines", &self.item_lines.len())
            .field("needs_closing_password", &self.needs_closing_password)
            .finish_non_exhaustive()
    }
}

fn check_terms(
    form: &BillForm,
    ctx: &BillContext<'_>,
    problems: &mut Vec<BillProblem>,
) -> Option<CheckedTerms> {
    let code = form.terms.as_deref()?;
    let Some(term) = ctx.terms.iter().find(|t| t.code == code) else {
        problems.push(BillProblem::TermsUnknown);
        return None;
    };
    if !term.is_custom() {
        if term.discount_percent.is_some() && !ctx.caps.terms {
            problems.push(BillProblem::DiscountNeedsUpdate);
        }
        return Some(CheckedTerms {
            code: term.code.clone(),
            days: None,
            discount_percent: None,
            discount_days: None,
        });
    }
    let days = form.terms_days.filter(|d| *d <= TERMS_DAYS_MAX);
    if days.is_none() {
        problems.push(BillProblem::TermsDaysInvalid);
    }
    let (percent, discount_days) = (form.discount_percent, form.discount_days);
    if percent.is_some() || discount_days.is_some() {
        if ctx.caps.terms {
            let ok = match (percent, discount_days, days) {
                (Some(_), Some(d), Some(net)) => d <= DISCOUNT_DAYS_MAX && d < net,
                (Some(_), Some(d), None) => d <= DISCOUNT_DAYS_MAX,
                _ => false,
            };
            if !ok {
                problems.push(BillProblem::DiscountInvalid);
            }
        } else {
            problems.push(BillProblem::DiscountNeedsUpdate);
        }
    }
    Some(CheckedTerms {
        code: term.code.clone(),
        days,
        discount_percent: percent,
        discount_days,
    })
}

fn check_dates(form: &BillForm, ctx: &BillContext<'_>, problems: &mut Vec<BillProblem>) {
    let date = form.date;
    let due = form.due_date.filter(|_| form.due_overridden);
    if let (Some(d), Some(due)) = (date, due)
        && due < d
    {
        problems.push(BillProblem::DueBeforeDate);
    }
    if form.discount_overridden
        && let Some(disc) = form.discount_date
    {
        if !ctx.caps.terms {
            problems.push(BillProblem::DiscountNeedsUpdate);
        }
        let before = date.is_some_and(|d| disc < d);
        let after = form.due_date.is_some_and(|due| disc > due);
        if before || after {
            problems.push(BillProblem::DiscountDateInvalid);
        }
    }
}

/// Checks the item lines; returns them checked (in line order).
fn check_items(
    form: &BillForm,
    ctx: &BillContext<'_>,
    problems: &mut Vec<BillProblem>,
) -> Vec<CheckedItemLine> {
    let mut out = Vec::new();
    if form.items.used().next().is_some() && !ctx.caps.lines {
        problems.push(BillProblem::ItemsNeedUpdate);
    }
    for l in form.items.used() {
        let k = l.key;
        let amount = match (l.qty, l.rate) {
            (None, Some(_)) => Err(BillProblem::ItemRateNeedsQty(k)),
            (Some(q), _) if !q.is_positive() => Err(BillProblem::ItemQtyNotPositive(k)),
            _ => match l.line_amount() {
                Ok(Some(a)) if a.is_positive() => Ok(a),
                Ok(_) => Err(BillProblem::LineAmountNotPositive(k)),
                Err(ItemAmountError::QuantityNotPositive) => {
                    Err(BillProblem::ItemQtyNotPositive(k))
                }
                Err(ItemAmountError::RateNegative) => Err(BillProblem::ItemRateNegative(k)),
                Err(ItemAmountError::TooLarge) => Err(BillProblem::ItemAmountTooLarge(k)),
            },
        };
        let amount = match amount {
            Ok(a) => Some(a),
            Err(p) => {
                problems.push(p);
                None
            }
        };
        if l.account.is_none() {
            problems.push(BillProblem::LineAccountMissing(l.key));
        }
        let billable = l.billable && form.kind == BillKind::Bill;
        if billable && l.customer.is_none() {
            problems.push(BillProblem::BillableNeedsCustomer(l.key));
        }
        let tag = line_tag_default(&form.header_tags, l.tag, ctx.tags);
        if let (Some(account), Some(amount)) = (l.account, amount) {
            out.push(CheckedItemLine {
                item: l.item,
                description: trimmed(&l.description),
                qty: l.qty,
                rate: l.rate.filter(|_| l.qty.is_some()),
                amount,
                account,
                contact: l.customer,
                billable,
                tag,
            });
        }
    }
    out
}

/// Checks the expense lines; returns them checked (in line order).
fn check_expenses(
    form: &BillForm,
    ctx: &BillContext<'_>,
    problems: &mut Vec<BillProblem>,
) -> Vec<PostingLine> {
    let mut out = Vec::new();
    for l in form.expenses.used() {
        let amount = l.amount.filter(|m| m.is_positive());
        if l.account.is_none() {
            problems.push(BillProblem::LineAccountMissing(l.key));
        }
        if amount.is_none() {
            problems.push(BillProblem::LineAmountNotPositive(l.key));
        }
        let credit = form.kind == BillKind::Credit;
        // A credit line is never billable (process review S2: dropped).
        let billable = l.billable && !credit;
        let needs_lines =
            (l.customer.is_some() || billable || (credit && l.tag.is_some())) && !ctx.caps.lines;
        if needs_lines {
            problems.push(BillProblem::LineDetailNeedsUpdate(l.key));
        } else if billable && l.customer.is_none() {
            problems.push(BillProblem::BillableNeedsCustomer(l.key));
        }
        let tag = if credit && !ctx.caps.lines {
            None
        } else {
            line_tag_default(&form.header_tags, l.tag, ctx.tags)
        };
        if let (Some(account), Some(amount)) = (l.account, amount) {
            out.push(PostingLine {
                account,
                amount,
                memo: trimmed(&l.memo),
                contact: l.customer,
                billable,
                tag,
            });
        }
    }
    out
}

/// Checks the form. The lines (expenses + items) must add up to the amount
/// due exactly.
///
/// # Errors
/// Every problem found, in field order: vendor, date, ref, terms, dates,
/// amount, the lines, then the read-from-document state.
pub fn check(form: &BillForm, ctx: &BillContext<'_>) -> Result<CheckedBill, Vec<BillProblem>> {
    let mut problems = Vec::new();
    let is_bill = form.kind == BillKind::Bill;
    if form.vendor.is_none() {
        problems.push(BillProblem::VendorMissing);
    }
    let mut needs_closing_password = false;
    match form.date {
        None => problems.push(BillProblem::DateMissing),
        Some(d) => {
            let closed = ctx
                .closing_date
                .is_some_and(|c| d <= c || ctx.original_date.is_some_and(|o| o <= c));
            if closed {
                if ctx.caps.closing {
                    needs_closing_password = true;
                } else {
                    problems.push(BillProblem::ClosedPeriodNeedsUpdate);
                }
            }
        }
    }
    // UTF-16 units, as cl-api and the engine count them.
    if crate::lists::engine_len(form.reference.trim()) > REF_MAX {
        problems.push(BillProblem::RefTooLong);
    }
    let terms = if is_bill {
        let t = check_terms(form, ctx, &mut problems);
        check_dates(form, ctx, &mut problems);
        t
    } else {
        None
    };
    let used = form.expenses.used().count() + form.items.used().count();
    if used > MAX_LINES {
        problems.push(BillProblem::TooManyLines);
    }
    let total = form.lines_total();
    let amount = match (form.amount_due, total) {
        (Some(a), _) => Some(a),
        (None, Some(t)) if t.is_positive() => Some(t),
        _ => None,
    };
    match amount {
        None if used == 0 => problems.push(BillProblem::AmountMissing),
        Some(m) if !m.is_positive() => problems.push(BillProblem::AmountNotPositive),
        _ => {}
    }
    if used == 0 {
        problems.push(BillProblem::NoLines);
    }
    let expense_lines = check_expenses(form, ctx, &mut problems);
    let item_lines = check_items(form, ctx, &mut problems);
    match (amount, total) {
        (_, None) => {
            // An item line whose qty × rate fails already has its problem.
            if !form.items.used().any(|l| l.line_amount().is_err()) {
                problems.push(BillProblem::LinesTotalTooLarge);
            }
        }
        (Some(a), Some(t)) => match a.checked_sub(t).ok() {
            Some(d) if d.is_zero() => {}
            Some(difference) => problems.push(BillProblem::LinesDisagree { difference }),
            None => problems.push(BillProblem::LinesTotalTooLarge),
        },
        (None, Some(_)) => {}
    }
    let unaccepted = form.read.unaccepted().into_iter().any(|t| match t {
        ReadTarget::Line(k) => form.expenses.lines.iter().any(|l| l.key == k),
        _ => true,
    });
    if unaccepted {
        problems.push(BillProblem::ReadNotAccepted);
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    match (form.vendor, form.date, amount) {
        (Some(vendor), Some(date), Some(amount)) => Ok(CheckedBill {
            kind: form.kind,
            vendor,
            date,
            reference: trimmed(&form.reference),
            terms,
            due_date: form.due_date.filter(|_| is_bill && form.due_overridden),
            discount_date: form
                .discount_date
                .filter(|_| is_bill && form.discount_overridden),
            amount,
            memo: trimmed(&form.memo),
            header_tags: form.header_tags.clone(),
            expense_lines,
            item_lines,
            bill_received: (is_bill && ctx.caps.received).then_some(true),
            attachments: form.attachments.clone(),
            needs_closing_password,
        }),
        // Every `None` above pushed a problem, so this arm is unreachable;
        // answer with a problem rather than panic.
        _ => Err(vec![BillProblem::AmountMissing]),
    }
}

/// Case folding for refs: each character upper-cased then lower-cased
/// (std only), so `ß`, `SS` and `ss` compare equal, as do `Σ`, `σ` and `ς`.
/// No other normalisation (no Unicode NFC/NFKC, no space or dash folding).
fn fold(s: &str) -> String {
    s.trim()
        .chars()
        .flat_map(char::to_uppercase)
        .flat_map(char::to_lowercase)
        .collect()
}

/// Two refs (vendor invoice numbers) are the same: trimmed, Unicode case
/// folded, nothing else (`INV-55871` = ` inv-55871 `, but not `INV 55871`).
#[must_use]
pub fn ref_matches(a: &str, b: &str) -> bool {
    fold(a) == fold(b)
}

/// The duplicate-ref window around a bill date: 18 calendar months before
/// and after, inclusive, the day clamped to the month end (Aug 31 − 18
/// months = Feb 28 or 29). Clamped to the supported dates at the edges.
#[must_use]
pub fn duplicate_window(date: CalendarDate) -> (CalendarDate, CalendarDate) {
    let from = date
        .add_months(-DUPLICATE_WINDOW_MONTHS)
        .unwrap_or_else(|_| CalendarDate::min());
    let to = date
        .add_months(DUPLICATE_WINDOW_MONTHS)
        .unwrap_or_else(|_| CalendarDate::max());
    (from, to)
}

/// One engine row the duplicate check looks at (`/bills` or
/// `/vendor-credits`). `Debug` is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct DuplicateCandidate {
    /// Transaction id.
    pub id: TransactionId,
    /// Vendor.
    pub vendor: ContactId,
    /// Date.
    pub date: CalendarDate,
    /// The vendor invoice number.
    pub reference: String,
    /// `status == posted` (void rows are ignored).
    pub posted: bool,
}

impl fmt::Debug for DuplicateCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DuplicateCandidate")
            .field("id", &self.id)
            .field("posted", &self.posted)
            .finish_non_exhaustive()
    }
}

/// The rows that already use `reference` (D3): same vendor, posted, another
/// id, the ref equal by [`ref_matches`] and the date inside
/// [`duplicate_window`]. Sorted by date, then id. An empty ref has no
/// duplicates.
#[must_use]
pub fn find_duplicates<'a>(
    vendor: ContactId,
    reference: &str,
    date: CalendarDate,
    own: Option<TransactionId>,
    rows: &'a [DuplicateCandidate],
) -> Vec<&'a DuplicateCandidate> {
    if reference.trim().is_empty() {
        return Vec::new();
    }
    let (from, to) = duplicate_window(date);
    let mut out: Vec<&DuplicateCandidate> = rows
        .iter()
        .filter(|r| {
            r.posted
                && r.vendor == vendor
                && Some(r.id) != own
                && r.date >= from
                && r.date <= to
                && ref_matches(&r.reference, reference)
        })
        .collect();
    out.sort_by_key(|r| (r.date, r.id));
    out
}

#[cfg(test)]
mod tests;
