//! The physical inventory count sheet (P24 plan §3): one row per active
//! part with a blank Counted column to write in. On hand is printed only
//! when the owner asks (a blind count leaves it off).

use super::{Column, FormDoc, FormStyle, layout_form};
use crate::PrintError;
use crate::layout::Document;

/// One part on the sheet (the engine's figures, already formatted).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CountSheetLine {
    /// Item name.
    pub item: String,
    /// Description.
    pub description: String,
    /// Unit.
    pub unit: String,
    /// On hand as of the sheet's date (formatted).
    pub on_hand: String,
}

/// The words the sheet prints (the caller's translations).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CountSheetLabels {
    /// Title.
    pub title: String,
    /// "Count date".
    pub date: String,
    /// Column headers.
    pub item: String,
    /// Description header.
    pub description: String,
    /// Unit header.
    pub unit: String,
    /// On hand header.
    pub on_hand: String,
    /// Counted header.
    pub counted: String,
    /// "Counted by" line.
    pub counted_by: String,
}

/// A count sheet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CountSheetData {
    /// The count date (formatted).
    pub date: String,
    /// Print the on-hand column.
    pub show_on_hand: bool,
    /// Rows, in the screen's order.
    pub lines: Vec<CountSheetLine>,
    /// Fixed words.
    pub labels: CountSheetLabels,
}

/// The count sheet as a template document.
#[must_use]
pub fn form(d: &CountSheetData) -> FormDoc {
    let l = &d.labels;
    let mut columns = vec![
        Column::text("item", &l.item, 50.0),
        Column::text(
            "description",
            &l.description,
            if d.show_on_hand { 60.0 } else { 86.0 },
        ),
        Column::text("unit", &l.unit, 18.0),
    ];
    if d.show_on_hand {
        columns.push(Column::number("on_hand", &l.on_hand, 26.0));
    }
    columns.push(Column::text("counted", &l.counted, 36.5));
    FormDoc {
        title: l.title.clone(),
        fields: vec![
            (l.date.clone(), d.date.clone()),
            (l.counted_by.clone(), "______________________".into()),
        ],
        columns,
        rows: d
            .lines
            .iter()
            .map(|x| {
                let mut r = vec![x.item.clone(), x.description.clone(), x.unit.clone()];
                if d.show_on_hand {
                    r.push(x.on_hand.clone());
                }
                r.push("________".into());
                r
            })
            .collect(),
        ..FormDoc::default()
    }
}

/// Lay out a count sheet.
///
/// # Errors
/// As [`layout_form`].
pub fn layout(d: &CountSheetData, style: &FormStyle) -> Result<Document, PrintError> {
    layout_form(&form(d), style)
}

#[cfg(test)]
mod tests {
    use super::super::tests::style;
    use super::*;
    use crate::layout::resolve;

    fn data(show: bool) -> CountSheetData {
        CountSheetData {
            date: "11/04/2026".into(),
            show_on_hand: show,
            lines: vec![CountSheetLine {
                item: "Hardwood Mulch Bag".into(),
                description: "2 cu ft bag".into(),
                unit: "bag".into(),
                on_hand: "120".into(),
            }],
            labels: CountSheetLabels {
                title: "Physical Inventory Count".into(),
                date: "Count date".into(),
                item: "Item".into(),
                description: "Description".into(),
                unit: "Unit".into(),
                on_hand: "On hand".into(),
                counted: "Counted".into(),
                counted_by: "Counted by".into(),
            },
        }
    }

    #[test]
    fn a_count_sheet_leaves_counted_blank_and_on_hand_optional() {
        let p = &resolve(&layout(&data(true), &style()).unwrap()).unwrap()[0];
        let t = |tag: &str| {
            p.runs_of(tag)
                .next()
                .map(|r| r.text.clone())
                .unwrap_or_default()
        };
        assert_eq!(t("title"), "Physical Inventory Count");
        assert_eq!(t("row0.item"), "Hardwood Mulch Bag");
        assert_eq!(t("row0.on_hand"), "120");
        assert!(t("row0.counted").starts_with('_'));
        let blind = &resolve(&layout(&data(false), &style()).unwrap()).unwrap()[0];
        assert!(blind.runs_of("row0.on_hand").next().is_none());
    }
}
