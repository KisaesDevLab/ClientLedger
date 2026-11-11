//! One test per transition of the plan's §4 state machine (R-T15).

use super::*;

fn p(parts: &[&str]) -> RowPath {
    RowPath::of(parts.iter().copied())
}

fn shown() -> BuilderRunState {
    let mut s = BuilderRunState::new();
    let fx = s.refresh();
    assert_eq!(
        fx,
        [Effect::Run {
            generation: 1,
            open: None,
            open_all: false
        }]
    );
    s.run_ok(1, "s1", &[p(&["n:a", "account:1"])]);
    s
}

fn run_of(fx: &[Effect]) -> Option<(u64, Option<Vec<RowPath>>, bool)> {
    fx.iter().find_map(|e| match e {
        Effect::Run {
            generation,
            open,
            open_all,
        } => Some((*generation, open.clone(), *open_all)),
        _ => None,
    })
}

#[test]
fn the_first_run_sends_null_and_keeps_the_engines_defaults() {
    let s = shown();
    assert_eq!(s.phase(), Phase::Shown);
    assert!(s.is_open(&p(&["n:a", "account:1"])));
    assert_eq!(s.phase().name(), "shown");
}

#[test]
fn expand_is_debounced_then_runs_with_the_cumulative_open_set() {
    let mut s = shown();
    assert_eq!(s.expand(&p(&["n:a", "account:2"]), 1_000), []);
    assert_eq!(s.expand(&p(&["n:b", "account:3"]), 1_100), []);
    assert!(s.tick(1_200).is_empty(), "still inside the debounce");
    let fx = s.tick(1_250);
    let (generation, open, all) = run_of(&fx).expect("run");
    assert_eq!(generation, 2);
    assert!(!all);
    assert_eq!(
        open.expect("explicit"),
        [
            p(&["n:a", "account:1"]),
            p(&["n:a", "account:2"]),
            p(&["n:b", "account:3"])
        ]
    );
    assert_eq!(s.phase(), Phase::Running);
    s.run_ok(2, "s1", &[]);
    assert_eq!(s.phase(), Phase::Shown);
    assert!(s.is_open(&p(&["n:b", "account:3"])));
}

#[test]
fn collapse_hides_rows_at_once_and_cancels_paging_under_it() {
    let mut s = shown();
    let line = p(&["n:a", "account:1"]);
    let more = p(&["n:a", "account:1", "more"]);
    assert_eq!(s.more("a", &more, "c1").len(), 1);
    assert!(s.is_paging(&more));
    s.collapse(&line, 10);
    assert!(
        !s.is_paging(&more),
        "paging under the closed path is dropped"
    );
    let rows = vec![
        (p(&["n:a"]), false),
        (line.clone(), true),
        (p(&["n:a", "account:1", "t:1"]), false),
        (p(&["n:a", "total"]), false),
    ];
    assert_eq!(s.hidden(&rows), [false, false, true, false]);
    assert!(
        !s.page_ok(1, &more),
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
    assert_eq!(s.run_ok(2, "s1", &[]), []);
    assert_eq!(s.phase(), Phase::Running, "the old answer changed nothing");
}

#[test]
fn more_pages_then_replaces_the_more_row() {
    let mut s = shown();
    let more = p(&["n:a", "account:1", "more"]);
    let fx = s.more("a", &more, "c1");
    assert_eq!(
        fx,
        [Effect::Page {
            generation: 1,
            node_id: "a".into(),
            path: more.clone(),
            cursor: "c1".into()
        }]
    );
    assert!(s.more("a", &more, "c1").is_empty(), "one page at a time");
    assert!(s.page_ok(1, &more));
    assert!(!s.page_ok(1, &more), "not twice");
}

#[test]
fn a_page_of_an_older_generation_is_dropped() {
    let mut s = shown();
    let more = p(&["n:a", "account:1", "more"]);
    s.more("a", &more, "c1");
    s.refresh();
    assert!(!s.page_ok(1, &more));
}

#[test]
fn run_changed_on_a_page_shows_the_banner_or_reruns() {
    let mut s = shown();
    let more = p(&["n:a", "account:1", "more"]);
    s.more("a", &more, "c1");
    assert_eq!(s.page_failed(1, &more, Failure::BooksChanged), []);
    assert_eq!(s.banner(), Some(Banner::BooksChanged));
    s.more("a", &more, "c1");
    let fx = s.page_failed(1, &more, Failure::ParamsChanged);
    assert_eq!(run_of(&fx).expect("rerun").0, 2);
    s.run_ok(2, "s1", &[]);
    s.more("a", &more, "c2");
    s.page_failed(2, &more, Failure::Other);
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
    s.expand(&extra, 1_000);
    s.tick(1_000 + DEBOUNCE_MS);
    s.run_failed(3, Failure::Other);
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
fn a_rerun_on_the_same_stamp_reloads_the_pages_loaded() {
    let mut s = shown();
    let more = p(&["n:a", "account:1", "more"]);
    s.more("a", &more, "c1");
    s.page_ok(1, &more);
    s.more("a", &more, "c2");
    s.page_ok(1, &more);
    s.expand(&p(&["n:b", "account:9"]), 0);
    s.tick(DEBOUNCE_MS);
    let fx = s.run_ok(2, "s1", &[]);
    assert_eq!(
        fx,
        [Effect::Repage {
            generation: 2,
            path: more.clone(),
            pages: 2
        }]
    );
    // New books: the pages start again.
    s.refresh();
    assert_eq!(s.run_ok(3, "s2", &[]), []);
}

#[test]
fn a_stale_definition_reloads_then_runs_and_a_deleted_one_says_so() {
    let mut s = shown();
    s.refresh();
    assert_eq!(
        s.run_failed(2, Failure::StaleDefinition),
        [Effect::ReloadDefinition]
    );
    let fx = s.definition_reloaded();
    assert_eq!(run_of(&fx).expect("run").0, 3);
    s.run_failed(3, Failure::Deleted);
    assert_eq!(s.phase(), Phase::Deleted);
}

#[test]
fn expand_all_and_collapse_all() {
    let mut s = shown();
    let fx = s.expand_all();
    assert!(run_of(&fx).expect("run").2);
    s.run_ok(2, "s1", &[]);
    assert!(s.is_open(&p(&["anything"])));
    assert_eq!(
        s.hidden(&[(p(&["x"]), true), (p(&["x", "y"]), false)]),
        [false, false]
    );
    let fx = s.collapse_all();
    let (_, open, all) = run_of(&fx).expect("run");
    assert_eq!(open, Some(Vec::new()));
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
    s.run_ok(2, "s1", &all);
    s.collapse(&all[1], 0);
    assert!(!s.is_open_all());
    assert!(s.is_open(&all[0]) && s.is_open(&all[2]));
    assert!(!s.is_open(&all[1]));
    let fx = s.tick(DEBOUNCE_MS);
    let (_, open, _) = run_of(&fx).expect("run");
    assert_eq!(open, Some(vec![all[0].clone(), all[2].clone()]));
    s.run_ok(3, "s1", &[]);
    assert!(s.is_open(&all[0]) && !s.is_open(&all[1]));
}

#[test]
fn hidden_rows_follow_closed_prefixes_only() {
    let s = shown();
    let rows = vec![
        (p(&["n:a"]), false),
        (p(&["n:a", "account:1"]), true),
        (p(&["n:a", "account:1", "t:1"]), false),
        (p(&["n:a", "account:2"]), true),
        (p(&["n:a", "account:2", "item:x"]), true),
        (p(&["n:a", "account:2", "item:x", "t:2"]), false),
        (p(&["n:a", "account:22"]), false),
    ];
    // account:1 is open (the first run's default), account:2 is not.
    assert_eq!(
        s.hidden(&rows),
        [false, false, false, false, true, true, false]
    );
}

#[test]
fn more_than_a_thousand_open_paths_never_lock_the_report() {
    let mut s = shown();
    s.expand_all();
    let many: Vec<RowPath> = (0..1_001)
        .map(|i| p(&["n:a", &format!("account:{i:05}")]))
        .collect();
    s.run_ok(2, "s1", &many);
    // Refresh with everything open sends no list at all.
    let fx = s.refresh();
    let (_, open, all) = run_of(&fx).expect("run");
    assert!(all);
    assert_eq!(open, Some(Vec::new()));
    s.run_ok(3, "s1", &many);
    // Collapsing one leaves 1,000: exactly the cap, sent as is.
    s.collapse(&many[0], 0);
    let fx = s.tick(DEBOUNCE_MS);
    let (_, open, all) = run_of(&fx).expect("run");
    assert!(!all);
    assert_eq!(open.expect("list").len(), MAX_OPEN_PATHS);
    assert_eq!(s.banner(), None);
    s.run_ok(4, "s1", &[]);
    // Opening two more goes over: the list is cut to the cap and says so.
    s.expand(&many[0], 0);
    s.expand(&p(&["n:b", "account:x"]), 0);
    let fx = s.tick(DEBOUNCE_MS);
    let (_, open, _) = run_of(&fx).expect("run");
    assert_eq!(open.expect("list").len(), MAX_OPEN_PATHS);
    assert_eq!(s.banner(), Some(Banner::TooManyOpen));
}
