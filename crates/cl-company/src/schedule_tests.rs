use std::path::PathBuf;

use chrono::NaiveDate;
use cl_settings::{CompanyRecord, TimeOfDay};
use proptest::prelude::*;

use super::*;

/// US Central for 2026: CST (−6) with CDT (−5) from 2026-03-08 08:00 UTC
/// to 2026-11-01 07:00 UTC.
struct Central;

const DST_START: i64 = 1_772_956_800_000; // 2026-03-08T08:00:00Z
const DST_END: i64 = 1_793_516_400_000; // 2026-11-01T07:00:00Z

fn offset_ms(t: i64) -> i64 {
    if (DST_START..DST_END).contains(&t) {
        -5 * 3_600_000
    } else {
        -6 * 3_600_000
    }
}

impl LocalZone for Central {
    fn to_local(&self, t: Timestamp) -> NaiveDateTime {
        let ms = t.unix_millis() + offset_ms(t.unix_millis());
        chrono::DateTime::from_timestamp_millis(ms)
            .expect("in range")
            .naive_utc()
    }

    fn instants(&self, local: NaiveDateTime) -> Resolved {
        let wall = local.and_utc().timestamp_millis();
        let mut out: Vec<Timestamp> = [-6i64, -5]
            .iter()
            .map(|h| wall - h * 3_600_000)
            .filter(|t| *t + offset_ms(*t) == wall)
            .filter_map(|t| Timestamp::from_unix_millis(t).ok())
            .collect();
        out.sort();
        out.dedup();
        match out[..] {
            [] => Resolved::Gap,
            [a] => Resolved::One(a),
            [a, b, ..] => Resolved::Two(a, b),
        }
    }
}

fn utc(s: &str) -> Timestamp {
    Timestamp::parse_wire(s).expect("ts")
}

fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, m, d)
        .expect("d")
        .and_hms_opt(h, min, 0)
        .expect("t")
}

fn sched(freq: Frequency, h: u8, m: u8) -> BackupSchedule {
    BackupSchedule {
        enabled: true,
        frequency: freq,
        at: TimeOfDay::new(h, m).expect("time"),
        folder: PathBuf::from("E:/Backups"),
        ..BackupSchedule::default()
    }
}

fn run(slot: Timestamp, outcome: RunOutcome) -> ScheduleRun {
    ScheduleRun {
        slot,
        at: slot,
        outcome,
    }
}

fn done(slot: Timestamp) -> ScheduleRun {
    run(
        slot,
        RunOutcome::Done {
            sha256: "ab".into(),
        },
    )
}

#[test]
fn daily_weekly_and_monthly_slots() {
    let z = Central;
    let s = sched(Frequency::Daily, 18, 0);
    // 2026-11-04 is CST: 18:00 local = 00:00Z next day.
    let now = utc("2026-11-05T01:00:00.000Z");
    assert_eq!(
        latest_slot(&s, &z, now),
        Some(utc("2026-11-05T00:00:00.000Z"))
    );
    assert_eq!(
        next_slot(&s, &z, now),
        Some(utc("2026-11-06T00:00:00.000Z"))
    );
    // Weekly on Friday (4): 2026-11-06 is a Friday.
    let w = sched(Frequency::Weekly { weekday: 4 }, 9, 30);
    assert_eq!(
        next_slot(&w, &z, now),
        Some(utc("2026-11-06T15:30:00.000Z"))
    );
    assert_eq!(
        latest_slot(&w, &z, now),
        Some(utc("2026-10-30T14:30:00.000Z")),
        "last Friday, still daylight time"
    );
    // Monthly on the 28th.
    let m = sched(Frequency::Monthly { day: 28 }, 0, 15);
    assert_eq!(
        latest_slot(&m, &z, now),
        Some(utc("2026-10-28T05:15:00.000Z"))
    );
    assert_eq!(
        next_slot(&m, &z, now),
        Some(utc("2026-11-28T06:15:00.000Z"))
    );
}

#[test]
fn a_dst_gap_moves_to_the_next_valid_minute_and_an_overlap_takes_the_first() {
    let z = Central;
    // 2026-03-08 02:30 does not exist in Central time: 03:00 CDT.
    assert_eq!(z.instants(local(2026, 3, 8, 2, 30)), Resolved::Gap);
    assert_eq!(
        resolve(&z, local(2026, 3, 8, 2, 30)),
        Some(utc("2026-03-08T08:00:00.000Z"))
    );
    let s = sched(Frequency::Daily, 2, 30);
    assert_eq!(
        latest_slot(&s, &z, utc("2026-03-08T12:00:00.000Z")),
        Some(utc("2026-03-08T08:00:00.000Z"))
    );
    // 2026-11-01 01:30 happens twice: the earlier (CDT) instant.
    assert!(matches!(
        z.instants(local(2026, 11, 1, 1, 30)),
        Resolved::Two(..)
    ));
    let s = sched(Frequency::Daily, 1, 30);
    assert_eq!(
        latest_slot(&s, &z, utc("2026-11-01T12:00:00.000Z")),
        Some(utc("2026-11-01T06:30:00.000Z"))
    );
    // And only once that day: the next is the following day.
    assert_eq!(
        next_slot(&s, &z, utc("2026-11-01T06:30:00.000Z")),
        Some(utc("2026-11-02T07:30:00.000Z"))
    );
}

#[test]
fn due_is_one_catch_up_never_stacked() {
    let z = Central;
    let s = sched(Frequency::Daily, 18, 0);
    let now = utc("2026-11-10T15:00:00.000Z");
    // Last run a week ago: only the newest missed slot is due.
    let runs = vec![done(utc("2026-11-03T00:00:00.000Z"))];
    let slot = due(&s, &runs, &z, now).expect("due");
    assert_eq!(slot, utc("2026-11-10T00:00:00.000Z"));
    // After it ran, nothing until the next slot.
    let mut runs = runs;
    runs.push(done(slot));
    assert_eq!(due(&s, &runs, &z, now), None);
    assert_eq!(
        due(&s, &runs, &z, utc("2026-11-11T00:00:00.000Z")),
        Some(utc("2026-11-11T00:00:00.000Z"))
    );
}

#[test]
fn failures_use_the_slot_skips_do_not() {
    let z = Central;
    let s = sched(Frequency::Daily, 18, 0);
    let now = utc("2026-11-10T15:00:00.000Z");
    let slot = utc("2026-11-10T00:00:00.000Z");
    let failed = vec![run(
        slot,
        RunOutcome::Failed {
            code: FOLDER_UNREACHABLE.into(),
        },
    )];
    assert_eq!(due(&s, &failed, &z, now), None, "retry at the next slot");
    let skipped = vec![run(
        slot,
        RunOutcome::Skipped {
            reason: reason::LOCKED.into(),
        },
    )];
    assert_eq!(due(&s, &skipped, &z, now), Some(slot), "still a catch-up");
}

#[test]
fn a_new_schedule_and_a_clock_gone_back() {
    let z = Central;
    let now = utc("2026-11-10T15:00:00.000Z");
    let mut s = sched(Frequency::Daily, 18, 0);
    // Made this morning: last night's slot is not "missed".
    s.since = Some(utc("2026-11-10T14:00:00.000Z"));
    assert_eq!(due(&s, &[], &z, now), None);
    assert_eq!(
        due(&s, &[], &z, utc("2026-11-11T00:00:01.000Z")),
        Some(utc("2026-11-11T00:00:00.000Z"))
    );
    s.since = None;
    // A last run two days in the future (the clock went back): never.
    let runs = vec![done(utc("2026-11-12T00:00:00.000Z"))];
    assert_eq!(
        due(&s, &runs, &z, now),
        Some(utc("2026-11-10T00:00:00.000Z"))
    );
    // Less than a day ahead: treated as run (no double backup).
    let runs = vec![done(utc("2026-11-11T00:00:00.000Z"))];
    assert_eq!(due(&s, &runs, &z, now), None);
    // Off, no folder, bad day: never due.
    s.enabled = false;
    assert_eq!(due(&s, &[], &z, now), None);
}

fn id(n: u8) -> TenantId {
    TenantId::parse(&format!("1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{n:02x}")).expect("id")
}

fn list_with(schedules: &[(u8, bool)]) -> RecentCompanies {
    let mut l = RecentCompanies::default();
    for (n, sample) in schedules {
        let mut c = CompanyRecord::new(id(*n), format!("Co {n}"), cl_settings::ACCENT_PALETTE[0]);
        c.is_sample = *sample;
        c.schedule = Some(sched(Frequency::Daily, 18, 0));
        l.companies.push(c);
    }
    l
}

fn env(signed_in: &str) -> RunnerEnv {
    RunnerEnv {
        signed_in_since: Some(utc(signed_in)),
        ..RunnerEnv::default()
    }
}

#[test]
fn the_runner_skips_workstations_the_sample_and_waits_out_holds() {
    let z = Central;
    let now = utc("2026-11-10T00:00:30.000Z");
    let list = list_with(&[(1, true), (2, false)]);
    let mut r = Runner::new();
    let ws = RunnerEnv {
        workstation: true,
        ..env("2026-11-09T20:00:00.000Z")
    };
    assert_eq!(r.plan(now, ws, &list, &z), Plan::default());
    let held = RunnerEnv {
        hold: true,
        ..env("2026-11-09T20:00:00.000Z")
    };
    assert_eq!(r.plan(now, held, &list, &z), Plan::default());
    let p = r.plan(now, env("2026-11-09T20:00:00.000Z"), &list, &z);
    let job = p.run.expect("runs");
    assert_eq!(job.tenant_id, id(2), "never the practice company");
    assert_eq!(job.trigger, BackupTrigger::Scheduled);
    assert_eq!(p.record, Vec::new());
    // Tombstones never run either.
    let mut gone = list.clone();
    gone.companies[1].deleted_at = Some(now);
    assert_eq!(
        r.plan(now, env("2026-11-09T20:00:00.000Z"), &gone, &z),
        Plan::default()
    );
}

#[test]
fn locked_or_signed_out_records_one_skip_then_catches_up() {
    let z = Central;
    let slot = utc("2026-11-10T00:00:00.000Z");
    let mut list = list_with(&[(1, false)]);
    let mut r = Runner::new();
    let locked = RunnerEnv {
        locked: true,
        ..env("2026-11-09T20:00:00.000Z")
    };
    let p = r.plan(utc("2026-11-10T00:00:30.000Z"), locked, &list, &z);
    assert!(p.run.is_none());
    assert_eq!(p.record.len(), 1);
    assert_eq!(
        p.record[0].1.outcome,
        RunOutcome::Skipped {
            reason: reason::LOCKED.into()
        }
    );
    list.record_schedule_run(id(1), p.record[0].1.clone());
    // The next ticks write nothing more.
    let p = r.plan(utc("2026-11-10T00:01:00.000Z"), locked, &list, &z);
    assert_eq!(p, Plan::default());
    // A fresh runner (restart) reads the history: still one skip.
    let mut r2 = Runner::new();
    assert_eq!(
        r2.plan(utc("2026-11-10T00:01:30.000Z"), locked, &list, &z),
        Plan::default()
    );
    // Unlocked: the slot runs as a catch-up.
    let p = r.plan(
        utc("2026-11-10T00:02:00.000Z"),
        env("2026-11-09T20:00:00.000Z"),
        &list,
        &z,
    );
    let job = p.run.expect("runs");
    assert_eq!(job.slot, slot);
    assert_eq!(job.trigger, BackupTrigger::Missed);
    // Nobody signed in: SIGNED_OUT, once.
    let mut r3 = Runner::new();
    let list = list_with(&[(1, false)]);
    let p = r3.plan(
        utc("2026-11-10T00:00:30.000Z"),
        RunnerEnv::default(),
        &list,
        &z,
    );
    assert_eq!(
        p.record[0].1.outcome,
        RunOutcome::Skipped {
            reason: reason::SIGNED_OUT.into()
        }
    );
}

#[test]
fn a_missed_slot_runs_two_minutes_after_sign_in() {
    let z = Central;
    let list = list_with(&[(1, false)]);
    let mut r = Runner::new();
    let signed = env("2026-11-10T15:00:00.000Z");
    assert!(
        r.plan(utc("2026-11-10T15:01:59.000Z"), signed, &list, &z)
            .run
            .is_none()
    );
    let job = r
        .plan(utc("2026-11-10T15:02:00.000Z"), signed, &list, &z)
        .run
        .expect("catch-up");
    assert_eq!(job.trigger, BackupTrigger::Missed);
    assert_eq!(job.slot, utc("2026-11-10T00:00:00.000Z"));
}

#[test]
fn busy_is_tried_three_times_then_failed_and_refusals_once_per_sign_in() {
    let mut r = Runner::new();
    let job = Job {
        tenant_id: id(1),
        slot: utc("2026-11-10T00:00:00.000Z"),
        trigger: BackupTrigger::Scheduled,
        schedule: sched(Frequency::Daily, 18, 0),
    };
    let now = utc("2026-11-10T00:01:00.000Z");
    assert_eq!(r.finished(&job, &JobResult::GateBusy, now), None);
    assert_eq!(r.finished(&job, &JobResult::Busy, now), None);
    assert_eq!(r.finished(&job, &JobResult::Busy, now), None);
    let third = r.finished(&job, &JobResult::Busy, now).expect("recorded");
    assert_eq!(
        third.outcome,
        RunOutcome::Failed {
            code: "TENANT_BUSY".into()
        }
    );
    // Not allowed: one skip, and the slot is left alone this sign-in.
    let z = Central;
    let list = list_with(&[(1, false)]);
    let signed = env("2026-11-09T20:00:00.000Z");
    let job = r.plan(now, signed, &list, &z).run.expect("job");
    let skip = r.finished(&job, &JobResult::NotAllowed, now).expect("skip");
    assert_eq!(
        skip.outcome,
        RunOutcome::Skipped {
            reason: reason::NOT_ALLOWED.into()
        }
    );
    assert!(r.plan(now, signed, &list, &z).run.is_none());
    assert_eq!(r.finished(&job, &JobResult::NotAllowed, now), None);
    // Another sign-in tries again.
    assert!(
        r.plan(now, env("2026-11-09T21:00:00.000Z"), &list, &z)
            .run
            .is_some()
    );
}

#[test]
fn failure_codes_never_carry_paths() {
    let e = CompanyError::Io {
        what: "save backup",
        source: std::io::Error::other("E:\\Owner Name\\secret"),
    };
    assert_eq!(failure_code(&e), "WRITE_FAILED");
    assert_eq!(failure_code(&CompanyError::NoPassphrase), "NO_PASSPHRASE");
    assert_eq!(
        classify(Err(CompanyError::JobFailed {
            code: "TENANT_BUSY".into()
        })),
        JobResult::Busy
    );
}

proptest! {
    /// Whatever the schedule and time: the due slot is the newest slot at
    /// or before now, never in the future; once it is used nothing is due
    /// until the next slot, which is after now.
    #[test]
    fn due_never_runs_ahead_or_twice(
        kind in 0u8..3,
        n in 0u8..28,
        hour in 0u8..24,
        minute in 0u8..60,
        offset_min in 0i64..(400 * 24 * 60),
    ) {
        let freq = match kind {
            0 => Frequency::Daily,
            1 => Frequency::Weekly { weekday: n % 7 },
            _ => Frequency::Monthly { day: n + 1 },
        };
        let s = sched(freq, hour, minute);
        let z = Central;
        let base = utc("2026-01-01T00:00:00.000Z").unix_millis();
        let now = Timestamp::from_unix_millis(base + offset_min * 60_000).expect("ts");
        let slot = due(&s, &[], &z, now).expect("some slot in 40 days");
        prop_assert!(slot <= now);
        prop_assert_eq!(Some(slot), latest_slot(&s, &z, now));
        let runs = vec![done(slot)];
        prop_assert_eq!(due(&s, &runs, &z, now), None);
        let next = next_slot(&s, &z, now).expect("next");
        prop_assert!(next > now);
        prop_assert_eq!(due(&s, &runs, &z, next), Some(next));
    }
}
