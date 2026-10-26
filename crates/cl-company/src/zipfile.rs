//! Stored-entry zip helpers: write a zip of text files (Export All Data)
//! and read one plaintext entry of an archive (`envelope.json`).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
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

/// The central directory's own entry count (end of central directory
/// record, or its Zip64 form), `None` when it can't be found.
fn declared_entries(file: &mut File) -> std::io::Result<Option<u64>> {
    const EOCD: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    const ZIP64_LOCATOR: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];
    const ZIP64_EOCD: [u8; 4] = [0x50, 0x4b, 0x06, 0x06];
    let len = file.seek(SeekFrom::End(0))?;
    // The record is 22 bytes plus a comment of at most 65 535.
    let tail_len = len.min(22 + 65_535);
    let start = len - tail_len;
    file.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::new();
    file.take(tail_len).read_to_end(&mut tail)?;
    let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let Some(at) = (0..tail.len().saturating_sub(21)).rev().find(|&i| {
        tail[i..i + 4] == EOCD && i + 22 + usize::from(u16_at(&tail, i + 20)) == tail.len()
    }) else {
        return Ok(None);
    };
    let total = u16_at(&tail, at + 10);
    if total != u16::MAX {
        return Ok(Some(u64::from(total)));
    }
    // Zip64: the locator sits just before the record.
    let Some(loc) = at
        .checked_sub(20)
        .filter(|&l| tail[l..l + 4] == ZIP64_LOCATOR)
    else {
        return Ok(None);
    };
    let mut off = [0u8; 8];
    off.copy_from_slice(&tail[loc + 8..loc + 16]);
    file.seek(SeekFrom::Start(u64::from_le_bytes(off)))?;
    let mut rec = [0u8; 40];
    file.read_exact(&mut rec)?;
    if rec[..4] != ZIP64_EOCD {
        return Ok(None);
    }
    let mut n = [0u8; 8];
    n.copy_from_slice(&rec[32..40]);
    Ok(Some(u64::from_le_bytes(n)))
}

/// Opens `path` as a zip, `None` when it isn't one **or names an entry
/// twice** (security review L10): the zip reader keeps one of the two,
/// another reader may take the other, so such an archive is refused.
fn open_unique(path: &Path) -> std::io::Result<Option<ZipArchive<File>>> {
    let mut file = File::open(path)?;
    let declared = declared_entries(&mut file)?;
    file.rewind()?;
    let Ok(archive) = ZipArchive::new(file) else {
        return Ok(None);
    };
    if declared != Some(archive.len() as u64) {
        return Ok(None);
    }
    Ok(Some(archive))
}

/// Reads one entry by name (`None` when absent or not a zip).
pub(crate) fn read_entry(path: &Path, name: &str) -> std::io::Result<Option<Vec<u8>>> {
    let Some(mut archive) = open_unique(path)? else {
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
    let Some(archive) = open_unique(path)? else {
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

    /// An archive naming an entry twice is refused: the zip reader keeps
    /// one, the engine's reader might take the other.
    #[test]
    fn duplicate_entry_names_are_refused() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = dir.path().join("dup.zip");
        write_stored(
            &p,
            &[
                ("envelope.json".into(), b"{\"a\":1}".to_vec()),
                ("envelopX.json".into(), b"{\"a\":2}".to_vec()),
                ("manifest.json.enc".into(), b"m".to_vec()),
            ],
        )
        .expect("write");
        assert!(has_entry(&p, "manifest.json.enc").expect("names"));
        // Rename the second entry to the first's name, in its local
        // header and in the central directory.
        let mut bytes = std::fs::read(&p).expect("read");
        let (from, to) = (b"envelopX.json", b"envelope.json");
        let mut i = 0;
        while let Some(at) = bytes[i..].windows(from.len()).position(|w| w == from) {
            bytes[i + at..i + at + from.len()].copy_from_slice(to);
            i += at + from.len();
        }
        std::fs::write(&p, &bytes).expect("write");
        assert_eq!(read_entry(&p, "envelope.json").expect("read"), None);
        assert!(!has_entry(&p, "manifest.json.enc").expect("names"));
    }

    #[test]
    fn the_declared_count_is_found_past_a_comment() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = dir.path().join("c.zip");
        let file = File::create(&p).expect("create");
        let mut zip = ZipWriter::new(file);
        zip.set_comment("PK\u{5}\u{6} looks like a record")
            .expect("comment");
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for n in ["a", "b"] {
            zip.start_file(n, options).expect("start");
            zip.write_all(b"x").expect("write");
        }
        zip.finish().expect("finish");
        let mut f = File::open(&p).expect("open");
        assert_eq!(declared_entries(&mut f).expect("read"), Some(2));
        assert!(has_entry(&p, "b").expect("names"));
    }
}
