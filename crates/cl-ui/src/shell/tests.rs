//! Headless frame tests: run the real shell through egui without a window.

use cl_model::{COMMANDS, CommandId, Menu, commands};
use eframe::egui::{self, Event, Key, Modifiers, RawInput, Rect, vec2};

use super::{HostProbe, HostView, Shell, ShellOptions};
use crate::i18n::t;

fn demo() -> Shell {
    Shell::new(ShellOptions {
        demo: true,
        ..ShellOptions::default()
    })
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    Shell::install(&ctx);
    cl_grid::sequencer::install(&ctx);
    ctx
}

fn input(size: egui::Vec2, events: Vec<Event>) -> RawInput {
    RawInput {
        screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
        events,
        ..RawInput::default()
    }
}

fn key(key: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// A key press followed by its release (egui treats a press without a
/// release as a key repeat).
fn tap(k: Key, modifiers: Modifiers) -> Vec<Event> {
    vec![
        key(k, modifiers),
        Event::Key {
            key: k,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers,
        },
    ]
}

const CTRL: Modifiers = Modifiers {
    alt: false,
    ctrl: true,
    shift: false,
    mac_cmd: false,
    command: true,
};

/// Run one frame; returns the output.
fn frame(
    ctx: &egui::Context,
    shell: &mut Shell,
    size: egui::Vec2,
    events: Vec<Event>,
) -> egui::FullOutput {
    let mut out = ctx.run_ui(input(size, events), |ui| shell.ui(ui));
    // Nothing paints here; egui insists the texture updates are consumed.
    out.textures_delta.clear();
    out
}

fn settle(ctx: &egui::Context, shell: &mut Shell, size: egui::Vec2) {
    for _ in 0..3 {
        frame(ctx, shell, size, vec![]);
    }
}

const WIDE: egui::Vec2 = vec2(1440.0, 940.0);

#[test]
fn demo_shell_renders_wide_and_narrow() {
    for size in [WIDE, vec2(1024.0, 700.0), vec2(480.0, 320.0)] {
        let ctx = context();
        let mut shell = demo();
        settle(&ctx, &mut shell, size);
        let used = ctx.globally_used_rect();
        assert!(
            used.max.x <= size.x + 0.5 && used.max.y <= size.y + 0.5,
            "{size:?}: used {used:?}"
        );
    }
}

#[test]
fn shell_survives_200_percent_zoom_at_the_minimum_window() {
    let ctx = context();
    let mut shell = demo();
    shell.prefs.zoom_percent = 200;
    // 960 x 640 points at 200% is 480 x 320 egui points.
    let size = vec2(super::MIN_WINDOW[0] / 2.0, super::MIN_WINDOW[1] / 2.0);
    settle(&ctx, &mut shell, size);
    assert!((ctx.zoom_factor() - 2.0).abs() < f32::EPSILON);
    let used = ctx.globally_used_rect();
    assert!(
        used.max.x <= size.x + 0.5 && used.max.y <= size.y + 0.5,
        "{used:?}"
    );
    // The menu bar collapsed to one button and the rail is hidden until asked for.
    assert!(size.x < super::NARROW_WIDTH);
    assert!(!shell.rail_open_when_narrow());
    shell.toggle_rail();
    settle(&ctx, &mut shell, size);
    assert!(shell.rail_open_when_narrow());
}

#[test]
fn every_screen_command_opens_a_window_and_focuses_it() {
    let mut shell = Shell::new(ShellOptions::default());
    for c in COMMANDS.iter().filter(|c| c.opens) {
        shell.dispatch_now(c.id);
        assert_eq!(
            shell.windows.active().map(|w| w.kind),
            Some(c.id),
            "{} did not open",
            c.id
        );
        assert_eq!(
            shell
                .windows
                .all()
                .iter()
                .filter(|w| w.kind == c.id)
                .count(),
            1
        );
    }
}

#[test]
fn every_menu_and_rail_entry_runs_a_registered_command() {
    for m in Menu::ALL {
        assert!(commands::in_menu(*m).count() > 0, "{m:?}");
        assert!(crate::i18n::has(&super::menu::title_key(*m)), "{m:?}");
        for c in commands::in_menu(*m) {
            assert!(crate::i18n::has(&format!("cmd.{}", c.id)));
        }
    }
    for id in commands::ALL
        .rail_workspace
        .iter()
        .chain(commands::ALL.rail_tools)
    {
        let c = commands::find(id).expect("rail entry has a command");
        assert!(c.opens, "{id} should open a window");
        assert!(c.palette);
    }
}

#[test]
fn keyboard_runs_commands_through_the_keymap() {
    let ctx = context();
    let mut shell = demo();
    settle(&ctx, &mut shell, WIDE);

    // Ctrl+K opens the palette.
    frame(&ctx, &mut shell, WIDE, tap(Key::K, CTRL));
    assert!(shell.palette.open);
    frame(&ctx, &mut shell, WIDE, tap(Key::Escape, Modifiers::NONE));
    assert!(!shell.palette.open);

    // Ctrl+W opens Write Check (already open in the demo: it is focused).
    frame(&ctx, &mut shell, WIDE, tap(Key::W, CTRL));
    assert_eq!(
        shell.windows.active().map(|w| w.kind.as_str()),
        Some("vendors.write_check")
    );

    // Ctrl+Tab cycles forward, Ctrl+Shift+Tab back.
    let before = shell.windows.active_id();
    frame(&ctx, &mut shell, WIDE, tap(Key::Tab, CTRL));
    assert_ne!(shell.windows.active_id(), before);
    frame(
        &ctx,
        &mut shell,
        WIDE,
        tap(
            Key::Tab,
            Modifiers {
                shift: true,
                ..CTRL
            },
        ),
    );
    assert_eq!(shell.windows.active_id(), before);

    // Alt+H goes Home.
    frame(
        &ctx,
        &mut shell,
        WIDE,
        tap(
            Key::H,
            Modifiers {
                alt: true,
                ..Modifiers::NONE
            },
        ),
    );
    assert_eq!(
        shell.windows.active().map(|w| w.kind.as_str()),
        Some("nav.home")
    );

    // Ctrl+/ shows the shortcut overlay.
    frame(&ctx, &mut shell, WIDE, tap(Key::Slash, CTRL));
    assert!(shell.shortcuts_open());
}

#[test]
fn zoom_keys_stay_within_80_to_200_and_apply() {
    let ctx = context();
    let mut shell = Shell::new(ShellOptions::default());
    for _ in 0..12 {
        frame(&ctx, &mut shell, WIDE, tap(Key::Equals, CTRL));
    }
    assert_eq!(shell.prefs.zoom_percent, 200);
    assert!((ctx.zoom_factor() - 2.0).abs() < f32::EPSILON);
    for _ in 0..12 {
        frame(&ctx, &mut shell, WIDE, tap(Key::Minus, CTRL));
    }
    assert_eq!(shell.prefs.zoom_percent, 80);
    frame(&ctx, &mut shell, WIDE, tap(Key::Num0, CTRL));
    assert_eq!(shell.prefs.zoom_percent, 100);
}

#[test]
fn theme_command_cycles_and_both_themes_render() {
    let ctx = context();
    let mut shell = demo();
    shell.dispatch_now(CommandId::new("window.theme"));
    assert_eq!(shell.prefs.theme, crate::theme::ThemeChoice::Light);
    settle(&ctx, &mut shell, WIDE);
    assert_eq!(ctx.theme(), egui::Theme::Light);
    shell.dispatch_now(CommandId::new("window.theme"));
    settle(&ctx, &mut shell, WIDE);
    assert_eq!(shell.prefs.theme, crate::theme::ThemeChoice::Dark);
    assert_eq!(ctx.theme(), egui::Theme::Dark);
}

#[test]
fn quitting_with_unsaved_changes_prompts_and_keep_cancels() {
    let ctx = context();
    let mut shell = demo(); // Write Check #4418 has unsaved changes.
    shell.dispatch_now(CommandId::new("file.quit"));
    assert!(shell.windows.prompt().is_some());
    assert!(!shell.quit_approved());
    settle(&ctx, &mut shell, WIDE);
    shell
        .windows
        .answer_prompt(crate::windows::PromptAnswer::Keep);
    assert!(!shell.quit_approved());

    shell.dispatch_now(CommandId::new("file.quit"));
    let progress = shell
        .windows
        .answer_prompt(crate::windows::PromptAnswer::Discard);
    assert_eq!(progress, Some(crate::windows::QuitProgress::Ready));
}

#[test]
fn closing_the_os_window_with_unsaved_changes_does_not_exit() {
    let ctx = context();
    let mut shell = demo();
    settle(&ctx, &mut shell, WIDE);
    let mut raw = input(WIDE, vec![]);
    raw.viewports
        .entry(raw.viewport_id)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    ctx.run_ui(raw, |ui| shell.ui(ui)).textures_delta.clear();
    assert!(shell.windows.prompt().is_some());
    assert!(!shell.quit_approved());
}

#[test]
fn clean_shell_quits_at_once() {
    let mut shell = Shell::new(ShellOptions::default());
    shell.dispatch_now(CommandId::new("file.quit"));
    assert!(shell.quit_approved());
}

#[test]
fn unhandled_commands_say_so() {
    let ctx = context();
    let mut shell = Shell::new(ShellOptions::default());
    settle(&ctx, &mut shell, WIDE);
    shell.queue(CommandId::new("file.print"));
    frame(&ctx, &mut shell, WIDE, vec![]);
    assert_eq!(shell.toast_text(), Some(t("toast.not_here").as_str()));
}

#[test]
fn title_bar_text_follows_the_company() {
    let shell = demo();
    assert_eq!(
        super::title_bar::right_text(&shell, false),
        "Wed 11/04/2026   Books closed through 09/30/2026"
    );
    assert_eq!(
        super::title_bar::right_text(&shell, true),
        "Closed through 09/30/2026"
    );
    let bare = Shell::new(ShellOptions::default());
    assert!(bare.company.is_none());
    assert!(!super::title_bar::right_text(&bare, false).contains("closed"));
}

struct Probe(HostView);
impl HostProbe for Probe {
    fn view(&self) -> HostView {
        self.0.clone()
    }
}

#[test]
fn status_bar_hints_follow_the_window_in_front() {
    use super::status_bar::hints_for;
    let chip = |id: &str| super::chip_text(commands::find(id).unwrap().shortcut.unwrap());
    let general = hints_for(None);
    assert_eq!(general.len(), 4);
    assert_eq!(general[0].label, t("status.hint.go_to"));
    assert_eq!(general[0].chips, vec![chip("help.palette")]);
    assert_eq!(
        hints_for(Some(CommandId::new("customers.invoice"))),
        general
    );

    // Mockup Register.html: Enter record · + − T dates · Ctrl D delete ·
    // Ctrl E open form.
    let reg = hints_for(Some(CommandId::new("banking.register")));
    let labels: Vec<&str> = reg.iter().map(|h| h.label.as_str()).collect();
    assert_eq!(labels, ["record", "dates", "delete", "open form"]);
    assert_eq!(reg[0].chips, ["Enter"]);
    assert_eq!(reg[1].chips, ["+", "\u{2212}", "T"]);
    assert_eq!(reg[2].chips, vec![chip("form.delete_item")]);
    assert_eq!(reg[3].chips, vec![chip("edit.edit_transaction")]);

    // Mockup WriteCheck.html: Tab next field · Ctrl Ins insert line ·
    // Ctrl L list · Esc revert.
    let check = hints_for(Some(CommandId::new("vendors.write_check")));
    let labels: Vec<&str> = check.iter().map(|h| h.label.as_str()).collect();
    assert_eq!(labels, ["next field", "insert line", "list", "revert"]);
    assert_eq!(check[0].chips, ["Tab"]);
    assert_eq!(check[1].chips, vec![chip("form.insert_line")]);
    assert_eq!(check[2].chips, vec![chip("edit.open_list")]);
    assert_eq!(check[3].chips, vec![chip("form.revert")]);
}

#[test]
fn status_bar_reports_the_engine() {
    use super::status_bar::engine_text;
    assert_eq!(engine_text(&HostView::Unavailable), None);
    assert!(engine_text(&HostView::Running).unwrap().contains("running"));
    assert!(
        engine_text(&HostView::Failed("database".into()))
            .unwrap()
            .contains("database")
    );
    let shell = Shell::new(ShellOptions {
        host: Some(std::sync::Arc::new(Probe(HostView::Restarting))),
        ..ShellOptions::default()
    });
    assert_eq!(shell.host_view(), HostView::Restarting);
}

#[test]
fn popped_out_windows_get_their_own_viewport_and_dock_back() {
    let ctx = context();
    let mut shell = demo();
    settle(&ctx, &mut shell, WIDE);
    let id = shell.windows.all()[2].id;
    shell.windows.pop_out(id);
    settle(&ctx, &mut shell, WIDE);
    assert!(shell.windows.get(id).unwrap().popped_out);
    shell.windows.dock(id);
    settle(&ctx, &mut shell, WIDE);
    assert!(!shell.windows.get(id).unwrap().popped_out);
}

#[test]
fn column_widths_are_written_a_second_after_a_change_and_never_when_controlled() {
    for controlled in [false, true] {
        let dir = std::env::temp_dir().join(format!(
            "cl-ui-shell-widths-{controlled}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = crate::prefs::UiStore::new(&dir);
        let mut shell = Shell::new(ShellOptions {
            store: Some(store.clone()),
            ..ShellOptions::default()
        });
        shell.set_run_only(controlled);
        let ctx = context();
        let at = |ctx: &egui::Context, shell: &mut Shell, time: f64| {
            let mut raw = input(WIDE, vec![]);
            raw.time = Some(time);
            let mut out = ctx.run_ui(raw, |ui| shell.ui(ui));
            out.textures_delta.clear();
        };
        at(&ctx, &mut shell, 0.0);
        crate::grid_env::store_widths(&ctx, "a/lines", &[(cl_grid::ColumnId(1), 150)]);
        at(&ctx, &mut shell, 0.5);
        let path = dir.join(crate::prefs::GridWidths::FILE);
        assert!(!path.exists(), "debounced");
        at(&ctx, &mut shell, 1.6);
        assert_eq!(path.exists(), !controlled, "controlled = {controlled}");
        if !controlled {
            assert_eq!(
                store.load_grid_widths().get("a/lines"),
                Some(vec![(cl_grid::ColumnId(1), 150)])
            );
            let again = Shell::new(ShellOptions {
                store: Some(store),
                ..ShellOptions::default()
            });
            let ctx2 = context();
            let mut again = again;
            settle(&ctx2, &mut again, WIDE);
            assert_eq!(
                crate::grid_env::load_widths(&ctx2, "a/lines"),
                Some(vec![(cl_grid::ColumnId(1), 150)]),
                "loaded at start"
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn layout_is_remembered_per_company() {
    let dir = std::env::temp_dir().join(format!("cl-ui-shell-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = crate::prefs::UiStore::new(&dir);
    {
        let mut shell = Shell::new(ShellOptions {
            store: Some(store.clone()),
            company_key: "co-1".into(),
            ..ShellOptions::default()
        });
        shell.dispatch_now(CommandId::new("vendors.pay_bills"));
        shell.dispatch_now(CommandId::new("banking.register"));
        shell.save_layout();
    }
    let again = Shell::new(ShellOptions {
        store: Some(store.clone()),
        company_key: "co-1".into(),
        ..ShellOptions::default()
    });
    let kinds: Vec<&str> = again
        .windows
        .all()
        .iter()
        .map(|w| w.kind.as_str())
        .collect();
    assert_eq!(kinds, ["nav.home", "vendors.pay_bills", "banking.register"]);
    assert_eq!(
        again.windows.active().map(|w| w.kind.as_str()),
        Some("banking.register")
    );
    let other = Shell::new(ShellOptions {
        store: Some(store),
        company_key: "co-2".into(),
        ..ShellOptions::default()
    });
    assert_eq!(other.windows.len(), 1, "another company starts at Home");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn error_dialog_opens_for_owner_facing_errors() {
    let ctx = context();
    let mut shell = Shell::new(ShellOptions::default());
    shell.show_error(&cl_core::DateError::NoSuchDay);
    assert!(shell.error_dialog().is_some());
    settle(&ctx, &mut shell, WIDE);
    let d = shell.error_dialog().unwrap();
    assert_eq!(d.message, t("error.date.no_such_day"));
}

#[test]
fn about_and_privacy_open_from_help() {
    let ctx = context();
    let mut shell = Shell::new(ShellOptions::default());
    shell.dispatch_now(CommandId::new("help.about"));
    assert!(shell.about_open());
    settle(&ctx, &mut shell, WIDE);
    shell.dispatch_now(CommandId::new("help.privacy"));
    settle(&ctx, &mut shell, WIDE);
    assert!(shell.about_open());
}

/// Accessible names: every button, tab, and text box in a frame has a label
/// (AccessKit), so screen readers and the control channel can find it.
#[test]
fn interactive_widgets_have_accessible_labels() {
    use egui::accesskit::Role;
    for (size, open_palette) in [(WIDE, false), (vec2(480.0, 320.0), false), (WIDE, true)] {
        let ctx = context();
        ctx.enable_accesskit();
        let mut shell = demo();
        if open_palette {
            shell.dispatch_now(CommandId::new("help.palette"));
        }
        settle(&ctx, &mut shell, size);
        let out = frame(&ctx, &mut shell, size, vec![]);
        let update = out
            .platform_output
            .accesskit_update
            .expect("accesskit is on");
        let mut unnamed = Vec::new();
        let mut seen_tab = false;
        let mut seen_button = false;
        for (_, node) in &update.nodes {
            let interactive = matches!(
                node.role(),
                Role::Button | Role::Tab | Role::TextInput | Role::CheckBox | Role::MenuItem
            );
            seen_tab |= node.role() == Role::Tab;
            seen_button |= node.role() == Role::Button;
            let named = node.label().is_some_and(|l| !l.is_empty())
                || !node.labelled_by().is_empty()
                || node.value().is_some_and(|v| !v.is_empty())
                || node.description().is_some_and(|d| !d.is_empty());
            if interactive && !named {
                unnamed.push(format!("{:?} {:?}", node.role(), node));
            }
        }
        assert!(seen_button, "{size:?}: no buttons found");
        if size.x > 700.0 {
            assert!(seen_tab, "{size:?}: no tabs found");
        }
        assert!(unnamed.is_empty(), "{size:?}: unnamed widgets: {unnamed:?}");
    }
}

/// Opens a window the way screens do: through an event on the link (the
/// recovery-key and opening-balances windows have no menu command).
fn open_window(ctx: &egui::Context, shell: &mut Shell, kind: &'static str) {
    if commands::find(kind).is_some() {
        shell.dispatch_now(CommandId::new(kind));
    } else {
        shell
            .company_link()
            .expect("demo link")
            .push(crate::company::CompanyEvent::OpenWindow(CommandId::new(
                kind,
            )));
    }
    settle(ctx, shell, vec2(1280.0, 820.0));
}

/// `OpenRecord` (P14): a screen that shows no saved records yet simply
/// opens, or comes to the front, once.
#[test]
fn open_record_falls_back_to_opening_the_window() {
    let ctx = context();
    let mut shell = demo();
    settle(&ctx, &mut shell, vec2(1280.0, 820.0));
    let kind = CommandId::new(crate::screens::deposit::KIND);
    let txn =
        cl_core::ids::TransactionId::parse("d0000000-0000-4000-8000-b00000000001").expect("id");
    let n = shell.windows.len();
    for _ in 0..2 {
        shell
            .company_link()
            .expect("demo link")
            .push(crate::company::CompanyEvent::OpenRecord(kind, txn));
        settle(&ctx, &mut shell, vec2(1280.0, 820.0));
        assert_eq!(shell.windows.len(), n + 1);
        assert_eq!(
            shell.windows.active().map(|w| w.kind.as_str()),
            Some(crate::screens::deposit::KIND)
        );
    }
}

/// Every company screen draws in the demo shell, reports its state, and
/// the Open Company window fills the body once the company is closed.
#[test]
fn company_screens_render_in_the_demo_shell() {
    let ctx = context();
    let mut shell = demo();
    let size = vec2(1280.0, 820.0);
    settle(&ctx, &mut shell, size);
    for kind in [
        "file.open_company",
        "file.new_company",
        "file.backup",
        "file.portable_copy",
        "file.export_all",
        "file.delete_company",
        "company.recovery_key",
        "company.opening_balances",
    ] {
        open_window(&ctx, &mut shell, kind);
        let state = shell.active_screen_state();
        assert!(state["screen"].is_string(), "{kind}: {state}");
        // The same command focuses the existing window instead of opening
        // a second one.
        let n = shell.windows.len();
        open_window(&ctx, &mut shell, kind);
        assert_eq!(shell.windows.len(), n, "{kind} opened twice");
    }
    // Narrow layout of every open window.
    settle(&ctx, &mut shell, vec2(960.0, 640.0));
    for id in shell.windows.all().iter().map(|w| w.id).collect::<Vec<_>>() {
        shell.windows.activate(id);
        settle(&ctx, &mut shell, vec2(960.0, 640.0));
    }
    // Closing the company with the demo's dirty window prompts first;
    // discarding it finishes the close by itself.
    shell.dispatch_now(CommandId::new("file.close_company"));
    // (The demo's Write Check and the shown recovery key both count.)
    assert!(shell.windows.prompt().is_some());
    let mut rounds = 0;
    while shell.windows.prompt().is_some() {
        let _ = shell
            .windows
            .answer_prompt(crate::windows::PromptAnswer::Discard);
        shell.dispatch_now(CommandId::new("file.close_company"));
        rounds += 1;
        assert!(rounds < 10, "the close keeps prompting");
    }
    settle(&ctx, &mut shell, size);
    assert!(shell.company.is_none());
    assert_eq!(shell.active_screen_state()["screen"], "open_company");
    // With no company open, other screens go to Open Company instead.
    shell.dispatch_now(CommandId::new("file.backup"));
    settle(&ctx, &mut shell, size);
    assert_eq!(shell.active_screen_state()["screen"], "open_company");
    assert!(shell.toast_text().is_none() || shell.toast_text().is_some());
}

/// The interview moves through its pages with the form commands, refuses
/// to leave the first page without a name, and Esc reverts then closes.
#[test]
fn interview_pages_and_escape() {
    let ctx = context();
    let mut shell = demo();
    let size = vec2(1280.0, 820.0);
    settle(&ctx, &mut shell, size);
    shell.dispatch_now(CommandId::new("file.new_company"));
    settle(&ctx, &mut shell, size);
    assert_eq!(shell.active_screen_state()["page"], "names");
    shell.dispatch_now(CommandId::new("form.next_record"));
    assert_eq!(
        shell.active_screen_state()["page"],
        "names",
        "a name is required"
    );
    assert_eq!(shell.active_screen_state()["issues"], 1);
    // Nothing typed: Esc has nothing to revert, so the window closes (it is
    // clean, so no prompt).
    let before = shell.windows.len();
    shell.dispatch_now(CommandId::new("form.revert"));
    assert_eq!(shell.windows.len(), before - 1);
    assert!(shell.windows.prompt().is_none());
    // Opening balances: insert a line, Esc clears it, Esc again closes.
    open_window(&ctx, &mut shell, "company.opening_balances");
    assert_eq!(shell.active_screen_state()["screen"], "opening_balances");
    shell.dispatch_now(CommandId::new("form.insert_line"));
    assert_eq!(shell.active_screen_state()["bank_rows"], 2);
    let before = shell.windows.len();
    shell.dispatch_now(CommandId::new("form.revert"));
    // Nothing typed into the rows: Esc has nothing to revert and the
    // window is clean, so it closes without a prompt.
    assert_eq!(shell.windows.len(), before - 1, "closed");
    assert!(shell.windows.prompt().is_none());
    open_window(&ctx, &mut shell, "company.opening_balances");
    shell.dispatch_now(CommandId::new("form.save_close"));
    assert_eq!(
        shell.active_screen_state()["issues"],
        1,
        "nothing to post yet"
    );
}

/// T18 (S8): developer screens exist only with `--demo`, on every path.
#[test]
fn dev_screen_needs_demo() {
    let dev = CommandId::new("dev.grid_demo");
    // A shell without --demo knows no dev command.
    let mut plain = Shell::new(ShellOptions::default());
    assert!(plain.find_command("dev.grid_demo").is_none());
    plain.dispatch_now(dev);
    assert!(plain.windows.find_kind(dev).is_none(), "run");
    assert!(
        super::palette::matches_in("", plain.palette.include_dev)
            .iter()
            .all(|c| !c.id.as_str().starts_with("dev.")),
        "palette"
    );
    // Every window-building path goes through make_any: without --demo a
    // dev kind is a plain placeholder.
    let today = plain.today();
    let s = super::make_any(dev, None, None, today, plain.prefs, false);
    assert_eq!(s.state(), serde_json::Value::Null, "placeholder");
    let s = super::make_any(dev, None, None, today, plain.prefs, true);
    assert_eq!(s.state()["mode"], "lines", "the demo screen");

    // A company event asking for the window.
    let dir = std::env::temp_dir().join(format!("cl-ui-dev-gate-{}", std::process::id()));
    let link = std::sync::Arc::new(crate::company::CompanyLink::demo(&dir));
    let mut linked = Shell::new(ShellOptions {
        company: Some(std::sync::Arc::clone(&link)),
        ..ShellOptions::default()
    });
    link.push(crate::company::CompanyEvent::OpenWindow(dev));
    let ctx = context();
    settle(&ctx, &mut linked, WIDE);
    if let Some(id) = linked.windows.find_kind(dev) {
        let w = linked.windows.get(id).unwrap();
        assert_eq!(w.screen.state(), serde_json::Value::Null, "event");
    }

    // A remembered layout naming the dev window restores nothing for it.
    let store_dir = std::env::temp_dir().join(format!("cl-ui-dev-layout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&store_dir);
    let store = crate::prefs::UiStore::new(&store_dir);
    let layout = crate::windows::SessionLayout {
        version: crate::windows::SessionLayout::VERSION,
        tabs: vec![
            crate::windows::SavedTab {
                kind: "dev.grid_demo".into(),
                popped_out: false,
            },
            crate::windows::SavedTab {
                kind: "vendors.pay_bills".into(),
                popped_out: false,
            },
        ],
        active: 0,
    };
    store.save_layout("default", &layout).unwrap();
    let restored = Shell::new(ShellOptions {
        store: Some(store),
        ..ShellOptions::default()
    });
    assert!(restored.windows.find_kind(dev).is_none(), "layout restore");
    assert!(
        restored
            .windows
            .find_kind(CommandId::new("vendors.pay_bills"))
            .is_some()
    );

    // With --demo it opens, is in the palette, and is never saved in a
    // layout.
    let mut demo = demo();
    assert!(demo.find_command("dev.grid_demo").is_some());
    assert!(
        super::palette::matches_in("grid", demo.palette.include_dev)
            .iter()
            .any(|c| c.id == dev)
    );
    demo.dispatch_now(dev);
    let id = demo.windows.find_kind(dev).expect("opened");
    assert_eq!(demo.windows.active_id(), Some(id));
    assert_eq!(
        demo.windows.get(id).unwrap().screen.state()["mode"],
        "lines"
    );
    let snap = demo.windows.snapshot();
    assert!(
        snap.tabs.iter().all(|t| !t.kind.starts_with("dev.")),
        "{snap:?}"
    );
    assert!(snap.active < snap.tabs.len());
    let ctx = context();
    settle(&ctx, &mut demo, WIDE);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(store_dir);
}

#[test]
fn grid_demo_renders_in_both_themes_and_small_windows() {
    for theme in [
        crate::theme::ThemeChoice::Light,
        crate::theme::ThemeChoice::Dark,
    ] {
        for size in [WIDE, vec2(480.0, 320.0)] {
            let ctx = context();
            let mut shell = demo();
            shell.prefs.theme = theme;
            shell.dispatch_now(CommandId::new("dev.grid_demo"));
            settle(&ctx, &mut shell, size);
            let used = ctx.globally_used_rect();
            assert!(
                used.max.x <= size.x + 0.5 && used.max.y <= size.y + 0.5,
                "{theme:?} {size:?}: {used:?}"
            );
        }
    }
}
