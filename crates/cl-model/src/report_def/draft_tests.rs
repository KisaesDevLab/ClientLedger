use serde_json::json;

use super::*;
use crate::report_def::editor::{BuilderEditor, Origin};

const ACCT: &str = "11111111-1111-4111-8111-111111111111";
const ACCT2: &str = "22222222-2222-4222-8222-222222222222";
const GROUP: &str = "33333333-3333-4333-8333-333333333333";

fn section(
    id: &str,
    filters: &serde_json::Value,
    group_by: &serde_json::Value,
) -> serde_json::Value {
    json!({"type": "section", "id": id, "caption": "Rent", "source": "gl",
           "measure": "amount", "filters": filters, "groupBy": group_by,
           "display": "summary", "defaultExpanded": false, "showTotal": true,
           "hideZero": false})
}

/// The engine's example draft: an ambiguous account beside a known one,
/// and a tag grouping whose group wasn't found; a second row in a group
/// holds the same account placeholder.
fn drafted() -> BuilderDefinition {
    serde_json::from_value(json!({
        "schemaVersion": 1, "kind": "builder", "dates": {"mode": "range", "preset": "this_year"},
        "columns": {"detailFields": [], "compare": [], "headers": {}},
        "nodes": [
            section("rowa", &json!([{"field": "account", "op": "in", "ids": ["$ref:0", ACCT]}]),
                    &json!([{"key": "tag", "tagGroupId": "$ref:1"}, {"key": "month"}])),
            {"type": "group", "id": "grp", "caption": "More", "showTotal": false, "children": [
                section("rowb", &json!([{"field": "account", "op": "in", "ids": ["$ref:0"]},
                                        {"field": "contact", "op": "is_none"},
                                        {"field": "account_type", "op": "in", "values": ["expense"]}]), &json!([]))
            ]}
        ]
    }))
    .expect("a schema v1 definition, placeholders and all")
}

fn sec(def: &BuilderDefinition, id: &str) -> crate::report_def::Section {
    match crate::report_def::outline::find(&def.nodes, id) {
        Some(Node::Section(s)) => s.clone(),
        _ => panic!("no section {id}"),
    }
}

#[test]
fn placeholders_are_found_at_any_depth() {
    let d = drafted();
    let p: Vec<String> = placeholders(&d).into_iter().collect();
    assert_eq!(p, vec!["$ref:0".to_string(), "$ref:1".to_string()]);
    assert_eq!(rows_with(&d, "$ref:0"), vec!["rowa", "rowb"]);
    assert_eq!(rows_with(&d, "$ref:1"), vec!["rowa"]);
    assert!(is_placeholder("$ref:12"));
    assert!(!is_placeholder("$ref:"));
    assert!(!is_placeholder(ACCT));
    // The same cap as cl-api's re-check (A10 review S1).
    assert!(is_placeholder("$ref:9999"));
    assert!(!is_placeholder("$ref:10000"));
    assert!(!is_placeholder("$ref:\u{ff11}"));
}

/// Q-A10l: a heading or text row with a number word is refused, unless it
/// is exactly a books name.
#[test]
fn number_words_in_headings_are_found() {
    let none = BTreeSet::new();
    assert!(!states_a_number(&drafted(), &none));
    let set = |f: &dyn Fn(&mut BuilderDefinition)| {
        let mut d = drafted();
        f(&mut d);
        d
    };
    let cases: Vec<BuilderDefinition> = vec![
        set(&|d| d.title = Some("Bills over five hundred dollars".into())),
        set(&|d| {
            if let Node::Section(s) = &mut d.nodes[0] {
                s.caption = "Top ten vendors".into();
            }
        }),
        set(&|d| {
            if let Node::Section(s) = &mut d.nodes[0] {
                s.total_caption = Some("Total over a thousand".into());
            }
        }),
        set(&|d| {
            if let Node::Group(g) = &mut d.nodes[1] {
                g.caption = "Twenty-five more".into();
            }
        }),
        set(&|d| {
            d.columns
                .headers
                .insert("total".into(), "Five hundred and up".into());
        }),
        set(&|d| {
            d.nodes.push(Node::Text(crate::report_def::schema::TextRow {
                id: "note".into(),
                text: "Only bills over two thousand".into(),
            }));
        }),
    ];
    for d in &cases {
        assert!(states_a_number(d, &none), "{d:?}");
    }
    // An ordinary word passes; a books name passes as itself only.
    let one_time = set(&|d| d.title = Some("One-time customers".into()));
    assert!(!states_a_number(&one_time, &none));
    let vendor = set(&|d| {
        if let Node::Section(s) = &mut d.nodes[0] {
            s.caption = "Two Rivers Supply".into();
        }
    });
    let known: BTreeSet<String> = ["Two Rivers Supply".to_string()].into();
    assert!(states_a_number(&vendor, &none));
    assert!(!states_a_number(&vendor, &known));
    let more = set(&|d| {
        if let Node::Section(s) = &mut d.nodes[0] {
            s.caption = "Two Rivers Supply over five hundred".into();
        }
    });
    assert!(states_a_number(&more, &known));
}

/// A definition with a placeholder is never handed out to send.
#[test]
fn a_placeholder_is_never_sendable() {
    let d = drafted();
    assert!(sendable(&d).is_none());
    let d = resolve(&d, "$ref:0", &Answer::Pick(ACCT2.into())).expect("0");
    assert!(sendable(&d).is_none(), "one is left");
    let d = resolve(&d, "$ref:1", &Answer::Pick(GROUP.into())).expect("1");
    assert!(sendable(&d).is_some());
}

#[test]
fn a_pick_replaces_every_use_and_isn_t_listed_twice() {
    let d = resolve(&drafted(), "$ref:0", &Answer::Pick(ACCT.into())).expect("pick");
    // rowa had [$ref:0, ACCT]: picking ACCT leaves one ACCT.
    assert_eq!(sec(&d, "rowa").filters[0].ids, Some(vec![ACCT.to_string()]));
    assert_eq!(sec(&d, "rowb").filters[0].ids, Some(vec![ACCT.to_string()]));
    let d = resolve(&d, "$ref:1", &Answer::Pick(GROUP.into())).expect("pick");
    assert_eq!(
        sec(&d, "rowa").group_by[0].tag_group_id.as_deref(),
        Some(GROUP)
    );
}

/// Remove drops the id; an emptied filter goes; a tag grouping without its
/// group goes; other filters and groupings stay.
#[test]
fn remove_drops_emptied_filters_and_groupless_tag_groupings() {
    let d = resolve(&drafted(), "$ref:0", &Answer::Remove).expect("remove");
    assert_eq!(sec(&d, "rowa").filters[0].ids, Some(vec![ACCT.to_string()]));
    let b = sec(&d, "rowb");
    assert_eq!(b.filters.len(), 2, "the emptied account filter went");
    assert_eq!(b.filters[0].field, "contact");
    let d = resolve(&d, "$ref:1", &Answer::Remove).expect("remove");
    let a = sec(&d, "rowa");
    assert_eq!(a.group_by.len(), 1);
    assert_eq!(a.group_by[0].key, "month");
    assert!(sendable(&d).is_some());
    // The result passes the client rules (no empty list, no tag grouping
    // without its group).
    assert!(
        crate::report_def::check(&d).is_ok(),
        "{:?}",
        crate::report_def::issues(&d)
    );
}

#[test]
fn a_resolve_needs_a_real_token_and_a_real_pick() {
    let d = drafted();
    assert!(resolve(&d, "$ref:9", &Answer::Remove).is_none());
    assert!(resolve(&d, "$ref:0", &Answer::Pick("$ref:1".into())).is_none());
    assert!(resolve(&d, "$ref:0", &Answer::Pick(" ".into())).is_none());
}

/// The open list is read from the definition: an undo in the editor brings
/// a name back.
#[test]
fn what_is_open_follows_the_definition() {
    let draft = Draft {
        names: vec![
            DraftName {
                token: "$ref:0".into(),
                kind: RefKind::Account,
                name: Some("Rent".into()),
                ambiguous: true,
                candidates: vec![(ACCT.into(), "6300 Rent".into())],
            },
            DraftName {
                token: "$ref:1".into(),
                kind: RefKind::TagGroup,
                name: None,
                ambiguous: false,
                candidates: vec![],
            },
        ],
        notes: vec![Note::PickUnresolved],
        provider: Some("demo".into()),
    };
    let d = drafted();
    assert_eq!(draft.open(&d).len(), 2);
    assert!(draft.row_open(&d, "rowb"));
    let d2 = resolve(&d, "$ref:0", &Answer::Remove).expect("r");
    assert_eq!(draft.open(&d2).len(), 1);
    assert!(!draft.row_open(&d2, "rowb"));
    assert!(draft.row_open(&d2, "rowa"));
    assert!(
        !format!("{draft:?}").contains("Rent"),
        "Debug names nothing"
    );
}

/// The editor holds a draft but never offers it to run or save until every
/// name is settled; the placeholder rows carry their own problem.
#[test]
fn the_editor_holds_a_draft_but_won_t_run_or_save_it() {
    let mut ed = BuilderEditor::new(drafted(), "Rent", Origin::Drafted, None);
    assert!(ed.has_placeholders());
    assert!(!ed.can_run());
    assert!(!ed.can_memorize());
    assert!(!ed.can_save_changes());
    assert!(ed.dirty(), "a draft is unsaved");
    let codes: Vec<_> = ed.problems().iter().map(|p| p.code).collect();
    assert!(
        codes
            .iter()
            .all(|c| *c == crate::report_def::problems::ProblemCode::Unresolved),
        "{codes:?}"
    );
    assert!(ed.row_has_problem("rowb"));
    let d = resolve(ed.definition(), "$ref:0", &Answer::Pick(ACCT2.into())).expect("0");
    let d = resolve(&d, "$ref:1", &Answer::Remove).expect("1");
    let _ = ed.apply(crate::report_def::editor::Op::Load {
        def: Box::new(d),
        name: "Rent".into(),
        group_id: None,
    });
    assert!(!ed.has_placeholders());
    assert!(ed.can_run(), "{:?}", ed.problems());
    assert!(ed.can_memorize());
}

#[test]
fn the_describe_box_takes_one_to_a_thousand() {
    let mut s = Describe::default();
    assert!(!s.can_send());
    s.text = "Fuel by month".into();
    assert!(s.can_send());
    s.phase = Phase::Drafting;
    assert!(!s.can_send());
    assert!(s.busy());
    s.phase = Phase::Idle;
    s.text = "x".repeat(MAX_DESCRIPTION_CHARS + 1);
    assert!(!s.can_send());
    assert!(!format!("{s:?}").contains('x'));
}

#[test]
fn every_note_has_a_key() {
    for n in [
        Note::PickUnresolved,
        Note::AmountFilterLeftOut(None),
        Note::AmountFilterLeftOut(Some("rowa".into())),
        Note::PresetLeftOut,
        Note::NumbersNotUsed,
    ] {
        assert!(n.key().starts_with("builder.ai.note."));
    }
}
