//! A/R Aging Summary (U-142, exact today): one row per client plus the
//! "(no client)" row, the five buckets and the total, and the engine's
//! company-wide buckets and A/R total as the grand line (never a sum of
//! rows).

use cl_core::CalendarDate;
use cl_core::report::{
    AgingBucket, Basis, ColumnKind, Drill, ReportDates, ReportMeta, ReportTable, RowKind, RowPath,
};

use super::{amount, col, name_or, row};
use crate::resources::customer_statements::AgingBuckets;
use crate::resources::reports::ArAgingSummary;

pub(crate) const CAPTIONS: [&str; 6] = [
    "@reports.col.current",
    "@reports.col.days_1_30",
    "@reports.col.days_31_60",
    "@reports.col.days_61_90",
    "@reports.col.over_90",
    "@reports.col.total",
];

fn cells(b: &AgingBuckets, total: cl_core::Money) -> Vec<cl_core::report::Cell> {
    vec![
        amount(b.current),
        amount(b.days_1_30),
        amount(b.days_31_60),
        amount(b.days_61_90),
        amount(b.over_90),
        amount(total),
    ]
}

/// The summary as a table, as of `as_of`.
#[must_use]
pub fn summary(r: &ArAgingSummary, as_of: CalendarDate) -> ReportTable {
    let columns = AgingBucket::KEYS
        .iter()
        .zip(CAPTIONS)
        .map(|(k, c)| col(k, c, ColumnKind::Amount))
        .collect();
    let mut rows = Vec::with_capacity(r.customers.len() + 1);
    for (i, c) in r.customers.iter().enumerate() {
        let key = c
            .contact_id
            .map_or_else(|| "none".to_string(), |id| id.to_string());
        // The exact buckets (engine PR-B, Q-A07c), so the rows add up to
        // the exact grand line.
        let (buckets, total) = c.shown();
        let mut x = row(
            RowKind::Detail,
            1,
            RowPath::of(["clients", key.as_str()]),
            name_or(c.name.as_deref(), "@reports.row.no_client"),
            cells(buckets, total),
        );
        // Keep the engine's order; the index makes paths unique even if a
        // name repeats.
        x.path = x.path.child(i.to_string());
        x.drill = Some(Drill::Aging {
            contact: c.contact_id,
            bucket: AgingBucket::Total,
            as_of,
        });
        rows.push(x);
    }
    rows.push(row(
        RowKind::Grand,
        0,
        RowPath::of(["total"]),
        "@reports.row.total".into(),
        cells(&r.buckets_exact, r.total_exact),
    ));
    ReportTable {
        columns,
        rows,
        meta: ReportMeta {
            basis: None::<Basis>,
            dates: Some(ReportDates::AsOf(as_of)),
            ..ReportMeta::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ArAgingSummary {
        serde_json::from_str(include_str!(
            "../../../../tests/fixtures/reports/ar_aging_summary.json"
        ))
        .expect("fixture")
    }

    #[test]
    fn rows_and_the_engine_total_keep_the_exact_strings() {
        let as_of = CalendarDate::from_ymd(2026, 10, 31).expect("d");
        let t = summary(&sample(), as_of);
        assert_eq!(t.columns.len(), 6);
        let csv = t.to_canonical_csv();
        assert_eq!(
            csv,
            "kind,level,path,caption,current,days_1_30,days_31_60,days_61_90,over_90,total\n\
             detail,1,clients/7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a60/0,Harbor Dental,1200,0,350.5,0,0,1550.5\n\
             detail,1,clients/none/1,@reports.row.no_client,0.1,0.2,0,0,0,0.3\n\
             grand,0,total,@reports.row.total,1200.1,0.2,350.5,0,0,1550.8\n"
        );
        // A bucket cell drills to that client and bucket.
        let cell = t.rows[0].drill.as_ref().expect("drill").at(&t.columns[2]);
        assert!(matches!(
            cell,
            Drill::Aging {
                bucket: AgingBucket::Days31To60,
                contact: Some(_),
                ..
            }
        ));
        assert!(t.rows[2].drill.is_none(), "the grand line has no drill");
    }

    #[test]
    fn a_clients_exact_buckets_win_over_the_legacy_ones() {
        // Engine PR-B (Q-A07c): customers[] carry bucketsExact/totalExact.
        let mut v: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/reports/ar_aging_summary.json"
        ))
        .expect("fixture");
        v["customers"][1]["bucketsExact"] = serde_json::json!({"current": "0.1049",
            "days1to30": "0.2000", "days31to60": "0.0000", "days61to90": "0.0000",
            "over90": "0.0000"});
        v["customers"][1]["totalExact"] = serde_json::json!("0.3049");
        let r: ArAgingSummary = serde_json::from_value(v).expect("summary");
        let t = summary(&r, CalendarDate::from_ymd(2026, 10, 31).expect("d"));
        let csv = t.to_canonical_csv();
        assert!(
            csv.contains("clients/none/1,@reports.row.no_client,0.1049,0.2,0,0,0,0.3049\n"),
            "{csv}"
        );
        // Without them (an older engine) the legacy buckets show.
        assert!(
            summary(&sample(), CalendarDate::from_ymd(2026, 10, 31).expect("d"))
                .to_canonical_csv()
                .contains("@reports.row.no_client,0.1,0.2,0,0,0,0.3\n")
        );
    }
}
