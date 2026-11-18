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

// M10: no floating point anywhere near payroll (as in cl-api and cl-core).
#![deny(clippy::float_arithmetic)]

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
        if c.is_empty() || c.len() > 24 || looks_like_ssn(c) {
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

    /// True when the cell is shaped like an amount (digits with money
    /// marks; not a date). Shape only: nothing is parsed.
    #[must_use]
    pub fn is_amount(cell: &str) -> bool {
        let c = cell.trim();
        c.chars().any(|ch| ch.is_ascii_digit())
            && c.chars().all(|ch| {
                ch.is_ascii_digit() || matches!(ch, '$' | ',' | '.' | '-' | '(' | ')' | ' ')
            })
            && !looks_like_ssn(c)
            && c.chars().filter(|ch| *ch == '-').count() < 2
    }
}

/// `123-45-6789` or `123 45 6789`: a taxpayer number, never shown (M9).
fn looks_like_ssn(c: &str) -> bool {
    if c.len() == 9 && c.chars().all(|ch| ch.is_ascii_digit()) {
        return true;
    }
    let groups: Vec<&str> = c.split(['-', ' ']).collect();
    groups.len() == 3
        && groups.iter().map(|g| g.len()).eq([3, 2, 4])
        && groups
            .iter()
            .all(|g| g.chars().all(|ch| ch.is_ascii_digit()))
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
    /// The file's columns the owner left out when the layout was saved
    /// (seen on Match columns, not matched). `None`: a layout saved before
    /// this was kept, so every unmatched column counts as new (S4).
    pub left_out: Option<Vec<String>>,
}

/// The columns of a file (`headers`, with sample `rows`) that a saved
/// `layout` neither matches nor left out, and whose sample cells hold
/// amounts (S4): such a file stops on Match columns instead of applying the
/// layout silently. Judged by the cells' shape only ([`SampleGuard`]);
/// nothing is parsed. Returns the headers, in file order.
#[must_use]
pub fn new_amount_columns<T>(
    saved: &SavedLayout<T>,
    headers: &[String],
    rows: &[Vec<String>],
) -> Vec<String> {
    let known: Vec<String> = saved
        .layout
        .columns
        .iter()
        .map(|c| normalize_header(&c.header))
        .chain(saved.left_out.iter().flatten().map(|h| normalize_header(h)))
        .collect();
    headers
        .iter()
        .enumerate()
        .filter(|(_, h)| !h.trim().is_empty() && !known.contains(&normalize_header(h)))
        .filter(|(i, _)| {
            rows.iter()
                .filter_map(|r| r.get(*i))
                .any(|c| SampleGuard::is_amount(c))
        })
        .map(|(_, h)| h.clone())
        .collect()
}

/// The file's columns `layout` does not match (saved as left out).
#[must_use]
pub fn unmatched_columns(layout: &ColumnLayout, headers: &[String]) -> Vec<String> {
    let mapped: Vec<String> = layout
        .columns
        .iter()
        .map(|c| normalize_header(&c.header))
        .collect();
    headers
        .iter()
        .filter(|h| !h.trim().is_empty() && !mapped.contains(&normalize_header(h)))
        .cloned()
        .collect()
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

/// An account as the Accounts step's matcher sees it (cl-ui fills it from
/// the chart; only active accounts are passed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChartRow<T> {
    /// The account's id.
    pub id: T,
    /// Its name (never its number: numbering differs between companies).
    pub name: String,
    /// What kind of account it is.
    pub kind: ChartKind,
    /// A bank account of the checking kind (detail type or name).
    pub checking: bool,
}

/// The account kinds a payroll line may go to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartKind {
    /// Expense or cost of goods sold.
    Expense,
    /// A liability.
    Liability,
    /// A bank account.
    Bank,
    /// Anything else (never pre-selected).
    Other,
}

/// One way a name can match: it holds one of `any` as a word start and
/// none of `none`.
type Tier = (&'static [&'static str], &'static [&'static str]);

/// The generic payroll-liability names every payable falls back to.
const LIABILITY_FALLBACK: Tier = (
    &[
        "payroll liabilit",
        "payroll tax",
        "payroll payable",
        "payroll withh",
    ],
    &["sales"],
);

/// The kind a line's account must be and its name tiers, most specific
/// first. Net pay has its own rule ([`confident_account`]).
fn line_rule(line_type: &str) -> Option<(ChartKind, &'static [Tier])> {
    use ChartKind::{Expense, Liability};
    const NO: &[&str] = &[];
    Some(match line_type {
        "gross_wages_expense" => (
            Expense,
            &[
                (&["wage", "salar"], &["officer", "tax", "benefit"]),
                (
                    &["payroll"],
                    &["tax", "benefit", "fee", "service", "processing"],
                ),
            ],
        ),
        "officer_wages_expense" => (Expense, &[(&["officer"], &["tax"])]),
        "employer_tax_expense" => (Expense, &[(&["payroll tax", "employer tax"], NO)]),
        "employer_benefits_expense" => (
            Expense,
            &[
                (&["employee benefit", "benefit"], NO),
                (&["health ins"], NO),
            ],
        ),
        "reimbursement_expense" => (Expense, &[(&["reimburs"], NO)]),
        "contractor_expense" => (
            Expense,
            &[(&["contract labor", "contractor", "subcontract"], NO)],
        ),
        "fit_payable" => (
            Liability,
            &[(&["federal"], &["unemploy", "futa"]), LIABILITY_FALLBACK],
        ),
        "sit_payable" => (
            Liability,
            &[
                (&["state"], &["unemploy", "suta", "sales"]),
                LIABILITY_FALLBACK,
            ],
        ),
        "local_tax_payable" => (Liability, &[(&["local"], &["sales"]), LIABILITY_FALLBACK]),
        "ss_payable" => (
            Liability,
            &[(&["social security", "fica"], NO), LIABILITY_FALLBACK],
        ),
        "medicare_payable" => (Liability, &[(&["medicare"], NO), LIABILITY_FALLBACK]),
        "futa_payable" => (
            Liability,
            &[(&["futa", "federal unemploy"], NO), LIABILITY_FALLBACK],
        ),
        "suta_payable" => (
            Liability,
            &[(&["suta", "state unemploy"], NO), LIABILITY_FALLBACK],
        ),
        "health_ins_payable" => (Liability, &[(&["health"], NO), LIABILITY_FALLBACK]),
        "retirement_payable" => (
            Liability,
            &[(&["401", "retirement"], NO), LIABILITY_FALLBACK],
        ),
        "other_deduction_payable" => (Liability, &[(&["deduction"], NO), LIABILITY_FALLBACK]),
        "other_tax_payable" => (Liability, &[LIABILITY_FALLBACK]),
        _ => return None,
    })
}

/// True when `term` starts a word of `name` (both lower case): "state"
/// does not match "Real Estate", "fit" never matches "Benefits".
fn has_word_start(name: &str, term: &str) -> bool {
    name.match_indices(term).any(|(i, _)| {
        name[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric())
    })
}

fn tier_matches(name: &str, (any, none): Tier) -> bool {
    let n = name.to_lowercase();
    any.iter().any(|t| has_word_start(&n, t)) && !none.iter().any(|t| has_word_start(&n, t))
}

/// True when `row` is a sensible account for `line_type` by its kind and
/// name: how an engine suggestion is checked before it is pre-selected
/// (the engine's own matcher goes by account number and loose words, so
/// "6000" picks Advertising in a chart numbered differently; P30 QA-1).
/// Net pay never fits by name alone (see [`confident_account`]).
#[must_use]
pub fn account_fits<T>(line_type: &str, row: &ChartRow<T>) -> bool {
    line_rule(line_type).is_some_and(|(kind, tiers)| {
        row.kind == kind && tiers.iter().any(|t| tier_matches(&row.name, *t))
    })
}

/// The one account the name rules pick for `line_type`, or `None` when no
/// account (or more than one equally good) matches: the owner then
/// chooses. Tiers are tried most specific first; a tier with several
/// matches is skipped for the next one.
///
/// Net pay: a bank account named for payroll, else the only checking
/// account, else the only bank account; never the first of several.
#[must_use]
pub fn confident_account<T: Copy>(line_type: &str, rows: &[ChartRow<T>]) -> Option<T> {
    let only = |it: &mut dyn Iterator<Item = &ChartRow<T>>| {
        let first = it.next()?;
        it.next().is_none().then_some(first.id)
    };
    if line_type == NET_PAY_LINE {
        let banks = || rows.iter().filter(|r| r.kind == ChartKind::Bank);
        return only(&mut banks().filter(|r| tier_matches(&r.name, (&["payroll"], &[]))))
            .or_else(|| only(&mut banks().filter(|r| r.checking)))
            .or_else(|| only(&mut banks()));
    }
    let (kind, tiers) = line_rule(line_type)?;
    tiers.iter().find_map(|t| {
        only(
            &mut rows
                .iter()
                .filter(|r| r.kind == kind && tier_matches(&r.name, *t)),
        )
    })
}

/// The line type a ready-made file's description most likely is (for its
/// starting account only; the owner sees and changes every one). `debit`:
/// the file puts it on the debit side.
#[must_use]
pub fn line_for_description(description: &str, debit: bool) -> &'static str {
    let d = description.to_lowercase();
    let has = |terms: &[&str]| terms.iter().any(|t| has_word_start(&d, t));
    if d.split(|c: char| !c.is_alphanumeric()).any(|w| w == "net") {
        return NET_PAY_LINE;
    }
    if debit {
        if has(&["reimburs"]) {
            "reimbursement_expense"
        } else if has(&[
            "tax",
            "social security",
            "medicare",
            "futa",
            "suta",
            "unemploy",
            "fica",
        ]) {
            "employer_tax_expense"
        } else if has(&["benefit", "health", "401", "retirement", "insurance"]) {
            "employer_benefits_expense"
        } else if has(&["officer"]) {
            "officer_wages_expense"
        } else {
            "gross_wages_expense"
        }
    } else if has(&["futa", "federal unemploy"]) {
        "futa_payable"
    } else if has(&["suta", "state unemploy"]) {
        "suta_payable"
    } else if has(&["social security", "fica"]) {
        "ss_payable"
    } else if has(&["medicare"]) {
        "medicare_payable"
    } else if has(&["federal"]) {
        "fit_payable"
    } else if has(&["state"]) {
        "sit_payable"
    } else if has(&["local"]) {
        "local_tax_payable"
    } else if has(&["health"]) {
        "health_ins_payable"
    } else if has(&["401", "retirement"]) {
        "retirement_payable"
    } else if has(&["tax"]) {
        "other_tax_payable"
    } else {
        "other_deduction_payable"
    }
}

/// The Accounts step's starting choices for `lines`: the account the
/// company used last time for the line (`remembered`, from an earlier
/// import), else the engine's suggestion when it [fits](account_fits),
/// else [`confident_account`]. Lines with none stay empty ("Choose an
/// account") and keep Next disabled. Accounts not in `rows` (inactive or
/// gone) are never pre-selected.
#[must_use]
pub fn default_accounts<T: Copy + Eq>(
    lines: &[&str],
    rows: &[ChartRow<T>],
    engine: &BTreeMap<String, T>,
    remembered: &BTreeMap<String, T>,
) -> BTreeMap<String, T> {
    let row = |id: T| rows.iter().find(|r| r.id == id);
    lines
        .iter()
        .filter_map(|line| {
            let pick = remembered
                .get(*line)
                .copied()
                .filter(|id| row(*id).is_some())
                // Net pay: never the engine's pick (it takes the first
                // account named "Cash" or "Checking"), only the rule's.
                .or_else(|| {
                    engine
                        .get(*line)
                        .filter(|_| *line != NET_PAY_LINE)
                        .copied()
                        .filter(|id| row(*id).is_some_and(|r| account_fits(line, r)))
                })
                .or_else(|| confident_account(line, rows))?;
            Some(((*line).to_string(), pick))
        })
        .collect()
}

#[cfg(test)]
#[path = "payroll_import_tests.rs"]
mod tests;
