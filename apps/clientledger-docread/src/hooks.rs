//! Fault-injection hooks for cl-host's worker tests (feature
//! `test-support`, debug builds only; plan D2 "Test hooks", Q-A03t).
//!
//! A document starting `%CL-TEST-HOOK:<name>` (to the end of its first
//! line) is a hook. Without the feature such a file is just unreadable.
//!
//! | Hook | Does |
//! | --- | --- |
//! | `crash` | aborts the process |
//! | `hang-load` | hangs before its first `PageStarted` |
//! | `hang-page` | sends `PageStarted` for page 0, then hangs |
//! | `commit:<MiB>` | commits that much memory without touching it, then hangs |
//! | `rss:<MiB>` | allocates and touches that much memory, then hangs |
//! | `hold:<MiB>` | allocates, touches and keeps that much, then fails the job (`Unreadable`) |
//! | `big-frame` | writes a frame length over every cap, then hangs |
//! | `garble` | writes a frame that is not JSON, then hangs |
//! | `bad-pgm` | asks for OCR of a page that is not a PGM: `Unreadable` when the host refuses it with `BadImage`, `Empty` otherwise |
//! | `force-ocr` | reads the rest of the file (after the hook line) with every page sent to OCR |
//! | `spawn` | tries to start a program (this one): `Unreadable` when that is refused (the Windows job allows one process), `Empty` when it started |
//! | `region-lie` | answers a job with a word outside its region (a region job's reply the host must refuse) |
//!
//! Before `Ready`, `CL_DOCREAD_TEST_READY=hang` hangs, `=exit` exits and
//! `=old` announces the previous protocol version, then hangs (the host's
//! start-up tests).

use crate::batch::{Ctx, Fatal, Io, OcrAnswer, run_bytes};
use cl_docread::model::{Bbox, Confidence, PageWords, Reader, Reading, Word, WordSource};
use cl_docread::protocol::{Bytes, FromWorker, Job, JobFailure, OcrFailure};
use std::io::Write;
use std::time::Duration;

const PREFIX: &[u8] = b"%CL-TEST-HOOK:";

/// One hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    Crash,
    HangLoad,
    HangPage,
    Commit(usize),
    Rss(usize),
    Hold(usize),
    BigFrame,
    Garble,
    BadPgm,
    ForceOcr,
    Spawn,
    RegionLie,
}

/// The hook a document names, and the bytes after its first line.
#[must_use]
pub fn parse(bytes: &[u8]) -> Option<(Hook, &[u8])> {
    let rest = bytes.strip_prefix(PREFIX)?;
    let end = rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
    let name = std::str::from_utf8(&rest[..end]).ok()?.trim();
    let after = rest.get(end + 1..).unwrap_or(&[]);
    let mib = |s: &str| s.parse::<usize>().ok().map(|n| n * 1024 * 1024);
    let hook = match name.split_once(':') {
        None => match name {
            "crash" => Hook::Crash,
            "hang-load" => Hook::HangLoad,
            "hang-page" => Hook::HangPage,
            "big-frame" => Hook::BigFrame,
            "garble" => Hook::Garble,
            "bad-pgm" => Hook::BadPgm,
            "force-ocr" => Hook::ForceOcr,
            "spawn" => Hook::Spawn,
            "region-lie" => Hook::RegionLie,
            _ => return None,
        },
        Some(("commit", n)) => Hook::Commit(mib(n)?),
        Some(("rss", n)) => Hook::Rss(mib(n)?),
        Some(("hold", n)) => Hook::Hold(mib(n)?),
        Some(_) => return None,
    };
    Some((hook, after))
}

fn hang() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

/// Every 4 KiB page of `v` written, so it counts as resident.
fn touch(v: &mut [u8]) {
    for i in (0..v.len()).step_by(4096) {
        v[i] = 1;
    }
}

/// Run a hook for `job`.
///
/// # Errors
/// The pipe broke.
pub fn run(
    hook: Hook,
    rest: &[u8],
    ctx: &Ctx<'_>,
    io: &mut dyn Io,
    job: &Job,
) -> Result<FromWorker, Fatal> {
    match hook {
        Hook::Crash => std::process::abort(),
        Hook::HangLoad => hang(),
        Hook::HangPage => {
            io.send(&FromWorker::PageStarted {
                job: job.job,
                page: 0,
            })?;
            hang()
        }
        Hook::Commit(n) => {
            // Zeroed: committed by the allocator, never touched.
            let v = vec![0u8; n];
            std::hint::black_box(&v);
            hang()
        }
        Hook::Rss(n) => {
            let mut v = vec![0u8; n];
            touch(&mut v);
            std::hint::black_box(&v);
            hang()
        }
        Hook::Hold(n) => {
            let mut v = vec![0u8; n];
            touch(&mut v);
            std::hint::black_box(&v);
            std::mem::forget(v);
            Ok(FromWorker::JobFailed {
                job: job.job,
                reason: JobFailure::Unreadable,
            })
        }
        Hook::BigFrame => {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(&u32::MAX.to_be_bytes());
            let _ = out.flush();
            hang()
        }
        Hook::Garble => {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(&5u32.to_be_bytes());
            let _ = out.write_all(b"nope!");
            let _ = out.flush();
            hang()
        }
        Hook::BadPgm => {
            io.send(&FromWorker::PageStarted {
                job: job.job,
                page: 0,
            })?;
            let reason = match io.ocr(
                job.job,
                0,
                b"P6\n1 1\n255\n\0\0\0".to_vec(),
                cl_docread::protocol::OcrMode::Read,
            )? {
                OcrAnswer::Failed(OcrFailure::BadImage) => JobFailure::Unreadable,
                _ => JobFailure::Empty,
            };
            Ok(FromWorker::JobFailed {
                job: job.job,
                reason,
            })
        }
        Hook::Spawn => {
            // The only `process::Command` in the worker (xtask docread-check
            // allows it in this file only, which release builds never compile).
            let started = std::env::current_exe().and_then(|exe| {
                std::process::Command::new(exe)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
            });
            let reason = match started {
                Ok(mut child) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    JobFailure::Empty
                }
                Err(_) => JobFailure::Unreadable,
            };
            Ok(FromWorker::JobFailed {
                job: job.job,
                reason,
            })
        }
        Hook::RegionLie => {
            let page = job.region.map_or(0, |r| r.page);
            io.send(&FromWorker::PageStarted { job: job.job, page })?;
            let mut reading = Reading::empty(Reader::Ocr, page + 1);
            reading.words = vec![PageWords {
                page,
                aspect: 12_941,
                source: WordSource::Ocr,
                words: vec![Word {
                    text: "999.99".into(),
                    bbox: Bbox::PAGE,
                    confidence: Confidence::CERTAIN,
                }],
            }];
            Ok(FromWorker::JobDone {
                job: job.job,
                reading: Box::new(reading),
            })
        }
        Hook::ForceOcr => {
            let forced = Ctx {
                pdfium: ctx.pdfium,
                options: ctx.options,
                force_ocr: true,
            };
            let inner = Job {
                job: job.job,
                bytes: Bytes(Vec::new()),
                hint: job.hint,
                templates: job.templates.clone(),
                region: job.region,
            };
            run_bytes(&forced, io, &inner, rest)
        }
    }
}

/// The start-up hook (`CL_DOCREAD_TEST_READY`).
pub fn before_ready() {
    match std::env::var("CL_DOCREAD_TEST_READY").as_deref() {
        Ok("hang") => hang(),
        Ok("exit") => std::process::exit(9),
        Ok("old") => {
            let mut out = std::io::stdout().lock();
            let _ = cl_docread::protocol::write_frame(
                &mut out,
                &FromWorker::Ready {
                    version: cl_docread::protocol::PROTOCOL_VERSION - 1,
                },
                1024,
            );
            hang()
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_names_parse() {
        assert_eq!(parse(b"%CL-TEST-HOOK:crash"), Some((Hook::Crash, &b""[..])));
        assert_eq!(
            parse(b"%CL-TEST-HOOK:commit:1536\n"),
            Some((Hook::Commit(1536 * 1024 * 1024), &b""[..]))
        );
        assert_eq!(
            parse(b"%CL-TEST-HOOK:force-ocr\n%PDF-1.4"),
            Some((Hook::ForceOcr, &b"%PDF-1.4"[..]))
        );
        assert_eq!(parse(b"%CL-TEST-HOOK:spawn"), Some((Hook::Spawn, &b""[..])));
        assert_eq!(
            parse(b"%CL-TEST-HOOK:region-lie"),
            Some((Hook::RegionLie, &b""[..]))
        );
        assert_eq!(parse(b"%CL-TEST-HOOK:rss:x"), None);
        assert_eq!(parse(b"%CL-TEST-HOOK:nope"), None);
        assert_eq!(parse(b"%PDF-1.4"), None);
    }
}
