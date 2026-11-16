//! The one tag gate every posting save goes through (P29 WP7, Q-276,
//! Q-180): with "Require a tag on every line" on, each line must be saved
//! with a tag, and a line on an account type a group is required for needs
//! a tag of that group. The check runs on the tag each line is **saved
//! with** ([`crate::forms::check::line_tag_default`]: the line's own tag,
//! else the header's, Q-178), so a header tag satisfies it and a tag that
//! would be dropped at save never does.
//!
//! The gate keeps only [`LineTagProblem::Missing`] and
//! [`LineTagProblem::RequiredMissing`] from [`check_line_tags`]: a line is
//! saved with one tag (so `TooManyForEngine` cannot arise), and an inactive
//! tag already on a saved transaction must not block editing it
//! (`TagUnavailable` stays the tag pickers' business).
//!
//! The gate sits in each screen's save path, not in the forms' `check()`,
//! which Save as Template shares: a template may be saved without tags;
//! **Use** opens the form, whose save is checked.

use cl_core::ids::{AccountId, TagId};

use crate::forms::check::line_tag_default;
use crate::forms::item_lines::ItemLines;
use crate::forms::lines::FormLines;
use crate::lists::accounts::AccountKind;
use crate::lists::tags::{LineTagProblem, TagList, check_line_tags};

/// Where a line sits, as the owner sees it (0-based positions on the grid,
/// blank lines included, like the forms' own line problems, QA-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Place {
    /// The Expenses tab's line.
    Expenses(usize),
    /// The Items tab's line.
    Items(usize),
    /// A line of a form with one grid.
    Line(usize),
    /// Bulk Entry: section and row.
    Row(usize, usize),
    /// The whole entry (one tag for all of it: Daily Sales, a Bank Feed
    /// row).
    Entry,
}

/// One line as the gate sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagLine {
    /// Where it is.
    pub place: Place,
    /// Its account (for the required groups, Q-180); `None` skips them.
    pub account: Option<AccountId>,
    /// The tags it is saved with.
    pub tags: Vec<TagId>,
}

/// Why a save was refused: each line and its problem, in form order, no
/// repeats.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TagRefusal {
    /// The lines and what is wrong.
    pub lines: Vec<(Place, LineTagProblem)>,
}

/// The gate. `kind_of` answers an account's type from the chart.
///
/// # Errors
/// [`TagRefusal`] naming every line that breaks a rule.
pub fn check_form_tags(
    list: &TagList,
    lines: &[TagLine],
    kind_of: impl Fn(AccountId) -> Option<AccountKind>,
) -> Result<(), TagRefusal> {
    let mut out: Vec<(Place, LineTagProblem)> = Vec::new();
    for l in lines {
        let kind = l.account.and_then(&kind_of);
        for p in check_line_tags(list, &l.tags, kind, true) {
            if matches!(
                p,
                LineTagProblem::Missing | LineTagProblem::RequiredMissing(_)
            ) && !out.contains(&(l.place, p))
            {
                out.push((l.place, p));
            }
        }
    }
    if out.is_empty() {
        Ok(())
    } else {
        Err(TagRefusal { lines: out })
    }
}

/// True when the rules could refuse anything (nothing to check otherwise).
#[must_use]
pub fn rules_on(list: &TagList) -> bool {
    list.every_line() || !list.rules().required.is_empty()
}

fn one(tag: Option<TagId>) -> Vec<TagId> {
    tag.into_iter().collect()
}

/// An Expenses grid's used lines with the tag each is saved with.
#[must_use]
pub fn expense_lines(
    header: &[TagId],
    lines: &FormLines,
    list: &TagList,
    tabs: bool,
) -> Vec<TagLine> {
    lines
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.is_blank())
        .map(|(i, l)| TagLine {
            place: if tabs {
                Place::Expenses(i)
            } else {
                Place::Line(i)
            },
            account: l.account,
            tags: one(line_tag_default(header, l.tag, list)),
        })
        .collect()
}

/// An Items grid's used lines with the tag each is saved with.
#[must_use]
pub fn item_lines(header: &[TagId], items: &ItemLines, list: &TagList) -> Vec<TagLine> {
    items
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.is_blank())
        .map(|(i, l)| TagLine {
            place: Place::Items(i),
            account: l.account,
            tags: one(line_tag_default(header, l.tag, list)),
        })
        .collect()
}

/// A form with Expenses and Items tabs (Write Check, Enter Bill, card
/// charges).
#[must_use]
pub fn tabbed_lines(
    header: &[TagId],
    expenses: &FormLines,
    items: &ItemLines,
    list: &TagList,
) -> Vec<TagLine> {
    let mut out = expense_lines(header, expenses, list, true);
    out.extend(item_lines(header, items, list));
    out
}

/// An invoice's, sales receipt's or credit memo's used lines with the tag
/// each is saved with (one grid; an item line without its own account
/// skips the required groups).
#[must_use]
pub fn invoice_lines(form: &crate::forms::invoice::InvoiceForm, list: &TagList) -> Vec<TagLine> {
    form.lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.is_blank())
        .map(|(i, l)| TagLine {
            place: Place::Line(i),
            account: l.account,
            tags: one(line_tag_default(&form.header_tags, l.tag, list)),
        })
        .collect()
}

/// Every posting form and entry path, for the test that keeps the gate
/// complete: a new posting form must say whether it carries line tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PostingForm {
    /// Write Check.
    Check,
    /// Enter Bill and vendor credits.
    Bill,
    /// Card charges and credits.
    CardCharge,
    /// Personal funds used for the business.
    PersonalFunds,
    /// Make Journal Entries.
    JournalEntry,
    /// Make Deposits.
    Deposit,
    /// Invoices, sales receipts and credit memos (one form).
    Invoice,
    /// Daily Sales (one tag for the day).
    DailySales,
    /// Loan payments (the extra lines carry tags).
    LoanPayment,
    /// A register's new or edited entry.
    RegisterEntry,
    /// Bulk Entry.
    BulkEntry,
    /// Bank Feed accept with the owner's values.
    BankFeedAccept,
    /// Receive Payment.
    ReceivePayment,
    /// Pay Bills.
    PayBills,
    /// Transfer.
    Transfer,
    /// Owner contributions and draws.
    OwnerMoney,
    /// Refunds.
    Refund,
    /// Bounced checks.
    BouncedCheck,
    /// Finance charges.
    FinanceCharge,
    /// Inventory adjustments.
    InventoryAdjust,
    /// Pay Sales Tax.
    PaySalesTax,
}

impl PostingForm {
    /// Every posting form.
    pub const ALL: [PostingForm; 21] = [
        PostingForm::Check,
        PostingForm::Bill,
        PostingForm::CardCharge,
        PostingForm::PersonalFunds,
        PostingForm::JournalEntry,
        PostingForm::Deposit,
        PostingForm::Invoice,
        PostingForm::DailySales,
        PostingForm::LoanPayment,
        PostingForm::RegisterEntry,
        PostingForm::BulkEntry,
        PostingForm::BankFeedAccept,
        PostingForm::ReceivePayment,
        PostingForm::PayBills,
        PostingForm::Transfer,
        PostingForm::OwnerMoney,
        PostingForm::Refund,
        PostingForm::BouncedCheck,
        PostingForm::FinanceCharge,
        PostingForm::InventoryAdjust,
        PostingForm::PaySalesTax,
    ];

    /// The form has a tag field on its lines (or one for the whole entry),
    /// so its save goes through [`check_form_tags`]. The others have no
    /// tag to give (Q-P29r).
    #[must_use]
    pub const fn checks_tags(self) -> bool {
        match self {
            PostingForm::Check
            | PostingForm::Bill
            | PostingForm::CardCharge
            | PostingForm::PersonalFunds
            | PostingForm::JournalEntry
            | PostingForm::Deposit
            | PostingForm::Invoice
            | PostingForm::DailySales
            | PostingForm::LoanPayment
            | PostingForm::RegisterEntry
            | PostingForm::BulkEntry
            | PostingForm::BankFeedAccept => true,
            PostingForm::ReceivePayment
            | PostingForm::PayBills
            | PostingForm::Transfer
            | PostingForm::OwnerMoney
            | PostingForm::Refund
            | PostingForm::BouncedCheck
            | PostingForm::FinanceCharge
            | PostingForm::InventoryAdjust
            | PostingForm::PaySalesTax => false,
        }
    }

    /// New forms of this kind start with the default tag (Q-276): the
    /// forms with a header tag, and Bulk Entry's new sections. Forms whose
    /// only tag field is on the line don't (a tag alone would make a blank
    /// line count as used).
    #[must_use]
    pub const fn takes_default_tag(self) -> bool {
        matches!(
            self,
            PostingForm::Check
                | PostingForm::Bill
                | PostingForm::CardCharge
                | PostingForm::PersonalFunds
                | PostingForm::Invoice
                | PostingForm::DailySales
                | PostingForm::BulkEntry
        )
    }
}

/// The default tag a new form starts with: the company's default as the
/// header tags (empty when there is none or it is inactive). Edits never
/// call this.
#[must_use]
pub fn new_form_header_tags(list: &TagList) -> Vec<TagId> {
    list.default_tag().into_iter().collect()
}

#[cfg(test)]
mod tests;
