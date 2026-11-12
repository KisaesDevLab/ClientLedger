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

/// An editor on the shell's company with a dirty Gross profit report.
fn dirty_editor(link: &Arc<CompanyLink>, name: &str) -> BuilderScreen {
    let mut s = BuilderScreen::new(Arc::clone(link), None, today());
    assert!(s.open_with(OpenPayload::BuilderEditor(Box::new(BuilderSeed::Blank))));
    gross_profit(&mut s);
    s.apply(Op::Name(name.into()));
    s
}

#[test]
fn saving_from_the_close_prompt_closes_that_editor_only() {
    let mut shell = demo();
    let link = Arc::clone(shell.company_link().expect("demo link"));
    let ctx = themed();
    let mut first = dirty_editor(&link, "First report");
    let second = dirty_editor(&link, "Second report");
    assert!(Screen::dirty(&first) && Screen::dirty(&second));
    // The close prompt's Save on the first: Memorize, then it closes.
    assert!(!Screen::save(&mut first), "the window stays until saved");
    assert_eq!(first.save.state_name(), "memorize");
    super::save::memorize_for_test(&mut first);
    for t in 0..4 {
        frame(&ctx, &mut first, f64::from(t));
    }
    assert!(!Screen::dirty(&first), "saved");
    assert_eq!(first.ed.name(), "First report");
    assert!(
        first
            .ed
            .saved()
            .is_some_and(|x| x.kind == SavedKind::Builder)
    );
    let first_id = first.id.0;
    let second_id = second.id.0;
    // The second editor was opened first: "the first editor of its kind"
    // would be the wrong one.
    shell
        .windows
        .open_new(CommandId::new(KIND), Box::new(second));
    shell
        .windows
        .open_new(CommandId::new(KIND), Box::new(first));
    assert_eq!(editors(&shell), [second_id, first_id]);
    shell.apply_events_now();
    assert_eq!(editors(&shell), [second_id], "only the saved editor closed");
    shell.apply_events_now();
    assert_eq!(editors(&shell), [second_id], "and only once");
}

#[test]
fn tab_in_a_caption_field_moves_on_and_never_indents() {
    let (_d, _l, mut s) = editor(BuilderSeed::Blank);
    let ctx = themed();
    let g = add(&mut s, RowKind::Group, "Expenses");
    s.ed.set_folded(&g, true);
    let row = add(&mut s, RowKind::Group, "Second");
    frame(&ctx, &mut s, 0.0);
    // Focus the Row tab's Caption field for real.
    s.focus_to = None;
    let field = super::picker::text_id(s.id, &FieldKey::Caption(row.clone()));
    ctx.memory_mut(|m| m.request_focus(field));
    frame(&ctx, &mut s, 0.1);
    assert!(
        ctx.memory(|m| m.has_focus(field)),
        "the caption field has the keyboard"
    );
    assert!(!s.outline_focused);
    assert_eq!(s.scope(), Scope::Builder);
    // Tab as the shell sees it: nothing of the outline's in this scope.
    let km = Keymap::new(COMMANDS).expect("keymap");
    assert_eq!(
        km.resolve(s.scope(), Shortcut::new(Mods::NONE, Key::Tab)),
        None
    );
    // And egui's own Tab: focus moves on, the row stays where it is.
    let mut out = ctx.run_ui(
        egui::RawInput {
            time: Some(0.2),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 900.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ui| s.ui(ui),
    );
    out.textures_delta.clear();
    frame(&ctx, &mut s, 0.3);
    assert_eq!(
        tree::locate(&s.ed.definition().nodes, &row),
        Some(vec![1]),
        "not indented"
    );
    assert!(!ctx.memory(|m| m.has_focus(field)), "focus moved on");
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

// ---- WP-A3: against the demo's Report Builder (B4) ----

/// Adds a section listing every bank and asset transaction this year (a
/// demo line has more than 50).
fn every_expense(s: &mut BuilderScreen) -> String {
    let mut dates = s.ed.definition().dates.clone();
    dates.preset = Some("this_year".into());
    s.apply(Op::Dates(dates));
    let id = add(s, RowKind::Section, "Every asset transaction");
    let Some(Node::Section(mut sec)) = tree::find(&s.ed.definition().nodes, &id).cloned() else {
        panic!("section");
    };
    sec.filters.push(types_filter("asset"));
    sec.polarity = Some(cl_model::report_def::Polarity::Debit);
    sec.default_expanded = true;
    sec.display = cl_model::report_def::Display::Detail;
    s.apply(Op::Edit(Node::Section(sec)));
    id
}

/// The Run report payload the editor pushed.
fn run_payload(link: &CompanyLink) -> OpenPayload {
    link.drain()
        .into_iter()
        .rev()
        .find_map(|e| match e {
            CompanyEvent::OpenWith(_, p) if matches!(*p, OpenPayload::BuilderDefinition(_)) => {
                Some(*p)
            }
            _ => None,
        })
        .expect("Run report opened its tab")
}

#[test]
fn the_preview_rows_equal_the_result_tabs_up_to_the_cut() {
    let (_d, link, mut s) = editor(BuilderSeed::Blank);
    let ctx = themed();
    gross_profit(&mut s);
    every_expense(&mut s);
    assert!(s.ed.can_run(), "{:?}", s.ed.problems());
    settle(&ctx, &mut s, 1.0);
    let preview = s.preview.table.clone().expect("a preview");
    // The demo plays U-217: a preview run, cut by the engine at 50
    // transactions per line (the line's "more" row marks the cut).
    assert!(s.preview.engine_cut, "the demo answers a preview run");
    let (order, cut) = super::preview::shown(&preview, s.preview.engine_cut);
    assert!(cut, "the demo line has more than 50 transactions");
    let more = preview
        .rows
        .iter()
        .position(|r| r.kind == cl_core::report::RowKind::More)
        .expect("the cut line's more row");
    let details = preview.rows[..more]
        .iter()
        .rev()
        .take_while(|r| r.kind == cl_core::report::RowKind::Detail)
        .count();
    assert_eq!(details, super::preview::PREVIEW_DETAIL_ROWS);
    assert_eq!(s.state()["preview"]["cut"], true);
    // The full run in its result tab.
    let _ = link.drain();
    s.run_report();
    let payload = run_payload(&link);
    let spec = cl_model::reports::find("profit_loss").expect("p&l");
    let mut w = super::super::ReportWindow::new(
        Arc::clone(&link),
        CommandId::new(super::super::KIND),
        None,
        spec,
        today(),
    );
    w.poll();
    assert!(w.open_with(payload));
    w.poll();
    let tab = w.table.clone().expect("the full run");
    assert!(tab.rows.len() >= preview.rows.len());
    assert_eq!(tab.columns, preview.columns);
    // Up to the cut, row for row (the full run goes on past it).
    for &i in order.iter().filter(|&&i| i < more) {
        let (a, b) = (&preview.rows[i], &tab.rows[i]);
        assert_eq!(
            (a.kind, &a.path, &a.cells),
            (b.kind, &b.path, &b.cells),
            "row {i}"
        );
    }
}

/// Memorizes the editor's report under `name`, frames until done.
fn memorize_as(ctx: &egui::Context, s: &mut BuilderScreen, name: &str) {
    super::save::open_memorize(s, false);
    if let Some(d) = s.save.memorize.as_mut() {
        d.name = name.into();
    }
    super::save::memorize_for_test(s);
    for t in 0..4 {
        frame(ctx, s, f64::from(t));
    }
}

fn rename_first(s: &mut BuilderScreen, text: &str) {
    let first = s.ed.definition().nodes[0].id().to_string();
    s.apply(Op::Rename {
        id: first,
        text: text.into(),
    });
}

fn frames(ctx: &egui::Context, s: &mut BuilderScreen) {
    for t in 0..4 {
        frame(ctx, s, f64::from(t));
    }
}

#[test]
fn someone_elses_change_offers_a_copy_or_their_version() {
    let (_d, link, mut mine) = editor(BuilderSeed::Blank);
    let ctx = themed();
    gross_profit(&mut mine);
    memorize_as(&ctx, &mut mine, "Shared report");
    let saved = mine.ed.saved().cloned().expect("memorized");
    assert!(saved.can_edit, "the author may change it");
    // Someone else opens it and saves a change.
    let mut theirs = BuilderScreen::new(Arc::clone(&link), None, today());
    let seed = BuilderSeed::Customize(Box::new(CustomizeSeed {
        def: mine.ed.definition().clone(),
        title: "Shared report".into(),
        origin: Origin::Memorized,
        saved: Some(saved),
        dates: None,
        basis: None,
        adjusting: None,
        filters_dropped: false,
        editor: None,
    }));
    assert!(theirs.open_with(OpenPayload::BuilderEditor(Box::new(seed))));
    rename_first(&mut theirs, "Sales");
    assert!(theirs.command(CommandId::new("builder.save_changes")));
    frames(&ctx, &mut theirs);
    assert_eq!(theirs.ed.saved().map(|x| x.version), Some(2));
    // Mine, at version 1: someone changed it.
    rename_first(&mut mine, "Revenue");
    assert!(mine.command(CommandId::new("builder.save_changes")));
    frames(&ctx, &mut mine);
    assert!(mine.save.conflict, "the 409 dialog");
    assert_eq!(mine.state()["save"], "conflict");
    // Load their version: one undo step, the new baseline.
    super::save::conflict_choice(&mut mine, 1);
    frames(&ctx, &mut mine);
    assert!(!mine.save.conflict);
    assert_eq!(mine.ed.definition(), theirs.ed.definition());
    assert_eq!(mine.ed.saved().map(|x| x.version), Some(2));
    assert!(!Screen::dirty(&mine));
    assert!(mine.ed.undo(), "loading theirs is undoable");
    assert!(Screen::dirty(&mine));
    // Or a copy of mine.
    rename_first(&mut theirs, "Takings");
    theirs.command(CommandId::new("builder.save_changes"));
    frames(&ctx, &mut theirs);
    mine.command(CommandId::new("builder.save_changes"));
    frames(&ctx, &mut mine);
    assert!(mine.save.conflict);
    super::save::conflict_choice(&mut mine, 0);
    assert_eq!(
        mine.save.memorize.as_ref().map(|d| d.name.as_str()),
        Some("Shared report (copy)")
    );
}

#[test]
fn a_taken_name_is_said_at_the_name() {
    let (_d, link, mut first) = editor(BuilderSeed::Blank);
    let ctx = themed();
    gross_profit(&mut first);
    memorize_as(&ctx, &mut first, "Gross profit");
    let mut second = BuilderScreen::new(Arc::clone(&link), None, today());
    assert!(second.open_with(OpenPayload::BuilderEditor(Box::new(BuilderSeed::Blank))));
    gross_profit(&mut second);
    memorize_as(&ctx, &mut second, "Gross profit");
    let d = second
        .save
        .memorize
        .as_ref()
        .expect("the dialog stays open");
    assert_eq!(d.problem, Some("builder.save.name_taken"));
    assert!(second.ed.saved().is_none());
}

#[test]
fn a_name_no_longer_in_the_company_is_a_problem_until_the_next_edit() {
    let (_d, _l, mut s) = editor(BuilderSeed::Blank);
    let ctx = themed();
    let (inc, ..) = gross_profit(&mut s);
    let Some(Node::Section(mut sec)) = tree::find(&s.ed.definition().nodes, &inc).cloned() else {
        panic!("section");
    };
    sec.filters.push(Filter {
        field: "account".into(),
        op: FilterOp::In,
        ids: Some(vec![crate::company::demo::report_builder::STALE_ID.into()]),
        values: None,
        value: None,
    });
    s.apply(Op::Edit(Node::Section(sec)));
    assert!(s.ed.client_problems().is_empty(), "the desktop can't tell");
    let t = settle(&ctx, &mut s, 1.0);
    let p = s.ed.engine_problem().cloned().expect("the engine said so");
    assert_eq!(
        p.code,
        cl_model::report_def::problems::ProblemCode::NotInCompany
    );
    assert_eq!(p.node_id.as_deref(), Some(inc.as_str()));
    assert!(s.ed.row_has_problem(&inc) && !s.ed.can_run());
    let asked = s.preview.asked_count;
    let t = settle(&ctx, &mut s, t);
    assert_eq!(s.preview.asked_count, asked, "not sent again until an edit");
    // The next edit clears it (and runs again).
    s.apply(Op::Name("Renamed".into()));
    assert!(s.ed.engine_problem().is_none());
    settle(&ctx, &mut s, t + 1.0);
    assert!(s.preview.asked_count > asked);
}

/// R1: an answer made for the report before an edit never lands on the
/// edited one: the engine's problem for a filter just removed is not
/// shown, and the edited report runs.
#[test]
fn an_edit_while_the_preview_runs_drops_that_runs_answer() {
    let (_d, _l, mut s) = editor(BuilderSeed::Blank);
    let ctx = themed();
    let (inc, ..) = gross_profit(&mut s);
    let before = tree::find(&s.ed.definition().nodes, &inc).cloned();
    let Some(Node::Section(mut sec)) = before.clone() else {
        panic!("section");
    };
    sec.filters.push(Filter {
        field: "account".into(),
        op: FilterOp::In,
        ids: Some(vec![crate::company::demo::report_builder::STALE_ID.into()]),
        values: None,
        value: None,
    });
    s.apply(Op::Edit(Node::Section(sec)));
    // The debounce starts, then the run is sent; its answer waits for the
    // next frame.
    frame(&ctx, &mut s, 1.0);
    frame(&ctx, &mut s, 1.5);
    assert!(s.preview.in_flight(), "the run was sent");
    let asked = s.preview.asked_count;
    // The owner takes the filter out before the answer is read.
    let Some(Node::Section(back)) = before else {
        panic!("section");
    };
    s.apply(Op::Edit(Node::Section(back)));
    assert!(!s.preview.in_flight(), "the old run is dropped");
    let _ = settle(&ctx, &mut s, 2.0);
    assert!(s.ed.engine_problem().is_none(), "{:?}", s.ed.problems());
    assert!(s.ed.can_run());
    assert_eq!(s.preview.asked_count, asked + 1, "the edited report ran");
    assert!(s.preview.table.is_some());
    assert_eq!(s.preview.state_json()["phase"], "shown");
    // A late success for a run superseded by session dates is dropped too
    // (the reference is checked, not only the definition).
    frame(&ctx, &mut s, 10.0);
    s.preview.run_now();
    frame(&ctx, &mut s, 10.1);
    assert!(s.preview.in_flight());
    s.band.custom = Some(cl_model::reports::DateChoice::Fixed {
        from: CalendarDate::from_ymd(2026, 1, 1).expect("date"),
        to: CalendarDate::from_ymd(2026, 1, 31).expect("date"),
    });
    let shown_before = s.preview.shown_ref.clone();
    frame(&ctx, &mut s, 10.2);
    assert_eq!(
        s.preview.shown_ref, shown_before,
        "the answer for the old dates was not shown"
    );
}

/// U-217 fallback: an engine without `report-builder-preview-v1` gets the
/// screen run, and the preview cuts it on screen after 50 transactions in
/// display order; a preview run (the engine's own cut) shows every row.
#[test]
fn the_preview_cut_is_on_screen_without_the_engine_preview() {
    use cl_core::report::{ReportMeta, ReportRow, ReportTable, RowKind as K, RowPath};
    let row = |kind| ReportRow {
        kind,
        level: 0,
        path: RowPath(vec!["n:s1".into()]),
        caption: String::new(),
        cells: Vec::new(),
        drill: None,
        expandable: false,
    };
    let mut rows = vec![row(K::Group)];
    rows.extend((0..60).map(|_| row(K::Detail)));
    rows.push(row(K::More));
    rows.push(row(K::Total));
    let table = ReportTable {
        columns: Vec::new(),
        rows,
        meta: ReportMeta::default(),
    };
    let (order, cut) = super::preview::shown(&table, false);
    assert!(cut);
    let details = order
        .iter()
        .filter(|&&i| table.rows[i].kind == K::Detail)
        .count();
    assert_eq!(details, super::preview::PREVIEW_DETAIL_ROWS);
    assert!(order.iter().all(|&i| table.rows[i].kind != K::More));
    assert_eq!(order.last().map(|&i| table.rows[i].kind), Some(K::Total));
    // A preview run (the engine's cut per line) is cut the same way on
    // screen: at most 50 transactions in all (R4).
    let (engine, cut) = super::preview::shown(&table, true);
    assert!(cut, "the line's more row says the engine cut it");
    assert_eq!(engine, order);
    // Two lines of 30 from the engine: 50 shown in display order.
    let mut rows = vec![row(K::Group)];
    rows.extend((0..30).map(|_| row(K::Detail)));
    rows.push(row(K::Subtotal));
    rows.extend((0..30).map(|_| row(K::Detail)));
    rows.push(row(K::Total));
    let two = ReportTable {
        columns: Vec::new(),
        rows,
        meta: ReportMeta::default(),
    };
    let (order, cut) = super::preview::shown(&two, true);
    assert!(cut);
    let details = order
        .iter()
        .filter(|&&i| two.rows[i].kind == K::Detail)
        .count();
    assert_eq!(details, super::preview::PREVIEW_DETAIL_ROWS);
    assert_eq!(order.last().map(|&i| two.rows[i].kind), Some(K::Total));
}

/// The widget with the keyboard, by its accessible role (tests: AC6).
fn focused_role(ctx: &egui::Context) -> bool {
    ctx.memory(egui::Memory::focused).is_some()
}

/// AC6 (WP-B5): a new editor takes the keyboard (Guided's question); Next
/// moves it into the next step; Blank report in Advanced hands it to the
/// outline (the Add menu on an empty report) on the next frame.
#[test]
fn the_editor_takes_the_keyboard_and_keeps_it_across_modes() {
    let ctx = themed();
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let mut s = BuilderScreen::new(Arc::clone(&link), None, today());
    frame(&ctx, &mut s, 0.0);
    assert!(focused_role(&ctx), "Guided's question has the keyboard");
    ctx.memory_mut(|m| {
        if let Some(id) = m.focused() {
            m.surrender_focus(id);
        }
    });
    if let Some(g) = s.guided.as_mut() {
        g.set_source(cl_model::report_def::SourceKey::ArOpen);
    }
    assert_eq!(
        press(&mut s, Mods::ALT, Key::Letter('N')),
        Some("builder.guided_next")
    );
    frame(&ctx, &mut s, 0.1);
    assert!(focused_role(&ctx), "the next step's question has it");
    // Open in Advanced: the outline takes it on the next frame.
    s.open_in_advanced();
    frame(&ctx, &mut s, 0.2);
    frame(&ctx, &mut s, 0.3);
    assert!(s.outline_focused, "the outline row has the keyboard");
}

// ---- Fix pass (review R2, R3, R5, R7) ----

/// The result tab of the editor's Run report, as the shell would open it.
fn result_tab(link: &Arc<CompanyLink>, s: &mut BuilderScreen) -> super::super::ReportWindow {
    let _ = link.drain();
    s.run_report();
    let payload = run_payload(link);
    let spec = cl_model::reports::find("profit_loss").expect("p&l");
    let mut w = super::super::ReportWindow::new(
        Arc::clone(link),
        CommandId::new(super::super::KIND),
        None,
        spec,
        today(),
    );
    w.poll();
    assert!(w.open_with(payload));
    w.poll();
    w
}

/// The editor seed a window's Customize pushed.
fn customize_payload(link: &CompanyLink) -> OpenPayload {
    link.drain()
        .into_iter()
        .rev()
        .find_map(|e| match e {
            CompanyEvent::OpenWith(_, p) if matches!(*p, OpenPayload::BuilderEditor(_)) => Some(*p),
            _ => None,
        })
        .expect("Customize opened the editor")
}

/// R2: Customize from the editor's own result tab brings the editor back
/// as it is: later edits, undo and the saved row all stay.
#[test]
fn customize_from_its_own_result_tab_keeps_the_editor() {
    let (_d, link, mut s) = editor(BuilderSeed::Blank);
    let ctx = themed();
    gross_profit(&mut s);
    memorize_as(&ctx, &mut s, "Mine");
    let saved = s.ed.saved().cloned().expect("memorized");
    let mut w = result_tab(&link, &mut s);
    // Edits after Run report.
    rename_first(&mut s, "Sales");
    let def = s.ed.definition().clone();
    assert!(s.ed.dirty() && s.ed.can_undo());
    super::super::builder_run::customize(&mut w);
    let payload = customize_payload(&link);
    assert!(s.takes_in_place(&payload), "the same editor takes it");
    assert!(s.open_with(payload));
    assert_eq!(s.ed.definition(), &def, "the later edits stay");
    assert_eq!(
        s.ed.saved().map(|x| (&x.id, x.version)),
        Some((&saved.id, saved.version))
    );
    assert!(s.ed.dirty() && s.ed.can_undo());
    // Saving now updates its own row: no "name taken".
    assert!(s.command(CommandId::new("builder.save_changes")));
    frames(&ctx, &mut s);
    assert!(!s.ed.dirty(), "saved: {:?}", s.status);
    assert_eq!(s.ed.saved().map(|x| x.version), Some(saved.version + 1));
}

/// R2: a tab memorized as a new report is that report: its Customize opens
/// a new editor, never the one that ran it.
#[test]
fn a_result_tab_memorized_as_a_new_report_opens_a_new_editor() {
    let (_d, _l, mut s) = editor(BuilderSeed::Blank);
    gross_profit(&mut s);
    let other = Saved {
        id: "someone-else".into(),
        version: 1,
        kind: SavedKind::Builder,
        can_edit: true,
        def: s.ed.definition().clone(),
        name: "Other".into(),
        group_id: None,
        author: None,
        updated_at: None,
    };
    let seed = BuilderSeed::Customize(Box::new(CustomizeSeed {
        def: s.ed.definition().clone(),
        title: "Other".into(),
        origin: Origin::Memorized,
        saved: Some(other),
        dates: None,
        basis: None,
        adjusting: None,
        filters_dropped: false,
        editor: Some(s.id),
    }));
    assert!(!s.takes_in_place(&OpenPayload::BuilderEditor(Box::new(seed))));
}

/// R3: the saved baseline is what was sent; an edit made while the save
/// was under way stays a change.
#[test]
fn an_edit_made_while_saving_stays_unsaved() {
    let (_d, _l, mut s) = editor(BuilderSeed::Blank);
    let ctx = themed();
    gross_profit(&mut s);
    super::save::open_memorize(&mut s, false);
    if let Some(d) = s.save.memorize.as_mut() {
        d.name = "Mine".into();
    }
    super::save::memorize_for_test(&mut s);
    assert_eq!(s.save.state_name(), "saving");
    // The answer is read at the next frame; the owner types first.
    rename_first(&mut s, "Sales");
    frames(&ctx, &mut s);
    let saved = s.ed.saved().cloned().expect("memorized");
    assert_eq!(saved.name, "Mine");
    assert_eq!(saved.def.title.as_deref(), Some("Mine"), "Q-A08r");
    assert!(Screen::dirty(&s), "the rename is still a change");
    assert!(s.ed.can_save_changes());
}

/// R5: pressing Memorize again for the same write sends the same key; a
/// new name is a new action; a success forgets the key.
#[test]
fn a_memorize_retry_sends_the_same_key() {
    let (_d, link, mut first) = editor(BuilderSeed::Blank);
    let ctx = themed();
    gross_profit(&mut first);
    memorize_as(&ctx, &mut first, "Taken");
    assert_eq!(super::save::memo_key_for_test(&first), None, "done");
    let mut s = BuilderScreen::new(Arc::clone(&link), None, today());
    assert!(s.open_with(OpenPayload::BuilderEditor(Box::new(BuilderSeed::Blank))));
    gross_profit(&mut s);
    memorize_as(&ctx, &mut s, "Taken");
    let k1 = super::save::memo_key_for_test(&s).expect("kept after a refusal");
    super::save::memorize_for_test(&mut s);
    frames(&ctx, &mut s);
    assert_eq!(
        super::save::memo_key_for_test(&s),
        Some(k1.clone()),
        "same write, same key"
    );
    if let Some(d) = s.save.memorize.as_mut() {
        d.name = "Free".into();
    }
    super::save::memorize_for_test(&mut s);
    frames(&ctx, &mut s);
    assert!(s.ed.saved().is_some(), "memorized under the new name");
    assert_eq!(super::save::memo_key_for_test(&s), None);
}

/// R7: Customize on a memorized standard report carries the window's
/// dates and basis into the report and its baseline alike: untouched, it
/// has nothing to save.
#[test]
fn an_untouched_customize_of_a_memorized_standard_report_is_clean() {
    let (_d, _l, base) = editor(BuilderSeed::Blank);
    let mut def = base.ed.definition().clone();
    def.nodes = {
        let (_d2, _l2, mut g) = editor(BuilderSeed::Blank);
        gross_profit(&mut g);
        g.ed.definition().nodes.clone()
    };
    def.title = Some("Profit and Loss".into());
    let saved = Saved {
        id: "std1".into(),
        version: 1,
        kind: SavedKind::Standard,
        can_edit: true,
        def: def.clone(),
        name: "My P&L".into(),
        group_id: None,
        author: None,
        updated_at: None,
    };
    let seed = BuilderSeed::Customize(Box::new(CustomizeSeed {
        def,
        title: "My P&L".into(),
        origin: Origin::Standard("profit_loss".into()),
        saved: Some(saved),
        dates: Some(cl_model::reports::DateChoice::Preset(
            cl_model::reports::DatePreset::LastQuarter,
        )),
        basis: Some(cl_core::report::Basis::Cash),
        adjusting: None,
        filters_dropped: false,
        editor: None,
    }));
    let (_d, _l, s) = editor(seed);
    assert_eq!(
        s.ed.definition().dates.preset.as_deref(),
        Some("last_quarter")
    );
    assert_eq!(s.ed.definition().title.as_deref(), Some("My P&L"));
    assert!(!Screen::dirty(&s), "nothing to save on close");
    assert!(
        !s.ed.can_save_changes(),
        "a standard row's kind can't change"
    );
}

/// H1: the panes reflow at narrow widths and at 200% zoom (README rule 6):
/// side by side, the preview below, or all stacked; Guided moves its
/// preview below the steps.
#[test]
fn the_panes_reflow_on_a_narrow_window() {
    let (_d, _l, mut s) = editor(BuilderSeed::Blank);
    gross_profit(&mut s);
    let ctx = themed();
    for (w, h, want) in [
        (1400.0, 900.0, "wide"),
        (760.0, 640.0, "preview_below"),
        (480.0, 320.0, "stacked"),
    ] {
        for _ in 0..2 {
            let mut out = ctx.run_ui(
                egui::RawInput {
                    time: Some(1.0),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(w, h),
                    )),
                    ..Default::default()
                },
                |ui| {
                    s.ui(ui);
                },
            );
            out.textures_delta.clear();
        }
        assert_eq!(s.state()["layout"], want, "{w}x{h}");
    }
    let (_d, _l, mut g) = editor(BuilderSeed::Guided);
    let mut out = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(480.0, 320.0),
            )),
            ..Default::default()
        },
        |ui| g.ui(ui),
    );
    out.textures_delta.clear();
    assert_eq!(g.state()["layout"], "stacked");
}

/// Not covered by the review: Guided Done → Memorize → close asks nothing;
/// a later answer is a change Save changes keeps on the same report (no
/// "name taken" on its own name).
#[test]
fn guided_done_memorize_then_close_is_clean_and_saves_in_place() {
    let (_d, _l, mut s) = editor(BuilderSeed::Guided);
    let ctx = themed();
    if let Some(g) = s.guided.as_mut() {
        g.set_source(cl_model::report_def::SourceKey::ArOpen);
    }
    s.guided_changed();
    for _ in 0..4 {
        s.command(CommandId::new("builder.guided_next"));
    }
    assert!(Screen::dirty(&s), "a guided report not yet saved");
    memorize_as(&ctx, &mut s, "Who owes me");
    assert!(s.ed.saved().is_some(), "{:?}", s.save.memorize);
    assert!(!Screen::dirty(&s), "closing asks nothing");
    let id = s.ed.saved().map(|x| x.id.clone());
    if let Some(g) = s.guided.as_mut() {
        g.name = "Who owes me now".into();
    }
    s.guided_changed();
    assert!(Screen::dirty(&s) && s.ed.can_save_changes());
    assert!(!Screen::save(&mut s));
    frames(&ctx, &mut s);
    assert!(!Screen::dirty(&s), "saved in place: {:?}", s.status);
    assert_eq!(s.ed.saved().map(|x| x.id.clone()), id, "the same report");
}
/// Plan §3.4 (review "not covered"): an underlined amount in the preview
/// drills as in A07: a subtotal's detail tab under the preview's run.
#[test]
fn a_preview_amount_drills_to_its_detail_tab() {
    let (_d, link, mut s) = editor(BuilderSeed::Blank);
    let ctx = themed();
    gross_profit(&mut s);
    settle(&ctx, &mut s, 1.0);
    let table = s.preview.table.clone().expect("a preview");
    let d = table
        .rows
        .iter()
        .find_map(|r| match &r.drill {
            Some(d @ cl_core::report::Drill::BuilderDetail { .. }) => Some(d.clone()),
            _ => None,
        })
        .expect("a line with its transactions behind it");
    let _ = link.drain();
    super::preview::drill(&mut s, &d);
    let opened = link.drain().into_iter().any(|e| {
        matches!(e, CompanyEvent::OpenWith(_, p) if matches!(*p, OpenPayload::BuilderDetail(_)))
    });
    assert!(opened, "the detail tab opened");
    assert_eq!(s.preview.drill_note, None);
}

/// Step 5 (contrast): every outline badge letter is at least 4.5:1 on its
/// fill in light and dark.
#[test]
fn badge_letters_meet_contrast_in_both_themes() {
    for theme in [
        crate::theme::ThemeChoice::Light,
        crate::theme::ThemeChoice::Dark,
    ] {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, theme);
        let tokens = crate::theme::Tokens::of(&ctx);
        for kind in RowKind::ALL {
            let (fill, ink, _) = super::outline::badge_colors(kind, &tokens);
            let c = crate::theme::contrast(ink, fill);
            assert!(c >= 4.5, "{theme:?} {kind:?}: {c}");
        }
    }
}
