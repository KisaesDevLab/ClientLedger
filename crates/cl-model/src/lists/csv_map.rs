//! CSV import mapping (P11 step 6): which list field each column of an
//! owner's file fills, a first guess from the header names, and every data
//! row turned into a checked row the engine import takes.
//!
//! The reader (`cl_import::csv`) hands over text only. Prices are parsed
//! here exactly (`Money`, up to four decimals, never floating point) from
//! the file, which is the amount's source document (non-negotiable 4 and 6).
//! What the engine's import routes keep decides the fields offered:
//! accounts (number, name, type, detail type), contacts (name, company,
//! email, phone; U-46 for the rest) and items (name, description, price,
//! income account, taxable).

use std::collections::HashSet;

use cl_core::ids::AccountId;
use cl_core::{Money, UserInput};

use crate::lists::accounts::AccountKind;
use crate::lists::engine_len;
use crate::text::TextKey;

/// Most account rows one import takes (the engine's single transaction).
pub const ACCOUNT_ROWS_MAX: usize = 2000;
/// Most contact or item rows one import takes.
pub const ROWS_MAX: usize = 10_000;

/// The list a file is imported into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CsvList {
    /// Chart of Accounts.
    Accounts,
    /// Clients and vendors.
    Contacts,
    /// Items.
    Items,
}

impl CsvList {
    /// Every list that imports.
    pub const ALL: [CsvList; 3] = [CsvList::Accounts, CsvList::Contacts, CsvList::Items];

    /// The fields a column can fill, in form order.
    #[must_use]
    pub const fn targets(self) -> &'static [Target] {
        match self {
            CsvList::Accounts => &[
                Target::AccountNumber,
                Target::AccountName,
                Target::AccountType,
                Target::DetailType,
            ],
            CsvList::Contacts => &[
                Target::ContactName,
                Target::Company,
                Target::Email,
                Target::Phone,
            ],
            CsvList::Items => &[
                Target::ItemName,
                Target::Description,
                Target::Price,
                Target::IncomeAccount,
                Target::Taxable,
            ],
        }
    }

    /// Fields a mapping must fill.
    #[must_use]
    pub const fn required(self) -> &'static [Target] {
        match self {
            CsvList::Accounts => &[Target::AccountName, Target::AccountType],
            CsvList::Contacts => &[Target::ContactName],
            CsvList::Items => &[Target::ItemName],
        }
    }

    /// Most rows per import.
    #[must_use]
    pub const fn max_rows(self) -> usize {
        match self {
            CsvList::Accounts => ACCOUNT_ROWS_MAX,
            CsvList::Contacts | CsvList::Items => ROWS_MAX,
        }
    }

    /// Owner-facing name.
    #[must_use]
    pub const fn label(self) -> TextKey {
        match self {
            CsvList::Accounts => TextKey("csv.list.accounts"),
            CsvList::Contacts => TextKey("csv.list.contacts"),
            CsvList::Items => TextKey("csv.list.items"),
        }
    }

    /// Stable name for the control channel and file names.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            CsvList::Accounts => "accounts",
            CsvList::Contacts => "contacts",
            CsvList::Items => "items",
        }
    }
}

/// A list field a column can fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    /// Account number.
    AccountNumber,
    /// Account name.
    AccountName,
    /// Account type.
    AccountType,
    /// Account detail type.
    DetailType,
    /// Contact display name.
    ContactName,
    /// Company name.
    Company,
    /// Email.
    Email,
    /// Phone.
    Phone,
    /// Item name.
    ItemName,
    /// Item description.
    Description,
    /// Sales price.
    Price,
    /// Income account (name or number).
    IncomeAccount,
    /// Taxable (yes/no).
    Taxable,
}

impl Target {
    /// Owner-facing name.
    #[must_use]
    pub const fn label(self) -> TextKey {
        match self {
            Target::AccountNumber => TextKey("csv.field.account_number"),
            Target::AccountName | Target::ContactName | Target::ItemName => {
                TextKey("csv.field.name")
            }
            Target::AccountType => TextKey("csv.field.account_type"),
            Target::DetailType => TextKey("csv.field.detail_type"),
            Target::Company => TextKey("csv.field.company"),
            Target::Email => TextKey("csv.field.email"),
            Target::Phone => TextKey("csv.field.phone"),
            Target::Description => TextKey("csv.field.description"),
            Target::Price => TextKey("csv.field.price"),
            Target::IncomeAccount => TextKey("csv.field.income_account"),
            Target::Taxable => TextKey("csv.field.taxable"),
        }
    }

    /// Header names (normalized) that suggest this field.
    const fn synonyms(self) -> &'static [&'static str] {
        match self {
            Target::AccountNumber => &[
                "accountnumber",
                "number",
                "acctnum",
                "accountno",
                "acct",
                "accnt",
            ],
            Target::AccountName => &["name", "accountname", "account", "fullname"],
            Target::AccountType => &["type", "accounttype", "accnttype", "accttype"],
            Target::DetailType => &["detailtype", "detail", "subtype"],
            Target::ContactName => &[
                "displayname",
                "name",
                "client",
                "customer",
                "vendor",
                "contact",
                "fullname",
            ],
            Target::Company => &["company", "companyname", "business", "businessname"],
            Target::Email => &["email", "emailaddress", "mainemail"],
            Target::Phone => &["phone", "phonenumber", "mainphone", "telephone", "tel"],
            Target::ItemName => &[
                "name",
                "item",
                "itemname",
                "product",
                "service",
                "productservice",
            ],
            Target::Description => &["description", "salesdescription", "desc"],
            Target::Price => &["price", "unitprice", "salesprice", "rate", "amount"],
            Target::IncomeAccount => &[
                "incomeaccount",
                "account",
                "incomeaccountid",
                "salesaccount",
                "incomeaccountname",
            ],
            Target::Taxable => &["taxable", "tax", "salestax", "istaxable", "salestaxcode"],
        }
    }
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// A first guess at the mapping: each header matched to the list field
/// whose names it matches, each field used once, first column wins.
#[must_use]
pub fn guess(list: CsvList, headers: &[String]) -> Vec<Option<Target>> {
    let mut used = HashSet::new();
    headers
        .iter()
        .map(|h| {
            let n = normalize(h);
            let hit = list
                .targets()
                .iter()
                .copied()
                .find(|t| !used.contains(t) && t.synonyms().contains(&n.as_str()));
            if let Some(t) = hit {
                used.insert(t);
            }
            hit
        })
        .collect()
}

/// Something wrong with the mapping as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingProblem {
    /// A required field has no column.
    Missing(Target),
    /// Two columns fill the same field.
    Twice(Target),
    /// The file has more rows than one import takes.
    TooManyRows(usize),
    /// The file has a header and nothing else.
    NoRows,
}

impl MappingProblem {
    /// Owner-facing text (`{field}` and `{max}` filled in by the caller).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            MappingProblem::Missing(_) => TextKey("csv.problem.missing"),
            MappingProblem::Twice(_) => TextKey("csv.problem.twice"),
            MappingProblem::TooManyRows(_) => TextKey("csv.problem.too_many_rows"),
            MappingProblem::NoRows => TextKey("csv.problem.no_rows"),
        }
    }
}

/// The mapping's problems, in field order.
#[must_use]
pub fn check_mapping(
    list: CsvList,
    mapping: &[Option<Target>],
    data_rows: usize,
) -> Vec<MappingProblem> {
    let mut out = Vec::new();
    for t in list.targets() {
        let n = mapping.iter().filter(|m| **m == Some(*t)).count();
        if n == 0 && list.required().contains(t) {
            out.push(MappingProblem::Missing(*t));
        } else if n > 1 {
            out.push(MappingProblem::Twice(*t));
        }
    }
    if data_rows == 0 {
        out.push(MappingProblem::NoRows);
    } else if data_rows > list.max_rows() {
        out.push(MappingProblem::TooManyRows(list.max_rows()));
    }
    out
}

/// An account row for the engine's account import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRow {
    /// Number, if any.
    pub number: Option<String>,
    /// Name.
    pub name: String,
    /// Type.
    pub kind: AccountKind,
    /// Detail type key or text, if any.
    pub detail_type: Option<String>,
}

/// A contact row for the engine's contact import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactRow {
    /// Display name.
    pub name: String,
    /// Company.
    pub company: Option<String>,
    /// Email.
    pub email: Option<String>,
    /// Phone.
    pub phone: Option<String>,
}

/// An item row, created through the item form's save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRow {
    /// Name.
    pub name: String,
    /// Description.
    pub description: Option<String>,
    /// Sales price, exact.
    pub price: Option<Money>,
    /// Income account.
    pub income_account: AccountId,
    /// Taxable.
    pub taxable: bool,
}

/// One checked row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mapped {
    /// An account.
    Account(AccountRow),
    /// A client or vendor.
    Contact(ContactRow),
    /// An item.
    Item(ItemRow),
}

/// Why one row is left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowProblem {
    /// The name is blank.
    NameMissing,
    /// A value is longer than the engine takes.
    TooLong(Target),
    /// The account type isn't a known one.
    TypeUnknown,
    /// The price isn't an amount.
    PriceInvalid,
    /// The price is below zero.
    PriceNegative,
    /// The income account isn't in the chart, or isn't an income account.
    IncomeAccountUnknown,
    /// No income account in the row and none chosen for the file.
    IncomeAccountMissing,
    /// Not an email address.
    EmailInvalid,
    /// Not yes or no.
    TaxableInvalid,
    /// The name is already used (in the list or earlier in the file).
    NameTaken,
}

impl RowProblem {
    /// Owner-facing text (`{field}` filled in by the caller for `TooLong`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            RowProblem::NameMissing => TextKey("csv.row.name_missing"),
            RowProblem::TooLong(_) => TextKey("csv.row.too_long"),
            RowProblem::TypeUnknown => TextKey("csv.row.type_unknown"),
            RowProblem::PriceInvalid => TextKey("csv.row.price_invalid"),
            RowProblem::PriceNegative => TextKey("csv.row.price_negative"),
            RowProblem::IncomeAccountUnknown => TextKey("csv.row.income_unknown"),
            RowProblem::IncomeAccountMissing => TextKey("csv.row.income_missing"),
            RowProblem::EmailInvalid => TextKey("csv.row.email_invalid"),
            RowProblem::TaxableInvalid => TextKey("csv.row.taxable_invalid"),
            RowProblem::NameTaken => TextKey("csv.row.name_taken"),
        }
    }
}

/// A left-out row: the file line and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowIssue {
    /// Line in the file (1-based).
    pub line: usize,
    /// Why.
    pub problem: RowProblem,
}

/// What a file becomes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Built {
    /// Rows ready to send, with their file lines.
    pub rows: Vec<(usize, Mapped)>,
    /// Rows left out.
    pub issues: Vec<RowIssue>,
}

/// What the item rows need from the open company.
pub struct ItemLookups<'a> {
    /// The income account a cell names (number or name, any case).
    pub income_account: &'a dyn Fn(&str) -> Option<AccountId>,
    /// Used when the row names none.
    pub default_income: Option<AccountId>,
    /// True when an item (inactive ones too) already has the name.
    pub name_taken: &'a dyn Fn(&str) -> bool,
}

impl std::fmt::Debug for ItemLookups<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemLookups")
            .field("default_income", &self.default_income)
            .finish_non_exhaustive()
    }
}

/// The type an account-type cell names: the engine's names, the owner
/// labels, and the common type names other bookkeeping programs export
/// (`Bank`, `Accounts Receivable`, `Credit Card`, …), which also give a
/// detail type.
#[must_use]
pub fn parse_account_type(cell: &str) -> Option<(AccountKind, Option<&'static str>)> {
    let n = normalize(cell);
    if let Some(k) = AccountKind::ALL
        .into_iter()
        .find(|k| normalize(k.wire()) == n)
    {
        return Some((k, None));
    }
    let hit = match n.as_str() {
        "income" | "revenue" | "sales" => (AccountKind::Income, None),
        "otherincome" | "otherrevenue" => (AccountKind::OtherIncome, None),
        "costofgoodssold" | "cogs" | "costofsales" => (AccountKind::Cogs, None),
        "expenses" => (AccountKind::Expense, None),
        "otherexpenses" => (AccountKind::OtherExpense, None),
        "bank" | "checking" | "savings" => (AccountKind::Asset, Some("bank")),
        "accountsreceivable" | "ar" => (AccountKind::Asset, Some("accounts_receivable")),
        "othercurrentasset" | "othercurrentassets" => {
            (AccountKind::Asset, Some("other_current_asset"))
        }
        "fixedasset" | "fixedassets" => (AccountKind::Asset, Some("fixed_asset")),
        "otherasset" | "otherassets" => (AccountKind::Asset, Some("other_asset")),
        "accountspayable" | "ap" => (AccountKind::Liability, Some("accounts_payable")),
        "creditcard" => (AccountKind::Liability, Some("credit_card")),
        "othercurrentliability" | "othercurrentliabilities" => {
            (AccountKind::Liability, Some("other_current_liability"))
        }
        "longtermliability" | "longtermliabilities" => {
            (AccountKind::Liability, Some("long_term_liability"))
        }
        _ => return None,
    };
    Some(hit)
}

/// Yes or no as owners write it; blank is `None`.
fn parse_yes_no(cell: &str) -> Result<Option<bool>, ()> {
    match normalize(cell).as_str() {
        "" => Ok(None),
        "y" | "yes" | "true" | "1" | "t" | "x" | "tax" | "taxable" => Ok(Some(true)),
        "n" | "no" | "false" | "0" | "f" | "non" | "nontaxable" | "exempt" => Ok(Some(false)),
        _ => Err(()),
    }
}

/// A sales price from a file: blank is none; `$`, thousands separators and
/// up to four decimals allowed; never below zero.
fn parse_price(cell: &str) -> Result<Option<Money>, RowProblem> {
    let s = cell.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let m = Money::parse_user(s, UserInput::FULL).map_err(|_| RowProblem::PriceInvalid)?;
    if m.is_negative() {
        return Err(RowProblem::PriceNegative);
    }
    Ok(Some(m))
}

fn email_ok(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !s.chars().any(char::is_whitespace)
}

/// The trimmed cell a field maps to; `None` when unmapped or blank.
fn cell<'r>(mapping: &[Option<Target>], row: &'r [String], t: Target) -> Option<&'r str> {
    let i = mapping.iter().position(|m| *m == Some(t))?;
    let v = row.get(i)?.trim();
    (!v.is_empty()).then_some(v)
}

fn limit(value: Option<&str>, t: Target, max: usize) -> Result<Option<String>, RowProblem> {
    match value {
        Some(v) if engine_len(v) > max => Err(RowProblem::TooLong(t)),
        other => Ok(other.map(str::to_owned)),
    }
}

/// Turns data rows (line, cells) into checked rows. Names are checked for
/// duplicates within the file for accounts and items, ignoring case.
#[must_use]
pub fn build(
    list: CsvList,
    mapping: &[Option<Target>],
    rows: &[(usize, Vec<String>)],
    items: Option<&ItemLookups<'_>>,
) -> Built {
    let mut out = Built::default();
    let mut seen: HashSet<String> = HashSet::new();
    for (line, cells) in rows {
        let r = match list {
            CsvList::Accounts => account_row(mapping, cells),
            CsvList::Contacts => contact_row(mapping, cells),
            CsvList::Items => match items {
                Some(l) => item_row(mapping, cells, l),
                None => Err(RowProblem::IncomeAccountMissing),
            },
        };
        let r = r.and_then(|m| {
            let name = match &m {
                Mapped::Item(i) => Some(&i.name),
                Mapped::Account(_) | Mapped::Contact(_) => None,
            };
            match name {
                Some(n) if !seen.insert(n.to_lowercase()) => Err(RowProblem::NameTaken),
                _ => Ok(m),
            }
        });
        match r {
            Ok(m) => out.rows.push((*line, m)),
            Err(problem) => out.issues.push(RowIssue {
                line: *line,
                problem,
            }),
        }
    }
    out
}

fn account_row(mapping: &[Option<Target>], row: &[String]) -> Result<Mapped, RowProblem> {
    use crate::lists::accounts::{DETAIL_MAX, NAME_MAX, NUMBER_MAX};
    let name = cell(mapping, row, Target::AccountName).ok_or(RowProblem::NameMissing)?;
    let name = limit(Some(name), Target::AccountName, NAME_MAX)?.unwrap_or_default();
    let number = limit(
        cell(mapping, row, Target::AccountNumber),
        Target::AccountNumber,
        NUMBER_MAX,
    )?;
    let (kind, implied) = cell(mapping, row, Target::AccountType)
        .and_then(parse_account_type)
        .ok_or(RowProblem::TypeUnknown)?;
    let detail = match cell(mapping, row, Target::DetailType) {
        Some(d) => {
            let n = normalize(d);
            let known = kind
                .detail_types()
                .iter()
                .find(|k| normalize(k) == n)
                .map(|k| (*k).to_owned());
            limit(
                Some(known.as_deref().unwrap_or(d)),
                Target::DetailType,
                DETAIL_MAX,
            )?
        }
        None => implied.map(str::to_owned),
    };
    Ok(Mapped::Account(AccountRow {
        number,
        name,
        kind,
        detail_type: detail,
    }))
}

fn contact_row(mapping: &[Option<Target>], row: &[String]) -> Result<Mapped, RowProblem> {
    let name = cell(mapping, row, Target::ContactName).ok_or(RowProblem::NameMissing)?;
    let name = limit(Some(name), Target::ContactName, 255)?.unwrap_or_default();
    let company = limit(cell(mapping, row, Target::Company), Target::Company, 255)?;
    let email = limit(cell(mapping, row, Target::Email), Target::Email, 255)?;
    if email.as_deref().is_some_and(|e| !email_ok(e)) {
        return Err(RowProblem::EmailInvalid);
    }
    let phone = limit(cell(mapping, row, Target::Phone), Target::Phone, 30)?;
    Ok(Mapped::Contact(ContactRow {
        name,
        company,
        email,
        phone,
    }))
}

fn item_row(
    mapping: &[Option<Target>],
    row: &[String],
    l: &ItemLookups<'_>,
) -> Result<Mapped, RowProblem> {
    let name = cell(mapping, row, Target::ItemName).ok_or(RowProblem::NameMissing)?;
    let name = limit(Some(name), Target::ItemName, 255)?.unwrap_or_default();
    if (l.name_taken)(&name) {
        return Err(RowProblem::NameTaken);
    }
    let description = limit(
        cell(mapping, row, Target::Description),
        Target::Description,
        2000,
    )?;
    let price = parse_price(cell(mapping, row, Target::Price).unwrap_or_default())?;
    let income_account = match cell(mapping, row, Target::IncomeAccount) {
        Some(a) => (l.income_account)(a).ok_or(RowProblem::IncomeAccountUnknown)?,
        None => l.default_income.ok_or(RowProblem::IncomeAccountMissing)?,
    };
    let taxable = parse_yes_no(cell(mapping, row, Target::Taxable).unwrap_or_default())
        .map_err(|()| RowProblem::TaxableInvalid)?
        .unwrap_or(true);
    Ok(Mapped::Item(ItemRow {
        name,
        description,
        price,
        income_account,
        taxable,
    }))
}

#[cfg(test)]
mod tests;
