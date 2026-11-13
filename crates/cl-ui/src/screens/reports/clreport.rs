//! `.clreport` report files (A09 WP-A, Q-A09a, plan §3.3): export one
//! memorized report or a whole group, and import a file with a review of
//! its references. The seam: the Memorized Reports list and the Report
//! Builder's Report tab call [`start_export`] and [`start_import`]; WP-A
//! fills them in.

use cl_api::resources::reports::definitions::{DefinitionGroup, DefinitionRow};

use crate::company::CompanyLink;

/// What to export.
#[derive(Clone, Debug)]
#[expect(dead_code, reason = "A09 seam: WP-A's export reads the target")]
pub(crate) enum Target {
    /// One memorized report.
    Report(Box<DefinitionRow>),
    /// A group and its reports, in list order.
    Group {
        /// The group.
        group: DefinitionGroup,
        /// Its reports.
        reports: Vec<DefinitionRow>,
    },
}

/// Starts exporting `target` to a `.clreport` file (WP-A). Returns the
/// i18n key of a message to show, if any.
pub(crate) fn start_export(link: &CompanyLink, target: Target) -> Option<&'static str> {
    let _ = (link, target);
    None
}

/// Starts importing a `.clreport` file (WP-A). Returns the i18n key of a
/// message to show, if any.
pub(crate) fn start_import(link: &CompanyLink) -> Option<&'static str> {
    let _ = link;
    None
}
