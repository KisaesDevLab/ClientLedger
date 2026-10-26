//! Document intake panel (A01 WP-A): Admin gating, inline refusals, Test
//! and the confirmation, Save ordering, the 409, Remove, and no owner data
//! in the control channel.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cl_core::ids::TenantId;
use cl_settings::UserPrefs;
use eframe::egui;

use super::*;
use crate::company::CompanyLink;
use crate::screens::preferences::{Category, PreferencesScreen, SaveBlock};
use crate::widgets::file_picker::{Answer, PickerHook};
use crate::windows::Screen;

const OZARK: &str = "d0000000-0000-4000-8000-000000000001";
const OTHER: &str = "d0000000-0000-4000-8000-000000000002";

fn ozark() -> TenantId {
    TenantId::parse(OZARK).expect("id")
}

fn link(dir: &std::path::Path) -> Arc<CompanyLink> {
    let link = Arc::new(CompanyLink::demo(dir));
    link.store()
        .update(|l| {
            l.upsert(ozark(), "Ozark");
        })
        .expect("store");
    link
}

fn screen(link: &Arc<CompanyLink>) -> PreferencesScreen {
    PreferencesScreen::new(
        Arc::clone(link),
        Some((ozark(), "Ozark".to_owned())),
        UserPrefs::default(),
        Category::ReceiptInbox,
    )
}

/// Waits for every running Test.
fn settle(s: &mut PreferencesScreen) {
    let until = Instant::now() + cl_company::intake::PROBE_DEADLINE + Duration::from_secs(5);
    loop {
        s.poll();
        if !s.intake.busy() || Instant::now() > until {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!s.intake.busy(), "the test finished");
}

fn add(s: &mut PreferencesScreen, name: &str, path: &str) -> Result<IntakeFolderId, IntakeBlock> {
    s.intake.add = Some(AddRow {
        name: name.to_owned(),
        path: path.to_owned(),
        kind: Some(IntakeKind::Bills),
        problem: None,
    });
    let dirs = s.app_dirs();
    s.intake.add_folder(&mut s.edits.intake, &dirs)
}

fn unc_folder(id: IntakeFolderId, name: &str, path: &str) -> IntakeFolder {
    IntakeFolder {
        id,
        name: name.to_owned(),
        path: path.to_owned(),
        default_kind: IntakeKind::Bills,
        auto_read: false,
        keep_originals: false,
    }
}

fn texts(s: &mut PreferencesScreen) -> String {
    let ctx = egui::Context::default();
    crate::shell::Shell::install(&ctx);
    cl_grid::sequencer::install(&ctx);
    ctx.enable_accesskit();
    let mut out = Vec::new();
    for _ in 0..2 {
        let mut o = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 900.0),
                )),
                ..egui::RawInput::default()
            },
            |ui| s.ui(ui),
        );
        o.textures_delta.clear();
        if let Some(tree) = o.platform_output.accesskit_update {
            for (_, node) in tree.nodes {
                if let Some(v) = node.value() {
                    out.push(v.to_string());
                }
                if let Some(l) = node.label() {
                    out.push(l.to_string());
                }
            }
        }
    }
    out.join("\n")
}

fn stored(link: &CompanyLink) -> source::Stored {
    source::demo_get(link, ozark()).expect("demo").stored
}

fn here(link: &CompanyLink) -> BTreeMap<IntakeFolderId, HereNote> {
    link.store()
        .load()
        .expect("load")
        .get(ozark())
        .expect("rec")
        .prefs
        .intake_here
        .clone()
}

#[test]
fn an_admin_sees_the_empty_state_and_add_folder() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = link(dir.path());
    let mut s = screen(&link);
    assert_eq!(s.intake.load, LoadState::Ready, "the demo answers at once");
    assert!(s.intake.is_admin() && s.intake.editable());
    let all = texts(&mut s);
    assert!(all.contains("Document intake"), "{all}");
    assert!(
        all.contains("Add a folder where you save bills and receipts."),
        "{all}"
    );
    assert!(all.contains("Add folder"), "{all}");
    assert!(!all.contains("Ask an Admin"), "{all}");
    let state = s.state();
    assert_eq!(state["category"], "inbox");
    assert_eq!(state["intake"]["load"], "ready");
    assert_eq!(state["intake"]["admin"], true);
    assert_eq!(state["intake"]["folders"].as_array().map(Vec::len), Some(0));
}

#[test]
fn typed_paths_and_names_are_refused_inline_without_touching_disk() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = link(dir.path());
    let mut s = screen(&link);
    for (path, why) in [
        ("Scans", IntakeBlock::Path(PathProblem::NotAbsolute)),
        (r"\\?\C:\x", IntakeBlock::Path(PathProblem::Device)),
        ("https://x", IntakeBlock::Path(PathProblem::Url)),
        (r"\\server@SSL\x", IntakeBlock::Path(PathProblem::WebDav)),
        (
            r"C:\Scans\..\Windows",
            IntakeBlock::Path(PathProblem::DotDot),
        ),
        (r"C:\", IntakeBlock::Path(PathProblem::DriveRoot)),
    ] {
        assert_eq!(add(&mut s, "Bills", path), Err(why), "{path}");
        assert_eq!(
            s.intake.add.as_ref().and_then(|a| a.problem),
            Some(why),
            "shown on the row"
        );
        assert!(crate::i18n::has(why.message_key()));
    }
    // ClientLedger's own folders are refused.
    let backups = link.backup_dir.join("x").display().to_string();
    assert_eq!(
        add(&mut s, "Bills", &backups),
        Err(IntakeBlock::Path(PathProblem::AppFolder))
    );
    assert_eq!(add(&mut s, "  ", r"C:\Scans"), Err(IntakeBlock::NameEmpty));
    assert_eq!(
        add(&mut s, &"n".repeat(101), r"C:\Scans"),
        Err(IntakeBlock::NameTooLong)
    );
    // A UNC share passes the syntax rules. It is put in the list directly:
    // Add would test it, and no test resolves a UNC hostname.
    let dirs = s.app_dirs();
    assert!(
        check_new(
            &s.edits.intake,
            None,
            "Office scans",
            r"\\fileserver\Scans",
            &dirs
        )
        .is_ok()
    );
    let unc = new_folder_id();
    s.edits
        .intake
        .folders
        .push(unc_folder(unc, "Office scans", r"\\fileserver\Scans"));
    assert_eq!(
        add(&mut s, "office SCANS", r"D:\Other"),
        Err(IntakeBlock::NameTaken)
    );
    assert_eq!(
        add(&mut s, "Shoebox", "//FILESERVER/scans"),
        Err(IntakeBlock::PathTaken)
    );
    s.intake.remove(&mut s.edits.intake, unc);
    assert_eq!(s.edits.intake.folders, Vec::<IntakeFolder>::new());
    let all = texts(&mut s);
    assert!(!all.contains("fileserver"), "removed");
}

#[test]
fn add_tests_confirms_and_save_writes_the_engine_first_then_this_computer() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = link(dir.path());
    let bills = tempfile::tempdir().expect("bills");
    std::fs::write(bills.path().join("acme-bill.pdf"), b"%PDF-1.4 synthetic").expect("w");
    let path = bills.path().display().to_string();
    let mut s = screen(&link);
    let before = saves();
    let id = add(&mut s, "Smith Plumbing bills", &path).expect("added");
    assert!(s.dirty());
    settle(&mut s);
    // The passing test confirmed it for this computer, pending Save.
    let note = s
        .edits
        .intake
        .confirm
        .get(&id)
        .cloned()
        .expect("pending confirmation");
    assert_eq!(note.path_hash, cl_settings::path_hash(&path));
    assert!(note.since_ms.is_none());
    assert!(here(&link).is_empty(), "nothing written before Save");
    let state = s.state();
    let f = &state["intake"]["folders"][0];
    assert_eq!(f["pending"], true);
    assert_eq!(f["new"], true);
    assert_eq!(f["probe"], "ok");
    assert_eq!(f["files"], 1);
    assert_eq!(f["kind"], "bills");
    // No owner data in the control channel or Debug.
    let text = state.to_string();
    assert!(!text.contains("Smith") && !text.contains(&path), "{text}");
    assert!(!format!("{:?}", s.edits()).contains("Smith"));
    assert!(!format!("{:?}", s.intake).contains("Smith"));
    let all = texts(&mut s);
    assert!(all.contains("Works. 1 file would come in."), "{all}");
    assert!(
        all.contains("Ready on this computer: save to start"),
        "{all}"
    );

    assert!(Screen::save(&mut s), "{:?}", s.block);
    assert!(!s.dirty());
    let engine = stored(&link);
    assert_eq!(engine.folders.len(), 1);
    assert_eq!(engine.folders[0].path, path);
    assert_eq!(engine.folders[0].default_kind, IntakeKind::Bills);
    assert_eq!(s.intake.updated_at, engine.updated_at);
    let local = here(&link);
    assert_eq!(local.get(&id), Some(&note), "confirmed on this computer");
    assert!(saves() > before, "the inbox service is told");
    assert!(s.edits.intake.confirm.is_empty());
    assert_eq!(s.state()["intake"]["folders"][0]["here"], true);
    // The file is where it was.
    assert!(bills.path().join("acme-bill.pdf").exists());
}

#[test]
fn a_409_writes_nothing_and_reload_replaces_the_typed_changes() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = link(dir.path());
    let saved = link
        .demo_state()
        .expect("demo")
        .intake
        .put(
            ozark(),
            vec![IntakeFolder {
                id: new_folder_id(),
                name: "Bills".to_owned(),
                path: r"C:\Scans\Bills".to_owned(),
                default_kind: IntakeKind::Bills,
                auto_read: true,
                keep_originals: false,
            }],
            None,
        )
        .expect("seed");
    let mut s = screen(&link);
    assert_eq!(s.edits.intake.folders.len(), 1);
    let id = s.edits.intake.folders[0].id;
    s.intake
        .rename(&mut s.edits.intake, id, "Vendor bills")
        .expect("rename");
    // Someone else saves first.
    link.demo_state()
        .expect("demo")
        .intake
        .save_elsewhere(ozark(), saved.folders.clone());
    assert!(!Screen::save(&mut s));
    assert!(s.intake.stale);
    assert!(s.dirty(), "the typed change stays until Reload");
    assert_eq!(stored(&link).folders[0].name, "Bills", "nothing changed");
    assert!(here(&link).is_empty());
    let all = texts(&mut s);
    assert!(
        all.contains("Intake folders were changed elsewhere. Reload to see them."),
        "{all}"
    );
    assert!(all.contains("Reload"), "{all}");
    assert_eq!(s.state()["intake"]["stale"], true);
    // Reload: the engine's list replaces the typed change.
    s.reload_intake();
    assert!(!s.intake.stale);
    assert_eq!(s.edits.intake.folders[0].name, "Bills");
    assert!(!s.dirty());
    // Now a save goes through; auto-read round-trips unchanged (Q-A01j).
    s.intake
        .rename(&mut s.edits.intake, id, "Vendor bills")
        .expect("rename");
    assert!(Screen::save(&mut s), "{:?}", s.block);
    let engine = stored(&link);
    assert_eq!(engine.folders[0].name, "Vendor bills");
    assert!(engine.folders[0].auto_read);
    // The demo's own switch (the scenario's 409 path).
    s.intake
        .rename(&mut s.edits.intake, id, "Bills again")
        .expect("rename");
    link.demo_state().expect("demo").intake.stale_next = true;
    assert!(!Screen::save(&mut s));
    assert!(s.intake.stale);
}

#[test]
fn remove_drops_the_folder_and_its_confirmation_and_leaves_the_files() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = link(dir.path());
    let receipts = tempfile::tempdir().expect("r");
    std::fs::write(receipts.path().join("r1.jpg"), b"\xff\xd8\xff synthetic").expect("w");
    let mut s = screen(&link);
    let id = add(&mut s, "Receipts", &receipts.path().display().to_string()).expect("add");
    settle(&mut s);
    assert!(Screen::save(&mut s));
    assert!(here(&link).contains_key(&id));
    s.intake.removing = Some(id);
    let all = texts(&mut s);
    assert!(
        all.contains(
            "Remove this folder from ClientLedger? Files in the folder stay where they are."
        ),
        "{all}"
    );
    s.intake.remove(&mut s.edits.intake, id);
    assert!(Screen::save(&mut s), "{:?}", s.block);
    assert_eq!(stored(&link).folders, Vec::<IntakeFolder>::new());
    assert!(here(&link).is_empty(), "its confirmation is dropped");
    assert!(
        receipts.path().join("r1.jpg").exists(),
        "files stay where they are"
    );
}

#[test]
fn other_roles_see_a_read_only_list() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = link(dir.path());
    {
        let mut demo = link.demo_state().expect("demo");
        demo.intake.role = Some("bookkeeper".to_owned());
    }
    let mut s = screen(&link);
    assert!(!s.intake.is_admin());
    assert!(!s.intake.editable());
    let all = texts(&mut s);
    assert!(
        all.contains("Only an Admin can change intake folders. Ask an Admin."),
        "{all}"
    );
    assert!(all.contains("Ask an Admin to add one."), "{all}");
    assert!(!all.contains("Add folder"), "{all}");
    // A change can't be saved (the engine would also refuse on P29).
    s.edits.intake.folders.push(IntakeFolder {
        id: new_folder_id(),
        name: "Bills".to_owned(),
        path: r"C:\Bills".to_owned(),
        default_kind: IntakeKind::Bills,
        auto_read: false,
        keep_originals: false,
    });
    assert!(!Screen::save(&mut s));
    assert_eq!(s.block, Some(SaveBlock::Intake(IntakeBlock::ReadOnly)));
    assert_eq!(stored(&link).folders, Vec::<IntakeFolder>::new());
    assert_eq!(s.state()["intake"]["admin"], false);
    // Test and Add refuse too, whatever calls them (security review L5).
    let id = s.edits.intake.folders[0].id;
    let dirs = s.app_dirs();
    s.intake.test(&s.edits.intake, id, &dirs);
    assert!(!s.intake.busy(), "no test started");
    s.intake.add = Some(AddRow {
        name: "Receipts".to_owned(),
        path: r"C:\Receipts".to_owned(),
        kind: None,
        problem: None,
    });
    assert_eq!(
        s.intake.add_folder(&mut s.edits.intake, &dirs),
        Err(IntakeBlock::ReadOnly)
    );
    assert_eq!(s.edits.intake.folders.len(), 1);
    // Nor does Browse open a window (WP-D).
    let ctx = egui::Context::default();
    let hook = PickerHook::install(&ctx);
    s.intake.browse(&ctx, &dirs);
    assert!(!s.intake.browsing());
    assert_eq!(hook.asked(), 0);
}

/// Browse (WP-D, Q-A01m): the chosen folder fills the Add row's Folder
/// field and is then checked and tested like a typed one; a cancel keeps
/// what was typed; no path reaches the control channel.
#[test]
fn browse_fills_the_folder_field_and_a_cancel_keeps_the_typed_path() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = link(dir.path());
    let bills = tempfile::tempdir().expect("bills");
    let mut s = screen(&link);
    let ctx = egui::Context::default();
    let hook = PickerHook::install(&ctx);
    let dirs = s.app_dirs();
    // Without the Add row open there is nothing to fill.
    s.intake.browse(&ctx, &dirs);
    assert_eq!(hook.asked(), 0);

    s.intake.add = Some(AddRow {
        name: "Bills".to_owned(),
        ..AddRow::default()
    });
    hook.answer(Answer::Paths(vec![bills.path().to_path_buf()]));
    s.intake.browse(&ctx, &dirs);
    assert_eq!(hook.asked(), 1);
    assert_eq!(s.state()["intake"]["browsing"], true);
    s.poll();
    assert!(!s.intake.browsing());
    let shown = bills.path().display().to_string();
    assert_eq!(
        s.intake.add.as_ref().map(|a| a.path.as_str()),
        Some(shown.as_str())
    );
    let state = s.state().to_string();
    assert!(!state.contains(&shown), "no path in the control channel");
    // Add: tested at once, as for a typed path.
    let id = s
        .intake
        .add_folder(&mut s.edits.intake, &dirs)
        .expect("added");
    settle(&mut s);
    let f = s.edits.intake.folders.iter().find(|f| f.id == id).cloned();
    assert_eq!(f.map(|f| f.path), Some(shown.clone()));
    assert_eq!(s.state()["intake"]["folders"][0]["probe"], "ok");

    // A cancel leaves the typed path alone; a refused pick is shown inline.
    s.intake.add = Some(AddRow {
        name: "Receipts".to_owned(),
        path: r"C:\Typed\Receipts".to_owned(),
        ..AddRow::default()
    });
    hook.answer(Answer::Cancel);
    s.intake.browse(&ctx, &dirs);
    s.poll();
    assert_eq!(
        s.intake.add.as_ref().map(|a| a.path.as_str()),
        Some(r"C:\Typed\Receipts")
    );
    // The same folder again is refused on Add like a typed duplicate.
    hook.answer(Answer::Paths(vec![bills.path().to_path_buf()]));
    s.intake.browse(&ctx, &dirs);
    s.poll();
    assert_eq!(
        s.intake.add_folder(&mut s.edits.intake, &dirs),
        Err(IntakeBlock::PathTaken)
    );
    // The Browse button names the Add row.
    let all = texts(&mut s);
    assert!(all.contains("Browse for the new folder"), "{all}");
}

/// A folder another computer (or user) set up on a network computer names
/// that computer plainly before anyone here tests it (security review L4).
#[test]
fn an_unconfirmed_network_folder_names_its_computer() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = link(dir.path());
    link.demo_state().expect("demo").intake.save_elsewhere(
        ozark(),
        vec![unc_folder(
            new_folder_id(),
            "Office scans",
            r"\\nas-07\Scans",
        )],
    );
    let mut s = screen(&link);
    let all = texts(&mut s);
    assert!(
        all.contains(
            "On the network computer \"nas-07\": set on another computer or by another user."
        ),
        "{all}"
    );
    assert_eq!(unc_host(r"\\nas-07\Scans"), Some("nas-07"));
    assert_eq!(unc_host("//nas-07/Scans"), Some("nas-07"));
    assert_eq!(unc_host(r"C:\Scans"), None);
    assert_eq!(unc_host(r"\\?\C:\Scans"), None);
}

/// A Test that stopped answering still runs: Test stays off until its
/// thread is done, so a dead share never piles up threads (L4).
#[test]
fn test_stays_off_until_a_stuck_probe_is_done() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = link(dir.path());
    let folder = tempfile::tempdir().expect("f");
    let mut s = screen(&link);
    let id = new_folder_id();
    s.edits.intake.folders.push(unc_folder(
        id,
        "Bills",
        &folder.path().display().to_string(),
    ));
    let (tx, rx) = std::sync::mpsc::channel();
    s.intake.probes.insert(
        id,
        Probe {
            rx: Some(rx),
            report: None,
            path: s.edits.intake.folders[0].path.clone(),
            still_trying: false,
        },
    );
    tx.send(ProbeReport::not_responding()).expect("send");
    s.poll();
    assert!(s.intake.held(id) && !s.intake.testing(id));
    let dirs = s.app_dirs();
    s.intake.test(&s.edits.intake, id, &dirs);
    assert!(
        s.intake.probes[&id].still_trying,
        "the stuck probe is kept, no new one"
    );
    drop(tx);
    s.poll();
    assert!(!s.intake.held(id), "done: Test is offered again");
}

/// Every folder's controls name their folder for a screen reader (UI
/// check S4); Esc closes the Add row before the window (S5).
#[test]
fn controls_name_their_folder_and_esc_closes_the_add_row_first() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = link(dir.path());
    let a = tempfile::tempdir().expect("a");
    let mut s = screen(&link);
    add(&mut s, "Bills", &a.path().display().to_string()).expect("add");
    settle(&mut s);
    let all = texts(&mut s);
    for label in [
        "Kind for Bills",
        "Leave originals where they are (Bills)",
        "Read new documents automatically (Bills)",
        "Rename Bills",
        "Remove Bills",
        "Test Bills",
    ] {
        assert!(all.contains(label), "{label}: {all}");
    }
    assert!(Screen::save(&mut s));
    s.intake.add = Some(AddRow {
        name: "Half typed".to_owned(),
        ..AddRow::default()
    });
    assert!(!s.dirty(), "the Add row isn't an edit");
    assert!(
        s.command(cl_model::CommandId::new("form.revert")),
        "Esc closed the row"
    );
    assert!(s.intake.add.is_none());
    assert!(
        !s.command(cl_model::CommandId::new("form.revert")),
        "then Esc closes the window"
    );
}

/// "Set up on this computer" survives closing and reopening Preferences,
/// also for a company with no record on this computer yet (UI check M2).
#[test]
fn a_confirmation_survives_a_reopen_without_a_company_record() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let bills = tempfile::tempdir().expect("bills");
    let mut s = screen(&link);
    let id = add(&mut s, "Bills", &bills.path().display().to_string()).expect("add");
    settle(&mut s);
    assert!(Screen::save(&mut s), "{:?}", s.block);
    assert_eq!(s.state()["intake"]["folders"][0]["here"], true);
    drop(s);
    let s = screen(&link);
    assert!(s.committed.company.intake_here.contains_key(&id));
    assert_eq!(s.state()["intake"]["folders"][0]["here"], true);
}

#[test]
fn a_folder_another_company_uses_on_this_computer_is_refused() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = link(dir.path());
    let shared = tempfile::tempdir().expect("shared");
    let key = cl_company::intake::canonical_key(shared.path()).expect("key");
    link.store()
        .update(|l| {
            let other = l.upsert(TenantId::parse(OTHER).expect("id"), "Other");
            other.prefs.intake_here.insert(
                new_folder_id(),
                HereNote {
                    path_hash: "h".repeat(64),
                    canonical_key: key.clone(),
                    since_ms: None,
                    confirmed_at: None,
                },
            );
        })
        .expect("store");
    let mut s = screen(&link);
    add(&mut s, "Shared", &shared.path().display().to_string()).expect("add");
    settle(&mut s);
    assert!(!Screen::save(&mut s));
    assert_eq!(
        s.block,
        Some(SaveBlock::Intake(IntakeBlock::MappedByAnother))
    );
    assert_eq!(stored(&link).folders, Vec::<IntakeFolder>::new());
}

#[test]
fn a_full_folder_asks_before_bringing_everything_in() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = link(dir.path());
    let full = tempfile::tempdir().expect("full");
    for n in 0..=BRING_IN_ASK_ABOVE {
        std::fs::write(full.path().join(format!("scan-{n:03}.pdf")), b"%PDF-1.4").expect("w");
    }
    let mut s = screen(&link);
    let id = add(&mut s, "Old scans", &full.path().display().to_string()).expect("add");
    settle(&mut s);
    assert!(s.edits.intake.confirm.is_empty(), "asks first");
    assert_eq!(s.state()["intake"]["asking_bring_in"], true);
    let all = texts(&mut s);
    assert!(all.contains("This folder already holds 51 files."), "{all}");
    assert!(all.contains("Bring in 51 files now"), "{all}");
    assert!(all.contains("Only files added from now on"), "{all}");
    s.intake.confirm(&mut s.edits.intake, id, true);
    let note = s.edits.intake.confirm.get(&id).expect("confirmed");
    assert!(note.since_ms.is_some(), "only new files");
    assert_eq!(s.state()["intake"]["asking_bring_in"], false);
}

#[test]
fn test_of_a_missing_folder_reports_it_and_offers_no_confirmation() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = link(dir.path());
    let gone = dir.path().join("unplugged-drive");
    let mut s = screen(&link);
    let id = add(&mut s, "Scanner", &gone.display().to_string()).expect("add");
    settle(&mut s);
    assert!(s.edits.intake.confirm.is_empty());
    assert_eq!(s.state()["intake"]["folders"][0]["probe"], "missing");
    let all = texts(&mut s);
    assert!(
        all.contains("Can't find the folder (drive unplugged, renamed, or sync paused)"),
        "{all}"
    );
    assert!(!all.contains("Use this folder on this computer"), "{all}");
    // Saved anyway, it waits unconfirmed: the status says so.
    assert!(Screen::save(&mut s));
    assert!(here(&link).is_empty());
    s.intake.probes.remove(&id);
    let all = texts(&mut s);
    assert!(
        all.contains("Set this company's folder for this computer"),
        "{all}"
    );
}

#[test]
fn the_switches_round_trip_and_the_auto_read_hint_shows() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = link(dir.path());
    let mut s = screen(&link);
    // Put in directly: no test resolves a UNC hostname. The switches are
    // just stored.
    let id = new_folder_id();
    s.edits
        .intake
        .folders
        .push(unc_folder(id, "Receipts", r"\\fileserver\Receipts"));
    let f = s
        .edits
        .intake
        .folders
        .iter_mut()
        .find(|f| f.id == id)
        .expect("f");
    f.keep_originals = true;
    f.auto_read = true;
    f.default_kind = IntakeKind::Receipts;
    let all = texts(&mut s);
    assert!(all.contains("Leave originals where they are"), "{all}");
    assert!(all.contains("Read new documents automatically"), "{all}");
    assert!(
        all.contains(
            "Reading arrives in a coming update; until then new documents wait in the inbox."
        ),
        "{all}"
    );
    assert!(Screen::save(&mut s), "{:?}", s.block);
    let f = &stored(&link).folders[0];
    assert!(f.keep_originals && f.auto_read);
    assert_eq!(f.default_kind, IntakeKind::Receipts);
}

fn legacy_dir(link: &CompanyLink) -> Option<std::path::PathBuf> {
    link.store()
        .load()
        .expect("load")
        .get(ozark())
        .expect("rec")
        .prefs
        .receipt_inbox_dir
        .clone()
}

/// Q-A01q regression: on an engine without `intake-settings-v1` the P17
/// single folder is still chosen here, checked as P17 checked it, and
/// saved on the company record.
#[test]
fn an_engine_without_intake_settings_keeps_the_p17_folder_chooser() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = link(dir.path());
    link.demo_state().expect("demo").intake.needs_engine = true;
    let mut s = screen(&link);
    assert_eq!(s.intake.load, LoadState::NeedsEngine);
    assert_eq!(s.state()["intake"]["load"], "needs_engine");
    let all = texts(&mut s);
    assert!(
        all.contains("Document intake needs the latest engine."),
        "{all}"
    );
    assert!(all.contains("Watch a folder for receipts"), "{all}");
    assert!(!all.contains("Add folder"), "{all}");
    // An untouched panel never blocks a save, and writes no folder.
    s.edits.user.theme = cl_settings::ThemeChoice::Dark;
    assert!(Screen::save(&mut s), "{:?}", s.block);
    assert_eq!(legacy_dir(&link), None);
    // A network folder is refused as in P17 …
    s.edits.company.receipt_inbox_dir = Some(std::path::PathBuf::from(r"\\server\share\Receipts"));
    assert!(!Screen::save(&mut s));
    assert!(matches!(
        s.block,
        Some(SaveBlock::Folder(cl_settings::FolderError::Network))
    ));
    // … a folder on this computer is saved on the company record.
    let inbox = dir.path().join("Receipts");
    std::fs::create_dir(&inbox).expect("dir");
    s.edits.company.receipt_inbox_dir = Some(inbox.clone());
    assert!(Screen::save(&mut s), "{:?}", s.block);
    assert_eq!(legacy_dir(&link), Some(inbox.clone()));
    let all = texts(&mut s);
    assert!(all.contains(&*inbox.to_string_lossy()), "{all}");
    // Turned off again.
    s.edits.company.receipt_inbox_dir = None;
    assert!(Screen::save(&mut s), "{:?}", s.block);
    assert_eq!(legacy_dir(&link), None);
}

/// With intake folders in the engine the P17 field belongs to the
/// migration: a Preferences save never writes it back.
#[test]
fn with_intake_settings_a_save_leaves_the_p17_folder_alone() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = link(dir.path());
    let mut s = screen(&link);
    assert_eq!(s.intake.load, LoadState::Ready);
    // The migration cleared the field while the window was open.
    s.edits.company.receipt_inbox_dir = Some(std::path::PathBuf::from(r"C:\Receipts"));
    s.edits.user.theme = cl_settings::ThemeChoice::Dark;
    assert!(Screen::save(&mut s), "{:?}", s.block);
    assert_eq!(legacy_dir(&link), None);
}

#[test]
fn block_and_health_texts_exist() {
    for b in [
        IntakeBlock::NameEmpty,
        IntakeBlock::NameTooLong,
        IntakeBlock::NameTaken,
        IntakeBlock::TooMany,
        IntakeBlock::PathTaken,
        IntakeBlock::MappedByAnother,
        IntakeBlock::ReadOnly,
        IntakeBlock::NotLoaded,
        IntakeBlock::Refused,
    ] {
        assert!(crate::i18n::has(b.message_key()), "{b:?}");
    }
    for p in [
        PathProblem::Empty,
        PathProblem::TooLong,
        PathProblem::ControlCharacter,
        PathProblem::NotAbsolute,
        PathProblem::Device,
        PathProblem::Url,
        PathProblem::WebDav,
        PathProblem::DotDot,
        PathProblem::AppFolder,
        PathProblem::DriveRoot,
    ] {
        assert!(crate::i18n::has(path_problem_key(p)), "{p:?}");
    }
    for h in [
        Health::Ok,
        Health::NotConfirmed,
        Health::Missing,
        Health::Unreachable,
        Health::NotResponding,
        Health::PermissionDenied,
        Health::NotAFolder,
        Health::CantMove,
        Health::Moved,
        Health::MappedByAnother,
        Health::Slow,
        Health::TooMany,
    ] {
        assert!(crate::i18n::has(health_key(h).0), "{h:?}");
    }
    for k in [IntakeKind::Bills, IntakeKind::Receipts, IntakeKind::Auto] {
        assert!(crate::i18n::has(kind_key(k)));
    }
}

#[test]
fn the_engine_limits_and_the_stale_answer_match_the_client() {
    assert_eq!(MAX_FOLDERS, cl_api::INTAKE_FOLDERS_MAX);
    assert_eq!(
        crate::company::demo::intake::MAX_FOLDERS,
        cl_api::INTAKE_FOLDERS_MAX
    );
    assert_eq!(source::CAPABILITY, "intake-settings-v1");
    let stale = CompanyError::Api(cl_api::ApiError::Conflict {
        reason: cl_api::ConflictReason::Stale,
        current_version: None,
        code: None,
        message: cl_api::UserText::default(),
    });
    assert!(source::is_stale(&stale));
    let unsupported = CompanyError::Api(cl_api::ApiError::Local(
        cl_api::LocalValidation::Unsupported,
    ));
    assert!(!source::is_stale(&unsupported));
}
