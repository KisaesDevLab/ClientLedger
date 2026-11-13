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

/// The answer to a name clash. `Debug` names nothing (S-L5).
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Clash {
    /// No report has the name.
    None,
    /// Save under this name instead.
    Rename(String),
    /// Overwrite the report that has the name (after a confirm and a
    /// verified backup copy, Q-A09p).
    Replace,
}

impl std::fmt::Debug for Clash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Clash::None => "None",
            Clash::Rename(_) => "Rename(..)",
            Clash::Replace => "Replace",
        })
    }
}

/// Why Replace isn't offered (an i18n key).
pub(crate) const REPLACE_NOT_AUTHOR: &str = "report_file.replace.not_author";
/// Why Replace isn't offered: the kinds differ.
pub(crate) const REPLACE_OTHER_KIND: &str = "report_file.replace.other_kind";
/// Two reports of the file would Replace the same report (I2).
pub(crate) const REPLACE_TWICE: &str = "report_file.replace.twice";

/// What Continue does with one report. `Debug` names nothing (S-L5).
#[derive(Clone, PartialEq)]
pub(crate) struct Planned {
    /// The index in the file.
    pub(crate) index: usize,
    /// The name it is saved under (a Replace keeps the replaced report's
    /// own name and group: neither is sent).
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

impl std::fmt::Debug for Planned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Planned")
            .field("index", &self.index)
            .field("kind", &self.kind)
            .field("grouped", &self.group_id.is_some())
            .field("new_group", &self.new_group.is_some())
            .field("replace", &self.replace.is_some())
            .finish_non_exhaustive()
    }
}

/// The review. `Debug` shows counts only (S-L5).
#[derive(Clone, Default)]
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
    /// The file had several reports: it ends with Memorize all, even when
    /// one is left after a stop (#7).
    pub(crate) several: bool,
}

impl std::fmt::Debug for Review {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Review")
            .field("file", &self.file.len())
            .field("choices", &self.choices.len())
            .field("reports", &self.reports)
            .field("clashes", &self.clashes)
            .field("names", &self.names.len())
            .field("several", &self.several)
            .finish()
    }
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

/// The items of `v` at the indexes `keep`, in order.
fn keep_only<T>(v: Vec<T>, keep: &[usize]) -> Vec<T> {
    v.into_iter()
        .enumerate()
        .filter(|(i, _)| keep.contains(i))
        .map(|(_, x)| x)
        .collect()
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
            several: file.len() > 1,
        }
    }

    /// True for a file of one report (it opens unsaved, Q-A09j).
    pub(crate) fn single(&self) -> bool {
        self.file.len() == 1 && !self.several
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
                    // Replace can't stand now: a free name, not the taken one.
                    let taken = self.taken_names(i);
                    self.clashes[i] =
                        Clash::Rename(free_name(&r.name, |n| taken.iter().any(|t| same(t, n))));
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

    /// Takes back every choice for report `i` (Start over): its references
    /// are matched by the engine again.
    pub(crate) fn start_over(&mut self, i: usize) {
        let prefix = format!("{i}:");
        self.choices.retain(|k, _| !k.starts_with(&prefix));
    }

    /// True when the owner made a choice for report `i`.
    pub(crate) fn has_choices(&self, i: usize) -> bool {
        let prefix = format!("{i}:");
        self.choices.keys().any(|k| k.starts_with(&prefix))
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

    /// The report that report `i` would Replace (its id), if any.
    fn replace_target(&self, i: usize) -> Option<&str> {
        match self.clashes.get(i)? {
            Clash::Replace => self
                .reports
                .get(i)?
                .name_taken
                .as_ref()
                .map(|t| t.id.as_str()),
            _ => None,
        }
    }

    /// A problem with report `i`'s clash answer (an i18n key), if any.
    pub(crate) fn clash_problem(&self, i: usize) -> Option<&'static str> {
        match self.clashes.get(i)? {
            Clash::None => None,
            // Two reports of the file can't Replace the same one (I2): the
            // second would be refused after the first overwrote it.
            Clash::Replace => {
                let target = self.replace_target(i)?;
                (0..i)
                    .any(|j| self.replace_target(j) == Some(target))
                    .then_some(REPLACE_TWICE)
            }
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

    /// Takes the reports at `saved` (file indexes, already memorized) out
    /// of the review: their choices go and the others' are re-keyed
    /// (minor 7: a Memorize all stopped by a name taken meanwhile comes back
    /// to the review with the rest; the names are read again).
    pub(crate) fn remove(&mut self, saved: &[usize]) {
        let keep: Vec<usize> = (0..self.file.len())
            .filter(|i| !saved.contains(i))
            .collect();
        let mut choices = Choices::new();
        for (new, &old) in keep.iter().enumerate() {
            let prefix = format!("{old}:");
            for (k, v) in &self.choices {
                if let Some(token) = k.strip_prefix(&prefix) {
                    choices.insert(choice_key(new, token), v.clone());
                }
            }
        }
        self.file = keep_only(std::mem::take(&mut self.file), &keep);
        self.clashes = keep_only(std::mem::take(&mut self.clashes), &keep);
        // The preview is asked again; until then nothing is ready.
        self.reports.clear();
        self.choices = choices;
    }

    /// Unresolved references across every report of the last preview.
    pub(crate) fn unresolved_total(&self) -> usize {
        self.reports.iter().map(|r| r.unresolved.len()).sum()
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
                        // A Replace keeps the existing report's name as
                        // it is written (#15); the name isn't sent.
                        Clash::Replace => self
                            .names
                            .iter()
                            .find(|n| same(n, &r.name))
                            .map_or_else(|| r.name.trim().to_string(), |n| n.trim().to_string()),
                        Clash::None => r.name.trim().to_string(),
                    };
                    let file_group = self.file[i].group_name.clone();
                    let replacing = replace.is_some();
                    Planned {
                        index: i,
                        name,
                        // A Replace keeps the report's own group (#1).
                        group_id: if replacing { None } else { r.group_id.clone() },
                        new_group: if replacing || r.group_id.is_some() {
                            None
                        } else {
                            file_group
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
