//! The Print later queue and a print run's state (Q-55, Q-P19b, Q-P19n,
//! Q-P19t, Q-P19ab). No I/O: the UI drives it and makes the engine calls it
//! returns.
//!
//! One queue for every printable type. Forms (invoices, sales receipts,
//! credit memos) already have their numbers; a printed form leaves the
//! queue ([`PrintRun::sent`]).
//!
//! Checks get their numbers at print time:
//!
//! 1. [`PrintQueue::start_run`] numbers the chosen checks from the first
//!    number the owner picked; the preview shows them.
//! 2. The owner clicks **Print**: [`PrintRun::record_print`] gives the one
//!    `POST /checks/print` call (check ids in order, starting number) that
//!    records the numbers with the engine **before anything is printed**.
//! 3. When the engine accepted it, [`PrintRun::recorded`] returns the
//!    [`PendingChecks`] the UI saves on this computer
//!    (`cl_settings::UnconfirmedChecks`), so the Needs-attention reminder
//!    survives a restart; the UI then sends the pages to the printer and
//!    asks "Did all checks print correctly?". If the engine refused
//!    (`409 CHECK_NUMBER_IN_USE`, closed period), [`PrintRun::record_failed`]
//!    goes back to the preview.
//! 4. The answer ([`PrintRun::confirm`]):
//!    - **All printed** — nothing more to call; the saved batch is cleared.
//!    - **First bad number N** — one `POST /checks/requeue` for N and every
//!      later check ([`RequeueCall`]): the engine clears their numbers, logs
//!      them as spoiled (U-106; reusable, owner 2026-10-29) and they wait to
//!      print again. The payment stays.
//!    - **Closed without an answer** ([`PrintRun::dismissed`]) — the checks
//!      stay printed with their numbers; the saved batch keeps Home's
//!      reminder until the owner answers, now or after a restart
//!      ([`PrintRun::unconfirmed`]).
//!
//! Reprinting one printed check (Q-P19ab) is the same requeue:
//! [`PrintQueue::requeue`] puts it back with its old number, and the next
//! run numbers it afresh.

/// A printable document type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocKind {
    /// A check (numbered at print).
    Check,
    /// An invoice.
    Invoice,
    /// A sales receipt.
    SalesReceipt,
    /// A credit memo.
    CreditMemo,
}

impl DocKind {
    /// True when printing assigns the document's number.
    #[must_use]
    pub fn numbered_at_print(self) -> bool {
        matches!(self, DocKind::Check)
    }
}

/// A document waiting to print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEntry<Id> {
    /// The engine's id for the document.
    pub id: Id,
    /// Its type.
    pub kind: DocKind,
    /// The check number it printed with before, when it is a reprint.
    pub previous_number: Option<u64>,
}

/// Why a queue operation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QueueError {
    /// Nothing was selected.
    #[error("choose at least one document to print")]
    NothingSelected,
    /// A selected document is not in the queue, or is another type.
    #[error("a chosen document is not waiting to print")]
    NotQueued,
    /// Checks need a first check number.
    #[error("enter the first check number")]
    NoFirstNumber,
    /// The numbers would run past the largest check number.
    #[error("the check numbers are too large")]
    NumberOverflow,
    /// The bad number is not one this run printed.
    #[error("that number was not part of this print run")]
    NotInRun,
    /// The step does not fit the run's state or type.
    #[error("this print run is not waiting for that step")]
    WrongState,
}

/// The documents waiting to print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrintQueue<Id> {
    entries: Vec<QueueEntry<Id>>,
}

impl<Id> Default for PrintQueue<Id> {
    fn default() -> Self {
        PrintQueue {
            entries: Vec::new(),
        }
    }
}

impl<Id: Clone + PartialEq> PrintQueue<Id> {
    /// An empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything waiting, in the order added.
    #[must_use]
    pub fn entries(&self) -> &[QueueEntry<Id>] {
        &self.entries
    }

    /// What is waiting of one type.
    pub fn of_kind(&self, kind: DocKind) -> impl Iterator<Item = &QueueEntry<Id>> {
        self.entries.iter().filter(move |e| e.kind == kind)
    }

    /// Mark a document Print later (no change if it is already waiting).
    pub fn add(&mut self, id: Id, kind: DocKind) {
        if !self.contains(&id) {
            self.entries.push(QueueEntry {
                id,
                kind,
                previous_number: None,
            });
        }
    }

    /// Put a printed check back to print again (after the engine's
    /// requeue); `old_number` is the number it printed with.
    pub fn requeue(&mut self, id: Id, kind: DocKind, old_number: Option<u64>) {
        self.entries.retain(|e| e.id != id);
        self.entries.push(QueueEntry {
            id,
            kind,
            previous_number: old_number,
        });
    }

    /// Take a document off the queue (the owner unticked Print later).
    pub fn remove(&mut self, id: &Id) {
        self.entries.retain(|e| &e.id != id);
    }

    /// True when `id` is waiting.
    #[must_use]
    pub fn contains(&self, id: &Id) -> bool {
        self.entries.iter().any(|e| &e.id == id)
    }

    /// Start a print run of the `selected` documents of `kind`, in that
    /// order. Checks need `first_number`; forms ignore it.
    ///
    /// # Errors
    /// [`QueueError::NothingSelected`], [`QueueError::NotQueued`],
    /// [`QueueError::NoFirstNumber`] or [`QueueError::NumberOverflow`].
    pub fn start_run(
        &self,
        kind: DocKind,
        selected: &[Id],
        first_number: Option<u64>,
    ) -> Result<PrintRun<Id>, QueueError> {
        if selected.is_empty() {
            return Err(QueueError::NothingSelected);
        }
        let mut items = Vec::with_capacity(selected.len());
        let mut next = if kind.numbered_at_print() {
            Some(first_number.ok_or(QueueError::NoFirstNumber)?)
        } else {
            None
        };
        for id in selected {
            let e = self
                .entries
                .iter()
                .find(|e| &e.id == id && e.kind == kind)
                .ok_or(QueueError::NotQueued)?;
            let number = next;
            if let Some(n) = next {
                next = Some(n.checked_add(1).ok_or(QueueError::NumberOverflow)?);
            }
            items.push(RunItem {
                id: id.clone(),
                number,
                previous_number: e.previous_number,
            });
        }
        Ok(PrintRun {
            kind,
            items,
            state: RunState::Ready,
        })
    }

    /// Apply a finished run: printed documents leave the queue; requeued
    /// checks stay (or come back), remembering the number they printed with.
    pub fn apply(&mut self, outcome: &RunOutcome<Id>) {
        for p in &outcome.printed {
            self.remove(&p.id);
        }
        if let Some(r) = &outcome.requeue {
            for (id, old) in r.check_ids.iter().zip(&r.old_numbers) {
                self.requeue(id.clone(), DocKind::Check, Some(*old));
            }
        }
    }
}

/// A document in a run, with the number it prints with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunItem<Id> {
    /// The document.
    pub id: Id,
    /// Its check number in this run (checks only).
    pub number: Option<u64>,
    /// The number it printed with last time, for a reprint.
    pub previous_number: Option<u64>,
}

/// Where a run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// Numbered and shown in the preview; nothing recorded or sent.
    Ready,
    /// Checks: the `POST /checks/print` call is out.
    Recording,
    /// Checks: numbers recorded, pages sent, "Did all checks print
    /// correctly?" showing.
    AwaitingConfirmation,
    /// Checks: the question was closed without an answer; they stay
    /// printed and Home reminds the owner (Needs attention).
    Unconfirmed,
    /// Answered, or a form run that printed.
    Done,
}

/// The owner's answer to "Did all checks print correctly?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    /// Every check printed.
    AllPrinted,
    /// The first check that did not print correctly; it and every later
    /// check print again.
    FirstBad(u64),
}

/// `POST /checks/print`: record the numbers with the engine. The engine
/// numbers `check_ids` in order from `starting_number`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordPrint<Id> {
    /// Checks in print order.
    pub check_ids: Vec<Id>,
    /// The first number.
    pub starting_number: u64,
}

/// `POST /checks/requeue`: send these checks back to print again. The
/// engine logs `old_numbers` as spoiled (reusable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequeueCall<Id> {
    /// The checks to print again.
    pub check_ids: Vec<Id>,
    /// The numbers they printed with, same order.
    pub old_numbers: Vec<u64>,
}

/// A check batch recorded with the engine and not yet confirmed, for the
/// UI to save on this computer (`cl_settings::UnconfirmedChecks`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingChecks<Id> {
    /// Each check and the number it printed with, in print order.
    pub checks: Vec<(Id, u64)>,
}

/// A document that printed and keeps its number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Printed<Id> {
    /// The document.
    pub id: Id,
    /// The check number it printed with (checks only).
    pub number: Option<u64>,
}

/// What a finished run means for the engine and the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome<Id> {
    /// Printed and keeping their numbers (already recorded for checks).
    pub printed: Vec<Printed<Id>>,
    /// The requeue call to make, when checks must print again.
    pub requeue: Option<RequeueCall<Id>>,
    /// The first number to offer for the next run of checks: one past the
    /// last number used (pre-printed stock moves on); any number, including
    /// a spoiled one, may still be chosen.
    pub next_first_number: Option<u64>,
}

/// One print run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrintRun<Id> {
    kind: DocKind,
    items: Vec<RunItem<Id>>,
    state: RunState,
}

impl<Id: Clone + PartialEq> PrintRun<Id> {
    /// A saved, unanswered check batch, after a restart: answer it with
    /// [`PrintRun::confirm`].
    #[must_use]
    pub fn unconfirmed(checks: Vec<(Id, u64)>) -> PrintRun<Id> {
        PrintRun {
            kind: DocKind::Check,
            items: checks
                .into_iter()
                .map(|(id, n)| RunItem {
                    id,
                    number: Some(n),
                    previous_number: None,
                })
                .collect(),
            state: RunState::Unconfirmed,
        }
    }

    /// The documents and their numbers, in print order.
    #[must_use]
    pub fn items(&self) -> &[RunItem<Id>] {
        &self.items
    }

    /// The run's type.
    #[must_use]
    pub fn kind(&self) -> DocKind {
        self.kind
    }

    /// Where the run is.
    #[must_use]
    pub fn state(&self) -> RunState {
        self.state
    }

    /// True while Home should show the Needs-attention reminder.
    #[must_use]
    pub fn needs_attention(&self) -> bool {
        matches!(
            self.state,
            RunState::AwaitingConfirmation | RunState::Unconfirmed
        )
    }

    fn numbers(&self) -> Vec<(Id, u64)> {
        self.items
            .iter()
            .filter_map(|i| i.number.map(|n| (i.id.clone(), n)))
            .collect()
    }

    /// The owner clicked Print on a check run: the engine call that records
    /// the numbers. Make it before sending anything to the printer.
    ///
    /// # Errors
    /// [`QueueError::WrongState`] for forms or a run not in the preview.
    pub fn record_print(&mut self) -> Result<RecordPrint<Id>, QueueError> {
        if !self.kind.numbered_at_print() || self.state != RunState::Ready {
            return Err(QueueError::WrongState);
        }
        let starting_number = self
            .items
            .first()
            .and_then(|i| i.number)
            .ok_or(QueueError::WrongState)?;
        self.state = RunState::Recording;
        Ok(RecordPrint {
            check_ids: self.items.iter().map(|i| i.id.clone()).collect(),
            starting_number,
        })
    }

    /// The engine refused the numbers: back to the preview (pick another
    /// first number or fix the closing date, then Print again).
    ///
    /// # Errors
    /// [`QueueError::WrongState`] unless the call was out.
    pub fn record_failed(&mut self) -> Result<(), QueueError> {
        if self.state != RunState::Recording {
            return Err(QueueError::WrongState);
        }
        self.state = RunState::Ready;
        Ok(())
    }

    /// The engine recorded the numbers: save the returned batch, send the
    /// pages to the printer and ask "Did all checks print correctly?".
    ///
    /// # Errors
    /// [`QueueError::WrongState`] unless the call was out.
    pub fn recorded(&mut self) -> Result<PendingChecks<Id>, QueueError> {
        if self.state != RunState::Recording {
            return Err(QueueError::WrongState);
        }
        self.state = RunState::AwaitingConfirmation;
        Ok(PendingChecks {
            checks: self.numbers(),
        })
    }

    /// A form run went to the printer: done, and the forms leave the queue.
    ///
    /// # Errors
    /// [`QueueError::WrongState`] for checks (use
    /// [`PrintRun::record_print`]) or a run not in the preview.
    pub fn sent(&mut self) -> Result<RunOutcome<Id>, QueueError> {
        if self.kind.numbered_at_print() || self.state != RunState::Ready {
            return Err(QueueError::WrongState);
        }
        self.state = RunState::Done;
        Ok(self.outcome(self.items.len()))
    }

    /// The question was closed without an answer: the checks stay printed
    /// and the saved batch stays.
    ///
    /// # Errors
    /// [`QueueError::WrongState`] unless it was showing.
    pub fn dismissed(&mut self) -> Result<(), QueueError> {
        if self.state != RunState::AwaitingConfirmation {
            return Err(QueueError::WrongState);
        }
        self.state = RunState::Unconfirmed;
        Ok(())
    }

    /// The owner answered (now, or later from the reminder). Make the
    /// returned requeue call, if any, then clear the saved batch.
    ///
    /// # Errors
    /// [`QueueError::WrongState`] unless the run is waiting for an answer;
    /// [`QueueError::NotInRun`] for a bad number this run did not print.
    pub fn confirm(&mut self, answer: Confirmation) -> Result<RunOutcome<Id>, QueueError> {
        if !matches!(
            self.state,
            RunState::AwaitingConfirmation | RunState::Unconfirmed
        ) {
            return Err(QueueError::WrongState);
        }
        let good = match answer {
            Confirmation::AllPrinted => self.items.len(),
            Confirmation::FirstBad(n) => self
                .items
                .iter()
                .position(|i| i.number == Some(n))
                .ok_or(QueueError::NotInRun)?,
        };
        self.state = RunState::Done;
        Ok(self.outcome(good))
    }

    /// The first `good` items printed; the rest print again.
    fn outcome(&self, good: usize) -> RunOutcome<Id> {
        let (ok, bad) = self.items.split_at(good.min(self.items.len()));
        let requeue = (!bad.is_empty()).then(|| RequeueCall {
            check_ids: bad.iter().map(|i| i.id.clone()).collect(),
            old_numbers: bad.iter().filter_map(|i| i.number).collect(),
        });
        RunOutcome {
            printed: ok
                .iter()
                .map(|i| Printed {
                    id: i.id.clone(),
                    number: i.number,
                })
                .collect(),
            requeue,
            next_first_number: self
                .items
                .iter()
                .filter_map(|i| i.number)
                .max()
                .and_then(|n| n.checked_add(1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue() -> PrintQueue<&'static str> {
        let mut q = PrintQueue::new();
        for id in ["c1", "c2", "c3", "c4"] {
            q.add(id, DocKind::Check);
        }
        q.add("inv1", DocKind::Invoice);
        q.add("c1", DocKind::Check); // already waiting: no duplicate
        q
    }

    /// Start, record and send a check run.
    fn printed_run(
        q: &PrintQueue<&'static str>,
        ids: &[&'static str],
        first: u64,
    ) -> PrintRun<&'static str> {
        let mut run = q.start_run(DocKind::Check, ids, Some(first)).unwrap();
        run.record_print().unwrap();
        run.recorded().unwrap();
        run
    }

    #[test]
    fn a_check_run_numbers_in_order_from_the_first_number() {
        let q = queue();
        assert_eq!(q.entries().len(), 5);
        let run = q
            .start_run(DocKind::Check, &["c2", "c1", "c3"], Some(1001))
            .unwrap();
        let nums: Vec<_> = run.items().iter().map(|i| (i.id, i.number)).collect();
        assert_eq!(
            nums,
            [("c2", Some(1001)), ("c1", Some(1002)), ("c3", Some(1003))]
        );
        assert_eq!(run.state(), RunState::Ready);
        assert!(!run.needs_attention());
    }

    #[test]
    fn runs_refuse_bad_selections() {
        let q = queue();
        assert_eq!(
            q.start_run(DocKind::Check, &[], Some(1)),
            Err(QueueError::NothingSelected)
        );
        assert_eq!(
            q.start_run(DocKind::Check, &["c1"], None),
            Err(QueueError::NoFirstNumber)
        );
        assert_eq!(
            q.start_run(DocKind::Check, &["inv1"], Some(1)),
            Err(QueueError::NotQueued)
        );
        assert_eq!(
            q.start_run(DocKind::Check, &["zz"], Some(1)),
            Err(QueueError::NotQueued)
        );
        assert_eq!(
            q.start_run(DocKind::Check, &["c1", "c2"], Some(u64::MAX)),
            Err(QueueError::NumberOverflow)
        );
    }

    #[test]
    fn print_records_the_numbers_with_the_engine_before_anything_prints() {
        let q = queue();
        let mut run = q
            .start_run(DocKind::Check, &["c2", "c1"], Some(500))
            .unwrap();
        assert_eq!(
            run.confirm(Confirmation::AllPrinted),
            Err(QueueError::WrongState)
        );
        assert_eq!(run.recorded(), Err(QueueError::WrongState));
        let call = run.record_print().unwrap();
        assert_eq!(
            call,
            RecordPrint {
                check_ids: vec!["c2", "c1"],
                starting_number: 500
            }
        );
        assert_eq!(run.state(), RunState::Recording);
        assert_eq!(run.record_print(), Err(QueueError::WrongState));
        let pending = run.recorded().unwrap();
        assert_eq!(pending.checks, [("c2", 500), ("c1", 501)]);
        assert_eq!(run.state(), RunState::AwaitingConfirmation);
        assert!(run.needs_attention());
    }

    #[test]
    fn a_refused_record_goes_back_to_the_preview() {
        let q = queue();
        let mut run = q.start_run(DocKind::Check, &["c1"], Some(7)).unwrap();
        assert_eq!(run.record_failed(), Err(QueueError::WrongState));
        run.record_print().unwrap();
        run.record_failed().unwrap();
        assert_eq!(run.state(), RunState::Ready);
        assert!(run.record_print().is_ok());
    }

    #[test]
    fn all_printed_needs_no_more_calls_and_removes_the_checks() {
        let mut q = queue();
        let mut run = printed_run(&q, &["c1", "c2"], 500);
        let out = run.confirm(Confirmation::AllPrinted).unwrap();
        assert_eq!(
            out.printed,
            [
                Printed {
                    id: "c1",
                    number: Some(500)
                },
                Printed {
                    id: "c2",
                    number: Some(501)
                }
            ]
        );
        assert_eq!(out.requeue, None);
        assert_eq!(out.next_first_number, Some(502));
        q.apply(&out);
        assert!(!q.contains(&"c1") && !q.contains(&"c2") && q.contains(&"c3"));
        assert_eq!(run.state(), RunState::Done);
        assert_eq!(
            run.confirm(Confirmation::AllPrinted),
            Err(QueueError::WrongState)
        );
    }

    #[test]
    fn first_bad_number_gives_one_requeue_call_for_it_and_later_checks() {
        let mut q = queue();
        let mut run = printed_run(&q, &["c1", "c2", "c3", "c4"], 1001);
        let out = run.confirm(Confirmation::FirstBad(1003)).unwrap();
        assert_eq!(
            out.printed.iter().map(|p| p.id).collect::<Vec<_>>(),
            ["c1", "c2"]
        );
        assert_eq!(
            out.requeue,
            Some(RequeueCall {
                check_ids: vec!["c3", "c4"],
                old_numbers: vec![1003, 1004]
            })
        );
        assert_eq!(out.next_first_number, Some(1005));
        q.apply(&out);
        assert!(q.contains(&"c3") && q.contains(&"c4") && !q.contains(&"c1"));
        // Spoiled numbers are not blocked: any first number may be chosen.
        let again = q
            .start_run(DocKind::Check, &["c3", "c4"], Some(1003))
            .unwrap();
        assert_eq!(again.items()[0].number, Some(1003));
        assert_eq!(again.items()[0].previous_number, Some(1003));
        assert_eq!(again.items()[1].previous_number, Some(1004));
    }

    #[test]
    fn first_bad_equal_to_the_first_number_requeues_everything() {
        let mut run = printed_run(&queue(), &["c1", "c2"], 7);
        let out = run.confirm(Confirmation::FirstBad(7)).unwrap();
        assert_eq!(out.printed.len(), 0);
        assert_eq!(out.requeue.unwrap().check_ids, ["c1", "c2"]);
    }

    #[test]
    fn a_bad_number_outside_the_run_is_refused_and_the_run_still_waits() {
        let mut run = printed_run(&queue(), &["c1", "c2"], 7);
        assert_eq!(
            run.confirm(Confirmation::FirstBad(9)),
            Err(QueueError::NotInRun)
        );
        assert_eq!(
            run.confirm(Confirmation::FirstBad(6)),
            Err(QueueError::NotInRun)
        );
        assert_eq!(run.state(), RunState::AwaitingConfirmation);
        assert!(run.confirm(Confirmation::FirstBad(8)).is_ok());
    }

    #[test]
    fn closing_without_an_answer_keeps_the_reminder_across_a_restart() {
        let q = queue();
        let mut run = q.start_run(DocKind::Check, &["c1", "c2"], Some(7)).unwrap();
        assert_eq!(run.dismissed(), Err(QueueError::WrongState));
        run.record_print().unwrap();
        let pending = run.recorded().unwrap();
        run.dismissed().unwrap();
        assert_eq!(run.state(), RunState::Unconfirmed);
        assert!(run.needs_attention());
        // After a restart the saved batch rebuilds the run.
        let mut resumed = PrintRun::unconfirmed(pending.checks.clone());
        assert_eq!(resumed.state(), RunState::Unconfirmed);
        assert!(resumed.needs_attention());
        assert_eq!(resumed.record_print(), Err(QueueError::WrongState));
        let out = resumed.confirm(Confirmation::FirstBad(8)).unwrap();
        assert_eq!(out.requeue.unwrap().old_numbers, [8]);
        assert!(!resumed.needs_attention());
    }

    #[test]
    fn a_single_check_reprint_goes_through_the_queue_for_a_new_number() {
        let mut q: PrintQueue<&str> = PrintQueue::new();
        q.requeue("c9", DocKind::Check, Some(2040));
        assert_eq!(q.entries()[0].previous_number, Some(2040));
        let mut run = q.start_run(DocKind::Check, &["c9"], Some(2051)).unwrap();
        assert_eq!(run.record_print().unwrap().starting_number, 2051);
        run.recorded().unwrap();
        let out = run.confirm(Confirmation::AllPrinted).unwrap();
        q.apply(&out);
        assert_eq!(q.entries().len(), 0);
    }

    #[test]
    fn form_runs_keep_their_numbers_and_finish_when_sent() {
        let mut q = queue();
        q.add("inv2", DocKind::Invoice);
        assert_eq!(q.of_kind(DocKind::Invoice).count(), 2);
        let mut run = q
            .start_run(DocKind::Invoice, &["inv1", "inv2"], None)
            .unwrap();
        assert!(run.items().iter().all(|i| i.number.is_none()));
        assert_eq!(run.record_print(), Err(QueueError::WrongState));
        let out = run.sent().unwrap();
        assert_eq!(run.state(), RunState::Done);
        assert_eq!(out.printed.len(), 2);
        assert_eq!(out.requeue, None);
        assert_eq!(out.next_first_number, None);
        q.apply(&out);
        assert_eq!(q.of_kind(DocKind::Invoice).count(), 0);
        assert_eq!(q.of_kind(DocKind::Check).count(), 4);
        let mut checks = q.start_run(DocKind::Check, &["c1"], Some(1)).unwrap();
        assert_eq!(checks.sent(), Err(QueueError::WrongState));
        q.remove(&"c4");
        assert_eq!(q.of_kind(DocKind::Check).count(), 3);
        assert!(
            !DocKind::SalesReceipt.numbered_at_print() && !DocKind::CreditMemo.numbered_at_print()
        );
    }
}
