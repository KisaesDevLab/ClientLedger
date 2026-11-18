//! The import wizard's step machine (P31), one shell for every source.
//!
//! - [`Source`]: `QuickBooks` Desktop (a full conversion), Accounting Power,
//!   `QuickBooks` Online, `QuickBooks` Desktop GL only, and Generic CSV.
//! - [`FileSlot`]: one report the owner exports, with its name, where to
//!   find it (plain words, by text key), the engine kind, whether it is
//!   posted or only compared against, and when it is required. The
//!   Journal/GL slot takes several files (one per year).
//! - [`history_dates`]: Q-142 history depth ("two years plus opening
//!   balances" preselected, Q-P31b; or "All").
//! - [`Wizard`]: the steps and `can_continue` for each.

use std::collections::BTreeMap;

use cl_core::CalendarDate;

/// Where the books come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Source {
    /// `QuickBooks` Desktop: lists, history, open items and inventory.
    Qbd,
    /// Accounting Power.
    AccountingPower,
    /// `QuickBooks` Online.
    Qbo,
    /// `QuickBooks` Desktop, chart and general ledger only.
    QbdGl,
    /// The engine's Generic CSV layout.
    Generic,
}

impl Source {
    /// Every source, in picker order.
    pub const ALL: [Source; 5] = [
        Source::Qbd,
        Source::AccountingPower,
        Source::Qbo,
        Source::QbdGl,
        Source::Generic,
    ];

    /// A stable key for control-channel state and text keys
    /// (`imports.source.<key>`).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Source::Qbd => "qbd",
            Source::AccountingPower => "accounting_power",
            Source::Qbo => "qbo",
            Source::QbdGl => "qbd_gl",
            Source::Generic => "generic",
        }
    }

    /// Reads [`Source::key`].
    #[must_use]
    pub fn from_key(s: &str) -> Option<Source> {
        Source::ALL.into_iter().find(|x| x.key() == s)
    }

    /// The engine's `sourceSystem`.
    #[must_use]
    pub const fn engine_source(self) -> &'static str {
        match self {
            Source::Qbd | Source::QbdGl => "quickbooks_desktop",
            Source::AccountingPower => "accounting_power",
            Source::Qbo => "quickbooks_online",
            Source::Generic => "generic",
        }
    }

    /// A full QBD conversion (open items, inventory, spot check).
    #[must_use]
    pub const fn is_conversion(self) -> bool {
        matches!(self, Source::Qbd)
    }

    /// The company must have no transactions yet (Q-P31h).
    #[must_use]
    pub const fn requires_empty_company(self) -> bool {
        self.is_conversion()
    }

    /// The source's files carry no trial balance date; the wizard sends it.
    #[must_use]
    pub const fn needs_tb_date(self) -> bool {
        matches!(self, Source::AccountingPower | Source::Generic)
    }

    /// The steps the wizard shows for this source.
    #[must_use]
    pub fn steps(self) -> &'static [Step] {
        match self {
            Source::Qbd => &[
                Step::Source,
                Step::Dates,
                Step::Files,
                Step::Mapping,
                Step::Import,
                Step::TieOut,
                Step::SpotCheck,
                Step::Done,
            ],
            _ => &[
                Step::Source,
                Step::Dates,
                Step::Files,
                Step::Mapping,
                Step::Import,
                Step::TieOut,
                Step::Done,
            ],
        }
    }

    /// The reports this source imports, in upload order.
    #[must_use]
    pub fn slots(self) -> &'static [FileSlot] {
        match self {
            Source::Qbd => &QBD_SLOTS,
            Source::QbdGl => &QBD_GL_SLOTS,
            Source::AccountingPower => &AP_SLOTS,
            Source::Qbo => &QBO_SLOTS,
            Source::Generic => &GENERIC_SLOTS,
        }
    }
}

/// The wizard's steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Step {
    /// Pick where the books come from.
    Source,
    /// Conversion date and history depth.
    Dates,
    /// One drop target per report.
    Files,
    /// Accounts, names, items and classes (the engine's matches).
    Mapping,
    /// Backup, then the one-batch import.
    Import,
    /// The engine's tie-out; Mark verified.
    TieOut,
    /// The guided spot-check list (QBD, Q-144).
    SpotCheck,
    /// Finished.
    Done,
}

impl Step {
    /// A stable key (`imports.step.<key>`).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Step::Source => "source",
            Step::Dates => "dates",
            Step::Files => "files",
            Step::Mapping => "mapping",
            Step::Import => "import",
            Step::TieOut => "tieout",
            Step::SpotCheck => "spot_check",
            Step::Done => "done",
        }
    }
}

/// Posted, or only compared against (S9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// The commit posts it.
    Post,
    /// The tie-out compares the books with it; never posted.
    Reference,
}

impl Role {
    /// The engine's `role`.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Role::Post => "post",
            Role::Reference => "reference",
        }
    }
}

/// When a slot must have a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Need {
    /// Always.
    Required,
    /// Never (the owner may add one).
    Optional,
    /// With "two years plus opening balances" (the opening trial balance).
    TwoYearsOnly,
    /// When the reference trial balance shows an inventory balance.
    WithInventory,
}

/// Which date a slot's report must be as of (S5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DateRule {
    /// The file has no single date (lists, journals).
    None,
    /// As of the conversion date (reference trial balance, agings,
    /// valuation; QBD's aging reports call it "Report Date").
    Conversion,
    /// As of the day before history starts (opening trial balance).
    Opening,
}

/// One report the owner exports. Text is named by keys under
/// `imports.slot.<key>.{name,how}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileSlot {
    /// Stable key.
    pub key: &'static str,
    /// The engine's `kind`.
    pub kind: &'static str,
    /// The engine's `contactKind` for a list.
    pub contact_kind: Option<&'static str>,
    /// Posted or reference.
    pub role: Role,
    /// When it must have a file.
    pub need: Need,
    /// Several files allowed (one journal per year).
    pub multiple: bool,
    /// The date its report must be as of.
    pub date: DateRule,
}

const fn slot(
    key: &'static str,
    kind: &'static str,
    role: Role,
    need: Need,
    date: DateRule,
) -> FileSlot {
    FileSlot {
        key,
        kind,
        contact_kind: None,
        role,
        need,
        multiple: false,
        date,
    }
}

const fn list(key: &'static str, contact: &'static str, need: Need) -> FileSlot {
    FileSlot {
        key,
        kind: "contacts",
        contact_kind: Some(contact),
        role: Role::Post,
        need,
        multiple: false,
        date: DateRule::None,
    }
}

const fn journal(key: &'static str, need: Need) -> FileSlot {
    FileSlot {
        key,
        kind: "gl_transactions",
        contact_kind: None,
        role: Role::Post,
        need,
        multiple: true,
        date: DateRule::None,
    }
}

static QBD_SLOTS: [FileSlot; 11] = [
    slot(
        "qbd_account_list",
        "coa",
        Role::Post,
        Need::Required,
        DateRule::None,
    ),
    list("qbd_customer_list", "customer", Need::Required),
    list("qbd_vendor_list", "vendor", Need::Required),
    list("qbd_other_names", "other", Need::Optional),
    slot(
        "qbd_item_list",
        "items",
        Role::Post,
        Need::Optional,
        DateRule::None,
    ),
    slot(
        "qbd_tb_opening",
        "trial_balance",
        Role::Post,
        Need::TwoYearsOnly,
        DateRule::Opening,
    ),
    journal("qbd_journal", Need::Required),
    slot(
        "qbd_tb_reference",
        "trial_balance",
        Role::Reference,
        Need::Required,
        DateRule::Conversion,
    ),
    slot(
        "qbd_open_invoices",
        "open_invoices",
        Role::Post,
        Need::Required,
        DateRule::Conversion,
    ),
    slot(
        "qbd_unpaid_bills",
        "unpaid_bills",
        Role::Post,
        Need::Required,
        DateRule::Conversion,
    ),
    slot(
        "qbd_inventory_valuation",
        "inventory_valuation",
        Role::Post,
        Need::WithInventory,
        DateRule::Conversion,
    ),
];

static QBD_GL_SLOTS: [FileSlot; 6] = [
    slot(
        "qbd_account_list",
        "coa",
        Role::Post,
        Need::Required,
        DateRule::None,
    ),
    list("qbd_customer_list", "customer", Need::Optional),
    list("qbd_vendor_list", "vendor", Need::Optional),
    slot(
        "qbd_tb_opening",
        "trial_balance",
        Role::Post,
        Need::Optional,
        DateRule::Opening,
    ),
    journal("qbd_journal", Need::Required),
    slot(
        "qbd_tb_reference",
        "trial_balance",
        Role::Reference,
        Need::Required,
        DateRule::Conversion,
    ),
];

static AP_SLOTS: [FileSlot; 6] = [
    slot("ap_coa", "coa", Role::Post, Need::Required, DateRule::None),
    list("ap_customers", "customer", Need::Optional),
    list("ap_vendors", "vendor", Need::Optional),
    slot(
        "ap_tb_opening",
        "trial_balance",
        Role::Post,
        Need::Optional,
        DateRule::Opening,
    ),
    journal("ap_gl", Need::Required),
    slot(
        "ap_tb_reference",
        "trial_balance",
        Role::Reference,
        Need::Required,
        DateRule::Conversion,
    ),
];

static QBO_SLOTS: [FileSlot; 6] = [
    slot("qbo_coa", "coa", Role::Post, Need::Required, DateRule::None),
    list("qbo_customers", "customer", Need::Optional),
    list("qbo_vendors", "vendor", Need::Optional),
    slot(
        "qbo_tb_opening",
        "trial_balance",
        Role::Post,
        Need::Optional,
        DateRule::Opening,
    ),
    journal("qbo_journal", Need::Required),
    slot(
        "qbo_tb_reference",
        "trial_balance",
        Role::Reference,
        Need::Required,
        DateRule::Conversion,
    ),
];

static GENERIC_SLOTS: [FileSlot; 5] = [
    slot(
        "generic_coa",
        "coa",
        Role::Post,
        Need::Required,
        DateRule::None,
    ),
    // One file with a Type column (customer, vendor or both).
    list("generic_contacts", "customer", Need::Optional),
    slot(
        "generic_tb_opening",
        "trial_balance",
        Role::Post,
        Need::Optional,
        DateRule::Opening,
    ),
    journal("generic_gl", Need::Required),
    slot(
        "generic_tb_reference",
        "trial_balance",
        Role::Reference,
        Need::Required,
        DateRule::Conversion,
    ),
];

/// The document a QBD open-item row becomes (E2a, design a). An unapplied
/// customer payment ("Payment" on Open Invoices) comes in as a credit memo,
/// never as a customer payment (engine spike, P31): it then applies and
/// refunds like any credit. `None` is a row the engine refuses.
#[must_use]
pub fn open_item_document(qbd_type: &str) -> Option<&'static str> {
    match qbd_type.trim().to_ascii_lowercase().as_str() {
        "invoice" => Some("invoice"),
        "credit memo" | "payment" => Some("credit_memo"),
        "bill" => Some("bill"),
        "credit" | "bill credit" => Some("vendor_credit"),
        _ => None,
    }
}

/// How much history comes in (Q-142).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum HistoryDepth {
    /// Two full fiscal years before the conversion's, plus opening
    /// balances the day before (preselected, Q-P31b).
    #[default]
    TwoYears,
    /// Every transaction in the files.
    All,
}

impl HistoryDepth {
    /// A stable key (`imports.depth.<key>`).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            HistoryDepth::TwoYears => "two_years",
            HistoryDepth::All => "all",
        }
    }
}

/// The dates a depth gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryDates {
    /// History before this day is skipped (`historyFrom`); `None` = all.
    pub history_from: Option<CalendarDate>,
    /// The opening trial balance's date (the day before history starts).
    pub opening_tb: Option<CalendarDate>,
}

/// Why the dates can't be worked out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateProblem {
    /// The fiscal year's first month is not 1-12.
    BadFiscalMonth,
    /// The conversion date is after today.
    ConversionInFuture,
    /// The dates leave the calendar's range.
    OutOfRange,
    /// The opening balances are not dated before the conversion date.
    OpeningNotBefore,
}

impl DateProblem {
    /// A stable key (`imports.dates.problem.<key>`).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            DateProblem::BadFiscalMonth => "bad_fiscal_month",
            DateProblem::ConversionInFuture => "future",
            DateProblem::OutOfRange => "out_of_range",
            DateProblem::OpeningNotBefore => "opening_not_before",
        }
    }
}

/// The first day of the fiscal year `date` is in, for a year starting in
/// `fy_start_month`.
///
/// # Errors
/// `BadFiscalMonth` outside 1-12; `OutOfRange` at the calendar's edge.
pub fn fiscal_year_start(
    date: CalendarDate,
    fy_start_month: u32,
) -> Result<CalendarDate, DateProblem> {
    if !(1..=12).contains(&fy_start_month) {
        return Err(DateProblem::BadFiscalMonth);
    }
    let year = if date.month() >= fy_start_month {
        date.year()
    } else {
        date.year() - 1
    };
    CalendarDate::from_ymd(year, fy_start_month, 1).map_err(|_| DateProblem::OutOfRange)
}

/// Q-142: "two years plus opening balances" starts history on the first
/// day of the fiscal year two years before the conversion date's, with the
/// opening trial balance the day before; "All" sends no history start.
///
/// # Errors
/// See [`fiscal_year_start`].
pub fn history_dates(
    conversion: CalendarDate,
    fy_start_month: u32,
    depth: HistoryDepth,
) -> Result<HistoryDates, DateProblem> {
    let fy = fiscal_year_start(conversion, fy_start_month)?;
    match depth {
        HistoryDepth::All => Ok(HistoryDates {
            history_from: None,
            opening_tb: None,
        }),
        HistoryDepth::TwoYears => {
            let from = CalendarDate::from_ymd(fy.year() - 2, fy_start_month, 1)
                .map_err(|_| DateProblem::OutOfRange)?;
            let opening = from.add_days(-1).map_err(|_| DateProblem::OutOfRange)?;
            Ok(HistoryDates {
                history_from: Some(from),
                opening_tb: Some(opening),
            })
        }
    }
}

/// One file the owner dropped on a slot. The name is for display only;
/// `report_date` is what the engine read from the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DroppedFile {
    /// The file's name, shown in the slot (never logged).
    pub name: String,
    /// The engine accepted it (a session exists).
    pub uploaded: bool,
    /// Rows the engine could not read.
    pub problems: u32,
    /// The report date the engine read, when the file has one.
    pub report_date: Option<CalendarDate>,
}

impl DroppedFile {
    /// Uploaded with no row problems.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.uploaded && self.problems == 0
    }
}

/// What keeps the owner on a step. Text keys: `imports.blocker.<key>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocker {
    /// No conversion date.
    NoConversionDate,
    /// The dates are not usable.
    Dates(DateProblem),
    /// A required report has no file.
    MissingFile {
        /// The slot.
        slot: &'static str,
    },
    /// A file is still uploading or the engine refused it.
    NotUploaded {
        /// The slot.
        slot: &'static str,
    },
    /// A file has rows the engine could not read.
    RowProblems {
        /// The slot.
        slot: &'static str,
    },
    /// A report is not dated as it must be (S5).
    WrongDate {
        /// The slot.
        slot: &'static str,
        /// The date it must be as of.
        want: CalendarDate,
    },
    /// A slot that takes one file has several.
    OneFileOnly {
        /// The slot.
        slot: &'static str,
    },
    /// Accounts, names or classes are still unmatched (Q-P31c).
    MappingIncomplete,
    /// The import has not finished.
    NotImported,
    /// The tie-out is not zero (or was not verified).
    NotVerified,
    /// The company already has transactions (QBD, Q-P31h).
    CompanyNotEmpty,
    /// An opening trial balance was added but its date was not given
    /// (sources other than a QBD conversion).
    NoOpeningDate,
}

impl Blocker {
    /// A stable key.
    #[must_use]
    pub const fn key(&self) -> &'static str {
        match self {
            Blocker::NoConversionDate => "no_conversion_date",
            Blocker::Dates(_) => "dates",
            Blocker::MissingFile { .. } => "missing_file",
            Blocker::NotUploaded { .. } => "not_uploaded",
            Blocker::RowProblems { .. } => "row_problems",
            Blocker::WrongDate { .. } => "wrong_date",
            Blocker::OneFileOnly { .. } => "one_file_only",
            Blocker::MappingIncomplete => "mapping_incomplete",
            Blocker::NotImported => "not_imported",
            Blocker::NotVerified => "not_verified",
            Blocker::CompanyNotEmpty => "company_not_empty",
            Blocker::NoOpeningDate => "no_opening_date",
        }
    }
}

/// The wizard's state. Every engine fact (uploads, mapping, commit,
/// tie-out) is set by the screen from the engine's answers.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools, reason = "independent engine facts")]
pub struct Wizard {
    /// Where the books come from.
    pub source: Source,
    /// The current step.
    pub step: Step,
    /// The conversion date (the reference trial balance's date).
    pub conversion_date: Option<CalendarDate>,
    /// The fiscal year's first month (1-12).
    pub fy_start_month: u32,
    /// History depth (a QBD conversion).
    pub depth: HistoryDepth,
    /// The opening balances' date, as the owner gives it (every source but
    /// a QBD conversion, whose date follows the depth; phase step 7).
    pub opening_date: Option<CalendarDate>,
    /// Files per slot key.
    pub files: BTreeMap<&'static str, Vec<DroppedFile>>,
    /// The reference trial balance shows an inventory balance (the
    /// valuation is then required).
    pub has_inventory: bool,
    /// The company has transactions already.
    pub company_has_activity: bool,
    /// Every account, name and class is matched or created.
    pub mapping_complete: bool,
    /// The batch committed.
    pub imported: bool,
    /// The engine verified the tie-out (`allZero`, then verify).
    pub verified: bool,
}

impl Wizard {
    /// A new wizard on the source step.
    #[must_use]
    pub fn new(source: Source, fy_start_month: u32) -> Wizard {
        Wizard {
            source,
            step: Step::Source,
            conversion_date: None,
            fy_start_month,
            depth: HistoryDepth::default(),
            opening_date: None,
            files: BTreeMap::new(),
            has_inventory: false,
            company_has_activity: false,
            mapping_complete: false,
            imported: false,
            verified: false,
        }
    }

    /// The history dates: from the depth for a QBD conversion; for other
    /// sources no history start and the owner's opening balance date.
    ///
    /// # Errors
    /// `NoConversionDate`, or the date problem.
    pub fn dates(&self) -> Result<HistoryDates, Blocker> {
        let c = self.conversion_date.ok_or(Blocker::NoConversionDate)?;
        if !self.source.is_conversion() {
            if self.opening_date.is_some_and(|o| o >= c) {
                return Err(Blocker::Dates(DateProblem::OpeningNotBefore));
            }
            return Ok(HistoryDates {
                history_from: None,
                opening_tb: self.opening_date,
            });
        }
        history_dates(c, self.fy_start_month, self.depth).map_err(Blocker::Dates)
    }

    /// The slot is required with the current choices.
    #[must_use]
    pub fn is_required(&self, slot: &FileSlot) -> bool {
        match slot.need {
            Need::Required => true,
            Need::Optional => false,
            Need::TwoYearsOnly => self.depth == HistoryDepth::TwoYears,
            Need::WithInventory => self.has_inventory,
        }
    }

    /// The date a slot's report must be as of, when it has one.
    #[must_use]
    pub fn expected_date(&self, slot: &FileSlot) -> Option<CalendarDate> {
        match slot.date {
            DateRule::None => None,
            DateRule::Conversion => self.conversion_date,
            DateRule::Opening => self.dates().ok()?.opening_tb,
        }
    }

    /// Adds a dropped file to a slot (a single-file slot's file is replaced).
    pub fn drop_file(&mut self, slot: &'static str, file: DroppedFile) {
        let multiple = self
            .source
            .slots()
            .iter()
            .any(|s| s.key == slot && s.multiple);
        let list = self.files.entry(slot).or_default();
        if !multiple {
            list.clear();
        }
        list.push(file);
    }

    /// What keeps the owner on the current step (empty: Continue is on).
    #[must_use]
    pub fn blockers(&self, today: CalendarDate) -> Vec<Blocker> {
        let mut out = Vec::new();
        match self.step {
            Step::Source | Step::Done => {}
            Step::Dates => match self.conversion_date {
                None => out.push(Blocker::NoConversionDate),
                Some(c) => {
                    if c > today {
                        out.push(Blocker::Dates(DateProblem::ConversionInFuture));
                    }
                    if let Err(e) = self.dates() {
                        out.push(e);
                    }
                    if self.source.requires_empty_company() && self.company_has_activity {
                        out.push(Blocker::CompanyNotEmpty);
                    }
                }
            },
            Step::Files => self.file_blockers(&mut out),
            Step::Mapping => {
                if !self.mapping_complete {
                    out.push(Blocker::MappingIncomplete);
                }
            }
            Step::Import => {
                if !self.imported {
                    out.push(Blocker::NotImported);
                }
            }
            Step::TieOut | Step::SpotCheck => {
                if !self.verified {
                    out.push(Blocker::NotVerified);
                }
            }
        }
        out
    }

    fn file_blockers(&self, out: &mut Vec<Blocker>) {
        for slot in self.source.slots() {
            let files = self.files.get(slot.key).map_or(&[][..], Vec::as_slice);
            if files.is_empty() {
                if self.is_required(slot) {
                    out.push(Blocker::MissingFile { slot: slot.key });
                }
                continue;
            }
            if slot.date == DateRule::Opening && self.expected_date(slot).is_none() {
                out.push(Blocker::NoOpeningDate);
            }
            if !slot.multiple && files.len() > 1 {
                out.push(Blocker::OneFileOnly { slot: slot.key });
            }
            if files.iter().any(|f| !f.uploaded) {
                out.push(Blocker::NotUploaded { slot: slot.key });
            } else if files.iter().any(|f| f.problems > 0) {
                out.push(Blocker::RowProblems { slot: slot.key });
            }
            // Sources whose files carry no date get the wizard's date sent
            // with the upload, so only a date the engine read can differ.
            if let Some(want) = self.expected_date(slot)
                && files
                    .iter()
                    .any(|f| f.report_date.is_some_and(|d| d != want))
            {
                out.push(Blocker::WrongDate {
                    slot: slot.key,
                    want,
                });
            }
        }
    }

    /// Continue is on.
    #[must_use]
    pub fn can_continue(&self, today: CalendarDate) -> bool {
        self.blockers(today).is_empty()
    }

    /// Moves to the next step when nothing blocks it. Returns whether it
    /// moved.
    pub fn next(&mut self, today: CalendarDate) -> bool {
        if !self.can_continue(today) {
            return false;
        }
        let steps = self.source.steps();
        match steps.iter().position(|s| *s == self.step) {
            Some(i) if i + 1 < steps.len() => {
                self.step = steps[i + 1];
                true
            }
            _ => false,
        }
    }

    /// Moves back one step; never back past the import once it committed
    /// (the batch exists: History undoes it).
    pub fn back(&mut self) -> bool {
        let steps = self.source.steps();
        let Some(i) = steps.iter().position(|s| *s == self.step) else {
            return false;
        };
        if i == 0 || (self.imported && steps[i - 1] <= Step::Import) {
            return false;
        }
        self.step = steps[i - 1];
        true
    }

    /// Changes the source; files dropped for the old source are cleared.
    pub fn set_source(&mut self, source: Source) {
        if source != self.source {
            self.source = source;
            self.files.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn day(s: &str) -> CalendarDate {
        CalendarDate::parse_wire(s).expect("date")
    }

    fn file(date: Option<&str>) -> DroppedFile {
        DroppedFile {
            name: "report.csv".into(),
            uploaded: true,
            problems: 0,
            report_date: date.map(day),
        }
    }

    const TODAY: &str = "2026-11-18";

    #[test]
    fn two_years_matches_the_shoal_creek_fixture() {
        let d = history_dates(day("2026-10-27"), 1, HistoryDepth::TwoYears).expect("dates");
        assert_eq!(d.history_from, Some(day("2024-01-01")));
        assert_eq!(d.opening_tb, Some(day("2023-12-31")));
        let all = history_dates(day("2026-10-27"), 1, HistoryDepth::All).expect("dates");
        assert_eq!(all.history_from, None);
        assert_eq!(all.opening_tb, None);
        assert_eq!(HistoryDepth::default(), HistoryDepth::TwoYears, "Q-P31b");
    }

    /// Phase step 7: other sources' opening balances are as of the date the
    /// owner gives (the Bluebonnet fixtures: 2025-12-31 for a 2026-06-30
    /// conversion), never the QBD two-year rule.
    #[test]
    fn other_sources_use_the_owners_opening_date() {
        let today = day(TODAY);
        for source in [
            Source::Qbo,
            Source::Generic,
            Source::AccountingPower,
            Source::QbdGl,
        ] {
            let mut w = Wizard::new(source, 1);
            w.conversion_date = Some(day("2026-06-30"));
            let opening = *source
                .slots()
                .iter()
                .find(|s| s.date == DateRule::Opening)
                .expect("opening slot");
            assert!(!w.is_required(&opening), "{source:?}: optional");
            w.step = Step::Files;
            w.drop_file(opening.key, file(Some("2025-12-31")));
            assert!(
                w.blockers(today).contains(&Blocker::NoOpeningDate),
                "{source:?}"
            );
            w.opening_date = Some(day("2025-12-31"));
            assert_eq!(w.expected_date(&opening), Some(day("2025-12-31")));
            assert_eq!(
                w.dates().expect("dates"),
                HistoryDates {
                    history_from: None,
                    opening_tb: Some(day("2025-12-31"))
                }
            );
            assert!(
                !w.blockers(today)
                    .iter()
                    .any(|b| matches!(b, Blocker::WrongDate { .. } | Blocker::NoOpeningDate)),
                "{source:?}: {:?}",
                w.blockers(today)
            );
            w.opening_date = Some(day("2026-06-30"));
            w.step = Step::Dates;
            assert!(
                w.blockers(today)
                    .contains(&Blocker::Dates(DateProblem::OpeningNotBefore))
            );
        }
        // QBD keeps the depth rule.
        let mut q = Wizard::new(Source::Qbd, 1);
        q.conversion_date = Some(day("2026-10-27"));
        q.opening_date = Some(day("2025-12-31"));
        assert_eq!(
            q.dates().expect("dates").opening_tb,
            Some(day("2023-12-31"))
        );
    }

    #[test]
    fn unapplied_payments_come_in_as_credit_memos() {
        assert_eq!(open_item_document("Payment"), Some("credit_memo"));
        assert_eq!(open_item_document("Credit Memo"), Some("credit_memo"));
        assert_eq!(open_item_document("Invoice"), Some("invoice"));
        assert_eq!(open_item_document("Bill"), Some("bill"));
        assert_eq!(open_item_document("Credit"), Some("vendor_credit"));
        assert_eq!(open_item_document("Estimate"), None);
    }

    #[test]
    fn fiscal_years_starting_mid_year() {
        // A July fiscal year: 2026-03-01 is in the year that began 2025-07-01.
        let d = history_dates(day("2026-03-01"), 7, HistoryDepth::TwoYears).expect("dates");
        assert_eq!(d.history_from, Some(day("2023-07-01")));
        assert_eq!(d.opening_tb, Some(day("2023-06-30")));
        let d = history_dates(day("2026-07-01"), 7, HistoryDepth::TwoYears).expect("dates");
        assert_eq!(d.history_from, Some(day("2024-07-01")));
        assert_eq!(
            history_dates(day("2026-07-01"), 13, HistoryDepth::All),
            Err(DateProblem::BadFiscalMonth)
        );
        assert_eq!(
            fiscal_year_start(day("2026-07-01"), 0),
            Err(DateProblem::BadFiscalMonth)
        );
    }

    proptest! {
        /// Q-142: the history always starts on a fiscal year's first day,
        /// two whole fiscal years before the conversion's, and the opening
        /// trial balance is the day before.
        #[test]
        fn history_dates_hold_for_any_date_and_fiscal_year(
            days in 0i64..(365 * 80),
            month in 1u32..=12,
        ) {
            let conversion = day("1990-01-01").add_days(days).expect("in range");
            let d = history_dates(conversion, month, HistoryDepth::TwoYears).expect("dates");
            let from = d.history_from.expect("from");
            let opening = d.opening_tb.expect("opening");
            prop_assert_eq!(from.day(), 1);
            prop_assert_eq!(from.month(), month);
            prop_assert!(from <= conversion);
            prop_assert_eq!(opening.add_days(1).expect("next"), from);
            let fy = fiscal_year_start(conversion, month).expect("fy");
            prop_assert!(fy <= conversion);
            prop_assert_eq!(fy.year() - from.year(), 2);
            prop_assert_eq!(fy.month(), from.month());
            prop_assert_eq!(fiscal_year_start(from, month).expect("fy"), from);
            // Between two and three years of history, never more.
            let span = from.days_until(conversion);
            prop_assert!((730..=1096).contains(&span), "span {}", span);
            let all = history_dates(conversion, month, HistoryDepth::All).expect("all");
            prop_assert_eq!(all.history_from, None);
            prop_assert_eq!(all.opening_tb, None);
        }
    }

    #[test]
    fn every_source_has_a_reference_tb_and_a_journal() {
        for s in Source::ALL {
            let slots = s.slots();
            assert!(
                slots
                    .iter()
                    .any(|f| f.role == Role::Reference && f.need == Need::Required),
                "{s:?}: S9"
            );
            let journals: Vec<_> = slots.iter().filter(|f| f.multiple).collect();
            assert_eq!(journals.len(), 1, "{s:?}");
            assert_eq!(journals[0].kind, "gl_transactions");
            let mut keys: Vec<_> = slots.iter().map(|f| f.key).collect();
            keys.sort_unstable();
            keys.dedup();
            assert_eq!(keys.len(), slots.len(), "{s:?}: unique keys");
            assert_eq!(Source::from_key(s.key()), Some(s));
            assert_eq!(s.steps().first(), Some(&Step::Source));
            assert_eq!(s.steps().last(), Some(&Step::Done));
        }
        assert!(Source::Qbd.steps().contains(&Step::SpotCheck));
        assert!(!Source::Generic.steps().contains(&Step::SpotCheck));
        let qbd: Vec<_> = Source::Qbd.slots().iter().map(|f| f.kind).collect();
        for k in [
            "open_invoices",
            "unpaid_bills",
            "inventory_valuation",
            "items",
        ] {
            assert!(qbd.contains(&k), "{k}");
        }
        // QBD's agings and valuation are "as of" the conversion date.
        for f in Source::Qbd.slots() {
            if matches!(
                f.kind,
                "open_invoices" | "unpaid_bills" | "inventory_valuation"
            ) {
                assert_eq!(f.date, DateRule::Conversion, "{}", f.key);
                assert_eq!(f.need == Need::Required, f.kind != "inventory_valuation");
            }
        }
        assert_eq!(Source::Qbd.engine_source(), "quickbooks_desktop");
        assert_eq!(Source::QbdGl.engine_source(), "quickbooks_desktop");
        assert!(Source::Generic.needs_tb_date() && !Source::Qbo.needs_tb_date());
    }

    fn qbd_ready() -> Wizard {
        let mut w = Wizard::new(Source::Qbd, 1);
        w.conversion_date = Some(day("2026-10-27"));
        for slot in Source::Qbd.slots() {
            let date = match slot.date {
                DateRule::Conversion => Some("2026-10-27"),
                DateRule::Opening => Some("2023-12-31"),
                DateRule::None => None,
            };
            w.drop_file(slot.key, file(date));
        }
        w
    }

    #[test]
    fn walking_the_qbd_wizard() {
        let today = day(TODAY);
        let mut w = Wizard::new(Source::Qbd, 1);
        assert!(w.next(today), "source step has no blocker");
        assert_eq!(w.step, Step::Dates);
        assert_eq!(w.blockers(today), vec![Blocker::NoConversionDate]);
        assert!(!w.next(today));
        w.conversion_date = Some(day("2026-12-01"));
        assert_eq!(
            w.blockers(today),
            vec![Blocker::Dates(DateProblem::ConversionInFuture)]
        );
        w.conversion_date = Some(day("2026-10-27"));
        w.company_has_activity = true;
        assert_eq!(w.blockers(today), vec![Blocker::CompanyNotEmpty]);
        w.company_has_activity = false;
        assert!(w.next(today));
        assert_eq!(w.step, Step::Files);
        let missing = w.blockers(today);
        assert!(missing.contains(&Blocker::MissingFile {
            slot: "qbd_open_invoices"
        }));
        assert!(missing.contains(&Blocker::MissingFile {
            slot: "qbd_tb_opening"
        }));
        assert!(!missing.contains(&Blocker::MissingFile {
            slot: "qbd_other_names"
        }));
        assert!(
            !missing.contains(&Blocker::MissingFile {
                slot: "qbd_inventory_valuation"
            }),
            "no inventory balance"
        );
        let mut w2 = qbd_ready();
        w2.step = Step::Files;
        assert!(w2.can_continue(today));
        w2.next(today);
        assert_eq!(w2.step, Step::Mapping);
        assert_eq!(w2.blockers(today), vec![Blocker::MappingIncomplete]);
        w2.mapping_complete = true;
        assert!(w2.next(today));
        assert_eq!(w2.blockers(today), vec![Blocker::NotImported]);
        w2.imported = true;
        assert!(w2.next(today));
        assert_eq!(w2.step, Step::TieOut);
        assert!(!w2.back(), "never back past a committed import");
        assert_eq!(w2.blockers(today), vec![Blocker::NotVerified]);
        w2.verified = true;
        assert!(w2.next(today));
        assert_eq!(w2.step, Step::SpotCheck);
        assert!(w2.back());
        assert_eq!(w2.step, Step::TieOut);
        assert!(w2.next(today) && w2.next(today));
        assert_eq!(w2.step, Step::Done);
        assert!(!w2.next(today));
    }

    #[test]
    fn all_history_needs_no_opening_tb_but_inventory_needs_the_valuation() {
        let today = day(TODAY);
        let mut w = qbd_ready();
        w.step = Step::Files;
        w.files.remove("qbd_tb_opening");
        assert_eq!(
            w.blockers(today),
            vec![Blocker::MissingFile {
                slot: "qbd_tb_opening"
            }]
        );
        w.depth = HistoryDepth::All;
        assert!(w.can_continue(today));
        w.files.remove("qbd_inventory_valuation");
        assert!(w.can_continue(today));
        w.has_inventory = true;
        assert_eq!(
            w.blockers(today),
            vec![Blocker::MissingFile {
                slot: "qbd_inventory_valuation"
            }]
        );
    }

    #[test]
    fn several_journals_one_per_year_but_one_reference_tb() {
        let today = day(TODAY);
        let mut w = qbd_ready();
        w.step = Step::Files;
        w.drop_file("qbd_journal", file(None));
        assert_eq!(w.files["qbd_journal"].len(), 2);
        assert!(w.can_continue(today));
        // A second reference TB replaces the first.
        w.drop_file("qbd_tb_reference", file(Some("2026-10-27")));
        assert_eq!(w.files["qbd_tb_reference"].len(), 1);
        w.files
            .get_mut("qbd_tb_reference")
            .expect("slot")
            .push(file(Some("2026-10-27")));
        assert!(w.blockers(today).contains(&Blocker::OneFileOnly {
            slot: "qbd_tb_reference"
        }));
    }

    #[test]
    fn reports_must_be_dated_as_the_wizard_says() {
        let today = day(TODAY);
        let mut w = qbd_ready();
        w.step = Step::Files;
        w.drop_file("qbd_open_invoices", file(Some("2026-10-26")));
        w.drop_file("qbd_tb_opening", file(Some("2024-12-31")));
        let b = w.blockers(today);
        assert!(b.contains(&Blocker::WrongDate {
            slot: "qbd_open_invoices",
            want: day("2026-10-27")
        }));
        assert!(b.contains(&Blocker::WrongDate {
            slot: "qbd_tb_opening",
            want: day("2023-12-31")
        }));
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn row_problems_and_pending_uploads_block_files() {
        let today = day(TODAY);
        let mut w = qbd_ready();
        w.step = Step::Files;
        let mut bad = file(None);
        bad.problems = 3;
        w.drop_file("qbd_account_list", bad);
        let mut pending = file(None);
        pending.uploaded = false;
        w.drop_file("qbd_journal", pending);
        let b = w.blockers(today);
        assert!(b.contains(&Blocker::RowProblems {
            slot: "qbd_account_list"
        }));
        assert!(b.contains(&Blocker::NotUploaded {
            slot: "qbd_journal"
        }));
        assert!(!w.files["qbd_journal"][1].is_clean());
        for x in &b {
            assert_ne!(x.key(), "");
        }
    }

    #[test]
    fn other_sources_need_no_open_items_and_may_be_non_empty() {
        let today = day(TODAY);
        let mut w = Wizard::new(Source::Generic, 1);
        w.set_source(Source::AccountingPower);
        w.drop_file("ap_coa", file(None));
        w.set_source(Source::Generic);
        assert!(w.files.is_empty(), "a new source clears the files");
        w.step = Step::Dates;
        w.conversion_date = Some(day("2026-06-30"));
        w.company_has_activity = true;
        assert!(w.can_continue(today), "Q-P31h: a warning, not a block");
        w.opening_date = Some(day("2025-12-31"));
        w.step = Step::Files;
        for key in [
            "generic_coa",
            "generic_tb_opening",
            "generic_gl",
            "generic_tb_reference",
        ] {
            w.drop_file(key, file(None));
        }
        assert!(w.can_continue(today));
        assert!(!Source::Generic.requires_empty_company());
    }
}
