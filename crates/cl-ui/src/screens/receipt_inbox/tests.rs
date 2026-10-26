//! Receipt inbox window tests (no engine) for P17's single folder: the
//! demo stand-in acts as an engine **without** `intake-settings-v1`
//! (Q-A01q: on the current pin the P17 inbox must keep working exactly as
//! before) and takes the uploads; the folder is a temporary one (or the
//! demo's own). The intake-folder inbox is tested in `intake_tests`.

use std::time::{Duration, Instant};

use super::*;
use cl_settings::inbox_state::{InboxStateStore, company_key};

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn link() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    link.demo_state().expect("demo").intake.needs_engine = true;
    (dir, link)
}

/// Without intake settings the window is P17's: its state says so, and
/// the shell's tick runs P17's service (no intake service is made).
#[test]
fn without_intake_settings_the_p17_inbox_runs() {
    let (_dir, link) = link();
    let ctx = context();
    let mut s = screen(&link);
    frame(&ctx, &mut s);
    assert!(!s.intake_mode);
    assert_eq!(s.state()["mode"], Value::Null, "P17 state: {}", s.state());
    assert!(super::intake::existing(&ctx).is_none());
    assert!(!intake_active(&ctx, &link));
}

fn tenant(link: &CompanyLink) -> (TenantId, String) {
    let demo = link.demo_state().expect("demo");
    let c = &demo.companies[0];
    (c.tenant_id, c.name.clone())
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    ctx
}

fn screen(link: &Arc<CompanyLink>) -> ReceiptInboxScreen {
    ReceiptInboxScreen::new(Arc::clone(link), Some(tenant(link)), today())
}

/// One frame of the window.
fn frame(ctx: &egui::Context, s: &mut ReceiptInboxScreen) {
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| s.ui(ui));
    out.textures_delta.clear();
}

/// Frames until `done` holds (the listing thread needs real time: a file
/// is ready after two listings 2 s apart).
fn until(
    ctx: &egui::Context,
    s: &mut ReceiptInboxScreen,
    what: &str,
    done: impl Fn(&ReceiptInboxScreen) -> bool,
) {
    let start = Instant::now();
    loop {
        frame(ctx, s);
        if done(s) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "timed out waiting for {what}: {}",
            s.state()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn file_named(s: &ReceiptInboxScreen, ctx: &egui::Context, name: &str) -> InboxFile {
    let h = service::handle(ctx);
    let svc = h.lock();
    svc.listing()
        .and_then(|l| l.files.iter().find(|f| f.name == name).cloned())
        .unwrap_or_else(|| panic!("{name} listed: {}", s.state()))
}

fn ready(s: &ReceiptInboxScreen) -> bool {
    s.state()["ready"] == 2
}

#[test]
fn the_demo_inbox_lists_filters_and_waits_until_files_settle() {
    let (_dir, link) = link();
    let ctx = context();
    let mut s = screen(&link);
    frame(&ctx, &mut s);
    until(&ctx, &mut s, "the first listing", |s| {
        s.state()["folder"] == "ok"
    });
    let first = s.state();
    assert_eq!(first["listed"], 3, "notes.txt is left out: {first}");
    assert_eq!(first["too_large"], 1);
    until(&ctx, &mut s, "two ready files", ready);
    let st = s.state();
    assert_eq!(st["badge"], 2);
    assert_eq!(badge(&ctx), 2, "the menu's count");
    assert_eq!(st["selected"], false);
    // No file names in the control channel's view.
    let text = st.to_string();
    for (name, _) in demo::SEED {
        assert!(!text.contains(name), "{text}");
    }
    assert!(!text.contains(demo::LARGE));
    assert_eq!(s.title(), "Receipt Inbox");
    assert_eq!(s.scope(), Scope::Window);
}

#[test]
fn a_too_large_file_is_never_read() {
    let (_dir, link) = link();
    let ctx = context();
    let mut s = screen(&link);
    until(&ctx, &mut s, "two ready files", ready);
    let large = file_named(&s, &ctx, demo::LARGE);
    assert_eq!(large.state, FileState::TooLarge);
    s.select(&large);
    assert!(s.reading.is_none(), "not read");
    frame(&ctx, &mut s);
    assert_eq!(s.state()["loaded"], false);
}

#[test]
fn add_to_a_check_attaches_and_files_the_original_away() {
    let (_dir, link) = link();
    let ctx = context();
    let mut s = screen(&link);
    until(&ctx, &mut s, "two ready files", ready);
    let png = file_named(&s, &ctx, "Ozark Feed receipt.png");
    s.select(&png);
    until(&ctx, &mut s, "the file read", |s| {
        s.state()["loaded"] == true
    });
    assert_eq!(s.state()["selected_kind"], "png");
    assert_eq!(s.state()["preview"]["open"], true);

    s.picker = Some(picker::Picker::new(PickKind::Check));
    until(&ctx, &mut s, "the checks", |s| {
        s.picker.as_ref().is_some_and(|p| p.loaded)
    });
    let row = s
        .picker
        .as_ref()
        .and_then(|p| p.rows.first().cloned())
        .expect("a demo check");
    assert_eq!(row.txn_type, cl_api::TxnType::Expense);
    s.start_upload(Goal::Record(PickKind::Check, row.clone()));
    until(&ctx, &mut s, "the original filed", |s| {
        s.state()["filed"] == 1
    });
    let st = s.state();
    assert_eq!(st["problem"], Value::Null, "{st}");
    assert_eq!(st["notice"], true);
    assert_eq!(
        st["selected"], false,
        "the filed file is no longer selected"
    );
    let inbox = super::demo::folder(&link).expect("folder");
    assert!(!png.path.exists(), "moved");
    let filed = cl_company::inbox::filed_dir(&inbox, 2026, 11).join("Ozark Feed receipt.png");
    assert!(filed.exists(), "in Filed/2026-11");
    let on_check = crate::company::demo::checks::attachments(
        &link.demo_state().expect("demo"),
        AttachTo::Expense(row.id),
    );
    assert!(
        on_check
            .iter()
            .any(|a| a.file_name == "Ozark Feed receipt.png"),
        "the check holds the file"
    );
    until(&ctx, &mut s, "one file left", |s| s.state()["badge"] == 1);
}

/// Every interactive widget of the inbox has an accessible name: the list,
/// a selected file with its preview and actions, and the record picker.
#[test]
fn every_inbox_widget_has_an_accessible_name() {
    let (_dir, link) = link();
    let ctx = context();
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    let mut s = screen(&link);
    until(&ctx, &mut s, "two ready files", ready);
    let check = |s: &mut ReceiptInboxScreen, what: &str| {
        let mut nodes = Vec::new();
        for _ in 0..3 {
            let mut out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1440.0, 940.0),
                    )),
                    ..egui::RawInput::default()
                },
                |ui| s.ui(ui),
            );
            out.textures_delta.clear();
            nodes.extend(
                out.platform_output
                    .accesskit_update
                    .map(|u| u.nodes)
                    .unwrap_or_default(),
            );
        }
        assert!(
            nodes
                .iter()
                .any(|(_, n)| n.role() == egui::accesskit::Role::Button),
            "{what}: drawn"
        );
        let unnamed = crate::screens::unnamed_widgets(&nodes);
        assert!(unnamed.is_empty(), "{what}: unnamed widgets: {unnamed:?}");
    };
    check(&mut s, "list");
    let png = file_named(&s, &ctx, "Ozark Feed receipt.png");
    s.select(&png);
    until(&ctx, &mut s, "the file read", |s| {
        s.state()["loaded"] == true
    });
    check(&mut s, "selected");
    s.picker = Some(picker::Picker::new(PickKind::Check));
    until(&ctx, &mut s, "the checks", |s| {
        s.picker.as_ref().is_some_and(|p| p.loaded)
    });
    check(&mut s, "picker");
}

#[test]
fn new_check_leaves_the_file_until_a_saved_check_holds_it() {
    let (_dir, link) = link();
    let ctx = context();
    let mut s = screen(&link);
    until(&ctx, &mut s, "two ready files", ready);
    let pdf = file_named(&s, &ctx, demo::SEED[0].0);
    s.select(&pdf);
    until(&ctx, &mut s, "the file read", |s| {
        s.state()["loaded"] == true
    });
    let draft = DraftKey::new();
    s.start_upload(Goal::NewCheck(draft));
    frame(&ctx, &mut s);
    let opened = link.drain().into_iter().find_map(|e| match e {
        CompanyEvent::OpenWith(kind, payload) => match *payload {
            OpenPayload::InboxDraft(d) => Some((kind, d)),
            _ => None,
        },
        _ => None,
    });
    let (kind, handed) = opened.expect("Write Check opened with the draft");
    assert_eq!(kind.as_str(), WRITE_CHECK);
    assert_eq!(handed.draft, draft);
    assert_eq!(handed.file.name, demo::SEED[0].0);
    assert_eq!(&*handed.file.bytes, demo::SEED[0].1, "the bytes read");
    assert!(!format!("{handed:?}").contains(".pdf"), "redacted Debug");

    // Write Check takes it as its draft.
    let mut check = crate::screens::write_check::WriteCheckScreen::new(
        Arc::clone(&link),
        Some(tenant(&link)),
        today(),
    );
    assert!(check.open_with(OpenPayload::InboxDraft(handed.clone())));
    assert!(check.dirty(), "a check with a file is unsaved work");
    assert!(
        !check.open_with(OpenPayload::InboxDraft(handed.clone())),
        "one draft file only"
    );

    // The form is not saved: the file stays, marked, and is still offered.
    until(&ctx, &mut s, "the upload checked", |s| {
        s.state()["uploaded"] == 1
    });
    for _ in 0..5 {
        frame(&ctx, &mut s);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        pdf.path.exists(),
        "an unsaved form leaves the file in the inbox"
    );
    assert_eq!(s.state()["filed"], 0);
    assert_eq!(
        s.state()["badge"],
        1,
        "only never-uploaded files are counted"
    );

    // The check is saved: the engine moves the draft's file onto it, and
    // the next check of the uploads files the original away.
    let check_id =
        cl_core::ids::TransactionId::parse("d0000000-0000-4000-8000-c00000000001").expect("id");
    {
        let mut demo = link.demo_state().expect("demo");
        assert_eq!(
            crate::company::demo::checks::link_draft(&mut demo, draft, AttachTo::Expense(check_id)),
            1
        );
    }
    service::handle(&ctx).lock().poke();
    until(&ctx, &mut s, "the original filed", |s| {
        s.state()["filed"] == 1
    });
    assert!(!pdf.path.exists());
}

#[test]
fn new_bill_hands_the_draft_to_enter_bill() {
    let (_dir, link) = link();
    let ctx = context();
    let mut s = screen(&link);
    until(&ctx, &mut s, "two ready files", ready);
    let pdf = file_named(&s, &ctx, demo::SEED[0].0);
    s.select(&pdf);
    until(&ctx, &mut s, "the file read", |s| {
        s.state()["loaded"] == true
    });
    s.start_upload(Goal::NewBill(DraftKey::new()));
    frame(&ctx, &mut s);
    let kinds: Vec<String> = link
        .drain()
        .into_iter()
        .filter_map(|e| match e {
            CompanyEvent::OpenWith(kind, p) if matches!(*p, OpenPayload::InboxDraft(_)) => {
                Some(kind.as_str().to_string())
            }
            _ => None,
        })
        .collect();
    assert_eq!(kinds, [ENTER_BILL]);
    let store = InboxStateStore::in_dir(link.store().path().parent().expect("dir"));
    let company = company_key(&tenant(&link).0.to_string());
    let state = store.load().expect("load");
    let entry = state
        .find(&company, &path_hash(&pdf.path), pdf.size, pdf.modified_ms)
        .expect("recorded");
    assert!(!entry.linked);
    let id = AttachmentId::parse(&entry.attachments[0]).expect("id");
    let up = crate::company::demo::checks::find_attachment(&link.demo_state().expect("demo"), id)
        .expect("uploaded");
    assert_eq!(up.attachable_type, "draft");
    assert_eq!(up.file_size, Some(demo::SEED[0].1.len() as u64));
}

#[test]
fn stale_uploads_after_a_restore_do_not_hide_files() {
    let (_dir, link) = link();
    let ctx = context();
    let mut s = screen(&link);
    until(&ctx, &mut s, "two ready files", ready);
    let png = file_named(&s, &ctx, "Ozark Feed receipt.png");
    // An entry from before a restore: linked to an upload this company's
    // books no longer have (the move failed then).
    let read = cl_company::inbox::read_for_upload(&png.path).expect("read");
    let store = InboxStateStore::in_dir(link.store().path().parent().expect("dir"));
    let company = company_key(&tenant(&link).0.to_string());
    let identity = FileIdentity {
        path_hash: path_hash(&png.path),
        size: read.size,
        modified_ms: read.modified_ms,
        sha256: read.sha256,
    };
    store
        .update(|st| {
            st.record_upload(&company, &identity, "d0000000-0000-4000-8000-0000000fffff");
            st.mark_linked(&company, &identity);
        })
        .expect("save");
    // A new window on a fresh service reads the state, checks the upload
    // with the engine and forgets it: the file is offered again.
    let ctx = context();
    let mut s = screen(&link);
    until(&ctx, &mut s, "the listing", |s| s.state()["listed"] == 3);
    until(&ctx, &mut s, "the stale upload forgotten", |s| {
        s.state()["linked"] == 0 && s.state()["ready"] == 2
    });
    until(&ctx, &mut s, "both files counted", |s| {
        s.state()["badge"] == 2
    });
    assert!(png.path.exists(), "never moved: the books don't hold it");
    assert_eq!(
        store.load().expect("load").entries(&company).len(),
        0,
        "the stale entry is gone"
    );
}

#[test]
fn without_a_folder_the_inbox_points_to_preferences() {
    let (_dir, link) = link();
    let ctx = context();
    let mut s = screen(&link);
    s.demo_folder_used = true; // as a real company: no demo folder
    frame(&ctx, &mut s);
    assert_eq!(s.state()["folder"], "not_chosen");
    assert_eq!(s.state()["listed"], 0);
    s.choose_folder();
    let events = link.drain();
    assert!(events.iter().any(|e| matches!(
        e,
        CompanyEvent::OpenWindow(k) if k.as_str() == crate::screens::preferences::COMMAND
    )));
    let prefs = crate::screens::preferences::make(
        CommandId::new(crate::screens::preferences::COMMAND),
        &link,
        Some(tenant(&link)),
        cl_settings::UserPrefs::default(),
    )
    .expect("preferences");
    assert_eq!(prefs.state()["category"], "inbox", "opens on Receipt inbox");
}

#[test]
fn a_chosen_folder_that_is_missing_or_a_network_path_says_so() {
    let (dir, link) = link();
    let (id, _) = tenant(&link);
    let missing = dir.path().join("Receipts that are not there");
    link.store()
        .update(|l| l.upsert(id, "Ozark").prefs.receipt_inbox_dir = Some(missing))
        .expect("save");
    let ctx = context();
    let mut s = screen(&link);
    s.demo_folder_used = true;
    until(&ctx, &mut s, "the missing folder", |s| {
        s.state()["folder"] == "missing"
    });
    link.store()
        .update(|l| {
            l.upsert(id, "Ozark").prefs.receipt_inbox_dir =
                Some(PathBuf::from("\\\\server\\share\\Receipts"));
        })
        .expect("save");
    let ctx = context();
    let mut s = screen(&link);
    s.demo_folder_used = true;
    frame(&ctx, &mut s);
    assert_eq!(s.state()["folder"], "refused");
    // An empty folder.
    let empty = dir.path().join("Empty inbox");
    std::fs::create_dir(&empty).expect("dir");
    link.store()
        .update(|l| l.upsert(id, "Ozark").prefs.receipt_inbox_dir = Some(empty))
        .expect("save");
    let ctx = context();
    let mut s = screen(&link);
    s.demo_folder_used = true;
    until(&ctx, &mut s, "the empty folder", |s| {
        s.state()["folder"] == "ok"
    });
    assert_eq!(s.state()["listed"], 0);
}

#[test]
fn texts_for_sizes_dates_and_records() {
    assert_eq!(size_text(1), "1 KB");
    assert_eq!(size_text(1024), "1 KB");
    assert_eq!(size_text(1025), "2 KB");
    assert_eq!(size_text(1024 * 1024), "1.0 MB");
    assert_eq!(size_text(10 * 1024 * 1024 + 1), "10.0 MB");
    assert_eq!(size_text(1_300_000), "1.2 MB");
    let row = PickRow {
        id: cl_core::ids::TransactionId::parse("d0000000-0000-4000-8000-c00000000001").expect("id"),
        txn_type: cl_api::TxnType::Deposit,
        date: today(),
        number: Some("4417".into()),
        payee: Some("Ozark Feed & Seed".into()),
        amount: None,
    };
    assert_eq!(
        record_text(PickKind::Check, &row),
        "check 4417 (Ozark Feed & Seed, 11/04/2026)"
    );
    assert_eq!(row.target().attachable_type(), "deposit");
    assert_eq!(
        read_problem_key(FileError::OnlineOnly),
        "inbox.state.online_only"
    );
    assert_eq!(
        upload_problem_key(&ApiError::Local(LocalValidation::AttachmentType)),
        "inbox.read.wrong_type"
    );
}
