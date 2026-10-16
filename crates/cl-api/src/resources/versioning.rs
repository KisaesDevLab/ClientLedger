//! Record versions for optimistic concurrency (P05 plan §2.5).

/// The version a record was read at.
///
/// `Server` comes from the engine's `version` column (U-17). Until the engine
/// has it, `Fingerprint` is the SHA-256 of the record's editable fields, used
/// by the pre-save check.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordVersion {
    /// The engine's record version (U-17, `record-version-v1`).
    Server(u64),
    /// SHA-256 over the canonical JSON of the editable fields.
    Fingerprint([u8; 32]),
}

impl std::fmt::Debug for RecordVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordVersion::Server(v) => write!(f, "Server({v})"),
            RecordVersion::Fingerprint(h) => {
                write!(f, "Fingerprint(")?;
                for b in &h[..4] {
                    write!(f, "{b:02x}")?;
                }
                write!(f, "…)")
            }
        }
    }
}
