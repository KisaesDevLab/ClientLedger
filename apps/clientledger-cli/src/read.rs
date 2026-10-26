//! `clientledger-cli read <file> [--json]` (A03 step 19, Q-A03l): read one
//! bill or receipt on this computer, the way Enter Bill's "Read document"
//! does, and print what was found. No engine, no company and no network:
//! the sandboxed reading worker (and tesseract, for scans) does the work.
//!
//! The output is the document's own content, for the person at the
//! terminal; it is never logged. Without a company there is no vendor or
//! card match (those run in the desktop against the company's lists).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cl_core::{CalendarDate, Money};
use cl_docread::model::{CheckValue, Found, Payment, Reading};
use cl_docread::protocol::{JobFailure, JobHint, MAX_DOCUMENT_BYTES};
use cl_host::{
    AppPaths, CouldNotRead, DocJob, DocreadConfig, DocreadHost, JobOutcome, OcrTools,
    find_docread_exe,
};

use crate::host::EXIT_USAGE;

const EXIT_FAILED: u8 = 1;

/// Usage text for `read`.
pub const USAGE: &str = "\
usage: clientledger-cli read <file> [options]

Reads a bill or receipt (PDF, PNG, JPEG or TIFF) on this computer and shows
what was found. Nothing is sent anywhere and nothing is saved.

options:
  --json                 print the whole reading as JSON (amounts as strings)
  --today <YYYY-MM-DD>   the date two-digit years and date checks use
                         (default: today)
  --data-dir <path>      app data folder for the reader's scratch space
                         (default: your user data folder)
  --docread-exe <path>   the reading worker (default: next to this program)

exit status: 0 read, 1 couldn't read (enter it by hand), 2 usage error";

/// Parsed `read` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    pub file: PathBuf,
    pub json: bool,
    pub today: Option<CalendarDate>,
    pub data_dir: Option<PathBuf>,
    pub docread_exe: Option<PathBuf>,
}

/// Parse the arguments after `read`; `Ok(None)` is `--help`.
pub(crate) fn parse(args: &[String]) -> Result<Option<Options>, String> {
    let mut file = None;
    let mut opts = Options {
        file: PathBuf::new(),
        json: false,
        today: None,
        data_dir: None,
        docread_exe: None,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--help" | "-h" => return Ok(None),
            "--json" => opts.json = true,
            "--today" => {
                let v = value("--today")?;
                opts.today = Some(parse_date(&v).ok_or_else(|| {
                    format!("--today must be a date like 2026-11-04 (got `{v}`)")
                })?);
            }
            "--data-dir" => opts.data_dir = Some(PathBuf::from(value("--data-dir")?)),
            "--docread-exe" => opts.docread_exe = Some(PathBuf::from(value("--docread-exe")?)),
            s if s.starts_with('-') => return Err(format!("unknown option `{s}`")),
            s => {
                if file.is_some() {
                    return Err("give one file to read".into());
                }
                file = Some(PathBuf::from(s));
            }
        }
    }
    opts.file = file.ok_or("missing the file to read")?;
    Ok(Some(opts))
}

fn parse_date(s: &str) -> Option<CalendarDate> {
    let mut parts = s.splitn(3, '-');
    let y = parts.next()?.parse().ok()?;
    let m = parts.next()?.parse().ok()?;
    let d = parts.next()?.parse().ok()?;
    CalendarDate::from_ymd(y, m, d).ok()
}

fn local_today() -> Option<CalendarDate> {
    use chrono::Datelike as _;
    let now = chrono::Local::now().date_naive();
    CalendarDate::from_ymd(now.year(), now.month(), now.day()).ok()
}

/// Entry point for `clientledger-cli read`.
pub fn run(args: &[String]) -> ExitCode {
    let opts = match parse(args) {
        Ok(Some(o)) => o,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    match read(&opts) {
        Ok(reading) => {
            if opts.json {
                let Ok(s) = serde_json::to_string_pretty(&reading) else {
                    eprintln!("Couldn't show the reading as JSON.");
                    return ExitCode::from(EXIT_FAILED);
                };
                println!("{s}");
            } else {
                print!("{}", summary(&reading));
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("Couldn't read this document, enter it by hand. {message}");
            ExitCode::from(EXIT_FAILED)
        }
    }
}

fn read(opts: &Options) -> Result<Reading, String> {
    let bytes = read_file(&opts.file)?;
    let today = opts
        .today
        .or_else(local_today)
        .ok_or("Today's date couldn't be read from this computer; give --today.")?;
    let paths = match &opts.data_dir {
        Some(d) => AppPaths::at(std::path::absolute(d).unwrap_or_else(|_| d.clone())),
        None => AppPaths::for_current_user(),
    }
    .map_err(|_| "There's no place for the reader's scratch files; give --data-dir.")?;
    let exe = find_docread_exe(opts.docread_exe.as_deref())
        .ok_or("The reading program (clientledger-docread) wasn't found next to this one.")?;
    let mut host = DocreadHost::new(DocreadConfig::new(&paths, exe, OcrTools::find()));
    let job = DocJob {
        bytes,
        hint: JobHint::default(),
        templates: Vec::new(),
    };
    match host
        .read(std::slice::from_ref(&job), today)
        .into_iter()
        .next()
    {
        Some(JobOutcome::Read(r)) => Ok(*r),
        Some(JobOutcome::Failed(f)) => Err(failure_text(f).into()),
        Some(JobOutcome::CouldNotRead(c)) => Err(gave_up_text(c).into()),
        None => Err("The reader gave no answer.".into()),
    }
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|_| "That file wasn't found.".to_string())?;
    if !meta.is_file() {
        return Err("That isn't a file.".into());
    }
    if meta.len() > MAX_DOCUMENT_BYTES as u64 {
        return Err("The file is over 10 MB.".into());
    }
    std::fs::read(path).map_err(|_| "The file couldn't be opened.".into())
}

fn failure_text(f: JobFailure) -> &'static str {
    match f {
        JobFailure::UnsupportedType => "It isn't a PDF, PNG, JPEG or TIFF file.",
        JobFailure::Unreadable => "It's damaged, or there's nothing on it to read.",
        JobFailure::Protected => "The PDF has a password.",
        JobFailure::TooLarge => "It's over the size limits.",
        JobFailure::Empty => "The file is empty.",
        JobFailure::NeedsOcr => "It's a scan, and the scan reader isn't installed.",
    }
}

fn gave_up_text(c: CouldNotRead) -> &'static str {
    match c {
        CouldNotRead::Unavailable => "The reading program couldn't start.",
        CouldNotRead::TimedOut => "Reading it took too long.",
        CouldNotRead::TooMuchMemory => "Reading it needed too much memory.",
        CouldNotRead::Crashed | CouldNotRead::BadReply | CouldNotRead::Cancelled => {
            "The reader stopped on it."
        }
    }
}

/// `1,480.00`: the plain owner format (the exact value; `--json` keeps the
/// wire string).
fn money(m: Money) -> String {
    m.format(cl_core::MoneyStyle::default())
}

/// The plain summary: the fields found, the lines and the checks.
pub(crate) fn summary(r: &Reading) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let h = &r.header;
    let _ = writeln!(
        out,
        "Read {} of {} page(s) ({:?}); looks like: {:?}",
        r.pages.len(),
        r.page_count,
        r.reader,
        r.kind
    );
    let mut field = |name: &str, value: Option<String>| {
        if let Some(v) = value {
            let _ = writeln!(out, "  {name:<14} {v}");
        }
    };
    let text = |f: &Option<Found<String>>| f.as_ref().map(|f| f.value.replace('\n', ", "));
    let amount = |f: &Option<Found<Money>>| f.as_ref().map(|f| money(f.value));
    field("Vendor", text(&h.vendor));
    field("Address", text(&h.vendor_address));
    field("Phone", text(&h.vendor_phone));
    field("Account no.", text(&h.account_number));
    field("Date", h.date.as_ref().map(|d| d.value.to_string()));
    field("Reference", text(&h.reference));
    field("Terms", h.terms.as_ref().map(|t| t.value.text.clone()));
    field("Due date", h.due_date.as_ref().map(|d| d.value.to_string()));
    field("Subtotal", amount(&h.subtotal));
    field("Tax", amount(&h.tax));
    for fee in &h.fees {
        field("Fee", Some(money(fee.value)));
    }
    field("Total", amount(&h.total));
    field("Amount paid", amount(&h.amount_paid));
    field("Balance due", amount(&h.balance_due));
    field(
        "Paid by",
        h.payment.as_ref().map(|p| match &p.value {
            Payment::Card { last4 } => format!("card ending {}", last4.as_str()),
            Payment::Check { number } => format!("check {number}"),
            Payment::Cash => "cash".into(),
            Payment::Paid => "paid".into(),
        }),
    );
    if !r.lines.is_empty() {
        let _ = writeln!(out, "Lines:");
        for (i, l) in r.lines.iter().enumerate() {
            let desc = l
                .description
                .as_ref()
                .map_or(String::new(), |d| d.value.clone());
            let qty = match (&l.quantity, &l.unit_price) {
                (Some(q), Some(p)) => format!(" ({} x {})", q.value.to_wire(), money(p.value)),
                _ => String::new(),
            };
            let _ = writeln!(
                out,
                "  {:>2}. {desc}{qty}  {}",
                i + 1,
                money(l.amount.value)
            );
        }
    }
    if !r.checks.is_empty() {
        let _ = writeln!(out, "Check these (nothing was changed):");
        for c in &r.checks {
            let value = |v: &Option<CheckValue>| match v {
                Some(CheckValue::Money(m)) => money(*m),
                Some(CheckValue::Date(d)) => d.to_string(),
                Some(CheckValue::Text(t)) => t.clone(),
                None => String::new(),
            };
            let mut line = format!("  {:?}", c.code);
            if c.found.is_some() {
                let _ = write!(line, ": found {}", value(&c.found));
            }
            if c.expected.is_some() {
                let _ = write!(line, ", expected {}", value(&c.expected));
            }
            let _ = writeln!(out, "{line}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_docread::model::{Bbox, Check, CheckCode, Confidence, Line, Reader, RuleId, Spot};

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_string()).collect()
    }

    fn found<T>(value: T) -> Found<T> {
        Found {
            value,
            spot: Spot {
                page: 0,
                bbox: Bbox::new(1, 1, 2, 2).unwrap(),
            },
            rule: RuleId::new("t"),
            confidence: Confidence::HIGH,
        }
    }

    #[test]
    fn arguments() {
        let o = parse(&v(&["bill.pdf", "--json", "--today", "2026-11-04"]))
            .unwrap()
            .unwrap();
        assert_eq!(o.file, PathBuf::from("bill.pdf"));
        assert!(o.json);
        assert_eq!(o.today, CalendarDate::from_ymd(2026, 11, 4).ok());
        assert_eq!(parse(&v(&["--help"])).unwrap(), None);
        assert!(parse(&[]).is_err(), "a file is needed");
        assert!(parse(&v(&["a.pdf", "b.pdf"])).is_err());
        assert!(parse(&v(&["a.pdf", "--today", "11/04/2026"])).is_err());
        assert!(parse(&v(&["a.pdf", "--bogus"])).is_err());
        assert!(parse(&v(&["a.pdf", "--data-dir"])).is_err());
    }

    #[test]
    fn a_missing_file_fails_before_anything_starts() {
        let o = parse(&v(&["no-such-file.pdf", "--today", "2026-11-04"]))
            .unwrap()
            .unwrap();
        assert_eq!(read(&o).unwrap_err(), "That file wasn't found.");
    }

    #[test]
    fn the_summary_lists_fields_lines_and_checks() {
        let mut r = Reading::empty(Reader::TextPdf, 1);
        r.header.vendor = Some(found("Midwest Equipment Rental".to_string()));
        r.header.total = Some(found(Money::parse_wire("1480.00").unwrap()));
        r.lines.push(Line {
            description: Some(found("Delivery".to_string())),
            quantity: None,
            unit_price: None,
            amount: found(Money::parse_wire("150.00").unwrap()),
        });
        r.checks.push(Check {
            code: CheckCode::LinesVsSubtotal,
            rule: RuleId::new("t"),
            field: None,
            line: None,
            expected: Some(CheckValue::Money(Money::parse_wire("1480.00").unwrap())),
            found: Some(CheckValue::Money(Money::parse_wire("150.00").unwrap())),
            difference: None,
            spot: None,
        });
        let s = summary(&r);
        assert!(s.contains("Midwest Equipment Rental"), "{s}");
        assert!(s.contains("Total") && s.contains("1,480.00"), "{s}");
        assert!(s.contains("Delivery") && s.contains("150.00"), "{s}");
        assert!(
            s.contains("LinesVsSubtotal: found 150.00, expected 1,480.00"),
            "{s}"
        );
    }
}
