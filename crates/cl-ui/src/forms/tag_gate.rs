//! The forms' side of the tag gate (P29 WP7, Q-276, Q-180): the forms'
//! tag list built in one place with the company's rules, and a refusal in
//! plain words that names each line ("Expenses tab, line 2: Add a tag…").
//! The rule itself is `cl_model::forms::tag_rule`; the gate over the
//! engine's lists is `cl_mcp::forms::tag_gate` (A11 D0), shared with the
//! MCP tools.

use cl_api::{Account, Tag};
use cl_model::forms::tag_rule::{Place, TagLine, TagRefusal};
use cl_model::lists::tags::{LineTagProblem, TagList};
use eframe::egui;

pub use cl_mcp::forms::tag_gate::kind_in;

use crate::company::CompanyLink;
use crate::i18n::{t, t_with};

/// The tag list every form uses: the tags as read, with the company's
/// tag rules (`CompanyLink::tag_rules`).
#[must_use]
pub fn form_list(tags: &[Tag], link: &CompanyLink) -> TagList {
    cl_mcp::forms::tag_gate::form_list(tags, link.tag_rules())
}

fn problem_text(p: LineTagProblem, list: &TagList) -> String {
    match p {
        LineTagProblem::RequiredMissing(g) => match list.group(g).map(|g| g.name.trim()) {
            Some(name) if !name.is_empty() => t_with(p.key().0, &[("group", name)]),
            _ => t("tags.line.required_missing_any"),
        },
        _ => t(p.key().0),
    }
}

/// One line's refusal in words.
#[must_use]
pub fn line_text(place: Place, p: LineTagProblem, list: &TagList) -> String {
    let problem = problem_text(p, list);
    let n = |i: usize| (i + 1).to_string();
    match place {
        Place::Expenses(i) => t_with(
            "forms.line_where.expenses",
            &[("line", &n(i)), ("problem", &problem)],
        ),
        Place::Items(i) => t_with(
            "forms.line_where.items",
            &[("line", &n(i)), ("problem", &problem)],
        ),
        Place::Line(i) => t_with(
            "forms.line_where.line",
            &[("line", &n(i)), ("problem", &problem)],
        ),
        Place::Row(s, r) => t_with(
            "forms.line_where.row",
            &[("section", &n(s)), ("row", &n(r)), ("problem", &problem)],
        ),
        Place::Entry => problem,
    }
}

/// The refusal's lines in words, in form order.
#[must_use]
pub fn refusal_texts(r: &TagRefusal, list: &TagList) -> Vec<String> {
    r.lines
        .iter()
        .map(|(place, p)| line_text(*place, *p, list))
        .collect()
}

/// Runs the gate. `Err` holds the words to show (nothing is sent).
///
/// # Errors
/// The refused lines, in words.
pub fn check(list: &TagList, lines: &[TagLine], accounts: &[Account]) -> Result<(), Vec<String>> {
    cl_mcp::forms::tag_gate::check(list, lines, accounts).map_err(|r| refusal_texts(&r, list))
}

/// Shows a refusal under the form's problems (nothing when empty).
pub fn show(ui: &mut egui::Ui, tokens: &crate::theme::Tokens, texts: &[String]) {
    if texts.is_empty() {
        return;
    }
    ui.label(egui::RichText::new(t("forms.tags_refused")).color(tokens.warn_text));
    for line in texts {
        ui.add(egui::Label::new(egui::RichText::new(line).color(tokens.warn_text)).wrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::ids::TagGroupId;

    #[test]
    fn refusals_name_the_tab_and_line_in_plain_words() {
        let list = TagList::default();
        assert_eq!(
            line_text(Place::Expenses(1), LineTagProblem::Missing, &list),
            "Expenses tab, line 2: Add a tag. This company asks for a tag on every line."
        );
        assert_eq!(
            line_text(Place::Items(0), LineTagProblem::Missing, &list),
            "Items tab, line 1: Add a tag. This company asks for a tag on every line."
        );
        assert!(line_text(Place::Line(2), LineTagProblem::Missing, &list).starts_with("Line 3: "));
        assert!(
            line_text(Place::Row(0, 4), LineTagProblem::Missing, &list)
                .starts_with("Section 1, row 5: ")
        );
        let g = TagGroupId::parse("60000000-0000-4000-8000-000000000001").expect("id");
        assert_eq!(
            line_text(Place::Entry, LineTagProblem::RequiredMissing(g), &list),
            "Lines on this account need a tag from a required tag group."
        );
    }
}
