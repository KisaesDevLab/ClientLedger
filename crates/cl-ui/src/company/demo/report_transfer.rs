//! The demo engine's `.clreport` routes (A09 WP-A, U-220 parity): export
//! turns every id at an id path into `"$ref:N"` with what it named, and the
//! preview matches the references against the demo's lists with the
//! engine's rules (Q-A09i), applies the owner's choices with the engine's
//! leave-out rules, and runs the save check. No writes. Synthetic.
//!
//! Id paths, as the engine's:
//! - standard: `options.filters.{accountIds,contactIds,tagIds,itemIds}`,
//!   `options.tagSplit.tagGroupId`, `options.columnTagGroupId`,
//!   `options.budgetId`;
//! - builder: every node's (groups' children too) `filters[i].ids` of an
//!   account, contact, item or tag filter, and `groupBy[i].tagGroupId`.

use std::collections::BTreeMap;

use cl_api::resources::reports::definitions::{
    Candidate, Choices, Dropped, ExportWhat, NameTaken, PreviewProblem, PreviewReport,
    TransferReport, Unresolved,
};
use cl_api::{ApiError, ContactType};
use serde_json::{Map, Value, json};

use super::DemoState;

const STANDARD_LISTS: [(&str, &str); 4] = [
    ("accountIds", "account"),
    ("contactIds", "contact"),
    ("tagIds", "tag"),
    ("itemIds", "item"),
];

const BUILDER_FILTERS: [&str; 4] = ["account", "contact", "item", "tag"];

fn refusal(status: u16, code: &str, details: &Value) -> ApiError {
    cl_api::engine_refusal(
        status,
        &json!({"error": {"message": "refused", "code": code, "details": details}}),
    )
}

fn not_found() -> ApiError {
    ApiError::NotFound { code: None }
}

/// One row of a demo list as the matching sees it.
struct Row {
    id: String,
    name: String,
    /// Account number, contact type, tag group name, budget year.
    number: Option<String>,
    contact_type: Option<&'static str>,
    group: Option<String>,
    year: Option<i64>,
    account_type: Option<&'static str>,
}

impl Row {
    fn label(&self, kind: &str) -> String {
        match kind {
            "account" => match self.number.as_deref() {
                Some(n) if !n.is_empty() => format!("{n} \u{b7} {}", self.name),
                _ => self.name.clone(),
            },
            "contact" => format!("{} ({})", self.name, self.contact_type.unwrap_or("both")),
            "tag" => match &self.group {
                Some(g) => format!("{g}: {}", self.name),
                None => self.name.clone(),
            },
            "budget" => format!("{} ({})", self.name, self.year.unwrap_or_default()),
            _ => self.name.clone(),
        }
    }

    /// The reference this row exports as.
    fn reference(&self, kind: &str) -> Value {
        let mut m = Map::new();
        m.insert("kind".into(), Value::from(kind));
        m.insert("name".into(), Value::from(self.name.clone()));
        match kind {
            "account" => {
                if let Some(n) = self.number.as_ref().filter(|n| !n.is_empty()) {
                    m.insert("number".into(), Value::from(n.clone()));
                }
                m.insert("accountType".into(), json!(self.account_type));
            }
            "contact" => {
                m.insert("contactType".into(), json!(self.contact_type));
            }
            "tag" => {
                m.insert("tagGroupName".into(), json!(self.group));
            }
            "budget" => {
                m.insert("fiscalYear".into(), json!(self.year));
            }
            _ => {}
        }
        Value::Object(m)
    }
}

const fn contact_type(t: ContactType) -> &'static str {
    match t {
        ContactType::Customer => "customer",
        ContactType::Vendor => "vendor",
        ContactType::Both => "both",
    }
}

/// The demo's rows of `kind`, inactive ones too (they carry history).
fn rows(d: &mut DemoState, kind: &str) -> Vec<Row> {
    let blank = |id: String, name: String| Row {
        id,
        name,
        number: None,
        contact_type: None,
        group: None,
        year: None,
        account_type: None,
    };
    match kind {
        "account" => d
            .chart
            .clone()
            .unwrap_or_else(super::chart)
            .into_iter()
            .map(|a| Row {
                number: a.fields.account_number.clone(),
                account_type: Some(a.fields.account_type.as_str()),
                ..blank(a.id.to_string(), a.fields.name)
            })
            .collect(),
        "contact" => d
            .contacts
            .clone()
            .unwrap_or_else(super::contacts::contacts)
            .into_iter()
            .map(|c| Row {
                contact_type: Some(contact_type(c.fields.contact_type)),
                ..blank(c.id.to_string(), c.fields.display_name)
            })
            .collect(),
        "item" => d
            .items
            .clone()
            .unwrap_or_else(super::lists::items)
            .into_iter()
            .map(|i| blank(i.id.to_string(), i.fields.name))
            .collect(),
        "tagGroup" => tag_groups(d)
            .into_iter()
            .map(|(id, name)| blank(id, name))
            .collect(),
        "tag" => {
            let groups = tag_groups(d);
            d.tags
                .clone()
                .unwrap_or_else(super::lists::tags)
                .into_iter()
                .map(|t| Row {
                    group: t.fields.group_id.and_then(|g| {
                        let g = g.to_string();
                        groups
                            .iter()
                            .find(|(id, _)| *id == g)
                            .map(|(_, n)| n.clone())
                    }),
                    ..blank(t.id.to_string(), t.fields.name)
                })
                .collect()
        }
        "budget" => super::report_store::budgets(d)
            .into_iter()
            .map(|b| Row {
                year: Some(i64::from(b.fiscal_year)),
                ..blank(b.id, b.name)
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn tag_groups(d: &DemoState) -> Vec<(String, String)> {
    d.tag_groups
        .clone()
        .unwrap_or_else(super::lists::tag_groups)
        .into_iter()
        .map(|g| (g.id.to_string(), g.fields.name))
        .collect()
}

/// One id slot of a definition: its kind and where it is.
#[derive(Clone, Debug)]
enum Slot {
    /// An element of an id list: (pointer to the list, index).
    ListItem(String, usize),
    /// A single id at this pointer.
    Single(String),
}

/// Every id slot of `def` (a `kind` definition), in document order, with
/// the kind of row it names.
fn slots(def: &Value, kind: &str) -> Vec<(&'static str, Slot)> {
    let mut out = Vec::new();
    if kind == "standard" {
        for (key, k) in STANDARD_LISTS {
            let p = format!("/options/filters/{key}");
            if let Some(a) = def.pointer(&p).and_then(Value::as_array) {
                for i in 0..a.len() {
                    out.push((k, Slot::ListItem(p.clone(), i)));
                }
            }
        }
        for (p, k) in [
            ("/options/tagSplit/tagGroupId", "tagGroup"),
            ("/options/columnTagGroupId", "tagGroup"),
            ("/options/budgetId", "budget"),
        ] {
            if def.pointer(p).is_some_and(Value::is_string) {
                out.push((k, Slot::Single(p.to_string())));
            }
        }
    } else {
        nodes(def.get("nodes"), "/nodes", &mut out);
    }
    out
}

fn nodes(list: Option<&Value>, at: &str, out: &mut Vec<(&'static str, Slot)>) {
    let Some(list) = list.and_then(Value::as_array) else {
        return;
    };
    for (n, node) in list.iter().enumerate() {
        let here = format!("{at}/{n}");
        if let Some(filters) = node.get("filters").and_then(Value::as_array) {
            for (f, filter) in filters.iter().enumerate() {
                let field = filter.get("field").and_then(Value::as_str).unwrap_or("");
                let Some(k) = BUILDER_FILTERS.iter().find(|k| **k == field) else {
                    continue;
                };
                let p = format!("{here}/filters/{f}/ids");
                if let Some(ids) = filter.get("ids").and_then(Value::as_array) {
                    for i in 0..ids.len() {
                        out.push((k, Slot::ListItem(p.clone(), i)));
                    }
                }
            }
        }
        if let Some(gb) = node.get("groupBy").and_then(Value::as_array) {
            for (g, by) in gb.iter().enumerate() {
                if by.get("tagGroupId").is_some_and(Value::is_string) {
                    out.push((
                        "tagGroup",
                        Slot::Single(format!("{here}/groupBy/{g}/tagGroupId")),
                    ));
                }
            }
        }
        nodes(node.get("children"), &format!("{here}/children"), out);
    }
}

fn slot_value<'a>(def: &'a Value, s: &Slot) -> Option<&'a Value> {
    match s {
        Slot::ListItem(p, i) => def.pointer(p)?.get(*i),
        Slot::Single(p) => def.pointer(p),
    }
}

fn slot_mut<'a>(def: &'a mut Value, s: &Slot) -> Option<&'a mut Value> {
    match s {
        Slot::ListItem(p, i) => def.pointer_mut(p)?.get_mut(*i),
        Slot::Single(p) => def.pointer_mut(p),
    }
}

/// The export (`ids` in order, or a group's reports by name).
///
/// # Errors
/// 404 for a report or group that isn't there; 400 over 200 reports.
pub fn export(d: &mut DemoState, what: &ExportWhat) -> Result<Vec<TransferReport>, ApiError> {
    let (defs, groups) = super::report_store::definitions(d);
    let chosen: Vec<_> = match what {
        ExportWhat::Ids(ids) => ids
            .iter()
            .map(|id| {
                defs.iter()
                    .find(|r| r.id == *id)
                    .cloned()
                    .ok_or_else(not_found)
            })
            .collect::<Result<_, _>>()?,
        ExportWhat::Group(g) => {
            if !groups.iter().any(|x| x.id == *g) {
                return Err(not_found());
            }
            let mut v: Vec<_> = defs
                .iter()
                .filter(|r| r.group_id.as_deref() == Some(g.as_str()))
                .cloned()
                .collect();
            v.sort_by_key(|r| r.name.to_lowercase());
            v
        }
    };
    if chosen.len() > 200 {
        return Err(refusal(
            400,
            "VALIDATION_ERROR",
            &json!({"field": "groupId", "limit": 200}),
        ));
    }
    let mut lists: BTreeMap<&str, Vec<Row>> = BTreeMap::new();
    let mut out = Vec::new();
    for row in chosen {
        let mut def = row.definition.clone();
        let mut refs: Vec<Value> = Vec::new();
        let mut seen: Vec<(&str, String)> = Vec::new();
        for (k, s) in slots(&row.definition, &row.kind) {
            let Some(id) = slot_value(&def, &s)
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            let n = if let Some(n) = seen.iter().position(|(sk, sid)| *sk == k && *sid == id) {
                n
            } else {
                let list = lists.entry(k).or_insert_with(|| rows(d, k));
                refs.push(
                    list.iter()
                        .find(|r| r.id == id)
                        .map_or_else(|| json!({"kind": k, "missing": true}), |r| r.reference(k)),
                );
                seen.push((k, id));
                seen.len() - 1
            };
            if let Some(v) = slot_mut(&mut def, &s) {
                *v = Value::from(format!("$ref:{n}"));
            }
        }
        out.push(TransferReport {
            name: row.name.clone(),
            group_name: row
                .group_id
                .as_ref()
                .and_then(|g| groups.iter().find(|x| x.id == *g))
                .map(|g| g.name.clone()),
            kind: row.kind.clone(),
            definition: def,
            references: refs,
        });
    }
    Ok(out)
}

fn same(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// The rows `reference` matches, and the candidates when not exactly one.
fn matches<'a>(reference: &Value, list: &'a [Row]) -> (Vec<&'a Row>, Vec<&'a Row>) {
    if reference.get("missing").and_then(Value::as_bool) == Some(true) {
        return (Vec::new(), Vec::new());
    }
    let kind = reference.get("kind").and_then(Value::as_str).unwrap_or("");
    let name = reference.get("name").and_then(Value::as_str).unwrap_or("");
    let by_name = || -> Vec<&Row> { list.iter().filter(|r| same(&r.name, name)).collect() };
    let found: Vec<&Row> = match kind {
        "account" => match reference.get("number").and_then(Value::as_str) {
            // A number never resolves by name: a same-name account under
            // another number may be another account (engine Δ).
            Some(n) if !n.is_empty() => {
                let hit: Vec<&Row> = list
                    .iter()
                    .filter(|r| r.number.as_deref().is_some_and(|x| x.trim() == n.trim()))
                    .collect();
                if hit.len() == 1 {
                    return (hit, Vec::new());
                }
                return (Vec::new(), by_name());
            }
            _ => by_name(),
        },
        "contact" => {
            let want = reference.get("contactType").and_then(Value::as_str);
            by_name()
                .into_iter()
                .filter(|r| match (want, r.contact_type) {
                    (Some("customer"), Some(t)) => t == "customer" || t == "both",
                    (Some("vendor"), Some(t)) => t == "vendor" || t == "both",
                    _ => true,
                })
                .collect()
        }
        "tag" => {
            let group = reference.get("tagGroupName").and_then(Value::as_str);
            by_name()
                .into_iter()
                .filter(|r| match (group, r.group.as_deref()) {
                    (None, None) => true,
                    (Some(a), Some(b)) => same(a, b),
                    _ => false,
                })
                .collect()
        }
        "budget" => {
            let year = reference.get("fiscalYear").and_then(Value::as_i64);
            by_name().into_iter().filter(|r| r.year == year).collect()
        }
        _ => by_name(),
    };
    if found.len() == 1 {
        (found, Vec::new())
    } else {
        (Vec::new(), found)
    }
}

/// How one reference ends up.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Settled {
    Id(String),
    LeftOut,
    Open,
}

/// The import preview.
///
/// # Errors
/// 400 `DEFINITION_INVALID` (`SCHEMA_NEWER`, a kind that differs, a value
/// at an id path that isn't a token); 400 `VALIDATION_ERROR` for a choice
/// key outside the file or a chosen id that isn't the demo's row.
pub fn preview(
    d: &mut DemoState,
    reports: &[TransferReport],
    choices: &Choices,
) -> Result<Vec<PreviewReport>, ApiError> {
    let invalid = |path: String, code: &str| {
        refusal(
            400,
            "DEFINITION_INVALID",
            &json!({"path": path, "code": code}),
        )
    };
    for (i, r) in reports.iter().enumerate() {
        if r.definition
            .get("schemaVersion")
            .and_then(Value::as_u64)
            .is_some_and(|v| v > 1)
        {
            return Err(invalid(
                format!("reports.{i}.definition.schemaVersion"),
                "SCHEMA_NEWER",
            ));
        }
        if r.definition.get("kind").and_then(Value::as_str) != Some(r.kind.as_str()) {
            return Err(invalid(format!("reports.{i}.kind"), "SCHEMA"));
        }
        for (_, s) in slots(&r.definition, &r.kind) {
            let ok = slot_value(&r.definition, &s)
                .and_then(Value::as_str)
                .and_then(|t| t.strip_prefix("$ref:"))
                .and_then(|n| n.parse::<usize>().ok())
                .is_some_and(|n| n < r.references.len());
            if !ok {
                return Err(invalid(format!("reports.{i}.definition"), "SCHEMA"));
            }
        }
    }
    for key in choices.keys() {
        let known = key.split_once(':').is_some_and(|(i, tok)| {
            i.parse::<usize>()
                .ok()
                .and_then(|i| reports.get(i))
                .is_some_and(|r| {
                    tok.strip_prefix("$ref:")
                        .and_then(|n| n.parse::<usize>().ok())
                        .is_some_and(|n| n < r.references.len())
                })
        });
        if !known {
            return Err(refusal(
                400,
                "VALIDATION_ERROR",
                &json!({"field": "choices", "key": key}),
            ));
        }
    }
    let (defs, groups) = super::report_store::definitions(d);
    let mut lists: BTreeMap<String, Vec<Row>> = BTreeMap::new();
    let mut out = Vec::new();
    for (i, r) in reports.iter().enumerate() {
        let mut settled = Vec::new();
        let mut unresolved = Vec::new();
        for (n, reference) in r.references.iter().enumerate() {
            let kind = reference
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let list = lists.entry(kind.clone()).or_insert_with(|| rows(d, &kind));
            let token = format!("$ref:{n}");
            let (hit, mut candidates) = matches(reference, list);
            let choice = choices.get(&format!("{i}:{token}"));
            // A budget left out stays unresolved, its match a candidate.
            if kind == "budget" && matches!(choice, Some(None)) {
                candidates.extend(hit.iter().copied());
            }
            let s = match choice {
                Some(Some(id)) => {
                    if !list.iter().any(|x| x.id == *id) {
                        return Err(refusal(
                            400,
                            "VALIDATION_ERROR",
                            &json!({"field": "choices", "kind": kind}),
                        ));
                    }
                    Settled::Id(id.clone())
                }
                // A budget can't be left out.
                Some(None) if kind == "budget" => Settled::Open,
                Some(None) => Settled::LeftOut,
                _ => match hit.first() {
                    Some(row) => Settled::Id(row.id.clone()),
                    None => Settled::Open,
                },
            };
            if s == Settled::Open {
                unresolved.push(Unresolved {
                    reference_token: token,
                    reference: reference.clone(),
                    candidates: candidates
                        .iter()
                        .take(20)
                        .map(|c| Candidate {
                            id: c.id.clone(),
                            label: c.label(&kind),
                        })
                        .collect(),
                });
            }
            settled.push(s);
        }
        let (definition, dropped) = substitute(&r.definition, &r.kind, &settled);
        let problem = if unresolved.is_empty() {
            save_check(&definition, &r.kind)
        } else {
            None
        };
        let taken = defs.iter().find(|x| same(&x.name, &r.name));
        out.push(PreviewReport {
            name: r.name.clone(),
            kind: r.kind.clone(),
            ready: unresolved.is_empty() && problem.is_none(),
            definition,
            unresolved,
            dropped,
            group_id: r
                .group_name
                .as_deref()
                .and_then(|g| groups.iter().find(|x| same(&x.name, g)))
                .map(|g| g.id.clone()),
            name_taken: taken.map(|t| NameTaken {
                id: t.id.clone(),
                version: t.version,
                kind: t.kind.clone(),
                can_edit: t.can_edit,
            }),
            problem,
        });
    }
    Ok(out)
}

fn token_index(v: &Value) -> Option<usize> {
    v.as_str()?.strip_prefix("$ref:")?.parse().ok()
}

#[allow(
    clippy::many_single_char_names,
    reason = "slot, pointer, index and token read as in the engine's service"
)]
/// The definition with each settled reference's id in place and the
/// left-out ones removed by the engine's rules, and what each leave-out did.
fn substitute(def: &Value, kind: &str, settled: &[Settled]) -> (Value, Vec<Dropped>) {
    let mut v = def.clone();
    let mut effects: BTreeMap<usize, &'static str> = BTreeMap::new();
    let state = |n: Option<usize>| n.and_then(|n| settled.get(n));
    // Lists first, from the end so indexes hold; then singles.
    let mut all = slots(def, kind);
    all.reverse();
    let mut emptied: Vec<String> = Vec::new();
    for (_, s) in &all {
        let n = slot_value(def, s).and_then(token_index);
        match (s, state(n)) {
            (_, Some(Settled::Id(id))) => {
                if let Some(x) = slot_mut(&mut v, s) {
                    *x = Value::from(id.clone());
                }
            }
            (Slot::ListItem(p, i), Some(Settled::LeftOut)) => {
                if let Some(a) = v.pointer_mut(p).and_then(Value::as_array_mut) {
                    a.remove(*i);
                    if a.is_empty() && kind == "builder" {
                        emptied.push(p.clone());
                    }
                }
                if let Some(n) = n {
                    let e = if kind == "builder"
                        && v.pointer(p)
                            .and_then(Value::as_array)
                            .is_some_and(Vec::is_empty)
                    {
                        "filter_removed"
                    } else {
                        "removed_from_list"
                    };
                    let slot = effects.entry(n).or_insert(e);
                    if e == "filter_removed" {
                        *slot = e;
                    }
                }
            }
            (Slot::Single(p), Some(Settled::LeftOut)) => {
                let e = single_leave_out(&mut v, p);
                if let (Some(n), Some(e)) = (n, e) {
                    effects.entry(n).or_insert(e);
                }
            }
            _ => {}
        }
    }
    // An emptied builder filter goes (`.../filters/F/ids` → filter F), and
    // a removed group-by already left; then each list keeps an id once.
    emptied.sort();
    emptied.dedup();
    for p in emptied.iter().rev() {
        if let Some((filters, f)) = p
            .strip_suffix("/ids")
            .and_then(|q| q.rsplit_once('/'))
            .and_then(|(fs, f)| f.parse::<usize>().ok().map(|f| (fs.to_string(), f)))
            && let Some(a) = v.pointer_mut(&filters).and_then(Value::as_array_mut)
            && f < a.len()
        {
            a.remove(f);
        }
    }
    for (_, s) in slots(&v.clone(), kind) {
        if let Slot::ListItem(p, 0) = s
            && let Some(a) = v.pointer_mut(&p).and_then(Value::as_array_mut)
        {
            let mut seen = Vec::new();
            a.retain(|x| {
                let keep = !seen.contains(x);
                seen.push(x.clone());
                keep
            });
        }
    }
    let dropped = effects
        .into_iter()
        .map(|(n, e)| Dropped {
            reference_token: format!("$ref:{n}"),
            effect: e.into(),
        })
        .collect();
    (v, dropped)
}

/// Leaves out the single id at `p`, by the engine's rules.
fn single_leave_out(v: &mut Value, p: &str) -> Option<&'static str> {
    if p == "/options/tagSplit/tagGroupId" {
        if let Some(o) = v.pointer_mut("/options").and_then(Value::as_object_mut) {
            o.insert("tagSplit".into(), Value::Null);
        }
        return Some("tag_split_removed");
    }
    if p == "/options/columnTagGroupId" {
        if let Some(o) = v.pointer_mut("/options").and_then(Value::as_object_mut) {
            o.remove("columnTagGroupId");
            o.insert("columns".into(), Value::from("total"));
        }
        return Some("columns_total");
    }
    // A builder group-by on the tag group: `.../groupBy/G/tagGroupId`.
    let (list, g) = p.strip_suffix("/tagGroupId")?.rsplit_once('/')?;
    let g: usize = g.parse().ok()?;
    let a = v.pointer_mut(list)?.as_array_mut()?;
    if g < a.len() {
        a.remove(g);
    }
    Some("group_by_removed")
}

/// The save check on a settled definition: `None` when it would save.
fn save_check(def: &Value, kind: &str) -> Option<PreviewProblem> {
    let problem = |path: &str, code: &str| PreviewProblem {
        path: Some(path.into()),
        code: Some(code.into()),
    };
    if kind == "builder" {
        let text = serde_json::to_string(def).ok()?;
        return match cl_model::report_def::parse(&text) {
            Ok(_) => None,
            Err(cl_model::report_def::DefinitionError::Invalid { path, code }) => {
                Some(problem(&path, code.as_str()))
            }
            Err(_) => Some(problem("definition", "SCHEMA")),
        };
    }
    let ok = serde_json::from_value::<cl_model::reports::StandardDefinition>(def.clone())
        .is_ok_and(|s| s.check().is_ok());
    (!ok).then(|| problem("definition", "SCHEMA"))
}

/// Every demo row of a reference `kind` for the review's full-list
/// picker: `(id, label)` sorted by label.
pub(crate) fn pick_list(d: &mut DemoState, kind: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = rows(d, kind)
        .into_iter()
        .map(|r| {
            let l = r.label(kind);
            (r.id, l)
        })
        .collect();
    v.sort_by_key(|(_, l)| l.to_lowercase());
    v
}

#[cfg(test)]
#[path = "report_transfer_tests.rs"]
mod tests;
