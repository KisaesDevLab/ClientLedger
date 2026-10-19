use std::cell::Cell;
use std::rc::Rc;

use cl_core::{CalendarDate, Secret};

use super::*;

fn d(y: i32, m: u32, day: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, day).unwrap()
}

/// A password stand-in that counts its drops.
struct Counted(Rc<Cell<u32>>);

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

fn counted() -> (Rc<Cell<u32>>, impl Fn() -> Counted) {
    let drops = Rc::new(Cell::new(0));
    let d2 = Rc::clone(&drops);
    (drops, move || Counted(Rc::clone(&d2)))
}

#[test]
fn the_closing_date_itself_is_locked() {
    let closing = Some(d(2026, 9, 30));
    assert!(is_locked(d(2026, 9, 30), closing), "on the closing date");
    assert!(is_locked(d(2020, 1, 1), closing));
    assert!(!is_locked(d(2026, 10, 1), closing), "the day after");
    assert!(!is_locked(CalendarDate::min(), None), "no closing date");
}

#[test]
fn old_and_new_date_matrix() {
    let closing = Some(d(2026, 9, 30));
    let locked = d(2026, 9, 15);
    let open = d(2026, 10, 15);
    // (old, new, needs password)
    let cases = [
        (None, open, false),
        (None, locked, true),
        (Some(open), open, false),
        (Some(open), locked, true),
        (Some(locked), open, true),
        (Some(locked), locked, true),
    ];
    for (old, new, want) in cases {
        assert_eq!(
            needs_password(old, new, closing),
            want,
            "{old:?} -> {new:?}"
        );
        assert!(!needs_password(old, new, None), "no closing date");
    }
}

#[test]
fn the_happy_path() {
    let mut f: ClosingFlow = ClosingFlow::new(true);
    assert_eq!(f.stage(), ClosingStage::Idle);
    assert!(!f.dialog_open());
    assert_eq!(f.message(), None);
    assert!(f.handle(ClosingEvent::LockedEdit));
    assert_eq!(f.stage(), ClosingStage::Asking);
    assert_eq!(f.attempt(), 1);
    assert!(f.dialog_open());
    assert_eq!(f.message(), Some(TextKey("closing.ask")));
    assert!(f.password().is_none());
    assert!(f.handle(ClosingEvent::Submit(Secret::new("pw"))));
    assert_eq!(f.stage(), ClosingStage::Sending);
    assert_eq!(f.password().map(Secret::expose), Some("pw"));
    assert!(
        !f.handle(ClosingEvent::Cancel),
        "the change is already on its way"
    );
    assert!(f.handle(ClosingEvent::Saved));
    assert_eq!(f.stage(), ClosingStage::Done);
    assert!(f.password().is_none());
    assert!(!f.dialog_open());
}

#[test]
fn wrong_password_retries_in_the_same_dialog() {
    let mut f: ClosingFlow = ClosingFlow::new(true);
    f.handle(ClosingEvent::LockedEdit);
    f.handle(ClosingEvent::Submit(Secret::new("bad")));
    assert!(f.handle(ClosingEvent::PasswordRejected));
    assert_eq!(f.stage(), ClosingStage::Rejected);
    assert_eq!(f.message(), Some(TextKey("closing.wrong_password")));
    assert!(f.dialog_open());
    assert!(f.password().is_none(), "the wrong password is not kept");
    assert_eq!(f.attempt(), 1);
    f.handle(ClosingEvent::Submit(Secret::new("good")));
    assert_eq!(f.attempt(), 2);
    assert!(f.handle(ClosingEvent::Saved));
    assert_eq!(f.stage(), ClosingStage::Done);
}

#[test]
fn without_the_capability_the_dialog_only_explains() {
    let mut f: ClosingFlow = ClosingFlow::new(false);
    assert!(f.handle(ClosingEvent::LockedEdit));
    assert_eq!(f.stage(), ClosingStage::Unavailable);
    assert_eq!(f.message(), Some(TextKey("closing.unavailable")));
    assert!(!f.handle(ClosingEvent::Submit(Secret::new("pw"))));
    assert!(f.password().is_none());
    assert!(f.handle(ClosingEvent::Cancel));
    assert_eq!(f.stage(), ClosingStage::Cancelled);
}

#[test]
fn engine_refusals_start_the_flow() {
    let mut f: ClosingFlow = ClosingFlow::new(true);
    assert!(f.handle(ClosingEvent::EngineClosedPeriod {
        password_required: true
    }));
    assert_eq!(f.stage(), ClosingStage::Asking);
    f.handle(ClosingEvent::Cancel);
    assert_eq!(f.stage(), ClosingStage::Cancelled);

    // A refusal no password can lift.
    assert!(f.handle(ClosingEvent::EngineClosedPeriod {
        password_required: false
    }));
    assert_eq!(f.stage(), ClosingStage::Unavailable);
    f.handle(ClosingEvent::Cancel);

    // Still locked after the password went (the closing date moved).
    f.handle(ClosingEvent::LockedEdit);
    f.handle(ClosingEvent::Submit(Secret::new("pw")));
    assert!(f.handle(ClosingEvent::EngineClosedPeriod {
        password_required: true
    }));
    assert_eq!(f.stage(), ClosingStage::Rejected);
    assert!(f.password().is_none());
}

#[test]
fn other_failures_end_the_flow_and_it_can_start_again() {
    let mut f: ClosingFlow = ClosingFlow::new(true);
    f.handle(ClosingEvent::LockedEdit);
    f.handle(ClosingEvent::Submit(Secret::new("pw")));
    assert!(f.handle(ClosingEvent::SendFailed));
    assert_eq!(f.stage(), ClosingStage::Failed);
    assert!(f.password().is_none());
    for end in [
        ClosingStage::Failed,
        ClosingStage::Done,
        ClosingStage::Cancelled,
    ] {
        assert_eq!(f.stage(), end);
        assert!(f.handle(ClosingEvent::LockedEdit), "{end:?}");
        assert_eq!((f.stage(), f.attempt()), (ClosingStage::Asking, 1));
        match end {
            ClosingStage::Failed => {
                f.handle(ClosingEvent::Submit(Secret::new("pw")));
                f.handle(ClosingEvent::Saved);
            }
            _ => {
                f.handle(ClosingEvent::Cancel);
            }
        }
    }
}

#[test]
fn meaningless_events_change_nothing() {
    let mut f: ClosingFlow = ClosingFlow::new(true);
    for e in [
        ClosingEvent::Submit(Secret::new("pw")),
        ClosingEvent::PasswordRejected,
        ClosingEvent::Saved,
        ClosingEvent::SendFailed,
        ClosingEvent::Cancel,
    ] {
        assert!(!f.handle(e));
        assert_eq!(f.stage(), ClosingStage::Idle);
    }
    f.handle(ClosingEvent::LockedEdit);
    assert!(!f.handle(ClosingEvent::LockedEdit), "already asking");
    assert!(!f.handle(ClosingEvent::Saved));
    assert_eq!(f.stage(), ClosingStage::Asking);
}

#[test]
fn the_password_is_dropped_when_the_answer_arrives() {
    for end in [
        ClosingEvent::Saved,
        ClosingEvent::PasswordRejected,
        ClosingEvent::SendFailed,
        ClosingEvent::EngineClosedPeriod {
            password_required: true,
        },
    ] {
        let (drops, make) = counted();
        let mut f: ClosingFlow<Counted> = ClosingFlow::new(true);
        f.handle(ClosingEvent::LockedEdit);
        f.handle(ClosingEvent::Submit(make()));
        assert_eq!(drops.get(), 0, "held while sending");
        f.handle(end);
        assert_eq!(drops.get(), 1, "dropped on the answer");
        drop(f);
        assert_eq!(drops.get(), 1);
    }
}

#[test]
fn a_password_submitted_where_it_means_nothing_is_dropped_at_once() {
    let (drops, make) = counted();
    let mut f: ClosingFlow<Counted> = ClosingFlow::new(false);
    f.handle(ClosingEvent::LockedEdit);
    assert!(!f.handle(ClosingEvent::Submit(make())));
    assert_eq!(drops.get(), 1);
    // Cancel while asking holds nothing.
    let mut g: ClosingFlow<Counted> = ClosingFlow::new(true);
    g.handle(ClosingEvent::LockedEdit);
    g.handle(ClosingEvent::Cancel);
    assert_eq!(drops.get(), 1);
}

#[test]
fn debug_is_redacted() {
    let mut f: ClosingFlow = ClosingFlow::new(true);
    f.handle(ClosingEvent::LockedEdit);
    f.handle(ClosingEvent::Submit(Secret::new("hunter2-closing")));
    let s = format!("{f:?}");
    assert!(!s.contains("hunter2"), "{s}");
    assert!(s.contains("Sending"), "{s}");
    let e: ClosingEvent = ClosingEvent::Submit(Secret::new("hunter2-closing"));
    let s = format!("{e:?}");
    assert!(!s.contains("hunter2"), "{s}");
}
