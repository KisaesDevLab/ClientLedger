//! Enter Credit Card Charges (P16): a charge or a card credit, checked by
//! `cl_model::forms::card::check`, as the engine's posting, field by field
//! (no arithmetic).

use cl_api::{Account, BankPosting, CardChargeDraft, DepositDraft, LocalValidation, SplitLine};
use cl_model::forms::card::{CardForm, CardMode, CheckedCard};
use cl_model::forms::tag_rule::TagRefusal;
use cl_model::lists::tags::TagList;

/// The checked form as the engine posting: a charge (with its item lines)
/// or a card credit (a deposit on the card; each line's name falls back to
/// the payee). `line_billable` is whether the engine takes billable lines.
///
/// # Errors
/// What `BankPosting` refuses (nothing is sent).
pub fn posting(c: &CheckedCard, line_billable: bool) -> Result<BankPosting, LocalValidation> {
    let split = |l: &cl_model::forms::lines::PostingLine| SplitLine {
        account: l.account,
        amount: l.amount,
        memo: l.memo.clone(),
        contact: l.contact,
        billable: l.billable,
        tag: l.tag,
        item: None,
    };
    let posting = match c.mode {
        CardMode::Charge => BankPosting::card_charge_with(
            CardChargeDraft {
                pay_from: c.card,
                date: c.date,
                number: c.ref_no.clone(),
                payee: c.payee,
                memo: c.memo.clone(),
                lines: c
                    .lines
                    .iter()
                    .map(split)
                    .chain(c.item_lines.iter().map(super::check::item_split))
                    .collect(),
            },
            line_billable,
        )?,
        CardMode::Credit => BankPosting::deposit(DepositDraft::plain(
            c.card,
            c.date,
            c.memo.clone(),
            c.lines
                .iter()
                .map(|l| SplitLine {
                    contact: l.contact.or(c.payee),
                    billable: false,
                    ..split(l)
                })
                .collect(),
        ))?,
    };
    Ok(posting)
}

/// The checked charge as the engine draft (no arithmetic): the A04 draft
/// mode's convert.
#[must_use]
pub fn charge_draft(c: &CheckedCard) -> CardChargeDraft {
    CardChargeDraft {
        pay_from: c.card,
        date: c.date,
        number: c.ref_no.clone(),
        payee: c.payee,
        memo: c.memo.clone(),
        lines: c
            .lines
            .iter()
            .map(|l| SplitLine {
                account: l.account,
                amount: l.amount,
                memo: l.memo.clone(),
                contact: l.contact,
                billable: l.billable,
                tag: l.tag,
                item: None,
            })
            .collect(),
    }
}

/// The tag rules on the form as composed (P29 WP7, Q-276): `list` is the
/// company's tag list with its rules as they are now, `accounts` the chart
/// the form read.
///
/// # Errors
/// The refused lines (nothing is sent).
pub fn tag_gate(form: &CardForm, list: &TagList, accounts: &[Account]) -> Result<(), TagRefusal> {
    let lines =
        cl_model::forms::tag_rule::tabbed_lines(&form.header_tags, &form.lines, &form.items, list);
    super::tag_gate::check(list, &lines, accounts)
}
