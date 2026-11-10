//! The wizard's pure parts (P25): reading a file off the egui thread,
//! building the batch the engine gets, and the texts of file errors and
//! engine refusals. Nothing here draws or blocks.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use cl_api::{
    BankFileFormat, BankIdentity, BankImportOutcome, BankImportRefusal, BankImportRequest,
    BankImportRow, ChangedAction, ImportRowStatus,
};
use cl_core::ids::AccountId;
use cl_core::{CalendarDate, Money};
use cl_import::{
    AccountKind, Continuity, CsvMapping, FileFormat, ImportError, ParseOptions, ParsedFile,
    Section, SignCheck, check_as, to_display, to_feed_amount,
};
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};

/// The owner's choice for one previewed row (by its line).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowChoice {
    /// `Some(false)`: leave a new row out; `Some(true)`: add a possible
    /// duplicate anyway.
    pub include: Option<bool>,
    /// What to do with a row the bank changed.
    pub changed: Option<ChangedAction>,
}

/// A file read and parsed, with each section's link identity.
#[derive(Clone)]
pub struct Loaded {
    /// The base file name (owner data: never logged or in state).
    pub file_name: String,
    /// SHA-256 of the bytes, lower-case hex.
    pub sha256: String,
    /// The bytes (kept to read a CSV again with other columns).
    pub bytes: Arc<Vec<u8>>,
    /// What the reader found. A section's account number stays in here
    /// (never logged: its `Debug` is masked) and goes to the local engine
    /// only, which keeps the link key (Q-P25u).
    pub parsed: ParsedFile,
}

impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loaded")
            .field("sections", &self.parsed.sections.len())
            .finish_non_exhaustive()
    }
}

/// Why a file could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadFail {
    /// The file could not be opened.
    Open,
    /// Over 25 MB.
    TooLarge,
    /// The reader refused it.
    Import(ImportError),
    /// It read, but holds no transactions and no problems.
    NoTransactions,
}

impl ReadFail {
    /// The owner's text (the problem only; the caller adds the line).
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            ReadFail::Import(e) => match e.charset() {
                Some(cs) => crate::i18n::t_with(import_error_key(e), &[("charset", cs)]),
                None => crate::i18n::t(import_error_key(e)),
            },
            _ => crate::i18n::t(self.key().0),
        }
    }

    /// The i18n key and the line it points at.
    #[must_use]
    pub fn key(&self) -> (&'static str, Option<usize>) {
        match self {
            ReadFail::Open => ("bankimport.error.open", None),
            ReadFail::TooLarge => ("bankimport.error.too_large", None),
            ReadFail::NoTransactions => ("bankimport.error.no_transactions", None),
            ReadFail::Import(e) => (import_error_key(e), e.line()),
        }
    }
}

/// The i18n key of a file-level reader error.
#[must_use]
pub fn import_error_key(e: &ImportError) -> &'static str {
    match e {
        ImportError::Empty => "bankimport.error.empty",
        ImportError::TooLarge { .. } => "bankimport.error.too_large",
        ImportError::UnknownFormat => "bankimport.error.unknown_format",
        ImportError::UnsupportedCharset { .. } => "bankimport.error.charset_named",
        ImportError::BadText { .. } => "bankimport.error.charset",
        ImportError::Doctype { .. }
        | ImportError::Entity { .. }
        | ImportError::UnknownEntity { .. } => "bankimport.error.refused_markup",
        ImportError::TooManyElements { .. } => "bankimport.error.too_many_elements",
        ImportError::TooDeep { .. }
        | ImportError::TokenTooLong { .. }
        | ImportError::BadMarkup { .. } => "bankimport.error.bad_markup",
        ImportError::Truncated { .. } => "bankimport.error.truncated",
        ImportError::BankError { .. } => "bankimport.error.bank_error",
        ImportError::Investment { .. } => "bankimport.error.investment",
        ImportError::UnsupportedQifType { .. } => "bankimport.error.qif_type",
        ImportError::NoStatement => "bankimport.error.no_statement",
        ImportError::TooManyRows { .. } => "bankimport.error.too_many_rows",
        ImportError::CsvNeedsMapping => "bankimport.error.csv_mapping",
        ImportError::MappingDoesNotFit => "bankimport.error.mapping_fit",
        _ => "bankimport.error.unreadable",
    }
}

/// The i18n key of a row problem.
#[must_use]
pub fn problem_key(p: cl_import::ProblemKind) -> &'static str {
    use cl_import::ProblemKind as P;
    match p {
        P::MissingAmount => "bankimport.problem.missing_amount",
        P::BadAmount => "bankimport.problem.bad_amount",
        P::TooManyDigits => "bankimport.problem.digits",
        P::MissingDate => "bankimport.problem.missing_date",
        P::BadDate => "bankimport.problem.bad_date",
        P::ImpossibleDate => "bankimport.problem.impossible_date",
        P::MissingColumns => "bankimport.problem.columns",
        P::UnclearDirection => "bankimport.problem.direction",
        P::BelowOneCent => "bankimport.problem.below_cent",
        _ => "bankimport.problem.other",
    }
}

/// The i18n key of an engine refusal.
#[must_use]
pub fn refusal_key(r: &BankImportRefusal) -> &'static str {
    match r {
        BankImportRefusal::NotBankOrCard => "bankimport.refused.not_bank",
        BankImportRefusal::NotInCompany => "bankimport.refused.not_in_company",
        BankImportRefusal::InvalidRow { .. } => "bankimport.refused.row",
        BankImportRefusal::TooManyRows => "bankimport.error.too_many_rows",
        BankImportRefusal::TotalTooLarge => "bankimport.refused.total",
        BankImportRefusal::LinkedElsewhere(_) => "bankimport.linked_elsewhere",
        BankImportRefusal::RowsAlreadyUsed(_) => "bankimport.undo.used",
        BankImportRefusal::StillPreparing => "bankimport.undo.preparing",
        BankImportRefusal::AlreadyUndone => "bankimport.undo.already",
    }
}

/// The engine's name of a file format.
#[must_use]
pub fn wire_format(f: FileFormat) -> BankFileFormat {
    match f {
        FileFormat::Ofx => BankFileFormat::Ofx,
        FileFormat::Qfx => BankFileFormat::Qfx,
        FileFormat::Qif => BankFileFormat::Qif,
        FileFormat::Csv => BankFileFormat::Csv,
    }
}

/// SHA-256 of `bytes`, lower-case hex.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// The account a section names, to send to the local engine (Q-P25u).
#[must_use]
pub fn identity(section: &Section) -> Option<BankIdentity> {
    let id = section.account.as_ref()?;
    Some(BankIdentity {
        bank_id: id.bank_id().map(str::to_owned),
        acct_id: id.acct_id().to_owned(),
    })
}

/// Reads and parses `path` (on a worker thread).
///
/// # Errors
/// [`ReadFail`] when the file can't be opened, is too large or can't be read.
pub fn read_file(
    path: &Path,
    today: CalendarDate,
    mapping: Option<CsvMapping>,
) -> Result<Loaded, ReadFail> {
    let size = std::fs::metadata(path).map_err(|_| ReadFail::Open)?.len();
    if size > u64::try_from(cl_import::MAX_FILE_BYTES).unwrap_or(u64::MAX) {
        return Err(ReadFail::TooLarge);
    }
    let bytes = std::fs::read(path).map_err(|_| ReadFail::Open)?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    parse_bytes(file_name, Arc::new(bytes), today, mapping, false)
}

/// Parses `bytes` (a file already read, or read again with a new mapping).
/// A `remembered` mapping is used only when it fits the file (review DB2);
/// the parsed file's `csv_mapping` says which mapping was used.
///
/// # Errors
/// [`ReadFail`] when the reader refuses the file or it has no transactions.
pub fn parse_bytes(
    file_name: String,
    bytes: Arc<Vec<u8>>,
    today: CalendarDate,
    mapping: Option<CsvMapping>,
    remembered: bool,
) -> Result<Loaded, ReadFail> {
    let mut options = ParseOptions::new(today);
    options.csv_mapping = mapping;
    options.csv_mapping_remembered = remembered;
    let parsed = cl_import::parse(&bytes, &options).map_err(ReadFail::Import)?;
    if parsed
        .sections
        .iter()
        .all(|s| s.rows.is_empty() && s.problems.is_empty() && s.skipped.is_empty())
    {
        return Err(ReadFail::NoTransactions);
    }
    Ok(Loaded {
        file_name,
        sha256: sha256_hex(&bytes),
        bytes,
        parsed,
    })
}

/// The file's ending balance to send, in the holder's sign: none when the
/// file has none or a CSV running balance breaks (Q-P25l).
#[must_use]
pub fn ending_balance(section: &Section, flipped: bool) -> Option<(Decimal, CalendarDate)> {
    if matches!(section.sign.continuity, Continuity::Broken { .. }) {
        return None;
    }
    let b = section.ledger_balance?;
    let date = b.date.or(section.end)?;
    Some((to_display(b.amount, flipped), date))
}

/// The balance check's three figures as the owner reads them, in one sign
/// convention (QA-3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BalanceFigures {
    /// What the bank's file says.
    pub bank: Money,
    /// What the books will say once the rows are accepted.
    pub books: Money,
    /// `bank` minus `books`.
    pub difference: Money,
}

/// The engine's balance check in one sign: a bank account's balance as
/// is; a card's as the amount owed. The engine echoes the file's balance
/// in the holder's sign (a card's owed balance is negative there) while its
/// projection and difference are amounts owed (U-06: `difference =
/// -fileBalance - projected`), so a card's file balance is negated here.
#[must_use]
pub fn balance_figures(b: &cl_api::ImportBalanceCheck, card: bool) -> BalanceFigures {
    let bank = if card {
        b.file_balance.checked_neg().unwrap_or(b.file_balance)
    } else {
        b.file_balance
    };
    BalanceFigures {
        bank,
        books: b.projected,
        difference: b.difference,
    }
}

/// QA R2-5: why a CSV layout can't be used as chosen (an i18n key), so
/// Next being off is explained: money in and money out (or the amount and
/// its debit/credit column) on one column, or the date on an amount column.
#[must_use]
pub fn mapping_clash(m: &CsvMapping) -> Option<&'static str> {
    use cl_import::AmountColumns as A;
    match m.amount {
        A::DebitCredit { debit, credit } if debit == credit => {
            return Some("bankimport.csv.same_in_out");
        }
        A::Indicator { amount, indicator } if amount == indicator => {
            return Some("bankimport.csv.same_indicator");
        }
        _ => {}
    }
    let amount_columns = match m.amount {
        A::Signed { column } => vec![column],
        A::DebitCredit { debit, credit } => vec![debit, credit],
        A::Indicator { amount, indicator } => vec![amount, indicator],
    };
    amount_columns
        .contains(&m.date)
        .then_some("bankimport.csv.same_date")
}

/// The batch for `section` going into `account`.
#[must_use]
#[allow(clippy::too_many_arguments, reason = "one batch has this many parts")]
pub fn build_request(
    loaded: &Loaded,
    section: &Section,
    account: AccountId,
    flipped: bool,
    csv_mapping: Option<&CsvMapping>,
    relink: bool,
    choices: &BTreeMap<u32, RowChoice>,
) -> BankImportRequest {
    let rows = section
        .rows
        .iter()
        .map(|r| {
            let line = u32::try_from(r.line).unwrap_or(u32::MAX);
            let choice = choices.get(&line).copied().unwrap_or_default();
            BankImportRow {
                line,
                date: r.date,
                amount: to_feed_amount(r.file_amount, flipped),
                description: r.description.clone(),
                memo: r.memo.clone(),
                check_number: r.check_number,
                fitid: r.fitid.clone(),
                include: choice.include,
                changed_action: choice.changed,
            }
        })
        .collect();
    let balance = ending_balance(section, flipped);
    BankImportRequest {
        account,
        format: wire_format(loaded.parsed.format),
        file_name: loaded.file_name.clone(),
        source_file_sha256: loaded.sha256.clone(),
        section_index: u32::try_from(section.index).unwrap_or(0),
        section_start: section.start,
        section_end: section.end,
        amounts_flipped: flipped,
        csv_mapping: csv_mapping.and_then(|m| serde_json::to_value(m).ok()),
        relink_identity: relink,
        bank_identity: identity(section),
        file_ending_balance: balance.map(|b| b.0),
        file_ending_date: balance.map(|b| b.1),
        rows,
    }
}

/// Q-PR5b: the new rows of a preview with the same date and amount (feed
/// sign) as a row already imported into the account. An edited re-import
/// (descriptions changed in a spreadsheet) or a "pending then posted"
/// download shows here before it is sent; nothing is flagged by it.
#[must_use]
pub fn same_day_amount(
    outcome: &BankImportOutcome,
    rows: &[cl_api::BankImportRow],
    existing: &[(CalendarDate, Decimal)],
) -> usize {
    outcome
        .rows
        .iter()
        .filter(|r| r.status == ImportRowStatus::New)
        .filter_map(|r| rows.iter().find(|x| x.line == r.line))
        .filter(|x| existing.iter().any(|(d, a)| *d == x.date && *a == x.amount))
        .count()
}

/// The rows a commit adds, given the owner's choices.
#[must_use]
pub fn rows_to_send(outcome: &BankImportOutcome, choices: &BTreeMap<u32, RowChoice>) -> usize {
    outcome
        .rows
        .iter()
        .filter(|r| {
            let c = choices.get(&r.line).copied().unwrap_or_default();
            match r.status {
                // Q-P25c: look-alikes are added with a flag, like new rows.
                ImportRowStatus::New | ImportRowStatus::PossibleDuplicate => {
                    c.include != Some(false)
                }
                ImportRowStatus::Changed => c.changed == Some(ChangedAction::Add),
                ImportRowStatus::Duplicate => false,
            }
        })
        .count()
}

/// A file amount as the owner sees it (money out negative).
#[must_use]
pub fn shown(file_amount: Decimal, flipped: bool) -> Option<Money> {
    Money::parse_wire(&cl_api::wire_decimal(to_display(file_amount, flipped)).ok()?).ok()
}

/// The sign checks for `section` going into the chosen account (Q-196,
/// review DB1): a file that does not say it is a card's (CSV, a bank QIF)
/// gets the card rule once a card account is chosen.
#[must_use]
pub fn sign_check(section: &Section, card_target: bool) -> SignCheck {
    if card_target && !section.kind.is_card() {
        check_as(section, AccountKind::CreditCard)
    } else {
        section.sign
    }
}

/// Whether the owner must confirm the sign before going on (Q-196,
/// Q-P25t): with no earlier import of this account and format, when the
/// file looks reversed, and always for a card account (asked once; the
/// answer is kept with the batch).
///
/// QA-4: the remembered answer is not applied blindly. The sign evidence
/// runs on every file; when it has something to go on and says the other
/// way, the owner is asked again.
#[must_use]
pub fn sign_needs_confirm(check: &SignCheck, card_target: bool, remembered: Option<bool>) -> bool {
    match remembered {
        None => check.suggest_flip || card_target,
        Some(flip) => sign_disagrees(check, flip),
    }
}

/// Whether this file's sign evidence says the other way from the
/// remembered flip (QA-4). No evidence (`reason` none) never disagrees.
#[must_use]
pub fn sign_disagrees(check: &SignCheck, remembered_flip: bool) -> bool {
    check.reason.is_some() && check.suggest_flip != remembered_flip
}

/// QA-5: the file says what kind of account it is from (OFX
/// `BANKACCTFROM`/`CCACCTFROM`, QIF `!Type:Bank`/`CCard`/`Cash`) and the
/// chosen account is the other kind. A file that does not say (CSV) or a
/// credit line never warns. Returns the warning's i18n key.
#[must_use]
pub fn type_mismatch(kind: AccountKind, card_target: bool) -> Option<&'static str> {
    match kind {
        AccountKind::CreditCard if !card_target => Some("bankimport.type.card_file"),
        AccountKind::Checking
        | AccountKind::Savings
        | AccountKind::MoneyMarket
        | AccountKind::Cd
        | AccountKind::Cash
            if card_target =>
        {
            Some("bankimport.type.bank_file")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_lower_hex() {
        assert_eq!(sha256_hex(b"").len(), 64);
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn every_reader_error_has_owner_text() {
        for e in [
            ImportError::Empty,
            ImportError::UnknownFormat,
            ImportError::Doctype { line: 3 },
            ImportError::Truncated { line: 9 },
            ImportError::TooManyElements { line: 9 },
            ImportError::UnsupportedCharset {
                line: 2,
                charset: "KOI8-R".into(),
            },
            ImportError::Investment { line: 1 },
            ImportError::CsvNeedsMapping,
        ] {
            let key = import_error_key(&e);
            assert!(crate::i18n::has(key), "{key}");
        }
        assert_eq!(
            ReadFail::Import(ImportError::Doctype { line: 3 }).key().1,
            Some(3)
        );
        // Q-P25h: the refused charset is named, with what to do instead.
        let text = ReadFail::Import(ImportError::UnsupportedCharset {
            line: 2,
            charset: "KOI8-R".into(),
        })
        .text();
        assert!(text.contains("KOI8-R") && text.contains("UTF-8"), "{text}");
    }

    /// Every row problem and every engine refusal has owner text.
    #[test]
    fn every_problem_and_refusal_has_owner_text() {
        use cl_import::ProblemKind as P;
        for p in [
            P::MissingAmount,
            P::BadAmount,
            P::TooManyDigits,
            P::MissingDate,
            P::BadDate,
            P::ImpossibleDate,
            P::MissingColumns,
            P::UnclearDirection,
            P::BelowOneCent,
        ] {
            let key = problem_key(p);
            assert_ne!(key, "bankimport.problem.other", "{p:?} has its own text");
            assert!(crate::i18n::has(key), "{key}");
        }
        for r in [
            BankImportRefusal::NotBankOrCard,
            BankImportRefusal::NotInCompany,
            BankImportRefusal::InvalidRow {
                line: Some(4),
                field: None,
            },
            BankImportRefusal::TooManyRows,
            BankImportRefusal::TotalTooLarge,
            BankImportRefusal::LinkedElsewhere(cl_api::LinkedElsewhere {
                account_name: String::new(),
                mask: String::new(),
            }),
            BankImportRefusal::RowsAlreadyUsed(vec![3]),
            BankImportRefusal::StillPreparing,
            BankImportRefusal::AlreadyUndone,
        ] {
            let key = refusal_key(&r);
            assert!(crate::i18n::has(key), "{key}");
        }
        for e in [
            ImportError::Empty,
            ImportError::TooLarge { limit_bytes: 1 },
            ImportError::UnknownFormat,
            ImportError::BadText { line: 1 },
            ImportError::Entity { line: 1 },
            ImportError::UnknownEntity { line: 1 },
            ImportError::TooDeep { line: 1 },
            ImportError::TokenTooLong { line: 1 },
            ImportError::BadMarkup { line: 1 },
            ImportError::BankError {
                line: 1,
                code: "2000".into(),
            },
            ImportError::UnsupportedQifType { line: 1 },
            ImportError::NoStatement,
            ImportError::TooManyRows { limit: 1 },
            ImportError::MappingDoesNotFit,
        ] {
            let key = import_error_key(&e);
            assert!(crate::i18n::has(key), "{key}");
        }
        for k in [
            "bankimport.error.open",
            "bankimport.error.no_transactions",
            "bankimport.forbidden",
            "bankimport.undo.forbidden",
            "bankimport.done.prep_wait",
            "bankimport.done.prep_slow",
            "bankimport.skipped_preamble",
            "bankimport.skipped_opening",
        ] {
            assert!(crate::i18n::has(k), "{k}");
        }
    }
}
