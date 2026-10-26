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
    World {
        dir,
        link,
        tenant,
        intake,
    }
}

impl World {
    /// A folder in the demo engine (confirmed here unless `here` is false).
    fn folder(&self, name: &str, keep: bool, here: bool) -> (IntakeFolder, PathBuf) {
        let path = self.dir.path().join(name);
        std::fs::create_dir_all(&path).expect("dir");
        let f = IntakeFolder {
            id: intake_folders::new_folder_id(),
            name: name.to_owned(),
            path: path.to_string_lossy().into_owned(),
            default_kind: IntakeKind::Auto,
            auto_read: false,
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
                        confirmed_at: Some(cl_settings::recent::now()),
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
