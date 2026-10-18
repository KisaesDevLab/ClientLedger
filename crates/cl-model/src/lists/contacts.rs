//! Client and vendor rules (P11 steps 2-3): one contact record with the
//! client role, the vendor role or both (Q-77), the order and filters of
//! the Client and Vendor centers, the fixed payment-terms list the engine
//! knows, and the add/edit form's checks (lengths as the engine counts
//! them, a plain email check, a nine-digit tax ID, a vendor's default
//! expense account, Q-192, and its 1099 fields, Q-53).
//!
//! No amounts live here: a contact's open balance is the engine's figure
//! from its balance reports, shown as sent (non-negotiable 7). Jobs (U-35),
//! several ship-to addresses and contact people (U-36) and dated notes and
//! to-dos (U-37) wait for the engine; until then a contact has one ship-to
//! address and one notes field. There are no credit limits (Q-131).

use std::cmp::Ordering;

use cl_core::ids::{AccountId, ContactId};

use crate::lists::engine_len;
use crate::text::TextKey;

/// Longest display or company name the engine takes.
pub const NAME_MAX: usize = 255;
/// Longest first or last name.
pub const PERSON_MAX: usize = 100;
/// Longest email address.
pub const EMAIL_MAX: usize = 255;
/// Longest phone number.
pub const PHONE_MAX: usize = 30;
/// Longest street line.
pub const LINE_MAX: usize = 255;
/// Longest city.
pub const CITY_MAX: usize = 100;
/// Longest state.
pub const STATE_MAX: usize = 50;
/// Longest ZIP code.
pub const ZIP_MAX: usize = 20;
/// Longest terms text.
pub const TERMS_MAX: usize = 50;
/// Longest tax ID.
pub const TAX_ID_MAX: usize = 30;
/// Longest account number the vendor gave the company.
pub const VENDOR_ACCOUNT_MAX: usize = 50;
/// Longest notes text kept (the engine has no limit; this keeps a pasted
/// document out of the record).
pub const NOTES_MAX: usize = 4000;

/// Which center a contact is looked at from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Clients & Jobs (the engine's customers).
    Client,
    /// Vendors.
    Vendor,
}

/// A contact's roles (the engine's `contactType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Roles {
    /// Client only (`customer`).
    Client,
    /// Vendor only (`vendor`).
    Vendor,
    /// Client and vendor (`both`).
    Both,
}

impl Roles {
    /// The engine's `contactType` value.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Roles::Client => "customer",
            Roles::Vendor => "vendor",
            Roles::Both => "both",
        }
    }

    /// The roles for an engine `contactType` value.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Roles> {
        [Roles::Client, Roles::Vendor, Roles::Both]
            .into_iter()
            .find(|r| r.wire() == s)
    }

    /// The single role of a new contact made in `role`'s center.
    #[must_use]
    pub const fn only(role: Role) -> Roles {
        match role {
            Role::Client => Roles::Client,
            Role::Vendor => Roles::Vendor,
        }
    }

    /// True when the contact appears in `role`'s center.
    #[must_use]
    pub const fn has(self, role: Role) -> bool {
        matches!(
            (self, role),
            (Roles::Both, _) | (Roles::Client, Role::Client) | (Roles::Vendor, Role::Vendor)
        )
    }

    /// True when these roles include every role in `other`.
    #[must_use]
    pub const fn covers(self, other: Roles) -> bool {
        (self.has(Role::Client) || !other.has(Role::Client))
            && (self.has(Role::Vendor) || !other.has(Role::Vendor))
    }

    /// The roles with `role` switched on or off; `None` when that would
    /// leave the contact with no role.
    #[must_use]
    pub const fn with(self, role: Role, on: bool) -> Option<Roles> {
        let client = if matches!(role, Role::Client) {
            on
        } else {
            self.has(Role::Client)
        };
        let vendor = if matches!(role, Role::Vendor) {
            on
        } else {
            self.has(Role::Vendor)
        };
        match (client, vendor) {
            (true, true) => Some(Roles::Both),
            (true, false) => Some(Roles::Client),
            (false, true) => Some(Roles::Vendor),
            (false, false) => None,
        }
    }

    /// Owner-facing name.
    #[must_use]
    pub const fn label(self) -> TextKey {
        match self {
            Roles::Client => TextKey("contacts.roles.client"),
            Roles::Vendor => TextKey("contacts.roles.vendor"),
            Roles::Both => TextKey("contacts.roles.both"),
        }
    }
}

/// The payment terms the engine knows (`paymentTerms` in its company
/// schema). The list is fixed until U-43; a contact's other stored text is
/// kept as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Terms {
    /// Due on receipt.
    DueOnReceipt,
    /// Net 15.
    Net15,
    /// Net 30.
    Net30,
    /// Net 60.
    Net60,
    /// Net 90.
    Net90,
}

impl Terms {
    /// Every term, in list order.
    pub const ALL: [Terms; 5] = [
        Terms::DueOnReceipt,
        Terms::Net15,
        Terms::Net30,
        Terms::Net60,
        Terms::Net90,
    ];

    /// The engine's value.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Terms::DueOnReceipt => "due_on_receipt",
            Terms::Net15 => "net_15",
            Terms::Net30 => "net_30",
            Terms::Net60 => "net_60",
            Terms::Net90 => "net_90",
        }
    }

    /// The term for an engine value.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Terms> {
        Terms::ALL.into_iter().find(|t| t.wire() == s)
    }

    /// Owner-facing name.
    #[must_use]
    pub const fn label(self) -> TextKey {
        match self {
            Terms::DueOnReceipt => TextKey("contacts.terms.due_on_receipt"),
            Terms::Net15 => TextKey("contacts.terms.net_15"),
            Terms::Net30 => TextKey("contacts.terms.net_30"),
            Terms::Net60 => TextKey("contacts.terms.net_60"),
            Terms::Net90 => TextKey("contacts.terms.net_90"),
        }
    }
}

/// A contact's terms as stored: none, one of the fixed list, or text the
/// engine stored from elsewhere (an import), kept untouched.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum TermsChoice {
    /// No default terms.
    #[default]
    None,
    /// One of the fixed list.
    Fixed(Terms),
    /// Other stored text.
    Other(String),
}

impl TermsChoice {
    /// The stored value read back.
    #[must_use]
    pub fn from_stored(s: Option<&str>) -> TermsChoice {
        match s.map(str::trim) {
            None | Some("") => TermsChoice::None,
            Some(v) => Terms::from_wire(v)
                .map_or_else(|| TermsChoice::Other(v.to_string()), TermsChoice::Fixed),
        }
    }

    /// The value to store.
    #[must_use]
    pub fn stored(&self) -> Option<String> {
        match self {
            TermsChoice::None => None,
            TermsChoice::Fixed(t) => Some(t.wire().to_string()),
            TermsChoice::Other(s) => Some(s.clone()),
        }
    }
}

/// Kind of taxpayer number on a vendor's 1099 profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaxIdKind {
    /// Social Security number (a person).
    Ssn,
    /// Employer identification number (a business).
    Ein,
}

/// One contact as a center loaded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactEntry {
    /// Id.
    pub id: ContactId,
    /// Roles.
    pub roles: Roles,
    /// Display name.
    pub name: String,
    /// Company name.
    pub company: Option<String>,
    /// Email.
    pub email: Option<String>,
    /// Phone.
    pub phone: Option<String>,
    /// Active.
    pub active: bool,
}

/// What a center shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContactFilter {
    /// Also list inactive contacts.
    pub show_inactive: bool,
    /// Name, company, email or phone contains (case-insensitive).
    pub search: String,
}

/// Every loaded contact, sorted by name, case-insensitive.
#[derive(Debug, Clone, Default)]
pub struct ContactList {
    entries: Vec<ContactEntry>,
    /// Lowercased name, company, email and phone per entry.
    haystacks: Vec<String>,
}

fn name_order(a: &ContactEntry, b: &ContactEntry) -> Ordering {
    a.name
        .to_lowercase()
        .cmp(&b.name.to_lowercase())
        .then_with(|| a.name.cmp(&b.name))
        .then_with(|| a.id.to_string().cmp(&b.id.to_string()))
}

impl ContactList {
    /// The list of `entries`, in name order.
    #[must_use]
    pub fn new(mut entries: Vec<ContactEntry>) -> ContactList {
        entries.sort_by(name_order);
        let haystacks = entries
            .iter()
            .map(|e| {
                let parts = [
                    Some(e.name.as_str()),
                    e.company.as_deref(),
                    e.email.as_deref(),
                    e.phone.as_deref(),
                ];
                parts
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join("\n")
                    .to_lowercase()
            })
            .collect();
        ContactList { entries, haystacks }
    }

    /// Every entry, in name order.
    #[must_use]
    pub fn entries(&self) -> &[ContactEntry] {
        &self.entries
    }

    /// Number of contacts loaded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Nothing loaded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The contact `id`.
    #[must_use]
    pub fn entry(&self, id: ContactId) -> Option<&ContactEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Contacts in `role`'s center (a contact with both roles is in both).
    #[must_use]
    pub fn in_role(&self, role: Role) -> usize {
        self.entries.iter().filter(|e| e.roles.has(role)).count()
    }

    /// Indexes of the entries `role`'s center shows under `filter`.
    #[must_use]
    pub fn visible(&self, role: Role, filter: &ContactFilter) -> Vec<usize> {
        let q = filter.search.trim().to_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(i, e)| {
                e.roles.has(role)
                    && (e.active || filter.show_inactive)
                    && (q.is_empty() || self.haystacks[*i].contains(&q))
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Another active contact (either role) already called `name`, ignoring
    /// case and outer spaces. Inactive contacts do not hold their name: a
    /// merge leaves the merged-away duplicate inactive under the same name.
    #[must_use]
    pub fn name_owner(&self, name: &str, except: Option<ContactId>) -> Option<ContactId> {
        let want = name.trim().to_lowercase();
        if want.is_empty() {
            return None;
        }
        self.entries
            .iter()
            .find(|e| e.active && Some(e.id) != except && e.name.trim().to_lowercase() == want)
            .map(|e| e.id)
    }

    /// The contacts `source` may be merged into: active, in `role`, not
    /// `source` itself, and holding every role `source` has (a client and
    /// vendor only merges into another client and vendor, so neither its
    /// sales nor its bills land on a contact of the wrong kind), in name
    /// order.
    #[must_use]
    pub fn merge_targets(&self, role: Role, source: ContactId) -> Vec<&ContactEntry> {
        let from = self.entry(source).map_or(Roles::only(role), |e| e.roles);
        self.entries
            .iter()
            .filter(|e| e.id != source && e.active && e.roles.has(role) && e.roles.covers(from))
            .collect()
    }
}

/// One postal address on the form.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddressDraft {
    /// Street, first line.
    pub line1: String,
    /// Street, second line.
    pub line2: String,
    /// City.
    pub city: String,
    /// State.
    pub state: String,
    /// ZIP code.
    pub zip: String,
}

impl AddressDraft {
    /// True when every part is blank.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        [&self.line1, &self.line2, &self.city, &self.state, &self.zip]
            .iter()
            .all(|s| s.trim().is_empty())
    }
}

/// The add/edit contact form's content. `Debug` hides the tax ID.
#[derive(Clone, PartialEq, Eq)]
pub struct ContactDraft {
    /// Roles.
    pub roles: Roles,
    /// Display name.
    pub name: String,
    /// Company name.
    pub company: String,
    /// First name.
    pub first: String,
    /// Last name.
    pub last: String,
    /// Email.
    pub email: String,
    /// Phone.
    pub phone: String,
    /// Bill-to address.
    pub billing: AddressDraft,
    /// The one ship-to address (several wait for U-36).
    pub shipping: AddressDraft,
    /// Default terms.
    pub terms: TermsChoice,
    /// Notes (one field until U-37).
    pub notes: String,
    /// Vendor: the account its bills and checks start with (Q-192).
    pub expense_account: Option<AccountId>,
    /// Vendor: the account number the vendor gave the company.
    pub vendor_account: String,
    /// Vendor: payments are tracked for a 1099 (Q-53).
    pub track_1099: bool,
    /// Vendor: tax ID (SSN or EIN). When editing, empty keeps the one on
    /// file (it is never shown in full).
    pub tax_id: String,
    /// Vendor: remove the tax ID on file.
    pub clear_tax_id: bool,
    /// Vendor: the kind of tax ID.
    pub tax_id_kind: Option<TaxIdKind>,
    /// Vendor: backup withholding applies.
    pub backup_withholding: bool,
}

impl ContactDraft {
    /// A blank contact made in `role`'s center.
    #[must_use]
    pub fn new(role: Role) -> ContactDraft {
        ContactDraft {
            roles: Roles::only(role),
            name: String::new(),
            company: String::new(),
            first: String::new(),
            last: String::new(),
            email: String::new(),
            phone: String::new(),
            billing: AddressDraft::default(),
            shipping: AddressDraft::default(),
            terms: TermsChoice::None,
            notes: String::new(),
            expense_account: None,
            vendor_account: String::new(),
            track_1099: false,
            tax_id: String::new(),
            clear_tax_id: false,
            tax_id_kind: None,
            backup_withholding: false,
        }
    }

    /// Copies the bill-to address into the ship-to address.
    pub fn ship_to_billing(&mut self) {
        self.shipping = self.billing.clone();
    }
}

/// A draft that passed the checks, trimmed, ready for the engine
/// (`None` = empty). `Debug` hides the tax ID.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedContact {
    /// Roles.
    pub roles: Roles,
    /// Display name.
    pub name: String,
    /// Company name.
    pub company: Option<String>,
    /// First name.
    pub first: Option<String>,
    /// Last name.
    pub last: Option<String>,
    /// Email.
    pub email: Option<String>,
    /// Phone.
    pub phone: Option<String>,
    /// Bill-to address, part by part.
    pub billing: [Option<String>; 5],
    /// Ship-to address, part by part.
    pub shipping: [Option<String>; 5],
    /// Default terms.
    pub terms: Option<String>,
    /// Notes.
    pub notes: Option<String>,
    /// Default expense account.
    pub expense_account: Option<AccountId>,
    /// Vendor account number.
    pub vendor_account: Option<String>,
    /// Tracked for a 1099.
    pub track_1099: bool,
    /// Tax ID as typed (trimmed).
    pub tax_id: Option<String>,
    /// Remove the tax ID on file (nothing typed instead).
    pub clear_tax_id: bool,
    /// Kind of tax ID.
    pub tax_id_kind: Option<TaxIdKind>,
    /// Backup withholding.
    pub backup_withholding: bool,
}

impl std::fmt::Debug for ContactDraft {
    /// Every field except the tax ID, which shows only whether one was
    /// typed (B1: a tax ID never reaches a log or a panic message).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContactDraft")
            .field("roles", &self.roles)
            .field("name", &self.name)
            .field("company", &self.company)
            .field("first", &self.first)
            .field("last", &self.last)
            .field("email", &self.email)
            .field("phone", &self.phone)
            .field("billing", &self.billing)
            .field("shipping", &self.shipping)
            .field("terms", &self.terms)
            .field("notes", &self.notes)
            .field("expense_account", &self.expense_account)
            .field("vendor_account", &self.vendor_account)
            .field("track_1099", &self.track_1099)
            .field("has_tax_id", &!self.tax_id.trim().is_empty())
            .field("clear_tax_id", &self.clear_tax_id)
            .field("tax_id_kind", &self.tax_id_kind)
            .field("backup_withholding", &self.backup_withholding)
            .finish()
    }
}

impl std::fmt::Debug for CheckedContact {
    /// Every field except the tax ID, which shows only whether one is set.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckedContact")
            .field("roles", &self.roles)
            .field("name", &self.name)
            .field("company", &self.company)
            .field("first", &self.first)
            .field("last", &self.last)
            .field("email", &self.email)
            .field("phone", &self.phone)
            .field("billing", &self.billing)
            .field("shipping", &self.shipping)
            .field("terms", &self.terms)
            .field("notes", &self.notes)
            .field("expense_account", &self.expense_account)
            .field("vendor_account", &self.vendor_account)
            .field("track_1099", &self.track_1099)
            .field("has_tax_id", &self.tax_id.is_some())
            .field("clear_tax_id", &self.clear_tax_id)
            .field("tax_id_kind", &self.tax_id_kind)
            .field("backup_withholding", &self.backup_withholding)
            .finish()
    }
}

/// The form field a problem belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContactField {
    /// Roles.
    Roles,
    /// Display name.
    Name,
    /// Company.
    Company,
    /// First name.
    First,
    /// Last name.
    Last,
    /// Email.
    Email,
    /// Phone.
    Phone,
    /// Bill-to address.
    Billing,
    /// Ship-to address.
    Shipping,
    /// Terms.
    Terms,
    /// Notes.
    Notes,
    /// Default expense account.
    ExpenseAccount,
    /// Vendor account number.
    VendorAccount,
    /// Tax ID.
    TaxId,
}

/// Why a contact draft cannot be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContactProblem {
    /// No display name.
    NameMissing,
    /// Longer than the engine takes.
    NameTooLong,
    /// Another active client or vendor has this name.
    NameTaken,
    /// Longer than the engine takes.
    CompanyTooLong,
    /// Longer than the engine takes.
    PersonTooLong,
    /// Not an email address.
    EmailInvalid,
    /// Longer than the engine takes.
    PhoneTooLong,
    /// An address part is longer than the engine takes.
    BillingTooLong,
    /// An address part is longer than the engine takes.
    ShippingTooLong,
    /// Longer than the engine takes.
    TermsTooLong,
    /// Longer than kept.
    NotesTooLong,
    /// The chosen expense account is gone or not an expense account.
    ExpenseAccountInvalid,
    /// Longer than the engine takes.
    VendorAccountTooLong,
    /// Not nine digits.
    TaxIdInvalid,
}

impl ContactProblem {
    /// Plain-language message.
    #[must_use]
    pub const fn key(self) -> TextKey {
        TextKey(match self {
            ContactProblem::NameMissing => "contacts.problem.name_missing",
            ContactProblem::NameTooLong => "contacts.problem.name_too_long",
            ContactProblem::NameTaken => "contacts.problem.name_taken",
            ContactProblem::CompanyTooLong => "contacts.problem.company_too_long",
            ContactProblem::PersonTooLong => "contacts.problem.person_too_long",
            ContactProblem::EmailInvalid => "contacts.problem.email_invalid",
            ContactProblem::PhoneTooLong => "contacts.problem.phone_too_long",
            ContactProblem::BillingTooLong => "contacts.problem.billing_too_long",
            ContactProblem::ShippingTooLong => "contacts.problem.shipping_too_long",
            ContactProblem::TermsTooLong => "contacts.problem.terms_too_long",
            ContactProblem::NotesTooLong => "contacts.problem.notes_too_long",
            ContactProblem::ExpenseAccountInvalid => "contacts.problem.expense_account_invalid",
            ContactProblem::VendorAccountTooLong => "contacts.problem.vendor_account_too_long",
            ContactProblem::TaxIdInvalid => "contacts.problem.tax_id_invalid",
        })
    }

    /// The field to focus.
    #[must_use]
    pub const fn field(self) -> ContactField {
        match self {
            ContactProblem::NameMissing
            | ContactProblem::NameTooLong
            | ContactProblem::NameTaken => ContactField::Name,
            ContactProblem::CompanyTooLong => ContactField::Company,
            ContactProblem::PersonTooLong => ContactField::First,
            ContactProblem::EmailInvalid => ContactField::Email,
            ContactProblem::PhoneTooLong => ContactField::Phone,
            ContactProblem::BillingTooLong => ContactField::Billing,
            ContactProblem::ShippingTooLong => ContactField::Shipping,
            ContactProblem::TermsTooLong => ContactField::Terms,
            ContactProblem::NotesTooLong => ContactField::Notes,
            ContactProblem::ExpenseAccountInvalid => ContactField::ExpenseAccount,
            ContactProblem::VendorAccountTooLong => ContactField::VendorAccount,
            ContactProblem::TaxIdInvalid => ContactField::TaxId,
        }
    }
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// A plain email check: one `@`, something before it, a dotted domain
/// after it, no spaces. The engine's own check stays the rule.
#[must_use]
pub fn looks_like_email(s: &str) -> bool {
    let s = s.trim();
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.contains('@')
        && !s.chars().any(char::is_whitespace)
        && domain.split('.').filter(|p| !p.is_empty()).count() >= 2
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

/// The digits of a tax ID when it is a nine-digit SSN or EIN, written
/// with or without dashes or spaces.
#[must_use]
pub fn tax_id_digits(s: &str) -> Option<String> {
    let s = s.trim();
    if !s
        .chars()
        .all(|c| c.is_ascii_digit() || c == '-' || c == ' ')
    {
        return None;
    }
    let digits: String = s.chars().filter(char::is_ascii_digit).collect();
    (digits.len() == 9).then_some(digits)
}

/// A tax ID shown with only its last four digits (`•••-••-1234`); never
/// the whole number on screen once saved.
#[must_use]
pub fn masked_tax_id(s: &str) -> Option<String> {
    let digits: String = s.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 4 {
        return None;
    }
    let last = &digits[digits.len() - 4..];
    Some(format!("\u{2022}\u{2022}\u{2022}-\u{2022}\u{2022}-{last}"))
}

fn address(
    a: &AddressDraft,
    problem: ContactProblem,
    problems: &mut Vec<ContactProblem>,
) -> [Option<String>; 5] {
    let parts = [
        (&a.line1, LINE_MAX),
        (&a.line2, LINE_MAX),
        (&a.city, CITY_MAX),
        (&a.state, STATE_MAX),
        (&a.zip, ZIP_MAX),
    ];
    if parts.iter().any(|(s, max)| engine_len(s.trim()) > *max) && !problems.contains(&problem) {
        problems.push(problem);
    }
    parts.map(|(s, _)| non_empty(s))
}

/// Checks a draft against the loaded contacts. `editing` is the contact
/// being changed (`None` for a new one); `expense_ok` says whether an
/// account may be a vendor's default expense account (it exists and is an
/// expense-type account).
///
/// Vendor-only fields are only checked, and only kept, when the draft has
/// the vendor role; a client's draft drops them.
///
/// # Errors
/// Every problem found, in field order.
pub fn check(
    draft: &ContactDraft,
    list: &ContactList,
    editing: Option<ContactId>,
    expense_ok: impl Fn(AccountId) -> bool,
) -> Result<CheckedContact, Vec<ContactProblem>> {
    let mut problems = Vec::new();
    let name = draft.name.trim();
    if name.is_empty() {
        problems.push(ContactProblem::NameMissing);
    } else if engine_len(name) > NAME_MAX {
        problems.push(ContactProblem::NameTooLong);
    } else if list.name_owner(name, editing).is_some() {
        problems.push(ContactProblem::NameTaken);
    }
    if engine_len(draft.company.trim()) > NAME_MAX {
        problems.push(ContactProblem::CompanyTooLong);
    }
    if engine_len(draft.first.trim()) > PERSON_MAX || engine_len(draft.last.trim()) > PERSON_MAX {
        problems.push(ContactProblem::PersonTooLong);
    }
    let email = non_empty(&draft.email);
    if email
        .as_deref()
        .is_some_and(|e| engine_len(e) > EMAIL_MAX || !looks_like_email(e))
    {
        problems.push(ContactProblem::EmailInvalid);
    }
    if engine_len(draft.phone.trim()) > PHONE_MAX {
        problems.push(ContactProblem::PhoneTooLong);
    }
    let billing = address(
        &draft.billing,
        ContactProblem::BillingTooLong,
        &mut problems,
    );
    let shipping = address(
        &draft.shipping,
        ContactProblem::ShippingTooLong,
        &mut problems,
    );
    let terms = draft.terms.stored();
    if terms.as_deref().is_some_and(|t| engine_len(t) > TERMS_MAX) {
        problems.push(ContactProblem::TermsTooLong);
    }
    if engine_len(draft.notes.trim()) > NOTES_MAX {
        problems.push(ContactProblem::NotesTooLong);
    }
    let vendor = draft.roles.has(Role::Vendor);
    let expense_account = draft.expense_account.filter(|_| vendor);
    if expense_account.is_some_and(|a| !expense_ok(a)) {
        problems.push(ContactProblem::ExpenseAccountInvalid);
    }
    let vendor_account = non_empty(&draft.vendor_account).filter(|_| vendor);
    if vendor_account
        .as_deref()
        .is_some_and(|v| engine_len(v) > VENDOR_ACCOUNT_MAX)
    {
        problems.push(ContactProblem::VendorAccountTooLong);
    }
    let tax_id = non_empty(&draft.tax_id).filter(|_| vendor);
    if tax_id
        .as_deref()
        .is_some_and(|t| engine_len(t) > TAX_ID_MAX || tax_id_digits(t).is_none())
    {
        problems.push(ContactProblem::TaxIdInvalid);
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(CheckedContact {
        roles: draft.roles,
        name: name.to_string(),
        company: non_empty(&draft.company),
        first: non_empty(&draft.first),
        last: non_empty(&draft.last),
        email,
        phone: non_empty(&draft.phone),
        billing,
        shipping,
        terms,
        notes: non_empty(&draft.notes),
        expense_account,
        vendor_account,
        track_1099: vendor && draft.track_1099,
        clear_tax_id: vendor && draft.clear_tax_id && tax_id.is_none(),
        tax_id,
        tax_id_kind: draft.tax_id_kind.filter(|_| vendor),
        backup_withholding: vendor && draft.backup_withholding,
    })
}

#[cfg(test)]
mod tests;
