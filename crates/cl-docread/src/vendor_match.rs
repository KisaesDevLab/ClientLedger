//! Vendor matching (WP-A, step 11; plan D3 `vendor_match.rs`, S21). Runs in
//! the desktop on the extracted strings against the company's and the
//! tenant-wide vendors (U-90). Tiers, strongest first:
//! `vendor_account_number`; phone; past layouts (a similar fingerprint →
//! its contact, unless the printed name matches one other vendor on its
//! own: two vendors can send the same layout, so the name wins and
//! [`layout_check`] asks the owner to check); the engine's four
//! `matchByName` tiers
//! (`ai-name-match.ts:55-95`, ambiguous → none); address breaks ties.
//! Accounts: the vendor's default only (Q-A03r).
//!
//! Each tier that finds several vendors asks the address to break the tie
//! (one vendor's billing address agrees with the document's) and otherwise
//! passes to the next tier. Where the engine's first two name tiers take
//! the first row in list order, this module takes a unique match only, so
//! list order never decides (the plan's "ambiguous → none").
//!
//! Names are normalized with Unicode NFKC ([`fold_compat`], through
//! `unicode-normalization`), as the engine does.
//!
//! **Consumer:** the desktop reading service (WP-G); the signatures here
//! are fixed.

use crate::amount::{is_js_space, js_trim};
use crate::model::{
    Check, CheckCode, CheckValue, Confidence, FieldName, Fingerprint, Header, MAX_TEXT_CHARS,
    RuleId,
};
use crate::templates;
use cl_core::ids::ContactId;
use std::fmt;
use unicode_normalization::UnicodeNormalization;

/// One vendor the document could be from. Built by the desktop from
/// `GET /contacts` and the company's templates.
#[derive(Clone, PartialEq, Eq)]
pub struct VendorCandidate {
    pub contact: ContactId,
    pub display_name: String,
    pub company_name: Option<String>,
    /// `contacts.vendor_account_number`.
    pub account_number: Option<String>,
    pub phone: Option<String>,
    /// Billing address lines joined with `\n`.
    pub address: Option<String>,
    /// Fingerprints of this vendor's saved layout templates.
    pub layouts: Vec<Fingerprint>,
}

impl fmt::Debug for VendorCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VendorCandidate")
            .field("contact", &self.contact)
            .field("layouts", &self.layouts.len())
            .finish_non_exhaustive()
    }
}

/// Which tier matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VendorTier {
    AccountNumber,
    Phone,
    PastLayout,
    /// The engine's name tiers, 1 (exact) to 4 (loosest).
    Name(u8),
}

impl VendorTier {
    fn bp(self) -> u16 {
        match self {
            VendorTier::AccountNumber => 9_500,
            VendorTier::Phone | VendorTier::Name(1) => 9_000,
            VendorTier::PastLayout | VendorTier::Name(2) => 8_500,
            VendorTier::Name(3) => 7_500,
            VendorTier::Name(_) => 6_000,
        }
    }
}

/// Taken off when the address had to break a tie.
const TIE_BREAK_PENALTY: u16 = 500;

/// A matched vendor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VendorMatch {
    pub contact: ContactId,
    pub tier: VendorTier,
    pub confidence: Confidence,
    /// A past layout pointed at this other vendor, and the printed name
    /// won ([`layout_check`] names it).
    pub layout_of: Option<ContactId>,
}

/// The vendor the reading is from, or `None` (the desktop offers a new
/// vendor).
#[must_use]
pub fn match_vendor(
    header: &Header,
    fingerprint: Option<&Fingerprint>,
    vendors: &[VendorCandidate],
) -> Option<VendorMatch> {
    let address = header.vendor_address.as_ref().map(|a| a.value.as_str());
    let decide = |hits: Vec<usize>, tier: VendorTier| -> Option<VendorMatch> {
        let (index, tie_broken) = unique_or_tie_break(&hits, vendors, address)?;
        let bp = if tie_broken {
            tier.bp().saturating_sub(TIE_BREAK_PENALTY)
        } else {
            tier.bp()
        };
        Some(VendorMatch {
            contact: vendors[index].contact,
            tier,
            confidence: Confidence::saturating(u32::from(bp)),
            layout_of: None,
        })
    };
    // 1. The owner's account number with the vendor.
    if let Some(n) = header
        .account_number
        .as_ref()
        .map(|a| alnum_lower(&a.value))
        .filter(|n| n.len() >= 3)
    {
        let hits = indices(vendors, |v| {
            v.account_number.as_deref().map(alnum_lower).as_deref() == Some(n.as_str())
        });
        if let Some(m) = decide(hits, VendorTier::AccountNumber) {
            return Some(m);
        }
    }
    // 2. Phone.
    if let Some(p) = header
        .vendor_phone
        .as_ref()
        .and_then(|p| phone_key(&p.value))
    {
        let hits = indices(vendors, |v| {
            v.phone.as_deref().and_then(phone_key).as_deref() == Some(p.as_str())
        });
        if let Some(m) = decide(hits, VendorTier::Phone) {
            return Some(m);
        }
    }
    // 3. Past layouts: the vendors whose saved template is most similar.
    if let Some(fp) = fingerprint {
        let scores: Vec<u16> = vendors
            .iter()
            .map(|v| {
                v.layouts
                    .iter()
                    .map(|l| templates::similarity(fp, l))
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        if let Some(&best) = scores.iter().max()
            && best >= templates::MATCH_BP
        {
            let hits = (0..vendors.len()).filter(|&i| scores[i] == best).collect();
            if let Some(m) = decide(hits, VendorTier::PastLayout) {
                // Two vendors may send the same layout (one billing
                // program): a printed name that matches one other vendor
                // wins, and the owner is asked to check (review S4).
                return Some(match unique_by_name(header, vendors, address) {
                    Some(mut named) if named.contact != m.contact => {
                        named.layout_of = Some(m.contact);
                        named
                    }
                    _ => m,
                });
            }
        }
    }
    // 4. Names.
    by_name(header, vendors, &decide)
}

fn by_name(
    header: &Header,
    vendors: &[VendorCandidate],
    decide: &impl Fn(Vec<usize>, VendorTier) -> Option<VendorMatch>,
) -> Option<VendorMatch> {
    let target = header.vendor.as_ref().map(|v| v.value.as_str())?;
    name_tiers(target, vendors)
        .into_iter()
        .find_map(|(hits, tier)| decide(hits, VendorTier::Name(tier)))
}

/// The name tiers' match when it is one vendor on its own (no address
/// needed to break a tie); `None` otherwise.
fn unique_by_name(
    header: &Header,
    vendors: &[VendorCandidate],
    address: Option<&str>,
) -> Option<VendorMatch> {
    let target = header.vendor.as_ref().map(|v| v.value.as_str())?;
    for (hits, tier) in name_tiers(target, vendors) {
        match unique_or_tie_break(&hits, vendors, address) {
            Some((index, false)) => {
                let tier = VendorTier::Name(tier);
                return Some(VendorMatch {
                    contact: vendors[index].contact,
                    tier,
                    confidence: Confidence::saturating(u32::from(tier.bp())),
                    layout_of: None,
                });
            }
            Some((_, true)) => return None,
            None => {}
        }
    }
    None
}

/// The check for a name that won over a past layout (review S4): the
/// layout's vendor is named, the vendor field is marked.
#[must_use]
pub fn layout_check(
    m: &VendorMatch,
    header: &Header,
    vendors: &[VendorCandidate],
) -> Option<Check> {
    let other = m.layout_of?;
    let name = vendors.iter().find(|v| v.contact == other)?;
    let printed = header.vendor.as_ref()?;
    let text = |s: &str| CheckValue::Text(s.chars().take(MAX_TEXT_CHARS).collect());
    Some(Check {
        code: CheckCode::ReadersDisagree,
        rule: RuleId::new("vendor.layout"),
        field: Some(FieldName::Vendor),
        line: None,
        expected: Some(text(&name.display_name)),
        found: Some(text(&printed.value)),
        difference: None,
        spot: Some(printed.spot),
    })
}

fn indices(vendors: &[VendorCandidate], f: impl Fn(&VendorCandidate) -> bool) -> Vec<usize> {
    (0..vendors.len()).filter(|&i| f(&vendors[i])).collect()
}

/// One hit, or the one hit whose address agrees with the document's.
/// `(index, tie broken)`.
fn unique_or_tie_break(
    hits: &[usize],
    vendors: &[VendorCandidate],
    address: Option<&str>,
) -> Option<(usize, bool)> {
    match hits {
        [] => None,
        [one] => Some((*one, false)),
        _ => {
            let doc = address?;
            let agree: Vec<usize> = hits
                .iter()
                .copied()
                .filter(|&i| {
                    vendors[i]
                        .address
                        .as_deref()
                        .is_some_and(|a| same_address(doc, a))
                })
                .collect();
            match agree.as_slice() {
                [one] => Some((*one, true)),
                _ => None,
            }
        }
    }
}

/// The name tiers in order, each with every vendor it matches (display or
/// company name). Mirrors `matchByName`: the target, then the target with a
/// leading number stripped, each exact (tier 1, or 3 when stripped) then
/// punctuation-insensitive (tier 2, or 3); then the guarded token-subset
/// tier (4) on targets of at least three characters.
fn name_tiers(target: &str, vendors: &[VendorCandidate]) -> Vec<(Vec<usize>, u8)> {
    let mut out = Vec::new();
    if js_trim(target).is_empty() {
        return out;
    }
    let names = |v: &VendorCandidate| -> Vec<String> {
        std::iter::once(v.display_name.clone())
            .chain(v.company_name.clone())
            .collect()
    };
    let stripped = strip_leading_numeric_token(target);
    let candidates: Vec<(&str, bool)> = if !stripped.is_empty() && stripped != target {
        vec![(target, false), (stripped.as_str(), true)]
    } else {
        vec![(target, false)]
    };
    for (cand, was_stripped) in candidates {
        let canon = normalize_for_match(cand);
        out.push((
            indices(vendors, |v| {
                names(v).iter().any(|n| normalize_for_match(n) == canon)
            }),
            if was_stripped { 3 } else { 1 },
        ));
        let loose = normalize_loose(cand);
        if !loose.is_empty() {
            out.push((
                indices(vendors, |v| {
                    names(v).iter().any(|n| normalize_loose(n) == loose)
                }),
                if was_stripped { 3 } else { 2 },
            ));
        }
    }
    let loose_target = normalize_loose(if stripped.is_empty() {
        target
    } else {
        &stripped
    });
    if loose_target.chars().count() >= 3 {
        let tokens: Vec<&str> = loose_target.split(' ').filter(|t| !t.is_empty()).collect();
        if !tokens.is_empty() {
            out.push((
                indices(vendors, |v| {
                    names(v).iter().any(|n| {
                        let row = normalize_loose(n);
                        let row_tokens: Vec<&str> =
                            row.split(' ').filter(|t| !t.is_empty()).collect();
                        tokens.iter().all(|t| row_tokens.contains(t))
                    })
                }),
                4,
            ));
        }
    }
    out
}

/// `normalizeForMatch`: NFKC (see [`fold_compat`]), lower case, trimmed,
/// inner whitespace collapsed to one space.
#[must_use]
pub fn normalize_for_match(s: &str) -> String {
    let folded = fold_compat(s).to_lowercase();
    let mut out = String::new();
    let mut space = false;
    for c in js_trim(&folded).chars() {
        if is_js_space(c) {
            space = true;
        } else {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(c);
        }
    }
    out
}

/// `normalizeLoose`: [`normalize_for_match`] keeping only `a-z`, `0-9` and
/// single spaces.
#[must_use]
pub fn normalize_loose(s: &str) -> String {
    let kept: String = normalize_for_match(s)
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == ' ')
        .collect();
    kept.split(' ')
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `stripLeadingNumericToken`: `/^\s*#?\d[\d.]*[\s\-–—]+/` removed, then
/// trimmed ("6100 Office Supplies" → "Office Supplies", "401k Match"
/// unchanged).
#[must_use]
pub fn strip_leading_numeric_token(s: &str) -> String {
    let rest = s.trim_start_matches(is_js_space);
    let rest_no_hash = rest.strip_prefix('#').unwrap_or(rest);
    let mut chars = rest_no_hash.char_indices().peekable();
    let stripped = match chars.next() {
        Some((_, c)) if c.is_ascii_digit() => {
            let mut end = rest_no_hash.len();
            let mut seps = 0;
            let mut in_number = true;
            for (i, c) in chars {
                if in_number && (c.is_ascii_digit() || c == '.') {
                    continue;
                }
                in_number = false;
                if is_js_space(c) || matches!(c, '-' | '–' | '—') {
                    seps += 1;
                } else {
                    end = i;
                    break;
                }
            }
            (seps > 0).then(|| &rest_no_hash[end..])
        }
        _ => None,
    };
    js_trim(stripped.unwrap_or(s)).to_string()
}

/// Unicode NFKC, as the engine's `normalizeForMatch` applies it.
#[must_use]
pub fn fold_compat(s: &str) -> String {
    s.nfkc().collect()
}

fn alnum_lower(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The last ten digits of a US phone number.
fn phone_key(s: &str) -> Option<String> {
    let digits: String = s.chars().filter(char::is_ascii_digit).collect();
    match digits.len() {
        10 => Some(digits),
        11 if digits.starts_with('1') => Some(digits[1..].to_string()),
        _ => None,
    }
}

/// Two addresses agree when their first lines match loosely, or their ZIP
/// codes and street numbers both match.
fn same_address(a: &str, b: &str) -> bool {
    let first = |s: &str| normalize_loose(s.lines().next().unwrap_or(""));
    let (fa, fb) = (first(a), first(b));
    if !fa.is_empty() && fa == fb {
        return true;
    }
    let zip = |s: &str| -> Option<String> {
        s.split(|c: char| !c.is_ascii_digit())
            .rfind(|t| t.len() == 5)
            .map(str::to_string)
    };
    let number = |s: &str| -> Option<String> {
        let t = normalize_loose(s);
        t.split(' ')
            .next()
            .filter(|t| t.chars().all(|c| c.is_ascii_digit()) && !t.is_empty())
            .map(str::to_string)
    };
    zip(a).is_some() && zip(a) == zip(b) && number(a).is_some() && number(a) == number(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Bbox, Found, RuleId, Spot};
    use proptest::prelude::*;

    fn id(n: u8) -> ContactId {
        ContactId::parse(&format!("00000000-0000-4000-8000-0000000000{n:02x}")).unwrap()
    }

    fn vendor(n: u8, name: &str) -> VendorCandidate {
        VendorCandidate {
            contact: id(n),
            display_name: name.into(),
            company_name: None,
            account_number: None,
            phone: None,
            address: None,
            layouts: Vec::new(),
        }
    }

    fn plain(v: &str) -> Found<String> {
        Found {
            value: v.into(),
            spot: Spot {
                page: 0,
                bbox: Bbox::PAGE,
            },
            rule: RuleId::new("t"),
            confidence: Confidence::HIGH,
        }
    }

    fn header(name: &str) -> Header {
        Header {
            vendor: Some(plain(name)),
            ..Header::default()
        }
    }

    fn tier(h: &Header, vs: &[VendorCandidate]) -> Option<(ContactId, VendorTier)> {
        match_vendor(h, None, vs).map(|m| (m.contact, m.tier))
    }

    #[test]
    fn engine_helpers_match_ai_name_match_ts() {
        assert_eq!(
            normalize_for_match("  Office\u{00A0}  Supplies "),
            "office supplies"
        );
        assert_eq!(
            normalize_loose("Utilities - Electric"),
            "utilities electric"
        );
        assert_eq!(
            strip_leading_numeric_token("6100 Office Supplies"),
            "Office Supplies"
        );
        assert_eq!(strip_leading_numeric_token("1099-Contractor"), "Contractor");
        assert_eq!(strip_leading_numeric_token("#12.5 — Fees"), "Fees");
        assert_eq!(strip_leading_numeric_token("401k Match"), "401k Match");
        assert_eq!(strip_leading_numeric_token(" 7 "), "");
        assert_eq!(
            normalize_for_match("\u{FF32}iverbend \u{FB01}eld"),
            "riverbend field"
        );
        // Full NFKC: composition and compatibility forms beyond the few
        // a hand-made table had.
        assert_eq!(normalize_for_match("Cafe\u{301} \u{2167}"), "café viii");
        assert_eq!(normalize_for_match("\u{3392} Labs"), "mhz labs");
    }

    #[test]
    fn name_tiers_in_order() {
        let vs = [
            vendor(1, "Riverbend Supply Co."),
            vendor(2, "Office Supplies"),
            vendor(3, "Northside Electric LLC"),
        ];
        assert_eq!(
            tier(&header("riverbend supply co."), &vs),
            Some((id(1), VendorTier::Name(1)))
        );
        assert_eq!(
            tier(&header("Riverbend Supply Co"), &vs),
            Some((id(1), VendorTier::Name(2)))
        );
        assert_eq!(
            tier(&header("6100 Office Supplies"), &vs),
            Some((id(2), VendorTier::Name(3)))
        );
        assert_eq!(
            tier(&header("Northside"), &vs),
            Some((id(3), VendorTier::Name(4)))
        );
        assert_eq!(tier(&header("Hilltop Feed"), &vs), None);
    }

    #[test]
    fn ambiguity_is_none_unless_the_address_breaks_it() {
        let mut a = vendor(1, "Supply Depot");
        let mut b = vendor(2, "Supply Depot");
        a.address = Some("12 Elm St\nSpringfield, MO 65801".into());
        b.address = Some("900 Oak Ave\nJoplin, MO 64801".into());
        let vs = [a, b];
        assert_eq!(tier(&header("Supply Depot"), &vs), None);
        let mut h = header("Supply Depot");
        h.vendor_address = Some(plain("12 Elm Street\nSpringfield, MO 65801"));
        let m = match_vendor(&h, None, &vs).unwrap();
        assert_eq!((m.contact, m.tier), (id(1), VendorTier::Name(1)));
        assert_eq!(m.confidence.bp(), 9_000 - TIE_BREAK_PENALTY);
        // Two vendors share "supply": the subset tier stays silent.
        let vs = [vendor(1, "Supply Depot"), vendor(2, "Farm Supply")];
        assert_eq!(tier(&header("Supply"), &vs), None);
    }

    #[test]
    fn stronger_tiers_first() {
        let mut first = vendor(1, "Riverbend Supply Co.");
        let mut second = vendor(2, "Riverbend Supply");
        first.account_number = Some("RB-00417".into());
        second.phone = Some("+1 (417) 555-0142".into());
        let fp = Fingerprint {
            tokens: vec!["a".into(), "b".into()],
            aspect: 12_941,
        };
        let mut third = vendor(3, "Something Else");
        third.layouts = vec![fp.clone()];
        let vs = [first, second, third];
        let mut h = header("Riverbend Supply Co.");
        h.account_number = Some(plain("rb00417"));
        assert_eq!(tier(&h, &vs), Some((id(1), VendorTier::AccountNumber)));
        h.account_number = None;
        h.vendor_phone = Some(plain("417.555.0142"));
        assert_eq!(tier(&h, &vs), Some((id(2), VendorTier::Phone)));
        h.vendor_phone = None;
        // A past layout wins when no printed name says otherwise.
        let mut unnamed = h.clone();
        unnamed.vendor = Some(plain("Hilltop Feed"));
        let m = match_vendor(&unnamed, Some(&fp), &vs).unwrap();
        assert_eq!(
            (m.contact, m.tier, m.layout_of),
            (id(3), VendorTier::PastLayout, None)
        );
        assert_eq!(tier(&h, &vs), Some((id(1), VendorTier::Name(1))));
    }

    /// Review S4: two vendors send the same layout. The printed name that
    /// matches one other vendor wins over the past layout, with a check
    /// naming the layout's vendor; an ambiguous name leaves the layout.
    #[test]
    fn a_printed_name_beats_a_shared_layout() {
        let fp = Fingerprint {
            tokens: vec!["invoice".into(), "remit".into()],
            aspect: 12_941,
        };
        let mut a = vendor(1, "Riverbend Supply Co.");
        a.layouts = vec![fp.clone()];
        let b = vendor(2, "Hilltop Feed & Seed");
        let vs = [a, b];
        let h = header("Hilltop Feed & Seed");
        let m = match_vendor(&h, Some(&fp), &vs).unwrap();
        assert_eq!((m.contact, m.tier), (id(2), VendorTier::Name(1)));
        assert_eq!(m.layout_of, Some(id(1)));
        let c = layout_check(&m, &h, &vs).unwrap();
        assert_eq!(
            (c.code, c.field),
            (CheckCode::ReadersDisagree, Some(FieldName::Vendor))
        );
        assert!(matches!(&c.expected, Some(CheckValue::Text(t)) if t == "Riverbend Supply Co."));
        // The layout's own vendor printed: no disagreement, no check.
        let m = match_vendor(&header("Riverbend Supply Co."), Some(&fp), &vs).unwrap();
        assert_eq!(
            (m.contact, m.tier, m.layout_of),
            (id(1), VendorTier::PastLayout, None)
        );
        assert!(layout_check(&m, &h, &vs).is_none());
        // A name two vendors share: the layout stands.
        let vs = [
            vs[0].clone(),
            vendor(2, "Hilltop Feed"),
            vendor(3, "Hilltop Feed"),
        ];
        let m = match_vendor(&header("Hilltop Feed"), Some(&fp), &vs).unwrap();
        assert_eq!((m.contact, m.layout_of), (id(1), None));
    }

    #[test]
    fn debug_hides_names() {
        let text = format!("{:?}", vendor(1, "Riverbend"));
        assert!(!text.contains("Riverbend"), "{text}");
    }

    proptest! {
        /// A vendor list with the target's exact name always matches it at
        /// tier 1 when the name is unique, whatever else is in the list.
        #[test]
        fn exact_unique_name_always_matches(
            names in prop::collection::btree_set("[A-Z][a-z]{2,8}( [A-Z][a-z]{2,8})?", 1..8),
            pick in any::<prop::sample::Index>(),
        ) {
            let names: Vec<String> = names.into_iter().collect();
            let vs: Vec<VendorCandidate> = names
                .iter()
                .enumerate()
                .map(|(i, n)| vendor(u8::try_from(i).unwrap(), n))
                .collect();
            let i = pick.index(names.len());
            let m = match_vendor(&header(&names[i].to_uppercase()), None, &vs).unwrap();
            prop_assert_eq!(m.contact, id(u8::try_from(i).unwrap()));
            prop_assert_eq!(m.tier, VendorTier::Name(1));
        }
    }
}
