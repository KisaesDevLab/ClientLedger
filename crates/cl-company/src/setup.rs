//! The New Company interview (P08 step 2) and what it creates.
//!
//! The owner answers in plain language: names, what kind of business,
//! when the fiscal year starts, standard months or a retail (4-4-5)
//! calendar, which starter chart, sales tax, inventory, accrual or cash
//! (Q-44), an optional closing-date password (U-15), and the company
//! colour (Q-184). [`create_company`] then: creates the tenant with the
//! engine's system accounts only, imports the starter chart, writes the
//! profile and settings, marks setup complete, and records the company
//! locally with its colour and the flags the engine does not store yet
//! (Q-P08c).

use cl_api::{
    AccountingMethod, Capabilities, ClosingDateChange, CompanySettingsUpdate, EntityType,
    IdempotencyKey, NewCompany, Session,
};
use cl_core::ids::{CompanyId, TenantId};
use cl_core::{Rate, Secret};
use cl_settings::{Accent, CompanyFlags, FiscalCalendar, RecentStore};

use crate::error::CompanyError;
use crate::statistics::Statistics;
use crate::templates::StarterChart;

/// Shortest closing-date password (Q-211 family).
pub const CLOSING_PASSWORD_MIN_CHARS: usize = 12;

/// The interview's answers.
#[derive(Clone)]
pub struct CompanySetup {
    /// The name the owner does business as (tenant label and
    /// `businessName`).
    pub name: String,
    /// Legal name when different from the business name.
    pub legal_name: Option<String>,
    /// Legal form.
    pub entity_type: Option<EntityType>,
    /// First month of the fiscal year, 1-12.
    pub fiscal_year_start_month: u8,
    /// Standard months or a retail calendar (kept locally until U-25).
    pub fiscal_calendar: FiscalCalendar,
    /// Starter chart (Q-49).
    pub chart: StarterChart,
    /// Sales tax features on.
    pub sales_tax: bool,
    /// Default sales tax rate when on (exact).
    pub sales_tax_rate: Option<Rate>,
    /// The agency the sales tax is paid to (Q-PR4c): made a vendor and a
    /// sales tax agency after the company exists, with a "Sales tax" item
    /// at the rate as the company default.
    pub sales_tax_agency: Option<String>,
    /// Inventory features on.
    pub inventory: bool,
    /// Reporting basis (Q-44).
    pub basis: AccountingMethod,
    /// Closing-date password, when the engine supports it (U-15).
    pub closing_password: Option<Secret>,
    /// Stripe colour.
    pub accent: Accent,
}

impl std::fmt::Debug for CompanySetup {
    /// No names and no password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompanySetup")
            .field("chart", &self.chart)
            .field("fiscal_year_start_month", &self.fiscal_year_start_month)
            .field("basis", &self.basis)
            .finish_non_exhaustive()
    }
}

impl CompanySetup {
    /// Sensible defaults: calendar year, standard months, general service,
    /// no sales tax or inventory, accrual (Q-44), first palette colour.
    #[must_use]
    pub fn new(name: impl Into<String>) -> CompanySetup {
        CompanySetup {
            name: name.into(),
            legal_name: None,
            entity_type: None,
            fiscal_year_start_month: 1,
            fiscal_calendar: FiscalCalendar::Standard,
            chart: StarterChart::GeneralService,
            sales_tax: false,
            sales_tax_rate: None,
            sales_tax_agency: None,
            inventory: false,
            basis: AccountingMethod::Accrual,
            closing_password: None,
            accent: cl_settings::ACCENT_PALETTE[0],
        }
    }

    /// Checks the answers. `closing_password_supported` is whether the
    /// engine advertises `closing-password-v1`.
    #[must_use]
    pub fn validate(&self, closing_password_supported: bool) -> Vec<SetupIssue> {
        let mut out = Vec::new();
        let name = self.name.trim();
        if name.is_empty() {
            out.push(SetupIssue::NameRequired);
        } else if name.chars().count() > 255 {
            out.push(SetupIssue::NameTooLong);
        }
        if !(1..=12).contains(&self.fiscal_year_start_month) {
            out.push(SetupIssue::FiscalMonth);
        }
        if self.sales_tax {
            // Q-PR4c: the rate and the agency make the company's sales tax.
            match self.sales_tax_rate {
                Some(r) if !r.is_negative() && r.to_ten_thousandths() < 10_000 => {}
                _ => out.push(SetupIssue::TaxRateInvalid),
            }
            match self.agency_name() {
                None => out.push(SetupIssue::TaxAgencyRequired),
                Some(a) if a.chars().count() > 255 => out.push(SetupIssue::TaxAgencyTooLong),
                Some(_) => {}
            }
        }
        if let Some(p) = &self.closing_password {
            if !closing_password_supported {
                out.push(SetupIssue::ClosingPasswordUnavailable);
            } else if p.expose().chars().count() < CLOSING_PASSWORD_MIN_CHARS {
                out.push(SetupIssue::ClosingPasswordTooShort);
            }
        }
        out
    }

    /// The agency name, trimmed; `None` when blank.
    #[must_use]
    pub fn agency_name(&self) -> Option<&str> {
        self.sales_tax_agency
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    /// The local flags to record.
    #[must_use]
    pub fn flags(&self) -> CompanyFlags {
        CompanyFlags {
            fiscal_calendar: self.fiscal_calendar,
            sales_tax_enabled: self.sales_tax,
            inventory_enabled: self.inventory,
        }
    }

    /// The local flags to record when the engine stores the inventory
    /// switch itself (`company-inventory-flag-v1`, P24): the local
    /// inventory flag is never kept beside the engine's (one owner,
    /// Q-P24j).
    #[must_use]
    pub fn flags_for(&self, engine_has_inventory_flag: bool) -> CompanyFlags {
        let mut f = self.flags();
        if engine_has_inventory_flag {
            f.inventory_enabled = false;
        }
        f
    }
}

/// Something to fix before the company can be created.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SetupIssue {
    /// The company needs a name.
    NameRequired,
    /// Names are at most 255 characters.
    NameTooLong,
    /// The fiscal year starts in a month 1-12.
    FiscalMonth,
    /// The tax rate must be between 0 and 1 (a fraction).
    TaxRateInvalid,
    /// Sales tax on needs the agency it is paid to (Q-PR4c).
    TaxAgencyRequired,
    /// Agency names are at most 255 characters.
    TaxAgencyTooLong,
    /// The closing-date password needs 12+ characters.
    ClosingPasswordTooShort,
    /// The engine does not take a closing-date password yet (U-15).
    ClosingPasswordUnavailable,
}

impl SetupIssue {
    /// i18n key of the message.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            SetupIssue::NameRequired => "setup.issue.name_required",
            SetupIssue::NameTooLong => "setup.issue.name_too_long",
            SetupIssue::FiscalMonth => "setup.issue.fiscal_month",
            SetupIssue::TaxRateInvalid => "setup.issue.tax_rate",
            SetupIssue::TaxAgencyRequired => "setup.issue.tax_agency",
            SetupIssue::TaxAgencyTooLong => "setup.issue.tax_agency_too_long",
            SetupIssue::ClosingPasswordTooShort => "setup.issue.closing_password_short",
            SetupIssue::ClosingPasswordUnavailable => "setup.issue.closing_password_unavailable",
        }
    }
}

/// What [`create_company`] made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    /// The new tenant (open in the session on return).
    pub tenant_id: TenantId,
    /// Its company row.
    pub company_id: CompanyId,
    /// Chart rows the engine created.
    pub accounts_imported: u64,
    /// Chart rows the engine skipped (an existing number).
    pub accounts_skipped: usize,
    /// The closing-date password was registered (U-15).
    pub closing_password_set: bool,
    /// Q-PR4c: sales tax was asked for but its agency, item or default
    /// could not all be made (the company is usable; Sales Tax Setup
    /// finishes it).
    pub sales_tax_incomplete: bool,
}

/// A new `ClientLedger` company follows the Streamlined Sales Tax rules for
/// discounts and bad debts (Q-SPF6; existing and `MyBooks` Online companies
/// keep the engine's off, Q-P23aa). The "Sales tax" item for the typed
/// rate is the engine's, made from `defaultSalesTaxRate` (Q-P23f).
async fn streamlined_sales_tax(session: &Session, on: bool) -> Result<(), CompanyError> {
    let caps = session.client().capabilities();
    if caps.has(Capabilities::SALES_TAX_SSUTA) && caps.has(Capabilities::COMPANY_SALES_TAX_FLAGS) {
        session
            .set_sales_tax_settings(
                &cl_api::SalesTaxSettingsUpdate {
                    enabled: Some(on),
                    ssuta: Some(true),
                    ..cl_api::SalesTaxSettingsUpdate::default()
                },
                IdempotencyKey::new(),
            )
            .await?;
    }
    Ok(())
}

/// The name of the sales tax item the interview makes (owner data once
/// made; the owner renames it in Sales Tax Setup).
pub const SALES_TAX_ITEM_NAME: &str = "Sales tax";

/// Q-PR4c: the agency the owner named becomes a vendor and a sales tax
/// agency, a "Sales tax" item at the typed rate is paid to it, and that
/// item is the company default (`defaultSalesTaxId`). Each step looks for
/// what an earlier run made first (same vendor name, same item name), so
/// running it again makes nothing twice.
///
/// # Errors
/// `Unavailable` without `sales-tax-setup-v1`; any [`cl_api::ApiError`].
pub async fn set_up_sales_tax_agency(
    session: &Session,
    setup: &CompanySetup,
) -> Result<(), CompanyError> {
    let (true, Some(agency), Some(rate)) =
        (setup.sales_tax, setup.agency_name(), setup.sales_tax_rate)
    else {
        return Ok(());
    };
    if !session
        .client()
        .capabilities()
        .has(Capabilities::SALES_TAX_SETUP)
    {
        return Err(CompanyError::Unavailable("sales-tax-setup-v1"));
    }
    let st = session.sales_tax();
    let same = |a: &str| a.trim().eq_ignore_ascii_case(agency);
    // The agency: an existing one for a vendor of this name, else the
    // vendor (found or made) made an agency.
    let agencies = st.agencies(true).await?;
    let agency_id = if let Some(a) = agencies.iter().find(|a| same(&a.vendor_name)) {
        a.id
    } else {
        let found = session
            .contacts()
            .list_all(&cl_api::ContactFilter {
                search: Some(agency.to_string()),
                ..cl_api::ContactFilter::default()
            })
            .await?
            .into_iter()
            .find(|c| {
                same(&c.fields.display_name)
                    && matches!(
                        c.fields.contact_type,
                        cl_api::ContactType::Vendor | cl_api::ContactType::Both
                    )
            });
        let vendor = match found {
            Some(c) => c.id,
            None => {
                session
                    .contacts()
                    .create(
                        &cl_api::NewContact {
                            fields: cl_api::ContactFields::named(
                                cl_api::ContactType::Vendor,
                                agency,
                            ),
                            opening_balance: cl_core::Money::ZERO,
                            opening_balance_date: None,
                        },
                        IdempotencyKey::new(),
                    )
                    .await?
                    .record
                    .id
            }
        };
        st.create_agency(vendor, IdempotencyKey::new()).await?.id
    };
    // The item: one named "Sales tax" (not a legacy one) is reused.
    let items = st.items(true).await?;
    let item = if let Some(i) = items.iter().find(|i| {
        !i.is_legacy()
            && i.kind == cl_api::SalesTaxKind::Item
            && i.name.trim().eq_ignore_ascii_case(SALES_TAX_ITEM_NAME)
    }) {
        i.id
    } else {
        st.create_item(
            &cl_api::SalesTaxItemDraft {
                kind: cl_api::SalesTaxKind::Item,
                name: SALES_TAX_ITEM_NAME.to_string(),
                description: Some(SALES_TAX_ITEM_NAME.to_string()),
                rate: Some(cl_core::TaxRate::from(rate)),
                agency_id: Some(agency_id),
                member_ids: Vec::new(),
            },
            IdempotencyKey::new(),
        )
        .await?
        .id
    };
    if !session
        .client()
        .capabilities()
        .has(Capabilities::COMPANY_SALES_TAX_FLAGS)
    {
        return Err(CompanyError::Unavailable("company-sales-tax-flags-v1"));
    }
    session
        .set_sales_tax_settings(
            &cl_api::SalesTaxSettingsUpdate {
                default_sales_tax_id: Some(Some(item)),
                ..cl_api::SalesTaxSettingsUpdate::default()
            },
            IdempotencyKey::new(),
        )
        .await?;
    Ok(())
}

/// [`set_up_sales_tax_agency`] for the interview: true when it finished; a
/// failure is logged (scrubbed) and never fails the new company.
async fn sales_tax_agency_done(session: &Session, setup: &CompanySetup) -> bool {
    match set_up_sales_tax_agency(session, setup).await {
        Ok(()) => true,
        Err(e) => {
            let detail = cl_core::OwnerFacing::scrubbed_detail(&e);
            tracing::warn!(%detail, "sales tax agency not set up");
            false
        }
    }
}

/// P24: the engine owns the inventory switch (`company-inventory-flag-v1`).
/// Turned on after the chart import so the engine adopts the starter
/// chart's "Inventory Asset" (U-156) instead of making a second one.
/// Answers whether the engine stores the switch.
async fn engine_inventory(session: &Session, on: bool) -> Result<bool, CompanyError> {
    let owns = session
        .client()
        .capabilities()
        .has(Capabilities::COMPANY_INVENTORY_FLAG);
    if owns && on {
        session
            .set_inventory_enabled(true, IdempotencyKey::new())
            .await?;
    }
    Ok(owns)
}

/// Runs the interview's answers against the engine and records the
/// company locally. The session is left open on the new company. Table
/// statistics are refreshed at the end through `stats` (Q-335; a failed
/// refresh is logged, never returned).
///
/// # Errors
/// `Setup` with the issues (nothing sent); `Api`; `Settings`.
pub async fn create_company(
    session: &Session,
    store: &RecentStore,
    setup: &CompanySetup,
    stats: &Statistics,
) -> Result<Created, CompanyError> {
    let created = create_company_steps(session, store, setup).await?;
    stats.refresh().await;
    Ok(created)
}

/// [`create_company`] without the statistics refresh, for flows that load
/// more data afterwards and refresh at their own end.
pub(crate) async fn create_company_steps(
    session: &Session,
    store: &RecentStore,
    setup: &CompanySetup,
) -> Result<Created, CompanyError> {
    let caps = session.client().capabilities();
    let issues = setup.validate(caps.has(Capabilities::CLOSING_PASSWORD));
    if !issues.is_empty() {
        return Err(CompanyError::Setup(issues));
    }
    let name = setup.name.trim().to_string();
    let made = session
        .create_company(&NewCompany {
            company_name: name.clone(),
            business_type: Some("default".into()),
            entity_type: setup.entity_type.map(entity_wire),
            system_accounts_only: Some(true),
        })
        .await?;
    let open = session.open_company(made.tenant_id).await?;
    // Every call from here is pinned to the new company: a switch in the
    // middle makes them fail with `StaleScope` instead of landing elsewhere.
    let session = &session.pinned();

    // Profile: legal name and legal form (the business name is the tenant
    // name the engine already set).
    if setup.legal_name.is_some() || setup.entity_type.is_some() {
        let current = session.company_profile().await?;
        let mut update = current.record.profile.clone();
        update.business_name = name.clone();
        update.legal_name = setup
            .legal_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if setup.entity_type.is_some() {
            update.entity_type = setup.entity_type;
        }
        session
            .update_company_profile(&current.version, &update, IdempotencyKey::new())
            .await?;
    }

    session
        .update_company_settings(
            &CompanySettingsUpdate {
                fiscal_year_start_month: Some(setup.fiscal_year_start_month),
                accounting_method: Some(setup.basis),
                default_sales_tax_rate: if setup.sales_tax {
                    setup.sales_tax_rate.map(cl_core::TaxRate::from)
                } else {
                    None
                },
                currency: Some("USD".into()),
                ..CompanySettingsUpdate::default()
            },
            IdempotencyKey::new(),
        )
        .await?;

    streamlined_sales_tax(session, setup.sales_tax).await?;

    let rows = setup
        .chart
        .template()
        .import_rows(setup.sales_tax, setup.inventory);
    let mut imported = 0u64;
    let mut skipped = 0usize;
    for chunk in rows.chunks(cl_api::IMPORT_MAX_ROWS) {
        let r = session
            .accounts()
            .import(chunk, false, IdempotencyKey::new())
            .await?;
        imported += r.imported;
        skipped += r.skipped.len();
    }

    let engine_inventory = engine_inventory(session, setup.inventory).await?;

    let mut closing_password_set = false;
    if let Some(password) = &setup.closing_password {
        session
            .set_closing_date(
                &ClosingDateChange {
                    closing_date: None,
                    closing_password: Some(password.clone()),
                },
                IdempotencyKey::new(),
            )
            .await?;
        closing_password_set = true;
    }
    session.setup_complete(IdempotencyKey::new()).await?;

    // Q-PR4c: the company is usable whether or not this finishes; what is
    // left is said on the done page and Sales Tax Setup finishes it.
    let sales_tax_incomplete = !sales_tax_agency_done(session, setup).await;

    let now = cl_settings::recent::now();
    store.update(|list| {
        let rec = list.upsert(made.tenant_id, &name);
        rec.accent = setup.accent;
        rec.flags = setup.flags_for(engine_inventory);
        rec.transaction_count = Some(0);
        list.touch_opened(made.tenant_id, now);
    })?;
    Ok(Created {
        tenant_id: open.tenant_id,
        company_id: open.company_id,
        accounts_imported: imported,
        accounts_skipped: skipped,
        closing_password_set,
        sales_tax_incomplete,
    })
}

fn entity_wire(e: EntityType) -> String {
    match e {
        EntityType::SoleProp => "sole_prop",
        EntityType::SingleMemberLlc => "single_member_llc",
        EntityType::SCorp => "s_corp",
        EntityType::CCorp => "c_corp",
        EntityType::Partnership => "partnership",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_catches_each_issue() {
        let mut s = CompanySetup::new("  ");
        assert_eq!(s.validate(false), vec![SetupIssue::NameRequired]);
        s.name = "x".repeat(256);
        s.fiscal_year_start_month = 13;
        assert_eq!(
            s.validate(false),
            vec![SetupIssue::NameTooLong, SetupIssue::FiscalMonth]
        );
        let mut s = CompanySetup::new("Ozark Ridge Landscaping LLC");
        s.sales_tax = true;
        // Q-PR4c: sales tax needs a rate and the agency it is paid to.
        assert_eq!(
            s.validate(true),
            vec![SetupIssue::TaxRateInvalid, SetupIssue::TaxAgencyRequired]
        );
        s.sales_tax_agency = Some("  ".into());
        s.sales_tax_rate = Some(Rate::parse_wire("1.5").expect("rate"));
        assert_eq!(
            s.validate(true),
            vec![SetupIssue::TaxRateInvalid, SetupIssue::TaxAgencyRequired]
        );
        s.sales_tax_agency = Some("x".repeat(256));
        assert_eq!(
            s.validate(true),
            vec![SetupIssue::TaxRateInvalid, SetupIssue::TaxAgencyTooLong]
        );
        s.sales_tax_agency = Some(" Missouri Department of Revenue ".into());
        assert_eq!(s.agency_name(), Some("Missouri Department of Revenue"));
        s.sales_tax_rate = Some(Rate::parse_wire("0.0825").expect("rate"));
        s.closing_password = Some(Secret::new("short"));
        assert_eq!(
            s.validate(false),
            vec![SetupIssue::ClosingPasswordUnavailable]
        );
        assert_eq!(s.validate(true), vec![SetupIssue::ClosingPasswordTooShort]);
        s.closing_password = Some(Secret::new("a-long-enough-password"));
        assert_eq!(s.validate(true).len(), 0);
        assert!(!format!("{s:?}").contains("Ozark"));
        assert!(!format!("{s:?}").contains("long-enough"));
        assert!(!format!("{s:?}").contains("Missouri"));
        assert!(s.flags().sales_tax_enabled);
        // P24: with the engine's inventory switch the local flag is never
        // recorded beside it.
        s.inventory = true;
        assert!(s.flags_for(false).inventory_enabled);
        assert!(!s.flags_for(true).inventory_enabled);
        assert_eq!(
            s.flags_for(true).sales_tax_enabled,
            s.flags().sales_tax_enabled
        );
        assert_eq!(SetupIssue::FiscalMonth.key(), "setup.issue.fiscal_month");
    }
}
