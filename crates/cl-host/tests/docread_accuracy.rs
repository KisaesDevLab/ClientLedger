//! Reading accuracy and measured limits (A03 plan D6, §6 "Accuracy", step
//! 12a; Q-285, review S14/S15).
//!
//! Runs only with `CL_DOCREAD_ACCURACY=1`; `cargo xtask docread-accuracy`
//! sets it after building the **normal** worker optimised (profile
//! `docread-bench`) and writing the synthetic set (`cargo xtask
//! gen-invoices --holdout`). Every document goes through `DocreadHost` one
//! at a time, so each read includes a cold worker start (the S14 clock:
//! dispatch to `JobDone`).
//!
//! Scoring is per field (S15): a missing or wrong value is wrong; a field
//! the document does not print is right only when the reader returns
//! nothing. Vendor is right when `match_vendor` against the roster (with
//! confusable names) picks the right contact; amounts are exact `Money`;
//! references compare after `dupes::normalize_reference`.
//!
//! Paths: `text` (PDF with a text layer), `text-no-unicode` (PDF fonts
//! without `ToUnicode`, read through OCR), `scan`, `photo`, and the
//! template paths: a template is learned from each layout's first document
//! the way A04 will (the first-pass reading corrected to the truth, with
//! the spot of the page words that carry the true value; a value with no
//! such words is not learned), then documents 2-10 are read again with
//! every learned template of that path. The template re-read runs the
//! rules in-process on the words the worker returned (the same
//! `cl_docread::read` call the worker makes), so OCR is not repeated. On
//! the template paths each learned template's fingerprint is on the vendor
//! it was learned from, as A04 saves it, so `match_vendor`'s past-layout
//! tier works as it will in the desktop.
//!
//! Gates (Q-285, applied to seen and held-out separately, with
//! `CL_DOCREAD_GATE=1`): vendor, date, reference and total each ≥ 95 % on
//! `text`, ≥ 85 % on `scan`, `photo` and `text-no-unicode`, ≥ 98 % on the
//! template paths. Held-out results are reported as aggregates only.
//!
//! `private-fixtures/docread/` (git-ignored, Q-72) is scored too when
//! present: `<name>.<pdf|jpg|png|tif>` with `<name>.json` truth in the same
//! schema. Only aggregates are written, never a file name or a value.
//!
//! `stress/` documents (the largest legitimate inputs) are each read by a
//! fresh host; their time and memory peaks set the limits (step 12a).
//!
//! Reports go to `target/docread-accuracy/` (never committed).

use cl_core::ids::ContactId;
use cl_core::{CalendarDate, Money};
use cl_docread::dupes::normalize_reference;
use cl_docread::model::{
    Bbox, Confidence, Found, Header, PageWords, Payment, Reading, RuleId, Spot, Template,
};
use cl_docread::protocol::JobHint;
use cl_docread::templates;
use cl_docread::vendor_match::{VendorCandidate, match_vendor};
use cl_docread::{ReadInput, read};
use cl_host::{DocJob, DocreadConfig, DocreadHost, JobOutcome, OcrTools};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const GATED: [Field; 4] = [Field::Vendor, Field::Date, Field::Reference, Field::Total];
const ALL_FIELDS: [Field; 9] = [
    Field::Vendor,
    Field::Date,
    Field::Reference,
    Field::Total,
    Field::Subtotal,
    Field::Tax,
    Field::DueDate,
    Field::AccountNumber,
    Field::CardLast4,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Field {
    Vendor,
    Date,
    Reference,
    Total,
    Subtotal,
    Tax,
    DueDate,
    AccountNumber,
    CardLast4,
}

impl Field {
    fn name(self) -> &'static str {
        match self {
            Field::Vendor => "vendor",
            Field::Date => "date",
            Field::Reference => "reference",
            Field::Total => "total",
            Field::Subtotal => "subtotal",
            Field::Tax => "tax",
            Field::DueDate => "due_date",
            Field::AccountNumber => "account_number",
            Field::CardLast4 => "card_last4",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Route {
    Text,
    TextNoUnicode,
    Scan,
    Photo,
    TemplateText,
    TemplateScan,
    TemplatePhoto,
}

impl Route {
    fn name(self) -> &'static str {
        match self {
            Route::Text => "text",
            Route::TextNoUnicode => "text-no-unicode",
            Route::Scan => "scan",
            Route::Photo => "photo",
            Route::TemplateText => "template-text",
            Route::TemplateScan => "template-scan",
            Route::TemplatePhoto => "template-photo",
        }
    }

    /// Q-285 minimum in percent.
    fn gate(self) -> u32 {
        match self {
            Route::Text => 95,
            Route::TextNoUnicode | Route::Scan | Route::Photo => 85,
            Route::TemplateText | Route::TemplateScan | Route::TemplatePhoto => 98,
        }
    }

    fn template(self) -> Option<Route> {
        match self {
            Route::Text => Some(Route::TemplateText),
            Route::Scan => Some(Route::TemplateScan),
            Route::Photo => Some(Route::TemplatePhoto),
            _ => None,
        }
    }
}

/// A document's ground truth (the fields scored).
#[derive(Debug, Clone)]
struct Truth {
    layout: String,
    holdout: bool,
    seq: u32,
    vendor_key: String,
    vendor_name: String,
    date: Option<CalendarDate>,
    reference: Option<String>,
    total: Option<Money>,
    subtotal: Option<Money>,
    tax: Option<Money>,
    due_date: Option<CalendarDate>,
    account_number: Option<String>,
    card_last4: Option<String>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

fn money(v: &Value, k: &str) -> Option<Money> {
    s(v, k).and_then(|x| Money::parse_wire(&x).ok())
}

fn day(v: &Value, k: &str) -> Option<CalendarDate> {
    s(v, k).and_then(|x| CalendarDate::parse_wire(&x).ok())
}

impl Truth {
    fn parse(v: &Value) -> Truth {
        let vendor = v.get("vendor").cloned().unwrap_or(Value::Null);
        Truth {
            layout: s(v, "layout").unwrap_or_default(),
            holdout: v.get("holdout").and_then(Value::as_bool).unwrap_or(false),
            seq: v
                .get("seq")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
            vendor_key: s(&vendor, "key").unwrap_or_default(),
            vendor_name: s(&vendor, "name").unwrap_or_default(),
            date: day(v, "date"),
            reference: s(v, "reference"),
            total: money(v, "total"),
            subtotal: money(v, "subtotal"),
            tax: money(v, "tax"),
            due_date: day(v, "due_date"),
            account_number: s(v, "account_number"),
            card_last4: s(v, "card_last4"),
        }
    }
}

// ---------------------------------------------------------------------------
// Vendors
// ---------------------------------------------------------------------------

struct Roster {
    candidates: Vec<VendorCandidate>,
    keys: Vec<String>,
}

fn contact(i: usize) -> ContactId {
    ContactId::parse(&format!("00000000-0000-4000-8000-{i:012x}")).expect("valid id")
}

impl Roster {
    fn load(path: &Path) -> Option<Roster> {
        let v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        let mut candidates = Vec::new();
        let mut keys = Vec::new();
        for (i, e) in v.as_array()?.iter().enumerate() {
            keys.push(s(e, "key").unwrap_or_default());
            candidates.push(VendorCandidate {
                contact: contact(i),
                display_name: s(e, "name").unwrap_or_default(),
                company_name: None,
                account_number: None,
                phone: s(e, "phone"),
                address: s(e, "address"),
                layouts: Vec::new(),
            });
        }
        Some(Roster { candidates, keys })
    }

    /// The roster as the desktop builds it once templates are saved: each
    /// learned template's fingerprint on its vendor (A04 stores a template
    /// with the vendor of the bill it was learned from), so the past-layout
    /// tier of `match_vendor` can work on the template paths.
    fn with_layouts(&self, learned: &[(String, Template)]) -> Roster {
        let mut candidates = self.candidates.clone();
        for (key, t) in learned {
            if let Some(i) = self.keys.iter().position(|k| k == key) {
                candidates[i].layouts.push(t.fingerprint.clone());
            }
        }
        Roster {
            candidates,
            keys: self.keys.clone(),
        }
    }

    fn key_of(&self, header: &Header, reading: &Reading) -> Option<&str> {
        let m = match_vendor(header, reading.fingerprint.as_ref(), &self.candidates)?;
        let i = self
            .candidates
            .iter()
            .position(|c| c.contact == m.contact)?;
        Some(self.keys[i].as_str())
    }
}

fn loose(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

fn opt_eq<T: PartialEq>(got: Option<&T>, want: Option<&T>) -> bool {
    match (got, want) {
        (None, None) => true,
        (Some(g), Some(w)) => g == w,
        _ => false,
    }
}

fn right(field: Field, r: &Reading, t: &Truth, roster: Option<&Roster>) -> bool {
    let h = &r.header;
    match field {
        Field::Vendor => match roster {
            Some(ro) => ro.key_of(h, r) == Some(t.vendor_key.as_str()),
            None => h
                .vendor
                .as_ref()
                .is_some_and(|v| loose(&v.value) == loose(&t.vendor_name)),
        },
        Field::Date => opt_eq(h.date.as_ref().map(|f| &f.value), t.date.as_ref()),
        Field::Reference => opt_eq(
            h.reference
                .as_ref()
                .map(|f| normalize_reference(&f.value))
                .as_ref(),
            t.reference.as_deref().map(normalize_reference).as_ref(),
        ),
        Field::Total => opt_eq(h.total.as_ref().map(|f| &f.value), t.total.as_ref()),
        Field::Subtotal => opt_eq(h.subtotal.as_ref().map(|f| &f.value), t.subtotal.as_ref()),
        Field::Tax => opt_eq(h.tax.as_ref().map(|f| &f.value), t.tax.as_ref()),
        Field::DueDate => opt_eq(h.due_date.as_ref().map(|f| &f.value), t.due_date.as_ref()),
        Field::AccountNumber => opt_eq(
            h.account_number.as_ref().map(|f| loose(&f.value)).as_ref(),
            t.account_number.as_deref().map(loose).as_ref(),
        ),
        Field::CardLast4 => {
            let got = match h.payment.as_ref().map(|p| &p.value) {
                Some(Payment::Card { last4 }) => Some(last4.as_str().to_string()),
                _ => None,
            };
            opt_eq(got.as_ref(), t.card_last4.as_ref())
        }
    }
}

/// right / total per field.
#[derive(Debug, Clone, Default)]
struct Tally {
    docs: u32,
    /// Documents the reader gave up on (counted wrong in every field).
    failed: u32,
    fields: BTreeMap<Field, (u32, u32)>,
}

impl Tally {
    fn add(&mut self, r: Option<&Reading>, t: &Truth, roster: Option<&Roster>) {
        self.docs += 1;
        if r.is_none() {
            self.failed += 1;
        }
        for f in ALL_FIELDS {
            let ok = r.is_some_and(|r| right(f, r, t, roster));
            let e = self.fields.entry(f).or_default();
            e.0 += u32::from(ok);
            e.1 += 1;
        }
    }

    fn pct(&self, f: Field) -> Option<(u32, u32, u32)> {
        let (ok, n) = *self.fields.get(&f)?;
        (n > 0).then(|| (ok, n, (ok * 1000 / n)))
    }
}

// ---------------------------------------------------------------------------
// Template learning (as A04 will, review S15)
// ---------------------------------------------------------------------------

/// Page words on one line from `i`, up to `max` words.
fn runs(page: &PageWords, max: usize) -> Vec<(String, Bbox)> {
    let mut out = Vec::new();
    let ws = &page.words;
    for i in 0..ws.len() {
        let mut text = String::new();
        let mut bbox = ws[i].bbox;
        for (k, w) in ws[i..].iter().take(max).enumerate() {
            if k > 0 {
                let same_line = w.bbox.top().abs_diff(ws[i].bbox.top()) < 80;
                if !same_line {
                    break;
                }
                text.push(' ');
                bbox = bbox.union(w.bbox);
            }
            text.push_str(&w.text);
            out.push((text.clone(), bbox));
        }
    }
    out
}

fn spot_of(r: &Reading, mut matches: impl FnMut(&str) -> bool) -> Option<Spot> {
    for page in &r.words {
        for (text, bbox) in runs(page, 6) {
            if matches(&text) {
                return Some(Spot {
                    page: page.page,
                    bbox,
                });
            }
        }
    }
    None
}

fn corrected<T: Clone>(
    got: Option<&Found<T>>,
    want: Option<&T>,
    ok: bool,
    find: impl FnOnce() -> Option<Spot>,
) -> Option<Found<T>> {
    let want = want?;
    if ok {
        return got.cloned();
    }
    find().map(|spot| Found {
        value: want.clone(),
        spot,
        rule: RuleId::new("harness.corrected"),
        confidence: Confidence::CERTAIN,
    })
}

/// The first-pass reading corrected to the truth, then learned.
fn learn(r: &Reading, t: &Truth, today: CalendarDate) -> Option<Template> {
    let mut c = r.clone();
    let h = &r.header;
    let ok = |f| right(f, r, t, None);
    c.header.vendor = corrected(
        h.vendor.as_ref(),
        Some(&t.vendor_name),
        ok(Field::Vendor),
        || spot_of(r, |s| loose(s) == loose(&t.vendor_name)),
    );
    c.header.date = corrected(h.date.as_ref(), t.date.as_ref(), ok(Field::Date), || {
        spot_of(r, |s| {
            cl_docread::dates::parse(s, today).map(|p| p.date) == t.date
        })
    });
    c.header.reference = corrected(
        h.reference.as_ref(),
        t.reference.as_ref(),
        ok(Field::Reference),
        || {
            let want = t.reference.as_deref().map(normalize_reference);
            spot_of(r, |s| Some(normalize_reference(s)) == want)
        },
    );
    let amount = |want: Option<Money>| move |s: &str| cl_docread::amount::parse(s) == want;
    c.header.total = corrected(h.total.as_ref(), t.total.as_ref(), ok(Field::Total), || {
        spot_of(r, amount(t.total))
    });
    c.header.subtotal = corrected(
        h.subtotal.as_ref(),
        t.subtotal.as_ref(),
        ok(Field::Subtotal),
        || spot_of(r, amount(t.subtotal)),
    );
    c.header.tax = corrected(h.tax.as_ref(), t.tax.as_ref(), ok(Field::Tax), || {
        spot_of(r, amount(t.tax))
    });
    c.header.due_date = corrected(
        h.due_date.as_ref(),
        t.due_date.as_ref(),
        ok(Field::DueDate),
        || {
            spot_of(r, |s| {
                cl_docread::dates::parse(s, today).map(|p| p.date) == t.due_date
            })
        },
    );
    templates::learn(&c)
}

fn reread(r: &Reading, templates: &[Template], today: CalendarDate) -> Reading {
    let unread: Vec<u16> = r
        .pages
        .iter()
        .filter(|p| p.source.is_none())
        .map(|p| p.page)
        .collect();
    read(&ReadInput {
        pages: &r.words,
        page_count: r.page_count,
        unread_pages: &unread,
        templates,
        hint: JobHint::default(),
        today,
    })
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn env_path(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).map(PathBuf::from)
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 10, 26).expect("date")
}

/// One read document.
struct Doc {
    truth: Truth,
    route: Route,
    reading: Option<Reading>,
    /// Wall time dispatch → done (cold worker start included).
    elapsed: Duration,
    pages: u32,
}

struct Timing {
    per_page_ms: Vec<u64>,
}

fn percentile(v: &mut [u64], p: usize) -> u64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    let i = (v.len() * p).div_ceil(100).saturating_sub(1);
    v[i.min(v.len() - 1)]
}

fn host(scratch: &Path) -> Option<DocreadHost> {
    let worker = env_path("CL_DOCREAD_WORKER")?;
    let ocr = env_path("CL_OCR_PATH")
        .and_then(|d| OcrTools::at(&d))
        .map(|mut t| {
            if let Some(td) = env_path("CL_DOCREAD_TESSDATA") {
                t.tessdata = td;
            }
            t
        });
    if ocr.is_none() {
        assert!(
            std::env::var("CL_OCR_REQUIRE").as_deref() != Ok("1"),
            "tesseract is required (CL_OCR_REQUIRE=1) but CL_OCR_PATH has none"
        );
    }
    let paths = cl_host::AppPaths::at(scratch.to_path_buf()).expect("app paths");
    let mut config = DocreadConfig::new(&paths, worker, ocr);
    if let Some(ms) = std::env::var("CL_DOCREAD_POLL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        config.limits.poll = Duration::from_millis(ms);
    }
    Some(DocreadHost::new(config))
}

fn read_one(host: &mut DocreadHost, bytes: Vec<u8>) -> (Option<Reading>, Duration) {
    let job = DocJob {
        bytes,
        hint: JobHint::default(),
        templates: Vec::new(),
    };
    let t = Instant::now();
    let out = host.read(std::slice::from_ref(&job), today());
    let elapsed = t.elapsed();
    let reading = match out.into_iter().next() {
        Some(JobOutcome::Read(r)) => Some(*r),
        _ => None,
    };
    (reading, elapsed)
}

fn wanted_routes() -> Option<Vec<String>> {
    std::env::var("CL_DOCREAD_ROUTES")
        .ok()
        .map(|v| v.split(',').map(str::to_string).collect())
}

fn route_wanted(r: Route, want: Option<&Vec<String>>) -> bool {
    want.is_none_or(|w| w.iter().any(|x| x == r.name()))
}

// ---------------------------------------------------------------------------
// Tuning aids (A03 WP-A2): a reading cache and the seen misses
// ---------------------------------------------------------------------------

/// `CL_DOCREAD_CACHE=<dir>` keeps every worker reading; with
/// `CL_DOCREAD_REUSE=1` a kept reading's words go through the current rules
/// in-process instead of the worker (rule tuning without OCR). Clear the
/// folder after any change to the worker, clean-up or text layer. Timings
/// of reused readings are zero, so a gate run never reuses.
fn cache_path(part: &str, dir: &Path, file: &str) -> Option<PathBuf> {
    let root = env_path("CL_DOCREAD_CACHE")?;
    let layout = dir.file_name()?;
    Some(root.join(part).join(layout).join(format!("{file}.json")))
}

fn reuse(path: &Path) -> Option<Reading> {
    if std::env::var("CL_DOCREAD_REUSE").as_deref() != Ok("1") {
        return None;
    }
    let r: Reading = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    Some(reread(&r, &[], today()))
}

fn store(path: &Path, r: &Reading) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, serde_json::to_vec(r).unwrap_or_default());
}

fn shown<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map_or_else(|| "-".to_string(), |v| v.to_string())
}

/// `CL_DOCREAD_MISSES=1`: every wrong gated field of a **seen** document
/// (synthetic values only; never held-out or private documents).
fn misses<'a>(docs: &[&Doc], roster_of: impl Fn(Route) -> &'a Roster) -> String {
    let mut s = String::from(
        "| layout | seq | path | field | read | truth | rule |\n| --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for d in docs.iter().filter(|d| !d.truth.holdout) {
        let t = &d.truth;
        let roster = roster_of(d.route);
        for f in GATED {
            let ok = d
                .reading
                .as_ref()
                .is_some_and(|r| right(f, r, t, Some(roster)));
            if ok {
                continue;
            }
            let h = d.reading.as_ref().map(|r| &r.header);
            let rule = |r: Option<&RuleId>| shown(r.map(RuleId::as_str));
            let (got, want, rule) = match f {
                Field::Vendor => (
                    shown(h.and_then(|h| h.vendor.as_ref()).map(|v| v.value.clone())),
                    t.vendor_name.clone(),
                    rule(h.and_then(|h| h.vendor.as_ref()).map(|v| &v.rule)),
                ),
                Field::Date => (
                    shown(h.and_then(|h| h.date.as_ref()).map(|v| v.value)),
                    shown(t.date),
                    rule(h.and_then(|h| h.date.as_ref()).map(|v| &v.rule)),
                ),
                Field::Reference => (
                    shown(
                        h.and_then(|h| h.reference.as_ref())
                            .map(|v| v.value.clone()),
                    ),
                    shown(t.reference.clone()),
                    rule(h.and_then(|h| h.reference.as_ref()).map(|v| &v.rule)),
                ),
                _ => (
                    shown(h.and_then(|h| h.total.as_ref()).map(|v| v.value.to_wire())),
                    shown(t.total.map(Money::to_wire)),
                    rule(h.and_then(|h| h.total.as_ref()).map(|v| &v.rule)),
                ),
            };
            let _ = writeln!(
                s,
                "| {} | {} | {} | {} | {got} | {want} | {rule} |",
                t.layout,
                t.seq,
                d.route.name(),
                f.name()
            );
        }
    }
    s
}

/// Read every document of the synthetic set.
fn read_set(host: &mut DocreadHost, set: &Path) -> Vec<Doc> {
    let want = wanted_routes();
    // `CL_DOCREAD_PARTS=seen` while tuning: held-out documents are not
    // read at all, so no held-out number can steer a change (S15).
    let parts = std::env::var("CL_DOCREAD_PARTS").unwrap_or_default();
    let mut out = Vec::new();
    for part in ["seen", "holdout"] {
        if !parts.is_empty() && !parts.split(',').any(|p| p == part) {
            continue;
        }
        let Ok(dirs) = std::fs::read_dir(set.join(part)) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = dirs.filter_map(|e| e.ok().map(|e| e.path())).collect();
        dirs.sort();
        for dir in dirs {
            for seq in 1..=10u32 {
                let stem = format!("{seq:02}");
                let Ok(raw) = std::fs::read(dir.join(format!("{stem}.json"))) else {
                    continue;
                };
                let v: Value = serde_json::from_slice(&raw).expect("truth json");
                let truth = Truth::parse(&v);
                let tounicode = v.get("tounicode").and_then(Value::as_bool).unwrap_or(true);
                let pages = v
                    .get("pages")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(1);
                let pdf_route = if tounicode {
                    Route::Text
                } else {
                    Route::TextNoUnicode
                };
                for (file, route, n) in [
                    (format!("{stem}.pdf"), pdf_route, pages),
                    (format!("{stem}.scan.jpg"), Route::Scan, 1),
                    (format!("{stem}.photo.jpg"), Route::Photo, 1),
                ] {
                    if !route_wanted(route, want.as_ref()) {
                        continue;
                    }
                    let Ok(bytes) = std::fs::read(dir.join(&file)) else {
                        continue;
                    };
                    let cached = cache_path(part, &dir, &file);
                    let (reading, elapsed) = if let Some(r) = cached.as_ref().and_then(|p| reuse(p))
                    {
                        (Some(r), Duration::ZERO)
                    } else {
                        let (reading, elapsed) = read_one(host, bytes);
                        if let (Some(p), Some(r)) = (&cached, &reading) {
                            store(p, r);
                        }
                        (reading, elapsed)
                    };
                    out.push(Doc {
                        truth: truth.clone(),
                        route,
                        reading,
                        elapsed,
                        pages: n,
                    });
                }
            }
        }
    }
    out
}

/// Per template path, each learned template with the key of the vendor it
/// was learned from.
type Learned = BTreeMap<Route, Vec<(String, Template)>>;

/// Template paths: learn from document 1 of each layout, re-read 2-10.
fn template_docs(docs: &[Doc]) -> (Vec<Doc>, Learned) {
    let mut out = Vec::new();
    let mut by_route = BTreeMap::new();
    for base in [Route::Text, Route::Scan, Route::Photo] {
        let Some(route) = base.template() else {
            continue;
        };
        let keyed: Vec<(String, Template)> = docs
            .iter()
            .filter(|d| d.route == base && d.truth.seq == 1)
            .filter_map(|d| {
                let t = learn(d.reading.as_ref()?, &d.truth, today())?;
                Some((d.truth.vendor_key.clone(), t))
            })
            .collect();
        let learned: Vec<Template> = keyed.iter().map(|(_, t)| t.clone()).collect();
        by_route.insert(route, keyed);
        for d in docs.iter().filter(|d| d.route == base && d.truth.seq > 1) {
            let t = Instant::now();
            let reading = d.reading.as_ref().map(|r| reread(r, &learned, today()));
            out.push(Doc {
                truth: d.truth.clone(),
                route,
                reading,
                elapsed: t.elapsed(),
                pages: d.pages,
            });
        }
    }
    (out, by_route)
}

#[derive(Default)]
struct Report {
    /// (group, route) → tally; group is "seen", "holdout" or "private".
    tallies: BTreeMap<(String, Route), Tally>,
    /// Seen layouts only: (layout, route) → tally.
    per_layout: BTreeMap<(String, Route), Tally>,
    timing: BTreeMap<Route, Timing>,
}

impl Report {
    fn add(&mut self, d: &Doc, group: &str, roster: Option<&Roster>) {
        self.tallies
            .entry((group.to_string(), d.route))
            .or_default()
            .add(d.reading.as_ref(), &d.truth, roster);
        if group == "seen" {
            self.per_layout
                .entry((d.truth.layout.clone(), d.route))
                .or_default()
                .add(d.reading.as_ref(), &d.truth, roster);
        }
        if d.route.template().is_some() || matches!(d.route, Route::TextNoUnicode) {
            let ms = u64::try_from(d.elapsed.as_millis()).unwrap_or(u64::MAX)
                / u64::from(d.pages.max(1));
            self.timing
                .entry(d.route)
                .or_insert_with(|| Timing {
                    per_page_ms: Vec::new(),
                })
                .per_page_ms
                .push(ms);
        }
    }

    /// Gate failures: "group route field: x.y% < n%".
    fn gate_failures(&self) -> Vec<String> {
        let mut out = Vec::new();
        for ((group, route), t) in &self.tallies {
            if group == "private" {
                continue;
            }
            for f in GATED {
                if let Some((_, _, permille)) = t.pct(f)
                    && permille < route.gate() * 10
                {
                    out.push(format!(
                        "{group} {} {}: {}.{}% < {}%",
                        route.name(),
                        f.name(),
                        permille / 10,
                        permille % 10,
                        route.gate()
                    ));
                }
            }
        }
        out
    }

    fn table(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "| group | path | docs | failed | {} |",
            ALL_FIELDS.map(Field::name).join(" | ")
        );
        let _ = writeln!(s, "|{}", " --- |".repeat(4 + ALL_FIELDS.len()));
        for ((group, route), t) in &self.tallies {
            let cells: Vec<String> = ALL_FIELDS
                .iter()
                .map(|f| {
                    t.pct(*f).map_or_else(String::new, |(ok, n, pm)| {
                        let mark = if GATED.contains(f) && pm < route.gate() * 10 {
                            " ✗"
                        } else {
                            ""
                        };
                        format!("{}.{}% ({ok}/{n}){mark}", pm / 10, pm % 10)
                    })
                })
                .collect();
            let _ = writeln!(
                s,
                "| {group} | {} (≥{}%) | {} | {} | {} |",
                route.name(),
                route.gate(),
                t.docs,
                t.failed,
                cells.join(" | ")
            );
        }
        s
    }

    fn layouts_table(&self) -> String {
        let mut s =
            String::from("| seen layout | path | wrong (gated fields) |\n| --- | --- | --- |\n");
        for ((layout, route), t) in &self.per_layout {
            let wrong: Vec<String> = GATED
                .iter()
                .filter_map(|f| {
                    let (ok, n) = *t.fields.get(f)?;
                    (ok < n).then(|| format!("{} {}/{n}", f.name(), n - ok))
                })
                .collect();
            if !wrong.is_empty() {
                let _ = writeln!(s, "| {layout} | {} | {} |", route.name(), wrong.join(", "));
            }
        }
        s
    }

    fn timing_json(&mut self) -> Value {
        let mut out = serde_json::Map::new();
        for (route, t) in &mut self.timing {
            let n = t.per_page_ms.len();
            let max = t.per_page_ms.iter().copied().max().unwrap_or(0);
            let p50 = percentile(&mut t.per_page_ms, 50);
            let p95 = percentile(&mut t.per_page_ms, 95);
            out.insert(
                route.name().to_string(),
                json!({ "documents": n, "p50_ms_per_page": p50, "p95_ms_per_page": p95, "max_ms_per_page": max }),
            );
        }
        Value::Object(out)
    }
}

/// The largest legitimate documents (`stress/`, step 12a), each read by a
/// fresh host so its time and memory peaks are its own.
fn read_stress(set: &Path, scratch: &Path) -> Value {
    let mut out = serde_json::Map::new();
    for name in [
        "ten-pages.pdf",
        "ten-pages-scanned.pdf",
        "photo-12mp.jpg",
        "scan-600dpi.jpg",
        "scan-600dpi-colour.jpg",
        "picture-at-decode-cap.jpg",
    ] {
        let Ok(bytes) = std::fs::read(set.join("stress").join(name)) else {
            continue;
        };
        let mut h = host(scratch).expect("host");
        let (reading, elapsed) = read_one(&mut h, bytes);
        let st = h.stats();
        let mib = |b: u64| b / (1024 * 1024);
        out.insert(
            name.to_string(),
            json!({
                "read": reading.is_some(),
                "pages": reading.as_ref().map_or(0, |r| r.page_count),
                "ms": u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                "worker_peak_mib": mib(st.worker_peak_memory),
                "tesseract_peak_mib": mib(st.tesseract_peak_memory),
                "ocr_pages": st.ocr_pages,
                "kills": st.kills,
            }),
        );
    }
    Value::Object(out)
}

/// `private-fixtures/docread/`: aggregates only, never names or values.
fn read_private(host: &mut DocreadHost, report: &mut Report) -> u32 {
    let dir = repo().join("private-fixtures").join("docread");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let roster = Roster::load(&dir.join("roster.json"));
    let mut n = 0;
    let mut files: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    files.sort();
    for f in files {
        let ext = f
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let route = match ext.as_str() {
            "pdf" => Route::Text,
            "jpg" | "jpeg" | "png" | "tif" | "tiff" => Route::Photo,
            _ => continue,
        };
        let Some(truth) = std::fs::read(f.with_extension("json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .map(|v| Truth::parse(&v))
        else {
            continue;
        };
        let Ok(bytes) = std::fs::read(&f) else {
            continue;
        };
        let (reading, elapsed) = read_one(host, bytes);
        // A PDF read through OCR is a scan.
        let route = match (&reading, route) {
            (Some(r), Route::Text) if r.reader == cl_docread::model::Reader::Ocr => Route::Scan,
            _ => route,
        };
        report.add(
            &Doc {
                truth,
                route,
                reading,
                elapsed,
                pages: 1,
            },
            "private",
            roster.as_ref(),
        );
        n += 1;
    }
    n
}

/// `report.md` and `summary.json` in `out_dir`.
fn write_report(out_dir: &Path, report: &Report, summary: &Value) {
    let md = format!(
        "# Reading accuracy\n\nGated fields: vendor, date, reference, total (Q-285). \
         Held-out rows are aggregates only.\n\n{}\n## Seen layouts with wrong gated fields\n\n{}\n\
         ## Measurements\n\n```json\n{}\n```\n",
        report.table(),
        report.layouts_table(),
        serde_json::to_string_pretty(summary).unwrap_or_default()
    );
    std::fs::create_dir_all(out_dir).expect("report folder");
    std::fs::write(out_dir.join("report.md"), &md).expect("write report");
    std::fs::write(
        out_dir.join("summary.json"),
        serde_json::to_vec_pretty(summary).unwrap_or_default(),
    )
    .expect("write summary");
}

#[test]
fn reading_accuracy_against_q285() {
    if std::env::var("CL_DOCREAD_ACCURACY").as_deref() != Ok("1") {
        eprintln!("skipped: set CL_DOCREAD_ACCURACY=1 (cargo xtask docread-accuracy)");
        return;
    }
    let set = env_path("CL_DOCREAD_SET").unwrap_or_else(|| repo().join("target/docread-synth"));
    let out_dir =
        env_path("CL_DOCREAD_REPORT").unwrap_or_else(|| repo().join("target/docread-accuracy"));
    let roster = Roster::load(&set.join("roster.json"))
        .expect("no roster.json: run `cargo xtask gen-invoices --holdout` first");
    let scratch = std::env::temp_dir().join(format!("cl-docread-accuracy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("scratch");
    let mut host = host(&scratch).expect("CL_DOCREAD_WORKER must name the docread-bench worker");

    let started = Instant::now();
    let stress_only = std::env::var("CL_DOCREAD_STRESS_ONLY").as_deref() == Ok("1");
    let docs = if stress_only {
        Vec::new()
    } else {
        read_set(&mut host, &set)
    };
    assert!(
        stress_only || !docs.is_empty(),
        "the synthetic set is empty"
    );
    let (templated, learned) = if wanted_routes().is_none() {
        template_docs(&docs)
    } else {
        (Vec::new(), BTreeMap::new())
    };
    let rosters: BTreeMap<Route, Roster> = learned
        .iter()
        .map(|(route, keyed)| (*route, roster.with_layouts(keyed)))
        .collect();
    let roster_of = |route: Route| rosters.get(&route).unwrap_or(&roster);
    let mut report = Report::default();
    for d in docs.iter().chain(&templated) {
        let group = if d.truth.holdout { "holdout" } else { "seen" };
        report.add(d, group, Some(roster_of(d.route)));
    }
    let private = read_private(&mut host, &mut report);
    let stress = if wanted_routes().is_none() {
        read_stress(&set, &scratch)
    } else {
        Value::Null
    };
    let stats = host.stats();
    let failures = report.gate_failures();
    let timing = report.timing_json();
    let mib = |b: u64| b / (1024 * 1024);
    let summary = json!({
        "documents": docs.len(),
        "template_rereads": templated.len(),
        "private_documents": private,
        "seconds": started.elapsed().as_secs(),
        "tessdata": std::env::var("CL_DOCREAD_TESSDATA").map_or("bundled", |_| "override"),
        "memory_mib": {
            "worker_peak": mib(stats.worker_peak_memory),
            "tesseract_peak": mib(stats.tesseract_peak_memory),
            "measured_as": if cfg!(windows) { "commit (PeakPagefileUsage)" } else { "resident, sampled" },
        },
        "workers_started": stats.workers_started,
        "kills": stats.kills,
        "ocr_pages": stats.ocr_pages,
        "ocr_failures": stats.ocr_failures,
        "timing": timing,
        "stress": stress,
        "gate_failures": failures,
    });
    write_report(&out_dir, &report, &summary);
    if std::env::var("CL_DOCREAD_MISSES").as_deref() == Ok("1") {
        let all: Vec<&Doc> = docs.iter().chain(&templated).collect();
        std::fs::write(out_dir.join("misses-seen.md"), misses(&all, roster_of))
            .expect("write misses");
    }
    println!("{}", report.table());
    println!(
        "{}",
        serde_json::to_string_pretty(&summary).unwrap_or_default()
    );
    let _ = std::fs::remove_dir_all(&scratch);
    if std::env::var("CL_DOCREAD_GATE").as_deref() == Ok("1") {
        assert!(
            failures.is_empty(),
            "Q-285 gates not met:\n{}",
            failures.join("\n")
        );
    }
}

fn found<T>(value: T, spot: Spot) -> Found<T> {
    Found {
        value,
        spot,
        rule: RuleId::new("t"),
        confidence: Confidence::HIGH,
    }
}

#[test]
fn scoring_counts_missing_and_absent_fields_honestly() {
    let t = Truth {
        layout: "x".into(),
        holdout: false,
        seq: 1,
        vendor_key: "a".into(),
        vendor_name: "Acme Co".into(),
        date: CalendarDate::from_ymd(2026, 3, 4).ok(),
        reference: None,
        total: Money::parse_wire("10.10").ok(),
        subtotal: None,
        tax: None,
        due_date: None,
        account_number: None,
        card_last4: None,
    };
    let mut r = Reading::empty(cl_docread::model::Reader::TextPdf, 1);
    let spot = Spot {
        page: 0,
        bbox: Bbox::PAGE,
    };
    // Missing total: wrong. Absent reference with nothing read: right.
    assert!(!right(Field::Total, &r, &t, None));
    assert!(right(Field::Reference, &r, &t, None));
    r.header.total = Some(found(Money::parse_wire("10.1").unwrap(), spot));
    r.header.reference = Some(found("INV-1".to_string(), spot));
    r.header.vendor = Some(found("ACME CO.".to_string(), spot));
    assert!(right(Field::Total, &r, &t, None));
    assert!(!right(Field::Reference, &r, &t, None));
    assert!(right(Field::Vendor, &r, &t, None));
    let mut tally = Tally::default();
    tally.add(Some(&r), &t, None);
    tally.add(None, &t, None);
    assert_eq!(tally.pct(Field::Total), Some((1, 2, 500)));
    assert_eq!(tally.failed, 1);
    let mut v = vec![5, 1, 4, 2, 3, 6, 7, 8, 9, 10];
    assert_eq!(percentile(&mut v, 95), 10);
    assert_eq!(percentile(&mut v, 50), 5);
}
