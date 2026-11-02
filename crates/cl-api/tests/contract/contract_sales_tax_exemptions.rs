//! Exemption certificates against the real engine (P23 plan §2.7, golden
//! 10).
//!
//! - `expired_certificate_warns` (golden 10): Cedar Hollow's certificate
//!   expired 10/31 → `EXEMPTION_EXPIRED` on an 11/20 invoice preview;
//!   Briar Creek has none → `EXEMPTION_MISSING`; Pine Bluff's expires 12/15
//!   → `EXEMPTION_EXPIRING`; Ozark (taxable) gets no warning. The expiring
//!   report as of 11/04 lists Cedar Hollow (expired) and Pine Bluff. A
//!   certificate edit with an old version is 409 `STALE`; deactivate and
//!   reactivate.

use cl_api::{
    Capabilities, ExemptionStatus, NewInvoice, SalesTaxRefusal, TaxExemptionDraft,
    sales_tax_refusal,
};
use cl_core::ids::ContactId;

use crate::common::{date, key};
use crate::contract_sales_tax_setup::{Monett, caps, monett};
use crate::support::Harness;

fn draft(c: ContactId, number: &str, expires: (i32, u32, u32)) -> TaxExemptionDraft {
    TaxExemptionDraft {
        contact_id: c,
        number: number.into(),
        state: Some("MO".into()),
        expires_on: Some(date(expires.0, expires.1, expires.2)),
        attachment_id: None,
        note: None,
    }
}

fn warnings(h: &Harness, x: &Monett, c: ContactId) -> Vec<String> {
    let new = NewInvoice::new(
        c,
        date(2026, 11, 20),
        vec![Monett::line(x.plants, "1", "10.00")],
    );
    h.run(x.s.invoices().preview(&new, None))
        .unwrap_or_else(|e| panic!("preview: {e}"))
        .notice_codes()
        .into_iter()
        .filter(|c| c.starts_with("EXEMPTION_"))
        .collect()
}

/// Golden 10.
pub fn expired_certificate_warns(h: &Harness) {
    if !caps(h) || !h.require_cap(Capabilities::TAX_EXEMPTIONS, "U-151") {
        return;
    }
    let x = monett(h, "Exemptions", true);
    let st = x.s.sales_tax();
    let cedar = h
        .run(st.create_exemption(&draft(x.cedar, "RS-1001", (2026, 10, 31)), key()))
        .expect("Cedar Hollow certificate");
    let pine = h
        .run(st.create_exemption(&draft(x.pine, "RS-1003", (2026, 12, 15)), key()))
        .expect("Pine Bluff certificate");
    assert_eq!(cedar.contact_id, x.cedar);
    assert_eq!(cedar.state.as_deref(), Some("MO"));
    assert_eq!(cedar.expires_on, Some(date(2026, 10, 31)));
    let mine = h.run(st.exemptions(Some(x.pine))).expect("Pine's");
    assert_eq!(mine.iter().map(|e| e.id).collect::<Vec<_>>(), vec![pine.id]);

    assert_eq!(warnings(h, &x, x.cedar), vec!["EXEMPTION_EXPIRED"]);
    assert_eq!(warnings(h, &x, x.briar), vec!["EXEMPTION_MISSING"]);
    assert_eq!(warnings(h, &x, x.pine), vec!["EXEMPTION_EXPIRING"]);
    assert!(warnings(h, &x, x.ozark).is_empty(), "a taxable client");

    let rows = h
        .run(st.exemptions_expiring(date(2026, 11, 4), Some(60)))
        .expect("expiring");
    let got: Vec<_> = rows.iter().map(|r| (r.contact_id, r.status)).collect();
    assert!(
        got.contains(&(x.cedar, ExemptionStatus::Expired)),
        "{got:?}"
    );
    assert!(
        got.contains(&(x.pine, ExemptionStatus::Expiring)),
        "{got:?}"
    );
    let mine = [x.cedar, x.briar, x.pine, x.ozark, x.hollister];
    assert_eq!(
        rows.iter().filter(|r| mine.contains(&r.contact_id)).count(),
        2,
        "Cedar Hollow and Pine Bluff only"
    );

    // A renewal with the version read; then the old version is STALE.
    let renewed = h
        .run(st.update_exemption(
            cedar.id,
            &draft(x.cedar, "RS-1001", (2027, 10, 31)),
            cedar.updated_at,
            key(),
        ))
        .expect("renew");
    assert_eq!(renewed.expires_on, Some(date(2027, 10, 31)));
    assert!(warnings(h, &x, x.cedar).is_empty(), "renewed");
    let stale = h
        .run(st.update_exemption(
            cedar.id,
            &draft(x.cedar, "RS-1001", (2028, 10, 31)),
            cedar.updated_at,
            key(),
        ))
        .expect_err("an old version");
    assert_eq!(sales_tax_refusal(&stale), Some(SalesTaxRefusal::Stale));
    // Deactivated, a certificate no longer counts; reactivated it does.
    let off = h
        .run(st.deactivate_exemption(pine.id, key()))
        .expect("deactivate");
    assert!(!off.is_active);
    assert_eq!(warnings(h, &x, x.pine), vec!["EXEMPTION_MISSING"]);
    let on = h
        .run(st.reactivate_exemption(pine.id, key()))
        .expect("reactivate");
    assert!(on.is_active);
    assert_eq!(warnings(h, &x, x.pine), vec!["EXEMPTION_EXPIRING"]);
}
