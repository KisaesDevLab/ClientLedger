//! Daily Sales (P26): the checked day as the engine's draft, field by field
//! (no arithmetic), and the day's tag gate.

use cl_api::{Account, DailyEntryDraft, DailyValue};
use cl_model::forms::daily_sales::{CheckedDaily, DailyLayout, DailySalesForm};
use cl_model::forms::tag_rule::{Place, TagLine, TagRefusal};
use cl_model::lists::tags::TagList;

/// The checked day as the engine's draft, field by field (no arithmetic).
#[must_use]
pub fn to_draft(c: &CheckedDaily) -> DailyEntryDraft {
    DailyEntryDraft {
        template: c.template,
        date: c.date,
        tag: c.tag,
        notes: c.notes.clone(),
        values: c
            .values
            .iter()
            .map(|(line, amount)| DailyValue {
                line: *line,
                amount: *amount,
                tag: None,
            })
            .collect(),
    }
}

/// The tag rules on the day (P29 WP7, Q-276): one tag for the whole day,
/// so every line of its `layout` is saved with it.
///
/// # Errors
/// The refused lines (nothing is sent).
pub fn tag_gate(
    form: &DailySalesForm,
    layout: &DailyLayout,
    list: &TagList,
    accounts: &[Account],
) -> Result<(), TagRefusal> {
    let tags: Vec<cl_core::ids::TagId> = form.tag.into_iter().collect();
    let mut lines = vec![TagLine {
        place: Place::Entry,
        account: None,
        tags: tags.clone(),
    }];
    lines.extend(
        layout
            .lines
            .iter()
            .filter_map(|l| l.account)
            .map(|a| TagLine {
                place: Place::Entry,
                account: Some(a),
                tags: tags.clone(),
            }),
    );
    super::tag_gate::check(list, &lines, accounts)
}
