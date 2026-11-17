//! The payroll import wizard's rules (P30), with no egui and no I/O.
//!
//! - [`Provider`]: the picker, each provider's engine key and file mode.
//! - [`Step`]: `Choose → Map → Accounts → Check → Preview → Done`.
//! - [`PeriodInput`]: pay period and check date, all three required.
//! - [`template_for_headers`]: which saved layout fits a generic file.
//! - [`SampleGuard`]: only numbers and dates from the file may be shown.
//! - [`builtin_mapping`]: the desktop's column layouts per provider.
//! - [`TagPlan`]: each preview line's final tag and the "Require a tag on
//!   every line" gate before posting.
//!
//! Nothing here reads or adds an amount: the engine sums (non-negotiable 7).

use std::collections::BTreeMap;

use cl_core::CalendarDate;
use cl_core::ids::TagId;

/// The payroll services the picker offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Provider {
    /// Gusto payroll journal (one row per worker).
    Gusto,
    /// ADP Run general-ledger export (ready-made entries).
    AdpRun,
    /// The `QuickBooks` Payroll export file (Q-258).
    QuickBooksPayroll,
    /// Paychex Flex general-ledger summary (ready-made entries).
    Paychex,
    /// Square Payroll export (one row per worker, no pay date column).
    Square,
    /// The `OnPay` general-ledger summary (ready-made entries).
    OnPay,
    /// Rippling: read with the generic layout until real headers are known
    /// (Q-P30a, U-248).
    Rippling,
    /// Any other CSV: the owner maps the columns once.
    GenericCsv,
}

/// How a provider's file carries payroll.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileMode {
    /// One row per worker: columns are mapped, the engine sums.
    EmployeeLevel,
    /// One row per account line: descriptions are mapped to accounts.
    Prebuilt,
}

impl Provider {
    /// Every provider, in picker order.
    pub const ALL: [Provider; 8] = [
        Provider::Gusto,
        Provider::AdpRun,
        Provider::QuickBooksPayroll,
        Provider::Paychex,
        Provider::Square,
        Provider::OnPay,
        Provider::Rippling,
        Provider::GenericCsv,
    ];

    /// The engine's provider key (templates, description maps).
    #[must_use]
    pub const fn engine_key(self) -> &'static str {
        match self {
            Provider::Gusto => "gusto",
            Provider::AdpRun => "adp_run_gli",
            Provider::QuickBooksPayroll => "qbo_payroll",
            Provider::Paychex => "paychex_flex_gls",
            Provider::Square => "square_payroll",
            Provider::OnPay => "onpay_gl_summary",
            Provider::Rippling | Provider::GenericCsv => "custom",
        }
    }

    /// A stable key for control-channel state and i18n ids.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Provider::Gusto => "gusto",
            Provider::AdpRun => "adp_run",
            Provider::QuickBooksPayroll => "quickbooks_payroll",
            Provider::Paychex => "paychex",
            Provider::Square => "square",
            Provider::OnPay => "onpay",
            Provider::Rippling => "rippling",
            Provider::GenericCsv => "generic_csv",
        }
    }

    /// Reads [`Provider::key`].
    #[must_use]
    pub fn from_key(s: &str) -> Option<Provider> {
        Provider::ALL.into_iter().find(|p| p.key() == s)
    }

    /// The provider whose engine key the engine detected, if one of ours.
    #[must_use]
    pub fn from_engine_key(s: &str) -> Option<Provider> {
        Provider::ALL
            .into_iter()
            .filter(|p| !p.is_generic())
            .find(|p| p.engine_key() == s)
    }

    /// The file's mode.
    #[must_use]
    pub const fn mode(self) -> FileMode {
        match self {
            Provider::AdpRun | Provider::Paychex | Provider::OnPay => FileMode::Prebuilt,
            _ => FileMode::EmployeeLevel,
        }
    }

    /// Read with the generic layout (the owner's saved template).
    #[must_use]
    pub const fn is_generic(self) -> bool {
        matches!(self, Provider::Rippling | Provider::GenericCsv)
    }
}

/// The wizard's steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Step {
    /// Provider, file, pay period and check date.
    Choose,
    /// Columns (employee-level files only).
    Map,
    /// Accounts for each line.
    Accounts,
    /// The engine's row check.
    Check,
    /// The entries, one per pay date; Post.
    Preview,
    /// Posted.
    Done,
}

impl Step {
    /// Every step in order.
    pub const ALL: [Step; 6] = [
        Step::Choose,
        Step::Map,
        Step::Accounts,
        Step::Check,
        Step::Preview,
        Step::Done,
    ];

    /// A stable key (control-channel state, i18n ids).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Step::Choose => "choose",
            Step::Map => "map",
            Step::Accounts => "accounts",
            Step::Check => "check",
            Step::Preview => "preview",
            Step::Done => "done",
        }
    }

    /// The step after this one. `skip_map`: a ready-made file, or a generic
    /// file whose saved layout was applied (A2).
    #[must_use]
    pub const fn next(self, skip_map: bool) -> Step {
        match self {
            Step::Choose if skip_map => Step::Accounts,
            Step::Choose => Step::Map,
            Step::Map => Step::Accounts,
            Step::Accounts => Step::Check,
            Step::Check => Step::Preview,
            Step::Preview | Step::Done => Step::Done,
        }
    }

    /// The step before this one (Done has none: it is posted).
    #[must_use]
    pub const fn back(self, skip_map: bool) -> Option<Step> {
        match self {
            Step::Choose | Step::Done => None,
            Step::Map => Some(Step::Choose),
            Step::Accounts if skip_map => Some(Step::Choose),
            Step::Accounts => Some(Step::Map),
            Step::Check => Some(Step::Accounts),
            Step::Preview => Some(Step::Check),
        }
    }
}

/// Why the pay period can't be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeriodProblem {
    /// A date is missing.
    Missing,
    /// The period ends before it starts.
    EndsBeforeStart,
    /// The check date is in the future.
    CheckDateInFuture,
    /// The check date is before the period starts.
    CheckDateBeforeStart,
}

impl PeriodProblem {
    /// The i18n id of the message.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            PeriodProblem::Missing => "payroll.period.missing",
            PeriodProblem::EndsBeforeStart => "payroll.period.ends_before_start",
            PeriodProblem::CheckDateInFuture => "payroll.period.check_in_future",
            PeriodProblem::CheckDateBeforeStart => "payroll.period.check_before_start",
        }
    }
}

/// The pay period and check date the owner typed. For a file with several
/// pay dates: the first start to the last end, and the last pay date
/// (Q-P30i).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeriodInput {
    /// First day of the pay period.
    pub start: Option<CalendarDate>,
    /// Last day of the pay period.
    pub end: Option<CalendarDate>,
    /// The check (pay) date.
    pub check_date: Option<CalendarDate>,
}

impl PeriodInput {
    /// `(start, end, check date)` when usable.
    ///
    /// # Errors
    /// The first [`PeriodProblem`].
    pub fn checked(
        &self,
        today: CalendarDate,
    ) -> Result<(CalendarDate, CalendarDate, CalendarDate), PeriodProblem> {
        let (Some(s), Some(e), Some(c)) = (self.start, self.end, self.check_date) else {
            return Err(PeriodProblem::Missing);
        };
        if e < s {
            return Err(PeriodProblem::EndsBeforeStart);
        }
        if c > today {
            return Err(PeriodProblem::CheckDateInFuture);
        }
        if c < s {
            return Err(PeriodProblem::CheckDateBeforeStart);
        }
        Ok((s, e, c))
    }
}

/// A header as compared: trimmed, lower-case, inner spaces collapsed.
#[must_use]
pub fn normalize_header(h: &str) -> String {
    h.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A saved layout as [`template_for_headers`] sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateCandidate<T> {
    /// The caller's handle (the template id).
    pub id: T,
    /// The source columns it maps.
    pub sources: Vec<String>,
    /// When it last changed (RFC 3339 text sorts in time order).
    pub updated_at: Option<String>,
    /// Built into the engine (never used for generic files).
    pub is_system: bool,
}

/// The saved layout that fits `headers`: every source column it maps is
/// present (compared normalized); the most mapped columns wins; on a tie,
/// the most recently changed. System layouts are never picked.
#[must_use]
pub fn template_for_headers<'a, T>(
    headers: &[String],
    templates: &'a [TemplateCandidate<T>],
) -> Option<&'a TemplateCandidate<T>> {
    let have: Vec<String> = headers.iter().map(|h| normalize_header(h)).collect();
    templates
        .iter()
        .filter(|t| !t.is_system && !t.sources.is_empty())
        .filter(|t| {
            t.sources
                .iter()
                .all(|s| have.contains(&normalize_header(s)))
        })
        .max_by(|a, b| {
            a.sources
                .len()
                .cmp(&b.sources.len())
                .then_with(|| a.updated_at.cmp(&b.updated_at))
        })
}

/// What may be shown from a file's sample cells: numbers and dates only,
/// never text (a worker column holds names; Q-150).
#[derive(Clone, Copy, Debug, Default)]
pub struct SampleGuard;

impl SampleGuard {
    /// The cell when it is a number (money-like) or a date; else `None`.
    #[must_use]
    pub fn displayable(cell: &str) -> Option<&str> {
        let c = cell.trim();
        if c.is_empty() || c.len() > 24 {
            return None;
        }
        let numeric = c.chars().any(|ch| ch.is_ascii_digit())
            && c.chars().all(|ch| {
                ch.is_ascii_digit() || matches!(ch, '$' | ',' | '.' | '-' | '(' | ')' | ' ')
            });
        let date = c.chars().any(|ch| ch.is_ascii_digit())
            && c.chars()
                .all(|ch| ch.is_ascii_digit() || matches!(ch, '/' | '-'))
            && c.chars().filter(|ch| matches!(ch, '/' | '-')).count() == 2;
        (numeric || date).then_some(c)
    }
}

/// One column → field pair of a layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldColumn {
    /// The engine field (`gross_pay`).
    pub field: &'static str,
    /// The header in the file.
    pub header: String,
}

/// A column layout for an employee-level file, in plain data (cl-ui turns
/// it into the engine's `apply-mapping` body).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ColumnLayout {
    /// 0-based header row.
    pub header_row: u32,
    /// 0-based first data row.
    pub data_start_row: u32,
    /// Total rows at the end to drop.
    pub skip_footer_rows: u32,
    /// The file's date format.
    pub date_format: Option<&'static str>,
    /// Field → column.
    pub columns: Vec<FieldColumn>,
    /// Worker-column values that mark total rows.
    pub total_rows: Vec<&'static str>,
}

impl ColumnLayout {
    fn new(
        header_row: u32,
        footer: u32,
        date_format: &'static str,
        cols: &[(&'static str, &str)],
    ) -> Self {
        ColumnLayout {
            header_row,
            data_start_row: header_row + 1,
            skip_footer_rows: footer,
            date_format: Some(date_format),
            columns: cols
                .iter()
                .map(|(f, h)| FieldColumn {
                    field: f,
                    header: (*h).to_string(),
                })
                .collect(),
            total_rows: vec!["Total", "Totals", "Grand Total"],
        }
    }

    /// The column mapped to `field`.
    #[must_use]
    pub fn column(&self, field: &str) -> Option<&str> {
        self.columns
            .iter()
            .find(|c| c.field == field)
            .map(|c| c.header.as_str())
    }

    /// True when every mapped column is among `headers` (normalized).
    #[must_use]
    pub fn fits(&self, headers: &[String]) -> bool {
        let have: Vec<String> = headers.iter().map(|h| normalize_header(h)).collect();
        self.columns
            .iter()
            .all(|c| have.contains(&normalize_header(&c.header)))
    }
}

/// The engine fields an employee-level layout may map, with their i18n ids.
pub const MAPPABLE_FIELDS: [&str; 24] = [
    "employee_name",
    "department",
    "check_date",
    "pay_period_start",
    "pay_period_end",
    "gross_pay",
    "federal_income_tax",
    "state_income_tax",
    "local_income_tax",
    "social_security_ee",
    "medicare_ee",
    "other_ee_tax",
    "health_insurance_ee",
    "retirement_401k_ee",
    "hsa_ee",
    "other_deduction_ee",
    "reimbursement_ee",
    "net_pay",
    "social_security_er",
    "medicare_er",
    "futa_er",
    "suta_er",
    "other_er_tax",
    "health_insurance_er",
];

/// Fields the engine needs mapped.
pub const REQUIRED_FIELDS: [&str; 3] = ["employee_name", "gross_pay", "net_pay"];

/// The desktop's own layout for a provider's employee-level file.
///
/// - Gusto: the engine's layout plus `department ← Department` and the pay
///   period columns.
/// - Square: the engine's layout (no date column; the check date applies).
/// - `QuickBooks` Payroll: the desktop's own, because the engine's leaves out
///   Social Security and Medicare (it can't balance) and reads the pay date
///   from a period column; this one maps no date, so the check date applies.
/// - Generic and Rippling: none (the owner's saved layout or the Map step).
#[must_use]
pub fn builtin_mapping(p: Provider) -> Option<ColumnLayout> {
    match p {
        Provider::Gusto => Some(ColumnLayout::new(
            0,
            0,
            "YYYY-MM-DD",
            &[
                ("employee_name", "Employee Name"),
                ("department", "Department"),
                ("pay_period_start", "Pay Period Start"),
                ("pay_period_end", "Pay Period End"),
                ("check_date", "Check Date"),
                ("gross_pay", "Gross Pay"),
                ("federal_income_tax", "Federal Income Tax"),
                ("state_income_tax", "State Income Tax"),
                ("social_security_ee", "Social Security (Employee)"),
                ("medicare_ee", "Medicare (Employee)"),
                ("health_insurance_ee", "Health Insurance"),
                ("retirement_401k_ee", "401(k)"),
                ("reimbursement_ee", "Reimbursements"),
                ("net_pay", "Net Pay"),
                ("social_security_er", "Social Security (Employer)"),
                ("medicare_er", "Medicare (Employer)"),
                ("futa_er", "FUTA"),
                ("suta_er", "SUTA"),
            ],
        )),
        Provider::Square => Some(ColumnLayout::new(
            0,
            0,
            "YYYY-MM-DD",
            &[
                ("employee_name", "Name"),
                ("pay_period_start", "Pay Period Start"),
                ("pay_period_end", "Pay Period End"),
                ("gross_pay", "Gross Pay"),
                ("other_ee_tax", "Total Employee Taxes"),
                ("social_security_er", "Employer Social Security"),
                ("medicare_er", "Employer Medicare"),
                ("futa_er", "FUTA"),
                ("suta_er", "SUTA"),
                ("other_deduction_ee", "Total Deductions"),
                ("net_pay", "Net Pay"),
            ],
        )),
        Provider::QuickBooksPayroll => Some(ColumnLayout::new(
            3,
            1,
            "MM/DD/YYYY",
            &[
                ("employee_name", "Employee"),
                ("gross_pay", "Gross Pay"),
                ("federal_income_tax", "Federal Taxes"),
                ("state_income_tax", "State Taxes"),
                ("social_security_ee", "Social Security Employee"),
                ("medicare_ee", "Medicare Employee"),
                ("social_security_er", "Social Security Employer"),
                ("medicare_er", "Medicare Employer"),
                ("net_pay", "Net Pay"),
            ],
        )),
        _ => None,
    }
}

/// A layout the company saved (an engine template read back by cl-ui).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedLayout<T> {
    /// The template's id.
    pub id: T,
    /// The owner's name for it.
    pub name: String,
    /// The engine provider key it was saved for (`custom` = generic).
    pub provider_key: String,
    /// Built into the engine (never applied by the desktop).
    pub is_system: bool,
    /// When it last changed (RFC 3339 text).
    pub updated_at: Option<String>,
    /// The columns.
    pub layout: ColumnLayout,
    /// Department (as in the file) → tag, remembered with the layout.
    pub department_tags: BTreeMap<String, Option<TagId>>,
}

/// Which layout the Map step starts from (WP3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutChoice<T> {
    /// The company's saved layout (`id`). For a generic file the Map step
    /// is skipped with a "Using your saved layout" banner (A2).
    Saved(T),
    /// The desktop's own layout for this provider.
    Builtin,
    /// No layout fits: the owner maps the columns.
    Manual,
    /// A ready-made journal file: no columns to map.
    NotNeeded,
}

/// Picks the layout for a file of provider `p` with `headers`:
///
/// - a ready-made file needs none;
/// - a known provider uses the company's own saved layout for it when it
///   fits the file (the newest), else the desktop's built-in one;
/// - a generic file (and Rippling) uses [`template_for_headers`] over the
///   company's generic layouts, else the owner maps it.
#[must_use]
pub fn choose_layout<T: Clone>(
    p: Provider,
    headers: &[String],
    saved: &[SavedLayout<T>],
) -> LayoutChoice<T> {
    if p.mode() == FileMode::Prebuilt {
        return LayoutChoice::NotNeeded;
    }
    let candidates: Vec<TemplateCandidate<T>> = saved
        .iter()
        .filter(|s| s.provider_key == p.engine_key())
        .map(|s| TemplateCandidate {
            id: s.id.clone(),
            sources: s.layout.columns.iter().map(|c| c.header.clone()).collect(),
            updated_at: s.updated_at.clone(),
            is_system: s.is_system,
        })
        .collect();
    if p.is_generic() {
        return template_for_headers(headers, &candidates)
            .map_or(LayoutChoice::Manual, |t| LayoutChoice::Saved(t.id.clone()));
    }
    let own = candidates
        .iter()
        .filter(|c| !c.is_system)
        .filter(|c| {
            let have: Vec<String> = headers.iter().map(|h| normalize_header(h)).collect();
            c.sources
                .iter()
                .all(|s| have.contains(&normalize_header(s)))
        })
        .max_by(|a, b| a.updated_at.cmp(&b.updated_at));
    match own {
        Some(c) => LayoutChoice::Saved(c.id.clone()),
        None if builtin_mapping(p).is_some() => LayoutChoice::Builtin,
        None => LayoutChoice::Manual,
    }
}

/// The name a saved layout gets ("Gusto layout", "Generic layout 3 columns"
/// is the caller's i18n; this is the stable part used to find it again).
#[must_use]
pub fn layout_name_key(p: Provider) -> &'static str {
    if p.is_generic() {
        "payroll.layout.name_generic"
    } else {
        "payroll.layout.name_provider"
    }
}

/// The engine field for a field name read back from a saved layout
/// (unknown fields are dropped: the desktop saves only these).
#[must_use]
pub fn mappable_field(name: &str) -> Option<&'static str> {
    MAPPABLE_FIELDS.iter().copied().find(|f| *f == name)
}

/// A layout from the owner's Map step: header row 0, no total rows
/// unless the worker column says so, and the pairs chosen.
#[must_use]
pub fn manual_layout(
    pairs: &[(&'static str, String)],
    date_format: Option<&'static str>,
) -> ColumnLayout {
    ColumnLayout {
        header_row: 0,
        data_start_row: 1,
        skip_footer_rows: 0,
        date_format,
        columns: pairs
            .iter()
            .map(|(f, h)| FieldColumn {
                field: f,
                header: h.clone(),
            })
            .collect(),
        total_rows: vec!["Total", "Totals", "Grand Total"],
    }
}

/// Guesses a date format from sample date cells (`MM/DD/YYYY` when they
/// use slashes, else ISO).
#[must_use]
pub fn guess_date_format<'a>(samples: impl IntoIterator<Item = &'a str>) -> &'static str {
    if samples.into_iter().any(|s| s.contains('/')) {
        "MM/DD/YYYY"
    } else {
        "YYYY-MM-DD"
    }
}

/// One preview line as the tag plan sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanLine {
    /// The entry's position in the preview.
    pub entry: usize,
    /// The line's position in the entry.
    pub line: usize,
    /// The entry's pay date.
    pub date: CalendarDate,
    /// The line's description, for a refusal ("Gross Wages Expense").
    pub description: String,
    /// The tag the engine will stamp (department map or Mode B mapping).
    pub preview_tag: Option<TagId>,
}

/// A line tag to send (`lineTags`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineTagOut {
    /// Entry position.
    pub entry: usize,
    /// Line position.
    pub line: usize,
    /// The entry's date (pins the preview).
    pub date: CalendarDate,
    /// The tag.
    pub tag: Option<TagId>,
}

/// The first line left without a tag while every line needs one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UntaggedLine {
    /// Its pay date.
    pub date: CalendarDate,
    /// Its description.
    pub description: String,
    /// Lines without a tag in all.
    pub count: usize,
}

/// Each preview line's tag: the owner's choice on the line, else the
/// department or mapping tag, else the company's default tag. The
/// post-level `tagId` is never used: it would overwrite department tags.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagPlan {
    /// The preview's lines in order.
    pub lines: Vec<PlanLine>,
    /// The owner's choices: `(entry, line)` → tag (`None` = no tag).
    pub overrides: BTreeMap<(usize, usize), Option<TagId>>,
    /// The company's default tag for new lines.
    pub default_tag: Option<TagId>,
}

impl TagPlan {
    /// A plan for these lines with no choices yet.
    #[must_use]
    pub fn new(lines: Vec<PlanLine>, default_tag: Option<TagId>) -> TagPlan {
        TagPlan {
            lines,
            overrides: BTreeMap::new(),
            default_tag,
        }
    }

    /// The owner sets (or clears with `None`) a line's tag.
    pub fn set(&mut self, entry: usize, line: usize, tag: Option<TagId>) {
        self.overrides.insert((entry, line), tag);
    }

    /// A line's final tag.
    #[must_use]
    pub fn final_tag(&self, l: &PlanLine) -> Option<TagId> {
        match self.overrides.get(&(l.entry, l.line)) {
            Some(choice) => *choice,
            None => l.preview_tag.or(self.default_tag),
        }
    }

    /// The gate before `/post`: with `require_tag`, every line must end up
    /// with a tag; the refusal names the first untagged line. On success,
    /// the lines whose final tag differs from the preview's.
    ///
    /// # Errors
    /// [`UntaggedLine`] when a tag is required and missing.
    pub fn gate(&self, require_tag: bool) -> Result<Vec<LineTagOut>, UntaggedLine> {
        if require_tag {
            let untagged: Vec<&PlanLine> = self
                .lines
                .iter()
                .filter(|l| self.final_tag(l).is_none())
                .collect();
            if let Some(first) = untagged.first() {
                return Err(UntaggedLine {
                    date: first.date,
                    description: first.description.clone(),
                    count: untagged.len(),
                });
            }
        }
        Ok(self
            .lines
            .iter()
            .filter_map(|l| {
                let tag = self.final_tag(l);
                (tag != l.preview_tag).then_some(LineTagOut {
                    entry: l.entry,
                    line: l.line,
                    date: l.date,
                    tag,
                })
            })
            .collect())
    }
}

/// The line types an employee-level import posts, in the engine's order,
/// with whether each is a debit (expense) line. Used for the Accounts step.
pub const LINE_TYPES: [(&str, bool); 19] = [
    ("gross_wages_expense", true),
    ("officer_wages_expense", true),
    ("employer_tax_expense", true),
    ("employer_benefits_expense", true),
    ("reimbursement_expense", true),
    ("contractor_expense", true),
    ("fit_payable", false),
    ("sit_payable", false),
    ("local_tax_payable", false),
    ("ss_payable", false),
    ("medicare_payable", false),
    ("futa_payable", false),
    ("suta_payable", false),
    ("health_ins_payable", false),
    ("retirement_payable", false),
    ("other_deduction_payable", false),
    ("other_tax_payable", false),
    ("payroll_clearing", false),
    ("contractor_payable", false),
];

/// The line type paid from the bank (net pay, Q-P30f).
pub const NET_PAY_LINE: &str = "payroll_clearing";

#[cfg(test)]
#[path = "payroll_import_tests.rs"]
mod tests;
