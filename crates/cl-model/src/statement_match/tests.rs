use proptest::prelude::*;
use serde_json::{Value, json};

use super::*;

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn ymd(y: i32, m: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, d).expect("date")
}

fn txn(n: u64) -> TransactionId {
    TransactionId::parse(&format!("00000000-0000-0000-0000-{n:012}")).expect("id")
}

fn today() -> CalendarDate {
    ymd(2026, 11, 3)
}

fn line(d: u32, r: &str, kind: StatementKind, amount: &str) -> StatementLine {
    StatementLine {
        date: ymd(2026, 10, d),
        reference: r.into(),
        kind,
        amount: money(amount),
    }
}

fn item(id: u64, d: u32, r: &str, kind: StatementKind, amount: &str) -> BooksItem {
    BooksItem {
        id: txn(id),
        kind,
        date: ymd(2026, 10, d),
        reference: r.into(),
        amount: money(amount),
    }
}

use StatementKind::{Charge, Credit, CreditOrPayment, Payment};

#[test]
fn kinds_parse_and_pair() {
    assert_eq!(StatementKind::parse(" Invoice "), Some(Charge));
    assert_eq!(StatementKind::parse("BILL"), Some(Charge));
    assert_eq!(StatementKind::parse("credit memo"), Some(Credit));
    assert_eq!(StatementKind::parse("Pmt"), Some(Payment));
    assert_eq!(StatementKind::parse("refund"), None);
    assert!(Charge.compatible(Charge));
    assert!(!Charge.compatible(Credit));
    assert!(CreditOrPayment.compatible(Credit) && CreditOrPayment.compatible(Payment));
    assert!(Payment.compatible(CreditOrPayment));
    assert!(!CreditOrPayment.compatible(Charge));
}

#[test]
fn parse_paste_tabs_commas_quotes_and_bad_rows() {
    let text = "Date,Ref,Amount\n\
                10/02/2026,INV-1,\"1,330.00\"\n\
                \n\
                10/03/2026,CM-1,(75.00),credit\r\n\
                10/04/2026\tP 9\t-10.5\n\
                10/05/2026,X,0\n\
                13/40/2026,X,5\n\
                10/06/2026,X,abc\n\
                10/07/2026,X,5,refund\n\
                10/08/2026,X\n\
                10/09/2026,\"say \"\"hi\"\"\",5.001\n";
    let p = parse_paste(text, DateOrder::Mdy, today());
    assert!(p.header_skipped);
    assert_eq!(
        p.rows.iter().map(|r| r.line).collect::<Vec<_>>(),
        vec![2, 4, 5]
    );
    let lines = p.lines();
    assert_eq!(lines[0], line(2, "INV-1", Charge, "1330"));
    assert_eq!(lines[1], line(3, "CM-1", Credit, "75"));
    assert_eq!(
        lines[2],
        line(4, "P 9", CreditOrPayment, "10.5"),
        "the sign decides"
    );
    assert_eq!(
        p.bad,
        vec![
            BadRow {
                line: 6,
                problem: PasteProblem::ZeroAmount
            },
            BadRow {
                line: 7,
                problem: PasteProblem::Date
            },
            BadRow {
                line: 8,
                problem: PasteProblem::Amount
            },
            BadRow {
                line: 9,
                problem: PasteProblem::Kind
            },
            BadRow {
                line: 10,
                problem: PasteProblem::Columns
            },
            BadRow {
                line: 11,
                problem: PasteProblem::Amount
            },
        ]
    );
    assert_eq!(split_csv("a,\"b \"\"c\"\"\",d"), vec!["a", "b \"c\"", "d"]);
    // A bad first line that has a date or an amount is a bad row, not a
    // heading.
    let p = parse_paste("10/02/2026,X,abc\n", DateOrder::Mdy, today());
    assert!(!p.header_skipped);
    assert_eq!(p.bad.len(), 1);
    assert_eq!(PasteProblem::Kind.key(), TextKey("statement.paste.kind"));
    assert!(!format!("{:?}", lines[0]).contains("INV"));
    assert!(!format!("{:?}", item(1, 1, "INV-9", Charge, "5")).contains("INV"));
}

#[test]
fn pass_one_pairs_by_reference_and_exact_amount() {
    let stmt = vec![
        line(2, "inv-1", Charge, "100"),
        line(3, "INV-2", Charge, "50.01"),
    ];
    let books = vec![
        item(1, 20, "INV-1", Charge, "100"),
        item(2, 3, "INV-2", Charge, "50"),
    ];
    let r = match_lines(&stmt, &books, DEFAULT_DATE_SLACK_DAYS);
    assert_eq!(
        r.pairs,
        vec![MatchPair {
            statement: 0,
            books: txn(1),
            pass: MatchPass::Reference
        }],
        "a ref match ignores the date; one cent off never pairs"
    );
    assert_eq!(r.missing_in_books, vec![1]);
    assert_eq!(r.missing_on_statement, vec![txn(2)]);
    assert_eq!(r.ambiguous.len(), 0);
}

#[test]
fn duplicates_in_the_books_pair_once_closest_date_first() {
    let stmt = vec![line(10, "INV-1", Charge, "100")];
    let books = vec![
        item(9, 12, "INV-1", Charge, "100"),
        item(3, 9, "INV-1", Charge, "100"),
        item(2, 11, "INV-1", Charge, "100"),
    ];
    let r = match_lines(&stmt, &books, 7);
    // 9th and 11th are both one day away: the earlier date wins.
    assert_eq!(r.pairs[0].books, txn(3));
    assert_eq!(
        r.missing_on_statement,
        vec![txn(2), txn(9)],
        "never many-to-one"
    );
}

#[test]
fn pass_two_needs_a_unique_candidate_on_both_sides() {
    // A payment with a different ref, 1 day apart: paired.
    let stmt = vec![line(15, "PMT 1043", CreditOrPayment, "1810")];
    let books = vec![item(4, 14, "1043", Payment, "1810")];
    let r = match_lines(&stmt, &books, 7);
    assert_eq!(r.pairs[0].pass, MatchPass::AmountAndDate);
    // Outside the slack: not paired.
    let r = match_lines(&stmt, &books, 0);
    assert_eq!(r.missing_in_books, vec![0]);
    assert_eq!(r.missing_on_statement, vec![txn(4)]);
    assert!(
        match_lines(&stmt, &books, -5).pairs.is_empty(),
        "negative slack is zero"
    );

    // Two equal books items near one line: ambiguous with both listed.
    let books = vec![
        item(5, 16, "", Payment, "1810"),
        item(4, 14, "", Payment, "1810"),
    ];
    let r = match_lines(&stmt, &books, 7);
    assert_eq!(r.pairs.len(), 0);
    assert_eq!(
        r.ambiguous,
        vec![Ambiguous {
            statement: 0,
            candidates: vec![txn(4), txn(5)]
        }]
    );
    assert!(r.missing_on_statement.is_empty() && r.missing_in_books.is_empty());

    // Two lines near one item: both ambiguous, the item is not missing.
    let stmt = vec![line(13, "", Payment, "1810"), line(15, "", Payment, "1810")];
    let books = vec![item(4, 14, "", Payment, "1810")];
    let r = match_lines(&stmt, &books, 7);
    assert_eq!(r.pairs.len(), 0);
    assert_eq!(r.ambiguous.len(), 2);
    assert_eq!(r.missing_on_statement.len(), 0);
    // Kinds must agree.
    let stmt = vec![line(14, "", Charge, "1810")];
    let r = match_lines(&stmt, &books, 7);
    assert_eq!(r.missing_in_books, vec![0]);
}

#[test]
fn empty_inputs() {
    assert_eq!(match_lines(&[], &[], 7), MatchResult::default());
    let books = vec![item(1, 1, "A", Charge, "1")];
    assert_eq!(
        match_lines(&[], &books, 7).missing_on_statement,
        vec![txn(1)]
    );
}

fn books_from_json(raw: &str) -> Vec<BooksItem> {
    let v: Value = serde_json::from_str(raw).expect("books json");
    v["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| {
            let s = |k: &str| i[k].as_str().expect("string field").to_string();
            BooksItem {
                id: TransactionId::parse(&s("id")).expect("id"),
                kind: match s("kind").as_str() {
                    "bill" => Charge,
                    "credit" => Credit,
                    "payment" => Payment,
                    other => panic!("kind {other}"),
                },
                date: CalendarDate::parse_wire(&s("date")).expect("date"),
                reference: s("ref"),
                amount: Money::parse_wire(&s("amount")).expect("amount"),
            }
        })
        .collect()
}

#[test]
fn golden_vendor_statement_one_missing_each_side() {
    let tsv = include_str!("../../../../fixtures/statement-match/vendor_oct.tsv");
    let books = books_from_json(include_str!(
        "../../../../fixtures/statement-match/books.json"
    ));
    let expected: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/statement-match/expected.json"
    ))
    .expect("expected json");
    let parsed = parse_paste(tsv, DateOrder::Mdy, today());
    let stmt = parsed.lines();
    let r = match_lines(&stmt, &books, DEFAULT_DATE_SLACK_DAYS);
    let line_of = |i: usize| parsed.rows[i].line;
    let got = json!({
        "description": expected["description"],
        "header_skipped": parsed.header_skipped,
        "bad_rows": parsed.bad.iter().map(|b| b.line).collect::<Vec<_>>(),
        "pairs": r.pairs.iter().map(|p| json!({
            "line": line_of(p.statement),
            "books": p.books.to_string(),
            "pass": match p.pass {
                MatchPass::Reference => "reference",
                MatchPass::AmountAndDate => "amount_and_date",
            },
        })).collect::<Vec<_>>(),
        "missing_in_books": r.missing_in_books.iter().map(|&i| line_of(i)).collect::<Vec<_>>(),
        "missing_on_statement": r.missing_on_statement.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "ambiguous": r.ambiguous.iter().map(|a| json!({
            "line": line_of(a.statement),
            "candidates": a.candidates.iter().map(ToString::to_string).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    assert_eq!(got, expected);
}

// ---- Proptest (2): partial bijection, equal amounts, permutation
// invariance.

fn kind_strategy() -> impl Strategy<Value = StatementKind> {
    prop_oneof![
        Just(Charge),
        Just(Credit),
        Just(Payment),
        Just(CreditOrPayment)
    ]
}

fn stmt_strategy() -> impl Strategy<Value = StatementLine> {
    (
        1_u32..=12,
        prop_oneof![Just(""), Just("A"), Just("a"), Just("B")],
        kind_strategy(),
        1_i64..=4,
    )
        .prop_map(|(d, r, kind, cents)| StatementLine {
            date: ymd(2026, 10, d),
            reference: r.to_string(),
            kind,
            amount: Money::from_cents(cents * 100).expect("m"),
        })
}

fn books_strategy() -> impl Strategy<Value = Vec<BooksItem>> {
    proptest::collection::vec(
        (
            1_u32..=12,
            prop_oneof![Just(""), Just("A"), Just("B")],
            prop_oneof![Just(Charge), Just(Credit), Just(Payment)],
            1_i64..=4,
        ),
        0..10,
    )
    .prop_map(|rows| {
        rows.into_iter()
            .enumerate()
            .map(|(i, (d, r, kind, cents))| BooksItem {
                id: txn(u64::try_from(i).expect("i") + 1),
                kind,
                date: ymd(2026, 10, d),
                reference: r.to_string(),
                amount: Money::from_cents(cents * 100).expect("m"),
            })
            .collect()
    })
}

/// The result in value terms (statement lines by value, not index).
#[allow(clippy::type_complexity)]
fn normalised(
    stmt: &[StatementLine],
    r: &MatchResult,
) -> (
    Vec<(StatementLine, TransactionId, MatchPass)>,
    Vec<StatementLine>,
    Vec<TransactionId>,
    Vec<(StatementLine, Vec<TransactionId>)>,
) {
    let mut pairs: Vec<_> = r
        .pairs
        .iter()
        .map(|p| (stmt[p.statement].clone(), p.books, p.pass))
        .collect();
    pairs.sort();
    let mut missing: Vec<_> = r
        .missing_in_books
        .iter()
        .map(|&i| stmt[i].clone())
        .collect();
    missing.sort();
    let mut amb: Vec<_> = r
        .ambiguous
        .iter()
        .map(|a| (stmt[a.statement].clone(), a.candidates.clone()))
        .collect();
    amb.sort();
    (pairs, missing, r.missing_on_statement.clone(), amb)
}

proptest! {
    #[test]
    fn matching_is_a_permutation_invariant_partial_bijection(
        (stmt, books, stmt2, books2) in (
            proptest::collection::vec(stmt_strategy(), 0..10),
            books_strategy(),
        ).prop_flat_map(|(s, b)| (
            Just(s.clone()),
            Just(b.clone()),
            Just(s).prop_shuffle(),
            Just(b).prop_shuffle(),
        )),
        slack in 0_i64..=8,
    ) {
        let r = match_lines(&stmt, &books, slack);
        // A partial bijection with equal amounts and compatible kinds.
        let mut seen_s = std::collections::BTreeSet::new();
        let mut seen_b = std::collections::BTreeSet::new();
        for p in &r.pairs {
            prop_assert!(seen_s.insert(p.statement));
            prop_assert!(seen_b.insert(p.books));
            let b = books.iter().find(|b| b.id == p.books).expect("known id");
            prop_assert_eq!(stmt[p.statement].amount, b.amount);
            prop_assert!(stmt[p.statement].kind.compatible(b.kind));
        }
        // Every statement line is in exactly one bucket.
        let mut all_s: Vec<usize> = r.pairs.iter().map(|p| p.statement)
            .chain(r.missing_in_books.iter().copied())
            .chain(r.ambiguous.iter().map(|a| a.statement))
            .collect();
        all_s.sort_unstable();
        prop_assert_eq!(all_s, (0..stmt.len()).collect::<Vec<_>>());
        // A books item is paired, missing, or a candidate; never two of them.
        for b in &books {
            let paired = seen_b.contains(&b.id);
            let missing = r.missing_on_statement.contains(&b.id);
            let candidate = r.ambiguous.iter().any(|a| a.candidates.contains(&b.id));
            prop_assert_eq!(u8::from(paired) + u8::from(missing) + u8::from(candidate), 1);
        }
        // Deterministic and independent of the input order.
        prop_assert_eq!(&match_lines(&stmt, &books, slack), &r);
        let r2 = match_lines(&stmt2, &books2, slack);
        prop_assert_eq!(normalised(&stmt, &r), normalised(&stmt2, &r2));
    }
}
