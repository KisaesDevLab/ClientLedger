//! Report Builder editor tests (A08).

use std::sync::Arc;

use cl_model::CommandId;
use cl_model::report_def::editor::Origin;

use super::{BuilderSeed, CustomizeSeed, EditorId, KIND};
use crate::company::CompanyEvent;
use crate::shell::{Shell, ShellOptions};
use crate::windows::OpenPayload;

fn demo() -> Shell {
    Shell::new(ShellOptions {
        demo: true,
        ..ShellOptions::default()
    })
}

fn blank_definition() -> cl_model::report_def::BuilderDefinition {
    // A blank report breaks the "at least one section" rule: built, not
    // parsed (the editor accepts invalid states, plan §3.1).
    serde_json::from_str(
        r#"{"schemaVersion":1,"kind":"builder","dates":{"mode":"range"},
            "columns":{"detailFields":[],"compare":[],"headers":{}},"nodes":[]}"#,
    )
    .expect("a blank definition")
}

fn customize(editor: Option<EditorId>) -> BuilderSeed {
    BuilderSeed::Customize(Box::new(CustomizeSeed {
        def: blank_definition(),
        title: "Mine".into(),
        origin: Origin::Memorized,
        saved: None,
        dates: None,
        basis: None,
        adjusting: None,
        filters_dropped: false,
        editor,
    }))
}

fn open(shell: &mut Shell, seed: BuilderSeed) {
    let link = Arc::clone(shell.company_link().expect("demo link"));
    link.push(CompanyEvent::OpenWith(
        CommandId::new(KIND),
        Box::new(OpenPayload::BuilderEditor(Box::new(seed))),
    ));
    shell.apply_events_now();
}

fn editors(shell: &Shell) -> Vec<u64> {
    shell
        .windows
        .all()
        .iter()
        .filter(|w| w.kind.as_str() == KIND)
        .filter_map(|w| w.screen.state()["editor"].as_u64())
        .collect()
}

#[test]
fn a_customize_for_an_open_editor_returns_to_it_and_others_open_new_ones() {
    let mut shell = demo();
    open(&mut shell, BuilderSeed::Blank);
    let first = editors(&shell);
    assert_eq!(first.len(), 1, "the editor opened");
    // Customize from that editor's result tab: the same window (Q-A08x).
    open(&mut shell, customize(Some(EditorId(first[0]))));
    assert_eq!(editors(&shell), first);
    // Customize naming no editor, or one that is gone: a new window.
    open(&mut shell, customize(None));
    open(&mut shell, customize(Some(EditorId(u64::MAX))));
    assert_eq!(editors(&shell).len(), 3);
}

// ---- WP-A2: the editor screen on the demo company ----

use cl_core::CalendarDate;
use cl_model::report_def::editor::{Op, RowKind, Saved, SavedKind};
use cl_model::report_def::{Filter, FilterOp, Node, Term, outline as tree};
use cl_model::{COMMANDS, Key, Keymap, Mods, Scope, Shortcut};
use eframe::egui;

use super::{BuilderScreen, FieldKey, Pane};
use crate::company::CompanyLink;
use crate::windows::Screen;

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn editor(seed: BuilderSeed) -> (tempfile::TempDir, Arc<CompanyLink>, BuilderScreen) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let mut s = BuilderScreen::new(Arc::clone(&link), None, today());
    assert!(s.open_with(OpenPayload::BuilderEditor(Box::new(seed))));
    (dir, link, s)
}

/// A context with the app's theme (its text styles).
fn themed() -> egui::Context {
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    ctx
}

/// One frame at `secs`.
fn frame(ctx: &egui::Context, s: &mut BuilderScreen, secs: f64) {
    let mut out = ctx.run_ui(
        egui::RawInput {
            time: Some(secs),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 900.0),
            )),
            ..Default::default()
        },
        |ui| s.ui(ui),
    );
    out.textures_delta.clear();
}

/// Presses `mods+key` as the shell would: resolved in the screen's scope.
fn press(s: &mut BuilderScreen, mods: Mods, key: Key) -> Option<&'static str> {
    let km = Keymap::new(COMMANDS).expect("keymap");
    let id = km.resolve(s.scope(), Shortcut::new(mods, key))?;
    assert!(s.command(id), "{} reaches the editor", id.as_str());
    Some(id.as_str())
}

fn add(s: &mut BuilderScreen, kind: RowKind, caption: &str) -> String {
    s.apply(Op::Add {
        kind,
        caption: caption.into(),
    })
    .expect("added");
    s.ed.selection().expect("selected").to_string()
}

fn types_filter(t: &str) -> Filter {
    Filter {
        field: "account_type".into(),
        op: FilterOp::In,
        ids: None,
        values: Some(vec![t.into()]),
        value: None,
    }
}

/// Income, Cost of goods sold, Gross profit.
fn gross_profit(s: &mut BuilderScreen) -> (String, String, String) {
    let inc = add(s, RowKind::Section, "Income");
    let cogs = add(s, RowKind::Section, "Cost of goods sold");
    for (id, ty) in [(&inc, "revenue"), (&cogs, "cogs")] {
        let Some(Node::Section(mut sec)) = tree::find(&s.ed.definition().nodes, id).cloned() else {
            panic!("section");
        };
        sec.filters.push(types_filter(ty));
        s.apply(Op::Edit(Node::Section(sec)));
    }
    let gp = add(s, RowKind::Formula, "Gross profit");
    let Some(Node::Formula(mut f)) = tree::find(&s.ed.definition().nodes, &gp).cloned() else {
        panic!("formula");
    };
    f.terms = vec![
        Term {
            node_id: inc.clone(),
            sign: 1,
        },
        Term {
            node_id: cogs.clone(),
            sign: -1,
        },
    ];
    s.apply(Op::Edit(Node::Formula(f)));
    (inc, cogs, gp)
}

#[test]
fn every_outline_key_acts_on_the_tree_and_only_there() {
    let (_d, _l, mut s) = editor(BuilderSeed::Blank);
    let ctx = themed();
    frame(&ctx, &mut s, 0.0);
    // Off the tree the outline's keys are not bound: Tab in a caption
    // moves on, Delete deletes text.
    s.outline_focused = false;
    assert_eq!(s.scope(), Scope::Builder);
    let km = Keymap::new(COMMANDS).expect("keymap");
    for k in [Key::Tab, Key::Delete, Key::Insert, Key::F(2), Key::Up] {
        let id = km.resolve(Scope::Builder, Shortcut::new(Mods::NONE, k));
        assert!(
            !id.is_some_and(|c| c.as_str().starts_with("builder.")),
            "{k:?}"
        );
    }
    let g = add(&mut s, RowKind::Group, "Expenses");
    s.ed.set_folded(&g, true);
    let a = add(&mut s, RowKind::Text, "Note");
    s.outline_focused = true;
    assert_eq!(s.scope(), Scope::Outline);
    assert_eq!(press(&mut s, Mods::NONE, Key::Insert), Some("builder.add"));
    assert!(s.add_menu);
    assert_eq!(press(&mut s, Mods::NONE, Key::Tab), Some("builder.indent"));
    assert_eq!(tree::locate(&s.ed.definition().nodes, &a), Some(vec![0, 0]));
    assert_eq!(
        press(&mut s, Mods::SHIFT, Key::Tab),
        Some("builder.outdent")
    );
    assert_eq!(tree::locate(&s.ed.definition().nodes, &a), Some(vec![1]));
    assert_eq!(press(&mut s, Mods::ALT, Key::Up), Some("builder.move_up"));
    assert_eq!(tree::locate(&s.ed.definition().nodes, &a), Some(vec![0]));
    assert_eq!(
        press(&mut s, Mods::ALT, Key::Down),
        Some("builder.move_down")
    );
    assert_eq!(tree::locate(&s.ed.definition().nodes, &a), Some(vec![1]));
    assert_eq!(press(&mut s, Mods::NONE, Key::Up), Some("builder.prev_row"));
    assert_eq!(s.ed.selection(), Some(g.as_str()));
    assert_eq!(
        press(&mut s, Mods::NONE, Key::Down),
        Some("builder.next_row")
    );
    assert_eq!(s.ed.selection(), Some(a.as_str()));
    assert_eq!(press(&mut s, Mods::NONE, Key::F(2)), Some("builder.rename"));
    assert_eq!(s.rename.as_ref().map(|r| r.1.as_str()), Some("Note"));
    s.rename = None;
    assert_eq!(
        press(&mut s, Mods::NONE, Key::Delete),
        Some("builder.remove")
    );
    assert!(tree::find(&s.ed.definition().nodes, &a).is_none());
    assert!(s.status.is_some(), "the status line says what went");
    // Undo and redo, from the tree or anywhere in the editor.
    assert_eq!(
        press(&mut s, Mods::CMD, Key::Letter('Z')),
        Some("builder.undo")
    );
    assert!(tree::find(&s.ed.definition().nodes, &a).is_some());
    s.outline_focused = false;
    assert_eq!(
        press(&mut s, Mods::CMD, Key::Letter('Y')),
        Some("builder.redo")
    );
    assert!(tree::find(&s.ed.definition().nodes, &a).is_none());
    assert_eq!(
        press(&mut s, Mods::CMD, Key::Enter),
        Some("builder.refresh_preview")
    );
    // Left and Right fold and unfold.
    s.outline_focused = true;
    s.ed.select(Some(&g));
    s.ed.set_folded(&g, false);
    add(&mut s, RowKind::Blank, "");
    s.ed.select(Some(&g));
    assert_eq!(press(&mut s, Mods::NONE, Key::Left), Some("builder.fold"));
    assert!(s.ed.is_folded(&g));
    assert_eq!(
        press(&mut s, Mods::NONE, Key::Right),
        Some("builder.unfold")
    );
    assert!(!s.ed.is_folded(&g));
}

#[test]
fn f6_cycles_the_panes_both_ways() {
    let (_d, _l, mut s) = editor(BuilderSeed::Blank);
    assert_eq!(s.pane, Pane::Outline);
    for want in [Pane::Properties, Pane::Preview, Pane::Outline] {
        assert_eq!(
            press(&mut s, Mods::NONE, Key::F(6)),
            Some("builder.next_pane")
        );
        assert_eq!(s.pane, want);
        assert_eq!(s.focus_to, Some(want));
    }
    assert_eq!(
        press(&mut s, Mods::SHIFT, Key::F(6)),
        Some("builder.prev_pane")
    );
    assert_eq!(s.pane, Pane::Preview);
    // The focus moves for real in a frame.
    let ctx = themed();
    add(&mut s, RowKind::Text, "Row");
    s.focus_to = Some(Pane::Outline);
    frame(&ctx, &mut s, 0.0);
    frame(&ctx, &mut s, 0.1);
    assert!(s.outline_focused, "the selected tree row has the keyboard");
    assert_eq!(s.scope(), Scope::Outline);
}

#[test]
fn ctrl_z_in_a_text_field_reverts_that_field() {
    let (_d, _l, mut s) = editor(BuilderSeed::Blank);
    let g = add(&mut s, RowKind::Group, "Expenses");
    // Typed but not left yet.
    let key = FieldKey::Caption(g.clone());
    s.drafts.insert(key.clone(), "Costs".into());
    s.pending = Some(key);
    assert_eq!(
        press(&mut s, Mods::CMD, Key::Letter('Z')),
        Some("builder.undo")
    );
    let Some(Node::Group(grp)) = tree::find(&s.ed.definition().nodes, &g) else {
        panic!("group");
    };
    assert_eq!(grp.caption, "Expenses");
    assert!(s.drafts.is_empty() && s.pending.is_none());
}

/// Frames until the preview has nothing pending (demo answers at once).
fn settle(ctx: &egui::Context, s: &mut BuilderScreen, from: f64) -> f64 {
    let mut t = from;
    for _ in 0..20 {
        frame(ctx, s, t);
        t += 0.5;
        if !s.preview.busy() {
            break;
        }
    }
    t
}

#[test]
fn the_preview_runs_after_the_debounce_and_a_problem_keeps_the_table() {
    let (_d, link, mut s) = editor(BuilderSeed::Blank);
    let ctx = themed();
    let (_inc, _cogs, gp) = gross_profit(&mut s);
    assert!(s.ed.can_run(), "{:?}", s.ed.problems());
    frame(&ctx, &mut s, 0.0);
    assert_eq!(s.preview.asked_count, 0, "waits for the debounce");
    let t = settle(&ctx, &mut s, 0.5);
    assert_eq!(s.preview.asked_count, 1);
    let table = s.preview.table.clone().expect("a preview table");
    assert!(table.rows.iter().any(|r| r.caption == "Gross profit"));
    // Two edits in a row: one run.
    s.apply(Op::Name("P&L".into()));
    frame(&ctx, &mut s, t);
    s.apply(Op::Name("P&L two".into()));
    let t = settle(&ctx, &mut s, t + 0.1);
    assert_eq!(s.preview.asked_count, 2);
    // The formula above its rows: a problem; Run report is off and the
    // last good table stays, nothing sent.
    s.apply(Op::MoveUp(gp.clone()));
    s.apply(Op::MoveUp(gp));
    assert!(!s.ed.can_run());
    let _ = settle(&ctx, &mut s, t);
    assert_eq!(s.preview.asked_count, 2);
    assert!(s.preview.table.is_some());
    s.run_report();
    assert!(
        link.drain().is_empty(),
        "Run report sends nothing while problems exist"
    );
}

#[test]
fn run_report_opens_the_full_run_with_the_preview_reference() {
    let (_d, link, mut s) = editor(BuilderSeed::Blank);
    let ctx = themed();
    gross_profit(&mut s);
    settle(&ctx, &mut s, 1.0);
    s.run_report();
    let events = link.drain();
    let Some(CompanyEvent::OpenWith(kind, payload)) = events.into_iter().last() else {
        panic!("Run report opens a tab");
    };
    assert_eq!(kind.as_str(), super::super::KIND);
    let OpenPayload::BuilderDefinition(open) = *payload else {
        panic!("a builder definition");
    };
    assert_eq!(open.editor, s.id);
    assert_eq!(&open.def, s.ed.definition());
    // The tab's reference from what it is handed equals the preview's (AC8).
    let json = serde_json::to_value(&open.def).expect("json");
    let tab = super::super::builder_ref::run_ref(&super::super::builder_ref::RunInput {
        def: &open.def,
        json: &json,
        spec: super::super::builder_ref::spec_for(&open.def),
        options: &open.options,
        fiscal: &s.fiscal,
        today: s.today,
    })
    .expect("a reference");
    assert_eq!(Some(tab), s.preview.shown_ref);
}

#[test]
fn save_changes_needs_the_right_to_change_it() {
    let (_d, _l, mut s) = editor(BuilderSeed::Blank);
    add(&mut s, RowKind::Text, "Note");
    let saved = Saved {
        id: "r1".into(),
        version: 2,
        kind: SavedKind::Builder,
        can_edit: false,
        def: s.ed.definition().clone(),
        name: String::new(),
        group_id: None,
        author: None,
        updated_at: None,
    };
    s.ed.mark_saved(saved);
    s.apply(Op::Name("Mine".into()));
    assert!(s.ed.dirty() && !s.ed.can_save_changes());
    assert!(s.command(CommandId::new("builder.save_changes")));
    assert_eq!(s.save.state_name(), "idle", "nothing is sent");
}

#[test]
fn closing_a_dirty_editor_memorizes_then_closes() {
    let (_d, link, mut s) = editor(BuilderSeed::Blank);
    assert!(!Screen::dirty(&s));
    gross_profit(&mut s);
    assert!(Screen::dirty(&s));
    assert!(!Screen::save(&mut s), "the window stays until saved");
    assert_eq!(s.save.state_name(), "memorize");
    let ctx = themed();
    frame(&ctx, &mut s, 0.0);
    if let Some(d) = s.save.memorize.as_mut() {
        d.name = "Gross profit report".into();
    }
    super::save::memorize_for_test(&mut s);
    for t in 1..5 {
        frame(&ctx, &mut s, f64::from(t));
    }
    assert!(!Screen::dirty(&s), "saved");
    assert_eq!(s.ed.name(), "Gross profit report");
    assert!(s.ed.saved().is_some_and(|x| x.kind == SavedKind::Builder));
    assert!(
        link.drain()
            .iter()
            .any(|e| matches!(e, CompanyEvent::CloseWindow(k) if k.as_str() == KIND))
    );
}

#[test]
fn guided_steps_by_keyboard_then_open_in_advanced() {
    let (_d, _l, mut s) = editor(BuilderSeed::Guided);
    assert_eq!(s.state()["mode"], "guided");
    assert_eq!(
        press(&mut s, Mods::ALT, Key::Letter('N')),
        Some("builder.guided_next")
    );
    assert_eq!(
        s.guided.as_ref().map(|g| g.step),
        Some(cl_model::report_def::guided::Step::Source),
        "Next waits for an answer"
    );
    s.guided
        .as_mut()
        .expect("guided")
        .set_source(cl_model::report_def::SourceKey::ArOpen);
    s.guided_changed();
    press(&mut s, Mods::ALT, Key::Letter('N'));
    press(&mut s, Mods::ALT, Key::Letter('N'));
    s.guided.as_mut().expect("guided").display = Some(cl_model::report_def::Display::Summary);
    press(&mut s, Mods::ALT, Key::Letter('N'));
    press(&mut s, Mods::ALT, Key::Letter('B'));
    assert_eq!(
        s.guided.as_ref().map(|g| g.step),
        Some(cl_model::report_def::guided::Step::Lines)
    );
    s.open_in_advanced();
    assert_eq!(s.state()["mode"], "advanced");
    let def = s.ed.definition();
    assert_eq!(def.nodes.len(), 1);
    assert_eq!(def.columns.aging_buckets, Some(true), "Q-A08i");
    assert_eq!(def.dates.mode, cl_model::report_def::DateMode::AsOf);
    // One undo step back to blank.
    assert!(s.ed.undo());
    assert_eq!(s.ed.row_count(), 0);
}

#[test]
fn the_amount_filter_keeps_the_typed_digits() {
    let (_d, _l, mut s) = editor(BuilderSeed::Blank);
    let sec = add(&mut s, RowKind::Section, "Big sales");
    let Some(Node::Section(mut n)) = tree::find(&s.ed.definition().nodes, &sec).cloned() else {
        panic!("section");
    };
    n.filters.push(Filter {
        field: "amount".into(),
        op: FilterOp::Gte,
        ids: None,
        values: None,
        value: Some("0".into()),
    });
    s.apply(Op::Edit(Node::Section(n)));
    s.commit(&FieldKey::Amount(sec.clone(), 0), "1,234.50".into());
    let value = |s: &BuilderScreen| match tree::find(&s.ed.definition().nodes, &sec) {
        Some(Node::Section(x)) => x.filters[0].value.clone(),
        _ => None,
    };
    assert_eq!(value(&s).as_deref(), Some("1234.50"));
    s.commit(&FieldKey::Amount(sec.clone(), 0), "1e3".into());
    assert_eq!(value(&s).as_deref(), Some("1234.50"), "refused, unchanged");
    assert_eq!(
        s.status.as_deref(),
        Some(crate::i18n::t("builder.status.not_an_amount").as_str())
    );
}

#[test]
fn every_message_key_from_the_model_has_text() {
    use cl_model::report_def::editor::Refusal;
    use cl_model::report_def::editor::choices::greyed;
    use cl_model::report_def::problems::ProblemCode;
    use cl_model::report_def::validate::Code;
    for r in [
        Refusal::TooManyRows,
        Refusal::TooDeep,
        Refusal::NoGroupAbove,
        Refusal::TooManyFields,
        Refusal::EmptyCaption,
        Refusal::NotFound,
    ] {
        assert!(crate::i18n::has(r.key()), "{}", r.key());
    }
    let mut codes: Vec<ProblemCode> = Code::ALL.iter().map(|c| ProblemCode::Rule(*c)).collect();
    codes.extend([
        ProblemCode::NotInCompany,
        ProblemCode::NotOnCash,
        ProblemCode::Other,
    ]);
    for c in codes {
        assert!(crate::i18n::has(&c.key()), "{}", c.key());
    }
    for k in [
        greyed::BELOW,
        greyed::OTHER_UNIT,
        greyed::NO_FIGURES,
        greyed::AGING_OPEN_ITEMS,
        greyed::AGING_PERIODS,
        greyed::PERIODS_AGING,
        greyed::PERIODS_RUNNING,
        greyed::RUNNING_BALANCE,
        greyed::PERCENT_MONEY,
        greyed::NOT_FOR_SOURCE,
        "reports.builder.greyed.quantity_cash",
        "reports.builder.greyed.inventory",
        "reports.builder.greyed.project",
    ] {
        assert!(crate::i18n::has(k), "{k}");
    }
    for s in cl_model::report_def::SourceKey::ALL {
        assert!(crate::i18n::has(&format!("builder.source.{}", s.as_str())));
        assert!(crate::i18n::has(&format!(
            "builder.guided.source.{}",
            s.as_str()
        )));
    }
    for k in cl_model::report_def::fields::GROUP_KEYS {
        assert!(crate::i18n::has(&format!("builder.group.{k}")), "{k}");
    }
    for k in cl_model::report_def::fields::FILTER_FIELDS {
        assert!(
            crate::i18n::has(&format!("builder.filter.field.{k}")),
            "{k}"
        );
    }
}

#[test]
fn every_screen_draws_in_light_and_dark() {
    for theme in [
        crate::theme::ThemeChoice::Light,
        crate::theme::ThemeChoice::Dark,
    ] {
        let ctx = themed();
        crate::theme::apply(&ctx, theme);
        let (_d, _l, mut s) = editor(BuilderSeed::Blank);
        let (inc, ..) = gross_profit(&mut s);
        s.ed.select(Some(&inc));
        for tab in [
            super::PropTab::Row,
            super::PropTab::Columns,
            super::PropTab::Report,
        ] {
            s.tab = tab;
            frame(&ctx, &mut s, 1.0);
        }
        settle(&ctx, &mut s, 2.0);
        let (_d2, _l2, mut g) = editor(BuilderSeed::Guided);
        frame(&ctx, &mut g, 0.0);
    }
}
