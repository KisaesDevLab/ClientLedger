//! The intake inbox service (A01 C1, C4) against the demo engine and real
//! temporary folders, with short watcher timings: arrival as its own
//! draft, duplicates, the form claim, Add to…, Discard, folders that keep
//! their originals, unconfirmed and shared folders, the P17 migration, and
//! no owner data in `Debug` or the control channel.

use std::time::{Duration, Instant};

use cl_core::intake::IntakeKind;

use super::*;
use crate::data_bus::Change;

const PDF: &[u8] = b"%PDF-1.4\n%intake service test\n";
const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn fast() -> WatchConfig {
    WatchConfig {
        poll: Duration::from_millis(80),
        settle_recheck: Duration::from_millis(40),
        stable_for: Duration::from_millis(120),
        deadline: Duration::from_secs(2),
        slow: Duration::from_secs(1),
        max_interval: Duration::from_millis(400),
        repin_every: Duration::from_secs(1),
        backoff: Duration::from_millis(80),
    }
}

struct World {
    dir: tempfile::TempDir,
    link: Arc<CompanyLink>,
    tenant: TenantId,
    intake: Intake,
}

fn world() -> World {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let (tenant, name) = {
        let demo = link.demo_state().expect("demo");
        (demo.companies[0].tenant_id, demo.companies[0].name.clone())
    };
    link.store()
        .update(|l| {
            l.upsert(tenant, &name);
        })
        .expect("store");
    let mut intake = Intake::new();
    intake.watch_config = fast();
    // A01's inbox: an engine without document drafts (A04 tests use
    // `world_with_drafts`).
    link.demo_state().expect("demo").drafts.enabled = false;
    World {
        dir,
        link,
        tenant,
        intake,
    }
}

/// The inbox over a demo engine with document drafts (A04).
fn world_with_drafts() -> World {
    let w = world();
    w.link.demo_state().expect("demo").drafts.enabled = true;
    w
}

impl World {
    /// A folder in the demo engine (confirmed here unless `here` is false).
    fn folder(&self, name: &str, keep: bool, here: bool) -> (IntakeFolder, PathBuf) {
        self.folder_with(name, keep, here, false)
    }

    /// A folder whose new files are read automatically (`auto_read`).
    fn folder_with(
        &self,
        name: &str,
        keep: bool,
        here: bool,
        auto_read: bool,
    ) -> (IntakeFolder, PathBuf) {
        let path = self.dir.path().join(name);
        std::fs::create_dir_all(&path).expect("dir");
        let f = IntakeFolder {
            id: intake_folders::new_folder_id(),
            name: name.to_owned(),
            path: path.to_string_lossy().into_owned(),
            default_kind: if auto_read {
                IntakeKind::Bills
            } else {
                IntakeKind::Auto
            },
            auto_read,
            keep_originals: keep,
        };
        {
            let mut demo = self.link.demo_state().expect("demo");
            let mut list = demo.intake.get(self.tenant).folders;
            list.push(f.clone());
            demo.intake.save_elsewhere(self.tenant, list);
        }
        if here {
            self.confirm(self.tenant, &f);
        }
        (f, path)
    }

    fn confirm(&self, tenant: TenantId, f: &IntakeFolder) {
        self.confirm_at(tenant, f, cl_settings::recent::now());
    }

    fn confirm_at(&self, tenant: TenantId, f: &IntakeFolder, at: cl_core::Timestamp) {
        let key = cl_company::intake::canonical_key(Path::new(&f.path)).expect("key");
        self.link
            .store()
            .update(|l| {
                let rec = l.upsert(tenant, "Company");
                rec.prefs.intake_here.insert(
                    f.id,
                    HereNote {
                        path_hash: cl_settings::path_hash(&f.path),
                        canonical_key: key,
                        since_ms: None,
                        confirmed_at: Some(at),
                    },
                );
            })
            .expect("store");
    }

    fn tick(&mut self) {
        tick_service(
            &mut self.intake,
            &self.link,
            Some(self.tenant),
            today(),
            Instant::now(),
        );
    }

    /// Ticks until `done` holds (the folder threads need real time).
    fn until(&mut self, what: &str, done: impl Fn(&Intake) -> bool) {
        let start = Instant::now();
        loop {
            self.tick();
            if done(&self.intake) {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "timed out waiting for {what}: {:?}",
                self.intake.rows()
            );
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    fn row(&self, name: &str) -> Row {
        self.intake
            .rows()
            .into_iter()
            .find(|r| r.file.name == name)
            .unwrap_or_else(|| panic!("{name} listed: {:?}", self.intake.rows()))
    }

    fn state_of(&self, name: &str) -> Option<RowState> {
        self.intake
            .rows()
            .into_iter()
            .find(|r| r.file.name == name)
            .map(|r| r.state)
    }

    fn uploads(&self) -> Vec<cl_api::Attachment> {
        let demo = self.link.demo_state().expect("demo");
        // Every demo draft the inbox state knows of.
        self.intake
            .state
            .entries(&self.intake.company)
            .iter()
            .filter_map(|e| e.draft.as_deref())
            .filter_map(|d| DraftKey::parse(d).ok())
            .flat_map(|d| crate::company::demo::checks::attachments(&demo, AttachTo::Draft(d)))
            .collect()
    }
}

fn has(name: &str, state: RowState) -> impl Fn(&Intake) -> bool + '_ {
    move |s: &Intake| {
        s.rows()
            .iter()
            .any(|r| r.file.name == name && r.state == state)
    }
}

#[test]
fn a_saved_file_is_copied_in_as_its_own_draft() {
    let mut w = world();
    let (_, bills) = w.folder("Bills", false, true);
    w.until("the folder listed", |s| {
        s.runs.first().is_some_and(|r| r.listing().is_some())
    });
    assert_eq!(w.intake.runs[0].health(), Health::Ok);
    std::fs::write(bills.join("bill.pdf"), PDF).expect("w");
    w.until("the file copied in", has("bill.pdf", RowState::Arrived));
    let row = w.row("bill.pdf");
    let entry = w
        .intake
        .state
        .entry(&w.intake.company, row.entry.as_ref().expect("entry"))
        .cloned()
        .expect("entry");
    assert_eq!(entry.status, EntryStatus::Arrived);
    assert_eq!(entry.folder, Some(w.intake.runs[0].folder.id));
    assert!(entry.draft.is_some() && entry.upload_key.is_some());
    let up = w.uploads();
    assert_eq!(up.len(), 1);
    assert_eq!(up[0].attachable_type, "draft");
    assert_eq!(up[0].file_size, Some(PDF.len() as u64));
    assert_eq!(
        Some(up[0].id.to_string()),
        entry.attachment().map(str::to_owned)
    );
    assert_eq!(w.intake.badge(), 1);
    // Saved: a fresh service (a restart) knows the file.
    w.intake.flush_state();
    let stored = w
        .intake
        .store
        .as_ref()
        .expect("store")
        .load()
        .expect("load");
    assert_eq!(stored.entries(&w.intake.company).len(), 1);
    assert!(bills.join("bill.pdf").exists(), "nothing moved yet");
}

#[test]
fn the_same_bytes_twice_are_one_upload_and_one_duplicate() {
    let mut w = world();
    let (_, receipts) = w.folder("Receipts", false, true);
    let first = receipts.join("receipt.png");
    std::fs::write(&first, PNG).expect("w");
    w.until("the first copied in", has("receipt.png", RowState::Arrived));
    std::fs::write(receipts.join("receipt (copy).png"), PNG).expect("w");
    w.until(
        "the copy flagged",
        has("receipt (copy).png", RowState::Duplicate),
    );
    assert_eq!(w.uploads().len(), 1, "never uploaded on its own");
    let dup = w.row("receipt (copy).png");
    assert!(dup.seen_at.is_some(), "when the first came in");
    // It stays one entry, flagged once.
    for _ in 0..10 {
        w.tick();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(w.intake.state.entries(&w.intake.company).len(), 2);
    // Copy anyway: it goes up after all.
    let entry = dup.entry.clone().expect("entry");
    assert!(w.intake.copy_anyway(&entry).is_ok());
    w.until(
        "copied anyway",
        has("receipt (copy).png", RowState::Arrived),
    );
    assert_eq!(w.uploads().len(), 2);
}

#[test]
fn a_form_takes_the_arrival_draft_and_its_save_moves_the_original() {
    let mut w = world();
    let (_, bills) = w.folder("Bills", false, true);
    std::fs::write(bills.join("bill.pdf"), PDF).expect("w");
    w.until("copied in", has("bill.pdf", RowState::Arrived));
    let entry = w.row("bill.pdf").entry.expect("entry");
    let draft_text = w
        .intake
        .state
        .entry(&w.intake.company, &entry)
        .and_then(|e| e.draft.clone())
        .expect("draft");
    let link = Arc::clone(&w.link);
    let (draft, attachment, claim) = w
        .intake
        .claim_for_form(&link, &entry, FormKind::Bill)
        .expect("claimed")
        .expect("now");
    assert_eq!(draft.to_string(), draft_text, "the arrival's own draft");
    assert_eq!(w.state_of("bill.pdf"), Some(RowState::OnForm));
    // A second form can't take it while the first is open …
    assert_eq!(
        w.intake
            .claim_for_form(&link, &entry, FormKind::Check)
            .err(),
        Some("inbox.on_form.hint")
    );
    assert!(w.intake.discard(&link, &entry).is_err());
    // … and can once it closed.
    drop(claim);
    assert_eq!(w.state_of("bill.pdf"), Some(RowState::Arrived));
    let (_, _, claim) = w
        .intake
        .claim_for_form(&link, &entry, FormKind::Bill)
        .expect("claimed")
        .expect("now");
    // The form is saved: the engine moves the draft's file onto the bill,
    // the save is published, and the original moves to Processed/2026-11.
    let bill =
        cl_core::ids::TransactionId::parse("d0000000-0000-4000-8000-b00000000001").expect("id");
    {
        let mut demo = w.link.demo_state().expect("demo");
        assert_eq!(
            crate::company::demo::checks::link_draft(&mut demo, draft, AttachTo::Bill(bill)),
            1
        );
    }
    w.link.bus().publish(Change::Ledger(None));
    drop(claim);
    w.until("the original processed", |s| s.processed == 1);
    assert!(!bills.join("bill.pdf").exists());
    assert!(
        bills
            .join("Processed")
            .join("2026-11")
            .join("bill.pdf")
            .exists()
    );
    assert_eq!(w.intake.state.entries(&w.intake.company), &[]);
    let _ = attachment;
}

#[test]
fn add_to_in_a_folder_that_keeps_originals_leaves_the_file() {
    let mut w = world();
    let (_, scans) = w.folder("Scans", true, true);
    std::fs::write(scans.join("scan.pdf"), PDF).expect("w");
    w.until("copied in", has("scan.pdf", RowState::Arrived));
    let entry = w.row("scan.pdf").entry.expect("entry");
    let check =
        cl_core::ids::TransactionId::parse("d0000000-0000-4000-8000-c00000000001").expect("id");
    let link = Arc::clone(&w.link);
    assert!(
        w.intake
            .add_to(&link, &entry, AttachTo::Expense(check))
            .is_ok()
    );
    w.until("hidden as handled", |s| {
        !s.rows().iter().any(|r| r.file.name == "scan.pdf")
    });
    let e = w
        .intake
        .state
        .entry(&w.intake.company, &entry)
        .cloned()
        .expect("kept entry");
    assert_eq!(e.status, EntryStatus::Kept);
    for _ in 0..10 {
        w.tick();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(scans.join("scan.pdf").exists(), "the original stays");
    assert!(!scans.join("Processed").exists());
    let on_check = crate::company::demo::checks::attachments(
        &w.link.demo_state().expect("demo"),
        AttachTo::Expense(check),
    );
    assert_eq!(on_check.len(), 1);
}

#[test]
fn discard_moves_the_original_then_deletes_only_the_copy() {
    let mut w = world();
    let (_, bills) = w.folder("Bills", false, true);
    std::fs::write(bills.join("junk.pdf"), PDF).expect("w");
    w.until("copied in", has("junk.pdf", RowState::Arrived));
    let entry = w.row("junk.pdf").entry.expect("entry");
    let attachment = w
        .intake
        .state
        .entry(&w.intake.company, &entry)
        .and_then(|e| e.attachment().map(str::to_owned))
        .and_then(|a| AttachmentId::parse(&a).ok())
        .expect("attachment");
    let link = Arc::clone(&w.link);
    assert!(w.intake.discard(&link, &entry).is_ok());
    assert_eq!(
        w.intake
            .state
            .entry(&w.intake.company, &entry)
            .map(|e| e.status),
        Some(EntryStatus::Discarding),
        "saved before the move"
    );
    w.until("discarded", |s| {
        s.discarded == 1 && s.state.entries(&s.company).is_empty()
    });
    assert!(!bills.join("junk.pdf").exists());
    assert!(
        bills.join("Discarded").join("junk.pdf").exists(),
        "never deleted"
    );
    assert!(
        crate::company::demo::checks::find_attachment(
            &w.link.demo_state().expect("demo"),
            attachment
        )
        .is_none(),
        "the engine's copy is gone"
    );
}

#[test]
fn unconfirmed_and_shared_folders_are_never_watched() {
    let mut w = world();
    let elsewhere = w.dir.path().join("Restored elsewhere");
    let (f, _) = w.folder("Scanner", false, false);
    std::fs::remove_dir(Path::new(&f.path)).expect("rm");
    // Another company on this computer confirmed this folder first.
    let (shared, _) = w.folder("Shared", false, false);
    let other = TenantId::parse("d0000000-0000-4000-8000-0000000000ff").expect("id");
    w.confirm(other, &shared);
    std::thread::sleep(Duration::from_millis(5));
    w.confirm(w.tenant, &shared);
    w.until("loaded", Intake::loaded);
    let health: Vec<Health> = w.intake.runs.iter().map(Run::health).collect();
    assert_eq!(health, [Health::NotConfirmed, Health::MappedByAnother]);
    assert!(w.intake.runs.iter().all(|r| !r.watched()));
    assert_eq!(w.intake.problems(), 2);
    assert!(!Path::new(&f.path).exists(), "never touched");
    assert!(!elsewhere.exists());
}

#[test]
fn the_p17_folder_moves_over_and_nothing_in_it_moves() {
    let mut w = world();
    let p17 = w.dir.path().join("Receipt inbox");
    let filed = p17.join("Filed").join("2026-10");
    std::fs::create_dir_all(&filed).expect("dir");
    std::fs::write(filed.join("a.pdf"), PDF).expect("w");
    std::fs::write(p17.join("root.png"), PNG).expect("w");
    w.link
        .store()
        .update(|l| {
            l.upsert(w.tenant, "Company").prefs.receipt_inbox_dir = Some(p17.clone());
        })
        .expect("store");
    w.until("migrated and listed", |s| {
        s.migration == Migration::Migrated && s.runs.first().is_some_and(|r| r.listing().is_some())
    });
    let rec = w
        .link
        .store()
        .load()
        .expect("load")
        .get(w.tenant)
        .cloned()
        .expect("rec");
    assert_eq!(rec.prefs.receipt_inbox_dir, None, "cleared after success");
    let engine = w.link.demo_state().expect("demo").intake.get(w.tenant);
    assert_eq!(engine.folders.len(), 1);
    assert_eq!(engine.folders[0].name, "Receipts");
    assert_eq!(engine.folders[0].path, p17.to_string_lossy());
    assert!(rec.prefs.is_confirmed(&engine.folders[0]));
    assert!(w.intake.runs[0].watched());
    // The root file is copied in like any new file; nothing moves.
    w.until("copied in", has("root.png", RowState::Arrived));
    assert!(p17.join("root.png").exists());
    assert!(filed.join("a.pdf").exists(), "Filed/ untouched");
    assert!(
        !w.intake.rows().iter().any(|r| r.file.name == "a.pdf"),
        "Filed/ is never listed"
    );
    assert_eq!(w.intake.problems(), 0);
}

#[test]
fn switching_companies_swaps_the_watchers() {
    let mut w = world();
    let (_, bills) = w.folder("Bills", false, true);
    std::fs::write(bills.join("bill.pdf"), PDF).expect("w");
    w.until("copied in", has("bill.pdf", RowState::Arrived));
    let other = w.link.demo_state().expect("demo").companies[1].tenant_id;
    tick_service(&mut w.intake, &w.link, Some(other), today(), Instant::now());
    assert!(w.intake.runs.is_empty(), "the other company has no folders");
    assert!(w.intake.rows().is_empty());
    tick_service(&mut w.intake, &w.link, None, today(), Instant::now());
    assert_eq!(w.intake.problems(), 0);
    assert!(!w.intake.loaded());
}

/// A crash between saving `uploading` and the reply: after the restart the
/// draft is listed first. Found → `arrived` without a second upload; none →
/// sent again into the same draft.
#[test]
fn an_upload_cut_short_is_resolved_by_its_draft() {
    let mut w = world();
    let (f, bills) = w.folder("Bills", false, true);
    std::fs::write(bills.join("went-up.pdf"), PDF).expect("w");
    std::fs::write(bills.join("never-sent.pdf"), b"%PDF-1.4\n%other bytes\n").expect("w");
    let company = company_key(&w.tenant.to_string());
    let store = InboxStateStore::in_dir(w.link.store().path().parent().expect("dir"));
    let identity = |name: &str| {
        let p = bills.join(name);
        let read = cl_company::intake::read_for_upload(&p).expect("read");
        FileIdentity {
            path_hash: path_hash(&p),
            size: read.size,
            modified_ms: read.modified_ms,
            sha256: read.sha256,
        }
    };
    let (up, lost) = (identity("went-up.pdf"), identity("never-sent.pdf"));
    let (d_up, d_lost) = (DraftKey::new(), DraftKey::new());
    // The engine got the first upload before the crash.
    crate::company::demo::checks::upload(
        &mut w.link.demo_state().expect("demo"),
        AttachTo::Draft(d_up),
        "went-up.pdf",
        PDF,
    )
    .expect("upload");
    store
        .update(|s| {
            s.begin_upload(&company, Some(f.id), &up, &d_up.to_string(), "k1");
            s.begin_upload(&company, Some(f.id), &lost, &d_lost.to_string(), "k2");
        })
        .expect("save");
    w.until("both arrived", |s| {
        s.state
            .entries(&s.company)
            .iter()
            .filter(|e| e.status == EntryStatus::Arrived)
            .count()
            == 2
    });
    let demo = w.link.demo_state().expect("demo");
    assert_eq!(
        crate::company::demo::checks::attachments(&demo, AttachTo::Draft(d_up)).len(),
        1,
        "no second upload"
    );
    assert_eq!(
        crate::company::demo::checks::attachments(&demo, AttachTo::Draft(d_lost)).len(),
        1,
        "sent again into the same draft"
    );
}

/// The attachment id of `name`'s entry.
fn attachment_of(w: &World, name: &str) -> AttachmentId {
    let entry = w.row(name).entry.expect("entry");
    w.intake
        .state
        .entry(&w.intake.company, &entry)
        .and_then(|e| e.attachment().map(str::to_owned))
        .and_then(|a| AttachmentId::parse(&a).ok())
        .expect("attachment")
}

/// A form saved the arrival onto a bill before the inbox noticed (or an
/// older entry after the upgrade): Discard never deletes the bill's
/// document (A01 review B1).
#[test]
fn discard_never_deletes_a_copy_a_saved_record_holds() {
    let mut w = world();
    let (_, bills) = w.folder("Bills", false, true);
    std::fs::write(bills.join("bill.pdf"), PDF).expect("w");
    w.until("copied in", has("bill.pdf", RowState::Arrived));
    let attachment = attachment_of(&w, "bill.pdf");
    let entry = w.row("bill.pdf").entry.expect("entry");
    let draft = w
        .intake
        .state
        .entry(&w.intake.company, &entry)
        .and_then(|e| e.draft.as_deref())
        .and_then(|d| DraftKey::parse(d).ok())
        .expect("draft");
    let bill =
        cl_core::ids::TransactionId::parse("d0000000-0000-4000-8000-b00000000002").expect("id");
    {
        let mut demo = w.link.demo_state().expect("demo");
        crate::company::demo::checks::link_draft(&mut demo, draft, AttachTo::Bill(bill));
    }
    let link = Arc::clone(&w.link);
    assert!(w.intake.discard(&link, &entry).is_ok());
    w.until("handled", |s| s.state.entries(&s.company).is_empty());
    let kept = crate::company::demo::checks::find_attachment(
        &w.link.demo_state().expect("demo"),
        attachment,
    )
    .expect("the bill keeps its document");
    assert_ne!(kept.attachable_type, "draft");
    assert_eq!(w.intake.problem, Some("inbox.discard.on_record"));
    assert!(
        bills.join("Discarded").join("bill.pdf").exists(),
        "the original is never deleted"
    );
}

/// The same in a folder that keeps its originals: the entry is kept.
#[test]
fn discard_in_a_keep_folder_keeps_a_copy_on_a_record() {
    let mut w = world();
    let (_, scans) = w.folder("Scans", true, true);
    std::fs::write(scans.join("scan.pdf"), PDF).expect("w");
    w.until("copied in", has("scan.pdf", RowState::Arrived));
    let attachment = attachment_of(&w, "scan.pdf");
    let entry = w.row("scan.pdf").entry.expect("entry");
    let draft = w
        .intake
        .state
        .entry(&w.intake.company, &entry)
        .and_then(|e| e.draft.as_deref())
        .and_then(|d| DraftKey::parse(d).ok())
        .expect("draft");
    let check =
        cl_core::ids::TransactionId::parse("d0000000-0000-4000-8000-c00000000002").expect("id");
    {
        let mut demo = w.link.demo_state().expect("demo");
        crate::company::demo::checks::link_draft(&mut demo, draft, AttachTo::Expense(check));
    }
    let link = Arc::clone(&w.link);
    assert!(w.intake.discard(&link, &entry).is_ok());
    w.until("kept", |s| {
        s.state.entry(&s.company, &entry).map(|e| e.status) == Some(EntryStatus::Kept)
    });
    assert!(
        crate::company::demo::checks::find_attachment(
            &w.link.demo_state().expect("demo"),
            attachment
        )
        .is_some()
    );
    assert!(scans.join("scan.pdf").exists());
}

/// A check of the startup sweep that fails puts its uploads back, to be
/// checked again later (never skipped).
#[test]
fn a_failed_sweep_check_puts_its_uploads_back() {
    let mut w = world();
    w.until("loaded", Intake::loaded);
    let now = Instant::now();
    w.intake.sweep = vec!["c".into()];
    w.intake
        .reconcile_done(vec!["a".into(), "b".into()], Err(ApiError::Timeout), now);
    assert_eq!(w.intake.sweep, ["a", "b", "c"]);
    // Not again at once.
    let link = Arc::clone(&w.link);
    w.intake.start_reconcile(&link, now);
    assert!(w.intake.reconcile.is_none());
    assert_eq!(w.intake.sweep.len(), 3);
    w.intake.start_reconcile(&link, now + RETRY);
    assert!(w.intake.reconcile.is_some(), "tried again later");
}

/// Two companies on this computer confirmed one folder at the same
/// moment (a restore on the same computer): exactly one watches it.
#[test]
fn a_tie_between_two_companies_pauses_exactly_one() {
    let mut w = world();
    let (shared, _) = w.folder("Shared", false, false);
    let other = TenantId::parse("d0000000-0000-4000-8000-0000000000ff").expect("id");
    let at = cl_settings::recent::now();
    w.confirm_at(other, &shared, at);
    w.confirm_at(w.tenant, &shared, at);
    w.until("loaded", Intake::loaded);
    let ours = w.intake.runs[0].health();
    let we_go_first = w.tenant < other;
    assert_eq!(
        ours == Health::MappedByAnother,
        !we_go_first,
        "the lower id goes first"
    );
    assert_eq!(w.intake.runs[0].watched(), we_go_first);
}

/// A watcher replaced or stopped takes its queued moves with it: they are
/// queued again on the new one instead of being skipped for ever (A01
/// review S2).
#[test]
fn moves_queued_on_a_stopped_watcher_are_queued_again() {
    let mut w = world();
    let (f, bills) = w.folder("Bills", false, true);
    std::fs::write(bills.join("bill.pdf"), PDF).expect("w");
    w.until("copied in", has("bill.pdf", RowState::Arrived));
    let entry = w.row("bill.pdf").entry.expect("entry");
    w.intake.moving.insert(entry.path_hash.clone());
    let link = Arc::clone(&w.link);
    w.intake.apply_folders(&link, Vec::new());
    assert!(w.intake.moving.is_empty(), "forgotten with its watcher");
    // Kept watchers keep theirs.
    w.intake.apply_folders(&link, vec![f.clone()]);
    w.intake.moving.insert(entry.path_hash.clone());
    w.intake.apply_folders(&link, vec![f]);
    assert!(w.intake.moving.contains(&entry.path_hash));
}

/// The same file sent again in a session keeps its idempotency key, so an
/// upload whose answer was lost can't land twice (A01 review S3).
#[test]
fn a_file_sent_again_keeps_its_upload_key() {
    let mut w = world();
    let (_, bills) = w.folder("Bills", false, true);
    w.until("loaded", Intake::loaded);
    let p = bills.join("bill.pdf");
    std::fs::write(&p, PDF).expect("w");
    let read = cl_company::intake::read_for_upload(&p).expect("read");
    let identity = FileIdentity {
        path_hash: path_hash(&p),
        size: read.size,
        modified_ms: read.modified_ms,
        sha256: read.sha256,
    };
    let key = IdempotencyKey::new();
    w.intake
        .upload_keys
        .insert(identity.path_hash.clone(), (identity.clone(), key.clone()));
    w.until("copied in", has("bill.pdf", RowState::Arrived));
    let e = w
        .intake
        .state
        .entry(&w.intake.company, &identity)
        .cloned()
        .expect("entry");
    assert_eq!(e.upload_key, Some(key.as_header()));
    assert!(w.intake.upload_keys.is_empty(), "done with once it arrived");
}

/// An upload cut short whose draft holds only a short copy: that copy
/// doesn't count; the file is sent again and the whole one is used (A01
/// review S4).
#[test]
fn a_short_copy_in_the_draft_is_not_taken_as_arrived() {
    let mut w = world();
    let (f, bills) = w.folder("Bills", false, true);
    let p = bills.join("bill.pdf");
    std::fs::write(&p, PDF).expect("w");
    let read = cl_company::intake::read_for_upload(&p).expect("read");
    let identity = FileIdentity {
        path_hash: path_hash(&p),
        size: read.size,
        modified_ms: read.modified_ms,
        sha256: read.sha256,
    };
    let draft = DraftKey::new();
    let short = crate::company::demo::checks::upload(
        &mut w.link.demo_state().expect("demo"),
        AttachTo::Draft(draft),
        "bill.pdf",
        &PDF[..12],
    )
    .expect("upload");
    let company = company_key(&w.tenant.to_string());
    InboxStateStore::in_dir(w.link.store().path().parent().expect("dir"))
        .update(|s| {
            s.begin_upload(&company, Some(f.id), &identity, &draft.to_string(), "k");
        })
        .expect("save");
    w.until("arrived", |s| {
        s.state.entry(&s.company, &identity).map(|e| e.status) == Some(EntryStatus::Arrived)
    });
    let e = w
        .intake
        .state
        .entry(&w.intake.company, &identity)
        .cloned()
        .expect("entry");
    assert_ne!(e.attachment(), Some(short.id.to_string().as_str()));
    let whole = crate::company::demo::checks::attachments(
        &w.link.demo_state().expect("demo"),
        AttachTo::Draft(draft),
    )
    .into_iter()
    .find(|a| Some(a.id.to_string().as_str()) == e.attachment())
    .expect("the whole copy");
    assert_eq!(whole.file_size, Some(PDF.len() as u64));
}

/// A read a hung share never answers is given up after the deadline: the
/// copy slot frees for the other folders and that folder waits a while
/// (A01 review S5).
#[test]
fn a_read_that_never_answers_frees_the_copy_slot() {
    let mut w = world();
    let (f, bills) = w.folder("Bills", false, true);
    w.until("loaded", Intake::loaded);
    let (tx, rx) = mpsc::channel();
    let file = IntakeFile {
        path: bills.join("stuck.pdf"),
        name: "stuck.pdf".into(),
        kind: cl_company::intake::DocKind::Pdf,
        size: 10,
        modified_ms: 1,
        state: ItemState::Ready,
    };
    let now = Instant::now();
    w.intake.copy = Some(Copy::Reading {
        folder: f.id,
        file: file.clone(),
        rx,
        resume: None,
        started: now,
    });
    let link = Arc::clone(&w.link);
    w.intake.poll_copy(&link, now + Duration::from_secs(1));
    assert!(w.intake.copy.is_some(), "still waiting before the deadline");
    w.intake.poll_copy(&link, now + SCAN_DEADLINE);
    assert!(w.intake.copy.is_none(), "given up");
    assert!(!w.intake.due(&path_hash(&file.path), now + SCAN_DEADLINE));
    let run = &w.intake.runs[0];
    assert!(!w.intake.answering(run, now + SCAN_DEADLINE));
    assert!(w.intake.answering(run, now + SCAN_DEADLINE + RETRY));
    drop(tx);
}

/// AC1 through the inbox service with the real timings: a file saved into
/// a watched folder is a draft attachment within 15 s
/// (`CL_INTAKE_AC1_SECS` on a slow runner).
#[test]
fn a_saved_file_arrives_within_15_seconds_with_real_timings() {
    let mut w = world();
    w.intake.watch_config = WatchConfig::default();
    let (_, bills) = w.folder("Bills", false, true);
    w.until("listed", |s| {
        s.runs.first().is_some_and(|r| r.listing().is_some())
    });
    let budget = Duration::from_secs(
        std::env::var("CL_INTAKE_AC1_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(15),
    );
    let saved = Instant::now();
    std::fs::write(bills.join("Ozark Feed bill.pdf"), PDF).expect("w");
    loop {
        w.tick();
        if has("Ozark Feed bill.pdf", RowState::Arrived)(&w.intake) {
            break;
        }
        assert!(
            saved.elapsed() <= budget,
            "not copied in within {budget:?}: {:?}",
            w.intake.rows()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(w.uploads().len(), 1);
}

#[test]
fn no_owner_data_in_debug() {
    let mut w = world();
    let (_, bills) = w.folder("Secret Vendor Bills", false, true);
    std::fs::write(bills.join("Secret Vendor INV-1.pdf"), PDF).expect("w");
    w.until(
        "copied in",
        has("Secret Vendor INV-1.pdf", RowState::Arrived),
    );
    let rows = format!("{:?}", w.intake.rows());
    assert!(!rows.contains("Secret"), "{rows}");
    let snap = format!("{:?}", w.intake.runs[0].snapshot);
    assert!(!snap.contains("Secret"), "{snap}");
}

// ---------------------------------------------------------------------------
// A04 WP-I: claims, Read all, auto-read, follow, claimed elsewhere, Add to….

mod drafts_flow {
    use std::sync::atomic::{AtomicU32, Ordering};

    use cl_api::{DocumentDraftId, DraftKind, DraftStatus};
    use cl_docread::model::{Bbox, Confidence, Found, Reader as Source, Reading, RuleId, Spot};
    use cl_host::{DocJob, JobOutcome};
    use cl_settings::inbox_state::{DraftLink, InboxEntry};

    use super::*;
    use crate::company::demo::drafts as demo_drafts;
    use crate::reading::service::{Reader, ReadingService};

    const OTHER: &[u8] = b"%PDF-1.4\n%another computer's scan\n";

    /// Reads every document as a bill of 12.50 (no AI, no connection).
    struct Fixed(Arc<AtomicU32>);

    impl Reader for Fixed {
        fn read(&mut self, jobs: &[DocJob], _today: CalendarDate) -> Vec<JobOutcome> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let mut r = Reading::empty(Source::TextPdf, 1);
            r.kind = cl_docread::model::DocKind::Bill;
            r.header.total = Some(Found {
                value: cl_core::Money::parse_wire("12.50").expect("money"),
                spot: Spot {
                    page: 0,
                    bbox: Bbox::new(10, 10, 20, 20).expect("box"),
                },
                rule: RuleId::new("test"),
                confidence: Confidence::HIGH,
            });
            jobs.iter()
                .map(|_| JobOutcome::Read(Box::new(r.clone())))
                .collect()
        }
    }

    fn reader(w: &mut World) -> Arc<AtomicU32> {
        let calls = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&calls);
        w.intake.drafts.reader = Some(Arc::new(ReadingService::start(Box::new(move || {
            Some(Box::new(Fixed(c)) as Box<dyn Reader>)
        }))));
        calls
    }

    impl World {
        fn entry_of(&self, name: &str) -> InboxEntry {
            let id = self.row(name).entry.expect("entry");
            self.intake
                .state
                .entry(&self.intake.company, &id)
                .cloned()
                .expect("entry")
        }

        fn own_draft(&self, name: &str) -> DocumentDraftId {
            match self.entry_of(name).doc_draft {
                DraftLink::Own(id) => DocumentDraftId::parse(&id).expect("id"),
                other => panic!("{name} owns no draft: {other:?}"),
            }
        }

        fn draft(&self, id: DocumentDraftId) -> cl_api::DocumentDraft {
            let demo = self.link.demo_state().expect("demo");
            demo_drafts::get(&demo, self.tenant, id).expect("draft")
        }

        /// Another computer saved `bytes` and claimed them: its draft.
        fn elsewhere(&self, bytes: &[u8]) -> DocumentDraftId {
            let mut demo = self.link.demo_state().expect("demo");
            let a = crate::company::demo::checks::upload(
                &mut demo,
                AttachTo::Draft(DraftKey::new()),
                "scan.pdf",
                bytes,
            )
            .expect("upload");
            demo_drafts::create(
                &mut demo,
                self.tenant,
                a.id,
                DraftKind::Unknown,
                IdempotencyKey::new(),
            )
            .expect("claim")
            .id
        }

        /// A posted demo check to attach to.
        fn a_check(&self) -> cl_core::ids::TransactionId {
            let mut demo = self.link.demo_state().expect("demo");
            super::super::super::demo::rows(
                &mut demo,
                crate::screens::receipt_inbox::PickKind::Check,
                None,
                "",
            )[0]
            .id
        }

        fn attach_elsewhere(&self, id: DocumentDraftId) {
            let txn = self.a_check();
            let mut demo = self.link.demo_state().expect("demo");
            let token = demo_drafts::get(&demo, self.tenant, id)
                .expect("draft")
                .updated_at;
            demo_drafts::attach(
                &mut demo,
                self.tenant,
                id,
                txn,
                &token,
                IdempotencyKey::new(),
            )
            .expect("attached elsewhere");
        }

        fn follow_now(&mut self) {
            self.intake.drafts.follow_now();
        }

        fn file_exists(&self, a: AttachmentId) -> bool {
            crate::company::demo::checks::find_attachment(&self.link.demo_state().expect("demo"), a)
                .is_some()
        }
    }

    fn linked(name: &'static str, link: &'static str) -> impl Fn(&Intake) -> bool {
        move |s: &Intake| {
            s.rows()
                .iter()
                .any(|r| r.file.name == name && r.link == link)
        }
    }

    fn attachment(e: &InboxEntry) -> AttachmentId {
        AttachmentId::parse(e.attachment().expect("upload")).expect("id")
    }

    /// Q-289: an arrival is claimed (auto-read or not); `claimSent` is on
    /// disk before anything is sent; the claim gate keeps the plain paths
    /// shut until the answer is known (T8).
    #[test]
    fn arrivals_are_claimed_and_the_claim_is_saved_before_it_is_sent() {
        let mut w = world();
        let (_, bills) = w.folder("Bills", false, true);
        std::fs::write(bills.join("bill.pdf"), PDF).expect("w");
        w.until("copied in", has("bill.pdf", RowState::Arrived));
        w.link.demo_state().expect("demo").drafts.enabled = true;
        w.intake.drafts_on = true;
        let link = Arc::clone(&w.link);
        let requests = || link.demo_state().expect("demo").drafts.requests;
        let before = requests();
        w.intake.next_claim(&link, Instant::now());
        w.intake.flush_state();
        let stored = w
            .intake
            .store
            .as_ref()
            .expect("store")
            .load()
            .expect("load");
        let id = w.row("bill.pdf").entry.expect("entry");
        assert_eq!(
            stored
                .entry(&w.intake.company, &id)
                .map(|e| e.doc_draft.clone()),
            Some(DraftLink::ClaimSent),
            "saved first"
        );
        assert_eq!(requests(), before, "nothing sent before it was saved");
        // Unknown answer: the plain paths are refused (T8).
        assert_eq!(
            w.intake.claim_for_form(&link, &id, FormKind::Bill).err(),
            Some("inbox.claim.pending")
        );
        assert_eq!(
            w.intake
                .add_to(&link, &id, AttachTo::Expense(w.a_check()))
                .err(),
            Some("inbox.claim.pending")
        );
        assert_eq!(
            w.intake.discard(&link, &id).err(),
            Some("inbox.claim.pending")
        );
        w.until("claimed", linked("bill.pdf", "own"));
        let draft = w.draft(w.own_draft("bill.pdf"));
        let e = w.entry_of("bill.pdf");
        assert_eq!(draft.attachment_id, Some(attachment(&e)));
        assert_eq!(draft.status, DraftStatus::New, "not auto-read: waiting");
        assert_eq!(draft.kind, DraftKind::Unknown, "an Auto folder asks");
        assert_eq!(w.state_of("bill.pdf"), Some(RowState::Arrived));
        // Claimed: New bill opens it from Review Documents, never the plain
        // save path.
        assert_eq!(
            w.intake.claim_for_form(&link, &id, FormKind::Bill).err(),
            Some("inbox.claim.review")
        );
    }

    /// A claim whose reply was lost, then a restart: claimed again with a
    /// fresh key and the draft it made is adopted (one draft, T4).
    #[test]
    fn a_restart_claims_again_and_adopts_its_own_draft() {
        let mut w = world();
        let (_, bills) = w.folder("Bills", false, true);
        std::fs::write(bills.join("bill.pdf"), PDF).expect("w");
        w.until("copied in", has("bill.pdf", RowState::Arrived));
        let id = w.row("bill.pdf").entry.expect("entry");
        let a = attachment(&w.entry_of("bill.pdf"));
        // The claim was saved and sent; it committed, the reply was lost.
        let made = {
            let mut demo = w.link.demo_state().expect("demo");
            demo.drafts.enabled = true;
            demo_drafts::create(
                &mut demo,
                w.tenant,
                a,
                DraftKind::Bill,
                IdempotencyKey::new(),
            )
            .expect("claim")
            .id
        };
        let c = w.intake.company.clone();
        assert!(w.intake.state.begin_claim(&c, &id));
        w.intake.save_state();
        w.intake.flush_state();
        // Restart.
        let mut fresh = Intake::new();
        fresh.watch_config = fast();
        w.intake = fresh;
        w.until("adopted", linked("bill.pdf", "own"));
        assert_eq!(w.own_draft("bill.pdf"), made);
        let demo = w.link.demo_state().expect("demo");
        let all =
            demo_drafts::list(&demo, w.tenant, &cl_api::DraftListQuery::default()).expect("list");
        assert_eq!(all.total, 1, "one draft");
    }

    /// T5: the same bytes are another computer's draft: the arrival follows
    /// it as a duplicate, its own extra copy is deleted (entry and seen hash
    /// kept), nothing goes up again, Copy anyway and Add to… are refused,
    /// and the original moves once the other computer entered it.
    #[test]
    fn claimed_elsewhere_follows_the_other_draft_until_it_is_entered() {
        let mut w = world_with_drafts();
        let winner = w.elsewhere(PDF);
        let winner_file = w.draft(winner).attachment_id.expect("file");
        let (_, bills) = w.folder("Bills", false, true);
        std::fs::write(bills.join("bill.pdf"), PDF).expect("w");
        w.until(
            "claimed elsewhere",
            has("bill.pdf", RowState::ClaimedElsewhere),
        );
        let e = w.entry_of("bill.pdf");
        assert_eq!(e.doc_draft, DraftLink::Follows(winner.to_string()));
        assert_eq!(e.status, EntryStatus::Duplicate);
        assert!(!e.attachments.iter().any(|a| *a == winner_file.to_string()));
        w.until("the extra copy deleted", |s| {
            s.state
                .entries(&s.company)
                .iter()
                .all(|e| e.attachments.is_empty())
        });
        assert!(w.file_exists(winner_file), "the winner's file is untouched");
        assert!(
            w.intake
                .state
                .seen(&w.intake.company, &e.file.sha256)
                .is_some()
        );
        for _ in 0..2 {
            w.follow_now();
            for _ in 0..10 {
                w.tick();
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let list = w.intake.state.entries(&w.intake.company);
        assert_eq!(list.len(), 1, "never uploaded again");
        assert!(list[0].attachments.is_empty() && list[0].status == EntryStatus::Duplicate);
        assert_eq!(w.state_of("bill.pdf"), Some(RowState::ClaimedElsewhere));
        assert_eq!(w.intake.badge(), 0, "not the owner's to enter here");
        let link = Arc::clone(&w.link);
        assert_eq!(
            w.intake.copy_anyway(&e.file).err(),
            Some("inbox.claim.elsewhere")
        );
        assert!(
            w.intake
                .add_to(&link, &e.file, AttachTo::Expense(w.a_check()))
                .is_err()
        );
        // Entered on the other computer: followed by id, the original moves.
        w.attach_elsewhere(winner);
        w.follow_now();
        w.until("processed", |s| s.processed == 1);
        assert!(
            bills
                .join("Processed")
                .join("2026-11")
                .join("bill.pdf")
                .exists()
        );
        assert!(
            w.file_exists(winner_file),
            "the winner's file stays on its record"
        );
        w.until("forgotten", |s| s.state.entries(&s.company).is_empty());
    }

    /// M3: discarding a file another computer's draft holds moves only this
    /// computer's file; that draft is never discarded.
    #[test]
    fn discarding_a_followed_file_leaves_the_other_computers_draft() {
        let mut w = world_with_drafts();
        let winner = w.elsewhere(PDF);
        let (_, bills) = w.folder("Bills", false, true);
        std::fs::write(bills.join("bill.pdf"), PDF).expect("w");
        w.until(
            "claimed elsewhere",
            has("bill.pdf", RowState::ClaimedElsewhere),
        );
        let id = w.row("bill.pdf").entry.expect("entry");
        let link = Arc::clone(&w.link);
        let requests = link.demo_state().expect("demo").drafts.requests;
        assert!(w.intake.discard(&link, &id).is_ok());
        w.until("discarded", |s| s.discarded == 1);
        assert!(
            bills.join("Discarded").join("bill.pdf").exists(),
            "never deleted"
        );
        assert_eq!(
            w.draft(winner).status,
            DraftStatus::New,
            "still waiting there"
        );
        assert_eq!(
            link.demo_state().expect("demo").drafts.requests,
            requests,
            "no discard sent"
        );
    }

    /// D2: an own draft is discarded in the engine first, then the original
    /// moves and the copy goes; a draft entered elsewhere meanwhile is
    /// handled instead.
    #[test]
    fn discarding_an_own_draft_discards_it_in_the_engine_first() {
        let mut w = world_with_drafts();
        let (_, bills) = w.folder("Bills", false, true);
        std::fs::write(bills.join("junk.pdf"), PDF).expect("w");
        std::fs::write(bills.join("entered.pdf"), OTHER).expect("w");
        w.until("claimed", |s| {
            linked("junk.pdf", "own")(s) && linked("entered.pdf", "own")(s)
        });
        let junk = w.own_draft("junk.pdf");
        let copy = attachment(&w.entry_of("junk.pdf"));
        let link = Arc::clone(&w.link);
        let id = w.row("junk.pdf").entry.expect("entry");
        assert!(w.intake.discard(&link, &id).is_ok());
        w.until("discarded", |s| s.discarded == 1);
        assert_eq!(w.draft(junk).status, DraftStatus::Discarded);
        assert!(bills.join("Discarded").join("junk.pdf").exists());
        w.until("copy deleted", |s| s.state.entry(&s.company, &id).is_none());
        assert!(!w.file_exists(copy));
        // Entered on another computer before the owner's Discard here.
        let entered = w.own_draft("entered.pdf");
        w.attach_elsewhere(entered);
        let id = w.row("entered.pdf").entry.expect("entry");
        assert!(w.intake.discard(&link, &id).is_ok());
        w.until("processed instead", |s| s.processed == 1);
        assert_eq!(w.intake.problem, Some("inbox.discard.entered"));
        assert!(
            bills
                .join("Processed")
                .join("2026-11")
                .join("entered.pdf")
                .exists()
        );
    }

    /// D3: Add to… on a claimed file goes through the engine's attach: the
    /// draft closes on that record in one call (never a plain link).
    #[test]
    fn add_to_on_a_claimed_file_attaches_its_draft() {
        let mut w = world_with_drafts();
        let (_, bills) = w.folder("Bills", false, true);
        std::fs::write(bills.join("receipt.pdf"), PDF).expect("w");
        w.until("claimed", linked("receipt.pdf", "own"));
        let draft = w.own_draft("receipt.pdf");
        let file = attachment(&w.entry_of("receipt.pdf"));
        let check = w.a_check();
        let link = Arc::clone(&w.link);
        let id = w.row("receipt.pdf").entry.expect("entry");
        assert!(
            w.intake
                .add_to(&link, &id, AttachTo::Expense(check))
                .is_ok()
        );
        w.until("processed", |s| s.processed == 1);
        let d = w.draft(draft);
        assert_eq!(
            (d.status, d.converted_transaction_id),
            (DraftStatus::Converted, Some(check))
        );
        let on_check = crate::company::demo::checks::find_attachment(
            &w.link.demo_state().expect("demo"),
            file,
        )
        .expect("file");
        assert_eq!(on_check.attachable_id, check.to_string());
        assert!(
            bills
                .join("Processed")
                .join("2026-11")
                .join("receipt.pdf")
                .exists()
        );
        // An engine without attach: refused, never a plain link.
        assert_eq!(
            drafts::target_transaction(AttachTo::Draft(DraftKey::new())),
            None
        );
    }

    /// D6: followed by id; entered → Processed, discarded elsewhere →
    /// Discarded with the copy deleted, gone (drafts reset) → claimed again
    /// once (T6).
    #[test]
    fn follow_handles_entered_discarded_and_gone_drafts() {
        let mut w = world_with_drafts();
        let (_, bills) = w.folder("Bills", false, true);
        std::fs::write(bills.join("a.pdf"), PDF).expect("w");
        std::fs::write(bills.join("b.pdf"), OTHER).expect("w");
        w.until("claimed", |s| {
            linked("a.pdf", "own")(s) && linked("b.pdf", "own")(s)
        });
        let (a, b) = (w.own_draft("a.pdf"), w.own_draft("b.pdf"));
        let b_copy = attachment(&w.entry_of("b.pdf"));
        w.attach_elsewhere(a);
        {
            let mut demo = w.link.demo_state().expect("demo");
            demo_drafts::discard(&mut demo, w.tenant, b).expect("discarded elsewhere");
        }
        w.follow_now();
        w.until("moved", |s| s.processed == 1 && s.discarded == 1);
        assert!(
            bills
                .join("Processed")
                .join("2026-11")
                .join("a.pdf")
                .exists()
        );
        assert!(bills.join("Discarded").join("b.pdf").exists());
        w.until("b's copy deleted", |s| {
            s.state.entries(&s.company).is_empty()
        });
        assert!(!w.file_exists(b_copy));
        // Gone: the engine's drafts were reset (a restore).
        std::fs::write(bills.join("c.pdf"), b"%PDF-1.4\n%c\n").expect("w");
        w.until("claimed", linked("c.pdf", "own"));
        let old = w.own_draft("c.pdf");
        w.link.demo_state().expect("demo").drafts = demo_drafts::DemoDrafts::on();
        w.follow_now();
        w.until("claimed again", |s| {
            s.rows().iter().any(|r| {
                r.file.name == "c.pdf"
                    && r.link == "own"
                    && s.state
                        .entries(&s.company)
                        .iter()
                        .any(|e| e.doc_draft.id().is_some_and(|d| d != old.to_string()))
            })
        });
        let demo = w.link.demo_state().expect("demo");
        let all =
            demo_drafts::list(&demo, w.tenant, &cl_api::DraftListQuery::default()).expect("list");
        assert_eq!(all.total, 1, "claimed once, no double");
    }

    /// Auto-read folders read their arrivals locally once claimed; Read all
    /// reads every `new` draft (another computer's too), skipping a draft
    /// open in a form until it closes; "no reader" stores nothing.
    #[test]
    fn auto_read_and_read_all_read_on_this_computer() {
        let mut w = world_with_drafts();
        let calls = reader(&mut w);
        let (_, auto) = w.folder_with("Auto", false, true, true);
        let (_, plain) = w.folder("Plain", false, true);
        std::fs::write(auto.join("auto.pdf"), PDF).expect("w");
        w.until("auto-read", |s| s.drafts.stored == 1);
        let d = w.draft(w.own_draft("auto.pdf"));
        assert_eq!(d.status, DraftStatus::Ready);
        assert_eq!(d.kind, DraftKind::Bill, "a bills folder");
        assert_eq!(d.doc_total, cl_core::Money::parse_wire("12.50").ok());
        // Not auto-read: waits for Read all.
        std::fs::write(plain.join("plain.pdf"), OTHER).expect("w");
        w.until("claimed", linked("plain.pdf", "own"));
        let plain_draft = w.own_draft("plain.pdf");
        let theirs = w.elsewhere(b"%PDF-1.4\n%theirs\n");
        for _ in 0..5 {
            w.tick();
        }
        assert_eq!(w.draft(plain_draft).status, DraftStatus::New);
        // Open in a form: skipped, and read once the form closes.
        let held = w.intake.hold_draft(theirs);
        let link = Arc::clone(&w.link);
        assert!(w.intake.read_all(&link));
        w.until("plain read", |s| s.drafts.stored == 2);
        assert_eq!(
            w.draft(theirs).status,
            DraftStatus::New,
            "skipped while open"
        );
        assert!(w.intake.drafts.busy(), "still queued");
        drop(held);
        w.until("theirs read", |s| s.drafts.stored == 3);
        assert_eq!(w.draft(theirs).status, DraftStatus::Ready);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        w.until("idle", |s| s.read_progress().is_none() && !s.drafts.busy());
        // No reader on this computer: nothing is stored, the draft waits.
        w.intake.drafts.reader = Some(Arc::new(ReadingService::start(Box::new(|| None))));
        let waiting = w.elsewhere(b"%PDF-1.4\n%waiting\n");
        assert!(w.intake.read_all(&link));
        w.until("tried", |s| !s.drafts.busy());
        assert_eq!(w.draft(waiting).status, DraftStatus::New);
        assert_eq!(w.intake.read_counts(), (3, 0));
        // Cancel before it starts: nothing read.
        assert!(w.intake.read_all(&link));
        w.intake.cancel_reads();
        for _ in 0..5 {
            w.tick();
        }
        assert_eq!(w.draft(waiting).status, DraftStatus::New);
    }

    /// S17 / T-crash: a convert's 201 hands the upload back; and if the app
    /// stopped before that, the opening sweep finds it on its record.
    #[test]
    fn a_convert_moves_the_original_once_even_after_a_crash() {
        let mut w = world_with_drafts();
        let (_, bills) = w.folder("Bills", false, true);
        std::fs::write(bills.join("a.pdf"), PDF).expect("w");
        std::fs::write(bills.join("b.pdf"), OTHER).expect("w");
        w.until("claimed", |s| {
            linked("a.pdf", "own")(s) && linked("b.pdf", "own")(s)
        });
        let (a, b) = (w.own_draft("a.pdf"), w.own_draft("b.pdf"));
        let a_file = attachment(&w.entry_of("a.pdf"));
        // Entered here (the form's convert answered 201).
        w.attach_elsewhere(a);
        assert!(w.intake.handled_by_attachment(a_file));
        assert!(!w.intake.handled_by_attachment(a_file), "once");
        w.until("a processed", |s| s.processed == 1);
        // Entered, then the app stopped before the inbox heard of it.
        w.attach_elsewhere(b);
        w.intake.flush_state();
        let mut fresh = Intake::new();
        fresh.watch_config = fast();
        w.intake = fresh;
        w.until("b processed after the restart", |s| s.processed == 1);
        assert!(
            bills
                .join("Processed")
                .join("2026-11")
                .join("b.pdf")
                .exists()
        );
    }

    /// D1 "Legacy": a P17 upload shares its form's draft key: it is linked
    /// alone under a fresh key before its claim, so a convert moves only it.
    #[test]
    fn a_p17_upload_gets_its_own_draft_key_before_its_claim() {
        let mut w = world_with_drafts();
        w.until("running", |s| matches!(s.stage, Stage::Running));
        let shared = DraftKey::new();
        let (mine, other) = {
            let mut demo = w.link.demo_state().expect("demo");
            let up = |demo: &mut crate::company::demo::DemoState, b: &[u8]| {
                crate::company::demo::checks::upload(demo, AttachTo::Draft(shared), "x.pdf", b)
                    .expect("upload")
                    .id
            };
            (up(&mut demo, PDF), up(&mut demo, OTHER))
        };
        let file = FileIdentity {
            path_hash: "p17".into(),
            size: PDF.len() as u64,
            modified_ms: 1,
            sha256: "aa".into(),
        };
        let c = w.intake.company.clone();
        w.intake.state.record_upload(&c, &file, &mine.to_string());
        w.until("claimed", |s| {
            matches!(
                s.state.entry(&s.company, &file).map(|e| &e.doc_draft),
                Some(DraftLink::Own(_))
            )
        });
        let e = w.intake.state.entry(&c, &file).cloned().expect("entry");
        let key = DraftKey::parse(e.draft.as_deref().expect("key")).expect("key");
        assert_ne!(key, shared);
        let demo = w.link.demo_state().expect("demo");
        let alone: Vec<AttachmentId> =
            crate::company::demo::checks::attachments(&demo, AttachTo::Draft(key))
                .iter()
                .map(|a| a.id)
                .collect();
        assert_eq!(alone, [mine]);
        let left: Vec<AttachmentId> =
            crate::company::demo::checks::attachments(&demo, AttachTo::Draft(shared))
                .iter()
                .map(|a| a.id)
                .collect();
        assert_eq!(left, [other], "the form's other file stays");
    }

    /// 404 on the claim (the upload is gone, a restore): uploaded again
    /// once and claimed.
    #[test]
    fn a_claim_of_a_vanished_upload_uploads_it_again_once() {
        let mut w = world();
        let (_, bills) = w.folder("Bills", false, true);
        std::fs::write(bills.join("bill.pdf"), PDF).expect("w");
        w.until("copied in", has("bill.pdf", RowState::Arrived));
        let gone = attachment(&w.entry_of("bill.pdf"));
        {
            let mut demo = w.link.demo_state().expect("demo");
            crate::company::demo::checks::delete_attachment(&mut demo, gone);
            demo.drafts.enabled = true;
        }
        w.until("claimed", linked("bill.pdf", "own"));
        let now = attachment(&w.entry_of("bill.pdf"));
        assert_ne!(now, gone, "uploaded again");
        assert_eq!(w.draft(w.own_draft("bill.pdf")).attachment_id, Some(now));
    }

    /// Nothing about the file in `Debug` or the control channel's links.
    #[test]
    fn no_owner_data_in_draft_state() {
        let mut w = world_with_drafts();
        let (_, bills) = w.folder("Secret Vendor Bills", false, true);
        std::fs::write(bills.join("Secret Vendor INV-1.pdf"), PDF).expect("w");
        w.until("claimed", linked("Secret Vendor INV-1.pdf", "own"));
        let rows = format!("{:?}", w.intake.rows());
        assert!(!rows.contains("Secret"), "{rows}");
        w.intake.flush_state();
        let text =
            std::fs::read_to_string(w.intake.store.as_ref().expect("store").path()).expect("state");
        assert!(!text.contains("Secret"), "{text}");
        assert!(text.contains("docDraft"), "{text}");
    }
}
