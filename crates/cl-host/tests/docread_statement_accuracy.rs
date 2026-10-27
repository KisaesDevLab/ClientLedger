//! Statement reading accuracy (A06 step 6, ADD-02): the statement lines
//! metric over the synthetic statements (`cargo xtask gen-invoices
//! --holdout` writes them to `target/docread-synth/statements/`).
//!
//! Runs only with `CL_DOCREAD_ACCURACY=1`; `cargo xtask docread-accuracy`
//! sets it after building the normal worker optimised. Every document is a
//! statement job (`JobHint::statement`) through `DocreadHost`, AI off.
//!
//! A printed line counts as read when a read line has the same date,
//! reference (after `dupes::normalize_reference`; both absent counts) and
//! signed amount; each read line matches one printed line at most. Read
//! lines matching nothing are reported as extra, not gated.
//!
//! Gates (`CL_DOCREAD_GATE=1`, seen and held-out separately): ≥ 95 % of
//! lines on `text` PDFs, ≥ 85 % on `scan` and on `photo`. Header values
//! (opening, closing, period, statement date) are reported only.
//!
//! `CL_DOCREAD_CACHE` / `CL_DOCREAD_REUSE` work as in
//! `docread_accuracy.rs`: kept readings' words go through the current rules
//! in-process (rule tuning without OCR); a gate run never reuses.
//! Reports: `target/docread-accuracy/statements.md` and
//! `statements.json` (never committed).

use cl_core::{CalendarDate, Money};
use cl_docread::dupes::normalize_reference;
use cl_docread::model::{Reading, StatementRead};
use cl_docread::protocol::JobHint;
use cl_docread::{ReadInput, read};
use cl_host::{DocJob, DocreadConfig, DocreadHost, JobOutcome, OcrTools};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn env_path(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).map(PathBuf::from)
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 10, 26).expect("date")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Route {
    Text,
    Scan,
    Photo,
}

impl Route {
    fn name(self) -> &'static str {
        match self {
            Route::Text => "text",
            Route::Scan => "scan",
            Route::Photo => "photo",
        }
    }

    /// The minimum share of lines read, in per cent.
    fn gate(self) -> u64 {
        match self {
            Route::Text => 95,
            Route::Scan | Route::Photo => 85,
        }
    }
}

#[derive(Debug, Clone)]
struct TruthLine {
    date: CalendarDate,
    reference: Option<String>,
    amount: Money,
}

#[derive(Debug, Clone)]
struct Truth {
    layout: String,
    seq: u64,
    holdout: bool,
    lines: Vec<TruthLine>,
    opening: Option<Money>,
    closing: Option<Money>,
    from: Option<CalendarDate>,
    to: Option<CalendarDate>,
    date: Option<CalendarDate>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

fn money(v: &Value, k: &str) -> Option<Money> {
    s(v, k).map(|t| Money::parse_wire(&t).expect("truth amount"))
}

fn day(v: &Value, k: &str) -> Option<CalendarDate> {
    s(v, k).map(|t| CalendarDate::parse_wire(&t).expect("truth date"))
}

impl Truth {
    fn parse(v: &Value) -> Truth {
        Truth {
            layout: s(v, "layout").unwrap_or_default(),
            seq: v["seq"].as_u64().unwrap_or(0),
            holdout: v["holdout"].as_bool().unwrap_or(false),
            lines: v["lines"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|l| TruthLine {
                            date: day(l, "date").expect("line date"),
                            reference: s(l, "reference"),
                            amount: money(l, "amount").expect("line amount"),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            opening: money(v, "opening"),
            closing: money(v, "closing"),
            from: day(v, "period_from"),
            to: day(v, "period_to"),
            date: day(v, "statement_date"),
        }
    }
}

fn same_ref(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => normalize_reference(x) == normalize_reference(y),
        _ => false,
    }
}

/// (matched, extra) for one document.
fn score(t: &Truth, st: Option<&StatementRead>) -> (usize, usize, Vec<usize>) {
    let Some(st) = st else {
        return (0, 0, (0..t.lines.len()).collect());
    };
    let mut used = vec![false; st.lines.len()];
    let mut matched = 0;
    let mut missed = Vec::new();
    for (i, tl) in t.lines.iter().enumerate() {
        let hit = st.lines.iter().enumerate().position(|(k, l)| {
            !used[k]
                && l.date.value == tl.date
                && l.amount.value == tl.amount
                && same_ref(
                    l.reference.as_ref().map(|r| r.value.as_str()),
                    tl.reference.as_deref(),
                )
        });
        if let Some(k) = hit {
            used[k] = true;
            matched += 1;
        } else {
            missed.push(i);
        }
    }
    (matched, used.iter().filter(|u| !**u).count(), missed)
}

/// Why a printed line was not read right: the first read line (not
/// matched to another) that agrees on two of the three values names the
/// third; otherwise the line was not read at all.
fn why_missed(t: &Truth, st: Option<&StatementRead>, missed: &[usize]) -> Vec<&'static str> {
    let Some(st) = st else {
        return vec!["no reading"; missed.len()];
    };
    missed
        .iter()
        .map(|&i| {
            let tl = &t.lines[i];
            let refs = |l: &cl_docread::model::StatementLineRead| {
                same_ref(
                    l.reference.as_ref().map(|r| r.value.as_str()),
                    tl.reference.as_deref(),
                )
            };
            let found =
                |p: &dyn Fn(&cl_docread::model::StatementLineRead) -> bool| st.lines.iter().any(p);
            if found(&|l| {
                l.date.value == tl.date && refs(l) && l.amount.value.abs() == tl.amount.abs()
            }) {
                "sign"
            } else if found(&|l| l.date.value == tl.date && refs(l)) {
                "amount"
            } else if st
                .lines
                .iter()
                .any(|l| l.date.value == tl.date && l.amount.value == tl.amount)
            {
                "reference"
            } else if found(&|l| refs(l) && l.amount.value == tl.amount) {
                "date"
            } else {
                "not read"
            }
        })
        .collect()
}

#[derive(Default)]
struct Tally {
    docs: u64,
    lines: u64,
    matched: u64,
    extra: u64,
    unread: u64,
    header_right: BTreeMap<&'static str, (u64, u64)>,
    why: BTreeMap<&'static str, u64>,
}

impl Tally {
    fn pct(&self) -> u64 {
        (self.matched * 1000)
            .checked_div(self.lines)
            .map_or(0, |p| p / 10)
    }

    fn pct_tenths(&self) -> String {
        let t = (self.matched * 1000).checked_div(self.lines).unwrap_or(0);
        format!("{}.{}", t / 10, t % 10)
    }

    fn header(&mut self, k: &'static str, right: bool) {
        let e = self.header_right.entry(k).or_default();
        e.1 += 1;
        if right {
            e.0 += 1;
        }
    }
}

fn host(scratch: &Path) -> Option<DocreadHost> {
    let worker = env_path("CL_DOCREAD_WORKER")?;
    let ocr = env_path("CL_OCR_PATH").and_then(|d| OcrTools::at(&d));
    if ocr.is_none() {
        assert!(
            std::env::var("CL_OCR_REQUIRE").as_deref() != Ok("1"),
            "tesseract is required (CL_OCR_REQUIRE=1) but CL_OCR_PATH has none"
        );
    }
    let paths = cl_host::AppPaths::at(scratch.to_path_buf()).expect("app paths");
    Some(DocreadHost::new(DocreadConfig::new(&paths, worker, ocr)))
}

fn read_one(host: &mut DocreadHost, bytes: Vec<u8>) -> (Option<Reading>, Duration) {
    let job = DocJob {
        bytes: bytes.into(),
        hint: JobHint::statement(),
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

/// A kept reading's words through the current rules (tuning aid).
fn reread(r: &Reading) -> Reading {
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
        templates: &[],
        hint: JobHint::statement(),
        today: today(),
    })
}

fn cached(part: &str, dir: &Path, file: &str) -> Option<PathBuf> {
    let root = env_path("CL_DOCREAD_CACHE")?;
    Some(
        root.join("statements")
            .join(part)
            .join(dir.file_name()?)
            .join(format!("{file}.json")),
    )
}

fn gate_mode() -> bool {
    let gate = std::env::var("CL_DOCREAD_GATE").as_deref() == Ok("1");
    if gate {
        for var in ["CL_DOCREAD_ROUTES", "CL_DOCREAD_PARTS", "CL_DOCREAD_REUSE"] {
            assert!(
                std::env::var_os(var).is_none_or(|v| v.is_empty()),
                "{var} narrows or replays the set: not allowed with CL_DOCREAD_GATE=1"
            );
        }
    }
    gate
}

#[test]
#[allow(clippy::too_many_lines)]
fn statement_accuracy() {
    if std::env::var("CL_DOCREAD_ACCURACY").as_deref() != Ok("1") {
        eprintln!("skipped: set CL_DOCREAD_ACCURACY=1 (cargo xtask docread-accuracy)");
        return;
    }
    let gate = gate_mode();
    let set = env_path("CL_DOCREAD_SET")
        .unwrap_or_else(|| repo().join("target/docread-synth"))
        .join("statements");
    let out_dir =
        env_path("CL_DOCREAD_REPORT").unwrap_or_else(|| repo().join("target/docread-accuracy"));
    let scratch = std::env::temp_dir().join(format!("cl-docread-stmt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).expect("scratch");
    let mut host = host(&scratch).expect("CL_DOCREAD_WORKER must name the docread-bench worker");
    let routes: Option<Vec<String>> = std::env::var("CL_DOCREAD_ROUTES")
        .ok()
        .map(|v| v.split(',').map(str::to_string).collect());
    let parts = std::env::var("CL_DOCREAD_PARTS").unwrap_or_default();
    let reuse = std::env::var("CL_DOCREAD_REUSE").as_deref() == Ok("1");
    let started = Instant::now();
    let mut tallies: BTreeMap<(&'static str, Route), Tally> = BTreeMap::new();
    let mut misses = String::new();
    let mut max_ms_per_page = 0u128;
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
            for seq in 1..=20u32 {
                let stem = format!("{seq:02}");
                let Ok(raw) = std::fs::read(dir.join(format!("{stem}.json"))) else {
                    continue;
                };
                let v: Value = serde_json::from_slice(&raw).expect("truth json");
                let truth = Truth::parse(&v);
                let pages = v["pages"].as_u64().unwrap_or(1).max(1);
                for (file, route) in [
                    (format!("{stem}.pdf"), Route::Text),
                    (format!("{stem}.scan.jpg"), Route::Scan),
                    (format!("{stem}.scan.pdf"), Route::Scan),
                    (format!("{stem}.photo.jpg"), Route::Photo),
                ] {
                    if routes
                        .as_ref()
                        .is_some_and(|w| !w.iter().any(|x| x == route.name()))
                    {
                        continue;
                    }
                    let Ok(bytes) = std::fs::read(dir.join(&file)) else {
                        continue;
                    };
                    let keep = cached(part, &dir, &file);
                    let kept = keep
                        .as_ref()
                        .filter(|_| reuse)
                        .and_then(|p| std::fs::read(p).ok())
                        .and_then(|b| serde_json::from_slice::<Reading>(&b).ok());
                    let reading = if let Some(r) = kept {
                        Some(reread(&r))
                    } else {
                        let (r, elapsed) = read_one(&mut host, bytes);
                        max_ms_per_page =
                            max_ms_per_page.max(elapsed.as_millis() / u128::from(pages));
                        if let (Some(p), Some(r)) = (&keep, &r) {
                            if let Some(parent) = p.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            let _ = std::fs::write(p, serde_json::to_vec(r).unwrap_or_default());
                        }
                        r
                    };
                    let st = reading.as_ref().and_then(|r| r.statement.as_ref());
                    let (matched, extra, missed_lines) = score(&truth, st);
                    let t = tallies.entry((part, route)).or_default();
                    t.docs += 1;
                    t.lines += truth.lines.len() as u64;
                    t.matched += matched as u64;
                    t.extra += extra as u64;
                    for w in why_missed(&truth, st, &missed_lines) {
                        *t.why.entry(w).or_default() += 1;
                    }
                    if st.is_none() {
                        t.unread += 1;
                    }
                    t.header(
                        "closing",
                        st.and_then(|s| s.closing.as_ref().map(|c| c.value)) == truth.closing,
                    );
                    if truth.opening.is_some() {
                        t.header(
                            "opening",
                            st.and_then(|s| s.opening.as_ref().map(|c| c.value)) == truth.opening,
                        );
                    }
                    if truth.from.is_some() {
                        t.header(
                            "period",
                            st.and_then(|s| s.period_from.as_ref().map(|c| c.value)) == truth.from
                                && st.and_then(|s| s.period_to.as_ref().map(|c| c.value))
                                    == truth.to,
                        );
                    }
                    t.header(
                        "statement date",
                        st.and_then(|s| s.statement_date.as_ref().map(|c| c.value)) == truth.date,
                    );
                    if part == "seen" && (!missed_lines.is_empty() || extra > 0) {
                        let _ = writeln!(
                            misses,
                            "- {} {:02} {}: {} of {} lines missed {:?}, {} extra",
                            truth.layout,
                            truth.seq,
                            route.name(),
                            missed_lines.len(),
                            truth.lines.len(),
                            missed_lines,
                            extra
                        );
                    }
                    let _ = truth.holdout;
                }
            }
        }
    }
    assert!(!tallies.is_empty(), "the statement set is empty");
    let mut table = String::from(
        "| part | path | statements | lines | read right | % | extra | unread | closing | opening | period | statement date |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    let mut failures = Vec::new();
    let mut json_rows = Vec::new();
    for ((part, route), t) in &tallies {
        let h = |k: &str| {
            t.header_right
                .get(k)
                .map_or_else(|| "-".to_string(), |(r, n)| format!("{r}/{n}"))
        };
        let _ = writeln!(
            table,
            "| {part} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            route.name(),
            t.docs,
            t.lines,
            t.matched,
            t.pct_tenths(),
            t.extra,
            t.unread,
            h("closing"),
            h("opening"),
            h("period"),
            h("statement date"),
            t.why
                .iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        json_rows.push(json!({
            "part": part, "route": route.name(), "statements": t.docs, "lines": t.lines,
            "matched": t.matched, "extra": t.extra, "unread": t.unread,
        }));
        if t.pct() < route.gate() {
            failures.push(format!(
                "{part} {}: {} % of statement lines read, needs {} %",
                route.name(),
                t.pct_tenths(),
                route.gate()
            ));
        }
    }
    if gate {
        for part in ["seen", "holdout"] {
            for route in [Route::Text, Route::Scan, Route::Photo] {
                if !tallies.contains_key(&(part, route)) {
                    failures.push(format!("{part} {}: no statements read", route.name()));
                }
            }
        }
    }
    let summary = json!({
        "seconds": started.elapsed().as_secs(),
        "max_ms_per_page": u64::try_from(max_ms_per_page).unwrap_or(u64::MAX),
        "groups": json_rows,
        "gate_failures": failures,
    });
    let md = format!(
        "# Statement reading accuracy (A06)\n\nA line is right when its date, reference and \
         signed amount are all right. Gates: text ≥ 95 %, scan and photo ≥ 85 %, seen and \
         held-out separately.\n\n{table}\n## Seen misses\n\n{misses}\n"
    );
    std::fs::create_dir_all(&out_dir).expect("report folder");
    std::fs::write(out_dir.join("statements.md"), &md).expect("write report");
    std::fs::write(
        out_dir.join("statements.json"),
        serde_json::to_vec_pretty(&summary).unwrap_or_default(),
    )
    .expect("write summary");
    println!("{table}");
    let _ = std::fs::remove_dir_all(&scratch);
    if gate {
        assert!(
            failures.is_empty(),
            "statement gates not met:\n{}",
            failures.join("\n")
        );
    }
}

fn f<T>(value: T) -> cl_docread::model::Found<T> {
    cl_docread::model::Found {
        value,
        spot: cl_docread::model::Spot {
            page: 0,
            bbox: cl_docread::model::Bbox::PAGE,
        },
        rule: cl_docread::model::RuleId::new("t"),
        confidence: cl_docread::model::Confidence::HIGH,
    }
}

#[test]
fn a_line_needs_date_reference_and_amount() {
    let d = |n| CalendarDate::from_ymd(2026, 9, n).expect("date");
    let m = |t: &str| Money::parse_wire(t).expect("money");
    let truth = Truth {
        layout: "t".into(),
        seq: 1,
        holdout: false,
        lines: vec![
            TruthLine {
                date: d(3),
                reference: Some("INV-00123".into()),
                amount: m("10"),
            },
            TruthLine {
                date: d(4),
                reference: None,
                amount: m("-5"),
            },
        ],
        opening: None,
        closing: None,
        from: None,
        to: None,
        date: None,
    };
    let line = |date, r: Option<&str>, amount| cl_docread::model::StatementLineRead {
        date: f(date),
        reference: r.map(|r| f(r.to_string())),
        description: None,
        amount: f(amount),
        sign_from: cl_docread::model::SignSource::SingleColumn,
        running: None,
    };
    let st = StatementRead {
        lines: vec![
            line(d(3), Some("INV-123"), m("10")),
            line(d(4), None, m("5")),
            line(d(9), None, m("1")),
        ],
        ..StatementRead::default()
    };
    let (matched, extra, missed) = score(&truth, Some(&st));
    assert_eq!((matched, extra, missed), (1, 2, vec![1]), "sign matters");
    assert_eq!(score(&truth, None).0, 0);
}
