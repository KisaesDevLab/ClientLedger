//! Daily Sales Summary (P14 lane B, `clients.daily_sales`): one business
//! day's register report on the Write Check pattern (Q-261): the header
//! card (Layout Â· Date Â· Tag Â· Notes), the lines by section, the summary
//! with the cash over/short, Clear Â· Save & New Â· Save & Close, and a side
//! panel (Payments Clearing, the days not posted yet, the last 10 days).
//!
//! Money (plan Â§4): every amount is the owner's typed `Money`. The form's
//! totals and over/short come from `cl_model::forms::daily_sales::totals`,
//! an exact twin of the engine's `computeEntry`; the engine posts the
//! over/short line itself. A save writes a draft, asks the engine for its
//! preview, and **posts only when the preview's over/short equals the
//! form's to the cent** and no line is unmapped (Q-P14w); otherwise
//! nothing posts and the layout is read again. A failed post keeps the
//! draft, and unposted days show again when the form opens (Q-P14u).
//!
//! Every engine call runs on the request queue and is polled each frame.
//! The demo engine and the tests answer through [`DailyBooks`] at once.

mod form;
mod layout;
mod save;
mod screen;
mod template;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use cl_api::{
    Account, AccountFilter, ApiError, ClosedPeriodAuthorization, CompanySettings, DailyEntry,
    DailyEntryDraft, DailyEntryQuery, DailyEntrySummary, DailyLineInput, DailyPostOptions,
    DailyPreview, DailyTemplate, DailyValue, IdempotencyKey, NormalSide, Preset, RequestGroup,
    Section, Session, Tag, TagFilter,
};
use cl_core::ids::{
    AccountId, DailyEntryId, DailyLineId, DailyTemplateId, TenantId, TransactionId,
};
use cl_core::{CalendarDate, Secret};
use cl_model::CommandId;
use cl_model::forms::FormState;
use cl_model::forms::daily_sales::{
    DailyLayout, DailyProblem, DailySalesForm, DailySection, DailySide, LayoutLine,
};

use crate::company::{CompanyLink, Task};
use crate::forms::closing_prompt::ClosingPrompt;
use crate::screens::company::form::DateInput;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "clients.daily_sales";

/// The side panel lists this many recent days.
const RECENT_DAYS: u16 = 10;

/// The Daily Sales Summary window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(DailySalesScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

// ------------------------------------------------------------------ books

/// The engine's daily sales answered at once: the demo engine (WP-D.3)
/// and the screen tests. Mirrors `cl_api::DailySales` call for call; the
/// demo refuses exactly what the engine refuses (closing date with the
/// password "demo-close", unmapped lines, a day already posted).
pub trait DailyBooks: Send {
    /// Active layouts (without lines).
    ///
    /// # Errors
    /// As the engine.
    fn templates(&mut self) -> Result<Vec<DailyTemplate>, ApiError>;
    /// One layout with its lines.
    ///
    /// # Errors
    /// As the engine.
    fn template(&mut self, id: DailyTemplateId) -> Result<DailyTemplate, ApiError>;
    /// A new layout, blank or from a preset.
    ///
    /// # Errors
    /// As the engine.
    fn create_template(
        &mut self,
        name: &str,
        preset: Option<Preset>,
    ) -> Result<DailyTemplate, ApiError>;
    /// Renames a layout.
    ///
    /// # Errors
    /// As the engine.
    fn rename(&mut self, id: DailyTemplateId, name: &str) -> Result<DailyTemplate, ApiError>;
    /// Replaces a layout's lines.
    ///
    /// # Errors
    /// As the engine.
    fn replace_lines(
        &mut self,
        id: DailyTemplateId,
        lines: &[DailyLineInput],
    ) -> Result<DailyTemplate, ApiError>;
    /// Deletes a layout.
    ///
    /// # Errors
    /// As the engine.
    fn delete_template(&mut self, id: DailyTemplateId) -> Result<(), ApiError>;
    /// Days, newest first.
    ///
    /// # Errors
    /// As the engine.
    fn list(&mut self, q: &DailyEntryQuery) -> Result<Vec<DailyEntrySummary>, ApiError>;
    /// One day.
    ///
    /// # Errors
    /// As the engine.
    fn entry(&mut self, id: DailyEntryId) -> Result<DailyEntry, ApiError>;
    /// A new draft day.
    ///
    /// # Errors
    /// As the engine.
    fn create_draft(&mut self, d: &DailyEntryDraft) -> Result<DailyEntry, ApiError>;
    /// Replaces a draft day.
    ///
    /// # Errors
    /// As the engine.
    fn update_draft(
        &mut self,
        id: DailyEntryId,
        d: &DailyEntryDraft,
    ) -> Result<DailyEntry, ApiError>;
    /// The engine's figures for unsaved values.
    ///
    /// # Errors
    /// As the engine.
    fn preview(
        &mut self,
        template: DailyTemplateId,
        values: &[DailyValue],
    ) -> Result<DailyPreview, ApiError>;
    /// Posts a draft day.
    ///
    /// # Errors
    /// As the engine.
    fn post(&mut self, id: DailyEntryId, password: Option<&Secret>)
    -> Result<DailyEntry, ApiError>;
    /// Voids a posted day.
    ///
    /// # Errors
    /// As the engine.
    fn void(&mut self, id: DailyEntryId, password: Option<&Secret>)
    -> Result<DailyEntry, ApiError>;
}

/// Shared [`DailyBooks`].
pub type SharedBooks = Arc<Mutex<dyn DailyBooks>>;

/// The demo engine's daily sales (`company::demo::daily_sales`, WP-D.3);
/// `None` with an engine.
fn demo_books(link: &Arc<CompanyLink>) -> Option<SharedBooks> {
    link.is_demo().then(|| {
        Arc::new(Mutex::new(
            crate::company::demo::daily_sales::DemoDailyBooks::new(Arc::clone(link)),
        )) as SharedBooks
    })
}

/// The calls the screen makes, over the engine (async) or [`DailyBooks`]
/// (answered at once).
pub(crate) trait DailyApi {
    async fn templates(&self) -> Result<Vec<DailyTemplate>, ApiError>;
    async fn template(&self, id: DailyTemplateId) -> Result<DailyTemplate, ApiError>;
    async fn create_template(
        &self,
        name: &str,
        preset: Option<Preset>,
    ) -> Result<DailyTemplate, ApiError>;
    async fn rename(&self, id: DailyTemplateId, name: &str) -> Result<DailyTemplate, ApiError>;
    async fn replace_lines(
        &self,
        id: DailyTemplateId,
        lines: &[DailyLineInput],
    ) -> Result<DailyTemplate, ApiError>;
    async fn delete_template(&self, id: DailyTemplateId) -> Result<(), ApiError>;
    async fn list(&self, q: &DailyEntryQuery) -> Result<Vec<DailyEntrySummary>, ApiError>;
    async fn entry(&self, id: DailyEntryId) -> Result<DailyEntry, ApiError>;
    async fn create_draft(
        &self,
        d: &DailyEntryDraft,
        key: IdempotencyKey,
    ) -> Result<DailyEntry, ApiError>;
    async fn update_draft(
        &self,
        id: DailyEntryId,
        d: &DailyEntryDraft,
        key: IdempotencyKey,
    ) -> Result<DailyEntry, ApiError>;
    async fn preview(
        &self,
        template: DailyTemplateId,
        values: &[DailyValue],
    ) -> Result<DailyPreview, ApiError>;
    async fn post(
        &self,
        id: DailyEntryId,
        password: Option<Secret>,
    ) -> Result<DailyEntry, ApiError>;
    async fn void(
        &self,
        id: DailyEntryId,
        password: Option<Secret>,
    ) -> Result<DailyEntry, ApiError>;
}

fn options(password: Option<Secret>) -> DailyPostOptions {
    DailyPostOptions {
        closed_period: password.map(ClosedPeriodAuthorization::with_password),
    }
}

/// The engine.
pub(crate) struct Live(pub Session);

impl DailyApi for Live {
    async fn templates(&self) -> Result<Vec<DailyTemplate>, ApiError> {
        self.0.daily_sales().templates().await
    }
    async fn template(&self, id: DailyTemplateId) -> Result<DailyTemplate, ApiError> {
        self.0.daily_sales().template(id).await
    }
    async fn create_template(
        &self,
        name: &str,
        preset: Option<Preset>,
    ) -> Result<DailyTemplate, ApiError> {
        self.0.daily_sales().create_template(name, preset).await
    }
    async fn rename(&self, id: DailyTemplateId, name: &str) -> Result<DailyTemplate, ApiError> {
        self.0.daily_sales().rename(id, name).await
    }
    async fn replace_lines(
        &self,
        id: DailyTemplateId,
        lines: &[DailyLineInput],
    ) -> Result<DailyTemplate, ApiError> {
        self.0.daily_sales().replace_lines(id, lines).await
    }
    async fn delete_template(&self, id: DailyTemplateId) -> Result<(), ApiError> {
        self.0.daily_sales().delete_template(id).await
    }
    async fn list(&self, q: &DailyEntryQuery) -> Result<Vec<DailyEntrySummary>, ApiError> {
        self.0.daily_sales().list(q).await
    }
    async fn entry(&self, id: DailyEntryId) -> Result<DailyEntry, ApiError> {
        self.0.daily_sales().entry(id).await
    }
    async fn create_draft(
        &self,
        d: &DailyEntryDraft,
        key: IdempotencyKey,
    ) -> Result<DailyEntry, ApiError> {
        self.0.daily_sales().create_draft(d, key).await
    }
    async fn update_draft(
        &self,
        id: DailyEntryId,
        d: &DailyEntryDraft,
        key: IdempotencyKey,
    ) -> Result<DailyEntry, ApiError> {
        self.0.daily_sales().update_draft(id, d, key).await
    }
    async fn preview(
        &self,
        template: DailyTemplateId,
        values: &[DailyValue],
    ) -> Result<DailyPreview, ApiError> {
        self.0.daily_sales().preview(template, values).await
    }
    async fn post(
        &self,
        id: DailyEntryId,
        password: Option<Secret>,
    ) -> Result<DailyEntry, ApiError> {
        self.0
            .daily_sales()
            .post(id, &options(password), IdempotencyKey::new())
            .await
    }
    async fn void(
        &self,
        id: DailyEntryId,
        password: Option<Secret>,
    ) -> Result<DailyEntry, ApiError> {
        self.0
            .daily_sales()
            .void(id, &options(password), IdempotencyKey::new())
            .await
    }
}

/// [`DailyBooks`] as [`DailyApi`]: every call is answered at once.
pub(crate) struct Now(pub SharedBooks);

impl Now {
    fn with<T>(&self, f: impl FnOnce(&mut dyn DailyBooks) -> T) -> T {
        let mut b = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut *b)
    }
}

#[allow(
    clippy::unused_async_trait_impl,
    reason = "the books answer at once; the trait is async for the engine"
)]
impl DailyApi for Now {
    async fn templates(&self) -> Result<Vec<DailyTemplate>, ApiError> {
        self.with(|b| b.templates())
    }
    async fn template(&self, id: DailyTemplateId) -> Result<DailyTemplate, ApiError> {
        self.with(|b| b.template(id))
    }
    async fn create_template(
        &self,
        name: &str,
        preset: Option<Preset>,
    ) -> Result<DailyTemplate, ApiError> {
        self.with(|b| b.create_template(name, preset))
    }
    async fn rename(&self, id: DailyTemplateId, name: &str) -> Result<DailyTemplate, ApiError> {
        self.with(|b| b.rename(id, name))
    }
    async fn replace_lines(
        &self,
        id: DailyTemplateId,
        lines: &[DailyLineInput],
    ) -> Result<DailyTemplate, ApiError> {
        self.with(|b| b.replace_lines(id, lines))
    }
    async fn delete_template(&self, id: DailyTemplateId) -> Result<(), ApiError> {
        self.with(|b| b.delete_template(id))
    }
    async fn list(&self, q: &DailyEntryQuery) -> Result<Vec<DailyEntrySummary>, ApiError> {
        self.with(|b| b.list(q))
    }
    async fn entry(&self, id: DailyEntryId) -> Result<DailyEntry, ApiError> {
        self.with(|b| b.entry(id))
    }
    async fn create_draft(
        &self,
        d: &DailyEntryDraft,
        _key: IdempotencyKey,
    ) -> Result<DailyEntry, ApiError> {
        self.with(|b| b.create_draft(d))
    }
    async fn update_draft(
        &self,
        id: DailyEntryId,
        d: &DailyEntryDraft,
        _key: IdempotencyKey,
    ) -> Result<DailyEntry, ApiError> {
        self.with(|b| b.update_draft(id, d))
    }
    async fn preview(
        &self,
        template: DailyTemplateId,
        values: &[DailyValue],
    ) -> Result<DailyPreview, ApiError> {
        self.with(|b| b.preview(template, values))
    }
    async fn post(
        &self,
        id: DailyEntryId,
        password: Option<Secret>,
    ) -> Result<DailyEntry, ApiError> {
        self.with(|b| b.post(id, password.as_ref()))
    }
    async fn void(
        &self,
        id: DailyEntryId,
        password: Option<Secret>,
    ) -> Result<DailyEntry, ApiError> {
        self.with(|b| b.void(id, password.as_ref()))
    }
}

/// The answer of a future that is ready on its first poll ([`Now`]'s
/// calls never wait); `Internal` if it is not.
pub(crate) fn at_once<T>(fut: impl Future<Output = Result<T, ApiError>>) -> Result<T, ApiError> {
    let mut fut = std::pin::pin!(fut);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match fut.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(v) => v,
        std::task::Poll::Pending => Err(ApiError::Internal),
    }
}

/// One request of the screen (data only, so the same call runs over the
/// engine or the books).
pub(crate) enum Call {
    Templates,
    Template(DailyTemplateId),
    CreateTemplate(String, Option<Preset>),
    /// Save a layout: rename (when a name is given) and replace its lines;
    /// or, with no id, create it first ("Save as new layout").
    SaveLayout {
        id: Option<DailyTemplateId>,
        name: Option<String>,
        lines: Vec<DailyLineInput>,
    },
    DeleteTemplate(DailyTemplateId),
    List(DailyEntryQuery),
    Entry(DailyEntryId),
    /// Find the posted day of a transaction (`OpenRecord`).
    FindTransaction(TransactionId),
    Save(save::Plan),
    Void(DailyEntryId, Option<Secret>),
}

/// A running [`Call`] (its error inside: the books answer at once).
pub(crate) type Job = Task<Result<Reply, ApiError>>;

/// The answer of a [`Call`].
pub(crate) enum Reply {
    Templates(Vec<DailyTemplate>),
    Template(DailyTemplate),
    Unit,
    Days(Vec<DailyEntrySummary>),
    Entry(DailyEntry),
    Found(Option<DailyEntryId>),
    Outcome(save::Outcome),
}

/// Runs `call` over `api`.
pub(crate) async fn run<A: DailyApi>(api: &A, call: Call) -> Result<Reply, ApiError> {
    Ok(match call {
        Call::Templates => Reply::Templates(api.templates().await?),
        Call::Template(id) => Reply::Template(api.template(id).await?),
        Call::CreateTemplate(name, preset) => {
            Reply::Template(api.create_template(&name, preset).await?)
        }
        Call::SaveLayout { id, name, lines } => {
            let id = match (id, &name) {
                (Some(id), Some(n)) => api.rename(id, n).await?.id,
                (Some(id), None) => id,
                (None, n) => {
                    let n = n.clone().unwrap_or_default();
                    api.create_template(&n, None).await?.id
                }
            };
            Reply::Template(api.replace_lines(id, &lines).await?)
        }
        Call::DeleteTemplate(id) => {
            api.delete_template(id).await?;
            Reply::Unit
        }
        Call::List(q) => Reply::Days(api.list(&q).await?),
        Call::Entry(id) => Reply::Entry(api.entry(id).await?),
        Call::FindTransaction(txn) => {
            let q = DailyEntryQuery {
                limit: Some(500),
                ..DailyEntryQuery::default()
            };
            let days = api.list(&q).await?;
            Reply::Found(
                days.iter()
                    .find(|d| d.transaction == Some(txn))
                    .map(|d| d.id),
            )
        }
        Call::Save(plan) => Reply::Outcome(save::flow(api, plan).await?),
        Call::Void(id, password) => Reply::Entry(api.void(id, password).await?),
    })
}

// ------------------------------------------------------------ the screen

/// A header field the keyboard can be put in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Field {
    Layout,
    Date,
    Tag,
    Notes,
    Amount(DailyLineId),
}

/// Undo merges typing within one field.
mod field {
    pub const LAYOUT: u64 = 1;
    pub const DATE: u64 = 2;
    pub const TAG: u64 = 3;
    pub const NOTES: u64 = 4;
    /// Amount fields start here (plus the line's position).
    pub const AMOUNT: u64 = 100;
}

/// The texts of the inputs as typed (the form holds the values).
#[derive(Clone, Debug)]
pub(crate) struct Inputs {
    pub layout: String,
    pub date: DateInput,
    pub amounts: BTreeMap<DailyLineId, String>,
    /// Why a typed amount is not taken (i18n key).
    pub amount_problems: BTreeMap<DailyLineId, &'static str>,
    pub layout_problem: bool,
    pub rev: u64,
}

/// What the closing-date password is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    Save(cl_model::forms::SaveIntent),
    Void,
}

/// A day the engine has, shown in the form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Saved {
    pub id: DailyEntryId,
    pub status: cl_api::EntryStatus,
    pub transaction: Option<TransactionId>,
    pub date: CalendarDate,
}

/// Saving and its outcome.
#[derive(Default)]
pub(crate) struct SaveState {
    pub task: Option<Job>,
    pub intent: Option<cl_model::forms::SaveIntent>,
    pub key: Option<(save::SaveIdentity, IdempotencyKey)>,
    pub problems: Vec<DailyProblem>,
    pub error: Option<ApiError>,
    /// A plain note under the toolbar (i18n key).
    pub notice: Option<&'static str>,
    pub count: u32,
    /// `post`, `void`, `clear`, `load`, `mismatch`, `not_posted`.
    pub last_action: Option<&'static str>,
}

/// Void: the confirmation and its request.
#[derive(Default)]
pub(crate) struct VoidState {
    pub open: bool,
    pub task: Option<Job>,
}

/// Prev/Next over the saved days (newest first).
#[derive(Default)]
pub(crate) struct DayNav {
    pub list: Vec<DailyEntrySummary>,
    pub task: Option<Job>,
    pub loaded: bool,
    pub at: Option<usize>,
    pub want_older: Option<bool>,
}

/// Prev/Next reads this many saved days.
pub(crate) const NAV_DAYS: u16 = 100;

/// The side panel's readers (each fails on its own).
#[derive(Default)]
pub(crate) struct PanelState {
    pub recent: Vec<DailyEntrySummary>,
    pub recent_task: Option<Job>,
    pub recent_failed: bool,
    pub drafts: Vec<DailyEntrySummary>,
    pub drafts_task: Option<Job>,
}

/// The Daily Sales Summary window.
pub struct DailySalesScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) tenant: Option<(TenantId, String)>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) books: Option<SharedBooks>,
    pub(crate) today: CalendarDate,
    pub(crate) accounts: Vec<Account>,
    pub(crate) accounts_task: Option<Task<Vec<Account>>>,
    pub(crate) tags: Vec<Tag>,
    pub(crate) tags_task: Option<Task<Vec<Tag>>>,
    pub(crate) settings_task: Option<Task<CompanySettings>>,
    pub(crate) closing: Option<CalendarDate>,
    pub(crate) templates: Vec<DailyTemplate>,
    pub(crate) templates_task: Option<Job>,
    pub(crate) templates_loaded: bool,
    /// The layout in use, with its lines.
    pub(crate) template: Option<DailyTemplate>,
    pub(crate) template_task: Option<Job>,
    pub(crate) layout: DailyLayout,
    pub(crate) header: FormState<DailySalesForm>,
    pub(crate) inputs: Inputs,
    /// The day in the form, once the engine has it: a draft (a failed
    /// post, Q-P14u) or a posted or void day (read-only).
    pub(crate) saved: Option<Saved>,
    pub(crate) load_task: Option<Job>,
    pub(crate) save: SaveState,
    pub(crate) closing_prompt: ClosingPrompt<Pending>,
    pub(crate) void: VoidState,
    pub(crate) panel: PanelState,
    pub(crate) setup: Option<layout::Setup>,
    pub(crate) setup_task: Option<Job>,
    pub(crate) focus: Option<Field>,
    pub(crate) focused: Option<Field>,
    pub(crate) layout_list: bool,
    pub(crate) after_frame: Option<CommandId>,
    /// The fixed bars' sizes last frame (Q-265).
    pub(crate) frame_report: crate::layout::FrameReport,
    /// Where the condensed lines went last frame (Q-267; sizes only).
    pub(crate) condensed: crate::forms::condensed::CondensedReport,
    pub(crate) money_style: cl_core::MoneyStyle,
    pub(crate) bus: crate::data_bus::Subscription,
    pub(crate) open_task: Option<Job>,
    /// Another window changed the lists while the form was dirty.
    pub(crate) lists_stale: bool,
    pub(crate) nav: DayNav,
    /// Prev (true) or Next asked on unsaved changes: Save / Discard /
    /// Keep editing.
    pub(crate) nav_prompt: Option<bool>,
    /// Prev/Next to take once the prompt's save is done.
    pub(crate) after_save_nav: Option<bool>,
    /// Save as Template and the template this day came from (P15).
    pub(crate) save_template: crate::screens::templates::SaveFlow,
}

/// The cl-model section of an engine section (`None`: unknown).
pub(crate) fn section_of(s: &Section) -> Option<DailySection> {
    Some(match s {
        Section::Sales => DailySection::Sales,
        Section::Tax => DailySection::Tax,
        Section::Tips => DailySection::Tips,
        Section::Discount => DailySection::Discount,
        Section::Payment => DailySection::Payment,
        Section::Payout => DailySection::Payout,
        Section::Other => DailySection::Other,
        Section::Unknown(_) => return None,
    })
}

/// The engine section of a cl-model section.
pub(crate) fn engine_section(s: DailySection) -> Section {
    match s {
        DailySection::Sales => Section::Sales,
        DailySection::Tax => Section::Tax,
        DailySection::Tips => Section::Tips,
        DailySection::Discount => Section::Discount,
        DailySection::Payment => Section::Payment,
        DailySection::Payout => Section::Payout,
        DailySection::Other => Section::Other,
    }
}

/// The form's layout of an engine layout (all lines, in form order).
pub(crate) fn layout_of(t: &DailyTemplate) -> DailyLayout {
    let mut lines: Vec<_> = t.lines.clone();
    lines.sort_by_key(|l| l.sort);
    DailyLayout {
        name: t.name.clone(),
        lines: lines
            .iter()
            .map(|l| LayoutLine {
                id: Some(l.id),
                section: section_of(&l.section),
                label: l.label.clone(),
                account: l.account,
                side: match l.side {
                    NormalSide::Debit => DailySide::Debit,
                    NormalSide::Credit => DailySide::Credit,
                },
                active: l.active,
            })
            .collect(),
    }
}

/// The engine's account with `tag` (`payments_clearing`, â€¦).
pub(crate) fn system_account(accounts: &[Account], tag: &str) -> Option<AccountId> {
    accounts
        .iter()
        .find(|a| a.system_tag.as_deref() == Some(tag))
        .map(|a| a.id)
}

impl DailySalesScreen {
    /// A new day in the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> DailySalesScreen {
        let books = demo_books(&link);
        DailySalesScreen::with_books(link, tenant, today, books)
    }

    /// A new day answered by `books` (the demo engine, tests).
    #[must_use]
    pub fn with_books(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
        books: Option<SharedBooks>,
    ) -> DailySalesScreen {
        let group = link.group().and_then(Result::ok);
        let mut s = DailySalesScreen {
            link,
            tenant,
            group,
            books,
            today,
            accounts: Vec::new(),
            accounts_task: None,
            tags: Vec::new(),
            tags_task: None,
            settings_task: None,
            closing: None,
            templates: Vec::new(),
            templates_task: None,
            templates_loaded: false,
            template: None,
            template_task: None,
            layout: DailyLayout::default(),
            header: FormState::new(DailySalesForm {
                date: Some(today),
                ..DailySalesForm::default()
            }),
            inputs: Inputs {
                layout: String::new(),
                date: DateInput::new(today),
                amounts: BTreeMap::new(),
                amount_problems: BTreeMap::new(),
                layout_problem: false,
                rev: u64::MAX,
            },
            saved: None,
            load_task: None,
            save: SaveState::default(),
            closing_prompt: ClosingPrompt::default(),
            void: VoidState::default(),
            panel: PanelState::default(),
            setup: None,
            setup_task: None,
            focus: Some(Field::Layout),
            focused: None,
            layout_list: false,
            after_frame: None,
            frame_report: crate::layout::FrameReport::default(),
            condensed: crate::forms::condensed::CondensedReport::default(),
            money_style: cl_core::MoneyStyle::default(),
            bus: crate::data_bus::Subscription::detached(),
            open_task: None,
            lists_stale: false,
            nav: DayNav::default(),
            nav_prompt: None,
            after_save_nav: None,
            save_template: crate::screens::templates::SaveFlow::default(),
        };

        s.bus = s.link.bus().subscribe();
        s.start_load();
        s.poll();
        s
    }

    pub(crate) fn start_load(&mut self) {
        if self.link.is_demo() {
            if let Some(mut demo) = self.link.demo_state() {
                let chart = demo
                    .chart
                    .get_or_insert_with(crate::company::demo::chart)
                    .clone();
                let tags = demo.tags_mut().clone();
                let closing = demo.open.and_then(|open| {
                    demo.companies
                        .iter()
                        .find(|c| c.tenant_id == open)
                        .and_then(|c| c.closed_through)
                });
                drop(demo);
                self.closing = closing;
                self.accounts_task = Some(Task::ready(chart));
                self.tags_task = Some(Task::ready(tags));
            }
        } else if let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) {
            if let Ok(p) = group.read(&session, |s| async move {
                s.accounts().list_all(&AccountFilter::default()).await
            }) {
                self.accounts_task = Some(Task::Pending(p));
            }
            if let Ok(p) = group.read(&session, |s| async move {
                s.tags().list_all(&TagFilter::default()).await
            }) {
                self.tags_task = Some(Task::Pending(p));
            }
            if let Ok(p) = group.read(&session, |s| async move { s.company_settings().await }) {
                self.settings_task = Some(Task::Pending(p));
            }
        }
        self.load_templates();
        self.load_panel();
    }

    /// Runs `call` over the books at once, or on the queue (a write when
    /// `write`, never cancelled).
    pub(crate) fn run(&self, call: Call, write: bool) -> Option<Job> {
        if let Some(b) = &self.books {
            let api = Now(Arc::clone(b));
            return Some(Task::ready(at_once(run(&api, call))));
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return None;
        };
        let job = move |s: Session| async move { Ok(run(&Live(s), call).await) };
        let p = if write {
            let label = cl_api::OrphanLabel {
                what: "orphan.daily_sales_save",
                key: None,
            };
            group.write(&session, label, job)
        } else {
            group.read(&session, job)
        };
        p.ok().map(Task::Pending)
    }

    pub(crate) fn load_templates(&mut self) {
        self.templates_task = self.run(Call::Templates, false);
        if self.templates_task.is_none() {
            self.templates_loaded = true;
        }
    }

    pub(crate) fn load_template(&mut self, id: DailyTemplateId) {
        self.template_task = self.run(Call::Template(id), false);
    }

    pub(crate) fn load_panel(&mut self) {
        self.panel.recent_failed = false;
        self.panel.recent_task = self.run(
            Call::List(DailyEntryQuery {
                limit: Some(RECENT_DAYS),
                ..DailyEntryQuery::default()
            }),
            false,
        );
        self.panel.drafts_task = self.run(
            Call::List(DailyEntryQuery {
                status: Some(cl_api::EntryStatus::Draft),
                limit: Some(50),
                ..DailyEntryQuery::default()
            }),
            false,
        );
    }
}
