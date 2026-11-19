//! The exact transaction list (`GET /api/v1/reports/transaction-list?
//! exact=1`), one row per posted journal line, read window by window by
//! the IIF export (P32 D2). Accrual basis, posted entries only: the engine
//! leaves out voided, draft and cash-basis-only entries and the reversing
//! lines of a void.
//!
//! Amounts come from `amountExact` (debit minus credit) as a decimal
//! string only; a JSON number is a contract error.

use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money};
use serde_json::Value;

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::resources::operations::REPORT_TRANSACTION_LIST;
use crate::resources::reports::Reports;

/// One journal line of the exact transaction list.
#[derive(Clone, PartialEq, Eq)]
pub struct TxnListRow {
    /// The transaction.
    pub id: TransactionId,
    /// The journal line.
    pub line_id: String,
    /// The line's account.
    pub account_id: AccountId,
    /// The line's contact, else the transaction's.
    pub contact_id: Option<ContactId>,
    /// Engine `txnType`.
    pub txn_type: String,
    /// The number shown to the owner.
    pub txn_number: Option<String>,
    /// Date.
    pub txn_date: CalendarDate,
    /// Transaction memo (owner data).
    pub memo: Option<String>,
    /// Line description (owner data).
    pub line_description: Option<String>,
    /// Debit minus credit, exact.
    pub amount: Money,
}

/// Owner data: only ids and the type.
impl std::fmt::Debug for TxnListRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TxnListRow")
            .field("id", &self.id)
            .field("line_id", &self.line_id)
            .field("txn_type", &self.txn_type)
            .finish_non_exhaustive()
    }
}

/// One answer of the exact transaction list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxnList {
    /// The lines, in the engine's order.
    pub rows: Vec<TxnListRow>,
    /// The engine left out lines of accounts hidden from this user (U-33).
    pub hidden_notice: bool,
}

fn text(r: &Value, k: &str) -> Option<String> {
    r.get(k)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// A date as the engine sends it (`2026-01-31`, or a timestamp at
/// midnight UTC).
fn date(r: &Value, k: &str) -> Option<CalendarDate> {
    let s = r.get(k)?.as_str()?;
    CalendarDate::parse_wire(s.get(..10)?).ok()
}

/// Reads one answer (pure; the tests and the demo backend use it).
///
/// # Errors
/// `ReportShape` when a row lacks its id, line, account, date or exact
/// amount string.
pub fn parse_txn_list(v: &Value) -> Result<TxnList, ApiError> {
    let shape = |what| ApiError::ReportShape { what };
    let data = v
        .get("data")
        .and_then(Value::as_array)
        .ok_or(shape("transaction list"))?;
    let mut rows = Vec::with_capacity(data.len());
    for r in data {
        let id = text(r, "id")
            .and_then(|s| TransactionId::parse(&s).ok())
            .ok_or(shape("transaction id"))?;
        let amount = r
            .get("amountExact")
            .and_then(Value::as_str)
            .and_then(|s| Money::parse_wire(s).ok())
            .ok_or(shape("amountExact"))?;
        rows.push(TxnListRow {
            id,
            line_id: text(r, "lineId").ok_or(shape("lineId"))?,
            account_id: text(r, "accountId")
                .and_then(|s| AccountId::parse(&s).ok())
                .ok_or(shape("accountId"))?,
            contact_id: match text(r, "contactId") {
                Some(s) => Some(ContactId::parse(&s).map_err(|_| shape("contactId"))?),
                None => None,
            },
            txn_type: text(r, "txn_type").unwrap_or_default(),
            txn_number: text(r, "txn_number"),
            txn_date: date(r, "txn_date").ok_or(shape("txn_date"))?,
            memo: text(r, "memo"),
            line_description: text(r, "line_description"),
            amount,
        });
    }
    Ok(TxnList {
        rows,
        hidden_notice: super::adapters::hidden_notice(v),
    })
}

impl Reports<'_> {
    /// The posted journal lines dated `start..=end` (either end open),
    /// accrual basis, exact.
    ///
    /// # Errors
    /// `ReportShape`; `RateLimited`; any [`ApiError`].
    pub async fn transaction_list_exact(
        &self,
        start: Option<CalendarDate>,
        end: Option<CalendarDate>,
    ) -> Result<TxnList, ApiError> {
        let mut call = Call::at(Method::Get, REPORT_TRANSACTION_LIST.route());
        call.query = vec![
            ("exact".into(), "1".into()),
            ("basis".into(), "accrual".into()),
        ];
        if let Some(s) = start {
            call.query.push(("start_date".into(), s.to_wire()));
        }
        if let Some(e) = end {
            call.query.push(("end_date".into(), e.to_wire()));
        }
        let reply = self.0.send(&REPORT_TRANSACTION_LIST, call, None).await?;
        let v: Value = serde_json::from_slice(&reply.body).map_err(|_| ApiError::ReportShape {
            what: "transaction list",
        })?;
        parse_txn_list(&v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: &str = r#"{"id":"1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","lineId":"l1",
        "accountId":"5c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9b04","contactId":null,
        "txn_type":"journal_entry","txn_number":"7","txn_date":"2026-01-31",
        "memo":"m","line_description":null,"debitExact":"10.0050","creditExact":"0.0000",
        "amountExact":"10.0050"}"#;

    #[test]
    fn rows_read_exact_strings_only() {
        let v: Value = serde_json::from_str(&format!(r#"{{"data":[{ROW}],"hiddenNotice":false}}"#))
            .expect("json");
        let t = parse_txn_list(&v).expect("rows");
        assert_eq!(t.rows.len(), 1);
        assert_eq!(t.rows[0].amount.to_wire(), "10.0050");
        assert_eq!(t.rows[0].txn_date.to_wire(), "2026-01-31");
        assert!(!t.hidden_notice);
        assert!(!format!("{:?}", t.rows[0]).contains("10.0050"));
        let number = ROW.replace(r#""amountExact":"10.0050""#, r#""amountExact":10.005"#);
        let v: Value = serde_json::from_str(&format!(r#"{{"data":[{number}]}}"#)).expect("json");
        assert!(matches!(
            parse_txn_list(&v),
            Err(ApiError::ReportShape {
                what: "amountExact"
            })
        ));
        let stamp = ROW.replace("\"2026-01-31\"", "\"2026-01-31T00:00:00.000Z\"");
        let v: Value =
            serde_json::from_str(&format!(r#"{{"data":[{stamp}],"hiddenNotice":true}}"#))
                .expect("json");
        let t = parse_txn_list(&v).expect("rows");
        assert_eq!(t.rows[0].txn_date.to_wire(), "2026-01-31");
        assert!(t.hidden_notice);
    }
}
