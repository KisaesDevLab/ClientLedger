//! Review Documents (A04 D6, `vendors.review_documents`): the
//! company-wide queue of documents waiting to be entered. Drafts open in
//! the real forms in draft mode; Accept is the form's Save & Close or Save
//! & New (Q-A04b), so nothing posts from this list.
//!
//! Who builds what:
//! - [`queue`]: the list, its keys and `state.queue` (WP-F1, step 11).
//! - [`review`]: draft → form → convert, Page Up/Down through the queue,
//!   learning, and the control channel's review digests (WP-F2, steps 12
//!   and 14).
//! - [`attach`]: attach to an existing transaction (WP-F2, step 13).
//!
//! Frozen here: [`KIND`], [`make`], the window's [`Scope::List`] while its
//! list has the keyboard (J/K/Enter/Delete, Q-A04c), its
//! [`Screen::keeps_text_key`] (only a focused text field keeps Delete, not
//! a focused row) and [`queue_state`].
//!
//! **Opening a draft (the hand-off to WP-F2).** Enter (or a double click)
//! on a row publishes an [`OpenDraft`] on the egui context
//! ([`publish_open`]) and opens the form's window
//! (`CompanyEvent::OpenWindow(DraftForm::command())`). The form takes it
//! with [`take_open`] on its next frame and enters draft mode. A draft of
//! unknown kind first asks "What is this document?" with no default
//! (Q-A04n).

pub mod attach;
pub mod queue;
pub mod review;

use std::sync::Arc;
use std::time::{Duration, Instant};

use cl_api::{ApiError, DocumentDraft, DocumentDraftId, DraftPage, DuplicateMatch, RequestGroup};
use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::doc_draft::{DraftForm, QUEUE_PAGE};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, Align2, RichText, Sense, Ui, Vec2};
use serde_json::{Value, json};

use crate::company::demo::drafts as demo_drafts;
use crate::company::{CompanyEvent, CompanyLink};
use crate::i18n::{t, t_count, t_with};
use crate::screens::company::form;
use crate::screens::templates::engine::{Job, finished, run};
use crate::theme::Tokens;
use crate::windows::{Screen, Windows};
use queue::{QueueAction, QueueView};

/// The command and window kind.
pub const KIND: &str = "vendors.review_documents";

/// How often an open queue reads the list again (other computers, the
/// inbox's claims and readings).
const REFRESH: Duration = Duration::from_secs(15);

/// The Review Documents window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(DocReviewScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// The queue's hand-off to a draft form: which draft, the form chosen,
/// and the draft's change token as listed. Ids only; no owner data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDraft {
    /// The draft.
    pub id: DocumentDraftId,
    /// The form it opens in.
    pub form: DraftForm,
    /// Its `updated_at` as the queue listed it.
    pub updated_at: String,
}

fn open_id() -> egui::Id {
    egui::Id::unique("a04/review/open_draft")
}

/// Leaves `open` for its form ([`take_open`]).
pub fn publish_open(ctx: &egui::Context, open: OpenDraft) {
    ctx.data_mut(|d| d.insert_temp(open_id(), Some(open)));
}

/// The draft the queue asked `form` to open, once (WP-F2's draft forms
/// call it each frame).
#[must_use]
pub fn take_open(ctx: &egui::Context, form: DraftForm) -> Option<OpenDraft> {
    ctx.data_mut(|d| {
        let slot = d.get_temp_mut_or_default::<Option<OpenDraft>>(open_id());
        if slot
            .as_ref()
            .is_some_and(|o| o.form.command() == form.command())
        {
            slot.take()
        } else {
            None
        }
    })
}

/// A duplicate check out on the engine.
struct DupJob {
    id: DocumentDraftId,
    updated_at: String,
    job: Job<Vec<DuplicateMatch>>,
}

/// The Review Documents window.
pub struct DocReviewScreen {
    /// The open company.
    pub(crate) link: Arc<CompanyLink>,
    /// The company's tenant and name, when known.
    pub(crate) tenant: Option<(TenantId, String)>,
    /// The owner's today.
    #[expect(dead_code, reason = "WP-F2 (step 12) opens drafts with it")]
    pub(crate) today: CalendarDate,
    /// The list.
    pub(crate) queue: QueueView,
    group: Option<RequestGroup>,
    /// The page being read.
    load: Option<Job<DraftPage>>,
    /// The file names of a page's uploads (UI check item 5).
    names: Option<Job<Vec<(cl_api::AttachmentId, String)>>>,
    /// When the list was last asked for.
    loaded_at: Option<Instant>,
    /// The page's first row in the engine's list.
    offset: u32,
    /// `false` once the engine said it keeps no document drafts.
    supported: bool,
    /// The list couldn't be read.
    load_failed: bool,
    dups: Vec<DupJob>,
    /// Delete was pressed on this draft: the confirm card shows.
    confirm: Option<DocumentDraftId>,
    discard: Option<Job<DocumentDraft>>,
    /// A draft of unknown kind waits for the owner's choice of form.
    choose: Option<DocumentDraftId>,
    /// A problem to show (i18n key).
    problem: Option<&'static str>,
    /// Read all was running last frame.
    was_reading: bool,
}

impl std::fmt::Debug for DocReviewScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocReviewScreen")
            .field("queue", &self.queue)
            .finish_non_exhaustive()
    }
}

impl DocReviewScreen {
    /// A queue for the open company; its first page is asked for at once.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> DocReviewScreen {
        let group = link.group().and_then(Result::ok);
        let mut s = DocReviewScreen {
            link,
            tenant,
            today,
            queue: QueueView::default(),
            group,
            load: None,
            names: None,
            loaded_at: None,
            offset: 0,
            supported: true,
            load_failed: false,
            dups: Vec::new(),
            confirm: None,
            discard: None,
            choose: None,
            problem: None,
            was_reading: false,
        };
        s.reload();
        s
    }

    fn tenant_id(&self) -> Option<TenantId> {
        self.tenant.as_ref().map(|(t, _)| *t)
    }

    /// Reads the current page again.
    pub(crate) fn reload(&mut self) {
        if self.load.is_some() {
            return;
        }
        let query = queue::page_query(self.offset);
        let live = query.clone();
        let tenant = self.tenant_id();
        self.loaded_at = Some(Instant::now());
        self.load = Some(run(
            &self.link,
            self.group.as_ref(),
            None,
            move |s| match tenant {
                Some(t) => demo_drafts::list(s, t, &query),
                None => Err(ApiError::NoCompanyOpen),
            },
            move |s| async move { s.document_drafts().list(&live).await },
        ));
        self.poll();
    }

    /// Takes the answers that arrived and starts what is due.
    pub(crate) fn poll(&mut self) {
        if let Some(r) = finished(&mut self.load) {
            match r {
                Ok(page) => {
                    self.supported = true;
                    self.load_failed = false;
                    if page.drafts.is_empty() && self.offset > 0 && page.total > 0 {
                        // The last page emptied: back one page.
                        self.offset = self.offset.saturating_sub(QUEUE_PAGE);
                        self.reload();
                        return;
                    }
                    self.queue.set_page(&page);
                }
                Err(ApiError::Local(cl_api::LocalValidation::Unsupported)) => {
                    self.supported = false;
                    self.queue.set_rows(Vec::new(), 0);
                }
                Err(_) => self.load_failed = true,
            }
        }
        self.poll_names();
        self.poll_dups();
        self.start_dups();
        if let Some(r) = finished(&mut self.discard) {
            match r {
                Ok(_) | Err(ApiError::NotFound { .. }) => self.problem = None,
                Err(ApiError::DocumentDraft(cl_api::DraftRefusal::Closed { .. })) => {
                    self.problem = Some("doc_review.discard.closed");
                }
                Err(_) => self.problem = Some("doc_review.discard.failed"),
            }
            self.reload();
        }
    }

    /// The file names of uploads not known yet (on screen only).
    fn poll_names(&mut self) {
        if let Some(r) = finished(&mut self.names) {
            // Not read: those rows show no name (their date and status
            // still do) and aren't asked for again on every frame.
            let found = r.unwrap_or_else(|_| {
                tracing::debug!("queue file names not read");
                self.queue
                    .names_missing()
                    .into_iter()
                    .map(|a| (a, String::new()))
                    .collect()
            });
            self.queue.names.extend(found);
            return;
        }
        if self.names.is_some() {
            return;
        }
        let missing = self.queue.names_missing();
        if missing.is_empty() {
            return;
        }
        let live = missing.clone();
        self.names = Some(run(
            &self.link,
            self.group.as_ref(),
            None,
            move |s| {
                Ok(missing
                    .iter()
                    .filter_map(|a| {
                        crate::company::demo::checks::find_attachment(s, *a)
                            .map(|f| (*a, f.file_name))
                    })
                    .collect())
            },
            move |s| async move {
                let mut out = Vec::new();
                for a in live {
                    match s.attachments().get(a).await {
                        Ok(f) => out.push((a, f.file_name)),
                        // Gone or unreadable: shown as "(no name)" later.
                        Err(_) => out.push((a, String::new())),
                    }
                }
                Ok(out)
            },
        ));
    }

    fn poll_dups(&mut self) {
        let mut done = Vec::new();
        self.dups.retain_mut(|d| match d.job.take() {
            Some(r) => {
                done.push((d.id, d.updated_at.clone(), queue::duplicate_of(&r)));
                false
            }
            None => true,
        });
        for (id, at, check) in done {
            self.queue.duplicate_done(id, &at, check);
        }
    }

    fn start_dups(&mut self) {
        let Some(tenant) = self.tenant_id() else {
            return;
        };
        for (id, updated_at) in self.queue.next_checks(self.dups.len()) {
            let job = run(
                &self.link,
                self.group.as_ref(),
                None,
                move |s| demo_drafts::duplicates(s, tenant, id),
                move |s| async move { s.document_drafts().duplicates(id).await },
            );
            self.dups.push(DupJob {
                id,
                updated_at,
                job,
            });
        }
    }

    /// Carries out the key or button the list recorded.
    fn act(&mut self, ctx: &egui::Context) {
        let action = self.queue.action.take();
        // While the confirm card or the chooser asks, Enter and Delete
        // belong to its buttons: the list opens and discards nothing.
        if (self.confirm.is_some() || self.choose.is_some())
            && matches!(action, Some(QueueAction::Open(_) | QueueAction::Discard(_)))
        {
            return;
        }
        match action {
            Some(QueueAction::Open(id)) => self.open(ctx, id, None),
            Some(QueueAction::Discard(id)) => {
                self.choose = None;
                self.confirm = Some(id);
            }
            Some(QueueAction::ReadAll) => self.read_all(ctx),
            None => {}
        }
    }

    /// Opens draft `id` in its form (`form` when the owner chose one for
    /// a draft of unknown kind).
    fn open(&mut self, ctx: &egui::Context, id: DocumentDraftId, form: Option<DraftForm>) {
        let Some(row) = self.queue.all_rows().iter().find(|r| r.id == id) else {
            return;
        };
        // A06: a statement opens in Match Vendor Statement, never a form.
        if form.is_none() && row.kind == cl_model::doc_draft::DocKind::Statement {
            self.choose = None;
            self.link.push(CompanyEvent::OpenWith(
                cl_model::CommandId::new(crate::screens::bill::statement_match::KIND),
                Box::new(crate::windows::OpenPayload::StatementDraft(id)),
            ));
            return;
        }
        let Some(form) = form.or_else(|| row.kind.form()) else {
            self.confirm = None;
            self.choose = Some(id);
            return;
        };
        self.choose = None;
        publish_open(
            ctx,
            OpenDraft {
                id,
                form,
                updated_at: row.updated_at.clone(),
            },
        );
        if let Some(cmd) = cl_model::commands::find(form.command()) {
            self.link.push(CompanyEvent::OpenWindow(cmd.id));
        }
    }

    fn read_all(&mut self, ctx: &egui::Context) {
        let started = crate::screens::receipt_inbox::intake::existing(ctx)
            .is_some_and(|h| h.lock().read_all(&self.link));
        self.problem = (!started).then_some("doc_review.read_all.none");
    }

    /// The owner confirmed Discard: the engine discards the draft. The
    /// computer that brought the file in moves it to `Discarded/` when its
    /// inbox next follows the draft (A04 D6).
    fn discard_now(&mut self, id: DocumentDraftId) {
        self.confirm = None;
        let Some(tenant) = self.tenant_id() else {
            return;
        };
        self.discard = Some(run(
            &self.link,
            self.group.as_ref(),
            Some("doc_review.orphan.discard"),
            move |s| demo_drafts::discard(s, tenant, id),
            move |s| async move { s.document_drafts().discard(id).await },
        ));
        self.poll();
    }

    /// Read all's progress, from the inbox service; the list is read again
    /// when a batch ends.
    fn follow_reading(&mut self, ctx: &egui::Context) {
        let progress = crate::screens::receipt_inbox::intake::existing(ctx)
            .and_then(|h| h.lock().read_progress());
        self.queue.reading = progress;
        if self.was_reading && progress.is_none() {
            self.reload();
        }
        self.was_reading = progress.is_some();
    }

    fn toolbar(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.horizontal(|ui| {
            let mut text = self.queue.filter.clone();
            let r = ui.add(egui::Label::new(t("doc_review.filter")).selectable(false));
            let edit = ui
                .add_sized(
                    [220.0, 24.0],
                    egui::TextEdit::singleline(&mut text).hint_text(t("doc_review.filter.hint")),
                )
                .labelled_by(r.id);
            if edit.changed() {
                self.queue.set_filter(&text);
            }
            ui.add_space(12.0);
            if ui
                .button(t("doc_review.read_all"))
                .on_hover_text(t("doc_review.read_all.tip"))
                .clicked()
            {
                self.queue.action = Some(QueueAction::ReadAll);
            }
            if ui.button(t("doc_review.refresh")).clicked() {
                self.reload();
            }
            ui.add_space(12.0);
            let n = usize::try_from(self.queue.total).unwrap_or(usize::MAX);
            ui.label(
                RichText::new(t_count("doc_review.count", n, &[])).color(tokens.text_secondary),
            );
        });
    }

    fn confirm_card(&mut self, ui: &mut Ui, tokens: &Tokens, id: DocumentDraftId) {
        let mut yes = false;
        let mut no = false;
        form::card(ui, tokens, |ui| {
            form::inline_warning(ui, tokens, &t("doc_review.discard.ask"));
            ui.horizontal_wrapped(|ui| {
                yes = ui.button(t("doc_review.discard.yes")).clicked();
                no = ui.button(t("doc_review.discard.no")).clicked();
            });
        });
        if no {
            self.confirm = None;
        } else if yes {
            self.discard_now(id);
        }
    }

    fn chooser(&mut self, ui: &mut Ui, tokens: &Tokens, id: DocumentDraftId) {
        let mut chosen = None;
        let mut cancel = false;
        form::card(ui, tokens, |ui| {
            ui.label(
                RichText::new(t("doc_review.choose.title"))
                    .strong()
                    .color(tokens.text_primary),
            );
            form::hint(ui, tokens, &t("doc_review.choose.hint"));
            ui.horizontal_wrapped(|ui| {
                for f in DraftForm::ALL {
                    if ui.button(t(form_key(f))).clicked() {
                        chosen = Some(f);
                    }
                }
                cancel = ui.button(t("doc_review.choose.cancel")).clicked();
            });
        });
        if cancel {
            self.choose = None;
        } else if let Some(f) = chosen {
            self.open(ui.ctx(), id, Some(f));
        }
    }

    fn list(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        let total = self.queue.total;
        let selected = self.queue.selected();
        let offset = usize::try_from(self.offset).unwrap_or(0);
        let mut clicked = None;
        let mut open = None;
        let rows: Vec<_> = self.queue.rows().into_iter().cloned().collect();
        // Column headers over the cells below (UI check item 5).
        if !rows.is_empty() {
            let width = ui.available_width();
            let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 20.0), Sense::hover());
            let p = ui.painter();
            let font = egui::FontId::proportional(12.0);
            let y = rect.center().y;
            for (x, key) in [
                (8.0, "doc_review.col.arrived"),
                (100.0, "doc_review.col.file"),
                (330.0, "doc_review.col.kind"),
                (440.0, "doc_review.col.date"),
                (540.0, "doc_review.col.reference"),
            ] {
                if rect.left() + x < rect.right() - 220.0 {
                    p.text(
                        egui::pos2(rect.left() + x, y),
                        Align2::LEFT_CENTER,
                        t(key),
                        font.clone(),
                        tokens.text_secondary,
                    );
                }
            }
            p.text(
                egui::pos2(rect.right() - 8.0, y),
                Align2::RIGHT_CENTER,
                t("doc_review.col.status"),
                font.clone(),
                tokens.text_secondary,
            );
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .max_height(ui.available_height() - 40.0)
            .show(ui, |ui| {
                for (i, r) in rows.iter().enumerate() {
                    let view = self.queue.views.get(&r.id).cloned().unwrap_or_default();
                    let is_sel = selected == Some(r.id);
                    let width = ui.available_width();
                    let (rect, resp) =
                        ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::click());
                    let label = queue::row_label(r, offset + i, total);
                    resp.widget_info(|| {
                        egui::WidgetInfo::selected(egui::Role::Button, true, is_sel, &label)
                    });
                    if is_sel {
                        resp.scroll_to_me(None);
                    }
                    if ui.is_rect_visible(rect) {
                        let p = ui.painter();
                        if is_sel {
                            p.rect_filled(rect, 4.0, tokens.accent_tint);
                        } else if resp.hovered() {
                            p.rect_filled(rect, 4.0, tokens.surface_subtle);
                        }
                        let font = egui::FontId::proportional(13.0);
                        let y = rect.center().y;
                        // Status at the right edge, the amount before it,
                        // and the date, kind and reference in what's left
                        // (clipped, so nothing runs over at 200%).
                        let mut status = t(r.status().key().0);
                        if r.duplicate == cl_model::doc_draft::DuplicateCheck::Checking {
                            status = format!("{status} \u{b7} {}", t("doc_review.checking"));
                        }
                        let status_color = match r.status() {
                            cl_model::doc_draft::QueueStatus::Ready => tokens.text_primary,
                            cl_model::doc_draft::QueueStatus::WaitingToBeRead => {
                                tokens.text_secondary
                            }
                            _ => tokens.warn_text,
                        };
                        let status = p.layout_no_wrap(status, font.clone(), status_color);
                        let status_left = rect.right() - 8.0 - status.size().x;
                        p.galley(
                            egui::pos2(status_left, y - status.size().y / 2.0),
                            status,
                            status_color,
                        );
                        let amount = view.total.map(|m| m.format(style)).unwrap_or_default();
                        let amount = p.layout_no_wrap(
                            amount,
                            egui::FontId::monospace(13.0),
                            tokens.text_primary,
                        );
                        let amount_left = status_left - 24.0 - amount.size().x;
                        p.galley(
                            egui::pos2(amount_left, y - amount.size().y / 2.0),
                            amount,
                            tokens.text_primary,
                        );
                        let mut left = rect;
                        left.set_right((amount_left - 12.0).max(rect.left()));
                        let clip = p.with_clip_rect(left.intersect(p.clip_rect()));
                        let at = |x: f32| egui::pos2(rect.left() + x, y);
                        // The day it arrived and its file, so documents
                        // not read yet can be told apart (UI check item 5;
                        // file names on screen only).
                        let arrived = view.arrived.map(crate::shell::us_date).unwrap_or_default();
                        let file = view
                            .file
                            .and_then(|a| self.queue.names.get(&a).cloned())
                            .unwrap_or_default();
                        let date = view.date.map(crate::shell::us_date).unwrap_or_default();
                        let cells = [
                            (8.0, arrived, tokens.text_secondary),
                            (100.0, file, tokens.text_primary),
                            (330.0, queue::kind_words(r.kind), tokens.text_primary),
                            (440.0, date, tokens.text_secondary),
                            (
                                540.0,
                                view.reference.unwrap_or_default(),
                                tokens.text_primary,
                            ),
                        ];
                        for (x, text, color) in cells {
                            clip.text(at(x), Align2::LEFT_CENTER, text, font.clone(), color);
                        }
                    }
                    if resp.double_clicked() {
                        open = Some(r.id);
                    } else if resp.clicked() {
                        clicked = Some(r.id);
                    }
                }
            });
        if let Some(id) = clicked {
            self.queue.select(id);
        }
        if let Some(id) = open {
            self.queue.select(id);
            self.queue.action = Some(QueueAction::Open(id));
        }
    }

    fn pager(&mut self, ui: &mut Ui) {
        if self.queue.total <= u64::from(QUEUE_PAGE) && self.offset == 0 {
            return;
        }
        let last = u64::from(self.offset) + u64::from(QUEUE_PAGE) >= self.queue.total;
        let mut go: Option<u32> = None;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.offset > 0,
                    egui::Button::new(t("doc_review.page.prev")),
                )
                .clicked()
            {
                go = Some(self.offset.saturating_sub(QUEUE_PAGE));
            }
            let first = u64::from(self.offset) + 1;
            let end = (u64::from(self.offset) + u64::from(QUEUE_PAGE)).min(self.queue.total);
            ui.label(t_with(
                "doc_review.page.shown",
                &[
                    ("first", &first.to_string()),
                    ("last", &end.to_string()),
                    ("total", &self.queue.total.to_string()),
                ],
            ));
            if ui
                .add_enabled(!last, egui::Button::new(t("doc_review.page.next")))
                .clicked()
            {
                go = Some(self.offset.saturating_add(QUEUE_PAGE));
            }
        });
        if let Some(o) = go {
            self.offset = o.min(cl_api::draft_limits::MAX_OFFSET);
            self.reload();
        }
    }
}

/// A draft form's name in the "What is this document?" chooser.
#[must_use]
pub fn form_key(f: DraftForm) -> &'static str {
    match f {
        DraftForm::EnterBill => "doc_review.choose.bill",
        DraftForm::CardCharge => "doc_review.choose.card_charge",
        DraftForm::WriteCheck => "doc_review.choose.check",
        DraftForm::CashExpense => "doc_review.choose.cash_expense",
    }
}

impl Screen for DocReviewScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let tokens = Tokens::of(&ctx);
        self.poll();
        self.follow_reading(&ctx);
        if self.loaded_at.is_none_or(|at| at.elapsed() >= REFRESH) {
            self.reload();
        }
        self.act(&ctx);
        if self.load.is_some() || !self.dups.is_empty() || self.discard.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else {
            ctx.request_repaint_after(REFRESH);
        }
        ui.add_space(8.0);
        ui.heading(self.title());
        ui.add_space(6.0);
        if !self.supported {
            form::hint(ui, &tokens, &t("doc_review.unsupported"));
            return;
        }
        self.toolbar(ui, &tokens);
        if let Some(p) = self.queue.reading {
            form::progress_line(
                ui,
                &tokens,
                &t_with(
                    "doc_review.reading",
                    &[
                        ("done", &p.done.to_string()),
                        ("total", &p.total.to_string()),
                    ],
                ),
            );
        }
        if self.load_failed {
            form::inline_warning(ui, &tokens, &t("doc_review.load_failed"));
        }
        if let Some(k) = self.problem {
            form::inline_warning(ui, &tokens, &t(k));
        }
        if let Some(id) = self.confirm {
            self.confirm_card(ui, &tokens, id);
        } else if let Some(id) = self.choose {
            self.chooser(ui, &tokens, id);
        }
        ui.add_space(6.0);
        if self.queue.rows().is_empty() {
            let key = if self.queue.all_rows().is_empty() {
                "doc_review.empty"
            } else {
                "doc_review.no_match"
            };
            ui.label(RichText::new(t(key)).color(tokens.text_secondary));
        } else {
            self.list(ui, &tokens);
        }
        self.pager(ui);
        form::hint(ui, &tokens, &t("doc_review.keys"));
    }

    /// The list's keys while the window has focus; the shell never runs
    /// J/K/Enter while a text field such as the filter has focus (S8).
    fn scope(&self) -> Scope {
        Scope::List
    }

    fn command(&mut self, id: CommandId) -> bool {
        self.queue.command(id)
    }

    fn keeps_text_key(&self, ctx: &egui::Context, s: cl_model::Shortcut) -> bool {
        let _ = s;
        // A focused row is not a text field: Delete there discards
        // (`queue.discard`), as Delete deletes in other lists.
        ctx.text_edit_focused()
    }

    fn state(&self) -> Value {
        let mut q = self.queue.state();
        if let Value::Object(m) = &mut q {
            m.insert("supported".into(), json!(self.supported));
            m.insert("offset".into(), json!(self.offset));
            m.insert("confirm_discard".into(), json!(self.confirm.is_some()));
            m.insert("choose_kind".into(), json!(self.choose.is_some()));
            m.insert("problem".into(), json!(self.problem));
        }
        json!({ "screen": "review_documents", "queue": q })
    }
}

/// `state.queue`: the Review Documents window's queue, whether or not it is
/// in front ([`queue::QueueView::state`]); `null` when it isn't open.
#[must_use]
pub fn queue_state(windows: &Windows) -> Value {
    windows
        .all()
        .iter()
        .find(|w| w.kind.as_str() == KIND)
        .map_or(Value::Null, |w| {
            w.screen
                .state()
                .get("queue")
                .cloned()
                .unwrap_or(Value::Null)
        })
}

#[cfg(test)]
mod tests;
