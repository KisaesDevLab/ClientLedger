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
use std::path::{Path, PathBuf};
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
    /// Handed to the printer.
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
}

/// The OS print hand-off.
#[derive(Debug, Clone)]
pub struct OsPrint<R: CommandRunner> {
    os: Os,
    runner: R,
}

impl OsPrint<SystemRunner> {
    /// The hand-off for this computer.
    #[must_use]
    pub fn system() -> OsPrint<SystemRunner> {
        OsPrint::new(Os::current(), SystemRunner)
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
        OsPrint { os, runner }
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
