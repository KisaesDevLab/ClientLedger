//! Personal Funds (P16): an expense paid from an equity or liability
//! account, as the engine posting (`BankPosting::card_charge_with`), field
//! by field (no arithmetic). Who paid rides in the memo
//! (`cl_model::forms::personal_funds::memo_with_who`).

use cl_api::{Account, BankPosting, CardChargeDraft, LocalValidation, SplitLine};
use cl_model::forms::personal_funds::{CheckedPersonalFunds, PersonalFundsForm, memo_with_who};
use cl_model::forms::tag_rule::TagRefusal;
use cl_model::lists::tags::TagList;

/// The checked form with who paid (`who_name`, the contact's name) at the
/// start of its memo: what is sent.
#[must_use]
pub fn with_who(c: &CheckedPersonalFunds, who_name: Option<&str>) -> CheckedPersonalFunds {
    let memo = memo_with_who(who_name, c.memo.as_deref());
    CheckedPersonalFunds { memo, ..c.clone() }
}

/// The checked form (its memo already carrying who paid, [`with_who`]) as
/// the engine posting. `line_billable` is whether the engine takes
/// billable lines.
///
/// # Errors
/// What `BankPosting` refuses (nothing is sent).
pub fn posting(
    c: &CheckedPersonalFunds,
    line_billable: bool,
) -> Result<BankPosting, LocalValidation> {
    BankPosting::card_charge_with(
        CardChargeDraft {
            pay_from: c.offset,
            date: c.date,
            number: None,
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
        },
        line_billable,
    )
}

/// The tag rules on the form as composed (P29 WP7, Q-276).
///
/// # Errors
/// The refused lines (nothing is sent).
pub fn tag_gate(
    form: &PersonalFundsForm,
    list: &TagList,
    accounts: &[Account],
) -> Result<(), TagRefusal> {
    let lines =
        cl_model::forms::tag_rule::expense_lines(&form.header_tags, &form.lines, list, false);
    super::tag_gate::check(list, &lines, accounts)
}
