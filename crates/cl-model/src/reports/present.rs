//! The owner's presentation of a report (Q-271, AC5): tag rows under the
//! tag split captioned in the chosen label style (append, prepend or none,
//! with the owner's separator) and the split's account totals shown only
//! when asked for. The screen, every export, print and the goldens use
//! these, so a file carries what the window shows. Formatting only: no
//! amount is touched.

use cl_core::report::{ReportRow, ReportTable, RowKind, RowPath};

use super::options::{ReportOptions, tag_row_caption};

/// True when `row` is a tag split's account total.
fn is_split_total(row: &ReportRow) -> bool {
    row.kind == RowKind::AccountTotal && row.path.0.last().is_some_and(|p| p.ends_with("~split"))
}

/// True when the owner's options hide `row` (a split's account total
/// without "Show a total for each account").
#[must_use]
pub fn hidden(row: &ReportRow, options: &ReportOptions) -> bool {
    is_split_total(row)
        && !options
            .tag_split
            .as_ref()
            .is_some_and(|t| t.show_account_total)
}

/// The caption of `table.rows[i]` in the owner's label style when it is a
/// tag row under the tag split; `None` for every other row (its caption is
/// the table's).
#[must_use]
pub fn tag_caption(table: &ReportTable, i: usize, options: &ReportOptions) -> Option<String> {
    let r = table.rows.get(i)?;
    let split = options.tag_split.as_ref()?;
    if r.kind != RowKind::TagRow {
        return None;
    }
    let n = r.path.0.len().saturating_sub(1);
    let parent = RowPath(r.path.0[..n].to_vec());
    // The account row sits above its tag rows.
    let acct = table.rows[..i].iter().rev().find(|x| x.path == parent)?;
    Some(tag_row_caption(&acct.caption, &r.caption, split))
}

/// The rows at `rows` (indexes into `table`, in the order shown) as their
/// own table, tag rows captioned in the owner's style and hidden rows left
/// out: what exports, print and the goldens write.
#[must_use]
pub fn present(table: &ReportTable, rows: &[usize], options: &ReportOptions) -> ReportTable {
    let out = rows
        .iter()
        .filter_map(|&i| {
            let r = table.rows.get(i)?;
            if hidden(r, options) {
                return None;
            }
            let mut r = r.clone();
            if let Some(c) = tag_caption(table, i, options) {
                r.caption = c;
            }
            Some(r)
        })
        .collect();
    ReportTable {
        columns: table.columns.clone(),
        rows: out,
        meta: table.meta.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reports::{TagLabel, TagSplit};
    use cl_core::ids::TagGroupId;
    use cl_core::report::Cell;
    use rust_decimal::Decimal;

    fn r(kind: RowKind, path: &[&str], caption: &str, x: i64) -> ReportRow {
        ReportRow {
            kind,
            level: 1,
            path: RowPath::of(path.iter().copied()),
            caption: caption.into(),
            cells: vec![Cell::Amount(Decimal::new(x, 2))],
            drill: None,
            expandable: false,
        }
    }

    fn table() -> ReportTable {
        ReportTable {
            columns: Vec::new(),
            rows: vec![
                r(
                    RowKind::Account,
                    &["expenses", "a"],
                    "6100 \u{b7} Repairs",
                    100,
                ),
                r(
                    RowKind::TagRow,
                    &["expenses", "a", "t:1"],
                    "10 \u{b7} Lake Area",
                    50,
                ),
                r(
                    RowKind::AccountTotal,
                    &["expenses", "a~split"],
                    "@reports.row.total_of Repairs",
                    150,
                ),
                r(
                    RowKind::Total,
                    &["expenses~total"],
                    "@reports.pl.total_expenses",
                    150,
                ),
            ],
            meta: cl_core::report::ReportMeta::default(),
        }
    }

    fn opts(label: TagLabel, total: bool) -> ReportOptions {
        let mut o = ReportOptions::for_spec(crate::reports::find("profit_loss").expect("p&l"));
        o.tag_split = Some(TagSplit {
            tag_group_id: TagGroupId::parse("22222222-2222-4222-8222-222222222222").expect("g"),
            show_account_total: total,
            label,
            separator: "-".into(),
        });
        o
    }

    #[test]
    fn exports_carry_the_label_style_and_the_total_choice() {
        let t = table();
        let all: Vec<usize> = (0..t.rows.len()).collect();
        let p = present(&t, &all, &opts(TagLabel::Append, false));
        let captions: Vec<&str> = p.rows.iter().map(|r| r.caption.as_str()).collect();
        assert_eq!(
            captions,
            [
                "6100 \u{b7} Repairs",
                "6100-10 Repairs - Lake Area",
                "@reports.pl.total_expenses"
            ]
        );
        let p = present(&t, &all, &opts(TagLabel::Prepend, true));
        assert_eq!(p.rows[1].caption, "10-6100 Repairs - Lake Area");
        assert_eq!(p.rows.len(), 4, "the account total when asked for");
        let p = present(&t, &all, &opts(TagLabel::None, false));
        assert_eq!(p.rows[1].caption, "Repairs - Lake Area");
        // Amounts are untouched.
        assert_eq!(p.rows[1].cells, t.rows[1].cells);
        // Without the split nothing changes.
        let plain = ReportOptions::for_spec(crate::reports::find("profit_loss").expect("p&l"));
        assert_eq!(present(&t, &all, &plain).rows, t.rows);
    }
}
