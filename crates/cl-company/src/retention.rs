//! Backup retention (P08 step 5): keep the last `keep_last` files plus the
//! newest file of each of the last `keep_months` calendar months. Pure, so
//! the rule is unit-tested without touching a disk.

use std::path::PathBuf;

use chrono::Datelike;
use cl_core::Timestamp;

/// Default: the last 10 backups.
pub const KEEP_LAST: usize = 10;
/// Default: one per month for 12 months.
pub const KEEP_MONTHS: u32 = 12;

fn month_index(t: Timestamp) -> i64 {
    let d = t.as_utc();
    i64::from(d.year()) * 12 + i64::from(d.month()) - 1
}

/// The files to delete, given every backup file of one company with its
/// creation time. Files are never deleted when the input is empty, and the
/// newest file is always kept.
#[must_use]
pub fn prune_plan(
    files: &[(PathBuf, Timestamp)],
    now: Timestamp,
    keep_last: usize,
    keep_months: u32,
) -> Vec<PathBuf> {
    let mut sorted: Vec<&(PathBuf, Timestamp)> = files.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let now_month = month_index(now);
    let oldest_month = now_month - i64::from(keep_months) + 1;
    let mut kept_months = std::collections::HashSet::new();
    let mut delete = Vec::new();
    for (i, (path, at)) in sorted.iter().enumerate() {
        if i < keep_last.max(1) {
            kept_months.insert(month_index(*at));
            continue;
        }
        let m = month_index(*at);
        if m >= oldest_month && m <= now_month && kept_months.insert(m) {
            continue;
        }
        delete.push(path.clone());
    }
    delete
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(y: i32, m: u32, d: u32) -> Timestamp {
        Timestamp::parse_wire(&format!("{y:04}-{m:02}-{d:02}T12:00:00.000Z")).expect("ts")
    }

    fn file(n: usize, at: Timestamp) -> (PathBuf, Timestamp) {
        (PathBuf::from(format!("b{n}.clbackup")), at)
    }

    #[test]
    fn keeps_last_ten_and_one_per_month_for_twelve_months() {
        let now = ts(2026, 11, 4);
        let mut files = Vec::new();
        // 15 daily files in late October / early November.
        for i in 0..15u32 {
            files.push(file(i as usize, ts(2026, 10, 20 + (i % 10))));
        }
        // Two per month back to January.
        let mut n = 100;
        for m in 1..=9 {
            files.push(file(n, ts(2026, m, 3)));
            files.push(file(n + 1, ts(2026, m, 20)));
            n += 2;
        }
        // Old ones from last year.
        files.push(file(200, ts(2025, 10, 1)));
        files.push(file(201, ts(2025, 6, 1)));
        let delete = prune_plan(&files, now, KEEP_LAST, KEEP_MONTHS);
        let kept: Vec<&PathBuf> = files
            .iter()
            .map(|(p, _)| p)
            .filter(|p| !delete.contains(p))
            .collect();
        // 10 most recent + one per month for Jan..Sep (Oct is covered by
        // the ten) = 19 kept; 2025 files go.
        assert_eq!(kept.len(), 19, "{kept:?}");
        assert!(delete.contains(&PathBuf::from("b200.clbackup")));
        assert!(delete.contains(&PathBuf::from("b201.clbackup")));
        for m in 1..=9u32 {
            let month_kept = files
                .iter()
                .filter(|(p, t)| t.as_utc().month() == m && !delete.contains(p))
                .count();
            assert_eq!(month_kept, 1, "month {m}");
        }
    }

    #[test]
    fn small_sets_keep_everything_and_empty_is_empty() {
        let now = ts(2026, 11, 4);
        assert_eq!(prune_plan(&[], now, KEEP_LAST, KEEP_MONTHS).len(), 0);
        let files = vec![file(1, ts(2026, 1, 1)), file(2, ts(2024, 1, 1))];
        assert_eq!(prune_plan(&files, now, KEEP_LAST, KEEP_MONTHS).len(), 0);
        let files: Vec<_> = (0..3u32)
            .map(|i| file(i as usize, ts(2024, 1, 1 + i)))
            .collect();
        let delete = prune_plan(&files, now, 1, 0);
        assert_eq!(delete.len(), 2);
        assert!(
            !delete.contains(&PathBuf::from("b2.clbackup")),
            "newest kept"
        );
    }
}
