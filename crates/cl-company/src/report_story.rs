//! The golden company's documents (P28 WP2, Q-P28m): the practice company
//! posts its year as journal entries with no client or vendor, so the
//! sales, aging, tag and 1099 reports would be empty. `report_story.json`
//! adds real documents dated 11/01–12/31/2026 (October still matches the
//! mockup): invoices, a partial payment and one paid after an as-of date,
//! a credit memo, a held customer deposit, bills, a vendor credit and a
//! bill payment that uses it, numbered tags in two groups (tagged,
//! untagged and other-group lines), an adjusting entry, a duplicate pair
//! and a reclass in the closed period posted with the practice password.
//!
//! Test tenants only: the golden-report harness posts it on top of
//! [`crate::sample::load_into`]. Every amount comes from the fixture as an
//! exact string.

use std::collections::HashMap;

use cl_api::{
    AccountFilter, Basis, BillDraft, BillLineDraft, BillToPay, CheckNumbering,
    ClosedPeriodAuthorization, ContactFilter, CreditToApply, IdempotencyKey, InvoiceLine, JeLine,
    JeOptions, NewCreditMemo, NewCustomerPayment, NewInvoice, NewJournalEntry, PayBillsRequest,
    PayMethod, PaymentPreviewRequest, PostingOptions, SaveApplication, Session, Side, TagFields,
    TagGroupFields, VendorCreditDraft,
};
use cl_core::ids::{AccountId, ContactId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, Quantity, Secret};
use serde::Deserialize;

use crate::error::CompanyError;

const STORY_JSON: &str = include_str!("../../../fixtures/sample/report_story.json");

/// One tag of a group.
#[derive(Clone, Debug, Deserialize)]
pub struct StoryTag {
    /// Name.
    pub name: String,
    /// Tag number (Q-270).
    pub number: String,
}

/// A tag group.
#[derive(Clone, Debug, Deserialize)]
pub struct StoryTagGroup {
    /// Name.
    pub name: String,
    /// Its tags.
    pub tags: Vec<StoryTag>,
}

/// A document line: an account, an exact amount, an optional tag.
#[derive(Clone, Debug, Deserialize)]
pub struct StoryLine {
    /// Account number.
    pub account: String,
    /// Amount (invoices, bills, credits).
    #[serde(default)]
    pub amount: Option<Money>,
    /// Debit (journal entries).
    #[serde(default)]
    pub debit: Option<Money>,
    /// Credit (journal entries).
    #[serde(default)]
    pub credit: Option<Money>,
    /// Tag name.
    #[serde(default)]
    pub tag: Option<String>,
}

/// An application of a payment, bill payment or credit.
#[derive(Clone, Debug, Deserialize)]
pub struct StoryApply {
    /// The invoice (payments).
    #[serde(default)]
    pub invoice: Option<String>,
    /// The bill (bill payments, credits).
    #[serde(default)]
    pub bill: Option<String>,
    /// The vendor credit.
    #[serde(default)]
    pub credit: Option<String>,
    /// Amount applied.
    #[serde(default)]
    pub amount: Option<Money>,
    /// Cash paid (bill payments).
    #[serde(default)]
    pub cash: Option<Money>,
}

/// One posting of the story, in order.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoryStep {
    /// `invoice`, `payment`, `credit_memo`, `deposit_held`, `bill`,
    /// `vendor_credit`, `pay_bills` or `journal_entry`.
    pub kind: String,
    /// The step's id within the story (later steps name it).
    pub id: String,
    /// Date.
    pub date: CalendarDate,
    /// Due date (bills).
    #[serde(default)]
    pub due: Option<CalendarDate>,
    /// Client name.
    #[serde(default)]
    pub client: Option<String>,
    /// Vendor name.
    #[serde(default)]
    pub vendor: Option<String>,
    /// Vendor's document number.
    #[serde(default)]
    pub number: Option<String>,
    /// Memo.
    #[serde(default)]
    pub memo: Option<String>,
    /// Amount received (payments, deposits).
    #[serde(default)]
    pub amount: Option<Money>,
    /// Lines.
    #[serde(default)]
    pub lines: Vec<StoryLine>,
    /// Applications (payments).
    #[serde(default)]
    pub apply: Vec<StoryApply>,
    /// Bills paid (bill payments).
    #[serde(default)]
    pub bills: Vec<StoryApply>,
    /// Credits used (bill payments).
    #[serde(default)]
    pub credits: Vec<StoryApply>,
    /// An adjusting entry.
    #[serde(default)]
    pub adjusting: bool,
    /// Posted in the closed period with the practice password.
    #[serde(default)]
    pub closed_period: bool,
}

/// The story fixture.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportStory {
    /// Tag groups to create.
    pub tag_groups: Vec<StoryTagGroup>,
    /// The postings, in order.
    pub steps: Vec<StoryStep>,
    /// A budget for Budget vs. Actual (U-203), made after the steps when
    /// the engine has `budgets-exact-v1`.
    #[serde(default)]
    pub budget: Option<StoryBudget>,
}

/// A budget: per account number, twelve fiscal-period cells.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoryBudget {
    /// Name.
    pub name: String,
    /// The fiscal year.
    pub fiscal_year: i32,
    /// Account number to its twelve cells.
    pub lines: std::collections::BTreeMap<String, Vec<Money>>,
}

impl ReportStory {
    /// The committed fixture.
    ///
    /// # Panics
    /// Only if the committed fixture doesn't parse (a test pins it).
    #[must_use]
    pub fn load() -> ReportStory {
        serde_json::from_str(STORY_JSON).expect("fixtures/sample/report_story.json parses")
    }
}

/// What posting the story made: each step's transaction and each tag.
#[derive(Clone, Debug, Default)]
pub struct Posted {
    /// Step id to its transaction.
    pub transactions: HashMap<String, TransactionId>,
    /// Tag name to id.
    pub tags: HashMap<String, TagId>,
    /// Tag group name to id.
    pub tag_groups: HashMap<String, cl_core::ids::TagGroupId>,
    /// The story's budget, when made.
    pub budget_id: Option<String>,
}

/// Lookups by the fixture's names.
struct Names {
    accounts: HashMap<String, AccountId>,
    contacts: HashMap<String, ContactId>,
}

impl Names {
    fn account(&self, n: &str) -> Result<AccountId, CompanyError> {
        self.accounts.get(n).copied().ok_or_else(|| missing(n))
    }
    fn contact(&self, n: Option<&str>) -> Result<ContactId, CompanyError> {
        let n = n.unwrap_or("");
        self.contacts.get(n).copied().ok_or_else(|| missing(n))
    }
}

/// A fixture name or step the company or story doesn't have (test tenants
/// only; never shown to an owner).
fn missing(what: &str) -> CompanyError {
    CompanyError::JobFailed {
        code: format!("REPORT_STORY_MISSING {what}"),
    }
}

fn amount(m: Option<Money>, what: &str) -> Result<Money, CompanyError> {
    m.ok_or_else(|| missing(what))
}

/// Posts the story into the open company (the practice company, loaded by
/// [`crate::sample::load_into`]). Steps post one at a time in fixture
/// order, so same-day ledger order is fixed.
///
/// # Errors
/// Any [`cl_api::ApiError`]; a fixture name the company doesn't have.
#[allow(clippy::too_many_lines, reason = "one arm per document kind")]
pub async fn post(session: &Session, story: &ReportStory) -> Result<Posted, CompanyError> {
    let accounts = session
        .accounts()
        .list_all(&AccountFilter::default())
        .await?
        .into_iter()
        .filter_map(|a| a.fields.account_number.clone().map(|n| (n, a.id)))
        .collect();
    let contacts = session
        .contacts()
        .list_all(&ContactFilter::default())
        .await?
        .into_iter()
        .map(|c| (c.fields.display_name.clone(), c.id))
        .collect();
    let names = Names { accounts, contacts };
    let mut posted = Posted::default();
    for g in &story.tag_groups {
        let group = session
            .tags()
            .create_group(
                &TagGroupFields {
                    name: g.name.clone(),
                    description: None,
                    is_single_select: false,
                },
                IdempotencyKey::new(),
            )
            .await?;
        posted.tag_groups.insert(g.name.clone(), group.id);
        for t in &g.tags {
            let tag = session
                .tags()
                .create(
                    &TagFields {
                        name: t.name.clone(),
                        color: None,
                        group_id: Some(group.id),
                        description: None,
                        tag_number: Some(t.number.clone()),
                    },
                    IdempotencyKey::new(),
                )
                .await?
                .record;
            posted.tags.insert(t.name.clone(), tag.id);
        }
    }
    let tag = |l: &StoryLine| -> Result<Option<TagId>, CompanyError> {
        l.tag
            .as_ref()
            .map(|n| posted.tags.get(n).copied().ok_or_else(|| missing(n)))
            .transpose()
    };
    let one = Quantity::parse_wire("1").map_err(|_| missing("quantity"))?;
    let plain = PostingOptions::default();
    for s in &story.steps {
        let id = match s.kind.as_str() {
            "invoice" | "credit_memo" => {
                let mut lines = Vec::new();
                for l in &s.lines {
                    let mut x = InvoiceLine::account(
                        names.account(&l.account)?,
                        one,
                        amount(l.amount, &s.id)?,
                    );
                    x.tag_id = tag(l)?;
                    lines.push(x);
                }
                let client = names.contact(s.client.as_deref())?;
                if s.kind == "invoice" {
                    let mut n = NewInvoice::new(client, s.date, lines);
                    n.memo.clone_from(&s.memo);
                    session
                        .invoices()
                        .create(&n, &plain, IdempotencyKey::new())
                        .await?
                        .invoice
                        .record
                        .txn
                        .id
                } else {
                    let mut n = NewCreditMemo::new(client, s.date, lines);
                    n.memo.clone_from(&s.memo);
                    session
                        .credit_memos()
                        .create(&n, &plain, IdempotencyKey::new())
                        .await?
                        .id
                }
            }
            "payment" | "deposit_held" => {
                let client = names.contact(s.client.as_deref())?;
                let bank = names.account("1010")?;
                let mut r = PaymentPreviewRequest {
                    customer_id: client,
                    date: s.date,
                    amount: s.amount,
                    deposit_to: Some(bank),
                    applications: None,
                    credit_applications: Vec::new(),
                    hold_as_deposit: s.kind == "deposit_held",
                    editing_payment_id: None,
                };
                if s.kind == "payment" {
                    let mut apps = Vec::new();
                    for a in &s.apply {
                        let inv = a
                            .invoice
                            .as_ref()
                            .and_then(|i| posted.transactions.get(i))
                            .copied()
                            .ok_or_else(|| missing(&s.id))?;
                        apps.push(cl_api::ApplicationRequest {
                            invoice_id: inv,
                            amount: a.amount,
                            discount_amount: None,
                            write_off_amount: None,
                        });
                    }
                    r.applications = Some(apps);
                }
                let p = session.customer_payments().preview(&r).await?;
                // The preview's amounts and balance tokens, unchanged (what
                // the form saves, Q-P21s).
                let save = NewCustomerPayment {
                    customer_id: client,
                    date: s.date,
                    amount: p.amount,
                    deposit_to: bank,
                    payment_method: Some("check".into()),
                    ref_no: None,
                    memo: s.memo.clone(),
                    applications: p
                        .invoices
                        .iter()
                        .filter(|r| r.payment.is_positive())
                        .map(|r| SaveApplication {
                            invoice_id: r.invoice_id,
                            amount: r.payment,
                            expected_balance_due: r.balance_due_exact.clone(),
                            discount_amount: None,
                            write_off_amount: None,
                        })
                        .collect(),
                    credit_applications: Vec::new(),
                    write_off_account_id: None,
                    discount_account_id: None,
                    hold_as_deposit: s.kind == "deposit_held",
                    bank_feed_item_id: None,
                };
                session
                    .customer_payments()
                    .receive(&save, &plain, IdempotencyKey::new())
                    .await?
                    .id
            }
            "bill" | "vendor_credit" => {
                let vendor = names.contact(s.vendor.as_deref())?;
                let mut lines = Vec::new();
                let mut total = Money::ZERO;
                for l in &s.lines {
                    let a = amount(l.amount, &s.id)?;
                    total = total.checked_add(a).map_err(|_| missing(&s.id))?;
                    let mut x = BillLineDraft::expense(names.account(&l.account)?, a);
                    x.tag = tag(l)?;
                    lines.push(x);
                }
                if s.kind == "bill" {
                    let draft = BillDraft {
                        vendor,
                        date: s.date,
                        vendor_invoice_number: s.number.clone(),
                        terms: None,
                        due_date: s.due,
                        memo: s.memo.clone(),
                        amount_due: total,
                        lines,
                        received: true,
                        draft_attachment: None,
                    };
                    session
                        .bills()
                        .create(&draft, &plain, IdempotencyKey::new())
                        .await?
                        .record
                        .txn
                        .id
                } else {
                    let draft = VendorCreditDraft {
                        vendor,
                        date: s.date,
                        vendor_invoice_number: s.number.clone(),
                        memo: s.memo.clone(),
                        amount: total,
                        lines,
                        draft_attachment: None,
                    };
                    session
                        .vendor_credits()
                        .create(&draft, &plain, IdempotencyKey::new())
                        .await?
                        .record
                        .txn
                        .id
                }
            }
            "pay_bills" => {
                let step = |k: Option<&String>| -> Result<TransactionId, CompanyError> {
                    k.and_then(|k| posted.transactions.get(k))
                        .copied()
                        .ok_or_else(|| missing(&s.id))
                };
                let mut bills = Vec::new();
                for b in &s.bills {
                    bills.push(BillToPay {
                        bill: step(b.bill.as_ref())?,
                        cash: amount(b.cash, &s.id)?,
                        discount: Money::ZERO,
                    });
                }
                let mut credits = Vec::new();
                for c in &s.credits {
                    credits.push(CreditToApply {
                        credit: step(c.credit.as_ref())?,
                        bill: step(c.bill.as_ref())?,
                        amount: amount(c.amount, &s.id)?,
                    });
                }
                let paid = session
                    .bill_payments()
                    .pay(
                        &PayBillsRequest {
                            bank_account: names.account("1010")?,
                            date: s.date,
                            bills,
                            credits,
                            discount_account: None,
                            method: PayMethod::Check(CheckNumbering::PrintLater),
                            memo: s.memo.clone(),
                        },
                        &plain,
                        IdempotencyKey::new(),
                    )
                    .await?;
                paid.first().map(|p| p.id).ok_or_else(|| missing(&s.id))?
            }
            "journal_entry" => {
                let mut lines = Vec::new();
                for l in &s.lines {
                    let side = match (l.debit, l.credit) {
                        (Some(d), None) => Side::Debit(d),
                        (None, Some(c)) => Side::Credit(c),
                        _ => return Err(missing(&s.id)),
                    };
                    lines.push(JeLine {
                        account: names.account(&l.account)?,
                        side,
                        description: None,
                        tag: tag(l)?,
                        contact: None,
                    });
                }
                let entry = NewJournalEntry::new_with(
                    s.date,
                    s.memo.clone(),
                    Basis::Both,
                    lines,
                    JeOptions {
                        number: None,
                        adjusting: s.adjusting,
                        reverse_on: None,
                    },
                    &cl_api::AccountRoles::default(),
                )
                .map_err(|_| missing(&s.id))?;
                let opts = if s.closed_period {
                    PostingOptions {
                        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new(
                            crate::sample::SAMPLE_CLOSING_PASSWORD,
                        ))),
                    }
                } else {
                    PostingOptions::default()
                };
                session
                    .transactions()
                    .create_journal_entry(&entry, &opts, IdempotencyKey::new())
                    .await?
                    .record
                    .id
            }
            "check" => {
                let vendor = names.contact(s.vendor.as_deref())?;
                let mut lines = Vec::new();
                let mut total = Money::ZERO;
                for l in &s.lines {
                    let a = amount(l.amount, &s.id)?;
                    total = total.checked_add(a).map_err(|_| missing(&s.id))?;
                    lines.push(cl_api::SplitLine {
                        account: names.account(&l.account)?,
                        amount: a,
                        memo: None,
                        contact: None,
                        billable: false,
                        tag: tag(l)?,
                        item: None,
                    });
                }
                let draft = cl_api::CheckDraft {
                    bank: names.account("1010")?,
                    date: s.date,
                    payee: Some(vendor),
                    payee_name: s.vendor.clone().unwrap_or_default(),
                    check_number: s.number.as_deref().and_then(|n| n.parse().ok()),
                    print_later: s.number.is_none(),
                    payee_address: None,
                    memo: s.memo.clone(),
                    printed_memo: s.memo.clone(),
                    tag_ids: Vec::new(),
                    draft_attachment: None,
                    amount: total,
                    lines,
                };
                let p = cl_api::BankPosting::check(draft).map_err(|_| missing(&s.id))?;
                session
                    .transactions()
                    .create_bank_posting(&p, &plain, IdempotencyKey::new())
                    .await?
                    .record
                    .id
            }
            other => return Err(missing(other)),
        };
        posted.transactions.insert(s.id.clone(), id);
    }
    if let Some(b) = &story.budget
        && session
            .client()
            .capabilities()
            .has(cl_api::Capabilities::BUDGETS_EXACT)
    {
        posted.budget_id = Some(post_budget(session, &names, b).await?);
    }
    Ok(posted)
}

async fn post_budget(
    session: &Session,
    names: &Names,
    b: &StoryBudget,
) -> Result<String, CompanyError> {
    use cl_api::resources::reports::budgets::{BudgetLineWrite, NewBudget, PERIODS};
    let budgets = session.budgets();
    let made = budgets
        .create(
            &NewBudget {
                name: b.name.clone(),
                fiscal_year: b.fiscal_year,
            },
            IdempotencyKey::new(),
        )
        .await?;
    let mut lines = Vec::new();
    for (number, cells) in &b.lines {
        let periods: [Money; PERIODS] = cells.clone().try_into().map_err(|_| missing(number))?;
        lines.push(BudgetLineWrite {
            account_id: names.account(number)?,
            periods,
        });
    }
    budgets
        .save_lines(&made.id, made.version, &lines, IdempotencyKey::new())
        .await?;
    Ok(made.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_story_is_dated_after_october_except_the_closed_period_reclass() {
        let s = ReportStory::load();
        let nov = CalendarDate::from_ymd(2026, 11, 1).expect("d");
        let dec31 = CalendarDate::from_ymd(2026, 12, 31).expect("d");
        for step in &s.steps {
            if step.closed_period {
                // The reclass moves a balance between two asset accounts:
                // no income or expense, so every mockup figure holds.
                assert!(step.date <= CalendarDate::from_ymd(2026, 9, 30).expect("d"));
                assert!(step.lines.iter().all(|l| l.account.starts_with('1')));
            } else {
                assert!(step.date >= nov && step.date <= dec31, "{}", step.id);
            }
        }
        // Every journal entry balances.
        for step in s.steps.iter().filter(|x| x.kind == "journal_entry") {
            let d = Money::sum(step.lines.iter().filter_map(|l| l.debit)).expect("sum");
            let c = Money::sum(step.lines.iter().filter_map(|l| l.credit)).expect("sum");
            assert_eq!(d, c, "{}", step.id);
        }
        // Later steps name earlier ones.
        let mut seen = std::collections::HashSet::new();
        for step in &s.steps {
            for a in step.apply.iter().chain(&step.bills).chain(&step.credits) {
                for r in [&a.invoice, &a.bill, &a.credit].into_iter().flatten() {
                    assert!(seen.contains(r), "{} names {r} before it", step.id);
                }
            }
            assert!(seen.insert(step.id.clone()), "ids are unique");
        }
    }
}
