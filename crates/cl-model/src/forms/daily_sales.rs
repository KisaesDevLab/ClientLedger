//! The Daily Sales Summary form (P14 lane B): one business day's register
//! report keyed into the lines of a layout (the engine's daily-sales
//! template), its exact totals and over/short, and the layout itself.
//!
//! Money rules (plan §4):
//! - Every amount is the owner's typed [`Money`], never negative (Q-P14t).
//! - [`totals`] is an exact twin of the engine's `computeEntry`: inactive
//!   lines, lines without a value and zero values are skipped; a line
//!   without an account is listed as unmapped and adds nothing; mapped
//!   debit-side values add to the debits and credit-side values to the
//!   credits. **Over/short = debits − credits** (positive = over).
//! - The engine posts the over/short line itself; the app never sends it.
//!   [`CheckedDaily::expected_over_short`] is the post gate: the screen
//!   posts only when the engine's preview agrees to the cent (Q-P14w).
//! - Which side a section posts to is fixed (Q-P14s): sales, tax, tips and
//!   other credit; discount, payment and payout debit.

use std::collections::BTreeMap;
use std::fmt;

use cl_core::ids::{AccountId, DailyLineId, DailyTemplateId, TagId};
use cl_core::{CalendarDate, Money};

/// Most values one day takes (the engine's schema).
pub const MAX_VALUES: usize = 200;
/// Most lines one layout takes (the engine's schema).
pub const MAX_LAYOUT_LINES: usize = 200;
/// Longest line label, in characters.
pub const LABEL_MAX: usize = 120;
/// Longest note on a day, in characters.
pub const NOTES_MAX: usize = 2000;
/// The label of the manual sales tax line (Q-260).
pub const TAX_LINE_LABEL: &str = "Sales tax collected";

/// Where a line sits on the register report, in form order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DailySection {
    /// Sales.
    Sales,
    /// Discounts and returns.
    Discount,
    /// Sales tax collected (typed from the register report, Q-260).
    Tax,
    /// Tips.
    Tips,
    /// Payments received (tenders).
    Payment,
    /// Paid out of the drawer.
    Payout,
    /// Anything else.
    Other,
}

impl DailySection {
    /// Every section, in the order the form shows them.
    pub const ALL: [DailySection; 7] = [
        DailySection::Sales,
        DailySection::Discount,
        DailySection::Tax,
        DailySection::Tips,
        DailySection::Payment,
        DailySection::Payout,
        DailySection::Other,
    ];
}

/// Debit or credit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DailySide {
    /// Debit.
    Debit,
    /// Credit.
    Credit,
}

/// The fixed side of a section (Q-P14s).
#[must_use]
pub const fn side_for(section: DailySection) -> DailySide {
    match section {
        DailySection::Sales | DailySection::Tax | DailySection::Tips | DailySection::Other => {
            DailySide::Credit
        }
        DailySection::Discount | DailySection::Payment | DailySection::Payout => DailySide::Debit,
    }
}

/// One line of a layout.
#[derive(Clone, PartialEq, Eq)]
pub struct LayoutLine {
    /// The saved line (`None` until the layout is saved).
    pub id: Option<DailyLineId>,
    /// Where it sits; `None` for a section this version does not know
    /// (shown, never edited).
    pub section: Option<DailySection>,
    /// The owner's label.
    pub label: String,
    /// The account it posts to.
    pub account: Option<AccountId>,
    /// The side the engine posts it to (the section's side for new lines).
    pub side: DailySide,
    /// False for a removed line.
    pub active: bool,
}

impl LayoutLine {
    /// A new line in `section` on that section's side.
    #[must_use]
    pub fn new(section: DailySection, label: &str, account: Option<AccountId>) -> LayoutLine {
        LayoutLine {
            id: None,
            section: Some(section),
            label: label.to_string(),
            account,
            side: side_for(section),
            active: true,
        }
    }
}

/// Hides the label.
impl fmt::Debug for LayoutLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LayoutLine")
            .field("id", &self.id)
            .field("section", &self.section)
            .field("account", &self.account)
            .field("side", &self.side)
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

/// A layout: its name and lines in form order.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct DailyLayout {
    /// The owner's name for it.
    pub name: String,
    /// Its lines (active and removed).
    pub lines: Vec<LayoutLine>,
}

/// Hides the name.
impl fmt::Debug for DailyLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DailyLayout")
            .field("lines", &self.lines)
            .finish_non_exhaustive()
    }
}

/// Why a layout can't be saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayoutProblem {
    /// A line has no label.
    EmptyLabel {
        /// Its index.
        index: usize,
    },
    /// A label is over 120 characters.
    LabelTooLong {
        /// Its index.
        index: usize,
    },
    /// More than 200 lines.
    TooManyLines,
    /// No active tender (payment) line.
    NoTender,
    /// No active sales line.
    NoSalesLine,
}

impl LayoutProblem {
    /// The catalog key of the owner's message.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            LayoutProblem::EmptyLabel { .. } => "daily.layout.problem.empty_label",
            LayoutProblem::LabelTooLong { .. } => "daily.layout.problem.label_too_long",
            LayoutProblem::TooManyLines => "daily.layout.problem.too_many_lines",
            LayoutProblem::NoTender => "daily.layout.problem.no_tender",
            LayoutProblem::NoSalesLine => "daily.layout.problem.no_sales",
        }
    }
}

impl DailyLayout {
    /// What keeps this layout from being saved (empty when it can be).
    #[must_use]
    pub fn problems(&self) -> Vec<LayoutProblem> {
        let mut out = Vec::new();
        if self.lines.len() > MAX_LAYOUT_LINES {
            out.push(LayoutProblem::TooManyLines);
        }
        for (index, l) in self.lines.iter().enumerate() {
            let n = l.label.trim().chars().count();
            if n == 0 {
                out.push(LayoutProblem::EmptyLabel { index });
            } else if n > LABEL_MAX {
                out.push(LayoutProblem::LabelTooLong { index });
            }
        }
        let has = |s: DailySection| self.lines.iter().any(|l| l.active && l.section == Some(s));
        if !has(DailySection::Sales) {
            out.push(LayoutProblem::NoSalesLine);
        }
        if !has(DailySection::Payment) {
            out.push(LayoutProblem::NoTender);
        }
        out
    }

    /// Adds the manual "Sales tax collected" line to the sales tax
    /// liability account when no active tax line exists (Q-260). Returns
    /// whether a line was added.
    pub fn ensure_tax_line(&mut self, tax_account: Option<AccountId>) -> bool {
        if self
            .lines
            .iter()
            .any(|l| l.active && l.section == Some(DailySection::Tax))
        {
            return false;
        }
        let at = self
            .lines
            .iter()
            .rposition(|l| {
                matches!(
                    l.section,
                    Some(DailySection::Sales | DailySection::Discount)
                )
            })
            .map_or(0, |i| i + 1);
        self.lines.insert(
            at,
            LayoutLine::new(DailySection::Tax, TAX_LINE_LABEL, tax_account),
        );
        true
    }

    /// The active lines of `section`, with their indexes.
    pub fn in_section(&self, section: DailySection) -> impl Iterator<Item = (usize, &LayoutLine)> {
        self.lines
            .iter()
            .enumerate()
            .filter(move |(_, l)| l.active && l.section == Some(section))
    }
}

/// The form's value: one business day on a layout.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct DailySalesForm {
    /// The layout.
    pub template: Option<DailyTemplateId>,
    /// The business day.
    pub date: Option<CalendarDate>,
    /// The day's tag.
    pub tag: Option<TagId>,
    /// The owner's note.
    pub notes: String,
    /// The typed amount of each saved line (`None` = blank).
    pub values: BTreeMap<DailyLineId, Option<Money>>,
}

/// Hides the notes and amounts.
impl fmt::Debug for DailySalesForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DailySalesForm")
            .field("template", &self.template)
            .field("date", &self.date)
            .field("tag", &self.tag)
            .field("values", &self.values.len())
            .finish_non_exhaustive()
    }
}

/// Whether the drawer came out over, short or even.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OverShort {
    /// Tenders equal sales, tax and the rest.
    Even,
    /// More tenders than sales: posts as a credit to Cash Over/Short.
    Over(Money),
    /// Fewer tenders than sales: posts as a debit to Cash Over/Short.
    Short(Money),
}

impl OverShort {
    /// From the signed figure (debits − credits).
    #[must_use]
    pub fn of(signed: Money) -> OverShort {
        if signed.is_positive() {
            OverShort::Over(signed)
        } else if signed.is_negative() {
            OverShort::Short(signed.abs())
        } else {
            OverShort::Even
        }
    }
}

/// Hides the amount.
impl fmt::Debug for OverShort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            OverShort::Even => "Even",
            OverShort::Over(_) => "Over(..)",
            OverShort::Short(_) => "Short(..)",
        })
    }
}

/// The exact totals of a day, as the engine will compute them.
#[derive(Clone, PartialEq, Eq)]
pub struct DailyTotals {
    /// Sales lines.
    pub sales: Money,
    /// Discount lines.
    pub discounts: Money,
    /// Sales tax lines.
    pub tax: Money,
    /// Tips lines.
    pub tips: Money,
    /// Tenders.
    pub payments: Money,
    /// Paid-outs.
    pub payouts: Money,
    /// Other lines.
    pub other: Money,
    /// Debit-side total.
    pub debits: Money,
    /// Credit-side total.
    pub credits: Money,
    /// Debits − credits (positive = over), as the engine's `overShort`.
    pub signed_over_short: Money,
    /// The same, for owners.
    pub over_short: OverShort,
    /// Lines with an amount but no account (by index), as the engine's
    /// `unmappedLabels`.
    pub unmapped: Vec<usize>,
}

/// Hides the amounts.
impl fmt::Debug for DailyTotals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DailyTotals")
            .field("over_short", &self.over_short)
            .field("unmapped", &self.unmapped)
            .finish_non_exhaustive()
    }
}

/// The totals overflowed the engine's range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TotalsTooLarge;

/// The exact twin of the engine's `computeEntry` for `form` on `layout`.
///
/// # Errors
/// [`TotalsTooLarge`] when a total leaves the money range.
pub fn totals(form: &DailySalesForm, layout: &DailyLayout) -> Result<DailyTotals, TotalsTooLarge> {
    let add = |a: Money, b: Money| a.checked_add(b).map_err(|_| TotalsTooLarge);
    let zero = Money::ZERO;
    let mut t = DailyTotals {
        sales: zero,
        discounts: zero,
        tax: zero,
        tips: zero,
        payments: zero,
        payouts: zero,
        other: zero,
        debits: zero,
        credits: zero,
        signed_over_short: zero,
        over_short: OverShort::Even,
        unmapped: Vec::new(),
    };
    for (index, line) in layout.lines.iter().enumerate() {
        if !line.active {
            continue;
        }
        let Some(id) = line.id else { continue };
        let Some(Some(amount)) = form.values.get(&id).copied() else {
            continue;
        };
        if amount.is_zero() {
            continue;
        }
        if line.account.is_none() {
            t.unmapped.push(index);
            continue;
        }
        match line.side {
            DailySide::Debit => t.debits = add(t.debits, amount)?,
            DailySide::Credit => t.credits = add(t.credits, amount)?,
        }
        let bucket = match line.section {
            Some(DailySection::Sales) => &mut t.sales,
            Some(DailySection::Discount) => &mut t.discounts,
            Some(DailySection::Tax) => &mut t.tax,
            Some(DailySection::Tips) => &mut t.tips,
            Some(DailySection::Payment) => &mut t.payments,
            Some(DailySection::Payout) => &mut t.payouts,
            Some(DailySection::Other) | None => &mut t.other,
        };
        *bucket = add(*bucket, amount)?;
    }
    t.signed_over_short = t
        .debits
        .checked_sub(t.credits)
        .map_err(|_| TotalsTooLarge)?;
    t.over_short = OverShort::of(t.signed_over_short);
    Ok(t)
}

/// Why a day can't be posted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DailyProblem {
    /// No layout chosen.
    TemplateMissing,
    /// No date.
    DateMissing,
    /// No amount on any line with an account.
    NoAmounts,
    /// A negative amount (Q-P14t).
    NegativeAmount {
        /// The line's index in the layout.
        index: usize,
    },
    /// An amount on a line without an account.
    UnmappedWithAmount {
        /// The line's index in the layout.
        index: usize,
    },
    /// More than 200 values.
    TooManyValues,
    /// The totals are too large.
    TooLarge,
    /// Notes over 2000 characters.
    NotesTooLong,
}

impl DailyProblem {
    /// The catalog key of the owner's message.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            DailyProblem::TemplateMissing => "daily.problem.template_missing",
            DailyProblem::DateMissing => "daily.problem.date_missing",
            DailyProblem::NoAmounts => "daily.problem.no_amounts",
            DailyProblem::NegativeAmount { .. } => "daily.problem.negative",
            DailyProblem::UnmappedWithAmount { .. } => "daily.problem.unmapped",
            DailyProblem::TooManyValues => "daily.problem.too_many_values",
            DailyProblem::TooLarge => "daily.problem.too_large",
            DailyProblem::NotesTooLong => "daily.problem.notes_too_long",
        }
    }

    /// Short code for the control channel (`save.refused`).
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            DailyProblem::TemplateMissing => "template_missing",
            DailyProblem::DateMissing => "date_missing",
            DailyProblem::NoAmounts => "no_amounts",
            DailyProblem::NegativeAmount { .. } => "negative_amount",
            DailyProblem::UnmappedWithAmount { .. } => "unmapped_with_amount",
            DailyProblem::TooManyValues => "too_many_values",
            DailyProblem::TooLarge => "too_large",
            DailyProblem::NotesTooLong => "notes_too_long",
        }
    }
}

/// A day that passed [`check`]: what goes to the engine, and the over/short
/// the engine's preview must match before posting.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedDaily {
    /// The layout.
    pub template: DailyTemplateId,
    /// The business day.
    pub date: CalendarDate,
    /// The day's tag.
    pub tag: Option<TagId>,
    /// Trimmed notes (`None` when empty).
    pub notes: Option<String>,
    /// Non-zero amounts of active lines with an account, in layout order.
    pub values: Vec<(DailyLineId, Money)>,
    /// Debits − credits; the engine's preview must equal it to the cent.
    pub expected_over_short: Money,
    /// The totals shown on the form.
    pub totals: DailyTotals,
}

/// Hides the notes and amounts.
impl fmt::Debug for CheckedDaily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedDaily")
            .field("template", &self.template)
            .field("date", &self.date)
            .field("values", &self.values.len())
            .finish_non_exhaustive()
    }
}

/// The one exact gate before anything is sent.
///
/// # Errors
/// Every [`DailyProblem`] found, in form order.
pub fn check(
    form: &DailySalesForm,
    layout: &DailyLayout,
) -> Result<CheckedDaily, Vec<DailyProblem>> {
    let mut problems = Vec::new();
    if form.template.is_none() {
        problems.push(DailyProblem::TemplateMissing);
    }
    if form.date.is_none() {
        problems.push(DailyProblem::DateMissing);
    }
    let mut values = Vec::new();
    for (index, line) in layout.lines.iter().enumerate() {
        let Some(id) = line.id else { continue };
        let Some(Some(amount)) = form.values.get(&id).copied() else {
            continue;
        };
        if amount.is_negative() {
            problems.push(DailyProblem::NegativeAmount { index });
            continue;
        }
        if !line.active || amount.is_zero() {
            continue;
        }
        if line.account.is_none() {
            problems.push(DailyProblem::UnmappedWithAmount { index });
            continue;
        }
        values.push((id, amount));
    }
    if values.len() > MAX_VALUES {
        problems.push(DailyProblem::TooManyValues);
    }
    if form.notes.trim().chars().count() > NOTES_MAX {
        problems.push(DailyProblem::NotesTooLong);
    }
    let totals = totals(form, layout).ok();
    if totals.is_none() {
        problems.push(DailyProblem::TooLarge);
    }
    if values.is_empty()
        && !problems.iter().any(|p| {
            matches!(
                p,
                DailyProblem::NegativeAmount { .. } | DailyProblem::UnmappedWithAmount { .. }
            )
        })
    {
        problems.push(DailyProblem::NoAmounts);
    }
    match (form.template, form.date, totals) {
        (Some(template), Some(date), Some(totals)) if problems.is_empty() => {
            let notes = form.notes.trim();
            Ok(CheckedDaily {
                template,
                date,
                tag: form.tag,
                notes: (!notes.is_empty()).then(|| notes.to_string()),
                values,
                expected_over_short: totals.signed_over_short,
                totals,
            })
        }
        _ => Err(problems),
    }
}

#[cfg(test)]
#[path = "daily_sales_tests.rs"]
mod tests;
