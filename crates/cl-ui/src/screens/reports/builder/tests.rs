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
