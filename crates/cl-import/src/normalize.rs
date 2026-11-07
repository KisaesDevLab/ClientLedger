//! The normalized shape every bank file becomes, plus the shared helpers
//! the parsers use: decoding bytes to text (Q-P25h), reading amounts
//! exactly (plan §4), cleaning text (NFC, length caps) and check numbers.

use crate::csv::dates::DateColumn;
use crate::detect::FileFormat;
use crate::error::ImportError;
use crate::sign::SignCheck;
use cl_core::{CalendarDate, Money, UserInput};
use rust_decimal::Decimal;
use std::fmt;
use unicode_normalization::UnicodeNormalization as _;

/// Largest file read, in bytes (Q-P25d).
pub const MAX_FILE_BYTES: usize = 25 * 1024 * 1024;
/// Most rows in one section (Q-P25d; the engine refuses more).
pub const MAX_ROWS: usize = 20_000;
/// Longest description kept, in characters (the engine's varchar(500)).
pub const MAX_DESCRIPTION_CHARS: usize = 500;
/// Longest memo kept, in characters (Q-P25d).
pub const MAX_MEMO_CHARS: usize = 1_000;
/// Most integer digits in an amount (Q-P25q).
pub const MAX_INTEGER_DIGITS: usize = 12;
/// Most digits a check number can have and still be a check number; longer
/// references go into the memo as "Ref: …" (plan §4).
pub const MAX_CHECK_DIGITS: usize = 9;

/// A whole file, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFile {
    /// What the file is.
    pub format: FileFormat,
    /// How its bytes were turned into text.
    pub encoding: TextEncoding,
    /// Set when the decoding needed a remark (Q-P25h).
    pub encoding_note: Option<EncodingNote>,
    /// The bank named in an OFX/QFX sign-on block, when present.
    pub institution: Option<Institution>,
    /// One per account statement in the file; CSV and most QIF files have
    /// one (Q-P25i: one section per import, then "Import next account").
    pub sections: Vec<Section>,
    /// The CSV column mapping used, for CSV files (Q-P25w): show it, let
    /// the owner edit it, store it with the batch.
    pub csv_mapping: Option<crate::csv::mapping::CsvMapping>,
    /// The CSV header row (or "Column N" names when there is none), for
    /// the mapping step.
    pub csv_headers: Option<Vec<String>>,
}

/// One account's statement inside a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// 0-based position in the file (the batch's `sectionIndex`).
    pub index: usize,
    /// Bank, card or cash account, when the file says.
    pub kind: AccountKind,
    /// The account number the file names (OFX/QFX only). Prints masked.
    pub account: Option<AccountId>,
    /// A QIF `!Account` name, when the file has one (shown so the owner
    /// can tell sections apart; never logged).
    pub account_name: Option<String>,
    /// ISO currency code from the file (`CURDEF`), when present.
    pub currency: Option<String>,
    /// First date the statement covers: `DTSTART` when present, otherwise
    /// the earliest row date.
    pub start: Option<CalendarDate>,
    /// Last date the statement covers: `DTEND` when present, otherwise the
    /// latest row date.
    pub end: Option<CalendarDate>,
    /// The ending balance the file states (`LEDGERBAL`) or the CSV running
    /// balance proves (in the file's sign; flip with [`crate::to_display`]).
    pub ledger_balance: Option<Balance>,
    /// The rows to send, in file order.
    pub rows: Vec<BankRow>,
    /// Rows left out on purpose (zero amounts, review S10).
    pub skipped: Vec<SkippedRow>,
    /// Rows that could not be read, each with its line.
    pub problems: Vec<Problem>,
    /// The date column's order and evidence (QIF and CSV; `None` for
    /// OFX, whose dates have one fixed form).
    pub dates: Option<DateColumn>,
    /// Whether the amounts look like the other sign convention.
    pub sign: SignCheck,
}

impl Section {
    /// An empty section of the given kind, for parsers to fill.
    #[must_use]
    pub fn new(index: usize, kind: AccountKind) -> Self {
        Self {
            index,
            kind,
            account: None,
            account_name: None,
            currency: None,
            start: None,
            end: None,
            ledger_balance: None,
            rows: Vec::new(),
            skipped: Vec::new(),
            problems: Vec::new(),
            dates: None,
            sign: SignCheck::default(),
        }
    }

    /// Sum of the rows' file amounts (exact). `None` only on overflow, which
    /// 20,000 rows of at most 12 integer digits cannot reach.
    #[must_use]
    pub fn total(&self) -> Option<Decimal> {
        self.rows
            .iter()
            .try_fold(Decimal::ZERO, |acc, r| acc.checked_add(r.file_amount))
    }

    /// Fills `start`/`end` from the rows when the file gave none.
    pub(crate) fn fill_range_from_rows(&mut self) {
        let min = self.rows.iter().map(|r| r.date).min();
        let max = self.rows.iter().map(|r| r.date).max();
        if self.start.is_none() {
            self.start = min;
        }
        if self.end.is_none() {
            self.end = max;
        }
    }
}

/// What kind of account a section is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccountKind {
    /// OFX `CHECKING`, QIF `Bank`.
    Checking,
    /// OFX `SAVINGS`.
    Savings,
    /// OFX `MONEYMRKT`.
    MoneyMarket,
    /// OFX `CREDITLINE`.
    CreditLine,
    /// OFX `CD`.
    Cd,
    /// OFX `CCACCTFROM`, QIF `CCard`.
    CreditCard,
    /// QIF `Cash`.
    Cash,
    /// The file does not say (CSV).
    Unknown,
}

impl AccountKind {
    /// Whether amounts follow the card rules (Q-196).
    #[must_use]
    pub fn is_card(self) -> bool {
        matches!(self, Self::CreditCard)
    }
}

/// The account number a file names, kept whole for the link hash
/// ([`crate::link::account_key_hash`]) but shown and printed masked only
/// (Q-154): `Debug` never prints the account number (plan §4).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AccountId {
    bank_id: Option<String>,
    acct_id: String,
}

impl AccountId {
    /// From the file's `BANKID` (none for cards) and `ACCTID`, trimmed.
    /// `None` when the account number is blank.
    #[must_use]
    pub fn new(bank_id: Option<&str>, acct_id: &str) -> Option<Self> {
        let acct = trim_ws(acct_id);
        if acct.is_empty() {
            return None;
        }
        let bank = bank_id.map(trim_ws).filter(|b| !b.is_empty());
        Some(Self {
            bank_id: bank.map(str::to_owned),
            acct_id: acct.to_owned(),
        })
    }

    /// The masked form shown to the owner: `••••` and the last 4
    /// characters, fewer for short numbers so at least half stays hidden
    /// (`••••` alone for 1 character).
    #[must_use]
    pub fn mask(&self) -> String {
        let chars: Vec<char> = self.acct_id.chars().collect();
        let shown = (chars.len() / 2).min(4);
        let tail: String = chars[chars.len() - shown..].iter().collect();
        format!("\u{2022}\u{2022}\u{2022}\u{2022}{tail}")
    }

    /// The routing number (`BANKID`), when the file has one. Only for the
    /// link hash; never display or log it.
    #[must_use]
    pub fn bank_id(&self) -> Option<&str> {
        self.bank_id.as_deref()
    }

    /// The full account number. Only for the link hash; never display or
    /// log it.
    #[must_use]
    pub fn acct_id(&self) -> &str {
        &self.acct_id
    }
}

impl fmt::Debug for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("AccountId").field(&self.mask()).finish()
    }
}

/// A balance the file states, in the file's sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Balance {
    /// The amount, exact, at most 2 decimals.
    pub amount: Decimal,
    /// The date it is as of, when the file says.
    pub date: Option<CalendarDate>,
}

/// One transaction, normalized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankRow {
    /// 1-based line the row starts on (the batch's `line`).
    pub line: usize,
    /// The posted date (OFX: the first 8 digits of `DTPOSTED`, no time-zone
    /// shift).
    pub date: CalendarDate,
    /// The amount exactly as the file has it (sign included), normalized
    /// to at most 2 decimals. Use [`crate::to_display`] and
    /// [`crate::to_feed_amount`] for the shown and sent signs.
    pub file_amount: Decimal,
    /// The bank's description: NFC, at most [`MAX_DESCRIPTION_CHARS`]
    /// characters. For OFX it is NAME + MEMO by the golden rule
    /// ([`crate::dedupe::ofx_description`]).
    pub description: String,
    /// A memo, at most [`MAX_MEMO_CHARS`] characters: the QIF/CSV memo,
    /// plus "Ref: …" for references that are not check numbers.
    pub memo: Option<String>,
    /// A check number of at most 9 digits.
    pub check_number: Option<u32>,
    /// The bank's transaction id (OFX `FITID`), trimmed.
    pub fitid: Option<String>,
    /// The running balance after this row (CSV), in the file's sign.
    pub running_balance: Option<Decimal>,
    /// The OFX `TRNTYPE` (for example `DEBIT`, `CHECK`), kept for the sign
    /// check and the review screen.
    pub trn_type: Option<String>,
}

/// A row left out on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkippedRow {
    /// 1-based line.
    pub line: usize,
    /// Why.
    pub reason: SkipReason,
}

/// Why a row was left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkipReason {
    /// The amount is zero (review S10): listed, never sent.
    ZeroAmount,
}

/// A row that could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Problem {
    /// 1-based line the row starts on.
    pub line: usize,
    /// What is wrong.
    pub kind: ProblemKind,
}

/// What is wrong with a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProblemKind {
    /// No amount at all.
    MissingAmount,
    /// The amount is not a number (letters, an exponent, two signs, a
    /// decimal comma).
    BadAmount,
    /// More than 2 decimal places after dropping trailing zeros (`12.345`).
    TooManyDecimals,
    /// More than [`MAX_INTEGER_DIGITS`] digits before the point.
    TooManyDigits,
    /// No date.
    MissingDate,
    /// The date cannot be read in any order.
    BadDate,
    /// The date reads but does not exist in the chosen order (`02/30`).
    ImpossibleDate,
    /// A CSV row with fewer columns than the mapping needs.
    MissingColumns,
    /// A debit/credit or indicator row whose direction cannot be told
    /// (both columns filled, or an unknown indicator).
    UnclearDirection,
}

/// How the bytes were decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    /// UTF-8 without a byte-order mark.
    Utf8,
    /// UTF-8 with a byte-order mark.
    Utf8Bom,
    /// UTF-16, little-endian, with a byte-order mark.
    Utf16Le,
    /// UTF-16, big-endian, with a byte-order mark.
    Utf16Be,
    /// Windows-1252 (also used for ASCII, `NONE` and ISO-8859-1, Q-P25h).
    Windows1252,
}

/// A remark about decoding the wizard may show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EncodingNote {
    /// The file declares a single-byte character set but its bytes are
    /// valid UTF-8 with non-ASCII characters, so it was read as UTF-8
    /// (Q-P25h).
    DeclaredSingleByteReadAsUtf8,
}

/// The bank named in an OFX/QFX sign-on block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Institution {
    /// `FI/ORG`.
    pub org: Option<String>,
    /// `FI/FID`.
    pub fid: Option<String>,
    /// `INTU.BID` (QFX).
    pub intu_bid: Option<String>,
}

// ---------------------------------------------------------------------------
// Decoding (Q-P25h)

/// Text decoded from a file's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// The text, without a byte-order mark.
    pub text: String,
    /// How it was decoded.
    pub encoding: TextEncoding,
    /// A remark, when one applies.
    pub note: Option<EncodingNote>,
}

/// A character set a file declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Declared {
    /// Nothing declared: UTF-8 when valid, otherwise Windows-1252 (CSV, QIF).
    Nothing,
    /// A single-byte set: `NONE`, ASCII, ISO-8859-1, Windows-1252.
    SingleByte,
    /// UTF-8.
    Utf8,
}

/// Maps a declared character set name to how it is read; `None` for a set
/// that is refused. Case and `-`/`_` are ignored.
#[must_use]
pub fn declared_charset(name: &str) -> Option<Declared> {
    let key: String = name
        .trim()
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | ' '))
        .map(|c| c.to_ascii_uppercase())
        .collect();
    match key.as_str() {
        "" | "NONE" | "USASCII" | "ASCII" | "ANSI" | "ISO88591" | "LATIN1" | "8859" | "88591"
        | "1252" | "CP1252" | "WINDOWS1252" => Some(Declared::SingleByte),
        "UTF8" | "UNICODE" | "CSUTF8" => Some(Declared::Utf8),
        _ => None,
    }
}

/// Decodes a file: the byte-order mark wins, then the declared set
/// (Q-P25h). A file declared single-byte whose bytes are valid UTF-8 is
/// read as UTF-8 (with a note when it has non-ASCII text).
///
/// # Errors
/// [`ImportError::BadText`] for UTF-16 with an odd byte count or unpaired
/// surrogates.
pub fn decode(bytes: &[u8], declared: Declared) -> Result<Decoded, ImportError> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return Ok(Decoded {
            text: utf8_or_1252(rest),
            encoding: TextEncoding::Utf8Bom,
            note: None,
        });
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return Ok(Decoded {
            text: utf16(rest, true)?,
            encoding: TextEncoding::Utf16Le,
            note: None,
        });
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return Ok(Decoded {
            text: utf16(rest, false)?,
            encoding: TextEncoding::Utf16Be,
            note: None,
        });
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => {
            let note = (declared == Declared::SingleByte && !s.is_ascii())
                .then_some(EncodingNote::DeclaredSingleByteReadAsUtf8);
            Ok(Decoded {
                text: s.to_owned(),
                encoding: TextEncoding::Utf8,
                note,
            })
        }
        Err(_) => Ok(Decoded {
            text: bytes.iter().map(|&b| crate::csv::cp1252(b)).collect(),
            encoding: TextEncoding::Windows1252,
            note: None,
        }),
    }
}

/// UTF-8 when valid, otherwise Windows-1252 (a BOM followed by 1252 bytes
/// happens with some exporters).
fn utf8_or_1252(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => bytes.iter().map(|&b| crate::csv::cp1252(b)).collect(),
    }
}

fn utf16(bytes: &[u8], little: bool) -> Result<String, ImportError> {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| {
            if little {
                u16::from_le_bytes(c)
            } else {
                u16::from_be_bytes(c)
            }
        })
        .collect();
    let mut out = String::with_capacity(units.len());
    let mut line = 1;
    for c in char::decode_utf16(units) {
        let c = c.map_err(|_| ImportError::BadText { line })?;
        if c == '\n' {
            line += 1;
        }
        out.push(c);
    }
    if !bytes.len().is_multiple_of(2) {
        return Err(ImportError::BadText { line });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Amounts (plan §4)

/// Why an amount could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmountProblem {
    /// Blank.
    Missing,
    /// Not a number.
    Bad,
    /// More than 2 decimals.
    TooManyDecimals,
    /// More than 12 integer digits.
    TooManyDigits,
}

impl From<AmountProblem> for ProblemKind {
    fn from(p: AmountProblem) -> Self {
        match p {
            AmountProblem::Missing => Self::MissingAmount,
            AmountProblem::Bad => Self::BadAmount,
            AmountProblem::TooManyDecimals => Self::TooManyDecimals,
            AmountProblem::TooManyDigits => Self::TooManyDigits,
        }
    }
}

/// Reads an amount exactly, through `cl-core` money.
///
/// Accepts a leading or trailing minus (`-45.00`, `45.00-`), U+2212 `−`,
/// parentheses `(45.00)`, a `$`, a `+`, and US thousands groups
/// (`1,234.56`). Trailing zeros after the point are dropped first, so
/// `45.1000` is 45.10. Refused: more than 2 decimals, more than 12 integer
/// digits, exponents, decimal commas, letters.
///
/// # Errors
/// The [`AmountProblem`] that makes the text unusable.
pub fn parse_amount(text: &str) -> Result<Decimal, AmountProblem> {
    let t = text.trim_matches(is_ws);
    if t.is_empty() {
        return Err(AmountProblem::Missing);
    }
    // Long inputs (a million-digit TRNAMT) stop here, before any work that
    // grows with the digits beyond one scan.
    if t.len() > 64 {
        return if t
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'.' || b == b',' || b == b'-')
        {
            Err(AmountProblem::TooManyDigits)
        } else {
            Err(AmountProblem::Bad)
        };
    }
    let mut s: String = t.replace('\u{2212}', "-");
    if let Some(rest) = s.strip_prefix('+') {
        s = rest.trim_start().to_owned();
    }
    // A trailing minus (`45.00-`) becomes a leading one.
    if let Some(rest) = s.strip_suffix('-') {
        if rest.starts_with('-') || rest.starts_with('(') {
            return Err(AmountProblem::Bad);
        }
        s = format!("-{}", rest.trim_end());
    }
    // Drop zeros after the last fraction digit that matters: `45.1000`.
    let s = strip_fraction_zeros(&s);
    let money = Money::parse_user(&s, UserInput::FULL).map_err(|e| match e {
        cl_core::MoneyInputError::TooManyDecimals { .. } => AmountProblem::TooManyDecimals,
        cl_core::MoneyInputError::Empty => AmountProblem::Missing,
        cl_core::MoneyInputError::TooLarge => AmountProblem::TooManyDigits,
        _ => AmountProblem::Bad,
    })?;
    let d = money.as_decimal().normalize();
    if d.scale() > 2 {
        return Err(AmountProblem::TooManyDecimals);
    }
    let int_digits = d.trunc().abs().to_string().trim_start_matches('0').len();
    if int_digits > MAX_INTEGER_DIGITS {
        return Err(AmountProblem::TooManyDigits);
    }
    Ok(d)
}

/// `"(1,234.5000)"` → `"(1,234.5)"`; text without a point is unchanged.
fn strip_fraction_zeros(s: &str) -> String {
    let Some(dot) = s.find('.') else {
        return s.to_owned();
    };
    let digits_end = s[dot + 1..]
        .find(|c: char| !c.is_ascii_digit())
        .map_or(s.len(), |i| dot + 1 + i);
    let frac = s[dot + 1..digits_end].trim_end_matches('0');
    let frac = if frac.is_empty() { "0" } else { frac };
    format!("{}.{}{}", &s[..dot], frac, &s[digits_end..])
}

/// Formats a file amount the way [`parse_amount`] reads it back: plain,
/// minus first, at most 2 decimals (for tests and the demo).
#[must_use]
pub fn format_amount(d: Decimal) -> String {
    d.normalize().to_string()
}

// ---------------------------------------------------------------------------
// Text

/// The whitespace set of the engine's `norm` (golden rules): U+0009–U+000D,
/// U+0020 and U+00A0. Not `char::is_whitespace`.
#[must_use]
pub fn is_ws(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000D}' | '\u{0020}' | '\u{00A0}')
}

/// Trims [`is_ws`] characters from both ends.
#[must_use]
pub fn trim_ws(s: &str) -> &str {
    s.trim_matches(is_ws)
}

/// Cleans bank text: NFC, control characters other than tab/line breaks
/// dropped, trimmed, cut to `max` characters.
#[must_use]
pub fn clean_text(s: &str, max: usize) -> String {
    let nfc: String = s.nfc().filter(|c| !c.is_control() || is_ws(*c)).collect();
    trim_ws(&nfc).chars().take(max).collect()
}

/// Reads a check number: up to 9 digits (leading zeros allowed) is a check
/// number; anything else non-blank is a reference for the memo.
#[must_use]
pub fn check_or_ref(raw: &str) -> (Option<u32>, Option<String>) {
    let t = trim_ws(raw);
    if t.is_empty() {
        return (None, None);
    }
    let digits = t.trim_start_matches('0');
    if t.bytes().all(|b| b.is_ascii_digit()) && digits.len() <= MAX_CHECK_DIGITS {
        if let Ok(n) = digits.parse::<u32>() {
            return (Some(n), None);
        }
        if digits.is_empty() {
            return (Some(0), None);
        }
    }
    (None, Some(t.chars().take(100).collect()))
}

/// Joins a memo and an optional "Ref: …" note, cleaned and capped.
#[must_use]
pub fn memo_with_ref(memo: Option<&str>, reference: Option<&str>) -> Option<String> {
    let memo = memo
        .map(|m| clean_text(m, MAX_MEMO_CHARS))
        .filter(|m| !m.is_empty());
    let reference = reference.map(|r| format!("Ref: {}", clean_text(r, 100)));
    let joined = match (memo, reference) {
        (None, None) => return None,
        (Some(m), None) => m,
        (None, Some(r)) => r,
        (Some(m), Some(r)) => format!("{m} {r}"),
    };
    Some(joined.chars().take(MAX_MEMO_CHARS).collect())
}

/// Builds a row or a problem from raw parts shared by every parser, and
/// files it into the section: zero amounts are skipped (review S10).
pub(crate) struct RowParts<'a> {
    pub line: usize,
    pub date: Result<CalendarDate, ProblemKind>,
    pub amount: Result<Decimal, AmountProblem>,
    pub description: &'a str,
    pub memo: Option<&'a str>,
    pub check: Option<&'a str>,
    pub fitid: Option<&'a str>,
    pub running_balance: Option<Decimal>,
    pub trn_type: Option<&'a str>,
}

impl Section {
    /// Files one row: a problem, a skipped zero, or a [`BankRow`].
    pub(crate) fn push_parts(&mut self, p: &RowParts<'_>) {
        let amount = match p.amount {
            Ok(a) => a,
            Err(e) => {
                self.problems.push(Problem {
                    line: p.line,
                    kind: e.into(),
                });
                return;
            }
        };
        let date = match p.date {
            Ok(d) => d,
            Err(kind) => {
                self.problems.push(Problem { line: p.line, kind });
                return;
            }
        };
        if amount.is_zero() {
            self.skipped.push(SkippedRow {
                line: p.line,
                reason: SkipReason::ZeroAmount,
            });
            return;
        }
        let (check_number, reference) = p.check.map_or((None, None), check_or_ref);
        self.rows.push(BankRow {
            line: p.line,
            date,
            file_amount: amount,
            description: clean_text(p.description, MAX_DESCRIPTION_CHARS),
            memo: memo_with_ref(p.memo, reference.as_deref()),
            check_number,
            fitid: p
                .fitid
                .map(trim_ws)
                .filter(|f| !f.is_empty())
                .map(str::to_owned),
            running_balance: p.running_balance,
            trn_type: p
                .trn_type
                .map(trim_ws)
                .filter(|t| !t.is_empty())
                .map(str::to_ascii_uppercase),
        });
    }

    /// Refuses a section over [`MAX_ROWS`].
    pub(crate) fn check_row_limit(&self) -> Result<(), ImportError> {
        if self.rows.len() > MAX_ROWS {
            return Err(ImportError::TooManyRows { limit: MAX_ROWS });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
