//! A08: every engine `DEFINITION_INVALID {path, code}` in the shared cases
//! lands on the row (or the Columns or Report tab) it names, in owner
//! language (`report_def::problems`). PENDING without the engine's shared
//! fixtures, as `report_def_fixtures.rs`.

use std::path::PathBuf;

use cl_model::report_def::BuilderDefinition;
use cl_model::report_def::outline::walk;
use cl_model::report_def::problems::{Anchor, ProblemCode, map};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../engine/packages/shared/src/report-builder/fixtures")
}

#[test]
fn engine_paths_map_to_their_rows() {
    let dir = fixtures();
    let Ok(text) = std::fs::read_to_string(dir.join("cases.json")) else {
        println!("PENDING (PR-A report-defs-builder-v1): no shared fixtures yet");
        return;
    };
    let cases: Vec<serde_json::Value> = serde_json::from_str(&text).expect("cases.json");
    let mut mapped = 0;
    for case in &cases {
        let (Some(path), Some(code)) = (
            case["engine"]["path"].as_str(),
            case["engine"]["code"].as_str(),
        ) else {
            continue;
        };
        let file = case["file"].as_str().expect("file");
        let body = std::fs::read_to_string(dir.join(file)).expect(file);
        // Only bodies the editor could hold (serde's shape); the strict
        // parse refuses most of these by design.
        let Ok(def) = serde_json::from_str::<BuilderDefinition>(&body) else {
            continue;
        };
        let code = ProblemCode::from_engine(code);
        assert_ne!(code, ProblemCode::Other, "{file}");
        let p = map(&def, path, code);
        if path.starts_with("definition.columns") {
            assert!(matches!(p.anchor, Anchor::Columns(_)), "{file}");
        } else if path.starts_with("definition.nodes.") {
            let id = p.node_id.as_deref().expect(file);
            let row = walk(&def.nodes)
                .into_iter()
                // A repeated id (DUPLICATE_ID): the row at the path.
                .find(|v| v.node.id() == id && path.starts_with(&v.path))
                .expect(file);
            let Anchor::Row(field) = &p.anchor else {
                panic!("{file}: {:?}", p.anchor)
            };
            assert_eq!(
                path,
                if field.is_empty() {
                    row.path.clone()
                } else {
                    format!("{}.{field}", row.path)
                },
                "{file}"
            );
        } else {
            assert!(matches!(p.anchor, Anchor::Report(_)), "{file}");
        }
        mapped += 1;
    }
    assert!(mapped >= 20, "only {mapped} cases mapped");
}
