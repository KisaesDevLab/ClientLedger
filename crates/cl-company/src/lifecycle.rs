//! Open Company: the list, opening and closing a company (Q-182, Q-183),
//! and "New from existing" (Q-127).

use cl_api::{
    AccountFilter, CompanySettingsUpdate, IdempotencyKey, NewCompany, OpenCompany, Session,
};
use cl_core::Timestamp;
use cl_core::ids::TenantId;
use cl_settings::{Accent, RecentStore};

use crate::error::CompanyError;
use crate::setup::Created;
use crate::templates::ENGINE_SYSTEM_NUMBERS;

/// One row of the Open Company list: the engine's tenant plus what the
/// local store remembers about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompanyEntry {
    /// The engine tenant.
    pub tenant_id: TenantId,
    /// Name.
    pub name: String,
    /// Stripe colour.
    pub accent: Accent,
    /// Last opened on this computer.
    pub last_opened: Option<Timestamp>,
    /// Last backup from this computer.
    pub last_backup: Option<Timestamp>,
    /// Transactions, when known (a count only).
    pub transaction_count: Option<u64>,
    /// The practice company.
    pub is_sample: bool,
    /// Where it came from (`qbd` after P31).
    pub source: Option<String>,
    /// The user's role.
    pub role: String,
}

/// The companies the owner can open, most recently opened first, with
/// the local store brought in step with the engine (companies the engine
/// no longer lists are dropped from the store; new ones are added with a
/// fresh colour).
///
/// # Errors
/// `Api`; `Settings`.
pub async fn list_companies(
    session: &Session,
    store: &RecentStore,
) -> Result<Vec<CompanyEntry>, CompanyError> {
    let engine = session.companies().await?;
    let ids: Vec<TenantId> = engine.iter().map(|c| c.tenant_id).collect();
    let list = store.update(|list| {
        list.retain_existing(&ids);
        for c in &engine {
            list.upsert(c.tenant_id, &c.tenant_name);
        }
        list.clone()
    })?;
    let mut out: Vec<CompanyEntry> = engine
        .into_iter()
        .map(|c| {
            let rec = list.get(c.tenant_id);
            CompanyEntry {
                tenant_id: c.tenant_id,
                name: c.tenant_name,
                accent: rec.map_or(cl_settings::ACCENT_PALETTE[0], |r| r.accent),
                last_opened: rec.and_then(|r| r.last_opened).or(c.last_accessed_at),
                last_backup: rec.and_then(|r| r.last_backup),
                transaction_count: rec.and_then(|r| r.transaction_count),
                is_sample: rec.is_some_and(|r| r.is_sample),
                source: rec.and_then(|r| r.source.clone()),
                role: c.role,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.last_opened
            .cmp(&a.last_opened)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

/// Opens a company (the only way to switch, Q-183). The caller has already
/// saved or prompted for open forms and waited for pending writes.
///
/// # Errors
/// `Api`; `Settings`.
pub async fn open_company(
    session: &Session,
    store: &RecentStore,
    id: TenantId,
) -> Result<OpenCompany, CompanyError> {
    let open = session.open_company(id).await?;
    let now = cl_settings::recent::now();
    store.update(|list| list.touch_opened(id, now))?;
    Ok(open)
}

/// Back to the workspace (no company open).
///
/// # Errors
/// `Api`.
pub async fn close_company(session: &Session) -> Result<(), CompanyError> {
    session.close_company().await?;
    Ok(())
}

/// "New from existing" (Q-127): a new company with the source company's
/// chart of accounts, settings and legal form, without transactions,
/// contacts or items. The session is left open on the new company.
///
/// # Errors
/// `Api`; `Settings`.
pub async fn new_from_existing(
    session: &Session,
    store: &RecentStore,
    source: TenantId,
    new_name: &str,
    accent: Accent,
) -> Result<Created, CompanyError> {
    let new_name = new_name.trim().to_string();
    if new_name.is_empty() || new_name.chars().count() > 255 {
        return Err(CompanyError::Setup(vec![
            crate::setup::SetupIssue::NameRequired,
        ]));
    }
    session.open_company(source).await?;
    let src = &session.pinned();
    let accounts = src.accounts().list_all(&AccountFilter::default()).await?;
    let settings = src.company_settings().await?;
    let profile = src.company_profile().await?.record.profile;
    let flags = store
        .load()?
        .get(source)
        .map(|r| r.flags)
        .unwrap_or_default();

    let made = session
        .create_company(&NewCompany {
            company_name: new_name.clone(),
            business_type: Some("default".into()),
            entity_type: None,
            system_accounts_only: Some(true),
        })
        .await?;
    let open = session.open_company(made.tenant_id).await?;
    let session = &session.pinned();

    let rows: Vec<cl_api::AccountImportRow> = accounts
        .iter()
        .filter(|a| a.is_active && !a.is_system)
        .filter(|a| {
            a.fields
                .account_number
                .as_deref()
                .is_none_or(|n| !ENGINE_SYSTEM_NUMBERS.contains(&n))
        })
        .map(|a| cl_api::AccountImportRow {
            account_number: a.fields.account_number.clone(),
            name: a.fields.name.clone(),
            account_type: a.fields.account_type,
            detail_type: a.fields.detail_type.clone(),
        })
        .collect();
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
    session
        .update_company_settings(
            &CompanySettingsUpdate {
                fiscal_year_start_month: settings.fiscal_year_start_month,
                accounting_method: match settings.accounting_method.as_deref() {
                    Some("cash") => Some(cl_api::AccountingMethod::Cash),
                    Some("accrual") => Some(cl_api::AccountingMethod::Accrual),
                    _ => None,
                },
                default_payment_terms: settings.default_payment_terms.clone(),
                default_sales_tax_rate: settings.default_sales_tax_rate,
                currency: settings.currency.clone(),
                date_format: settings.date_format.clone(),
                ..CompanySettingsUpdate::default()
            },
            IdempotencyKey::new(),
        )
        .await?;
    if profile.entity_type.is_some() {
        let current = session.company_profile().await?;
        let mut update = current.record.profile.clone();
        update.business_name = new_name.clone();
        update.entity_type = profile.entity_type;
        update.industry = profile.industry.clone();
        session
            .update_company_profile(&current.version, &update, IdempotencyKey::new())
            .await?;
    }
    session.setup_complete(IdempotencyKey::new()).await?;

    let now = cl_settings::recent::now();
    store.update(|list| {
        let rec = list.upsert(made.tenant_id, &new_name);
        rec.accent = accent;
        rec.flags = flags;
        rec.transaction_count = Some(0);
        list.touch_opened(made.tenant_id, now);
    })?;
    Ok(Created {
        tenant_id: open.tenant_id,
        company_id: open.company_id,
        accounts_imported: imported,
        accounts_skipped: skipped,
        closing_password_set: false,
    })
}
