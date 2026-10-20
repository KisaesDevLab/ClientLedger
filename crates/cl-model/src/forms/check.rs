//! The Write Check form (P13): its value, the lines-versus-amount rules, the
//! check-number rules (Q-242, Q-246, Q-P13m), payee prefill (Q-191) and the
//! vendor default account (Q-192), the header-tag defaults (Q-178, Q-233)
//! and [`check`], the one exact gate before anything is sent.
//!
//! Money rules (plan §4): the header amount and every line amount are the
//! owner's typed [`Money`]. The lines must add up to the header amount
//! **exactly** (one cent off refuses the save); the engine's own float
//! tolerance is never relied on. Prefill and defaults never set an amount
//! from anywhere but the owner's own header amount (non-negotiable 4).

use std::fmt;

use cl_core::ids::{AccountId, ContactId, TagId};
use cl_core::{CalendarDate, Money};

use crate::forms::PayeePrefill;
use crate::forms::amount_words::MAX_CHECK_UNITS;
use crate::forms::lines::{FormLine, FormLines, PostingLine, lines_total, trimmed};
use crate::lists::tags::{TagList, effective_line_tags, engine_line_tag};
use crate::rowkey::{RowKey, RowKeyGen};
use crate::text::TextKey;

/// Most lines one check takes (the engine's `MAX_LINES`).
pub const MAX_LINES: usize = 500;
/// Longest payee address, in characters.
pub const ADDRESS_MAX: usize = 500;
/// The first check number proposed when nothing is known (Q-P13m).
pub const FIRST_CHECK_NUMBER: u32 = 1001;
/// Largest check number the engine stores (a 32-bit signed integer column).
pub const MAX_CHECK_NUMBER: u32 = 2_147_483_647;

/// A receipt uploaded before the first save, sent as `draftAttachmentId`
/// (Q-P13u). `Debug` hides the file name.
#[derive(Clone, PartialEq, Eq)]
pub struct AttachmentRef {
    /// The engine's attachment id.
    pub id: String,
    /// The file name the owner chose.
    pub name: String,
}

impl fmt::Debug for AttachmentRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AttachmentRef").finish_non_exhaustive()
    }
}

/// The check number field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumberField {
    /// What the owner typed (may be empty: no number).
    Typed(String),
    /// The proposed next number (Q-242), not yet overwritten.
    Proposed(u32),
    /// Print later is ticked: the field shows "To print" (Q-246). The
    /// number that was there comes back when Print later is unticked.
    ToPrint {
        /// The typed or proposed number, as text.
        kept: String,
        /// `kept` was the proposed number rather than typed.
        proposed: bool,
    },
}

impl NumberField {
    /// The number as shown, or `None` for "To print".
    #[must_use]
    pub fn text(&self) -> Option<String> {
        match self {
            NumberField::Typed(s) => Some(s.clone()),
            NumberField::Proposed(n) => Some(n.to_string()),
            NumberField::ToPrint { .. } => None,
        }
    }
}

/// Parses a typed check number: digits only (surrounding spaces allowed),
/// 1 to [`MAX_CHECK_NUMBER`]. `Ok(None)` for an empty field.
///
/// # Errors
/// [`CheckProblem::NumberInvalid`] for anything else.
pub fn parse_number(text: &str) -> Result<Option<u32>, CheckProblem> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    if !t.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CheckProblem::NumberInvalid);
    }
    match t.parse::<u32>() {
        Ok(n) if (1..=MAX_CHECK_NUMBER).contains(&n) => Ok(Some(n)),
        _ => Err(CheckProblem::NumberInvalid),
    }
}

/// The number to propose for a new check (Q-P13m): the larger of the
/// engine's next number for the account and the highest check number in the
/// account plus one; [`FIRST_CHECK_NUMBER`] when both are unknown.
#[must_use]
pub fn propose_number(engine_next: Option<u32>, highest: Option<u32>) -> u32 {
    match (engine_next, highest.map(|h| h.saturating_add(1))) {
        (None, None) => FIRST_CHECK_NUMBER,
        (Some(a), None) | (None, Some(a)) => a,
        (Some(a), Some(b)) => a.max(b),
    }
}

/// The tag a line is saved with (Q-178, Q-233): its own tag when it has
/// one, otherwise the first header tag in group order. One tag per line.
#[must_use]
pub fn line_tag_default(
    header_tags: &[TagId],
    line_tag: Option<TagId>,
    list: &TagList,
) -> Option<TagId> {
    let pos = |id: &TagId| {
        list.tags()
            .iter()
            .position(|t| t.id == *id)
            .unwrap_or(usize::MAX)
    };
    let mut header: Vec<TagId> = header_tags.to_vec();
    header.sort_by_key(pos);
    let own: Vec<TagId> = line_tag.into_iter().collect();
    let first: Vec<TagId> = effective_line_tags(list, &header, &own)
        .into_iter()
        .take(1)
        .collect();
    // One element at most, which `engine_line_tag` always takes.
    engine_line_tag(&first).ok().flatten()
}

/// The footer line under the lines (tri-state, plan WP6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TotalsStatus {
    /// The lines add up to the check amount.
    Balanced,
    /// This much of the check amount is not on a line yet.
    Remaining(Money),
    /// The lines are this much more than the check amount.
    Over(Money),
}

/// The Write Check form's complete editable value. `Debug` shows which
/// fields are filled, never their contents.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckForm {
    /// The bank account the check is written on.
    pub bank: Option<AccountId>,
    /// Check number.
    pub number: NumberField,
    /// Print later: queue the check for printing (Q-246).
    pub print_later: bool,
    /// Date.
    pub date: Option<CalendarDate>,
    /// Payee.
    pub payee: Option<ContactId>,
    /// The name printed on the check.
    pub payee_name: String,
    /// The payee address (several lines).
    pub address: String,
    /// The check amount the owner typed (or the lines total, see
    /// [`CheckForm::leave_lines`]).
    pub amount: Option<Money>,
    /// `amount` was taken from the lines total, not typed.
    pub amount_from_lines: bool,
    /// Memo (sent as memo and printed memo, Q-P13n).
    pub memo: String,
    /// Header tags, one per one-per-group group (Q-178).
    pub header_tags: Vec<TagId>,
    /// The Expenses lines.
    pub lines: FormLines,
    /// A receipt uploaded before the first save.
    pub draft_attachment: Option<AttachmentRef>,
}

impl fmt::Debug for CheckForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckForm")
            .field("bank", &self.bank.is_some())
            .field(
                "number",
                &matches!(self.number, NumberField::ToPrint { .. }),
            )
            .field("print_later", &self.print_later)
            .field("date", &self.date.is_some())
            .field("payee", &self.payee.is_some())
            .field("payee_name", &!self.payee_name.is_empty())
            .field("address", &!self.address.is_empty())
            .field("amount", &self.amount.is_some())
            .field("amount_from_lines", &self.amount_from_lines)
            .field("memo", &!self.memo.is_empty())
            .field("header_tags", &self.header_tags.len())
            .field("lines", &self.lines.lines.len())
            .field("draft_attachment", &self.draft_attachment.is_some())
            .finish()
    }
}

/// Drops trailing blank lines down to one (no new key needed).
fn collapse_trailing_blanks(lines: &mut Vec<FormLine>) {
    while lines.len() >= 2 && lines[lines.len() - 2..].iter().all(FormLine::is_blank) {
        lines.pop();
    }
}

impl CheckForm {
    /// A blank check on `bank`, dated `date`, numbered `number`, with one
    /// blank entry line.
    #[must_use]
    pub fn new(
        bank: Option<AccountId>,
        date: Option<CalendarDate>,
        number: NumberField,
        keys: &mut RowKeyGen,
    ) -> CheckForm {
        CheckForm {
            bank,
            number,
            print_later: false,
            date,
            payee: None,
            payee_name: String::new(),
            address: String::new(),
            amount: None,
            amount_from_lines: false,
            memo: String::new(),
            header_tags: Vec::new(),
            lines: FormLines::from_lines(Vec::new(), keys),
            draft_attachment: None,
        }
    }

    /// The owner typed the check amount (Q-P13r a). The amount is no longer
    /// "from the lines". When exactly one line is used and its amount is
    /// blank or equal to the previous check amount, it follows.
    pub fn set_amount(&mut self, amount: Option<Money>) {
        let previous = self.amount;
        self.amount = amount;
        self.amount_from_lines = false;
        {
            let mut used = self.lines.used_mut();
            if let (Some(line), None) = (used.next(), used.next())
                && (line.amount.is_none() || line.amount == previous)
            {
                line.amount = amount;
            }
        }
        collapse_trailing_blanks(&mut self.lines.lines);
    }

    /// The owner left the lines grid (Q-P13r b): a blank check amount takes
    /// the lines total and is flagged; an amount that came from the lines
    /// follows them. A typed amount never changes on its own (Q-P13r c).
    pub fn leave_lines(&mut self) {
        if self.amount.is_some() && !self.amount_from_lines {
            return;
        }
        let any_typed = self.lines.lines.iter().any(|l| l.amount.is_some());
        if any_typed {
            if let Some(total) = lines_total(&self.lines) {
                self.amount = Some(total);
                self.amount_from_lines = true;
            }
        } else if self.amount_from_lines {
            self.amount = None;
            self.amount_from_lines = false;
        }
    }

    /// The footer: the check amount against the lines total. `None` without
    /// a check amount or when the total leaves the money range.
    #[must_use]
    pub fn totals_status(&self) -> Option<TotalsStatus> {
        let left = self.amount?.checked_sub(lines_total(&self.lines)?).ok()?;
        Some(if left.is_zero() {
            TotalsStatus::Balanced
        } else if left.is_positive() {
            TotalsStatus::Remaining(left)
        } else {
            TotalsStatus::Over(left.abs())
        })
    }

    /// Ticks or unticks Print later (Q-246): ticked shows "To print" and
    /// keeps the number; unticked brings it back.
    pub fn set_print_later(&mut self, on: bool) {
        if on == self.print_later {
            return;
        }
        self.print_later = on;
        self.number = match (
            on,
            std::mem::replace(&mut self.number, NumberField::Typed(String::new())),
        ) {
            (true, NumberField::Typed(s)) => NumberField::ToPrint {
                kept: s,
                proposed: false,
            },
            (true, NumberField::Proposed(n)) => NumberField::ToPrint {
                kept: n.to_string(),
                proposed: true,
            },
            (false, NumberField::ToPrint { kept, proposed }) => match kept.parse::<u32>() {
                Ok(n) if proposed => NumberField::Proposed(n),
                _ => NumberField::Typed(kept),
            },
            (_, other) => other,
        };
    }

    /// True while the form is untouched enough for payee prefill (Q-P13q):
    /// no line used and no memo typed.
    #[must_use]
    pub fn can_prefill(&self) -> bool {
        self.lines.used().next().is_none() && self.memo.trim().is_empty()
    }

    /// The vendor's default expense account and tag (Q-192, Q-178) fill the
    /// first line only when it is untouched; a typed check amount follows
    /// onto that single line (Q-P13r a). Returns true when it filled.
    pub fn apply_vendor_default(
        &mut self,
        account: AccountId,
        tag: Option<TagId>,
        keys: &mut RowKeyGen,
    ) -> bool {
        if self.lines.used().next().is_some() {
            return false;
        }
        if self.lines.lines.is_empty() {
            self.lines.lines.push(FormLine::blank(keys.next_key()));
        }
        let Some(first) = self.lines.lines.first_mut() else {
            return false;
        };
        first.account = Some(account);
        first.tag = tag;
        first.amount = self.amount;
        self.lines.ensure_entry(keys);
        true
    }
}

/// Copies the last check to this payee (Q-191): line accounts, memos,
/// customers, billable and tags, plus the memo, address and header tags.
/// Every line amount is cleared and the check amount is not touched
/// (non-negotiable 4). `last` must have been built with this form's
/// [`RowKeyGen`], so its line keys are new to this form.
impl PayeePrefill for CheckForm {
    fn prefill_from_last(&mut self, last: &CheckForm) {
        let entry = self.lines.lines.last().filter(|l| l.is_blank()).cloned();
        let mut lines: Vec<FormLine> = last
            .lines
            .lines
            .iter()
            .map(|l| FormLine {
                amount: None,
                ..l.clone()
            })
            .filter(|l| !l.is_blank())
            .collect();
        lines.extend(entry);
        self.lines = FormLines { lines };
        self.memo.clone_from(&last.memo);
        self.address.clone_from(&last.address);
        self.header_tags.clone_from(&last.header_tags);
    }
}

/// What a check is checked against.
#[derive(Debug, Clone, Copy)]
pub struct CheckContext<'a> {
    /// The tags (for the header-tag defaults).
    pub tags: &'a TagList,
    /// A saved check is being changed: it keeps a number unless it is
    /// printed later, so a cleared number is refused
    /// ([`CheckProblem::NumberInvalid`]) instead of being sent without one.
    pub editing: bool,
}

/// The field a problem belongs to (the screen focuses it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckField {
    /// Bank account.
    Bank,
    /// Check number.
    Number,
    /// Date.
    Date,
    /// Pay to the order of.
    Payee,
    /// Check amount.
    Amount,
    /// Address.
    Address,
    /// The lines as a whole.
    Lines,
    /// One line's account.
    LineAccount(RowKey),
    /// One line's amount.
    LineAmount(RowKey),
    /// One line's customer.
    LineCustomer(RowKey),
}

/// Why a check cannot be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckProblem {
    /// No bank account.
    BankMissing,
    /// No date.
    DateMissing,
    /// No payee.
    PayeeMissing,
    /// No check amount.
    AmountMissing,
    /// The check amount is zero or less.
    AmountNotPositive,
    /// The lines don't add up to the check amount; `difference` is the
    /// check amount minus the lines total (negative when the lines are
    /// more).
    LinesDisagree {
        /// Check amount − lines total, exact.
        difference: Money,
    },
    /// A used line has no account.
    LineAccountMissing(RowKey),
    /// A used line's amount is blank, zero or less.
    LineAmountNotPositive(RowKey),
    /// A billable line has no customer.
    BillableNeedsCustomer(RowKey),
    /// The check number is not a whole number from 1 to 2,147,483,647.
    NumberInvalid,
    /// A number is set while Print later is ticked (Q-246).
    NumberWithPrintLater,
    /// The address is longer than [`ADDRESS_MAX`] characters.
    AddressTooLong,
    /// The check amount is more than 999,999,999,999.99.
    AmountTooLarge,
    /// More than [`MAX_LINES`] lines.
    TooManyLines,
}

impl CheckProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            CheckProblem::BankMissing => TextKey("check.problem.bank_missing"),
            CheckProblem::DateMissing => TextKey("check.problem.date_missing"),
            CheckProblem::PayeeMissing => TextKey("check.problem.payee_missing"),
            CheckProblem::AmountMissing => TextKey("check.problem.amount_missing"),
            CheckProblem::AmountNotPositive => TextKey("check.problem.amount_not_positive"),
            CheckProblem::LinesDisagree { .. } => TextKey("check.problem.lines_disagree"),
            CheckProblem::LineAccountMissing(_) => TextKey("check.problem.line_account_missing"),
            CheckProblem::LineAmountNotPositive(_) => {
                TextKey("check.problem.line_amount_not_positive")
            }
            CheckProblem::BillableNeedsCustomer(_) => {
                TextKey("check.problem.billable_needs_customer")
            }
            CheckProblem::NumberInvalid => TextKey("check.problem.number_invalid"),
            CheckProblem::NumberWithPrintLater => TextKey("check.problem.number_with_print_later"),
            CheckProblem::AddressTooLong => TextKey("check.problem.address_too_long"),
            CheckProblem::AmountTooLarge => TextKey("check.problem.amount_too_large"),
            CheckProblem::TooManyLines => TextKey("check.problem.too_many_lines"),
        }
    }

    /// The field to focus.
    #[must_use]
    pub const fn field(self) -> CheckField {
        match self {
            CheckProblem::BankMissing => CheckField::Bank,
            CheckProblem::DateMissing => CheckField::Date,
            CheckProblem::PayeeMissing => CheckField::Payee,
            CheckProblem::AmountMissing
            | CheckProblem::AmountNotPositive
            | CheckProblem::AmountTooLarge => CheckField::Amount,
            CheckProblem::LinesDisagree { .. } | CheckProblem::TooManyLines => CheckField::Lines,
            CheckProblem::LineAccountMissing(k) => CheckField::LineAccount(k),
            CheckProblem::LineAmountNotPositive(k) => CheckField::LineAmount(k),
            CheckProblem::BillableNeedsCustomer(k) => CheckField::LineCustomer(k),
            CheckProblem::NumberInvalid | CheckProblem::NumberWithPrintLater => CheckField::Number,
            CheckProblem::AddressTooLong => CheckField::Address,
        }
    }
}

/// A check that passed [`check`], trimmed, ready to map field by field onto
/// the engine draft (no arithmetic left to do). `Debug` is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedCheck {
    /// Bank account.
    pub bank: AccountId,
    /// Check number; `None` for Print later or a check without a number.
    pub number: Option<u32>,
    /// Print later.
    pub print_later: bool,
    /// Date.
    pub date: CalendarDate,
    /// Payee.
    pub payee: ContactId,
    /// Name on the check, trimmed (may be empty: the engine uses the
    /// payee's display name).
    pub payee_name: String,
    /// Address, when typed (trimmed).
    pub address: Option<String>,
    /// Check amount, more than zero, equal to the sum of the lines.
    pub amount: Money,
    /// Memo, when typed (trimmed).
    pub memo: Option<String>,
    /// Header tags.
    pub header_tags: Vec<TagId>,
    /// The lines, each with its effective tag (Q-178).
    pub lines: Vec<PostingLine>,
    /// A receipt uploaded before the first save.
    pub draft_attachment: Option<AttachmentRef>,
}

impl fmt::Debug for CheckedCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedCheck")
            .field("print_later", &self.print_later)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

/// Check amount − sum of the typed line amounts, in exact units. Saturates
/// at the money range (only reachable with absurd line sums; the save is
/// refused either way).
fn difference(header: Money, lines: &FormLines) -> Money {
    let sum: i128 = lines
        .lines
        .iter()
        .filter_map(|l| l.amount)
        .map(Money::to_ten_thousandths)
        .fold(0_i128, i128::saturating_add);
    let diff = header.to_ten_thousandths().saturating_sub(sum);
    Money::from_ten_thousandths(diff).unwrap_or(if diff > 0 { Money::MAX } else { Money::MIN })
}

fn check_number(form: &CheckForm) -> Result<Option<u32>, CheckProblem> {
    match (&form.number, form.print_later) {
        (NumberField::ToPrint { .. }, true) => Ok(None),
        (NumberField::Typed(s), true) if s.trim().is_empty() => Ok(None),
        (_, true) => Err(CheckProblem::NumberWithPrintLater),
        (NumberField::Typed(s), false) => parse_number(s),
        (NumberField::Proposed(n), false) if (1..=MAX_CHECK_NUMBER).contains(n) => Ok(Some(*n)),
        (NumberField::Proposed(_) | NumberField::ToPrint { .. }, false) => {
            Err(CheckProblem::NumberInvalid)
        }
    }
}

/// Checks the form. The lines must add up to the check amount exactly.
///
/// # Errors
/// Every problem found, in field order: bank, number, date, payee, amount,
/// address, then the lines.
pub fn check(form: &CheckForm, ctx: &CheckContext<'_>) -> Result<CheckedCheck, Vec<CheckProblem>> {
    let mut problems = Vec::new();
    if form.bank.is_none() {
        problems.push(CheckProblem::BankMissing);
    }
    let number = match check_number(form) {
        Ok(None) if ctx.editing && !form.print_later => {
            problems.push(CheckProblem::NumberInvalid);
            None
        }
        Ok(n) => n,
        Err(p) => {
            problems.push(p);
            None
        }
    };
    if form.date.is_none() {
        problems.push(CheckProblem::DateMissing);
    }
    if form.payee.is_none() {
        problems.push(CheckProblem::PayeeMissing);
    }
    match form.amount {
        None => problems.push(CheckProblem::AmountMissing),
        Some(m) if !m.is_positive() => problems.push(CheckProblem::AmountNotPositive),
        Some(m) if m.to_ten_thousandths() > MAX_CHECK_UNITS => {
            problems.push(CheckProblem::AmountTooLarge);
        }
        Some(_) => {}
    }
    if form.address.chars().count() > ADDRESS_MAX {
        problems.push(CheckProblem::AddressTooLong);
    }
    if form.lines.used().count() > MAX_LINES {
        problems.push(CheckProblem::TooManyLines);
    }
    let mut lines = Vec::new();
    for l in form.lines.used() {
        let account = l.account;
        let amount = l.amount.filter(|m| m.is_positive());
        if account.is_none() {
            problems.push(CheckProblem::LineAccountMissing(l.key));
        }
        if amount.is_none() {
            problems.push(CheckProblem::LineAmountNotPositive(l.key));
        }
        if l.billable && l.customer.is_none() {
            problems.push(CheckProblem::BillableNeedsCustomer(l.key));
        }
        if let (Some(account), Some(amount)) = (account, amount) {
            lines.push(PostingLine {
                account,
                amount,
                memo: trimmed(&l.memo),
                contact: l.customer,
                billable: l.billable,
                tag: line_tag_default(&form.header_tags, l.tag, ctx.tags),
            });
        }
    }
    if let Some(header) = form.amount {
        let difference = difference(header, &form.lines);
        if !difference.is_zero() {
            problems.push(CheckProblem::LinesDisagree { difference });
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    match (form.bank, form.date, form.payee, form.amount) {
        (Some(bank), Some(date), Some(payee), Some(amount)) => Ok(CheckedCheck {
            bank,
            number,
            print_later: form.print_later,
            date,
            payee,
            payee_name: form.payee_name.trim().to_string(),
            address: trimmed(&form.address),
            amount,
            memo: trimmed(&form.memo),
            header_tags: form.header_tags.clone(),
            lines,
            draft_attachment: form.draft_attachment.clone(),
        }),
        // Every `None` above pushed a problem, so this arm is unreachable;
        // answer with a problem rather than panic.
        _ => Err(vec![CheckProblem::AmountMissing]),
    }
}

#[cfg(test)]
mod tests;
