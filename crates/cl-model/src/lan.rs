//! LAN multi-user (P33 D4) logic without egui: the save-conflict field diff
//! and the three-way "Keep mine" merge (plan step 11, review S2 and M5,
//! Q-P33r).
//!
//! A record is compared as its **editable fields** (the same JSON the
//! record's fingerprint is taken over, `cl_api::EditableFields`): header
//! fields, plus one array of lines. Values are compared as canonical JSON
//! text, so amounts are compared as the exact strings the engine sent; no
//! arithmetic happens here and no float is ever built.
//!
//! - **Nothing editable changed on their side** (theirs equals the
//!   original): the form saves again on the new stamp without asking
//!   ([`Conflict::silent_resave`]). Only then; when their side changed a
//!   field, even one this user did not touch, the dialog asks (plan wording,
//!   the narrower rule; see Q-P33aj).
//! - **Use theirs**: the form takes their record.
//! - **Keep mine**: their record, with each header field this user changed
//!   put back, and the lines taken **whole** from one side (mine when this
//!   user changed any line, theirs otherwise), so an entry that balanced on
//!   either side still balances. [`Merge::undone`] names each of their
//!   changes the merge would undo.
//!
//! Lines carry no stable id (the engine recreates line ids on save), so the
//! diff aligns them by a longest common subsequence on their content, and
//! one inserted line does not mark every later line changed (M5).

use std::collections::BTreeSet;

use serde_json::{Map, Value};

/// Canonical JSON text: object keys sorted, no whitespace.
#[must_use]
pub fn canonical(v: &Value) -> String {
    let mut out = String::new();
    write_canonical(v, &mut out);
    out
}

fn write_canonical(v: &Value, out: &mut String) {
    match v {
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                write_canonical(&map[k.as_str()], out);
            }
            out.push('}');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// One record's editable fields, split into header fields and lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// Header fields by wire name (canonical text and the value).
    header: Vec<(String, Value)>,
    /// The lines, in order (`None` when the record has no lines).
    lines: Option<Vec<Value>>,
    /// The key the lines sit under.
    lines_key: String,
}

impl Snapshot {
    /// Splits an editable-fields object. Every key except `lines_key` is a
    /// header field; a missing `lines_key` means the record has no lines.
    /// A value that is not an object is one header field named `value`.
    #[must_use]
    pub fn from_editable(v: &Value, lines_key: &str) -> Snapshot {
        let mut header = Vec::new();
        let mut lines = None;
        match v {
            Value::Object(map) => {
                for (k, val) in map {
                    if k == lines_key {
                        lines = Some(match val {
                            Value::Array(a) => a.clone(),
                            Value::Null => Vec::new(),
                            other => vec![other.clone()],
                        });
                    } else {
                        header.push((k.clone(), val.clone()));
                    }
                }
            }
            other => header.push(("value".to_string(), other.clone())),
        }
        header.sort_by(|a, b| a.0.cmp(&b.0));
        Snapshot {
            header,
            lines,
            lines_key: lines_key.to_string(),
        }
    }

    /// The editable-fields object again.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        for (k, v) in &self.header {
            map.insert(k.clone(), v.clone());
        }
        if let Some(l) = &self.lines {
            map.insert(self.lines_key.clone(), Value::Array(l.clone()));
        }
        Value::Object(map)
    }

    /// A header field (`Null` when absent).
    #[must_use]
    pub fn field(&self, name: &str) -> &Value {
        self.header
            .iter()
            .find(|(k, _)| k == name)
            .map_or(&Value::Null, |(_, v)| v)
    }

    /// The lines (empty when none).
    #[must_use]
    pub fn lines(&self) -> &[Value] {
        self.lines.as_deref().unwrap_or(&[])
    }

    fn line_texts(&self) -> Vec<String> {
        self.lines().iter().map(canonical).collect()
    }

    fn field_names<'a>(&'a self, others: &[&'a Snapshot]) -> BTreeSet<&'a str> {
        let mut names: BTreeSet<&str> = self.header.iter().map(|(k, _)| k.as_str()).collect();
        for o in others {
            names.extend(o.header.iter().map(|(k, _)| k.as_str()));
        }
        names
    }

    fn same_field(&self, other: &Snapshot, name: &str) -> bool {
        canonical(self.field(name)) == canonical(other.field(name))
    }

    fn same_lines(&self, other: &Snapshot) -> bool {
        self.line_texts() == other.line_texts()
    }

    fn same(&self, other: &Snapshot) -> bool {
        self.same_lines(other)
            && self
                .field_names(&[other])
                .iter()
                .all(|n| self.same_field(other, n))
    }
}

/// One aligned pair of lines: indexes into the left and the right side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinePair {
    /// The line on the left side (absent: added on the right).
    pub left: Option<usize>,
    /// The line on the right side (absent: removed on the right).
    pub right: Option<usize>,
    /// Both sides hold the same text.
    pub same: bool,
}

/// Aligns two line lists: lines equal on both sides are matched by a
/// longest common subsequence; between two matches, removed and added
/// lines pair up in order as changed lines, and the rest stand alone.
#[must_use]
#[allow(clippy::many_single_char_names, reason = "the textbook LCS indexes")]
pub fn align_lines(left: &[String], right: &[String]) -> Vec<LinePair> {
    let (n, m) = (left.len(), right.len());
    // lcs[i][j]: the LCS length of left[i..] and right[j..].
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if left[i] == right[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    let (mut gap_l, mut gap_r): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
    let flush = |out: &mut Vec<LinePair>, gl: &mut Vec<usize>, gr: &mut Vec<usize>| {
        let k = gl.len().max(gr.len());
        for x in 0..k {
            out.push(LinePair {
                left: gl.get(x).copied(),
                right: gr.get(x).copied(),
                same: false,
            });
        }
        gl.clear();
        gr.clear();
    };
    while i < n || j < m {
        if i < n && j < m && left[i] == right[j] {
            flush(&mut out, &mut gap_l, &mut gap_r);
            out.push(LinePair {
                left: Some(i),
                right: Some(j),
                same: true,
            });
            i += 1;
            j += 1;
        } else if j >= m || (i < n && lcs[i + 1][j] >= lcs[i][j + 1]) {
            gap_l.push(i);
            i += 1;
        } else {
            gap_r.push(j);
            j += 1;
        }
    }
    flush(&mut out, &mut gap_l, &mut gap_r);
    out
}

/// One header field that differs somewhere among the three versions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldChange {
    /// Wire name.
    pub field: String,
    /// When the form was opened.
    pub original: Value,
    /// This user's value now.
    pub mine: Value,
    /// The value saved by someone else.
    pub theirs: Value,
}

impl FieldChange {
    /// This user changed it.
    #[must_use]
    pub fn mine_changed(&self) -> bool {
        canonical(&self.mine) != canonical(&self.original)
    }

    /// The other user changed it.
    #[must_use]
    pub fn theirs_changed(&self) -> bool {
        canonical(&self.theirs) != canonical(&self.original)
    }
}

/// One of their changes that "Keep mine" would undo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undone {
    /// Their value of this header field gives way to mine.
    Field(String),
    /// Their lines give way to mine (`changed` of their lines differed
    /// from the original).
    Lines {
        /// How many of their lines differ from the original.
        changed: usize,
    },
}

/// What "Keep mine" saves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merge {
    /// The editable fields to save (on their stamp).
    pub result: Snapshot,
    /// Their changes this undoes; the dialog lists them.
    pub undone: Vec<Undone>,
    /// The lines came from my side.
    pub lines_from_mine: bool,
}

/// A save that came back 409 `STALE`: the three versions of the record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// As the form opened it.
    pub original: Snapshot,
    /// As the form holds it now.
    pub mine: Snapshot,
    /// As saved by someone else (read again after the 409).
    pub theirs: Snapshot,
}

impl Conflict {
    /// The three versions, each an editable-fields object with its lines
    /// under `lines_key`.
    #[must_use]
    pub fn new(original: &Value, mine: &Value, theirs: &Value, lines_key: &str) -> Conflict {
        Conflict {
            original: Snapshot::from_editable(original, lines_key),
            mine: Snapshot::from_editable(mine, lines_key),
            theirs: Snapshot::from_editable(theirs, lines_key),
        }
    }

    /// Nothing editable changed on their side (a posting moved a balance,
    /// a file was attached): save again on the new stamp without asking.
    #[must_use]
    pub fn silent_resave(&self) -> bool {
        self.theirs.same(&self.original)
    }

    /// Mine and theirs are already the same: nothing to choose either.
    #[must_use]
    pub fn already_equal(&self) -> bool {
        self.mine.same(&self.theirs)
    }

    /// Header fields that differ among the three, by name.
    #[must_use]
    pub fn field_changes(&self) -> Vec<FieldChange> {
        self.original
            .field_names(&[&self.mine, &self.theirs])
            .into_iter()
            .filter(|n| {
                !(self.original.same_field(&self.mine, n)
                    && self.original.same_field(&self.theirs, n))
            })
            .map(|n| FieldChange {
                field: n.to_string(),
                original: self.original.field(n).clone(),
                mine: self.mine.field(n).clone(),
                theirs: self.theirs.field(n).clone(),
            })
            .collect()
    }

    /// Mine and theirs lines, aligned (left: mine, right: theirs).
    #[must_use]
    pub fn line_diff(&self) -> Vec<LinePair> {
        align_lines(&self.mine.line_texts(), &self.theirs.line_texts())
    }

    /// This user changed a line.
    #[must_use]
    pub fn mine_changed_lines(&self) -> bool {
        !self.mine.same_lines(&self.original)
    }

    /// The other user changed a line.
    #[must_use]
    pub fn theirs_changed_lines(&self) -> bool {
        !self.theirs.same_lines(&self.original)
    }

    /// "Use theirs".
    #[must_use]
    pub fn use_theirs(&self) -> Snapshot {
        self.theirs.clone()
    }

    /// "Keep mine": their record, my changed header fields, and the lines
    /// whole from one side.
    #[must_use]
    pub fn keep_mine(&self) -> Merge {
        let mut result = self.theirs.clone();
        let mut undone = Vec::new();
        for c in self.field_changes() {
            if !c.mine_changed() {
                continue;
            }
            if c.theirs_changed() && canonical(&c.theirs) != canonical(&c.mine) {
                undone.push(Undone::Field(c.field.clone()));
            }
            if let Some(slot) = result.header.iter_mut().find(|(k, _)| *k == c.field) {
                slot.1 = c.mine.clone();
            } else {
                result.header.push((c.field.clone(), c.mine.clone()));
                result.header.sort_by(|a, b| a.0.cmp(&b.0));
            }
        }
        let lines_from_mine = self.mine_changed_lines();
        if lines_from_mine {
            result.lines.clone_from(&self.mine.lines);
            if self.theirs_changed_lines() && !self.mine.same_lines(&self.theirs) {
                let changed = align_lines(&self.original.line_texts(), &self.theirs.line_texts())
                    .iter()
                    .filter(|p| !p.same)
                    .count();
                undone.push(Undone::Lines { changed });
            }
        }
        Merge {
            result,
            undone,
            lines_from_mine,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    fn line(acct: &str, debit: &str, credit: &str) -> Value {
        json!({"accountId": acct, "debit": debit, "credit": credit})
    }

    fn je(memo: &str, date: &str, lines: &[Value]) -> Value {
        json!({"memo": memo, "txnDate": date, "lines": lines})
    }

    #[test]
    fn canonical_sorts_keys_and_keeps_amount_text() {
        let v = json!({"b": "10.50", "a": [1, {"z": null, "y": "0.10"}]});
        assert_eq!(
            canonical(&v),
            r#"{"a":[1,{"y":"0.10","z":null}],"b":"10.50"}"#
        );
    }

    #[test]
    fn a_change_they_made_to_nothing_editable_resaves_silently() {
        let o = je(
            "rent",
            "2026-11-01",
            &[line("a", "10.00", "0"), line("b", "0", "10.00")],
        );
        let mine = je(
            "rent nov",
            "2026-11-01",
            &[line("a", "10.00", "0"), line("b", "0", "10.00")],
        );
        let c = Conflict::new(&o, &mine, &o, "lines");
        assert!(c.silent_resave());
        let theirs = je(
            "rent",
            "2026-11-02",
            &[line("a", "10.00", "0"), line("b", "0", "10.00")],
        );
        let c = Conflict::new(&o, &mine, &theirs, "lines");
        assert!(!c.silent_resave());
    }

    #[test]
    fn keep_mine_takes_my_header_fields_over_theirs_and_names_what_it_undoes() {
        let ls = [line("a", "10.00", "0"), line("b", "0", "10.00")];
        let o = je("rent", "2026-11-01", &ls);
        let mine = je("rent nov", "2026-11-01", &ls);
        let theirs = je("rent (paid)", "2026-11-02", &ls);
        let c = Conflict::new(&o, &mine, &theirs, "lines");
        let m = c.keep_mine();
        assert_eq!(m.result.field("memo"), &json!("rent nov"));
        // Their date stays: I never touched it.
        assert_eq!(m.result.field("txnDate"), &json!("2026-11-02"));
        assert_eq!(m.undone, vec![Undone::Field("memo".into())]);
        assert!(!m.lines_from_mine);
        let fields: Vec<String> = c.field_changes().into_iter().map(|f| f.field).collect();
        assert_eq!(fields, vec!["memo", "txnDate"]);
    }

    #[test]
    fn keep_mine_never_mixes_lines_from_both_sides() {
        let o = je(
            "x",
            "d",
            &[line("a", "10.00", "0"), line("b", "0", "10.00")],
        );
        // I change the amount on both lines; they add a split.
        let mine = je(
            "x",
            "d",
            &[line("a", "12.00", "0"), line("b", "0", "12.00")],
        );
        let theirs = je(
            "x",
            "d",
            &[
                line("a", "10.00", "0"),
                line("b", "0", "6.00"),
                line("c", "0", "4.00"),
            ],
        );
        let c = Conflict::new(&o, &mine, &theirs, "lines");
        let m = c.keep_mine();
        assert!(m.lines_from_mine);
        assert_eq!(
            m.result.lines(),
            mine["lines"].as_array().expect("lines").as_slice()
        );
        assert_eq!(m.undone, vec![Undone::Lines { changed: 2 }]);
        // When I left the lines alone, theirs stay whole.
        let mine2 = je(
            "y",
            "d",
            &[line("a", "10.00", "0"), line("b", "0", "10.00")],
        );
        let m2 = Conflict::new(&o, &mine2, &theirs, "lines").keep_mine();
        assert!(!m2.lines_from_mine);
        assert_eq!(
            m2.result.lines(),
            theirs["lines"].as_array().expect("l").as_slice()
        );
        assert_eq!(m2.undone, Vec::<Undone>::new());
    }

    #[test]
    fn one_inserted_line_does_not_mark_every_line_changed() {
        let t = |v: &[&str]| v.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        let pairs = align_lines(&t(&["a", "b", "c"]), &t(&["a", "x", "b", "c"]));
        assert_eq!(pairs.iter().filter(|p| !p.same).count(), 1);
        assert_eq!(
            pairs[1],
            LinePair {
                left: None,
                right: Some(1),
                same: false
            }
        );
        // A changed line pairs with its replacement.
        let pairs = align_lines(&t(&["a", "b", "c"]), &t(&["a", "B", "c"]));
        assert_eq!(
            pairs[1],
            LinePair {
                left: Some(1),
                right: Some(1),
                same: false
            }
        );
    }

    #[test]
    fn snapshot_round_trips_and_handles_a_record_without_lines() {
        let v = json!({"name": "Acme", "notes": null});
        let s = Snapshot::from_editable(&v, "lines");
        assert_eq!(s.lines().len(), 0);
        assert_eq!(s.to_value(), v);
        let o = je("m", "d", &[line("a", "1.00", "0")]);
        assert_eq!(Snapshot::from_editable(&o, "lines").to_value(), o);
    }

    fn arb_lines() -> impl Strategy<Value = Vec<String>> {
        proptest::collection::vec(prop_oneof!["a", "b", "c", "d"], 0..8)
    }

    proptest! {
        /// Every line of both sides appears exactly once, in order, and
        /// matched pairs hold equal text.
        #[test]
        fn alignment_covers_both_sides_in_order(l in arb_lines(), r in arb_lines()) {
            let pairs = align_lines(&l, &r);
            let left: Vec<usize> = pairs.iter().filter_map(|p| p.left).collect();
            let right: Vec<usize> = pairs.iter().filter_map(|p| p.right).collect();
            prop_assert_eq!(left, (0..l.len()).collect::<Vec<_>>());
            prop_assert_eq!(right, (0..r.len()).collect::<Vec<_>>());
            for p in &pairs {
                if p.same {
                    prop_assert_eq!(&l[p.left.unwrap_or(0)], &r[p.right.unwrap_or(0)]);
                }
            }
            if l == r {
                prop_assert!(pairs.iter().all(|p| p.same));
            }
        }

        /// Keep mine's lines are exactly one side's lines, never a mix, so
        /// a balanced side stays balanced.
        #[test]
        fn keep_mine_lines_are_one_sides(o in arb_lines(), m in arb_lines(), t in arb_lines()) {
            let mk = |v: &[String]| json!({"memo": "x", "lines": v});
            let c = Conflict::new(&mk(&o), &mk(&m), &mk(&t), "lines");
            let merged: Vec<String> = c.keep_mine().result.lines().iter()
                .map(|v| v.as_str().unwrap_or_default().to_string()).collect();
            prop_assert!(merged == m || merged == t);
            prop_assert_eq!(c.silent_resave(), o == t);
        }
    }
}
