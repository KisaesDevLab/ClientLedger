//! Running the `PostgreSQL` helper programs.
//!
//! Every child gets a clean `PG*` environment (so a stray `PGHOST` or
//! `PGPASSWORD` on the owner's machine cannot redirect us), no console window
//! on Windows, no stdin, and captured output that is scrubbed before it can
//! reach an error message.

use crate::{PgError, Result, Secret};
use std::path::Path;
use std::process::{Command, Stdio};

/// Longest output tail kept in an error message.
const TAIL_BYTES: usize = 1500;

/// A command with the standard hygiene applied.
pub(crate) fn command(program: &Path) -> Command {
    let mut c = Command::new(program);
    for (key, _) in std::env::vars_os() {
        if key
            .to_str()
            .is_some_and(|k| k.to_ascii_uppercase().starts_with("PG"))
        {
            c.env_remove(key);
        }
    }
    c.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// Last few lines of `text`, at most [`TAIL_BYTES`] bytes, with secrets removed.
pub(crate) fn tail(text: &str, secret: Option<&Secret>) -> String {
    let scrubbed = secret.map_or_else(|| text.to_string(), |s| s.scrub(text));
    let trimmed = scrubbed.trim();
    if trimmed.len() <= TAIL_BYTES {
        return trimmed.to_string();
    }
    let mut start = trimmed.len() - TAIL_BYTES;
    while !trimmed.is_char_boundary(start) {
        start += 1;
    }
    format!("...{}", &trimmed[start..])
}

/// Result of a finished command.
pub(crate) struct Finished {
    pub success: bool,
    pub code: Option<i32>,
    pub output: String,
}

/// Run to completion, capturing stdout and stderr together.
pub(crate) fn finish(
    mut cmd: Command,
    program: &'static str,
    secret: Option<&Secret>,
) -> Result<Finished> {
    let out = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| PgError::io(format!("start {program}"), e))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok(Finished {
        success: out.status.success(),
        code: out.status.code(),
        output: tail(&text, secret),
    })
}

/// Run and require success.
pub(crate) fn run(cmd: Command, program: &'static str, secret: Option<&Secret>) -> Result<String> {
    let f = finish(cmd, program, secret)?;
    if f.success {
        Ok(f.output)
    } else {
        Err(PgError::Command {
            program,
            status: f
                .code
                .map_or_else(|| "killed".into(), |c| format!("exit {c}")),
            detail: f.output,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_scrubs_and_truncates() {
        let s = Secret::new("sekret");
        assert_eq!(tail("a sekret b", Some(&s)), "a *** b");
        let long = "x".repeat(5000);
        let t = tail(&long, None);
        assert!(t.starts_with("..."));
        assert_eq!(t.len(), TAIL_BYTES + 3);
    }

    #[test]
    fn tail_respects_char_boundaries() {
        let long = "é".repeat(2000);
        let t = tail(&long, None);
        assert!(t.starts_with("..."));
    }

    #[test]
    fn missing_program_is_an_io_error() {
        let cmd = command(Path::new("definitely-not-a-real-program-xyz"));
        let err = run(cmd, "initdb", None).unwrap_err();
        assert!(matches!(err, PgError::Io { .. }), "{err:?}");
    }
}
