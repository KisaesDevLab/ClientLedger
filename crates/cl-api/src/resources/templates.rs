//! Stored transaction templates (P15 lane T, U-62, `templates-v1`):
//! `/templates` CRUD, groups, `from-transaction`, `GET /{id}/draft`,
//! `enter`, `skip` and `resolve`.
//!
//! A template is the exact create body of its form's engine endpoint, minus
//! the date and numbers (`target/plans/P15-engine-templates.md` §3), so
//! [`TemplateDraft`] wraps the same checked drafts the forms already save,
//! and [`TemplateDraft::to_value`] runs the same checked builders
//! (`BankPosting::*`, `NewJournalEntry`, the daily-sales body) before
//! anything is sent: a template can never hold what Save would refuse.
//!
//! - Amounts go out as `Money::to_wire()` strings and come back through the
//!   strict `Money::parse_wire`; a JSON number is a parse error. The list
//!   `total` is display-only and never summed.
//! - A deposit that takes customer payments in whole (grouped) cannot be a
//!   template: `Local(TemplateGrouping)`, nothing sent (Q-P15e).
//! - Without `templates-v1` every call is refused locally
//!   (`Local(TemplatesUnavailable)`), nothing sent.
//! - A reference the engine dropped from a draft (an inactive or deleted
//!   account) comes back as [`MISSING_ACCOUNT`] (and its `warnings`
//!   entry), never as some other account: the form shows the field empty.
//! - Names, memos and amounts are never logged; `Debug` shows ids only.

use std::fmt;

use cl_core::ids::{
    AccountId, ContactId, DailyLineId, DailyTemplateId, TemplateGroupId, TemplateId, TransactionId,
};
use cl_core::{CalendarDate, Money};
use serde_json::{Map, Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::bank_postings::{
    BankPosting, CardChargeDraft, CashBack, CheckDraft, DepositCaps, DepositDraft, SplitLine,
    TransferDraft,
};
use crate::resources::daily_sales::{DailyEntryDraft, DailyValue, draft_body};
use crate::resources::operations::{
    TEMPLATE_DELETE, TEMPLATE_DRAFT, TEMPLATE_ENTER, TEMPLATE_GET, TEMPLATE_GROUP_DELETE,
    TEMPLATE_GROUP_PUT, TEMPLATE_GROUPS, TEMPLATE_GROUPS_CREATE, TEMPLATE_PUT, TEMPLATE_RESOLVE,
    TEMPLATE_SKIP, TEMPLATES_CREATE, TEMPLATES_FROM_TXN, TEMPLATES_LIST,
};
use crate::resources::posting::{PostingOptions, posting_body};
use crate::resources::transactions::{
    AccountRoles, Basis, JeLine, JeOptions, NewJournalEntry, Side,
};
use crate::resources::versioning::saved_unreadable;
use crate::session::{Session, json_body};
use crate::wire::Capabilities;

/// The longest template name the engine takes (characters).
pub const TEMPLATE_NAME_MAX: usize = 255;
/// The longest group name the engine takes (characters).
pub const TEMPLATE_GROUP_NAME_MAX: usize = 100;
/// The most templates one list call returns.
pub const TEMPLATE_PAGE_MAX: u32 = 500;
/// Most days between occurrences ("Every N").
pub const INTERVAL_MAX: u16 = 365;
/// Most times a schedule can be set to run ("after N times").
pub const COUNT_MAX: u32 = 9999;

/// The account a draft names when the engine removed the one it had
/// (`NOT_FOUND` / inactive, listed in the draft's warnings). It is never a
/// real account: a form shows the field empty, and Save refuses it.
pub const MISSING_ACCOUNT: AccountId = AccountId::from_uuid(uuid::Uuid::nil());

/// The daily-sales layout a draft names when the engine removed it.
pub const MISSING_LAYOUT: DailyTemplateId = DailyTemplateId::from_uuid(uuid::Uuid::nil());

/// What a template stores: one form's checked draft (its date and numbers
/// are ignored on the wire). A card credit is a [`TemplateDraft::Deposit`]
/// into the card, told apart by the template's `clientForm`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateDraft {
    /// Write Check (`check`).
    Check(CheckDraft),
    /// Card charge or other non-check payment (`expense`).
    CardCharge(CardChargeDraft),
    /// Make Deposit or card credit (`deposit`; never with grouped payments).
    Deposit(DepositDraft),
    /// Transfer Funds (`transfer`).
    Transfer(TransferDraft),
    /// Make Journal Entry (`journal_entry`; never with Reverse on).
    Journal(NewJournalEntry),
    /// Daily Sales Summary (`daily_sales`; never "Enter automatically").
    DailySales(DailyEntryDraft),
}

/// The engine's kind of template (`kind`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TemplateKind {
    /// `check`.
    Check,
    /// `expense` (a card charge or other payment that is not a check).
    Expense,
    /// `deposit` (a deposit or a card credit).
    Deposit,
    /// `transfer`.
    Transfer,
    /// `journal_entry`.
    JournalEntry,
    /// `daily_sales`.
    DailySales,
    /// `legacy`: a schedule made in the online `MyBooks` that the desktop shows
    /// read-only (Q-P15m).
    Legacy,
    /// A kind this version does not know (shown read-only).
    Other(String),
}

impl TemplateKind {
    /// The wire word.
    #[must_use]
    pub fn as_wire(&self) -> &str {
        match self {
            TemplateKind::Check => "check",
            TemplateKind::Expense => "expense",
            TemplateKind::Deposit => "deposit",
            TemplateKind::Transfer => "transfer",
            TemplateKind::JournalEntry => "journal_entry",
            TemplateKind::DailySales => "daily_sales",
            TemplateKind::Legacy => "legacy",
            TemplateKind::Other(s) => s,
        }
    }

    /// From the wire word.
    #[must_use]
    pub fn from_wire(s: &str) -> TemplateKind {
        match s {
            "check" => TemplateKind::Check,
            "expense" => TemplateKind::Expense,
            "deposit" => TemplateKind::Deposit,
            "transfer" => TemplateKind::Transfer,
            "journal_entry" => TemplateKind::JournalEntry,
            "daily_sales" => TemplateKind::DailySales,
            "legacy" => TemplateKind::Legacy,
            other => TemplateKind::Other(other.to_string()),
        }
    }

    /// The desktop can open, change and enter it (not legacy or unknown).
    #[must_use]
    pub fn is_editable(&self) -> bool {
        !matches!(self, TemplateKind::Legacy | TemplateKind::Other(_))
    }
}

/// Which desktop form a template opens in (`clientForm`, an opaque hint the
/// engine returns as stored).
pub mod client_form {
    /// Write Check.
    pub const WRITE_CHECK: &str = "write_check";
    /// Record Card Charge.
    pub const CARD_CHARGE: &str = "card_charge";
    /// Record Card Credit (a deposit into the card).
    pub const CARD_CREDIT: &str = "card_credit";
    /// Make Deposit.
    pub const DEPOSIT: &str = "deposit";
    /// Transfer Funds.
    pub const TRANSFER: &str = "transfer";
    /// Make Journal Entry.
    pub const JOURNAL_ENTRY: &str = "journal_entry";
    /// Daily Sales Summary.
    pub const DAILY_SALES: &str = "daily_sales";
}

impl TemplateDraft {
    /// The engine's kind for this draft.
    #[must_use]
    pub fn kind(&self) -> TemplateKind {
        match self {
            TemplateDraft::Check(_) => TemplateKind::Check,
            TemplateDraft::CardCharge(_) => TemplateKind::Expense,
            TemplateDraft::Deposit(_) => TemplateKind::Deposit,
            TemplateDraft::Transfer(_) => TemplateKind::Transfer,
            TemplateDraft::Journal(_) => TemplateKind::JournalEntry,
            TemplateDraft::DailySales(_) => TemplateKind::DailySales,
        }
    }

    /// The form a draft of this kind opens in by default (a card credit
    /// names [`client_form::CARD_CREDIT`] itself).
    #[must_use]
    pub fn client_form(&self) -> &'static str {
        match self {
            TemplateDraft::Check(_) => client_form::WRITE_CHECK,
            TemplateDraft::CardCharge(_) => client_form::CARD_CHARGE,
            TemplateDraft::Deposit(_) => client_form::DEPOSIT,
            TemplateDraft::Transfer(_) => client_form::TRANSFER,
            TemplateDraft::Journal(_) => client_form::JOURNAL_ENTRY,
            TemplateDraft::DailySales(_) => client_form::DAILY_SALES,
        }
    }

    /// "Enter automatically" is possible (never for daily sales, Q-P15d).
    #[must_use]
    pub fn allows_auto(&self) -> bool {
        !matches!(self, TemplateDraft::DailySales(_))
    }

    /// The draft as the engine stores it: the kind's own checked create
    /// body without the date and numbers (check number, entry or reference
    /// number, Reverse on, a draft attachment). `caps` decides what the
    /// checked builders take, as on Save.
    ///
    /// # Errors
    /// `TemplateGrouping` for a deposit with grouped payments (Q-P15e);
    /// otherwise the kind's own local refusals (`TooFewLines`,
    /// `NonPositiveAmount`, `TotalMismatch`, `Unbalanced`, …). Nothing is
    /// sent for any of them.
    pub fn to_value(&self, caps: &Capabilities) -> Result<Value, LocalValidation> {
        let billable = caps.has(Capabilities::LINE_BILLABLE);
        let (mut v, strip): (Value, &[&str]) = match self {
            TemplateDraft::Check(d) => {
                let mut v = BankPosting::check_with(d.clone(), billable)?.create_value();
                // Always the header tags, so the template keeps exactly
                // these (Q-181).
                v["tagIds"] = json!(d.tag_ids);
                (
                    v,
                    &["txnDate", "checkNumber", "draftAttachmentId", "txnType"],
                )
            }
            TemplateDraft::CardCharge(d) => {
                let mut v = BankPosting::card_charge_with(d.clone(), billable)?.create_value();
                // The card form's header tags travel on its lines (P14),
                // so the header list is empty (E3).
                v["tagIds"] = json!([]);
                (v, &["txnDate", "txnNumber", "txnType"])
            }
            TemplateDraft::Deposit(d) => {
                if !d.payments.is_empty() {
                    return Err(LocalValidation::TemplateGrouping);
                }
                let v = BankPosting::deposit_with(d.clone(), DepositCaps::from_caps(caps))?
                    .create_value();
                (v, &["txnDate", "txnType", "strictAmounts"])
            }
            TemplateDraft::Transfer(d) => (
                BankPosting::transfer(d.clone())?.create_value(),
                &["txnDate", "txnType"],
            ),
            TemplateDraft::Journal(j) => (
                j.to_value(),
                &["txnDate", "txnType", "txnNumber", "reverseOn"],
            ),
            TemplateDraft::DailySales(d) => {
                let v = draft_body(d, true).map_err(|e| match e {
                    ApiError::Local(v) => v,
                    _ => LocalValidation::InvalidValue,
                })?;
                (v, &["businessDate"])
            }
        };
        if let Value::Object(map) = &mut v {
            for k in strip {
                map.remove(*k);
            }
        }
        Ok(v)
    }

    /// Reads a stored draft of `kind`, dated `date` (the form's date; the
    /// engine stores none). Amounts must be exact strings.
    ///
    /// # Errors
    /// `Decode` for a JSON number amount, a malformed id or amount, or a
    /// kind the desktop does not open (legacy).
    pub fn from_value(
        kind: &TemplateKind,
        date: CalendarDate,
        v: &Value,
    ) -> Result<TemplateDraft, ApiError> {
        let r = Reader(v);
        Ok(match kind {
            TemplateKind::Check => TemplateDraft::Check(CheckDraft {
                bank: r.account("bankAccountId")?,
                date,
                payee: r.id("contactId")?,
                payee_name: r.text("payeeNameOnCheck")?.unwrap_or_default(),
                check_number: None,
                print_later: r.flag("printLater")?,
                payee_address: r.text("payeeAddress")?,
                memo: r.text("memo")?,
                printed_memo: r.text("printedMemo")?,
                tag_ids: r.ids("tagIds")?,
                draft_attachment: None,
                amount: r.money("amount")?,
                lines: r.split_lines("accountId")?,
            }),
            TemplateKind::Expense => TemplateDraft::CardCharge(CardChargeDraft {
                pay_from: r.account("payFromAccountId")?,
                date,
                number: None,
                payee: r.id("contactId")?,
                memo: r.text("memo")?,
                lines: r.split_lines("expenseAccountId")?,
            }),
            TemplateKind::Deposit => {
                let mut lines = Vec::new();
                let mut deductions = Vec::new();
                for l in r.lines()? {
                    let mut s = l.split_line("accountId")?;
                    if s.amount.is_negative() {
                        // A fee: negative on the wire, positive here.
                        s.amount = s.amount.abs();
                        deductions.push(s);
                    } else {
                        lines.push(s);
                    }
                }
                let cash_back = match v.get("cashBack") {
                    None | Some(Value::Null) => None,
                    Some(c) => {
                        let c = Reader(c);
                        Some(CashBack {
                            account: c.account("accountId")?,
                            amount: c.money("amount")?,
                            memo: c.text("memo")?,
                        })
                    }
                };
                TemplateDraft::Deposit(DepositDraft {
                    account: r.account("depositToAccountId")?,
                    date,
                    memo: r.text("memo")?,
                    payments: Vec::new(),
                    lines,
                    deductions,
                    cash_back,
                })
            }
            TemplateKind::Transfer => TemplateDraft::Transfer(TransferDraft {
                from: r.account("fromAccountId")?,
                to: r.account("toAccountId")?,
                date,
                amount: r.money("amount")?,
                memo: r.text("memo")?,
            }),
            TemplateKind::JournalEntry => TemplateDraft::Journal(journal_from(&r, date)?),
            TemplateKind::DailySales => {
                let layout = r
                    .id::<DailyTemplateId>("templateId")?
                    .unwrap_or(MISSING_LAYOUT);
                let mut values = Vec::new();
                for l in r.array("values")? {
                    let l = Reader(l);
                    let Some(line) = l.id::<DailyLineId>("templateLineId")? else {
                        continue;
                    };
                    values.push(DailyValue {
                        line,
                        amount: l.money("amount")?,
                        tag: l.id("tagId")?,
                    });
                }
                TemplateDraft::DailySales(DailyEntryDraft {
                    template: layout,
                    date,
                    tag: r.id("tagId")?,
                    notes: r.text("notes")?,
                    values,
                })
            }
            TemplateKind::Legacy | TemplateKind::Other(_) => {
                return Err(decode());
            }
        })
    }
}

/// A journal entry template: lines with exactly one side each. Not
/// checked for balance here (the engine refuses an unbalanced draft); the
/// form's own check runs before any save.
fn journal_from(r: &Reader<'_>, date: CalendarDate) -> Result<NewJournalEntry, ApiError> {
    let mut lines = Vec::new();
    for l in r.lines()? {
        let debit = l.money("debit")?;
        let credit = l.money("credit")?;
        let side = match (debit.is_positive(), credit.is_positive()) {
            (true, false) => Side::Debit(debit),
            (false, true) => Side::Credit(credit),
            _ => return Err(decode()),
        };
        lines.push(JeLine {
            account: l.account("accountId")?,
            side,
            description: l.text("description")?,
            tag: l.id("tagId")?,
            contact: l.id("contactId")?,
        });
    }
    let basis = match r.text("basis")?.as_deref() {
        Some("cash") => Basis::Cash,
        Some("accrual") => Basis::Accrual,
        _ => Basis::Both,
    };
    // Adjusting travels with the template (Q-P15g); the number and
    // Reverse on never do.
    let opts = JeOptions {
        adjusting: r.flag("isAdjusting")?,
        ..JeOptions::default()
    };
    NewJournalEntry::new_with(
        date,
        r.text("memo")?,
        basis,
        lines,
        opts,
        &AccountRoles::default(),
    )
    .map_err(|_| decode())
}

fn decode() -> ApiError {
    ApiError::Decode {
        what: "template draft",
    }
}

/// Strict field reads over one JSON object of a draft.
struct Reader<'a>(&'a Value);

impl<'a> Reader<'a> {
    fn get(&self, key: &str) -> Option<&'a Value> {
        self.0.get(key).filter(|v| !v.is_null())
    }

    fn text(&self, key: &str) -> Result<Option<String>, ApiError> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::String(s)) if s.is_empty() => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(decode()),
        }
    }

    fn flag(&self, key: &str) -> Result<bool, ApiError> {
        match self.get(key) {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(decode()),
        }
    }

    fn id<T: std::str::FromStr>(&self, key: &str) -> Result<Option<T>, ApiError> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::String(s)) => s.parse().map(Some).map_err(|_| decode()),
            Some(_) => Err(decode()),
        }
    }

    /// An account the engine may have removed: [`MISSING_ACCOUNT`] then.
    fn account(&self, key: &str) -> Result<AccountId, ApiError> {
        Ok(self.id(key)?.unwrap_or(MISSING_ACCOUNT))
    }

    fn ids<T: std::str::FromStr>(&self, key: &str) -> Result<Vec<T>, ApiError> {
        let mut out = Vec::new();
        for v in self.array(key)? {
            match v {
                Value::Null => {}
                Value::String(s) => out.push(s.parse().map_err(|_| decode())?),
                _ => return Err(decode()),
            }
        }
        Ok(out)
    }

    /// An exact amount: a decimal string only (a JSON number is refused).
    fn money(&self, key: &str) -> Result<Money, ApiError> {
        match self.get(key) {
            Some(Value::String(s)) => Money::parse_wire(s).map_err(|_| decode()),
            _ => Err(decode()),
        }
    }

    fn array(&self, key: &str) -> Result<&'a [Value], ApiError> {
        match self.get(key) {
            None => Ok(&[]),
            Some(Value::Array(a)) => Ok(a),
            Some(_) => Err(decode()),
        }
    }

    fn lines(&self) -> Result<Vec<Reader<'a>>, ApiError> {
        Ok(self.array("lines")?.iter().map(Reader).collect())
    }

    fn split_line(&self, account_key: &str) -> Result<SplitLine, ApiError> {
        Ok(SplitLine {
            account: self.account(account_key)?,
            amount: self.money("amount")?,
            memo: self.text("description")?,
            contact: self.id("contactId")?,
            billable: self.flag("isBillable")?,
            tag: self.id("tagId")?,
        })
    }

    fn split_lines(&self, account_key: &str) -> Result<Vec<SplitLine>, ApiError> {
        self.lines()?
            .iter()
            .map(|l| l.split_line(account_key))
            .collect()
    }
}

// ------------------------------------------------------------------ schedule

/// What happens on a template's dates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScheduleMode {
    /// "Remind me": listed in Templates Due, never posted by itself.
    Remind,
    /// "Enter automatically": the catch-up run posts it (never in a
    /// closed period).
    Auto,
}

impl ScheduleMode {
    fn as_wire(self) -> &'static str {
        match self {
            ScheduleMode::Remind => "remind",
            ScheduleMode::Auto => "auto",
        }
    }
}

/// How often.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Frequency {
    /// Every N days.
    Daily,
    /// Every N weeks.
    Weekly,
    /// Every two weeks (× N).
    Biweekly,
    /// The 1st and the 15th.
    Semimonthly,
    /// Every N months (clamped to the month end, Q-P15j).
    Monthly,
    /// Every N quarters.
    Quarterly,
    /// Every N years.
    Annually,
}

impl Frequency {
    /// Every frequency, in the order the dialog lists them.
    pub const ALL: [Frequency; 7] = [
        Frequency::Daily,
        Frequency::Weekly,
        Frequency::Biweekly,
        Frequency::Semimonthly,
        Frequency::Monthly,
        Frequency::Quarterly,
        Frequency::Annually,
    ];

    /// The wire word.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Frequency::Daily => "daily",
            Frequency::Weekly => "weekly",
            Frequency::Biweekly => "biweekly",
            Frequency::Semimonthly => "semimonthly",
            Frequency::Monthly => "monthly",
            Frequency::Quarterly => "quarterly",
            Frequency::Annually => "annually",
        }
    }

    /// From the wire word.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Frequency> {
        Frequency::ALL.into_iter().find(|f| f.as_wire() == s)
    }
}

/// When a schedule stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScheduleEnd {
    /// It runs until the owner stops it.
    Never,
    /// No date after this one.
    On(CalendarDate),
    /// After this many more times (the engine counts down).
    After(u32),
}

/// A template's schedule. "Don't remind me" is no schedule at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TemplateSchedule {
    /// Remind or enter automatically.
    pub mode: ScheduleMode,
    /// How often.
    pub frequency: Frequency,
    /// Every N (1–365).
    pub interval: u16,
    /// The next date it is due (the engine's answer after a save is what
    /// the list shows).
    pub next_date: CalendarDate,
    /// When it stops.
    pub end: ScheduleEnd,
}

impl TemplateSchedule {
    /// The schedule body.
    ///
    /// # Errors
    /// `InvalidValue` for an interval outside 1–365, a count outside
    /// 1–9999 or an end date before the next date (nothing sent).
    pub fn to_value(&self) -> Result<Value, LocalValidation> {
        if !(1..=INTERVAL_MAX).contains(&self.interval) {
            return Err(LocalValidation::InvalidValue);
        }
        let mut v = json!({
            "mode": self.mode.as_wire(),
            "frequency": self.frequency.as_wire(),
            "intervalValue": self.interval,
            "nextDate": self.next_date.to_wire(),
        });
        match self.end {
            ScheduleEnd::Never => {}
            ScheduleEnd::On(d) => {
                if d < self.next_date {
                    return Err(LocalValidation::InvalidValue);
                }
                v["endDate"] = json!(d.to_wire());
            }
            ScheduleEnd::After(n) => {
                if !(1..=COUNT_MAX).contains(&n) {
                    return Err(LocalValidation::InvalidValue);
                }
                v["remainingCount"] = json!(n);
            }
        }
        Ok(v)
    }

    fn from_value(v: &Value) -> Result<Option<TemplateSchedule>, ApiError> {
        if v.is_null() {
            return Ok(None);
        }
        let bad = || ApiError::Decode {
            what: "template schedule",
        };
        let r = Reader(v);
        let mode = match r.text("mode").map_err(|_| bad())?.as_deref() {
            Some("auto") => ScheduleMode::Auto,
            Some("remind" | "reminder") => ScheduleMode::Remind,
            _ => return Err(bad()),
        };
        let frequency = r
            .text("frequency")
            .map_err(|_| bad())?
            .as_deref()
            .and_then(Frequency::from_wire)
            .ok_or_else(bad)?;
        let interval = match r.get("intervalValue") {
            None => 1,
            Some(n) => n
                .as_u64()
                .and_then(|n| u16::try_from(n).ok())
                .ok_or_else(bad)?,
        };
        let date = |key: &str| -> Result<Option<CalendarDate>, ApiError> {
            match r.text(key).map_err(|_| bad())? {
                None => Ok(None),
                Some(s) => CalendarDate::parse_wire(&s).map(Some).map_err(|_| bad()),
            }
        };
        let next_date = date("nextDate")?.ok_or_else(bad)?;
        let end = match (date("endDate")?, r.get("remainingCount")) {
            (Some(d), _) => ScheduleEnd::On(d),
            (None, Some(n)) => ScheduleEnd::After(
                n.as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(bad)?,
            ),
            (None, None) => ScheduleEnd::Never,
        };
        Ok(Some(TemplateSchedule {
            mode,
            frequency,
            interval,
            next_date,
            end,
        }))
    }
}

// ------------------------------------------------------------------ records

/// One template as the list shows it.
#[derive(Clone, PartialEq, Eq)]
pub struct Template {
    /// Its id.
    pub id: TemplateId,
    /// The owner's name for it (owner data: never logged).
    pub name: String,
    /// The engine's kind.
    pub kind: TemplateKind,
    /// The desktop form it opens in, as stored.
    pub client_form: Option<String>,
    /// Its group.
    pub group: Option<TemplateGroupId>,
    /// The group's name.
    pub group_name: Option<String>,
    /// The bank, card or other main account.
    pub account: Option<AccountId>,
    /// The payee or received-from name.
    pub contact: Option<ContactId>,
    /// The header amount (display only; never summed).
    pub total: Option<Money>,
    /// Its schedule; `None` is "Don't remind me".
    pub schedule: Option<TemplateSchedule>,
    /// The last date it was entered.
    pub last_entered: Option<CalendarDate>,
    /// Occurrences that could not be entered and are still open.
    pub open_skipped: u32,
    /// Made in the online `MyBooks` (read-only here, Q-P15m).
    pub legacy: bool,
    /// The row version a change must name (409 `STALE` otherwise).
    pub version: u32,
    /// The stored draft ([`Templates::get`] only; `None` in lists and for
    /// legacy templates).
    pub draft: Option<Value>,
}

/// Ids, the kind and the version only.
impl fmt::Debug for Template {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Template")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("group", &self.group)
            .field("schedule", &self.schedule)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl Template {
    /// The stored draft for a form, dated `date`.
    ///
    /// # Errors
    /// `Decode` when there is no draft (a list row, a legacy template) or
    /// it does not read.
    pub fn draft_for(&self, date: CalendarDate) -> Result<TemplateDraft, ApiError> {
        let v = self.draft.as_ref().ok_or_else(decode)?;
        TemplateDraft::from_value(&self.kind, date, v)
    }

    /// Reads one template object (`templates[]` or `template`).
    ///
    /// # Errors
    /// `Decode` for a missing id or name, or an amount that is not an
    /// exact string.
    pub fn from_value(v: &Value) -> Result<Template, ApiError> {
        let bad = || ApiError::Decode { what: "template" };
        let r = Reader(v);
        let total = match r.get("total") {
            None => None,
            Some(Value::String(s)) => Some(Money::parse_wire(s).map_err(|_| bad())?),
            Some(_) => return Err(bad()),
        };
        let kind = TemplateKind::from_wire(&r.text("kind").map_err(|_| bad())?.ok_or_else(bad)?);
        let legacy = r.flag("legacy").map_err(|_| bad())? || kind == TemplateKind::Legacy;
        let last = match r.text("lastEnteredDate").map_err(|_| bad())? {
            None => None,
            // A timestamp is cut to its date (the engine's own day).
            Some(s) => {
                Some(CalendarDate::parse_wire(s.get(..10).unwrap_or("")).map_err(|_| bad())?)
            }
        };
        let count = |key: &str| -> Result<u32, ApiError> {
            match r.get(key) {
                None => Ok(0),
                Some(Value::Array(a)) => Ok(u32::try_from(a.len()).unwrap_or(u32::MAX)),
                Some(n) => n
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(bad),
            }
        };
        Ok(Template {
            id: r.id("id").map_err(|_| bad())?.ok_or_else(bad)?,
            name: r.text("name").map_err(|_| bad())?.ok_or_else(bad)?,
            kind,
            client_form: r.text("clientForm").map_err(|_| bad())?,
            group: r.id("groupId").map_err(|_| bad())?,
            group_name: r.text("groupName").map_err(|_| bad())?,
            account: r.id("accountId").map_err(|_| bad())?,
            contact: r.id("contactId").map_err(|_| bad())?,
            total,
            schedule: TemplateSchedule::from_value(v.get("schedule").unwrap_or(&Value::Null))?,
            last_entered: last,
            open_skipped: count("openSkipped")?,
            legacy,
            version: count("version")?,
            draft: r.get("draft").cloned(),
        })
    }
}

/// A template group (a folder only, Q-P15a).
#[derive(Clone, PartialEq, Eq)]
pub struct TemplateGroup {
    /// Its id.
    pub id: TemplateGroupId,
    /// Its name.
    pub name: String,
}

/// The id only.
impl fmt::Debug for TemplateGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TemplateGroup")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl TemplateGroup {
    fn from_value(v: &Value) -> Result<TemplateGroup, ApiError> {
        let bad = || ApiError::Decode {
            what: "template group",
        };
        let r = Reader(v);
        Ok(TemplateGroup {
            id: r.id("id").map_err(|_| bad())?.ok_or_else(bad)?,
            name: r.text("name").map_err(|_| bad())?.ok_or_else(bad)?,
        })
    }
}

/// A template to store ([`Templates::create`]).
#[derive(Clone, PartialEq, Eq)]
pub struct NewTemplate {
    /// Its name (required, at most 255 characters, unique per company).
    pub name: String,
    /// Its group.
    pub group: Option<TemplateGroupId>,
    /// The form it opens in; `None` takes the draft's default.
    pub client_form: Option<String>,
    /// The form's checked draft.
    pub draft: TemplateDraft,
    /// Its schedule; `None` is "Don't remind me".
    pub schedule: Option<TemplateSchedule>,
}

/// The draft's kind only.
impl fmt::Debug for NewTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewTemplate")
            .field("kind", &self.draft.kind())
            .field("group", &self.group)
            .field("schedule", &self.schedule)
            .finish_non_exhaustive()
    }
}

/// A change to a template ([`Templates::update`]); `None` leaves a field
/// as it is.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TemplateChange {
    /// New name.
    pub name: Option<String>,
    /// New group (`Some(None)`: no group).
    pub group: Option<Option<TemplateGroupId>>,
    /// New form hint.
    pub client_form: Option<String>,
    /// New draft ("Replace" from a form, Q-P15l).
    pub draft: Option<TemplateDraft>,
    /// New schedule (`Some(None)`: "Don't remind me").
    pub schedule: Option<Option<TemplateSchedule>>,
}

/// Which fields change only.
impl fmt::Debug for TemplateChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TemplateChange")
            .field("name", &self.name.is_some())
            .field("group", &self.group)
            .field("draft", &self.draft.as_ref().map(TemplateDraft::kind))
            .field("schedule", &self.schedule)
            .finish_non_exhaustive()
    }
}

/// A template's draft for one date ([`Templates::draft`]): what "Use" fills
/// a form with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateUse {
    /// The draft, dated by the caller.
    pub draft: TemplateDraft,
    /// The form it opens in, as stored.
    pub client_form: Option<String>,
    /// What the engine flagged in the draft.
    pub warnings: Vec<DraftWarning>,
}

/// What [`Templates::enter`] posted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entered {
    /// The new transaction.
    pub transaction: TransactionId,
    /// The occurrence date it filled (`None`: a template with no schedule,
    /// entered today).
    pub occurrence: Option<CalendarDate>,
}

/// Why a reference in a template's draft needs the owner's attention
/// (`GET /templates/{id}/draft` `warnings[].code`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DraftWarningCode {
    /// `ACCOUNT_INACTIVE`.
    AccountInactive,
    /// `CONTACT_INACTIVE`.
    ContactInactive,
    /// `TAG_INACTIVE`.
    TagInactive,
    /// `NOT_FOUND`: the reference was removed from the draft.
    NotFound,
    /// A code this version does not know.
    Other(String),
}

impl DraftWarningCode {
    fn from_wire(s: &str) -> DraftWarningCode {
        match s {
            "ACCOUNT_INACTIVE" => DraftWarningCode::AccountInactive,
            "CONTACT_INACTIVE" => DraftWarningCode::ContactInactive,
            "TAG_INACTIVE" => DraftWarningCode::TagInactive,
            "NOT_FOUND" => DraftWarningCode::NotFound,
            other => DraftWarningCode::Other(other.to_string()),
        }
    }
}

/// One warning on a template's draft: the draft field (engine path) and
/// why. The form shows it as a banner; nothing is logged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftWarning {
    /// The draft field, as the engine names it (`lines[0].accountId`).
    pub field: String,
    /// Why.
    pub code: DraftWarningCode,
}

/// The engine code of a date that was already entered or posted
/// (409 on `enter`, `resolve`).
pub const OCCURRENCE_TAKEN: &str = "OCCURRENCE_TAKEN";
/// The engine code of a template name that is already used (409).
pub const TEMPLATE_NAME_TAKEN: &str = "TEMPLATE_NAME_TAKEN";

/// True for the engine's "that date was already entered" answer: a retry
/// of a lost `enter` reply, treated as done (§5 Lost reply).
#[must_use]
pub fn is_occurrence_taken(e: &ApiError) -> bool {
    matches!(e, ApiError::Conflict { code: Some(c), .. } if c.as_str() == OCCURRENCE_TAKEN)
}

/// True for "a template with that name already exists".
#[must_use]
pub fn is_name_taken(e: &ApiError) -> bool {
    matches!(e, ApiError::Conflict { code: Some(c), .. } if c.as_str() == TEMPLATE_NAME_TAKEN)
}

// --------------------------------------------------------------------- calls

/// Template calls of a session (`session.templates()`).
#[derive(Clone, Copy, Debug)]
pub struct Templates<'a>(pub(crate) &'a Session);

impl Session {
    /// Templates of the open company.
    #[must_use]
    pub fn templates(&self) -> Templates<'_> {
        Templates(self)
    }
}

fn name_ok(name: &str, max: usize) -> Result<&str, ApiError> {
    let n = name.trim();
    let len = n.chars().count();
    if len == 0 || len > max {
        return Err(ApiError::Local(LocalValidation::InvalidValue));
    }
    Ok(n)
}

fn write_call(method: Method, route: &'static str, path: String, body: Vec<u8>) -> Call {
    let mut call = Call::new(method, route, path).json_body(body);
    call.idempotency_key = Some(IdempotencyKey::new());
    call
}

fn body_of(reply: &[u8], what: &'static str) -> Result<Value, ApiError> {
    serde_json::from_slice(reply).map_err(|_| ApiError::Decode { what })
}

fn template_of(reply: &[u8]) -> Result<Template, ApiError> {
    let v = body_of(reply, "template")?;
    Template::from_value(v.get("template").unwrap_or(&v))
}

impl Templates<'_> {
    fn gate(self) -> Result<(), ApiError> {
        if self.0.client().capabilities().has(Capabilities::TEMPLATES) {
            Ok(())
        } else {
            Err(ApiError::Local(LocalValidation::TemplatesUnavailable))
        }
    }

    /// The draft body for `draft` with this engine's capabilities.
    fn draft_value(self, draft: &TemplateDraft) -> Result<Value, ApiError> {
        draft
            .to_value(&self.0.client().capabilities())
            .map_err(ApiError::Local)
    }

    /// Templates, `limit` (1–500) from `offset`, optionally of one group:
    /// `(rows, total)`.
    ///
    /// # Errors
    /// `Local(TemplatesUnavailable)`, `Local(PageLimit)` (nothing sent);
    /// `Decode`; any [`ApiError`].
    pub async fn list(
        &self,
        group: Option<TemplateGroupId>,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<Template>, u32), ApiError> {
        self.gate()?;
        if !(1..=TEMPLATE_PAGE_MAX).contains(&limit) {
            return Err(ApiError::Local(LocalValidation::PageLimit));
        }
        let mut call = Call::at(Method::Get, TEMPLATES_LIST.route());
        if let Some(g) = group {
            call.query.push(("groupId".into(), g.to_string()));
        }
        call.query.push(("limit".into(), limit.to_string()));
        call.query.push(("offset".into(), offset.to_string()));
        let reply = self.0.send(&TEMPLATES_LIST, call, None).await?;
        let v = body_of(&reply.body, "templates")?;
        let rows = v
            .get("templates")
            .and_then(Value::as_array)
            .ok_or(ApiError::Decode { what: "templates" })?
            .iter()
            .map(Template::from_value)
            .collect::<Result<Vec<_>, _>>()?;
        let total = v
            .get("total")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(u32::try_from(rows.len()).unwrap_or(u32::MAX));
        Ok((rows, total))
    }

    /// Every template (pages of 200 until the total).
    ///
    /// # Errors
    /// As [`Templates::list`].
    pub async fn list_all(&self) -> Result<Vec<Template>, ApiError> {
        let mut out = Vec::new();
        loop {
            let offset = u32::try_from(out.len()).unwrap_or(u32::MAX);
            let (rows, total) = self.list(None, 200, offset).await?;
            let done = rows.is_empty() || offset + u32::try_from(rows.len()).unwrap_or(0) >= total;
            out.extend(rows);
            if done {
                return Ok(out);
            }
        }
    }

    /// One template with its draft and schedule.
    ///
    /// # Errors
    /// `NotFound` (also another company's); `Decode`; any [`ApiError`].
    pub async fn get(&self, id: TemplateId) -> Result<Template, ApiError> {
        self.gate()?;
        let call = Call::new(
            Method::Get,
            TEMPLATE_GET.route(),
            format!("/api/v1/templates/{id}"),
        );
        let reply = self.0.send(&TEMPLATE_GET, call, None).await?;
        template_of(&reply.body)
    }

    /// Stores a template (posts nothing).
    ///
    /// # Errors
    /// Nothing is sent for `Local(TemplatesUnavailable)`,
    /// `Local(TemplateGrouping)`, `Local(InvalidValue)` (an empty or
    /// over-long name, a bad schedule) or the draft's own refusals. From
    /// the engine: `Conflict` ([`is_name_taken`]), `Validation`
    /// (`TEMPLATE_AUTO_UNSUPPORTED`, `TEMPLATE_DEPOSIT_GROUPING`, …); any
    /// [`ApiError`].
    pub async fn create(&self, t: &NewTemplate) -> Result<Template, ApiError> {
        self.gate()?;
        let name = name_ok(&t.name, TEMPLATE_NAME_MAX)?;
        if !t.draft.allows_auto() && t.schedule.is_some_and(|s| s.mode == ScheduleMode::Auto) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut body = json!({
            "name": name,
            "kind": t.draft.kind().as_wire(),
            "clientForm": t.client_form.as_deref().unwrap_or(t.draft.client_form()),
            "draft": self.draft_value(&t.draft)?,
        });
        if let Some(g) = t.group {
            body["groupId"] = json!(g);
        }
        if let Some(s) = &t.schedule {
            body["schedule"] = s.to_value().map_err(ApiError::Local)?;
        }
        let call = write_call(
            Method::Post,
            TEMPLATES_CREATE.route(),
            TEMPLATES_CREATE.route().to_string(),
            json_body(&body)?,
        );
        let reply = self.0.send(&TEMPLATES_CREATE, call, None).await?;
        template_of(&reply.body).map_err(saved_unreadable)
    }

    /// Stores a template from a posted transaction (a snapshot of it).
    ///
    /// # Errors
    /// As [`Templates::create`]; `Validation` (`TEMPLATE_SOURCE_INVALID`,
    /// `TEMPLATE_DEPOSIT_GROUPING`).
    pub async fn from_transaction(
        &self,
        txn: TransactionId,
        name: &str,
        group: Option<TemplateGroupId>,
        schedule: Option<&TemplateSchedule>,
    ) -> Result<Template, ApiError> {
        self.gate()?;
        let mut body = json!({
            "transactionId": txn,
            "name": name_ok(name, TEMPLATE_NAME_MAX)?,
        });
        if let Some(g) = group {
            body["groupId"] = json!(g);
        }
        if let Some(s) = schedule {
            body["schedule"] = s.to_value().map_err(ApiError::Local)?;
        }
        let call = write_call(
            Method::Post,
            TEMPLATES_FROM_TXN.route(),
            TEMPLATES_FROM_TXN.route().to_string(),
            json_body(&body)?,
        );
        let reply = self.0.send(&TEMPLATES_FROM_TXN, call, None).await?;
        template_of(&reply.body).map_err(saved_unreadable)
    }

    /// Changes a template that is still at `version`.
    ///
    /// # Errors
    /// `Conflict { Stale }` when it changed since it was read; otherwise as
    /// [`Templates::create`].
    pub async fn update(
        &self,
        id: TemplateId,
        version: u32,
        c: &TemplateChange,
    ) -> Result<Template, ApiError> {
        self.gate()?;
        let mut body = Map::new();
        body.insert("version".into(), json!(version));
        if let Some(n) = &c.name {
            body.insert("name".into(), json!(name_ok(n, TEMPLATE_NAME_MAX)?));
        }
        if let Some(g) = &c.group {
            body.insert("groupId".into(), json!(g));
        }
        if let Some(f) = &c.client_form {
            body.insert("clientForm".into(), json!(f));
        }
        if let Some(d) = &c.draft {
            body.insert("draft".into(), self.draft_value(d)?);
            if c.client_form.is_none() {
                body.insert("clientForm".into(), json!(d.client_form()));
            }
        }
        if let Some(s) = &c.schedule {
            let v = match s {
                None => Value::Null,
                Some(s) => s.to_value().map_err(ApiError::Local)?,
            };
            body.insert("schedule".into(), v);
        }
        let call = write_call(
            Method::Put,
            TEMPLATE_PUT.route(),
            format!("/api/v1/templates/{id}"),
            json_body(&Value::Object(body))?,
        );
        let reply = self.0.send(&TEMPLATE_PUT, call, None).await?;
        template_of(&reply.body).map_err(saved_unreadable)
    }

    /// Deletes a template that is still at `version` (its posted
    /// transactions stay).
    ///
    /// # Errors
    /// `Conflict { Stale }`; `NotFound`; any [`ApiError`].
    pub async fn delete(&self, id: TemplateId, version: u32) -> Result<(), ApiError> {
        self.gate()?;
        let mut call = Call::new(
            Method::Delete,
            TEMPLATE_DELETE.route(),
            format!("/api/v1/templates/{id}"),
        );
        call.query.push(("version".into(), version.to_string()));
        call.idempotency_key = Some(IdempotencyKey::new());
        self.0.send(&TEMPLATE_DELETE, call, None).await?;
        Ok(())
    }

    /// The template groups by name.
    ///
    /// # Errors
    /// `Decode`; any [`ApiError`].
    pub async fn groups(&self) -> Result<Vec<TemplateGroup>, ApiError> {
        self.gate()?;
        let call = Call::at(Method::Get, TEMPLATE_GROUPS.route());
        let reply = self.0.send(&TEMPLATE_GROUPS, call, None).await?;
        let v = body_of(&reply.body, "template groups")?;
        v.get("groups")
            .and_then(Value::as_array)
            .ok_or(ApiError::Decode {
                what: "template groups",
            })?
            .iter()
            .map(TemplateGroup::from_value)
            .collect()
    }

    /// Creates a group.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for an empty or over-long name; `Conflict` for
    /// a name in use; any [`ApiError`].
    pub async fn create_group(&self, name: &str) -> Result<TemplateGroup, ApiError> {
        self.gate()?;
        let body = json!({ "name": name_ok(name, TEMPLATE_GROUP_NAME_MAX)? });
        let call = write_call(
            Method::Post,
            TEMPLATE_GROUPS_CREATE.route(),
            TEMPLATE_GROUPS_CREATE.route().to_string(),
            json_body(&body)?,
        );
        let reply = self.0.send(&TEMPLATE_GROUPS_CREATE, call, None).await?;
        let v = body_of(&reply.body, "template group").map_err(saved_unreadable)?;
        TemplateGroup::from_value(v.get("group").unwrap_or(&v)).map_err(saved_unreadable)
    }

    /// Renames a group.
    ///
    /// # Errors
    /// As [`Templates::create_group`].
    pub async fn rename_group(
        &self,
        id: TemplateGroupId,
        name: &str,
    ) -> Result<TemplateGroup, ApiError> {
        self.gate()?;
        let body = json!({ "name": name_ok(name, TEMPLATE_GROUP_NAME_MAX)? });
        let call = write_call(
            Method::Put,
            TEMPLATE_GROUP_PUT.route(),
            format!("/api/v1/templates/groups/{id}"),
            json_body(&body)?,
        );
        let reply = self.0.send(&TEMPLATE_GROUP_PUT, call, None).await?;
        let v = body_of(&reply.body, "template group").map_err(saved_unreadable)?;
        TemplateGroup::from_value(v.get("group").unwrap_or(&v)).map_err(saved_unreadable)
    }

    /// Deletes a group; its templates move to no group.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn delete_group(&self, id: TemplateGroupId) -> Result<(), ApiError> {
        self.gate()?;
        let mut call = Call::new(
            Method::Delete,
            TEMPLATE_GROUP_DELETE.route(),
            format!("/api/v1/templates/groups/{id}"),
        );
        call.idempotency_key = Some(IdempotencyKey::new());
        self.0.send(&TEMPLATE_GROUP_DELETE, call, None).await?;
        Ok(())
    }

    /// The template's draft for `date`, with warnings ("Use"; posts
    /// nothing).
    ///
    /// # Errors
    /// `NotFound`; `Decode` (a JSON number amount, a kind the desktop does
    /// not open); any [`ApiError`].
    pub async fn draft(&self, id: TemplateId, date: CalendarDate) -> Result<TemplateUse, ApiError> {
        self.gate()?;
        let mut call = Call::new(
            Method::Get,
            TEMPLATE_DRAFT.route(),
            format!("/api/v1/templates/{id}/draft"),
        );
        call.query.push(("date".into(), date.to_wire()));
        let reply = self.0.send(&TEMPLATE_DRAFT, call, None).await?;
        use_from_json(&reply.body, date)
    }

    /// Posts one occurrence now ("Enter now"; Templates Due "Enter it
    /// now"). `date` is the occurrence (the schedule's next date when
    /// `None`); `opts` carries the closing-date password for a closed
    /// period.
    ///
    /// # Errors
    /// `ClosedPeriod`, `ClosingPasswordRejected`,
    /// `Local(ClosedPeriodOverrideUnavailable)`; `Conflict` with
    /// [`is_occurrence_taken`] (already entered: a lost reply's retry is
    /// done); `Validation` (`OCCURRENCE_NOT_DUE`, a missing account); any
    /// [`ApiError`].
    pub async fn enter(
        &self,
        id: TemplateId,
        date: Option<CalendarDate>,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<Entered, ApiError> {
        self.gate()?;
        let mut body = json!({});
        if let Some(d) = date {
            body["occurrenceDate"] = json!(d.to_wire());
        }
        let body = posting_body(self.0, body, opts)?;
        let mut call = Call::new(
            Method::Post,
            TEMPLATE_ENTER.route(),
            format!("/api/v1/templates/{id}/enter"),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&TEMPLATE_ENTER, call, None).await?;
        entered_from_json(&reply.body).map_err(saved_unreadable)
    }

    /// Dismisses one occurrence ("Skip this one", "Dismiss").
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn skip(&self, id: TemplateId, date: CalendarDate) -> Result<(), ApiError> {
        self.gate()?;
        let call = write_call(
            Method::Post,
            TEMPLATE_SKIP.route(),
            format!("/api/v1/templates/{id}/occurrences/{}/skip", date.to_wire()),
            json_body(&json!({}))?,
        );
        self.0.send(&TEMPLATE_SKIP, call, None).await?;
        Ok(())
    }

    /// Marks an occurrence entered by a transaction the owner saved in a
    /// form ("Open in form", Q-P15p).
    ///
    /// # Errors
    /// `Conflict` ([`is_occurrence_taken`]); `Validation` (another kind,
    /// not posted); any [`ApiError`].
    pub async fn resolve(
        &self,
        id: TemplateId,
        date: CalendarDate,
        txn: TransactionId,
    ) -> Result<(), ApiError> {
        self.gate()?;
        let call = write_call(
            Method::Post,
            TEMPLATE_RESOLVE.route(),
            format!(
                "/api/v1/templates/{id}/occurrences/{}/resolve",
                date.to_wire()
            ),
            json_body(&json!({ "transactionId": txn }))?,
        );
        self.0.send(&TEMPLATE_RESOLVE, call, None).await?;
        Ok(())
    }
}

/// Reads a `GET /templates/{id}/draft` answer for a form dated `date`.
///
/// # Errors
/// `Decode` for an unknown shape, a JSON number amount or a kind the
/// desktop does not open.
pub fn use_from_json(body: &[u8], date: CalendarDate) -> Result<TemplateUse, ApiError> {
    let v = body_of(body, "template draft")?;
    let r = Reader(&v);
    let kind = TemplateKind::from_wire(&r.text("kind")?.ok_or_else(decode)?);
    let draft = TemplateDraft::from_value(&kind, date, v.get("draft").ok_or_else(decode)?)?;
    let mut warnings = Vec::new();
    for w in r.array("warnings")? {
        let w = Reader(w);
        warnings.push(DraftWarning {
            field: w.text("field")?.unwrap_or_default(),
            code: DraftWarningCode::from_wire(&w.text("code")?.unwrap_or_default()),
        });
    }
    Ok(TemplateUse {
        draft,
        client_form: r.text("clientForm")?,
        warnings,
    })
}

fn entered_from_json(body: &[u8]) -> Result<Entered, ApiError> {
    let bad = || ApiError::Decode {
        what: "entered template",
    };
    let v = body_of(body, "entered template")?;
    let transaction = v
        .pointer("/transaction/id")
        .or_else(|| v.get("transactionId"))
        .and_then(Value::as_str)
        .and_then(|s| TransactionId::parse(s).ok())
        .ok_or_else(bad)?;
    let occurrence = v
        .pointer("/occurrence/occurrenceDate")
        .and_then(Value::as_str)
        .map(|s| CalendarDate::parse_wire(s).map_err(|_| bad()))
        .transpose()?;
    Ok(Entered {
        transaction,
        occurrence,
    })
}

#[cfg(test)]
#[path = "templates_tests.rs"]
mod tests;
