//! The Template dialog (P15 lane T): name, group, what happens on the
//! dates, how often and when it ends, with [`TemplateDialog::check`], the
//! gate before anything is sent, and [`next_dates`], the dialog's preview.
//!
//! - The preview implements the engine's month rule (Q-P15j): monthly,
//!   quarterly and yearly dates are counted from the anchor (the next date
//!   the owner typed) and clamped to the end of a shorter month, so Jan 31
//!   gives Feb 28/29, Mar 31, Apr 30. It is display only: the engine's
//!   `nextDate` after the save is what the list shows.
//! - Daily Sales and templates made in the online `MyBooks` never offer
//!   "Enter automatically" (Q-P15d, Q-P15m).
//! - No amounts here: a template's amounts live in its form's draft.
//! - [`reason_key`] and [`should_open_due`] are the Templates Due window's
//!   plain words and its open rule (lane C).

use std::fmt;

use cl_core::CalendarDate;

use crate::text::TextKey;

/// The longest template name (characters).
pub const NAME_MAX: usize = 255;
/// The longest group name (characters).
pub const GROUP_NAME_MAX: usize = 100;
/// Most units between dates ("Every N").
pub const INTERVAL_MAX: u16 = 365;
/// Most times a schedule can run ("after N times").
pub const COUNT_MAX: u32 = 9999;
/// How many dates the dialog previews.
pub const PREVIEW_COUNT: usize = 5;

/// What happens on the template's dates (Q-P15b words).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    /// "Remind me".
    #[default]
    Remind,
    /// "Enter automatically".
    Auto,
    /// "Don't remind me" (no schedule).
    Never,
}

impl Mode {
    /// Every mode, in the dialog's order.
    pub const ALL: [Mode; 3] = [Mode::Remind, Mode::Auto, Mode::Never];

    /// The owner's words.
    #[must_use]
    pub const fn key(self) -> TextKey {
        TextKey(match self {
            Mode::Remind => "templates.mode.remind",
            Mode::Auto => "templates.mode.auto",
            Mode::Never => "templates.mode.never",
        })
    }

    /// The control-channel name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Mode::Remind => "remind",
            Mode::Auto => "auto",
            Mode::Never => "never",
        }
    }
}

/// How often (the engine's frequencies).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Frequency {
    /// Every N days.
    Daily,
    /// Every N weeks.
    Weekly,
    /// Every 2 × N weeks.
    Biweekly,
    /// The 1st and the 15th.
    Semimonthly,
    /// Every N months.
    #[default]
    Monthly,
    /// Every N quarters.
    Quarterly,
    /// Every N years.
    Annually,
}

impl Frequency {
    /// Every frequency, in the dialog's order.
    pub const ALL: [Frequency; 7] = [
        Frequency::Daily,
        Frequency::Weekly,
        Frequency::Biweekly,
        Frequency::Semimonthly,
        Frequency::Monthly,
        Frequency::Quarterly,
        Frequency::Annually,
    ];

    /// The dialog's word ("Monthly").
    #[must_use]
    pub const fn key(self) -> TextKey {
        TextKey(match self {
            Frequency::Daily => "templates.freq.daily",
            Frequency::Weekly => "templates.freq.weekly",
            Frequency::Biweekly => "templates.freq.biweekly",
            Frequency::Semimonthly => "templates.freq.semimonthly",
            Frequency::Monthly => "templates.freq.monthly",
            Frequency::Quarterly => "templates.freq.quarterly",
            Frequency::Annually => "templates.freq.annually",
        })
    }

    /// The control-channel name (the engine's word).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Frequency::Daily => "daily",
            Frequency::Weekly => "weekly",
            Frequency::Biweekly => "biweekly",
            Frequency::Semimonthly => "semimonthly",
            Frequency::Monthly => "monthly",
            Frequency::Quarterly => "quarterly",
            Frequency::Annually => "annually",
        }
    }

    /// Months per step for the month-based frequencies.
    const fn months(self) -> Option<i32> {
        match self {
            Frequency::Monthly => Some(1),
            Frequency::Quarterly => Some(3),
            Frequency::Annually => Some(12),
            _ => None,
        }
    }
}

/// When the schedule stops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Ends {
    /// Never.
    #[default]
    Never,
    /// On this date (no later date).
    On(Option<CalendarDate>),
    /// After this many times.
    After(Option<u32>),
}

/// A schedule as the dialog holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Schedule {
    /// How often.
    pub frequency: Frequency,
    /// Every N (1–365).
    pub interval: u16,
    /// The next date.
    pub next_date: CalendarDate,
    /// When it stops.
    pub ends: Ends,
}

/// Where the template goes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum GroupChoice<G> {
    /// No group.
    #[default]
    None,
    /// An existing group.
    Existing(G),
    /// A new group with this name.
    New(String),
}

/// The Template dialog. `G` is the group id type (the UI's engine id).
#[derive(Clone, PartialEq, Eq)]
pub struct TemplateDialog<G> {
    /// The template's name.
    pub name: String,
    /// Its group.
    pub group: GroupChoice<G>,
    /// What happens on the dates.
    pub mode: Mode,
    /// How often.
    pub frequency: Frequency,
    /// Every N (typed).
    pub interval: Option<u16>,
    /// The next date.
    pub next_date: Option<CalendarDate>,
    /// When it stops.
    pub ends: Ends,
    /// "Enter automatically" is offered (not for Daily Sales or a template
    /// made online).
    pub auto_allowed: bool,
}

/// The mode and dates only (the name is owner data).
impl<G: fmt::Debug> fmt::Debug for TemplateDialog<G> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TemplateDialog")
            .field("group", &self.group)
            .field("mode", &self.mode)
            .field("frequency", &self.frequency)
            .field("interval", &self.interval)
            .field("next_date", &self.next_date)
            .field("ends", &self.ends)
            .finish_non_exhaustive()
    }
}

/// Why the dialog can't be saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DialogProblem {
    /// No name.
    NameMissing,
    /// Over 255 characters.
    NameTooLong,
    /// The new group's name is empty or over 100 characters.
    GroupName,
    /// "Enter automatically" where it is not offered.
    AutoNotAllowed,
    /// Every N outside 1–365.
    Interval,
    /// No next date.
    NextDateMissing,
    /// The end date is missing or before the next date.
    EndDate,
    /// The count is missing or outside 1–9999.
    Count,
}

impl DialogProblem {
    /// Plain-language message.
    #[must_use]
    pub const fn key(self) -> TextKey {
        TextKey(match self {
            DialogProblem::NameMissing => "templates.problem.name_missing",
            DialogProblem::NameTooLong => "templates.problem.name_too_long",
            DialogProblem::GroupName => "templates.problem.group_name",
            DialogProblem::AutoNotAllowed => "templates.problem.auto_not_allowed",
            DialogProblem::Interval => "templates.problem.interval",
            DialogProblem::NextDateMissing => "templates.problem.next_date",
            DialogProblem::EndDate => "templates.problem.end_date",
            DialogProblem::Count => "templates.problem.count",
        })
    }

    /// The control-channel name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            DialogProblem::NameMissing => "name_missing",
            DialogProblem::NameTooLong => "name_too_long",
            DialogProblem::GroupName => "group_name",
            DialogProblem::AutoNotAllowed => "auto_not_allowed",
            DialogProblem::Interval => "interval",
            DialogProblem::NextDateMissing => "next_date",
            DialogProblem::EndDate => "end_date",
            DialogProblem::Count => "count",
        }
    }
}

/// A dialog that passed [`TemplateDialog::check`].
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedDialog<G> {
    /// The name, trimmed.
    pub name: String,
    /// The group (a new group's name trimmed).
    pub group: GroupChoice<G>,
    /// `None` for "Don't remind me"; else the mode (`true` = automatic)
    /// and the schedule.
    pub schedule: Option<(bool, Schedule)>,
}

/// Ids and the schedule only.
impl<G: fmt::Debug> fmt::Debug for CheckedDialog<G> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedDialog")
            .field("schedule", &self.schedule)
            .finish_non_exhaustive()
    }
}

impl<G: Clone> TemplateDialog<G> {
    /// A new template's dialog: Remind me, monthly, from `next`.
    #[must_use]
    pub fn new(name: String, next: CalendarDate, auto_allowed: bool) -> TemplateDialog<G> {
        TemplateDialog {
            name,
            group: GroupChoice::None,
            mode: Mode::Remind,
            frequency: Frequency::Monthly,
            interval: Some(1),
            next_date: Some(next),
            ends: Ends::Never,
            auto_allowed,
        }
    }

    /// The modes the dialog offers.
    #[must_use]
    pub fn modes(&self) -> Vec<Mode> {
        Mode::ALL
            .into_iter()
            .filter(|m| self.auto_allowed || *m != Mode::Auto)
            .collect()
    }

    /// The schedule as typed, when it is complete enough to preview.
    #[must_use]
    pub fn schedule(&self) -> Option<Schedule> {
        if self.mode == Mode::Never {
            return None;
        }
        Some(Schedule {
            frequency: self.frequency,
            interval: self.interval.filter(|n| (1..=INTERVAL_MAX).contains(n))?,
            next_date: self.next_date?,
            ends: self.ends,
        })
    }

    /// The checked dialog, or every problem.
    ///
    /// # Errors
    /// See [`DialogProblem`].
    pub fn check(&self) -> Result<CheckedDialog<G>, Vec<DialogProblem>> {
        let mut p = Vec::new();
        let name = self.name.trim();
        if name.is_empty() {
            p.push(DialogProblem::NameMissing);
        } else if name.chars().count() > NAME_MAX {
            p.push(DialogProblem::NameTooLong);
        }
        let group = match &self.group {
            GroupChoice::New(n) => {
                let n = n.trim();
                if n.is_empty() || n.chars().count() > GROUP_NAME_MAX {
                    p.push(DialogProblem::GroupName);
                }
                GroupChoice::New(n.to_string())
            }
            other => other.clone(),
        };
        let mut schedule = None;
        if self.mode != Mode::Never {
            if self.mode == Mode::Auto && !self.auto_allowed {
                p.push(DialogProblem::AutoNotAllowed);
            }
            let interval = self.interval.filter(|n| (1..=INTERVAL_MAX).contains(n));
            if interval.is_none() {
                p.push(DialogProblem::Interval);
            }
            if self.next_date.is_none() {
                p.push(DialogProblem::NextDateMissing);
            }
            match self.ends {
                Ends::Never => {}
                Ends::On(d) => {
                    if d.is_none() || self.next_date.zip(d).is_some_and(|(n, e)| e < n) {
                        p.push(DialogProblem::EndDate);
                    }
                }
                Ends::After(n) => {
                    if !n.is_some_and(|n| (1..=COUNT_MAX).contains(&n)) {
                        p.push(DialogProblem::Count);
                    }
                }
            }
            if let (Some(interval), Some(next_date)) = (interval, self.next_date) {
                schedule = Some((
                    self.mode == Mode::Auto,
                    Schedule {
                        frequency: self.frequency,
                        interval,
                        next_date,
                        ends: self.ends,
                    },
                ));
            }
        }
        if !p.is_empty() {
            return Err(p);
        }
        Ok(CheckedDialog {
            name: name.to_string(),
            group,
            schedule,
        })
    }
}

/// The date `k` steps after the anchor (the engine's `nextAfter`, Q-P15j):
/// month-based steps from the anchor clamped to the month end; day steps
/// exact; semimonthly on the 1st and the 15th.
fn step(s: &Schedule, k: u32) -> Option<CalendarDate> {
    let n = i64::from(s.interval);
    let k64 = i64::from(k);
    match s.frequency {
        Frequency::Daily => s.next_date.add_days(n * k64).ok(),
        Frequency::Weekly => s.next_date.add_days(7 * n * k64).ok(),
        Frequency::Biweekly => s.next_date.add_days(14 * n * k64).ok(),
        Frequency::Semimonthly => {
            let mut d = s.next_date;
            for _ in 0..k {
                d = if d.day() < 15 {
                    CalendarDate::from_ymd(d.year(), d.month(), 15).ok()?
                } else {
                    d.start_of_month().add_months(1).ok()?
                };
            }
            Some(d)
        }
        f => {
            let months = f.months()? * i32::from(s.interval) * i32::try_from(k).ok()?;
            s.next_date.add_months(months).ok()
        }
    }
}

/// The schedule's next `n` dates, from its next date, stopping at its end
/// (display only; the engine's answer after a save is the truth).
#[must_use]
pub fn next_dates(s: &Schedule, n: usize) -> Vec<CalendarDate> {
    let limit = match s.ends {
        Ends::After(Some(c)) => usize::try_from(c).unwrap_or(usize::MAX).min(n),
        _ => n,
    };
    let mut out = Vec::with_capacity(limit);
    for k in 0..u32::try_from(limit).unwrap_or(u32::MAX) {
        let Some(d) = step(s, k) else { break };
        if let Ends::On(Some(end)) = s.ends
            && d > end
        {
            break;
        }
        out.push(d);
    }
    out
}

/// An automatic template with more missed dates than this asks before the
/// catch-up posts them (owner answer 2026-10-24, Q-PR1d).
pub const CATCH_UP_ASK_ABOVE: usize = 3;

/// The dates of `s` on or before `as_of`, from its next date (at most
/// `max`): the missed dates a catch-up run would post. Decision only; the
/// engine's run is the truth.
#[must_use]
pub fn missed_dates(s: &Schedule, as_of: CalendarDate, max: usize) -> Vec<CalendarDate> {
    let mut out = Vec::new();
    let mut k = 0u32;
    while out.len() < max {
        if let Ends::After(Some(c)) = s.ends
            && k >= c
        {
            break;
        }
        let Some(d) = step(s, k) else { break };
        if d > as_of {
            break;
        }
        if let Ends::On(Some(end)) = s.ends
            && d > end
        {
            break;
        }
        out.push(d);
        k += 1;
    }
    out
}

/// True when a catch-up of `missed` dates asks the owner first.
#[must_use]
pub fn catch_up_asks(missed: usize) -> bool {
    missed > CATCH_UP_ASK_ABOVE
}

/// The plain-language "how often" text: a key and its `{n}` / `{day}`
/// values ("Every 2 weeks", "Monthly on the 31st (or last day)").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HowOften {
    /// The text key.
    pub key: TextKey,
    /// Every N.
    pub n: u16,
    /// The day of the month, with its ordinal suffix ("31st").
    pub day: Option<String>,
}

/// "1st", "2nd", "3rd", "4th", "11th", "21st", …
#[must_use]
pub fn ordinal(day: u32) -> String {
    let suffix = match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{day}{suffix}")
}

/// How often `s` runs, in plain words.
#[must_use]
pub fn how_often(s: &Schedule) -> HowOften {
    let one = s.interval <= 1;
    let day = s.next_date.day();
    let (key, with_day) = match (s.frequency, one) {
        (Frequency::Daily, true) => ("templates.how.daily", false),
        (Frequency::Daily, false) => ("templates.how.days", false),
        (Frequency::Weekly, true) => ("templates.how.weekly", false),
        (Frequency::Weekly, false) => ("templates.how.weeks", false),
        (Frequency::Biweekly, true) => ("templates.how.biweekly", false),
        (Frequency::Biweekly, false) => ("templates.how.biweeks", false),
        (Frequency::Semimonthly, _) => ("templates.how.semimonthly", false),
        (Frequency::Monthly, true) if day > 28 => ("templates.how.monthly_end", true),
        (Frequency::Monthly, true) => ("templates.how.monthly", true),
        (Frequency::Monthly, false) if day > 28 => ("templates.how.months_end", true),
        (Frequency::Monthly, false) => ("templates.how.months", true),
        (Frequency::Quarterly, true) => ("templates.how.quarterly", false),
        (Frequency::Quarterly, false) => ("templates.how.quarters", false),
        (Frequency::Annually, true) => ("templates.how.annually", false),
        (Frequency::Annually, false) => ("templates.how.years", false),
    };
    HowOften {
        key: TextKey(key),
        n: s.interval,
        day: with_day.then(|| ordinal(day)),
    }
}

/// How far back an automatic schedule may start (the engine's rule).
pub const AUTO_START_DAYS_BACK: i64 = 366;

/// The earliest next date an automatic schedule may have when it is
/// created, moved or switched to automatic: `today` − 366 days.
#[must_use]
pub fn earliest_auto_start(today: CalendarDate) -> Option<CalendarDate> {
    today.add_days(-AUTO_START_DAYS_BACK).ok()
}

/// The Templates Due wording for an engine skip reason.
#[must_use]
pub fn reason_key(code: &str) -> TextKey {
    TextKey(match code {
        "CLOSED_PERIOD" => "templates_due.reason.closed_period",
        "INVALID_ACCOUNT" => "templates_due.reason.invalid_account",
        "INVALID_CONTACT" => "templates_due.reason.invalid_contact",
        "INVALID_TAG" => "templates_due.reason.invalid_tag",
        "CHECK_NUMBER" => "templates_due.reason.check_number",
        "NEEDS_CHECK" => "templates_due.reason.needs_check",
        "PERMISSION" => "templates_due.reason.permission",
        "CATCH_UP_CAPPED" => "templates_due.reason.catch_up_capped",
        _ => "templates_due.reason.other",
    })
}

/// Templates Due opens after a run that posted something, has due
/// reminders, or skipped something new; older open skips only show a
/// status-bar chip (plan C1).
#[must_use]
pub fn should_open_due(posted: usize, reminders: usize, new_skips: usize) -> bool {
    posted + reminders + new_skips > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn missed_dates_stop_at_today_the_end_and_the_count() {
        let d = |y, m, day| CalendarDate::from_ymd(y, m, day).expect("date");
        let monthly = Schedule {
            frequency: Frequency::Monthly,
            interval: 1,
            next_date: d(2026, 7, 1),
            ends: Ends::Never,
        };
        let four = missed_dates(&monthly, d(2026, 10, 24), 100);
        assert_eq!(
            four,
            vec![d(2026, 7, 1), d(2026, 8, 1), d(2026, 9, 1), d(2026, 10, 1)]
        );
        assert!(catch_up_asks(four.len()), "4 asks first");
        let three = missed_dates(&monthly, d(2026, 9, 30), 100);
        assert_eq!(three.len(), 3);
        assert!(!catch_up_asks(three.len()), "3 posts as today");
        assert_eq!(
            missed_dates(&monthly, d(2026, 6, 30), 100),
            Vec::<CalendarDate>::new()
        );
        let ended = Schedule {
            ends: Ends::On(Some(d(2026, 8, 15))),
            ..monthly
        };
        assert_eq!(missed_dates(&ended, d(2026, 10, 24), 100).len(), 2);
        let counted = Schedule {
            ends: Ends::After(Some(1)),
            ..monthly
        };
        assert_eq!(missed_dates(&counted, d(2026, 10, 24), 100).len(), 1);
        assert_eq!(missed_dates(&monthly, d(2027, 10, 24), 5).len(), 5);
    }

    fn d(y: i32, m: u32, day: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, day).expect("date")
    }

    fn sched(f: Frequency, n: u16, next: CalendarDate) -> Schedule {
        Schedule {
            frequency: f,
            interval: n,
            next_date: next,
            ends: Ends::Never,
        }
    }

    /// The engine's test vectors (`recurring.next-occurrence.test.ts`,
    /// engine plan §6): Jan 31 monthly over 14 months, Feb 29 yearly,
    /// quarterly from May 31, interval 3.
    #[test]
    fn engine_vectors() {
        let jan31 = next_dates(&sched(Frequency::Monthly, 1, d(2026, 1, 31)), 14);
        let want: Vec<CalendarDate> = [
            (2026, 1, 31),
            (2026, 2, 28),
            (2026, 3, 31),
            (2026, 4, 30),
            (2026, 5, 31),
            (2026, 6, 30),
            (2026, 7, 31),
            (2026, 8, 31),
            (2026, 9, 30),
            (2026, 10, 31),
            (2026, 11, 30),
            (2026, 12, 31),
            (2027, 1, 31),
            (2027, 2, 28),
        ]
        .into_iter()
        .map(|(y, m, x)| d(y, m, x))
        .collect();
        assert_eq!(jan31, want, "clamped, then back to the 31st");
        assert_eq!(
            next_dates(&sched(Frequency::Annually, 1, d(2028, 2, 29)), 5),
            vec![
                d(2028, 2, 29),
                d(2029, 2, 28),
                d(2030, 2, 28),
                d(2031, 2, 28),
                d(2032, 2, 29)
            ]
        );
        assert_eq!(
            next_dates(&sched(Frequency::Quarterly, 1, d(2026, 5, 31)), 4),
            vec![
                d(2026, 5, 31),
                d(2026, 8, 31),
                d(2026, 11, 30),
                d(2027, 2, 28)
            ]
        );
        assert_eq!(
            next_dates(&sched(Frequency::Monthly, 3, d(2026, 1, 31)), 3),
            vec![d(2026, 1, 31), d(2026, 4, 30), d(2026, 7, 31)]
        );
        assert_eq!(
            next_dates(&sched(Frequency::Semimonthly, 1, d(2026, 1, 15)), 4),
            vec![d(2026, 1, 15), d(2026, 2, 1), d(2026, 2, 15), d(2026, 3, 1)]
        );
        assert_eq!(
            next_dates(&sched(Frequency::Biweekly, 1, d(2026, 1, 2)), 2),
            vec![d(2026, 1, 2), d(2026, 1, 16)]
        );
        assert_eq!(
            next_dates(&sched(Frequency::Daily, 2, d(2026, 12, 31)), 2),
            vec![d(2026, 12, 31), d(2027, 1, 2)]
        );
        assert_eq!(
            next_dates(&sched(Frequency::Weekly, 1, d(2026, 11, 4)), 2),
            vec![d(2026, 11, 4), d(2026, 11, 11)]
        );
    }

    #[test]
    fn ends_stop_the_preview() {
        let mut s = sched(Frequency::Monthly, 1, d(2026, 9, 15));
        s.ends = Ends::After(Some(3));
        assert_eq!(
            next_dates(&s, 5),
            vec![d(2026, 9, 15), d(2026, 10, 15), d(2026, 11, 15)]
        );
        s.ends = Ends::On(Some(d(2026, 10, 15)));
        assert_eq!(next_dates(&s, 5), vec![d(2026, 9, 15), d(2026, 10, 15)]);
    }

    #[test]
    fn plain_words() {
        let h = how_often(&sched(Frequency::Weekly, 2, d(2026, 11, 4)));
        assert_eq!((h.key.as_str(), h.n), ("templates.how.weeks", 2));
        let h = how_often(&sched(Frequency::Monthly, 1, d(2026, 1, 31)));
        assert_eq!(h.key.as_str(), "templates.how.monthly_end");
        assert_eq!(h.day.as_deref(), Some("31st"));
        let h = how_often(&sched(Frequency::Monthly, 1, d(2026, 1, 15)));
        assert_eq!(h.day.as_deref(), Some("15th"));
        for (n, s) in [
            (1, "1st"),
            (2, "2nd"),
            (3, "3rd"),
            (11, "11th"),
            (22, "22nd"),
        ] {
            assert_eq!(ordinal(n), s);
        }
        for f in Frequency::ALL {
            for n in [1, 2] {
                let h = how_often(&sched(f, n, d(2026, 1, 31)));
                assert!(h.key.as_str().starts_with("templates.how."));
            }
        }
        assert_eq!(
            reason_key("CLOSED_PERIOD").as_str(),
            "templates_due.reason.closed_period"
        );
        assert_eq!(
            reason_key("PERMISSION").as_str(),
            "templates_due.reason.permission"
        );
        assert_eq!(
            reason_key("CATCH_UP_CAPPED").as_str(),
            "templates_due.reason.catch_up_capped"
        );
        assert_eq!(reason_key("NEW").as_str(), "templates_due.reason.other");
        assert_eq!(
            earliest_auto_start(d(2026, 11, 4)),
            Some(d(2025, 11, 3)),
            "366 days back"
        );
        assert!(!should_open_due(0, 0, 0) && should_open_due(0, 0, 1));
    }

    fn dialog() -> TemplateDialog<u8> {
        TemplateDialog::new("Shop rent".into(), d(2026, 12, 1), true)
    }

    #[test]
    fn dialog_rules() {
        assert!(dialog().check().is_ok());
        let mut t = dialog();
        t.name = "   ".into();
        t.interval = Some(0);
        t.next_date = None;
        t.ends = Ends::After(Some(10_000));
        assert_eq!(
            t.check().expect_err("bad"),
            vec![
                DialogProblem::NameMissing,
                DialogProblem::Interval,
                DialogProblem::NextDateMissing,
                DialogProblem::Count
            ]
        );
        let mut t = dialog();
        t.ends = Ends::On(Some(d(2026, 11, 30)));
        t.group = GroupChoice::New(" ".into());
        assert_eq!(
            t.check().expect_err("bad"),
            vec![DialogProblem::GroupName, DialogProblem::EndDate]
        );
        // Daily Sales: no "Enter automatically".
        let mut t = TemplateDialog::<u8>::new("Counter".into(), d(2026, 11, 4), false);
        assert_eq!(t.modes(), vec![Mode::Remind, Mode::Never]);
        t.mode = Mode::Auto;
        assert_eq!(
            t.check().expect_err("auto"),
            vec![DialogProblem::AutoNotAllowed]
        );
        // "Don't remind me" ignores the schedule fields.
        let mut t = dialog();
        t.mode = Mode::Never;
        t.interval = None;
        let c = t.check().expect("ok");
        assert_eq!(c.schedule, None);
        assert_eq!(t.schedule(), None);
        let mut t = dialog();
        t.name = "x".repeat(NAME_MAX + 1);
        assert_eq!(
            t.check().expect_err("long"),
            vec![DialogProblem::NameTooLong]
        );
        let mut t = dialog();
        t.mode = Mode::Auto;
        t.group = GroupChoice::New(" Monthly bills ".into());
        let c = t.check().expect("ok");
        assert_eq!(c.group, GroupChoice::New("Monthly bills".into()));
        assert!(c.schedule.is_some_and(|(auto, _)| auto));
        assert!(!format!("{c:?} {t:?}").contains("Shop"));
        for p in [
            DialogProblem::NameMissing,
            DialogProblem::NameTooLong,
            DialogProblem::GroupName,
            DialogProblem::AutoNotAllowed,
            DialogProblem::Interval,
            DialogProblem::NextDateMissing,
            DialogProblem::EndDate,
            DialogProblem::Count,
        ] {
            assert!(p.key().as_str().starts_with("templates.problem."));
            assert_ne!(p.name(), "");
        }
        for m in Mode::ALL {
            assert!(m.key().as_str().starts_with("templates.mode."));
            assert_ne!(m.name(), "");
        }
        for f in Frequency::ALL {
            assert!(f.key().as_str().starts_with("templates.freq."));
            assert_ne!(f.name(), "");
        }
    }

    /// A table copy of the engine's clamped rule: day = min(anchor day,
    /// days in the target month), counted from the anchor.
    fn engine_month(anchor: CalendarDate, months: i32) -> CalendarDate {
        let total = anchor.year() * 12 + i32::try_from(anchor.month()).expect("m") - 1 + months;
        let (y, m) = (
            total.div_euclid(12),
            u32::try_from(total.rem_euclid(12) + 1).expect("m"),
        );
        let last = CalendarDate::from_ymd(y, m, 1)
            .expect("first")
            .end_of_month()
            .day();
        CalendarDate::from_ymd(y, m, anchor.day().min(last)).expect("date")
    }

    proptest! {
        #[test]
        fn month_preview_matches_the_engine_rule(
            y in 2000i32..2100, m in 1u32..=12, day in 1u32..=31,
            f in prop_oneof![Just(Frequency::Monthly), Just(Frequency::Quarterly),
                Just(Frequency::Annually)],
            n in 1u16..=12,
        ) {
            let first = CalendarDate::from_ymd(y, m, 1).expect("d");
            let day = day.min(first.end_of_month().day());
            let anchor = CalendarDate::from_ymd(y, m, day).expect("d");
            let s = sched(f, n, anchor);
            let got = next_dates(&s, PREVIEW_COUNT);
            prop_assert_eq!(got.len(), PREVIEW_COUNT);
            let step = f.months().expect("months") * i32::from(n);
            for (k, g) in got.iter().enumerate() {
                let k = i32::try_from(k).expect("k");
                prop_assert_eq!(*g, engine_month(anchor, step * k));
            }
            prop_assert!(got.windows(2).all(|w| w[0] < w[1]));
        }
    }
}
