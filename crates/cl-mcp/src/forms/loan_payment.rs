//! Loan Payment (P16): the checked payment as the engine posting, field by
//! field (no arithmetic: the lines come from `check`).

use cl_api::{Account, BankPosting, CardChargeDraft, CheckDraft, LocalValidation, SplitLine};
use cl_model::forms::loan_payment::{CheckedLoanPayment, LoanPaymentForm, LoanPosting};
use cl_model::forms::tag_rule::TagRefusal;
use cl_model::lists::tags::TagList;

/// The checked payment as the engine posting: a check or an online
/// payment. `lender_name` is the name printed on a check.
///
/// # Errors
/// What `BankPosting` refuses (nothing is sent).
pub fn posting(
    c: &CheckedLoanPayment,
    lender_name: Option<String>,
) -> Result<BankPosting, LocalValidation> {
    let lines: Vec<SplitLine> = c
        .lines
        .iter()
        .map(|l| SplitLine {
            account: l.account,
            amount: l.amount,
            memo: l.memo.clone(),
            contact: None,
            billable: false,
            tag: None,
            item: None,
        })
        .collect();
    match c.posting {
        LoanPosting::Check(number) => BankPosting::check(CheckDraft {
            bank: c.pay_from,
            date: c.date,
            payee: c.lender,
            payee_name: lender_name.unwrap_or_default(),
            check_number: number,
            print_later: false,
            payee_address: None,
            memo: c.memo.clone(),
            printed_memo: c.memo.clone(),
            tag_ids: Vec::new(),
            draft_attachment: None,
            amount: c.payment,
            lines,
        }),
        LoanPosting::Online => BankPosting::card_charge(CardChargeDraft {
            pay_from: c.pay_from,
            date: c.date,
            number: None,
            payee: c.lender,
            memo: c.memo.clone(),
            lines,
        }),
    }
}

/// The tag rules on the extra lines (P29 WP7, Q-276). The principal and
/// interest lines have no tag field and are not checked.
///
/// # Errors
/// The refused lines (nothing is sent).
pub fn tag_gate(
    form: &LoanPaymentForm,
    list: &TagList,
    accounts: &[Account],
) -> Result<(), TagRefusal> {
    let lines = cl_model::forms::tag_rule::expense_lines(&[], &form.extras, list, false);
    super::tag_gate::check(list, &lines, accounts)
}
