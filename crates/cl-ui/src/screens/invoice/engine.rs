//! Who answers Create Invoice: the live engine over the window's request
//! group (reads and writes on the queue, never on the egui thread) or the
//! `--demo` engine (`company::demo::invoices`, answered at once). Each call
//! returns a [`Job`] the screen polls each frame.
//!
//! Also the one place the API's and the form model's types meet: the
//! checked [`InvoiceRequest`] becomes a `cl_api::NewInvoice` field by field
//! and the engine's preview becomes the model's [`PreviewFigures`]. Nothing
//! is computed here.

use std::future::Future;

use cl_api::{
    ApiError, InvoicePreview, LocalValidation, NewInvoice, OrphanLabel, PriceSource, RequestGroup,
    Session, SignedPercent,
};
use cl_model::forms::invoice::{
    InvoiceRequest, LayoutPick, PreviewFigures, PreviewLineFigures, PreviewTotals, RateSource,
};

use crate::company::demo::DemoState;
use crate::company::{CompanyLink, Task};

/// A call in flight (or answered).
pub(crate) enum Job<T> {
    /// On the queue (or a demo answer).
    Task(Task<Result<T, ApiError>>),
    /// Answered at once.
    Done(Option<Result<T, ApiError>>),
}

impl<T> Job<T> {
    /// The answer once; `None` while running.
    pub(crate) fn take(&mut self) -> Option<Result<T, ApiError>> {
        match self {
            Job::Task(t) => t.poll_result(),
            Job::Done(v) => v.take(),
        }
    }
}

impl<T> std::fmt::Debug for Job<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Job")
    }
}

/// Polls `slot`: the answer once, the slot emptied.
pub(crate) fn poll<T>(slot: &mut Option<Job<T>>) -> Option<Result<T, ApiError>> {
    let r = slot.as_mut()?.take()?;
    *slot = None;
    Some(r)
}

/// A read: the demo answers at once, the live engine on the queue.
pub(crate) fn read<T, F, Fut>(
    link: &CompanyLink,
    group: Option<&RequestGroup>,
    demo: impl FnOnce(&mut DemoState) -> Result<T, ApiError>,
    live: F,
) -> Job<T>
where
    T: Send + 'static,
    F: FnOnce(Session) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
{
    if let Some(mut d) = link.demo_state() {
        return Job::Done(Some(demo(&mut d)));
    }
    let (Some(session), Some(group)) = (link.session(), group) else {
        return Job::Done(Some(Err(ApiError::NoCompanyOpen)));
    };
    match group.read(&session, move |s| async move { live(s).await.map(Ok) }) {
        Ok(p) => Job::Task(Task::Pending(p)),
        Err(_) => Job::Done(Some(Err(ApiError::Local(LocalValidation::InvalidValue)))),
    }
}

/// A write (labelled for the orphan log; never names a value).
pub(crate) fn write<T, F, Fut>(
    link: &CompanyLink,
    group: Option<&RequestGroup>,
    label: OrphanLabel,
    demo: impl FnOnce(&mut DemoState) -> Result<T, ApiError>,
    live: F,
) -> Job<T>
where
    T: Send + 'static,
    F: FnOnce(Session) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
{
    if let Some(mut d) = link.demo_state() {
        return Job::Done(Some(demo(&mut d)));
    }
    let (Some(session), Some(group)) = (link.session(), group) else {
        return Job::Done(Some(Err(ApiError::NoCompanyOpen)));
    };
    match group.write(
        &session,
        label,
        move |s| async move { live(s).await.map(Ok) },
    ) {
        Ok(p) => Job::Task(Task::Pending(p)),
        Err(_) => Job::Done(Some(Err(ApiError::Local(LocalValidation::InvalidValue)))),
    }
}

/// A percent the model holds as a `Quantity` (markup, discount) as the
/// API's exact percent.
fn percent(q: Option<cl_core::Quantity>) -> Result<Option<SignedPercent>, ApiError> {
    q.map(|q| {
        let w = q.to_wire();
        let t = if w.contains('.') {
            w.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            w
        };
        SignedPercent::parse(&t)
    })
    .transpose()
    .map_err(ApiError::Local)
}

/// The checked request as the engine's invoice, field by field (the tax
/// rate is never sent; a billable line sends no item, quantity or price).
///
/// # Errors
/// `Local(InvalidValue)` for a markup the API can't carry.
pub(crate) fn new_invoice(r: &InvoiceRequest) -> Result<NewInvoice, ApiError> {
    let mut lines = Vec::with_capacity(r.lines.len());
    for l in &r.lines {
        let markup = percent(l.markup_percent)?;
        lines.push(cl_api::InvoiceLine {
            account_id: l.account,
            item_id: l.item,
            description: l.description.clone(),
            service_date: l.service_date,
            quantity: l.quantity,
            unit_price: l.unit_price,
            discount_percent: percent(l.discount_percent)?,
            is_taxable: l.taxable,
            tax_rate: None,
            tag_id: l.tag,
            billable_source_line_id: l.billable_source_line,
            markup_percent: markup,
        });
    }
    let mut n = NewInvoice::new(r.client, r.date, lines);
    n.due_date = r.due_date;
    n.payment_terms.clone_from(&r.terms);
    n.terms_days = r.terms_days.map(u32::from);
    n.memo = Some(r.memo.clone());
    n.customer_message.clone_from(&r.customer_message);
    n.po_number.clone_from(&r.po_number);
    n.tag_ids.clone_from(&r.header_tags);
    n.hide_zero_lines = r.hide_zero_lines;
    n.txn_number.clone_from(&r.number);
    n.print_later = Some(r.print_later);
    n.layout_id = match &r.layout {
        LayoutPick::Unset => cl_api::LayoutChoice::Omitted,
        LayoutPick::CompanyDefault => cl_api::LayoutChoice::CompanyDefault,
        LayoutPick::Layout(id) => cl_api::InvoiceLayoutId::parse(id)
            .map(cl_api::LayoutChoice::Layout)
            .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?,
    };
    Ok(n)
}

/// Where the engine says a rate came from.
#[must_use]
pub(crate) fn source_of(s: Option<PriceSource>) -> RateSource {
    match s {
        Some(PriceSource::Input) => RateSource::Typed,
        Some(PriceSource::Item) => RateSource::Item,
        Some(PriceSource::PriceLevel) => RateSource::PriceLevel,
        Some(PriceSource::PriceLevelPercent) => RateSource::PriceLevelPercent,
        Some(PriceSource::Unpriced) => RateSource::Unpriced,
        Some(PriceSource::BillableCost) => RateSource::Billable,
        Some(PriceSource::Other) | None => RateSource::Other,
    }
}

/// The engine's preview as the form model's figures (copied, never
/// recomputed).
#[must_use]
pub(crate) fn figures_of(p: &InvoicePreview) -> PreviewFigures {
    PreviewFigures {
        lines: p
            .lines
            .iter()
            .map(|l| PreviewLineFigures {
                quantity: l.quantity,
                unit_price: l.unit_price,
                source: source_of(l.unit_price_source),
                amount: l.amount,
                tax_amount: l.tax_amount,
                display_rate: l.display_rate,
                billed_elsewhere: l.error.is_some(),
            })
            .collect(),
        billed_on: p
            .lines
            .iter()
            .filter_map(|l| l.error.as_ref())
            .map(|e| e.invoice_number.clone().unwrap_or_default())
            .collect(),
        totals: PreviewTotals {
            subtotal: p.subtotal,
            tax_total: p.tax_total,
            tax_rate: p.tax_rate,
            total: p.total,
            due_date: p.due_date,
            balance_after: p.contact_balance_after,
        },
    }
}

/// The orphan-log label of a write.
pub(crate) fn label(what: &'static str, key: Option<cl_api::IdempotencyKey>) -> OrphanLabel {
    OrphanLabel { what, key }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::ids::ContactId;
    use cl_core::{CalendarDate, Money, Quantity};
    use cl_model::forms::invoice::RequestLine;
    use cl_model::rowkey::RowKeyGen;

    #[test]
    fn the_request_maps_field_by_field_and_never_sends_a_tax_rate() {
        let client = ContactId::parse("d0000000-0000-4000-8000-c00000000001").expect("id");
        let date = CalendarDate::from_ymd(2026, 11, 4).expect("date");
        let r = InvoiceRequest {
            client,
            date,
            due_date: None,
            terms: Some("custom".into()),
            terms_days: Some(12),
            number: Some("1097".into()),
            memo: String::new(),
            po_number: Some(String::new()),
            customer_message: Some("Thanks".into()),
            header_tags: Some(Vec::new()),
            hide_zero_lines: Some(true),
            print_later: false,
            layout: LayoutPick::Unset,
            lines: vec![RequestLine {
                key: RowKeyGen::new().next_key(),
                account: None,
                item: None,
                description: Some("x".into()),
                service_date: None,
                quantity: Some(Quantity::parse_wire("18").expect("q")),
                unit_price: Some(Money::parse_wire("65").expect("m")),
                discount_percent: None,
                taxable: Some(false),
                tag: None,
                billable_source_line: None,
                markup_percent: Some(Quantity::parse_wire("10").expect("q")),
            }],
        };
        let n = new_invoice(&r).expect("maps");
        assert_eq!(n.terms_days, Some(12));
        assert_eq!(n.txn_number.as_deref(), Some("1097"));
        assert_eq!(n.lines[0].tax_rate, None);
        assert_eq!(
            n.lines[0]
                .markup_percent
                .map(SignedPercent::to_wire)
                .as_deref(),
            Some("10")
        );
        assert_eq!(n.memo.as_deref(), Some(""));
    }

    #[test]
    fn sources_map_and_unknowns_are_other() {
        assert_eq!(source_of(Some(PriceSource::Input)), RateSource::Typed);
        assert_eq!(
            source_of(Some(PriceSource::PriceLevelPercent)),
            RateSource::PriceLevelPercent
        );
        assert_eq!(
            source_of(Some(PriceSource::BillableCost)),
            RateSource::Billable
        );
        assert_eq!(source_of(None), RateSource::Other);
    }

    #[test]
    fn stored_percents_reach_the_wire_exactly() {
        let q = |s: &str| Some(Quantity::parse_wire(s).expect("q"));
        assert_eq!(
            percent(q("10.0000"))
                .expect("ok")
                .map(SignedPercent::to_wire),
            Some("10".to_string())
        );
        assert_eq!(
            percent(q("12.5")).expect("ok").map(SignedPercent::to_wire),
            Some("12.5".to_string())
        );
        assert_eq!(percent(None).expect("ok"), None);
    }
}
