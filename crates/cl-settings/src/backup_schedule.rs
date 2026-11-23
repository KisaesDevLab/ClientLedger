//! Scheduled backups and their history (P34, Q-42, Q-P34k).
//!
//! Kept per company in `companies.json` next to the backup history. Every
//! field here is read leniently: a value an older or newer build cannot
//! read becomes "none" instead of failing the whole file, because
//! [`crate::RecentCompanies::parse`] starts an empty list on any error and
//! the next save would lose every company's history (review note B1).
//!
//! Amounts never appear here except the test restore's trial-balance
//! totals, which are the engine's exact decimal strings (`Money`), as
//! [`crate::TransferTotals`] keeps them.

use std::path::PathBuf;

use cl_core::Timestamp;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Runs remembered per company (newest last).
pub const SCHEDULE_RUNS: usize = 50;
/// Default number of newest scheduled backups kept.
pub const DEFAULT_KEEP_LAST: usize = 10;
/// Default number of months that keep their newest scheduled backup.
pub const DEFAULT_KEEP_MONTHS: u32 = 12;

/// A local wall-clock time of day (`HH:MM`, 24-hour).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeOfDay {
    hour: u8,
    minute: u8,
}

impl TimeOfDay {
    /// `hour` 0–23, `minute` 0–59.
    #[must_use]
    pub fn new(hour: u8, minute: u8) -> Option<TimeOfDay> {
        (hour < 24 && minute < 60).then_some(TimeOfDay { hour, minute })
    }

    /// The hour (0–23).
    #[must_use]
    pub fn hour(self) -> u8 {
        self.hour
    }

    /// The minute (0–59).
    #[must_use]
    pub fn minute(self) -> u8 {
        self.minute
    }

    /// Parses `H:MM` or `HH:MM` (24-hour).
    #[must_use]
    pub fn parse(s: &str) -> Option<TimeOfDay> {
        let (h, m) = s.trim().split_once(':')?;
        if h.is_empty() || h.len() > 2 || m.len() != 2 {
            return None;
        }
        TimeOfDay::new(h.parse().ok()?, m.parse().ok()?)
    }

    /// `HH:MM`.
    #[must_use]
    pub fn text(self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }
}

impl Default for TimeOfDay {
    /// 6:00 p.m., after a typical working day.
    fn default() -> Self {
        TimeOfDay {
            hour: 18,
            minute: 0,
        }
    }
}

impl Serialize for TimeOfDay {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.text())
    }
}

impl<'de> Deserialize<'de> for TimeOfDay {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        TimeOfDay::parse(&s).ok_or_else(|| serde::de::Error::custom("not a time of day"))
    }
}

/// How often a schedule runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "every")]
pub enum Frequency {
    /// Every day.
    #[default]
    Daily,
    /// Once a week on `weekday` (0 = Monday … 6 = Sunday).
    Weekly {
        /// 0 = Monday … 6 = Sunday.
        weekday: u8,
    },
    /// Once a month on `day` (1–28, so every month has it).
    Monthly {
        /// 1–28.
        day: u8,
    },
}

impl Frequency {
    /// True when the numbers are in range.
    #[must_use]
    pub fn is_valid(self) -> bool {
        match self {
            Frequency::Daily => true,
            Frequency::Weekly { weekday } => weekday < 7,
            Frequency::Monthly { day } => (1..=28).contains(&day),
        }
    }
}

/// A company's backup schedule (Q-42: any folder the owner picks).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupSchedule {
    /// Off keeps the settings without running.
    pub enabled: bool,
    /// Daily, weekly or monthly.
    pub frequency: Frequency,
    /// Local time of day.
    pub at: TimeOfDay,
    /// Where the files go. Never replaced by another folder when it is
    /// missing (an unplugged drive fails the run instead).
    pub folder: PathBuf,
    /// Newest scheduled backups kept.
    pub keep_last: usize,
    /// Months that keep their newest scheduled backup.
    pub keep_months: u32,
    /// Test-restore each scheduled backup after it is written.
    pub test_each: bool,
    /// When the schedule was turned on or changed: slots before it never
    /// count as missed (Q-P34ba).
    pub since: Option<Timestamp>,
}

impl Default for BackupSchedule {
    fn default() -> Self {
        BackupSchedule {
            enabled: false,
            frequency: Frequency::Daily,
            at: TimeOfDay::default(),
            folder: PathBuf::new(),
            keep_last: DEFAULT_KEEP_LAST,
            keep_months: DEFAULT_KEEP_MONTHS,
            test_each: false,
            since: None,
        }
    }
}

impl BackupSchedule {
    /// True when it should run: on, a folder named, a valid frequency.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.enabled && !self.folder.as_os_str().is_empty() && self.frequency.is_valid()
    }
}

/// How one scheduled slot ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum RunOutcome {
    /// Written and verified.
    Done {
        /// SHA-256 of the file.
        sha256: String,
    },
    /// Tried and failed (`FOLDER_UNREACHABLE`, `NO_PASSPHRASE`,
    /// `TENANT_BUSY`, the engine's job code, …). The slot is used up; the
    /// next slot tries again.
    Failed {
        /// A code, never a path or a name.
        code: String,
    },
    /// Not tried (the app was locked, nobody allowed was signed in, the
    /// engine refused the user). The slot stays due as a catch-up.
    Skipped {
        /// A code.
        reason: String,
    },
}

impl RunOutcome {
    /// Done and Failed use up their slot; Skipped does not (Q-P34bb).
    #[must_use]
    pub fn consumes_slot(&self) -> bool {
        !matches!(self, RunOutcome::Skipped { .. })
    }
}

/// One scheduled slot's run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleRun {
    /// The slot it was for.
    pub slot: Timestamp,
    /// When it ran (or was skipped).
    pub at: Timestamp,
    /// What happened.
    #[serde(flatten)]
    pub outcome: RunOutcome,
}

/// Why a backup file was made (shown in History).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupTrigger {
    /// Back Up Company.
    Manual,
    /// The schedule, on time.
    Scheduled,
    /// The schedule, catching up a slot missed while the app was closed.
    Missed,
    /// Closing or switching company (`backup_on_close`).
    OnClose,
    /// Before Verify Data fixed something (Q-170).
    BeforeFix,
}

/// The test restore's trial balance, exact (engine decimal strings).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestTrialBalance {
    /// Every row equal.
    pub rows_equal: bool,
    /// Debit and credit totals equal.
    pub totals_equal: bool,
    /// Debits in the backup.
    pub expected_debit: cl_core::Money,
    /// Credits in the backup.
    pub expected_credit: cl_core::Money,
    /// Debits after the trial restore.
    pub actual_debit: cl_core::Money,
    /// Credits after the trial restore.
    pub actual_credit: cl_core::Money,
}

/// The last test restore of a backup file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupTest {
    /// When.
    pub at: Timestamp,
    /// Restores cleanly.
    pub passed: bool,
    /// Why not (a code; plain words are the UI's).
    #[serde(default)]
    pub reason: Option<String>,
    /// The trial-balance comparison, when the engine got that far.
    #[serde(default)]
    pub tb: Option<TestTrialBalance>,
}

/// `Option<T>` that reads anything it cannot parse as `None`.
pub(crate) fn lenient_opt<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value::<Option<T>>(v).unwrap_or(None))
}

/// `Vec<T>` that drops the items it cannot parse.
pub(crate) fn lenient_vec<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|i| serde_json::from_value(i).ok())
            .collect(),
        _ => Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_of_day_parses_and_prints() {
        assert_eq!(TimeOfDay::parse("6:05"), TimeOfDay::new(6, 5));
        assert_eq!(
            TimeOfDay::parse("23:59").map(TimeOfDay::text),
            Some("23:59".into())
        );
        assert_eq!(TimeOfDay::parse("24:00"), None);
        assert_eq!(TimeOfDay::parse("7:5"), None);
        assert_eq!(TimeOfDay::parse("nope"), None);
        assert_eq!(TimeOfDay::default().text(), "18:00");
    }

    #[test]
    fn schedule_defaults_keep_ten_and_twelve_months() {
        let s: BackupSchedule = serde_json::from_str("{}").expect("empty");
        assert_eq!(s.keep_last, DEFAULT_KEEP_LAST);
        assert_eq!(s.keep_months, DEFAULT_KEEP_MONTHS);
        assert!(!s.test_each);
        assert!(!s.is_active());
        let s = BackupSchedule {
            enabled: true,
            folder: PathBuf::from("D:/Backups"),
            frequency: Frequency::Monthly { day: 29 },
            ..BackupSchedule::default()
        };
        assert!(!s.is_active(), "day 29 is not offered");
    }

    #[test]
    fn frequency_and_outcome_wire_shapes() {
        let v = serde_json::to_value(Frequency::Weekly { weekday: 4 }).expect("json");
        assert_eq!(v, serde_json::json!({"every": "weekly", "weekday": 4}));
        let run = ScheduleRun {
            slot: Timestamp::from_unix_millis(0).expect("ts"),
            at: Timestamp::from_unix_millis(1).expect("ts"),
            outcome: RunOutcome::Failed {
                code: "FOLDER_UNREACHABLE".into(),
            },
        };
        let v = serde_json::to_value(&run).expect("json");
        assert_eq!(v["outcome"], "failed");
        assert_eq!(v["code"], "FOLDER_UNREACHABLE");
        let back: ScheduleRun = serde_json::from_value(v).expect("back");
        assert_eq!(back, run);
        assert!(run.outcome.consumes_slot());
        assert!(
            !RunOutcome::Skipped {
                reason: "LOCKED".into()
            }
            .consumes_slot()
        );
    }
}
