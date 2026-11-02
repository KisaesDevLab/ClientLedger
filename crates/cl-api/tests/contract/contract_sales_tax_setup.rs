//! Sales tax setup against the real engine (P23 plan §2.4, §6.1 seed,
//! WP-4), and the golden "monett" seed the other P23 contract files share.
//! Every figure is the engine's; test code only compares strings.
//!
//! The P23 tests share one company of their own ("Sales Tax Co", created
//! once: tenant creation is rate limited), so the company-wide switches
//! (default sales tax, SSUTA) never leak into Contract Co. Each test seeds
//! its own agencies, sales taxes, clients and bank account, and reads the
//! liability report by its own items (or agencies); the few company-wide
//! figures checked use dates no other P23 test posts on.
//!
//! - `setup_crud_stale_and_deactivate`: agencies, items and groups, codes;
//!   `expectedUpdatedAt` 409 `STALE`; `NAME_IN_USE`, `ITEM_IN_GROUP`,
//!   `AGENCY_IN_USE`, `AGENCY_EXISTS`, `SYSTEM_CODE`, `CODE_IN_USE` (a
//!   taken code); deactivate and reactivate; a replayed
//!   create with the same key makes one item; 8.875 % reads back as the
//!   6-place string "0.088750".

use cl_api::{
    AccountType, ApiError, Capabilities, ContactFields, ContactType, ItemFields, Method,
    NewCompany, NewContact, PostingOptions, SalesTaxAgencyId, SalesTaxBasis, SalesTaxItem,
    SalesTaxItemDraft, SalesTaxItemId, SalesTaxKind, SalesTaxLiability, SalesTaxRefusal,
    SalesTaxSettingsUpdate, Session, TaxCodeDraft, TaxCodeId, Transaction, sales_tax_refusal,
};
use cl_core::ids::{AccountId, ContactId, ItemId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money, Quantity, TaxRate};
use serde_json::json;

use crate::common::{account, date, key, m};
use crate::contract_bank_postings::detail_account;
use crate::contract_receive_payment::system_account;
use crate::support::{Harness, unique};

/// The P23 capabilities every sales tax contract test needs.
pub(crate) fn caps(h: &Harness) -> bool {
    h.require_cap(Capabilities::SALES_TAX_SETUP, "U-147")
        && h.require_cap(Capabilities::SALES_TAX_RATE6, "U-148")
        && h.require_cap(Capabilities::COMPANY_SALES_TAX_FLAGS, "U-147")
        && h.require_cap(Capabilities::IDEMPOTENCY_SALES_TAX, "U-147")
        && h.require_cap(Capabilities::SALES_TAX_LIABILITY, "U-149")
}

/// The engine code of an error (any kind that carries one).
pub(crate) fn code_of(e: &ApiError) -> Option<String> {
    match e {
        ApiError::Conflict { code: Some(c), .. }
        | ApiError::NotFound { code: Some(c) }
        | ApiError::Forbidden { code: Some(c) }
        | ApiError::Server { code: Some(c), .. } => Some(c.as_str().to_string()),
        ApiError::Validation(v) => v.code().map(|c| c.as_str().to_string()),
        _ => None,
    }
}

pub(crate) fn q(s: &str) -> Quantity {
    Quantity::parse_wire(s).expect("qty")
}

pub(crate) fn rate(s: &str) -> TaxRate {
    TaxRate::parse_wire(s).expect("rate")
}

/// The golden "monett" seed (§6.1) in a company of its own.
pub(crate) struct Monett {
    pub s: Session,
    pub tenant: TenantId,
    pub checking: AccountId,
    pub income: AccountId,
    pub sales_tax_discounts: AccountId,
    pub sales_discounts: AccountId,
    pub bad_debts: AccountId,
    pub stp: AccountId,
    pub ar: AccountId,
    pub state_agency: SalesTaxAgencyId,
    pub city_agency: SalesTaxAgencyId,
    pub state: SalesTaxItemId,
    pub springfield: SalesTaxItemId,
    pub city: SalesTaxItemId,
    pub monett: SalesTaxItemId,
    pub code_tax: TaxCodeId,
    pub plants: ItemId,
    pub install_labor: ItemId,
    pub shrubs: ItemId,
    pub mulch: ItemId,
    pub perennials: ItemId,
    pub hollister: ContactId,
    pub ozark: ContactId,
    pub cedar: ContactId,
    pub briar: ContactId,
    pub pine: ContactId,
}

/// The P23 company (created on first use), opened on the switcher session.
pub(crate) fn sales_tax_company(h: &Harness) -> (TenantId, Session) {
    static ONCE: std::sync::OnceLock<TenantId> = std::sync::OnceLock::new();
    let tenant = *ONCE.get_or_init(|| {
        h.run(h.s1.create_company(&NewCompany {
            company_name: unique("Sales Tax Co"),
            business_type: Some("default".into()),
            entity_type: None,
            system_accounts_only: None,
        }))
        .unwrap_or_else(|e| panic!("create Sales Tax Co: {e}"))
        .tenant_id
    });
    let s = h.s3();
    h.run(s.open_company(tenant))
        .unwrap_or_else(|e| panic!("open Sales Tax Co: {e}"));
    (tenant, s)
}

pub(crate) fn contact(
    h: &Harness,
    s: &Session,
    t: ContactType,
    name: &str,
    sales_tax: Option<SalesTaxItemId>,
    code: Option<TaxCodeId>,
) -> ContactId {
    let mut fields = ContactFields::named(t, unique(name));
    fields.default_sales_tax_id = sales_tax;
    fields.tax_code_id = code;
    h.run(s.contacts().create(
        &NewContact {
            fields,
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("create {name}: {e}"))
    .record
    .id
}

pub(crate) fn tax_item(
    h: &Harness,
    s: &Session,
    name: &str,
    r: &str,
    agency: SalesTaxAgencyId,
) -> SalesTaxItem {
    h.run(s.sales_tax().create_item(
        &SalesTaxItemDraft {
            kind: SalesTaxKind::Item,
            name: unique(name),
            description: None,
            rate: Some(rate(r)),
            agency_id: Some(agency),
            member_ids: Vec::new(),
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("create {name}: {e}"))
}

fn product(h: &Harness, s: &Session, income: AccountId, name: &str, code: TaxCodeId) -> ItemId {
    h.run(s.items().create(
        &ItemFields {
            name: unique(name),
            description: None,
            unit_price: None,
            income_account_id: income,
            is_taxable: true,
            item_type: cl_api::ItemType::Service,
            discount_percent: None,
            tax_code_id: Some(code),
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("item {name}: {e}"))
    .record
    .id
}

/// The monett seed in the P23 company, all of it new for this test (names
/// carry `label`): accounts, two agencies, State / Springfield / City and
/// the Monett group (made the company default), a Res code, five items and
/// five clients; SSUTA as asked.
pub(crate) fn monett(h: &Harness, label: &str, ssuta: bool) -> Monett {
    let (tenant, s) = sales_tax_company(h);
    let checking = detail_account(h, &s, AccountType::Asset, "bank");
    let income = account(h, &s, AccountType::Revenue, &format!("{label} sales")).id;
    let sales_tax_discounts = account(h, &s, AccountType::OtherRevenue, "Sales tax discounts").id;
    let sales_discounts = account(h, &s, AccountType::Revenue, "Sales discounts").id;
    let bad_debts = account(h, &s, AccountType::Expense, "Bad debts").id;
    let stp = system_account(h, &s, "sales_tax_payable");
    let ar = system_account(h, &s, "accounts_receivable");
    let st = &s.sales_tax();
    let agency = |name: &str| {
        let v = contact(h, &s, ContactType::Vendor, name, None, None);
        h.run(st.create_agency(v, key()))
            .unwrap_or_else(|e| panic!("agency {name}: {e}"))
            .id
    };
    let state_agency = agency("Sample State Revenue Dept.");
    let city_agency = agency("Sample City Revenue Office");
    let state = tax_item(h, &s, "State", "0.042250", state_agency).id;
    let springfield = tax_item(h, &s, "Springfield", "0.088750", state_agency).id;
    let city = tax_item(h, &s, "City", "0.038750", city_agency).id;
    let monett = h
        .run(st.create_item(
            &SalesTaxItemDraft {
                kind: SalesTaxKind::Group,
                name: unique("Monett"),
                description: None,
                rate: None,
                agency_id: None,
                member_ids: vec![state, city],
            },
            key(),
        ))
        .expect("group Monett")
        .id;
    let codes = h.run(st.codes(false)).expect("codes");
    let code = |c: &str| {
        codes
            .iter()
            .find(|x| x.code == c && x.is_system)
            .unwrap_or_else(|| panic!("system code {c}"))
            .id
    };
    let (code_tax, code_non) = (code("Tax"), code("Non"));
    let code_res = h
        .run(st.create_code(
            &TaxCodeDraft {
                code: unique("R").chars().take(8).collect(),
                name: "Resale".into(),
                taxable: false,
            },
            key(),
        ))
        .expect("code Res")
        .id;
    h.run(s.set_sales_tax_settings(
        &SalesTaxSettingsUpdate {
            enabled: Some(true),
            basis: Some(SalesTaxBasis::Accrual),
            default_sales_tax_id: Some(Some(monett)),
            ssuta: Some(ssuta),
        },
        key(),
    ))
    .expect("sales tax settings");
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({
            "writeOffAccountId": bad_debts.to_string(),
            "writeOffThreshold": "200.00",
        })),
    ))
    .expect("write-off settings");
    let plants = product(h, &s, income, "Plants", code_tax);
    let install_labor = product(h, &s, income, "Install labor", code_non);
    let shrubs = product(h, &s, income, "Shrubs 3 gal", code_tax);
    let mulch = product(h, &s, income, "Mulch yard", code_tax);
    let perennials = product(h, &s, income, "Perennials", code_tax);
    let client = |name: &str, tax: Option<SalesTaxItemId>, code: Option<TaxCodeId>| {
        contact(h, &s, ContactType::Customer, name, tax, code)
    };
    let hollister = client("Hollister Family", None, None);
    let ozark = client("Ozark Outfitters", Some(springfield), None);
    let cedar = client("Cedar Hollow Supply", Some(springfield), Some(code_res));
    let briar = client("Briar Creek Co-op", Some(springfield), Some(code_res));
    let pine = client("Pine Bluff Nursery", Some(springfield), Some(code_res));
    Monett {
        s,
        tenant,
        checking,
        income,
        sales_tax_discounts,
        sales_discounts,
        bad_debts,
        stp,
        ar,
        state_agency,
        city_agency,
        state,
        springfield,
        city,
        monett,
        code_tax,
        plants,
        install_labor,
        shrubs,
        mulch,
        perennials,
        hollister,
        ozark,
        cedar,
        briar,
        pine,
    }
}

impl Monett {
    /// An item line at a typed price.
    pub(crate) fn line(item: ItemId, qty: &str, price: &str) -> cl_api::InvoiceLine {
        cl_api::InvoiceLine {
            unit_price: Some(m(price)),
            ..cl_api::InvoiceLine::item(item, q(qty))
        }
    }

    /// Posts an invoice; answers its id and the saved record.
    pub(crate) fn invoice(
        &self,
        h: &Harness,
        client: ContactId,
        on: CalendarDate,
        lines: Vec<cl_api::InvoiceLine>,
    ) -> cl_api::InvoiceRecord {
        h.run(self.s.invoices().create(
            &cl_api::NewInvoice::new(client, on, lines),
            &PostingOptions::default(),
            key(),
        ))
        .unwrap_or_else(|e| panic!("invoice: {e}"))
        .invoice
        .record
    }

    /// One line of Plants at `price` for `client`.
    pub(crate) fn plants(
        &self,
        h: &Harness,
        client: ContactId,
        on: CalendarDate,
        price: &str,
    ) -> cl_api::InvoiceRecord {
        self.invoice(h, client, on, vec![Self::line(self.plants, "1", price)])
    }

    pub(crate) fn txn(&self, h: &Harness, id: TransactionId) -> Transaction {
        h.run(self.s.transactions().get(id)).expect("txn").record
    }

    /// Net debit of `account` on a transaction (debit − credit).
    pub(crate) fn net(t: &Transaction, account: AccountId) -> Money {
        t.lines
            .iter()
            .filter(|l| l.account_id == account)
            .fold(Money::ZERO, |acc, l| {
                acc.checked_add(l.debit)
                    .and_then(|x| x.checked_sub(l.credit))
                    .expect("sum")
            })
    }

    pub(crate) fn liability(
        &self,
        h: &Harness,
        basis: SalesTaxBasis,
        from: CalendarDate,
        to: CalendarDate,
    ) -> SalesTaxLiability {
        h.run(
            self.s
                .reports()
                .sales_tax_liability(cl_api::LiabilityQuery {
                    start_date: from,
                    end_date: to,
                    basis: Some(basis),
                    agency_id: None,
                }),
        )
        .unwrap_or_else(|e| panic!("liability: {e}"))
    }

    /// What `agency` owes as of `as_of` on the company basis.
    pub(crate) fn owed(&self, h: &Harness, agency: SalesTaxAgencyId, as_of: CalendarDate) -> Money {
        let o = h
            .run(self.s.sales_tax().owed(as_of, None, Some(agency)))
            .unwrap_or_else(|e| panic!("owed: {e}"));
        o.agencies
            .iter()
            .find(|a| a.agency_id == Some(agency))
            .map_or(Money::ZERO, |a| a.total)
    }

    /// Previews and receives a payment into Operating checking: `apply`
    /// is `(invoice, cash, discount, write-off)` (empty: unapplied, a
    /// credit). Answers the preview and the saved payment.
    pub(crate) fn receive(
        &self,
        h: &Harness,
        client: ContactId,
        on: CalendarDate,
        amount: &str,
        apply: &[(TransactionId, &str, Option<&str>, Option<&str>)],
    ) -> (cl_api::PaymentPreview, cl_api::PaymentRecord) {
        let mut r =
            crate::contract_receive_payment::request(client, on, Some(amount), Some(self.checking));
        r.applications = Some(
            apply
                .iter()
                .map(|(inv, cash, d, w)| cl_api::ApplicationRequest {
                    invoice_id: *inv,
                    amount: Some(m(cash)),
                    discount_amount: d.map(m),
                    write_off_amount: w.map(m),
                })
                .collect(),
        );
        let p = crate::contract_receive_payment::preview(h, &self.s, &r);
        let mut save = crate::contract_receive_payment::save_of(&p, client, on, self.checking);
        save.discount_account_id = Some(self.sales_discounts);
        save.write_off_account_id = Some(self.bad_debts);
        let rec = h
            .run(
                self.s
                    .customer_payments()
                    .receive(&save, &PostingOptions::default(), key()),
            )
            .unwrap_or_else(|e| panic!("receive: {e}"));
        (p, rec)
    }

    pub(crate) fn set_ssuta(&self, h: &Harness, on: bool) {
        h.run(self.s.set_sales_tax_settings(
            &SalesTaxSettingsUpdate {
                ssuta: Some(on),
                ..SalesTaxSettingsUpdate::default()
            },
            key(),
        ))
        .expect("ssuta switch");
    }
}

/// The liability row of `item`.
pub(crate) fn row(l: &SalesTaxLiability, item: SalesTaxItemId) -> &cl_api::LiabilityItem {
    l.agencies
        .iter()
        .flat_map(|a| &a.items)
        .find(|i| i.tax_item_id == Some(item))
        .unwrap_or_else(|| panic!("liability row of {item}"))
}

/// Setup CRUD, versions and refusals (plan §2.4).
#[allow(clippy::too_many_lines, reason = "one walk through the setup routes")]
pub fn setup_crud_stale_and_deactivate(h: &Harness) {
    if !caps(h) {
        return;
    }
    let x = monett(h, "Sales tax setup", true);
    let (s, st) = (&x.s, x.s.sales_tax());

    // The agencies and items read back; 8.875 % is "0.088750" on the wire.
    let agencies = h.run(st.agencies(false)).expect("agencies");
    assert!(agencies.iter().any(|a| a.id == x.state_agency));
    assert!(agencies.iter().any(|a| a.id == x.city_agency));
    let items = h.run(st.items(false)).expect("items");
    let springfield = items
        .iter()
        .find(|i| i.id == x.springfield)
        .expect("Springfield");
    assert_eq!(springfield.rate, rate("0.088750"));
    assert_eq!(springfield.agency_id, Some(x.state_agency));
    let group = items.iter().find(|i| i.id == x.monett).expect("Monett");
    assert_eq!(group.kind, SalesTaxKind::Group);
    assert_eq!(group.rate, rate("0.081000"), "Σ member rates");
    let mut members: Vec<_> = group.members.iter().map(|mb| mb.id).collect();
    members.sort();
    let mut want = vec![x.state, x.city];
    want.sort();
    assert_eq!(members, want);
    let raw = h
        .run(s.raw_mapped(Method::Get, "/api/v1/sales-tax/items", None))
        .expect("raw items");
    let list = raw
        .get("items")
        .or_else(|| raw.get("data"))
        .unwrap_or(&raw)
        .as_array()
        .cloned()
        .unwrap_or_default();
    let wire = list
        .iter()
        .find(|v| v["id"] == json!(x.springfield.to_string()))
        .expect("Springfield on the wire");
    assert_eq!(wire["rate"], json!("0.088750"), "a 6-place string");

    // A replayed create with the same key makes one item.
    let k = key();
    let draft = SalesTaxItemDraft {
        kind: SalesTaxKind::Item,
        name: unique("County"),
        description: Some("County tax".into()),
        rate: Some(rate("0.005000")),
        agency_id: Some(x.state_agency),
        member_ids: Vec::new(),
    };
    let first = h.run(st.create_item(&draft, k.clone())).expect("create");
    let again = h.run(st.create_item(&draft, k)).expect("replay");
    assert_eq!(first.id, again.id, "the replay answers the same item");
    let n = h
        .run(st.items(false))
        .expect("items")
        .iter()
        .filter(|i| i.name == draft.name)
        .count();
    assert_eq!(n, 1, "posted once");
    // The same name again: 409 NAME_IN_USE.
    let dup = h
        .run(st.create_item(&draft, key()))
        .expect_err("a second County");
    assert_eq!(sales_tax_refusal(&dup), Some(SalesTaxRefusal::NameInUse));

    // Edit with the version read; then the old version is STALE.
    let mut edit = draft.clone();
    edit.rate = Some(rate("0.006250"));
    let edited = h
        .run(st.update_item(first.id, &edit, first.updated_at, key()))
        .expect("edit");
    assert_eq!(edited.rate, rate("0.006250"));
    let stale = h
        .run(st.update_item(first.id, &draft, first.updated_at, key()))
        .expect_err("an old version");
    assert_eq!(sales_tax_refusal(&stale), Some(SalesTaxRefusal::Stale));

    // A group member can't be deactivated; a plain item can and comes back.
    let in_group = h
        .run(st.set_item_active(x.state, false, key()))
        .expect_err("State is in Monett");
    assert_eq!(
        sales_tax_refusal(&in_group),
        Some(SalesTaxRefusal::ItemInGroup)
    );
    let off = h
        .run(st.set_item_active(first.id, false, key()))
        .expect("deactivate");
    assert!(!off.is_active);
    assert!(
        !h.run(st.items(false))
            .expect("items")
            .iter()
            .any(|i| i.id == first.id),
        "inactive items are left out"
    );
    assert!(
        h.run(st.items(true))
            .expect("items")
            .iter()
            .any(|i| i.id == first.id && !i.is_active),
        "includeInactive lists it"
    );
    let on = h
        .run(st.set_item_active(first.id, true, key()))
        .expect("reactivate");
    assert!(on.is_active);
    // An item needs an agency (checked here, nothing sent) and a group
    // only items.
    let mut no_agency = draft.clone();
    no_agency.agency_id = None;
    assert!(matches!(
        h.run(st.create_item(&no_agency, key())),
        Err(ApiError::Local(_))
    ));
    let nested = h
        .run(st.create_item(
            &SalesTaxItemDraft {
                kind: SalesTaxKind::Group,
                name: unique("Nested"),
                description: None,
                rate: None,
                agency_id: None,
                member_ids: vec![x.monett],
            },
            key(),
        ))
        .expect_err("a group in a group");
    assert_eq!(
        sales_tax_refusal(&nested),
        Some(SalesTaxRefusal::GroupMemberNotItem)
    );

    // Agencies: active items keep one in use.
    let in_use = h
        .run(st.set_agency_active(x.city_agency, false, key()))
        .expect_err("City uses it");
    assert_eq!(
        sales_tax_refusal(&in_use),
        Some(SalesTaxRefusal::AgencyInUse)
    );
    let vendor = contact(h, s, ContactType::Vendor, "County Treasurer", None, None);
    let county = h.run(st.create_agency(vendor, key())).expect("agency");
    let twice = h
        .run(st.create_agency(vendor, key()))
        .expect_err("the same vendor twice");
    assert_eq!(
        sales_tax_refusal(&twice),
        Some(SalesTaxRefusal::AgencyExists)
    );
    let county_off = h
        .run(st.set_agency_active(county.id, false, key()))
        .expect("deactivate an unused agency");
    assert!(!county_off.is_active);
    assert!(
        h.run(st.set_agency_active(county.id, true, key()))
            .expect("reactivate")
            .is_active
    );

    // Codes: Tax and Non are the system's; the owner's own can be edited,
    // deactivated and reactivated.
    let sys = h
        .run(st.update_code(
            x.code_tax,
            &TaxCodeDraft {
                code: "Tax".into(),
                name: "Changed".into(),
                taxable: false,
            },
            None,
            key(),
        ))
        .expect_err("a system code");
    assert_eq!(sales_tax_refusal(&sys), Some(SalesTaxRefusal::SystemCode));
    let out = h
        .run(st.create_code(
            &TaxCodeDraft {
                code: unique("O").chars().take(8).collect(),
                name: "Out of state".into(),
                taxable: false,
            },
            key(),
        ))
        .expect("code");
    let renamed = h
        .run(st.update_code(
            out.id,
            &TaxCodeDraft {
                code: out.code.clone(),
                name: "Out of state sale".into(),
                taxable: false,
            },
            out.updated_at,
            key(),
        ))
        .expect("edit code");
    assert_eq!(renamed.name, "Out of state sale");
    if out.updated_at.is_some() {
        let stale = h
            .run(st.update_code(
                out.id,
                &TaxCodeDraft {
                    code: out.code.clone(),
                    name: "Again".into(),
                    taxable: false,
                },
                out.updated_at,
                key(),
            ))
            .expect_err("an old version of the code");
        assert_eq!(sales_tax_refusal(&stale), Some(SalesTaxRefusal::Stale));
    }
    // Codes are unique (case-insensitive): the same code again is 409
    // CODE_IN_USE.
    let taken = h
        .run(st.create_code(
            &TaxCodeDraft {
                code: out.code.to_lowercase(),
                name: "Again".into(),
                taxable: false,
            },
            key(),
        ))
        .expect_err("the code is taken");
    assert_eq!(sales_tax_refusal(&taken), Some(SalesTaxRefusal::CodeInUse));
    let gone = h
        .run(st.deactivate_code(out.id, key()))
        .expect("deactivate code");
    assert!(!gone.is_active);
    let back = h
        .run(st.reactivate_code(out.id, key()))
        .expect("reactivate code");
    assert!(back.is_active);

    // The company settings read back (flags of U-147).
    let settings = h.run(s.company_settings()).expect("settings");
    assert_eq!(settings.default_sales_tax_id, Some(x.monett));
    assert_eq!(settings.sales_tax_ssuta, Some(true));
    assert_eq!(settings.sales_tax_basis, Some(SalesTaxBasis::Accrual));
    assert_ne!(settings.sales_tax_enabled, Some(false));
    let _ = (x.tenant, date(2026, 11, 1));
}
