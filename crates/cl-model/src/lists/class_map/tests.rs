use super::*;
use proptest::prelude::*;

/// The synthetic class list (`fixtures/lists/classes.csv`), as P31 will
/// read it: one class per row with its status.
fn fixture() -> Vec<ClassInput> {
    include_str!("../../../../../fixtures/lists/classes.csv")
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (name, status) = l.rsplit_once(',').expect("two columns");
            ClassInput {
                full_name: name.to_owned(),
                active: status.trim() == "Active",
                used_directly: false,
            }
        })
        .collect()
}

#[test]
fn naming_rule() {
    assert_eq!(split_class("Overhead"), Some((None, "Overhead".into())));
    assert_eq!(
        split_class("Crew:Crew 1"),
        Some((Some("Crew".into()), "Crew 1".into()))
    );
    assert_eq!(
        split_class(" Region : North : Lake "),
        Some((Some("Region".into()), "North:Lake".into()))
    );
    assert_eq!(
        split_class("Crew: :North"),
        Some((Some("Crew".into()), "North".into()))
    );
    assert_eq!(split_class(" : "), None);
    assert_eq!(split_class(""), None);
}

#[test]
fn fixture_classes_become_tags_and_groups() {
    let plan = plan_classes(&fixture(), &["Storm Cleanup"]);
    let groups: Vec<_> = plan.groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(groups, ["Crew", "Route", "Region"]);
    assert!(plan.groups.iter().all(|g| g.one_per_line));
    let tags: Vec<_> = plan
        .tags
        .iter()
        .map(|t| (t.group.as_deref(), t.tag.as_str(), t.active))
        .collect();
    assert_eq!(
        tags,
        vec![
            (None, "Overhead", true),
            (Some("Crew"), "Crew 1", true),
            (Some("Crew"), "Crew 2", true),
            (Some("Route"), "North Route", true),
            (Some("Route"), "South Route", true),
            (Some("Route"), "East", true),
            (Some("Region"), "East (Region)", true),
            (Some("Region"), "North:Lake", true),
            (None, "Spring Promo 2025", false),
        ]
    );
    assert_eq!(plan.problems, vec![]);
    // The parent-only class has no tag; its children are found by name.
    assert_eq!(plan.tag_for("Crew"), None);
    assert_eq!(
        plan.tag_for("crew : crew 2").map(|t| t.tag.as_str()),
        Some("Crew 2")
    );
    assert_eq!(
        plan.tag_for("Region:North:Lake")
            .map(|t| t.group.as_deref()),
        Some(Some("Region")),
        "the group keeps the first spelling"
    );
}

#[test]
fn parents_used_on_transactions_get_their_own_tag() {
    let plan = plan_classes(
        &[ClassInput::new("Crew"), ClassInput::new("Crew:North")],
        &[],
    );
    assert_eq!(plan.groups.len(), 1);
    assert_eq!(
        plan.tag_for("Crew")
            .map(|t| (t.group.clone(), t.tag.clone())),
        Some((None, "Crew".into()))
    );
}

#[test]
fn clashes_existing_tags_duplicates_and_problems() {
    let long = "x".repeat(NAME_MAX + 1);
    let classes = vec![
        ClassInput::new("North"),
        ClassInput::new("Route:North"),
        ClassInput::new("Crew:North"),
        ClassInput::new("route:north"),
        ClassInput::new("  "),
        ClassInput::new(&long),
        ClassInput::new(&format!("{long}:Child")),
        ClassInput::new(&format!("Parent:{long}")),
    ];
    let plan = plan_classes(&classes, &["north (crew)"]);
    let tags: Vec<_> = plan.tags.iter().map(|t| t.tag.as_str()).collect();
    assert_eq!(tags, ["North", "North (Route)", "North (Crew) 2"]);
    let problems: Vec<_> = plan.problems.iter().map(|(_, p)| *p).collect();
    assert_eq!(
        problems,
        [
            ClassProblem::Blank,
            ClassProblem::TooLong,
            ClassProblem::TooLong,
            ClassProblem::TooLong
        ]
    );
    assert_eq!(ClassProblem::Blank.key().as_str(), "classes.problem.blank");
    assert_eq!(
        ClassProblem::TooLong.key().as_str(),
        "classes.problem.too_long"
    );
    // A long parent that only groups children is reported too.
    let plan = plan_classes(
        &[
            ClassInput {
                full_name: long.clone(),
                active: true,
                used_directly: false,
            },
            ClassInput::new(&format!("{long}:A")),
        ],
        &[],
    );
    assert_eq!(plan.problems.len(), 2);
    assert_eq!(plan.groups.len(), 0);
    // A clash whose bracketed name would be too long is reported.
    let near = "y".repeat(NAME_MAX - 2);
    let plan = plan_classes(&[ClassInput::new(&format!("Gr:{near}"))], &[&near]);
    assert_eq!(
        plan.problems,
        vec![(format!("Gr:{near}"), ClassProblem::TooLong)]
    );
}

proptest! {
    /// Any class list: tag names are unique ignoring case (and new against
    /// the existing ones), every grouped tag's group is planned once, and
    /// every non-blank class is either mapped, a parent-only group, or
    /// reported.
    #[test]
    fn plans_are_consistent(
        names in proptest::collection::vec("[A-Ca-c ]{0,3}(:[A-Ca-c ]{0,3}){0,2}", 0..24),
        existing in proptest::collection::vec("[A-Ca-c]{1,3}", 0..4),
    ) {
        let classes: Vec<ClassInput> = names.iter().map(|n| ClassInput::new(n)).collect();
        let existing_refs: Vec<&str> = existing.iter().map(String::as_str).collect();
        let plan = plan_classes(&classes, &existing_refs);
        let mut seen: HashSet<String> = existing.iter().map(|e| e.to_lowercase()).collect();
        for t in &plan.tags {
            prop_assert!(seen.insert(t.tag.to_lowercase()), "duplicate tag {}", t.tag);
            if let Some(g) = &t.group {
                prop_assert_eq!(plan.groups.iter().filter(|x| &x.name == g).count(), 1);
            }
        }
        let mut group_keys = HashSet::new();
        for g in &plan.groups {
            prop_assert!(group_keys.insert(g.name.to_lowercase()));
        }
        for n in &names {
            if split_class(n).is_none() {
                continue;
            }
            let mapped = plan.tag_for(n).is_some();
            let group_only = plan
                .groups
                .iter()
                .any(|g| g.name.eq_ignore_ascii_case(n.trim()));
            prop_assert!(mapped || group_only, "{n:?} is lost");
        }
    }
}
