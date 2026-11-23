//! The Tags list's side of the save-conflict dialog (P33 D4 step 11), as
//! Transfer Funds' (header-only): an edit is guarded by the tag as the
//! panel opened it, a 409 `STALE` reads their tag, saves again without
//! asking when nothing editable changed on their side, and opens
//! [`crate::screens::conflict_dialog`] otherwise.

use std::sync::Arc;

use cl_api::{ApiError, Tag, Versioned};
use cl_core::ids::TagId;
use cl_model::forms::SaveIntent;
use cl_model::lan::Conflict;
use cl_model::lists::tags::TagDraft;
use serde_json::{Value, json};

use super::{Panel, TagsScreen};
use crate::company::Task;
use crate::forms::conflict::{GONE, OpenConflict, Read, Words, read_of};
use crate::screens::conflict_dialog::{ConflictChoice, fields_from_theirs};

/// The editable fields of a tag as the panel holds them.
#[must_use]
pub fn tag_value(d: &TagDraft) -> Value {
    json!({
        "tagNumber": d.number.trim(),
        "name": d.name.trim(),
        "tagGroup": d.group.map(|g| g.to_string()),
        "color": d.color.trim(),
        "description": d.description.trim(),
    })
}

/// A tag as the panel would show it.
pub(crate) fn draft_of(t: &Tag) -> TagDraft {
    TagDraft::from_entry(&crate::tag_names::entry(t))
}

impl TagsScreen {
    /// A save came back stale: read their tag (the dialog follows).
    pub(crate) fn tag_conflict_start(&mut self, id: TagId, intent: SaveIntent) {
        if self.link.is_demo() {
            let found = self
                .link
                .demo_state()
                .and_then(|mut d| d.tags_mut().iter().find(|x| x.id == id).cloned());
            let got = found
                .map(|t| Versioned {
                    version: t.version(),
                    record: t,
                    stock_warnings: Vec::new(),
                })
                .ok_or(ApiError::NotFound { code: None });
            self.tag_conflict_read(got, intent);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.tag_stale();
            return;
        };
        match group.read(&session, move |s| async move { s.tags().get(id).await }) {
            Ok(p) => self.tag_conflict.reading = Some((Task::Pending(p), intent)),
            Err(_) => self.tag_stale(),
        }
    }

    /// The panel says the tag changed elsewhere (the plain refusal).
    fn tag_stale(&mut self) {
        self.warn = Some("lan.list.changed_elsewhere");
    }

    /// Polls the read of their tag.
    pub(crate) fn poll_tag_conflict(&mut self) {
        if let Some((r, intent)) = self.tag_conflict.poll() {
            self.tag_conflict_read(r, intent);
        }
    }

    pub(crate) fn tag_conflict_read(
        &mut self,
        r: Result<Versioned<Tag>, ApiError>,
        intent: SaveIntent,
    ) {
        let theirs = match read_of(r) {
            Read::Theirs(v) => v,
            Read::Gone => {
                self.warn = Some(GONE);
                return;
            }
            Read::Reload => {
                self.tag_stale();
                return;
            }
        };
        let Panel::Tag(f) = &self.panel else {
            return;
        };
        let conflict = Conflict::new(
            &tag_value(f.form.original()),
            &tag_value(f.form.current()),
            &tag_value(&draft_of(&theirs.record)),
            "lines",
        );
        if conflict.silent_resave() {
            let mine = f.form.current().clone();
            self.adopt_tag(theirs.record, mine);
            self.save_tag(intent);
            return;
        }
        let words = Words::new(Arc::default(), None, cl_core::MoneyStyle::default(), &[]);
        let groups: Vec<(String, String)> = self
            .groups
            .iter()
            .map(|g| (g.id.to_string(), g.fields.name.clone()))
            .collect();
        let dialog =
            crate::screens::conflict_dialog::ConflictDialog::new(conflict, words.line_text())
                .with_field_text(Box::new(move |field, v| match field {
                    "tagGroup" => v.as_str().and_then(|id| {
                        groups.iter().find(|(k, _)| k == id).map(|(_, n)| n.clone())
                    }),
                    _ => None,
                }));
        self.tag_conflict.open = Some(OpenConflict {
            dialog,
            theirs,
            intent,
        });
    }

    /// Their tag becomes the panel's "as opened" and `mine` its fields.
    fn adopt_tag(&mut self, theirs: Tag, mine: TagDraft) {
        if let Panel::Tag(f) = &mut self.panel {
            f.form.load(draft_of(&theirs));
            if *f.form.current() != mine {
                f.form.edit_step(|d| *d = mine);
            }
            f.error = None;
            f.opened = Some(theirs);
        }
        self.warn = None;
    }

    /// Draws the open dialog and runs the owner's choice.
    pub(crate) fn tag_conflict_ui(&mut self, ctx: &eframe::egui::Context) {
        if let Some((open, choice)) = self.tag_conflict.ui(ctx) {
            self.run_tag_choice(open, choice);
        }
    }

    #[allow(clippy::needless_pass_by_value)] // the dialog hands over its choice
    pub(crate) fn run_tag_choice(
        &mut self,
        open: OpenConflict<Versioned<Tag>>,
        choice: ConflictChoice,
    ) {
        match choice {
            ConflictChoice::Back => self.tag_stale(),
            ConflictChoice::UseTheirs => {
                let theirs = open.theirs.record;
                if let Panel::Tag(f) = &mut self.panel {
                    f.form.load(draft_of(&theirs));
                    f.opened = Some(theirs);
                }
                self.warn = None;
            }
            ConflictChoice::KeepMine(_) => {
                let Panel::Tag(f) = &self.panel else {
                    return;
                };
                let tf = draft_of(&open.theirs.record);
                let mut mine = f.form.current().clone();
                for field in fields_from_theirs(open.dialog.conflict()) {
                    match field.as_str() {
                        "tagNumber" => mine.number.clone_from(&tf.number),
                        "name" => mine.name.clone_from(&tf.name),
                        "tagGroup" => mine.group = tf.group,
                        "color" => mine.color.clone_from(&tf.color),
                        "description" => mine.description.clone_from(&tf.description),
                        _ => {}
                    }
                }
                self.adopt_tag(open.theirs.record, mine);
                self.save_tag(open.intent);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_has_plain_words() {
        for k in tag_value(&TagDraft::default())
            .as_object()
            .expect("object")
            .keys()
        {
            assert!(crate::i18n::has(&format!("lan.field.{k}")), "{k}");
        }
    }
}
