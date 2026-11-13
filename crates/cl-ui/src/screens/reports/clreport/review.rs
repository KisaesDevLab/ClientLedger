//! The import review's state (no egui): the file's reports, the owner's
//! choices, the engine's last preview, and each name clash's answer. The
//! window draws it and sends the preview again after every choice.
//!
//! Rules (plan §3.3, engine U-220):
//! - Continue (or Memorize all) only when every report is `ready` and every
//!   clash has a usable answer.
//! - A clash defaults to Rename with the next free `"<name> (2)"`; Replace
//!   is offered only when the report there may be changed by this user and
//!   is of the same kind (the engine refuses a kind change).
//! - A budget can't be left out (the engine keeps it unresolved).

use cl_api::resources::reports::definitions::{Choices, PreviewReport, TransferReport, choice_key};
use cl_model::report_def::clreport::{FileReport, RefKind, Reference, free_name};
use serde_json::Value;

/// The answer to a name clash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Clash {
    /// No report has the name.
    None,
    /// Save under this name instead.
    Rename(String),
    /// Overwrite the report that has the name (after a confirm and a
    /// verified backup copy, Q-A09p).
    Replace,
}

/// Why Replace isn't offered (an i18n key).
pub(crate) const REPLACE_NOT_AUTHOR: &str = "report_file.replace.not_author";
/// Why Replace isn't offered: the kinds differ.
pub(crate) const REPLACE_OTHER_KIND: &str = "report_file.replace.other_kind";

/// What Continue does with one report.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Planned {
    /// The index in the file.
    pub(crate) index: usize,
    /// The name it is saved under.
    pub(crate) name: String,
    /// This company's group (`None`: no group, or one to create).
    pub(crate) group_id: Option<String>,
    /// The file's group name, when this company has no group of that name
    /// (Memorize all creates it).
    pub(crate) new_group: Option<String>,
    /// `standard` or `builder`.
    pub(crate) kind: String,
    /// The definition with this company's ids.
    pub(crate) definition: Value,
    /// Replace this report (id, version read by the preview).
    pub(crate) replace: Option<(String, u64)>,
}

/// The review.
#[derive(Clone, Debug, Default)]
pub(crate) struct Review {
    /// The file's reports, as the preview takes them.
    pub(crate) file: Vec<TransferReport>,
    /// The owner's choices so far.
    pub(crate) choices: Choices,
    /// The last preview (one per report, in file order).
    pub(crate) reports: Vec<PreviewReport>,
    /// Each report's clash answer.
    pub(crate) clashes: Vec<Clash>,
    /// The memorized report names of this company (for a free name).
    pub(crate) names: Vec<String>,
}

/// The preview's report body for a file report (`group` → `groupName`).
pub(crate) fn transfer_of(r: &FileReport) -> TransferReport {
    TransferReport {
        name: r.name.clone(),
        group_name: r.group.clone(),
        kind: r.kind.clone(),
        definition: r.definition.clone(),
        references: r.references.clone(),
    }
}

/// A file report for an exported one (`groupName` → `group`).
pub(crate) fn file_report_of(r: &TransferReport) -> FileReport {
    FileReport {
        name: r.name.clone(),
        group: r.group_name.clone(),
        kind: r.kind.clone(),
        definition: r.definition.clone(),
        references: r.references.clone(),
    }
}

fn same(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

impl Review {
    /// A review of `file` (before any preview).
    pub(crate) fn new(file: &[FileReport], names: Vec<String>) -> Review {
        Review {
            file: file.iter().map(transfer_of).collect(),
            choices: Choices::new(),
            reports: Vec::new(),
            clashes: vec![Clash::None; file.len()],
            names,
        }
    }

    /// True for a file of one report (it opens unsaved, Q-A09j).
    pub(crate) fn single(&self) -> bool {
        self.file.len() == 1
    }

    /// Takes a preview answer. A clash seen for the first time gets Rename
    /// with a free name; one that went away is cleared; an answer the
    /// owner gave is kept.
    pub(crate) fn take(&mut self, reports: Vec<PreviewReport>) {
        self.clashes.resize(reports.len(), Clash::None);
        for (i, r) in reports.iter().enumerate() {
            match (&r.name_taken, &self.clashes[i]) {
                (None, _) => self.clashes[i] = Clash::None,
                (Some(_), Clash::None) => {
                    let taken = self.taken_names(i);
                    self.clashes[i] =
                        Clash::Rename(free_name(&r.name, |n| taken.iter().any(|t| same(t, n))));
                }
                (Some(_), Clash::Replace) if Self::replace_refused(&reports[i]).is_some() => {
                    self.clashes[i] = Clash::Rename(r.name.clone());
                }
                _ => {}
            }
        }
        self.reports = reports;
    }

    /// The names a Rename of report `i` must not take: this company's, and
    /// the other reports' of this file as they will be saved.
    fn taken_names(&self, i: usize) -> Vec<String> {
        let mut v = self.names.clone();
        for (j, r) in self.file.iter().enumerate() {
            if j == i {
                continue;
            }
            v.push(match self.clashes.get(j) {
                Some(Clash::Rename(n)) => n.clone(),
                _ => r.name.clone(),
            });
        }
        v
    }

    /// Why Replace can't be chosen for `r`, if it can't.
    pub(crate) fn replace_refused(r: &PreviewReport) -> Option<&'static str> {
        let t = r.name_taken.as_ref()?;
        if !t.can_edit {
            Some(REPLACE_NOT_AUTHOR)
        } else if t.kind != r.kind {
            Some(REPLACE_OTHER_KIND)
        } else {
            None
        }
    }

    /// Picks `id` for reference `token` of report `i` (`None`: leave it
    /// out). The preview is sent again.
    pub(crate) fn choose(&mut self, i: usize, token: &str, id: Option<String>) {
        self.choices.insert(choice_key(i, token), id);
    }

    /// The owner's answer to report `i`'s clash.
    pub(crate) fn set_clash(&mut self, i: usize, c: Clash) {
        if c == Clash::Replace
            && self
                .reports
                .get(i)
                .is_none_or(|r| Self::replace_refused(r).is_some())
        {
            return;
        }
        if let Some(slot) = self.clashes.get_mut(i) {
            *slot = c;
        }
    }

    /// A problem with report `i`'s clash answer (an i18n key), if any.
    pub(crate) fn clash_problem(&self, i: usize) -> Option<&'static str> {
        match self.clashes.get(i)? {
            Clash::None | Clash::Replace => None,
            Clash::Rename(n) => {
                let n = n.trim();
                if n.is_empty() {
                    Some("report_file.rename.empty")
                } else if self.taken_names(i).iter().any(|t| same(t, n)) {
                    Some("report_file.rename.taken")
                } else {
                    None
                }
            }
        }
    }

    /// True when Continue (or Memorize all) may go on.
    pub(crate) fn can_continue(&self) -> bool {
        !self.reports.is_empty()
            && self.reports.len() == self.file.len()
            && self.reports.iter().all(|r| r.ready)
            && (0..self.reports.len()).all(|i| self.clash_problem(i).is_none())
    }

    /// References of report `i` the engine matched on its own (neither
    /// unresolved nor left out).
    pub(crate) fn matched(&self, i: usize) -> usize {
        let (Some(f), Some(r)) = (self.file.get(i), self.reports.get(i)) else {
            return 0;
        };
        f.references
            .len()
            .saturating_sub(r.unresolved.len() + r.dropped.len())
    }

    /// The reference `token` of report `i`, read for display.
    pub(crate) fn reference(&self, i: usize, token: &str) -> Option<Reference> {
        let n: usize = token.strip_prefix("$ref:")?.parse().ok()?;
        Reference::of(self.file.get(i)?.references.get(n)?)
    }

    /// True when reference `token` of report `i` can be left out.
    pub(crate) fn can_leave_out(&self, i: usize, token: &str) -> bool {
        self.reference(i, token)
            .is_some_and(|r| r.kind != RefKind::Budget)
    }

    /// True when any answer is Replace.
    pub(crate) fn replaces(&self) -> bool {
        self.clashes.contains(&Clash::Replace)
    }

    /// What Continue saves, in file order; `None` until it may go on.
    pub(crate) fn plan(&self) -> Option<Vec<Planned>> {
        if !self.can_continue() {
            return None;
        }
        Some(
            self.reports
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let replace = match self.clashes[i] {
                        Clash::Replace => r.name_taken.as_ref().map(|t| (t.id.clone(), t.version)),
                        _ => None,
                    };
                    let name = match &self.clashes[i] {
                        Clash::Rename(n) => n.trim().to_string(),
                        _ => r.name.trim().to_string(),
                    };
                    let file_group = self.file[i].group_name.clone();
                    Planned {
                        index: i,
                        name,
                        group_id: r.group_id.clone(),
                        new_group: if r.group_id.is_none() {
                            file_group
                        } else {
                            None
                        },
                        kind: r.kind.clone(),
                        definition: r.definition.clone(),
                        replace,
                    }
                })
                .collect(),
        )
    }
}
