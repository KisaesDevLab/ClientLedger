use super::*;
use proptest::prelude::*;
use std::str::FromStr as _;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

#[test]
fn amounts_in_every_written_form() {
    let ok = |s: &str, want: &str| assert_eq!(parse_amount(s), Ok(dec(want)), "{s}");
    ok("-45.00", "-45");
    ok("45.00-", "-45");
    ok("(45.00)", "-45");
    ok("\u{2212}45.10", "-45.1");
    ok("$1,234.56", "1234.56");
    ok("-$1,234.56", "-1234.56");
    ok("+12", "12");
    ok(" 45.1000 ", "45.1");
    ok("45.10000000", "45.1");
    ok("0.01", "0.01");
    ok("-999999999999.99", "-999999999999.99");
    ok("000012.50", "12.5");
    ok("0", "0");
}

#[test]
fn extra_decimals_round_half_away_from_zero() {
    // Owner answer Q-P25q (2026-11-07): round to the cent, keep the text.
    let r = |s: &str, want: &str| {
        let a = parse_amount_detail(s).unwrap();
        assert_eq!(a.value, dec(want), "{s}");
        assert_eq!(a.rounded_from.as_deref(), Some(s.trim()), "{s}");
    };
    r("12.345", "12.35");
    r("12.3449", "12.34");
    r("-2.005", "-2.01");
    r("(2.005)", "-2.01");
    r("2.005-", "-2.01");
    r(" 1,234.5678 ", "1234.57");
    r("999999999999.994", "999999999999.99");
    // Trailing zeros are not rounding.
    assert_eq!(parse_amount_detail("45.1000").unwrap().rounded_from, None);
    assert_eq!(parse_amount_detail("45.10").unwrap().rounded_from, None);
}

#[test]
fn amount_problems() {
    let bad = |s: &str, want: AmountProblem| assert_eq!(parse_amount(s), Err(want), "{s}");
    bad("", AmountProblem::Missing);
    bad("   ", AmountProblem::Missing);
    bad("999999999999.995", AmountProblem::TooManyDigits);
    bad("1234567890123", AmountProblem::TooManyDigits);
    bad("1e5", AmountProblem::Bad);
    bad("12,50", AmountProblem::Bad);
    bad("abc", AmountProblem::Bad);
    bad("--5", AmountProblem::Bad);
    bad("-5-", AmountProblem::Bad);
    bad(&"9".repeat(1_000_000), AmountProblem::TooManyDigits);
}

proptest! {
    #[test]
    fn parse_format_round_trip(cents in -99_999_999_999_999i64..=99_999_999_999_999i64) {
        let d = Decimal::new(cents, 2);
        prop_assert_eq!(parse_amount(&format_amount(d)), Ok(d.normalize()));
    }
}

#[test]
fn decoding_rules() {
    // Declared 1252 but valid UTF-8: read as UTF-8, with a note.
    let d = decode("Caf\u{e9}".as_bytes(), Declared::SingleByte).unwrap();
    assert_eq!(d.text, "Caf\u{e9}");
    assert_eq!(d.note, Some(EncodingNote::DeclaredSingleByteReadAsUtf8));
    // Real 1252 bytes.
    let d = decode(b"Caf\xe9 \x80", Declared::SingleByte).unwrap();
    assert_eq!(d.text, "Caf\u{e9} \u{20ac}");
    assert_eq!(d.encoding, TextEncoding::Windows1252);
    // BOMs.
    let d = decode(b"\xEF\xBB\xBFabc", Declared::Nothing).unwrap();
    assert_eq!(
        (d.text.as_str(), d.encoding),
        ("abc", TextEncoding::Utf8Bom)
    );
    let d = decode(b"\xFF\xFEa\x00\n\x00b\x00", Declared::Nothing).unwrap();
    assert_eq!(
        (d.text.as_str(), d.encoding),
        ("a\nb", TextEncoding::Utf16Le)
    );
    let d = decode(b"\xFE\xFF\x00a", Declared::Nothing).unwrap();
    assert_eq!((d.text.as_str(), d.encoding), ("a", TextEncoding::Utf16Be));
    assert_eq!(
        decode(b"\xFF\xFEa\x00\n\x00\x00\xD8", Declared::Nothing),
        Err(ImportError::BadText { line: 2 })
    );
    assert_eq!(declared_charset("ISO-8859-1"), Some(Declared::SingleByte));
    assert_eq!(declared_charset("usascii"), Some(Declared::SingleByte));
    assert_eq!(declared_charset("NONE"), Some(Declared::SingleByte));
    assert_eq!(declared_charset("cp1252"), Some(Declared::SingleByte));
    assert_eq!(declared_charset("utf-8"), Some(Declared::Utf8));
    assert_eq!(declared_charset("Shift_JIS"), None);
}

#[test]
fn text_and_checks() {
    assert_eq!(clean_text("  Cafe\u{301}\u{0}  x ", 500), "Caf\u{e9}  x");
    assert_eq!(
        clean_text(&"a".repeat(600), MAX_DESCRIPTION_CHARS).len(),
        500
    );
    assert_eq!(check_or_ref("001234"), (Some(1234), None));
    assert_eq!(check_or_ref("123456789"), (Some(123_456_789), None));
    assert_eq!(
        check_or_ref("1234567890"),
        (None, Some("1234567890".to_owned()))
    );
    assert_eq!(check_or_ref("ATM"), (None, Some("ATM".to_owned())));
    assert_eq!(check_or_ref(" "), (None, None));
    assert_eq!(
        memo_with_ref(Some("rent"), Some("X9")),
        Some("rent Ref: X9".to_owned())
    );
    assert_eq!(memo_with_ref(None, None), None);
}

#[test]
fn zero_rows_are_skipped_not_sent() {
    let mut s = Section::new(0, AccountKind::Checking);
    let date = CalendarDate::from_ymd(2026, 11, 3).unwrap();
    for (line, amount) in [
        (1, "0.00"),
        (2, "-4.50"),
        (3, "12.3x"),
        (4, "0.004"),
        (5, "-1.005"),
    ] {
        s.push_parts(&RowParts {
            line,
            date: Ok(date),
            amount: parse_amount_detail(amount),
            description: "x",
            memo: None,
            check: None,
            fitid: Some(" F1 "),
            running_balance: None,
            trn_type: Some("debit"),
        });
    }
    let skipped: Vec<usize> = s.skipped.iter().map(|k| k.line).collect();
    assert_eq!(skipped, vec![1], "only a real zero is skipped");
    assert!(s.skipped.iter().all(|k| k.reason == SkipReason::ZeroAmount));
    assert_eq!(s.rows.len(), 2);
    assert_eq!(s.rows[0].fitid.as_deref(), Some("F1"));
    assert_eq!(s.rows[0].trn_type.as_deref(), Some("DEBIT"));
    assert_eq!(s.rows[0].rounded_from, None);
    assert_eq!(s.rows[1].file_amount, dec("-1.01"));
    assert_eq!(s.rows[1].rounded_from.as_deref(), Some("-1.005"));
    assert_eq!(
        s.problems,
        vec![
            Problem {
                line: 3,
                kind: ProblemKind::BadAmount
            },
            // 0.004 rounds to zero: listed with its line, not dropped.
            Problem {
                line: 4,
                kind: ProblemKind::BelowOneCent
            }
        ]
    );
    assert_eq!(s.total(), Some(dec("-5.51")));
}
