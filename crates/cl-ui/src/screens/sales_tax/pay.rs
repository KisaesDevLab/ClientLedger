//! Pay Sales Tax (P23 plan §2.6, §3 WP-3, `vendors.pay_sales_tax`; U-150):
//! the agency, "pay tax owed through", the payment date and account, check
//! (printed later by default, or a typed number) or electronic; the
//! engine's lines of what each item owes, the owner's amounts and
//! adjustments (account and reason); the check total is the engine's.
//! Posting sends each line's `expectedOwed` (a byte copy); 409
//! `SALES_TAX_OWED_CHANGED` previews again with a plain message. One
//! idempotency key per save attempt, reused through the closing-date
//! password. "Match a payment you already made" links an outside check or
//! entry. Past payments are listed with Void.
//!
//! The screen adds nothing up: every figure is the preview's.

use std::collections::BTreeMap;
use std::sync::Arc;

use cl_api::{
    Account, AccountFilter, ApiError, Capabilities, ClosedPeriodAuthorization, IdempotencyKey,
    LinkLine, LinkPaymentRequest, OrphanLabel, PayAdjustment, PayLine, PayPreview,
    PaySalesTaxRefusal, PaySalesTaxRequest, PaySalesTaxResult, PostingOptions, RequestGroup,
    SalesTaxAgency, SalesTaxItem, SalesTaxPayMethod, SalesTaxPayProblemKind, SalesTaxPaymentRecord,
    TransactionFilter,
};
use cl_core::ids::{AccountId, SalesTaxAgencyId, SalesTaxItemId, TransactionId};
use cl_core::{CalendarDate, Money, MoneyStyle, Secret};
use cl_model::forms::sales_tax::pay::{
    LinkForm, PayFigures, PayForm, PayLineFigure, PayMethod, PayPostPlan, PayRequestPlan,
};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::forms::closing_prompt::{ClosingClick, ClosingPrompt, Verb};
use crate::i18n::{t, t_with};
use crate::screens::company::form::{self, DateInput};
use crate::screens::invoice::engine::{self, Job};
use crate::theme::Tokens;
use crate::windows::{OpenPayload, Screen};

/// The command and window kind.
pub const KIND: &str = "vendors.pay_sales_tax";

/// What the closing-date password is for.
#[derive(Clone)]
pub(crate) enum Pending {
    Pay(PayPostPlan),
    Link(LinkPaymentRequest),
    Void(TransactionId),
    /// Undo a link (closing on its date).
    Unlink(TransactionId),
    /// "Mark paid through" (no GL, no closing, no undo).
    MarkPaid(SalesTaxAgencyId, CalendarDate),
}

/// What a write answered.
enum Wrote {
    Paid(PaySalesTaxResult),
    Linked,
    Voided,
    Unlinked,
    MarkedPaid(cl_api::MarkPaidResult),
}

/// One outside payment that can be matched: (id, date, number, STP debit).
type Candidate = (TransactionId, CalendarDate, String, Money);

/// The Pay Sales Tax window.
pub struct PayScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    available: bool,
    today: CalendarDate,
    pub(crate) form: PayForm,
    pub(crate) through: DateInput,
    pub(crate) date: DateInput,
    amounts: BTreeMap<SalesTaxItemId, String>,
    agencies: Vec<SalesTaxAgency>,
    items: Vec<SalesTaxItem>,
    accounts: Vec<Account>,
    history: Vec<SalesTaxPaymentRecord>,
    agencies_job: Option<Job<Vec<SalesTaxAgency>>>,
    items_job: Option<Job<Vec<SalesTaxItem>>>,
    accounts_job: Option<Job<Vec<Account>>>,
    history_job: Option<Job<Vec<SalesTaxPaymentRecord>>>,
    preview_job: Option<(u64, Job<PayPreview>)>,
    write_job: Option<Job<Wrote>>,
    /// The save attempt's key (its revision), reused through the prompt.
    key: Option<(u64, IdempotencyKey)>,
    pub(crate) closing_prompt: ClosingPrompt<Pending>,
    /// The change in flight (for the closing-date password).
    pending_kept: Option<Pending>,
    /// "Match a payment you already made" when open.
    pub(crate) linking: Option<LinkForm>,
    link_texts: BTreeMap<SalesTaxItemId, String>,
    candidates: Vec<Candidate>,
    candidates_job: Option<Job<Vec<Candidate>>>,
    /// The last payment made here.
    pub(crate) paid: Option<PaySalesTaxResult>,
    /// The last "Mark paid through" answer.
    pub(crate) marked: Option<cl_api::MarkPaidResult>,
    /// The "Mark paid through" confirmation is open.
    pub(crate) confirm_mark: bool,
    notice: Option<&'static str>,
    problem: Option<&'static str>,
    error: Option<ApiError>,
    previews: u32,
}

fn fmt(m: Money) -> String {
    m.format(MoneyStyle::default())
}

fn method(m: PayMethod) -> SalesTaxPayMethod {
    match m {
        PayMethod::Check => SalesTaxPayMethod::Check,
        PayMethod::Electronic => SalesTaxPayMethod::Electronic,
    }
}

/// The request the engine takes for a plan (`lines`: the posted lines).
fn request(p: &PayRequestPlan, lines: Option<Vec<PayLine>>) -> PaySalesTaxRequest {
    PaySalesTaxRequest {
        agency_id: p.agency,
        through_date: p.through,
        date: p.date,
        bank_account_id: p.bank,
        method: method(p.method),
        lines: lines.or_else(|| {
            p.amounts.as_ref().map(|a| {
                a.iter()
                    .map(|(i, m)| PayLine {
                        tax_item_id: *i,
                        amount: *m,
                        expected_owed: None,
                    })
                    .collect()
            })
        }),
        adjustments: p
            .adjustments
            .iter()
            .map(|a| PayAdjustment {
                tax_item_id: a.item,
                account_id: a.account,
                amount: a.amount,
                reason: a.reason.clone(),
            })
            .collect(),
        print_later: p.print_later,
        check_number: p.check_number,
        memo: None,
        expected_owed: None,
    }
}

/// The preview as the model's figures (copied, never recomputed).
fn figures_of(p: &PayPreview) -> PayFigures {
    PayFigures {
        lines: p
            .lines
            .iter()
            .map(|l| PayLineFigure {
                item: l.tax_item_id,
                owed: l.owed,
                adjustment: l.adjustment,
                amount: l.amount,
                expected_owed: l.expected_owed,
            })
            .collect(),
        total: p.total,
        owed: p.expected_owed_total(),
        legacy_unassigned: p
            .problems
            .iter()
            .any(|x| x.kind() == SalesTaxPayProblemKind::LegacyUnassigned),
        more_than_owed: p
            .problems
            .iter()
            .any(|x| x.kind() == SalesTaxPayProblemKind::AmountMoreThanOwed),
    }
}

impl PayScreen {
    /// The window over the open company: tax owed through the end of last
    /// month, paid today.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, today: CalendarDate) -> PayScreen {
        let group = link.group().and_then(Result::ok);
        let available = link.is_demo() || link.capabilities().has(Capabilities::SALES_TAX_PAY);
        let (first, _) = super::liability::month_of(today);
        let through = first.add_days(-1).unwrap_or(today);
        let mut s = PayScreen {
            link,
            group,
            available,
            today,
            form: PayForm::new(None, Some(through), Some(today)),
            through: DateInput::new(through),
            date: DateInput::new(today),
            amounts: BTreeMap::new(),
            agencies: Vec::new(),
            items: Vec::new(),
            accounts: Vec::new(),
            history: Vec::new(),
            agencies_job: None,
            items_job: None,
            accounts_job: None,
            history_job: None,
            preview_job: None,
            write_job: None,
            key: None,
            closing_prompt: ClosingPrompt::default(),
            pending_kept: None,
            linking: None,
            link_texts: BTreeMap::new(),
            candidates: Vec::new(),
            candidates_job: None,
            paid: None,
            marked: None,
            confirm_mark: false,
            notice: None,
            problem: None,
            error: None,
            previews: 0,
        };
        s.load();
        s
    }

    fn load(&mut self) {
        if !self.available {
            return;
        }
        self.agencies_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(crate::company::demo::sales_tax::agencies(d, false)),
            |s| async move { s.sales_tax().agencies(false).await },
        ));
        self.items_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(crate::company::demo::sales_tax::items(d, true)),
            |s| async move { s.sales_tax().items(true).await },
        ));
        self.accounts_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| {
                Ok(d.chart
                    .get_or_insert_with(crate::company::demo::chart)
                    .clone())
            },
            |s| async move { s.accounts().list_all(&AccountFilter::default()).await },
        ));
        self.read_history();
        self.poll();
    }

    fn read_history(&mut self) {
        let agency = self.form.agency;
        self.history_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| Ok(crate::company::demo::sales_tax_owed::list(d, agency)),
            move |s| async move {
                s.sales_tax_payments()
                    .list(cl_api::SalesTaxPaymentQuery {
                        agency_id: agency,
                        ..cl_api::SalesTaxPaymentQuery::default()
                    })
                    .await
            },
        ));
    }

    fn busy(&self) -> bool {
        self.agencies_job.is_some()
            || self.items_job.is_some()
            || self.accounts_job.is_some()
            || self.history_job.is_some()
            || self.preview_job.is_some()
            || self.write_job.is_some()
            || self.candidates_job.is_some()
    }

    /// Picks the agency (Pay Sales Tax from Sales Tax Owed opens on it).
    pub(crate) fn set_agency(&mut self, agency: Option<SalesTaxAgencyId>) {
        self.form.set_agency(agency);
        self.amounts.clear();
        self.paid = None;
        self.notice = None;
        self.read_history();
        self.preview();
    }

    /// Asks the engine for the figures of what is on screen.
    pub(crate) fn preview(&mut self) {
        let rev = self.form.revision();
        if self.form.figures().is_some()
            || self.preview_job.as_ref().is_some_and(|(r, _)| *r == rev)
        {
            return;
        }
        let Ok(plan) = self.form.request_plan() else {
            return;
        };
        let r = request(&plan, None);
        let live = r.clone();
        self.preview_job = Some((
            rev,
            engine::read(
                &self.link,
                self.group.as_ref(),
                move |d| crate::company::demo::sales_tax_owed::preview(d, &r),
                move |s| async move { s.sales_tax_payments().preview(&live).await },
            ),
        ));
        self.poll();
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.agencies_job) {
            match r {
                Ok(v) => {
                    self.agencies = v;
                    if self.form.agency.is_none()
                        && let Some(a) = self.agencies.first().map(|a| a.id)
                    {
                        self.set_agency(Some(a));
                    }
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.items_job) {
            self.items = r.unwrap_or_default();
        }
        if let Some(r) = engine::poll(&mut self.accounts_job) {
            self.accounts = r.unwrap_or_default();
            if self.form.bank.is_none() {
                self.form.bank = self.banks().first().map(|(id, _)| *id);
                self.form.changed();
                self.preview();
            }
        }
        if let Some(r) = engine::poll(&mut self.history_job) {
            self.history = r.unwrap_or_default();
        }
        if let Some(r) = engine::poll(&mut self.candidates_job) {
            self.candidates = r.unwrap_or_default();
        }
        if let Some((rev, mut job)) = self.preview_job.take() {
            match job.take() {
                None => self.preview_job = Some((rev, job)),
                Some(Ok(p)) => {
                    if self.form.apply_preview(rev, figures_of(&p)) {
                        self.previews += 1;
                        self.error = None;
                    }
                    self.preview();
                }
                Some(Err(e)) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.write_job) {
            match r {
                Ok(w) => self.wrote(w),
                Err(e) => self.write_failed(e),
            }
        }
    }

    fn banks(&self) -> Vec<(AccountId, String)> {
        self.accounts
            .iter()
            .filter(|a| {
                a.is_active
                    && crate::screens::register::class_of(a)
                        == Some(cl_model::register::AccountClass::Bank)
            })
            .map(|a| (a.id, crate::account_names::shown_name(a)))
            .collect()
    }

    fn item_name(&self, id: Option<SalesTaxItemId>) -> String {
        id.and_then(|id| self.items.iter().find(|i| i.id == id))
            .map_or_else(|| t("sales_tax.pay.unassigned"), |i| i.name.clone())
    }

    fn closing_supported(&self) -> bool {
        self.link.is_demo() || self.link.capabilities().has(Capabilities::CLOSING_PASSWORD)
    }

    // ---- Writes ---------------------------------------------------------------------

    /// Ctrl+S / "Pay": posts the preview's lines.
    pub(crate) fn pay(&mut self) {
        self.notice = None;
        match self.form.post_plan() {
            Ok(plan) => self.send(Pending::Pay(plan), None),
            Err(p) => self.problem = Some(p.key().0),
        }
    }

    fn key_for(&mut self, attempt: u64) -> IdempotencyKey {
        match &self.key {
            Some((a, k)) if *a == attempt => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.key = Some((attempt, k.clone()));
                k
            }
        }
    }

    fn send(&mut self, pending: Pending, password: Option<Secret>) {
        if self.write_job.is_some() {
            return;
        }
        self.problem = None;
        let attempt = match &pending {
            Pending::Pay(p) => p.attempt,
            Pending::Link(_) => u64::MAX - 1,
            Pending::Void(_) => u64::MAX,
            Pending::Unlink(_) => u64::MAX - 2,
            Pending::MarkPaid(..) => u64::MAX - 3,
        };
        let key = self.key_for(attempt);
        let demo_key = key.as_header();
        let label = OrphanLabel {
            what: "orphan.sales_tax_pay",
            key: Some(key.clone()),
        };
        let opts = PostingOptions {
            closed_period: password
                .clone()
                .map(ClosedPeriodAuthorization::with_password),
        };
        let job = match pending.clone() {
            Pending::Pay(plan) => {
                let lines: Vec<PayLine> = plan
                    .lines
                    .iter()
                    .map(|(i, a, e)| PayLine {
                        tax_item_id: *i,
                        amount: *a,
                        expected_owed: Some(*e),
                    })
                    .collect();
                let mut r = request(&plan.request, Some(lines));
                // The preview's owed total, copied (engine PR #219).
                r.expected_owed = Some(plan.expected_owed);
                let live = r.clone();
                engine::write(
                    &self.link,
                    self.group.as_ref(),
                    label,
                    move |d| {
                        crate::company::demo::sales_tax_owed::create(
                            d,
                            &r,
                            password.as_ref(),
                            Some(&demo_key),
                        )
                        .map(Wrote::Paid)
                    },
                    move |s| async move {
                        s.sales_tax_payments()
                            .create(&live, &opts, key)
                            .await
                            .map(Wrote::Paid)
                    },
                )
            }
            Pending::Link(r) => {
                let live = r.clone();
                engine::write(
                    &self.link,
                    self.group.as_ref(),
                    label,
                    move |d| {
                        crate::company::demo::sales_tax_owed::link(
                            d,
                            &r,
                            password.as_ref(),
                            Some(&demo_key),
                        )
                        .map(|()| Wrote::Linked)
                    },
                    move |s| async move {
                        s.sales_tax_payments()
                            .link(&live, &opts, key)
                            .await
                            .map(|()| Wrote::Linked)
                    },
                )
            }
            Pending::Void(id) => {
                let reason = t("sales_tax.pay.void_reason");
                engine::write(
                    &self.link,
                    self.group.as_ref(),
                    label,
                    move |d| {
                        crate::company::demo::sales_tax_owed::void(d, id, password.as_ref())
                            .map(|()| Wrote::Voided)
                    },
                    move |s| async move {
                        s.transactions()
                            .void(id, &reason, &opts, key)
                            .await
                            .map(|_| Wrote::Voided)
                    },
                )
            }
            Pending::Unlink(id) => engine::write(
                &self.link,
                self.group.as_ref(),
                label,
                move |d| {
                    crate::company::demo::sales_tax_owed::unlink(
                        d,
                        id,
                        password.as_ref(),
                        Some(&demo_key),
                    )
                    .map(|_| Wrote::Unlinked)
                },
                move |s| async move {
                    s.sales_tax_payments()
                        .unlink(id, &opts, key)
                        .await
                        .map(|_| Wrote::Unlinked)
                },
            ),
            Pending::MarkPaid(agency, through) => engine::write(
                &self.link,
                self.group.as_ref(),
                label,
                move |d| {
                    crate::company::demo::sales_tax_owed::mark_paid_through(
                        d,
                        agency,
                        through,
                        Some(&demo_key),
                    )
                    .map(Wrote::MarkedPaid)
                },
                move |s| async move {
                    s.sales_tax_payments()
                        .mark_paid_through(agency, through, None, key)
                        .await
                        .map(Wrote::MarkedPaid)
                },
            ),
        };
        self.write_job = Some(job);
        self.pending_kept = Some(pending);
        self.poll();
    }

    fn wrote(&mut self, w: Wrote) {
        self.closing_prompt.saved();
        self.key = None;
        self.pending_kept = None;
        match w {
            Wrote::Paid(r) => {
                self.notice = Some(if r.print_status.as_deref() == Some("queue") {
                    "sales_tax.pay.saved_queued"
                } else {
                    "sales_tax.pay.saved"
                });
                self.paid = Some(r);
                self.amounts.clear();
                let agency = self.form.agency;
                let (bank, through, date) = (self.form.bank, self.form.through, self.form.date);
                self.form = PayForm::new(agency, through, date);
                self.form.bank = bank;
            }
            Wrote::Linked => {
                self.linking = None;
                self.notice = Some("sales_tax.pay.linked");
                self.form.changed();
            }
            Wrote::Voided => {
                self.notice = Some("sales_tax.pay.voided");
                self.paid = None;
                self.form.changed();
            }
            Wrote::Unlinked => {
                self.notice = Some("sales_tax.pay.unlinked");
                self.form.changed();
            }
            Wrote::MarkedPaid(r) => {
                self.notice = Some("sales_tax.pay.marked_paid");
                self.marked = Some(r);
                self.amounts.clear();
                self.form.changed();
            }
        }
        crate::forms::publish_saved(&self.link, &self.link.bus().subscribe(), &[]);
        self.read_history();
        self.preview();
    }

    fn write_failed(&mut self, e: ApiError) {
        let pending = self.pending_kept.take();
        if let Some(p) = pending.clone()
            && self
                .closing_prompt
                .failed(&e, p, self.closing_supported(), None)
        {
            return;
        }
        match cl_api::pay_sales_tax_refusal(&e) {
            Some(PaySalesTaxRefusal::OwedChanged) => {
                // What is owed moved: the figures are previewed again and
                // the owner checks them before paying.
                self.notice = Some("sales_tax.pay.owed_changed");
                self.key = None;
                self.form.changed();
                self.preview();
            }
            Some(r) => {
                self.key = None;
                self.problem = Some(match r {
                    PaySalesTaxRefusal::TotalNotPositive => "pay_sales_tax.problem.nothing_to_pay",
                    PaySalesTaxRefusal::TaxItemNotForAgency => {
                        "sales_tax.pay.refusal.not_for_agency"
                    }
                    PaySalesTaxRefusal::AdjustmentAccountInvalid => {
                        "sales_tax.pay.refusal.adjustment_account"
                    }
                    PaySalesTaxRefusal::CheckNoOrPrintLater => "pay_sales_tax.problem.check_number",
                    PaySalesTaxRefusal::CheckNumberInUse => "sales_tax.pay.refusal.number_in_use",
                    PaySalesTaxRefusal::LinkTotalMismatch => "sales_tax.pay.refusal.link_total",
                    PaySalesTaxRefusal::NotStpPayment => "sales_tax.pay.refusal.not_stp",
                    PaySalesTaxRefusal::AlreadyLinked => "sales_tax.pay.refusal.already_linked",
                    PaySalesTaxRefusal::NotLinked => "sales_tax.pay.refusal.not_linked",
                    PaySalesTaxRefusal::Stale | PaySalesTaxRefusal::OwedChanged => {
                        "sales_tax.pay.refusal.stale"
                    }
                });
            }
            None => self.error = Some(e),
        }
    }

    /// The closing-date dialog's button: send again with the password.
    fn submit_closing(&mut self) {
        let Some((pending, secret)) = self.closing_prompt.submit() else {
            return;
        };
        self.send(pending, Some(secret));
    }

    /// Voids a payment (closing-date password when it is in a closed
    /// period).
    pub(crate) fn void(&mut self, id: TransactionId) {
        self.closing_prompt.set_verb(Verb::Void);
        self.send(Pending::Void(id), None);
    }

    /// Undoes a linked outside payment (closing-date password when it is
    /// in a closed period). Its check stays; what it paid is owed again.
    pub(crate) fn unlink(&mut self, id: TransactionId) {
        self.closing_prompt.set_verb(Verb::Change);
        self.send(Pending::Unlink(id), None);
    }

    /// "Mark paid through…": opens the confirmation (the write can't be
    /// undone).
    pub(crate) fn ask_mark_paid(&mut self) {
        self.problem = None;
        if self.form.agency.is_none() {
            self.problem = Some("pay_sales_tax.problem.no_agency");
        } else if self.form.through.is_none() {
            self.problem = Some("pay_sales_tax.problem.no_date");
        } else {
            self.confirm_mark = true;
        }
    }

    /// The confirmation's "Mark as paid": records what the agency's items
    /// owe through the date as paid outside these books.
    pub(crate) fn confirm_mark_paid(&mut self) {
        self.confirm_mark = false;
        if let (Some(a), Some(d)) = (self.form.agency, self.form.through) {
            self.send(Pending::MarkPaid(a, d), None);
        }
    }

    fn mark_dialog(&mut self, ctx: &egui::Context, tokens: &Tokens) {
        if !self.confirm_mark {
            return;
        }
        let agency = self
            .form
            .agency
            .and_then(|a| self.agencies.iter().find(|x| x.id == a))
            .map(|a| a.vendor_name.clone())
            .unwrap_or_default();
        let date = self
            .form
            .through
            .map(crate::shell::us_date)
            .unwrap_or_default();
        let (mut confirm, mut cancel) = (false, false);
        egui::Window::new(t("sales_tax.pay.mark.title"))
            .id(egui::Id::unique("pay_sales_tax_mark_paid"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(t_with(
                    "sales_tax.pay.mark.body",
                    &[("agency", &agency), ("date", &date)],
                ));
                form::inline_warning(ui, tokens, &t("sales_tax.pay.mark.no_undo"));
                ui.horizontal(|ui| {
                    confirm = ui.button(t("sales_tax.pay.mark.confirm")).clicked();
                    cancel = ui.button(t("sales_tax.pay.mark.cancel")).clicked();
                });
            });
        if confirm {
            self.confirm_mark_paid();
        } else if cancel {
            self.confirm_mark = false;
        }
    }

    /// "Match a payment you already made".
    pub(crate) fn open_link(&mut self) {
        let mut f = LinkForm::default();
        f.agency = self.form.agency;
        self.linking = Some(f);
        self.link_texts.clear();
        self.candidates_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(crate::company::demo::sales_tax_owed::link_candidates(d)),
            |s| async move {
                let accounts = s.accounts().list_all(&AccountFilter::default()).await?;
                let Some(stp) = accounts
                    .iter()
                    .find(|a| a.system_tag.as_deref() == Some("sales_tax_payable"))
                    .map(|a| a.id)
                else {
                    return Ok(Vec::new());
                };
                let page = s
                    .transactions()
                    .list(
                        &TransactionFilter {
                            account_id: Some(stp),
                            ..TransactionFilter::default()
                        },
                        cl_api::PageRequest::first_max(),
                    )
                    .await?;
                Ok(page
                    .data
                    .into_iter()
                    .filter(|x| x.status != cl_api::TxnStatus::Void)
                    .filter_map(|x| {
                        Some((
                            x.id,
                            x.txn_date,
                            x.txn_number.unwrap_or_default(),
                            x.total.filter(|m| m.is_positive())?,
                        ))
                    })
                    .collect())
            },
        ));
        self.poll();
    }

    fn send_link(&mut self) {
        let Some(f) = &self.linking else { return };
        match f.plan() {
            Ok(p) => {
                let r = LinkPaymentRequest {
                    transaction_id: p.payment,
                    agency_id: p.agency,
                    lines: p
                        .lines
                        .iter()
                        .map(|(i, m)| LinkLine {
                            tax_item_id: *i,
                            amount: *m,
                        })
                        .collect(),
                };
                self.closing_prompt.set_verb(Verb::Change);
                self.send(Pending::Link(r), None);
            }
            Err(p) => self.problem = Some(p.key().0),
        }
    }

    // ---- Drawing ----------------------------------------------------------------

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        match self
            .closing_prompt
            .window(ui.ctx(), &tokens, "pay_sales_tax")
        {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => self.closing_prompt.cancel(),
            ClosingClick::None => {}
        }
        ui.label(RichText::new(t("sales_tax.pay.title")).size(22.0).strong());
        if !self.available {
            form::hint(ui, &tokens, &t("sales_tax.pay.needs_update"));
            return;
        }
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        if let Some(n) = self.notice {
            form::hint(ui, &tokens, &t(n));
        }
        if self.agencies.is_empty() && self.agencies_job.is_none() {
            form::hint(ui, &tokens, &t("sales_tax.pay.no_agencies"));
            return;
        }
        self.header(ui);
        ui.add_space(6.0);
        if let Some(f) = self.form.figures().cloned() {
            if f.legacy_unassigned {
                form::inline_warning(ui, &tokens, &t("sales_tax.pay.legacy"));
            }
            if f.more_than_owed {
                form::inline_warning(ui, &tokens, &t("sales_tax.pay.more_than_owed"));
            }
            self.lines(ui, &f);
        } else {
            form::progress_line(ui, &tokens, &t("sales_tax.pay.updating"));
        }
        ui.add_space(6.0);
        self.adjustments_ui(ui);
        if let Some(p) = self.problem {
            form::inline_warning(ui, &tokens, &t(p));
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let enabled = self.write_job.is_none() && !self.closing_prompt.dialog_open();
            if ui
                .add_enabled(enabled, egui::Button::new(t("sales_tax.pay.pay")))
                .on_hover_text(t("sales_tax.pay.pay_tip"))
                .clicked()
            {
                self.pay();
            }
            if ui.button(t("sales_tax.pay.link")).clicked() {
                self.open_link();
            }
            if ui
                .add_enabled(enabled, egui::Button::new(t("sales_tax.pay.mark")))
                .on_hover_text(t("sales_tax.pay.mark_tip"))
                .clicked()
            {
                self.ask_mark_paid();
            }
        });
        self.mark_dialog(ui.ctx(), &tokens);
        if let Some(m) = &self.marked
            && let Some(total) = m.total
        {
            form::hint(
                ui,
                &tokens,
                &t_with(
                    "sales_tax.pay.marked_total",
                    &[
                        ("amount", &fmt(total)),
                        ("date", &crate::shell::us_date(m.through_date)),
                    ],
                ),
            );
        }
        if self.linking.is_some() {
            ui.add_space(8.0);
            self.link_ui(ui, &tokens);
        }
        ui.add_space(10.0);
        self.history_ui(ui, &tokens);
    }

    fn header(&mut self, ui: &mut Ui) {
        let mut changed = false;
        ui.horizontal(|ui| {
            let l = ui.label(t("sales_tax.pay.agency"));
            let name = self
                .form
                .agency
                .and_then(|a| self.agencies.iter().find(|x| x.id == a))
                .map(|a| a.vendor_name.clone())
                .unwrap_or_default();
            let mut picked = None;
            egui::ComboBox::from_id_salt("pay_sales_tax_agency")
                .selected_text(name)
                .show_ui(ui, |ui| {
                    for a in &self.agencies {
                        if ui
                            .selectable_label(self.form.agency == Some(a.id), &a.vendor_name)
                            .clicked()
                        {
                            picked = Some(a.id);
                        }
                    }
                })
                .response
                .labelled_by(l.id);
            if let Some(a) = picked
                && Some(a) != self.form.agency
            {
                self.set_agency(Some(a));
            }
        });
        ui.horizontal(|ui| {
            let a = form::date_field(
                ui,
                &t("sales_tax.pay.through"),
                &mut self.through,
                self.today,
            );
            let b = form::date_field(ui, &t("sales_tax.pay.date"), &mut self.date, self.today);
            if a.changed() || b.changed() {
                self.form.through = self.through.value;
                self.form.date = self.date.value;
                changed = true;
            }
        });
        ui.horizontal(|ui| {
            let l = ui.label(t("sales_tax.pay.bank"));
            let banks = self.banks();
            let name = self
                .form
                .bank
                .and_then(|b| banks.iter().find(|x| x.0 == b))
                .map(|b| b.1.clone())
                .unwrap_or_default();
            let mut picked = None;
            egui::ComboBox::from_id_salt("pay_sales_tax_bank")
                .selected_text(name)
                .show_ui(ui, |ui| {
                    for (id, n) in &banks {
                        if ui
                            .selectable_label(self.form.bank == Some(*id), n)
                            .clicked()
                        {
                            picked = Some(*id);
                        }
                    }
                })
                .response
                .labelled_by(l.id);
            if let Some(b) = picked {
                self.form.bank = Some(b);
                changed = true;
            }
        });
        ui.horizontal(|ui| {
            let mut m = self.form.method;
            ui.radio_value(&mut m, PayMethod::Check, t("sales_tax.pay.check"));
            ui.radio_value(&mut m, PayMethod::Electronic, t("sales_tax.pay.electronic"));
            if m != self.form.method {
                self.form.method = m;
                changed = true;
            }
            if self.form.method == PayMethod::Check {
                let mut later = self.form.print_later;
                if ui
                    .checkbox(&mut later, t("sales_tax.pay.print_later"))
                    .changed()
                {
                    self.form.print_later = later;
                    if later {
                        self.form.check_number.clear();
                    }
                    changed = true;
                }
                if !self.form.print_later {
                    let before = self.form.check_number.clone();
                    form::field(
                        ui,
                        &t("sales_tax.pay.check_number"),
                        &mut self.form.check_number,
                    );
                    changed |= before != self.form.check_number;
                }
            }
        });
        if changed {
            self.form.changed();
            self.preview();
        }
    }

    fn lines(&mut self, ui: &mut Ui, f: &PayFigures) {
        let mut edits: Vec<(SalesTaxItemId, String)> = Vec::new();
        egui::Grid::new("pay_sales_tax_lines")
            .striped(true)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                for h in [
                    "sales_tax.pay.col.item",
                    "sales_tax.pay.col.owed",
                    "sales_tax.pay.col.pay",
                    "sales_tax.pay.col.adjustment",
                    "sales_tax.pay.col.amount",
                ] {
                    ui.label(RichText::new(t(h)).strong().size(12.0));
                }
                ui.end_row();
                for l in &f.lines {
                    let name = self.item_name(l.item);
                    ui.label(&name);
                    ui.label(RichText::new(fmt(l.owed)).monospace());
                    match l.item {
                        Some(item) => {
                            let mut text = self.amounts.get(&item).cloned().unwrap_or_default();
                            let r = ui.add_sized(
                                [110.0, 22.0],
                                egui::TextEdit::singleline(&mut text)
                                    .hint_text(fmt(l.amount))
                                    .font(egui::TextStyle::Monospace),
                            );
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::Role::TextInput,
                                    true,
                                    t_with("sales_tax.pay.amount_for", &[("item", &name)]),
                                )
                            });
                            if r.changed() {
                                edits.push((item, text));
                            }
                        }
                        None => {
                            ui.label("");
                        }
                    }
                    ui.label(RichText::new(fmt(l.adjustment)).monospace());
                    ui.label(RichText::new(fmt(l.amount)).monospace().strong());
                    ui.end_row();
                }
                ui.label(RichText::new(t("sales_tax.pay.total")).strong());
                for _ in 0..3 {
                    ui.label("");
                }
                ui.label(RichText::new(fmt(f.total)).monospace().strong());
                ui.end_row();
            });
        for (item, text) in edits {
            self.form.set_amount(item, &text);
            self.amounts.insert(item, text);
            self.preview();
        }
    }

    fn adjustments_ui(&mut self, ui: &mut Ui) {
        let items: Vec<(SalesTaxItemId, String)> = self
            .form
            .figures()
            .map(|f| {
                f.lines
                    .iter()
                    .filter_map(|l| l.item)
                    .map(|i| (i, self.item_name(Some(i))))
                    .collect()
            })
            .unwrap_or_default();
        let accounts: Vec<(AccountId, String)> = self
            .accounts
            .iter()
            .filter(|a| {
                a.is_active
                    && matches!(
                        a.fields.account_type,
                        cl_api::AccountType::Revenue | cl_api::AccountType::Expense
                    )
            })
            .map(|a| (a.id, crate::account_names::shown_name(a)))
            .collect();
        let mut remove = None;
        let mut changed = false;
        let n = self.form.adjustments().len();
        for i in 0..n {
            let a = self.form.adjustments()[i].clone();
            let mut next = a.clone();
            ui.horizontal(|ui| {
                let l = ui.label(t("sales_tax.pay.adj.item"));
                egui::ComboBox::from_id_salt(("pay_adj_item", i))
                    .selected_text(
                        next.item
                            .and_then(|x| items.iter().find(|(id, _)| *id == x))
                            .map(|x| x.1.clone())
                            .unwrap_or_default(),
                    )
                    .show_ui(ui, |ui| {
                        for (id, name) in &items {
                            ui.selectable_value(&mut next.item, Some(*id), name);
                        }
                    })
                    .response
                    .labelled_by(l.id);
                let l = ui.label(t("sales_tax.pay.adj.account"));
                egui::ComboBox::from_id_salt(("pay_adj_account", i))
                    .selected_text(
                        next.account
                            .and_then(|x| accounts.iter().find(|(id, _)| *id == x))
                            .map(|x| x.1.clone())
                            .unwrap_or_default(),
                    )
                    .show_ui(ui, |ui| {
                        for (id, name) in &accounts {
                            ui.selectable_value(&mut next.account, Some(*id), name);
                        }
                    })
                    .response
                    .labelled_by(l.id);
                form::field(ui, &t("sales_tax.pay.adj.amount"), &mut next.amount)
                    .on_hover_text(t("sales_tax.pay.adj.amount_tip"));
                form::field(ui, &t("sales_tax.pay.adj.reason"), &mut next.reason);
                if ui.button(t("sales_tax.pay.adj.remove")).clicked() {
                    remove = Some(i);
                }
            });
            if next != a {
                self.form.edit_adjustment(i, |x| *x = next);
                changed = true;
            }
        }
        if let Some(i) = remove {
            self.form.remove_adjustment(i);
            changed = true;
        }
        if ui
            .button(t("sales_tax.pay.add_adjustment"))
            .on_hover_text(t("sales_tax.pay.add_adjustment_tip"))
            .clicked()
        {
            self.form.add_adjustment();
        }
        if changed {
            self.preview();
        }
    }

    fn link_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let items: Vec<(SalesTaxItemId, String)> = self
            .form
            .agency
            .map(|a| {
                self.items
                    .iter()
                    .filter(|i| i.agency_id == Some(a))
                    .map(|i| (i.id, i.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let candidates = self.candidates.clone();
        let mut send = false;
        let mut cancel = false;
        let mut texts = self.link_texts.clone();
        if let Some(f) = self.linking.as_mut() {
            form::card(ui, tokens, |ui| {
                ui.label(RichText::new(t("sales_tax.pay.link.title")).strong());
                form::hint(ui, tokens, &t("sales_tax.pay.link.hint"));
                if candidates.is_empty() {
                    form::hint(ui, tokens, &t("sales_tax.pay.link.none"));
                }
                for (id, date, number, amount) in &candidates {
                    let label = format!(
                        "{} {} {}",
                        crate::shell::us_date(*date),
                        number,
                        fmt(*amount)
                    );
                    if ui.radio(f.payment == Some(*id), label).clicked() {
                        f.payment = Some(*id);
                    }
                }
                for (id, name) in &items {
                    let mut text = texts.get(id).cloned().unwrap_or_default();
                    if form::field(
                        ui,
                        &t_with("sales_tax.pay.link.part", &[("item", name)]),
                        &mut text,
                    )
                    .changed()
                    {
                        f.set_amount(*id, &text);
                        texts.insert(*id, text);
                    }
                }
                ui.horizontal(|ui| {
                    send = ui.button(t("sales_tax.pay.link.save")).clicked();
                    cancel = ui.button(t("sales_tax.pay.link.cancel")).clicked();
                });
            });
        }
        self.link_texts = texts;
        if send {
            self.send_link();
        }
        if cancel {
            self.linking = None;
            self.problem = None;
        }
    }

    fn history_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        ui.label(RichText::new(t("sales_tax.pay.history")).strong());
        if self.history.is_empty() {
            form::hint(ui, tokens, &t("sales_tax.pay.history.none"));
            return;
        }
        let mut void = None;
        let mut unlink = None;
        egui::Grid::new("pay_sales_tax_history")
            .striped(true)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                for p in &self.history {
                    ui.label(crate::shell::us_date(p.date));
                    ui.label(RichText::new(fmt(p.total)).monospace());
                    ui.label(if p.linked {
                        t("sales_tax.pay.history.linked")
                    } else if p.print_status.as_deref() == Some("queue") {
                        t("sales_tax.pay.history.queued")
                    } else {
                        p.check_number.map(|n| n.to_string()).unwrap_or_default()
                    });
                    if p.is_void() {
                        ui.label(t("sales_tax.pay.history.void"));
                    } else if p.linked {
                        // A matched outside payment is unlinked, never voided
                        // here (its check is the owner's own).
                        if ui
                            .button(t_with(
                                "sales_tax.pay.unlink_on",
                                &[("date", &crate::shell::us_date(p.date))],
                            ))
                            .on_hover_text(t("sales_tax.pay.unlink_tip"))
                            .clicked()
                        {
                            unlink = Some(p.id);
                        }
                    } else if ui
                        .button(t_with(
                            "sales_tax.pay.void_on",
                            &[("date", &crate::shell::us_date(p.date))],
                        ))
                        .clicked()
                    {
                        void = Some(p.id);
                    }
                    ui.end_row();
                }
            });
        if let Some(id) = void {
            self.void(id);
        }
        if let Some(id) = unlink {
            self.unlink(id);
        }
    }

    fn state_json(&self) -> Value {
        let f = self.form.figures();
        json!({
            "screen": "pay_sales_tax",
            "available": self.available,
            "agency": self.form.agency.is_some(),
            "agencies": self.agencies.len(),
            "method": match self.form.method { PayMethod::Check => "check", PayMethod::Electronic => "electronic" },
            "print_later": self.form.print_later,
            "lines": f.map_or(0, |f| f.lines.len()),
            "total": f.map(|f| f.total.to_wire()),
            "adjustments": self.form.adjustments().len(),
            "previews": self.previews,
            "paid": self.paid.as_ref().map(|p| json!({"queued": p.print_status.as_deref() == Some("queue")})),
            "history": self.history.len(),
            "voided": self.history.iter().filter(|p| p.is_void()).count(),
            "linked": self.history.iter().filter(|p| p.linked).count(),
            "confirm_mark": self.confirm_mark,
            "marked": self.marked.as_ref().map(|m| json!({"lines": m.lines.len()})),
            "linking": self.linking.is_some(),
            "candidates": self.candidates.len(),
            "closing_dialog": self.closing_prompt.dialog_open(),
            "notice": self.notice,
            "problem": self.problem,
            "error": self.error.is_some(),
        })
    }
}

impl Screen for PayScreen {
    fn title(&self) -> String {
        t("sales_tax.pay.title")
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        egui::ScrollArea::vertical().show(ui, |ui| self.draw(ui));
        if self.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        Scope::Form
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(handled) = self.closing_prompt.command(id) {
            return handled;
        }
        match id.as_str() {
            KIND => true,
            "form.save_close" | "form.save_new" => {
                self.pay();
                true
            }
            // Demo only: a few taxed documents to pay (the scenario).
            "dev.demo_sales_tax_story" => {
                let Some(mut d) = self.link.demo_state() else {
                    return false;
                };
                crate::company::demo::sales_tax_owed::story(&mut d);
                drop(d);
                self.form.changed();
                self.preview();
                true
            }
            _ => false,
        }
    }

    fn open_with(&mut self, payload: OpenPayload) -> bool {
        match payload {
            OpenPayload::PaySalesTax(a) => {
                self.set_agency(Some(a));
                true
            }
            _ => false,
        }
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

#[cfg(test)]
#[path = "pay_tests.rs"]
mod tests;
