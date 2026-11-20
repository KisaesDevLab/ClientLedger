//! Import Payroll (P30, `payroll.import`): a six-step wizard that turns the
//! payroll the owner's payroll service already ran into one journal entry
//! per pay date, plus a History of earlier imports. No mockup exists; the
//! screen follows Import Bank File's step-list pattern (P25).
//!
//! 1. **Choose.** Payroll service, the file (≤ 10 MB, checked first), the pay
//!    period and check date (always sent). The upload happens on Next.
//! 2. **Match columns** (one row per worker). A known service uses the
//!    company's saved layout or the built-in one; a generic file whose
//!    saved layout fits skips this step. Only numbers and dates from the
//!    file are shown (`SampleGuard`); the worker column never is.
//! 3. **Accounts.** Line type → account (or file description → account for
//!    a ready-made journal file); net pay from a bank account by default.
//! 4. **Check.** The engine's row check, in our words (never its message).
//! 5. **Review entries.** The engine's entries and totals; tags per line;
//!    the "Require a tag on every line" gate before Post.
//! 6. **Done.** Entries posted and the expected Bank Feed withdrawals.
//!
//! Post: one key per body (a changed body — confirmation, password, tags —
//! gets a new key); never sent again on its own. A lost answer offers
//! "Check status" and "Try again" (same key). Overlaps ask first; a closed
//! period asks for the closing-date password. Every post attempt waits for
//! one verified backup first (Q-P30h; continuing it with the password or
//! Post anyway reuses that backup); a failed backup posts nothing.
//!
//! Control-channel state: steps, counts, flags and the provider key only;
//! never amounts, names, departments, file names or cell text.

// M10: no floating point in payroll code (money is `rust_decimal`); the
// drawing's pixel sums are allowed where they are, one function each.
#![deny(clippy::float_arithmetic)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use cl_api::payroll::{
    ColumnMap, ColumnSource, DescriptionMapping, ImportMode, LineTag, Overlap, PayrollRefusal,
    PayrollSessionId, PayrollTemplate, PostOutcome, PostRequest, Preview, ReverseOutcome,
    SessionPage, SessionStatus, SkipRule, UploadOutcome, UploadRequest, ValidationSummary,
    payroll_refusal,
};
use cl_api::{
    Account, AccountFilter, AccountType, ApiError, IdempotencyKey, OrphanLabel, RequestGroup,
    Session, Tag,
};
use cl_core::ids::{AccountId, TagId, TransactionId};
use cl_core::{CalendarDate, Secret};
use cl_model::CommandId;
use cl_model::payroll_import::{
    ChartKind, ChartRow, ColumnLayout, FieldColumn, FileMode, LayoutChoice, PeriodInput,
    PeriodProblem, PlanLine, Provider, REQUIRED_FIELDS, SavedLayout, Step, TagPlan, UntaggedLine,
    builtin_mapping, choose_layout, mappable_field,
};
use cl_model::permissions::Door;
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink, Task};
use crate::i18n::t;
use crate::windows::Screen;

pub(crate) mod draw;
#[cfg(test)]
mod tests;

/// The command and window kind.
pub const KIND: &str = "payroll.import";

/// The Import Payroll window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    _open: Option<(cl_core::ids::TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND).then(|| {
        let mut s = PayrollImportScreen::new(Arc::clone(link), today);
        s.take_history_request();
        Box::new(s) as Box<_>
    })
}

/// Home's "payroll import not posted yet" link: the window opens (or comes
/// to the front) on History, showing only the unposted imports.
static HISTORY_UNPOSTED: std::sync::Mutex<bool> = std::sync::Mutex::new(false);

/// Asks the Import Payroll window to show History filtered to unposted
/// imports (taken by the next window made, or the open one's next frame).
pub fn open_history_unposted() {
    *HISTORY_UNPOSTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
}

/// Drafts whose post's answer was lost (QA-15, B1): kept across windows so
/// History offers "Check status" and never Discard for them until a read
/// shows them posted or gone. Ids only, per company link (the link's
/// address: one open company, and tests' demo links never meet).
static UNSETTLED: std::sync::Mutex<BTreeSet<(usize, PayrollSessionId)>> =
    std::sync::Mutex::new(BTreeSet::new());

fn unsettled() -> std::sync::MutexGuard<'static, BTreeSet<(usize, PayrollSessionId)>> {
    UNSETTLED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn take_history_unposted() -> bool {
    std::mem::take(
        &mut *HISTORY_UNPOSTED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Reads the payroll file the owner chose (on a background thread),
/// refusing it before anything is sent when it is over the engine's limit.
fn read_payroll_file(path: &std::path::Path) -> crate::screens::write_check::attach::FileRead {
    let meta = std::fs::metadata(path).map_err(|_| "payroll.file.open")?;
    if !meta.is_file() {
        return Err("payroll.file.open");
    }
    if meta.len() > cl_api::payroll::MAX_UPLOAD_BYTES as u64 {
        return Err("payroll.file.too_large");
    }
    let bytes = std::fs::read(path).map_err(|_| "payroll.file.open")?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok((name, bytes))
}

/// A request in flight or already answered (the demo answers at once).
pub(crate) enum Job<T> {
    Running(Task<T>),
    Done(Option<Result<T, ApiError>>),
}

impl<T> Job<T> {
    fn take(&mut self) -> Option<Result<T, ApiError>> {
        match self {
            Job::Running(t) => t.try_take(),
            Job::Done(v) => v.take(),
        }
    }
}

fn poll<T>(slot: &mut Option<Job<T>>) -> Option<Result<T, ApiError>> {
    let r = slot.as_mut()?.take()?;
    *slot = None;
    Some(r)
}

/// Which part of the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// The wizard.
    Import,
    /// Earlier imports.
    History,
}

/// What went wrong (shown in the step; never the engine's text).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Trouble {
    Refused(PayrollRefusal),
    Unknown,
    PasswordWrong,
    PasswordWait,
    Restricted,
    Unsupported,
    EngineDown,
    /// The books stopped before the post was sent (QA-15): nothing was
    /// posted; Post may be pressed again once they run.
    NotSent,
    /// The books are running again after `EngineDown` or `NotSent`
    /// (`post`: it was the post that wasn't sent).
    EngineBack {
        post: bool,
    },
    FileUnreadable,
    /// 404 on post or Check status: the import was discarded (another
    /// window, History or Home); nothing was posted.
    Discarded,
    Other,
}

impl Trouble {
    fn of(e: &ApiError) -> Trouble {
        if let Some(r) = payroll_refusal(e) {
            return Trouble::Refused(r);
        }
        match e {
            ApiError::OutcomeUnknown { .. } => Trouble::Unknown,
            ApiError::ClosingPasswordRejected => Trouble::PasswordWrong,
            ApiError::ClosingPasswordRateLimited { .. } => Trouble::PasswordWait,
            ApiError::Sensitive(_) | ApiError::Forbidden { .. } => Trouble::Restricted,
            ApiError::Local(cl_api::LocalValidation::Unsupported) => Trouble::Unsupported,
            ApiError::EngineUnavailable { .. } => Trouble::EngineDown,
            ApiError::Validation(_) => Trouble::FileUnreadable,
            _ => Trouble::Other,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Trouble::Refused(r) => match r {
                PayrollRefusal::AlreadyPosted => "already_posted",
                PayrollRefusal::NotReady => "not_ready",
                PayrollRefusal::DuplicateFile { .. } | PayrollRefusal::DuplicateRun { .. } => {
                    "duplicate"
                }
                PayrollRefusal::PreviewChanged => "preview_changed",
                PayrollRefusal::NotPosted => "not_posted",
                PayrollRefusal::Unbalanced { .. } => "unbalanced",
                PayrollRefusal::Unmapped { .. } => "unmapped",
                PayrollRefusal::AmountInvalid => "amounts",
                PayrollRefusal::DiscardUnsupported => "other",
                PayrollRefusal::DateInvalid { .. } => "date_invalid",
            },
            Trouble::Unknown => "unknown",
            Trouble::PasswordWrong => "password_wrong",
            Trouble::PasswordWait => "password_wait",
            Trouble::Restricted => "restricted",
            Trouble::Unsupported => "unsupported",
            Trouble::EngineDown => "engine_down",
            Trouble::NotSent => "not_sent",
            Trouble::EngineBack { post: true } => "engine_back_post",
            Trouble::EngineBack { post: false } => "engine_back",
            Trouble::FileUnreadable => "file_unreadable",
            Trouble::Discarded => "discarded",
            Trouble::Other => "other",
        }
    }
}

/// A body's identity for its key: the same body keeps its key.
#[derive(Clone, Debug, PartialEq, Eq)]
struct BodyPrint {
    force: bool,
    password: bool,
    tags: Vec<LineTagPrint>,
    departments: Vec<(String, Option<TagId>)>,
    token: Option<String>,
}

type LineTagPrint = (usize, usize, Option<TagId>);

/// What a verified backup before posting covers (Q-P30h): the draft and
/// its entries as posted, without the post's authorization (Post anyway,
/// the closing-date password). Continuing the same post after the
/// engine's question reuses the backup; a changed draft does not.
#[derive(Clone, Debug, PartialEq, Eq)]
struct DraftPrint {
    id: PayrollSessionId,
    tags: Vec<LineTagPrint>,
    departments: Vec<(String, Option<TagId>)>,
    token: Option<String>,
}

/// A post made by Post and held while the backup before it is made
/// (Q-P30h). Sent exactly as made: the backup never changes the body or
/// its key.
struct HeldPost {
    id: PayrollSessionId,
    line_tags: Vec<LineTag>,
    departments: BTreeMap<String, Option<TagId>>,
    token: Option<String>,
    force: bool,
    password: Option<Secret>,
    key: IdempotencyKey,
    /// The draft this post is for (what its backup covers).
    draft: DraftPrint,
}

/// A reverse body's identity for its key (S2): the import, the reason and
/// a hash of the closing-date password (never the password; no Debug, so
/// it can't reach a log).
#[derive(Clone, PartialEq, Eq)]
struct ReversePrint {
    id: PayrollSessionId,
    reason: String,
    password: Option<[u8; 32]>,
}

impl ReversePrint {
    fn of(id: PayrollSessionId, reason: &str, password: Option<&Secret>) -> ReversePrint {
        use sha2::Digest as _;
        ReversePrint {
            id,
            reason: reason.to_string(),
            password: password.map(|p| sha2::Sha256::digest(p.expose().as_bytes()).into()),
        }
    }
}

/// The Import Payroll window.
pub struct PayrollImportScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    today: CalendarDate,
    pub(crate) view: View,
    pub(crate) step: Step,
    pub(crate) visited: BTreeSet<Step>,
    // Choose.
    pub(crate) provider: Provider,
    pick_rx: Option<Receiver<Vec<std::path::PathBuf>>>,
    /// The chosen file, being read off the egui thread.
    reading: Option<Receiver<crate::screens::write_check::attach::FileRead>>,
    pub(crate) file: Option<(String, Vec<u8>)>,
    pub(crate) file_error: Option<&'static str>,
    pub(crate) period_text: [String; 3],
    pub(crate) period_problem: Option<PeriodProblem>,
    upload_job: Option<Job<UploadOutcome>>,
    pub(crate) upload: Option<UploadOutcome>,
    /// The posted imports, read when the engine says this payroll may be
    /// one of them, to name the earlier post's date (QA-12). A failed read
    /// only leaves the warning undated.
    prior_job: Option<Job<SessionPage>>,
    pub(crate) prior: Vec<cl_api::payroll::SessionSummary>,
    // Map.
    templates_job: Option<Job<Vec<PayrollTemplate>>>,
    templates: Vec<PayrollTemplate>,
    template_job: Option<Job<PayrollTemplate>>,
    pub(crate) layout: ColumnLayout,
    pub(crate) layout_choice: Option<LayoutChoice<uuid::Uuid>>,
    pub(crate) save_layout: bool,
    /// Columns with amounts the saved layout doesn't cover (S4): the file
    /// stopped on Match columns instead of applying it silently.
    pub(crate) new_columns: Vec<String>,
    pub(crate) departments: Vec<String>,
    /// Every department in the file, from the check (S3); `None` until
    /// then or from an engine without the list.
    pub(crate) file_departments: Option<Vec<String>>,
    pub(crate) departments_truncated: bool,
    pub(crate) department_tags: BTreeMap<String, Option<TagId>>,
    map_job: Option<Job<()>>,
    // Accounts.
    chart_job: Option<Job<Vec<Account>>>,
    pub(crate) chart: Vec<Account>,
    tags_job: Option<Job<Vec<Tag>>>,
    pub(crate) tags: Vec<Tag>,
    accounts_job: Option<Job<AccountHints>>,
    /// The engine's suggestions and the saved accounts, until the chart
    /// is here to check them against.
    account_hints: Option<AccountHints>,
    /// The active chart as the matcher sees it.
    pub(crate) chart_rows: Vec<ChartRow<AccountId>>,
    pub(crate) line_accounts: BTreeMap<String, AccountId>,
    desc_job: Option<Job<Vec<DescriptionMapping>>>,
    pub(crate) descriptions: Vec<DescriptionMapping>,
    save_accounts_job: Option<Job<()>>,
    // Check.
    validate_job: Option<Job<ValidationSummary>>,
    pub(crate) validation: Option<ValidationSummary>,
    // Preview.
    preview_job: Option<Job<Preview>>,
    pub(crate) preview: Option<Preview>,
    pub(crate) plan: TagPlan,
    pub(crate) untagged: Option<UntaggedLine>,
    /// The company's tag rule and default tag as last read (QA-14): on
    /// the Check and Review steps, on Post and when Preferences change.
    pub(crate) rules: cl_model::lists::tags::TagRules,
    /// Change notices: a tag or tag-option change re-reads the rules.
    bus: crate::data_bus::Subscription,
    // Post.
    key: Option<(BodyPrint, IdempotencyKey)>,
    pub(crate) force: bool,
    pub(crate) password: Option<Secret>,
    pub(crate) password_text: String,
    /// The closed-period prompt is open (with the closing date, if known).
    #[allow(clippy::option_option)] // outer: open or not; inner: the date if known
    pub(crate) asking_password: Option<Option<CalendarDate>>,
    /// The closing date the engine last named (kept for the prompt after a
    /// wrong password, QA-7).
    lock_date: Option<CalendarDate>,
    /// The prompt just opened: focus its field and scroll it into view.
    pub(crate) password_focus: bool,
    pub(crate) overlaps: Option<Vec<Overlap>>,
    post_job: Option<Job<PostOutcome>>,
    /// Q-P30h: the verified backup taken before the post goes out.
    backup_job: Option<Job<Result<(), cl_company::CompanyError>>>,
    /// The post waiting for that backup: exactly the body and key Post
    /// made, sent as is once the backup is made.
    held_post: Option<HeldPost>,
    /// The draft the last verified backup was made for, kept only while
    /// the engine's answer to that post is a question (the closing-date
    /// password, the overlap check): continuing it sends without a second
    /// backup. Any other answer, a stop or a failure clears it.
    backed_up: Option<DraftPrint>,
    /// The backup failed: nothing was posted (no "post without backup").
    pub(crate) backup_failed: Option<cl_company::CompanyError>,
    /// The owner stopped the post while the backup was being made.
    pub(crate) backup_stopped: bool,
    /// P31 QA-20: the close prompt is up; a finished backup parks the post.
    close_asked: bool,
    /// The post whose backup finished under the close prompt (sent on Keep).
    parked_post: Option<HeldPost>,
    /// P31 QA-2 (Q-P31ad): the inline backup passphrase.
    pub(crate) passphrase: crate::widgets::passphrase::PassphraseAsk,
    /// No backup passphrase is set: the boxes show above Post.
    pub(crate) need_passphrase: bool,
    status_job: Option<Job<cl_api::payroll::SessionSummary>>,
    pub(crate) status_answer: Option<bool>,
    pub(crate) trouble: Option<Trouble>,
    pub(crate) posted: Option<Vec<TransactionId>>,
    pub(crate) posts_sent: u32,
    /// A post's answer was lost: it may still post, so this window never
    /// discards the draft (B1). Cleared only by a definite answer to the
    /// post itself, or Check status saying it posted.
    pub(crate) post_unsettled: bool,
    // History.
    history_job: Option<Job<SessionPage>>,
    pub(crate) history: Option<SessionPage>,
    pub(crate) unposted_only: bool,
    pub(crate) reversing: Option<(PayrollSessionId, String)>,
    reverse_key: Option<(ReversePrint, IdempotencyKey)>,
    reverse_job: Option<Job<ReverseOutcome>>,
    /// A reverse's answer was lost (S2): its body is locked until a
    /// definite answer, so Try again resends exactly what was sent.
    pub(crate) reverse_unsettled: bool,
    pub(crate) reversed: Option<u64>,
    discard_job: Option<Job<()>>,
}

fn parse_date(s: &str) -> Option<CalendarDate> {
    let s = s.trim();
    if let Ok(d) = CalendarDate::parse_wire(s) {
        return Some(d);
    }
    let mut it = s.split('/');
    let (m, d, y) = (it.next()?, it.next()?, it.next()?);
    CalendarDate::parse_wire(&format!("{y:0>4}-{m:0>2}-{d:0>2}")).ok()
}

/// A date as the screen shows and takes it.
pub(crate) fn show_date(d: CalendarDate) -> String {
    format!("{:02}/{:02}/{:04}", d.month(), d.day(), d.year())
}

/// The ColumnMap the engine takes for a layout; the file's other columns
/// (`headers` not matched) are saved as left out (S4).
fn column_map(
    l: &ColumnLayout,
    departments: &BTreeMap<String, Option<TagId>>,
    headers: &[String],
) -> ColumnMap {
    let mut mappings = BTreeMap::new();
    for c in &l.columns {
        let format = matches!(
            c.field,
            "check_date" | "pay_period_start" | "pay_period_end"
        )
        .then(|| l.date_format.map(str::to_string))
        .flatten();
        mappings.insert(
            c.field.to_string(),
            ColumnSource {
                source: c.header.clone(),
                format,
            },
        );
    }
    ColumnMap {
        header_row: l.header_row,
        data_start_row: l.data_start_row,
        skip_footer_rows: l.skip_footer_rows,
        date_format: l.date_format.map(str::to_string),
        mappings,
        skip_rules: vec![
            SkipRule {
                field: "employee_name".into(),
                values: None,
            },
            SkipRule {
                field: "employee_name".into(),
                values: Some(l.total_rows.iter().map(|s| (*s).to_string()).collect()),
            },
        ],
        department_tags: departments.clone(),
        left_out: Some(cl_model::payroll_import::unmatched_columns(l, headers)),
    }
}

/// The engine's suggested accounts and the company's saved ones, by line.
type AccountHints = (BTreeMap<String, AccountId>, BTreeMap<String, AccountId>);

/// True for a bank account (net pay's withdrawals match in the Bank Feed).
fn is_bank_account(a: &Account) -> bool {
    a.fields.account_type == AccountType::Asset
        && a.fields
            .detail_type
            .as_deref()
            .is_none_or(|d| cl_api::BANK_REGISTER_DETAIL_TYPES.contains(&d))
}

/// The active chart as the Accounts step's matcher sees it (QA-1).
pub(crate) fn chart_rows(chart: &[Account]) -> Vec<ChartRow<AccountId>> {
    chart
        .iter()
        .filter(|a| a.is_active)
        .map(|a| {
            let kind = match a.fields.account_type {
                AccountType::Expense | AccountType::Cogs => ChartKind::Expense,
                AccountType::Liability => ChartKind::Liability,
                AccountType::Asset if is_bank_account(a) => ChartKind::Bank,
                _ => ChartKind::Other,
            };
            let checking = kind == ChartKind::Bank
                && (a.fields.detail_type.as_deref() == Some("checking")
                    || a.fields.name.to_lowercase().contains("checking"));
            ChartRow {
                id: a.id,
                name: a.fields.name.clone(),
                kind,
                checking,
            }
        })
        .collect()
}
/// A saved template as the model's layout.
fn saved_layout(t: &PayrollTemplate) -> Option<SavedLayout<uuid::Uuid>> {
    let m = t.column_map.as_ref()?;
    let columns = m
        .mappings
        .iter()
        .filter_map(|(f, s)| {
            Some(FieldColumn {
                field: mappable_field(f)?,
                header: s.source.clone(),
            })
        })
        .collect();
    let date_format = match m.date_format.as_deref() {
        Some("MM/DD/YYYY") => Some("MM/DD/YYYY"),
        Some("YYYY-MM-DD") => Some("YYYY-MM-DD"),
        _ => None,
    };
    Some(SavedLayout {
        id: t.id,
        name: t.name.clone(),
        provider_key: t.provider_key.clone(),
        is_system: t.is_system,
        updated_at: t.updated_at.clone(),
        layout: ColumnLayout {
            header_row: m.header_row,
            data_start_row: m.data_start_row,
            skip_footer_rows: m.skip_footer_rows,
            date_format,
            columns,
            total_rows: vec!["Total", "Totals", "Grand Total"],
        },
        department_tags: m.department_tags.clone(),
        left_out: m.left_out.clone(),
    })
}

impl PayrollImportScreen {
    /// A fresh wizard on step 1.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, today: CalendarDate) -> PayrollImportScreen {
        let group = link.group().and_then(Result::ok);
        let bus = link.bus().subscribe();
        let mut s = PayrollImportScreen {
            rules: cl_model::lists::tags::TagRules::default(),
            bus,
            link,
            group,
            today,
            view: View::Import,
            step: Step::Choose,
            visited: BTreeSet::from([Step::Choose]),
            provider: Provider::Gusto,
            pick_rx: None,
            reading: None,
            file: None,
            file_error: None,
            period_text: Default::default(),
            period_problem: None,
            upload_job: None,
            upload: None,
            prior_job: None,
            prior: Vec::new(),
            templates_job: None,
            templates: Vec::new(),
            template_job: None,
            layout: ColumnLayout::default(),
            layout_choice: None,
            save_layout: true,
            new_columns: Vec::new(),
            departments: Vec::new(),
            file_departments: None,
            departments_truncated: false,
            department_tags: BTreeMap::new(),
            map_job: None,
            chart_job: None,
            chart: Vec::new(),
            tags_job: None,
            tags: Vec::new(),
            accounts_job: None,
            account_hints: None,
            chart_rows: Vec::new(),
            line_accounts: BTreeMap::new(),
            desc_job: None,
            descriptions: Vec::new(),
            save_accounts_job: None,
            validate_job: None,
            validation: None,
            preview_job: None,
            preview: None,
            plan: TagPlan::default(),
            untagged: None,
            key: None,
            force: false,
            password: None,
            password_text: String::new(),
            asking_password: None,
            lock_date: None,
            password_focus: false,
            overlaps: None,
            post_job: None,
            backup_job: None,
            backed_up: None,
            held_post: None,
            backup_failed: None,
            backup_stopped: false,
            close_asked: false,
            parked_post: None,
            passphrase: crate::widgets::passphrase::PassphraseAsk::default(),
            need_passphrase: false,
            status_job: None,
            status_answer: None,
            trouble: None,
            posted: None,
            posts_sent: 0,
            post_unsettled: false,
            history_job: None,
            history: None,
            unposted_only: false,
            reversing: None,
            reverse_key: None,
            reverse_job: None,
            reverse_unsettled: false,
            reversed: None,
            discard_job: None,
        };
        s.need_passphrase = crate::widgets::passphrase::needed(&s.link);
        if !s.allowed() {
            s.trouble = Some(Trouble::Restricted);
        } else if !s.supported() {
            s.trouble = Some(Trouble::Unsupported);
        } else {
            s.load_lists();
        }
        s
    }

    /// U-247: payroll is refused while any account is hidden from the user.
    pub(crate) fn allowed(&self) -> bool {
        self.link.rights().door_open(Door::PayrollImport)
    }

    /// The engine has every payroll capability (the demo plays one).
    pub(crate) fn supported(&self) -> bool {
        self.link.is_demo() || cl_api::payroll::payroll_supported(&self.link.capabilities())
    }

    // ---- Running requests ------------------------------------------------

    fn read<T, F, Fut>(
        &self,
        demo: impl FnOnce(&mut crate::company::demo::DemoState) -> Result<T, ApiError>,
        real: F,
    ) -> Job<T>
    where
        T: Send + 'static,
        F: FnOnce(Session) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T, ApiError>> + Send + 'static,
    {
        if let Some(mut d) = self.link.demo_state() {
            return Job::Done(Some(demo(&mut d)));
        }
        match (self.link.session(), self.group.as_ref()) {
            (Some(session), Some(group)) => group.read(&session, real).map_or(
                Job::Done(Some(Err(ApiError::EngineUnavailable { permanent: false }))),
                |p| Job::Running(Task::Pending(p)),
            ),
            _ => Job::Done(Some(Err(ApiError::EngineUnavailable { permanent: false }))),
        }
    }

    fn write<T, F, Fut>(
        &self,
        what: &'static str,
        key: Option<IdempotencyKey>,
        demo: impl FnOnce(&mut crate::company::demo::DemoState) -> Result<T, ApiError>,
        real: F,
    ) -> Job<T>
    where
        T: Send + 'static,
        F: FnOnce(Session) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T, ApiError>> + Send + 'static,
    {
        if let Some(mut d) = self.link.demo_state() {
            return Job::Done(Some(demo(&mut d)));
        }
        match (self.link.session(), self.group.as_ref()) {
            (Some(session), Some(group)) => group
                .write(&session, OrphanLabel { what, key }, real)
                .map_or(
                    Job::Done(Some(Err(ApiError::EngineUnavailable { permanent: false }))),
                    |p| Job::Running(Task::Pending(p)),
                ),
            _ => Job::Done(Some(Err(ApiError::EngineUnavailable { permanent: false }))),
        }
    }

    fn load_lists(&mut self) {
        self.chart_job = Some(self.read(
            |d| Ok(crate::company::demo::home::accounts(d)),
            |s| async move { s.accounts().list_all(&AccountFilter::default()).await },
        ));
        self.tags_job = Some(self.read(
            |d| Ok(d.tags.clone().unwrap_or_default()),
            |s| async move { s.tags().list_all(&cl_api::TagFilter::default()).await },
        ));
        self.templates_job = Some(self.read(
            |d| Ok(d.payroll.templates()),
            |s| async move { s.payroll().templates().await },
        ));
        // The engine's suggestions and the company's saved accounts; the
        // starting choices are made once the chart is here (QA-1). The
        // demo has no suggestions of its own: the rules pick the same.
        self.accounts_job = Some(self.read(
            |d| {
                let remembered = d
                    .payroll
                    .account_mappings()
                    .into_iter()
                    .filter_map(|a| Some((a.line_type, a.account?)))
                    .collect();
                Ok((BTreeMap::new(), remembered))
            },
            |s| async move {
                let suggested = s.payroll().suggest_accounts().await?;
                let remembered = s
                    .payroll()
                    .account_mappings()
                    .await?
                    .into_iter()
                    .filter_map(|a| Some((a.line_type, a.account?)))
                    .collect();
                Ok((suggested, remembered))
            },
        ));
    }

    /// Takes every answer that arrived.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn poll(&mut self) {
        self.engine_back();
        if let Some(paths) = crate::widgets::file_picker::poll(&mut self.pick_rx)
            && let Some(p) = paths.into_iter().next()
        {
            self.take_path(&p);
        }
        if let Some(r) = crate::screens::write_check::attach::poll_file_read(&mut self.reading) {
            match r {
                Ok((name, bytes)) => self.take_file(name, bytes),
                Err(k) => {
                    self.file = None;
                    self.file_error = Some(k);
                }
            }
        }
        if let Some(Ok(v)) = poll(&mut self.chart_job) {
            self.chart = v;
        }
        if let Some(Ok(v)) = poll(&mut self.tags_job) {
            self.tags = v;
        }
        // QA-14: Preferences › Tags or the Tags list changed while this
        // window is open: the tag list and the rule are read again, and an
        // open Review applies a new default tag at once.
        let changes = self.link.bus().take(&mut self.bus);
        if changes.tags {
            self.tags_job = Some(self.read(
                |d| Ok(d.tags.clone().unwrap_or_default()),
                |s| async move { s.tags().list_all(&cl_api::TagFilter::default()).await },
            ));
            self.refresh_tag_rules();
        }
        // Q-P30h: a backup passphrase was set (Back Up): "no passphrase"
        // no longer applies. Nothing is posted until Post is pressed.
        if changes.backup_passphrase {
            self.need_passphrase = false;
        }
        // P32 QA-14: Send to My Accountant discarded every unposted
        // import, this one included: start over and say why.
        if changes.payroll_discarded && self.upload.is_some() && self.may_discard() {
            // Already gone: nothing to discard again when the old state drops.
            self.upload = None;
            self.start_over();
            self.file_error = Some("payroll.discarded_by_transfer");
        }
        if changes.backup_passphrase
            && matches!(
                self.backup_failed,
                Some(cl_company::CompanyError::NoPassphrase)
            )
        {
            self.backup_failed = None;
        }
        if let Some(Ok(v)) = poll(&mut self.templates_job) {
            self.templates = v;
        }
        // A layout that couldn't be saved only means mapping again next time.
        if let Some(Ok(t)) = poll(&mut self.template_job) {
            self.templates.retain(|x| x.id != t.id);
            self.templates.push(t);
        }
        if let Some(Ok(v)) = poll(&mut self.accounts_job) {
            self.account_hints = Some(v);
        }
        if !self.chart.is_empty()
            && let Some((suggested, remembered)) = self.account_hints.take()
        {
            let lines: Vec<&str> = cl_model::payroll_import::LINE_TYPES
                .iter()
                .map(|(l, _)| *l)
                .collect();
            let rows = chart_rows(&self.chart);
            for (k, a) in
                cl_model::payroll_import::default_accounts(&lines, &rows, &suggested, &remembered)
            {
                self.line_accounts.entry(k).or_insert(a);
            }
            self.chart_rows = rows;
        }
        if let Some(r) = poll(&mut self.prior_job) {
            self.prior = r.map(|p| p.sessions).unwrap_or_default();
        }
        if let Some(r) = poll(&mut self.upload_job) {
            match r {
                Ok(u) => self.after_upload(u),
                Err(e) => self.trouble = Some(Trouble::of(&e)),
            }
        }
        if let Some(r) = poll(&mut self.map_job) {
            match r {
                Ok(()) => self.go(Step::Accounts),
                Err(e) => self.trouble = Some(Trouble::of(&e)),
            }
        }
        if let Some(r) = poll(&mut self.desc_job) {
            match r {
                Ok(v) => self.take_descriptions(v),
                Err(e) => self.trouble = Some(Trouble::of(&e)),
            }
        }
        if let Some(r) = poll(&mut self.save_accounts_job) {
            match r {
                Ok(()) => self.start_validate(),
                Err(e) => self.trouble = Some(Trouble::of(&e)),
            }
        }
        if let Some(r) = poll(&mut self.validate_job) {
            match r {
                Ok(v) => {
                    self.take_file_departments(&v);
                    self.validation = Some(v);
                    self.go(Step::Check);
                }
                Err(e) => self.trouble = Some(Trouble::of(&e)),
            }
        }
        if let Some(r) = poll(&mut self.preview_job) {
            match r {
                Ok(p) => self.take_preview(p),
                Err(e) => self.trouble = Some(Trouble::of(&e)),
            }
        }
        // Before the post's own answer: the demo's backup and post both
        // answer within one poll.
        if let Some(r) = poll(&mut self.backup_job) {
            self.after_backup(r.map_err(cl_company::CompanyError::from).and_then(|x| x));
        }
        if let Some(r) = poll(&mut self.post_job) {
            self.after_post(r);
        }
        if let Some(r) = poll(&mut self.status_job) {
            match r {
                Ok(s) if s.status == SessionStatus::Posted => {
                    self.status_answer = Some(true);
                    self.finish(s.entries);
                }
                Ok(_) => {
                    // Not posted yet, but the lost post may still land:
                    // Try again (same key) stays offered (M2), and nothing
                    // discards the draft (B1).
                    self.status_answer = Some(false);
                    self.clear_trouble();
                }
                // Discarded meanwhile: the lost post can't land either.
                Err(ApiError::NotFound { .. }) => self.import_gone(),
                Err(e) => self.trouble = Some(Trouble::of(&e)),
            }
        }
        if let Some(r) = poll(&mut self.history_job) {
            match r {
                Ok(p) => {
                    // Check status after a lost reverse: reversed now is
                    // settled (nothing to send again).
                    if self.reverse_unsettled
                        && let Some((id, _)) = &self.reversing
                        && p.sessions
                            .iter()
                            .any(|s| s.id == *id && s.status == SessionStatus::Cancelled)
                    {
                        self.reverse_unsettled = false;
                        self.reversing = None;
                        self.reverse_key = None;
                        self.password = None;
                        self.trouble = None;
                    }
                    // QA-15: a draft whose post's answer was lost is
                    // settled once History shows it posted, reversed or
                    // gone (a list filtered to drafts says nothing of the
                    // others). This window's own draft settles through its
                    // own Check status.
                    if !self.unposted_only {
                        let own = self.upload.as_ref().map(|u| u.session);
                        let me = self.link_key();
                        unsettled().retain(|(link, id)| {
                            *link != me
                                || Some(*id) == own
                                || p.sessions
                                    .iter()
                                    .any(|s| s.id == *id && s.status.is_unposted_draft())
                        });
                    }
                    self.history = Some(p);
                }
                Err(e) => self.trouble = Some(Trouble::of(&e)),
            }
        }
        if let Some(r) = poll(&mut self.reverse_job) {
            self.reverse_unsettled = matches!(r, Err(ApiError::OutcomeUnknown { .. }));
            match r {
                Ok(o) => {
                    self.reversed = Some(o.reversed);
                    self.reversing = None;
                    self.reverse_key = None;
                    self.password = None;
                    self.asking_password = None;
                    self.link.bus().publish(crate::data_bus::Change::Payroll);
                    self.load_history();
                }
                Err(e) => self.refused(&e),
            }
        }
        if let Some(r) = poll(&mut self.discard_job) {
            if let Err(e) = r {
                self.trouble = Some(Trouble::of(&e));
            }
            self.link.bus().publish(crate::data_bus::Change::Payroll);
            if self.view == View::History {
                self.load_history();
            }
        }
    }

    fn busy(&self) -> bool {
        self.pick_rx.is_some()
            || self.reading.is_some()
            || self.upload_job.is_some()
            || self.map_job.is_some()
            || self.desc_job.is_some()
            || self.save_accounts_job.is_some()
            || self.validate_job.is_some()
            || self.preview_job.is_some()
            || self.post_job.is_some()
            || self.backup_job.is_some()
            || self.status_job.is_some()
            || self.history_job.is_some()
            || self.reverse_job.is_some()
            || self.discard_job.is_some()
            || self.chart_job.is_some()
            || self.accounts_job.is_some()
    }

    pub(crate) fn go(&mut self, step: Step) {
        self.step = step;
        self.visited.insert(step);
        self.clear_trouble();
        if matches!(step, Step::Check | Step::Preview) {
            self.refresh_tag_rules();
        }
    }

    /// Reads the company's tag rule and default tag again (QA-14). Lines
    /// the owner tagged by hand keep their tag; the others take the new
    /// default. A shown "add a tag" refusal is worked out again.
    pub(crate) fn refresh_tag_rules(&mut self) {
        self.rules = self.link.tag_rules();
        self.plan.default_tag = self.rules.default_tag;
        if self.untagged.is_some() {
            self.untagged = self.plan.gate(self.rules.every_line).err();
        }
    }

    /// The tag rule is on and no default tag fills the lines: the Check
    /// step says tags are still needed (QA-14).
    pub(crate) fn tags_still_needed(&self) -> bool {
        self.rules.every_line && self.rules.default_tag.is_none()
    }

    /// Clears the shown problem, except a post whose answer was lost: its
    /// "Check status" and "Try again" stay until it is settled.
    fn clear_trouble(&mut self) {
        self.trouble = self.post_unsettled.then_some(Trouble::Unknown);
    }

    /// A post or reverse is out and not answered, or a post's answer was
    /// lost: nothing may discard this import and Cancel/Back wait (B1).
    pub(crate) fn in_flight(&self) -> bool {
        self.post_job.is_some() || self.reverse_job.is_some()
    }

    /// The draft may be discarded: not posted, no post out, and no post
    /// whose answer was lost (it may still commit, B1).
    fn may_discard(&self) -> bool {
        self.posted.is_none() && self.post_job.is_none() && !self.post_unsettled
    }

    /// Lets go of the uploaded draft: discarded when that is safe, else
    /// kept for History's "Check status" (the lost post may have posted).
    fn release_upload(&mut self) {
        let safe = self.may_discard();
        if let Some(u) = self.upload.take()
            && safe
        {
            // This window's own flags decide for its own draft (the
            // shared lost-answer list is for other windows' drafts).
            self.send_discard(u.session);
        }
    }

    // ---- Step 1 -----------------------------------------------------------

    /// Opens the system file window (answered by `dialog_answer` under the
    /// control channel).
    pub(crate) fn browse(&mut self, ctx: &eframe::egui::Context) {
        use crate::widgets::file_picker::{PAYROLL_FILES, PickUse, pick_files};
        self.pick_rx = Some(pick_files(
            ctx,
            PickUse::PayrollFile,
            &[PAYROLL_FILES],
            false,
        ));
    }

    /// Starts reading the chosen file on a background thread (up to 10 MB
    /// from a possibly slow disk or share); `poll` takes the bytes.
    pub(crate) fn take_path(&mut self, p: &std::path::Path) {
        self.file_error = None;
        match crate::screens::write_check::attach::read_in_background(p, read_payroll_file) {
            Ok(rx) => self.reading = Some(rx),
            Err(_) => self.file_error = Some("payroll.file.open"),
        }
    }

    pub(crate) fn take_file(&mut self, name: String, bytes: Vec<u8>) {
        self.file_error = if bytes.len() > cl_api::payroll::MAX_UPLOAD_BYTES {
            Some("payroll.file.too_large")
        } else if cl_api::payroll::file_extension(&name).is_none() {
            Some("payroll.file.type")
        } else {
            None
        };
        self.file = self.file_error.is_none().then_some((name, bytes));
        self.forget_upload();
    }

    fn forget_upload(&mut self) {
        // A draft whose post's answer was lost is kept (History's Check
        // status settles it); this window lets go of it.
        self.release_upload();
        self.post_unsettled = false;
        self.status_answer = None;
        self.prior.clear();
        self.file_departments = None;
        self.departments_truncated = false;
        self.preview = None;
        self.validation = None;
        self.posted = None;
        self.key = None;
        self.force = false;
        self.password = None;
        self.overlaps = None;
        self.backed_up = None;
        // A refusal of the whole screen stays on show.
        if self.allowed() && self.supported() {
            self.trouble = None;
        }
    }

    pub(crate) fn period(&self) -> PeriodInput {
        PeriodInput {
            start: parse_date(&self.period_text[0]),
            end: parse_date(&self.period_text[1]),
            check_date: parse_date(&self.period_text[2]),
        }
    }

    /// The engine said this payroll is already posted (QA-6): the upload's
    /// run check, or a refused post. Post stays off; History reverses it.
    pub(crate) fn posted_duplicate(&self) -> bool {
        self.upload.as_ref().is_some_and(|u| u.is_duplicate_by_key)
            || matches!(
                self.trouble,
                Some(Trouble::Refused(
                    PayrollRefusal::DuplicateFile { .. } | PayrollRefusal::DuplicateRun { .. }
                ))
            )
    }

    /// Reads the posted imports to find the earlier post of this payroll
    /// (QA-12). Not part of `busy`; a failure leaves the text undated.
    fn look_up_prior(&mut self) {
        if self.prior_job.is_some() {
            return;
        }
        self.prior_job = Some(self.read(
            |d| Ok(d.payroll.sessions(Some(SessionStatus::Posted))),
            |s| async move { s.payroll().sessions_all(Some(SessionStatus::Posted)).await },
        ));
    }

    /// When the earlier post of this payroll was made (engine text), if it
    /// is known (QA-12): the refusal's own date; else its earlier import by
    /// id; else the one posted import with this upload's run (provider and
    /// run key, the engine's rule) or, for the same file, its file hash.
    /// Several or none: `None` (the undated text; never a guess).
    pub(crate) fn prior_posted_at(&self) -> Option<String> {
        if let Some(Trouble::Refused(
            PayrollRefusal::DuplicateFile { prior, posted_at }
            | PayrollRefusal::DuplicateRun { prior, posted_at },
        )) = &self.trouble
        {
            if posted_at.is_some() {
                return posted_at.clone();
            }
            if let Some(id) = prior {
                return self
                    .prior
                    .iter()
                    .find(|s| s.id == *id)
                    .and_then(|s| s.posted_at.clone());
            }
        }
        let u = self.upload.as_ref()?;
        let posted = |s: &&cl_api::payroll::SessionSummary| {
            s.status == SessionStatus::Posted && s.id != u.session
        };
        let one = |v: Vec<&cl_api::payroll::SessionSummary>| match v.as_slice() {
            [s] => s.posted_at.clone(),
            _ => None,
        };
        if u.is_duplicate_by_key && u.run_key.is_some() && u.detected_provider.is_some() {
            return one(self
                .prior
                .iter()
                .filter(posted)
                .filter(|s| s.run_key == u.run_key && s.provider == u.detected_provider)
                .collect());
        }
        if u.is_duplicate && u.file_hash.is_some() {
            return one(self
                .prior
                .iter()
                .filter(posted)
                .filter(|s| s.file_hash == u.file_hash)
                .collect());
        }
        None
    }

    /// The chosen file doesn't look like the chosen service's (QA-11): the
    /// engine read it as another service's, or the built-in layout's
    /// columns aren't in it. Generic files never warn.
    pub(crate) fn service_mismatch(&self) -> bool {
        let Some(u) = &self.upload else {
            return false;
        };
        if self.provider.is_generic() {
            return false;
        }
        let other = u
            .detected_provider
            .as_deref()
            .and_then(Provider::from_engine_key)
            .is_some_and(|p| p != self.provider);
        let misfit = matches!(self.layout_choice, Some(LayoutChoice::Builtin))
            && builtin_mapping(self.provider).is_some_and(|l| !l.fits(&u.headers));
        other || misfit
    }
    pub(crate) fn can_next(&self) -> bool {
        if self.busy()
            || !self.allowed()
            || !self.supported()
            || self.asking_password.is_some()
            || self.overlaps.is_some()
        {
            return false;
        }
        match self.step {
            Step::Choose => self.file.is_some() && self.period().checked(self.today).is_ok(),
            Step::Map => self.missing_fields().is_empty(),
            Step::Accounts => self.accounts_complete(),
            Step::Check => self
                .validation
                .as_ref()
                .is_some_and(ValidationSummary::can_continue),
            Step::Preview => {
                self.preview.as_ref().is_some_and(Preview::is_balanced) && !self.posted_duplicate()
            }
            Step::Done => false,
        }
    }

    fn start_upload(&mut self) {
        let checked = self.period().checked(self.today);
        self.period_problem = checked.err();
        let (Ok((start, end, check)), Some((name, bytes))) = (checked, self.file.clone()) else {
            return;
        };
        let mode = match self.provider.mode() {
            FileMode::EmployeeLevel => ImportMode::EmployeeLevel,
            FileMode::Prebuilt => ImportMode::PrebuiltJe,
        };
        let req = UploadRequest {
            mode,
            bytes,
            file_name: name,
            period_start: start,
            period_end: end,
            check_date: check,
        };
        let req2 = req.clone();
        self.upload_job = Some(self.write(
            "orphan.payroll_upload",
            None,
            move |d| d.payroll.upload(&req2),
            move |s| async move { s.payroll().upload(&req).await },
        ));
    }

    fn after_upload(&mut self, u: UploadOutcome) {
        tracing::info!(target: "payroll", provider = self.provider.key(), rows = u.row_count, "payroll file read");
        let headers = u.headers.clone();
        let duplicate = u.is_duplicate || u.is_duplicate_by_key;
        self.upload = Some(u);
        if duplicate {
            self.look_up_prior();
        }
        if self.provider.mode() == FileMode::Prebuilt {
            self.layout_choice = Some(LayoutChoice::NotNeeded);
            self.load_descriptions();
            return;
        }
        let saved: Vec<SavedLayout<uuid::Uuid>> =
            self.templates.iter().filter_map(saved_layout).collect();
        let choice = choose_layout(self.provider, &headers, &saved);
        self.layout = match &choice {
            LayoutChoice::Saved(id) => {
                let s = saved.iter().find(|s| s.id == *id);
                if let Some(s) = s {
                    self.department_tags.clone_from(&s.department_tags);
                }
                s.map(|s| s.layout.clone()).unwrap_or_default()
            }
            LayoutChoice::Builtin => builtin_mapping(self.provider).unwrap_or_default(),
            LayoutChoice::Manual | LayoutChoice::NotNeeded => {
                cl_model::payroll_import::manual_layout(&[], Some(self.guess_format()))
            }
        };
        self.refresh_departments();
        // S4: a saved layout applies silently only when the file has no
        // amount columns it neither matches nor left out.
        self.new_columns = match &choice {
            LayoutChoice::Saved(id) => saved
                .iter()
                .find(|s| s.id == *id)
                .map(|s| {
                    let rows = self
                        .upload
                        .as_ref()
                        .map(|u| u.samples.0.clone())
                        .unwrap_or_default();
                    cl_model::payroll_import::new_amount_columns(s, &headers, &rows)
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let skip = self.provider.is_generic()
            && matches!(choice, LayoutChoice::Saved(_))
            && self.new_columns.is_empty();
        self.layout_choice = Some(choice);
        if !self.new_columns.is_empty() {
            tracing::info!(target: "payroll", new_columns = self.new_columns.len(), "saved layout stops on new columns");
            // The owner's answer updates the saved layout.
            self.save_layout = true;
            self.go(Step::Map);
        } else if skip {
            // A2: a generic file whose saved layout fits needs no Map step.
            self.save_layout = false;
            self.apply_mapping();
        } else {
            self.go(Step::Map);
        }
    }

    fn guess_format(&self) -> &'static str {
        let cells: Vec<&str> = self
            .upload
            .iter()
            .flat_map(|u| u.samples.0.iter().flatten())
            .map(String::as_str)
            .filter(|c| cl_model::payroll_import::SampleGuard::displayable(c).is_some())
            .collect();
        cl_model::payroll_import::guess_date_format(
            cells
                .into_iter()
                .filter(|c| c.contains('/') || c.matches('-').count() == 2),
        )
    }

    // ---- Step 2 -----------------------------------------------------------

    /// The column index of `header` in the upload.
    fn column_index(&self, header: &str) -> Option<usize> {
        let n = cl_model::payroll_import::normalize_header(header);
        self.upload
            .as_ref()?
            .headers
            .iter()
            .position(|h| cl_model::payroll_import::normalize_header(h) == n)
    }

    /// A displayable sample for `field` (numbers and dates only).
    pub(crate) fn sample(&self, field: &str) -> Option<String> {
        if field == "employee_name" || field == "department" {
            return None;
        }
        let i = self.column_index(self.layout.column(field)?)?;
        self.upload
            .as_ref()?
            .samples
            .0
            .iter()
            .filter_map(|r| r.get(i))
            .find_map(|c| cl_model::payroll_import::SampleGuard::displayable(c).map(str::to_string))
            // QA-10: an ISO date shows as the screen's MM/DD/YYYY.
            .map(|c| CalendarDate::parse_wire(&c).map_or(c, show_date))
    }

    /// The departments in the sample rows' Department column (names of
    /// departments, never of people).
    pub(crate) fn refresh_departments(&mut self) {
        let Some(i) = self
            .layout
            .column("department")
            .and_then(|h| self.column_index(h))
        else {
            self.departments.clear();
            return;
        };
        let full = self.file_departments.clone().unwrap_or_default();
        let mut v: Vec<String> = self
            .upload
            .iter()
            .flat_map(|u| u.samples.0.iter())
            .filter_map(|r| r.get(i))
            .map(|c| c.trim().to_string())
            .chain(full)
            .filter(|c| !c.is_empty() && c.chars().count() <= cl_api::payroll::MAX_DEPARTMENT_CHARS)
            .collect();
        v.sort();
        v.dedup();
        for d in &v {
            self.department_tags.entry(d.clone()).or_insert(None);
        }
        // S3: a saved layout's mapping for a department the sample rows
        // don't show is kept (it may be further down the file).
        self.departments = v;
    }

    /// The check listed every department in the file (S3): the engine has
    /// `payroll-import-review-v1` (the demo plays it).
    fn take_file_departments(&mut self, v: &ValidationSummary) {
        let listed = self.link.is_demo()
            || self
                .link
                .capabilities()
                .has(cl_api::Capabilities::PAYROLL_IMPORT_REVIEW);
        if !listed || self.layout.column("department").is_none() {
            return;
        }
        let Some(all) = v.departments.clone() else {
            return;
        };
        let before = self.departments.len();
        self.file_departments = Some(all);
        self.departments_truncated = v.departments_truncated;
        self.refresh_departments();
        tracing::info!(target: "payroll", departments = self.departments.len(), added = self.departments.len().saturating_sub(before), truncated = v.departments_truncated, "departments listed");
    }

    /// Sets the column for `field` (`None` clears it).
    pub(crate) fn set_column(&mut self, field: &'static str, header: Option<String>) {
        self.layout.columns.retain(|c| c.field != field);
        if let Some(h) = header {
            self.layout.columns.push(FieldColumn { field, header: h });
        }
        if field == "department" {
            self.refresh_departments();
        }
    }

    pub(crate) fn missing_fields(&self) -> Vec<&'static str> {
        let headers = self
            .upload
            .as_ref()
            .map(|u| u.headers.clone())
            .unwrap_or_default();
        REQUIRED_FIELDS
            .iter()
            .copied()
            .filter(|f| {
                self.layout
                    .column(f)
                    .is_none_or(|h| !headers.iter().any(|x| x.trim() == h.trim()))
            })
            .collect()
    }

    fn apply_mapping(&mut self) {
        let Some(id) = self.upload.as_ref().map(|u| u.session) else {
            return;
        };
        let headers = self
            .upload
            .as_ref()
            .map(|u| u.headers.clone())
            .unwrap_or_default();
        let map = column_map(&self.layout, &self.department_tags, &headers);
        if self.save_layout {
            let existing = match &self.layout_choice {
                Some(LayoutChoice::Saved(t)) => Some(*t),
                _ => None,
            };
            let name = crate::i18n::t_with(
                cl_model::payroll_import::layout_name_key(self.provider),
                &[(
                    "provider",
                    &t(&format!("payroll.provider.{}", self.provider.key())),
                )],
            );
            let key = self.provider.engine_key();
            let (m1, m2, n2) = (map.clone(), map.clone(), name.clone());
            let now = crate::company::demo::DemoState::now().to_wire();
            // The layout is saved alongside; a failure here doesn't stop
            // the import (the owner maps again next time).
            let job = self.write(
                "orphan.payroll_upload",
                None,
                move |d| Ok(d.payroll.save_template(existing, &n2, key, &m2, now)),
                move |s| async move { s.payroll().save_template(existing, &name, key, &m1).await },
            );
            self.template_job = Some(job);
        }
        self.new_columns.clear();
        let m2 = map.clone();
        self.map_job = Some(self.write(
            "orphan.payroll_upload",
            None,
            move |d| d.payroll.apply_mapping(id, &m2),
            move |s| async move { s.payroll().apply_mapping(id, &map).await },
        ));
    }

    // ---- Step 3 -----------------------------------------------------------

    fn load_descriptions(&mut self) {
        let Some(id) = self.upload.as_ref().map(|u| u.session) else {
            return;
        };
        self.desc_job = Some(self.read(
            move |d| d.payroll.description_map(id),
            move |s| async move { s.payroll().description_map(id).await },
        ));
    }

    fn take_descriptions(&mut self, mut v: Vec<DescriptionMapping>) {
        // A saved account stays; the engine's own guess stays only when it
        // fits the line the description reads as (QA-1); else the line's
        // starting account, if one was confident, else none.
        let rows = chart_rows(&self.chart);
        for m in &mut v {
            let debit = m.side.as_deref() == Some("debit");
            let lt = cl_model::payroll_import::line_for_description(&m.description, debit);
            let fits = |id: AccountId| {
                rows.iter()
                    .find(|r| r.id == id)
                    .is_some_and(|r| cl_model::payroll_import::account_fits(lt, r))
            };
            if m.suggested && !m.account.is_some_and(fits) {
                m.account = None;
            }
            if m.account.is_none() {
                m.account = self.line_accounts.get(lt).copied();
            }
        }
        self.descriptions = v;
        self.go(Step::Accounts);
    }

    /// The line types this import needs accounts for.
    pub(crate) fn needed_lines(&self) -> Vec<&'static str> {
        let fields: BTreeSet<&str> = self.layout.columns.iter().map(|c| c.field).collect();
        let has = |f: &[&str]| f.iter().any(|x| fields.contains(x));
        cl_model::payroll_import::LINE_TYPES
            .iter()
            .map(|(lt, _)| *lt)
            .filter(|lt| match *lt {
                "gross_wages_expense" | "payroll_clearing" => true,
                "employer_tax_expense" => has(&[
                    "social_security_er",
                    "medicare_er",
                    "futa_er",
                    "suta_er",
                    "other_er_tax",
                ]),
                "employer_benefits_expense" => has(&["health_insurance_er"]),
                "reimbursement_expense" => has(&["reimbursement_ee"]),
                "fit_payable" => has(&["federal_income_tax"]),
                "sit_payable" => has(&["state_income_tax", "local_income_tax"]),
                "ss_payable" => has(&["social_security_ee", "social_security_er"]),
                "medicare_payable" => has(&["medicare_ee", "medicare_er"]),
                "futa_payable" => has(&["futa_er"]),
                "suta_payable" => has(&["suta_er"]),
                "health_ins_payable" => has(&["health_insurance_ee", "health_insurance_er"]),
                "retirement_payable" => has(&["retirement_401k_ee"]),
                "other_deduction_payable" => has(&["hsa_ee", "other_deduction_ee"]),
                "other_tax_payable" => has(&["other_ee_tax", "other_er_tax"]),
                _ => false,
            })
            .collect()
    }

    pub(crate) fn accounts_complete(&self) -> bool {
        if self.provider.mode() == FileMode::Prebuilt {
            !self.descriptions.is_empty() && self.descriptions.iter().all(|d| d.account.is_some())
        } else {
            self.needed_lines()
                .iter()
                .all(|l| self.line_accounts.contains_key(*l))
        }
    }

    /// Net pay is paid from a bank account (its withdrawals match, Q-P30f).
    pub(crate) fn net_pay_from_bank(&self) -> bool {
        self.line_accounts
            .get(cl_model::payroll_import::NET_PAY_LINE)
            .is_some_and(|a| self.is_bank(*a))
    }

    pub(crate) fn is_bank(&self, id: AccountId) -> bool {
        self.chart.iter().any(|a| a.id == id && is_bank_account(a))
    }

    fn save_accounts(&mut self) {
        let Some(id) = self.upload.as_ref().map(|u| u.session) else {
            return;
        };
        if self.provider.mode() == FileMode::Prebuilt {
            let list = self.descriptions.clone();
            let l2 = list.clone();
            let key = self.provider.engine_key();
            self.save_accounts_job = Some(self.write(
                "orphan.payroll_upload",
                None,
                move |d| {
                    d.payroll.save_description_map(key, &l2);
                    Ok(())
                },
                move |s| async move { s.payroll().save_description_map(id, key, &list).await },
            ));
        } else {
            // Every line with an account: the owner's choices and the
            // confident starting ones only (QA-1), never a first-of-type.
            let map: BTreeMap<String, AccountId> = self.line_accounts.clone();

            let m2 = map.clone();
            self.save_accounts_job = Some(self.write(
                "orphan.payroll_upload",
                None,
                move |d| {
                    d.payroll.save_account_mappings(&m2);
                    Ok(())
                },
                move |s| async move { s.payroll().save_account_mappings(&map).await },
            ));
        }
    }

    // ---- Step 4 -----------------------------------------------------------

    fn start_validate(&mut self) {
        let Some(id) = self.upload.as_ref().map(|u| u.session) else {
            return;
        };
        self.validate_job = Some(self.write(
            "orphan.payroll_upload",
            None,
            move |d| d.payroll.validate(id),
            move |s| async move { s.payroll().validate(id).await },
        ));
    }

    // ---- Step 5 -----------------------------------------------------------

    /// The department → tag map sent with the entries and the post: the
    /// departments in the file when the check listed them all (a saved
    /// mapping for one not in this file stays saved, unsent), else every
    /// mapping known (S3).
    fn departments_sent(&self) -> BTreeMap<String, Option<TagId>> {
        if self.provider.mode() == FileMode::Prebuilt || self.layout.column("department").is_none()
        {
            return BTreeMap::new();
        }
        match &self.file_departments {
            Some(all) if !self.departments_truncated => self
                .department_tags
                .iter()
                .filter(|(d, _)| all.contains(d))
                .map(|(d, t)| (d.clone(), *t))
                .collect(),
            _ => self.department_tags.clone(),
        }
    }

    /// Department tags chosen after the layout was saved (the Check step
    /// lists departments past the sample rows): saved with it, so the next
    /// file maps them too (S3).
    fn save_department_tags(&mut self) {
        let Some(tpl) = self.current_template() else {
            return;
        };
        let Some(mut map) = tpl.column_map.clone() else {
            return;
        };
        if map.department_tags == self.department_tags {
            return;
        }
        map.department_tags.clone_from(&self.department_tags);
        let (id, name, key) = (tpl.id, tpl.name.clone(), tpl.provider_key.clone());
        let (m2, n2, k2) = (map.clone(), name.clone(), key.clone());
        let now = crate::company::demo::DemoState::now().to_wire();
        self.template_job = Some(self.write(
            "orphan.payroll_upload",
            None,
            move |d| Ok(d.payroll.save_template(Some(id), &n2, &k2, &m2, now)),
            move |s| async move { s.payroll().save_template(Some(id), &name, &key, &map).await },
        ));
    }

    /// The saved layout this import uses or just saved.
    fn current_template(&self) -> Option<&PayrollTemplate> {
        let key = self.provider.engine_key();
        match &self.layout_choice {
            Some(LayoutChoice::Saved(id)) => self.templates.iter().find(|t| t.id == *id),
            _ if self.save_layout => self
                .templates
                .iter()
                .filter(|t| !t.is_system && t.provider_key == key)
                .filter(|t| {
                    t.column_map.as_ref().is_some_and(|m| {
                        m.mappings
                            .iter()
                            .all(|(f, s)| self.layout.column(f) == Some(s.source.as_str()))
                    })
                })
                .max_by(|a, b| a.updated_at.cmp(&b.updated_at)),
            _ => None,
        }
    }

    pub(crate) fn start_preview(&mut self) {
        let Some(id) = self.upload.as_ref().map(|u| u.session) else {
            return;
        };
        self.save_department_tags();
        let depts = self.departments_sent();
        let d2 = depts.clone();
        let chart = self.chart.clone();
        self.preview_job = Some(self.write(
            "orphan.payroll_upload",
            None,
            move |d| {
                d.payroll.preview(id, &d2, &|a| {
                    chart
                        .iter()
                        .find(|x| x.id == a)
                        .map(|x| x.fields.name.clone())
                })
            },
            move |s| async move { s.payroll().preview(id, &depts).await },
        ));
    }

    fn take_preview(&mut self, p: Preview) {
        let lines = p
            .entries
            .iter()
            .enumerate()
            .flat_map(|(ei, e)| {
                e.lines.iter().enumerate().map(move |(li, l)| PlanLine {
                    entry: ei,
                    line: li,
                    date: e.date,
                    description: l.description.clone(),
                    preview_tag: l.tag,
                })
            })
            .collect();
        self.rules = self.link.tag_rules();
        self.plan = TagPlan::new(lines, self.rules.default_tag);
        self.untagged = None;
        self.preview = Some(p);
        // A refusal that made us read the entries again stays on show.
        let kept = self
            .trouble
            .take()
            .filter(|t| matches!(t, Trouble::Refused(_)));
        self.go(Step::Preview);
        self.trouble = kept;
    }

    // ---- Post (WP5) -------------------------------------------------------

    /// Sends `/post` once: the gate first, then a key for this exact body,
    /// then a verified backup (Q-P30h), then the post. Every way in (Post,
    /// Post anyway, the closing-date password, Try again) comes here, so
    /// every attempt has a verified backup: one per attempt. Continuing
    /// the same draft after the engine's question (`continuing`) reuses
    /// the backup made for it; anything else makes a new one.
    pub(crate) fn post(&mut self) {
        self.post_with(false);
    }

    fn post_with(&mut self, continuing: bool) {
        let Some(id) = self.upload.as_ref().map(|u| u.session) else {
            return;
        };
        if self.post_job.is_some() || self.backup_job.is_some() || self.parked_post.is_some() {
            return;
        }
        // P31 QA-2 (Q-P31ad): no passphrase yet: the one typed in the boxes
        // is saved first (as Back Up saves it); nothing is sent without it.
        if self.need_passphrase {
            if !self.passphrase.save(&self.link) {
                return;
            }
            self.need_passphrase = false;
        }
        self.backup_failed = None;
        self.backup_stopped = false;
        // QA-14: the rule and default as they are now (Preferences may
        // have changed since Review opened).
        self.refresh_tag_rules();
        let tags = match self.plan.gate(self.rules.every_line) {
            Ok(t) => t,
            Err(u) => {
                self.untagged = Some(u);
                return;
            }
        };
        self.untagged = None;
        let line_tags: Vec<LineTag> = tags
            .iter()
            .map(|t| LineTag {
                entry: t.entry,
                line: t.line,
                date: t.date,
                tag: t.tag,
            })
            .collect();
        let departments = self.departments_sent();
        let token = self.preview.as_ref().and_then(|p| p.token.clone());
        let print = BodyPrint {
            force: self.force,
            password: self.password.is_some(),
            tags: line_tags.iter().map(|t| (t.entry, t.line, t.tag)).collect(),
            departments: departments.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            token: token.clone(),
        };
        let draft = DraftPrint {
            id,
            tags: print.tags.clone(),
            departments: print.departments.clone(),
            token: token.clone(),
        };
        let key = match &self.key {
            Some((p, k)) if *p == print => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.key = Some((print, k.clone()));
                k
            }
        };
        let reuse = continuing && self.backed_up.as_ref() == Some(&draft);
        let held = HeldPost {
            id,
            line_tags,
            departments,
            token,
            force: self.force,
            password: self.password.clone(),
            key,
            draft,
        };
        if reuse {
            // The same attempt, continued: its verified backup stands.
            self.status_answer = None;
            self.clear_trouble();
            self.send_post(held);
            return;
        }
        self.backed_up = None;
        self.held_post = Some(held);
        self.start_backup();
    }

    /// The verified backup before a post (Q-P30h), off the egui thread.
    /// Anything that stops it from starting is a failed backup (with Try
    /// again), never a post without one.
    fn start_backup(&mut self) {
        self.backup_failed = None;
        self.backup_stopped = false;
        self.status_answer = None;
        // The lost-answer note stays (Check status still applies).
        self.clear_trouble();
        if let Some(mut d) = self.link.demo_state() {
            let r = d.payroll.backup().map(|r| {
                Ok(r.map_err(|()| cl_company::CompanyError::JobFailed {
                    code: "DEMO".into(),
                }))
            });
            self.backup_job = Some(Job::Done(r));
            return;
        }
        let unavailable =
            || cl_company::CompanyError::Api(ApiError::EngineUnavailable { permanent: false });
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.backup_failed_now(unavailable());
            return;
        };
        let store = self.link.store().clone();
        let secrets = self.link.secrets();
        let default_dir = self.link.backup_dir.clone();
        let task = crate::company::spawn_lifecycle(&self.link, group, move || async move {
            let cl_api::Scope::Company { tenant_id, .. } = session.scope() else {
                return Err(cl_company::CompanyError::UnknownCompany);
            };
            // The folder chosen in Preferences, else the default (read
            // here, off the egui thread).
            let dest_dir = store
                .load()
                .ok()
                .and_then(|l| l.backup_folder)
                .unwrap_or(default_dir);
            // P31: the before-import backups share one retention, so a
            // payroll post never prunes the one an import still needs.
            let protected = crate::screens::qbd_import::protected_backups(&session).await;
            cl_company::backup(
                &session,
                &store,
                secrets.as_ref(),
                &cl_company::BackupRequest {
                    tenant_id,
                    kind: cl_settings::BackupKind::BeforeImport,
                    dest_dir,
                    apply_retention: true,
                    protected,
                },
                &cl_company::backup::silent(),
            )
            .await
            .map(|_| ())
        });
        match task {
            Ok(t) => self.backup_job = Some(Job::Running(t)),
            Err(_) => self.backup_failed_now(unavailable()),
        }
    }

    fn backup_failed_now(&mut self, e: cl_company::CompanyError) {
        self.held_post = None;
        self.backed_up = None;
        self.backup_failed = Some(e);
    }

    fn after_backup(&mut self, r: Result<(), cl_company::CompanyError>) {
        match (r, self.held_post.take()) {
            (Ok(()), Some(held)) => {
                tracing::info!(target: "payroll", "backup made before posting");
                self.backed_up = Some(held.draft.clone());
                if self.close_asked {
                    // P31 QA-20: the close prompt is up; Close drops this
                    // unsent, Keep sends it.
                    self.parked_post = Some(held);
                } else {
                    self.send_post(held);
                }
            }
            // Stopped meanwhile: the backup is kept, nothing is sent.
            (Ok(()), None) => {}
            (Err(e), _) => {
                tracing::warn!(target: "payroll", detail = %cl_core::OwnerFacing::scrubbed_detail(&e), "backup before posting failed; nothing posted");
                if matches!(e, cl_company::CompanyError::NoPassphrase) {
                    self.need_passphrase = true;
                }
                self.backup_failed_now(e);
            }
        }
    }

    /// "Stop, don't post" (or Cancel/Esc) while the backup is made: the
    /// post is dropped; the backup itself finishes and is kept.
    pub(crate) fn stop_backup(&mut self) -> bool {
        if self.backup_job.is_none() && self.parked_post.is_none() {
            return false;
        }
        self.backup_job = None;
        self.parked_post = None;
        self.held_post = None;
        self.backed_up = None;
        self.backup_stopped = true;
        true
    }

    /// The backup is being made before the post.
    pub(crate) fn backing_up(&self) -> bool {
        self.backup_job.is_some() || self.parked_post.is_some()
    }

    fn send_post(&mut self, held: HeldPost) {
        let HeldPost {
            id,
            line_tags,
            departments,
            token,
            force,
            password,
            key,
            draft: _,
        } = held;
        let auth = password.clone();
        let req = PostRequest {
            force_post: force,
            line_tags,
            department_tags: departments,
            preview_token: token,
            closed_period: auth.map(cl_api::ClosedPeriodAuthorization::with_password),
        };
        let (k2, k3) = (key.clone(), key.clone());
        self.posts_sent += 1;
        self.trouble = None;
        self.status_answer = None;
        self.post_job = Some(self.write(
            "orphan.payroll_post",
            Some(key),
            move |d| {
                let closing = crate::company::demo::home::closing_date(d);
                d.payroll.post(id, force, &k2, password.as_ref(), closing)
            },
            move |s| async move { s.payroll().post(id, &req, k3).await },
        ));
    }

    /// "Try again" after the backup failed (Q-P30h): a new backup, then
    /// the post (the same body keeps its key).
    pub(crate) fn backup_try_again(&mut self) {
        if self.view == View::Import && self.backup_failed.is_some() {
            self.post();
        }
    }

    /// "Try again" after a lost answer: the same body and key, on the
    /// owner's click only.
    pub(crate) fn try_again(&mut self) {
        match (self.view, self.reversing.clone()) {
            (View::History, Some((id, reason))) if self.reverse_unsettled => {
                self.reverse(id, &reason);
            }
            (View::Import, _) if self.post_unsettled => self.post(),
            _ => {}
        }
    }

    /// Back to the wizard: a reverse in progress in History is dropped —
    /// unless its answer is outstanding (S2: it stays, reason locked, for
    /// Check status and Try again).
    pub(crate) fn show_import(&mut self) {
        self.view = View::Import;
        if self.reverse_outstanding() {
            return;
        }
        self.reversing = None;
        self.reverse_key = None;
        self.password = None;
        self.asking_password = None;
    }

    /// A reverse is out, or its answer was lost (S2): its reason and the
    /// row buttons are locked until it is settled.
    pub(crate) fn reverse_outstanding(&self) -> bool {
        self.reverse_job.is_some() || self.reverse_unsettled
    }

    /// "Check status" after a lost answer.
    pub(crate) fn check_status(&mut self) {
        if self.view == View::History {
            self.load_history();
            return;
        }
        self.check_post_status();
    }

    /// History's "Check status" on a draft whose post's answer was lost
    /// (QA-15): this window's own draft asks the engine about it (as the
    /// wizard does); another's reads History again, which settles it.
    pub(crate) fn check_draft_status(&mut self, id: PayrollSessionId) {
        if self.upload.as_ref().is_some_and(|u| u.session == id) && self.post_unsettled {
            self.check_post_status();
        }
        self.load_history();
    }

    fn check_post_status(&mut self) {
        let Some(id) = self.upload.as_ref().map(|u| u.session) else {
            return;
        };
        self.status_job = Some(self.read(
            move |d| {
                d.payroll
                    .sessions(None)
                    .sessions
                    .into_iter()
                    .find(|s| s.id == id)
                    .ok_or(ApiError::NotFound { code: None })
            },
            move |s| async move { s.payroll().session(id).await },
        ));
    }

    fn after_post(&mut self, r: Result<PostOutcome, ApiError>) {
        // Only a lost answer leaves the post unsettled; any other answer
        // (posted, a question, a refusal) is the engine's definite word.
        self.post_unsettled = matches!(r, Err(ApiError::OutcomeUnknown { .. }));
        if let Some(id) = self.upload.as_ref().map(|u| u.session) {
            self.set_unsettled(id, self.post_unsettled);
        }
        // Q-P30h: the backup covers this attempt only while the engine's
        // answer is a question the owner may continue (the closing-date
        // password, the overlap check); any other answer ends the attempt.
        let question = matches!(
            r,
            Ok(PostOutcome::NeedsConfirmation { .. })
                | Err(ApiError::ClosedPeriod(_)
                    | ApiError::ClosingPasswordRejected
                    | ApiError::ClosingPasswordRateLimited { .. })
        );
        if !question {
            self.backed_up = None;
        }
        match r {
            Ok(PostOutcome::Posted { entries, .. }) => self.finish(entries),
            Ok(PostOutcome::NeedsConfirmation { overlaps }) => self.overlaps = Some(overlaps),
            // payroll-import-review-v1: a discard that won the race with
            // this post answers the waiting post 404; nothing was posted.
            Err(ApiError::NotFound { .. }) => self.import_gone(),
            // QA-15: the books were down before the post went out (cl-api
            // answers OutcomeUnknown once an attempt may have reached
            // them): nothing was posted, Post may be pressed again.
            Err(ApiError::EngineUnavailable { .. }) => {
                self.trouble = Some(Trouble::NotSent);
            }
            Err(e) => self.refused(&e),
        }
    }

    fn link_key(&self) -> usize {
        Arc::as_ptr(&self.link) as usize
    }

    /// Marks (or clears) draft `id` as having a post whose answer was lost.
    fn set_unsettled(&self, id: PayrollSessionId, on: bool) {
        let k = (self.link_key(), id);
        if on {
            unsettled().insert(k);
        } else {
            unsettled().remove(&k);
        }
    }

    /// A post's answer for draft `id` was lost (this window or another).
    pub(crate) fn is_unsettled(&self, id: PayrollSessionId) -> bool {
        unsettled().contains(&(self.link_key(), id))
    }

    /// The books are running (the status bar's signal; the demo always
    /// is). Without a supervisor to ask, the next successful call clears
    /// the message instead.
    pub(crate) fn engine_running(&self) -> bool {
        if self.link.is_demo() {
            return true;
        }
        self.link
            .supervisor()
            .is_some_and(|s| matches!(s.status().state, cl_host::HostState::Running))
    }

    /// QA-15: "the books aren't running" is replaced once they are, by a
    /// line saying what to do (whatever step it showed on).
    fn engine_back(&mut self) {
        let post = match self.trouble {
            Some(Trouble::NotSent) => true,
            Some(Trouble::EngineDown) => false,
            _ => return,
        };
        if self.engine_running() {
            self.trouble = Some(Trouble::EngineBack { post });
        }
    }

    /// The import no longer exists (discarded elsewhere): nothing was
    /// posted. This window lets go of it without a DELETE and starts over
    /// at the file step.
    fn import_gone(&mut self) {
        if let Some(id) = self.upload.as_ref().map(|u| u.session) {
            self.set_unsettled(id, false);
        }
        self.post_unsettled = false;
        self.clear_backup_notice();
        self.upload = None;
        self.forget_upload();
        self.go(Step::Choose);
        self.trouble = Some(Trouble::Discarded);
    }

    /// The backup's failure or "stopped" line belongs to the Post it was
    /// for: gone once the import is posted, gone, or the owner steps back.
    fn clear_backup_notice(&mut self) {
        self.backup_failed = None;
        self.backup_stopped = false;
        self.backed_up = None;
    }

    fn refused(&mut self, e: &ApiError) {
        match e {
            ApiError::ClosedPeriod(c) => {
                self.lock_date = c.lock_date.or(self.lock_date);
                self.asking_password = Some(self.lock_date);
                self.password_focus = true;
                self.password = None;
                return;
            }
            ApiError::ClosingPasswordRejected | ApiError::ClosingPasswordRateLimited { .. } => {
                self.asking_password = Some(self.lock_date);
                self.password_focus = true;
                self.password = None;
                // The next body differs (another password): new keys.
                self.key = None;
                self.reverse_key = None;
            }
            _ => {}
        }
        let trouble = Trouble::of(e);
        if self.view == View::History
            && !matches!(trouble, Trouble::Unknown | Trouble::PasswordWrong)
        {
            self.reversing = None;
            self.reverse_key = None;
        }
        if matches!(
            trouble,
            Trouble::Refused(
                PayrollRefusal::AlreadyPosted
                    | PayrollRefusal::PreviewChanged
                    | PayrollRefusal::NotReady
            )
        ) && self.view == View::Import
        {
            // Someone else changed it: read the entries again.
            self.key = None;
            self.start_preview();
        }
        // QA-12: a refusal without its date names the earlier import: read
        // the posted imports to find when it was posted.
        if let Trouble::Refused(
            PayrollRefusal::DuplicateFile {
                prior: Some(_),
                posted_at: None,
            }
            | PayrollRefusal::DuplicateRun {
                prior: Some(_),
                posted_at: None,
            },
        ) = &trouble
        {
            self.look_up_prior();
        }
        self.trouble = Some(trouble);
    }

    /// "Post anyway" after the overlap question: a new body, a new key,
    /// the same attempt (its backup stands, Q-P30h).
    pub(crate) fn post_anyway(&mut self) {
        self.overlaps = None;
        self.force = true;
        self.post_with(true);
    }

    /// The closing-date password was entered.
    pub(crate) fn give_password(&mut self) {
        if self.password_text.is_empty() {
            return;
        }
        self.password = Some(Secret::new(std::mem::take(&mut self.password_text)));
        self.asking_password = None;
        match (self.view, self.reversing.clone()) {
            (View::History, Some((id, reason))) => self.reverse(id, &reason),
            // The same attempt, continued: its backup stands (Q-P30h).
            (View::Import, _) => self.post_with(true),
            (View::History, None) => {}
        }
    }

    fn finish(&mut self, entries: Vec<TransactionId>) {
        tracing::info!(target: "payroll", provider = self.provider.key(), entries = entries.len(), "payroll posted");
        self.posted = Some(entries);
        self.post_unsettled = false;
        self.clear_backup_notice();
        if let Some(id) = self.upload.as_ref().map(|u| u.session) {
            self.set_unsettled(id, false);
        }
        self.upload_done();
        self.password = None;
        self.trouble = None;
        self.go(Step::Done);
        self.link.bus().publish(crate::data_bus::Change::Payroll);
        self.link.bus().publish(crate::data_bus::Change::BankFeed);
    }

    fn upload_done(&mut self) {
        // Posted: nothing left to discard on close.
        if let Some(u) = &mut self.upload {
            u.is_duplicate = false;
            u.is_duplicate_by_key = false;
        }
    }

    /// Entries whose net pay is paid from a bank account (each is an
    /// expected Bank Feed withdrawal, Q-151).
    pub(crate) fn expected_withdrawals(&self) -> usize {
        self.preview.as_ref().map_or(0, |p| {
            p.entries
                .iter()
                .filter(|e| {
                    e.lines
                        .iter()
                        .any(|l| !l.credit.is_zero() && l.account.is_some_and(|a| self.is_bank(a)))
                })
                .count()
        })
    }

    // ---- History ------------------------------------------------------------

    /// Home's link asked for History filtered to unposted imports.
    fn take_history_request(&mut self) {
        if take_history_unposted() && self.allowed() && self.supported() {
            self.unposted_only = true;
            self.show_history();
        }
    }

    pub(crate) fn show_history(&mut self) {
        self.view = View::History;
        self.reversed = None;
        self.load_history();
    }

    /// History: every page (M1); unposted only asks the engine for each
    /// draft status (it filters one status at a time).
    pub(crate) fn load_history(&mut self) {
        let statuses: Vec<Option<SessionStatus>> = if self.unposted_only {
            [
                SessionStatus::Uploaded,
                SessionStatus::Mapped,
                SessionStatus::Validated,
            ]
            .into_iter()
            .map(Some)
            .collect()
        } else {
            vec![None]
        };
        let demo_statuses = statuses.clone();
        self.history_job = Some(self.read(
            move |d| {
                let mut p = SessionPage {
                    total: 0,
                    sessions: Vec::new(),
                };
                for st in demo_statuses {
                    p.sessions.extend(d.payroll.sessions(st).sessions);
                }
                p.total = p.sessions.len() as u64;
                Ok(p)
            },
            move |s| async move {
                let several = statuses.len() > 1;
                let mut p = SessionPage {
                    total: 0,
                    sessions: Vec::new(),
                };
                for st in statuses {
                    let page = s.payroll().sessions_all(st).await?;
                    p.total += page.total;
                    p.sessions.extend(page.sessions);
                }
                if several {
                    // Newest pay first across the draft statuses.
                    p.sessions.sort_by_key(|x| std::cmp::Reverse(x.check_date));
                }
                Ok(p)
            },
        ));
    }

    /// Reverses a posted import with `reason` (own key; the same key until
    /// the body changes, e.g. a password was added).
    pub(crate) fn reverse(&mut self, id: PayrollSessionId, reason: &str) {
        if self.reverse_job.is_some() {
            return;
        }
        // S2: the same body keeps its key; another reason or password is
        // another body (the engine would refuse a reused key).
        let print = ReversePrint::of(id, reason, self.password.as_ref());
        let key = match &self.reverse_key {
            Some((p, k)) if *p == print => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.reverse_key = Some((print, k.clone()));
                k
            }
        };
        self.reversing = Some((id, reason.to_string()));
        let password = self.password.clone();
        let reason = reason.to_string();
        let pw2 = password.clone();
        let (k2, k3) = (key.clone(), key.clone());
        self.posts_sent += 1;
        self.trouble = None;
        self.reverse_job = Some(self.write(
            "orphan.payroll_reverse",
            Some(key),
            move |d| {
                let closing = crate::company::demo::home::closing_date(d);
                d.payroll.reverse(id, &k2, password.as_ref(), closing)
            },
            move |s| async move {
                let auth = pw2.map(cl_api::ClosedPeriodAuthorization::with_password);
                // P33: on a LAN host, under maintenance (U-293).
                let lan = crate::lan_link::host_running();
                crate::lan_status::with_maintenance(
                    &s,
                    lan,
                    s.payroll().reverse(id, &reason, auth, k3),
                )
                .await
            },
        ));
    }

    /// History may discard `id`: not this window's own draft while its post
    /// is out or its answer was lost (B1).
    pub(crate) fn may_discard_session(&self, id: PayrollSessionId) -> bool {
        if self.upload.as_ref().is_some_and(|u| u.session == id) {
            return self.may_discard();
        }
        // QA-15: another window's post to it was lost; it may still post.
        !self.is_unsettled(id)
    }

    /// Discards an unposted draft (Cancel, History, Home).
    pub(crate) fn discard(&mut self, id: PayrollSessionId) {
        if !self.may_discard_session(id) {
            return;
        }
        self.send_discard(id);
    }

    /// The DELETE itself; callers have checked it is safe.
    fn send_discard(&mut self, id: PayrollSessionId) {
        self.discard_job = Some(self.write(
            "orphan.payroll_upload",
            None,
            move |d| d.payroll.discard(id),
            move |s| async move { s.payroll().discard(id).await },
        ));
    }

    // ---- Moving -------------------------------------------------------------

    pub(crate) fn next(&mut self) {
        if !self.can_next() {
            if self.step == Step::Choose {
                self.period_problem = self.period().checked(self.today).err();
            }
            return;
        }
        self.trouble = None;
        match self.step {
            Step::Choose => self.start_upload(),
            Step::Map => self.apply_mapping(),
            Step::Accounts => self.save_accounts(),
            Step::Check => self.start_preview(),
            Step::Preview => self.post(),
            Step::Done => {}
        }
    }

    /// "Match columns again" (S4): back to Match columns from a later step,
    /// even when the saved layout was applied without it. The answer is
    /// saved to the layout; the check and entries are made again.
    pub(crate) fn can_match_again(&self) -> bool {
        self.provider.mode() == FileMode::EmployeeLevel
            && matches!(self.step, Step::Accounts | Step::Check | Step::Preview)
            && self.upload.is_some()
            && !self.busy()
            && !self.in_flight()
            && !self.post_unsettled
            && self.overlaps.is_none()
            && self.asking_password.is_none()
    }

    pub(crate) fn match_again(&mut self) {
        if !self.can_match_again() {
            return;
        }
        self.save_layout = true;
        self.validation = None;
        self.preview = None;
        self.plan = TagPlan::default();
        self.untagged = None;
        self.key = None;
        self.force = false;
        self.clear_backup_notice();
        self.go(Step::Map);
    }

    pub(crate) fn skip_map(&self) -> bool {
        self.provider.mode() == FileMode::Prebuilt
            || (self.provider.is_generic()
                && matches!(self.layout_choice, Some(LayoutChoice::Saved(_)))
                && !self.visited.contains(&Step::Map))
    }

    pub(crate) fn back(&mut self) {
        // QA-15: after a lost answer Back waits too, until Check status.
        if self.busy() || self.in_flight() || self.post_unsettled {
            return;
        }
        if let Some(s) = self.step.back(self.skip_map()) {
            if s == Step::Choose {
                self.forget_upload();
            }
            self.step = s;
            self.clear_trouble();
            self.clear_backup_notice();
            self.untagged = None;
        }
    }

    /// Cancel: a draft uploaded and not posted is discarded (Q-P30c) — but
    /// never while a post or reverse is out (Cancel is disabled then), nor
    /// after a post's answer was lost: that draft is kept for History's
    /// "Check status" (B1).
    pub(crate) fn cancel(&mut self) {
        // Q-P30h: Cancel while the backup is made stops the post only.
        if self.stop_backup() {
            return;
        }
        // QA-15: Cancel is off while a post is out or its answer was lost
        // (Check status settles it; closing the tab keeps the import).
        if self.in_flight() || self.post_unsettled {
            return;
        }
        // QA-8: a chosen file or an import under way asks first, as the
        // tab's close button does; closing then discards it (Drop).
        if Screen::dirty(self) {
            self.link
                .push(CompanyEvent::RequestClose(CommandId::new(KIND)));
            return;
        }
        self.release_upload();
        // The demo answers at once; the engine's answer is not waited for.
        let _ = poll(&mut self.discard_job);
        self.link
            .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
    }

    /// Demo and scenarios: a fixture with its golden's period.
    fn demo_file(&mut self, stem: &str) -> bool {
        let Some(bytes) = crate::company::demo::payroll::fixture_bytes(stem) else {
            return false;
        };
        let (provider, period) = match stem {
            "gusto" => (Provider::Gusto, ["12/28/2024", "01/24/2025", "01/24/2025"]),
            "adp_run_gli" => (Provider::AdpRun, ["02/01/2025", "02/14/2025", "02/14/2025"]),
            "quickbooks_payroll" => (
                Provider::QuickBooksPayroll,
                ["03/01/2025", "03/14/2025", "03/14/2025"],
            ),
            "paychex_flex_gls" => (
                Provider::Paychex,
                ["03/29/2025", "04/11/2025", "04/11/2025"],
            ),
            "square" => (Provider::Square, ["04/26/2025", "05/09/2025", "05/09/2025"]),
            "onpay_gl_summary" => (Provider::OnPay, ["05/31/2025", "06/13/2025", "06/13/2025"]),
            "generic_period1" => (
                Provider::GenericCsv,
                ["06/28/2025", "07/11/2025", "07/11/2025"],
            ),
            _ => (
                Provider::GenericCsv,
                ["07/12/2025", "07/25/2025", "07/25/2025"],
            ),
        };
        self.provider = provider;
        self.period_text = period.map(str::to_string);
        self.take_file(format!("{stem}.csv"), bytes.to_vec());
        true
    }

    pub(crate) fn link_ref(&self) -> &CompanyLink {
        &self.link
    }

    pub(crate) fn templates_list(&self) -> &[PayrollTemplate] {
        &self.templates
    }

    /// Starts the wizard again (scenarios run several imports in one window).
    pub(crate) fn start_over(&mut self) {
        let keep = (
            Arc::clone(&self.link),
            self.today,
            self.templates.clone(),
            self.chart.clone(),
            self.tags.clone(),
            self.line_accounts.clone(),
        );
        *self = PayrollImportScreen::new(keep.0, keep.1);
        self.templates = keep.2;
        self.chart = keep.3;
        self.tags = keep.4;
        self.line_accounts.extend(keep.5);
    }
}

impl Drop for PayrollImportScreen {
    /// Closing the window (the close prompt answered "Don't save", or the
    /// app quitting) is Cancel: a draft uploaded and not posted is
    /// discarded (Q-P30c). The write outlives the window (an orphan write).
    /// A post out or whose answer was lost keeps the draft (B1): the close
    /// prompt said it may be posted; History's "Check status" settles it.
    fn drop(&mut self) {
        self.release_upload();
    }
}

impl Screen for PayrollImportScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn fixed_layout(&self) -> bool {
        true
    }

    /// A chosen file or an uploaded one not yet posted: closing (or
    /// Cancel) asks first (QA-8).
    fn dirty(&self) -> bool {
        (self.file.is_some() || self.upload.is_some()) && self.posted.is_none()
    }

    /// A post is out or its answer was lost: the close prompt says the
    /// import may already be posted and that closing keeps it (B1).
    fn save_unconfirmed(&self) -> bool {
        self.upload.is_some() && self.posted.is_none() && !self.may_discard()
    }

    fn can_save(&self) -> bool {
        false
    }

    /// QA-13: the payroll wizard's own question (not the bank import's).
    fn close_words(&self, _quitting: bool) -> Option<crate::windows::CloseWords> {
        Some(crate::windows::CloseWords {
            message: t("payroll.close_prompt"),
            close: t("payroll.close_prompt.discard"),
            keep: t("payroll.close_prompt.keep"),
        })
    }

    /// P31 QA-20: a post whose backup finished under the close prompt waits
    /// for the answer (Keep sends it; Close drops the window unsent).
    fn close_asked(&mut self, asked: bool) {
        self.close_asked = asked;
        if !asked && let Some(held) = self.parked_post.take() {
            self.send_post(held);
        }
    }

    fn ui(&mut self, ui: &mut eframe::egui::Ui) {
        self.take_history_request();
        self.poll();
        if self.busy() || self.prior_job.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        } else if matches!(self.trouble, Some(Trouble::EngineDown | Trouble::NotSent)) {
            // QA-15: notice the books coming back without any input.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(500));
        }
        self.draw(ui);
    }

    fn scope(&self) -> cl_model::Scope {
        cl_model::Scope::Form
    }

    fn command(&mut self, id: CommandId) -> bool {
        let s = id.as_str();
        if let Some(stem) = s.strip_prefix("dev.payroll_demo_file_") {
            return self.link.is_demo() && self.demo_file(stem);
        }
        match s {
            "form.save_new" => {
                self.next();
                true
            }
            "form.step_back" => {
                self.back();
                true
            }
            "form.revert" => {
                if self.stop_backup() {
                    return true;
                }
                if self.asking_password.is_some() || self.overlaps.is_some() {
                    self.asking_password = None;
                    self.overlaps = None;
                    return true;
                }
                match self.step {
                    Step::Map | Step::Accounts | Step::Check | Step::Preview => {
                        self.back();
                        true
                    }
                    Step::Choose | Step::Done => false,
                }
            }
            "dev.payroll_demo_reset" => {
                let Some(mut d) = self.link.demo_state() else {
                    return false;
                };
                d.payroll.reset();
                // A previous theme's run may have turned the tag rule on.
                d.require_tag_every_line = false;
                // The demo story's books are closed through 09/30/2026,
                // after every payroll fixture: the scenarios open them
                // (dev.payroll_demo_close_books closes them again).
                if let Some(open) = d.open
                    && let Some(c) = d.companies.iter_mut().find(|c| c.tenant_id == open)
                {
                    c.closed_through = None;
                }
                drop(d);
                self.templates.clear();
                self.line_accounts.clear();
                self.start_over();
                true
            }
            // Scenarios: a viewer with hidden accounts (U-247).
            "dev.payroll_demo_restricted" => {
                if !self.link.is_demo() {
                    return false;
                }
                let Ok(doors) = serde_json::from_str::<cl_core::sensitive_doors::SensitiveDoors>(
                    cl_core::sensitive_doors::ENGINE_ANSWER_JSON,
                ) else {
                    return false;
                };
                drop(doors);
                self.link
                    .set_rights(cl_model::permissions::BuiltinRole::Bookkeeper.rights());
                let first = crate::company::demo::chart().first().map(|a| a.id);
                if let Some(mut d) = self.link.demo_state() {
                    d.hides_sensitive = true;
                    d.sensitive.extend(first);
                }
                self.trouble = Some(Trouble::Restricted);
                true
            }
            // Scenarios: books closed through 03/31/2025 (the demo password).
            "dev.payroll_demo_close_books" => {
                let Some(mut d) = self.link.demo_state() else {
                    return false;
                };
                if let Some(open) = d.open
                    && let Some(c) = d.companies.iter_mut().find(|c| c.tenant_id == open)
                {
                    c.closed_through = CalendarDate::from_ymd(2025, 3, 31).ok();
                }
                true
            }
            // Scenarios: "Require a tag on every line" on, no default tag.
            "dev.payroll_demo_require_tag" => {
                let Some(mut d) = self.link.demo_state() else {
                    return false;
                };
                d.require_tag_every_line = true;
                d.default_tag = None;
                true
            }
            // Scenarios: the saved layouts forget their left-out columns (as
            // one saved before S4), so the next file's unmatched amount
            // columns count as new.
            "dev.payroll_demo_forget_left_out" => {
                let Some(mut d) = self.link.demo_state() else {
                    return false;
                };
                d.payroll.forget_left_out();
                self.templates = d.payroll.templates();
                true
            }
            // Scenarios (Q-P30h): the next before-post backup fails, or
            // stays running until released.
            "dev.payroll_demo_backup_fail" | "dev.payroll_demo_backup_hold" => {
                let Some(mut d) = self.link.demo_state() else {
                    return false;
                };
                d.payroll.backup_mode = if s.ends_with("fail") {
                    crate::company::demo::payroll::DemoBackup::FailOnce
                } else {
                    crate::company::demo::payroll::DemoBackup::HoldOnce
                };
                true
            }
            "dev.payroll_demo_backup_release" => {
                if !self.link.is_demo() || self.backup_job.is_none() {
                    return false;
                }
                if let Some(mut d) = self.link.demo_state() {
                    d.payroll.backups_taken += 1;
                }
                self.backup_job = Some(Job::Done(Some(Ok(Ok(())))));
                true
            }
            "dev.payroll_start_over" => {
                self.start_over();
                true
            }
            _ => false,
        }
    }

    fn state(&self) -> Value {
        // Steps, counts and flags only: never amounts, names, departments,
        // file names or cell text.
        let preview = self.preview.as_ref().map(|p| {
            json!({
                "entries": p.entries.len(),
                "lines": p.entries.iter().map(|e| e.lines.len()).sum::<usize>(),
                "balanced": p.is_balanced(),
                "split_lines": p.entries.iter().flat_map(|e| &e.lines).filter(|l| l.department.is_some()).count(),
                "tagged_lines": p.entries.iter().flat_map(|e| &e.lines).filter(|l| l.tag.is_some()).count(),
            })
        });
        let validation = self.validation.as_ref().map(|v| {
            json!({
                "rows": v.total_rows,
                "errors": v.messages.iter().filter(|m| m.is_error).count(),
                "warnings": v.messages.iter().filter(|m| !m.is_error).count(),
            })
        });
        let mut s = json!({
            "screen": KIND,
            "view": match self.view { View::Import => "import", View::History => "history" },
            "step": self.step.key(),
            "steps_visited": self.visited.iter().map(|s| s.key()).collect::<Vec<_>>(),
            "provider": self.provider.key(),
            "file_chosen": self.file.is_some(),
            "file_error": self.file_error,
            "period_problem": self.period_problem.map(PeriodProblem::key),
            "can_next": self.can_next(),
            "busy": self.busy(),
            "uploaded": self.upload.is_some(),
            "duplicate_warning": self.upload.as_ref().is_some_and(|u| u.is_duplicate || u.is_duplicate_by_key),
            "layout": self.layout_choice.as_ref().map(|c| match c {
                LayoutChoice::Saved(_) => "saved",
                LayoutChoice::Builtin => "builtin",
                LayoutChoice::Manual => "manual",
                LayoutChoice::NotNeeded => "not_needed",
            }),
            "new_columns": self.new_columns.len(),
            "can_match_again": self.can_match_again(),
            "departments": self.departments.len(),
            "departments_from_check": self.file_departments.is_some(),
            "departments_truncated": self.departments_truncated,
            "departments_tagged": self.department_tags.values().filter(|t| t.is_some()).count(),
            "accounts_complete": self.accounts_complete(),
            "net_pay_from_bank": self.net_pay_from_bank(),
            "validation": validation,
            "preview": preview,
            "untagged": self.untagged.as_ref().map(|u| u.count),
            "overlap_open": self.overlaps.as_ref().map(Vec::len),
            "password_open": self.asking_password.is_some(),
            "trouble": self.trouble.as_ref().map(Trouble::code),
            "status_posted": self.status_answer,
            "posted": self.posted.as_ref().map(Vec::len),
            "expected_withdrawals": self.posted.as_ref().map(|_| self.expected_withdrawals()),
            "posts_sent": self.posts_sent,
            "post_unsettled": self.post_unsettled,
            "in_flight": self.in_flight(),
            "allowed": self.allowed(),
            "history": self.history.as_ref().map(|h| json!({
                "rows": h.sessions.len(),
                "posted": h.sessions.iter().filter(|s| s.status == SessionStatus::Posted).count(),
                "reversed": h.sessions.iter().filter(|s| s.status == SessionStatus::Cancelled).count(),
                "unposted": h.sessions.iter().filter(|s| s.status.is_unposted_draft()).count(),
            })),
            "reversed": self.reversed,
        });
        // QA-6, QA-11 (flags only).
        s["posted_duplicate"] = json!(self.posted_duplicate());
        s["service_mismatch"] = json!(self.service_mismatch());
        // QA-12, QA-14, QA-15 (flags and counts only).
        s["prior_posted_known"] = json!(self.prior_posted_at().is_some());
        s["tags_still_needed"] = json!(self.tags_still_needed());
        // Q-P30h: the backup before a post (flags only; never its path).
        s["backing_up"] = json!(self.backing_up());
        s["backup_failed"] = json!(self.backup_failed.is_some());
        s["backup_stopped"] = json!(self.backup_stopped);
        s["unconfirmed_drafts"] = json!(self.history.as_ref().map_or(0, |h| {
            h.sessions
                .iter()
                .filter(|x| self.is_unsettled(x.id))
                .count()
        }));
        s
    }
}
