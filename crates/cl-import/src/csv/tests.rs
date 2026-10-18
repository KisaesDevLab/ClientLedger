use super::*;
use proptest::prelude::*;

fn fields(text: &str) -> Vec<Vec<String>> {
    parse(text)
        .expect("parse")
        .into_iter()
        .map(|r| r.fields)
        .collect()
}

#[test]
fn plain_quoted_and_line_ends() {
    let rows = fields("Name,Price\r\nMowing,45.00\nEdging,\"1,200.50\"\rLast,\"say \"\"hi\"\"\"");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[1], vec!["Mowing", "45.00"]);
    assert_eq!(rows[2], vec!["Edging", "1,200.50"]);
    assert_eq!(rows[3], vec!["Last", "say \"hi\""]);
}

#[test]
fn line_breaks_inside_quotes_and_line_numbers() {
    let rows = parse("a,b\n\"one\ntwo\",x\n\n,\nc,d\n").expect("parse");
    assert_eq!(rows.len(), 3, "blank and empty rows skipped");
    assert_eq!(rows[1].fields[0], "one\ntwo");
    assert_eq!(rows[1].line, 2);
    assert_eq!(rows[2].line, 6);
}

#[test]
fn bom_separators_and_encodings() {
    assert_eq!(fields("\u{FEFF}a;b\n1;2")[1], vec!["1", "2"]);
    assert_eq!(fields("a\tb\n1\t2")[1], vec!["1", "2"]);
    assert_eq!(sniff_separator("\"x,y\";z;w"), ';');
    assert_eq!(sniff_separator("single"), ',');
    assert_eq!(decode(b"\xEF\xBB\xBFCaf\xC3\xA9"), "Café");
    assert_eq!(
        decode(b"Caf\xE9 \x93q\x94 \x80"),
        "Café \u{201C}q\u{201D} €"
    );
}

#[test]
fn errors() {
    assert_eq!(parse(""), Err(CsvError::Empty));
    assert_eq!(parse("\n\n , \n"), Err(CsvError::Empty));
    assert_eq!(
        parse("a,b\nc,\"open\n"),
        Err(CsvError::UnclosedQuote { line: 2 })
    );
    let wide = vec!["x"; MAX_COLUMNS + 1].join(",");
    assert_eq!(parse(&wide), Err(CsvError::TooManyColumns { line: 1 }));
    let long = "y".repeat(MAX_FIELD_BYTES + 1);
    assert_eq!(parse(&long), Err(CsvError::FieldTooLong { line: 1 }));
    let quoted_long = format!("\"{long}\"");
    assert_eq!(parse(&quoted_long), Err(CsvError::FieldTooLong { line: 1 }));
    let many = "a\n".repeat(MAX_ROWS + 2);
    assert_eq!(parse(&many), Err(CsvError::TooManyRows));
    assert!(CsvError::TooManyRows.to_string().contains("rows"));
}

#[test]
fn writing_quotes_and_formulas() {
    assert_eq!(field("plain"), "plain");
    assert_eq!(field("a,b"), "\"a,b\"");
    assert_eq!(field("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert_eq!(field(" pad"), "\" pad\"");
    assert_eq!(cell_text("=SUM(A1)"), "'=SUM(A1)");
    assert_eq!(cell_text("+1"), "'+1");
    assert_eq!(cell_text("@x"), "'@x");
    assert_eq!(cell_text("-x"), "'-x");
    assert_eq!(cell_text("-12.50"), "-12.50");
    assert_eq!(cell_text("Route"), "Route");
    let out = write([["Tag", "Group"], ["North, East", ""]]);
    assert_eq!(out, "Tag,Group\r\n\"North, East\",\r\n");
}

proptest! {
    /// Whatever is written reads back the same (rows that are not all blank).
    #[test]
    fn write_then_parse_round_trips(
        rows in proptest::collection::vec(
            proptest::collection::vec("[a-z ,;\"\n\r\t'=é0-9]{0,8}", 1..5), 1..8)
    ) {
        let rows: Vec<Vec<String>> = rows
            .into_iter()
            .filter(|r| !r.iter().all(|f| f.trim().is_empty()))
            .collect();
        prop_assume!(!rows.is_empty());
        let text = write(&rows);
        let back: Vec<Vec<String>> = parse_with(&text, ',')
            .expect("parse")
            .into_iter()
            .map(|r| r.fields)
            .collect();
        // CR alone inside a quoted value reads back as a line break.
        let norm = |s: &String| s.replace("\r\n", "\n").replace('\r', "\n");
        let want: Vec<Vec<String>> = rows.iter().map(|r| r.iter().map(norm).collect()).collect();
        prop_assert_eq!(back, want);
    }

    /// Any text either reads or fails with a typed error, never panics.
    #[test]
    fn never_panics(text in "\\PC{0,200}") {
        let _ = parse(&text);
    }
}
