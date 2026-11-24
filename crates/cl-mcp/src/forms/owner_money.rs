//! Owner Contributions and Draws (P16): the checked form as the engine
//! posting, field by field: one line of exactly the typed amount.

use cl_api::{BankPosting, CardChargeDraft, CheckDraft, DepositDraft, LocalValidation, SplitLine};
use cl_model::forms::owner_money::{CheckedOwnerMoney, OwnerPosting};

/// The checked form as the engine posting: a check, a card or bank payment,
/// or a deposit, each with one equity line of exactly the typed amount.
/// `name` is the name printed on a check (the contact's, else the typed).
///
/// # Errors
/// What `BankPosting` refuses (nothing is sent).
pub fn posting(
    c: &CheckedOwnerMoney,
    name: Option<String>,
) -> Result<BankPosting, LocalValidation> {
    let line = SplitLine {
        account: c.equity,
        amount: c.amount,
        memo: None,
        contact: None,
        billable: false,
        tag: None,
        item: None,
    };
    match c.posting {
        OwnerPosting::Check { number } => BankPosting::check(CheckDraft {
            bank: c.bank,
            date: c.date,
            payee: c.who,
            payee_name: name.unwrap_or_default(),
            check_number: number,
            print_later: false,
            payee_address: None,
            memo: c.memo.clone(),
            printed_memo: c.memo.clone(),
            tag_ids: Vec::new(),
            draft_attachment: None,
            amount: c.amount,
            lines: vec![line],
        }),
        OwnerPosting::Payment => BankPosting::card_charge(CardChargeDraft {
            pay_from: c.bank,
            date: c.date,
            number: None,
            payee: c.who,
            memo: c.memo.clone(),
            lines: vec![line],
        }),
        OwnerPosting::Deposit => BankPosting::deposit(DepositDraft::plain(
            c.bank,
            c.date,
            c.memo.clone(),
            vec![SplitLine {
                contact: c.who,
                ..line
            }],
        )),
    }
}
