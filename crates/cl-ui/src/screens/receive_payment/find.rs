//! Find a payment (Q-P22q): no shared Find window exists (Write Check and
//! Enter Bill open a register; Create Invoice walks its saved records), so
//! Receive Payment has a small dialog: client, date range, amount and
//! reference number over `GET /payments` (client and dates go to the
//! engine; the amount and reference number are matched here, an equality
//! check only). A result opens the payment.

use std::sync::Arc;

use cl_api::{ApiError, PaymentStatus, PaymentSummary};
use cl_core::ids::{ContactId, TransactionId};
use cl_core::{CalendarDate, Money, UserInput};
use eframe::egui::{self, RichText, Ui};

use super::engine::{self, Job};
use super::{Dialog, ReceivePaymentScreen, shown};
use crate::i18n::{t, t_with};
use crate::screens::card_charge::kit::{self, NameInput, NameOutcome};
use crate::screens::company::form::{self, DateInput};
use crate::theme::Tokens;

/// The most results listed.
const MAX_RESULTS: usize = 200;

/// The Find dialog's fields and results.
pub(crate) struct FindPayment {
    pub client_input: NameInput,
    pub client: Option<ContactId>,
    pub from: DateInput,
    pub to: DateInput,
    pub amount: String,
    pub ref_no: String,
    pub job: Option<Job<Vec<PaymentSummary>>>,
    /// The last search's matches (`None` before the first search).
    pub results: Option<Vec<PaymentSummary>>,
    /// The amount typed isn't an amount.
    pub amount_problem: bool,
}

impl Default for FindPayment {
    fn default() -> Self {
        let blank = DateInput {
            text: String::new(),
            value: None,
        };
        FindPayment {
            client_input: NameInput::default(),
            client: None,
            from: blank.clone(),
            to: blank,
            amount: String::new(),
            ref_no: String::new(),
            job: None,
            results: None,
            amount_problem: false,
        }
    }
}

/// The rows of `all` that match the typed amount and reference number
/// (equality; the reference number ignores case and spaces at the ends).
pub(crate) fn matching(
    all: Vec<PaymentSummary>,
    amount: Option<Money>,
    ref_no: &str,
) -> Vec<PaymentSummary> {
    let want = ref_no.trim().to_lowercase();
    all.into_iter()
        .filter(|p| amount.is_none_or(|a| p.amount == a))
        .filter(|p| {
            want.is_empty()
                || p.ref_no
                    .as_deref()
                    .is_some_and(|r| r.trim().to_lowercase().contains(&want))
        })
        .take(MAX_RESULTS)
        .collect()
}

impl ReceivePaymentScreen {
    /// Opens the Find dialog (Find, Ctrl+F).
    pub(crate) fn open_find(&mut self) {
        self.dialog = Some(Dialog::Find);
    }

    /// Searches with the dialog's fields.
    pub(crate) fn run_find(&mut self) {
        let f = &mut self.find;
        let amount = if f.amount.trim().is_empty() {
            None
        } else if let Ok(m) = Money::parse_user(f.amount.trim(), UserInput::MONEY) {
            Some(m)
        } else {
            f.amount_problem = true;
            return;
        };
        f.amount_problem = false;
        let client = f.client;
        let (from, to) = (f.from.value, f.to.value);
        let ref_no = f.ref_no.clone();
        let ref2 = ref_no.clone();
        let in_range =
            move |d: CalendarDate| from.is_none_or(|x| d >= x) && to.is_none_or(|x| d <= x);
        f.job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| {
                let all = crate::company::demo::receivables::list(d, client)
                    .into_iter()
                    .filter(|p| in_range(p.date))
                    .rev()
                    .collect();
                Ok(matching(all, amount, &ref2))
            },
            move |s| async move {
                let q = cl_api::PaymentListQuery {
                    contact: client,
                    from,
                    to,
                };
                let mut all = Vec::new();
                let mut offset = 0u64;
                let limit = cl_api::PageRequest::MAX_LIMIT;
                loop {
                    let page = s
                        .customer_payments()
                        .list(&q, cl_api::PageRequest::new(limit, offset)?)
                        .await?;
                    let n = page.data.len();
                    all.extend(page.data);
                    offset += u64::try_from(n).unwrap_or(u64::MAX);
                    if n < usize::from(limit) || offset >= page.total || all.len() >= 5_000 {
                        break;
                    }
                }
                Ok::<_, ApiError>(matching(all, amount, &ref_no))
            },
        ));
        self.poll_find();
    }

    pub(crate) fn poll_find(&mut self) {
        if let Some(r) = engine::poll(&mut self.find.job) {
            match r {
                Ok(v) => self.find.results = Some(v),
                Err(e) => self.error = Some(e),
            }
        }
    }

    /// The Find dialog (a card over the form).
    pub(crate) fn find_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let lists = Arc::clone(&self.books.lists);
        let today = self.today;
        let mut search = false;
        let mut close = false;
        let mut open: Option<TransactionId> = None;
        let names: Vec<(ContactId, String)> = self
            .find
            .results
            .iter()
            .flatten()
            .map(|p| {
                (
                    p.customer_id,
                    self.books.contact_name(p.customer_id).unwrap_or_default(),
                )
            })
            .collect();
        form::card(ui, tokens, |ui| {
            ui.label(RichText::new(t("payment.find.title")).strong());
            crate::forms::header::wrap_row(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                kit::wrap_item(ui, "payment-find/client", |ui| {
                    let f = &mut self.find;
                    let (out, _) = kit::name_field(
                        ui,
                        tokens,
                        &t("payment.find.client"),
                        &mut f.client_input,
                        &lists,
                        f.client,
                        240.0,
                    );
                    match out {
                        NameOutcome::Picked(c) => f.client = Some(c),
                        NameOutcome::Cleared => f.client = None,
                        NameOutcome::None => {}
                    }
                });
                kit::wrap_item(ui, "payment-find/from", |ui| {
                    kit::date_field(
                        ui,
                        tokens,
                        &t("payment.find.from"),
                        &mut self.find.from,
                        today,
                        120.0,
                    );
                });
                kit::wrap_item(ui, "payment-find/to", |ui| {
                    kit::date_field(
                        ui,
                        tokens,
                        &t("payment.find.to"),
                        &mut self.find.to,
                        today,
                        120.0,
                    );
                });
                kit::wrap_item(ui, "payment-find/amount", |ui| {
                    let r = kit::text_field(
                        ui,
                        tokens,
                        &t("payment.find.amount"),
                        &mut self.find.amount,
                        120.0,
                        20,
                    );
                    search |= form::activated(&r);
                });
                kit::wrap_item(ui, "payment-find/ref", |ui| {
                    let r = kit::text_field(
                        ui,
                        tokens,
                        &t("payment.find.ref_no"),
                        &mut self.find.ref_no,
                        120.0,
                        40,
                    );
                    search |= form::activated(&r);
                });
            });
            if self.find.amount_problem {
                ui.label(RichText::new(t("payment.find.amount_problem")).color(tokens.warn_text));
            }
            ui.horizontal(|ui| {
                search |= ui.button(t("payment.find.search")).clicked();
                close = ui.button(t("payment.find.close")).clicked();
            });
            if self.find.job.is_some() {
                form::hint(ui, tokens, &t("payment.find.searching"));
            }
            if let Some(rows) = &self.find.results {
                if rows.is_empty() {
                    form::hint(ui, tokens, &t("payment.find.none"));
                }
                for (p, (_, name)) in rows.iter().zip(&names) {
                    ui.horizontal(|ui| {
                        let mut text = format!(
                            "{} \u{b7} {} \u{b7} {}",
                            crate::shell::us_date(p.date),
                            name,
                            shown(p.amount)
                        );
                        if let Some(r) = p.ref_no.as_deref().filter(|r| !r.is_empty()) {
                            text.push_str(" \u{b7} ");
                            text.push_str(r);
                        }
                        if p.status == PaymentStatus::Void {
                            text.push_str(" \u{b7} ");
                            text.push_str(&t("payment.find.void"));
                        }
                        ui.label(RichText::new(text).text_style(egui::TextStyle::Monospace));
                        let label = t_with(
                            "payment.find.open",
                            &[("date", &crate::shell::us_date(p.date))],
                        );
                        if ui
                            .button(t("payment.find.open_button"))
                            .on_hover_text(label.clone())
                            .clicked()
                        {
                            open = Some(p.id);
                        }
                    });
                }
            }
        });
        if search {
            self.run_find();
        }
        if close {
            self.dialog = None;
        }
        if let Some(id) = open {
            self.dialog = None;
            self.open_id(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(amount: &str, r: Option<&str>) -> PaymentSummary {
        serde_json::from_value(serde_json::json!({
            "id": "00000000-0000-4000-8000-000000000001",
            "customerId": "00000000-0000-4000-8000-000000000002",
            "date": "2026-11-04",
            "amount": amount,
            "refNo": r,
            "status": "posted",
        }))
        .expect("summary")
    }

    #[test]
    fn find_matches_the_amount_and_the_reference_number() {
        let all = vec![
            row("4000.00", Some("10442")),
            row("40.00", Some("10443")),
            row("4000.00", None),
        ];
        let m = Money::parse_wire("4000.00").ok();
        assert_eq!(matching(all.clone(), m, "").len(), 2);
        assert_eq!(matching(all.clone(), m, " 10442 ").len(), 1);
        assert_eq!(matching(all.clone(), None, "1044").len(), 2);
        assert_eq!(matching(all, None, "").len(), 3);
    }
}
