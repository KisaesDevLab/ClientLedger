//! Fuzz smoke mode for CI (P25 plan §6, review S12): the fixture corpus,
//! 2,000 proptest cases per parser (random bytes and token soups), and the
//! worst-case shapes, each within 5× the time of a well-formed input of the
//! same size. The real fuzz targets live in `crates/cl-import/fuzz/`
//! (cargo-fuzz, outside the workspace).

use cl_core::CalendarDate;
use cl_import::fuzz::{fuzz_any, fuzz_csv, fuzz_ofx, fuzz_qif};
use cl_import::{ImportError, ParseOptions, parse};
use proptest::prelude::*;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn fixtures() -> Vec<Vec<u8>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/bank");
    let mut out = Vec::new();
    for sub in [dir.clone(), dir.join("malformed")] {
        for entry in std::fs::read_dir(sub).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                out.push(std::fs::read(path).unwrap());
            }
        }
    }
    out
}

#[test]
fn corpus_runs_through_every_entry_point() {
    let corpus = fixtures();
    assert!(corpus.len() >= 30);
    for bytes in &corpus {
        fuzz_ofx(bytes);
        fuzz_qif(bytes);
        fuzz_csv(bytes);
        fuzz_any(bytes);
        // Every prefix of the first 2 KB: truncation at any byte.
        for cut in (0..bytes.len().min(2048)).step_by(7) {
            fuzz_any(&bytes[..cut]);
        }
    }
}

const OFX_TOKENS: [&str; 24] = [
    "<OFX>",
    "</OFX>",
    "<STMTRS>",
    "</STMTRS>",
    "<CCSTMTRS>",
    "<BANKTRANLIST>",
    "</BANKTRANLIST>",
    "<STMTTRN>",
    "</STMTTRN>",
    "<TRNAMT>",
    "-12.345",
    "<DTPOSTED>",
    "20261103[-8:PST]",
    "<FITID>",
    "&amp;",
    "&#x1F600;",
    "&bogus;",
    "&",
    "<!--",
    "-->",
    "<![CDATA[",
    "]]>",
    "\r\n",
    "<?xml version=\"1.0\"?>",
];
const QIF_TOKENS: [&str; 12] = [
    "!Type:Bank\n",
    "!Type:CCard\n",
    "!Account\n",
    "D1/ 5'26\n",
    "D13/45/99\n",
    "T-1,234.56\n",
    "U(5.00)\n",
    "PCafé\n",
    "N1234567890\n",
    "^\n",
    "M\u{0}\n",
    "!Type:Cat\n",
];
const CSV_TOKENS: [&str; 12] = [
    "Date",
    "Amount",
    "Debit",
    "Credit",
    "CR/DR",
    "\"",
    ",",
    ";",
    "\t",
    "\r\n",
    "11/03/2026",
    "(45.00)",
];

fn soup(tokens: &'static [&'static str]) -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(prop::sample::select(tokens), 0..200)
        .prop_map(|t| t.concat().into_bytes())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2_000, ..ProptestConfig::default() })]

    #[test]
    fn ofx_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..512), tokens in soup(&OFX_TOKENS)) {
        fuzz_ofx(&bytes);
        fuzz_ofx(&tokens);
        let mut prefixed = b"OFXHEADER:100\nCHARSET:1252\n".to_vec();
        prefixed.extend_from_slice(&tokens);
        fuzz_any(&prefixed);
    }

    #[test]
    fn qif_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..512), tokens in soup(&QIF_TOKENS)) {
        fuzz_qif(&bytes);
        fuzz_qif(&tokens);
        fuzz_any(&tokens);
    }

    #[test]
    fn csv_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..512), tokens in soup(&CSV_TOKENS)) {
        fuzz_csv(&bytes);
        fuzz_csv(&tokens);
        fuzz_any(&tokens);
    }
}

// ---------------------------------------------------------------------------
// Worst-case shapes

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 7).unwrap()
}

/// The fastest of three runs (less noise on shared CI machines).
fn time(input: &[u8]) -> (Duration, Result<usize, ImportError>) {
    let mut best = Duration::MAX;
    let mut out = Err(ImportError::Empty);
    for _ in 0..3 {
        let start = Instant::now();
        out = parse(input, &ParseOptions::new(today())).map(|f| f.sections.len());
        best = best.min(start.elapsed());
    }
    (best, out)
}

/// A well-formed OFX file of about `size` bytes (the baseline).
fn benign_ofx(size: usize) -> Vec<u8> {
    let mut s = String::from(
        "OFXHEADER:100\nCHARSET:1252\n\n<OFX><BANKMSGSRSV1><STMTTRNRS><STMTRS><BANKACCTFROM><BANKID>1<ACCTID>12345678</BANKACCTFROM><BANKTRANLIST>\n",
    );
    let mut i = 0u32;
    while s.len() < size && i < 19_000 {
        let _ = writeln!(
            s,
            "<STMTTRN><TRNTYPE>DEBIT<DTPOSTED>20261103<TRNAMT>-{}.25<FITID>F{i}<NAME>SHOP NUMBER {i} MAIN STREET</STMTTRN>",
            i % 997 + 1
        );
        i += 1;
    }
    while s.len() < size {
        s.push_str("<!-- padding to the measured size ---------------------------------------->\n");
    }
    s.push_str("</BANKTRANLIST></STMTRS></STMTTRNRS></BANKMSGSRSV1></OFX>\n");
    s.into_bytes()
}

/// A well-formed CSV file of about `size` bytes.
fn benign_csv(size: usize) -> Vec<u8> {
    let mut s = String::from("Date,Description,Amount\n");
    let line = format!("11/03/2026,{},-12.50\n", "D".repeat(200));
    while s.len() + line.len() < size {
        s.push_str(&line);
    }
    s.into_bytes()
}

fn assert_within(name: &str, worst: &[u8], baseline: &[u8]) -> Result<usize, ImportError> {
    let (base, _) = time(baseline);
    let (t, result) = time(worst);
    // 5× the baseline, plus a small floor so tiny inputs are not flaky.
    let limit = base * 5 + Duration::from_millis(50);
    assert!(
        t <= limit,
        "{name}: took {} ms, limit {} ms",
        t.as_millis(),
        limit.as_millis()
    );
    result
}

#[test]
fn unclosed_stmttrn_megabyte() {
    let mut s =
        String::from("OFXHEADER:100\n\n<OFX><BANKMSGSRSV1><STMTTRNRS><STMTRS><BANKTRANLIST>\n");
    while s.len() < 1 << 20 {
        s.push_str("<STMTTRN><TRNAMT>-1.00<NAME>X\n");
    }
    let r = assert_within("unclosed STMTTRN", s.as_bytes(), &benign_ofx(1 << 20));
    assert!(matches!(r, Err(ImportError::TooDeep { .. })), "{r:?}");
    // The XML form: every STMTTRN open, never closed.
    let x = format!(
        "<?xml version=\"1.0\"?><?OFX OFXHEADER=\"200\"?><OFX>{}",
        "<STMTTRN><TRNAMT>1</TRNAMT>".repeat(40_000)
    );
    let r = assert_within("unclosed STMTTRN xml", x.as_bytes(), &benign_ofx(x.len()));
    assert!(matches!(r, Err(ImportError::TooDeep { .. })), "{r:?}");
}

#[test]
fn hundred_thousand_nested_tags() {
    let s = format!("OFXHEADER:100\n\n{}", "<A>".repeat(100_000));
    let r = assert_within("nested", s.as_bytes(), &benign_ofx(s.len()));
    assert_eq!(r, Err(ImportError::TooDeep { line: 3 }));
}

#[test]
fn million_digit_trnamt() {
    let s = format!(
        "OFXHEADER:100\n\n<OFX><STMTRS><BANKTRANLIST><STMTTRN><TRNAMT>{}</STMTTRN></BANKTRANLIST></STMTRS></OFX>",
        "9".repeat(1_000_000)
    );
    let r = assert_within("million digits", s.as_bytes(), &benign_ofx(s.len()));
    assert!(
        matches!(r, Err(ImportError::TokenTooLong { line: 3 })),
        "{r:?}"
    );
}

#[test]
fn huge_csv_field() {
    let size = cl_import::MAX_FILE_BYTES - 1024;
    let mut s = String::from("Date,Description,Amount\n11/03/2026,\"");
    s.push_str(&"x".repeat(size - s.len() - 10));
    s.push_str("\",-1.00\n");
    let r = assert_within("25 MB field", s.as_bytes(), &benign_csv(size));
    assert!(matches!(r, Err(ImportError::Csv(_))), "{r:?}");
}

#[test]
fn billion_laughs_and_entity_floods() {
    let laughs = b"<?xml version=\"1.0\"?>\n<!DOCTYPE lolz [\n<!ENTITY lol \"lol\">\n<!ENTITY lol2 \"&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;\">\n<!ENTITY lol3 \"&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;\">\n]>\n<?OFX OFXHEADER=\"200\"?><OFX>&lol3;</OFX>";
    let r = assert_within("billion laughs", laughs, &benign_ofx(laughs.len()));
    assert_eq!(r, Err(ImportError::Doctype { line: 2 }));
    // Many entities in many short values: linear.
    let mut s = String::from("OFXHEADER:100\n\n<OFX><STMTRS><BANKTRANLIST>");
    while s.len() < 1 << 20 {
        s.push_str("<STMTTRN><NAME>&amp;&lt;&#65;&bogus;AT&T</STMTTRN>");
    }
    s.push_str("</BANKTRANLIST></STMTRS></OFX>");
    assert_within("entity flood", s.as_bytes(), &benign_ofx(1 << 20)).ok();
}

#[test]
fn qif_and_csv_floods() {
    let q = "!Type:Bank\n".to_owned() + &"D1/5/26\nT-1.00\n^\n".repeat(60_000);
    let benign = benign_csv(q.len());
    assert_within("qif flood", q.as_bytes(), &benign).ok();
    let quotes = "\"".repeat(1 << 20);
    assert_within("quote flood", quotes.as_bytes(), &benign_csv(1 << 20)).ok();
    let seps = ",".repeat(1 << 20);
    assert_within("separator flood", seps.as_bytes(), &benign_csv(1 << 20)).ok();
}
