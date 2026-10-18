//! Synthetic data for `--demo` (the mockups' companies). Nothing here is
//! real; the figures come from `docs/design/README.md`.

use cl_core::ids::TenantId;
use cl_core::{CalendarDate, Timestamp};
use cl_settings::{ACCENT_PALETTE, Accent};

pub mod contacts;
pub mod lists;

/// One demo company card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoCompany {
    /// Fake tenant id.
    pub tenant_id: TenantId,
    /// Name.
    pub name: String,
    /// Stripe.
    pub accent: Accent,
    /// Meta line as the mockup shows it.
    pub meta: String,
    /// The practice company.
    pub is_sample: bool,
    /// Closing date shown in the title bar when opened.
    pub closed_through: Option<CalendarDate>,
}

/// Demo-mode state the screens share.
#[derive(Clone, Debug)]
pub struct DemoState {
    /// The four companies of the Open Company mockup.
    pub companies: Vec<DemoCompany>,
    /// Which one is open.
    pub open: Option<TenantId>,
    /// The open company's chart of accounts as the demo "engine" keeps it
    /// (built on first use from the sample fixture, P11).
    pub chart: Option<Vec<cl_api::Account>>,
    /// Accounts marked sensitive in the demo (Q-157).
    pub sensitive: std::collections::BTreeSet<cl_core::ids::AccountId>,
    /// The open company's clients and vendors as the demo "engine" keeps
    /// them (built on first use from the sample fixture, P11).
    pub contacts: Option<Vec<cl_api::Contact>>,
    /// Each demo contact's fixture name (history follows a rename).
    pub contact_names: std::collections::BTreeMap<cl_core::ids::ContactId, String>,
    /// Merges done in the demo: (merged away, kept).
    pub merged: Vec<(cl_core::ids::ContactId, cl_core::ids::ContactId)>,
    /// Vendors' 1099 profiles saved in the demo.
    pub profiles: std::collections::BTreeMap<cl_core::ids::ContactId, cl_api::Vendor1099Profile>,
    /// The open company's items as the demo "engine" keeps them (built on
    /// first use, P11 step 4).
    pub items: Option<Vec<cl_api::Item>>,
}

fn id(n: u8) -> TenantId {
    TenantId::parse(&format!("d0000000-0000-4000-8000-0000000000{n:02x}")).expect("demo id")
}

impl DemoState {
    /// The mockup list.
    #[must_use]
    pub fn new() -> DemoState {
        DemoState {
            companies: vec![
                DemoCompany {
                    tenant_id: id(1),
                    name: "Ozark Ridge Landscaping LLC".into(),
                    accent: ACCENT_PALETTE[0],
                    meta: "Last opened today 8:10 AM \u{b7} Backed up today 8:12 AM \u{b7} 1,512 transactions"
                        .into(),
                    is_sample: false,
                    closed_through: CalendarDate::from_ymd(2026, 9, 30).ok(),
                },
                DemoCompany {
                    tenant_id: id(2),
                    name: "Shoal Creek Farm".into(),
                    accent: ACCENT_PALETTE[1],
                    meta: "Last opened 10/28/2026 \u{b7} Converted from QuickBooks Desktop \u{b7} Verified"
                        .into(),
                    is_sample: false,
                    closed_through: None,
                },
                DemoCompany {
                    tenant_id: id(3),
                    name: "Kurt \u{2014} Rental Duplex".into(),
                    accent: ACCENT_PALETTE[2],
                    meta: "Last opened 10/02/2026 \u{b7} Backup reminder: last backup 35 days ago".into(),
                    is_sample: false,
                    closed_through: None,
                },
                DemoCompany {
                    tenant_id: id(4),
                    name: "Practice company \u{b7} Ozark Ridge (sample)".into(),
                    accent: Accent([0xC9, 0xC4, 0xBA]),
                    meta: "Made-up data to try things safely \u{b7} Reset any time".into(),
                    is_sample: true,
                    closed_through: None,
                },
            ],
            open: Some(id(1)),
            chart: None,
            sensitive: std::collections::BTreeSet::new(),
            contacts: None,
            contact_names: std::collections::BTreeMap::new(),
            merged: Vec::new(),
            profiles: std::collections::BTreeMap::new(),
            items: None,
        }
    }

    /// The story date of the mockups.
    #[must_use]
    pub fn today() -> CalendarDate {
        CalendarDate::from_ymd(2026, 11, 4).unwrap_or_else(|_| CalendarDate::min())
    }

    /// A fixed timestamp for demo records.
    #[must_use]
    pub fn now() -> Timestamp {
        Timestamp::parse_wire("2026-11-04T08:10:00.000Z").unwrap_or_else(|_| {
            Timestamp::from_unix_millis(0).unwrap_or_else(|_| unreachable!("epoch"))
        })
    }

    /// The demo contacts, built from the sample on first use.
    pub fn contacts_mut(&mut self) -> &mut Vec<cl_api::Contact> {
        if self.contacts.is_none() {
            let cs = contacts::contacts();
            self.contact_names = contacts::original_names(&cs);
            self.contacts = Some(cs);
        }
        self.contacts.get_or_insert_with(Vec::new)
    }

    /// The demo items, built on first use.
    pub fn items_mut(&mut self) -> &mut Vec<cl_api::Item> {
        self.items.get_or_insert_with(lists::items)
    }

    /// A fresh fake tenant id for a company made in the demo.
    #[must_use]
    pub fn fresh_id(&self) -> TenantId {
        let n = u8::try_from(self.companies.len() + 1).unwrap_or(u8::MAX);
        id(n)
    }
}

impl Default for DemoState {
    fn default() -> Self {
        DemoState::new()
    }
}

/// What the Preferences window shows in the demo: the mockup's AI panel
/// (on, Ollama, `llama3.1:8b`, nothing spent) over Ozark Ridge's settings.
#[must_use]
pub fn preferences_loaded() -> crate::screens::preferences::Loaded {
    let settings: Option<cl_api::CompanySettings> = serde_json::from_value(serde_json::json!({
        "fiscalYearStartMonth": 1,
        "accountingMethod": "accrual",
        "defaultSalesTaxRate": "0.0825",
        "lockDate": "2026-09-30",
    }))
    .ok();
    let ai: cl_api::AiConfig = serde_json::from_value(serde_json::json!({
        "isEnabled": true,
        "fallbackChain": ["ollama"],
        "categorizationProvider": "ollama",
        "categorizationModel": "llama3.1:8b",
        "ollamaBaseUrl": "http://localhost:11434",
        "piiProtectionLevel": "strict",
        "trackUsage": true,
        "monthlyBudgetLimit": 10,
        "adminDisclosureAcceptedAt": "2026-10-01T08:00:00.000Z",
        "disclosureVersion": 1,
    }))
    .unwrap_or_default();
    let usage: cl_api::AiUsage = serde_json::from_value(serde_json::json!({
        "totalCalls": 0, "totalInputTokens": 0, "totalOutputTokens": 0, "totalCost": 0,
        "byProvider": {}, "byJobType": {},
    }))
    .unwrap_or_default();
    let consent: Option<cl_api::CompanyConsent> = serde_json::from_value(serde_json::json!({
        "companyId": "d0000000-0000-4000-8000-0000000000c1",
        "companyName": "Ozark Ridge Landscaping LLC",
        "systemVersion": 1,
        "acceptedVersion": 1,
        "acceptedAt": "2026-10-01T08:05:00.000Z",
        "aiEnabled": true,
        "enabledTasks": {"categorization": true, "receipt_ocr": true, "report_summary": true},
        "isStale": false,
    }))
    .ok();
    crate::screens::preferences::Loaded {
        settings,
        ai: Some(ai),
        consent,
        smtp: None,
        usage: Some(usage),
        done: true,
    }
}

/// The demo recovery key shown by the recovery-key screen (synthetic;
/// redacted by `inspect` like any key).
pub const DEMO_RECOVERY_KEY: &str = "K7F2-9QMD-3XTB-8RVC-PL6N-WY4H";

/// Demo chart-of-accounts names for the opening-balances pickers.
pub const DEMO_ACCOUNTS: [(&str, &str, &str); 8] = [
    ("1010", "Operating Checking", "asset"),
    ("1020", "Savings", "asset"),
    ("2050", "Business Credit Card", "liability"),
    ("2200", "Vehicle and Equipment Loans", "liability"),
    ("1500", "Equipment", "asset"),
    ("1510", "Vehicles", "asset"),
    ("3100", "Owner Contributions", "equity"),
    ("2300", "Line of Credit", "liability"),
];

/// A demo account id from its number (stable across runs).
#[must_use]
pub fn account_id(n: u32) -> cl_core::ids::AccountId {
    cl_core::ids::AccountId::parse(&format!("d0000000-0000-4000-8000-a{n:011}"))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

/// One demo chart row before it becomes an engine-shaped account.
struct DemoRow {
    number: String,
    name: String,
    kind: cl_api::AccountType,
    detail: Option<String>,
    parent: Option<u32>,
    active: bool,
    system: bool,
}

/// The demo chart: the sample company's accounts, the engine's system
/// accounts the sample posts to, and a few sub-accounts and an inactive
/// account so the hierarchy and the filter have something to show. The
/// balances are the fixture's own figures at the story date, standing in
/// for the engine (credit-normal accounts shown positive). Synthetic.
#[must_use]
pub fn chart() -> Vec<cl_api::Account> {
    use cl_api::{Account, AccountFields, AccountType};
    let sample = cl_company::Sample::load();
    let story = DemoState::today();
    let mut rows: Vec<DemoRow> = sample
        .company
        .accounts
        .iter()
        .map(|a| {
            let detail = match a.detail.as_deref() {
                Some("checking" | "savings") => Some("bank".to_string()),
                other => other.map(str::to_string),
            };
            DemoRow {
                number: a.number.clone(),
                name: a.name.clone(),
                kind: a.account_type,
                detail,
                parent: None,
                active: true,
                system: false,
            }
        })
        .collect();
    for (number, name, t, detail, system) in [
        (
            "1200",
            "Accounts Receivable",
            AccountType::Asset,
            "accounts_receivable",
            true,
        ),
        (
            "2000",
            "Accounts Payable",
            AccountType::Liability,
            "accounts_payable",
            true,
        ),
        (
            "3900",
            "Retained Earnings",
            AccountType::Equity,
            "retained_earnings",
            true,
        ),
    ] {
        rows.push(DemoRow {
            number: number.into(),
            name: name.into(),
            kind: t,
            detail: Some(detail.into()),
            parent: None,
            active: true,
            system,
        });
    }
    for (number, name, parent, active) in [
        ("6710", "Truck Repairs", 6700, true),
        ("6720", "Trailer Registration", 6700, true),
        ("6721", "Plates and Permits", 6720, true),
        ("6450", "Answering Service", 0, false),
    ] {
        rows.push(DemoRow {
            number: number.into(),
            name: name.into(),
            kind: AccountType::Expense,
            detail: None,
            parent: (parent > 0).then_some(parent),
            active,
            system: false,
        });
    }
    rows.into_iter()
        .map(|r| {
            let DemoRow {
                number,
                name,
                kind: t,
                detail,
                parent,
                active,
                system,
            } = r;
            let n: u32 = number.parse().unwrap_or(0);
            let net = sample.balance(&number, story);
            let credit_normal = matches!(
                t,
                AccountType::Liability
                    | AccountType::Equity
                    | AccountType::Revenue
                    | AccountType::OtherRevenue
            );
            let balance = if credit_normal {
                net.checked_neg().ok()
            } else {
                Some(net)
            };
            Account {
                id: account_id(n),
                fields: AccountFields {
                    name,
                    account_number: Some(number),
                    account_type: t,
                    detail_type: detail,
                    description: None,
                    parent_id: parent.map(account_id),
                },
                is_active: active,
                is_system: system,
                system_tag: None,
                balance,
                created_at: Some(DemoState::now()),
                updated_at: Some(DemoState::now()),
            }
        })
        .collect()
}

/// The demo quick report of an account: the fixture's lines on it, newest
/// first (at most 200).
#[must_use]
pub fn account_report(account: &cl_api::Account) -> cl_api::AccountReport {
    let sample = cl_company::Sample::load();
    let number = account.fields.account_number.clone().unwrap_or_default();
    let mut data: Vec<cl_api::AccountReportLine> = Vec::new();
    for t in sample.transactions.iter().rev() {
        for l in t.lines.iter().filter(|l| l.account == number) {
            if data.len() >= 200 {
                break;
            }
            let id = cl_core::ids::TransactionId::parse(&format!(
                "d0000000-0000-4000-8000-b{:011}",
                t.id
            ))
            .unwrap_or_else(|_| unreachable!("a well-formed demo id"));
            data.push(cl_api::AccountReportLine {
                transaction_id: id,
                txn_type: t.kind.clone(),
                txn_number: Some(t.number.clone()),
                txn_date: cl_api::ReportDate::Day(t.date),
                memo: Some(t.memo.clone()),
                check_number: None,
                payee: t.contact.clone(),
                debit: l.debit,
                credit: l.credit,
            });
        }
    }
    cl_api::AccountReport {
        account_id: account.id,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_list_is_the_mockup() {
        let d = DemoState::new();
        assert_eq!(d.companies.len(), 4);
        assert_eq!(d.companies[0].name, "Ozark Ridge Landscaping LLC");
        assert_eq!(d.open, Some(d.companies[0].tenant_id));
        assert!(d.companies[3].is_sample);
        assert!(d.companies[0].closed_through.is_some());
        let fresh = d.fresh_id();
        assert!(d.companies.iter().all(|c| c.tenant_id != fresh));
        assert_eq!(DemoState::today().to_wire(), "2026-11-04");
        assert_eq!(DemoState::now().to_wire(), "2026-11-04T08:10:00.000Z");
        assert_eq!(DemoState::default().companies.len(), 4);
        assert_eq!(DEMO_ACCOUNTS.len(), 8);
        assert!(DEMO_RECOVERY_KEY.contains('-'));
    }
}
