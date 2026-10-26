//! The reading worker under cl-host (A03 plan D2, §6 "Worker and host",
//! AC5, T14): hostile files and fault hooks each fail their own job, the
//! worker is replaced and the next job is read.
//!
//! The worker is built on demand with its `test-support` hooks
//! (`common::docread_worker`). Tests that need `PDFium` or tesseract skip when
//! they are missing unless `CL_PDFIUM_REQUIRE=1` / `CL_OCR_REQUIRE=1`.

mod common;

use cl_core::CalendarDate;
use cl_docread::model::{Reader, WordSource};
use cl_docread::protocol::{JobFailure, JobHint, MAX_DOCUMENT_BYTES};
use cl_host::{CouldNotRead, DocJob, DocreadConfig, DocreadHost, JobOutcome, OcrTools};
use common::{Scratch, pdf, pdfium_present};
use std::path::Path;
use std::time::{Duration, Instant};

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 10, 26).unwrap()
}

fn config(s: &Scratch) -> DocreadConfig {
    let mut c = DocreadConfig::new(&s.paths(), common::docread_worker(), None);
    c.limits.poll = Duration::from_millis(50);
    c
}

fn doc(bytes: Vec<u8>) -> DocJob {
    DocJob {
        bytes,
        hint: JobHint::default(),
        templates: Vec::new(),
    }
}

fn hook(name: &str) -> DocJob {
    doc(format!("%CL-TEST-HOOK:{name}\n").into_bytes())
}

/// A file the worker answers at once (`UnsupportedType`): proves it runs.
fn junk() -> DocJob {
    doc(b"not a document".to_vec())
}

const UNSUPPORTED: JobOutcome = JobOutcome::Failed(JobFailure::UnsupportedType);

/// Scratch folders and child records left in the docread folder (the
/// empty `.pids` folder itself stays).
fn leftovers(s: &Scratch) -> usize {
    let dir = s.paths().docread_dir();
    let folders = std::fs::read_dir(&dir).map_or(0, |d| {
        d.flatten().filter(|e| e.file_name() != ".pids").count()
    });
    let records = std::fs::read_dir(dir.join(".pids")).map_or(0, Iterator::count);
    folders + records
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/docs")
            .join(name),
    )
    .unwrap()
}

fn text_pdf() -> Vec<u8> {
    let body: String = (0..30)
        .map(|i| pdf::text(72, 760 - i * 20, 10, &format!("Widget {i} 2 x 3.00 6.00")))
        .collect();
    pdf::build(&[body])
}

fn could_not(o: &JobOutcome) -> Option<CouldNotRead> {
    match o {
        JobOutcome::CouldNotRead(c) => Some(*c),
        _ => None,
    }
}

#[test]
fn plain_and_hostile_files_fail_their_own_job() {
    let s = Scratch::new("docread-plain");
    let mut host = DocreadHost::new(config(&s));
    let pdfium = pdfium_present();
    let mut jobs = vec![
        doc(Vec::new()),
        doc(vec![b'%'; MAX_DOCUMENT_BYTES + 1]),
        junk(),
        doc(fixture("bomb_30000.png")),
        doc(fixture("encrypted.pdf")),
        doc(fixture("receipt.png")),
    ];
    if pdfium {
        jobs.push(doc(text_pdf()));
    }
    let out = host.read(&jobs, today());
    assert_eq!(out[0], JobOutcome::Failed(JobFailure::Empty));
    assert_eq!(out[1], JobOutcome::Failed(JobFailure::TooLarge));
    assert_eq!(out[2], UNSUPPORTED);
    assert_eq!(out[3], JobOutcome::Failed(JobFailure::TooLarge));
    let protected = if pdfium {
        JobFailure::Protected
    } else {
        JobFailure::Unreadable
    };
    assert_eq!(out[4], JobOutcome::Failed(protected));
    // No OCR configured: a picture needs it.
    assert_eq!(out[5], JobOutcome::Failed(JobFailure::NeedsOcr));
    if pdfium {
        let JobOutcome::Read(r) = &out[6] else {
            panic!("{:?}", out[6])
        };
        assert_eq!(r.reader, Reader::TextPdf);
        assert_eq!(r.words[0].source, WordSource::TextLayer);
        assert!(r.words[0].words.len() >= 150);
    }
    assert_eq!(host.stats().workers_started, 1);
    assert_eq!(host.stats().kills, 0);
    // The worker's scratch folder is gone.
    let left = leftovers(&s);
    assert_eq!(left, 0);
}

#[test]
fn a_crash_is_retried_once_then_could_not_read_and_the_next_job_is_read() {
    let s = Scratch::new("docread-crash");
    let mut host = DocreadHost::new(config(&s));
    let out = host.read(&[hook("crash"), junk()], today());
    assert_eq!(could_not(&out[0]), Some(CouldNotRead::Crashed));
    assert_eq!(out[1], UNSUPPORTED);
    let st = host.stats();
    assert_eq!((st.kills, st.workers_started), (2, 3));
}

#[test]
fn a_load_that_hangs_meets_the_first_page_deadline() {
    let s = Scratch::new("docread-hang-load");
    let mut c = config(&s);
    c.limits.first_page = Duration::from_millis(800);
    let mut host = DocreadHost::new(c);
    let started = Instant::now();
    let out = host.read(&[hook("hang-load"), junk()], today());
    assert_eq!(could_not(&out[0]), Some(CouldNotRead::TimedOut));
    assert_eq!(out[1], UNSUPPORTED);
    assert!(started.elapsed() < Duration::from_secs(30));
}

#[test]
fn a_page_that_hangs_meets_the_page_limit() {
    let s = Scratch::new("docread-hang-page");
    let mut c = config(&s);
    c.limits.per_page = Duration::from_millis(800);
    let mut host = DocreadHost::new(c);
    let out = host.read(&[hook("hang-page"), junk()], today());
    assert_eq!(could_not(&out[0]), Some(CouldNotRead::TimedOut));
    assert_eq!(out[1], UNSUPPORTED);
}

#[test]
fn the_memory_watchdog_stops_a_worker_over_its_limit() {
    let s = Scratch::new("docread-memory");
    let mut c = config(&s);
    c.limits.worker_memory = 256 * 1024 * 1024;
    // Keep the hard cap out of the way: this is the watchdog's test.
    c.limits.hard_cap_percent = 400;
    let mut host = DocreadHost::new(c);
    // Windows counts commit (memory reserved and promised, even untouched:
    // resident memory stays flat, review M2); elsewhere resident memory.
    let grow = if cfg!(windows) {
        "commit:384"
    } else {
        "rss:384"
    };
    let out = host.read(&[hook(grow), junk()], today());
    assert_eq!(could_not(&out[0]), Some(CouldNotRead::TooMuchMemory));
    assert_eq!(out[1], UNSUPPORTED);
}

/// The hard cap (Q-A03s): on Windows the job refuses commit past
/// 1.25 × the watchdog, so a 1.5 GiB allocation fails at once (the worker
/// aborts) instead of waiting for the next poll; elsewhere the worker's
/// own data limit or the watchdog ends it. Either way the job could not be
/// read and the next one is.
#[test]
fn a_huge_allocation_is_stopped_by_the_hard_cap() {
    let s = Scratch::new("docread-hard-cap");
    let mut c = config(&s);
    // A slow watchdog: the cap must act on its own.
    c.limits.poll = Duration::from_secs(5);
    let mut host = DocreadHost::new(c);
    let grow = if cfg!(windows) {
        "commit:1536"
    } else {
        "rss:1536"
    };
    let started = Instant::now();
    let out = host.read(&[hook(grow), junk()], today());
    let why = could_not(&out[0]);
    if cfg!(windows) {
        assert_eq!(why, Some(CouldNotRead::Crashed));
        // Two attempts, each ended by the cap, not by a 5 s poll.
        assert!(
            started.elapsed() < Duration::from_secs(9),
            "{:?}",
            started.elapsed()
        );
    } else {
        assert!(
            matches!(
                why,
                Some(CouldNotRead::Crashed | CouldNotRead::TooMuchMemory)
            ),
            "{why:?}"
        );
    }
    assert_eq!(out[1], UNSUPPORTED);
}

#[test]
fn oversized_and_garbled_frames_kill_the_worker() {
    let s = Scratch::new("docread-frames");
    let mut host = DocreadHost::new(config(&s));
    let out = host.read(&[hook("big-frame"), hook("garble"), junk()], today());
    assert_eq!(could_not(&out[0]), Some(CouldNotRead::BadReply));
    assert_eq!(could_not(&out[1]), Some(CouldNotRead::BadReply));
    assert_eq!(out[2], UNSUPPORTED);
    assert_eq!(host.stats().kills, 4);
}

#[test]
fn a_worker_that_never_gets_ready_is_given_up_on() {
    for how in ["hang", "exit"] {
        let s = Scratch::new(&format!("docread-ready-{how}"));
        let mut c = config(&s);
        c.limits.ready = Duration::from_millis(800);
        c.test_env = vec![("CL_DOCREAD_TEST_READY".into(), how.into())];
        let mut host = DocreadHost::new(c);
        let out = host.read(&[junk(), junk()], today());
        assert!(
            out.iter()
                .all(|o| could_not(o) == Some(CouldNotRead::Unavailable)),
            "{how}: {out:?}"
        );
        assert_eq!(host.stats().workers_started, 2, "{how}");
    }
    // Only CL_DOCREAD_TEST_ names reach the worker.
    let s = Scratch::new("docread-ready-other");
    let mut c = config(&s);
    c.test_env = vec![("OTHER_READY".into(), "hang".into())];
    let mut host = DocreadHost::new(c);
    assert_eq!(host.read(&[junk()], today()), [UNSUPPORTED]);
}

/// Security review M3: on Windows the job allows one process, so a worker
/// can't start a second stage. Elsewhere this is not enforced yet (P37):
/// the hook's start succeeds, which keeps the test honest about that.
#[test]
fn a_worker_can_start_nothing() {
    let s = Scratch::new("docread-spawn");
    let mut host = DocreadHost::new(config(&s));
    let out = host.read(&[hook("spawn"), junk()], today());
    let expected = if cfg!(windows) {
        JobFailure::Unreadable
    } else {
        JobFailure::Empty
    };
    assert_eq!(out[0], JobOutcome::Failed(expected));
    assert_eq!(out[1], UNSUPPORTED);
}

/// Security review M1, M2 on Linux: a live worker has no core file, a CPU
/// limit and is not dumpable (read from `/proc`, as the kernel applies
/// them).
#[cfg(target_os = "linux")]
#[test]
fn a_worker_runs_with_its_unix_limits() {
    let s = Scratch::new("docread-unix-limits");
    let mut c = config(&s);
    c.limits.first_page = Duration::from_secs(20);
    c.limits.kills_per_job = 1;
    let mut host = DocreadHost::new(c);
    let cancel = host.cancel_handle();
    let reader = std::thread::spawn(move || host.read(&[hook("hang-load")], today()));
    let me = std::process::id().to_string();
    let worker = (0..100).find_map(|_| {
        std::thread::sleep(Duration::from_millis(100));
        std::fs::read_dir("/proc").ok()?.flatten().find_map(|e| {
            let stat = std::fs::read_to_string(e.path().join("stat")).ok()?;
            // `pid (comm) state ppid ...`
            let after = stat.rsplit_once(") ")?.1;
            let ppid = after.split_whitespace().nth(1)?;
            (stat.contains("(clientledger-do") && ppid == me).then(|| e.path())
        })
    });
    let found = worker.map(|p| std::fs::read_to_string(p.join("limits")).unwrap_or_default());
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = reader.join();
    let limits = found.expect("the worker was running");
    let row = |name: &str| {
        limits
            .lines()
            .find(|l| l.starts_with(name))
            .map(|l| l.split_whitespace().collect::<Vec<_>>())
            .unwrap_or_default()
    };
    // "Max core file size  0  0  bytes"
    assert!(
        row("Max core file size").ends_with(&["0", "0", "bytes"]),
        "{limits}"
    );
    // "Max cpu time  <soft>  <hard>  seconds", both finite.
    let cpu = row("Max cpu time");
    assert!(
        cpu.len() == 6 && cpu[3].parse::<u64>().is_ok() && cpu[4].parse::<u64>().is_ok(),
        "{limits}"
    );
}

#[test]
fn a_missing_worker_program_is_unavailable() {
    let s = Scratch::new("docread-missing");
    let mut c = config(&s);
    c.worker_exe = s.file("no-such-worker.exe");
    let mut host = DocreadHost::new(c);
    let out = host.read(&[junk()], today());
    assert_eq!(could_not(&out[0]), Some(CouldNotRead::Unavailable));
}

#[test]
fn workers_are_recycled_by_job_count_and_by_memory() {
    let s = Scratch::new("docread-recycle");
    let mut host = DocreadHost::new(config(&s));
    let jobs: Vec<DocJob> = (0..45).map(|_| junk()).collect();
    let out = host.read(&jobs, today());
    assert!(out.iter().all(|o| *o == UNSUPPORTED));
    let st = host.stats();
    assert_eq!((st.workers_started, st.recycles, st.kills), (3, 2, 0));

    // A zero setting still lets each worker read one job.
    let s = Scratch::new("docread-recycle-zero");
    let mut c = config(&s);
    c.limits.recycle_jobs = 0;
    let mut host = DocreadHost::new(c);
    assert_eq!(
        host.read(&[junk(), junk()], today()),
        [UNSUPPORTED, UNSUPPORTED]
    );
    assert_eq!(host.stats().workers_started, 2);

    let s = Scratch::new("docread-recycle-mem");
    let mut c = config(&s);
    c.limits.recycle_memory = 64 * 1024 * 1024;
    let mut host = DocreadHost::new(c);
    let out = host.read(&[hook("hold:128"), junk(), junk()], today());
    assert_eq!(out[0], JobOutcome::Failed(JobFailure::Unreadable));
    assert_eq!(&out[1..], [UNSUPPORTED, UNSUPPORTED]);
    let st = host.stats();
    assert_eq!((st.workers_started, st.recycles, st.kills), (2, 1, 0));
}

#[test]
fn a_page_image_that_is_not_a_pgm_never_reaches_tesseract() {
    let s = Scratch::new("docread-pgm");
    let mut c = config(&s);
    // A tesseract that does not exist: the header check comes first.
    c.ocr = Some(OcrTools {
        exe: s.file("no-tesseract.exe"),
        tessdata: s.file("tessdata"),
    });
    let mut host = DocreadHost::new(c);
    let out = host.read(&[hook("bad-pgm"), junk()], today());
    assert_eq!(out[0], JobOutcome::Failed(JobFailure::Unreadable));
    assert_eq!(out[1], UNSUPPORTED);
    let st = host.stats();
    assert_eq!((st.bad_images, st.ocr_pages), (1, 0));
}

#[test]
fn cancel_stops_the_worker_and_the_rest_of_the_jobs() {
    let s = Scratch::new("docread-cancel");
    let mut host = DocreadHost::new(config(&s));
    let cancel = host.cancel_handle();
    let setter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    let started = Instant::now();
    let out = host.read(&[hook("hang-load"), junk()], today());
    setter.join().unwrap();
    assert!(
        out.iter()
            .all(|o| could_not(o) == Some(CouldNotRead::Cancelled)),
        "{out:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(20));
}

/// Tesseract when it is here; `CL_OCR_REQUIRE=1` makes its absence a
/// failure.
fn ocr_tools() -> Option<OcrTools> {
    let t = OcrTools::find();
    assert!(
        t.is_some() || std::env::var("CL_OCR_REQUIRE").as_deref() != Ok("1"),
        "tesseract is required (CL_OCR_REQUIRE=1) but was not found (cargo xtask ocr-build, ocr-models)"
    );
    t
}

#[test]
fn a_scanned_page_is_read_by_tesseract_with_a_scrubbed_environment() {
    let (Some(tools), true) = (ocr_tools(), pdfium_present()) else {
        eprintln!("tesseract or PDFium missing: skipped");
        return;
    };
    let s = Scratch::new("docread-ocr");
    let mut c = config(&s);
    c.ocr = Some(tools);
    let mut host = DocreadHost::new(c);
    // Every page to OCR, text layer or not: a real tesseract run on a
    // rendered page (on Windows Leptonica's temp file from stdin lands in
    // the per-page folder).
    let page = pdf::text(72, 600, 24, "INVOICE") + &pdf::text(300, 500, 24, "Total 544.82");
    let mut bytes = b"%CL-TEST-HOOK:force-ocr\n".to_vec();
    bytes.extend_from_slice(&pdf::build(&[page]));
    let out = host.read(&[doc(bytes)], today());
    let JobOutcome::Read(r) = &out[0] else {
        panic!("{:?}", out[0])
    };
    assert_eq!(r.reader, Reader::Ocr);
    let words = &r.words[0].words;
    // Synthetic text only: safe to show on failure.
    let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
    let total = words
        .iter()
        .find(|w| w.text == "544.82")
        .unwrap_or_else(|| panic!("tesseract read {texts:?}"));
    // "Total " is 60 pt wide from x = 300 pt: the amount starts near 360 pt
    // of 612; the 24 pt capitals sit above the baseline at 500 pt of 792.
    let b = total.bbox;
    assert!((5_750..=6_050).contains(&b.left()), "{b:?}");
    assert!((3_300..=3_650).contains(&b.top()), "{b:?}");
    let label = words.iter().find(|w| w.text == "Total").unwrap().bbox;
    assert!((4_800..=5_050).contains(&label.left()), "{label:?}");
    assert!(words.iter().any(|w| w.text == "INVOICE"));
    assert_eq!(host.stats().ocr_pages, 1);
    assert_eq!(host.stats().ocr_failures, 0);
    let left = leftovers(&s);
    assert_eq!(left, 0, "per-page and worker folders are removed");
}

/// Text turned a quarter turn anticlockwise (reads bottom to top).
fn turned_text(x: i32, y: i32, size: i32, s: &str) -> String {
    format!("BT /F1 {size} Tf 0 1 -1 0 {x} {y} Tm ({s}) Tj ET\n")
}

#[test]
fn a_page_lying_on_its_side_is_turned_and_read() {
    let (Some(tools), true) = (ocr_tools(), pdfium_present()) else {
        eprintln!("tesseract or PDFium missing: skipped");
        return;
    };
    let s = Scratch::new("docread-osd");
    let mut c = config(&s);
    c.ocr = Some(tools);
    let mut host = DocreadHost::new(c);
    let mut page = turned_text(80, 72, 22, "INVOICE Riverbend Electric Co.");
    for i in 0..14 {
        page += &turned_text(
            130 + i * 30,
            72,
            16,
            &format!("Install receptacles kitchen {i} hours 72.17 288.68"),
        );
    }
    page += &turned_text(570, 300, 18, "Total 544.82");
    let mut bytes = b"%CL-TEST-HOOK:force-ocr\n".to_vec();
    bytes.extend_from_slice(&pdf::build(&[page]));
    let out = host.read(&[doc(bytes)], today());
    let JobOutcome::Read(r) = &out[0] else {
        panic!("{:?}", out[0])
    };
    let words = &r.words[0].words;
    let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
    assert!(
        texts.contains(&"544.82") && texts.contains(&"Riverbend"),
        "tesseract read {texts:?}"
    );
    // Words map back onto the page as it is shown: the turned total sits
    // in a narrow, tall box.
    let b = words.iter().find(|w| w.text == "544.82").unwrap().bbox;
    assert!(b.height() > b.width(), "{b:?}");
    // Read once sideways, once turned.
    assert_eq!(host.stats().ocr_pages, 2);
}
