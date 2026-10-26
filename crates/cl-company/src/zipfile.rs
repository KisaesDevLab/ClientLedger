//! Stored-entry zip helpers: write a zip of text files (Export All Data)
//! and read one plaintext entry of an archive (`envelope.json`).

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// Largest plaintext entry read from an archive (the envelope is tiny).
const ENTRY_CAP: u64 = 256 * 1024;

/// Writes `entries` (name, bytes) as stored entries.
pub(crate) fn write_stored(path: &Path, entries: &[(String, Vec<u8>)]) -> std::io::Result<()> {
    let file = File::create(path)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in entries {
        zip.start_file(name, options)?;
        zip.write_all(bytes)?;
    }
    zip.finish()?.flush()
}

/// Reads one entry by name (`None` when absent or not a zip).
pub(crate) fn read_entry(path: &Path, name: &str) -> std::io::Result<Option<Vec<u8>>> {
    let file = File::open(path)?;
    let Ok(mut archive) = ZipArchive::new(file) else {
        return Ok(None);
    };
    let Ok(entry) = archive.by_name(name) else {
        return Ok(None);
    };
    if entry.size() > ENTRY_CAP {
        return Ok(None);
    }
    // The declared size is not trusted: read at most the cap plus one byte
    // and refuse anything that keeps going.
    let mut out = Vec::new();
    entry.take(ENTRY_CAP + 1).read_to_end(&mut out)?;
    if out.len() as u64 > ENTRY_CAP {
        return Ok(None);
    }
    Ok(Some(out))
}

/// Whether the archive has an entry called `name` (`false` when it is not
/// a zip). Looks the name up in the central directory instead of listing
/// names: an engine archive holds one entry per table (well over a
/// hundred) with `manifest.json.enc` written last.
pub(crate) fn has_entry(path: &Path, name: &str) -> std::io::Result<bool> {
    let file = File::open(path)?;
    let Ok(archive) = ZipArchive::new(file) else {
        return Ok(false);
    };
    Ok(archive.index_for_name(name).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = dir.path().join("x.zip");
        write_stored(
            &p,
            &[
                ("envelope.json".into(), b"{\"a\":1}".to_vec()),
                ("data/t.csv".into(), b"id\n1\n".to_vec()),
            ],
        )
        .expect("write");
        assert_eq!(
            read_entry(&p, "envelope.json").expect("read"),
            Some(b"{\"a\":1}".to_vec())
        );
        assert_eq!(read_entry(&p, "missing").expect("read"), None);
        assert!(has_entry(&p, "data/t.csv").expect("names"));
        assert!(!has_entry(&p, "data/u.csv").expect("names"));
        let not_zip = dir.path().join("n.bin");
        std::fs::write(&not_zip, b"hello").expect("write");
        assert_eq!(read_entry(&not_zip, "envelope.json").expect("read"), None);
        assert!(!has_entry(&not_zip, "envelope.json").expect("read"));
    }

    /// An engine archive: one entry per table, the manifest written last.
    #[test]
    fn finds_an_entry_after_many_others() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = dir.path().join("big.zip");
        let mut entries: Vec<(String, Vec<u8>)> = (0..200)
            .map(|i| (format!("data/t{i}.ndjson.enc"), Vec::new()))
            .collect();
        entries.insert(0, ("envelope.json".into(), b"{}".to_vec()));
        entries.push(("manifest.json.enc".into(), b"m".to_vec()));
        write_stored(&p, &entries).expect("write");
        assert!(has_entry(&p, "manifest.json.enc").expect("names"));
    }
}
