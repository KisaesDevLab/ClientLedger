//! Every committed scenario file must parse, so a typo is caught by
//! `cargo xtask test` and not only by `ui-check`.

use std::path::Path;

use cl_automation::scenario::Scenario;

#[test]
fn every_scenario_file_parses() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios");
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "yaml") {
            let s = Scenario::load(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(
                path.file_stem().unwrap().to_str().unwrap(),
                s.name,
                "the scenario name must match the file name"
            );
            count += 1;
        }
    }
    assert!(count >= 1, "no scenarios found");
}

#[test]
fn the_shell_scenario_covers_minimum_window_and_200_percent_zoom() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios");
    let text = std::fs::read_to_string(dir.join("shell.yaml")).unwrap();
    assert!(text.contains("percent: 200"));
    assert!(text.contains("width: 960, height: 640"));
    assert!(text.contains("assert_no_clipping"));
}
