//! Make General Journal Entries (P15): the context the entry is checked
//! in, built from the engine's lists, and the checked entry as the engine's
//! `NewJournalEntry`.
//!
//! The gate is `cl_model::forms::journal::check` (balanced exactly, the
//! A/R-A/P rule, the dates), then `NewJournalEntry::new_with` checks again
//! before anything is sent. The checked value maps field by field onto the
//! engine's entry; nothing is added up here.

use cl_api::{
    Account, AccountRoles, Basis, Capabilities, Contact, JeLine, JeOptions, LocalValidation,
    NewJournalEntry, Side,
};
use cl_model::forms::journal::{CheckedJournal, JeSide, JournalContext, JournalForm, NameKind};
use cl_model::forms::tag_rule::TagRefusal;
use cl_model::lists::tags::TagList;
use cl_model::register::AccountClass;

use super::accounts::class_of;

/// The company's A/R and A/P accounts.
#[must_use]
pub fn roles(accounts: &[Account]) -> AccountRoles {
    let mut r = AccountRoles::default();
    for a in accounts {
        match class_of(a) {
            Some(AccountClass::Receivable) => {
                r.receivable.insert(a.id);
            }
            Some(AccountClass::Payable) => {
                r.payable.insert(a.id);
            }
            _ => {}
        }
    }
    r
}

/// What `check` needs: the A/R-A/P accounts, each name's kind and the
/// engine's P15 capabilities (`has` answers whether the engine has one).
#[must_use]
pub fn context(
    accounts: &[Account],
    contacts: &[Contact],
    has: impl Fn(&str) -> bool,
) -> JournalContext {
    let roles = roles(accounts);
    let names = contacts
        .iter()
        .map(|c| {
            let kind = match c.fields.contact_type {
                cl_api::ContactType::Customer => NameKind {
                    customer: true,
                    vendor: false,
                },
                cl_api::ContactType::Vendor => NameKind {
                    customer: false,
                    vendor: true,
                },
                cl_api::ContactType::Both => NameKind {
                    customer: true,
                    vendor: true,
                },
            };
            (c.id, kind)
        })
        .collect();
    JournalContext {
        receivable: roles.receivable,
        payable: roles.payable,
        names,
        names_supported: has(Capabilities::JE_NAMES),
        number_supported: has(Capabilities::JE_NUMBER),
        adjusting_supported: has(Capabilities::JE_ADJUSTING),
        reversing_supported: has(Capabilities::JE_REVERSING),
    }
}

/// The checked form as the engine's entry, field by field.
///
/// # Errors
/// What `NewJournalEntry::new_with` refuses (nothing is sent).
pub fn entry_of(
    c: &CheckedJournal,
    basis: Basis,
    roles: &AccountRoles,
) -> Result<NewJournalEntry, LocalValidation> {
    let lines = c
        .lines
        .iter()
        .map(|l| JeLine {
            account: l.account,
            side: match l.side {
                JeSide::Debit(m) => Side::Debit(m),
                JeSide::Credit(m) => Side::Credit(m),
            },
            description: l.memo.clone(),
            tag: l.tag,
            contact: l.contact,
        })
        .collect();
    NewJournalEntry::new_with(
        c.date,
        c.memo.clone(),
        basis,
        lines,
        JeOptions {
            number: c.number.clone(),
            adjusting: c.adjusting,
            reverse_on: c.reverse_on,
        },
        roles,
    )
}

/// The tag rules on the entry's lines (P29 WP7, Q-276). An entry has no
/// header tag.
///
/// # Errors
/// The refused lines (nothing is sent).
pub fn tag_gate(
    form: &JournalForm,
    list: &TagList,
    accounts: &[Account],
) -> Result<(), TagRefusal> {
    let lines = cl_model::forms::tag_rule::expense_lines(&[], &form.lines, list, false);
    super::tag_gate::check(list, &lines, accounts)
}
