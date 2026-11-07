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
    Section, to_display, to_feed_amount,
};
use cl_settings::SecretStore;
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
    /// What the reader found.
    pub parsed: ParsedFile,
    /// Per section: the mask and keyed hash of the account number, when
    /// the file names one and the link key could be read.
    pub identities: Vec<Option<BankIdentity>>,
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
        ImportError::UnsupportedCharset { .. } | ImportError::BadText { .. } => {
            "bankimport.error.charset"
        }
        ImportError::Doctype { .. }
        | ImportError::Entity { .. }
        | ImportError::UnknownEntity { .. } => "bankimport.error.refused_markup",
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
        P::TooManyDecimals => "bankimport.problem.decimals",
        P::TooManyDigits => "bankimport.problem.digits",
        P::MissingDate => "bankimport.problem.missing_date",
        P::BadDate => "bankimport.problem.bad_date",
        P::ImpossibleDate => "bankimport.problem.impossible_date",
        P::MissingColumns => "bankimport.problem.columns",
        P::UnclearDirection => "bankimport.problem.direction",
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

fn hex_bytes(hex: &str) -> Option<Vec<u8>> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

/// The identity of each section: its mask, and the keyed hash when a link
/// key is at hand (`key`: the company's 64-hex keychain value).
#[must_use]
pub fn identities(parsed: &ParsedFile, key: Option<&str>) -> Vec<Option<BankIdentity>> {
    let key = key.and_then(hex_bytes);
    parsed
        .sections
        .iter()
        .map(|s| {
            let id = s.account.as_ref()?;
            let key = key.as_ref()?;
            Some(BankIdentity {
                mask: id.mask(),
                account_key_hash: cl_import::link::account_key_hash(key, id),
            })
        })
        .collect()
}

/// Reads and parses `path` (on a worker thread). The link key is read from
/// `secrets` under `link_id` only when the file names an account.
///
/// # Errors
/// [`ReadFail`] when the file can't be opened, is too large or can't be read.
pub fn read_file(
    path: &Path,
    today: CalendarDate,
    mapping: Option<CsvMapping>,
    secrets: Option<(Arc<dyn SecretStore>, uuid::Uuid)>,
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
    parse_bytes(file_name, Arc::new(bytes), today, mapping, secrets)
}

/// Parses `bytes` (a file already read, or read again with a new mapping).
///
/// # Errors
/// [`ReadFail`] when the reader refuses the file or it has no transactions.
pub fn parse_bytes(
    file_name: String,
    bytes: Arc<Vec<u8>>,
    today: CalendarDate,
    mapping: Option<CsvMapping>,
    secrets: Option<(Arc<dyn SecretStore>, uuid::Uuid)>,
) -> Result<Loaded, ReadFail> {
    let mut options = ParseOptions::new(today);
    options.csv_mapping = mapping;
    let parsed = cl_import::parse(&bytes, &options).map_err(ReadFail::Import)?;
    if parsed
        .sections
        .iter()
        .all(|s| s.rows.is_empty() && s.problems.is_empty() && s.skipped.is_empty())
    {
        return Err(ReadFail::NoTransactions);
    }
    let key = if parsed.sections.iter().any(|s| s.account.is_some()) {
        secrets.and_then(|(store, id)| cl_settings::link_key(store.as_ref(), id).ok())
    } else {
        None
    };
    let identities = identities(&parsed, key.as_ref().map(cl_core::Secret::expose));
    Ok(Loaded {
        file_name,
        sha256: sha256_hex(&bytes),
        bytes,
        parsed,
        identities,
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
        bank_identity: loaded.identities.get(section.index).cloned().flatten(),
        file_ending_balance: balance.map(|b| b.0),
        file_ending_date: balance.map(|b| b.1),
        rows,
    }
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

/// Whether the owner must confirm the sign before going on (Q-196): the
/// reader suggests a flip, or it is a card file the account has no earlier
/// import of.
#[must_use]
pub fn sign_needs_confirm(section: &Section, remembered: Option<bool>) -> bool {
    remembered.is_none() && (section.sign.suggest_flip || section.kind == AccountKind::CreditCard)
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
        assert_eq!(hex_bytes("0aff"), Some(vec![10, 255]));
        assert_eq!(hex_bytes("zz"), None);
    }

    #[test]
    fn every_reader_error_has_owner_text() {
        for e in [
            ImportError::Empty,
            ImportError::UnknownFormat,
            ImportError::Doctype { line: 3 },
            ImportError::Truncated { line: 9 },
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
    }
}
