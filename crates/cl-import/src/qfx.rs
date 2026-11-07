//! QFX: Quicken's OFX (an `INTU.BID` in the sign-on block). The body is
//! plain OFX, so this reads it with [`crate::ofx`] and reports the format
//! as QFX.

use crate::detect::FileFormat;
use crate::error::ImportError;
use crate::normalize::ParsedFile;

/// Reads a QFX file.
///
/// # Errors
/// As [`crate::ofx::parse`].
pub fn parse(bytes: &[u8]) -> Result<ParsedFile, ImportError> {
    crate::ofx::parse_as(bytes, Some(FileFormat::Qfx))
}
