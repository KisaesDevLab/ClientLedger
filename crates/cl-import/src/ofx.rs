//! OFX 1.x (SGML) and 2.x (XML) bank and card statements (P25 step 2).
//!
//! - Several statements become several sections; investment statements
//!   (`INVSTMTRS`) are refused; a `SEVERITY` of `ERROR` is a file error.
//! - Dates use the first 8 digits (`YYYYMMDD`), with no time-zone shift:
//!   `20261103120000.000[-8:PST]` and `20261103[+12]` are both 2026-11-03.
//! - The description is NAME only ([`crate::dedupe::ofx_description`],
//!   Q-P25r); `PAYEE/NAME` stands in for NAME. MEMO goes to the row's memo,
//!   with "Ref: …" for `REFNUM`.
//! - `TRNTYPE` is kept for the sign check.

pub mod sgml;
pub mod xml;

use crate::detect::FileFormat;
use crate::error::ImportError;
use crate::normalize::{
    AccountId, AccountKind, Balance, Decoded, Institution, ParsedFile, ProblemKind, RowParts,
    Section, decode, parse_amount, parse_amount_detail, trim_ws,
};
use cl_core::CalendarDate;

/// Deepest element nesting accepted (review S12).
pub const MAX_DEPTH: usize = 32;
/// Longest tag name or value accepted, in bytes (review S12).
pub const MAX_TOKEN_BYTES: usize = 64 * 1024;
/// Most elements accepted in one file. A statement row is about ten
/// elements, so this is far above 20,000 rows in several sections, yet a
/// 25 MB file of empty tags stops early.
pub const MAX_NODES: usize = 1_000_000;

/// An element: its upper-cased name, its text (leaves), its children, and
/// the line of its opening tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Node {
    pub name: String,
    pub text: Option<String>,
    pub children: Vec<Node>,
    pub line: usize,
}

impl Node {
    pub(crate) fn new(name: String, line: usize) -> Self {
        Self {
            name,
            text: None,
            children: Vec::new(),
            line,
        }
    }

    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    fn text_of(&self, name: &str) -> Option<&str> {
        self.child(name).and_then(|c| c.text.as_deref())
    }

    /// Every descendant named `name`, in document order (iterative; depth
    /// is capped by the tokenizer anyway).
    fn descendants<'a>(&'a self, name: &str) -> Vec<&'a Node> {
        let mut out = Vec::new();
        let mut stack: Vec<&Node> = self.children.iter().rev().collect();
        while let Some(n) = stack.pop() {
            if n.name == name {
                out.push(n);
            }
            stack.extend(n.children.iter().rev());
        }
        out
    }

    fn first(&self, name: &str) -> Option<&Node> {
        let mut stack: Vec<&Node> = self.children.iter().rev().collect();
        while let Some(n) = stack.pop() {
            if n.name == name {
                return Some(n);
            }
            stack.extend(n.children.iter().rev());
        }
        None
    }
}

/// Reads an OFX file (QFX too: the format is `Qfx` when the sign-on block
/// has an `INTU.BID`).
///
/// # Errors
/// See [`crate::parse`].
pub fn parse(bytes: &[u8]) -> Result<ParsedFile, ImportError> {
    parse_as(bytes, None)
}

/// Reads an OFX file, forcing the format when `format` is given.
pub(crate) fn parse_as(
    bytes: &[u8],
    format: Option<FileFormat>,
) -> Result<ParsedFile, ImportError> {
    if bytes.len() > crate::MAX_FILE_BYTES {
        return Err(ImportError::TooLarge {
            limit_bytes: crate::MAX_FILE_BYTES,
        });
    }
    let (
        Decoded {
            text,
            encoding,
            note,
        },
        is_xml,
    ) = decode_body(bytes)?;
    // SGML: the body starts at the first `<`; the header lines before it
    // still count for line numbers.
    let start = if is_xml {
        0
    } else {
        text.find('<').ok_or(ImportError::UnknownFormat)?
    };
    let first_line = 1 + text[..start].matches('\n').count();
    let root = sgml::build_tree(&text[start..], is_xml, first_line)?;
    let ofx = root.first("OFX").ok_or(ImportError::NoStatement)?;

    if let Some(sev) = ofx
        .first("SIGNONMSGSRSV1")
        .and_then(|s| s.first("STATUS"))
        .and_then(bank_error)
    {
        return Err(sev);
    }
    if let Some(inv) = ofx
        .first("INVSTMTRS")
        .or_else(|| ofx.first("INVSTMTMSGSRSV1"))
    {
        return Err(ImportError::Investment { line: inv.line });
    }
    let institution = institution(ofx);

    let mut sections = Vec::new();
    for trnrs in ofx
        .descendants("STMTTRNRS")
        .into_iter()
        .chain(ofx.descendants("CCSTMTTRNRS"))
    {
        if let Some(err) = trnrs.child("STATUS").and_then(bank_error) {
            return Err(err);
        }
    }
    let mut statements: Vec<(&Node, bool)> = ofx
        .descendants("STMTRS")
        .into_iter()
        .map(|n| (n, false))
        .chain(ofx.descendants("CCSTMTRS").into_iter().map(|n| (n, true)))
        .collect();
    statements.sort_by_key(|(n, _)| n.line);
    for (stmt, card) in statements {
        let mut section = statement(stmt, card, sections.len());
        section.check_row_limit()?;
        section.fill_range_from_rows();
        crate::sign::analyze(&mut section);
        sections.push(section);
    }
    if sections.is_empty() {
        return Err(ImportError::NoStatement);
    }
    let format = format.unwrap_or(if institution.intu_bid.is_some() {
        FileFormat::Qfx
    } else {
        FileFormat::Ofx
    });
    let has_institution =
        institution.org.is_some() || institution.fid.is_some() || institution.intu_bid.is_some();
    Ok(ParsedFile {
        format,
        encoding,
        encoding_note: note,
        institution: has_institution.then_some(institution),
        sections,
        csv_mapping: None,
        csv_headers: None,
    })
}

/// Decodes using the declared character set: the header is ASCII, so it is
/// read from the raw bytes first. A UTF-16 file (byte-order mark) is
/// decoded first instead, and its header only tells XML from SGML.
/// Returns the text and whether the body follows the XML rules.
fn decode_body(bytes: &[u8]) -> Result<(Decoded, bool), ImportError> {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        let decoded = decode(bytes, crate::normalize::Declared::Nothing)?;
        let is_xml = xml::is_xml(&decoded.text);
        return Ok((decoded, is_xml));
    }
    let raw = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let head_len = raw.len().min(4096);
    let head: String = raw[..head_len].iter().map(|&b| char::from(b)).collect();
    let header = xml::read_header(&head)?;
    Ok((decode(bytes, header.declared)?, header.xml))
}

/// A `STATUS` aggregate with `SEVERITY` `ERROR`.
fn bank_error(status: &Node) -> Option<ImportError> {
    let sev = status.child("SEVERITY")?;
    if !sev
        .text
        .as_deref()
        .is_some_and(|t| t.eq_ignore_ascii_case("ERROR"))
    {
        return None;
    }
    let code: String = status
        .text_of("CODE")
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_digit)
        .take(10)
        .collect();
    Some(ImportError::BankError {
        line: sev.line,
        code,
    })
}

fn institution(ofx: &Node) -> Institution {
    let sonrs = ofx.first("SONRS");
    let fi = sonrs.and_then(|s| s.child("FI"));
    let text = |n: Option<&Node>, name: &str| {
        n.and_then(|n| n.text_of(name))
            .map(|t| trim_ws(t).chars().take(64).collect::<String>())
            .filter(|t| !t.is_empty())
    };
    Institution {
        org: text(fi, "ORG"),
        fid: text(fi, "FID"),
        intu_bid: text(sonrs, "INTU.BID"),
    }
}

/// The first 8 digits of an OFX date as a calendar date.
fn ofx_date(text: &str) -> Result<CalendarDate, ProblemKind> {
    let t = trim_ws(text);
    if t.is_empty() {
        return Err(ProblemKind::MissingDate);
    }
    let digits = t.get(..8).filter(|d| d.bytes().all(|b| b.is_ascii_digit()));
    let Some(d) = digits else {
        return Err(ProblemKind::BadDate);
    };
    let n = |r: std::ops::Range<usize>| d[r].parse::<u32>().unwrap_or(0);
    let year = i32::try_from(n(0..4)).map_err(|_| ProblemKind::BadDate)?;
    CalendarDate::from_ymd(year, n(4..6), n(6..8)).map_err(|_| ProblemKind::ImpossibleDate)
}

fn statement(stmt: &Node, card: bool, index: usize) -> Section {
    let (kind, account) = if card {
        let from = stmt.child("CCACCTFROM");
        (
            AccountKind::CreditCard,
            from.and_then(|f| AccountId::new(None, f.text_of("ACCTID").unwrap_or_default())),
        )
    } else {
        let from = stmt.child("BANKACCTFROM");
        let kind = match from
            .and_then(|f| f.text_of("ACCTTYPE"))
            .map(str::to_ascii_uppercase)
            .as_deref()
        {
            Some("SAVINGS") => AccountKind::Savings,
            Some("MONEYMRKT") => AccountKind::MoneyMarket,
            Some("CREDITLINE") => AccountKind::CreditLine,
            Some("CD") => AccountKind::Cd,
            Some("CHECKING") => AccountKind::Checking,
            _ => AccountKind::Unknown,
        };
        (
            kind,
            from.and_then(|f| {
                AccountId::new(f.text_of("BANKID"), f.text_of("ACCTID").unwrap_or_default())
            }),
        )
    };
    let mut section = Section::new(index, kind);
    section.account = account;
    section.currency = stmt
        .text_of("CURDEF")
        .map(|c| {
            trim_ws(c)
                .chars()
                .take(3)
                .collect::<String>()
                .to_ascii_uppercase()
        })
        .filter(|c| c.len() == 3);
    if let Some(list) = stmt.child("BANKTRANLIST") {
        section.start = list.text_of("DTSTART").and_then(|d| ofx_date(d).ok());
        section.end = list.text_of("DTEND").and_then(|d| ofx_date(d).ok());
        for trn in list.children.iter().filter(|c| c.name == "STMTTRN") {
            push_transaction(&mut section, trn);
        }
    }
    if let Some(bal) = stmt.child("LEDGERBAL")
        && let Some(amount) = bal.text_of("BALAMT").and_then(|a| parse_amount(a).ok())
    {
        section.ledger_balance = Some(Balance {
            amount,
            date: bal.text_of("DTASOF").and_then(|d| ofx_date(d).ok()),
        });
    }
    section
}

fn push_transaction(section: &mut Section, trn: &Node) {
    let name = trn
        .text_of("NAME")
        .or_else(|| trn.child("PAYEE").and_then(|p| p.text_of("NAME")));
    let description = crate::dedupe::ofx_description(name);
    let reference = trn.text_of("REFNUM").map(trim_ws).filter(|r| !r.is_empty());
    let memo = crate::normalize::memo_with_ref(
        crate::dedupe::ofx_memo(trn.text_of("MEMO")).as_deref(),
        reference,
    );
    let amount = trn.text_of("TRNAMT").map_or(
        Err(crate::normalize::AmountProblem::Missing),
        parse_amount_detail,
    );
    section.push_parts(&RowParts {
        line: trn.line,
        date: trn
            .text_of("DTPOSTED")
            .map_or(Err(ProblemKind::MissingDate), ofx_date),
        amount,
        description: &description,
        memo: memo.as_deref(),
        check: trn.text_of("CHECKNUM"),
        fitid: trn.text_of("FITID"),
        running_balance: None,
        trn_type: trn.text_of("TRNTYPE"),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ofx_dates_keep_the_day() {
        let d = CalendarDate::from_ymd(2026, 11, 3).unwrap();
        assert_eq!(ofx_date("20261103120000.000[-8:PST]"), Ok(d));
        assert_eq!(ofx_date("20261103[+12]"), Ok(d));
        assert_eq!(ofx_date("20261103235959[+12:NZST]"), Ok(d));
        assert_eq!(ofx_date("20261103"), Ok(d));
        assert_eq!(ofx_date("2026110"), Err(ProblemKind::BadDate));
        assert_eq!(ofx_date("20260230"), Err(ProblemKind::ImpossibleDate));
        assert_eq!(ofx_date(""), Err(ProblemKind::MissingDate));
    }
}
