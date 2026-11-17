//! P29 WP8 (U-215, `1099-thresholds-v1`): the company's own 1099 threshold
//! per box. An Admin sets and clears amounts (sent and read as decimal
//! strings); a bookkeeper is refused with 403 `OWNER_REQUIRED`.

use std::collections::BTreeMap;

use cl_api::{ApiError, THRESHOLDS_1099, Thresholds1099};
use cl_core::Money;

use crate::common::{key, m};
use crate::support::Harness;

fn amount_of(t: &Thresholds1099, code: &str) -> Option<Money> {
    t.boxes
        .iter()
        .find(|b| b.code == code)
        .unwrap_or_else(|| panic!("box {code} listed"))
        .amount
}

/// Round trip in tenant B (`h.s3()`): set NEC-1 and MISC-2, read them back
/// exactly, clear them (back to the year table) before anything is
/// asserted, so no other 1099 test sees the change.
pub fn admin_sets_and_clears_thresholds(h: &Harness) {
    if !h.require_cap(THRESHOLDS_1099, "U-215") {
        return;
    }
    let s = h.s3();
    let before = h.run(s.form1099().thresholds()).expect("read");
    let mut set = BTreeMap::new();
    set.insert("NEC-1".to_owned(), Some(m("1234.56")));
    set.insert("MISC-2".to_owned(), Some(m("0.00")));
    let after_set = h.run(s.form1099().set_thresholds(&set, key()));
    let read_back = h.run(s.form1099().thresholds());
    let mut clear = BTreeMap::new();
    clear.insert("NEC-1".to_owned(), None);
    clear.insert("MISC-2".to_owned(), None);
    let cleared = h.run(s.form1099().set_thresholds(&clear, key()));

    let cleared = cleared.expect("clear");
    assert_eq!(amount_of(&cleared, "NEC-1"), None);
    assert_eq!(amount_of(&cleared, "MISC-2"), None);
    let after_set = after_set.expect("set");
    let read_back = read_back.expect("read back");
    assert_eq!(after_set, read_back, "PUT answers as GET");
    assert_eq!(amount_of(&read_back, "NEC-1"), Some(m("1234.56")));
    assert_eq!(amount_of(&read_back, "MISC-2"), Some(m("0.00")));
    let nec = read_back
        .boxes
        .iter()
        .find(|b| b.code == "NEC-1")
        .expect("NEC-1");
    assert_eq!(
        nec.effective,
        Some(m("1234.56")),
        "the company amount applies"
    );
    assert!(nec.standard.is_some(), "the engine reports the year table");
    // Every box the engine listed before is still listed, and a cleared
    // box is back on the year's standard amount.
    assert_eq!(before.boxes.len(), cleared.boxes.len());
    for b in &cleared.boxes {
        if b.amount.is_none() {
            assert_eq!(b.effective, b.standard, "{}", b.code);
        }
    }
}

/// A bookkeeper may read the thresholds (company settings, view) but not
/// change them.
pub fn bookkeeper_is_refused_threshold_changes(h: &Harness) {
    if !h.require_cap(THRESHOLDS_1099, "U-215") {
        return;
    }
    let (_, bk) = h.bookkeeper();
    let mut set = BTreeMap::new();
    set.insert("NEC-1".to_owned(), Some(m("1.00")));
    let r = h.run(bk.form1099().set_thresholds(&set, key()));
    assert!(
        matches!(&r, Err(ApiError::Forbidden { code: Some(c) }) if c.as_str() == "OWNER_REQUIRED"),
        "{r:?}"
    );
    let still = h.run(h.s1.form1099().thresholds()).expect("admin read");
    assert_ne!(
        amount_of(&still, "NEC-1"),
        Some(m("1.00")),
        "nothing was written"
    );
}
