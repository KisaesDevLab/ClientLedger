//! The pop-out viewer's outbox (A04 D4, S7): the compare tools' events in
//! the pop-out document window go to the window it follows, the frame
//! after (the screens already drew this one). A window that closed
//! meanwhile gets nothing; nothing here names a value or a file.

use std::collections::HashMap;
use std::sync::Arc;

use crate::widgets::compare::{CompareEvent, CompareView, DocWords};
use crate::windows::{WindowId, Windows};

/// Which document and compare state an event was made against (review
/// L2): the feed's generation and the page words. A word pick or a box
/// names word indexes and page spots of that document only.
#[derive(Debug, Clone, Default)]
pub struct Stamp {
    generation: u64,
    words: Option<Arc<DocWords>>,
}

impl Stamp {
    /// The stamp of a window's compare view (none: nothing to pick from).
    #[must_use]
    pub fn of(view: Option<&CompareView>) -> Option<Stamp> {
        view.map(|v| Stamp {
            generation: v.feed.generation,
            words: v.words.clone(),
        })
    }

    fn same(&self, other: &Stamp) -> bool {
        self.generation == other.generation
            && match (&self.words, &other.words) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

/// One event for one window.
#[derive(Debug, Clone)]
pub struct Outgoing {
    /// The window the viewer followed when the event happened.
    pub owner: WindowId,
    /// What that window showed then.
    pub stamp: Option<Stamp>,
    /// The event.
    pub event: CompareEvent,
}

/// Events tied to the page's words and spots.
fn names_the_page(e: &CompareEvent) -> bool {
    matches!(e, CompareEvent::PageClick { .. } | CompareEvent::PageBox(_))
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

    /// Queues the pop-out's events for `owner` (none followed: dropped),
    /// stamped with what the window showed (`stamp`, [`Stamp::of`]).
    pub fn push(
        &mut self,
        owner: Option<WindowId>,
        stamp: Option<&Stamp>,
        events: Vec<CompareEvent>,
    ) {
        let Some(owner) = owner else {
            return;
        };
        self.waiting
            .extend(events.into_iter().map(|event| Outgoing {
                owner,
                stamp: stamp.cloned(),
                event,
            }));
    }

    /// True when events wait.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    /// Hands every waiting event to its window's screen, in order. A word
    /// pick or a box made against another document or compare state than
    /// the window shows now (it moved to the next draft, its targets
    /// changed) is dropped. Returns how many were delivered.
    pub fn deliver(&mut self, windows: &mut Windows) -> usize {
        let waiting = std::mem::take(&mut self.waiting);
        // What each window shows now, before any event changes it.
        let mut now: HashMap<WindowId, Option<Stamp>> = HashMap::new();
        for o in &waiting {
            now.entry(o.owner).or_insert_with(|| {
                windows
                    .get_mut(o.owner)
                    .and_then(|w| Stamp::of(w.screen.compare().as_ref()))
            });
        }
        let mut n = 0;
        for o in waiting {
            if names_the_page(&o.event) {
                let current = now.get(&o.owner).and_then(Option::as_ref);
                let same = match (&o.stamp, current) {
                    (Some(was), Some(is)) => was.same(is),
                    _ => false,
                };
                if !same {
                    tracing::debug!("a pop-out page pick for another document was dropped");
                    continue;
                }
            }
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
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use cl_model::commands;
    use eframe::egui::Ui;

    use super::*;
    use crate::widgets::compare::{CompareFeed, TargetKey};
    use crate::windows::Screen;

    struct Form {
        got: Rc<RefCell<Vec<CompareEvent>>>,
        generation: Rc<Cell<u64>>,
    }

    impl Screen for Form {
        fn title(&self) -> String {
            "Enter Bill".into()
        }
        fn ui(&mut self, _ui: &mut Ui) {}
        fn compare(&self) -> Option<CompareView> {
            Some(CompareView {
                feed: CompareFeed {
                    generation: self.generation.get(),
                    ..CompareFeed::default()
                },
                values: Vec::new(),
                words: None,
                note: None,
            })
        }
        fn on_viewer_event(&mut self, event: CompareEvent) {
            self.got.borrow_mut().push(event);
        }
    }

    fn click() -> CompareEvent {
        CompareEvent::PageClick {
            page: 0,
            first: 2,
            last: 2,
        }
    }

    #[test]
    fn events_reach_the_window_followed_and_no_other() {
        let got = Rc::new(RefCell::new(Vec::new()));
        let other = Rc::new(RefCell::new(Vec::new()));
        let generation = Rc::new(Cell::new(1));
        let mut windows = Windows::new();
        let kind = |s: &str| commands::find(s).expect("command").id;
        let bill = windows.open_new(
            kind("vendors.enter_bill"),
            Box::new(Form {
                got: Rc::clone(&got),
                generation: Rc::clone(&generation),
            }),
        );
        let check = windows.open_new(
            kind("vendors.write_check"),
            Box::new(Form {
                got: Rc::clone(&other),
                generation: Rc::new(Cell::new(1)),
            }),
        );
        let stamp = |w: &mut Windows, id| {
            Stamp::of(w.get_mut(id).and_then(|w| w.screen.compare()).as_ref())
        };
        let mut outbox = ViewerOutbox::new();
        let key = TargetKey::field("amount_due");
        let s = stamp(&mut windows, bill);
        outbox.push(
            Some(bill),
            s.as_ref(),
            vec![CompareEvent::TargetClicked(key), click()],
        );
        outbox.push(None, None, vec![CompareEvent::ToggleHighlights]);
        assert!(!outbox.is_empty());
        assert_eq!(outbox.deliver(&mut windows), 2);
        assert!(outbox.is_empty());
        assert_eq!(
            *got.borrow(),
            vec![CompareEvent::TargetClicked(key), click()]
        );
        assert!(other.borrow().is_empty());
        // A window that closed before delivery gets nothing.
        let s = stamp(&mut windows, check);
        outbox.push(
            Some(check),
            s.as_ref(),
            vec![CompareEvent::ToggleHighlights],
        );
        windows.close_now(check);
        assert_eq!(outbox.deliver(&mut windows), 0);
    }

    /// Review L2: a word pick made against what the window showed before
    /// (it moved to the next draft) is dropped; other events still go.
    #[test]
    fn a_page_pick_for_another_document_is_dropped() {
        let got = Rc::new(RefCell::new(Vec::new()));
        let generation = Rc::new(Cell::new(7));
        let mut windows = Windows::new();
        let kind = |s: &str| commands::find(s).expect("command").id;
        let bill = windows.open_new(
            kind("vendors.enter_bill"),
            Box::new(Form {
                got: Rc::clone(&got),
                generation: Rc::clone(&generation),
            }),
        );
        let s = Stamp::of(
            windows
                .get_mut(bill)
                .and_then(|w| w.screen.compare())
                .as_ref(),
        );
        let mut outbox = ViewerOutbox::new();
        outbox.push(
            Some(bill),
            s.as_ref(),
            vec![click(), CompareEvent::ToggleHighlights],
        );
        // The window moved on before the next frame.
        generation.set(8);
        assert_eq!(outbox.deliver(&mut windows), 1);
        assert_eq!(*got.borrow(), vec![CompareEvent::ToggleHighlights]);
        // Without a stamp a pick never goes.
        outbox.push(Some(bill), None, vec![click()]);
        assert_eq!(outbox.deliver(&mut windows), 0);
    }
}
