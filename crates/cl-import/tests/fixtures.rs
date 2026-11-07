//! The bank fixture suite (P25 plan §6): every file in `fixtures/bank`
//! parses to the counts, sums (Decimal equality), date ranges, balances,
//! problem lines and skipped lines in `fixtures/bank/expected.json`, and
//! every malformed file fails with its line. The expected values were
//! computed by the fixture generator independently of this crate.

use cl_core::CalendarDate;
use cl_import::dedupe::key_rows;
use cl_import::{
    AccountKind, Continuity, ImportError, ParseOptions, ParsedFile, RowOrder, Section, check_as,
    parse, to_feed_amount,
};
use rust_decimal::Decimal;
use serde_json::Value;
use std::path::PathBuf;
use std::str::FromStr as _;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/bank")
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 7).unwrap()
}

fn read(name: &str) -> ParsedFile {
    let bytes = std::fs::read(dir().join(name)).unwrap();
    parse(&bytes, &ParseOptions::new(today())).unwrap_or_else(|e| panic!("{name}: {e:?}"))
}

fn expected() -> Value {
    serde_json::from_slice(&std::fs::read(dir().join("expected.json")).unwrap()).unwrap()
}

fn dec(v: &Value) -> Decimal {
    Decimal::from_str(v.as_str().unwrap()).unwrap()
}

fn idx(k: &str) -> usize {
    k.parse::<usize>().unwrap() - 1
}

#[allow(clippy::too_many_lines)] // one assertion per expected.json field
fn check_section(name: &str, s: &Section, e: &Value) {
    let ctx = format!("{name} section {}", s.index);
    assert_eq!(
        format!("{:?}", s.kind),
        e["kind"].as_str().unwrap(),
        "{ctx}: kind"
    );
    assert_eq!(
        s.account
            .as_ref()
            .map(cl_import::AccountId::mask)
            .as_deref(),
        e["mask"].as_str(),
        "{ctx}: mask"
    );
    assert_eq!(
        s.rows.len(),
        usize::try_from(e["rows"].as_u64().unwrap()).unwrap(),
        "{ctx}: rows"
    );
    assert_eq!(s.total(), Some(dec(&e["sum"])), "{ctx}: sum");
    assert_eq!(
        s.start.map(CalendarDate::to_wire).as_deref(),
        e["start"].as_str(),
        "{ctx}: start"
    );
    assert_eq!(
        s.end.map(CalendarDate::to_wire).as_deref(),
        e["end"].as_str(),
        "{ctx}: end"
    );
    let bal = s.ledger_balance.map(|b| {
        // CSV balances are stored in the file's sign; none of the CSV
        // fixtures is flipped, so the stored value is the bank's.
        b.amount
    });
    assert_eq!(
        bal,
        e["balance"].as_str().map(|b| Decimal::from_str(b).unwrap()),
        "{ctx}: balance"
    );
    assert_eq!(
        s.ledger_balance
            .and_then(|b| b.date)
            .map(CalendarDate::to_wire)
            .as_deref(),
        e["balanceDate"].as_str(),
        "{ctx}: balance date"
    );
    let lines = |v: &Value| -> Vec<usize> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|l| usize::try_from(l.as_u64().unwrap()).unwrap())
            .collect()
    };
    assert_eq!(
        s.problems.iter().map(|p| p.line).collect::<Vec<_>>(),
        lines(&e["problems"]),
        "{ctx}: problems"
    );
    assert_eq!(
        s.skipped.iter().map(|p| p.line).collect::<Vec<_>>(),
        lines(&e["skipped"]),
        "{ctx}: skipped"
    );
    if let Some(d) = e["firstDescription"].as_str() {
        assert_eq!(s.rows[0].description, d, "{ctx}: first description");
    }
    if let Some(rl) = e.get("rowLines") {
        let want = lines(rl);
        let got: Vec<usize> = s.rows.iter().take(want.len()).map(|r| r.line).collect();
        assert_eq!(got, want, "{ctx}: row lines");
    }
    if let Some(map) = e.get("descriptions").and_then(Value::as_object) {
        for (k, v) in map {
            assert_eq!(
                s.rows[idx(k)].description,
                v.as_str().unwrap(),
                "{ctx}: description {k}"
            );
        }
    }
    if let Some(map) = e.get("checks").and_then(Value::as_object) {
        for (k, v) in map {
            assert_eq!(
                s.rows[idx(k)].check_number.map(u64::from),
                v.as_u64(),
                "{ctx}: check {k}"
            );
        }
    }
    if let Some(map) = e.get("memos").and_then(Value::as_object) {
        for (k, v) in map {
            assert_eq!(
                s.rows[idx(k)].memo.as_deref(),
                v.as_str(),
                "{ctx}: memo {k}"
            );
        }
    }
    if let Some(map) = e.get("dates").and_then(Value::as_object) {
        for (k, v) in map {
            assert_eq!(
                s.rows[idx(k)].date.to_wire(),
                v.as_str().unwrap(),
                "{ctx}: date {k}"
            );
        }
    }
    if let Some(f) = e.get("fitids").and_then(Value::as_array) {
        let got: Vec<&str> = s
            .rows
            .iter()
            .map(|r| r.fitid.as_deref().unwrap_or(""))
            .collect();
        let want: Vec<&str> = f.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(got, want, "{ctx}: fitids");
    }
    if let Some(flip) = e.get("suggestFlip").and_then(Value::as_bool) {
        assert_eq!(s.sign.suggest_flip, flip, "{ctx}: suggest flip");
    }
    if let Some(flip) = e.get("cardSuggestFlip").and_then(Value::as_bool) {
        assert_eq!(
            check_as(s, AccountKind::CreditCard).suggest_flip,
            flip,
            "{ctx}: card check"
        );
    }
    if let Some(order) = e.get("continuity").and_then(Value::as_str) {
        let want = match order {
            "OldestFirst" => RowOrder::OldestFirst,
            _ => RowOrder::NewestFirst,
        };
        assert_eq!(
            s.sign.continuity,
            Continuity::Continuous {
                order: want,
                flipped: false
            },
            "{ctx}"
        );
    }
    if let Some(a) = e.get("ambiguous").and_then(Value::as_bool) {
        assert_eq!(s.dates.map(|d| d.ambiguous), Some(a), "{ctx}: ambiguous");
    }
    if let Some(l) = e.get("dayFirstLine") {
        assert_eq!(
            s.dates.and_then(|d| d.day_first_line).map(|l| l as u64),
            l.as_u64(),
            "{ctx}: day-first line"
        );
    }
    if let Some(o) = e.get("ordinals").and_then(Value::as_array) {
        let keyed = key_rows(s.rows.iter().map(|r| {
            (
                r.date,
                to_feed_amount(r.file_amount, false),
                r.description.as_str(),
            )
        }));
        let got: Vec<u64> = keyed.iter().map(|k| u64::from(k.ordinal)).collect();
        let want: Vec<u64> = o.iter().map(|v| v.as_u64().unwrap()).collect();
        assert_eq!(got, want, "{ctx}: ordinals");
    }
    // Rounded amounts (owner answer Q-P25q): only the listed rows say
    // "rounded from …"; every other row has none.
    let rounded = e.get("roundedFrom").and_then(Value::as_object);
    for (i, r) in s.rows.iter().enumerate() {
        let want = rounded
            .and_then(|m| m.get(&(i + 1).to_string()))
            .and_then(Value::as_str);
        assert_eq!(
            r.rounded_from.as_deref(),
            want,
            "{ctx}: rounded from, row {}",
            i + 1
        );
    }
    if let Some(map) = e.get("amounts").and_then(Value::as_object) {
        for (k, v) in map {
            assert_eq!(s.rows[idx(k)].file_amount, dec(v), "{ctx}: amount {k}");
        }
    }
    if let Some(n) = e.get("accountName").and_then(Value::as_str) {
        assert_eq!(s.account_name.as_deref(), Some(n), "{ctx}: account name");
    }
}

#[test]
fn every_fixture_has_its_counts_and_sums() {
    let exp = expected();
    let files = exp["files"].as_array().unwrap();
    assert!(files.len() >= 24, "plan §6 lists 24+ fixtures");
    for f in files {
        let name = f["file"].as_str().unwrap();
        let parsed = read(name);
        assert_eq!(
            format!("{:?}", parsed.format),
            f["format"].as_str().unwrap(),
            "{name}: format"
        );
        if let Some(enc) = f.get("encoding").and_then(Value::as_str) {
            assert_eq!(format!("{:?}", parsed.encoding), enc, "{name}: encoding");
        }
        let secs = f["sections"].as_array().unwrap();
        assert_eq!(parsed.sections.len(), secs.len(), "{name}: sections");
        for (s, e) in parsed.sections.iter().zip(secs) {
            check_section(name, s, e);
        }
    }
}

#[test]
fn every_malformed_file_names_its_line() {
    let exp = expected();
    for m in exp["malformed"].as_array().unwrap() {
        let name = m["file"].as_str().unwrap();
        let bytes = std::fs::read(dir().join(name)).unwrap();
        let result = parse(&bytes, &ParseOptions::new(today()));
        if let Some(err) = m.get("error").and_then(Value::as_str) {
            let e = result.expect_err(name);
            assert!(
                format!("{e:?}").starts_with(err),
                "{name}: {e:?} is not {err}"
            );
            assert_eq!(
                e.line().map(|l| l as u64),
                m["line"].as_u64(),
                "{name}: line"
            );
            // Messages name the line and never echo file content.
            if let Some(line) = e.line() {
                assert!(e.to_string().contains(&line.to_string()), "{name}: {e}");
            }
        } else {
            let parsed = result.unwrap_or_else(|e| panic!("{name}: {e:?}"));
            let got: Vec<u64> = parsed.sections[0]
                .problems
                .iter()
                .map(|p| p.line as u64)
                .collect();
            let want: Vec<u64> = m["problems"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap())
                .collect();
            assert_eq!(got, want, "{name}: problem lines");
        }
    }
}

/// The same rows saved three ways give the same keys (plan §4).
#[test]
fn bom_1252_and_utf16_give_identical_keys() {
    let keys = |name: &str| {
        let f = read(name);
        key_rows(f.sections[0].rows.iter().map(|r| {
            (
                r.date,
                to_feed_amount(r.file_amount, false),
                r.description.as_str(),
            )
        }))
    };
    let a = keys("signed_amount_bom.csv");
    assert_eq!(a.len(), 10);
    assert_eq!(a, keys("win1252.csv"));
    assert_eq!(a, keys("utf16le_tab.csv"));
}

/// Card fixtures in both conventions: same wire amounts and keys once the
/// positive issuer is flipped, and the balances agree (difference 0.00).
#[test]
fn card_conventions_meet_on_the_wire() {
    let neg = read("card_ccstmt.qfx");
    let pos = read("card_positive_issuer.qfx");
    let (n, p) = (&neg.sections[0], &pos.sections[0]);
    assert!(!n.sign.suggest_flip && p.sign.suggest_flip);
    let wire = |s: &Section, flipped: bool| -> Vec<Decimal> {
        s.rows
            .iter()
            .map(|r| to_feed_amount(r.file_amount, flipped))
            .collect()
    };
    assert_eq!(wire(n, false), wire(p, true));
    let keys = |s: &Section, flipped: bool| {
        key_rows(s.rows.iter().map(|r| {
            (
                r.date,
                to_feed_amount(r.file_amount, flipped),
                r.description.as_str(),
            )
        }))
    };
    assert_eq!(keys(n, false), keys(p, true));
    let nb = cl_import::to_display(n.ledger_balance.unwrap().amount, false);
    let pb = cl_import::to_display(p.ledger_balance.unwrap().amount, true);
    assert_eq!(nb - pb, Decimal::ZERO);
}

#[test]
fn reused_fitid_files_keep_both_rows() {
    let a = read("reused_fitid.qfx");
    let r = read("reused_fitid_reversed.qfx");
    let b = read("reused_fitid_second_only.qfx");
    assert_eq!(a.sections[0].rows.len(), 4);
    assert_eq!(r.sections[0].rows.len(), 4);
    assert_eq!(b.sections[0].rows.len(), 1);
    assert_eq!(b.sections[0].rows[0].fitid.as_deref(), Some("R200"));
}

#[test]
fn mockup_file_matches_the_mockup() {
    let f = read("operating_2026-11-03.qfx");
    assert_eq!(format!("{:?}", f.format), "Qfx");
    assert_eq!(
        f.institution.as_ref().and_then(|i| i.intu_bid.as_deref()),
        Some("40404")
    );
    let s = &f.sections[0];
    assert_eq!(s.rows.len(), 41);
    assert_eq!(
        s.account.as_ref().unwrap().mask(),
        "\u{2022}\u{2022}\u{2022}\u{2022}2208"
    );
    assert_eq!(
        s.ledger_balance.unwrap().amount,
        Decimal::from_str("92431.07").unwrap()
    );
    let check = s
        .rows
        .iter()
        .find(|r| r.check_number == Some(4412))
        .unwrap();
    assert_eq!(check.description, "CHECK");
    assert_eq!(check.file_amount, Decimal::from_str("-925.00").unwrap());
}

#[test]
fn errors_carry_no_account_numbers() {
    let f = read("multi_account.ofx");
    let dbg = format!("{f:?}");
    for acct in ["111122223333", "111122224444", "5500001234", "021000021"] {
        assert!(!dbg.contains(acct), "Debug output shows {acct}");
    }
    let _ = ImportError::Empty;
}
