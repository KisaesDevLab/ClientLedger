//! P08 company lifecycle against the live engine.
//!
//! Available today (real assertions): a company created with only its
//! system accounts, the starter chart imported through
//! `POST /accounts/import` (system numbers skipped, never overwritten),
//! settings written without the closing date, setup-complete, guided
//! opening balances reviewed on the balance sheet, open invoices and
//! unpaid bills, the trial balance as exact CSV strings, Export All Data
//! row counts, the virtual retained-earnings roll across a fiscal-year
//! boundary (Q-48), and company switches under 2 s (Q-51).
//!
//! Pending by capability (the branch runs once the engine advertises it):
//! `tenants-list-v1` / `tenant-archive-v1` (E5a list, create, rename,
//! export job, import with verification), `tenant-delete-v1` (E5b safe
//! delete), `recovery-key-v1` (U-23) and `closing-password-v1` (U-15).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use cl_api::{
    AccountFilter, AccountImportRow, AccountRef, AccountType, AccountingMethod, ApiError,
    ArchiveRequest, BillLine, Capabilities, ClosingDateChange, CompanySettingsUpdate,
    ContactFields, ContactType, DeleteRequest, ImportRequest, InvoiceLine, JobState,
    LocalValidation, Method, NewBill, NewCompany, NewContact, NewInvoice, NewTenant, PageRequest,
    ReportBasis,
};
use cl_core::ids::TenantId;
use cl_core::{Money, Quantity, Rate, Secret};
use serde_json::json;

use crate::common::{date, je2, key, m, post};
use crate::support::{Harness, unique};

/// The engine's own system account numbers (never imported, never
/// overwritten): seeded by every template with `systemAccountsOnly`.
const SYSTEM_NUMBERS: [&str; 7] = [
    "10100", "10150", "10200", "20100", "20900", "30000", "30120",
];

/// Tenant C: a company created with system accounts only, shared by the
/// lifecycle tests in this file (created on first use).
fn tenant_c(h: &Harness) -> TenantId {
    static ONCE: std::sync::OnceLock<TenantId> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| {
        h.run(h.s1.create_company(&NewCompany {
            company_name: unique("Lifecycle Co"),
            business_type: Some("default".into()),
            entity_type: Some("single_member_llc".into()),
            system_accounts_only: Some(true),
        }))
        .unwrap_or_else(|e| panic!("create tenant C: {e}"))
        .tenant_id
    })
}

fn session_c(h: &Harness) -> cl_api::Session {
    let c = tenant_c(h);
    let s = h.s3();
    h.run(s.open_company(c))
        .unwrap_or_else(|e| panic!("open tenant C: {e}"));
    s
}

fn row(number: &str, name: &str, t: AccountType, detail: Option<&str>) -> AccountImportRow {
    AccountImportRow {
        account_number: Some(number.into()),
        name: name.into(),
        account_type: t,
        detail_type: detail.map(str::to_string),
    }
}

/// A company created with `systemAccountsOnly` has only the engine's
/// system accounts; the imported chart adds the rest, skips a system
/// number, and the settings writer never touches the closing date.
pub fn coa_import_settings_and_setup(h: &Harness) {
    let s = session_c(h);
    let before = h
        .run(s.accounts().list_all(&AccountFilter::default()))
        .expect("accounts");
    assert!(
        before.len() <= SYSTEM_NUMBERS.len() + 2,
        "systemAccountsOnly seeded {} accounts",
        before.len()
    );
    for a in &before {
        if let Some(n) = &a.fields.account_number {
            assert!(
                SYSTEM_NUMBERS.contains(&n.as_str()),
                "non-system account seeded: {n}"
            );
        }
    }
    let rows = vec![
        row(
            "1010",
            "Operating Checking",
            AccountType::Asset,
            Some("checking"),
        ),
        row("1020", "Savings", AccountType::Asset, None),
        row("2200", "Truck Loan", AccountType::Liability, None),
        row("4000", "Landscaping Income", AccountType::Revenue, None),
        row("6100", "Fuel", AccountType::Expense, None),
        // A system number: the engine skips it and keeps its own row.
        row("30000", "Not Opening Balances", AccountType::Equity, None),
    ];
    let r = h
        .run(s.accounts().import(&rows, false, key()))
        .expect("import");
    assert_eq!(r.imported, 5, "{r:?}");
    assert_eq!(r.skipped.len(), 1, "{r:?}");
    assert_eq!(r.skipped[0].account_number.as_deref(), Some("30000"));
    let after = h
        .run(s.accounts().list_all(&AccountFilter::default()))
        .expect("accounts");
    let ob = after
        .iter()
        .find(|a| a.fields.account_number.as_deref() == Some("30000"))
        .expect("system 30000 present");
    assert_ne!(
        ob.fields.name, "Not Opening Balances",
        "system account overwritten"
    );
    assert!(
        after
            .iter()
            .any(|a| a.fields.account_number.as_deref() == Some("1010"))
    );

    // A second import of the same numbers skips them all (nothing lost).
    let again = h
        .run(s.accounts().import(&rows[..2], false, key()))
        .expect("import again");
    assert_eq!(again.imported, 0);
    assert_eq!(again.skipped.len(), 2);

    let settings = h
        .run(s.update_company_settings(
            &CompanySettingsUpdate {
                fiscal_year_start_month: Some(1),
                accounting_method: Some(AccountingMethod::Accrual),
                default_sales_tax_rate: Some(Rate::parse_wire("0.0825").expect("rate")),
                ..CompanySettingsUpdate::default()
            },
            key(),
        ))
        .expect("settings");
    assert_eq!(settings.accounting_method.as_deref(), Some("accrual"));
    assert_eq!(
        settings
            .default_sales_tax_rate
            .map(Rate::to_wire)
            .as_deref(),
        Some("0.0825")
    );
    assert_eq!(
        settings.lock_date, None,
        "settings writer touched the closing date"
    );
    h.run(s.setup_complete(key())).expect("setup complete");
    let profile = h.run(s.company_profile()).expect("profile");
    assert_eq!(profile.record.setup_complete, Some(true));

    // The engine hides some built-in slugs (`default` among them) from the
    // public list; the list itself must be non-empty and well-formed.
    let options = h.run(s.coa_template_options()).expect("templates");
    assert!(!options.is_empty(), "{options:?}");
    assert!(
        options
            .iter()
            .all(|o| !o.value.is_empty() && !o.label.is_empty()),
        "{options:?}"
    );
}

/// Guided opening balances (Q-45): bank and loan balances post one
/// balanced entry as of the start date, open invoices and unpaid bills
/// land on A/R and A/P, and the balance sheet shows them.
pub fn opening_balances_and_review(h: &Harness) {
    let s = session_c(h);
    let start = date(2026, 1, 1);
    let posted = h
        .run(s.post_opening_balances(
            &[
                cl_api::OpeningBalance {
                    account: AccountRef::Number("1010".into()),
                    balance: m("84210.55"),
                },
                cl_api::OpeningBalance {
                    account: AccountRef::Number("2200".into()),
                    balance: m("12000.00"),
                },
            ],
            start,
            key(),
        ))
        .expect("opening balances");
    assert_eq!(posted.lines_created, 3, "two balances plus the offset line");

    let accounts = h
        .run(s.accounts().list_all(&AccountFilter::default()))
        .expect("accounts");
    let by_number = |n: &str| {
        accounts
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some(n))
            .unwrap_or_else(|| panic!("account {n}"))
            .id
    };
    let opening_equity = by_number("30000");
    let client = h
        .run(s.contacts().create(
            &NewContact {
                fields: ContactFields::named(ContactType::Customer, unique("Open AR")),
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        ))
        .expect("client")
        .record;
    let vendor = h
        .run(s.contacts().create(
            &NewContact {
                fields: ContactFields::named(ContactType::Vendor, unique("Open AP")),
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        ))
        .expect("vendor")
        .record;
    let inv = h
        .run(s.create_invoice(
            &NewInvoice {
                txn_date: date(2025, 12, 15),
                due_date: Some(date(2026, 1, 14)),
                contact_id: client.id,
                lines: vec![InvoiceLine {
                    account_id: opening_equity,
                    description: Some("Opening balance".into()),
                    quantity: Quantity::parse_wire("1").expect("q"),
                    unit_price: m("1250.00"),
                    is_taxable: false,
                    tax_rate: Rate::parse_wire("0").expect("r"),
                }],
                memo: Some("Opening balance".into()),
                txn_number: None,
            },
            key(),
        ))
        .expect("invoice");
    assert_eq!(inv.total.map(Money::to_wire).as_deref(), Some("1250.0000"));
    let bill = h
        .run(s.create_bill(
            &NewBill {
                contact_id: vendor.id,
                txn_date: date(2025, 12, 20),
                due_date: Some(date(2026, 1, 19)),
                vendor_invoice_number: Some("OB-1".into()),
                memo: Some("Opening balance".into()),
                lines: vec![BillLine {
                    account_id: opening_equity,
                    description: None,
                    amount: m("300.00"),
                }],
            },
            key(),
        ))
        .expect("bill");
    assert_eq!(bill.total.map(Money::to_wire).as_deref(), Some("300.0000"));

    let bs = h
        .run(
            s.reports()
                .balance_sheet(date(2026, 1, 31), ReportBasis::Accrual),
        )
        .expect("balance sheet");
    let checking = bs
        .assets
        .iter()
        .find(|e| e.account_number.as_deref() == Some("1010"))
        .expect("checking on the balance sheet");
    assert_eq!(checking.balance.display(2).as_deref(), Some("84210.55"));
    let ar = bs
        .assets
        .iter()
        .find(|e| e.account_number.as_deref() == Some("10200"))
        .expect("A/R on the balance sheet");
    assert_eq!(ar.balance.display(2).as_deref(), Some("1250.00"));
    let loan = bs
        .liabilities
        .iter()
        .find(|e| e.account_number.as_deref() == Some("2200"))
        .expect("loan");
    assert_eq!(loan.balance.display(2).as_deref(), Some("12000.00"));
    assert_eq!(
        bs.total_assets.display(2),
        bs.total_liabilities_and_equity.display(2),
        "balance sheet balances"
    );

    // Exact strings: the CSV form, never the JSON numbers.
    let csv = h
        .run(s.reports().trial_balance_csv(
            date(2025, 1, 1),
            date(2026, 1, 31),
            ReportBasis::Accrual,
        ))
        .expect("tb csv");
    assert!(csv.contains("\"84,210.55\""), "{csv}");
    assert!(csv.contains("\"12,000.00\""), "{csv}");

    let manifest = h.run(s.export_manifest()).expect("export manifest");
    let names: Vec<&str> = manifest.files.iter().map(|f| f.name.as_str()).collect();
    for want in [
        "accounts.csv",
        "contacts.csv",
        "transactions.csv",
        "journal_lines.csv",
    ] {
        assert!(names.contains(&want), "{names:?}");
    }
    let txns = manifest
        .files
        .iter()
        .find(|f| f.name == "transactions.csv")
        .expect("transactions.csv");
    assert!(txns.row_count >= 3, "{txns:?}");
    let text = h.run(s.export_file("accounts.csv")).expect("accounts.csv");
    assert!(text.lines().count() > 5, "{text}");
}

/// Q-48: the engine rolls prior-year net income into retained earnings
/// without a closing entry. Income in December, an expense in January;
/// the January balance sheet and trial balance show the prior year's net
/// income as retained earnings and only January in net income.
pub fn year_boundary_retained_earnings(h: &Harness) {
    let s = session_c(h);
    let accounts = h
        .run(s.accounts().list_all(&AccountFilter::default()))
        .expect("accounts");
    let by_number = |n: &str| {
        accounts
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some(n))
            .unwrap_or_else(|| panic!("account {n}"))
            .id
    };
    let (cash, income, fuel) = (by_number("1020"), by_number("4000"), by_number("6100"));
    post(
        h,
        &s,
        &je2(date(2025, 12, 15), "Dec income", cash, income, "1000.00"),
    );
    post(
        h,
        &s,
        &je2(date(2026, 1, 10), "Jan fuel", fuel, cash, "150.00"),
    );

    let bs = h
        .run(
            s.reports()
                .balance_sheet(date(2026, 1, 31), ReportBasis::Accrual),
        )
        .expect("balance sheet");
    let re = bs
        .equity
        .iter()
        .find(|e| {
            e.account_number.as_deref() == Some("30120") || e.name.starts_with("Retained Earnings")
        })
        .expect("retained earnings row");
    // Prior-year net income: the 1,000 of income; the opening-balance
    // offset is equity of its own (30000), not retained earnings. The
    // opening invoice and bill of the previous test are dated in 2025 and
    // post to the opening-balances equity account, so they do not touch it.
    assert_eq!(re.balance.display(2).as_deref(), Some("1000.00"), "{bs:?}");
    let ni = bs
        .equity
        .iter()
        .find(|e| e.name.starts_with("Net Income"))
        .expect("net income row");
    assert_eq!(ni.balance.display(2).as_deref(), Some("-150.00"), "{bs:?}");

    let csv = h
        .run(s.reports().trial_balance_csv(
            date(2026, 1, 1),
            date(2026, 1, 31),
            ReportBasis::Accrual,
        ))
        .expect("tb csv");
    let re_line = csv
        .lines()
        .find(|l| l.starts_with("\"30120\""))
        .unwrap_or_else(|| panic!("virtual 30120 row in {csv}"));
    assert!(re_line.contains("\"1,000.00\""), "{re_line}");
    let pl = h
        .run(
            s.reports()
                .profit_loss(date(2026, 1, 1), date(2026, 1, 31), ReportBasis::Accrual),
        )
        .expect("p&l");
    assert_eq!(pl.net_income.display(2).as_deref(), Some("-150.00"));
}

/// Q-51: switching companies stays under 2 s each. The engine rate-limits
/// company creation with the auth routes (8 per binary), so this creates
/// as many extra companies as the budget allows and then performs ten
/// switches round-robin across every company.
pub fn ten_switches_under_two_seconds(h: &Harness) {
    let s = h.s3();
    let budget_left = 8u64.saturating_sub(h.observer.auth_calls());
    let extra = budget_left.saturating_sub(1).min(3);
    for i in 0..extra {
        h.run(s.create_company(&NewCompany {
            company_name: unique(&format!("Switch {i}")),
            business_type: Some("default".into()),
            entity_type: None,
            system_accounts_only: Some(true),
        }))
        .unwrap_or_else(|e| panic!("create: {e}"));
    }
    let companies = h.run(s.companies()).expect("companies");
    assert!(companies.len() >= 3, "{}", companies.len());
    let limit = Duration::from_secs(2);
    let mut slowest = Duration::ZERO;
    for i in 0..10 {
        let target = companies[i % companies.len()].tenant_id;
        let started = Instant::now();
        h.run(s.open_company(target))
            .unwrap_or_else(|e| panic!("switch {i}: {e}"));
        let took = started.elapsed();
        slowest = slowest.max(took);
        assert!(took < limit, "switch {i} took {took:?}");
    }
    eprintln!(
        "    10 switches across {} companies, slowest {slowest:.0?}",
        companies.len()
    );
}

/// E5a: list, create and rename through `/api/v1/tenants`.
pub fn tenants_list_create_rename_e5a(h: &Harness) {
    let s = h.s3();
    if h.require_cap(Capabilities::TENANTS_LIST, "U-05") {
        let list = h.run(s.tenants().list()).expect("list");
        let info = h.run(h.client.desktop_info()).expect("info");
        assert!(list.iter().all(|t| Some(t.id) != info.workspace_tenant_id));
        assert!(list.iter().any(|t| t.id == h.contract_co));
        let made = h
            .run(s.tenants().create(&NewTenant {
                name: unique("E5 Co"),
                business_type: Some("default".into()),
                coa_template: None,
                system_accounts_only: Some(true),
            }))
            .expect("create");
        let row = h
            .run(s.tenants().list())
            .expect("list")
            .into_iter()
            .find(|t| t.id == made.tenant_id)
            .expect("listed");
        let renamed = h
            .run(
                s.tenants()
                    .rename(made.tenant_id, "E5 Renamed", row.updated_at, key()),
            )
            .expect("rename");
        assert_eq!(renamed.name, "E5 Renamed");
        let stale = h
            .run(
                s.tenants()
                    .rename(made.tenant_id, "Again", row.updated_at, key()),
            )
            .expect_err("stale");
        assert!(matches!(
            stale,
            ApiError::Conflict {
                reason: cl_api::ConflictReason::Stale,
                ..
            }
        ));
    } else {
        let raw = h
            .run(s.raw(Method::Get, "/api/v1/tenants", &[], None))
            .expect("raw");
        assert_eq!(raw.status, 404, "the pinned engine has no /api/v1/tenants");
    }
}

/// E5a: export job, poll, import as a new tenant with the engine's
/// verification, and the trial balance string-equal before and after.
pub fn archive_export_import_e5a(h: &Harness) {
    let s = h.s3();
    let c = tenant_c(h);
    if h.require_cap(Capabilities::TENANT_ARCHIVE, "U-05") {
        h.run(s.open_company(c)).expect("open");
        let before = h
            .run(s.reports().trial_balance_csv(
                date(2025, 1, 1),
                date(2026, 12, 31),
                ReportBasis::Accrual,
            ))
            .expect("tb before");
        h.run(s.close_company()).expect("workspace");
        let passphrase = Secret::new(format!("contract-{}", unique("pass")));
        let job = h
            .run(s.tenants().export_archive(
                c,
                &ArchiveRequest {
                    include_attachments: true,
                    include_audit: true,
                    host: json!({"app": "ClientLedger", "appVersion": "test", "kind": "backup"}),
                    passphrase: passphrase.clone(),
                },
            ))
            .expect("export job");
        let deadline = Instant::now() + Duration::from_secs(120);
        let result = loop {
            let st = h.run(s.tenants().archive_job(&job.job_id)).expect("job");
            match st.status {
                JobState::Done => break st.result.expect("result"),
                JobState::Failed => panic!("export failed: {:?}", st.code),
                JobState::Queued | JobState::Running => {
                    assert!(Instant::now() < deadline, "export did not finish");
                    std::thread::sleep(Duration::from_millis(300));
                }
            }
        };
        let path = result
            .path
            .clone()
            .expect("desktop export reports its path");
        assert!(path.is_absolute());
        let imported = h
            .run_for(
                Duration::from_secs(120),
                s.tenants().import_archive(&ImportRequest {
                    path,
                    passphrase,
                    scratch: false,
                }),
            )
            .expect("import");
        assert_ne!(imported.tenant_id, c, "import creates a new tenant");
        let v = imported.verification.expect("verification");
        assert_eq!(v.trial_balance_equal, Some(true));
        assert_eq!(v.content_equal, Some(true));
        h.run(s.open_company(imported.tenant_id))
            .expect("open restored");
        let after = h
            .run(s.reports().trial_balance_csv(
                date(2025, 1, 1),
                date(2026, 12, 31),
                ReportBasis::Accrual,
            ))
            .expect("tb after");
        assert_eq!(before, after, "trial balance differs after restore");
        // The source company still exists.
        let list = h.run(s.companies()).expect("companies");
        assert!(list.iter().any(|t| t.tenant_id == c));
    } else {
        let raw = h
            .run(s.raw(
                Method::Post,
                &format!("/api/v1/tenants/{c}/archives"),
                &[],
                Some(&json!({"passphrase": "x"})),
            ))
            .expect("raw");
        assert_eq!(raw.status, 404, "the pinned engine has no archive routes");
    }
}

/// E5b: the safe delete refuses without a fresh archive and needs the
/// typed name; cl-api refuses locally before anything is sent.
pub fn safe_delete_e5b(h: &Harness) {
    let s = h.s3();
    h.run(s.close_company()).expect("workspace");
    let c = tenant_c(h);
    let before = h.observer.total();
    let e = h
        .run(s.tenants().delete(
            c,
            &DeleteRequest {
                confirm_name: "x".into(),
                archive_path: PathBuf::from("relative.clbackup"),
                archive_sha256: String::new(),
                passphrase: Secret::new("x"),
                closing_password: None,
            },
        ))
        .expect_err("no backup");
    assert_eq!(e, ApiError::Local(LocalValidation::BackupRequired));
    assert_eq!(h.observer.total(), before, "nothing may be sent");
    if h.require_cap(Capabilities::TENANT_DELETE, "U-05 (E5b)") {
        let raw = h
            .run(s.raw(
                Method::Post,
                &format!("/api/v1/tenants/{c}/delete"),
                &[],
                Some(&json!({"confirmName": "wrong name", "archive": {"path": "/nope", "sha256": "00"}, "passphrase": "x"})),
            ))
            .expect("raw");
        assert!(
            (400..500).contains(&raw.status),
            "a wrong name or missing archive must be refused: {}",
            raw.status
        );
    } else {
        let raw = h
            .run(s.raw(
                Method::Post,
                &format!("/api/v1/tenants/{c}/delete"),
                &[],
                Some(&json!({"confirmName": "x"})),
            ))
            .expect("raw");
        assert_eq!(raw.status, 404, "the pinned engine has no safe delete");
    }
}

/// U-23: rotate the recovery key and reset the password with it.
pub fn recovery_key_u23(h: &Harness) {
    let s = h.s3();
    if h.require_cap(Capabilities::RECOVERY_KEY, "U-23") {
        let key = h.run(s.rotate_recovery_key()).expect("rotate");
        assert!(key.expose().len() >= 16);
        let wrong = h
            .run(h.client.recover_password(&cl_api::PasswordRecovery {
                email: crate::support::OWNER_EMAIL.into(),
                recovery_key: Secret::new("WRONGWRONGWRONGWRONG"),
                new_password: Secret::new("another-password-123"),
            }))
            .expect_err("wrong key");
        assert!(matches!(
            wrong,
            ApiError::Unauthorized(_) | ApiError::RateLimited { .. }
        ));
    } else {
        let raw = h
            .run(s.raw(Method::Post, "/api/v1/auth/recovery-key/rotate", &[], None))
            .expect("raw");
        assert_eq!(raw.status, 404, "the pinned engine has no recovery key");
    }
}

/// U-15: the closing date is written only through the password-gated
/// route; without the capability cl-api refuses locally and sends nothing.
pub fn closing_password_u15(h: &Harness) {
    let s = session_c(h);
    let change = ClosingDateChange {
        closing_date: Some(date(2025, 12, 31)),
        closing_password: Some(Secret::new("closing-password-123")),
    };
    if h.require_cap(Capabilities::CLOSING_PASSWORD, "U-15") {
        let st = h.run(s.set_closing_date(&change, key())).expect("set");
        assert_eq!(st.lock_date, Some(date(2025, 12, 31)));
        let wrong = h
            .run(s.set_closing_date(
                &ClosingDateChange {
                    closing_date: None,
                    closing_password: Some(Secret::new("not-the-password")),
                },
                key(),
            ))
            .expect_err("wrong password");
        assert!(matches!(
            wrong,
            ApiError::Unauthorized(_) | ApiError::Forbidden { .. }
        ));
        h.run(s.set_closing_date(
            &ClosingDateChange {
                closing_date: None,
                ..change
            },
            key(),
        ))
        .expect("clear");
    } else {
        let before = h.observer.total();
        let e = h
            .run(s.set_closing_date(&change, key()))
            .expect_err("gated");
        assert_eq!(
            e,
            ApiError::Local(LocalValidation::ClosingDateWriteUnavailable)
        );
        assert_eq!(h.observer.total(), before, "nothing may be sent");
        let settings = h.run(s.company_settings()).expect("settings");
        assert_eq!(settings.lock_date, None);
    }
    let _ = PageRequest::first_max();
}
