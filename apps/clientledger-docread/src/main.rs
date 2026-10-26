//! `clientledger-docread`: the document reading worker (A03, plan D2).
//!
//! `cl-host` starts it with a scrubbed environment and an empty working
//! directory, writes `Batch` frames to stdin and reads
//! `cl_docread::protocol::FromWorker` frames from stdout. It reads untrusted
//! files (PDF through `PDFium`, images in Rust), asks the host to OCR pages
//! (`NeedOcr`), runs the rules and answers `JobDone`/`JobFailed`. A closed
//! stdin ends it.
//!
//! It has no network code and spawns nothing; its limits (`limits.rs`) are
//! set first in `main`, before `PDFium`, threads or stdin. stdout carries
//! frames only; stderr carries fixed outcome lines (`docread: ...`), never
//! a path or document content.
//!
//! Exit codes: 0 stdin closed; 3 the host breached the protocol; 4 the pipe
//! broke; 5 the host was already gone at start (Linux).
#![forbid(unsafe_code)]

// Test hooks crash, hang and allocate on purpose: never in a release build.
// Tests build the worker in the dev profile (or `docread-bench`, which keeps
// debug assertions) with this feature on demand.
#[cfg(all(feature = "test-support", not(debug_assertions)))]
compile_error!(
    "the `test-support` feature is for tests only and must not be enabled in a release build"
);

mod batch;
mod decode;
#[cfg(feature = "test-support")]
mod hooks;
mod limits;
mod pdfium;
mod region;
mod second_read;
#[cfg(test)]
mod test_pdf;

use batch::{Ctx, Fatal, Io, OcrAnswer};
use cl_docread::protocol::{
    Bytes, FrameError, FromWorker, JobFailure, JobId, MAX_FROM_WORKER_FRAME, MAX_JOB_DONE_FRAME,
    MAX_TO_WORKER_FRAME, OcrMode, PROTOCOL_VERSION, ToWorker, read_frame, write_frame,
};
use pdfium_render::prelude::Pdfium;
use std::io::{Read, Write};
use std::process::ExitCode;

/// Frames other than `JobDone` and `NeedOcr` are tiny.
const MAX_SMALL_FRAME: usize = 4 * 1024;

/// The worker's end of the pipe.
struct Pipe<R, W> {
    input: R,
    output: W,
}

impl<R: Read, W: Write> Pipe<R, W> {
    fn read(&mut self) -> Result<ToWorker, FrameError> {
        read_frame(&mut self.input, MAX_TO_WORKER_FRAME)
    }

    /// Send a job's last frame; a `JobDone` over its cap becomes `JobFailed`
    /// (`TooLarge`).
    fn finish(&mut self, msg: &FromWorker) -> Result<(), Fatal> {
        match self.send(msg) {
            Err(Fatal::Protocol) => {
                let FromWorker::JobDone { job, .. } = msg else {
                    return Err(Fatal::Protocol);
                };
                eprintln!("docread: outcome: reading over the frame limit");
                self.send(&FromWorker::JobFailed {
                    job: *job,
                    reason: JobFailure::TooLarge,
                })
            }
            other => other,
        }
    }
}

fn frame_max(msg: &FromWorker) -> usize {
    match msg {
        FromWorker::JobDone { .. } => MAX_JOB_DONE_FRAME,
        FromWorker::NeedOcr { .. } => MAX_FROM_WORKER_FRAME,
        _ => MAX_SMALL_FRAME,
    }
}

impl<R: Read, W: Write> Io for Pipe<R, W> {
    fn send(&mut self, msg: &FromWorker) -> Result<(), Fatal> {
        write_frame(&mut self.output, msg, frame_max(msg)).map_err(|e| match e {
            FrameError::TooLarge { .. } | FrameError::Garbled => Fatal::Protocol,
            _ => Fatal::Pipe,
        })
    }

    fn ocr(
        &mut self,
        job: JobId,
        page: u16,
        pgm: Vec<u8>,
        mode: OcrMode,
    ) -> Result<OcrAnswer, Fatal> {
        self.send(&FromWorker::NeedOcr {
            job,
            page,
            pgm: Bytes(pgm),
            mode,
        })?;
        loop {
            match self.read().map_err(|_| Fatal::Pipe)? {
                ToWorker::OcrResult {
                    job: j,
                    page: p,
                    tsv,
                } if (j, p) == (job, page) => return Ok(OcrAnswer::Tsv(tsv)),
                ToWorker::OcrFailed {
                    job: j,
                    page: p,
                    reason,
                } if (j, p) == (job, page) => return Ok(OcrAnswer::Failed(reason)),
                ToWorker::Render { job, page, .. } => {
                    self.send(&FromWorker::Unsupported { job, page })?;
                }
                _ => {
                    eprintln!("docread: protocol: unexpected frame during ocr");
                    return Err(Fatal::Protocol);
                }
            }
        }
    }
}

/// Serve batches until stdin closes.
fn serve<R: Read, W: Write>(pipe: &mut Pipe<R, W>, pdfium: Option<&Pdfium>) -> Result<(), Fatal> {
    pipe.send(&FromWorker::Ready {
        version: PROTOCOL_VERSION,
    })?;
    loop {
        match pipe.read() {
            Ok(ToWorker::Batch { jobs, options }) => {
                if !batch::batch_within_caps(&jobs) {
                    eprintln!("docread: protocol: batch over a limit");
                    return Err(Fatal::Protocol);
                }
                let ctx = Ctx {
                    pdfium,
                    options,
                    force_ocr: false,
                };
                for job in &jobs {
                    limits::job_started();
                    let done = batch::run_job(&ctx, pipe, job)?;
                    pipe.finish(&done)?;
                }
            }
            Ok(ToWorker::Render { job, page, .. }) => {
                pipe.send(&FromWorker::Unsupported { job, page })?;
            }
            Ok(ToWorker::OcrResult { .. } | ToWorker::OcrFailed { .. }) => {
                eprintln!("docread: protocol: ocr answer outside a job");
                return Err(Fatal::Protocol);
            }
            Err(FrameError::Closed) => return Ok(()),
            Err(FrameError::TooLarge { .. } | FrameError::Garbled) => {
                eprintln!("docread: protocol: bad frame");
                return Err(Fatal::Protocol);
            }
            Err(FrameError::Io(_)) => return Err(Fatal::Pipe),
        }
    }
}

fn main() -> ExitCode {
    let applied = limits::apply();
    for line in limits::report(applied) {
        eprintln!("{line}");
    }
    if applied.orphaned {
        return ExitCode::from(5);
    }
    #[cfg(feature = "test-support")]
    hooks::before_ready();
    let pdfium = pdfium::load();
    if pdfium.is_none() {
        eprintln!("docread: outcome: pdf reader unavailable");
    }
    let mut pipe = Pipe {
        input: std::io::stdin().lock(),
        output: std::io::stdout().lock(),
    };
    match serve(&mut pipe, pdfium.as_ref()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Fatal::Protocol) => ExitCode::from(3),
        Err(Fatal::Pipe) => ExitCode::from(4),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::CalendarDate;
    use cl_docread::protocol::{BatchOptions, Job, JobHint, OcrFailure, Tsv};

    fn frames(msgs: &[ToWorker]) -> Vec<u8> {
        let mut buf = Vec::new();
        for m in msgs {
            write_frame(&mut buf, m, MAX_TO_WORKER_FRAME).unwrap();
        }
        buf
    }

    fn replies(out: &[u8]) -> Vec<FromWorker> {
        let mut r = out;
        let mut v = Vec::new();
        while let Ok(m) = read_frame::<_, FromWorker>(&mut r, MAX_FROM_WORKER_FRAME) {
            v.push(m);
        }
        v
    }

    fn options(ocr: bool) -> BatchOptions {
        BatchOptions {
            ocr,
            today: CalendarDate::from_ymd(2026, 10, 26).unwrap(),
        }
    }

    fn job(id: u32, bytes: &[u8]) -> Job {
        Job {
            job: id,
            bytes: Bytes(bytes.to_vec()),
            hint: JobHint::default(),
            templates: Vec::new(),
            region: None,
        }
    }

    fn run(input: &[u8]) -> (Result<(), Fatal>, Vec<FromWorker>) {
        let mut pipe = Pipe {
            input,
            output: Vec::new(),
        };
        let r = serve(&mut pipe, None);
        (r, replies(&pipe.output))
    }

    fn receipt() -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/docs/receipt.png"),
        )
        .unwrap()
    }

    #[test]
    fn ready_then_each_job_answered_in_order_then_clean_exit() {
        let input = frames(&[
            ToWorker::Render {
                job: 1,
                page: 0,
                dpi: 96,
            },
            ToWorker::Batch {
                jobs: vec![job(1, b""), job(2, &receipt()), job(3, b"junk")],
                options: options(true),
            },
            // The answer to job 2's page.
            ToWorker::OcrFailed {
                job: 2,
                page: 0,
                reason: OcrFailure::TimedOut,
            },
        ]);
        let (r, out) = run(&input);
        assert!(r.is_ok());
        assert_eq!(
            out[0],
            FromWorker::Ready {
                version: PROTOCOL_VERSION
            }
        );
        assert_eq!(out[1], FromWorker::Unsupported { job: 1, page: 0 });
        let kinds: Vec<String> = out[2..]
            .iter()
            .map(|m| match m {
                FromWorker::JobFailed { job, reason } => format!("failed {job} {reason:?}"),
                FromWorker::PageStarted { job, page } => format!("page {job} {page}"),
                FromWorker::NeedOcr { job, .. } => format!("ocr {job}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "failed 1 Empty",
                "page 2 0",
                "ocr 2",
                "failed 2 Unreadable",
                "failed 3 UnsupportedType"
            ]
        );
    }

    #[test]
    fn protocol_breaches_end_the_worker() {
        // An OCR answer for a page that was not asked about.
        let input = frames(&[
            ToWorker::Batch {
                jobs: vec![job(2, &receipt())],
                options: options(true),
            },
            ToWorker::OcrResult {
                job: 2,
                page: 5,
                tsv: Tsv(String::new()),
            },
        ]);
        assert!(matches!(run(&input).0, Err(Fatal::Protocol)));
        let input = frames(&[ToWorker::OcrFailed {
            job: 1,
            page: 0,
            reason: OcrFailure::Crashed,
        }]);
        assert!(matches!(run(&input).0, Err(Fatal::Protocol)));
        let too_many: Vec<Job> = (0..21).map(|i| job(i, b"x")).collect();
        let input = frames(&[ToWorker::Batch {
            jobs: too_many,
            options: options(false),
        }]);
        assert!(matches!(run(&input).0, Err(Fatal::Protocol)));
        let mut garbled = 5u32.to_be_bytes().to_vec();
        garbled.extend_from_slice(b"nope!");
        assert!(matches!(run(&garbled).0, Err(Fatal::Protocol)));
        assert!(matches!(run(&[0, 0]).0, Err(Fatal::Pipe)));
    }

    fn region_job(id: u32, page: u16, b: (u16, u16, u16, u16)) -> Job {
        Job {
            region: Some(cl_docread::Spot {
                page,
                bbox: cl_docread::Bbox::new(b.0, b.1, b.2, b.3).unwrap(),
            }),
            ..job(id, &receipt())
        }
    }

    /// Version 4 (A04): a region the host should never send is a breach,
    /// before any page is touched; a good one is read.
    #[test]
    fn hostile_regions_end_the_worker() {
        let breach = |j: Job| {
            let input = frames(&[ToWorker::Batch {
                jobs: vec![j],
                options: options(true),
            }]);
            let (r, out) = run(&input);
            (matches!(r, Err(Fatal::Protocol)), out.len())
        };
        // Under 1 % on a side; an empty box.
        assert_eq!(breach(region_job(1, 0, (0, 0, 99, 5000))), (true, 1));
        assert_eq!(breach(region_job(1, 0, (0, 0, 5000, 99))), (true, 1));
        assert_eq!(breach(region_job(1, 0, (700, 700, 700, 700))), (true, 1));
        // A page the worker never reads.
        assert_eq!(breach(region_job(1, 10, (0, 0, 5000, 5000))), (true, 1));
        assert_eq!(
            breach(region_job(1, u16::MAX, (0, 0, 5000, 5000))),
            (true, 1)
        );
        // Templates on a region job.
        let mut with_templates = region_job(1, 0, (0, 0, 5000, 5000));
        with_templates.templates = vec![cl_docread::Template {
            fingerprint: cl_docread::model::Fingerprint {
                tokens: Vec::new(),
                aspect: 1,
            },
            fields: Vec::new(),
            line_table: None,
            samples: 1,
        }];
        assert_eq!(breach(with_templates), (true, 1));

        // A sound region on a page past the end fails its own job only.
        let input = frames(&[ToWorker::Batch {
            jobs: vec![region_job(1, 3, (0, 0, 5000, 5000)), job(2, b"junk")],
            options: options(true),
        }]);
        let (r, out) = run(&input);
        assert!(r.is_ok());
        assert_eq!(
            &out[1..],
            [
                FromWorker::JobFailed {
                    job: 1,
                    reason: JobFailure::Unreadable
                },
                FromWorker::JobFailed {
                    job: 2,
                    reason: JobFailure::UnsupportedType
                }
            ]
        );
    }

    #[test]
    fn an_oversized_reading_becomes_too_large() {
        let mut pipe = Pipe {
            input: &b""[..],
            output: Vec::new(),
        };
        let mut reading = cl_docread::Reading::empty(cl_docread::model::Reader::Ocr, 1);
        reading.words = vec![cl_docread::PageWords {
            page: 0,
            aspect: 1,
            source: cl_docread::model::WordSource::Ocr,
            words: (0..20_000)
                .map(|_| cl_docread::Word {
                    text: "w".repeat(200),
                    bbox: cl_docread::Bbox::PAGE,
                    confidence: cl_docread::Confidence::NONE,
                })
                .collect(),
        }];
        let done = FromWorker::JobDone {
            job: 4,
            reading: Box::new(reading),
        };
        pipe.finish(&done).unwrap();
        assert_eq!(
            replies(&pipe.output),
            [FromWorker::JobFailed {
                job: 4,
                reason: JobFailure::TooLarge
            }]
        );
    }
}
