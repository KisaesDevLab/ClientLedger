use super::*;

fn tid(n: u32) -> TagId {
    TagId::parse(&format!("70000000-0000-4000-8000-{n:012}")).expect("id")
}

fn gid(n: u32) -> TagGroupId {
    TagGroupId::parse(&format!("60000000-0000-4000-8000-{n:012}")).expect("id")
}

fn group(n: u32, name: &str, single: bool, order: i64) -> GroupEntry {
    GroupEntry {
        id: gid(n),
        name: name.to_string(),
        description: None,
        single,
        required_for: BTreeSet::new(),
        order,
    }
}

fn tag(n: u32, name: &str, group: Option<u32>, active: bool) -> TagEntry {
    TagEntry {
        id: tid(n),
        name: name.to_string(),
        group: group.map(gid),
        color: None,
        description: None,
        active,
        usage: Some(0),
    }
}

/// Region (one per line, required on expense lines), Crew (any number),
/// two ungrouped tags (one inactive) and a tag whose group is gone.
fn sample() -> TagList {
    let mut region = group(1, "Region", true, 1);
    region.required_for.insert(AccountKind::Expense);
    TagList::new(
        vec![group(2, "Crew", false, 2), region],
        vec![
            tag(1, "West", Some(1), true),
            tag(2, "East", Some(1), true),
            tag(3, "Crew B", Some(2), true),
            tag(4, "Crew A", Some(2), true),
            tag(5, "Warranty", None, true),
            tag(6, "Old promo", None, false),
            tag(7, "Orphan", Some(9), true),
        ],
    )
}

#[test]
fn groups_by_order_tags_by_group_then_name_and_orphans_ungrouped() {
    let l = sample();
    let g: Vec<&str> = l.groups().iter().map(|g| g.name.as_str()).collect();
    assert_eq!(g, ["Region", "Crew"]);
    let t: Vec<&str> = l.tags().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        t,
        [
            "East",
            "West",
            "Crew A",
            "Crew B",
            "Old promo",
            "Orphan",
            "Warranty"
        ]
    );
    assert_eq!(l.tag(tid(7)).and_then(|t| t.group), None);
    assert_eq!(l.group_of(tid(1)).map(|g| g.id), Some(gid(1)));
    assert_eq!(l.len(), 7);
    assert!(!l.is_empty());
}

#[test]
fn filter_matches_tag_or_group_names_and_hides_inactive() {
    let l = sample();
    assert_eq!(l.visible(&TagFilter::default()).len(), 6);
    let all = TagFilter {
        show_inactive: true,
        ..TagFilter::default()
    };
    assert_eq!(l.visible(&all).len(), 7);
    let region = TagFilter {
        search: "REGION".into(),
        ..TagFilter::default()
    };
    assert_eq!(l.visible(&region).len(), 2);
    let crew = TagFilter {
        search: "crew a".into(),
        ..TagFilter::default()
    };
    assert_eq!(l.visible(&crew).len(), 1);
}

#[test]
fn names_are_unique_ignoring_case_inactive_included() {
    let l = sample();
    assert_eq!(l.tag_name_owner("old PROMO", None), Some(tid(6)));
    assert_eq!(l.tag_name_owner("Old promo", Some(tid(6))), None);
    assert_eq!(l.tag_name_owner(" ", None), None);
    assert_eq!(l.group_name_owner("crew", None), Some(gid(2)));
    assert_eq!(l.group_name_owner("crew", Some(gid(2))), None);
    assert_eq!(l.group_name_owner("", None), None);
    let targets: Vec<TagId> = l.merge_targets(tid(5)).iter().map(|t| t.id).collect();
    assert!(!targets.contains(&tid(5)) && !targets.contains(&tid(6)));
    assert_eq!(targets.len(), 3);
}

/// S4: a tag merges into its own group, ungrouped tags, or a group that
/// takes several tags per line, never into another one-per-line group.
#[test]
fn merge_targets_keep_one_per_line_groups_apart() {
    let l = sample();
    let ids = |src: u32| -> Vec<TagId> { l.merge_targets(tid(src)).iter().map(|t| t.id).collect() };
    // Region (one per line): its own group, Crew (any number), ungrouped,
    // and the orphan.
    let from_region = ids(1);
    assert!(from_region.contains(&tid(2)), "own group");
    assert!(
        from_region.contains(&tid(3)) && from_region.contains(&tid(4)),
        "multi group"
    );
    assert!(
        from_region.contains(&tid(5)) && from_region.contains(&tid(7)),
        "ungrouped"
    );
    // Crew and ungrouped tags never merge into Region's one-per-line tags.
    for src in [3, 5, 7] {
        let t = ids(src);
        assert!(!t.contains(&tid(1)) && !t.contains(&tid(2)), "{src}: {t:?}");
    }
    assert!(ids(3).contains(&tid(4)) && ids(3).contains(&tid(5)));
}

#[test]
fn tag_check() {
    let l = sample();
    let ok = check_tag(
        &TagDraft {
            name: " North ".into(),
            group: Some(gid(1)),
            color: "#A1B2C3".into(),
            description: "  ".into(),
        },
        &l,
        None,
    )
    .expect("ok");
    assert_eq!(ok.name, "North");
    assert_eq!(ok.color.as_deref(), Some("#a1b2c3"));
    assert_eq!(ok.description, None);
    let bad = check_tag(
        &TagDraft {
            name: "east".into(),
            group: Some(gid(9)),
            color: "red".into(),
            description: "d".repeat(DESCRIPTION_MAX + 1),
        },
        &l,
        None,
    )
    .expect_err("bad");
    assert_eq!(
        bad,
        vec![
            TagProblem::NameTaken,
            TagProblem::GroupGone,
            TagProblem::ColorInvalid,
            TagProblem::DescriptionTooLong
        ]
    );
    for p in &bad {
        assert!(p.key().as_str().starts_with("tags.problem."));
    }
    assert_eq!(bad[1].field(), TagField::Group);
    assert_eq!(bad[2].field(), TagField::Color);
    let long = TagDraft {
        name: "n".repeat(NAME_MAX + 1),
        ..TagDraft::default()
    };
    assert_eq!(
        check_tag(&long, &l, None).expect_err("long"),
        vec![TagProblem::NameTooLong]
    );
    assert_eq!(
        check_tag(&TagDraft::default(), &l, None).expect_err("blank"),
        vec![TagProblem::NameMissing]
    );
    let d = TagDraft::from_entry(l.tag(tid(2)).expect("east"));
    assert!(check_tag(&d, &l, Some(tid(2))).is_ok(), "its own name");
    assert!(is_color("#00ff00") && !is_color("#00ff0") && !is_color("00ff00x"));
}

#[test]
fn group_check() {
    let l = sample();
    let mut d = GroupDraft {
        name: "Job type".into(),
        single: true,
        ..GroupDraft::default()
    };
    d.required_for.insert(AccountKind::Income);
    let ok = check_group(&d, &l, None).expect("ok");
    assert!(ok.single);
    assert!(ok.required_for.contains(&AccountKind::Income));
    d.name = "REGION".into();
    assert_eq!(
        check_group(&d, &l, None).expect_err("taken"),
        vec![TagProblem::GroupNameTaken]
    );
    assert!(check_group(&d, &l, Some(gid(1))).is_ok());
    let back = GroupDraft::from_entry(l.group(gid(1)).expect("region"));
    assert!(back.single && back.required_for.contains(&AccountKind::Expense));
    d.description = "x".repeat(DESCRIPTION_MAX + 1);
    d.name = String::new();
    assert_eq!(
        check_group(&d, &l, None).expect_err("bad"),
        vec![TagProblem::NameMissing, TagProblem::DescriptionTooLong]
    );
}

/// Q-179, Q-180 and the engine's one tag per line (U-45).
#[test]
fn line_rules() {
    let l = sample();
    // Ungrouped and many-per-group tags are unlimited.
    assert_eq!(
        check_line_tags(&l, &[tid(5), tid(3), tid(4), tid(1)], None, true),
        Vec::<LineTagProblem>::new()
    );
    // Two of a one-per-group group.
    assert_eq!(
        check_line_tags(&l, &[tid(1), tid(2)], None, true),
        vec![LineTagProblem::OnePerGroup(gid(1))]
    );
    // Region is required on expense lines, not on income lines.
    assert_eq!(
        check_line_tags(&l, &[tid(5)], Some(AccountKind::Expense), true),
        vec![LineTagProblem::RequiredMissing(gid(1))]
    );
    assert_eq!(
        check_line_tags(&l, &[tid(5)], Some(AccountKind::Income), true),
        Vec::<LineTagProblem>::new()
    );
    assert_eq!(
        check_line_tags(&l, &[tid(2)], Some(AccountKind::Expense), false),
        Vec::<LineTagProblem>::new()
    );
    // Inactive or unknown tags.
    assert_eq!(
        check_line_tags(&l, &[tid(6)], None, true),
        vec![LineTagProblem::TagUnavailable(tid(6))]
    );
    // One tag per line until the engine keeps more.
    assert_eq!(
        check_line_tags(&l, &[tid(5), tid(3)], None, false),
        vec![LineTagProblem::TooManyForEngine]
    );
    for p in [
        LineTagProblem::OnePerGroup(gid(1)),
        LineTagProblem::RequiredMissing(gid(1)),
        LineTagProblem::TagUnavailable(tid(1)),
        LineTagProblem::TooManyForEngine,
    ] {
        assert!(p.key().as_str().starts_with("tags.line."));
    }
    assert_eq!(engine_line_tag(&[]), Ok(None));
    assert_eq!(engine_line_tag(&[tid(1)]), Ok(Some(tid(1))));
    assert_eq!(
        engine_line_tag(&[tid(1), tid(5)]),
        Err(LineTagProblem::TooManyForEngine)
    );
}

/// Q-178: the header's tags are every line's defaults.
#[test]
fn header_tags_default_to_lines() {
    let l = sample();
    // A line with nothing takes the header's tags.
    assert_eq!(
        effective_line_tags(&l, &[tid(1), tid(5)], &[]),
        [tid(1), tid(5)]
    );
    // The line's own Region tag wins over the header's.
    assert_eq!(
        effective_line_tags(&l, &[tid(1), tid(5)], &[tid(2)]),
        [tid(2), tid(5)]
    );
    // Many-per-group tags add up; no repeats.
    assert_eq!(
        effective_line_tags(&l, &[tid(3), tid(4)], &[tid(3)]),
        [tid(3), tid(4)]
    );
}

/// Q-181: a rule's tags replace the same one-per-group group and add to
/// the rest.
#[test]
fn rule_tags_replace_their_group() {
    let l = sample();
    assert_eq!(
        apply_rule_tags(&l, &[tid(1), tid(3), tid(5)], &[tid(2), tid(4)]),
        [tid(3), tid(5), tid(2), tid(4)]
    );
    assert_eq!(apply_rule_tags(&l, &[tid(5)], &[tid(5)]), [tid(5)]);
}

/// P11 acceptance: 20k-record lists filter under 100 ms.
#[test]
fn twenty_thousand_tags_filter_quickly() {
    let groups: Vec<GroupEntry> = (0..50u32)
        .map(|n| group(n, &format!("Group {n:02}"), n % 2 == 0, i64::from(n)))
        .collect();
    let tags: Vec<TagEntry> = (0..20_000u32)
        .map(|n| tag(n, &format!("Tag {n:05}"), Some(n % 50), n % 11 != 0))
        .collect();
    let list = TagList::new(groups, tags);
    let filter = TagFilter {
        show_inactive: false,
        search: "group 1".into(),
    };
    let start = std::time::Instant::now();
    let rows = list.visible(&filter);
    let all = list.visible(&TagFilter::default());
    let took = start.elapsed();
    assert!(!rows.is_empty() && !all.is_empty());
    assert!(took.as_millis() < 100, "filtering took {took:?}");
}
