//! The inbox window over intake folders (A01 C2, C3, C5) in `--demo`: the
//! demo lays its folders out when the window opens (Bills keeps
//! originals, Scanner isn't set up here, P17's folder migrates to
//! Receipts), and the window shows every file with its folder and state.

use std::time::{Duration, Instant};

use cl_core::CalendarDate;
use cl_core::ids::TenantId;

use super::super::demo;
use super::super::intake::FormKind;
use super::*;
use crate::windows::Screen;

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn fast() -> cl_company::intake::WatchConfig {
    cl_company::intake::WatchConfig {
        poll: Duration::from_millis(100),
        settle_recheck: Duration::from_millis(50),
        stable_for: Duration::from_millis(150),
        deadline: Duration::from_secs(2),
        slow: Duration::from_secs(1),
        max_interval: Duration::from_millis(400),
        repin_every: Duration::from_secs(1),
        backoff: Duration::from_millis(100),
    }
}

fn setup() -> (
    tempfile::TempDir,
    Arc<CompanyLink>,
    egui::Context,
    super::super::ReceiptInboxScreen,
) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let (tenant, name): (TenantId, String) = {
        let d = link.demo_state().expect("demo");
        (d.companies[0].tenant_id, d.companies[0].name.clone())
    };
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    intake::handle(&ctx).lock().watch_config = fast();
    let s = super::super::ReceiptInboxScreen::new(Arc::clone(&link), Some((tenant, name)), today());
    (dir, link, ctx, s)
}

fn frame(
    ctx: &egui::Context,
    s: &mut super::super::ReceiptInboxScreen,
) -> Vec<(egui::accesskit::NodeId, egui::accesskit::Node)> {
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
    out.platform_output
        .accesskit_update
        .map(|u| u.nodes)
        .unwrap_or_default()
}

fn until(
    ctx: &egui::Context,
    s: &mut super::super::ReceiptInboxScreen,
    what: &str,
    done: impl Fn(&Value) -> bool,
) {
    let start = Instant::now();
    loop {
        frame(ctx, s);
        if done(&s.state()) {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "timed out waiting for {what}: {}",
            s.state()
        );
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn row(ctx: &egui::Context, name: &str) -> Row {
    intake::handle(ctx)
        .lock()
        .rows()
        .into_iter()
        .find(|r| r.file.name == name)
        .unwrap_or_else(|| panic!("{name} listed"))
}

#[test]
fn the_demo_window_lays_out_its_folders_and_copies_files_in() {
    let (_dir, link, ctx, mut s) = setup();
    until(&ctx, &mut s, "the demo files in", |st| {
        st["migration"] == "migrated"
            && st["arrived"] == 3
            && st["duplicate"] == 1
            && st["too_large"] == 1
            && st["not_supported"] == 1
    });
    let st = s.state();
    assert_eq!(st["mode"], "intake");
    assert_eq!(st["folder"], "ok");
    let health: Vec<&str> = st["folders"]
        .as_array()
        .expect("folders")
        .iter()
        .map(|f| f["health"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(health, ["ok", "not_confirmed", "ok"], "{st}");
    assert_eq!(st["folders"][0]["keep_originals"], true);
    assert_eq!(st["problems"], 1, "Scanner needs setting up");
    assert_eq!(super::super::intake_problems(&ctx), 1);
    assert_eq!(
        super::super::badge(&ctx),
        4,
        "three arrivals and a duplicate"
    );
    // No names or paths in the control channel.
    let text = st.to_string();
    for (_, name, _, _) in demo::INTAKE_SEED {
        assert!(!text.contains(name), "{text}");
    }
    for word in ["Bills", "Receipts", "Scanner", "Intake demo"] {
        assert!(!text.contains(word), "{text}");
    }
    // The late file (two halves 3 s apart) is the scenario's: these short
    // timings would take its first half in (WP-B's tests cover settling).
    let _ = link;
}

#[test]
fn new_check_opens_with_the_arrival_draft_and_tiff_waits_for_the_viewer() {
    let (_dir, link, ctx, mut s) = setup();
    until(&ctx, &mut s, "the demo files in", |st| st["arrived"] == 3);
    // TIFF: no preview yet, actions still there.
    let tif = row(&ctx, "Galena Gravel fax.tif");
    s.select_row(&ctx, &tif);
    until(&ctx, &mut s, "the TIFF read", |st| st["loaded"] == true);
    let st = s.state();
    assert_eq!(st["tiff_note"], true);
    assert_eq!(st["preview"]["open"], false, "{st}");
    // The PNG: preview, then New check.
    let png = row(&ctx, "Ozark Feed receipt.png");
    s.select_row(&ctx, &png);
    until(&ctx, &mut s, "the PNG read", |st| {
        st["loaded"] == true && st["selected_state"] == "arrived"
    });
    assert_eq!(s.state()["preview"]["open"], true);
    let f = Frame::read(&ctx);
    let png = row(&ctx, "Ozark Feed receipt.png");
    s.new_form(&ctx, &f, &png, FormKind::Check);
    let handed = link
        .drain()
        .into_iter()
        .find_map(|e| match e {
            CompanyEvent::OpenWith(kind, p) => match *p {
                OpenPayload::InboxDraft(d) => Some((kind, d)),
                _ => None,
            },
            _ => None,
        })
        .expect("Write Check opened");
    assert_eq!(handed.0.as_str(), WRITE_CHECK);
    let (kind, d) = handed;
    let _ = kind;
    let entry = png.entry.clone().expect("entry");
    let draft = {
        let h = intake::handle(&ctx);
        let s = h.lock();
        s.rows()
            .into_iter()
            .find(|r| r.entry.as_ref() == Some(&entry))
            .map(|r| r.state)
    };
    assert_eq!(draft, Some(RowState::OnForm));
    assert!(d.claim.is_some());
    assert!(!format!("{d:?}").contains("Ozark"), "redacted Debug");
    // Write Check takes the arrival draft; nothing else is uploaded.
    let mut check = crate::screens::write_check::WriteCheckScreen::new(
        Arc::clone(&link),
        s.open.clone(),
        today(),
    );
    assert!(check.open_with(OpenPayload::InboxDraft(d)));
    drop(check);
    // The form closed unsaved: the file is free again.
    until(&ctx, &mut s, "freed", |st| st["on_form"] == 0);
}

#[test]
fn every_intake_inbox_widget_has_an_accessible_name() {
    let (_dir, _link, ctx, mut s) = setup();
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    until(&ctx, &mut s, "the demo files in", |st| {
        st["arrived"] == 3 && st["duplicate"] == 1
    });
    let check = |s: &mut super::super::ReceiptInboxScreen, what: &str| {
        let mut nodes = Vec::new();
        for _ in 0..3 {
            nodes.extend(frame(&ctx, s));
        }
        let unnamed = crate::screens::unnamed_widgets(&nodes);
        assert!(unnamed.is_empty(), "{what}: unnamed widgets: {unnamed:?}");
    };
    check(&mut s, "list");
    let dup = row(&ctx, "Ozark Feed receipt (copy).png");
    s.select_row(&ctx, &dup);
    until(&ctx, &mut s, "the duplicate read", |st| {
        st["loaded"] == true
    });
    check(&mut s, "duplicate");
    s.arrivals.confirm_discard = true;
    check(&mut s, "discard confirm");
}

#[test]
fn discard_of_a_duplicate_moves_it_to_discarded() {
    let (dir, _link, ctx, mut s) = setup();
    until(&ctx, &mut s, "the duplicate", |st| st["duplicate"] == 1);
    let dup = row(&ctx, "Ozark Feed receipt (copy).png");
    let entry = dup.entry.clone().expect("entry");
    let path = dup.file.path.clone();
    let link = Arc::clone(&s.link);
    intake::handle(&ctx)
        .lock()
        .discard(&link, &entry)
        .expect("discard");
    until(&ctx, &mut s, "discarded", |st| st["discarded"] == 1);
    assert!(!path.exists());
    let discarded = path
        .parent()
        .expect("dir")
        .join("Discarded")
        .join("Ozark Feed receipt (copy).png");
    assert!(discarded.exists(), "never deleted");
    assert!(path.starts_with(dir.path()));
}
