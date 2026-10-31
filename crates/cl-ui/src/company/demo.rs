//! Synthetic data for `--demo` (the mockups' companies). Nothing here is
//! real; the figures come from `docs/design/README.md`.

use cl_core::ids::TenantId;
use cl_core::{CalendarDate, Timestamp};
use cl_settings::{ACCENT_PALETTE, Accent};

pub mod batch;
// P20: Create Invoice (the engine's sales money rule, held to the golden file).
pub mod billable;
pub mod bills;
pub mod checks;
pub mod contacts;
pub mod daily_sales;
pub mod deposits;
// A04 WP-D: document drafts (`document-drafts-v1`, U-95).
pub mod drafts;
pub mod home;
pub mod intake;
pub mod invoices;
pub mod journal;
pub mod lists;
pub mod money;
// P18: Pay Bills.
pub mod pay_bills;
pub mod price_levels;
pub mod printing;
pub mod register;
pub mod template_store;
pub mod templates;

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
    /// The open company's tags and tag groups (built on first use, P11
    /// step 5).
    pub tags: Option<Vec<cl_api::Tag>>,
    /// Tag groups.
    pub tag_groups: Option<Vec<cl_api::TagGroup>>,
    /// Required tag groups by account type (the demo company's local
    /// preference, Q-180).
    pub required_tag_groups:
        std::collections::BTreeMap<cl_core::ids::TagGroupId, std::collections::BTreeSet<String>>,
    /// Tags merged or deleted in the demo (their usage moved or gone).
    pub tags_gone: std::collections::BTreeSet<cl_core::ids::TagId>,
    /// Transactions recorded or changed in the demo registers (P12).
    pub ledger: register::DemoLedger,
    /// Checks written in the demo (P13): their own fields and history.
    pub checks: checks::DemoChecks,
    /// Deposits made in the demo (P14): cash back and the write count.
    pub deposits: deposits::DemoDeposits,
    /// Daily-sales layouts and days (P14).
    pub daily_sales: daily_sales::DemoDailySales,
    /// The open company's entity type (the sample's single-member LLC).
    pub entity_type: cl_api::EntityType,
    /// The demo company's AI switches (Preferences > AI).
    pub ai: DemoAi,
    /// Stored templates (P15), deleted ones included (soft delete).
    pub templates: Vec<templates::DemoTemplate>,
    /// Template groups (P15).
    pub template_groups: Vec<templates::DemoTemplateGroup>,
    /// Every handled occurrence under its unique `(template, date)` key
    /// (P15: each due date posts exactly once).
    pub occurrences:
        std::collections::BTreeMap<(cl_core::ids::TemplateId, CalendarDate), templates::Occ>,
    /// Journal entries' P15 fields (names, tags, Adjusting, reversals).
    pub journal: journal::DemoJournals,
    /// The practice company's templates were added (P15 WP-D; once, on
    /// the first open through Open Company).
    pub templates_seeded: bool,
    /// Templates whose owner (last editor, else creator) lost the kind's
    /// permission: the run skips their dates with `PERMISSION` (the demo
    /// has one user; tests and scenarios set this).
    pub templates_owner_revoked: std::collections::BTreeSet<cl_core::ids::TemplateId>,
    /// Bulk Entry saves (P15A): the idempotency replay map.
    pub batch: batch::DemoBatches,
    /// Bills and vendor credits entered in the demo (P17).
    pub bills: bills::DemoBills,
    /// Bill payments made in the demo (P18).
    pub pay_bills: pay_bills::DemoPayBills,
    /// Form printing (P19).
    pub printing: printing::DemoPrinting,
    /// Intake folders as the demo engine keeps them (A01, U-93).
    pub intake: intake::DemoIntake,
    /// Document drafts as the demo engine keeps them (A04, WP-D).
    pub drafts: drafts::DemoDrafts,
    /// A03: the demo engine stores "Card ends in" on credit card accounts
    /// (`document-drafts-v1`). Off like every demo capability; the
    /// developer command `dev.demo_card_digits` turns it on and off for
    /// the Chart of Accounts scenario.
    pub card_digits: bool,
    /// Invoices the demo engine keeps (P20).
    pub invoices: invoices::DemoInvoices,
    /// Price levels and the clients on them (P20).
    pub price_levels: price_levels::DemoPriceLevels,
}

/// The demo's AI switches: Preferences > AI writes them, Write Check's
/// account suggestions read them (the AI runs on a local model in the
/// demo, and the company agreed to the notice).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DemoAi {
    /// AI is turned on.
    pub enabled: bool,
    /// The account-suggestion task (`categorization`) is on.
    pub categorization: bool,
    /// Reading bills and receipts (`receipt_ocr`) is on (P17 Read with AI).
    pub reading: bool,
    /// A03 step 21: the demo engine offers the document reading assist
    /// (`document-assist-v1`) to Enter Bill. Off: the demo's Enter Bill
    /// keeps P17's Read with AI (bill.yaml sections 9 and 10); tests turn
    /// it on.
    pub assist: bool,
    /// The company's consent to send document text online (Preferences ›
    /// Reading documents online). Off, as for every company.
    pub text_cloud: bool,
    /// The company's consent to send document pictures online. Off.
    pub images_cloud: bool,
}

impl Default for DemoAi {
    fn default() -> Self {
        DemoAi {
            enabled: true,
            categorization: true,
            reading: true,
            assist: false,
            text_cloud: false,
            images_cloud: false,
        }
    }
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
            tags: None,
            tag_groups: None,
            required_tag_groups: std::collections::BTreeMap::new(),
            tags_gone: std::collections::BTreeSet::new(),
            ledger: register::DemoLedger::default(),
            checks: checks::DemoChecks::default(),
            deposits: deposits::DemoDeposits::default(),
            daily_sales: daily_sales::DemoDailySales::default(),
            entity_type: cl_api::EntityType::SingleMemberLlc,
            ai: DemoAi::default(),
            templates: Vec::new(),
            template_groups: Vec::new(),
            occurrences: std::collections::BTreeMap::new(),
            journal: journal::DemoJournals::default(),
            templates_seeded: false,
            templates_owner_revoked: std::collections::BTreeSet::new(),
            batch: batch::DemoBatches::default(),
            bills: bills::DemoBills::default(),
            pay_bills: pay_bills::DemoPayBills::default(),
            printing: printing::DemoPrinting::default(),
            intake: intake::DemoIntake::default(),
            // The demo engine has document drafts and attach (A04).
            drafts: drafts::DemoDrafts::on(),
            card_digits: false,
            invoices: invoices::DemoInvoices::default(),
            price_levels: price_levels::DemoPriceLevels::seeded(),
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
            let mut cs = contacts::contacts();
            bills::add_story_contacts(&mut cs);
            self.contact_names = contacts::original_names(&cs);
            self.contacts = Some(cs);
        }
        self.contacts.get_or_insert_with(Vec::new)
    }

    /// The demo items, built on first use.
    pub fn items_mut(&mut self) -> &mut Vec<cl_api::Item> {
        self.items.get_or_insert_with(lists::items)
    }

    /// The demo tags, built on first use.
    pub fn tags_mut(&mut self) -> &mut Vec<cl_api::Tag> {
        self.tags.get_or_insert_with(lists::tags)
    }

    /// The demo tag groups, built on first use.
    pub fn tag_groups_mut(&mut self) -> &mut Vec<cl_api::TagGroup> {
        self.tag_groups.get_or_insert_with(lists::tag_groups)
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
    preferences_loaded_with(DemoAi::default())
}

/// [`preferences_loaded`] with the demo's current AI switches.
#[must_use]
pub fn preferences_loaded_with(switches: DemoAi) -> crate::screens::preferences::Loaded {
    let settings: Option<cl_api::CompanySettings> = serde_json::from_value(serde_json::json!({
        "fiscalYearStartMonth": 1,
        "accountingMethod": "accrual",
        "defaultSalesTaxRate": "0.0825",
        "lockDate": "2026-09-30",
    }))
    .ok();
    let ai: cl_api::AiConfig = serde_json::from_value(serde_json::json!({
        "isEnabled": switches.enabled,
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
        "aiEnabled": switches.enabled,
        "enabledTasks": {
            "categorization": switches.categorization, "receipt_ocr": switches.reading, "report_summary": true,
            "document_text_cloud": switches.text_cloud, "document_images_cloud": switches.images_cloud,
        },
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
    ("30160", "Owner's Contribution", "equity"),
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
    /// The engine's `system_tag` (the role the engine finds it by).
    tag: Option<&'static str>,
}

/// The demo chart: the sample company's accounts, the engine's system
/// accounts the sample posts to, and a few sub-accounts and an inactive
/// account so the hierarchy and the filter have something to show. The
/// balances are the fixture's own figures at the story date, standing in
/// for the engine's stored balance (debits minus credits for every type, so
/// credit-normal accounts carry a negative figure). Synthetic.
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
                parent: a.parent.as_deref().and_then(|p| p.parse().ok()),
                active: true,
                system: false,
                // The sample's own owner equity accounts carry the engine's
                // equity roles (U-56): an engine with `equity-roles-v1`
                // adopts or seeds them for a single-member LLC, so the owner
                // helpers find them by role.
                tag: money::sample_equity_role(&a.number).map(money::EquityRole::tag),
            }
        })
        .collect();
    for (number, name, t, detail, tag) in [
        (
            "1200",
            "Accounts Receivable",
            AccountType::Asset,
            "accounts_receivable",
            Some("accounts_receivable"),
        ),
        (
            "2000",
            "Accounts Payable",
            AccountType::Liability,
            "accounts_payable",
            Some("accounts_payable"),
        ),
        (
            "3900",
            "Retained Earnings",
            AccountType::Equity,
            "retained_earnings",
            Some("retained_earnings"),
        ),
        // P14: where payments wait for a deposit, the daily-sales tax line
        // and the over/short line (the engine's system accounts).
        (
            "10150",
            "Payments Clearing",
            AccountType::Asset,
            "other_current_asset",
            Some("payments_clearing"),
        ),
        (
            "20900",
            "Sales Tax Payable",
            AccountType::Liability,
            "other_current_liability",
            Some("sales_tax_payable"),
        ),
        (
            "60900",
            "Cash Over/Short",
            AccountType::Expense,
            "other_expense",
            Some("cash_over_short"),
        ),
    ] {
        rows.push(DemoRow {
            number: number.into(),
            name: name.into(),
            kind: t,
            detail: Some(detail.into()),
            parent: None,
            active: true,
            system: true,
            tag,
        });
    }
    for (number, name, parent, active) in [
        ("6710", "Truck Repairs", 6700, true),
        ("6720", "Trailer Registration", 6700, true),
        ("6721", "Plates and Permits", 6720, true),
        ("6450", "Answering Service", 0, false),
        // The Enter Bill story vendor's default account (P17).
        ("6210", "Equipment rental", 0, true),
    ] {
        rows.push(DemoRow {
            number: number.into(),
            name: name.into(),
            kind: AccountType::Expense,
            detail: None,
            parent: (parent > 0).then_some(parent),
            active,
            system: false,
            tag: None,
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
                tag,
            } = r;
            let n: u32 = number.parse().unwrap_or(0);
            // Debits minus credits for every type, like the engine's
            // stored `accounts.balance`; screens flip it for display.
            // The fixture posts payables to its own A/P number; the demo
            // ledger keeps them on the chart's Accounts Payable (P17).
            let posted_to = if tag == Some("accounts_payable") {
                sample.company.system_accounts.accounts_payable.as_str()
            } else {
                number.as_str()
            };
            let balance = Some(sample.balance(posted_to, story));
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
                system_tag: tag.map(str::to_string),
                balance,
                created_at: Some(DemoState::now()),
                updated_at: Some(DemoState::now()),
                card_last4: None,
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
