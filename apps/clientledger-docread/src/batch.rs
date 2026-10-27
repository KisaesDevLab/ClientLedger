//! One job: sniff the file, read each of its first pages (PDF text layer,
//! else OCR through the host), run the rules, answer `JobDone` or
//! `JobFailed` (plan D2, §5 "Missing pieces", "Empty", "Large").
//!
//! The pipe is behind [`Io`] so the job logic is tested without a process.

use crate::decode::{self, ImageError, Kind, Tiff};
use crate::pdfium::{self, Opened};
use crate::region;
use crate::second_read;
use cl_core::CalendarDate;
use cl_docread::dates;
use cl_docread::model::{Bbox, Confidence, PageWords, Spot, Word, WordSource};
use cl_docread::ocr_geom;
use cl_docread::pdf_text::MIN_TEXT_QUALITY_BP;
use cl_docread::pipeline::region_reading;
use cl_docread::preprocess;
use cl_docread::protocol::{
    BatchOptions, FromWorker, Job, JobFailure, JobId, MAX_BATCH_DOCUMENT_BYTES, MAX_DOCUMENT_BYTES,
    MAX_JOBS_PER_BATCH, MAX_TEMPLATES, OcrFailure, OcrMode, Tsv, region_ok, whole_page,
};
use cl_docread::tsv::{self, OcrWord, PixelBox};
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
    fn ocr(
        &mut self,
        job: JobId,
        page: u16,
        pgm: Vec<u8>,
        mode: OcrMode,
    ) -> Result<OcrAnswer, Fatal>;
}

/// What a job needs besides its own bytes.
pub struct Ctx<'a> {
    pub pdfium: Option<&'a Pdfium>,
    pub options: BatchOptions,
    /// Every page goes to OCR, text layer or not (a test hook).
    pub force_ocr: bool,
}

/// Whether a batch keeps within the protocol's caps (the host splits
/// larger sets, so a breach here is the host's fault). A region job's box
/// must keep the region rules (`protocol::region_ok`) and it carries no
/// templates (version 4).
#[must_use]
pub fn batch_within_caps(jobs: &[Job]) -> bool {
    let total: usize = jobs.iter().map(|j| j.bytes.0.len()).sum();
    jobs.len() <= MAX_JOBS_PER_BATCH
        && total <= MAX_BATCH_DOCUMENT_BYTES
        && jobs.iter().all(|j| j.templates.len() <= MAX_TEMPLATES)
        && jobs.iter().all(|j| {
            j.region
                .is_none_or(|r| region_ok(r) && j.templates.is_empty())
        })
}

/// The pages a job reads: its region's page, or the first ten (twenty
/// for a statement). `None`
/// when the region's page is past the document's end.
fn pages_to_read(job: &Job, count: u16) -> Option<std::ops::Range<u16>> {
    match job.region {
        None => Some(0..count.min(job.hint.max_pages())),
        Some(r) if r.page < count => Some(r.page..r.page + 1),
        Some(_) => None,
    }
}

/// OCR one page: the region's box only, unless the job reads whole
/// pages or the box nearly covers the page (`protocol::whole_page`).
fn ocr_for_job(
    io: &mut dyn Io,
    (job, page): (&Job, u16),
    grey: &GrayImage,
    today: CalendarDate,
    disagree: &mut Vec<(u16, second_read::Disagreement<Bbox>)>,
) -> Result<Option<Vec<Word>>, Fatal> {
    match job.region {
        Some(r) if !whole_page(r.bbox) => region::ocr_region(io, (job.job, page), grey, r.bbox),
        _ => ocr_page(io, (job.job, page), grey, today, disagree),
    }
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
    /// Amounts the best model's second read gave differently (page, spot).
    disagree: Vec<(u16, second_read::Disagreement<Bbox>)>,
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
    let Some(range) = pages_to_read(job, count) else {
        return Ok(failed(job.job, JobFailure::Unreadable));
    };
    let mut pages = Pages::default();
    for i in range {
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
                Some(grey) => {
                    ocr_for_job(io, (job, i), &grey, ctx.options.today, &mut pages.disagree)?
                }
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
    let Some(range) = pages_to_read(job, count) else {
        return Ok(failed(job.job, JobFailure::Unreadable));
    };
    let mut pages = Pages::default();
    let mut too_large = false;
    for i in range {
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
        match ocr_for_job(io, (job, i), &grey, ctx.options.today, &mut pages.disagree)? {
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
///
/// A poor first read ([`good_read`]: few confident words, or few for all
/// it read) may be a page lying on its
/// side or upside down: the host then reports tesseract's orientation,
/// and a quarter-turned page is cleaned and read again; the better read
/// is kept (plan R2).
///
/// A page whose numbers read with low confidence is read again with the
/// more accurate model as well (`OcrMode::ReadBest`, Q-A03b), and a date
/// that still does not read is read again on its own (`OcrMode::Word`).
/// Each only replaces words read from the same pixels; nothing is guessed.
/// The second read keeps the first read's amounts ([`second_read`]); the
/// amounts it read differently go to `disagree`.
fn ocr_page(
    io: &mut dyn Io,
    (job, page): (JobId, u16),
    grey: &GrayImage,
    today: CalendarDate,
    disagree: &mut Vec<(u16, second_read::Disagreement<Bbox>)>,
) -> Result<Option<Vec<Word>>, Fatal> {
    let cleaned = preprocess::clean(grey);
    let first = ocr_raw(io, (job, page), &cleaned, OcrMode::Read)?;
    let first_score = first.as_deref().map_or(0, read_score);
    let (cleaned, words) = if first.as_deref().is_none_or(good_read) {
        (cleaned, first)
    } else if let Some(quarters) = orientation(io, job, page, &cleaned)? {
        let turned = preprocess::clean_turned(grey, quarters);
        match ocr_raw(io, (job, page), &turned, OcrMode::Read)? {
            Some(w) if read_score(&w) > first_score => (turned, Some(w)),
            _ => (cleaned, first),
        }
    } else {
        (cleaned, first)
    };
    let Some(mut words) = words else {
        return Ok(None);
    };
    if numbers_unsure(&words)
        && let Some(better) = ocr_raw(io, (job, page), &cleaned, OcrMode::ReadBest)?
    {
        let (kept, found) = second_read::merge(words, better);
        words = kept;
        disagree.extend(found.into_iter().map(|d| {
            (
                page,
                second_read::Disagreement {
                    at: ocr_geom::to_page(d.at, &cleaned.transform),
                    first: d.first,
                    other: d.other,
                },
            )
        }));
    }
    reread_dates(io, (job, page), &cleaned.image, &mut words, today)?;
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

/// Confident words (at least two letters or digits, confidence ≥ 60 %)
/// below which a read is second-guessed (see [`good_read`]).
const GOOD_READ: usize = 12;
/// The share of words (at least two letters or digits) that must read
/// confidently, in percent, for a read not to be second-guessed.
const GOOD_READ_SHARE_PCT: usize = 33;
/// Tesseract's orientation confidence needed to turn a page.
const MIN_ORIENT_CONFIDENCE_CENTI: u32 = 200;
/// Mean confidence of a page's words with digits below which it is read
/// again with the more accurate model (seen scans and photos: about a
/// third of scans and half of photos; nearly all of the best model's gain).
const UNSURE_NUMBERS_BP: u64 = 9_300;
/// Dates read again on their own per page, and the enlargements tried.
const MAX_REREADS: usize = 4;
const REREAD_SCALES: [u32; 3] = [1, 2, 3];

/// Confident words in a read.
pub(crate) fn read_score(words: &[OcrWord]) -> usize {
    words
        .iter()
        .filter(|w| {
            w.confidence.bp() >= 6_000
                && w.text.chars().filter(|c| c.is_alphanumeric()).count() >= 2
        })
        .count()
}

/// Whether a first read is good enough not to ask for the page's
/// orientation: at least [`GOOD_READ`] confident words, and confident words
/// at least [`GOOD_READ_SHARE_PCT`] of the words with two or more letters or
/// digits.
///
/// The share matters: a page lying on its side reads as hundreds of short
/// shapes ("Oo", "CO", "NNN"), and a dozen of them can come back confident.
/// Seen on the A03 sideways test page: 3 of 60 (Windows) and 11 or more of
/// 123 (Linux and macOS builds) against 94 of 94 once turned; the count
/// alone let Linux and macOS skip the orientation pass.
fn good_read(words: &[OcrWord]) -> bool {
    let candidates = words
        .iter()
        .filter(|w| w.text.chars().filter(|c| c.is_alphanumeric()).count() >= 2)
        .count();
    let confident = read_score(words);
    confident >= GOOD_READ && confident * 100 >= candidates * GOOD_READ_SHARE_PCT
}

/// Whether the words with digits (at least two letters or digits) read
/// with a mean confidence below [`UNSURE_NUMBERS_BP`].
pub(crate) fn numbers_unsure(words: &[OcrWord]) -> bool {
    let (sum, n) = words
        .iter()
        .filter(|w| {
            w.text.chars().filter(|c| c.is_alphanumeric()).count() >= 2
                && w.text.chars().any(|c| c.is_ascii_digit())
        })
        .fold((0u64, 0u64), |(s, n), w| {
            (s + u64::from(w.confidence.bp()), n + 1)
        });
    n > 0 && sum < UNSURE_NUMBERS_BP * n
}

/// OCR one cleaned page; `None` when it could not be read.
fn ocr_raw(
    io: &mut dyn Io,
    at: (JobId, u16),
    cleaned: &preprocess::Cleaned,
    mode: OcrMode,
) -> Result<Option<Vec<OcrWord>>, Fatal> {
    ocr_raw_image(io, at, &cleaned.image, mode)
}

/// OCR one image (a cleaned page or a cut-out of one); `None` when it
/// could not be read.
pub(crate) fn ocr_raw_image(
    io: &mut dyn Io,
    (job, page): (JobId, u16),
    image: &GrayImage,
    mode: OcrMode,
) -> Result<Option<Vec<OcrWord>>, Fatal> {
    let Ok(pgm) = preprocess::to_pgm(image) else {
        return Ok(None);
    };
    let tsv = match io.ocr(job, page, pgm, mode)? {
        OcrAnswer::Tsv(t) => t,
        OcrAnswer::Failed(reason) => {
            if mode == OcrMode::Read {
                eprintln!("docread: outcome: ocr failed ({})", ocr_reason(reason));
            }
            return Ok(None);
        }
    };
    let (w, h) = image.dimensions();
    Ok(tsv::parse(&tsv.0, w, h).ok())
}

/// Read again, on their own, the dates on a page that did not read
/// (`dates::unread_date_shape`: one numeric word, or a month word, day and
/// year on one line). A re-read replaces the words' text only when it
/// reads as a plausible date and has as many words; at most
/// [`MAX_REREADS`] per page.
fn reread_dates(
    io: &mut dyn Io,
    at: (JobId, u16),
    image: &GrayImage,
    words: &mut [OcrWord],
    today: CalendarDate,
) -> Result<(), Fatal> {
    let mut tries = 0;
    let mut i = 0;
    while i < words.len() && tries < MAX_REREADS {
        let one = [words[i].text.as_str()];
        let span = if dates::unread_date_shape(&one, today) {
            1
        } else if i + 3 <= words.len()
            && words[i..i + 3].iter().all(|w| same_line(w, &words[i]))
            && dates::unread_date_shape(
                &[
                    words[i].text.as_str(),
                    words[i + 1].text.as_str(),
                    words[i + 2].text.as_str(),
                ],
                today,
            )
        {
            3
        } else {
            i += 1;
            continue;
        };
        tries += 1;
        let target = &mut words[i..i + span];
        if let Some(read) = reread(io, at, image, union_px(target), span, today)? {
            for (w, (text, confidence)) in target.iter_mut().zip(read) {
                w.text = text;
                w.confidence = confidence;
            }
        }
        i += span;
    }
    Ok(())
}

fn same_line(a: &OcrWord, b: &OcrWord) -> bool {
    (a.block, a.paragraph, a.line) == (b.block, b.paragraph, b.line)
}

fn union_px(words: &[OcrWord]) -> PixelBox {
    let left = words.iter().map(|w| w.px.left).min().unwrap_or(0);
    let top = words.iter().map(|w| w.px.top).min().unwrap_or(0);
    let right = words
        .iter()
        .map(|w| w.px.left.saturating_add(w.px.width))
        .max()
        .unwrap_or(0);
    let bottom = words
        .iter()
        .map(|w| w.px.top.saturating_add(w.px.height))
        .max()
        .unwrap_or(0);
    PixelBox {
        left,
        top,
        width: right.saturating_sub(left),
        height: bottom.saturating_sub(top),
    }
}

/// One re-read of `px` per enlargement until one gives `n` words that
/// read as a date (a single word may come back split: it is joined).
/// `None` when none does or the host can't re-read.
fn reread(
    io: &mut dyn Io,
    (job, page): (JobId, u16),
    image: &GrayImage,
    px: PixelBox,
    n: usize,
    today: CalendarDate,
) -> Result<Option<Vec<(String, Confidence)>>, Fatal> {
    for scale in REREAD_SCALES {
        let Some(crop) = preprocess::reread_crop(image, px, scale) else {
            return Ok(None);
        };
        let Ok(pgm) = preprocess::to_pgm(&crop) else {
            return Ok(None);
        };
        let OcrAnswer::Tsv(tsv) = io.ocr(job, page, pgm, OcrMode::Word)? else {
            return Ok(None);
        };
        let (w, h) = crop.dimensions();
        let Ok(got) = tsv::parse(&tsv.0, w, h) else {
            continue;
        };
        let lowest = got.iter().map(|w| w.confidence).min();
        let read: Vec<(String, Confidence)> = if n == 1 {
            let text: String = got.iter().map(|w| w.text.as_str()).collect();
            lowest.map(|c| vec![(text, c)]).unwrap_or_default()
        } else {
            got.into_iter().map(|w| (w.text, w.confidence)).collect()
        };
        let joined = read
            .iter()
            .map(|(t, _)| t.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        if read.len() == n && dates::reads_as_date(&joined, today) {
            return Ok(Some(read));
        }
    }
    Ok(None)
}

/// The quarter turns (clockwise) tesseract's orientation pass says the
/// cleaned page needs, added to any turn already applied; `None` when it
/// is upright, unsure, or the pass failed.
pub(crate) fn orientation(
    io: &mut dyn Io,
    job: JobId,
    page: u16,
    cleaned: &preprocess::Cleaned,
) -> Result<Option<i32>, Fatal> {
    let Ok(pgm) = preprocess::to_pgm(&cleaned.image) else {
        return Ok(None);
    };
    let OcrAnswer::Tsv(report) = io.ocr(job, page, pgm, OcrMode::Orient)? else {
        return Ok(None);
    };
    let Some((rotate, confidence)) = tsv::parse_orientation(&report.0) else {
        return Ok(None);
    };
    if rotate == 0 || confidence < MIN_ORIENT_CONFIDENCE_CENTI {
        return Ok(None);
    }
    let already = (cleaned.transform.rotation_mdeg + 45_000).div_euclid(90_000);
    Ok(Some(already + i32::from(rotate / 90)))
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
/// A region job answers with its page's words in the box only.
fn finish(ctx: &Ctx<'_>, job: &Job, count: u16, pages: &Pages) -> FromWorker {
    if let Some(region) = job.region {
        return finish_region(ctx, job, count, pages, region);
    }
    if pages.read.iter().all(|p| p.words.is_empty()) {
        let reason = if pages.needed_ocr && !ctx.options.ocr {
            JobFailure::NeedsOcr
        } else {
            JobFailure::Unreadable
        };
        return failed(job.job, reason);
    }
    let mut reading = read(&ReadInput {
        pages: &pages.read,
        page_count: count,
        unread_pages: &pages.unread,
        templates: &job.templates,
        hint: job.hint,
        today: ctx.options.today,
    });
    second_read::add_checks(&mut reading, &pages.disagree);
    if reading
        .validate(job.templates.len(), job.hint.statement)
        .is_err()
    {
        eprintln!("docread: outcome: reading over a limit");
        return failed(job.job, JobFailure::Unreadable);
    }
    FromWorker::JobDone {
        job: job.job,
        reading: Box::new(reading),
    }
}

/// A region job's answer: the words in the box (possibly none), or the
/// failure when its page could not be read.
fn finish_region(ctx: &Ctx<'_>, job: &Job, count: u16, pages: &Pages, region: Spot) -> FromWorker {
    let Some(page) = pages.read.iter().find(|p| p.page == region.page) else {
        let reason = if pages.needed_ocr && !ctx.options.ocr {
            JobFailure::NeedsOcr
        } else {
            JobFailure::Unreadable
        };
        return failed(job.job, reason);
    };
    let reading = region_reading(page, count, region.bbox);
    if reading.validate(0, false).is_err() {
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
    use cl_docread::model::{MAX_PAGES, Reader};
    use cl_docread::protocol::{Bytes, JobHint, OcrMode, validate_pgm};

    /// A host stand-in: records frames, answers OCR with `answer`.
    pub(crate) struct FakeHost {
        pub sent: Vec<FromWorker>,
        /// Read calls (orientation calls are in `orient_calls`).
        pub ocr_calls: Vec<(u16, u32, u32)>,
        pub answer: Box<dyn FnMut(u32, u32) -> OcrAnswer>,
        pub orient_calls: Vec<(u32, u32)>,
        /// The orientation report (upright by default).
        pub orient: String,
        /// Second reads with the best model; unavailable by default.
        pub best_calls: Vec<(u32, u32)>,
        pub best: Box<dyn FnMut(u32, u32) -> OcrAnswer>,
        /// Single-value re-reads (crop sizes); failing by default.
        pub word_calls: Vec<(u32, u32)>,
        pub word: Box<dyn FnMut(u32, u32) -> OcrAnswer>,
    }

    impl FakeHost {
        pub(crate) fn new(answer: impl FnMut(u32, u32) -> OcrAnswer + 'static) -> FakeHost {
            FakeHost {
                sent: Vec::new(),
                ocr_calls: Vec::new(),
                answer: Box::new(answer),
                orient_calls: Vec::new(),
                orient: "Rotate: 0\nOrientation confidence: 9.00\n".into(),
                best_calls: Vec::new(),
                best: Box::new(|_, _| OcrAnswer::Failed(OcrFailure::Unavailable)),
                word_calls: Vec::new(),
                word: Box::new(|_, _| OcrAnswer::Failed(OcrFailure::Crashed)),
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
        fn ocr(
            &mut self,
            _job: JobId,
            page: u16,
            pgm: Vec<u8>,
            mode: OcrMode,
        ) -> Result<OcrAnswer, Fatal> {
            let (w, h) = validate_pgm(&pgm).map_err(|_| Fatal::Protocol)?;
            match mode {
                OcrMode::Orient => {
                    self.orient_calls.push((w, h));
                    Ok(OcrAnswer::Tsv(Tsv(self.orient.clone())))
                }
                OcrMode::ReadBest => {
                    self.best_calls.push((w, h));
                    Ok((self.best)(w, h))
                }
                OcrMode::Word => {
                    self.word_calls.push((w, h));
                    Ok((self.word)(w, h))
                }
                OcrMode::Read => {
                    self.ocr_calls.push((page, w, h));
                    Ok((self.answer)(w, h))
                }
            }
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
            region: None,
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

    /// TSV of `n` confident words across the image.
    fn words(n: u32, w: u32, h: u32) -> OcrAnswer {
        use std::fmt::Write as _;
        let mut s = String::from(
            "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n",
        );
        for i in 0..n {
            let _ = writeln!(
                s,
                "5\t1\t1\t1\t{}\t1\t{}\t{}\t{}\t{}\t95\tWord{i}",
                i + 1,
                w / 10,
                (h / 40) * (i + 1),
                w / 5,
                h / 60
            );
        }
        OcrAnswer::Tsv(Tsv(s))
    }

    #[test]
    fn a_poor_read_of_a_page_on_its_side_is_turned_and_read_again() {
        // First read poor; the orientation pass says turn 90; the second
        // read (of a canvas with swapped sides) is good and kept.
        let mut n = 0;
        let mut host = FakeHost::new(move |w, h| {
            n += 1;
            if n == 1 {
                one_word(w, h)
            } else {
                words(20, w, h)
            }
        });
        host.orient =
            "Orientation in degrees: 270\nRotate: 90\nOrientation confidence: 4.10\n".into();
        let out = run_job(&ctx(true), &mut host, &job(fixture("receipt.png"))).unwrap();
        let FromWorker::JobDone { reading, .. } = out else {
            panic!("{out:?}")
        };
        assert_eq!(host.orient_calls.len(), 1);
        assert_eq!(host.ocr_calls.len(), 2);
        let ((_, w1, h1), (_, w2, h2)) = (host.ocr_calls[0], host.ocr_calls[1]);
        assert_eq!((w1, h1), (h2, w2), "the sides swap");
        assert_eq!(reading.words[0].words.len(), 20);
        assert!(
            reading.words[0]
                .words
                .iter()
                .all(|w| w.bbox.right() <= 10_000)
        );

        // Unsure orientation: no second read.
        let mut host = FakeHost::new(one_word);
        host.orient = "Rotate: 90\nOrientation confidence: 0.40\n".into();
        run_job(&ctx(true), &mut host, &job(fixture("receipt.png"))).unwrap();
        assert_eq!((host.orient_calls.len(), host.ocr_calls.len()), (1, 1));
        // A good first read is never second-guessed.
        let mut host = FakeHost::new(|w, h| words(20, w, h));
        run_job(&ctx(true), &mut host, &job(fixture("receipt.png"))).unwrap();
        assert_eq!((host.orient_calls.len(), host.ocr_calls.len()), (0, 1));
    }

    #[test]
    fn a_dozen_confident_shapes_among_many_still_ask_for_the_orientation() {
        // A page on its side read as short shapes: 12 confident among 120
        // (the count alone passed; Linux and macOS CI, A03).
        let shapes = |w: u32, h: u32| {
            use std::fmt::Write as _;
            let mut s = String::from(
                "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n",
            );
            // 12 lines of 10 shapes; the first on each line confident.
            for line in 0..12u32 {
                for word in 0..10u32 {
                    let (text, conf) = if word == 0 { ("Oo", 70) } else { ("oOo", 30) };
                    let _ = writeln!(
                        s,
                        "5\t1\t1\t1\t{}\t{}\t{}\t{}\t{}\t{}\t{conf}\t{text}",
                        line + 1,
                        word + 1,
                        (w / 11) * word + 1,
                        (h / 14) * (line + 1),
                        w / 15,
                        h / 30
                    );
                }
            }
            OcrAnswer::Tsv(Tsv(s))
        };
        let mut n = 0;
        let mut host = FakeHost::new(move |w, h| {
            n += 1;
            if n == 1 {
                shapes(w, h)
            } else {
                words(20, w, h)
            }
        });
        host.orient = "Rotate: 90\nOrientation confidence: 19.40\n".into();
        let out = run_job(&ctx(true), &mut host, &job(fixture("receipt.png"))).unwrap();
        let FromWorker::JobDone { reading, .. } = out else {
            panic!("{out:?}")
        };
        assert_eq!((host.orient_calls.len(), host.ocr_calls.len()), (1, 2));
        assert_eq!(reading.words[0].words.len(), 20);

        // A read mostly confident is still never second-guessed.
        let mixed = |w, h| {
            let list: Vec<(&str, u32)> = (0..30)
                .map(|i| {
                    if i % 3 == 0 {
                        ("blur", 40)
                    } else {
                        ("Total", 90)
                    }
                })
                .collect();
            tsv_of(&list, w, h)
        };
        let mut host = FakeHost::new(mixed);
        run_job(&ctx(true), &mut host, &job(fixture("receipt.png"))).unwrap();
        assert_eq!((host.orient_calls.len(), host.ocr_calls.len()), (0, 1));
    }

    /// TSV of `(text, confidence %)` words, one per line down the image.
    fn tsv_of(list: &[(&str, u32)], w: u32, h: u32) -> OcrAnswer {
        use std::fmt::Write as _;
        let mut s = String::from(
            "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n",
        );
        for (i, (text, conf)) in (0u32..).zip(list) {
            let _ = writeln!(
                s,
                "5\t1\t1\t1\t{}\t1\t{}\t{}\t{}\t{}\t{conf}\t{text}",
                i + 1,
                w / 10,
                (h / 40) * (i + 1),
                w / 5,
                h / 60
            );
        }
        OcrAnswer::Tsv(Tsv(s))
    }

    fn texts(reading: &cl_docread::model::Reading) -> Vec<String> {
        reading.words[0]
            .words
            .iter()
            .map(|w| w.text.clone())
            .collect()
    }

    #[test]
    fn a_page_with_unsure_numbers_is_read_again_with_the_best_model() {
        let unsure = [("Invoice", 96), ("1042", 70), ("12.40", 80), ("Total", 96)];
        let sure = [("Invoice", 96), ("1042", 95), ("12.40", 94), ("Total", 96)];
        let mut host = FakeHost::new(move |w, h| tsv_of(&unsure, w, h));
        host.best = Box::new(move |w, h| tsv_of(&sure, w, h));
        let out = run_job(&ctx(true), &mut host, &job(fixture("receipt.png"))).unwrap();
        let FromWorker::JobDone { reading, .. } = out else {
            panic!("{out:?}")
        };
        assert_eq!((host.ocr_calls.len(), host.best_calls.len()), (1, 1));
        // The same image is read again, and the second read is kept.
        assert_eq!(
            (host.ocr_calls[0].1, host.ocr_calls[0].2),
            host.best_calls[0]
        );
        let conf: Vec<u16> = reading.words[0]
            .words
            .iter()
            .map(|w| w.confidence.bp())
            .collect();
        assert_eq!(conf, [9_600, 9_500, 9_400, 9_600]);

        // Without the best model (the host can't) the first read stays.
        let mut host = FakeHost::new(move |w, h| tsv_of(&unsure, w, h));
        let out = run_job(&ctx(true), &mut host, &job(fixture("receipt.png"))).unwrap();
        let FromWorker::JobDone { reading, .. } = out else {
            panic!("{out:?}")
        };
        assert_eq!(host.best_calls.len(), 1);
        assert_eq!(reading.words[0].words[1].confidence.bp(), 7_000);

        // Confident numbers are not read again.
        let mut host = FakeHost::new(move |w, h| tsv_of(&sure, w, h));
        run_job(&ctx(true), &mut host, &job(fixture("receipt.png"))).unwrap();
        assert_eq!(host.best_calls.len(), 0);
    }

    /// Review S1: the second read never changes a first-read amount; the
    /// other value becomes a check on the field that uses it.
    #[test]
    fn a_second_read_keeps_the_first_amount_and_flags_the_other() {
        // "Total 12.40" on one line, unsure; the best model reads 12.48.
        let line = |amount: &'static str, conf: u32| {
            move |w: u32, h: u32| {
                OcrAnswer::Tsv(Tsv(format!(
                    "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                     5\t1\t1\t1\t1\t1\t{}\t{}\t{}\t{}\t96\tTotal\n\
                     5\t1\t1\t1\t1\t2\t{}\t{}\t{}\t{}\t{conf}\t{amount}\n",
                    w / 10,
                    h / 2,
                    w / 6,
                    h / 60,
                    w * 6 / 10,
                    h / 2,
                    w / 6,
                    h / 60,
                )))
            }
        };
        let mut host = FakeHost::new(line("12.40", 70));
        host.best = Box::new(line("12.48", 95));
        let out = run_job(&ctx(true), &mut host, &job(fixture("receipt.png"))).unwrap();
        let FromWorker::JobDone { reading, .. } = out else {
            panic!("{out:?}")
        };
        assert_eq!(host.best_calls.len(), 1);
        assert_eq!(texts(&reading), ["Total", "12.40"]);
        let total = reading.header.total.as_ref().expect("total");
        assert_eq!(total.value, cl_core::Money::parse_wire("12.40").unwrap());
        let c = reading
            .checks
            .iter()
            .find(|c| c.code == cl_docread::model::CheckCode::ReadersDisagree)
            .expect("check");
        assert_eq!(c.field, Some(cl_docread::model::FieldName::Total));
    }

    #[test]
    fn a_date_that_did_not_read_is_read_again_on_its_own() {
        let page = [
            ("Date:", 96),
            ("64/13/26", 93),
            ("FES", 90),
            ("21", 95),
            ("2026", 95),
            ("Total", 96),
            ("12.40", 96),
        ];
        let mut host = FakeHost::new(move |w, h| {
            // The month triple on one line.
            let OcrAnswer::Tsv(Tsv(s)) = tsv_of(&page, w, h) else {
                unreachable!()
            };
            let s = s
                .lines()
                .enumerate()
                .map(|(i, l)| {
                    if (4..=5).contains(&i) {
                        let mut c: Vec<&str> = l.split('\t').collect();
                        c[4] = "3";
                        c.join("\t")
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            OcrAnswer::Tsv(Tsv(s + "\n"))
        });
        let mut n = 0;
        host.word = Box::new(move |w, h| {
            n += 1;
            match n {
                // The numeric date: first enlargement still wrong, second right.
                1 => tsv_of(&[("64/13/26", 80)], w, h),
                2 => tsv_of(&[("04/13/26", 71)], w, h),
                // The month triple reads at once.
                _ => tsv_of(&[("FEB", 88), ("21", 90), ("2026", 92)], w, h),
            }
        });
        let out = run_job(&ctx(true), &mut host, &job(fixture("receipt.png"))).unwrap();
        let FromWorker::JobDone { reading, .. } = out else {
            panic!("{out:?}")
        };
        assert_eq!(host.word_calls.len(), 3);
        // The second try was enlarged twice as much as the first.
        assert!(host.word_calls[1].0 > host.word_calls[0].0);
        assert_eq!(
            texts(&reading),
            ["Date:", "04/13/26", "FEB", "21", "2026", "Total", "12.40"]
        );
        assert_eq!(reading.words[0].words[1].confidence.bp(), 7_100);

        // A re-read that never reads as a date changes nothing.
        let mut host = FakeHost::new(move |w, h| tsv_of(&page[..2], w, h));
        host.word = Box::new(|w, h| tsv_of(&[("64/13/2b", 80)], w, h));
        let out = run_job(&ctx(true), &mut host, &job(fixture("receipt.png"))).unwrap();
        let FromWorker::JobDone { reading, .. } = out else {
            panic!("{out:?}")
        };
        assert_eq!(host.word_calls.len(), REREAD_SCALES.len());
        assert_eq!(texts(&reading), ["Date:", "64/13/26"]);
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

    fn region_job(bytes: Vec<u8>, b: (u16, u16, u16, u16)) -> Job {
        Job {
            region: Some(Spot {
                page: 0,
                bbox: Bbox::new(b.0, b.1, b.2, b.3).unwrap(),
            }),
            ..job(bytes)
        }
    }

    /// Version 4: only the box is OCR'd (after the page's orientation is
    /// asked), and the answer is that box's words and nothing else.
    #[test]
    fn a_region_reads_only_its_box_and_answers_with_words() {
        let mut host = FakeHost::new(one_word);
        let j = region_job(fixture("receipt.png"), (2_000, 3_000, 8_000, 7_000));
        let out = run_job(&ctx(true), &mut host, &j).unwrap();
        let FromWorker::JobDone { reading, .. } = out else {
            panic!("{out:?}")
        };
        assert_eq!(host.started(), [0]);
        assert_eq!(host.orient_calls.len(), 1, "orientation always asked");
        assert_eq!(host.ocr_calls.len(), 1);
        // The cut-out, not the page, went to OCR.
        let png = decode::decode_image(&fixture("receipt.png")).unwrap();
        let cleaned = preprocess::clean(&png);
        let (_, w, h) = host.ocr_calls[0];
        let (pw, ph) = cleaned.image.dimensions();
        assert!(w < pw && h < ph, "{w}x{h} of {pw}x{ph}");
        // The middle of the cut-out is the middle of the box.
        let words = &reading.words[0].words;
        assert_eq!(words.len(), 1);
        let b = words[0].bbox;
        let (cx, cy) = (
            u32::midpoint(u32::from(b.left()), u32::from(b.right())),
            u32::midpoint(u32::from(b.top()), u32::from(b.bottom())),
        );
        assert!(
            cx.abs_diff(5_000) <= 250 && cy.abs_diff(5_000) <= 250,
            "{b:?}"
        );
        assert!(cl_docread::pipeline::is_region_reading(
            &reading,
            j.region.unwrap()
        ));
        assert_eq!(reading.header, cl_docread::model::Header::default());
        assert!(reading.fingerprint.is_none());
    }

    #[test]
    fn a_region_keeps_only_words_centred_inside_it() {
        // The word sits in the middle tenth of the cut-out; a box whose
        // margin holds another word must not answer with it.
        let two = |w: u32, h: u32| tsv_of(&[("inside", 95)], w, h);
        let mut host = FakeHost::new(two);
        let j = region_job(fixture("receipt.png"), (0, 0, 10_000, 1_000));
        let out = run_job(&ctx(true), &mut host, &j).unwrap();
        let FromWorker::JobDone { reading, .. } = out else {
            panic!("{out:?}")
        };
        let region = j.region.unwrap().bbox;
        assert!(
            reading.words[0]
                .words
                .iter()
                .all(|w| cl_docread::protocol::centre_in(region, w.bbox))
        );
    }

    #[test]
    fn a_nearly_whole_page_region_is_a_page_read() {
        let mut host = FakeHost::new(|w, h| words(20, w, h));
        let j = region_job(fixture("receipt.png"), (100, 100, 9_950, 9_950));
        let out = run_job(&ctx(true), &mut host, &j).unwrap();
        let FromWorker::JobDone { reading, .. } = out else {
            panic!("{out:?}")
        };
        let png = decode::decode_image(&fixture("receipt.png")).unwrap();
        let (pw, ph) = preprocess::clean(&png).image.dimensions();
        assert_eq!(host.ocr_calls, [(0, pw, ph)]);
        // A good read asks no orientation, as a full read.
        assert_eq!(host.orient_calls.len(), 0);
        assert!(reading.words[0].words.len() <= 20);
        assert_eq!(reading.header, cl_docread::model::Header::default());
    }

    #[test]
    fn a_region_that_cannot_be_read_fails_its_job() {
        let j = region_job(fixture("receipt.png"), (2_000, 2_000, 4_000, 4_000));
        let out = run_job(&ctx(false), &mut FakeHost::new(one_word), &j).unwrap();
        assert_eq!(reason(&out), Some(JobFailure::NeedsOcr));
        let mut host = FakeHost::new(|_, _| OcrAnswer::Failed(OcrFailure::TimedOut));
        let out = run_job(&ctx(true), &mut host, &j).unwrap();
        assert_eq!(reason(&out), Some(JobFailure::Unreadable));
        // Nothing read in the box is an answer, not a failure.
        let mut host = FakeHost::new(|w, h| tsv_of(&[], w, h));
        let out = run_job(&ctx(true), &mut host, &j).unwrap();
        let FromWorker::JobDone { reading, .. } = out else {
            panic!("{out:?}")
        };
        assert_eq!(reading.words[0].words.len(), 0);
        // A TIFF region reads only its own page.
        let mut host = FakeHost::new(one_word);
        let mut t = region_job(fixture("three_pages.tif"), (0, 0, 5_000, 5_000));
        t.region = Some(Spot {
            page: 2,
            bbox: Bbox::new(0, 0, 5_000, 5_000).unwrap(),
        });
        let out = run_job(&ctx(true), &mut host, &t).unwrap();
        assert!(matches!(out, FromWorker::JobDone { .. }), "{out:?}");
        assert_eq!(host.started(), [2]);
    }

    #[test]
    fn region_caps_on_a_batch() {
        let ok = region_job(vec![1], (0, 0, 100, 100));
        assert!(batch_within_caps(std::slice::from_ref(&ok)));
        let small = region_job(vec![1], (0, 0, 99, 100));
        assert!(!batch_within_caps(&[small]));
        let mut far = ok.clone();
        far.region = Some(Spot {
            page: MAX_PAGES,
            bbox: Bbox::PAGE,
        });
        assert!(!batch_within_caps(&[far]));
    }

    #[test]
    fn a_region_on_a_text_pdf_takes_the_text_layer_words() {
        with_pdfium(|c| {
            let mut body: String = (0..30)
                .map(|i| text(72, 760 - i * 20, 10, &format!("Widget {i} 2 x 3.00 6.00")))
                .collect();
            body += &text(400, 100, 12, "Total 544.82");
            // The total's line: x 400..~490 pt of 612, about 680-695 pt down
            // of 792.
            let j = region_job(build(&[Page::letter(body)]), (6_000, 8_400, 9_800, 9_000));
            let mut host = FakeHost::new(one_word);
            let out = run_job(&c, &mut host, &j).unwrap();
            let FromWorker::JobDone { reading, .. } = out else {
                panic!("{out:?}")
            };
            assert_eq!(host.ocr_calls, []);
            assert_eq!(host.orient_calls, []);
            assert_eq!(reading.words[0].source, WordSource::TextLayer);
            let got: Vec<&str> = reading.words[0]
                .words
                .iter()
                .map(|w| w.text.as_str())
                .collect();
            assert!(
                got.contains(&"544.82") && !got.contains(&"INVOICE"),
                "{got:?}"
            );
        });
    }
}
