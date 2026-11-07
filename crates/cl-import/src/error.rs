//! Why a whole bank file could not be read.
//!
//! Every variant that comes from a place in the file carries its 1-based
//! line. Messages never contain amounts, names, memos or account numbers
//! (no PII in logs); the wizard maps each variant to its own i18n text.

use thiserror::Error;

/// A file-level failure. Row-level trouble is a [`crate::Problem`] instead.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ImportError {
    /// The file has no bytes, or nothing but blank space.
    #[error("the file is empty")]
    Empty,
    /// Larger than [`crate::MAX_FILE_BYTES`].
    #[error("the file is larger than {limit_bytes} bytes")]
    TooLarge {
        /// The limit, in bytes.
        limit_bytes: usize,
    },
    /// Not OFX, QFX, QIF or CSV.
    #[error("the file is not a bank file this app can read (OFX, QFX, QIF or CSV)")]
    UnknownFormat,
    /// A character set other than the accepted ones (Q-P25h).
    #[error("line {line}: the file uses a character set this app cannot read")]
    UnsupportedCharset {
        /// Line of the declaration.
        line: usize,
    },
    /// Bytes that are not valid in the file's character set (UTF-16 only;
    /// single-byte sets accept every byte).
    #[error("line {line}: the file has characters that cannot be read")]
    BadText {
        /// Line of the bad bytes.
        line: usize,
    },
    /// `<!DOCTYPE …>` in an OFX file (refused, review S12).
    #[error("line {line}: the file has a document type declaration, which is not allowed")]
    Doctype {
        /// Line of the declaration.
        line: usize,
    },
    /// `<!ENTITY …>` in an OFX file (refused, review S12).
    #[error("line {line}: the file defines its own entities, which is not allowed")]
    Entity {
        /// Line of the declaration.
        line: usize,
    },
    /// An `&name;` reference other than the five standard XML entities, in
    /// an OFX 2 (XML) file.
    #[error("line {line}: the file uses an unknown entity")]
    UnknownEntity {
        /// Line of the reference.
        line: usize,
    },
    /// Markup nested deeper than [`crate::ofx::MAX_DEPTH`].
    #[error("line {line}: the file is nested too deeply")]
    TooDeep {
        /// Line of the tag that went too deep.
        line: usize,
    },
    /// A tag name or value longer than [`crate::ofx::MAX_TOKEN_BYTES`].
    #[error("line {line}: the file has a value that is too long")]
    TokenTooLong {
        /// Line the value starts on.
        line: usize,
    },
    /// The file ends inside a tag, or with a block still open.
    #[error("line {line}: the file ends too early; it may be cut off")]
    Truncated {
        /// Line of the innermost block that never closes.
        line: usize,
    },
    /// Markup that cannot be read (a broken tag, a closing tag with no
    /// matching opening tag).
    #[error("line {line}: the file has markup that cannot be read")]
    BadMarkup {
        /// Line of the broken markup.
        line: usize,
    },
    /// The bank put an error in the file instead of transactions (a
    /// `SEVERITY` of `ERROR`).
    #[error("line {line}: the bank reported an error (code {code}) instead of transactions")]
    BankError {
        /// Line of the `SEVERITY`.
        line: usize,
        /// The bank's status code, as written in the file (digits only).
        code: String,
    },
    /// An investment statement (OFX `INVSTMTRS`, QIF `!Type:Invst`).
    #[error("line {line}: investment statements cannot be imported into a bank account")]
    Investment {
        /// Line of the statement header.
        line: usize,
    },
    /// A QIF section type that is not `Bank`, `CCard` or `Cash` (for example
    /// `Oth A`, `Oth L`, `Invoice`).
    #[error("line {line}: this kind of QIF account cannot be imported")]
    UnsupportedQifType {
        /// Line of the `!Type:` header.
        line: usize,
    },
    /// An OFX file with no bank or card statement in it.
    #[error("the file has no bank or card statement")]
    NoStatement,
    /// One section has more than [`crate::MAX_ROWS`] rows.
    #[error("the file has more than {limit} transactions for one account")]
    TooManyRows {
        /// The limit.
        limit: usize,
    },
    /// The CSV reader refused the file.
    #[error("{0}")]
    Csv(#[from] crate::csv::CsvError),
    /// No date or amount column could be guessed; the owner has to map the
    /// columns.
    #[error("choose which columns hold the date and the amount")]
    CsvNeedsMapping,
    /// The CSV mapping names a column the file does not have.
    #[error("the column choices do not fit this file")]
    MappingDoesNotFit,
}

impl ImportError {
    /// The 1-based line the error points at, when it has one.
    #[must_use]
    pub fn line(&self) -> Option<usize> {
        match self {
            Self::UnsupportedCharset { line }
            | Self::BadText { line }
            | Self::Doctype { line }
            | Self::Entity { line }
            | Self::UnknownEntity { line }
            | Self::TooDeep { line }
            | Self::TokenTooLong { line }
            | Self::Truncated { line }
            | Self::BadMarkup { line }
            | Self::BankError { line, .. }
            | Self::Investment { line }
            | Self::UnsupportedQifType { line } => Some(*line),
            Self::Csv(e) => match e {
                crate::csv::CsvError::UnclosedQuote { line }
                | crate::csv::CsvError::TooManyColumns { line }
                | crate::csv::CsvError::FieldTooLong { line } => Some(*line),
                crate::csv::CsvError::Empty | crate::csv::CsvError::TooManyRows => None,
            },
            Self::Empty
            | Self::TooLarge { .. }
            | Self::UnknownFormat
            | Self::NoStatement
            | Self::TooManyRows { .. }
            | Self::CsvNeedsMapping
            | Self::MappingDoesNotFit => None,
        }
    }
}
