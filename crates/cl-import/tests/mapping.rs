//! CSV mappings and profiles as WP-B uses them (Q-P25w), plus decoding
//! remarks on OFX files.

use cl_core::CalendarDate;
use cl_import::csv::mapping::{guess, read_table};
use cl_import::csv::profile::{CsvProfile, find, fingerprint};
use cl_import::{
    AmountColumns, CsvMapping, DateOrder, EncodingNote, ParseOptions, ProblemKind, parse,
};
use std::path::PathBuf;

fn bytes(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/bank")
            .join(name),
    )
    .unwrap()
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 7).unwrap()
}

#[test]
fn mapping_json_round_trips_with_a_stable_shape() {
    let table = read_table(&bytes("debit_credit_balance.csv")).unwrap();
    let m = guess(&table).unwrap();
    assert_eq!(
        m.amount,
        AmountColumns::DebitCredit {
            debit: 2,
            credit: 3
        }
    );
    assert_eq!(m.balance, Some(4));
    let json = serde_json::to_value(&m).unwrap();
    assert_eq!(json["amount"]["layout"], "debitCredit");
    assert_eq!(json["dateOrder"], "mdy");
    assert_eq!(json["hasHeader"], true);
    let back: CsvMapping = serde_json::from_value(json).unwrap();
    assert_eq!(back, m);
    assert!(m.fits(&table));
    let other = read_table(&bytes("drcr_indicator.csv")).unwrap();
    assert!(!m.fits(&other));
}

#[test]
fn indicator_and_profiles() {
    let table = read_table(&bytes("drcr_indicator.csv")).unwrap();
    let m = guess(&table).unwrap();
    assert_eq!(
        m.amount,
        AmountColumns::Indicator {
            amount: 2,
            indicator: 3
        }
    );
    let profiles = vec![
        CsvProfile {
            fingerprint: "cols:9".to_owned(),
            mapping: m.clone(),
        },
        CsvProfile {
            fingerprint: fingerprint(&table),
            mapping: m.clone(),
        },
    ];
    assert_eq!(
        find(&profiles, &table).map(|p| &p.fingerprint),
        Some(&profiles[1].fingerprint)
    );
    let other = read_table(&bytes("signed_amount_bom.csv")).unwrap();
    assert!(find(&profiles, &other).is_none());
    assert_ne!(fingerprint(&table), fingerprint(&other));
}

#[test]
fn day_first_mapping_reads_the_flagged_file() {
    let raw = bytes("dates_ddmm_impossible.csv");
    let mut opts = ParseOptions::new(today());
    let first = parse(&raw, &opts).unwrap();
    assert_eq!(
        first.sections[0].dates.and_then(|d| d.day_first_line),
        Some(3)
    );
    // The owner switches to day/month: every row reads except 02/30, which
    // is no date either way (as day/month it is the 2nd of month 30).
    let mut m = first.csv_mapping.clone().unwrap();
    m.date_order = DateOrder::Dmy;
    opts.csv_mapping = Some(m);
    let again = parse(&raw, &opts).unwrap();
    let s = &again.sections[0];
    assert_eq!(s.rows.len(), 4);
    assert_eq!(s.rows[1].date, CalendarDate::from_ymd(2026, 2, 13).unwrap());
    assert_eq!(s.problems.len(), 1);
    assert_eq!(s.problems[0].line, 5);
    assert_eq!(s.problems[0].kind, ProblemKind::ImpossibleDate);
    assert_eq!(s.dates.and_then(|d| d.day_first_line), None);
}

#[test]
fn ofx_declared_1252_but_utf8_reads_as_utf8() {
    let body = "OFXHEADER:100\r\nDATA:OFXSGML\r\nENCODING:USASCII\r\nCHARSET:1252\r\n\r\n<OFX><BANKMSGSRSV1><STMTTRNRS><STMTRS><BANKACCTFROM><BANKID>1<ACCTID>99887766<ACCTTYPE>CHECKING</BANKACCTFROM><BANKTRANLIST><STMTTRN><TRNTYPE>POS<DTPOSTED>20261103<TRNAMT>-4.50<FITID>A<NAME>CAF\u{c9} \u{2014} CR\u{c8}ME</STMTTRN></BANKTRANLIST></STMTRS></STMTTRNRS></BANKMSGSRSV1></OFX>\r\n";
    let f = parse(body.as_bytes(), &ParseOptions::new(today())).unwrap();
    assert_eq!(
        f.encoding_note,
        Some(EncodingNote::DeclaredSingleByteReadAsUtf8)
    );
    assert_eq!(
        f.sections[0].rows[0].description,
        "CAF\u{c9} \u{2014} CR\u{c8}ME"
    );
}

#[test]
fn qif_date_order_override() {
    let q = b"!Type:Bank\nD13/02/2026\nT-1.00\nPA\n^\nD03/04/2026\nT-2.00\nPB\n^\n";
    let us = cl_import::qif::parse(q, today(), None).unwrap();
    assert_eq!(us.sections[0].dates.and_then(|d| d.day_first_line), Some(2));
    assert_eq!(us.sections[0].rows.len(), 1);
    let dm = cl_import::qif::parse(q, today(), Some(DateOrder::Dmy)).unwrap();
    assert_eq!(dm.sections[0].rows.len(), 2);
    assert_eq!(
        dm.sections[0].rows[1].date,
        CalendarDate::from_ymd(2026, 4, 3).unwrap()
    );
}
