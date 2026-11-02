//! Unit tests of the P23 sales tax setup, owed and exemption resources
//! (wiremock; exact wire strings; P23 plan §6.3).

use super::*;
use crate::resources::testkit::{json, query, requests, session_with_caps};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const AGENCY: &str = "7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const VENDOR: &str = "7b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const STATE: &str = "7c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const CITY: &str = "7d0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const GROUP: &str = "7e0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const CLIENT: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const CERT: &str = "7f0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

const ALL: [&str; 4] = [
    Capabilities::SALES_TAX_SETUP,
    Capabilities::IDEMPOTENCY_SALES_TAX,
    Capabilities::SALES_TAX_LIABILITY,
    Capabilities::TAX_EXEMPTIONS,
];

fn item_id(s: &str) -> SalesTaxItemId {
    SalesTaxItemId::parse(s).expect("id")
}

fn agency() -> SalesTaxAgencyId {
    SalesTaxAgencyId::parse(AGENCY).expect("id")
}

fn rate(s: &str) -> TaxRate {
    TaxRate::parse_wire(s).expect("rate")
}

fn item_draft() -> SalesTaxItemDraft {
    SalesTaxItemDraft {
        kind: SalesTaxKind::Item,
        name: " Springfield ".into(),
        description: Some("Springfield sales tax".into()),
        rate: Some(rate("0.08875")),
        agency_id: Some(agency()),
        member_ids: Vec::new(),
    }
}

fn group_draft(members: Vec<SalesTaxItemId>) -> SalesTaxItemDraft {
    SalesTaxItemDraft {
        kind: SalesTaxKind::Group,
        name: "Monett".into(),
        description: None,
        rate: None,
        agency_id: None,
        member_ids: members,
    }
}

pub(crate) fn refusal(code: &str) -> ApiError {
    ApiError::Conflict {
        reason: ConflictReason::Unspecified,
        current_version: None,
        code: crate::errors::ErrorCode::parse(code),
        message: crate::errors::UserText::default(),
        details: crate::ConflictDetails::default(),
    }
}

#[allow(clippy::needless_pass_by_value, reason = "takes the call result as is")]
fn local(e: Result<Value, ApiError>) -> Option<LocalValidation> {
    match e {
        Err(ApiError::Local(l)) => Some(l),
        _ => None,
    }
}

#[test]
fn every_refusal_code_classifies() {
    for code in sales_tax_refusal_codes() {
        assert!(sales_tax_refusal(&refusal(code)).is_some(), "{code}");
    }
    let stale = ApiError::Conflict {
        reason: ConflictReason::Stale,
        current_version: None,
        code: None,
        message: crate::errors::UserText::default(),
        details: crate::ConflictDetails::default(),
    };
    assert_eq!(sales_tax_refusal(&stale), Some(SalesTaxRefusal::Stale));
}

#[test]
fn item_drafts_are_checked_before_sending() {
    let body = item_draft().body().expect("item");
    assert_eq!(
        body,
        json!({"kind":"item","name":"Springfield","description":"Springfield sales tax",
               "rate":"0.088750","agencyId":AGENCY})
    );
    let mut d = item_draft();
    d.agency_id = None;
    assert_eq!(local(d.body()), Some(LocalValidation::InvalidValue));
    let mut d = item_draft();
    d.rate = Some(rate("1"));
    assert_eq!(local(d.body()), Some(LocalValidation::InvalidValue));
    let mut d = item_draft();
    d.rate = None;
    assert_eq!(local(d.body()), Some(LocalValidation::InvalidValue));
    let mut d = item_draft();
    d.name = "   ".into();
    assert_eq!(local(d.body()), Some(LocalValidation::InvalidValue));
    d.name = "x".repeat(SALES_TAX_NAME_MAX + 1);
    assert_eq!(local(d.body()), Some(LocalValidation::InvalidValue));
    // Zero is a valid rate (a 0% item for an exempt location).
    let mut d = item_draft();
    d.rate = Some(TaxRate::ZERO);
    assert!(d.body().is_ok());
}

#[test]
fn group_drafts_take_one_to_ten_distinct_items() {
    let body = group_draft(vec![item_id(STATE), item_id(CITY)])
        .body()
        .expect("group");
    assert_eq!(
        body,
        json!({"kind":"group","name":"Monett","memberIds":[STATE, CITY]})
    );
    assert_eq!(
        local(group_draft(Vec::new()).body()),
        Some(LocalValidation::TooFewLines)
    );
    let eleven: Vec<SalesTaxItemId> = (0..11)
        .map(|i| item_id(&format!("7c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{i:02}")))
        .collect();
    assert_eq!(
        local(group_draft(eleven).body()),
        Some(LocalValidation::TooManyLines)
    );
    assert_eq!(
        local(group_draft(vec![item_id(STATE), item_id(STATE)]).body()),
        Some(LocalValidation::InvalidValue)
    );
    let mut g = group_draft(vec![item_id(STATE)]);
    g.rate = Some(rate("0.01"));
    assert_eq!(local(g.body()), Some(LocalValidation::InvalidValue));
}

#[test]
fn items_decode_four_and_six_place_rates_and_both_member_forms() {
    let list = format!(
        r#"{{"data":[
          {{"id":"{STATE}","kind":"item","name":"State","rate":"0.042250","agencyId":"{AGENCY}",
            "agencyName":"Sample State Revenue Dept.","isActive":true,"inUse":true,
            "updatedAt":"2026-11-01T10:00:00.000Z"}},
          {{"id":"{CITY}","kind":"item","name":"Sales tax 8.25%","rate":"0.0825","agencyId":null,
            "legacyRate":"0.0825","isActive":true,"inUse":false}},
          {{"id":"{GROUP}","kind":"group","name":"Monett","rate":"0.081000",
            "members":[{{"id":"{STATE}","name":"State","rate":"0.042250","agencyId":"{AGENCY}"}},"{CITY}"]}}]}}"#
    );
    let items: Vec<SalesTaxItem> = decode_list(list.as_bytes(), "items", "items").expect("decode");
    assert_eq!(items[0].rate.format_percent(), "4.225%");
    assert!(items[0].in_use && !items[0].is_legacy());
    assert!(items[1].is_legacy() && items[1].needs_agency());
    assert_eq!(items[2].kind, SalesTaxKind::Group);
    assert!(!items[2].needs_agency());
    assert_eq!(items[2].members.len(), 2);
    assert_eq!(items[2].members[1].id, item_id(CITY));
    assert_eq!(items[2].members[1].rate, None);
    assert_eq!(items[2].rate.format_percent(), "8.100%");
    // A bare array decodes too; a JSON number rate does not.
    let bare = format!(r#"[{{"id":"{STATE}","kind":"item","name":"S","rate":"0.0425"}}]"#);
    assert_eq!(
        decode_list::<SalesTaxItem>(bare.as_bytes(), "items", "items")
            .expect("bare")
            .len(),
        1
    );
    let number = format!(r#"[{{"id":"{STATE}","kind":"item","name":"S","rate":0.0425}}]"#);
    assert!(decode_list::<SalesTaxItem>(number.as_bytes(), "items", "items").is_err());
}

#[test]
fn debug_hides_owner_data() {
    let a: SalesTaxAgency = serde_json::from_str(&format!(
        r#"{{"id":"{AGENCY}","vendorId":"{VENDOR}","vendorName":"Secret Revenue Office"}}"#
    ))
    .expect("agency");
    assert!(a.is_active);
    assert!(!format!("{a:?}").contains("Secret"));
    let x: TaxExemption = serde_json::from_str(&format!(
        r#"{{"id":"{CERT}","contactId":"{CLIENT}","number":"EX-99887766","state":"MO",
             "expiresOn":"2026-12-15","note":"private note"}}"#
    ))
    .expect("exemption");
    let dbg = format!("{x:?}");
    assert!(
        !dbg.contains("99887766") && !dbg.contains("private"),
        "{dbg}"
    );
    let d = TaxExemptionDraft {
        contact_id: x.contact_id,
        number: "EX-99887766".into(),
        state: Some("mo".into()),
        expires_on: x.expires_on,
        attachment_id: None,
        note: None,
    };
    assert!(!format!("{d:?}").contains("99887766"));
    let body = d.body().expect("body");
    assert_eq!(body["state"], "MO");
    assert_eq!(body["number"], "EX-99887766");
    let mut bad = d.clone();
    bad.state = Some("Missouri".into());
    assert_eq!(local(bad.body()), Some(LocalValidation::InvalidValue));
    bad.state = None;
    bad.number = " ".into();
    assert_eq!(local(bad.body()), Some(LocalValidation::InvalidValue));
}

#[test]
fn tax_code_drafts_hold_eight_characters() {
    let d = TaxCodeDraft {
        code: " Res ".into(),
        name: "Resale".into(),
        taxable: false,
    };
    assert_eq!(
        d.body().expect("code"),
        json!({"code":"Res","name":"Resale","taxable":false})
    );
    let long = TaxCodeDraft {
        code: "ABCDEFGHI".into(),
        ..d
    };
    assert_eq!(local(long.body()), Some(LocalValidation::InvalidValue));
}

#[tokio::test]
async fn setup_writes_are_keyed_and_need_the_capability() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/sales-tax/items"))
        .respond_with(json(
            201,
            &format!(
                r#"{{"item":{{"id":"{STATE}","kind":"item","name":"Springfield","rate":"0.088750","agencyId":"{AGENCY}"}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/sales-tax/items/{STATE}/deactivate")))
        .respond_with(json(
            200,
            &format!(
                r#"{{"id":"{STATE}","kind":"item","name":"Springfield","rate":"0.088750","isActive":false}}"#
            ),
        ))
        .mount(&server)
        .await;
    let none = MockServer::start().await;
    let s = session_with_caps(&none, &[]).await;
    assert_eq!(
        s.sales_tax()
            .create_item(&item_draft(), IdempotencyKey::new())
            .await
            .expect_err("cap"),
        ApiError::Local(LocalValidation::Unsupported)
    );
    let s = session_with_caps(&server, &ALL).await;
    let made = s
        .sales_tax()
        .create_item(&item_draft(), IdempotencyKey::new())
        .await
        .expect("create");
    assert_eq!(made.rate, rate("0.08875"));
    let off = s
        .sales_tax()
        .set_item_active(item_id(STATE), false, IdempotencyKey::new())
        .await
        .expect("deactivate");
    assert!(!off.is_active);
    let posts = requests(&server, "POST").await;
    assert_eq!(posts.len(), 2);
    for r in &posts {
        assert!(r.headers.get("idempotency-key").is_some(), "{}", r.url);
    }
    let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
    assert_eq!(body["rate"], "0.088750");
}

#[tokio::test]
async fn owed_reads_engine_strings() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/sales-tax/owed"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"agencies":[{{"agencyId":"{AGENCY}","items":[{{"taxItemId":"{STATE}","owed":"80.70"}}],"total":"80.70"}}],
                    "unassigned":"0.00","total":"80.70"}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let owed = s
        .sales_tax()
        .owed(
            CalendarDate::from_ymd(2026, 11, 30).expect("date"),
            Some(SalesTaxBasis::Cash),
            None,
        )
        .await
        .expect("owed");
    assert_eq!(owed.total.to_wire(), "80.7000");
    assert_eq!(owed.agencies[0].items[0].owed.to_wire(), "80.7000");
    let q = query(&requests(&server, "GET").await[1]);
    assert!(
        q.contains("asOf=2026-11-30") && q.contains("basis=cash"),
        "{q}"
    );
    assert!(!format!("{owed:?}").contains("80.7"));
    // A sub-cent owed figure is refused (2-place strings only).
    let bad: Result<SalesTaxOwed, _> = serde_json::from_str(r#"{"agencies":[],"total":"80.705"}"#);
    assert!(bad.is_err());
}

#[tokio::test]
async fn exemptions_list_and_expiring_report() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/sales-tax/exemptions"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"data":[{{"id":"{CERT}","contactId":"{CLIENT}","number":"N1","expiresOn":"2026-10-31","isActive":true}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/reports/exemptions-expiring"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"rows":[{{"exemptionId":"{CERT}","contactId":"{CLIENT}","contactName":"Cedar Hollow Supply",
                     "number":"N1","expiresOn":"2026-10-31","status":"expired"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let c = ContactId::parse(CLIENT).expect("id");
    let list = s.sales_tax().exemptions(Some(c)).await.expect("list");
    assert_eq!(list.len(), 1);
    let soon = s
        .sales_tax()
        .exemptions_expiring(CalendarDate::from_ymd(2026, 11, 1).expect("d"), Some(60))
        .await
        .expect("expiring");
    assert_eq!(soon[0].status, ExemptionStatus::Expired);
    let gets = requests(&server, "GET").await;
    assert!(query(&gets[1]).contains(&format!("contactId={CLIENT}")));
    assert!(query(&gets[2]).contains("within=60"));
}

#[test]
fn documents_carry_the_sales_tax_and_line_codes() {
    use crate::resources::invoices::InvoicePreview;
    use crate::resources::sales::{InvoiceLine, NewInvoice};
    let p: InvoicePreview = serde_json::from_str(&format!(
        r#"{{"lines":[{{"amount":"1000.00","taxAmount":"88.75",
              "taxCode":{{"id":null,"code":"Tax","taxable":true,"source":"item"}}}},
             {{"amount":"500.00","taxAmount":"0.00",
              "taxCode":{{"code":"Res","taxable":false,"source":"client"}}}}],
            "subtotal":"1500.00","taxableBase":"1000.00","taxRate":"0.088750","taxTotal":"88.75",
            "total":"1588.75",
            "salesTax":{{"id":"{GROUP}","name":"Monett","rate":"0.081000","kind":"group",
              "members":[{{"id":"{STATE}","name":"State","rate":"0.042250","tax":"0.26"}},
                         {{"itemId":"{CITY}","name":"City","rate":"0.038750","amount":"0.23"}}]}}}}"#
    ))
    .expect("preview");
    assert_eq!(p.tax_rate, Some(rate("0.08875")), "six places decode");
    let st = p.sales_tax.as_ref().expect("sales tax");
    assert_eq!(st.rate.format_percent(), "8.100%");
    assert_eq!(
        st.members[1].tax.map(Money::to_wire).as_deref(),
        Some("0.2300")
    );
    let code = p.lines[1].tax_code.as_ref().expect("code");
    assert_eq!((code.code.as_str(), code.taxable), ("Res", false));
    assert_eq!(code.source, Some(TaxCodeSource::Client));
    assert!(!format!("{st:?}").contains("Monett"));
    let mut line = InvoiceLine::item(
        cl_core::ids::ItemId::parse(STATE).expect("id"),
        cl_core::Quantity::parse_wire("1").expect("q"),
    );
    line.tax_code_id = Some(TaxCodeId::parse(CERT).expect("id"));
    let mut n = NewInvoice::new(
        ContactId::parse(CLIENT).expect("id"),
        CalendarDate::from_ymd(2026, 11, 4).expect("d"),
        vec![line],
    );
    let v = serde_json::to_value(&n).expect("json");
    assert!(v.get("salesTaxId").is_none() && v.get("useCurrentRates").is_none());
    assert_eq!(v["lines"][0]["taxCodeId"], CERT);
    n.sales_tax_id = Some(item_id(GROUP));
    n.use_current_rates = true;
    let v = serde_json::to_value(&n).expect("json");
    assert_eq!(v["salesTaxId"], GROUP);
    assert_eq!(v["useCurrentRates"], true);
    assert_eq!(
        doc_tax_problem("RATE_CHANGED"),
        Some(DocTaxProblem::RateChanged)
    );
    assert_eq!(
        doc_tax_problem("EXEMPTION_EXPIRED"),
        Some(DocTaxProblem::ExemptionExpired)
    );
    assert_eq!(doc_tax_problem("OVERAPPLIED"), None);
    assert_eq!(doc_tax_problem_codes().len(), 4);
}

#[test]
fn company_settings_read_and_write_the_sales_tax_flags() {
    use crate::resources::company::{CompanySettings, SalesTaxSettingsUpdate};
    let s: CompanySettings = serde_json::from_str(&format!(
        r#"{{"defaultSalesTaxRate":"0.081000","salesTaxEnabled":true,"salesTaxBasis":"cash",
            "defaultSalesTaxId":"{GROUP}","salesTaxSsuta":false}}"#
    ))
    .expect("settings");
    assert_eq!(s.default_sales_tax_rate, Some(rate("0.081")));
    assert_eq!(s.sales_tax_basis, Some(SalesTaxBasis::Cash));
    assert_eq!(s.default_sales_tax_id, Some(item_id(GROUP)));
    assert_eq!(s.sales_tax_ssuta, Some(false));
    let old: CompanySettings = serde_json::from_str("{}").expect("old engine");
    assert_eq!(old.sales_tax_enabled, None);
    let u = SalesTaxSettingsUpdate {
        enabled: Some(true),
        basis: Some(SalesTaxBasis::Accrual),
        default_sales_tax_id: Some(None),
        ssuta: Some(true),
    };
    let none = Capabilities::default();
    assert_eq!(
        u.body(&none).expect_err("cap"),
        ApiError::Local(LocalValidation::Unsupported)
    );
    let caps: Capabilities = [Capabilities::COMPANY_SALES_TAX_FLAGS]
        .into_iter()
        .collect();
    assert_eq!(
        u.body(&caps).expect("body"),
        json!({"salesTaxEnabled":true,"salesTaxBasis":"accrual","defaultSalesTaxId":null,
               "salesTaxSsuta":true})
    );
    assert!(SalesTaxSettingsUpdate::default().is_empty());
}

#[test]
fn contacts_and_items_send_their_tax_defaults_only_when_set() {
    use crate::resources::contacts::{ContactFields, ContactType};
    let mut c = ContactFields::named(ContactType::Customer, "Ozark Outfitters");
    let v = serde_json::to_value(&c).expect("json");
    assert!(v.get("defaultSalesTaxId").is_none() && v.get("taxCodeId").is_none());
    c.default_sales_tax_id = Some(item_id(GROUP));
    c.tax_code_id = Some(TaxCodeId::parse(CERT).expect("id"));
    let v = serde_json::to_value(&c).expect("json");
    assert_eq!(v["defaultSalesTaxId"], GROUP);
    assert_eq!(v["taxCodeId"], CERT);
    let back: ContactFields = serde_json::from_value(v).expect("decode");
    assert_eq!(back.default_sales_tax_id, Some(item_id(GROUP)));
}
