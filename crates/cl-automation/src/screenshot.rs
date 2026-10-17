//! Turning screen pixels into a PNG file, and where such a file may be
//! written.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

/// Why a screenshot could not be saved.
#[derive(Debug, thiserror::Error)]
pub enum ScreenshotError {
    /// The destination is not acceptable.
    #[error("{0}")]
    Path(String),
    /// The pixel data does not match the size.
    #[error("the screenshot data has the wrong size")]
    Size,
    /// Writing failed.
    #[error("the screenshot could not be written: {0}")]
    Io(#[from] std::io::Error),
    /// PNG encoding failed.
    #[error("the screenshot could not be encoded")]
    Encode,
}

/// Encode RGBA pixels (8 bits per channel, row by row) as a PNG.
///
/// # Errors
/// [`ScreenshotError::Size`] if `rgba` is not `width * height * 4` bytes.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, ScreenshotError> {
    let expected = u64::from(width) * u64::from(height) * 4;
    if width == 0 || height == 0 || rgba.len() as u64 != expected {
        return Err(ScreenshotError::Size);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|_| ScreenshotError::Encode)?;
        writer
            .write_image_data(rgba)
            .map_err(|_| ScreenshotError::Encode)?;
        writer.finish().map_err(|_| ScreenshotError::Encode)?;
    }
    Ok(out)
}

/// Image size from a PNG's header (tests and the compare page).
#[must_use]
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let dec = png::Decoder::new(std::io::Cursor::new(bytes));
    let reader = dec.read_info().ok()?;
    let info = reader.info();
    Some((info.width, info.height))
}

/// Check a destination: it must end in `.png`, its folder must exist, and an
/// existing entry must be a plain file (never a symlink or folder).
///
/// # Errors
/// [`ScreenshotError::Path`] with a plain message.
pub fn check_destination(path: &Path) -> Result<(), ScreenshotError> {
    let bad = |m: &str| Err(ScreenshotError::Path(m.to_string()));
    let is_png = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if !is_png {
        return bad("screenshots must be saved as .png files");
    }
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() && !p.is_dir() => {
            return bad("the screenshot folder does not exist");
        }
        _ => {}
    }
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => bad("the screenshot path is a symbolic link"),
        Ok(m) if !m.is_file() => bad("the screenshot path is not a file"),
        _ => Ok(()),
    }
}

/// Write PNG bytes to `path` after [`check_destination`].
///
/// # Errors
/// Any [`ScreenshotError`].
pub fn write_png(path: &Path, bytes: &[u8]) -> Result<(), ScreenshotError> {
    check_destination(path)?;
    // Replace through a sibling temp file so a half-written PNG is never seen.
    let tmp = path.with_extension("png.part");
    let _ = fs::remove_file(&tmp);
    {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_reads_back_the_size() {
        let px = vec![255u8; 3 * 2 * 4];
        let bytes = encode_png(3, 2, &px).unwrap();
        assert_eq!(png_size(&bytes), Some((3, 2)));
        assert!(matches!(
            encode_png(3, 2, &px[..10]),
            Err(ScreenshotError::Size)
        ));
        assert!(matches!(encode_png(0, 2, &[]), Err(ScreenshotError::Size)));
    }

    #[test]
    fn destination_rules() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check_destination(&dir.path().join("a.png")).is_ok());
        assert!(check_destination(&dir.path().join("a.txt")).is_err());
        assert!(check_destination(&dir.path().join("missing").join("a.png")).is_err());
        let sub = dir.path().join("d.png");
        fs::create_dir(&sub).unwrap();
        assert!(check_destination(&sub).is_err());
    }

    #[test]
    fn write_replaces_an_existing_file_and_leaves_no_part_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.png");
        let bytes = encode_png(1, 1, &[0, 0, 0, 255]).unwrap();
        write_png(&p, &bytes).unwrap();
        write_png(&p, &bytes).unwrap();
        assert_eq!(fs::read(&p).unwrap(), bytes);
        assert!(!dir.path().join("s.png.part").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_destination_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.txt");
        fs::write(&victim, "keep").unwrap();
        let link = dir.path().join("s.png");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        assert!(write_png(&link, b"x").is_err());
        assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
    }
}
