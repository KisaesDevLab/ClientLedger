//! `cargo xtask preview`: an unsigned, portable PREVIEW build for testers
//! (Q-253), ahead of the real installers in P35 (`cargo xtask package`).
//!
//! Steps: release-build `clientledger`, `clientledger-cli` and the reading
//! worker `clientledger-docread`, build the engine single executable, fetch
//! the pinned `PostgreSQL` and `PDFium`, build tesseract and fetch its
//! models (A03), lay out
//! `target/preview/ClientLedger-preview-<version>-<sha>-<os>-<arch>/`, then
//! archive it (`.zip` on Windows and macOS, `.tar.gz` on Linux) with a
//! `.sha256` file beside it.
//!
//! The folder uses the installed layout the app already searches in release
//! builds: the engine in `engine/` next to the app (`cl_host::find_engine_exe`)
//! and `PostgreSQL` in `pg/` next to the app (`cl_pg::PgBinaries::discover`);
//! the reading worker beside the app and tesseract in `ocr/`
//! (`cl_host::find_docread_exe`, `cl_host::find_ocr_dir`).

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const README_TEMPLATE: &str = include_str!("../preview/README-PREVIEW.txt");

/// Repository documents copied into the folder root.
const DOCS: [&str; 3] = ["LICENSE", "NOTICE.md", "ATTRIBUTION.md"];

/// Archive format for a platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    TarGz,
}

impl ArchiveKind {
    /// `.tar.gz` on Linux (keeps file modes the usual way), `.zip` elsewhere.
    #[must_use]
    pub fn for_os(os: &str) -> Self {
        if os == "linux" {
            ArchiveKind::TarGz
        } else {
            ArchiveKind::Zip
        }
    }

    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            ArchiveKind::Zip => "zip",
            ArchiveKind::TarGz => "tar.gz",
        }
    }
}

/// `ClientLedger-preview-<version>-<sha>-<os>-<arch>` (the target dir is
/// already `<os>-<arch>`, for example `windows-x64`).
#[must_use]
pub fn folder_name(version: &str, short_sha: &str, target_dir: &str) -> String {
    format!("ClientLedger-preview-{version}-{short_sha}-{target_dir}")
}

/// The README with its placeholders filled in.
#[must_use]
pub fn readme(version: &str, commit: &str, target_dir: &str) -> String {
    README_TEMPLATE
        .replace("{version}", version)
        .replace("{commit}", commit)
        .replace("{target}", target_dir)
}

/// One line in `sha256sum` format: `<hex>  <file name>`.
#[must_use]
pub fn checksum_line(hex: &str, file_name: &str) -> String {
    format!("{hex}  {file_name}\n")
}

/// Where each built piece goes inside the folder, relative to its root.
/// These are the places the release app looks (see the module docs).
#[derive(Debug, PartialEq, Eq)]
pub struct Layout {
    pub app: PathBuf,
    pub cli: PathBuf,
    /// The document reading worker, next to the app
    /// (`cl_host::find_docread_exe`).
    pub docread: PathBuf,
    pub engine_dir: PathBuf,
    pub pg_dir: PathBuf,
    /// Tesseract and `tessdata/` (`cl_host::find_ocr_dir`: `<exe_dir>/ocr`).
    pub ocr_dir: PathBuf,
}

#[must_use]
pub fn layout(windows: bool) -> Layout {
    let exe = |n: &str| {
        if windows {
            format!("{n}.exe")
        } else {
            n.to_string()
        }
    };
    Layout {
        app: PathBuf::from(exe("clientledger")),
        cli: PathBuf::from(exe("clientledger-cli")),
        docread: PathBuf::from(exe("clientledger-docread")),
        engine_dir: PathBuf::from("engine"),
        pg_dir: PathBuf::from("pg"),
        ocr_dir: PathBuf::from("ocr"),
    }
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .context("could not run git")?;
    if !out.status.success() {
        bail!("git {} failed ({})", args.join(" "), out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Short commit, with `-dirty` when the working tree has changes.
fn short_sha(root: &Path) -> Result<String> {
    let sha = git(root, &["rev-parse", "--short=8", "HEAD"])?;
    let dirty = !git(root, &["status", "--porcelain", "--untracked-files=no"])?.is_empty();
    Ok(if dirty { format!("{sha}-dirty") } else { sha })
}

fn cargo_target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(|| root.join("target"), PathBuf::from)
}

/// Copy a directory tree. Skips `skip` names at any depth.
fn copy_tree(from: &Path, to: &Path, skip: &[&str]) -> Result<()> {
    fs::create_dir_all(to).with_context(|| format!("create {}", to.display()))?;
    for entry in fs::read_dir(from).with_context(|| format!("read {}", from.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        if skip.iter().any(|s| name == **s) {
            continue;
        }
        let src = entry.path();
        let dst = to.join(&name);
        if entry.file_type()?.is_dir() {
            copy_tree(&src, &dst, skip)?;
        } else {
            fs::copy(&src, &dst).with_context(|| format!("copy {}", src.display()))?;
        }
    }
    Ok(())
}

/// Every file under `dir`, relative to it, sorted (stable archives).
fn files_under(dir: &Path) -> Result<Vec<PathBuf>> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let p = entry.path();
            if entry.file_type()?.is_dir() {
                walk(base, &p, out)?;
            } else {
                out.push(p.strip_prefix(base)?.to_path_buf());
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    out.sort();
    Ok(out)
}

/// Archive member name: `<top>/<rel>` with forward slashes.
fn member_name(top: &str, rel: &Path) -> String {
    let mut s = top.to_string();
    for c in rel.components() {
        s.push('/');
        s.push_str(&c.as_os_str().to_string_lossy());
    }
    s
}

#[cfg(unix)]
fn file_mode(p: &Path) -> Result<u32> {
    use std::os::unix::fs::PermissionsExt;
    Ok(fs::metadata(p)?.permissions().mode() & 0o777)
}

/// On Windows there are no modes to read; programs (and everything in
/// `bin/`) are marked executable so a zip made here still works elsewhere.
#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn file_mode(p: &Path) -> Result<u32> {
    let exe = p.extension().is_some_and(|e| e == "exe")
        || p.parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n == "bin");
    Ok(if exe { 0o755 } else { 0o644 })
}

/// Zip timestamp (UTC) for Unix seconds, so entries do not show 1980.
fn zip_time(secs: u64) -> Option<zip::DateTime> {
    let s = crate::engine::rfc3339_utc(secs); // YYYY-MM-DDTHH:MM:SSZ
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<u16>().ok();
    let b = |r| n(r).and_then(|v| u8::try_from(v).ok());
    zip::DateTime::from_date_and_time(
        n(0..4)?,
        b(5..7)?,
        b(8..10)?,
        b(11..13)?,
        b(14..16)?,
        b(17..19)?,
    )
    .ok()
}

/// Write `folder` (its name becomes the top-level directory) to `archive`.
///
/// # Errors
/// Any I/O or archive error.
pub fn write_archive(folder: &Path, archive: &Path, kind: ArchiveKind) -> Result<()> {
    let top = folder
        .file_name()
        .ok_or_else(|| anyhow!("folder has no name"))?
        .to_string_lossy()
        .to_string();
    let files = files_under(folder)?;
    let out = fs::File::create(archive).with_context(|| format!("create {}", archive.display()))?;
    match kind {
        ArchiveKind::Zip => {
            use zip::write::SimpleFileOptions;
            let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(out));
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let stamp = zip_time(now).unwrap_or_default();
            for rel in &files {
                let src = folder.join(rel);
                let opts = SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .system(zip::System::Unix)
                    .last_modified_time(stamp)
                    .unix_permissions(file_mode(&src)?)
                    .large_file(fs::metadata(&src)?.len() >= u64::from(u32::MAX));
                zip.start_file(member_name(&top, rel), opts)?;
                let mut f = fs::File::open(&src)?;
                std::io::copy(&mut f, &mut zip)?;
            }
            zip.finish()?.flush()?;
        }
        ArchiveKind::TarGz => {
            let gz = flate2::write::GzEncoder::new(
                std::io::BufWriter::new(out),
                flate2::Compression::default(),
            );
            let mut tar = tar::Builder::new(gz);
            tar.mode(tar::HeaderMode::Deterministic);
            for rel in &files {
                let src = folder.join(rel);
                let mut header = tar::Header::new_gnu();
                header.set_size(fs::metadata(&src)?.len());
                header.set_mode(file_mode(&src)?);
                header.set_cksum();
                tar.append_data(&mut header, member_name(&top, rel), fs::File::open(&src)?)?;
            }
            tar.into_inner()?.finish()?.flush()?;
        }
    }
    Ok(())
}

/// Lowercase hex SHA-256 of a file.
///
/// # Errors
/// The file cannot be read.
pub fn sha256_file(p: &Path) -> Result<String> {
    let mut f = fs::File::open(p).with_context(|| format!("open {}", p.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(crate::fetch_lock::hex(&h.finalize()))
}

/// Entry point for `cargo xtask preview`.
///
/// # Errors
/// Any failing build step, a missing input, or an I/O error.
pub fn preview(args: &[String]) -> Result<()> {
    if let Some(bad) = args.first() {
        bail!("`preview` takes no options (got `{bad}`)");
    }
    let root = crate::engine::repo_root()?;
    let target = crate::engine::host_target()?;
    let version = env!("CARGO_PKG_VERSION");
    let sha = short_sha(&root)?;
    let commit = git(&root, &["rev-parse", "HEAD"])?;

    println!("preview: release build of the apps");
    let mut c = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    c.args([
        "build",
        "--release",
        "-p",
        "clientledger",
        "-p",
        "clientledger-cli",
        "-p",
        "clientledger-docread",
    ])
    .current_dir(&root);
    let status = c.status().context("could not start cargo")?;
    if !status.success() {
        bail!("release build failed ({status})");
    }
    crate::engine::engine(&[]).context("building the engine failed")?;
    crate::pg_fetch::pg_fetch(&[]).context("fetching PostgreSQL failed")?;
    crate::pdfium_fetch::pdfium_fetch(&[]).context("fetching PDFium failed")?;
    // Reading scans (A03): a build without tesseract would read text PDFs
    // only, so it is required here as PDFium is (plan §5).
    crate::ocr_build::ocr_build(&[]).context("building tesseract failed")?;
    crate::ocr_models::ocr_models(&[]).context("fetching the OCR models failed")?;

    let name = folder_name(version, &sha, target.dir);
    let preview_dir = root.join("target").join("preview");
    let folder = preview_dir.join(&name);
    if folder.exists() {
        fs::remove_dir_all(&folder).with_context(|| format!("clear {}", folder.display()))?;
    }
    fs::create_dir_all(&folder)?;

    let lay = layout(cfg!(windows));
    let release = cargo_target_dir(&root).join("release");
    for exe in [&lay.app, &lay.cli, &lay.docread] {
        fs::copy(release.join(exe), folder.join(exe))
            .with_context(|| format!("copy {}", exe.display()))?;
    }
    let engine_src = root.join("target").join("engine").join(target.dir);
    let engine_dst = folder.join(&lay.engine_dir);
    fs::create_dir_all(&engine_dst)?;
    for f in [target.exe_name, "manifest.json"] {
        fs::copy(engine_src.join(f), engine_dst.join(f))
            .with_context(|| format!("copy engine {f}"))?;
    }
    copy_tree(
        &root.join("target").join("pg").join(target.dir),
        &folder.join(&lay.pg_dir),
        &["pg-fetch.stamp"],
    )?;
    // PDF previews (P17): the app looks in `<exe_dir>/pdfium/`; the license
    // texts travel with the library.
    let pdfium = crate::pdfium_fetch::host()?;
    copy_tree(
        &crate::pdfium_fetch::out_dir(&pdfium)?,
        &folder.join("pdfium"),
        &[crate::pdfium_fetch::STAMP],
    )?;
    // Tesseract, its models and their license texts: the app looks in
    // `<exe_dir>/ocr/`.
    copy_tree(
        &root
            .join("target")
            .join("ocr")
            .join(crate::ocr_build::host_triple()?),
        &folder.join(&lay.ocr_dir),
        &[crate::ocr_build::STAMP],
    )?;
    for doc in DOCS {
        fs::copy(root.join(doc), folder.join(doc)).with_context(|| format!("copy {doc}"))?;
    }
    fs::write(
        folder.join("README-PREVIEW.txt"),
        readme(version, &commit, target.dir),
    )?;

    let kind = ArchiveKind::for_os(std::env::consts::OS);
    let archive_name = format!("{name}.{}", kind.extension());
    let archive = preview_dir.join(&archive_name);
    println!("preview: writing {archive_name}");
    write_archive(&folder, &archive, kind)?;
    let hex = sha256_file(&archive)?;
    let sum = preview_dir.join(format!("{archive_name}.sha256"));
    fs::write(&sum, checksum_line(&hex, &archive_name))?;
    println!("preview: {}", archive.display());
    println!("preview: sha256 {hex}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_name_has_version_sha_and_platform() {
        assert_eq!(
            folder_name("0.1.0", "0c7656d1", "windows-x64"),
            "ClientLedger-preview-0.1.0-0c7656d1-windows-x64"
        );
    }

    #[test]
    fn linux_gets_tar_gz_and_others_zip() {
        assert_eq!(ArchiveKind::for_os("linux"), ArchiveKind::TarGz);
        assert_eq!(ArchiveKind::for_os("windows"), ArchiveKind::Zip);
        assert_eq!(ArchiveKind::for_os("macos"), ArchiveKind::Zip);
        assert_eq!(ArchiveKind::TarGz.extension(), "tar.gz");
    }

    #[test]
    fn readme_placeholders_are_all_filled() {
        let r = readme("0.1.0", "abc123", "macos-arm64");
        assert!(r.contains("Version 0.1.0, built from commit abc123 for macos-arm64."));
        assert!(
            !r.contains('{'),
            "unfilled placeholder in README-PREVIEW.txt"
        );
        assert!(r.contains("Do NOT keep your real books"));
    }

    #[test]
    fn checksum_line_matches_sha256sum_format() {
        assert_eq!(checksum_line("ab", "x.zip"), "ab  x.zip\n");
    }

    /// The folder must use the installed layout the release app searches:
    /// `engine/<engine exe>` and `pg/` next to the app.
    #[test]
    fn layout_matches_the_installed_lookup() {
        let w = layout(true);
        assert_eq!(w.app, PathBuf::from("clientledger.exe"));
        assert_eq!(w.engine_dir, PathBuf::from("engine"));
        assert_eq!(w.pg_dir, PathBuf::from("pg"));
        assert_eq!(layout(false).cli, PathBuf::from("clientledger-cli"));
        // The reading worker next to the app, tesseract in `ocr/` (A03).
        assert_eq!(w.docread, PathBuf::from("clientledger-docread.exe"));
        assert_eq!(layout(false).docread, PathBuf::from("clientledger-docread"));
        assert_eq!(w.ocr_dir, PathBuf::from("ocr"));
    }

    fn sample_folder(dir: &Path) -> PathBuf {
        let folder = dir.join("ClientLedger-preview-test");
        fs::create_dir_all(folder.join("pg").join("bin")).unwrap();
        fs::write(folder.join("README-PREVIEW.txt"), b"hello").unwrap();
        fs::write(folder.join("pg").join("bin").join("postgres"), b"pg").unwrap();
        folder
    }

    #[test]
    fn zip_archive_has_top_folder_and_executable_bits() {
        let dir = tempfile::tempdir().unwrap();
        let folder = sample_folder(dir.path());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = folder.join("pg").join("bin").join("postgres");
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let archive = dir.path().join("a.zip");
        write_archive(&folder, &archive, ArchiveKind::Zip).unwrap();
        let mut z = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
        let names: Vec<String> = z.file_names().map(|n| n.unwrap().to_string()).collect();
        assert!(names.contains(&"ClientLedger-preview-test/README-PREVIEW.txt".to_string()));
        let pg = z
            .by_name("ClientLedger-preview-test/pg/bin/postgres")
            .unwrap();
        assert_eq!(pg.unix_mode().unwrap() & 0o111, 0o111);
    }

    #[test]
    fn tar_gz_archive_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let folder = sample_folder(dir.path());
        let archive = dir.path().join("a.tar.gz");
        write_archive(&folder, &archive, ArchiveKind::TarGz).unwrap();
        let mut t = tar::Archive::new(flate2::read::GzDecoder::new(
            fs::File::open(&archive).unwrap(),
        ));
        let mut seen = Vec::new();
        for e in t.entries().unwrap() {
            let mut e = e.unwrap();
            let path = e.path().unwrap().to_string_lossy().to_string();
            let mut body = String::new();
            e.read_to_string(&mut body).unwrap();
            seen.push((path, body));
        }
        assert!(seen.contains(&(
            "ClientLedger-preview-test/README-PREVIEW.txt".into(),
            "hello".into()
        )));
    }

    #[test]
    fn zip_time_converts_unix_seconds() {
        // 2026-10-20T18:41:08Z
        let t = zip_time(1_792_521_668).unwrap();
        assert_eq!(
            (
                t.year(),
                t.month(),
                t.day(),
                t.hour(),
                t.minute(),
                t.second()
            ),
            (2026, 10, 20, 18, 41, 8)
        );
    }

    #[test]
    fn sha256_of_a_known_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x");
        fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
