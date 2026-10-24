//! Tags as owners see them (Q-270): `number · name` (`10 · Lake Area`),
//! listed by number, then name, numbered tags first. Every screen that
//! shows or picks a tag goes through these helpers so the lists agree
//! (the engine sorts numbers as text; the client compares all-digit numbers
//! as numbers, like account numbers).

use cl_api::Tag;
use cl_model::lists::tags::TagEntry;

/// The tag as every screen shows it.
#[must_use]
pub fn label(tag: &Tag) -> String {
    cl_model::tags::label(tag.fields.tag_number.as_deref(), &tag.fields.name)
}

/// The tag's sort key: number, then name.
#[must_use]
pub fn sort_key(tag: &Tag) -> cl_model::tags::TagSortKey {
    cl_model::tags::sort_key(tag.fields.tag_number.as_deref(), &tag.fields.name)
}

/// `tags` in display order.
#[must_use]
pub fn sorted<'a>(tags: impl IntoIterator<Item = &'a Tag>) -> Vec<&'a Tag> {
    let mut out: Vec<&Tag> = tags.into_iter().collect();
    out.sort_by_cached_key(|t| sort_key(t));
    out
}

/// The label of tag `id` in `tags`; empty when it is not there.
#[must_use]
pub fn label_of(tags: &[Tag], id: cl_core::ids::TagId) -> String {
    tags.iter()
        .find(|t| t.id == id)
        .map(label)
        .unwrap_or_default()
}

/// The tag as `cl_model`'s tag rules see it.
#[must_use]
pub fn entry(tag: &Tag) -> TagEntry {
    TagEntry {
        id: tag.id,
        number: tag
            .fields
            .tag_number
            .clone()
            .filter(|n| !n.trim().is_empty()),
        name: tag.fields.name.clone(),
        group: tag.fields.group_id,
        color: tag.fields.color.clone(),
        description: tag.fields.description.clone(),
        active: tag.is_active,
        usage: tag.usage_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_api::TagFields;
    use cl_core::ids::TagId;

    fn tag(n: u32, number: Option<&str>, name: &str) -> Tag {
        Tag {
            id: TagId::parse(&format!("70000000-0000-4000-8000-{n:012}")).expect("id"),
            fields: TagFields {
                name: name.to_string(),
                color: None,
                group_id: None,
                description: None,
                tag_number: number.map(str::to_string),
            },
            is_active: true,
            usage_count: None,
            sort_order: None,
            created_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn labels_and_order() {
        let tags = vec![
            tag(1, None, "Zed"),
            tag(2, Some("1000"), "Annex"),
            tag(3, Some("900"), "Lake Area"),
            tag(4, Some(""), "Alpha"),
        ];
        let shown: Vec<String> = sorted(&tags).into_iter().map(label).collect();
        assert_eq!(shown, ["900 · Lake Area", "1000 · Annex", "Alpha", "Zed"]);
        assert_eq!(label_of(&tags, tags[2].id), "900 · Lake Area");
        assert_eq!(label_of(&tags[..1], tags[2].id), "");
        assert_eq!(entry(&tags[3]).number, None);
        assert_eq!(entry(&tags[2]).number.as_deref(), Some("900"));
    }
}
