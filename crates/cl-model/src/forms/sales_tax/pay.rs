//! Pay Sales Tax (P23 plan §2.6, §3 WP-3): the agency, the "through"
//! date, the payment date and account, how it is paid, the engine's
//! preview of what each item owes, the owner's amounts and adjustments,
//! and the plan to post. "Match a payment you already made" is
//! [`LinkForm`].
//!
//! No money arithmetic: what is owed, each line's amount and the check
//! total are the preview's. Changing an amount or an adjustment asks for
//! a new preview (the total shown is the engine's for exactly what will
//! be sent). Each posted line carries the preview's `expectedOwed`, a
//! byte copy (409 `SALES_TAX_OWED_CHANGED` when it moved).

use std::collections::BTreeMap;
use std::fmt;

use cl_core::ids::{AccountId, SalesTaxAgencyId, SalesTaxItemId, TransactionId};
use cl_core::{CalendarDate, Money, UserInput};

use crate::text::TextKey;

/// Longest adjustment reason.
pub const REASON_MAX: usize = 200;

/// How the tax is paid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PayMethod {
    /// A check: printed later (the default) or a typed number.
    #[default]
    Check,
    /// Paid on the agency's site or by bank transfer.
    Electronic,
}

/// One line of the engine's preview.
#[derive(Clone, PartialEq, Eq)]
pub struct PayLineFigure {
    /// The item (`None`: legacy tax not assigned to an item; shown, never
    /// paid here).
    pub item: Option<SalesTaxItemId>,
    /// Owed through the date.
    pub owed: Money,
    /// Σ adjustments on it.
    pub adjustment: Money,
    /// What this payment pays.
    pub amount: Money,
    /// Copied back on post.
    pub expected_owed: Money,
}

impl fmt::Debug for PayLineFigure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayLineFigure")
            .field("item", &self.item)
            .finish_non_exhaustive()
    }
}

/// The engine's preview.
#[derive(Clone, PartialEq, Eq)]
pub struct PayFigures {
    /// The lines.
    pub lines: Vec<PayLineFigure>,
    /// The check amount.
    pub total: Money,
    /// The preview's owed total of the payment's items (`owed`), sent back
    /// as the top-level `expectedOwed` (a byte copy, never a sum made here).
    pub owed: Option<Money>,
    /// Legacy tax without an item is owed (the banner).
    pub legacy_unassigned: bool,
    /// More than owed is paid on an item (a warning only).
    pub more_than_owed: bool,
}

impl fmt::Debug for PayFigures {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayFigures")
            .field("lines", &self.lines)
            .field("legacy_unassigned", &self.legacy_unassigned)
            .finish_non_exhaustive()
    }
}

/// An adjustment as typed.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct AdjustmentEntry {
    /// The item.
    pub item: Option<SalesTaxItemId>,
    /// The account.
    pub account: Option<AccountId>,
    /// The amount as typed; negative lowers what is paid (`-1.96`).
    pub amount: String,
    /// Why.
    pub reason: String,
}

impl fmt::Debug for AdjustmentEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdjustmentEntry")
            .field("item", &self.item)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// A checked adjustment.
#[derive(Clone, PartialEq, Eq)]
pub struct AdjustmentPlan {
    /// The item.
    pub item: SalesTaxItemId,
    /// The account.
    pub account: AccountId,
    /// Signed, whole cents, not zero.
    pub amount: Money,
    /// Why (trimmed).
    pub reason: String,
}

impl fmt::Debug for AdjustmentPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdjustmentPlan")
            .field("item", &self.item)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// What the preview call sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayRequestPlan {
    /// The agency.
    pub agency: SalesTaxAgencyId,
    /// Owed through.
    pub through: CalendarDate,
    /// The payment date.
    pub date: CalendarDate,
    /// The bank account.
    pub bank: AccountId,
    /// How.
    pub method: PayMethod,
    /// Queue the check (also a check with no number: the engine queues it).
    pub print_later: bool,
    /// A typed check number (the engine takes a positive whole number).
    pub check_number: Option<u32>,
    /// The owner's amounts (`None`: everything owed).
    pub amounts: Option<Vec<(SalesTaxItemId, Money)>>,
    /// Adjustments.
    pub adjustments: Vec<AdjustmentPlan>,
}

/// One posted line: (item, amount, expected owed), all the preview's.
pub type PostLine = (SalesTaxItemId, Money, Money);

/// What Pay posts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayPostPlan {
    /// The request (its `amounts` are the posted lines' amounts).
    pub request: PayRequestPlan,
    /// The lines with their `expectedOwed`.
    pub lines: Vec<PostLine>,
    /// The preview's owed total, sent as the top-level `expectedOwed`.
    pub expected_owed: Money,
    /// The save attempt (the UI keys each attempt).
    pub attempt: u64,
}

/// Why Pay can't go yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PayProblem {
    /// No agency.
    NoAgency,
    /// No through date or payment date.
    NoDate,
    /// No bank account.
    NoBank,
    /// A typed amount is not an amount or is negative.
    BadAmount,
    /// An adjustment lacks its item, account, amount or reason.
    BadAdjustment,
    /// A check has a number and Print later both.
    CheckNumberOrPrintLater,
    /// The check number is not a whole number above zero.
    BadCheckNumber,
    /// The preview is being recalculated.
    PreviewPending,
    /// The check total is zero or less.
    NothingToPay,
}

impl PayProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            PayProblem::NoAgency => TextKey("pay_sales_tax.problem.no_agency"),
            PayProblem::NoDate => TextKey("pay_sales_tax.problem.no_date"),
            PayProblem::NoBank => TextKey("pay_sales_tax.problem.no_bank"),
            PayProblem::BadAmount => TextKey("pay_sales_tax.problem.bad_amount"),
            PayProblem::BadAdjustment => TextKey("pay_sales_tax.problem.bad_adjustment"),
            PayProblem::CheckNumberOrPrintLater => TextKey("pay_sales_tax.problem.check_number"),
            PayProblem::BadCheckNumber => TextKey("pay_sales_tax.problem.bad_check_number"),
            PayProblem::PreviewPending => TextKey("pay_sales_tax.problem.updating"),
            PayProblem::NothingToPay => TextKey("pay_sales_tax.problem.nothing_to_pay"),
        }
    }
}

/// The Pay Sales Tax window.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct PayForm {
    /// The agency.
    pub agency: Option<SalesTaxAgencyId>,
    /// Owed through.
    pub through: Option<CalendarDate>,
    /// The payment date.
    pub date: Option<CalendarDate>,
    /// The bank account.
    pub bank: Option<AccountId>,
    /// How.
    pub method: PayMethod,
    /// Print later (a check's default).
    pub print_later: bool,
    /// A typed check number.
    pub check_number: String,
    amounts: BTreeMap<SalesTaxItemId, String>,
    adjustments: Vec<AdjustmentEntry>,
    figures: Option<(u64, PayFigures)>,
    revision: u64,
}

impl fmt::Debug for PayForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayForm")
            .field("agency", &self.agency)
            .field("through", &self.through)
            .field("date", &self.date)
            .field("method", &self.method)
            .field("amounts", &self.amounts.len())
            .field("adjustments", &self.adjustments.len())
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

fn parse_amount(s: &str) -> Option<Money> {
    Money::parse_user(s, UserInput::CENTS).ok()
}

impl PayForm {
    /// A new payment: a check printed later.
    #[must_use]
    pub fn new(
        agency: Option<SalesTaxAgencyId>,
        through: Option<CalendarDate>,
        date: Option<CalendarDate>,
    ) -> PayForm {
        PayForm {
            agency,
            through,
            date,
            print_later: true,
            ..PayForm::default()
        }
    }

    /// The revision of what the preview depends on (bumped by every
    /// change; a preview answer for an older one is dropped).
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Marks the inputs changed: the figures shown are stale.
    pub fn changed(&mut self) {
        self.revision += 1;
    }

    /// Sets the agency (amounts and adjustments of another agency go).
    pub fn set_agency(&mut self, agency: Option<SalesTaxAgencyId>) {
        if self.agency != agency {
            self.agency = agency;
            self.amounts.clear();
            self.adjustments.clear();
            self.changed();
        }
    }

    /// Types an item's amount (blank: what the engine says).
    pub fn set_amount(&mut self, item: SalesTaxItemId, text: &str) {
        if text.trim().is_empty() {
            self.amounts.remove(&item);
        } else {
            self.amounts.insert(item, text.to_string());
        }
        self.changed();
    }

    /// The amount typed for `item`.
    #[must_use]
    pub fn amount_text(&self, item: SalesTaxItemId) -> Option<&str> {
        self.amounts.get(&item).map(String::as_str)
    }

    /// Adds a blank adjustment row ("Add adjustment"); returns its index.
    pub fn add_adjustment(&mut self) -> usize {
        self.adjustments.push(AdjustmentEntry::default());
        self.changed();
        self.adjustments.len() - 1
    }

    /// Edits an adjustment row.
    pub fn edit_adjustment(&mut self, i: usize, f: impl FnOnce(&mut AdjustmentEntry)) {
        if let Some(a) = self.adjustments.get_mut(i) {
            f(a);
            self.revision += 1;
        }
    }

    /// Removes an adjustment row.
    pub fn remove_adjustment(&mut self, i: usize) {
        if i < self.adjustments.len() {
            self.adjustments.remove(i);
            self.changed();
        }
    }

    /// The adjustment rows.
    #[must_use]
    pub fn adjustments(&self) -> &[AdjustmentEntry] {
        &self.adjustments
    }

    /// Stores a preview made at `revision` (false: stale, dropped).
    pub fn apply_preview(&mut self, revision: u64, figures: PayFigures) -> bool {
        if revision != self.revision {
            return false;
        }
        self.figures = Some((revision, figures));
        true
    }

    /// The figures for what is on screen now.
    #[must_use]
    pub fn figures(&self) -> Option<&PayFigures> {
        self.figures
            .as_ref()
            .filter(|(r, _)| *r == self.revision)
            .map(|(_, f)| f)
    }

    /// What the preview call sends.
    ///
    /// # Errors
    /// The first [`PayProblem`] of the inputs.
    pub fn request_plan(&self) -> Result<PayRequestPlan, PayProblem> {
        let agency = self.agency.ok_or(PayProblem::NoAgency)?;
        let (through, date) = self.through.zip(self.date).ok_or(PayProblem::NoDate)?;
        let bank = self.bank.ok_or(PayProblem::NoBank)?;
        let number = self.check_number.trim();
        // A check with no number waits in the print queue (the engine's
        // rule); a number together with Print later is refused.
        let check_number = match self.method {
            PayMethod::Check if self.print_later && !number.is_empty() => {
                return Err(PayProblem::CheckNumberOrPrintLater);
            }
            PayMethod::Check if !number.is_empty() => Some(
                number
                    .parse::<u32>()
                    .ok()
                    .filter(|n| *n > 0 && i32::try_from(*n).is_ok())
                    .ok_or(PayProblem::BadCheckNumber)?,
            ),
            _ => None,
        };
        // A typed amount is what the payment pays for the item, after its
        // adjustments (the engine's `lines[].amount`). The engine pays only
        // the items sent once any is, so every other item of the last
        // preview goes along at that preview's amount (a copy).
        let amounts = if self.amounts.is_empty() {
            None
        } else {
            let mut typed = BTreeMap::new();
            for (item, text) in &self.amounts {
                let m = parse_amount(text)
                    .filter(|m| !m.is_negative())
                    .ok_or(PayProblem::BadAmount)?;
                typed.insert(*item, m);
            }
            let mut v: Vec<(SalesTaxItemId, Money)> = Vec::new();
            if let Some((_, last)) = &self.figures {
                for l in &last.lines {
                    if let Some(item) = l.item
                        && !typed.contains_key(&item)
                    {
                        v.push((item, l.amount));
                    }
                }
            }
            v.extend(typed);
            Some(v)
        };
        let mut adjustments = Vec::with_capacity(self.adjustments.len());
        for a in &self.adjustments {
            let amount = parse_amount(&a.amount).filter(|m| !m.is_zero());
            let reason = a.reason.trim();
            match (a.item, a.account, amount) {
                (Some(item), Some(account), Some(amount))
                    if !reason.is_empty() && reason.chars().count() <= REASON_MAX =>
                {
                    adjustments.push(AdjustmentPlan {
                        item,
                        account,
                        amount,
                        reason: reason.to_string(),
                    });
                }
                _ => return Err(PayProblem::BadAdjustment),
            }
        }
        Ok(PayRequestPlan {
            agency,
            through,
            date,
            bank,
            method: self.method,
            print_later: self.method == PayMethod::Check && check_number.is_none(),
            check_number,
            amounts,
            adjustments,
        })
    }

    /// What Pay posts: the preview's lines (unassigned ones left out), each
    /// with the preview's `amount` (what is paid for the item, adjustments
    /// included; the engine's `lines[].amount`) and `expectedOwed` copied,
    /// plus the preview's owed total. The adjustments go with the request;
    /// the engine splits Sales Tax Payable and the adjustment accounts (no
    /// arithmetic here).
    ///
    /// # Errors
    /// A [`PayProblem`] of the inputs, `PreviewPending` without current
    /// figures, `NothingToPay` when the preview's total is not above zero.
    pub fn post_plan(&self) -> Result<PayPostPlan, PayProblem> {
        let mut request = self.request_plan()?;
        let f = self.figures().ok_or(PayProblem::PreviewPending)?;
        if !f.total.is_positive() {
            return Err(PayProblem::NothingToPay);
        }
        let lines: Vec<PostLine> = f
            .lines
            .iter()
            .filter_map(|l| Some((l.item?, l.amount, l.expected_owed)))
            .collect();
        request.amounts = Some(lines.iter().map(|(i, a, _)| (*i, *a)).collect());
        Ok(PayPostPlan {
            request,
            lines,
            expected_owed: f.owed.ok_or(PayProblem::PreviewPending)?,
            attempt: self.revision,
        })
    }
}

// ---- Link ------------------------------------------------------------------------

/// Why a link can't go yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinkProblem {
    /// No payment picked.
    NoPayment,
    /// No agency.
    NoAgency,
    /// No item has an amount.
    NoLines,
    /// An amount is not an amount above zero.
    BadAmount,
}

impl LinkProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            LinkProblem::NoPayment => TextKey("pay_sales_tax.link.no_payment"),
            LinkProblem::NoAgency => TextKey("pay_sales_tax.problem.no_agency"),
            LinkProblem::NoLines => TextKey("pay_sales_tax.link.no_lines"),
            LinkProblem::BadAmount => TextKey("pay_sales_tax.problem.bad_amount"),
        }
    }
}

/// What a link sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkPlan {
    /// The outside check or journal entry.
    pub payment: TransactionId,
    /// The agency.
    pub agency: SalesTaxAgencyId,
    /// Its split, in item order (the engine checks the total, 400
    /// `LINK_TOTAL_MISMATCH`).
    pub lines: Vec<(SalesTaxItemId, Money)>,
}

/// "Match a payment you already made".
#[derive(Clone, PartialEq, Eq, Default)]
pub struct LinkForm {
    /// The payment.
    pub payment: Option<TransactionId>,
    /// The agency.
    pub agency: Option<SalesTaxAgencyId>,
    amounts: BTreeMap<SalesTaxItemId, String>,
}

impl fmt::Debug for LinkForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinkForm")
            .field("payment", &self.payment)
            .field("agency", &self.agency)
            .field("lines", &self.amounts.len())
            .finish()
    }
}

impl LinkForm {
    /// Types an item's part (blank removes it).
    pub fn set_amount(&mut self, item: SalesTaxItemId, text: &str) {
        if text.trim().is_empty() {
            self.amounts.remove(&item);
        } else {
            self.amounts.insert(item, text.to_string());
        }
    }

    /// The plan to send.
    ///
    /// # Errors
    /// The first [`LinkProblem`].
    pub fn plan(&self) -> Result<LinkPlan, LinkProblem> {
        let payment = self.payment.ok_or(LinkProblem::NoPayment)?;
        let agency = self.agency.ok_or(LinkProblem::NoAgency)?;
        if self.amounts.is_empty() {
            return Err(LinkProblem::NoLines);
        }
        let mut lines = Vec::with_capacity(self.amounts.len());
        for (item, text) in &self.amounts {
            let m = parse_amount(text)
                .filter(|m| m.is_positive())
                .ok_or(LinkProblem::BadAmount)?;
            lines.push((*item, m));
        }
        Ok(LinkPlan {
            payment,
            agency,
            lines,
        })
    }
}
