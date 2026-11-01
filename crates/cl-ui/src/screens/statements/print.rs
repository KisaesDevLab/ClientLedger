//! An issued statement's snapshot (or a computed statement) as its
//! printed form (`cl_print::forms::statement`), and its PDF. Every figure
//! is the engine's string, parsed for display only; nothing is added up
//! here. Used by the Statements screen and the Outbox (statement rows).

use cl_api::{ApiError, CustomerStatement, IssuedStatementRecord, StatementFormat};
use cl_print::forms::statement::{self as form, Aging, StatementData, StatementLine};

use crate::i18n::t;

/// The words for a statement line's transaction type.
pub(crate) fn txn_word(txn_type: &str) -> String {
    let key = format!("statements.txn.{txn_type}");
    if crate::i18n::has(&key) {
        t(&key)
    } else {
        txn_type.replace('_', " ")
    }
}

/// The printed form's data of `s` (`number`: an issued statement's).
pub(crate) fn data_of(s: &CustomerStatement, number: &str, message: &str) -> StatementData {
    let date = crate::shell::us_date;
    let format = match s.format {
        StatementFormat::OpenItem => form::StatementFormat::OpenItem,
        StatementFormat::BalanceForward => form::StatementFormat::BalanceForward,
        StatementFormat::Transaction => form::StatementFormat::Transaction,
    };
    let b = &s.aging.buckets;
    StatementData {
        format,
        number: number.to_string(),
        date: date(s.as_of),
        customer: std::iter::once(s.contact.name.clone())
            .chain(s.contact.address.iter().cloned())
            .collect(),
        balance_forward: s
            .opening_balance
            .map(|m| (s.start.map(date).unwrap_or_default(), m.as_decimal())),
        lines: s
            .lines
            .iter()
            .map(|l| {
                let number = l.number.clone().unwrap_or_default();
                let word = txn_word(&l.txn_type);
                StatementLine {
                    date: date(l.date),
                    description: l
                        .description
                        .clone()
                        .filter(|d| !d.is_empty())
                        .unwrap_or_else(|| {
                            if number.is_empty() {
                                word.clone()
                            } else {
                                format!("{word} {number}")
                            }
                        }),
                    number,
                    due_date: l.due_date.map(date).unwrap_or_default(),
                    amount: l.amount.as_decimal(),
                    open: l.open.map(cl_core::Money::as_decimal),
                    balance: l.balance.as_decimal(),
                }
            })
            .collect(),
        amount_due: s.amount_due.as_decimal(),
        aging: Some(Aging {
            current: b.current.as_decimal(),
            days_1_30: b.days_1_30.as_decimal(),
            days_31_60: b.days_31_60.as_decimal(),
            days_61_90: b.days_61_90.as_decimal(),
            over_90: b.over_90.as_decimal(),
        }),
        message: message.to_string(),
    }
}

/// The statements' pages, one statement after another.
///
/// # Errors
/// `Local(InvalidValue)` when a statement can't be laid out.
pub(crate) fn document(
    records: &[IssuedStatementRecord],
    company: &str,
    message: &str,
) -> Result<cl_print::layout::Document, ApiError> {
    let style = crate::screens::invoice::email::pdf_style(company);
    let mut doc: Option<cl_print::layout::Document> = None;
    for r in records {
        let d = data_of(&r.snapshot, &r.number, message);
        let one = form::layout(&d, &style)
            .map_err(|_| ApiError::Local(cl_api::LocalValidation::InvalidValue))?;
        match doc.as_mut() {
            Some(all) => all.pages.extend(one.pages),
            None => doc = Some(one),
        }
    }
    doc.ok_or(ApiError::Local(cl_api::LocalValidation::InvalidValue))
}

/// One issued statement's PDF (`Statement-<number>.pdf`, at most 10 MB).
/// Called on the request queue, never in a frame.
///
/// # Errors
/// `Local(InvalidValue)` when the PDF can't be made;
/// `Local(AttachmentTooLarge)` over 10 MB.
pub(crate) fn pdf_of(
    r: &IssuedStatementRecord,
    company: &str,
    message: &str,
) -> Result<(String, Vec<u8>), ApiError> {
    let doc = document(std::slice::from_ref(r), company, message)?;
    let bytes = cl_print::render(&doc)
        .map_err(|_| ApiError::Local(cl_api::LocalValidation::InvalidValue))?;
    if bytes.len() > cl_api::EMAIL_PDF_MAX_BYTES {
        return Err(ApiError::Local(cl_api::LocalValidation::AttachmentTooLarge));
    }
    Ok((cl_api::statement_pdf_name(&r.number), bytes))
}
