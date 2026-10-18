//! Atomic JSON files: read with a size cap, write through a temp file and
//! rename, so a crash never leaves a half-written settings file.

use std::fs;
use std::io;
use std::path::Path;

/// Largest settings file read (a corrupt or hostile file is ignored).
pub(crate) const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// Reads a file when it exists and is under the cap.
pub(crate) fn read_capped(path: &Path) -> io::Result<Option<String>> {
    match fs::metadata(path) {
        Ok(m) if m.len() <= MAX_BYTES => fs::read_to_string(path).map(Some),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "settings file too large",
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Writes `text` to `path` atomically (temp file in the same folder, then
/// rename). The folder is created when missing.
pub(crate) fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "settings path has no folder")
    })?;
    fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("settings");
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    fs::write(&tmp, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    if fs::rename(&tmp, path).is_ok() {
        return Ok(());
    }
    // Windows refuses to rename over an open file; replace in two steps and
    // clean up the temp file on failure.
    let result = fs::remove_file(path).and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_and_missing_is_none() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = dir.path().join("sub").join("x.json");
        assert_eq!(read_capped(&p).expect("read"), None);
        write_atomic(&p, "{}").expect("write");
        assert_eq!(read_capped(&p).expect("read").as_deref(), Some("{}"));
        write_atomic(&p, "{\"a\":1}").expect("overwrite");
        assert_eq!(read_capped(&p).expect("read").as_deref(), Some("{\"a\":1}"));
        assert!(
            fs::read_dir(dir.path().join("sub")).expect("dir").count() == 1,
            "no temp file left behind"
        );
    }
}
