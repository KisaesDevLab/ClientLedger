//! Type-ahead ranking for account, name and item pickers (P10 plan §2.7).
//!
//! One deterministic ranking shared by grid pickers, header autofill fields
//! and P11's list filters. Matching is case-insensitive. A query matches an
//! entry in the first of these tiers that applies (P10 plan Rev 2 §2.8):
//!
//! 1. [`MatchTier::Exact`]: the query is exactly the number or full name;
//! 2. [`MatchTier::Number`]: the account number starts with the query
//!    (`51` finds `5100` and `5110`), ties by number (5100 first);
//! 3. [`MatchTier::NamePrefix`]: the full name starts with it, ties by the
//!    shorter name (`Holl` offers `Hollister Family` before its project);
//! 4. [`MatchTier::SegmentPrefix`]: a later `Parent:Child` segment starts
//!    with it (`Mulch` finds `Materials:Mulch & stone`, `Fall` the project);
//! 5. [`MatchTier::WordPrefix`]: a later word starts with it (`stone`);
//! 6. [`MatchTier::Substring`]: it appears anywhere in the name.
//!
//! Inactive entries are left out unless the query is exactly their name or
//! number. Remaining ties sort by name, then number, then id, so the same
//! list and query always give the same order. Lower-casing happens once,
//! when the index is built; a query costs one pass over the entries plus a
//! partial sort, and [`LookupIndex::rank_counted`] reports the comparisons
//! so tests can bound the work without timing it.

use std::cell::Cell;
use std::cmp::Ordering;

/// One pickable entry, as the screen loaded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupEntry {
    /// The screen's id for the entry (opaque here).
    pub id: u64,
    /// Account or tag number (Q-270), when the list has numbers and the
    /// entry has one.
    pub number: Option<String>,
    /// Full name, with `:` between hierarchy levels (`Materials:Plants`).
    pub full_name: String,
    /// Inactive entries are hidden unless typed exactly.
    pub active: bool,
}

/// How a query matched (lower is better).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchTier {
    /// Exactly the number or the full name.
    Exact,
    /// Account number prefix.
    Number,
    /// Full-name prefix.
    NamePrefix,
    /// Prefix of a later `Parent:Child` segment.
    SegmentPrefix,
    /// Prefix of a later word.
    WordPrefix,
    /// Anywhere in the name.
    Substring,
    /// No query: every active entry (the full list): numbered entries by
    /// number (`900` before `1000`), then the rest by name.
    All,
}

/// One ranked match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ranked {
    /// The entry's id.
    pub id: u64,
    /// The entry's position in the list given to [`LookupIndex::new`].
    pub index: usize,
    /// How it matched.
    pub tier: MatchTier,
}

#[derive(Debug, Clone)]
struct Indexed {
    entry: LookupEntry,
    name_lc: String,
    number_lc: Option<String>,
    /// Byte offsets (in `name_lc`) where a segment after the first starts.
    segments: Vec<usize>,
    /// Byte offsets (in `name_lc`) where a later word starts.
    words: Vec<usize>,
}

/// Characters that end a word inside a name.
fn is_word_break(c: char) -> bool {
    c.is_whitespace() || matches!(c, ':' | '·' | '&' | '-' | '/' | ',' | '(' | ')' | '.')
}

impl Indexed {
    fn new(entry: LookupEntry) -> Indexed {
        let name_lc = entry.full_name.to_lowercase();
        let number_lc = entry.number.as_deref().map(str::to_lowercase);
        let mut segments = Vec::new();
        let mut words = Vec::new();
        let mut prev: Option<char> = None;
        for (i, c) in name_lc.char_indices() {
            if let Some(p) = prev {
                if p == ':' && !c.is_whitespace() {
                    segments.push(i);
                } else if is_word_break(p) && !is_word_break(c) {
                    words.push(i);
                }
            }
            prev = Some(c);
        }
        // `Materials: Plants`: a space after the colon still starts a segment.
        let mut extra = Vec::new();
        for (i, _) in name_lc.match_indices(':') {
            let rest = &name_lc[i + 1..];
            let skip = rest.len() - rest.trim_start().len();
            if skip > 0 && skip < rest.len() {
                extra.push(i + 1 + skip);
            }
        }
        for e in extra {
            if !segments.contains(&e) {
                segments.push(e);
                words.retain(|w| *w != e);
            }
        }
        segments.sort_unstable();
        Indexed {
            entry,
            name_lc,
            number_lc,
            segments,
            words,
        }
    }

    fn tier(&self, q: &str) -> Option<MatchTier> {
        if self.exact(q) {
            return Some(MatchTier::Exact);
        }
        if self.number_lc.as_deref().is_some_and(|n| n.starts_with(q)) {
            return Some(MatchTier::Number);
        }
        let name = self.name_lc.as_str();
        if name.starts_with(q) {
            return Some(MatchTier::NamePrefix);
        }
        if self.segments.iter().any(|&i| name[i..].starts_with(q)) {
            return Some(MatchTier::SegmentPrefix);
        }
        if self.words.iter().any(|&i| name[i..].starts_with(q)) {
            return Some(MatchTier::WordPrefix);
        }
        name.contains(q).then_some(MatchTier::Substring)
    }

    fn exact(&self, q: &str) -> bool {
        self.name_lc == q || self.number_lc.as_deref() == Some(q)
    }
}

/// A list prepared for type-ahead.
#[derive(Debug, Clone, Default)]
pub struct LookupIndex {
    items: Vec<Indexed>,
}

impl LookupIndex {
    /// Index a list (lower-cases every name and number once).
    #[must_use]
    pub fn new(entries: Vec<LookupEntry>) -> LookupIndex {
        LookupIndex {
            items: entries.into_iter().map(Indexed::new).collect(),
        }
    }

    /// Number of entries, active or not.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// No entries at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The entry at `index` (as given to [`LookupIndex::new`]).
    #[must_use]
    pub fn entry(&self, index: usize) -> Option<&LookupEntry> {
        self.items.get(index).map(|i| &i.entry)
    }

    /// The entry with this id.
    #[must_use]
    pub fn by_id(&self, id: u64) -> Option<&LookupEntry> {
        self.items.iter().map(|i| &i.entry).find(|e| e.id == id)
    }

    fn order(&self, a: &Ranked, b: &Ranked) -> Ordering {
        let (x, y) = (&self.items[a.index], &self.items[b.index]);
        let by_tier = match a.tier {
            MatchTier::Number => x.number_lc.cmp(&y.number_lc),
            MatchTier::NamePrefix => x.name_lc.len().cmp(&y.name_lc.len()),
            MatchTier::All => match (&x.number_lc, &y.number_lc) {
                (Some(a), Some(b)) => crate::lists::accounts::number_cmp(a, b),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
            _ => Ordering::Equal,
        };
        a.tier
            .cmp(&b.tier)
            .then(by_tier)
            .then_with(|| x.name_lc.cmp(&y.name_lc))
            .then_with(|| x.entry.full_name.cmp(&y.entry.full_name))
            .then_with(|| x.number_lc.cmp(&y.number_lc))
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.index.cmp(&b.index))
    }

    /// The best `limit` matches for `query`, best first. An empty query
    /// lists active entries by number, then name (as [`MatchTier::All`]).
    #[must_use]
    pub fn rank(&self, query: &str, limit: usize) -> Vec<Ranked> {
        self.rank_counted(query, limit).0
    }

    /// [`LookupIndex::rank`], plus the number of ordering comparisons it
    /// made (the work bound tests use instead of wall time).
    #[must_use]
    pub fn rank_counted(&self, query: &str, limit: usize) -> (Vec<Ranked>, u64) {
        if limit == 0 {
            return (Vec::new(), 0);
        }
        let q = query.trim().to_lowercase();
        let mut found: Vec<Ranked> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let tier = if q.is_empty() {
                    Some(MatchTier::All)
                } else {
                    item.tier(&q)
                }?;
                (item.entry.active || (!q.is_empty() && item.exact(&q))).then_some(Ranked {
                    id: item.entry.id,
                    index,
                    tier,
                })
            })
            .collect();
        let comparisons = Cell::new(0u64);
        let cmp = |a: &Ranked, b: &Ranked| {
            comparisons.set(comparisons.get() + 1);
            self.order(a, b)
        };
        if found.len() > limit {
            found.select_nth_unstable_by(limit - 1, cmp);
            found.truncate(limit);
        }
        found.sort_unstable_by(cmp);
        (found, comparisons.get())
    }

    /// The single entry whose name or number is exactly `query`
    /// (case-insensitive), when exactly one is. Tab on such text resolves
    /// the cell without opening the list.
    #[must_use]
    pub fn exact(&self, query: &str) -> Option<&LookupEntry> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return None;
        }
        let mut hits = self.items.iter().filter(|i| i.exact(&q));
        let first = hits.next()?;
        hits.next().is_none().then_some(&first.entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: u64, number: Option<&str>, name: &str) -> LookupEntry {
        LookupEntry {
            id,
            number: number.map(str::to_string),
            full_name: name.to_string(),
            active: true,
        }
    }

    fn accounts() -> LookupIndex {
        let mut inactive = e(9, Some("6900"), "Old truck");
        inactive.active = false;
        LookupIndex::new(vec![
            e(1, Some("5100"), "Materials:Plants"),
            e(2, Some("5110"), "Materials:Mulch & stone"),
            e(3, Some("6200"), "Fuel"),
            e(4, Some("6210"), "Equipment rental"),
            e(5, None, "Hollister Family:Fall bed install"),
            e(6, Some("1000"), "Mulch fund"),
            inactive,
        ])
    }

    fn ids(ix: &LookupIndex, q: &str) -> Vec<(u64, MatchTier)> {
        ix.rank(q, 10).into_iter().map(|r| (r.id, r.tier)).collect()
    }

    #[test]
    fn tiers_in_order() {
        let ix = accounts();
        assert_eq!(
            ids(&ix, "51"),
            [(1, MatchTier::Number), (2, MatchTier::Number)],
            "number ties sort by number: 5100 before 5110"
        );
        assert_eq!(ids(&ix, "5110"), [(2, MatchTier::Exact)]);
        assert_eq!(ids(&ix, "fuel"), [(3, MatchTier::Exact)]);
        assert_eq!(
            ids(&ix, "mulch"),
            [(6, MatchTier::NamePrefix), (2, MatchTier::SegmentPrefix)]
        );
        assert_eq!(ids(&ix, "Mulch")[1], (2, MatchTier::SegmentPrefix));
        assert_eq!(ids(&ix, "stone"), [(2, MatchTier::WordPrefix)]);
        assert_eq!(
            ids(&ix, "ulch"),
            [(2, MatchTier::Substring), (6, MatchTier::Substring)]
        );
        assert_eq!(ids(&ix, "fall"), [(5, MatchTier::SegmentPrefix)]);
        assert_eq!(ids(&ix, "bed"), [(5, MatchTier::WordPrefix)]);
        assert_eq!(ids(&ix, "Holl"), [(5, MatchTier::NamePrefix)]);
        assert_eq!(ids(&ix, "zzz"), []);
    }

    #[test]
    fn exact_beats_prefix_and_shorter_names_lead() {
        let ix = LookupIndex::new(vec![
            e(1, None, "Hollister Family:Fall bed install"),
            e(2, None, "Hollister Family"),
            e(3, Some("5100"), "Materials:Plants"),
            e(4, None, "5100 Ranch"),
        ]);
        assert_eq!(
            ids(&ix, "Holl"),
            [(2, MatchTier::NamePrefix), (1, MatchTier::NamePrefix)],
            "the customer before its project"
        );
        assert_eq!(ids(&ix, "fall"), [(1, MatchTier::SegmentPrefix)]);
        assert_eq!(
            ids(&ix, "5100"),
            [(3, MatchTier::Exact), (4, MatchTier::NamePrefix)]
        );
        let (r, n) = ix.rank_counted("holl", 1);
        assert_eq!(r.len(), 1);
        assert!(n >= 1);
        assert_eq!(ix.rank_counted("holl", 0), (Vec::new(), 0));
    }

    #[test]
    fn number_beats_name() {
        let ix = LookupIndex::new(vec![e(1, None, "62 Ranch"), e(2, Some("6200"), "Fuel")]);
        assert_eq!(
            ids(&ix, "62"),
            [(2, MatchTier::Number), (1, MatchTier::NamePrefix)]
        );
    }

    #[test]
    fn inactive_entries_need_an_exact_match() {
        let ix = accounts();
        assert_eq!(ids(&ix, "old"), []);
        assert_eq!(ids(&ix, "69"), []);
        assert_eq!(ids(&ix, "old truck"), [(9, MatchTier::Exact)]);
        assert_eq!(ids(&ix, " OLD TRUCK "), [(9, MatchTier::Exact)]);
        assert_eq!(ids(&ix, "6900"), [(9, MatchTier::Exact)]);
        assert!(ids(&ix, "").iter().all(|(id, _)| *id != 9));
    }

    #[test]
    fn empty_query_lists_active_by_number_then_name_and_limit_applies() {
        let ix = accounts();
        let all: Vec<u64> = ix.rank("", 100).iter().map(|r| r.id).collect();
        assert_eq!(all, [6, 1, 2, 3, 4, 5], "by number; the unnumbered last");
        let two: Vec<u64> = ix.rank("", 2).iter().map(|r| r.id).collect();
        assert_eq!(two, [6, 1]);
        // Numbers compare as numbers: 900 before 1000.
        let mixed = LookupIndex::new(vec![
            e(1, Some("1000"), "Checking"),
            e(2, Some("900"), "Petty cash"),
            e(3, None, "Brokerage"),
            e(4, None, "Annex"),
        ]);
        let all: Vec<u64> = mixed.rank("", 10).iter().map(|r| r.id).collect();
        assert_eq!(all, [2, 1, 4, 3]);
        assert_eq!(ix.rank("m", 0), Vec::<Ranked>::new());
        assert_eq!(ix.len(), 7);
        assert!(!ix.is_empty());
        assert!(LookupIndex::default().is_empty());
    }

    #[test]
    fn ranking_is_deterministic_regardless_of_input_order() {
        let a = accounts();
        let mut rev: Vec<LookupEntry> = (0..a.len()).filter_map(|i| a.entry(i).cloned()).collect();
        rev.reverse();
        let b = LookupIndex::new(rev);
        for q in ["", "m", "5", "a", "mulch", "e"] {
            let x: Vec<u64> = a.rank(q, 50).iter().map(|r| r.id).collect();
            let y: Vec<u64> = b.rank(q, 50).iter().map(|r| r.id).collect();
            assert_eq!(x, y, "{q:?}");
        }
    }

    #[test]
    fn segments_after_a_spaced_colon() {
        let ix = LookupIndex::new(vec![e(1, None, "Materials: Plants")]);
        assert_eq!(ids(&ix, "plants"), [(1, MatchTier::SegmentPrefix)]);
    }

    #[test]
    fn exact_lookup() {
        let ix = accounts();
        assert_eq!(ix.exact("fuel").map(|e| e.id), Some(3));
        assert_eq!(ix.exact("6210").map(|e| e.id), Some(4));
        assert_eq!(ix.exact("fu"), None);
        assert_eq!(ix.exact(""), None);
        let dup = LookupIndex::new(vec![e(1, None, "Fuel"), e(2, None, "fuel")]);
        assert_eq!(dup.exact("FUEL"), None, "ambiguous");
        assert_eq!(
            ix.by_id(2).map(|e| e.full_name.as_str()),
            Some("Materials:Mulch & stone")
        );
        assert_eq!(ix.by_id(99), None);
    }

    /// Tags carry their number in `number` (Q-270), so tiers 1 and 2 find a
    /// tag by its exact number and by a number prefix.
    #[test]
    fn tags_are_found_by_number() {
        let ix = LookupIndex::new(vec![
            e(1, Some("10"), "Lake Area"),
            e(2, Some("100"), "Hill Crew"),
            e(3, Some("2"), "North"),
            e(4, None, "10th Street"),
            e(5, None, "Spring"),
        ]);
        assert_eq!(
            ids(&ix, "10"),
            [
                (1, MatchTier::Exact),
                (2, MatchTier::Number),
                (4, MatchTier::NamePrefix)
            ]
        );
        assert_eq!(
            ids(&ix, "1"),
            [
                (1, MatchTier::Number),
                (2, MatchTier::Number),
                (4, MatchTier::NamePrefix)
            ],
            "number prefix, ties by number"
        );
        assert_eq!(ix.exact("10").map(|t| t.id), Some(1));
        assert_eq!(ix.exact("lake area").map(|t| t.id), Some(1));
        let all: Vec<u64> = ix.rank("", 10).iter().map(|r| r.id).collect();
        assert_eq!(all, [3, 1, 2, 4, 5], "by number, then the unnumbered");
    }

    #[test]
    fn unicode_case_folding() {
        let ix = LookupIndex::new(vec![e(1, None, "Ölwechsel GmbH"), e(2, None, "Café Ruiz")]);
        assert_eq!(ids(&ix, "öl"), [(1, MatchTier::NamePrefix)]);
        assert_eq!(ids(&ix, "RUIZ"), [(2, MatchTier::WordPrefix)]);
        assert_eq!(ids(&ix, "CAFÉ"), [(2, MatchTier::NamePrefix)]);
    }
}
