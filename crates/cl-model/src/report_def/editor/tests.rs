#![allow(
    clippy::many_single_char_names,
    reason = "short names for rows in tests"
)]

//! Editor tests (A08 WP-A1): every operation and refusal, undo and redo,
//! dirty, trimming, the choices, and random operation sequences.

use proptest::prelude::*;

use super::choices::{
    amount_filter_value, column_rules, filter_ops, formula_terms, greyed, group_by_choices,
};
use super::*;
use crate::report_def::fields::{self, Availability};
use crate::report_def::problems::Anchor;
use crate::report_def::validate::Code;
use crate::report_def::{CompareBase, CompareSet, Filter, FilterOp, Term, parse};

fn add(e: &mut BuilderEditor, kind: RowKind, caption: &str) -> String {
    e.apply(Op::Add {
        kind,
        caption: caption.into(),
    })
    .expect("added");
    e.selection().expect("selected").to_string()
}

fn section_mut(e: &BuilderEditor, id: &str) -> Section {
    match outline::find(&e.definition().nodes, id) {
        Some(Node::Section(s)) => s.clone(),
        other => panic!("{other:?}"),
    }
}

fn formula_mut(e: &BuilderEditor, id: &str) -> Formula {
    match outline::find(&e.definition().nodes, id) {
        Some(Node::Formula(f)) => f.clone(),
        other => panic!("{other:?}"),
    }
}

/// A P&L with Gross profit: Income, Cost of goods sold, a formula.
fn gross_profit() -> (BuilderEditor, String, String, String) {
    let mut e = BuilderEditor::blank();
    let inc = add(&mut e, RowKind::Section, "Income");
    let mut s = section_mut(&e, &inc);
    s.filters.push(Filter {
        field: "account_type".into(),
        op: FilterOp::In,
        ids: None,
        values: Some(vec!["revenue".into()]),
        value: None,
    });
    e.apply(Op::Edit(Node::Section(s))).unwrap();
    let cogs = add(&mut e, RowKind::Section, "Cost of goods sold");
    let mut s = section_mut(&e, &cogs);
    s.filters.push(Filter {
        field: "account_type".into(),
        op: FilterOp::In,
        ids: None,
        values: Some(vec!["cogs".into()]),
        value: None,
    });
    e.apply(Op::Edit(Node::Section(s))).unwrap();
    let gp = add(&mut e, RowKind::Formula, "Gross profit");
    let mut f = formula_mut(&e, &gp);
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
    e.apply(Op::Edit(Node::Formula(f))).unwrap();
    (e, inc, cogs, gp)
}

#[test]
fn a_pnl_with_gross_profit_from_blank_has_no_problems() {
    let (e, inc, cogs, gp) = gross_profit();
    assert_eq!(
        (inc.as_str(), cogs.as_str(), gp.as_str()),
        ("s1", "s2", "f1")
    );
    assert_eq!(e.client_problems(), &[]);
    assert!(e.can_run());
    assert_eq!(e.row_count(), 3);
    let text = to_text(e.definition()).unwrap();
    assert_eq!(&parse(&text).unwrap(), e.definition());
}

#[test]
fn add_goes_after_the_selection_with_fresh_ids_and_a_caption() {
    let mut e = BuilderEditor::blank();
    assert_eq!(
        e.apply(Op::Add {
            kind: RowKind::Group,
            caption: "   ".into()
        }),
        Err(Refusal::EmptyCaption)
    );
    let g = add(&mut e, RowKind::Group, "  Expenses  ");
    let Some(Node::Group(group)) = outline::find(&e.definition().nodes, &g) else {
        panic!()
    };
    assert_eq!(group.caption, "Expenses", "trimmed");
    // The selected group is open: the new row goes in it.
    let t = add(&mut e, RowKind::Text, "");
    assert_eq!(outline::locate(&e.definition().nodes, &t), Some(vec![0, 0]));
    // A folded group: after it.
    e.select(Some(&g));
    e.set_folded(&g, true);
    let b = add(&mut e, RowKind::Blank, "");
    let p = add(&mut e, RowKind::PageBreak, "");
    assert_eq!(outline::locate(&e.definition().nodes, &b), Some(vec![1]));
    assert_eq!(outline::locate(&e.definition().nodes, &p), Some(vec![2]));
    for kind in RowKind::ALL {
        let id = add(&mut e, kind, "Row");
        assert!(crate::report_def::validate::valid_node_id(&id));
        assert_ne!(id, "section");
    }
}

#[test]
fn two_hundred_rows_is_the_cap() {
    let mut e = BuilderEditor::blank();
    for _ in 0..200 {
        add(&mut e, RowKind::Blank, "");
    }
    assert!(!e.can_add());
    assert_eq!(
        e.apply(Op::Add {
            kind: RowKind::Blank,
            caption: String::new()
        }),
        Err(Refusal::TooManyRows)
    );
    assert_eq!(e.row_count(), 200);
}

#[test]
fn groups_nest_eight_deep() {
    let mut e = BuilderEditor::blank();
    for d in 1..=8 {
        add(&mut e, RowKind::Group, &format!("Level {d}"));
    }
    assert_eq!(
        e.apply(Op::Add {
            kind: RowKind::Blank,
            caption: String::new()
        }),
        Err(Refusal::TooDeep),
        "into the open group at depth 8"
    );
    let deepest = e.selection().unwrap().to_string();
    e.set_folded(&deepest, true);
    add(&mut e, RowKind::Blank, "");
}

#[test]
fn indent_and_outdent_follow_the_group_rules() {
    let mut e = BuilderEditor::blank();
    let a = add(&mut e, RowKind::Blank, "");
    assert_eq!(e.apply(Op::Indent(a.clone())), Err(Refusal::NoGroupAbove));
    let g = add(&mut e, RowKind::Group, "G");
    e.set_folded(&g, true);
    let b = add(&mut e, RowKind::Blank, "");
    e.apply(Op::Indent(b.clone())).unwrap();
    assert!(!e.is_folded(&g), "the group opens to show its row");
    assert_eq!(outline::locate(&e.definition().nodes, &b), Some(vec![1, 0]));
    e.apply(Op::Outdent(b.clone())).unwrap();
    assert_eq!(outline::locate(&e.definition().nodes, &b), Some(vec![2]));
    // Nothing to do: no undo step.
    let r = e.apply(Op::Outdent(b.clone())).unwrap();
    assert!(!r.changed);
    assert_eq!(e.apply(Op::Indent("zz".into())), Err(Refusal::NotFound));
    // Moves stop at the edge.
    assert!(!e.apply(Op::MoveUp(a)).unwrap().changed);
    assert!(e.apply(Op::MoveUp(b)).unwrap().changed);
}

#[test]
fn removing_a_row_drops_it_from_formulas_and_percent_of_as_one_step() {
    let (mut e, inc, _cogs, gp) = gross_profit();
    let mut c = e.definition().columns.clone();
    c.percent_of = Some(inc.clone());
    e.apply(Op::Columns(c)).unwrap();
    let r = e.apply(Op::Remove(inc.clone())).unwrap();
    assert_eq!(
        r.removed,
        Some(Removed {
            rows: 1,
            terms: 1,
            percent_cleared: true
        })
    );
    assert_eq!(formula_mut(&e, &gp).terms.len(), 1);
    assert_eq!(e.definition().columns.percent_of, None);
    assert!(e.undo());
    assert_eq!(formula_mut(&e, &gp).terms.len(), 2);
    assert_eq!(e.definition().columns.percent_of, Some(inc));
}

#[test]
fn removing_a_group_removes_its_rows_and_selects_a_neighbour() {
    let mut e = BuilderEditor::blank();
    let g = add(&mut e, RowKind::Group, "G");
    add(&mut e, RowKind::Blank, "");
    add(&mut e, RowKind::Blank, "");
    e.select(Some(&g));
    e.set_folded(&g, true);
    let after = add(&mut e, RowKind::Text, "after");
    let r = e.apply(Op::Remove(g)).unwrap();
    assert_eq!(r.removed.unwrap().rows, 3);
    assert_eq!(e.selection(), Some(after.as_str()));
    e.apply(Op::Remove(after)).unwrap();
    assert_eq!(e.selection(), None);
    assert_eq!(e.apply(Op::Remove("zz".into())), Err(Refusal::NotFound));
}

#[test]
fn rename_trims_and_refuses_an_empty_caption() {
    let mut e = BuilderEditor::blank();
    let s = add(&mut e, RowKind::Section, "Income");
    assert_eq!(
        e.apply(Op::Rename {
            id: s.clone(),
            text: "  ".into()
        }),
        Err(Refusal::EmptyCaption)
    );
    e.apply(Op::Rename {
        id: s.clone(),
        text: " Sales ".into(),
    })
    .unwrap();
    assert_eq!(section_mut(&e, &s).caption, "Sales");
    let t = add(&mut e, RowKind::Text, "Note");
    e.apply(Op::Rename {
        id: t,
        text: "  ".into(),
    })
    .unwrap();
    let b = add(&mut e, RowKind::Blank, "");
    assert_eq!(
        e.apply(Op::Rename {
            id: b,
            text: "x".into()
        }),
        Err(Refusal::NotFound)
    );
}

#[test]
fn long_text_and_uuids_are_problems_counted_in_utf16_units() {
    let mut e = BuilderEditor::blank();
    let s = add(&mut e, RowKind::Section, "Income");
    // 100 astral characters are 200 UTF-16 units: fine.
    let astral = "\u{1F600}".repeat(100);
    e.apply(Op::Rename {
        id: s.clone(),
        text: astral.clone(),
    })
    .unwrap();
    let schema = |e: &BuilderEditor| {
        e.client_problems()
            .iter()
            .any(|p| p.code == ProblemCode::Rule(Code::Schema))
    };
    assert!(!schema(&e));
    e.apply(Op::Rename {
        id: s.clone(),
        text: format!("{astral}a"),
    })
    .unwrap();
    assert!(schema(&e), "201 units");
    let p = &e.client_problems()[0];
    assert_eq!(
        (p.node_id.as_deref(), &p.anchor),
        (Some(s.as_str()), &Anchor::Row("caption".into()))
    );
    assert!(e.row_has_problem(&s) && !e.can_run());
    e.apply(Op::Rename {
        id: s.clone(),
        text: "Sales 123e4567-e89b-12d3-a456-426614174000".into(),
    })
    .unwrap();
    assert!(
        e.client_problems()
            .iter()
            .any(|p| p.code == ProblemCode::Rule(Code::UuidInText))
    );
}

#[test]
fn the_twenty_fifth_transaction_column_is_refused() {
    let mut e = BuilderEditor::blank();
    let mut c = e.definition().columns.clone();
    c.detail_fields = (0..25).map(|i| format!("f{i}")).collect();
    assert_eq!(e.apply(Op::Columns(c)), Err(Refusal::TooManyFields));
    let mut c = e.definition().columns.clone();
    c.headers.insert("total".into(), "  Oct 2026 ".into());
    c.headers.insert("pct".into(), "   ".into());
    e.apply(Op::Columns(c)).unwrap();
    let h = &e.definition().columns.headers;
    assert_eq!(h.get("total").map(String::as_str), Some("Oct 2026"));
    assert!(!h.contains_key("pct"), "an emptied rename goes back");
}

#[test]
fn undo_and_redo_cross_invalid_states_and_clear_the_engine_problem() {
    let (mut e, inc, cogs, gp) = gross_profit();
    // Moving the formula above its terms: a problem, not a refusal.
    e.apply(Op::MoveUp(gp.clone())).unwrap();
    e.apply(Op::MoveUp(gp.clone())).unwrap();
    assert!(
        e.client_problems()
            .iter()
            .all(|p| p.code == ProblemCode::Rule(Code::FormulaForwardRef))
    );
    assert!(!e.can_run());
    assert!(e.undo() && e.undo());
    assert_eq!(e.client_problems(), &[]);
    assert!(e.redo());
    assert_ne!(e.client_problems(), &[]);
    assert!(e.undo());
    // The engine's problem holds until the next edit.
    e.set_engine_problem("definition.nodes.0.filters.0", "NOT_IN_COMPANY");
    assert_eq!(e.problems().len(), 1);
    assert!(e.row_has_problem(&inc) && !e.can_run());
    e.apply(Op::Rename {
        id: cogs,
        text: "COGS".into(),
    })
    .unwrap();
    assert!(e.engine_problem().is_none() && e.can_run());
    // A new change drops redo.
    assert!(e.undo());
    assert!(e.can_redo());
    e.apply(Op::Name("P&L".into())).unwrap();
    assert!(!e.can_redo() && !e.redo());
}

#[test]
fn undo_keeps_a_hundred_steps() {
    let mut e = BuilderEditor::blank();
    for _ in 0..120 {
        add(&mut e, RowKind::Blank, "");
    }
    let mut n = 0;
    while e.undo() {
        n += 1;
    }
    assert_eq!(n, UNDO_DEPTH);
    assert_eq!(e.row_count(), 20);
    assert!(!e.undo());
}

fn saved_from(e: &BuilderEditor, kind: SavedKind, can_edit: bool) -> Saved {
    Saved {
        id: "r1".into(),
        version: 3,
        kind,
        can_edit,
        def: e.definition().clone(),
        name: e.name().into(),
        group_id: None,
        author: None,
        updated_at: None,
    }
}

#[test]
fn dirty_compares_structure_and_save_changes_needs_the_right() {
    let mut e = BuilderEditor::blank();
    assert!(!e.dirty(), "blank and never saved");
    let s = add(&mut e, RowKind::Section, "Income");
    assert!(e.dirty());
    e.apply(Op::Name("Income report".into())).unwrap();
    assert_eq!(e.definition().title.as_deref(), Some("Income report"));
    e.mark_saved(saved_from(&e, SavedKind::Builder, true));
    assert!(!e.dirty() && !e.can_save_changes());
    assert_eq!(e.origin(), &Origin::Memorized);
    e.apply(Op::Rename {
        id: s.clone(),
        text: "Sales".into(),
    })
    .unwrap();
    assert!(e.dirty() && e.can_save_changes());
    // Undo back to the saved state: clean again (structure, not depth).
    e.undo();
    assert!(!e.dirty());
    // Selection and folding are session state.
    e.select(None);
    e.set_folded(&s, true);
    assert!(!e.dirty());
    e.apply(Op::MemorizedGroup(Some("grp".into()))).unwrap();
    assert!(e.dirty());
    e.not_author();
    assert!(
        e.dirty() && !e.can_save_changes(),
        "NOT_AUTHOR keeps the edits"
    );
    let mut std = BuilderEditor::new(
        e.definition().clone(),
        "x",
        Origin::Standard("profit_loss".into()),
        None,
    );
    std.mark_saved(saved_from(&std, SavedKind::Standard, true));
    std.apply(Op::Name("y".into())).unwrap();
    assert!(
        std.dirty() && !std.can_save_changes(),
        "a standard row's kind can't change"
    );
    std.forget_saved();
    assert!(std.saved().is_none() && std.dirty());
}

#[test]
fn load_is_one_undo_step() {
    let (mut e, ..) = gross_profit();
    let before = e.definition().clone();
    e.apply(Op::Load {
        def: Box::new(blank_definition()),
        name: "Theirs".into(),
        group_id: None,
    })
    .unwrap();
    assert_eq!(e.row_count(), 0);
    assert_eq!(e.selection(), None);
    e.undo();
    assert_eq!(e.definition(), &before);
}

#[test]
fn the_outline_moves_folds_and_hides_folded_rows() {
    let mut e = BuilderEditor::blank();
    let g = add(&mut e, RowKind::Group, "G");
    let a = add(&mut e, RowKind::Blank, "");
    e.select(Some(&g));
    e.set_folded(&g, true);
    let b = add(&mut e, RowKind::Blank, "");
    e.set_folded(&g, false);
    let ids =
        |e: &BuilderEditor| -> Vec<String> { e.visible_rows().into_iter().map(|r| r.id).collect() };
    assert_eq!(ids(&e), [g.clone(), a.clone(), b.clone()]);
    e.select(Some(&a));
    e.fold_or_parent();
    assert_eq!(e.selection(), Some(g.as_str()), "Left on a row: its group");
    e.fold_or_parent();
    assert!(e.is_folded(&g));
    assert_eq!(ids(&e), [g.clone(), b.clone()]);
    e.select_step(true);
    assert_eq!(e.selection(), Some(b.as_str()));
    e.select_step(true);
    assert_eq!(e.selection(), Some(b.as_str()), "stays at the end");
    e.select_step(false);
    e.unfold_or_child();
    assert!(!e.is_folded(&g));
    e.unfold_or_child();
    assert_eq!(e.selection(), Some(a.as_str()));
    e.select(Some("nope"));
    assert_eq!(e.selection(), None);
    e.select_step(false);
    assert_eq!(e.selection(), Some(g.as_str()));
}

#[test]
fn formula_terms_offer_earlier_rows_of_one_unit() {
    let (mut e, inc, cogs, gp) = gross_profit();
    let count = add(&mut e, RowKind::Section, "Invoices");
    let mut s = section_mut(&e, &count);
    s.source = SourceKey::Sales;
    s.measure = Measure::Count;
    e.apply(Op::Edit(Node::Section(s))).unwrap();
    let note = add(&mut e, RowKind::Text, "note");
    e.select(Some(&inc));
    let g = add(&mut e, RowKind::Group, "In a group");
    e.select(Some(&g));
    let f2 = add(&mut e, RowKind::Formula, "Inner");
    let terms = formula_terms(e.definition(), &gp);
    let of = |id: &str| {
        terms
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.availability)
            .unwrap()
    };
    assert_eq!(of(&inc), Availability::Available);
    assert_eq!(of(&cogs), Availability::Available);
    assert_eq!(of(&count), Availability::Greyed(greyed::BELOW));
    assert_eq!(of(&note), Availability::Greyed(greyed::NO_FIGURES));
    // The inner formula can't use the group it sits in; a count row
    // above would be another unit for a formula already on dollars.
    let inner = formula_terms(e.definition(), &f2);
    assert_eq!(
        inner.iter().find(|t| t.id == g).unwrap().availability,
        Availability::Greyed(greyed::BELOW)
    );
    assert_eq!(formula_terms(e.definition(), "zz"), Vec::new());
    let mut f = formula_mut(&e, &f2);
    f.terms.push(Term {
        node_id: inc.clone(),
        sign: 1,
    });
    e.apply(Op::Edit(Node::Formula(f))).unwrap();
    e.apply(Op::MoveDown(count.clone())).unwrap();
    let late = formula_terms(e.definition(), &gp);
    assert!(late.iter().any(|t| t.id == count));
}

#[test]
fn column_rules_grey_what_does_not_go_together() {
    let (mut e, ..) = gross_profit();
    let r = column_rules(e.definition());
    assert_eq!(r.aging, Availability::Greyed(greyed::AGING_OPEN_ITEMS));
    assert!(r.periods.is_available() && r.compare.is_available());
    assert!(r.percent_of.is_available());
    let mut c = e.definition().columns.clone();
    c.compare.push(CompareSet {
        base: CompareBase::PriorPeriod,
        amount: true,
        dollar_change: true,
        percent_change: false,
    });
    e.apply(Op::Columns(c)).unwrap();
    assert_eq!(
        column_rules(e.definition()).running_balance,
        Availability::Greyed(greyed::PERIODS_RUNNING)
    );
    // An A/R aging: aging on greys periods and compare.
    let mut g = crate::report_def::guided::GuidedState::new();
    g.set_source(SourceKey::ArOpen);
    g.display = Some(Display::Summary);
    let aging = g.to_definition("Owed").unwrap();
    let r = column_rules(&aging);
    assert!(r.aging.is_available());
    assert_eq!(r.periods, Availability::Greyed(greyed::PERIODS_AGING));
    // A count section: no "% of".
    let mut e2 = BuilderEditor::blank();
    let s = add(&mut e2, RowKind::Section, "Count");
    let mut sec = section_mut(&e2, &s);
    sec.measure = Measure::Count;
    e2.apply(Op::Edit(Node::Section(sec))).unwrap();
    assert_eq!(
        column_rules(e2.definition()).percent_of,
        Availability::Greyed(greyed::PERCENT_MONEY)
    );
}

#[test]
fn group_bys_and_filter_operators_follow_the_source() {
    let open = group_by_choices(SourceKey::ArOpen);
    assert_eq!(open.len(), fields::GROUP_KEYS.len());
    let month = open.iter().find(|(k, _)| *k == "month").unwrap().1;
    assert_eq!(month, Availability::Greyed(greyed::NOT_FOR_SOURCE));
    assert!(
        open.iter()
            .find(|(k, _)| *k == "contact")
            .unwrap()
            .1
            .is_available()
    );
    assert_eq!(filter_ops("amount"), &[FilterOp::Gte, FilterOp::Lte]);
    assert!(filter_ops("contact").contains(&FilterOp::IsNone));
    assert!(!filter_ops("account_type").contains(&FilterOp::IsNone));
}

#[test]
fn the_amount_filter_keeps_the_exact_digits_typed() {
    assert_eq!(amount_filter_value("1234.5").as_deref(), Some("1234.5"));
    assert_eq!(amount_filter_value("1,234.50").as_deref(), Some("1234.50"));
    assert_eq!(amount_filter_value(" $250 ").as_deref(), Some("250"));
    assert_eq!(amount_filter_value("-12.0001").as_deref(), Some("-12.0001"));
    assert_eq!(amount_filter_value("(40)").as_deref(), Some("-40"));
    assert_eq!(amount_filter_value(".5").as_deref(), Some("0.5"));
    for bad in ["1.23456", "1e3", "abc", "", "1,23", "--1", "NaN"] {
        assert_eq!(amount_filter_value(bad), None, "{bad}");
    }
}

// ---- Random operation sequences ----

fn op_strategy() -> impl Strategy<Value = (u8, u8, u8)> {
    (0u8..12, any::<u8>(), any::<u8>())
}

/// An operation from three random bytes, aimed at rows that exist.
fn make_op(e: &BuilderEditor, (what, pick, extra): (u8, u8, u8)) -> Op {
    let ids: Vec<String> = outline::walk(&e.definition().nodes)
        .iter()
        .map(|v| v.node.id().to_string())
        .collect();
    let id = if ids.is_empty() {
        "none".to_string()
    } else {
        ids[usize::from(pick) % ids.len()].clone()
    };
    let kind = RowKind::ALL[usize::from(extra) % RowKind::ALL.len()];
    match what {
        0..=3 => Op::Add {
            kind,
            caption: format!("Row {extra}"),
        },
        4 => Op::Remove(id),
        5 => Op::MoveUp(id),
        6 => Op::MoveDown(id),
        7 => Op::Indent(id),
        8 => Op::Outdent(id),
        9 => Op::Rename {
            id,
            text: format!(" Name {extra} "),
        },
        10 => Op::Name(format!("Report {extra}")),
        _ => {
            let mut c = e.definition().columns.clone();
            c.percent_of = Some(id);
            Op::Columns(c)
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn random_edits_keep_the_tree_sound_and_undo_returns_to_the_start(
        ops in proptest::collection::vec(op_strategy(), 1..60),
        select in proptest::collection::vec(any::<u8>(), 60),
    ) {
        let mut e = BuilderEditor::blank();
        let start = e.definition().clone();
        let mut states = vec![e.definition().clone()];
        for (i, raw) in ops.into_iter().enumerate() {
            // Move the selection around so Add lands in many places.
            let rows = e.visible_rows();
            if !rows.is_empty() {
                let r = &rows[usize::from(select[i % select.len()]) % rows.len()];
                let id = r.id.clone();
                e.select(Some(&id));
            }
            let op = make_op(&e, raw);
            if let Ok(a) = e.apply(op) && a.changed {
                states.push(e.definition().clone());
            }
            let walk = outline::walk(&e.definition().nodes);
            let mut seen = std::collections::HashSet::new();
            prop_assert!(walk.iter().all(|v| seen.insert(v.node.id().to_string())), "ids unique");
            prop_assert!(walk.len() <= limits::NODES);
            prop_assert!(walk.iter().all(|v| v.depth <= limits::DEPTH));
            prop_assert!(walk.iter().all(|v| crate::report_def::validate::valid_node_id(v.node.id())));
            let text = to_text(e.definition()).unwrap();
            let back: BuilderDefinition = serde_json::from_str(&text).unwrap();
            prop_assert_eq!(&back, e.definition());
        }
        // Undo walks back through every state to the start; redo restores.
        let end = e.definition().clone();
        for s in states.iter().rev().skip(1) {
            prop_assert!(e.undo());
            prop_assert_eq!(e.definition(), s);
        }
        prop_assert!(!e.undo());
        prop_assert_eq!(e.definition(), &start);
        while e.redo() {}
        prop_assert_eq!(e.definition(), &end);
    }
}
