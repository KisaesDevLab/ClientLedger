//! The names a section's filters pick from (accounts, names, items, tags)
//! and the tag groups a "Tag" grouping needs: read once, when a filter or
//! grouping first asks. Ids are kept as the engine's text (the definition
//! stores them so, `filters.N.ids`).

use super::BuilderScreen;
use crate::screens::invoice::engine::{self, Job};

/// One pick list: (id text, caption).
pub(crate) type List = Vec<(String, String)>;

/// Which list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Which {
    /// Accounts ("4000 · Sales").
    Accounts,
    /// Clients and vendors.
    Contacts,
    /// Items.
    Items,
    /// Tags.
    Tags,
    /// Tag groups (group by tag).
    TagGroups,
}

impl Which {
    /// The list an id filter on `field` picks from.
    pub(crate) fn for_field(field: &str) -> Option<Which> {
        Some(match field {
            "account" => Which::Accounts,
            "contact" => Which::Contacts,
            "item" => Which::Items,
            "tag" => Which::Tags,
            _ => return None,
        })
    }

    const fn index(self) -> usize {
        match self {
            Which::Accounts => 0,
            Which::Contacts => 1,
            Which::Items => 2,
            Which::Tags => 3,
            Which::TagGroups => 4,
        }
    }
}

/// The lists, and the reads under way.
#[derive(Default)]
pub(crate) struct Lists {
    lists: [Option<List>; 5],
    jobs: [Option<Job<List>>; 5],
}

impl Lists {
    /// The list, once read.
    pub(crate) fn get(&self, w: Which) -> Option<&List> {
        self.lists[w.index()].as_ref()
    }

    /// True while a read is under way.
    pub(crate) fn busy(&self) -> bool {
        self.jobs.iter().any(Option::is_some)
    }

    /// Takes finished reads. A failed read leaves an empty list; the
    /// filter still shows the ids it has.
    pub(crate) fn poll(&mut self) {
        for i in 0..self.jobs.len() {
            if let Some(r) = engine::poll(&mut self.jobs[i]) {
                self.lists[i] = Some(r.unwrap_or_default());
            }
        }
    }
}

fn sorted(mut v: List) -> List {
    v.sort_by_key(|(_, n)| n.to_lowercase());
    v
}

fn accounts_of(all: &[cl_api::Account]) -> List {
    sorted(
        all.iter()
            .filter(|a| a.is_active)
            .map(|a| {
                let caption = match a.fields.account_number.as_deref().filter(|n| !n.is_empty()) {
                    Some(n) => format!("{n} \u{b7} {}", a.fields.name),
                    None => a.fields.name.clone(),
                };
                (a.id.to_string(), caption)
            })
            .collect(),
    )
}

/// Starts reading `w` (once).
pub(crate) fn ensure(s: &mut BuilderScreen, w: Which) {
    let i = w.index();
    if s.lists.lists[i].is_some() || s.lists.jobs[i].is_some() {
        return;
    }
    let link = &s.link;
    let group = s.group.as_ref();
    s.lists.jobs[i] = Some(match w {
        Which::Accounts => engine::read(
            link,
            group,
            |d| {
                Ok(accounts_of(
                    &d.chart.clone().unwrap_or_else(crate::company::demo::chart),
                ))
            },
            |s| async move {
                let all = s
                    .accounts()
                    .list_all(&cl_api::AccountFilter::default())
                    .await?;
                Ok(accounts_of(&all))
            },
        ),
        Which::Contacts => engine::read(
            link,
            group,
            |_| Ok(Vec::new()),
            |s| async move {
                let f = cl_api::ContactFilter {
                    is_active: Some(true),
                    ..cl_api::ContactFilter::default()
                };
                Ok(sorted(
                    s.contacts()
                        .list_all(&f)
                        .await?
                        .into_iter()
                        .map(|c| (c.id.to_string(), c.fields.display_name))
                        .collect(),
                ))
            },
        ),
        Which::Items => engine::read(
            link,
            group,
            |d| {
                Ok(sorted(
                    d.items
                        .clone()
                        .unwrap_or_else(crate::company::demo::lists::items)
                        .into_iter()
                        .filter(|i| i.is_active)
                        .map(|i| (i.id.to_string(), i.fields.name))
                        .collect(),
                ))
            },
            |s| async move {
                Ok(sorted(
                    s.items()
                        .list_all(&cl_api::ItemFilter::default())
                        .await?
                        .into_iter()
                        .filter(|i| i.is_active)
                        .map(|i| (i.id.to_string(), i.fields.name))
                        .collect(),
                ))
            },
        ),
        Which::Tags => engine::read(
            link,
            group,
            |d| {
                Ok(sorted(
                    d.tags
                        .clone()
                        .unwrap_or_else(crate::company::demo::lists::tags)
                        .into_iter()
                        .filter(|t| t.is_active)
                        .map(|t| (t.id.to_string(), t.fields.name))
                        .collect(),
                ))
            },
            |s| async move {
                Ok(sorted(
                    s.tags()
                        .list_all(&cl_api::TagFilter::default())
                        .await?
                        .into_iter()
                        .filter(|t| t.is_active)
                        .map(|t| (t.id.to_string(), t.fields.name))
                        .collect(),
                ))
            },
        ),
        Which::TagGroups => engine::read(
            link,
            group,
            |d| {
                Ok(sorted(
                    d.tag_groups
                        .clone()
                        .unwrap_or_else(crate::company::demo::lists::tag_groups)
                        .into_iter()
                        .map(|g| (g.id.to_string(), g.fields.name))
                        .collect(),
                ))
            },
            |s| async move {
                Ok(sorted(
                    s.tags()
                        .groups()
                        .await?
                        .into_iter()
                        .map(|g| (g.id.to_string(), g.fields.name))
                        .collect(),
                ))
            },
        ),
    });
}

/// The caption of `id` in `w`, else a short placeholder (an id never
/// shows, plan §4).
pub(crate) fn caption_of(s: &BuilderScreen, w: Which, id: &str) -> String {
    s.lists
        .get(w)
        .and_then(|l| l.iter().find(|(x, _)| x == id))
        .map_or_else(
            || crate::i18n::t("builder.filter.unknown_name"),
            |(_, n)| n.clone(),
        )
}
