//! Reads the books for an IIF file from the engine and writes it (P32
//! D2).
//!
//! - **Who:** the owner must see every account (Q-P32i): the caller passes
//!   whether the user holds the `sensitive_accounts` right, and every
//!   answer that says lines were hidden (`hiddenNotice`) aborts too,
//!   because the list would silently miss them.
//! - **Range:** all dates by default, future-dated entries included. The
//!   windows are one calendar month each, oldest first, between the first
//!   and last posted dates; the first window has no start and the last no
//!   end, so no entry can fall outside them (Q-P32m).
//! - **One snapshot:** the ledger-version stamp is read before the lists
//!   and after the last window; a change aborts ("your books changed").
//!   Lines are grouped by transaction id ([`super::TxnCollector`]); a
//!   repeated line or a transaction seen in two windows aborts.
//! - **Output:** the plan is made in full, then streamed to
//!   `<dest>.partial` and renamed; the partial file is removed on every
//!   other exit.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cl_api::{
    AccountFilter, ApiError, ContactFilter, ItemFilter, PageRequest, Session, SortDir,
    TransactionFilter, TxnSort, TxnStatus,
};
use cl_core::CalendarDate;

use super::{
    Books, ContactKind, Counts, IifError, Limitation, Options, SourceAccount, SourceContact,
    SourceItem, SourceLine, SourceTxn, TxnCollector, plan,
};
use crate::backup::{Progress, ProgressSink};

/// What to export and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IifRequest {
    /// The `.iif` file to write (an existing file is replaced; the Save
    /// dialog asked).
    pub dest: PathBuf,
    /// First date, or all earlier dates.
    pub start: Option<CalendarDate>,
    /// Last date, or all later dates (future-dated entries included).
    pub end: Option<CalendarDate>,
    /// Lists and transactions.
    pub options: Options,
    /// The user holds the `sensitive_accounts` right (Q-P32i).
    pub sees_hidden: bool,
}

/// What [`export_iif`] wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IifOutcome {
    /// The file.
    pub path: PathBuf,
    /// What it holds.
    pub counts: Counts,
    /// What it can't carry.
    pub limitations: Vec<Limitation>,
    /// Characters written as `?`.
    pub unmappable_chars: u64,
    /// File size.
    pub bytes: u64,
}

/// One window of the transaction list: `start..=end`, either end open.
pub type Window = (Option<CalendarDate>, Option<CalendarDate>);

/// The month windows that tile `start..=end` (either open) given the
/// first and last posted dates: the first window keeps the open start and
/// the last the open end, so together they cover every date of the range
/// exactly once. No posted entries: one window for the whole range.
#[must_use]
pub fn windows(
    start: Option<CalendarDate>,
    end: Option<CalendarDate>,
    first: Option<CalendarDate>,
    last: Option<CalendarDate>,
) -> Vec<Window> {
    let (Some(first), Some(last)) = (first, last) else {
        return vec![(start, end)];
    };
    // The months to split: the posted dates within the range.
    let lo = start.map_or(first, |s| s.max(first));
    let hi = end.map_or(last, |e| e.min(last));
    if lo > hi {
        return vec![(start, end)];
    }
    let mut out = Vec::new();
    let mut month = lo.start_of_month();
    let last_month = hi.start_of_month();
    let mut from = start;
    while month < last_month {
        let to = month.end_of_month();
        out.push((from, Some(to)));
        from = to.add_days(1).ok();
        match month.add_months(1) {
            Ok(m) => month = m,
            Err(_) => break,
        }
    }
    out.push((from, end));
    out
}

/// How long to wait when the engine asks for a pause between reports.
const RATE_PAUSE: Duration = Duration::from_secs(5);
/// How many pauses one window may take.
const RATE_TRIES: u32 = 30;

async fn window_rows(
    session: &Session,
    w: Window,
    cancel: &AtomicBool,
) -> Result<cl_api::TxnList, IifError> {
    for _ in 0..RATE_TRIES {
        if cancel.load(Ordering::Relaxed) {
            return Err(IifError::Cancelled);
        }
        match session.reports().transaction_list_exact(w.0, w.1).await {
            Err(ApiError::RateLimited { retry_after }) => {
                tokio::time::sleep(retry_after.unwrap_or(RATE_PAUSE).min(RATE_PAUSE * 12)).await;
            }
            other => return other.map_err(IifError::from),
        }
    }
    Err(IifError::Api(ApiError::RateLimited { retry_after: None }))
}

async fn bound(session: &Session, dir: SortDir) -> Result<Option<CalendarDate>, IifError> {
    let filter = TransactionFilter {
        status: Some(TxnStatus::Posted),
        sort: Some((TxnSort::Date, dir)),
        ..TransactionFilter::default()
    };
    let page = PageRequest::new(1, 0)?;
    let rows = session.transactions().list(&filter, page).await?;
    Ok(rows.data.first().map(|t| t.txn_date))
}

/// Reads the lists (inactive entries included).
async fn lists(session: &Session) -> Result<Books, IifError> {
    let accounts = session
        .accounts()
        .list_all(&AccountFilter::default())
        .await?
        .into_iter()
        .map(|a| SourceAccount {
            id: a.id.to_string(),
            name: a.fields.name,
            number: a.fields.account_number,
            parent_id: a.fields.parent_id.map(|p| p.to_string()),
            account_type: wire_name(&a.fields.account_type),
            detail_type: a.fields.detail_type,
            system_tag: a.system_tag,
            description: a.fields.description,
        })
        .collect();
    let contacts = session
        .contacts()
        .list_all(&ContactFilter::default())
        .await?
        .into_iter()
        .filter_map(|c| {
            let kind = ContactKind::parse(&wire_name(&c.fields.contact_type))?;
            Some(SourceContact {
                id: c.id.to_string(),
                name: c.fields.display_name,
                kind,
            })
        })
        .collect();
    let items = session
        .items()
        .list_all(&ItemFilter::default())
        .await?
        .into_iter()
        .map(|i| SourceItem {
            id: i.id.to_string(),
            name: i.fields.name,
            item_type: wire_name(&i.fields.item_type),
            description: i.fields.description,
            income_account_id: Some(i.fields.income_account_id.to_string()),
        })
        .collect();
    Ok(Books {
        accounts,
        contacts,
        items,
        txns: Vec::new(),
    })
}

/// A serde enum's wire name (`"asset"`, `"customer"`, `"inventory_part"`).
fn wire_name<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The rows of one window as headers and lines.
fn split_rows(list: cl_api::TxnList) -> Vec<(SourceTxn, SourceLine)> {
    list.rows
        .into_iter()
        .map(|r| {
            let contact = r.contact_id.map(|c| c.to_string());
            (
                SourceTxn {
                    id: r.id.to_string(),
                    date: r.txn_date,
                    txn_type: r.txn_type,
                    number: r.txn_number,
                    memo: r.memo,
                    contact_id: None,
                    lines: Vec::new(),
                },
                SourceLine {
                    line_id: r.line_id,
                    account_id: r.account_id.to_string(),
                    contact_id: contact,
                    amount: r.amount,
                    memo: r.line_description,
                },
            )
        })
        .collect()
}

/// Reads the books and writes the IIF file.
///
/// # Errors
/// `HiddenAccounts` (nothing read, or a window hid lines); `BooksChanged`;
/// `Duplicate`; `Inconsistent`; `Stopped` with every transaction that
/// can't be written; `Cancelled`; `Io`; `Api`. Nothing is left at `dest`
/// or `<dest>.partial` on any error.
pub async fn export_iif(
    session: &Session,
    req: &IifRequest,
    progress: &ProgressSink,
    cancel: &AtomicBool,
) -> Result<IifOutcome, IifError> {
    if !req.sees_hidden {
        return Err(IifError::HiddenAccounts);
    }
    progress(Progress::Starting);
    let before = session.ledger_version().await?;
    let mut books = lists(session).await?;
    if req.options.include_transactions {
        let first = bound(session, SortDir::Asc).await?;
        let last = bound(session, SortDir::Desc).await?;
        let ws = windows(req.start, req.end, first, last);
        let total = ws.len();
        let mut collector = TxnCollector::default();
        for (i, w) in ws.into_iter().enumerate() {
            let answer = window_rows(session, w, cancel).await?;
            if answer.hidden_notice {
                return Err(IifError::HiddenAccounts);
            }
            collector.add_window(split_rows(answer))?;
            progress(Progress::Working {
                percent: u8::try_from((i + 1) * 90 / total.max(1)).ok(),
                step: None,
            });
        }
        books.txns = collector.finish();
    }
    if session.ledger_version().await? != before {
        return Err(IifError::BooksChanged);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(IifError::Cancelled);
    }
    let planned = plan(&books, req.options)?;
    progress(Progress::Copying);
    let dest = req.dest.clone();
    let p2 = planned.clone();
    let bytes = tokio::task::spawn_blocking(move || write_file(&p2, &dest))
        .await
        .map_err(|_| IifError::Io(std::io::Error::other("worker stopped")))??;
    progress(Progress::Done);
    Ok(IifOutcome {
        path: req.dest.clone(),
        counts: planned.counts,
        limitations: planned.limitations,
        unmappable_chars: planned.unmappable_chars,
        bytes,
    })
}

/// `<dest>.partial`.
#[must_use]
pub fn partial_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".partial");
    PathBuf::from(s)
}

/// Streams the plan to `<dest>.partial`, then renames it to `dest`. The
/// partial file never survives an error.
///
/// # Errors
/// Any I/O error.
pub fn write_file(plan: &super::Plan, dest: &Path) -> Result<u64, IifError> {
    use std::io::Write as _;
    let partial = partial_path(dest);
    let result = (|| -> std::io::Result<u64> {
        let file = std::fs::File::create(&partial)?;
        let mut w = std::io::BufWriter::new(file);
        plan.write_to(&mut w)?;
        w.flush()?;
        let file = w
            .into_inner()
            .map_err(std::io::IntoInnerError::into_error)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&partial, dest)?;
        Ok(std::fs::metadata(dest)?.len())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    Ok(result?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> CalendarDate {
        CalendarDate::parse_wire(s).expect("date")
    }

    /// Every date of the range falls in exactly one window.
    fn covers_once(ws: &[Window], day: CalendarDate) -> bool {
        ws.iter()
            .filter(|(a, b)| a.is_none_or(|a| a <= day) && b.is_none_or(|b| day <= b))
            .count()
            == 1
    }

    #[test]
    fn windows_tile_the_range_exactly() {
        let ws = windows(None, None, Some(d("2025-11-15")), Some(d("2026-02-03")));
        assert_eq!(ws.len(), 4);
        assert_eq!(ws[0], (None, Some(d("2025-11-30"))));
        assert_eq!(ws[1], (Some(d("2025-12-01")), Some(d("2025-12-31"))));
        assert_eq!(ws[3], (Some(d("2026-02-01")), None));
        let mut day = d("2020-01-01");
        while day < d("2030-01-01") {
            assert!(covers_once(&ws, day), "{day:?}");
            day = day.add_days(7).expect("day");
        }
        // A range inside the books keeps its own ends.
        let ws = windows(
            Some(d("2026-01-10")),
            Some(d("2026-03-05")),
            Some(d("2025-01-01")),
            Some(d("2027-01-01")),
        );
        assert_eq!(ws.first().map(|w| w.0), Some(Some(d("2026-01-10"))));
        assert_eq!(ws.last().map(|w| w.1), Some(Some(d("2026-03-05"))));
        assert_eq!(ws.len(), 3);
        // No posted entries, or a range outside them: one window.
        assert_eq!(windows(None, None, None, None), vec![(None, None)]);
        assert_eq!(
            windows(
                Some(d("2030-01-01")),
                None,
                Some(d("2025-01-01")),
                Some(d("2026-01-01"))
            ),
            vec![(Some(d("2030-01-01")), None)]
        );
        // One month.
        assert_eq!(
            windows(None, None, Some(d("2026-01-05")), Some(d("2026-01-20"))),
            vec![(None, None)]
        );
    }

    #[test]
    fn a_failed_write_leaves_no_partial_file() {
        let dir = tempfile::tempdir().expect("tmp");
        let plan = super::super::plan(&Books::default(), Options::default()).expect("plan");
        // A destination inside a folder that doesn't exist: create fails.
        let dest = dir.path().join("missing").join("x.iif");
        assert!(write_file(&plan, &dest).is_err());
        assert!(!partial_path(&dest).exists());
        // A destination that is a folder: the rename fails after writing.
        let folder = dir.path().join("taken.iif");
        std::fs::create_dir(&folder).expect("dir");
        std::fs::write(folder.join("f"), b"x").expect("file");
        assert!(write_file(&plan, &folder).is_err());
        assert!(!partial_path(&folder).exists());
        // A good one.
        let ok = dir.path().join("ok.iif");
        write_file(&plan, &ok).expect("write");
        assert!(ok.is_file());
        assert!(!partial_path(&ok).exists());
    }
}
