//! The export fill (A09 #2, #3, #4): pages go where their `more` row was,
//! the cap trims a page and keeps its `more` row, a page that brings
//! nothing for its cursor stops the fill.

use cl_core::CalendarDate;
use cl_core::report::{Drill, ReportRow, ReportTable, RowKind, RowPath};

use super::*;
use crate::resources::report_builder::{ColumnLayout, RunDates, RunPurpose};

fn row(kind: RowKind, path: &str, caption: &str) -> ReportRow {
    ReportRow {
        kind,
        level: 2,
        path: RowPath::of([path]),
        caption: caption.into(),
        cells: Vec::new(),
        drill: None,
        expandable: false,
    }
}

fn more(path: &str, cursor: &str) -> ReportRow {
    let mut r = row(RowKind::More, path, "more");
    r.drill = Some(Drill::BuilderPage {
        node_id: "s1".into(),
        path: RowPath::of([path]),
        cursor: Some(cursor.into()),
    });
    r
}

fn details(path: &str, n: usize) -> Vec<ReportRow> {
    (0..n)
        .map(|i| {
            let mut r = row(RowKind::Detail, path, &format!("{path}-{i}"));
            r.level = 0;
            r
        })
        .collect()
}

fn request() -> RunRequest {
    let d = CalendarDate::from_ymd(2026, 10, 1).expect("d");
    RunRequest {
        reference: RunRef {
            definition: None,
            definition_id: Some("def".into()),
            version: None,
            dates: RunDates::as_of(d),
            period_ranges: Vec::new(),
            compare_ranges: Vec::new(),
            fiscal_year_starts: vec![d],
            basis: None,
            adjusting: None,
        },
        open: Vec::new(),
        collapsed: Vec::new(),
        expand_all: true,
        purpose: RunPurpose::Export,
    }
}

/// Two lines, each with 2 detail rows and a `more` row.
fn run() -> BuilderRun {
    let mut rows = vec![row(RowKind::Account, "a", "A")];
    rows.extend(details("a", 2).into_iter().map(|mut r| {
        r.level = 2;
        r
    }));
    rows.push(more("a", "a1"));
    rows.push(row(RowKind::Account, "b", "B"));
    rows.extend(details("b", 2).into_iter().map(|mut r| {
        r.level = 2;
        r
    }));
    rows.push(more("b", "b1"));
    BuilderRun {
        table: ReportTable {
            rows,
            ..ReportTable::default()
        },
        layout: ColumnLayout::default(),
        detail_columns: Vec::new(),
        detail_layout: ColumnLayout::default(),
        fingerprint: "fp".into(),
        stamp: 7,
        denied: Vec::new(),
        expanded: Vec::new(),
        truncated: true,
        preview_rows: None,
    }
}

/// Pages: `a1` → 3 rows then `a2` → 1 row and done; `b1` → 2 rows, done.
fn page(dr: &DetailRequest) -> DetailPage {
    let (path, n, next) = match dr.cursor.as_deref() {
        Some("a1") => ("a", 3, Some("a2")),
        Some("a2") => ("a", 1, None),
        Some("b1") => ("b", 2, None),
        other => panic!("no such cursor {other:?}"),
    };
    DetailPage {
        rows: details(path, n),
        cursor: next.map(str::to_string),
        subtotal: Vec::new(),
    }
}

fn fill(max: usize) -> (Filled, Vec<String>) {
    let mut f = ExportFill::new(&request(), run(), max);
    let mut asked = Vec::new();
    while let Some(dr) = f.next_request() {
        assert_eq!((dr.fingerprint.as_str(), dr.stamp), ("fp", 7));
        asked.push(dr.cursor.clone().unwrap_or_default());
        let p = page(&dr);
        f.take(p);
        assert!(asked.len() < 10, "no loop");
    }
    (f.finish(), asked)
}

fn captions(t: &ReportTable) -> Vec<String> {
    t.rows.iter().map(|r| r.caption.clone()).collect()
}

#[test]
fn every_page_goes_where_its_more_row_was() {
    let (f, asked) = fill(MAX_EXPORT_ROWS);
    assert_eq!(asked, ["a1", "a2", "b1"]);
    assert!(!f.cut);
    let c = captions(&f.run.table);
    assert_eq!(c.len(), 2 + 2 + 3 + 1 + 2 + 2, "lines + every detail row");
    assert_eq!(&c[..7], ["A", "a-0", "a-1", "a-0", "a-1", "a-2", "a-0"]);
    assert_eq!(c[7], "B");
    assert!(f.run.table.rows.iter().all(|r| r.kind != RowKind::More));
    // Page rows sit one level under their line.
    assert!(
        f.run
            .table
            .rows
            .iter()
            .filter(|r| r.kind == RowKind::Detail)
            .all(|r| r.level == 2)
    );
}

/// Minor 4: the cap is exact; the page that would pass it is trimmed, its
/// `more` row stays (with its cursor) and the file is cut.
#[test]
fn the_cap_trims_the_page_and_keeps_the_more_row() {
    // 4 detail rows to start; a cap of 6 takes 2 of a1's 3.
    let (f, asked) = fill(6);
    assert_eq!(asked, ["a1"]);
    assert!(f.cut);
    let t = &f.run.table;
    assert_eq!(
        t.rows.iter().filter(|r| r.kind == RowKind::Detail).count(),
        6
    );
    let m: Vec<_> = t.rows.iter().filter(|r| r.kind == RowKind::More).collect();
    assert_eq!(m.len(), 2, "a's (cut) and b's (never asked)");
    assert!(matches!(
        &m[0].drill,
        Some(Drill::BuilderPage { cursor: Some(c), .. }) if c == "a1"
    ));
    // At the cap already: nothing is asked.
    let (f, asked) = fill(4);
    assert!(asked.is_empty() && f.cut);
}

/// A page that brings nothing for the same cursor stops (an engine bug
/// can't loop the export); the `more` row stays, the file is cut.
#[test]
fn a_page_that_brings_nothing_stops() {
    let mut f = ExportFill::new(&request(), run(), MAX_EXPORT_ROWS);
    let dr = f.next_request().expect("a1");
    f.take(DetailPage {
        rows: Vec::new(),
        cursor: dr.cursor.clone(),
        subtotal: Vec::new(),
    });
    assert!(f.next_request().is_none());
    assert!(f.finish().cut);
}

#[test]
fn splice_without_a_cursor_drops_the_more_row() {
    let mut t = run().table;
    splice(&mut t, &RowPath::of(["b"]), details("b", 1), None);
    assert_eq!(t.rows.iter().filter(|r| r.kind == RowKind::More).count(), 1);
    assert_eq!(t.rows.last().map(|r| r.caption.as_str()), Some("b-0"));
}
