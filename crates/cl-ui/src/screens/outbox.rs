//! The Outbox (P20 WP-B step 4, `clients.outbox`; Q-P20d, Q-P20q, Q-P20s).
//!
//! Two tabs: **Outbox**, the messages stored with the company in the
//! engine, sent only when the owner clicks Send all (Q-P20d); and **Not
//! sent yet**, the invoices never printed, emailed or saved as a PDF
//! (`GET /invoices?sent=false`, Q-P20s). Send all goes one message at a
//! time, makes each PDF from the engine's record, stops at the first
//! "no email server" or sign-in refusal and reports each message. Names
//! and addresses are shown, never logged.

use std::collections::HashMap;
use std::sync::Arc;

use cl_api::{
    ApiError, Capabilities, Contact, ContactFilter, IdempotencyKey, InvoiceListQuery,
    InvoiceRecord, InvoiceRefusal, InvoiceSummary, OutboxId, OutboxItem, PageRequest, RequestGroup,
    Versioned, invoice_refusal,
};
use cl_core::ids::{TenantId, TransactionId};
use cl_core::{CalendarDate, MoneyStyle};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink};
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::invoice::engine::{self, Job, label};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "clients.outbox";

/// The Outbox window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    _today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND).then(|| Box::new(OutboxScreen::new(Arc::clone(link), open)) as Box<_>)
}

/// Reads the Outbox's count again after a change (Email Later, Send all,
/// Remove) and notes it on the link for Home and the status bar: at once
/// in the demo, on the window's queue on a live engine.
pub(crate) fn refresh_count(link: &Arc<CompanyLink>, group: Option<&RequestGroup>) {
    let set = |link: &CompanyLink, n: usize| {
        link.set_outbox_count(Some(u32::try_from(n).unwrap_or(u32::MAX)));
    };
    if let Some(n) = link
        .demo_state()
        .map(|d| crate::company::demo::invoices::outbox(&d).len())
    {
        set(link, n);
        return;
    }
    if !link.capabilities().has(Capabilities::INVOICE_EMAIL) {
        return;
    }
    let (Some(session), Some(group)) = (link.session(), group) else {
        return;
    };
    let target = Arc::clone(link);
    // The answer lands on the link; nothing waits for it.
    let _ = group.read(&session, move |s| async move {
        let n = s.invoices().outbox().await?.len();
        set(&target, n);
        Ok(())
    });
}

/// The tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Outbox,
    NotSent,
}

/// One message's Send all result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Sent,
    Failed(&'static str),
    Skipped,
}

/// The Outbox window.
pub struct OutboxScreen {
    link: Arc<CompanyLink>,
    tenant: Option<(TenantId, String)>,
    group: Option<RequestGroup>,
    tab: Tab,
    items: Vec<OutboxItem>,
    items_job: Option<Job<Vec<OutboxItem>>>,
    unsent: Vec<InvoiceSummary>,
    unsent_job: Option<Job<Vec<InvoiceSummary>>>,
    contacts: Vec<Contact>,
    contacts_job: Option<Job<Vec<Contact>>>,
    /// Layouts and item names for the PDFs (U-130).
    books: crate::screens::invoice::email::PrintBooks,
    books_job: Option<Job<crate::screens::invoice::email::PrintBooks>>,
    /// Send all: the messages still to go, the one in flight, the results.
    queue: Vec<OutboxItem>,
    read_job: Option<(OutboxItem, Job<Versioned<InvoiceRecord>>)>,
    send_job: Option<(OutboxItem, Job<()>)>,
    remove_job: Option<Job<()>>,
    /// One key per message, reused when its send is tried again (the
    /// engine replays it under `invoice-email-v1`: never a second email).
    keys: HashMap<OutboxId, IdempotencyKey>,
    results: Vec<(String, Outcome)>,
    error: Option<ApiError>,
    unavailable: bool,
    bus: crate::data_bus::Subscription,
}

impl OutboxScreen {
    /// The Outbox of the open company.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, tenant: Option<(TenantId, String)>) -> OutboxScreen {
        let group = link.group().and_then(Result::ok);
        let unavailable = !link.is_demo() && !link.capabilities().has(Capabilities::INVOICE_EMAIL);
        let mut s = OutboxScreen {
            bus: link.bus().subscribe(),
            link,
            tenant,
            group,
            tab: Tab::Outbox,
            items: Vec::new(),
            items_job: None,
            unsent: Vec::new(),
            unsent_job: None,
            contacts: Vec::new(),
            contacts_job: None,
            books: crate::screens::invoice::email::PrintBooks::default(),
            books_job: None,
            queue: Vec::new(),
            read_job: None,
            send_job: None,
            remove_job: None,
            keys: HashMap::new(),
            results: Vec::new(),
            error: None,
            unavailable,
        };
        s.refresh();
        s
    }

    fn refresh(&mut self) {
        if self.unavailable {
            return;
        }
        self.items_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(crate::company::demo::invoices::outbox(d)),
            |s| async move { s.invoices().outbox().await },
        ));
        self.unsent_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(demo_unsent(d)),
            |s| async move {
                let q = InvoiceListQuery {
                    sent: Some(false),
                    ..InvoiceListQuery::default()
                };
                Ok(s.invoices().list(&q, PageRequest::first_max()).await?.data)
            },
        ));
        self.books_job = Some(crate::screens::invoice::email::print_books_job(
            &self.link,
            self.group.as_ref(),
        ));
        if self.contacts.is_empty() {
            self.contacts_job = Some(engine::read(
                &self.link,
                self.group.as_ref(),
                |d| Ok(d.contacts_mut().clone()),
                |s| async move { s.contacts().list_all(&ContactFilter::default()).await },
            ));
        }
        self.poll();
    }

    fn busy(&self) -> bool {
        self.items_job.is_some()
            || self.unsent_job.is_some()
            || self.contacts_job.is_some()
            || self.sending()
    }

    fn sending(&self) -> bool {
        self.read_job.is_some() || self.send_job.is_some() || !self.queue.is_empty()
    }

    fn poll(&mut self) {
        if let Some(r) = engine::poll(&mut self.items_job) {
            match r {
                Ok(v) => {
                    self.link
                        .set_outbox_count(Some(u32::try_from(v.len()).unwrap_or(u32::MAX)));
                    self.items = v;
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.unsent_job) {
            self.unsent = r.unwrap_or_default();
        }
        if let Some(r) = engine::poll(&mut self.contacts_job) {
            self.contacts = r.unwrap_or_default();
        }
        if let Some(Ok(b)) = engine::poll(&mut self.books_job) {
            self.books = b;
        }
        if let Some(Err(e)) = engine::poll(&mut self.remove_job) {
            self.error = Some(e);
        }
        if let Some((item, mut job)) = self.read_job.take() {
            match job.take() {
                None => self.read_job = Some((item, job)),
                Some(Ok(v)) => self.send_one(item, &v.record),
                Some(Err(e)) => self.failed(&item, &e),
            }
        }
        if let Some((item, mut job)) = self.send_job.take() {
            match job.take() {
                None => self.send_job = Some((item, job)),
                Some(Ok(())) => {
                    self.keys.remove(&item.id);
                    self.results.push((number(&item), Outcome::Sent));
                    self.next();
                }
                Some(Err(e)) => self.failed(&item, &e),
            }
        }
    }

    /// Send all (Q-P20d): one message at a time.
    fn send_all(&mut self) {
        if self.sending() || self.items.is_empty() {
            return;
        }
        self.results.clear();
        self.error = None;
        self.queue = self.items.iter().rev().cloned().collect();
        self.next();
    }

    fn next(&mut self) {
        let Some(item) = self.queue.pop() else {
            crate::forms::publish_saved(&self.link, &self.bus, &[]);
            self.refresh();
            return;
        };
        let id = item.transaction_id;
        let job = engine::read(
            &self.link,
            self.group.as_ref(),
            |d| crate::company::demo::invoices::get(d, id).ok_or(ApiError::NotFound { code: None }),
            move |s| async move { s.invoices().get(id).await },
        );
        self.read_job = Some((item, job));
        if self.link.is_demo() {
            self.poll();
        }
    }

    fn send_one(&mut self, item: OutboxItem, r: &InvoiceRecord) {
        let client = r
            .txn
            .contact_id
            .and_then(|c| self.contacts.iter().find(|x| x.id == c));
        let terms = crate::screens::invoice::email::terms_text(r.payment_terms.as_deref());
        let data = crate::screens::invoice::email::print_data(r, client, &terms, &self.books);
        let style = crate::screens::invoice::email::pdf_style_for(
            self.tenant.as_ref().map_or("", |(_, n)| n.as_str()),
            self.books.layout_of(r),
        );
        let msg = item.message();
        let (id, outbox_id) = (item.transaction_id, item.id);
        // The same message tried again keeps its key (Q-P20 plan §5).
        let key = self.keys.entry(outbox_id).or_default().clone();
        let (demo_data, demo_style, m2) = (data.clone(), style.clone(), msg.clone());
        // The PDF is made on the queue, off the window's frame (S10); the
        // engine takes the Outbox message out in the same commit
        // (`outboxId`).
        let job = engine::write(
            &self.link,
            self.group.as_ref(),
            label("orphan.invoice_email", Some(key.clone())),
            |d| {
                crate::screens::invoice::email::render_pdf(&demo_data, &demo_style)?;
                crate::company::demo::invoices::email(d, id, &m2, Some(outbox_id))?;
                Ok(())
            },
            move |s| async move {
                let (name, pdf) = crate::screens::invoice::email::render_pdf(&data, &style)?;
                s.invoices()
                    .email(id, &msg, &name, &pdf, Some(outbox_id), key)
                    .await?;
                Ok(())
            },
        );
        self.send_job = Some((item, job));
        if self.link.is_demo() {
            self.poll();
        }
    }

    /// A message that didn't go: reported; an email-server, offline or
    /// sign-in refusal stops the rest.
    fn failed(&mut self, item: &OutboxItem, e: &ApiError) {
        let refusal = invoice_refusal(e);
        if !crate::screens::register::keeps_key(e) && refusal != Some(InvoiceRefusal::Retry) {
            self.keys.remove(&item.id);
        }
        let stop = refusal.is_some_and(InvoiceRefusal::stops_sending)
            || matches!(
                e,
                ApiError::Unauthorized { .. } | ApiError::Forbidden { .. }
            );
        let key = crate::screens::invoice::email::send_problem(refusal);
        self.results.push((number(item), Outcome::Failed(key)));
        if stop {
            for rest in std::mem::take(&mut self.queue) {
                self.results.push((number(&rest), Outcome::Skipped));
            }
            self.refresh();
        } else {
            self.next();
        }
    }

    fn remove(&mut self, id: cl_api::OutboxId) {
        self.remove_job = Some(engine::write(
            &self.link,
            self.group.as_ref(),
            label("orphan.outbox_remove", None),
            |d| match crate::company::demo::invoices::remove_from_outbox(d, id) {
                Ok(()) | Err(ApiError::NotFound { .. }) => Ok(()),
                Err(e) => Err(e),
            },
            move |s| async move { s.invoices().remove_from_outbox(id).await },
        ));
        self.items.retain(|i| i.id != id);
        self.keys.remove(&id);
        self.link
            .set_outbox_count(Some(u32::try_from(self.items.len()).unwrap_or(u32::MAX)));
        self.poll();
    }

    fn draw(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        ui.label(RichText::new(t("cmd.clients.outbox")).size(22.0).strong());
        if self.unavailable {
            form::hint(ui, &tokens, &t("outbox.unavailable"));
            return;
        }
        ui.horizontal(|ui| {
            let n = self.items.len().to_string();
            let m = self.unsent.len().to_string();
            if ui
                .selectable_label(
                    self.tab == Tab::Outbox,
                    t_with("outbox.tab.outbox", &[("n", &n)]),
                )
                .clicked()
            {
                self.tab = Tab::Outbox;
            }
            if ui
                .selectable_label(
                    self.tab == Tab::NotSent,
                    t_with("outbox.tab.not_sent", &[("n", &m)]),
                )
                .clicked()
            {
                self.tab = Tab::NotSent;
            }
        });
        ui.separator();
        if let Some(e) = &self.error {
            form::inline_error(ui, &tokens, e);
        }
        match self.tab {
            Tab::Outbox => self.outbox_ui(ui, &tokens),
            Tab::NotSent => self.unsent_ui(ui, &tokens),
        }
    }

    fn outbox_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::hint(ui, tokens, &t("outbox.explain"));
        let sending = self.sending();
        if ui
            .add_enabled(
                !sending && !self.items.is_empty(),
                egui::Button::new(t("outbox.send_all")),
            )
            .on_hover_text(t("outbox.send_all.tip"))
            .clicked()
        {
            self.send_all();
        }
        if sending {
            form::progress_line(ui, tokens, &t("outbox.sending"));
        }
        for (n, r) in &self.results {
            let text = match r {
                Outcome::Sent => t_with("outbox.result.sent", &[("number", n)]),
                Outcome::Failed(k) => format!(
                    "{} {}",
                    t_with("outbox.result.failed", &[("number", n)]),
                    t(k)
                ),
                Outcome::Skipped => t_with("outbox.result.skipped", &[("number", n)]),
            };
            ui.label(text);
        }
        if self.items.is_empty() {
            ui.label(RichText::new(t("outbox.empty")).color(tokens.text_secondary));
            return;
        }
        let mut remove = None;
        egui::Grid::new("outbox/items")
            .striped(true)
            .show(ui, |ui| {
                ui.strong(t("outbox.col.invoice"));
                ui.strong(t("outbox.col.to"));
                ui.strong(t("outbox.col.subject"));
                ui.label("");
                ui.end_row();
                for i in &self.items {
                    ui.label(number(i));
                    ui.label(i.to.join(", "));
                    ui.label(&i.subject);
                    if ui
                        .add_enabled(!sending, egui::Button::new(t("outbox.remove")))
                        .clicked()
                    {
                        remove = Some(i.id);
                    }
                    ui.end_row();
                }
            });
        if let Some(id) = remove {
            self.remove(id);
        }
    }

    fn unsent_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::hint(ui, tokens, &t("outbox.not_sent.explain"));
        if self.unsent.is_empty() {
            ui.label(RichText::new(t("outbox.not_sent.empty")).color(tokens.text_secondary));
            return;
        }
        let style = MoneyStyle::default();
        let mut open = None;
        egui::Grid::new("outbox/unsent")
            .striped(true)
            .show(ui, |ui| {
                ui.strong(t("outbox.col.invoice"));
                ui.strong(t("outbox.col.date"));
                ui.strong(t("outbox.col.client"));
                ui.strong(t("outbox.col.total"));
                ui.end_row();
                for i in &self.unsent {
                    let n = i.txn_number.clone().unwrap_or_default();
                    if ui.link(t_with("outbox.open", &[("number", &n)])).clicked() {
                        open = Some(i.id);
                    }
                    ui.label(crate::shell::us_date(i.txn_date));
                    ui.label(i.contact_name.clone().unwrap_or_default());
                    ui.label(
                        RichText::new(i.total.map(|m| m.format(style)).unwrap_or_default())
                            .monospace(),
                    );
                    ui.end_row();
                }
            });
        if let Some(id) = open {
            self.link.push(CompanyEvent::OpenRecord(
                CommandId::new(crate::screens::invoice::KIND),
                id,
            ));
        }
    }

    fn state_json(&self) -> Value {
        json!({
            "screen": "outbox",
            "tab": match self.tab { Tab::Outbox => "outbox", Tab::NotSent => "not_sent" },
            "unavailable": self.unavailable,
            "items": self.items.len(),
            "not_sent": self.unsent.len(),
            "sending": self.sending(),
            "results": self.results.iter().map(|(_, r)| match r {
                Outcome::Sent => "sent",
                Outcome::Failed(_) => "failed",
                Outcome::Skipped => "skipped",
            }).collect::<Vec<_>>(),
        })
    }
}

fn number(i: &OutboxItem) -> String {
    i.txn_number.clone().unwrap_or_default()
}

/// The demo's never-sent invoices as list rows.
fn demo_unsent(d: &mut crate::company::demo::DemoState) -> Vec<InvoiceSummary> {
    let ids: Vec<TransactionId> = crate::company::demo::invoices::unsent(d);
    let contacts = d.contacts_mut().clone();
    ids.into_iter()
        .filter_map(|id| crate::company::demo::invoices::get(d, id))
        .map(|v| {
            let r = v.record;
            InvoiceSummary {
                id: r.txn.id,
                txn_number: r.txn.txn_number.clone(),
                txn_date: r.txn.txn_date,
                due_date: r.due_date,
                contact_id: r.txn.contact_id,
                contact_name: r
                    .txn
                    .contact_id
                    .and_then(|c| contacts.iter().find(|x| x.id == c))
                    .map(|c| c.fields.display_name.clone()),
                total: r.txn.total,
                balance_due: r.balance_due,
                invoice_status: r.invoice_status,
                status: r.txn.status,
            }
        })
        .collect()
}

impl Screen for OutboxScreen {
    fn title(&self) -> String {
        t("cmd.clients.outbox")
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        let changes = self.link.bus().take(&mut self.bus);
        if changes.ledger_any() && !self.busy() {
            self.refresh();
        }
        egui::ScrollArea::vertical().show(ui, |ui| self.draw(ui));
        if self.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        Scope::Global
    }

    fn command(&mut self, id: CommandId) -> bool {
        id.as_str() == KIND
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_outbox_lists_queued_mail_and_sends_all() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let mut s = OutboxScreen::new(Arc::clone(&link), None);
        assert_eq!(s.state_json()["items"], 0);
        assert!(!s.unavailable);
        for k in [
            "outbox.tab.outbox",
            "outbox.tab.not_sent",
            "outbox.send_all",
            "outbox.empty",
        ] {
            assert!(crate::i18n::has(k), "{k}");
        }
        s.send_all();
        assert!(!s.sending(), "nothing to send");
    }
}
