//! Verify Data's model (P34 B3, U-12): the findings of one check run,
//! grouped errors first, which of them can be fixed and are selected, the
//! plain words for each check code, and what "Open" opens.
//!
//! The UI turns the engine's run into [`FindingView`]s; nothing here
//! computes or compares an amount (they are the engine's exact figures,
//! shown as they came).

use std::collections::BTreeSet;

use cl_core::Money;

/// How serious a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// The books are wrong.
    Error,
    /// Something to look at.
    Warning,
}

/// What a finding is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SubjectKind {
    /// An account.
    Account,
    /// A transaction.
    Transaction,
    /// A document draft (Review Documents).
    DocumentDraft,
    /// An item.
    Item,
    /// Something this version cannot open.
    Other,
}

/// The engine's three figures for a finding (expected, actual, difference).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Figures {
    /// What it should be.
    pub expected: Money,
    /// What it is.
    pub actual: Money,
    /// The engine's difference.
    pub difference: Money,
}

/// One finding as the screen shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindingView {
    /// The engine's finding id.
    pub id: String,
    /// The check code (`ACCOUNT_BALANCE_DRIFT`, …).
    pub code: String,
    /// Error or warning.
    pub severity: Severity,
    /// What it is about.
    pub subject: SubjectKind,
    /// That record's id.
    pub subject_id: String,
    /// Its name or number (owner data: shown, never logged).
    pub label: String,
    /// The engine's figures, when it gave them.
    pub figures: Option<Figures>,
    /// The engine offers a fix.
    pub fixable: bool,
}

/// What "Open" on a finding opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenTarget {
    /// The account's register.
    Register(String),
    /// The transaction in its form.
    Transaction(String),
    /// Review Documents.
    ReviewDocuments,
    /// The item list.
    Items,
}

/// The i18n key of a check code's plain words (`verify.code.<code>`), or
/// the generic one for a code this version does not know.
#[must_use]
pub fn code_key(code: &str) -> &'static str {
    match code {
        "UNBALANCED_TRANSACTION" => "verify.code.unbalanced_transaction",
        "ACCOUNT_BALANCE_DRIFT" => "verify.code.account_balance_drift",
        "DRAFT_AFFECTS_BOOKS" => "verify.code.draft_affects_books",
        "LINE_ACCOUNT_SCOPE" => "verify.code.line_account_scope",
        "AR_CONTROL_DIFFERENCE" => "verify.code.ar_control_difference",
        "AP_CONTROL_DIFFERENCE" => "verify.code.ap_control_difference",
        "INVENTORY_VALUE_DIFFERENCE" => "verify.code.inventory_value_difference",
        _ => "verify.code.other",
    }
}

/// Every code with its own words (the i18n test checks each has text).
pub const KNOWN_CODES: [&str; 7] = [
    "UNBALANCED_TRANSACTION",
    "ACCOUNT_BALANCE_DRIFT",
    "DRAFT_AFFECTS_BOOKS",
    "LINE_ACCOUNT_SCOPE",
    "AR_CONTROL_DIFFERENCE",
    "AP_CONTROL_DIFFERENCE",
    "INVENTORY_VALUE_DIFFERENCE",
];

/// What "Open" opens for a finding, if anything.
#[must_use]
pub fn open_target(f: &FindingView) -> Option<OpenTarget> {
    if f.subject_id.is_empty() {
        return None;
    }
    match f.subject {
        SubjectKind::Account => Some(OpenTarget::Register(f.subject_id.clone())),
        SubjectKind::Transaction => Some(OpenTarget::Transaction(f.subject_id.clone())),
        SubjectKind::DocumentDraft => Some(OpenTarget::ReviewDocuments),
        SubjectKind::Item => Some(OpenTarget::Items),
        SubjectKind::Other => None,
    }
}

/// One run's findings and the owner's selection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VerifyModel {
    findings: Vec<FindingView>,
    selected: BTreeSet<String>,
}

impl VerifyModel {
    /// The findings of a run, errors first (the engine's order kept within
    /// each group); every fixable one starts selected.
    #[must_use]
    pub fn new(mut findings: Vec<FindingView>) -> VerifyModel {
        findings.sort_by_key(|f| f.severity);
        let selected = findings
            .iter()
            .filter(|f| f.fixable)
            .map(|f| f.id.clone())
            .collect();
        VerifyModel { findings, selected }
    }

    /// Every finding, errors first.
    #[must_use]
    pub fn findings(&self) -> &[FindingView] {
        &self.findings
    }

    /// The findings of one severity.
    pub fn of(&self, s: Severity) -> impl Iterator<Item = &FindingView> {
        self.findings.iter().filter(move |f| f.severity == s)
    }

    /// How many of each severity.
    #[must_use]
    pub fn counts(&self) -> (usize, usize) {
        (
            self.of(Severity::Error).count(),
            self.of(Severity::Warning).count(),
        )
    }

    /// True with nothing found.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    /// Selects or clears a finding; a finding the engine cannot fix is
    /// never selected.
    pub fn toggle(&mut self, id: &str) {
        let fixable = self.findings.iter().any(|f| f.id == id && f.fixable);
        if !fixable {
            return;
        }
        if !self.selected.remove(id) {
            self.selected.insert(id.to_string());
        }
    }

    /// True when selected.
    #[must_use]
    pub fn is_selected(&self, id: &str) -> bool {
        self.selected.contains(id)
    }

    /// The selected finding ids, in the list's order.
    #[must_use]
    pub fn selected_ids(&self) -> Vec<String> {
        self.findings
            .iter()
            .filter(|f| self.selected.contains(&f.id))
            .map(|f| f.id.clone())
            .collect()
    }

    /// Fixable findings in the run.
    #[must_use]
    pub fn fixable_count(&self) -> usize {
        self.findings.iter().filter(|f| f.fixable).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: &str, sev: Severity, subject: SubjectKind, fixable: bool) -> FindingView {
        FindingView {
            id: id.into(),
            code: "ACCOUNT_BALANCE_DRIFT".into(),
            severity: sev,
            subject,
            subject_id: format!("s-{id}"),
            label: "Checking".into(),
            figures: None,
            fixable,
        }
    }

    #[test]
    fn errors_come_first_and_only_fixable_ones_select() {
        let mut m = VerifyModel::new(vec![
            f("w1", Severity::Warning, SubjectKind::Account, true),
            f("e1", Severity::Error, SubjectKind::Transaction, false),
            f("w2", Severity::Warning, SubjectKind::Item, false),
        ]);
        assert_eq!(m.findings()[0].id, "e1");
        assert_eq!(m.counts(), (1, 2));
        assert_eq!(m.selected_ids(), vec!["w1".to_string()]);
        m.toggle("e1");
        assert!(!m.is_selected("e1"), "report-only findings never select");
        m.toggle("w1");
        assert_eq!(m.selected_ids(), Vec::<String>::new());
        m.toggle("w1");
        assert!(m.is_selected("w1"));
        assert_eq!(m.fixable_count(), 1);
        assert!(!m.is_clean());
        assert!(VerifyModel::new(Vec::new()).is_clean());
    }

    #[test]
    fn open_targets_and_words() {
        assert_eq!(
            open_target(&f("a", Severity::Warning, SubjectKind::Account, true)),
            Some(OpenTarget::Register("s-a".into()))
        );
        assert_eq!(
            open_target(&f("t", Severity::Error, SubjectKind::Transaction, false)),
            Some(OpenTarget::Transaction("s-t".into()))
        );
        assert_eq!(
            open_target(&f("d", Severity::Error, SubjectKind::DocumentDraft, false)),
            Some(OpenTarget::ReviewDocuments)
        );
        assert_eq!(
            open_target(&f("o", Severity::Error, SubjectKind::Other, false)),
            None
        );
        for c in KNOWN_CODES {
            assert_ne!(code_key(c), "verify.code.other", "{c}");
        }
        assert_eq!(code_key("SOMETHING_NEW"), "verify.code.other");
    }
}
