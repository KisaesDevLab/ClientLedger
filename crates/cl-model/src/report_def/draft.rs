//! Describe the report (A10, Q-A10b–i): the state of an AI draft in the
//! builder. No egui, no I/O, no amounts.
//!
//! The engine's draft is a schema v1 definition that may hold `"$ref:N"`
//! placeholders where a name matched no row or several (U-220 tokens, at
//! `filters[].ids[]` and `groupBy[].tagGroupId`). The owner picks a row for
//! each or removes it; until none is left the definition is never sent to
//! a run, a preview or a save ([`sendable`]).
//!
//! A resolve works by token, never by the engine's site paths: removing an
//! id or a filter shifts every index after it. What is still unresolved is
//! read from the definition itself (the tokens in it), so an undo or redo
//! in the editor can't put the list out of step.

use std::collections::BTreeSet;

use super::clreport::RefKind;
use super::schema::{BuilderDefinition, Node};

/// The longest description the engine takes (UTF-16 units).
pub const MAX_DESCRIPTION_CHARS: usize = 1000;

/// The placeholder prefix.
pub const PREFIX: &str = "$ref:";

/// True when `description` may be sent: 1–1,000 UTF-16 units once
/// trimmed (the engine's limit).
#[must_use]
pub fn description_ok(description: &str) -> bool {
    let d = description.trim();
    !d.is_empty() && d.encode_utf16().count() <= MAX_DESCRIPTION_CHARS
}

/// True for a `$ref:N` placeholder (N: 1–4 ASCII digits; the same rule as
/// `cl-api`'s re-check of a draft).
#[must_use]
pub fn is_placeholder(s: &str) -> bool {
    s.strip_prefix(PREFIX)
        .is_some_and(|n| !n.is_empty() && n.len() <= 4 && n.bytes().all(|c| c.is_ascii_digit()))
}

fn walk<'a>(nodes: &'a [Node], f: &mut impl FnMut(&'a Node)) {
    for n in nodes {
        f(n);
        walk(n.children(), f);
    }
}

fn walk_mut(nodes: &mut [Node], f: &mut impl FnMut(&mut Node)) {
    for n in nodes {
        f(n);
        if let Node::Group(g) = n {
            walk_mut(&mut g.children, f);
        }
    }
}

/// The placeholders left in `def`.
#[must_use]
pub fn placeholders(def: &BuilderDefinition) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    walk(&def.nodes, &mut |n| {
        if let Node::Section(s) = n {
            for f in &s.filters {
                for id in f.ids.iter().flatten() {
                    if is_placeholder(id) {
                        out.insert(id.clone());
                    }
                }
            }
            for g in &s.group_by {
                if let Some(id) = g.tag_group_id.as_ref().filter(|x| is_placeholder(x)) {
                    out.insert(id.clone());
                }
            }
        }
    });
    out
}

/// True when any placeholder is left.
#[must_use]
pub fn has_placeholders(def: &BuilderDefinition) -> bool {
    !placeholders(def).is_empty()
}

/// The definition, when it may be sent to the engine (a run, a preview, a
/// save): never with a placeholder in it (the engine refuses one, and the
/// owner hasn't decided it yet).
#[must_use]
pub fn sendable(def: &BuilderDefinition) -> Option<&BuilderDefinition> {
    (!has_placeholders(def)).then_some(def)
}

/// The rows that hold `token`, in outline order.
#[must_use]
pub fn rows_with(def: &BuilderDefinition, token: &str) -> Vec<String> {
    let mut out = Vec::new();
    walk(&def.nodes, &mut |n| {
        if let Node::Section(s) = n {
            let here = s
                .filters
                .iter()
                .any(|f| f.ids.iter().flatten().any(|i| i == token))
                || s.group_by
                    .iter()
                    .any(|g| g.tag_group_id.as_deref() == Some(token));
            if here {
                out.push(s.id.clone());
            }
        }
    });
    out
}

/// Every heading and text the draft would print: the title, row and total
/// captions, text rows and renamed column headers.
fn texts(def: &BuilderDefinition) -> Vec<&str> {
    let mut out: Vec<&str> = def.title.iter().map(String::as_str).collect();
    out.extend(def.columns.headers.values().map(String::as_str));
    walk(&def.nodes, &mut |n| match n {
        Node::Section(s) => {
            out.push(&s.caption);
            out.extend(s.total_caption.as_deref());
        }
        Node::Group(g) => {
            out.push(&g.caption);
            out.extend(g.total_caption.as_deref());
        }
        Node::Formula(f) => out.push(&f.caption),
        Node::Text(t) => out.push(&t.text),
        Node::Blank(_) | Node::PageBreak(_) => {}
    });
    out
}

/// True when a heading or text of an AI draft states a number in words
/// (Q-A10l, [`super::number_words`]) and isn't exactly a books name in
/// `known` (a vendor called "Two Rivers Supply"). Such a draft is refused
/// whole, as one with a digit is (non-negotiable 4).
#[must_use]
pub fn states_a_number(def: &BuilderDefinition, known: &BTreeSet<String>) -> bool {
    texts(def)
        .into_iter()
        .any(|t| super::number_words::states_a_number(t) && !known.contains(t.trim()))
}

/// What the owner does with a name.
#[derive(Clone, PartialEq, Eq)]
pub enum Answer {
    /// This row of the books (an id from the candidates or the full list).
    Pick(String),
    /// Leave it out: the id goes; a filter left with no ids goes; a tag
    /// grouping left with no tag group goes.
    Remove,
}

impl std::fmt::Debug for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Answer::Pick(_) => "Pick(..)",
            Answer::Remove => "Remove",
        })
    }
}

/// `def` with `token` answered everywhere it appears. Returns `None` when
/// the token isn't in it, or a pick is itself a placeholder.
#[must_use]
pub fn resolve(def: &BuilderDefinition, token: &str, answer: &Answer) -> Option<BuilderDefinition> {
    if !is_placeholder(token) || !placeholders(def).contains(token) {
        return None;
    }
    if let Answer::Pick(id) = answer
        && (id.trim().is_empty() || is_placeholder(id))
    {
        return None;
    }
    let mut out = def.clone();
    walk_mut(&mut out.nodes, &mut |n| {
        let Node::Section(s) = n else {
            return;
        };
        for f in &mut s.filters {
            let Some(ids) = f.ids.as_mut() else {
                continue;
            };
            if !ids.iter().any(|i| i == token) {
                continue;
            }
            let mut next: Vec<String> = Vec::with_capacity(ids.len());
            for i in ids.drain(..) {
                let i = if i == token {
                    match answer {
                        Answer::Pick(id) => id.clone(),
                        Answer::Remove => continue,
                    }
                } else {
                    i
                };
                // A pick already in the list isn't listed twice.
                if !next.contains(&i) {
                    next.push(i);
                }
            }
            *ids = next;
        }
        // An emptied list filter has nothing left to filter by.
        s.filters
            .retain(|f| f.ids.as_ref().is_none_or(|ids| !ids.is_empty()));
        match answer {
            Answer::Pick(id) => {
                for g in &mut s.group_by {
                    if g.tag_group_id.as_deref() == Some(token) {
                        g.tag_group_id = Some(id.clone());
                    }
                }
            }
            // A tag grouping needs its group: without one it goes.
            Answer::Remove => s
                .group_by
                .retain(|g| g.tag_group_id.as_deref() != Some(token)),
        }
    });
    Some(out)
}

/// A name the engine couldn't match (the draft's side table: what it was
/// meant to be and the rows offered). `Debug` names no owner data.
#[derive(Clone, PartialEq, Eq)]
pub struct DraftName {
    /// `$ref:N`.
    pub token: String,
    /// What it is meant to be.
    pub kind: RefKind,
    /// The name (a books name or the owner's words), when the engine gave
    /// one.
    pub name: Option<String>,
    /// Several rows match (pick one of `candidates`); otherwise none did.
    pub ambiguous: bool,
    /// The rows that could be meant: `(id, label)`.
    pub candidates: Vec<(String, String)>,
}

impl std::fmt::Debug for DraftName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftName")
            .field("token", &self.token)
            .field("kind", &self.kind)
            .field("ambiguous", &self.ambiguous)
            .field("candidates", &self.candidates.len())
            .finish_non_exhaustive()
    }
}

/// An engine note, as the app words it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// Some names are left for the owner.
    PickUnresolved,
    /// The amount in the description was left out: the owner adds the
    /// amount filter (on this row, when known; Q-A10e).
    AmountFilterLeftOut(Option<String>),
    /// A period the app has no preset for was left out.
    PresetLeftOut,
    /// Numbers in the description were not used.
    NumbersNotUsed,
}

impl Note {
    /// The i18n key of its sentence.
    #[must_use]
    pub const fn key(&self) -> &'static str {
        match self {
            Note::PickUnresolved => "builder.ai.note.pick_unresolved",
            Note::AmountFilterLeftOut(Some(_)) => "builder.ai.note.amount_filter_left_out_row",
            Note::AmountFilterLeftOut(None) => "builder.ai.note.amount_filter_left_out",
            Note::PresetLeftOut => "builder.ai.note.preset_left_out",
            Note::NumbersNotUsed => "builder.ai.note.numbers_not_used",
        }
    }
}

/// A draft being checked in the builder (until Keep or Discard).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    /// The names, as the engine listed them.
    pub names: Vec<DraftName>,
    /// Its notes.
    pub notes: Vec<Note>,
    /// `anthropic`, `openai`, `ollama` (or `demo`).
    pub provider: Option<String>,
}

impl Draft {
    /// The names still to pick or remove in `def`, in the engine's order.
    #[must_use]
    pub fn open<'a>(&'a self, def: &BuilderDefinition) -> Vec<&'a DraftName> {
        let left = placeholders(def);
        self.names
            .iter()
            .filter(|n| left.contains(&n.token))
            .collect()
    }

    /// True when row `id` holds a name still to pick (its outline mark).
    #[must_use]
    pub fn row_open(&self, def: &BuilderDefinition, id: &str) -> bool {
        self.open(def)
            .iter()
            .any(|n| rows_with(def, &n.token).iter().any(|r| r == id))
    }
}

/// Where a "Describe the report" request is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// Nothing sent.
    #[default]
    Idle,
    /// The company's AI switches are being read (nothing sent yet).
    Checking,
    /// The description is with the engine.
    Drafting,
}

/// The describe panel: the owner's words and the request's phase.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Describe {
    /// The description being typed (owner text: never logged).
    pub text: String,
    /// Where the request is.
    pub phase: Phase,
}

impl std::fmt::Debug for Describe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Describe")
            .field("len", &self.text.chars().count())
            .field("phase", &self.phase)
            .finish()
    }
}

impl Describe {
    /// True when Draft may be pressed: words that fit, and nothing on its
    /// way.
    #[must_use]
    pub fn can_send(&self) -> bool {
        self.phase == Phase::Idle && description_ok(&self.text)
    }

    /// True while a request is on its way.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.phase != Phase::Idle
    }
}

#[cfg(test)]
#[path = "draft_tests.rs"]
mod tests;
