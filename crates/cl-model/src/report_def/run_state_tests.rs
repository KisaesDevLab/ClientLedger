//! One test per transition of the plan's §4 state machine (R-T15), with
//! the engine's open rule (PR-C, Q-A07r): `open` and `collapsed` lists,
//! `expandAll`, and `defaultExpanded` sections open unless collapsed.

use super::*;

fn p(parts: &[&str]) -> RowPath {
    RowPath::of(parts.iter().copied())
}

/// The first run (no lists) answered with one default-open line.
fn shown() -> BuilderRunState {
    let mut s = BuilderRunState::new();
    let fx = s.refresh();
    assert_eq!(
        fx,
        [Effect::Run {
            generation: 1,
            open: Vec::new(),
            collapsed: Vec::new(),
            expand_all: false,
        }]
    );
    s.run_ok(1, 7, &[p(&["n:a", "account:1"])]);
    s
}

type Sent = (u64, Vec<RowPath>, Vec<RowPath>, bool);

fn run_of(fx: &[Effect]) -> Option<Sent> {
    fx.iter().find_map(|e| match e {
        Effect::Run {
            generation,
            open,
            collapsed,
            expand_all,
        } => Some((*generation, open.clone(), collapsed.clone(), *expand_all)),
        _ => None,
    })
}

#[test]
fn the_first_run_sends_empty_lists_and_keeps_the_engines_defaults() {
    let s = shown();
    assert_eq!(s.phase(), Phase::Shown);
    assert!(s.is_open(&p(&["n:a", "account:1"])));
    assert_eq!(s.phase().name(), "shown");
    assert_eq!(s.stamp(), Some(7));
}

#[test]
fn expand_is_debounced_then_runs_with_the_paths_opened_by_hand() {
    let mut s = shown();
    assert_eq!(s.expand(&p(&["n:a", "account:2"]), 1_000), []);
    assert_eq!(s.expand(&p(&["n:b", "account:3"]), 1_100), []);
    assert!(s.tick(1_200).is_empty(), "still inside the debounce");
    let fx = s.tick(1_250);
    let (generation, open, collapsed, all) = run_of(&fx).expect("run");
    assert_eq!(generation, 2);
    assert!(!all);
    // The default-open line needs no entry.
    assert_eq!(open, [p(&["n:a", "account:2"]), p(&["n:b", "account:3"])]);
    assert_eq!(collapsed, Vec::<RowPath>::new());
    assert_eq!(s.phase(), Phase::Running);
    let now_open = [
        p(&["n:a", "account:1"]),
        p(&["n:a", "account:2"]),
        p(&["n:b", "account:3"]),
    ];
    s.run_ok(2, 7, &now_open);
    assert_eq!(s.phase(), Phase::Shown);
    assert!(s.is_open(&p(&["n:b", "account:3"])));
}

#[test]
fn collapsing_a_default_open_line_lists_it_as_collapsed() {
    let mut s = shown();
    let line = p(&["n:a", "account:1"]);
    s.collapse(&line, 0);
    let (_, open, collapsed, _) = run_of(&s.tick(DEBOUNCE_MS)).expect("run");
    assert_eq!(open, Vec::<RowPath>::new());
    assert_eq!(collapsed, std::slice::from_ref(&line));
    s.run_ok(2, 7, &[]);
    assert!(!s.is_open(&line));
    // Opening it again moves it back: `open` wins over `collapsed`.
    s.expand(&line, 1_000);
    let (_, open, collapsed, _) = run_of(&s.tick(1_000 + DEBOUNCE_MS)).expect("run");
    assert_eq!(open, [line]);
    assert_eq!(collapsed, Vec::<RowPath>::new());
}

#[test]
fn collapse_hides_rows_at_once_and_cancels_paging_under_it() {
    let mut s = shown();
    let line = p(&["n:a", "account:1"]);
    // A `more` row carries its line's path.
    assert_eq!(s.more("a", &line, Some("c1")).len(), 1);
    assert!(s.is_paging(&line));
    s.collapse(&line, 10);
    assert!(
        !s.is_paging(&line),
        "paging under the closed path is dropped"
    );
    let rows = vec![
        (p(&["n:a"]), false, false),
        (line.clone(), true, false),
        (p(&["n:a", "account:1", "t:1"]), false, false),
        // A detail row and the more row share the line's path.
        (line.clone(), false, true),
        (line.clone(), false, true),
        (p(&["n:a"]), false, false),
    ];
    assert_eq!(s.hidden(&rows), [false, false, true, true, true, false]);
    assert!(
        !s.page_ok(1, &line),
        "a late page under a closed path is dropped"
    );
}

#[test]
fn a_superseded_run_is_cancelled_and_its_answer_dropped() {
    let mut s = shown();
    s.expand(&p(&["n:a", "account:2"]), 0);
    let fx = s.tick(DEBOUNCE_MS);
    assert_eq!(run_of(&fx).expect("run").0, 2);
    s.expand(&p(&["n:a", "account:3"]), 200);
    let fx = s.tick(200 + DEBOUNCE_MS);
    assert!(fx.contains(&Effect::Cancel { generation: 2 }));
    assert_eq!(run_of(&fx).expect("run").0, 3);
    assert!(!s.run_ok(2, 7, &[]));
    assert_eq!(s.phase(), Phase::Running, "the old answer changed nothing");
}

#[test]
fn more_pages_then_replaces_the_more_row() {
    let mut s = shown();
    let line = p(&["n:a", "account:1"]);
    let fx = s.more("a", &line, Some("c1"));
    assert_eq!(
        fx,
        [Effect::Page {
            generation: 1,
            node_id: "a".into(),
            path: line.clone(),
            cursor: Some("c1".into())
        }]
    );
    assert!(
        s.more("a", &line, Some("c1")).is_empty(),
        "one page at a time"
    );
    assert!(s.page_ok(1, &line));
    assert!(!s.page_ok(1, &line), "not twice");
    // A capped path with no cursor pages from the start.
    assert!(matches!(
        s.more("a", &line, None).as_slice(),
        [Effect::Page { cursor: None, .. }]
    ));
}

#[test]
fn a_page_of_an_older_generation_is_dropped() {
    let mut s = shown();
    let line = p(&["n:a", "account:1"]);
    s.more("a", &line, Some("c1"));
    s.refresh();
    assert!(!s.page_ok(1, &line));
}

#[test]
fn run_changed_on_a_page_shows_the_banner_or_reruns() {
    let mut s = shown();
    let line = p(&["n:a", "account:1"]);
    s.more("a", &line, Some("c1"));
    assert_eq!(s.page_failed(1, &line, Failure::BooksChanged), []);
    assert_eq!(s.banner(), Some(Banner::BooksChanged));
    s.more("a", &line, Some("c1"));
    let fx = s.page_failed(1, &line, Failure::ParamsChanged);
    assert_eq!(run_of(&fx).expect("rerun").0, 2);
    s.run_ok(2, 7, &[]);
    s.more("a", &line, Some("c2"));
    s.page_failed(2, &line, Failure::Other);
    assert_eq!(s.banner(), Some(Banner::TryAgain));
    assert_eq!(s.phase(), Phase::Shown, "the rows shown stay");
}

#[test]
fn a_failed_or_slow_run_after_expand_keeps_the_table_and_reverts_open() {
    let mut s = shown();
    let extra = p(&["n:a", "account:2"]);
    s.expand(&extra, 0);
    s.tick(DEBOUNCE_MS);
    s.run_failed(2, Failure::Timeout);
    assert_eq!(s.phase(), Phase::Shown);
    assert_eq!(s.banner(), Some(Banner::TookTooLong));
    assert!(!s.is_open(&extra), "reverted");
    // The lists are reverted too: the next run doesn't carry the path.
    let (_, open, _, _) = run_of(&s.refresh()).expect("run");
    assert_eq!(open, Vec::<RowPath>::new());
    s.run_ok(3, 7, &[p(&["n:a", "account:1"])]);
    s.expand(&extra, 1_000);
    s.tick(1_000 + DEBOUNCE_MS);
    s.run_failed(4, Failure::Other);
    assert_eq!(s.banner(), Some(Banner::TryAgain));
    assert!(!s.is_open(&extra));
}

#[test]
fn a_first_run_that_fails_has_no_table_to_keep() {
    let mut s = BuilderRunState::new();
    s.refresh();
    s.run_failed(1, Failure::Timeout);
    assert_eq!(s.phase(), Phase::TimedOut);
    let mut s = BuilderRunState::new();
    s.refresh();
    s.run_failed(1, Failure::Other);
    assert_eq!(s.phase(), Phase::Failed);
    let mut s = BuilderRunState::new();
    s.refresh();
    s.run_failed(1, Failure::TooLarge);
    assert_eq!(s.phase(), Phase::Unavailable(Cap::TooLarge));
    assert_eq!(s.phase().name(), "too_large");
    let mut s = BuilderRunState::new();
    s.refresh();
    s.run_failed(1, Failure::NeedsEngineUpdate);
    assert_eq!(s.phase().name(), "unavailable");
}

#[test]
fn the_debounce_is_visible_and_every_change_moves_the_revision() {
    let mut s = shown();
    let r0 = s.revision();
    assert_eq!(s.due_at(), None);
    s.expand(&p(&["n:b", "account:9"]), 40);
    assert_eq!(
        s.due_at(),
        Some(40 + DEBOUNCE_MS),
        "the window asks a frame then"
    );
    assert!(s.revision() > r0, "an expand changes the rows shown");
    let r1 = s.revision();
    s.tick(40 + DEBOUNCE_MS);
    assert_eq!(s.due_at(), None);
    s.run_failed(2, Failure::Other);
    assert!(s.revision() > r1, "a rollback changes the rows shown");
}

/// S-M1: a stale memorized version is refused again on every rerun, so the
/// state never reruns by itself; the owner's Refresh does.
#[test]
fn a_stale_definition_waits_for_refresh_and_a_deleted_one_says_so() {
    let mut s = shown();
    s.refresh();
    s.run_failed(2, Failure::StaleDefinition);
    assert_eq!(s.phase(), Phase::Shown);
    assert_eq!(s.banner(), Some(Banner::TryAgain));
    assert_eq!(s.tick(u64::MAX), [], "nothing is sent by itself");
    let mut s = BuilderRunState::new();
    s.refresh();
    s.run_failed(1, Failure::StaleDefinition);
    assert_eq!(s.phase(), Phase::Failed);
    let mut s = shown();
    s.refresh();
    s.run_failed(2, Failure::Deleted);
    assert_eq!(s.phase(), Phase::Deleted);
}

#[test]
fn expand_all_and_collapse_all() {
    let mut s = shown();
    let fx = s.expand_all();
    assert!(run_of(&fx).expect("run").3);
    s.run_ok(2, 7, &[p(&["x"])]);
    assert!(s.is_open(&p(&["anything"])));
    assert_eq!(
        s.hidden(&[(p(&["x"]), true, false), (p(&["x", "y"]), false, false)]),
        [false, false]
    );
    let fx = s.collapse_all();
    let (_, open, collapsed, all) = run_of(&fx).expect("run");
    assert_eq!(open, Vec::<RowPath>::new());
    // Everything shown open is closed, default-open sections included.
    assert_eq!(collapsed, [p(&["x"])]);
    assert!(!all);
}

#[test]
fn a_detail_tab_checks_the_echoed_subtotal_for_equality() {
    let amount = |s: &str| -> Cell {
        serde_json::from_value(serde_json::json!({ "amount": s })).expect("cell")
    };
    let a = vec![amount("125.10")];
    let mut d = DetailPaging::default();
    d.page(Some("c".into()), a.clone());
    assert!(d.has_more());
    d.page(None, a.clone());
    assert!(!d.has_more() && !d.books_changed);
    assert_eq!(d.pages, 2);
    d.page(None, vec![amount("125.11")]);
    assert!(d.books_changed);
    assert_eq!(d.subtotal, Some(a), "the footer stays the first echo");
}

#[test]
fn after_expand_all_collapsing_one_line_keeps_the_others_open() {
    let mut s = shown();
    s.expand_all();
    let all = [
        p(&["n:a", "account:1"]),
        p(&["n:a", "account:2"]),
        p(&["n:b", "account:3"]),
    ];
    s.run_ok(2, 7, &all);
    s.collapse(&all[1], 0);
    assert!(!s.is_open_all());
    assert!(s.is_open(&all[0]) && s.is_open(&all[2]));
    assert!(!s.is_open(&all[1]));
    let fx = s.tick(DEBOUNCE_MS);
    let (_, open, collapsed, expand_all) = run_of(&fx).expect("run");
    // The engine ignores `collapsed` under expandAll, so it is dropped and
    // the others are listed.
    assert!(!expand_all);
    assert_eq!(open, [all[0].clone(), all[2].clone()]);
    assert_eq!(collapsed, [all[1].clone()]);
    s.run_ok(3, 7, &[all[0].clone(), all[2].clone()]);
    assert!(s.is_open(&all[0]) && !s.is_open(&all[1]));
}

#[test]
fn hidden_rows_follow_closed_prefixes_only() {
    let s = shown();
    let rows = vec![
        (p(&["n:a"]), false, false),
        (p(&["n:a", "account:1"]), true, false),
        (p(&["n:a", "account:1", "t:1"]), false, false),
        (p(&["n:a", "account:1"]), false, true),
        (p(&["n:a", "account:2"]), true, false),
        (p(&["n:a", "account:2", "item:x"]), true, false),
        (p(&["n:a", "account:2", "item:x", "t:2"]), false, false),
        (p(&["n:a", "account:22"]), false, false),
    ];
    // account:1 is open (the first run's default), account:2 is not.
    assert_eq!(
        s.hidden(&rows),
        [false, false, false, false, false, true, true, false]
    );
}

#[test]
fn more_than_a_thousand_paths_never_lock_the_report() {
    let mut s = shown();
    s.expand_all();
    let many: Vec<RowPath> = (0..1_001)
        .map(|i| p(&["n:a", &format!("account:{i:05}")]))
        .collect();
    s.run_ok(2, 7, &many);
    // Refresh with everything open sends no lists at all.
    let (_, open, collapsed, all) = run_of(&s.refresh()).expect("run");
    assert!(all);
    assert!(open.is_empty() && collapsed.is_empty());
    s.run_ok(3, 7, &many);
    // Collapsing one leaves 1,000 opened by hand: exactly the cap.
    s.collapse(&many[0], 0);
    let (_, open, collapsed, all) = run_of(&s.tick(DEBOUNCE_MS)).expect("run");
    assert!(!all);
    assert_eq!(open.len(), MAX_OPEN_PATHS);
    assert_eq!(collapsed, [many[0].clone()]);
    assert_eq!(s.banner(), None);
    s.run_ok(4, 7, &many[1..]);
    // Opening two more goes over: the list is cut to the cap and says so.
    s.expand(&many[0], 0);
    s.expand(&p(&["n:b", "account:x"]), 0);
    let (_, open, _, _) = run_of(&s.tick(DEBOUNCE_MS)).expect("run");
    assert_eq!(open.len(), MAX_OPEN_PATHS);
    assert_eq!(s.banner(), Some(Banner::TooManyOpen));
}

#[test]
fn collapse_all_over_a_thousand_default_open_lines_is_cut_and_says_so() {
    let mut s = BuilderRunState::new();
    s.refresh();
    let many: Vec<RowPath> = (0..1_001)
        .map(|i| p(&["n:a", &format!("account:{i:05}")]))
        .collect();
    s.run_ok(1, 7, &many);
    let (_, open, collapsed, _) = run_of(&s.collapse_all()).expect("run");
    assert_eq!(open, Vec::<RowPath>::new());
    assert_eq!(collapsed.len(), MAX_OPEN_PATHS);
    assert_eq!(s.banner(), Some(Banner::TooManyOpen));
}

#[test]
fn a_failed_expand_all_reverts_the_flag_and_too_large_keeps_the_table() {
    let mut s = shown();
    s.expand_all();
    s.run_failed(2, Failure::TooLarge);
    assert_eq!(s.phase(), Phase::Shown, "the table on screen stays");
    assert_eq!(s.banner(), Some(Banner::TooLarge));
    assert!(!s.is_open_all(), "reverted");
    assert!(
        !run_of(&s.refresh()).expect("run").3,
        "Refresh doesn't resend it"
    );
    s.run_ok(3, 7, &[]);
    s.expand_all();
    s.run_ok(4, 7, &[p(&["n:a", "account:1"]), p(&["n:a", "account:2"])]);
    // A collapse after Expand all that fails goes back to Expand all.
    s.collapse(&p(&["n:a", "account:2"]), 0);
    s.tick(DEBOUNCE_MS);
    s.run_failed(5, Failure::Other);
    assert!(s.is_open_all());
    assert!(run_of(&s.refresh()).expect("run").3);
}
