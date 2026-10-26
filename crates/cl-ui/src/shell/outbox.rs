//! The pop-out viewer's outbox (A04 D4, S7): the compare tools' events in
//! the pop-out document window go to the window it follows, the frame
//! after (the screens already drew this one). A window that closed
//! meanwhile gets nothing; nothing here names a value or a file.

use crate::widgets::compare::CompareEvent;
use crate::windows::{WindowId, Windows};

/// One event for one window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outgoing {
    /// The window the viewer followed when the event happened.
    pub owner: WindowId,
    /// The event.
    pub event: CompareEvent,
}

/// Events waiting for the next frame.
#[derive(Debug, Default)]
pub struct ViewerOutbox {
    waiting: Vec<Outgoing>,
}

impl ViewerOutbox {
    /// Empty.
    #[must_use]
    pub fn new() -> ViewerOutbox {
        ViewerOutbox::default()
    }

    /// Queues the pop-out's events for `owner` (none followed: dropped).
    pub fn push(&mut self, owner: Option<WindowId>, events: Vec<CompareEvent>) {
        let Some(owner) = owner else {
            return;
        };
        self.waiting
            .extend(events.into_iter().map(|event| Outgoing { owner, event }));
    }

    /// True when events wait.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    /// Hands every waiting event to its window's screen, in order.
    /// Returns how many were delivered.
    pub fn deliver(&mut self, windows: &mut Windows) -> usize {
        let mut n = 0;
        for o in std::mem::take(&mut self.waiting) {
            if let Some(w) = windows.get_mut(o.owner) {
                w.screen.on_viewer_event(o.event);
                n += 1;
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use cl_model::commands;
    use eframe::egui::Ui;

    use super::*;
    use crate::widgets::compare::TargetKey;
    use crate::windows::Screen;

    struct Form(Rc<RefCell<Vec<CompareEvent>>>);

    impl Screen for Form {
        fn title(&self) -> String {
            "Enter Bill".into()
        }
        fn ui(&mut self, _ui: &mut Ui) {}
        fn on_viewer_event(&mut self, event: CompareEvent) {
            self.0.borrow_mut().push(event);
        }
    }

    #[test]
    fn events_reach_the_window_followed_and_no_other() {
        let got = Rc::new(RefCell::new(Vec::new()));
        let other = Rc::new(RefCell::new(Vec::new()));
        let mut windows = Windows::new();
        let kind = |s: &str| commands::find(s).expect("command").id;
        let bill = windows.open_new(kind("vendors.enter_bill"), Box::new(Form(Rc::clone(&got))));
        let check = windows.open_new(
            kind("vendors.write_check"),
            Box::new(Form(Rc::clone(&other))),
        );
        let mut outbox = ViewerOutbox::new();
        let key = TargetKey::field("amount_due");
        outbox.push(
            Some(bill),
            vec![
                CompareEvent::TargetClicked(key),
                CompareEvent::PageClick {
                    page: 0,
                    first: 2,
                    last: 2,
                },
            ],
        );
        outbox.push(None, vec![CompareEvent::ToggleHighlights]);
        assert!(!outbox.is_empty());
        assert_eq!(outbox.deliver(&mut windows), 2);
        assert!(outbox.is_empty());
        assert_eq!(
            *got.borrow(),
            vec![
                CompareEvent::TargetClicked(key),
                CompareEvent::PageClick {
                    page: 0,
                    first: 2,
                    last: 2
                }
            ]
        );
        assert!(other.borrow().is_empty());
        // A window that closed before delivery gets nothing.
        outbox.push(Some(check), vec![CompareEvent::ToggleHighlights]);
        windows.close_now(check);
        assert_eq!(outbox.deliver(&mut windows), 0);
    }
}
