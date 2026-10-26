//! One job: sniff the file, read each of its first pages (PDF text layer,
//! else OCR through the host), run the rules, answer `JobDone` or
//! `JobFailed` (plan D2, §5 "Missing pieces", "Empty", "Large").
//!
//! The pipe is behind [`Io`] so the job logic is tested without a process.

use crate::decode::{self, ImageError, Kind, Tiff};
use crate::pdfium::{self, Opened};
use cl_docread::model::{MAX_PAGES, PageWords, Word, WordSource};
use cl_docread::ocr_geom;
use cl_docread::pdf_text::MIN_TEXT_QUALITY_BP;
use cl_docread::preprocess;
use cl_docread::protocol::{
    BatchOptions, FromWorker, Job, JobFailure, JobId, MAX_BATCH_DOCUMENT_BYTES, MAX_DOCUMENT_BYTES,
    MAX_JOBS_PER_BATCH, MAX_TEMPLATES, OcrFailure, Tsv,
};
use cl_docread::tsv;
use cl_docread::{ReadInput, read};
use image::GrayImage;
use pdfium_render::prelude::Pdfium;

/// A pipe failure or a protocol breach by the host: the worker exits.
#[derive(Debug)]
pub enum Fatal {
    /// The pipe broke or closed.
    Pipe,
    /// The host sent something out of order or over a limit.
    Protocol,
}

/// The host's answer to `NeedOcr`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrAnswer {
    Tsv(Tsv),
    Failed(OcrFailure),
}

/// The worker's side of the pipe.
pub trait Io {
    /// Send a frame.
    ///
    /// # Errors
    /// The pipe broke.
    fn send(&mut self, msg: &FromWorker) -> Result<(), Fatal>;
    /// Ask the host to OCR `pgm` and wait for its answer.
    ///
    /// # Errors
    /// The pipe broke or the host answered out of order.
    fn ocr(&mut self, job: JobId, page: u16, pgm: Vec<u8>) -> Result<OcrAnswer, Fatal>;
}

/// What a job needs besides its own bytes.
pub struct Ctx<'a> {
    pub pdfium: Option<&'a Pdfium>,
    pub options: BatchOptions,
    /// Every page goes to OCR, text layer or not (a test hook).
    pub force_ocr: bool,
}

/// Whether a batch keeps within the protocol's caps (the host splits
/// larger sets, so a breach here is the host's fault).
#[must_use]
pub fn batch_within_caps(jobs: &[Job]) -> bool {
    let total: usize = jobs.iter().map(|j| j.bytes.0.len()).sum();
    jobs.len() <= MAX_JOBS_PER_BATCH
        && total <= MAX_BATCH_DOCUMENT_BYTES
        && jobs.iter().all(|j| j.templates.len() <= MAX_TEMPLATES)
}

fn failed(job: JobId, reason: JobFailure) -> FromWorker {
    FromWorker::JobFailed { job, reason }
}

/// Read one job and return its final frame (`JobDone` or `JobFailed`).
///
/// # Errors
/// The pipe broke or the host breached the protocol.
pub fn run_job(ctx: &Ctx<'_>, io: &mut dyn Io, job: &Job) -> Result<FromWorker, Fatal> {
    run_bytes(ctx, io, job, &job.bytes.0)
}

/// [`run_job`] on `bytes` (a hook may hand on what follows its line).
///
/// # Errors
/// See [`run_job`].
pub fn run_bytes(
    ctx: &Ctx<'_>,
    io: &mut dyn Io,
    job: &Job,
    bytes: &[u8],
) -> Result<FromWorker, Fatal> {
    if bytes.is_empty() {
        return Ok(failed(job.job, JobFailure::Empty));
    }
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Ok(failed(job.job, JobFailure::TooLarge));
    }
    #[cfg(feature = "test-support")]
    if let Some((hook, rest)) = crate::hooks::parse(bytes) {
        return crate::hooks::run(hook, rest, ctx, io, job);
    }
    match decode::sniff(bytes) {
        Kind::Pdf => read_pdf(ctx, io, job, bytes),
        Kind::Png | Kind::Jpeg => {
            // Decoding counts against the host's dispatch-to-first-page
            // deadline.
            match decode::decode_image(bytes) {
                Ok(img) => read_images(ctx, io, job, 1, |_| Ok(img.clone())),
                Err(e) => Ok(failed(job.job, image_failure(e))),
            }
        }
        Kind::Tiff => match Tiff::open(bytes.to_vec()) {
            Ok(mut t) => {
                let count = t.pages();
                read_images(ctx, io, job, count, |p| t.page(p))
            }
            Err(e) => Ok(failed(job.job, image_failure(e))),
        },
        Kind::Unknown => Ok(failed(job.job, JobFailure::UnsupportedType)),
    }
}

fn image_failure(e: ImageError) -> JobFailure {
    match e {
        ImageError::TooLarge => JobFailure::TooLarge,
        ImageError::Unreadable => JobFailure::Unreadable,
    }
}

/// The pages read so far.
#[derive(Default)]
struct Pages {
    read: Vec<PageWords>,
    unread: Vec<u16>,
    /// A page had nothing to read without OCR, and OCR is unavailable.
    needed_ocr: bool,
}

fn read_pdf(ctx: &Ctx<'_>, io: &mut dyn Io, job: &Job, bytes: &[u8]) -> Result<FromWorker, Fatal> {
    let Some(pdfium) = ctx.pdfium else {
        eprintln!("docread: outcome: pdf reader unavailable");
        return Ok(failed(job.job, JobFailure::Unreadable));
    };
    let doc = match pdfium::open(pdfium, bytes) {
        Opened::Doc(d) => d,
        Opened::Protected => return Ok(failed(job.job, JobFailure::Protected)),
        Opened::Unreadable => return Ok(failed(job.job, JobFailure::Unreadable)),
    };
    let count = pdfium::page_count(&doc);
    if count == 0 {
        return Ok(failed(job.job, JobFailure::Unreadable));
    }
    let mut pages = Pages::default();
    for i in 0..count.min(MAX_PAGES) {
        io.send(&FromWorker::PageStarted {
            job: job.job,
            page: i,
        })?;
        let Ok(page) = doc.pages().get(i32::from(i)) else {
            pages.unread.push(i);
            continue;
        };
        let aspect = pdfium::aspect(&page);
        let (words, quality) = if ctx.force_ocr {
            (Vec::new(), 0)
        } else {
            pdfium::text_layer(&page)
        };
        if quality >= MIN_TEXT_QUALITY_BP {
            pages
                .read
                .push(page_words(i, aspect, WordSource::TextLayer, words));
            continue;
        }
        let ocr_words = if ctx.options.ocr {
            match pdfium::render_grey(&page) {
                Some(grey) => ocr_page(io, job.job, i, &grey)?,
                None => None,
            }
        } else {
            if words.is_empty() {
                pages.needed_ocr = true;
            }
            None
        };
        match ocr_words {
            Some(w) => pages.read.push(page_words(i, aspect, WordSource::Ocr, w)),
            // A weak text layer beats nothing.
            None if !words.is_empty() => {
                pages
                    .read
                    .push(page_words(i, aspect, WordSource::TextLayer, words));
            }
            None => pages.unread.push(i),
        }
    }
    Ok(finish(ctx, job, count, &pages))
}

/// Pictures (one PNG/JPEG, or TIFF pages): OCR only.
fn read_images(
    ctx: &Ctx<'_>,
    io: &mut dyn Io,
    job: &Job,
    count: u16,
    mut page: impl FnMut(u16) -> Result<GrayImage, ImageError>,
) -> Result<FromWorker, Fatal> {
    if !ctx.options.ocr {
        return Ok(failed(job.job, JobFailure::NeedsOcr));
    }
    let mut pages = Pages::default();
    let mut too_large = false;
    for i in 0..count.min(MAX_PAGES) {
        io.send(&FromWorker::PageStarted {
            job: job.job,
            page: i,
        })?;
        let grey = match page(i) {
            Ok(g) => g,
            Err(e) => {
                too_large |= e == ImageError::TooLarge;
                pages.unread.push(i);
                continue;
            }
        };
        let aspect = aspect_px(grey.width(), grey.height());
        match ocr_page(io, job.job, i, &grey)? {
            Some(w) => pages.read.push(page_words(i, aspect, WordSource::Ocr, w)),
            None => pages.unread.push(i),
        }
    }
    if too_large && pages.read.is_empty() {
        return Ok(failed(job.job, JobFailure::TooLarge));
    }
    Ok(finish(ctx, job, count, &pages))
}

fn aspect_px(w: u32, h: u32) -> u32 {
    if w == 0 {
        return 0;
    }
    let a = (u64::from(h) * 10_000 + u64::from(w) / 2) / u64::from(w);
    u32::try_from(a).unwrap_or(u32::MAX)
}

fn page_words(page: u16, aspect: u32, source: WordSource, words: Vec<Word>) -> PageWords {
    PageWords {
        page,
        aspect,
        source,
        words,
    }
}

/// Clean the page, ask the host for OCR, map the words back to the page.
/// `None` when the page could not be read.
fn ocr_page(
    io: &mut dyn Io,
    job: JobId,
    page: u16,
    grey: &GrayImage,
) -> Result<Option<Vec<Word>>, Fatal> {
    let cleaned = preprocess::clean(grey);
    let Ok(pgm) = preprocess::to_pgm(&cleaned.image) else {
        return Ok(None);
    };
    let tsv = match io.ocr(job, page, pgm)? {
        OcrAnswer::Tsv(t) => t,
        OcrAnswer::Failed(reason) => {
            eprintln!("docread: outcome: ocr failed ({})", ocr_reason(reason));
            return Ok(None);
        }
    };
    let (w, h) = cleaned.image.dimensions();
    let Ok(words) = tsv::parse(&tsv.0, w, h) else {
        return Ok(None);
    };
    Ok(Some(
        words
            .into_iter()
            .map(|o| Word {
                text: o.text,
                bbox: ocr_geom::to_page(o.px, &cleaned.transform),
                confidence: o.confidence,
            })
            .collect(),
    ))
}

fn ocr_reason(r: OcrFailure) -> &'static str {
    match r {
        OcrFailure::Unavailable => "unavailable",
        OcrFailure::TimedOut => "timed out",
        OcrFailure::Crashed => "crashed",
        OcrFailure::BadImage => "bad image",
        OcrFailure::TooMuchOutput => "too much output",
    }
}

/// The rules over the pages read, or the job's failure when nothing was.
fn finish(ctx: &Ctx<'_>, job: &Job, count: u16, pages: &Pages) -> FromWorker {
    if pages.read.iter().all(|p| p.words.is_empty()) {
        let reason = if pages.needed_ocr && !ctx.options.ocr {
            JobFailure::NeedsOcr
        } else {
            JobFailure::Unreadable
        };
        return failed(job.job, reason);
    }
    let reading = read(&ReadInput {
        pages: &pages.read,
        page_count: count,
        unread_pages: &pages.unread,
        templates: &job.templates,
        hint: job.hint,
        today: ctx.options.today,
    });
    if reading.validate(job.templates.len()).is_err() {
        eprintln!("docread: outcome: reading over a limit");
        return failed(job.job, JobFailure::Unreadable);
    }
    FromWorker::JobDone {
        job: job.job,
        reading: Box::new(reading),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::test_pdf::{Page, build, text};
    use cl_core::CalendarDate;
    use cl_docread::model::Reader;
    use cl_docread::protocol::{Bytes, JobHint, validate_pgm};

    /// A host stand-in: records frames, answers OCR with `answer`.
    pub(crate) struct FakeHost {
        pub sent: Vec<FromWorker>,
        pub ocr_calls: Vec<(u16, u32, u32)>,
        pub answer: Box<dyn FnMut(u32, u32) -> OcrAnswer>,
    }

    impl FakeHost {
        pub(crate) fn new(answer: impl FnMut(u32, u32) -> OcrAnswer + 'static) -> FakeHost {
            FakeHost {
                sent: Vec::new(),
                ocr_calls: Vec::new(),
                answer: Box::new(answer),
            }
        }

        fn started(&self) -> Vec<u16> {
            self.sent
                .iter()
                .filter_map(|m| match m {
                    FromWorker::PageStarted { page, .. } => Some(*page),
                    _ => None,
                })
                .collect()
        }
    }

    impl Io for FakeHost {
        fn send(&mut self, msg: &FromWorker) -> Result<(), Fatal> {
            self.sent.push(msg.clone());
            Ok(())
        }
        fn ocr(&mut self, _job: JobId, page: u16, pgm: Vec<u8>) -> Result<OcrAnswer, Fatal> {
            let (w, h) = validate_pgm(&pgm).map_err(|_| Fatal::Protocol)?;
            self.ocr_calls.push((page, w, h));
            Ok((self.answer)(w, h))
        }
    }

    /// TSV with one word box covering the middle tenth of the image.
    pub(crate) fn one_word(w: u32, h: u32) -> OcrAnswer {
        OcrAnswer::Tsv(Tsv(format!(
            "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
             5\t1\t1\t1\t1\t1\t{}\t{}\t{}\t{}\t91.5\tTotal\n",
            w * 45 / 100,
            h * 45 / 100,
            w / 10,
            h / 10
        )))
    }

    fn ctx(ocr: bool) -> Ctx<'static> {
        Ctx {
            pdfium: None,
            options: BatchOptions {
                ocr,
                today: CalendarDate::from_ymd(2026, 10, 26).unwrap(),
            },
            force_ocr: false,
        }
    }

    fn job(bytes: Vec<u8>) -> Job {
        Job {
            job: 7,
            bytes: Bytes(bytes),
            hint: JobHint::default(),
            templates: Vec::new(),
        }
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/docs")
                .join(name),
        )
        .unwrap()
    }

    fn reason(m: &FromWorker) -> Option<JobFailure> {
        match m {
            FromWorker::JobFailed { reason, .. } => Some(*reason),
            _ => None,
        }
    }

    #[test]
    fn empty_unknown_and_oversized_files_fail_plainly() {
        let mut host = FakeHost::new(one_word);
        let c = ctx(true);
        for (bytes, want) in [
            (Vec::new(), JobFailure::Empty),
            (
                b"MZ\x90\0 not a document".to_vec(),
                JobFailure::UnsupportedType,
            ),
            (vec![b'%'; MAX_DOCUMENT_BYTES + 1], JobFailure::TooLarge),
            (fixture("bomb_30000.png"), JobFailure::TooLarge),
            (fixture("bomb_tiff.tif"), JobFailure::TooLarge),
        ] {
            let out = run_job(&c, &mut host, &job(bytes)).unwrap();
            assert_eq!(reason(&out), Some(want));
        }
        assert_eq!(host.ocr_calls, []);
    }

    #[test]
    fn a_picture_is_read_with_ocr_and_its_words_land_on_the_page() {
        let mut host = FakeHost::new(one_word);
        let out = run_job(&ctx(true), &mut host, &job(fixture("receipt.png"))).unwrap();
        let FromWorker::JobDone { reading, .. } = out else {
            panic!("{out:?}")
        };
        assert_eq!(reading.reader, Reader::Ocr);
        assert_eq!(host.started(), [0]);
        assert_eq!(host.ocr_calls.len(), 1);
        let w = &reading.words[0].words[0];
        // The middle tenth of the cleaned image is the middle tenth of the page.
        assert!(w.bbox.left().abs_diff(4_500) <= 30, "{:?}", w.bbox);
        assert!(w.bbox.right().abs_diff(5_500) <= 30, "{:?}", w.bbox);
        assert_eq!(w.confidence.bp(), 9_150);
        // The receipt is narrow: enlarged for OCR, but the box maps back.
        let (_, ow, _) = host.ocr_calls[0];
        let png = decode::decode_image(&fixture("receipt.png")).unwrap();
        assert!(ow >= png.width());
    }

    #[test]
    fn without_ocr_a_picture_needs_it() {
        let mut host = FakeHost::new(one_word);
        let out = run_job(&ctx(false), &mut host, &job(fixture("receipt.jpg"))).unwrap();
        assert_eq!(reason(&out), Some(JobFailure::NeedsOcr));
    }

    #[test]
    fn tiff_pages_each_go_to_ocr_and_failures_are_unread_pages() {
        let mut n = 0;
        let mut host = FakeHost::new(move |w, h| {
            n += 1;
            if n == 2 {
                OcrAnswer::Failed(OcrFailure::TimedOut)
            } else {
                one_word(w, h)
            }
        });
        let out = run_job(&ctx(true), &mut host, &job(fixture("three_pages.tif"))).unwrap();
        let FromWorker::JobDone { reading, .. } = out else {
            panic!("{out:?}")
        };
        assert_eq!(host.started(), [0, 1, 2]);
        assert_eq!(host.ocr_calls.len(), 3);
        assert_eq!(reading.page_count, 3);
        assert_eq!(reading.words.len(), 2);
        assert_eq!(
            reading.pages.iter().filter(|p| p.source.is_none()).count(),
            1
        );

        let mut host = FakeHost::new(|_, _| OcrAnswer::Failed(OcrFailure::Crashed));
        let out = run_job(&ctx(true), &mut host, &job(fixture("three_pages.tif"))).unwrap();
        assert_eq!(reason(&out), Some(JobFailure::Unreadable));
        let mut host = FakeHost::new(|_, _| OcrAnswer::Tsv(Tsv("garbage".into())));
        let out = run_job(&ctx(true), &mut host, &job(fixture("g4_fax.tif"))).unwrap();
        assert_eq!(reason(&out), Some(JobFailure::Unreadable));
    }

    #[test]
    fn caps_on_a_batch() {
        let small = |n: usize| (0..n).map(|_| job(vec![1])).collect::<Vec<_>>();
        assert!(batch_within_caps(&small(MAX_JOBS_PER_BATCH)));
        assert!(!batch_within_caps(&small(MAX_JOBS_PER_BATCH + 1)));
        let big: Vec<Job> = (0..5).map(|_| job(vec![0; MAX_DOCUMENT_BYTES])).collect();
        assert!(!batch_within_caps(&big));
    }

    fn with_pdfium(f: impl FnOnce(Ctx<'_>)) {
        let Some(p) = crate::pdfium::tests::pdfium() else {
            return;
        };
        let mut c = ctx(true);
        c.pdfium = Some(&p);
        f(c);
    }

    #[test]
    fn a_text_pdf_reads_without_ocr() {
        with_pdfium(|c| {
            let body: String = (0..30)
                .map(|i| text(72, 760 - i * 20, 10, &format!("Widget {i} 2 x 3.00 6.00")))
                .collect();
            let mut host = FakeHost::new(one_word);
            let out = run_job(&c, &mut host, &job(build(&[Page::letter(body)]))).unwrap();
            let FromWorker::JobDone { reading, .. } = out else {
                panic!("{out:?}")
            };
            assert_eq!(reading.reader, Reader::TextPdf);
            assert_eq!(host.ocr_calls, []);
            assert_eq!(reading.words[0].source, WordSource::TextLayer);
            assert_eq!(reading.words[0].aspect, 12_941);
        });
    }

    #[test]
    fn a_pdf_without_text_goes_to_ocr_and_only_ten_pages_are_read() {
        with_pdfium(|c| {
            let pages: Vec<Page> = (0..12)
                .map(|_| Page::letter("0 0 0 rg 72 600 200 40 re f\n".into()))
                .collect();
            let pdf = build(&pages);
            let mut host = FakeHost::new(one_word);
            let out = run_job(&c, &mut host, &job(pdf.clone())).unwrap();
            let FromWorker::JobDone { reading, .. } = out else {
                panic!("{out:?}")
            };
            assert_eq!(host.started(), (0..10).collect::<Vec<u16>>());
            assert_eq!(host.ocr_calls.len(), 10);
            assert!(
                host.ocr_calls
                    .iter()
                    .all(|&(_, w, h)| (w, h) == (2550, 3300))
            );
            assert_eq!(reading.page_count, 12);
            assert_eq!(reading.reader, Reader::Ocr);

            // Without OCR the same file needs it.
            let mut no_ocr = ctx(false);
            no_ocr.pdfium = c.pdfium;
            let out = run_job(&no_ocr, &mut FakeHost::new(one_word), &job(pdf)).unwrap();
            assert_eq!(reason(&out), Some(JobFailure::NeedsOcr));

            let enc = fixture("encrypted.pdf");
            let out = run_job(&c, &mut FakeHost::new(one_word), &job(enc)).unwrap();
            assert_eq!(reason(&out), Some(JobFailure::Protected));
            let broken = b"%PDF-1.7\n1 0 obj << broken".to_vec();
            let out = run_job(&c, &mut FakeHost::new(one_word), &job(broken)).unwrap();
            assert_eq!(reason(&out), Some(JobFailure::Unreadable));
        });
    }

    #[test]
    fn without_pdfium_a_pdf_is_unreadable() {
        let out = run_job(
            &ctx(true),
            &mut FakeHost::new(one_word),
            &job(fixture("two_pages.pdf")),
        )
        .unwrap();
        assert_eq!(reason(&out), Some(JobFailure::Unreadable));
    }
}
