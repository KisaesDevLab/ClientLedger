//! Printing a PDF through the operating system (Q-14b, Q-P19a).
//!
//! `cl-print` makes the bytes; this module, one of the crates allowed to
//! start processes, writes them to a private folder in app data and hands
//! the file to the OS:
//!
//! - **Windows:** the shell's `Print` verb (default printer) or `PrintTo`
//!   verb (a chosen printer) through `powershell.exe Start-Process`, once per
//!   copy. Printer lists and the default printer come from
//!   `Get-CimInstance Win32_Printer`.
//! - **macOS and Linux:** `lp [-d printer] -n copies`; `lpstat -d` for the
//!   default printer and `lpstat -a` (else `-p`) for the list.
//! - **Fallback:** when the print command fails, the PDF opens in the OS
//!   viewer (`Start-Process`, `open`, `xdg-open`) so the owner can print it
//!   from there; when that fails too, the outcome says where the file is.
//!
//! The UI shows a preview first and calls [`OsPrint::print_pdf`] only when
//! the owner clicks Print (owner 2026-10-29): nothing here prints on its
//! own. File paths and printer names reach PowerShell through environment
//! variables, never inside the script text. Calls wait for the command and
//! must run off the UI thread. The commands go through a [`CommandRunner`]
//! so tests never print.

use crate::error::{HostError, Result};
use crate::paths::{AppPaths, Os};
use cl_print::{Document, PageSize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Most copies one hand-off asks for.
pub const MAX_COPIES: u16 = 99;

/// A command to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// The program (looked up on `PATH`).
    pub program: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// Extra environment variables.
    pub env: Vec<(String, String)>,
}

/// What a command reported.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandOutput {
    /// Exit status zero.
    pub success: bool,
    /// Standard output.
    pub stdout: String,
}

/// Runs commands; tests pass a fake.
pub trait CommandRunner {
    /// Run `cmd` to completion.
    ///
    /// # Errors
    /// When the program cannot be started.
    fn run(&self, cmd: &CommandSpec) -> std::io::Result<CommandOutput>;
}

/// The real runner: no console window, no input, output captured.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, cmd: &CommandSpec) -> std::io::Result<CommandOutput> {
        let mut c = std::process::Command::new(&cmd.program);
        c.args(&cmd.args)
            .envs(cmd.env.iter().map(|(k, v)| (k, v)))
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let out = c.output()?;
        Ok(CommandOutput {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        })
    }
}

/// What happened to a print request, for the UI to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrintOutcome {
    /// Printed straight to the printer (Windows print system).
    Printed {
        /// The PDF written alongside (for Save or a later reprint).
        path: PathBuf,
    },
    /// Handed to the OS print command (`lp`, or the PDF app's print verb
    /// on Windows: the app launched; whether it printed is up to it).
    Sent {
        /// The PDF.
        path: PathBuf,
    },
    /// Printing failed; the PDF is open in the viewer to print from there.
    OpenedInViewer {
        /// The PDF.
        path: PathBuf,
    },
    /// Neither worked; the PDF is saved at `path`.
    Failed {
        /// The PDF.
        path: PathBuf,
    },
    /// The printer name can't be passed on safely (a `"` or a control
    /// character, blank, or over 256 characters); nothing was printed.
    InvalidPrinter {
        /// The PDF.
        path: PathBuf,
    },
}

/// Longest printer name passed to the OS.
pub const MAX_PRINTER_NAME: usize = 256;

/// True when `name` may be passed to the print commands: not blank, at
/// most [`MAX_PRINTER_NAME`] characters, no `"` (it would end the quoted
/// `PrintTo` argument) and no control characters.
#[must_use]
pub fn valid_printer_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.chars().count() <= MAX_PRINTER_NAME
        && !name.chars().any(|c| c == '"' || c.is_control())
}

/// Why printing straight to the printer failed (the caller falls back to
/// the PDF hand-off). Messages carry no document content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DirectPrintError {
    /// No printer named and no default printer.
    #[error("there is no default printer")]
    NoPrinter,
    /// The printer could not be opened.
    #[error("the printer could not be opened")]
    OpenPrinter,
    /// The built-in fonts could not be loaded for printing.
    #[error("the print fonts could not be loaded")]
    FontLoad,
    /// A Windows print call failed (the step named).
    #[error("the printer refused the job ({0})")]
    Gdi(&'static str),
    /// The layout could not be resolved.
    #[error("the page could not be laid out: {0}")]
    Layout(cl_print::PrintError),
}

/// Prints a laid-out document straight to a printer (Windows: the
/// Windows print system, Q-P19ae). Tests pass a fake.
pub trait DirectPrinter: Send + Sync {
    /// Print `doc` on `printer` (the default when `None`), `copies` times.
    ///
    /// # Errors
    /// Any [`DirectPrintError`]; the caller falls back to the PDF.
    fn print(
        &self,
        doc: &Document,
        printer: Option<&str>,
        copies: u16,
    ) -> std::result::Result<(), DirectPrintError>;
}

/// The Windows print system (GDI), drawing `cl-print`'s layout with the
/// embedded Plex fonts.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsPrinter;

#[cfg(windows)]
impl DirectPrinter for WindowsPrinter {
    fn print(
        &self,
        doc: &Document,
        printer: Option<&str>,
        copies: u16,
    ) -> std::result::Result<(), DirectPrintError> {
        crate::win_print::print(doc, printer, copies, None)
    }
}

/// Millimetres on the page to printer device units: `dpi` from
/// `LOGPIXELSX/Y`, and the printable area's offset from the physical page
/// (`PHYSICALOFFSETX/Y`), so positions hold against the paper's edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceMap {
    /// Dots per inch across.
    pub dpi_x: i32,
    /// Dots per inch down.
    pub dpi_y: i32,
    /// Unprintable margin on the left, in device units.
    pub offset_x: i32,
    /// Unprintable margin at the top, in device units.
    pub offset_y: i32,
}

/// `v` rounded to the nearest `i32` (clamped). Page geometry only.
#[allow(clippy::cast_possible_truncation)] // Clamped to the i32 range first.
fn round_i32(v: f32) -> i32 {
    if v.is_nan() {
        return 0;
    }
    v.round().clamp(-2_147_483_000.0, 2_147_483_000.0) as i32
}

/// A device count as `f32` (DPI and offsets are far below 2^24, so exact).
#[allow(clippy::cast_precision_loss)]
fn dev_f32(v: i32) -> f32 {
    v as f32
}

impl DeviceMap {
    /// Device x for `mm` from the paper's left edge.
    #[must_use]
    pub fn x(&self, mm: f32) -> i32 {
        round_i32(mm * dev_f32(self.dpi_x) / 25.4) - self.offset_x
    }

    /// Device y for `mm` from the paper's top edge.
    #[must_use]
    pub fn y(&self, mm: f32) -> i32 {
        round_i32(mm * dev_f32(self.dpi_y) / 25.4) - self.offset_y
    }

    /// A horizontal length in device units.
    #[must_use]
    pub fn len_x(&self, mm: f32) -> i32 {
        round_i32(mm * dev_f32(self.dpi_x) / 25.4)
    }

    /// A vertical length in device units.
    #[must_use]
    pub fn len_y(&self, mm: f32) -> i32 {
        round_i32(mm * dev_f32(self.dpi_y) / 25.4)
    }

    /// `LOGFONT` height for `size_pt` (negative: the em size).
    #[must_use]
    pub fn font_height(&self, size_pt: f32) -> i32 {
        -round_i32(size_pt * dev_f32(self.dpi_y) / 72.0).max(1)
    }

    /// Pen width for `width_pt`, at least one dot.
    #[must_use]
    pub fn pen_width(&self, width_pt: f32) -> i32 {
        round_i32(width_pt * dev_f32(self.dpi_x) / 72.0).max(1)
    }

    /// `ExtTextOutW` spacing for glyphs starting at `x_mm` with these
    /// advances (mm), one entry per UTF-16 unit of `text`: each glyph lands
    /// where the layout put it, rounding once per glyph without drift.
    #[must_use]
    pub fn glyph_dx(&self, text: &str, x_mm: f32, advances_mm: &[f32]) -> Vec<i32> {
        let mut out = Vec::with_capacity(text.len());
        let mut pos = x_mm;
        for (c, adv) in text.chars().zip(advances_mm.iter().copied()) {
            let next = pos + adv;
            out.push(self.x(next) - self.x(pos));
            if c.len_utf16() == 2 {
                out.push(0);
            }
            pos = next;
        }
        out
    }

    /// Back to millimetres across (the round trip stays within 0.5 mm).
    #[must_use]
    pub fn x_mm(&self, dev: i32) -> f32 {
        dev_f32(dev + self.offset_x) * 25.4 / dev_f32(self.dpi_x)
    }

    /// Back to millimetres down.
    #[must_use]
    pub fn y_mm(&self, dev: i32) -> f32 {
        dev_f32(dev + self.offset_y) * 25.4 / dev_f32(self.dpi_y)
    }
}

/// The OS print hand-off.
#[derive(Clone)]
pub struct OsPrint<R: CommandRunner> {
    os: Os,
    runner: R,
    direct: Option<Arc<dyn DirectPrinter>>,
}

impl<R: CommandRunner + std::fmt::Debug> std::fmt::Debug for OsPrint<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OsPrint")
            .field("os", &self.os)
            .field("runner", &self.runner)
            .field("direct", &self.direct.is_some())
            .finish()
    }
}

impl OsPrint<SystemRunner> {
    /// The hand-off for this computer: on Windows it prints straight
    /// through the Windows print system first (Q-P19ae).
    #[must_use]
    pub fn system() -> OsPrint<SystemRunner> {
        let os = OsPrint::new(Os::current(), SystemRunner);
        #[cfg(windows)]
        let os = os.with_direct(Arc::new(WindowsPrinter));
        os
    }
}

const FILE_VAR: &str = "CL_PRINT_FILE";
const PRINTER_VAR: &str = "CL_PRINTER";

fn powershell(script: &str, env: Vec<(String, String)>) -> CommandSpec {
    CommandSpec {
        program: "powershell.exe".into(),
        args: vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            format!("$ErrorActionPreference = 'Stop'; {script}"),
        ],
        env,
    }
}

fn plain(program: &str, args: Vec<String>) -> CommandSpec {
    CommandSpec {
        program: program.into(),
        args,
        // lpstat's messages are translated; read them in English.
        env: vec![("LC_ALL".into(), "C".into()), ("LANG".into(), "C".into())],
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl<R: CommandRunner> OsPrint<R> {
    /// The hand-off for `os` through `runner`.
    pub fn new(os: Os, runner: R) -> OsPrint<R> {
        OsPrint {
            os,
            runner,
            direct: None,
        }
    }

    /// Print through `direct` first ([`OsPrint::print_document`]).
    #[must_use]
    pub fn with_direct(mut self, direct: Arc<dyn DirectPrinter>) -> OsPrint<R> {
        self.direct = Some(direct);
        self
    }

    /// Print a laid-out document: straight to the printer when this system
    /// can (Windows, Letter pages), else, or when that fails, hand the
    /// already written `pdf_path` to the OS ([`OsPrint::print_pdf`]: print
    /// verb or `lp`, then the viewer). Envelope-sized documents always go
    /// through the PDF (the direct path does not change the paper size).
    pub fn print_document(
        &self,
        doc: &Document,
        pdf_path: &Path,
        printer: Option<&str>,
        copies: u16,
    ) -> PrintOutcome {
        if printer.is_some_and(|p| !valid_printer_name(p)) {
            return PrintOutcome::InvalidPrinter {
                path: pdf_path.to_path_buf(),
            };
        }
        if let Some(d) = &self.direct
            && doc.size == PageSize::LETTER
        {
            match d.print(doc, printer, copies.clamp(1, MAX_COPIES)) {
                Ok(()) => {
                    tracing::info!(copies, "document printed directly");
                    return PrintOutcome::Printed {
                        path: pdf_path.to_path_buf(),
                    };
                }
                Err(e) => tracing::warn!(error = %e, "direct print failed; using the PDF"),
            }
        }
        self.print_pdf(pdf_path, printer, copies)
    }

    fn ok(&self, cmd: &CommandSpec) -> bool {
        self.runner.run(cmd).is_ok_and(|o| o.success)
    }

    /// The commands that print `path`.
    #[must_use]
    pub fn print_commands(
        &self,
        path: &Path,
        printer: Option<&str>,
        copies: u16,
    ) -> Vec<CommandSpec> {
        let copies = copies.clamp(1, MAX_COPIES);
        let printer = printer.map(str::trim).filter(|p| !p.is_empty());
        match self.os {
            Os::Windows => {
                let mut env = vec![(FILE_VAR.to_string(), path_text(path))];
                let script = if let Some(p) = printer {
                    env.push((PRINTER_VAR.to_string(), p.to_string()));
                    "Start-Process -FilePath $env:CL_PRINT_FILE -Verb PrintTo \
                     -ArgumentList ([char]34 + $env:CL_PRINTER + [char]34) -WindowStyle Hidden"
                } else {
                    "Start-Process -FilePath $env:CL_PRINT_FILE -Verb Print -WindowStyle Hidden"
                };
                (0..copies)
                    .map(|_| powershell(script, env.clone()))
                    .collect()
            }
            Os::MacOs | Os::Unix => {
                let mut args = Vec::new();
                if let Some(p) = printer {
                    args.push("-d".into());
                    args.push(p.to_string());
                }
                args.push("-n".into());
                args.push(copies.to_string());
                args.push("--".into());
                args.push(path_text(path));
                vec![plain("lp", args)]
            }
        }
    }

    /// The command that opens `path` in the OS viewer.
    #[must_use]
    pub fn open_command(&self, path: &Path) -> CommandSpec {
        match self.os {
            Os::Windows => powershell(
                "Start-Process -FilePath $env:CL_PRINT_FILE",
                vec![(FILE_VAR.into(), path_text(path))],
            ),
            Os::MacOs => plain("open", vec![path_text(path)]),
            Os::Unix => plain("xdg-open", vec![path_text(path)]),
        }
    }

    /// Print `path` on `printer` (the default when `None`), `copies` times
    /// (1 to [`MAX_COPIES`]); if printing fails, open it in the viewer.
    pub fn print_pdf(&self, path: &Path, printer: Option<&str>, copies: u16) -> PrintOutcome {
        if printer.is_some_and(|p| !valid_printer_name(p)) {
            return PrintOutcome::InvalidPrinter {
                path: path.to_path_buf(),
            };
        }
        let sent = self
            .print_commands(path, printer, copies)
            .iter()
            .all(|c| self.ok(c));
        if sent {
            tracing::info!(copies, "PDF handed to the printer");
            return PrintOutcome::Sent {
                path: path.to_path_buf(),
            };
        }
        tracing::warn!("print hand-off failed; opening the PDF instead");
        if self.open_pdf(path) {
            PrintOutcome::OpenedInViewer {
                path: path.to_path_buf(),
            }
        } else {
            PrintOutcome::Failed {
                path: path.to_path_buf(),
            }
        }
    }

    /// Open `path` in the OS viewer; true when that worked.
    pub fn open_pdf(&self, path: &Path) -> bool {
        self.ok(&self.open_command(path))
    }

    /// The default printer's name, if there is one.
    pub fn default_printer_name(&self) -> Option<String> {
        match self.os {
            Os::Windows => {
                let out = self
                    .runner
                    .run(&powershell(
                        "(Get-CimInstance Win32_Printer -Filter 'Default=true' | Select-Object -First 1).Name",
                        vec![],
                    ))
                    .ok()
                    .filter(|o| o.success)?;
                out.stdout
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .map(str::to_string)
            }
            Os::MacOs | Os::Unix => {
                let out = self
                    .runner
                    .run(&plain("lpstat", vec!["-d".into()]))
                    .ok()
                    .filter(|o| o.success)?;
                parse_lpstat_default(&out.stdout)
            }
        }
    }

    /// The installed printers' names (empty when none or unknown).
    pub fn list_printers(&self) -> Vec<String> {
        match self.os {
            Os::Windows => self
                .runner
                .run(&powershell(
                    "Get-CimInstance Win32_Printer | ForEach-Object { $_.Name }",
                    vec![],
                ))
                .ok()
                .filter(|o| o.success)
                .map(|o| {
                    o.stdout
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            Os::MacOs | Os::Unix => {
                if let Some(o) = self
                    .runner
                    .run(&plain("lpstat", vec!["-a".into()]))
                    .ok()
                    .filter(|o| o.success)
                {
                    let names = first_words(&o.stdout, 0);
                    if !names.is_empty() {
                        return names;
                    }
                }
                self.runner
                    .run(&plain("lpstat", vec!["-p".into()]))
                    .ok()
                    .filter(|o| o.success)
                    .map(|o| {
                        o.stdout
                            .lines()
                            .filter(|l| l.starts_with("printer "))
                            .filter_map(|l| l.split_whitespace().nth(1))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default()
            }
        }
    }
}

/// `system default destination: Office` → `Office`.
fn parse_lpstat_default(out: &str) -> Option<String> {
    out.lines().find_map(|l| {
        let l = l.trim();
        if l.starts_with("no system default") {
            return None;
        }
        l.split_once(": ")
            .map(|(_, name)| name.trim().to_string())
            .filter(|n| !n.is_empty())
    })
}

/// Word `idx` of each non-empty line.
fn first_words(out: &str, idx: usize) -> Vec<String> {
    out.lines()
        .filter_map(|l| l.split_whitespace().nth(idx))
        .map(str::to_string)
        .collect()
}

static SEQ: AtomicU32 = AtomicU32::new(0);

/// Write `bytes` to a new PDF in the app data print folder. `kind` names the
/// file (`checks`, `invoice`; letters, digits and `-` only; never a payee
/// or customer name).
///
/// # Errors
/// When the folder or file cannot be written.
pub fn write_temp_pdf(paths: &AppPaths, kind: &str, bytes: &[u8]) -> Result<PathBuf> {
    let dir = paths.print_dir();
    crate::paths::create_private_dir(&dir)?;
    let kind: String = kind
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(32)
        .collect();
    let kind = if kind.is_empty() {
        "document".to_string()
    } else {
        kind
    };
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{kind}-{millis}-{}-{n}.pdf", std::process::id()));
    std::fs::write(&path, bytes).map_err(|e| HostError::io("write the PDF to print", e))?;
    Ok(path)
}

/// Delete PDFs in the print folder older than `age`; returns how many.
/// (A viewer may still have a recent one open, so only old files go.)
#[must_use]
pub fn sweep_print_dir(paths: &AppPaths, age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(paths.print_dir()) else {
        return 0;
    };
    let now = SystemTime::now();
    entries
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("pdf"))
        })
        .filter(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| now.duration_since(t).is_ok_and(|d| d >= age))
        })
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    /// Records commands; answers from a script (default: success, no output).
    #[derive(Default)]
    struct Fake {
        calls: RefCell<Vec<CommandSpec>>,
        answers: RefCell<VecDeque<std::io::Result<CommandOutput>>>,
    }

    impl Fake {
        fn answering(answers: Vec<std::io::Result<CommandOutput>>) -> Fake {
            Fake {
                calls: RefCell::default(),
                answers: RefCell::new(answers.into()),
            }
        }
    }

    impl CommandRunner for &Fake {
        fn run(&self, cmd: &CommandSpec) -> std::io::Result<CommandOutput> {
            self.calls.borrow_mut().push(cmd.clone());
            self.answers.borrow_mut().pop_front().unwrap_or_else(|| {
                Ok(CommandOutput {
                    success: true,
                    stdout: String::new(),
                })
            })
        }
    }

    #[allow(clippy::unnecessary_wraps)] // Matches the runner signature.
    fn ok(stdout: &str) -> std::io::Result<CommandOutput> {
        Ok(CommandOutput {
            success: true,
            stdout: stdout.into(),
        })
    }

    #[allow(clippy::unnecessary_wraps)]
    fn fail() -> std::io::Result<CommandOutput> {
        Ok(CommandOutput {
            success: false,
            stdout: String::new(),
        })
    }

    fn missing() -> std::io::Result<CommandOutput> {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no such program",
        ))
    }

    const PDF: &str = "C:\\Users\\o'brien & co\\print\\checks-1.pdf";

    #[test]
    fn windows_prints_with_the_print_verb_and_the_path_in_the_environment() {
        let fake = Fake::default();
        let os = OsPrint::new(Os::Windows, &fake);
        assert_eq!(
            os.print_pdf(Path::new(PDF), None, 1),
            PrintOutcome::Sent { path: PDF.into() }
        );
        let calls = fake.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].program, "powershell.exe");
        let script = calls[0].args.last().unwrap();
        assert!(script.contains("-Verb Print ") && script.contains("$env:CL_PRINT_FILE"));
        assert!(
            !script.contains("o'brien"),
            "the path never enters the script"
        );
        assert_eq!(calls[0].env, vec![(FILE_VAR.to_string(), PDF.to_string())]);
    }

    #[test]
    fn windows_uses_print_to_for_a_chosen_printer_once_per_copy() {
        let fake = Fake::default();
        let os = OsPrint::new(Os::Windows, &fake);
        assert!(matches!(
            os.print_pdf(Path::new(PDF), Some("Office Laser"), 3),
            PrintOutcome::Sent { .. }
        ));
        let calls = fake.calls.borrow();
        assert_eq!(calls.len(), 3);
        for c in calls.iter() {
            let script = c.args.last().unwrap();
            assert!(script.contains("-Verb PrintTo") && script.contains("$env:CL_PRINTER"));
            assert!(!script.contains("Office Laser"));
            assert!(
                c.env
                    .contains(&(PRINTER_VAR.to_string(), "Office Laser".to_string()))
            );
        }
    }

    #[test]
    fn unix_and_mac_use_lp_with_printer_and_copies() {
        for os_kind in [Os::Unix, Os::MacOs] {
            let fake = Fake::default();
            let os = OsPrint::new(os_kind, &fake);
            os.print_pdf(Path::new("/tmp/a b.pdf"), Some("Office"), 2);
            os.print_pdf(Path::new("/tmp/a b.pdf"), None, 0);
            let calls = fake.calls.borrow();
            assert_eq!(calls[0].program, "lp");
            assert_eq!(
                calls[0].args,
                ["-d", "Office", "-n", "2", "--", "/tmp/a b.pdf"]
            );
            assert_eq!(calls[1].args, ["-n", "1", "--", "/tmp/a b.pdf"]);
        }
        let fake = Fake::default();
        OsPrint::new(Os::Unix, &fake).print_pdf(Path::new("/x.pdf"), None, 500);
        assert_eq!(fake.calls.borrow()[0].args[1], "99");
    }

    #[test]
    fn a_failed_print_opens_the_viewer_and_both_failing_reports_the_file() {
        let fake = Fake::answering(vec![fail(), ok("")]);
        let os = OsPrint::new(Os::Unix, &fake);
        assert_eq!(
            os.print_pdf(Path::new("/x.pdf"), None, 1),
            PrintOutcome::OpenedInViewer {
                path: "/x.pdf".into()
            }
        );
        assert_eq!(fake.calls.borrow()[1].program, "xdg-open");

        let fake = Fake::answering(vec![missing(), fail()]);
        let os = OsPrint::new(Os::MacOs, &fake);
        assert_eq!(
            os.print_pdf(Path::new("/x.pdf"), None, 1),
            PrintOutcome::Failed {
                path: "/x.pdf".into()
            }
        );
        assert_eq!(fake.calls.borrow()[1].program, "open");

        // Windows: the second copy failing still falls back.
        let fake = Fake::answering(vec![ok(""), fail(), ok("")]);
        let os = OsPrint::new(Os::Windows, &fake);
        assert!(matches!(
            os.print_pdf(Path::new(PDF), None, 2),
            PrintOutcome::OpenedInViewer { .. }
        ));
        let calls = fake.calls.borrow();
        assert!(
            calls[2]
                .args
                .last()
                .unwrap()
                .ends_with("Start-Process -FilePath $env:CL_PRINT_FILE")
        );
    }

    #[test]
    fn printer_names_that_cannot_be_passed_safely_are_refused() {
        assert!(valid_printer_name("Office Laser"));
        assert!(valid_printer_name(r"\\server\HP 400"));
        for bad in ["", "  ", "Office\" & calc", "a\nb"] {
            assert!(!valid_printer_name(bad), "{bad:?}");
        }
        assert!(!valid_printer_name(&"x".repeat(MAX_PRINTER_NAME + 1)));
        let fake = Fake::default();
        let os = OsPrint::new(Os::Windows, &fake);
        assert_eq!(
            os.print_pdf(Path::new(PDF), Some("Office\" & calc"), 1),
            PrintOutcome::InvalidPrinter { path: PDF.into() }
        );
        assert_eq!(fake.calls.borrow().len(), 0);
    }

    struct FakeDirect {
        ok: bool,
        calls: std::sync::Mutex<Vec<(Option<String>, u16)>>,
    }

    impl FakeDirect {
        fn new(ok: bool) -> Arc<FakeDirect> {
            Arc::new(FakeDirect {
                ok,
                calls: std::sync::Mutex::default(),
            })
        }
    }

    impl DirectPrinter for FakeDirect {
        fn print(
            &self,
            _doc: &Document,
            printer: Option<&str>,
            copies: u16,
        ) -> std::result::Result<(), DirectPrintError> {
            self.calls
                .lock()
                .unwrap()
                .push((printer.map(str::to_string), copies));
            if self.ok {
                Ok(())
            } else {
                Err(DirectPrintError::Gdi("StartDoc"))
            }
        }
    }

    fn letter_doc() -> Document {
        Document {
            pages: vec![cl_print::Page::default()],
            ..Document::default()
        }
    }

    #[test]
    fn documents_print_directly_first_and_fall_back_to_the_pdf() {
        let direct = FakeDirect::new(true);
        let fake = Fake::default();
        let os = OsPrint::new(Os::Windows, &fake).with_direct(direct.clone());
        assert_eq!(
            os.print_document(&letter_doc(), Path::new(PDF), Some("Office Laser"), 200),
            PrintOutcome::Printed { path: PDF.into() }
        );
        assert_eq!(
            direct.calls.lock().unwrap()[0],
            (Some("Office Laser".into()), MAX_COPIES)
        );
        assert_eq!(fake.calls.borrow().len(), 0);

        // Direct fails: the print verb runs.
        let fake = Fake::default();
        let os = OsPrint::new(Os::Windows, &fake).with_direct(FakeDirect::new(false));
        assert_eq!(
            os.print_document(&letter_doc(), Path::new(PDF), None, 1),
            PrintOutcome::Sent { path: PDF.into() }
        );
        assert!(
            fake.calls.borrow()[0]
                .args
                .last()
                .unwrap()
                .contains("-Verb Print")
        );

        // Envelopes skip the direct path.
        let direct = FakeDirect::new(true);
        let fake = Fake::default();
        let os = OsPrint::new(Os::Windows, &fake).with_direct(direct.clone());
        let env = Document {
            size: PageSize::ENVELOPE_10,
            ..letter_doc()
        };
        assert!(matches!(
            os.print_document(&env, Path::new(PDF), None, 1),
            PrintOutcome::Sent { .. }
        ));
        assert_eq!(direct.calls.lock().unwrap().len(), 0);

        // Bad printer names reach neither path.
        assert!(matches!(
            os.print_document(&letter_doc(), Path::new(PDF), Some("a\"b"), 1),
            PrintOutcome::InvalidPrinter { .. }
        ));
        assert_eq!(direct.calls.lock().unwrap().len(), 0);

        // Without a direct printer (macOS, Linux) it is the PDF path.
        let fake = Fake::default();
        let os = OsPrint::new(Os::Unix, &fake);
        assert!(matches!(
            os.print_document(&letter_doc(), Path::new("/x.pdf"), None, 1),
            PrintOutcome::Sent { .. }
        ));
    }

    #[test]
    fn millimetres_map_to_device_units_within_half_a_millimetre() {
        let points = [
            0.0, 12.0, 17.0, 25.4, 30.0, 49.5, 88.9, 165.0, 177.8, 203.2, 215.9, 279.4,
        ];
        for (dpi, off) in [(300, 50), (600, 100), (1200, 211), (600, 0)] {
            let m = DeviceMap {
                dpi_x: dpi,
                dpi_y: dpi,
                offset_x: off,
                offset_y: off + 7,
            };
            for mm in points {
                assert!((m.x_mm(m.x(mm)) - mm).abs() <= 0.5, "{dpi} {mm}");
                assert!((m.y_mm(m.y(mm)) - mm).abs() <= 0.5, "{dpi} {mm}");
            }
            assert!(m.pen_width(0.1) >= 1);
        }
        let m = DeviceMap {
            dpi_x: 600,
            dpi_y: 600,
            offset_x: 50,
            offset_y: 60,
        };
        assert_eq!(m.x(25.4), 550);
        assert_eq!(m.y(25.4), 540);
        assert_eq!(m.len_x(25.4), 600);
        assert_eq!(m.len_y(2.54), 60);
        assert_eq!(m.font_height(12.0), -100);
        assert_eq!(m.pen_width(0.5), 4);
    }

    #[test]
    fn glyph_spacing_adds_up_to_the_layout_width_without_drift() {
        let m = DeviceMap {
            dpi_x: 600,
            dpi_y: 600,
            offset_x: 37,
            offset_y: 0,
        };
        let text = "9,999,999,999.99";
        let font = cl_print::Font::Mono;
        let adv: Vec<f32> = text.chars().map(|c| font.advance_mm(c, 10.0)).collect();
        let x0 = 167.3;
        let dx = m.glyph_dx(text, x0, &adv);
        assert_eq!(dx.len(), text.len());
        let total: i32 = dx.iter().sum();
        assert_eq!(total, m.x(x0 + font.width_mm(text, 10.0)) - m.x(x0));
        // A character outside the BMP takes two UTF-16 units.
        assert_eq!(m.glyph_dx("a\u{1F600}", 0.0, &[1.0, 2.0]).len(), 3);
    }

    #[test]
    fn default_printer_is_read_from_each_system() {
        let fake = Fake::answering(vec![ok("\r\nOffice Laser\r\n")]);
        assert_eq!(
            OsPrint::new(Os::Windows, &fake)
                .default_printer_name()
                .as_deref(),
            Some("Office Laser")
        );
        assert!(
            fake.calls.borrow()[0]
                .args
                .last()
                .unwrap()
                .contains("Default=true")
        );
        let fake = Fake::answering(vec![ok("system default destination: Back_Room\n")]);
        assert_eq!(
            OsPrint::new(Os::Unix, &fake)
                .default_printer_name()
                .as_deref(),
            Some("Back_Room")
        );
        assert_eq!(fake.calls.borrow()[0].args, ["-d"]);
        assert!(
            fake.calls.borrow()[0]
                .env
                .contains(&("LC_ALL".into(), "C".into()))
        );
        let fake = Fake::answering(vec![ok("no system default destination\n")]);
        assert_eq!(OsPrint::new(Os::MacOs, &fake).default_printer_name(), None);
        let fake = Fake::answering(vec![fail()]);
        assert_eq!(
            OsPrint::new(Os::Windows, &fake).default_printer_name(),
            None
        );
        let fake = Fake::answering(vec![ok("  \n")]);
        assert_eq!(
            OsPrint::new(Os::Windows, &fake).default_printer_name(),
            None
        );
    }

    #[test]
    fn printers_are_listed_from_each_system() {
        let fake = Fake::answering(vec![ok("Office Laser\r\nMicrosoft Print to PDF\r\n\r\n")]);
        assert_eq!(
            OsPrint::new(Os::Windows, &fake).list_printers(),
            ["Office Laser", "Microsoft Print to PDF"]
        );
        let fake = Fake::answering(vec![ok(
            "Office accepting requests since Mon\nBack_Room accepting requests since Tue\n",
        )]);
        assert_eq!(
            OsPrint::new(Os::Unix, &fake).list_printers(),
            ["Office", "Back_Room"]
        );
        let fake = Fake::answering(vec![
            fail(),
            ok("printer Office is idle.  enabled since Mon\nother line\n"),
        ]);
        assert_eq!(OsPrint::new(Os::MacOs, &fake).list_printers(), ["Office"]);
        assert_eq!(fake.calls.borrow()[1].args, ["-p"]);
        let fake = Fake::answering(vec![missing(), missing()]);
        assert_eq!(OsPrint::new(Os::Unix, &fake).list_printers().len(), 0);
    }

    #[test]
    fn temp_pdfs_go_to_the_print_folder_with_safe_names_and_old_ones_are_swept() {
        let dir = tempfile_dir("os-print");
        let paths = AppPaths::at(&dir).unwrap();
        let a = write_temp_pdf(&paths, "Checks", b"%PDF-1.7").unwrap();
        let b = write_temp_pdf(&paths, "Acme & Sons/..", b"%PDF-1.7").unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with(paths.print_dir()));
        let name = b.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("acme---sons---") && b.extension().is_some_and(|x| x == "pdf"),
            "{name}"
        );
        assert_eq!(std::fs::read(&a).unwrap(), b"%PDF-1.7");
        assert_eq!(sweep_print_dir(&paths, Duration::from_secs(3600)), 0);
        assert_eq!(sweep_print_dir(&paths, Duration::ZERO), 2);
        assert!(!a.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tempfile_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "cl-host-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
