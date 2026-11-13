//! Problems in owner language (ADD-03 A08, plan §3.1): any `{path, code}`
//! (the client rules' issues, or the engine's `DEFINITION_INVALID` from a
//! preview run or a save) mapped to the outline row, tab and field it is
//! about, for the "Problems to fix" callouts and the row's warning tint.
//!
//! The UI words each problem with the i18n key [`ProblemCode::key`]
//! (`builder.problem.<code>`), the row's caption filled in. No egui, no
//! I/O.

use super::outline;
use super::schema::BuilderDefinition;
use super::validate::{Code, Issue, issues};

/// What a problem is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProblemCode {
    /// A rule the desktop checks too (the engine's codes).
    Rule(Code),
    /// A filter names an account or name no longer in this company (engine
    /// only).
    NotInCompany,
    /// A measure the engine refuses on cash basis (engine only).
    NotOnCash,
    /// A code this build doesn't know (a newer engine).
    Other,
}

impl ProblemCode {
    /// The engine's code named `s`.
    #[must_use]
    pub fn from_engine(s: &str) -> ProblemCode {
        match s {
            "NOT_IN_COMPANY" => ProblemCode::NotInCompany,
            "NOT_ON_CASH" => ProblemCode::NotOnCash,
            _ => Code::parse(s).map_or(ProblemCode::Other, ProblemCode::Rule),
        }
    }

    /// The i18n key of its sentence.
    #[must_use]
    pub fn key(self) -> String {
        let code = match self {
            ProblemCode::Rule(c) => c.as_str(),
            ProblemCode::NotInCompany => "NOT_IN_COMPANY",
            ProblemCode::NotOnCash => "NOT_ON_CASH",
            ProblemCode::Other => "OTHER",
        };
        format!("builder.problem.{}", code.to_ascii_lowercase())
    }
}

/// Where in the editor a problem shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// The Row tab of the problem's row; `field` is the path below the row
    /// (`filters.0.op`, `caption`; empty for the row itself).
    Row(String),
    /// The Columns tab; `field` is the path below `columns`
    /// (`percentOf`, `headers.total`).
    Columns(String),
    /// The Report tab or the settings band (`title`, `dates.preset`,
    /// `nodes` for the row cap).
    Report(String),
}

/// One problem to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The outline row it is about ("Go to row"), if any.
    pub node_id: Option<String>,
    /// Where it shows.
    pub anchor: Anchor,
    /// What it is.
    pub code: ProblemCode,
}

/// Maps `{path, code}` onto `def`: the row is the one whose walk path is
/// the longest prefix of `path` (at a `.` boundary).
#[must_use]
pub fn map(def: &BuilderDefinition, path: &str, code: ProblemCode) -> Problem {
    let rest = path.strip_prefix("definition").unwrap_or(path);
    let rest = rest.strip_prefix('.').unwrap_or(rest);
    if let Some(field) = rest.strip_prefix("columns") {
        return Problem {
            node_id: None,
            anchor: Anchor::Columns(field.trim_start_matches('.').to_string()),
            code,
        };
    }
    let row = outline::walk(&def.nodes)
        .into_iter()
        .filter(|v| {
            path == v.path
                || path
                    .strip_prefix(v.path.as_str())
                    .is_some_and(|r| r.starts_with('.'))
        })
        .max_by_key(|v| v.path.len());
    match row {
        Some(v) => Problem {
            node_id: Some(v.node.id().to_string()),
            anchor: Anchor::Row(path[v.path.len()..].trim_start_matches('.').to_string()),
            code,
        },
        None => Problem {
            node_id: None,
            anchor: Anchor::Report(rest.to_string()),
            code,
        },
    }
}

/// The client rules' issues as problems, in the rules' order, one per
/// `{path, code}`.
#[must_use]
pub fn from_issues(def: &BuilderDefinition, list: &[Issue]) -> Vec<Problem> {
    let mut out: Vec<Problem> = Vec::new();
    for i in list {
        let p = map(def, &i.path, ProblemCode::Rule(i.code));
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Every client-side problem in `def`.
#[must_use]
pub fn of(def: &BuilderDefinition) -> Vec<Problem> {
    from_issues(def, &issues(def))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(nodes: &serde_json::Value) -> BuilderDefinition {
        serde_json::from_value(serde_json::json!({
            "schemaVersion": 1, "kind": "builder", "dates": {"mode": "range"},
            "columns": {"detailFields": [], "compare": [], "headers": {}},
            "nodes": nodes,
        }))
        .unwrap()
    }

    #[test]
    fn a_path_lands_on_the_deepest_row_it_names() {
        let d = def(&serde_json::json!([
            {"type": "group", "id": "g", "caption": "G", "showTotal": false, "children": [
                {"type": "blank", "id": "b"},
                {"type": "formula", "id": "f", "caption": "F", "terms": []}
            ]},
            {"type": "blank", "id": "b1"},
            {"type": "blank", "id": "b10"}
        ]));
        let p = map(
            &d,
            "definition.nodes.0.children.1.terms.0.nodeId",
            ProblemCode::Rule(Code::FormulaForwardRef),
        );
        assert_eq!(p.node_id.as_deref(), Some("f"));
        assert_eq!(p.anchor, Anchor::Row("terms.0.nodeId".into()));
        // Prefixes end at a `.`: `nodes.1` is not a prefix of `nodes.10`.
        let p = map(
            &d,
            "definition.nodes.2.id",
            ProblemCode::Rule(Code::DuplicateId),
        );
        assert_eq!(p.node_id.as_deref(), Some("b10"));
        let p = map(&d, "definition.nodes.0", ProblemCode::Rule(Code::TooDeep));
        assert_eq!(
            (p.node_id.as_deref(), p.anchor),
            (Some("g"), Anchor::Row(String::new()))
        );
        let p = map(
            &d,
            "definition.columns.percentOf",
            ProblemCode::Rule(Code::PercentBase),
        );
        assert_eq!(
            (p.node_id, p.anchor),
            (None, Anchor::Columns("percentOf".into()))
        );
        let p = map(
            &d,
            "definition.nodes",
            ProblemCode::Rule(Code::TooManyNodes),
        );
        assert_eq!(
            (p.node_id, p.anchor),
            (None, Anchor::Report("nodes".into()))
        );
        let p = map(&d, "definition.title", ProblemCode::Rule(Code::UuidInText));
        assert_eq!(p.anchor, Anchor::Report("title".into()));
        // A path past the end (a row removed since) is about the report.
        let p = map(
            &d,
            "definition.nodes.7.filters.0",
            ProblemCode::NotInCompany,
        );
        assert_eq!(p.node_id, None);
    }

    #[test]
    fn engine_codes_have_their_own_sentences() {
        assert_eq!(
            ProblemCode::from_engine("NOT_IN_COMPANY").key(),
            "builder.problem.not_in_company"
        );
        assert_eq!(
            ProblemCode::from_engine("NOT_ON_CASH"),
            ProblemCode::NotOnCash
        );
        assert_eq!(
            ProblemCode::from_engine("FORMULA_UNITS"),
            ProblemCode::Rule(Code::FormulaUnits)
        );
        assert_eq!(
            ProblemCode::from_engine("SOMETHING_NEW"),
            ProblemCode::Other
        );
        assert_eq!(
            ProblemCode::Rule(Code::MixedPolarity).key(),
            "builder.problem.mixed_polarity"
        );
    }

    #[test]
    fn every_code_has_a_distinct_key() {
        let mut keys: Vec<String> = Code::ALL
            .iter()
            .map(|c| ProblemCode::Rule(*c).key())
            .collect();
        keys.extend(
            [
                ProblemCode::NotInCompany,
                ProblemCode::NotOnCash,
                ProblemCode::Other,
            ]
            .map(ProblemCode::key),
        );
        let n = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), n);
    }
}
