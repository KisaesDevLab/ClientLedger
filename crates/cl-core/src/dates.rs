//! Calendar dates and timestamps.
//!
//! - [`CalendarDate`] is a plain calendar day with no time zone: the engine's
//!   `date` columns (`txnDate`, `lockDate`, ...). Its wire form is strictly
//!   `YYYY-MM-DD`, so a date can never shift by a day through a time zone.
//! - [`Timestamp`] is an instant in UTC: the engine's `timestamptz` columns,
//!   sent as RFC 3339 (`2026-10-15T14:03:07.123Z`). The engine runs on
//!   JavaScript `Date`, which keeps milliseconds only, so `PostgreSQL`'s
//!   microseconds are already gone when a value reaches us.
//!
//! There is no clock here: "today" and "now" are always passed in by the
//! caller (chrono is built without its `clock` feature).

use chrono::{DateTime, Datelike, Days, Months, NaiveDate, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

pub use chrono::Weekday;

/// Why text or numbers are not a valid date or timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DateError {
    /// Not `YYYY-MM-DD` (or not RFC 3339 for a timestamp).
    #[error("not a date in YYYY-MM-DD form")]
    Format,
    /// The right shape but no such day (`2026-02-29`, `2026-13-01`).
    #[error("no such calendar day")]
    NoSuchDay,
    /// Outside 1900-01-01 ..= 2199-12-31.
    #[error("date is outside the supported range")]
    OutOfRange,
    /// Not an RFC 3339 timestamp with `Z` or an offset.
    #[error("not an RFC 3339 timestamp")]
    TimestampFormat,
}

/// A calendar day between 1900-01-01 and 2199-12-31.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CalendarDate(NaiveDate);

const MIN_YEAR: i32 = 1900;
const MAX_YEAR: i32 = 2199;

impl CalendarDate {
    /// The first supported day, 1900-01-01.
    #[must_use]
    pub fn min() -> CalendarDate {
        CalendarDate(ymd_or_epoch(MIN_YEAR, 1, 1))
    }

    /// The last supported day, 2199-12-31.
    #[must_use]
    pub fn max() -> CalendarDate {
        CalendarDate(ymd_or_epoch(MAX_YEAR, 12, 31))
    }

    /// The day `year-month-day`.
    ///
    /// # Errors
    /// [`DateError::NoSuchDay`] for an impossible day,
    /// [`DateError::OutOfRange`] outside the supported years.
    pub fn from_ymd(year: i32, month: u32, day: u32) -> Result<CalendarDate, DateError> {
        if !(MIN_YEAR..=MAX_YEAR).contains(&year) {
            return Err(DateError::OutOfRange);
        }
        NaiveDate::from_ymd_opt(year, month, day)
            .map(CalendarDate)
            .ok_or(DateError::NoSuchDay)
    }

    /// Wrap a chrono date (checked against the supported range).
    ///
    /// # Errors
    /// [`DateError::OutOfRange`].
    pub fn from_naive(d: NaiveDate) -> Result<CalendarDate, DateError> {
        if (MIN_YEAR..=MAX_YEAR).contains(&d.year()) {
            Ok(CalendarDate(d))
        } else {
            Err(DateError::OutOfRange)
        }
    }

    /// Strict `YYYY-MM-DD` (exactly ten characters, ASCII digits).
    ///
    /// # Errors
    /// [`DateError::Format`] for any other shape (`2026-1-5`, a time part,
    /// spaces); otherwise as [`CalendarDate::from_ymd`].
    pub fn parse_wire(s: &str) -> Result<CalendarDate, DateError> {
        let b = s.as_bytes();
        let shape_ok = b.len() == 10
            && b.iter().enumerate().all(|(i, c)| {
                if i == 4 || i == 7 {
                    *c == b'-'
                } else {
                    c.is_ascii_digit()
                }
            });
        if !shape_ok {
            return Err(DateError::Format);
        }
        let num = |r: std::ops::Range<usize>| s.get(r).and_then(|x| x.parse::<u32>().ok());
        let (Some(year), Some(month), Some(day)) = (num(0..4), num(5..7), num(8..10)) else {
            return Err(DateError::Format);
        };
        let year = i32::try_from(year).map_err(|_| DateError::Format)?;
        CalendarDate::from_ymd(year, month, day)
    }

    /// `YYYY-MM-DD`.
    #[must_use]
    pub fn to_wire(self) -> String {
        self.0.format("%Y-%m-%d").to_string()
    }

    /// The year.
    #[must_use]
    pub fn year(self) -> i32 {
        self.0.year()
    }

    /// The month, 1–12.
    #[must_use]
    pub fn month(self) -> u32 {
        self.0.month()
    }

    /// The day of the month, 1–31.
    #[must_use]
    pub fn day(self) -> u32 {
        self.0.day()
    }

    /// The day of the week.
    #[must_use]
    pub fn weekday(self) -> Weekday {
        self.0.weekday()
    }

    /// The chrono value (read-only view).
    #[must_use]
    pub fn as_naive(self) -> NaiveDate {
        self.0
    }

    /// `self + days` (negative goes back).
    ///
    /// # Errors
    /// [`DateError::OutOfRange`] when the result leaves the supported range.
    pub fn add_days(self, days: i64) -> Result<CalendarDate, DateError> {
        let n = days.unsigned_abs();
        let d = if days >= 0 {
            self.0.checked_add_days(Days::new(n))
        } else {
            self.0.checked_sub_days(Days::new(n))
        };
        d.ok_or(DateError::OutOfRange)
            .and_then(CalendarDate::from_naive)
    }

    /// `self + months`, clamped to the last day of a shorter month
    /// (Jan 31 + 1 month = Feb 28 or 29).
    ///
    /// # Errors
    /// [`DateError::OutOfRange`] when the result leaves the supported range.
    pub fn add_months(self, months: i32) -> Result<CalendarDate, DateError> {
        let n = months.unsigned_abs();
        let d = if months >= 0 {
            self.0.checked_add_months(Months::new(n))
        } else {
            self.0.checked_sub_months(Months::new(n))
        };
        d.ok_or(DateError::OutOfRange)
            .and_then(CalendarDate::from_naive)
    }

    /// Days from `self` to `other` (negative when `other` is earlier).
    #[must_use]
    pub fn days_until(self, other: CalendarDate) -> i64 {
        other.0.signed_duration_since(self.0).num_days()
    }

    /// The first day of this month.
    #[must_use]
    pub fn start_of_month(self) -> CalendarDate {
        CalendarDate(self.0.with_day(1).unwrap_or(self.0))
    }

    /// The last day of this month.
    #[must_use]
    pub fn end_of_month(self) -> CalendarDate {
        let first = self.start_of_month().0;
        // Day 1 + 1 month is always a valid day; -1 day is the month's end.
        let end = first
            .checked_add_months(Months::new(1))
            .and_then(|d| d.pred_opt())
            .unwrap_or(NaiveDate::MAX);
        if end.year() > MAX_YEAR {
            // December 2199: the next month is outside the range, but the
            // last day of December is not.
            return CalendarDate(ymd_or_epoch(MAX_YEAR, 12, 31));
        }
        CalendarDate(end)
    }

    /// The first day of this calendar quarter (Jan, Apr, Jul, Oct).
    #[must_use]
    pub fn start_of_quarter(self) -> CalendarDate {
        let m = (self.month() - 1) / 3 * 3 + 1;
        CalendarDate(ymd_or_epoch(self.year(), m, 1))
    }

    /// The last day of this calendar quarter.
    #[must_use]
    pub fn end_of_quarter(self) -> CalendarDate {
        let m = (self.month() - 1) / 3 * 3 + 3;
        CalendarDate(ymd_or_epoch(self.year(), m, 1)).end_of_month()
    }

    /// January 1 of this year.
    #[must_use]
    pub fn start_of_year(self) -> CalendarDate {
        CalendarDate(ymd_or_epoch(self.year(), 1, 1))
    }

    /// December 31 of this year.
    #[must_use]
    pub fn end_of_year(self) -> CalendarDate {
        CalendarDate(ymd_or_epoch(self.year(), 12, 31))
    }

    /// The most recent `first` weekday on or before `self` (for example the
    /// Sunday that starts this week).
    ///
    /// # Errors
    /// [`DateError::OutOfRange`] in the first days of 1900.
    pub fn week_start(self, first: Weekday) -> Result<CalendarDate, DateError> {
        let back = (7 + self.weekday().num_days_from_sunday() - first.num_days_from_sunday()) % 7;
        self.add_days(-i64::from(back))
    }

    /// True in a leap year.
    #[must_use]
    pub fn is_leap_year(self) -> bool {
        self.0.leap_year()
    }
}

/// A day that is known to exist (callers pass valid months and day 1, 28..31
/// checked values); falls back to the Unix epoch instead of panicking.
fn ymd_or_epoch(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap_or_default()
}

impl fmt::Display for CalendarDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_wire())
    }
}

impl fmt::Debug for CalendarDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CalendarDate({})", self.to_wire())
    }
}

impl Serialize for CalendarDate {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_wire())
    }
}

impl<'de> Deserialize<'de> for CalendarDate {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = d.deserialize_str(StrVisitor("a YYYY-MM-DD date string"))?;
        CalendarDate::parse_wire(&s).map_err(serde::de::Error::custom)
    }
}

/// An instant in UTC.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Timestamp(DateTime<Utc>);

impl Timestamp {
    /// Parse RFC 3339 with `Z` or a numeric offset (converted to UTC). The
    /// date and time must be separated by an upper-case `T`.
    ///
    /// # Errors
    /// [`DateError::TimestampFormat`].
    pub fn parse_wire(s: &str) -> Result<Timestamp, DateError> {
        if s.as_bytes().get(10) != Some(&b'T') {
            return Err(DateError::TimestampFormat);
        }
        DateTime::parse_from_rfc3339(s)
            .map(|t| Timestamp(t.with_timezone(&Utc)))
            .map_err(|_| DateError::TimestampFormat)
    }

    /// RFC 3339 in UTC with `Z`: milliseconds (the engine's form,
    /// `2026-10-15T14:03:07.120Z`), or more digits only when the value has
    /// sub-millisecond precision, so nothing is lost.
    #[must_use]
    pub fn to_wire(self) -> String {
        let form = if self.0.timestamp_subsec_nanos().is_multiple_of(1_000_000) {
            SecondsFormat::Millis
        } else {
            SecondsFormat::AutoSi
        };
        self.0.to_rfc3339_opts(form, true)
    }

    /// From milliseconds since 1970-01-01T00:00:00Z (the caller's clock or a
    /// JWT `exp` times 1000).
    ///
    /// # Errors
    /// [`DateError::OutOfRange`] for values chrono cannot represent.
    pub fn from_unix_millis(ms: i64) -> Result<Timestamp, DateError> {
        DateTime::from_timestamp_millis(ms)
            .map(Timestamp)
            .ok_or(DateError::OutOfRange)
    }

    /// Milliseconds since 1970-01-01T00:00:00Z (sub-millisecond part dropped).
    #[must_use]
    pub fn unix_millis(self) -> i64 {
        self.0.timestamp_millis()
    }

    /// The chrono value (read-only view). Display code converts it to the
    /// owner's local time.
    #[must_use]
    pub fn as_utc(self) -> DateTime<Utc> {
        self.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_wire())
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Timestamp({})", self.to_wire())
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_wire())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = d.deserialize_str(StrVisitor("an RFC 3339 timestamp string"))?;
        Timestamp::parse_wire(&s).map_err(serde::de::Error::custom)
    }
}

struct StrVisitor(&'static str);

impl serde::de::Visitor<'_> for StrVisitor {
    type Value = String;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<String, E> {
        Ok(v.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> CalendarDate {
        CalendarDate::parse_wire(s).unwrap()
    }

    #[test]
    fn wire_dates_are_strict() {
        assert_eq!(d("2026-10-15").to_wire(), "2026-10-15");
        assert_eq!(d("2024-02-29").day(), 29);
        assert_eq!(
            CalendarDate::parse_wire("2026-02-29"),
            Err(DateError::NoSuchDay)
        );
        assert_eq!(
            CalendarDate::parse_wire("2026-13-01"),
            Err(DateError::NoSuchDay)
        );
        assert_eq!(
            CalendarDate::parse_wire("2026-00-10"),
            Err(DateError::NoSuchDay)
        );
        assert_eq!(
            CalendarDate::parse_wire("1899-12-31"),
            Err(DateError::OutOfRange)
        );
        assert_eq!(
            CalendarDate::parse_wire("2200-01-01"),
            Err(DateError::OutOfRange)
        );
        for s in [
            "",
            "2026-1-05",
            "2026/01/05",
            "20260105",
            "2026-01-05T00:00:00Z",
            " 2026-01-05",
            "2026-01-5 ",
            "+026-01-05",
            "２０２６-01-05",
        ] {
            assert_eq!(CalendarDate::parse_wire(s), Err(DateError::Format), "{s:?}");
        }
        assert_eq!(CalendarDate::min().to_wire(), "1900-01-01");
        assert_eq!(CalendarDate::max().to_wire(), "2199-12-31");
        assert_eq!(
            CalendarDate::from_ymd(2026, 2, 29),
            Err(DateError::NoSuchDay)
        );
        assert_eq!(
            CalendarDate::from_naive(NaiveDate::from_ymd_opt(1800, 1, 1).unwrap()),
            Err(DateError::OutOfRange)
        );
    }

    #[test]
    fn helpers() {
        let x = d("2024-02-10");
        assert_eq!(x.start_of_month().to_wire(), "2024-02-01");
        assert_eq!(x.end_of_month().to_wire(), "2024-02-29");
        assert_eq!(d("2025-02-10").end_of_month().to_wire(), "2025-02-28");
        assert_eq!(d("2026-08-31").start_of_quarter().to_wire(), "2026-07-01");
        assert_eq!(d("2026-08-31").end_of_quarter().to_wire(), "2026-09-30");
        assert_eq!(d("2026-12-01").end_of_quarter().to_wire(), "2026-12-31");
        assert_eq!(x.start_of_year().to_wire(), "2024-01-01");
        assert_eq!(x.end_of_year().to_wire(), "2024-12-31");
        assert_eq!(d("2199-12-05").end_of_month().to_wire(), "2199-12-31");
        assert_eq!(x.add_days(20).unwrap().to_wire(), "2024-03-01");
        assert_eq!(x.add_days(-41).unwrap().to_wire(), "2023-12-31");
        assert_eq!(
            d("2024-01-31").add_months(1).unwrap().to_wire(),
            "2024-02-29"
        );
        assert_eq!(
            d("2024-03-31").add_months(-1).unwrap().to_wire(),
            "2024-02-29"
        );
        assert_eq!(d("2199-12-31").add_days(1), Err(DateError::OutOfRange));
        assert_eq!(d("1900-01-01").add_months(-1), Err(DateError::OutOfRange));
        assert_eq!(d("2026-01-01").days_until(d("2026-12-31")), 364);
        assert_eq!(d("2026-12-31").days_until(d("2026-01-01")), -364);
        // 2026-10-15 is a Thursday.
        assert_eq!(d("2026-10-15").weekday(), Weekday::Thu);
        assert_eq!(
            d("2026-10-15").week_start(Weekday::Sun).unwrap().to_wire(),
            "2026-10-11"
        );
        assert_eq!(
            d("2026-10-11").week_start(Weekday::Sun).unwrap().to_wire(),
            "2026-10-11"
        );
        assert_eq!(
            d("2026-10-15").week_start(Weekday::Mon).unwrap().to_wire(),
            "2026-10-12"
        );
        assert_eq!(
            d("1900-01-01").week_start(Weekday::Sun),
            Err(DateError::OutOfRange)
        );
        assert!(d("2000-06-01").is_leap_year() && !d("1900-06-01").is_leap_year());
        assert_eq!((x.year(), x.month(), x.day()), (2024, 2, 10));
        assert_eq!(x.as_naive(), NaiveDate::from_ymd_opt(2024, 2, 10).unwrap());
        assert_eq!(format!("{x} {x:?}"), "2024-02-10 CalendarDate(2024-02-10)");
    }

    #[test]
    fn date_serde_is_strings_only() {
        let x = d("2026-10-15");
        assert_eq!(serde_json::to_string(&x).unwrap(), "\"2026-10-15\"");
        assert_eq!(
            serde_json::from_str::<CalendarDate>("\"2026-10-15\"").unwrap(),
            x
        );
        assert!(serde_json::from_str::<CalendarDate>("20261015").is_err());
        assert!(serde_json::from_str::<CalendarDate>("\"2026-10-15T00:00:00.000Z\"").is_err());
    }

    #[test]
    fn timestamps() {
        let t = Timestamp::parse_wire("2026-10-15T14:03:07.120Z").unwrap();
        assert_eq!(t.to_wire(), "2026-10-15T14:03:07.120Z");
        let o = Timestamp::parse_wire("2026-10-15T09:03:07.120-05:00").unwrap();
        assert_eq!(o, t);
        assert_eq!(o.to_wire(), "2026-10-15T14:03:07.120Z");
        let whole = Timestamp::parse_wire("2026-10-15T14:03:07Z").unwrap();
        assert_eq!(whole.to_wire(), "2026-10-15T14:03:07.000Z");
        let micro = Timestamp::parse_wire("2026-10-15T14:03:07.123456Z").unwrap();
        assert_eq!(micro.to_wire(), "2026-10-15T14:03:07.123456Z");
        for s in [
            "2026-10-15",
            "2026-10-15 14:03:07Z",
            "2026-10-15t14:03:07Z",
            "2026-10-15T14:03:07",
            "2026-10-15T14:03:07.120+0500",
            "",
        ] {
            assert_eq!(
                Timestamp::parse_wire(s),
                Err(DateError::TimestampFormat),
                "{s:?}"
            );
        }
        assert_eq!(t.unix_millis(), 1_792_072_987_120);
        assert_eq!(Timestamp::from_unix_millis(1_792_072_987_120).unwrap(), t);
        assert_eq!(
            Timestamp::from_unix_millis(i64::MAX),
            Err(DateError::OutOfRange)
        );
        assert_eq!(t.as_utc().timestamp(), 1_792_072_987);
        assert_eq!(
            serde_json::to_string(&t).unwrap(),
            "\"2026-10-15T14:03:07.120Z\""
        );
        assert_eq!(
            serde_json::from_str::<Timestamp>("\"2026-10-15T14:03:07.120Z\"").unwrap(),
            t
        );
        assert!(serde_json::from_str::<Timestamp>("1791036187120").is_err());
        assert_eq!(format!("{t:?}"), "Timestamp(2026-10-15T14:03:07.120Z)");
        assert_eq!(t.to_string(), t.to_wire());
    }
}
