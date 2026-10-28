//! Lookup ranking properties (P10 plan Rev 2 §2.8, T16). The ranked list
//! equals a naive reference ranking, is independent of input order, hides
//! inactive entries unless typed exactly, and the work is bounded by
//! counting comparisons (timing lives in the release `grid-bench`, S9).

use cl_model::lookup::{LookupEntry, LookupIndex, MatchTier};
use proptest::prelude::*;

const WORDS: [&str; 10] = [
    "Materials",
    "Plants",
    "Mulch",
    "stone",
    "Fuel",
    "Hollister",
    "Family",
    "Fall",
    "bed",
    "51",
];

fn entry() -> impl Strategy<Value = (Option<u16>, Vec<usize>, bool, bool)> {
    (
        proptest::option::of(5000u16..5200),
        proptest::collection::vec(0..WORDS.len(), 1..4),
        any::<bool>(),
        any::<bool>(),
    )
}

fn build(raw: &[(Option<u16>, Vec<usize>, bool, bool)]) -> Vec<LookupEntry> {
    raw.iter()
        .enumerate()
        .map(|(i, (num, words, colon, inactive))| {
            let sep = if *colon { ":" } else { " " };
            let name = words
                .iter()
                .map(|w| WORDS[*w])
                .collect::<Vec<_>>()
                .join(sep);
            LookupEntry {
                id: u64::try_from(i).unwrap(),
                number: num.map(|n| n.to_string()),
                full_name: name,
                active: !inactive,
            }
        })
        .collect()
}

fn is_break(c: char) -> bool {
    c.is_whitespace() || matches!(c, ':' | '·' | '&' | '-' | '/' | ',' | '(' | ')' | '.')
}

/// The specification, written plainly.
fn naive_tier(e: &LookupEntry, q: &str) -> Option<MatchTier> {
    let name = e.full_name.to_lowercase();
    let number = e.number.as_deref().map(str::to_lowercase);
    if name == q || number.as_deref() == Some(q) {
        return Some(MatchTier::Exact);
    }
    if number.as_deref().is_some_and(|n| n.starts_with(q)) {
        return Some(MatchTier::Number);
    }
    if name.starts_with(q) {
        return Some(MatchTier::NamePrefix);
    }
    let segments: Vec<&str> = name.split(':').skip(1).map(str::trim_start).collect();
    if segments.iter().any(|s| s.starts_with(q)) {
        return Some(MatchTier::SegmentPrefix);
    }
    let chars: Vec<(usize, char)> = name.char_indices().collect();
    let word_start = chars
        .windows(2)
        .filter(|w| is_break(w[0].1) && !is_break(w[1].1))
        .any(|w| name[w[1].0..].starts_with(q));
    if word_start {
        return Some(MatchTier::WordPrefix);
    }
    name.contains(q).then_some(MatchTier::Substring)
}

fn naive_rank(entries: &[LookupEntry], query: &str, limit: usize) -> Vec<u64> {
    let q = query.trim().to_lowercase();
    let mut hits: Vec<(MatchTier, &LookupEntry)> = entries
        .iter()
        .filter_map(|e| {
            let t = if q.is_empty() {
                Some(MatchTier::All)
            } else {
                naive_tier(e, &q)
            }?;
            (e.active || (!q.is_empty() && t == MatchTier::Exact)).then_some((t, e))
        })
        .collect();
    hits.sort_by(|(ta, a), (tb, b)| {
        let (an, bn) = (a.full_name.to_lowercase(), b.full_name.to_lowercase());
        let (anum, bnum) = (
            a.number.as_deref().map(str::to_lowercase),
            b.number.as_deref().map(str::to_lowercase),
        );
        let within = match ta {
            MatchTier::Number => anum.cmp(&bnum),
            MatchTier::NamePrefix => an.len().cmp(&bn.len()),
            // The full list: numbered entries by number value, then the rest.
            MatchTier::All => match (&a.number, &b.number) {
                (Some(x), Some(y)) => x.parse::<u64>().unwrap().cmp(&y.parse::<u64>().unwrap()),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            },
            _ => std::cmp::Ordering::Equal,
        };
        ta.cmp(tb)
            .then(within)
            .then_with(|| an.cmp(&bn))
            .then_with(|| a.full_name.cmp(&b.full_name))
            .then_with(|| anum.cmp(&bnum))
            .then_with(|| a.id.cmp(&b.id))
    });
    hits.into_iter().take(limit).map(|(_, e)| e.id).collect()
}

fn query() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        (0..WORDS.len()).prop_map(|w| WORDS[w].to_string()),
        (0..WORDS.len(), 1usize..4).prop_map(|(w, n)| WORDS[w].chars().take(n).collect()),
        "[a-z0-9 :]{1,4}",
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    #[test]
    fn matches_the_reference_ranking(raw in proptest::collection::vec(entry(), 0..40), q in query(), limit in 1usize..12) {
        let entries = build(&raw);
        let ix = LookupIndex::new(entries.clone());
        let got: Vec<u64> = ix.rank(&q, limit).iter().map(|r| r.id).collect();
        prop_assert_eq!(got, naive_rank(&entries, &q, limit), "{:?}", q);
    }

    #[test]
    fn input_order_does_not_matter(raw in proptest::collection::vec(entry(), 0..40), q in query()) {
        let entries = build(&raw);
        let mut reversed = entries.clone();
        reversed.reverse();
        let a: Vec<u64> = LookupIndex::new(entries).rank(&q, 50).iter().map(|r| r.id).collect();
        let b: Vec<u64> = LookupIndex::new(reversed).rank(&q, 50).iter().map(|r| r.id).collect();
        prop_assert_eq!(a, b);
    }

    #[test]
    fn inactive_entries_only_when_exact(raw in proptest::collection::vec(entry(), 0..40), q in query()) {
        let entries = build(&raw);
        let ix = LookupIndex::new(entries.clone());
        for r in ix.rank(&q, 100) {
            let e = &entries[r.index];
            prop_assert_eq!(e.id, r.id);
            prop_assert!(e.active || r.tier == MatchTier::Exact);
        }
    }
}

/// The plan's examples (T16).
#[test]
fn plan_examples() {
    let entries = vec![
        LookupEntry {
            id: 1,
            number: Some("5110".into()),
            full_name: "Materials:Mulch & stone".into(),
            active: true,
        },
        LookupEntry {
            id: 2,
            number: Some("5100".into()),
            full_name: "Materials:Plants".into(),
            active: true,
        },
        LookupEntry {
            id: 3,
            number: None,
            full_name: "Hollister Family:Fall bed install".into(),
            active: true,
        },
        LookupEntry {
            id: 4,
            number: None,
            full_name: "Hollister Family".into(),
            active: true,
        },
    ];
    let ix = LookupIndex::new(entries);
    let first = |q: &str| ix.rank(q, 1).first().map(|r| r.id);
    assert_eq!(first("51"), Some(2), "5100 before 5110");
    assert_eq!(first("5110"), Some(1), "exact number first");
    assert_eq!(first("Holl"), Some(4), "the customer before its project");
    assert_eq!(first("Fall"), Some(3));
    assert_eq!(first("Mulch"), Some(1));
}

/// 20,000 entries: the partial sort keeps the comparison count linear.
#[test]
fn twenty_thousand_entries_need_linear_comparisons() {
    let entries: Vec<LookupEntry> = (0..20_000u64)
        .map(|i| LookupEntry {
            id: i,
            number: (i % 3 != 0).then(|| format!("{}", 10_000 + i)),
            full_name: format!(
                "{}:{} {i}",
                WORDS[usize::try_from(i % 10).unwrap()],
                WORDS[usize::try_from(i / 10 % 10).unwrap()]
            ),
            active: i % 17 != 0,
        })
        .collect();
    let n = u64::try_from(entries.len()).unwrap();
    let ix = LookupIndex::new(entries);
    for q in ["", "m", "mulch", "1", "100", "stone 1", "zzz"] {
        let (r, comparisons) = ix.rank_counted(q, 8);
        assert!(r.len() <= 8);
        assert!(comparisons <= 8 * n, "{q:?}: {comparisons} comparisons");
    }
}
