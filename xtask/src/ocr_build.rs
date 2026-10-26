//! `cargo xtask ocr-build [--force]`: build the `tesseract` command-line
//! program from pinned source for the host platform (A03, Q-283, plan D1).
//!
//! Leptonica and Tesseract come from `ocr.lock` (SHA-256 checked, cached in
//! `target/ocr/.cache`). Both are configured with **every** optional
//! dependency switched off explicitly: no image codecs (Tesseract only reads
//! the 8-bit PGM our code writes), no zlib, curl, libarchive, OpenMP,
//! training tools (pango/cairo) or graphics. `CMAKE_DISABLE_FIND_PACKAGE_*`
//! stops a library that happens to be installed on the build machine from
//! being picked up anyway.
//!
//! Leptonica is static; Tesseract is one static executable. Windows links
//! the static C runtime (`MultiThreaded`); Linux links libstdc++ and libgcc
//! statically (GCC Runtime Library Exception, ATTRIBUTION.md), so the
//! program needs only the C library.
//!
//! After the build a **linkage check** (`dumpbin /dependents`, `ldd`,
//! `otool -L`) fails on any library outside the operating system's own.
//!
//! Output: `target/ocr/<triple>/{tesseract[.exe], tessdata/, LICENSES/}`.
//! Model data (`tessdata/`) is filled by `cargo xtask ocr-models` (WP-B).
//!
//! Tools needed: `cmake` (3.18+) and a C/C++ compiler. On Windows the MSVC
//! environment is loaded from `vcvars64.bat` (found with `vswhere`) when the
//! shell has not loaded it already; `CMake` and Ninja come with the Visual
//! Studio C++ workload.

use anyhow::{Context, Result, anyhow, bail};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::fetch_lock::{Entry, fetch_verified, key_value, parse_entry};

/// Name of the pin file at the repository root.
pub const LOCK_FILE: &str = "ocr.lock";

/// Marker written inside a finished `target/ocr/<triple>/` directory.
pub const STAMP: &str = "ocr-build.stamp";

/// Parsed `ocr.lock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    pub tesseract_version: String,
    pub leptonica_version: String,
    pub tesseract: Entry,
    pub leptonica: Entry,
}

impl Lock {
    /// Parse the lock file text.
    ///
    /// # Errors
    /// A missing version or source line, a malformed line, or a url that does
    /// not name the pinned version.
    pub fn parse(text: &str) -> Result<Lock> {
        let mut tv = None;
        let mut lv = None;
        let mut entries = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(v) = key_value(LOCK_FILE, n + 1, line, "tesseract")? {
                tv = Some(v);
                continue;
            }
            if let Some(v) = key_value(LOCK_FILE, n + 1, line, "leptonica")? {
                lv = Some(v);
                continue;
            }
            entries.push(parse_entry(LOCK_FILE, n + 1, line)?);
        }
        let tesseract_version =
            tv.ok_or_else(|| anyhow!("{LOCK_FILE}: missing `tesseract = ...`"))?;
        let leptonica_version =
            lv.ok_or_else(|| anyhow!("{LOCK_FILE}: missing `leptonica = ...`"))?;
        for v in [&tesseract_version, &leptonica_version] {
            if !v
                .split('.')
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            {
                bail!("{LOCK_FILE}: version `{v}` must be dotted numbers");
            }
        }
        let take = |name: &str, version: &str| -> Result<Entry> {
            let e = entries
                .iter()
                .find(|e| e.target == name)
                .ok_or_else(|| anyhow!("{LOCK_FILE}: no `{name}` source line"))?;
            if !e.url.contains(&format!("/{version}/"))
                && !e.url.ends_with(&format!("/{version}.tar.gz"))
            {
                bail!("{LOCK_FILE}: the {name} url does not name version {version}");
            }
            Ok(e.clone())
        };
        Ok(Lock {
            tesseract: take("tesseract", &tesseract_version)?,
            leptonica: take("leptonica", &leptonica_version)?,
            tesseract_version,
            leptonica_version,
        })
    }
}

/// The host's target triple (the same names as `target/pdfium/`).
///
/// # Errors
/// A platform `ClientLedger` does not ship.
pub fn host_triple() -> Result<&'static str> {
    Ok(crate::pdfium_fetch::host()?.triple)
}

/// The program's file name on this platform.
#[must_use]
pub fn exe_name() -> &'static str {
    if cfg!(windows) {
        "tesseract.exe"
    } else {
        "tesseract"
    }
}

// ---------------------------------------------------------------------------
// `CMake` options (S17): every optional dependency off, by name.
// ---------------------------------------------------------------------------

/// Packages neither build may find, whatever is installed on the machine.
const NO_FIND: [&str; 16] = [
    "PkgConfig",
    "ZLIB",
    "PNG",
    "JPEG",
    "TIFF",
    "GIF",
    "WebP",
    "OpenJPEG",
    "LibArchive",
    "CURL",
    "OpenMP",
    "OpenCL",
    "ICU",
    "Pango",
    "Cairo",
    "SW",
];

fn common_options() -> Vec<String> {
    let mut v: Vec<String> = vec![
        "-DCMAKE_BUILD_TYPE=Release".into(),
        "-DBUILD_SHARED_LIBS=OFF".into(),
        // Static C runtime on MSVC; the policy makes CMake honour the variable
        // for projects whose minimum version predates 3.15 (Leptonica: 3.10).
        "-DCMAKE_POLICY_DEFAULT_CMP0091=NEW".into(),
        "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded".into(),
        "-DCMAKE_POSITION_INDEPENDENT_CODE=ON".into(),
    ];
    v.extend(
        NO_FIND
            .iter()
            .map(|p| format!("-DCMAKE_DISABLE_FIND_PACKAGE_{p}=ON")),
    );
    if cfg!(target_os = "macos") {
        v.push("-DCMAKE_OSX_DEPLOYMENT_TARGET=11.0".into());
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            "x86_64"
        };
        v.push(format!("-DCMAKE_OSX_ARCHITECTURES={arch}"));
        // Never pick anything up from Homebrew.
        v.push("-DCMAKE_IGNORE_PREFIX_PATH=/opt/homebrew;/usr/local".into());
        v.push("-DCMAKE_FIND_FRAMEWORK=NEVER".into());
    }
    if cfg!(target_os = "linux") {
        // One binary for every distribution: no libstdc++/libgcc_s at run time.
        v.push("-DCMAKE_EXE_LINKER_FLAGS=-static-libstdc++ -static-libgcc".into());
    }
    v
}

/// Leptonica: library only, no codecs (PNM/BMP readers are built in).
#[must_use]
pub fn leptonica_options() -> Vec<String> {
    let mut v = common_options();
    v.extend(
        [
            "-DBUILD_PROG=OFF",
            // Defaults to ON on Windows, which downloads codecs with `sw`.
            "-DSW_BUILD=OFF",
            "-DSTRICT_CONF=OFF",
            "-DENABLE_ZLIB=OFF",
            "-DENABLE_PNG=OFF",
            "-DENABLE_GIF=OFF",
            "-DENABLE_JPEG=OFF",
            "-DENABLE_TIFF=OFF",
            "-DENABLE_WEBP=OFF",
            "-DENABLE_OPENJPEG=OFF",
        ]
        .map(String::from),
    );
    v
}

/// Tesseract: the command-line program only. The legacy engine stays (page
/// orientation detection needs it).
#[must_use]
pub fn tesseract_options(leptonica_prefix: &Path) -> Vec<String> {
    let mut v = common_options();
    v.extend(
        [
            "-DBUILD_TRAINING_TOOLS=OFF",
            "-DBUILD_TESTS=OFF",
            "-DGRAPHICS_DISABLED=ON",
            "-DOPENMP_BUILD=OFF",
            "-DDISABLE_TIFF=ON",
            "-DDISABLE_ARCHIVE=ON",
            "-DDISABLE_CURL=ON",
            "-DDISABLED_LEGACY_ENGINE=OFF",
            "-DENABLE_NATIVE=OFF",
            "-DENABLE_LTO=OFF",
            "-DENABLE_CCACHE=OFF",
            "-DUSE_SYSTEM_ICU=OFF",
            "-DINSTALL_CONFIGS=OFF",
        ]
        .map(String::from),
    );
    v.push(format!(
        "-DLeptonica_DIR={}",
        leptonica_prefix
            .join("lib")
            .join("cmake")
            .join("leptonica")
            .display()
    ));
    v
}

// ---------------------------------------------------------------------------
// Linkage allowlist (S17)
// ---------------------------------------------------------------------------

/// Libraries the program may load, by platform. Everything else fails.
#[must_use]
pub fn allowed_library(os: &str, lib: &str) -> bool {
    let lib_lower = lib.to_ascii_lowercase();
    match os {
        "windows" => matches!(lib_lower.as_str(), "kernel32.dll" | "advapi32.dll"),
        "linux" => {
            let base = lib_lower.rsplit('/').next().unwrap_or(&lib_lower);
            base.starts_with("linux-vdso.so")
                || base.starts_with("ld-linux")
                || [
                    "libc.so.",
                    "libm.so.",
                    "libpthread.so.",
                    "libdl.so.",
                    "librt.so.",
                ]
                .iter()
                .any(|p| base.starts_with(p))
        }
        "macos" => matches!(
            lib,
            "/usr/lib/libSystem.B.dylib" | "/usr/lib/libc++.1.dylib"
        ),
        _ => false,
    }
}

/// Library names from `dumpbin /dependents` output.
#[must_use]
pub fn parse_dumpbin(out: &str) -> Vec<String> {
    let mut libs = Vec::new();
    let mut inside = false;
    for line in out.lines() {
        let t = line.trim();
        if t.starts_with("Image has the following dependencies") {
            inside = true;
            continue;
        }
        if inside {
            if t.is_empty() {
                if !libs.is_empty() {
                    break;
                }
                continue;
            }
            if t.starts_with("Summary") || t.starts_with("Image has the following delay") {
                break;
            }
            libs.push(t.to_string());
        }
    }
    libs
}

/// Library names from `ldd` output (`statically linked` gives none).
#[must_use]
pub fn parse_ldd(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|l| {
            let t = l.trim();
            if t.is_empty() || t.contains("statically linked") {
                return None;
            }
            // `libc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x...)`,
            // `/lib64/ld-linux-x86-64.so.2 (0x...)`, `linux-vdso.so.1 (0x...)`
            Some(t.split_whitespace().next()?.to_string())
        })
        .collect()
}

/// Library paths from `otool -L` output (the first line names the file).
#[must_use]
pub fn parse_otool(out: &str) -> Vec<String> {
    out.lines()
        .skip(1)
        .filter_map(|l| Some(l.split_whitespace().next()?.to_string()))
        .collect()
}

/// Libraries of `exe` outside the allowlist.
///
/// # Errors
/// The inspection tool could not run.
pub fn linkage_violations(exe: &Path, env: &BTreeMap<String, String>) -> Result<Vec<String>> {
    let os = std::env::consts::OS;
    let (libs, tool) = match os {
        "windows" => (
            parse_dumpbin(&capture(
                Command::new("dumpbin").arg("/dependents").arg(exe),
                env,
            )?),
            "dumpbin /dependents",
        ),
        "linux" => (
            parse_ldd(&capture(Command::new("ldd").arg(exe), env)?),
            "ldd",
        ),
        "macos" => (
            parse_otool(&capture(Command::new("otool").arg("-L").arg(exe), env)?),
            "otool -L",
        ),
        other => bail!("no linkage check for {other}"),
    };
    if libs.is_empty() && os != "linux" {
        bail!("{tool} listed no libraries; the output format may have changed");
    }
    println!("ocr-build: {tool}: {}", libs.join(", "));
    Ok(libs
        .into_iter()
        .filter(|l| !allowed_library(os, l))
        .map(|l| format!("tesseract links `{l}`, which is not an operating-system library"))
        .collect())
}

// ---------------------------------------------------------------------------
// Unpacking
// ---------------------------------------------------------------------------

/// The path of an archive member below its top folder, or `None` to skip it
/// (the top folder itself, absolute paths, `..`, odd components).
#[must_use]
pub fn member_path(member: &str) -> Option<PathBuf> {
    let p = Path::new(member);
    let mut out = PathBuf::new();
    for (i, c) in p.components().enumerate() {
        match c {
            Component::Normal(s) if i > 0 => out.push(s),
            Component::Normal(_) => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn unpack_source(tgz: &Path, dest: &Path) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(fs::File::open(tgz)?);
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries().context("read source archive")? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        let member = entry.path()?.to_string_lossy().replace('\\', "/");
        let Some(rel) = member_path(&member) else {
            continue;
        };
        let out = dest.join(&rel);
        if kind.is_dir() {
            fs::create_dir_all(&out)?;
            continue;
        }
        if !kind.is_file() {
            // Symlinks and the like: neither project needs them to build.
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&out)
            .with_context(|| format!("write {}", out.display()))?
            .write_all(&bytes)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Running tools
// ---------------------------------------------------------------------------

fn apply_env(cmd: &mut Command, env: &BTreeMap<String, String>) {
    for (k, v) in env {
        cmd.env(k, v);
    }
}

fn run(cmd: &mut Command, env: &BTreeMap<String, String>) -> Result<()> {
    apply_env(cmd, env);
    println!("==> {cmd:?}");
    let status = cmd
        .status()
        .with_context(|| format!("could not start {}", cmd.get_program().display()))?;
    if !status.success() {
        bail!("{} failed ({status})", cmd.get_program().display());
    }
    Ok(())
}

fn capture(cmd: &mut Command, env: &BTreeMap<String, String>) -> Result<String> {
    apply_env(cmd, env);
    let out = cmd
        .output()
        .with_context(|| format!("could not start {}", cmd.get_program().display()))?;
    if !out.status.success() {
        bail!(
            "{} failed ({}): {}",
            cmd.get_program().display(),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// On Windows, the MSVC developer environment (`cl`, `dumpbin`, `cmake`,
/// `ninja` on `PATH`), loaded from `vcvars64.bat` unless the shell already
/// has it. Empty elsewhere.
fn build_env() -> Result<BTreeMap<String, String>> {
    if !cfg!(windows) || std::env::var_os("VSCMD_VER").is_some() {
        return Ok(BTreeMap::new());
    }
    let pf86 =
        std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
    let vswhere = Path::new(&pf86)
        .join("Microsoft Visual Studio")
        .join("Installer")
        .join("vswhere.exe");
    let install = capture(
        Command::new(&vswhere).args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ]),
        &BTreeMap::new(),
    )
    .context("vswhere: is Visual Studio (or Build Tools) with the C++ workload installed?")?;
    let install = install.lines().next().unwrap_or("").trim().to_string();
    if install.is_empty() {
        bail!("vswhere found no Visual Studio with the C++ tools");
    }
    let vcvars = Path::new(&install).join(r"VC\Auxiliary\Build\vcvars64.bat");
    // `&`, not `&&`: vcvars can end with a non-zero errorlevel from an
    // optional probe while still setting everything up (checked below).
    let mut cmd = Command::new("cmd");
    vcvars_args(&mut cmd, &vcvars);
    let out = cmd.output().context("run vcvars64.bat")?;
    let mut env = BTreeMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some((k, v)) = line.split_once('=')
            && !k.is_empty()
        {
            env.insert(k.to_string(), v.to_string());
        }
    }
    if !env.contains_key("VSCMD_VER") {
        bail!("vcvars64.bat did not set up the MSVC environment");
    }
    Ok(env)
}

#[cfg(windows)]
fn vcvars_args(cmd: &mut Command, vcvars: &Path) {
    use std::os::windows::process::CommandExt as _;
    // cmd.exe does not understand the C runtime's `\"` quoting: pass the line
    // as is.
    cmd.raw_arg(format!(
        "/d /s /c \"call \"{}\" >nul 2>nul & set\"",
        vcvars.display()
    ));
}

#[cfg(not(windows))]
fn vcvars_args(_cmd: &mut Command, _vcvars: &Path) {}

fn jobs() -> String {
    std::env::var("CARGO_BUILD_JOBS")
        .ok()
        .filter(|j| j.parse::<u32>().is_ok_and(|n| n > 0))
        .unwrap_or_else(|| "4".into())
}

fn cmake_project(
    name: &str,
    src: &Path,
    build: &Path,
    prefix: &Path,
    options: &[String],
    env: &BTreeMap<String, String>,
) -> Result<()> {
    let started = Instant::now();
    let mut configure = Command::new("cmake");
    configure.arg("-S").arg(src).arg("-B").arg(build);
    if cfg!(windows) {
        configure.args(["-G", "Ninja"]);
    }
    configure
        .arg(format!("-DCMAKE_INSTALL_PREFIX={}", prefix.display()))
        .arg(format!("-DCMAKE_PREFIX_PATH={}", prefix.display()))
        .args(options);
    run(&mut configure, env)?;
    run(
        Command::new("cmake").arg("--build").arg(build).args([
            "--config",
            "Release",
            "--parallel",
            &jobs(),
        ]),
        env,
    )?;
    run(
        Command::new("cmake")
            .arg("--install")
            .arg(build)
            .args(["--config", "Release"]),
        env,
    )?;
    println!(
        "ocr-build: {name} built in {:.1} min",
        started.elapsed().as_secs_f64() / 60.0
    );
    Ok(())
}

fn copy_license(src_dir: &Path, names: &[&str], dest: &Path) -> Result<()> {
    for n in names {
        let p = src_dir.join(n);
        if p.is_file() {
            fs::copy(&p, dest).with_context(|| format!("copy {}", p.display()))?;
            return Ok(());
        }
    }
    bail!(
        "no license file ({}) in {}",
        names.join(", "),
        src_dir.display()
    )
}

/// Entry point for `cargo xtask ocr-build [--force]`.
///
/// # Errors
/// An unsupported host, missing tools, a hash mismatch, a failed build, or a
/// linkage violation.
pub fn ocr_build(args: &[String]) -> Result<()> {
    let force = args.iter().any(|a| a == "--force");
    if let Some(bad) = args.iter().find(|a| *a != "--force") {
        bail!("unknown ocr-build option `{bad}`");
    }
    let started = Instant::now();
    let root = crate::engine::repo_root()?;
    let triple = host_triple()?;
    let lock = Lock::parse(&fs::read_to_string(root.join(LOCK_FILE)).context(LOCK_FILE)?)?;
    let base = root.join("target").join("ocr");
    let out = base.join(triple);
    let stamp = format!(
        "tesseract {} {}\nleptonica {} {}\n",
        lock.tesseract_version,
        lock.tesseract.sha256,
        lock.leptonica_version,
        lock.leptonica.sha256
    );
    if !force && fs::read_to_string(out.join(STAMP)).is_ok_and(|s| s == stamp) {
        println!("ocr-build: already built in {}", out.display());
        return Ok(());
    }

    let cache = base.join(".cache");
    let lept_tgz = fetch_verified("ocr-build", LOCK_FILE, &lock.leptonica, &cache, "tar.gz")?;
    let tess_tgz = fetch_verified("ocr-build", LOCK_FILE, &lock.tesseract, &cache, "tar.gz")?;

    let work = base.join(".work").join(triple);
    let _ = fs::remove_dir_all(&work);
    let (lept_src, tess_src) = (work.join("src-leptonica"), work.join("src-tesseract"));
    let prefix = work.join("prefix");
    unpack_source(&lept_tgz, &lept_src)?;
    unpack_source(&tess_tgz, &tess_src)?;

    let env = build_env()?;
    cmake_project(
        "Leptonica",
        &lept_src,
        &work.join("build-leptonica"),
        &prefix,
        &leptonica_options(),
        &env,
    )?;
    cmake_project(
        "Tesseract",
        &tess_src,
        &work.join("build-tesseract"),
        &prefix,
        &tesseract_options(&prefix),
        &env,
    )?;

    let built = prefix.join("bin").join(exe_name());
    if !built.is_file() {
        bail!("the build produced no {}", built.display());
    }
    let violations = linkage_violations(&built, &env)?;
    crate::report(
        "ocr-linkage",
        "tesseract links only operating-system libraries",
        &violations,
    )?;

    let stage = base.join(".stage").join(triple);
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(stage.join("tessdata"))?;
    fs::create_dir_all(stage.join("LICENSES"))?;
    fs::copy(&built, stage.join(exe_name()))?;
    copy_license(
        &tess_src,
        &["LICENSE"],
        &stage.join("LICENSES").join("tesseract-LICENSE.txt"),
    )?;
    copy_license(
        &lept_src,
        &["leptonica-license.txt", "LICENSE"],
        &stage.join("LICENSES").join("leptonica-LICENSE.txt"),
    )?;
    fs::File::create(stage.join(STAMP))?.write_all(stamp.as_bytes())?;
    let _ = fs::remove_dir_all(&out);
    fs::rename(&stage, &out).context("move into target/ocr")?;

    let size = fs::metadata(out.join(exe_name()))?.len();
    println!(
        "ocr-build: tesseract {} + leptonica {} ready in {} ({} KiB program) after {:.1} min",
        lock.tesseract_version,
        lock.leptonica_version,
        out.display(),
        size / 1024,
        started.elapsed().as_secs_f64() / 60.0
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "9218e62793116d42a9f6d14cd9348518b27f382096eea3d0f2d1a24616bb5884";

    fn lock_text(tv: &str, turl: &str) -> String {
        format!(
            "# c\ntesseract = {tv}\nleptonica = 1.87.0\ntesseract {SHA} {turl}\nleptonica {SHA} https://github.com/DanBloomberg/leptonica/releases/download/1.87.0/leptonica-1.87.0.tar.gz\n"
        )
    }

    #[test]
    fn parses_the_committed_lock() {
        let root = crate::engine::repo_root().unwrap();
        let lock = Lock::parse(&fs::read_to_string(root.join(LOCK_FILE)).unwrap()).unwrap();
        assert!(lock.tesseract_version.starts_with("5."));
        assert!(
            lock.tesseract
                .url
                .starts_with("https://github.com/tesseract-ocr/")
        );
        assert!(
            lock.leptonica
                .url
                .starts_with("https://github.com/DanBloomberg/")
        );
    }

    #[test]
    fn refuses_a_lock_whose_url_names_another_version() {
        let good = "https://github.com/tesseract-ocr/tesseract/archive/refs/tags/5.5.3.tar.gz";
        assert!(Lock::parse(&lock_text("5.5.3", good)).is_ok());
        assert!(Lock::parse(&lock_text("5.5.2", good)).is_err());
        assert!(Lock::parse(&lock_text("5.x", good)).is_err());
        assert!(Lock::parse("tesseract = 5.5.3\n").is_err());
    }

    #[test]
    fn every_optional_dependency_is_switched_off_by_name() {
        let l = leptonica_options();
        for o in [
            "-DSW_BUILD=OFF",
            "-DENABLE_ZLIB=OFF",
            "-DENABLE_PNG=OFF",
            "-DENABLE_JPEG=OFF",
            "-DENABLE_TIFF=OFF",
            "-DENABLE_GIF=OFF",
            "-DENABLE_WEBP=OFF",
            "-DENABLE_OPENJPEG=OFF",
            "-DBUILD_SHARED_LIBS=OFF",
        ] {
            assert!(l.iter().any(|x| x == o), "{o}");
        }
        let t = tesseract_options(Path::new("p"));
        for o in [
            "-DBUILD_TRAINING_TOOLS=OFF",
            "-DGRAPHICS_DISABLED=ON",
            "-DOPENMP_BUILD=OFF",
            "-DDISABLE_CURL=ON",
            "-DDISABLE_ARCHIVE=ON",
            "-DDISABLE_TIFF=ON",
            "-DENABLE_NATIVE=OFF",
            "-DDISABLED_LEGACY_ENGINE=OFF",
        ] {
            assert!(t.iter().any(|x| x == o), "{o}");
        }
        for p in [
            "CURL",
            "LibArchive",
            "OpenMP",
            "OpenCL",
            "ZLIB",
            "PkgConfig",
        ] {
            let o = format!("-DCMAKE_DISABLE_FIND_PACKAGE_{p}=ON");
            assert!(l.contains(&o) && t.contains(&o), "{o}");
        }
    }

    #[test]
    fn linkage_allowlist_is_os_libraries_only() {
        assert!(allowed_library("windows", "KERNEL32.dll"));
        assert!(!allowed_library("windows", "VCRUNTIME140.dll"));
        assert!(!allowed_library("windows", "WS2_32.dll"));
        assert!(!allowed_library(
            "windows",
            "api-ms-win-crt-runtime-l1-1-0.dll"
        ));
        assert!(allowed_library("linux", "libc.so.6"));
        assert!(allowed_library("linux", "/lib64/ld-linux-x86-64.so.2"));
        assert!(!allowed_library("linux", "libstdc++.so.6"));
        assert!(!allowed_library("linux", "libcurl.so.4"));
        assert!(!allowed_library("linux", "libgomp.so.1"));
        assert!(allowed_library("macos", "/usr/lib/libSystem.B.dylib"));
        assert!(!allowed_library(
            "macos",
            "/opt/homebrew/opt/leptonica/lib/liblept.5.dylib"
        ));
    }

    #[test]
    fn parses_tool_output() {
        let dumpbin = "Dump of file x.exe\n\nFile Type: EXECUTABLE IMAGE\n\n  Image has the following dependencies:\n\n    KERNEL32.dll\n    ADVAPI32.dll\n\n  Summary\n\n        1000 .data\n";
        assert_eq!(parse_dumpbin(dumpbin), ["KERNEL32.dll", "ADVAPI32.dll"]);
        let ldd = "\tlinux-vdso.so.1 (0x00007ffd)\n\tlibm.so.6 => /lib/x86_64-linux-gnu/libm.so.6 (0x1)\n\t/lib64/ld-linux-x86-64.so.2 (0x2)\n";
        assert_eq!(
            parse_ldd(ldd),
            [
                "linux-vdso.so.1",
                "libm.so.6",
                "/lib64/ld-linux-x86-64.so.2"
            ]
        );
        let otool = "t:\n\t/usr/lib/libc++.1.dylib (compatibility version 1.0.0)\n\t/usr/lib/libSystem.B.dylib (x)\n";
        assert_eq!(
            parse_otool(otool),
            ["/usr/lib/libc++.1.dylib", "/usr/lib/libSystem.B.dylib"]
        );
    }

    #[test]
    fn archive_members_stay_inside_the_output() {
        assert_eq!(
            member_path("tesseract-5.5.3/src/api/a.cpp"),
            Some(PathBuf::from("src/api/a.cpp"))
        );
        assert_eq!(member_path("tesseract-5.5.3/"), None);
        for bad in ["/etc/passwd", "t/../../x", "../x"] {
            assert_eq!(member_path(bad), None, "{bad}");
        }
    }
}
