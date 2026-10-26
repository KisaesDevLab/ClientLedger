//! The worker pipe (**frozen** in A03 step 2; plan D2 "Protocol").
//!
//! `cl-host` starts `clientledger-docread` per batch and talks to it over
//! stdin/stdout. Each frame is a `u32` big-endian length followed by that
//! many bytes of JSON: [`ToWorker`] one way, [`FromWorker`] the other.
//! Document bytes and page images travel as base64 strings inside the JSON
//! ([`Bytes`]).
//!
//! Sequence: worker → `Ready`; host → `Batch`; per job the worker sends
//! `PageStarted` for each page, `NeedOcr` for a page without a usable text
//! layer (the host runs tesseract and answers `OcrResult` or `OcrFailed`),
//! then `JobDone` or `JobFailed`. `Render` is reserved for P37 and always
//! answered `Unsupported` (Q-A03u).
//!
//! An oversized or garbled frame is a protocol breach: the reader returns
//! [`FrameError`] and the host kills the worker. File names, contacts and
//! accounts never cross this pipe; `Debug` prints no document content.
//!
//! **Change rule.** Frozen: WP-B (worker, host) and WP-A (rules) build on it
//! in parallel. Changes go through the orchestrator.

use crate::model::{Reading, Template};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use cl_core::CalendarDate;
use cl_core::intake::IntakeKind;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::{Read, Write};

/// Protocol version the worker announces in `Ready`; the host refuses any
/// other (worker and host ship together).
pub const PROTOCOL_VERSION: u32 = 2;

/// Largest document (plan §5 "Large"): 10 MiB.
pub const MAX_DOCUMENT_BYTES: usize = 10 * 1024 * 1024;
/// Sum of document bytes in one batch; the host splits larger sets.
pub const MAX_BATCH_DOCUMENT_BYTES: usize = 40 * 1024 * 1024;
/// Jobs in one batch (the host also recycles the worker after 20 jobs).
pub const MAX_JOBS_PER_BATCH: usize = 20;
/// Templates sent with one job (plan §5).
pub const MAX_TEMPLATES: usize = 500;
/// Largest host → worker frame: the batch's documents in base64 plus
/// templates and JSON overhead.
pub const MAX_TO_WORKER_FRAME: usize = 64 * 1024 * 1024;
/// Largest page image the worker may ask the host to OCR (plan D2).
pub const MAX_PGM_BYTES: usize = 32 * 1024 * 1024;
/// Largest tesseract TSV the host passes back.
pub const MAX_TSV_BYTES: usize = 4 * 1024 * 1024;
/// Largest `JobDone` frame.
pub const MAX_JOB_DONE_FRAME: usize = 4 * 1024 * 1024;
/// Largest worker → host frame of any kind: a `NeedOcr` page in base64.
pub const MAX_FROM_WORKER_FRAME: usize = MAX_PGM_BYTES / 3 * 4 + 64 * 1024;
/// Largest OCR page: width.
pub const MAX_PGM_WIDTH: u32 = 3_400;
/// Largest OCR page: height.
pub const MAX_PGM_HEIGHT: u32 = 4_400;

/// The host's number for one document in a batch.
pub type JobId = u32;

/// Raw bytes, base64 in JSON. `Debug` prints only the length.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Bytes(pub Vec<u8>);

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes({} bytes)", self.0.len())
    }
}

impl Serialize for Bytes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = Bytes;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a base64 string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Bytes, E> {
                STANDARD
                    .decode(v)
                    .map(Bytes)
                    .map_err(|_| E::custom("invalid base64"))
            }
        }
        d.deserialize_str(V)
    }
}

/// What the owner said the document is, if anything (an intake folder's
/// kind or the button they used). The reader still decides; this breaks
/// ties.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct JobHint {
    pub kind: Option<IntakeKind>,
}

/// One document to read.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub job: JobId,
    /// The file's bytes (at most [`MAX_DOCUMENT_BYTES`]); the worker sniffs
    /// the type itself.
    pub bytes: Bytes,
    pub hint: JobHint,
    /// The company's vendor layout templates (at most [`MAX_TEMPLATES`]);
    /// the reading names the one used by index.
    pub templates: Vec<Template>,
}

impl fmt::Debug for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Job")
            .field("job", &self.job)
            .field("bytes", &self.bytes)
            .field("hint", &self.hint)
            .field("templates", &self.templates.len())
            .finish()
    }
}

/// Options for the whole batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BatchOptions {
    /// Whether the host can run OCR (tesseract present). Without it, pages
    /// with no text layer are reported as not read.
    pub ocr: bool,
    /// The owner's "today": two-digit years and date sanity checks.
    pub today: CalendarDate,
}

/// Host → worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum ToWorker {
    /// Read these documents, in order.
    Batch {
        jobs: Vec<Job>,
        options: BatchOptions,
    },
    /// Tesseract's TSV for a page the worker asked about.
    OcrResult { job: JobId, page: u16, tsv: Tsv },
    /// The page could not be read.
    OcrFailed {
        job: JobId,
        page: u16,
        reason: OcrFailure,
    },
    /// Reserved for P37 (the viewer in the worker); answered `Unsupported`.
    Render { job: JobId, page: u16, dpi: u16 },
}

/// Tesseract's TSV output (at most [`MAX_TSV_BYTES`]). `Debug` prints only
/// the length.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Tsv(pub String);

impl fmt::Debug for Tsv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Tsv({} bytes)", self.0.len())
    }
}

/// What the host runs tesseract for (protocol version 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OcrMode {
    /// Read the words (TSV).
    #[default]
    Read,
    /// Report the page's orientation (`--psm 0`): a quarter-turned page
    /// whose first read came back poor.
    Orient,
}

/// Why OCR of a page failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OcrFailure {
    /// No tesseract or model data on this machine.
    Unavailable,
    /// The page took longer than the per-page limit.
    TimedOut,
    /// Tesseract crashed, was killed for memory, or exited with an error.
    Crashed,
    /// The page image failed the host's PGM check.
    BadImage,
    /// Tesseract's output was over [`MAX_TSV_BYTES`].
    TooMuchOutput,
}

/// Worker → host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum FromWorker {
    /// The worker set its limits and is waiting for a batch.
    Ready { version: u32 },
    /// Work on a page began (the host's per-page clock starts).
    PageStarted { job: JobId, page: u16 },
    /// Please OCR this page: an 8-bit binary PGM (`P5`, at most
    /// [`MAX_PGM_WIDTH`] × [`MAX_PGM_HEIGHT`], maxval 255). With
    /// [`OcrMode::Orient`] the answer's text is tesseract's orientation
    /// report, not TSV (version 2).
    NeedOcr {
        job: JobId,
        page: u16,
        pgm: Bytes,
        mode: OcrMode,
    },
    /// The job is finished.
    JobDone { job: JobId, reading: Box<Reading> },
    /// The job could not be read.
    JobFailed { job: JobId, reason: JobFailure },
    /// A request this worker does not support (`Render`).
    Unsupported { job: JobId, page: u16 },
}

/// Why a document could not be read. The owner sees "Couldn't read, enter
/// by hand" plus a plain reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobFailure {
    /// Not a PDF or an image the reader knows.
    UnsupportedType,
    /// Damaged, or no page had anything to read.
    Unreadable,
    /// Password-protected PDF.
    Protected,
    /// Over a size limit (bytes, pixels, pages).
    TooLarge,
    /// A zero-byte file.
    Empty,
    /// Every page needed OCR and OCR is unavailable.
    NeedsOcr,
}

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

/// A frame could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The other side closed the pipe between frames.
    #[error("the pipe closed")]
    Closed,
    /// Declared or encoded length over the cap.
    #[error("frame of {len} bytes is over the {max}-byte limit")]
    TooLarge { len: usize, max: usize },
    /// Not valid JSON for the expected message. The serde message is not
    /// kept: it can quote document content.
    #[error("garbled frame")]
    Garbled,
    #[error("pipe error: {0}")]
    Io(#[from] std::io::Error),
}

/// Write one frame (`u32` BE length + JSON).
///
/// # Errors
/// The encoded message is over `max` bytes, or the pipe failed.
pub fn write_frame<W: Write, T: Serialize>(
    w: &mut W,
    msg: &T,
    max: usize,
) -> Result<(), FrameError> {
    let body = serde_json::to_vec(msg).map_err(|_| FrameError::Garbled)?;
    if body.len() > max {
        return Err(FrameError::TooLarge {
            len: body.len(),
            max,
        });
    }
    let len = u32::try_from(body.len()).map_err(|_| FrameError::TooLarge {
        len: body.len(),
        max,
    })?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()?;
    Ok(())
}

/// Read one frame. The length is checked against `max` **before** any body
/// byte is read or buffered.
///
/// # Errors
/// [`FrameError::Closed`] at a clean end of stream, `TooLarge`, `Garbled`,
/// or an I/O error (including a stream that ends inside a frame).
pub fn read_frame<R: Read, T: serde::de::DeserializeOwned>(
    r: &mut R,
    max: usize,
) -> Result<T, FrameError> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < len.len() {
        let n = r.read(&mut len[got..])?;
        if n == 0 {
            return if got == 0 {
                Err(FrameError::Closed)
            } else {
                Err(FrameError::Io(std::io::ErrorKind::UnexpectedEof.into()))
            };
        }
        got += n;
    }
    let len = usize::try_from(u32::from_be_bytes(len)).map_err(|_| FrameError::TooLarge {
        len: usize::MAX,
        max,
    })?;
    if len > max {
        return Err(FrameError::TooLarge { len, max });
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|_| FrameError::Garbled)
}

// ---------------------------------------------------------------------------
// PGM pages
// ---------------------------------------------------------------------------

/// A page image that is not a PGM the host will pass to tesseract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PgmError {
    #[error("not a binary PGM (P5) header")]
    Header,
    #[error("page image is larger than the OCR limit")]
    TooLarge,
    #[error("only 8-bit grey (maxval 255) is accepted")]
    Depth,
    #[error("pixel data does not match the header")]
    Length,
}

/// Check an 8-bit binary PGM exactly as plan D2 asks: `P5`, single
/// whitespace-separated decimal header fields with no comments, width and
/// height `1..=` [`MAX_PGM_WIDTH`] × [`MAX_PGM_HEIGHT`], maxval 255, and
/// exactly width × height pixel bytes after the header. Returns
/// `(width, height)`.
///
/// # Errors
/// See [`PgmError`].
pub fn validate_pgm(bytes: &[u8]) -> Result<(u32, u32), PgmError> {
    if bytes.len() > MAX_PGM_BYTES {
        return Err(PgmError::TooLarge);
    }
    let rest = bytes.strip_prefix(b"P5").ok_or(PgmError::Header)?;
    let mut pos = 0usize;
    let mut field = || -> Result<u32, PgmError> {
        // Exactly one whitespace byte before each field.
        if !rest.get(pos).is_some_and(u8::is_ascii_whitespace) {
            return Err(PgmError::Header);
        }
        pos += 1;
        let start = pos;
        while rest.get(pos).is_some_and(u8::is_ascii_digit) {
            pos += 1;
            if pos - start > 5 {
                return Err(PgmError::Header);
            }
        }
        if pos == start {
            return Err(PgmError::Header);
        }
        std::str::from_utf8(&rest[start..pos])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or(PgmError::Header)
    };
    let (w, h, maxval) = (field()?, field()?, field()?);
    // One whitespace byte ends the header.
    if !rest.get(pos).is_some_and(u8::is_ascii_whitespace) {
        return Err(PgmError::Header);
    }
    pos += 1;
    if w == 0 || h == 0 || w > MAX_PGM_WIDTH || h > MAX_PGM_HEIGHT {
        return Err(PgmError::TooLarge);
    }
    if maxval != 255 {
        return Err(PgmError::Depth);
    }
    let pixels = usize::try_from(u64::from(w) * u64::from(h)).map_err(|_| PgmError::TooLarge)?;
    if rest.len() - pos != pixels {
        return Err(PgmError::Length);
    }
    Ok((w, h))
}

/// Build an 8-bit binary PGM from grey pixels, row by row.
///
/// # Errors
/// The size is over the OCR limit or `pixels` is not `width × height`.
pub fn encode_pgm(width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>, PgmError> {
    if width == 0 || height == 0 || width > MAX_PGM_WIDTH || height > MAX_PGM_HEIGHT {
        return Err(PgmError::TooLarge);
    }
    let n =
        usize::try_from(u64::from(width) * u64::from(height)).map_err(|_| PgmError::TooLarge)?;
    if pixels.len() != n {
        return Err(PgmError::Length);
    }
    let mut out = format!("P5\n{width} {height}\n255\n").into_bytes();
    out.extend_from_slice(pixels);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Reader;

    fn options() -> BatchOptions {
        BatchOptions {
            ocr: true,
            today: CalendarDate::from_ymd(2026, 10, 26).unwrap(),
        }
    }

    #[test]
    fn frames_round_trip() {
        let msgs = vec![
            ToWorker::Batch {
                jobs: vec![Job {
                    job: 7,
                    bytes: Bytes(b"%PDF-1.7 secret".to_vec()),
                    hint: JobHint {
                        kind: Some(IntakeKind::Bills),
                    },
                    templates: Vec::new(),
                }],
                options: options(),
            },
            ToWorker::OcrResult {
                job: 7,
                page: 0,
                tsv: Tsv("level\tpage_num\n".into()),
            },
            ToWorker::OcrFailed {
                job: 7,
                page: 1,
                reason: OcrFailure::TimedOut,
            },
            ToWorker::Render {
                job: 7,
                page: 0,
                dpi: 150,
            },
        ];
        let mut buf = Vec::new();
        for m in &msgs {
            write_frame(&mut buf, m, MAX_TO_WORKER_FRAME).unwrap();
        }
        let mut r = buf.as_slice();
        for m in &msgs {
            let got: ToWorker = read_frame(&mut r, MAX_TO_WORKER_FRAME).unwrap();
            assert_eq!(&got, m);
        }
        assert!(matches!(
            read_frame::<_, ToWorker>(&mut r, MAX_TO_WORKER_FRAME),
            Err(FrameError::Closed)
        ));

        let back = vec![
            FromWorker::Ready {
                version: PROTOCOL_VERSION,
            },
            FromWorker::PageStarted { job: 7, page: 0 },
            FromWorker::NeedOcr {
                job: 7,
                page: 0,
                pgm: Bytes(encode_pgm(2, 1, &[0, 255]).unwrap()),
                mode: OcrMode::Orient,
            },
            FromWorker::JobDone {
                job: 7,
                reading: Box::new(Reading::empty(Reader::Ocr, 1)),
            },
            FromWorker::JobFailed {
                job: 8,
                reason: JobFailure::Protected,
            },
            FromWorker::Unsupported { job: 7, page: 0 },
        ];
        let mut buf = Vec::new();
        for m in &back {
            write_frame(&mut buf, m, MAX_FROM_WORKER_FRAME).unwrap();
        }
        let mut r = buf.as_slice();
        for m in &back {
            let got: FromWorker = read_frame(&mut r, MAX_FROM_WORKER_FRAME).unwrap();
            assert_eq!(&got, m);
        }
    }

    #[test]
    fn oversized_frames_are_refused_before_reading_the_body() {
        let mut huge = (u32::MAX).to_be_bytes().to_vec();
        huge.extend_from_slice(b"{}");
        assert!(matches!(
            read_frame::<_, FromWorker>(&mut huge.as_slice(), MAX_FROM_WORKER_FRAME),
            Err(FrameError::TooLarge { .. })
        ));
        let msg = FromWorker::NeedOcr {
            job: 1,
            page: 0,
            pgm: Bytes(vec![0; 100]),
            mode: OcrMode::Read,
        };
        let mut buf = Vec::new();
        assert!(matches!(
            write_frame(&mut buf, &msg, 50),
            Err(FrameError::TooLarge { .. })
        ));
        assert_eq!(buf, Vec::<u8>::new());
    }

    #[test]
    fn garbled_and_truncated_frames_are_errors() {
        let mut f = 5u32.to_be_bytes().to_vec();
        f.extend_from_slice(b"nope!");
        assert!(matches!(
            read_frame::<_, FromWorker>(&mut f.as_slice(), 100),
            Err(FrameError::Garbled)
        ));
        let mut f = 2u32.to_be_bytes().to_vec();
        f.extend_from_slice(br#"{"type":"ready","version":1,"extra":1}"#);
        // Length says 2: truncated JSON.
        assert!(matches!(
            read_frame::<_, FromWorker>(&mut f.as_slice(), 100),
            Err(FrameError::Garbled)
        ));
        let body = br#"{"type":"ready","version":1,"extra":1}"#;
        let mut f = u32::try_from(body.len()).unwrap().to_be_bytes().to_vec();
        f.extend_from_slice(body);
        assert!(
            matches!(
                read_frame::<_, FromWorker>(&mut f.as_slice(), 100),
                Err(FrameError::Garbled)
            ),
            "unknown fields are refused"
        );
        let mut f = 10u32.to_be_bytes().to_vec();
        f.extend_from_slice(b"{}");
        assert!(matches!(
            read_frame::<_, FromWorker>(&mut f.as_slice(), 100),
            Err(FrameError::Io(_))
        ));
        let f = [0u8, 0];
        assert!(matches!(
            read_frame::<_, FromWorker>(&mut f.as_slice(), 100),
            Err(FrameError::Io(_))
        ));
    }

    #[test]
    fn debug_prints_no_document_content() {
        let job = ToWorker::Batch {
            jobs: vec![Job {
                job: 1,
                bytes: Bytes(b"Riverbend Supply".to_vec()),
                hint: JobHint::default(),
                templates: Vec::new(),
            }],
            options: options(),
        };
        let tsv = ToWorker::OcrResult {
            job: 1,
            page: 0,
            tsv: Tsv("Riverbend 544.82".into()),
        };
        for text in [format!("{job:?}"), format!("{tsv:?}")] {
            assert!(
                !text.contains("Riverbend") && !text.contains("544"),
                "{text}"
            );
        }
    }

    #[test]
    fn pgm_header_is_checked_exactly() {
        let ok = encode_pgm(3, 2, &[1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!(validate_pgm(&ok), Ok((3, 2)));
        let mut long = ok.clone();
        long.push(0);
        assert_eq!(validate_pgm(&long), Err(PgmError::Length));
        assert_eq!(validate_pgm(&ok[..ok.len() - 1]), Err(PgmError::Length));
        assert_eq!(validate_pgm(b"P6\n1 1\n255\n\0\0\0"), Err(PgmError::Header));
        assert_eq!(validate_pgm(b"P5\n1 1\n65535\n\0\0"), Err(PgmError::Depth));
        assert_eq!(
            validate_pgm(b"P5\n# c\n1 1\n255\n\0"),
            Err(PgmError::Header)
        );
        assert_eq!(validate_pgm(b"P5\n3401 1\n255\n"), Err(PgmError::TooLarge));
        assert_eq!(validate_pgm(b"P5\n0 1\n255\n"), Err(PgmError::TooLarge));
        assert_eq!(validate_pgm(b"P5\n1  1\n255\n\0"), Err(PgmError::Header));
        assert_eq!(validate_pgm(b"P5\n1 1\n255"), Err(PgmError::Header));
        assert_eq!(validate_pgm(b"P5\n999999 1\n255\n"), Err(PgmError::Header));
        assert_eq!(encode_pgm(3401, 1, &[0; 3401]), Err(PgmError::TooLarge));
        assert_eq!(encode_pgm(2, 2, &[0; 3]), Err(PgmError::Length));
    }

    #[test]
    fn the_largest_page_fits_its_frame() {
        let pgm = encode_pgm(
            MAX_PGM_WIDTH,
            MAX_PGM_HEIGHT,
            &vec![255; (MAX_PGM_WIDTH * MAX_PGM_HEIGHT) as usize],
        )
        .unwrap();
        assert!(pgm.len() <= MAX_PGM_BYTES);
        let msg = FromWorker::NeedOcr {
            job: u32::MAX,
            page: u16::MAX,
            pgm: Bytes(pgm),
            mode: OcrMode::Read,
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &msg, MAX_FROM_WORKER_FRAME).unwrap();
        // A full batch of the largest documents also fits.
        let raw = MAX_BATCH_DOCUMENT_BYTES;
        assert!(raw.div_ceil(3) * 4 + 4 * 1024 * 1024 < MAX_TO_WORKER_FRAME);
    }
}
