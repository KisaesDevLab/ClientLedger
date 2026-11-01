//! Changing a recorded transaction from the register (P12 S3): Enter (or
//! typing) on a row, or Ctrl+E for the Splits panel.
//!
//! An edit always starts from `GET /transactions/{id}` (the record and its
//! version), never from the register's figures. Simple transactions open in
//! the pinned row (a check, card charge or deposit with one line, a
//! transfer, a two-line journal entry); checks and card charges with more
//! lines open with Splits. Everything else shows its lines read-only
//! (Q-P12m). The save goes through `update_bank_posting` /
//! `update_journal_entry` with the version read; a change made elsewhere in
//! the meantime is refused (`Conflict { Stale }`) and offered a reload.

use cl_api::{ApiError, ConflictReason, Transaction, TxnStatus, TxnType, Versioned};
use cl_core::Money;
use cl_core::ids::{AccountId, TransactionId};
use cl_grid::{RowKey, TextKey};
use cl_model::register::{AccountClass, EntryDraft, EntryKind, Side};
use eframe::egui::{self, RichText, Ui};

use super::RegisterScreen;
use super::entry_row::{E_DEPOSIT, E_PAYMENT, Editing};
use super::splits::SplitRow;
use crate::company::Task;
use crate::i18n::t;
use crate::theme::Tokens;

/// One other-side line as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedLine {
    /// Account.
    pub account: AccountId,
    /// Amount.
    pub amount: Money,
    /// Line memo.
    pub memo: String,
    /// Line customer.
    pub customer: Option<cl_core::ids::ContactId>,
    /// Line tag.
    pub tag: Option<cl_core::ids::TagId>,
}

/// What an edit opens.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant, reason = "built once per edit")]
pub enum Seed {
    /// The transaction in the pinned row.
    Edit {
        /// The edit target.
        editing: Editing,
        /// The row's draft.
        draft: EntryDraft,
        /// The other-side lines.
        lines: Vec<SeedLine>,
    },
    /// Not a kind the register changes: its lines, read-only.
    ReadOnly(Vec<ReadLine>),
}

/// One line of the read-only view: account text and the engine's amounts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadLine {
    /// Account (number · name).
    pub account: String,
    /// Debit.
    pub debit: Money,
    /// Credit.
    pub credit: Money,
}

fn read_only(t: &Transaction) -> Seed {
    Seed::ReadOnly(
        t.lines
            .iter()
            .map(|l| ReadLine {
                account: match (&l.account_number, &l.account_name) {
                    (Some(n), Some(name)) if !n.is_empty() => format!("{n} \u{b7} {name}"),
                    (_, Some(name)) => name.clone(),
                    _ => String::new(),
                },
                debit: l.debit,
                credit: l.credit,
            })
            .collect(),
    )
}

/// The P14 form that shows the saved transaction `t` (its window kind), by
/// the shape of its lines (`class` gives each account's class): a deposit
/// (Make Deposit, or Put Money In when it comes only from equity), a
/// transfer, a card charge or credit, Pay Yourself (a bank payment to
/// equity), a loan payment (a bank payment with a liability line), money
/// paid with personal funds (a payment out of equity or a liability, no
/// bank or card), a daily sales day; a journal entry opens Make Journal
/// Entry (P15). `None`: a check or anything else (the register shows it
/// itself).
#[must_use]
pub fn form_for(
    t: &Transaction,
    class: &dyn Fn(AccountId) -> Option<AccountClass>,
) -> Option<&'static str> {
    use crate::screens::{
        card_charge, daily_sales, deposit, journal_entry, loan_payment, owner_money,
        personal_funds, transfer,
    };
    let debits: Vec<Option<AccountClass>> = t
        .lines
        .iter()
        .filter(|l| l.debit.is_positive())
        .map(|l| class(l.account_id))
        .collect();
    let credits: Vec<Option<AccountClass>> = t
        .lines
        .iter()
        .filter(|l| l.credit.is_positive())
        .map(|l| class(l.account_id))
        .collect();
    let has = |v: &[Option<AccountClass>], c: AccountClass| v.contains(&Some(c));
    let all = |v: &[Option<AccountClass>], c: AccountClass| {
        !v.is_empty() && v.iter().all(|x| *x == Some(c))
    };
    Some(match t.txn_type {
        // P18: a bill payment opens read-only with Void (Q-P18f).
        TxnType::BillPayment => crate::screens::pay_bills::view::KIND,
        // P21: a customer payment opens in Receive Payment (edit, void,
        // record a bounced check).
        TxnType::CustomerPayment => crate::screens::receive_payment::KIND,
        TxnType::DailySales => daily_sales::KIND,
        TxnType::JournalEntry => journal_entry::KIND,
        TxnType::Transfer => transfer::KIND,
        TxnType::Deposit if has(&debits, AccountClass::CreditCard) => card_charge::KIND,
        TxnType::Deposit
            if has(&debits, AccountClass::Bank) && all(&credits, AccountClass::Equity) =>
        {
            owner_money::KIND
        }
        TxnType::Deposit => deposit::KIND,
        TxnType::Expense if has(&credits, AccountClass::CreditCard) => card_charge::KIND,
        TxnType::Expense if has(&credits, AccountClass::Bank) => {
            if all(&debits, AccountClass::Equity) {
                owner_money::KIND
            } else if has(&debits, AccountClass::OtherLiability) {
                loan_payment::KIND
            } else {
                return None;
            }
        }
        TxnType::Expense
            if has(&credits, AccountClass::Equity)
                || has(&credits, AccountClass::OtherLiability) =>
        {
            personal_funds::KIND
        }
        _ => return None,
    })
}

/// When a row goes to its P14 form instead of the register's own edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Route {
    /// Only when the register can't change it in place (a read-only shape).
    Shape,
    /// The engine doesn't let the register change it: the form, else the
    /// "can't be edited here" note.
    NotEditable,
    /// Edit Transaction (Ctrl+E): the form whenever one shows it.
    Form,
}

/// What an edit of `v` in the register of `own` (class `class`) opens.
#[must_use]
pub fn seed(v: &Versioned<Transaction>, own: AccountId, class: AccountClass) -> Seed {
    let t = &v.record;
    let own_lines: Vec<_> = t.lines.iter().filter(|l| l.account_id == own).collect();
    let others: Vec<_> = t.lines.iter().filter(|l| l.account_id != own).collect();
    let [mine] = own_lines.as_slice() else {
        return read_only(t);
    };
    if t.status != TxnStatus::Posted || others.is_empty() {
        return read_only(t);
    }
    // A line that takes in a document (a payment out of Payments Clearing,
    // a refunded vendor credit) or has an engine role (cash back): an edit
    // here would send the deposit without its links and release them (B1).
    // Make Deposit edits it.
    if t.lines
        .iter()
        .any(|l| l.source_transaction_id.is_some() || l.line_role.is_some())
    {
        return read_only(t);
    }
    // A journal entry the register's row can't show whole (adjusting, a
    // reversing pair, a name on its line, a number: the row sends none, B3)
    // goes to Make Journal Entry.
    if t.txn_type == TxnType::JournalEntry
        && (t.is_adjusting
            || t.reverse_on.is_some()
            || t.reverses_transaction_id.is_some()
            || t.reversal_transaction_id.is_some()
            || mine.contact_id.is_some()
            || t.txn_number
                .as_deref()
                .is_some_and(|n| !n.trim().is_empty()))
    {
        return read_only(t);
    }
    let (side, amount) = if mine.credit.is_positive() && mine.debit.is_zero() {
        (Side::Payment, mine.credit)
    } else if mine.debit.is_positive() && mine.credit.is_zero() {
        (Side::Deposit, mine.debit)
    } else {
        return read_only(t);
    };
    // Every other line sits on the opposite side with a positive amount.
    let mut lines = Vec::new();
    for l in &others {
        let (m, zero) = match side {
            Side::Payment => (l.debit, l.credit),
            Side::Deposit => (l.credit, l.debit),
        };
        if !m.is_positive() || !zero.is_zero() {
            return read_only(t);
        }
        lines.push(SeedLine {
            account: l.account_id,
            amount: m,
            memo: l.description.clone().unwrap_or_default(),
            customer: l.contact_id,
            tag: l.tag_id,
        });
    }
    let any_customer = lines.iter().any(|l| l.customer.is_some());
    let kind = match (t.txn_type, side) {
        (TxnType::Expense, Side::Payment) if !any_customer => {
            if class == AccountClass::Bank {
                EntryKind::Check
            } else {
                EntryKind::CardCharge
            }
        }
        (TxnType::Deposit, Side::Deposit) if lines.len() == 1 => EntryKind::Deposit,
        (TxnType::Transfer, _) if lines.len() == 1 && !any_customer => EntryKind::Transfer,
        (TxnType::JournalEntry, _) if lines.len() == 1 && !any_customer => EntryKind::JournalEntry,
        _ => return read_only(t),
    };
    let payee = if kind == EntryKind::Deposit {
        lines[0].customer
    } else {
        t.contact_id
    };
    let number = match kind {
        EntryKind::Check => t
            .check_number
            .map(|n| n.to_string())
            .or_else(|| t.txn_number.clone()),
        _ => t.txn_number.clone(),
    };
    let draft = EntryDraft {
        date: Some(t.txn_date),
        number: number.unwrap_or_default(),
        payee,
        account: Some(lines[0].account),
        memo: t.memo.clone().unwrap_or_default(),
        payment: (side == Side::Payment).then_some(amount),
        deposit: (side == Side::Deposit).then_some(amount),
        tags: Vec::new(),
    };
    Seed::Edit {
        editing: Editing {
            id: t.id,
            old_date: t.txn_date,
            version: v.version,
            kind,
            side,
            basis: t.basis,
            own_line: (mine.description.clone(), mine.tag_id),
        },
        draft,
        lines,
    }
}

/// An edit being read.
pub(super) struct EditLoad {
    splits: bool,
    route: Route,
    /// The record, and whether it is a check (it has a print status).
    task: Task<(Versioned<Transaction>, bool)>,
}

impl RegisterScreen {
    /// Enter or typing on a row, or Ctrl+E: read the transaction and open
    /// it in the pinned row.
    pub(super) fn begin_edit(&mut self, key: RowKey, splits: bool) {
        self.begin_edit_routed(key, splits, Route::Shape);
    }

    /// [`Self::begin_edit`], with when the row goes to its P14 form
    /// (`CompanyEvent::OpenRecord`) instead.
    pub(super) fn begin_edit_routed(&mut self, key: RowKey, splits: bool, route: Route) {
        let Some(row) = self.model.row_for_key(key) else {
            return;
        };
        let id = row.txn_id;
        if !row.editable {
            // A void row stays as it is; any other the engine doesn't let
            // the register change may still have its form (a daily sales
            // day, a grouped deposit).
            if row.void {
                self.entry.problems = vec![TextKey("register.edit.not_editable")];
            } else {
                self.load_routed(id, splits, Route::NotEditable);
            }
            return;
        }
        if self.model.locked(row) && !self.closing_supported() {
            self.on_locked_edit(key);
            return;
        }
        if self.entry.editing.is_none() && self.entry_dirty() {
            self.entry.problems = vec![TextKey("register.edit.finish_entry")];
            return;
        }
        self.load_routed(id, splits, route);
    }

    /// Reads `id` for an edit.
    pub(super) fn load_edit(&mut self, id: TransactionId, splits: bool) {
        self.load_routed(id, splits, Route::Shape);
    }

    fn load_routed(&mut self, id: TransactionId, splits: bool, route: Route) {
        self.read_only = None;
        self.entry.problems.clear();
        self.entry.error = None;
        let task = if let Some(mut demo) = self.link.demo_state() {
            let chart = demo
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            let contacts = demo.contacts_mut().clone();
            let got = demo.ledger.get(&chart, &contacts, id);
            // The demo's stand-in for a print status: a check Write Check
            // keeps details for, or one with a number.
            let check = got
                .as_ref()
                .is_some_and(|v| v.record.txn_type == TxnType::Expense)
                && (demo.checks.knows(id)
                    || got
                        .as_ref()
                        .is_some_and(|v| v.record.check_number.is_some()));
            drop(demo);
            match got {
                Some(v) => Task::ready((v, check)),
                None => return,
            }
        } else {
            let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
                return;
            };
            match group.read(&session, move |s| async move {
                let v = s.transactions().get(id).await?;
                // A check has a print status: only the check read tells.
                let check = v.record.txn_type == TxnType::Expense
                    && match s.checks().get(id).await {
                        Ok(_) => true,
                        Err(ApiError::Decode { .. }) => false,
                        Err(e) => return Err(e),
                    };
                Ok((v, check))
            }) {
                Ok(p) => Task::Pending(p),
                Err(_) => return,
            }
        };
        self.edit_load = Some(EditLoad {
            splits,
            route,
            task,
        });
        self.poll_edit();
    }

    /// The P14 form for `t`, if one shows it.
    fn form_kind(&self, t: &Transaction) -> Option<&'static str> {
        let class = |id: AccountId| {
            self.accounts
                .iter()
                .find(|a| a.id == id)
                .and_then(super::class_of)
        };
        form_for(t, &class)
    }

    /// Takes a finished read and opens the edit.
    pub(super) fn poll_edit(&mut self) {
        let Some(load) = self.edit_load.as_mut() else {
            return;
        };
        let Some(result) = load.task.try_take() else {
            return;
        };
        let want_splits = load.splits;
        let route = load.route;
        self.edit_load = None;
        let Some(own) = self.account else { return };
        let (v, is_check) = match result {
            Ok(v) => v,
            Err(e) => {
                self.entry.error = Some(e);
                return;
            }
        };
        // A check is changed in Write Check: its printed-check question
        // (Q-P13i) and its header tags go through `update_check` there; the
        // row's plain expense update has neither (B4).
        let form = if is_check && v.record.status == TxnStatus::Posted {
            Some(crate::screens::write_check::KIND)
        } else {
            self.form_kind(&v.record)
        };
        if is_check && form.is_some() {
            self.read_only = None;
            self.link.push(crate::company::CompanyEvent::OpenRecord(
                cl_model::CommandId::new(crate::screens::write_check::KIND),
                v.record.id,
            ));
            return;
        }
        let s = seed(&v, own, self.class);
        let to_form = match route {
            Route::Form | Route::NotEditable => form,
            Route::Shape => form.filter(|_| matches!(s, Seed::ReadOnly(_))),
        };
        if let Some(kind) = to_form {
            // Double-click / Edit opens the form that owns it (P14).
            self.read_only = None;
            self.link.push(crate::company::CompanyEvent::OpenRecord(
                cl_model::CommandId::new(kind),
                v.record.id,
            ));
            return;
        }
        if route == Route::NotEditable {
            self.entry.problems = vec![TextKey("register.edit.not_editable")];
            return;
        }
        self.open_seed(s, want_splits);
    }

    fn open_seed(&mut self, s: Seed, want_splits: bool) {
        let (editing, draft, lines) = match s {
            Seed::ReadOnly(lines) => {
                self.read_only = Some(lines);
                return;
            }
            Seed::Edit {
                editing,
                draft,
                lines,
            } => (editing, draft, lines),
        };
        let lists = std::sync::Arc::clone(&self.lists);
        self.close_splits();
        self.entry.form.load(draft, &*lists);
        self.entry.proposed = None;
        self.entry.key = None;
        let a = self.entry.form.adapter_mut();
        a.lock_number = editing.kind != EntryKind::CardCharge;
        a.label = t("register.entry.row_edit");
        let side = editing.side;
        let kind = editing.kind;
        self.entry.editing = Some(editing);
        self.edit_lines = lines
            .iter()
            .map(|l| SplitRow {
                account: Some(l.account),
                amount: Some(l.amount),
                memo: l.memo.clone(),
                customer: l.customer,
                tag: l.tag,
                ..SplitRow::blank(cl_grid::RowKeyGen::new().next_key())
            })
            .collect();
        let splittable = matches!(kind, EntryKind::Check | EntryKind::CardCharge);
        if lines.len() > 1 || (want_splits && splittable) {
            self.open_splits(
                lines
                    .into_iter()
                    .map(|l| (Some(l.account), Some(l.amount), l.memo, l.customer, l.tag))
                    .collect(),
            );
        } else {
            self.entry.focus = Some(match side {
                Side::Payment => E_PAYMENT,
                Side::Deposit => E_DEPOSIT,
            });
        }
    }

    /// A save refused because the transaction changed elsewhere: read it
    /// again and restart the edit.
    pub(super) fn reload_edit(&mut self) {
        let Some(e) = &self.entry.editing else { return };
        let id = e.id;
        let splits = self.splits.is_some();
        self.load_edit(id, splits);
    }

    /// The last save failed because the record changed elsewhere.
    pub(super) fn stale(&self) -> bool {
        matches!(
            self.entry.error,
            Some(ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            })
        )
    }

    /// The edit's extra lines: the read-only view, the stale-save offer.
    pub(super) fn edit_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if self.stale() && self.entry.editing.is_some() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(t("register.edit.changed_elsewhere")).color(tokens.warn_text),
                );
                if ui.button(t("register.edit.reload")).clicked() {
                    self.reload_edit();
                }
            });
        }
        let style = self.parts.frame.money_style;
        let mut close = false;
        if let Some(lines) = &self.read_only {
            ui.label(RichText::new(t("register.edit.read_only")).color(tokens.text_secondary));
            egui::Grid::new("register/read_only")
                .num_columns(3)
                .spacing([16.0, 2.0])
                .show(ui, |ui| {
                    ui.label(RichText::new(t("register.entry.col.account")).strong());
                    ui.label(RichText::new(t("register.edit.debit")).strong());
                    ui.label(RichText::new(t("register.edit.credit")).strong());
                    ui.end_row();
                    for l in lines {
                        ui.label(&l.account);
                        let amount = |m: Money| {
                            if m.is_zero() {
                                String::new()
                            } else {
                                m.format(style)
                            }
                        };
                        ui.label(RichText::new(amount(l.debit)).monospace());
                        ui.label(RichText::new(amount(l.credit)).monospace());
                        ui.end_row();
                    }
                });
            close = ui.button(t("register.edit.close")).clicked();
        }
        if close {
            self.read_only = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_api::{RecordVersion, TransactionLine};
    use cl_core::CalendarDate;
    use cl_core::ids::JournalLineId;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    fn acct(n: u32) -> AccountId {
        crate::company::demo::account_id(n)
    }

    fn line(i: u32, account: u32, debit: &str, credit: &str) -> TransactionLine {
        TransactionLine {
            id: JournalLineId::parse(&format!("d0000000-0000-4000-8000-c{i:011}")).expect("id"),
            account_id: acct(account),
            account_name: Some(format!("A{account}")),
            account_number: Some(account.to_string()),
            debit: money(debit),
            credit: money(credit),
            description: None,
            line_order: Some(i64::from(i)),
            tag_id: None,
            tag_name: None,
            contact_id: None,
            contact_name: None,
            is_billable: false,
            billed_status: None,
            source_transaction_id: None,
            line_role: None,
        }
    }

    fn txn(kind: TxnType, lines: Vec<TransactionLine>) -> Versioned<Transaction> {
        Versioned {
            record: Transaction {
                id: TransactionId::parse("d0000000-0000-4000-8000-b00000000001").expect("id"),
                txn_type: kind,
                txn_number: None,
                check_number: Some(4500),
                txn_date: CalendarDate::from_ymd(2026, 11, 2).expect("date"),
                status: TxnStatus::Posted,
                basis: cl_api::Basis::Both,
                contact_id: None,
                contact_name: None,
                memo: Some("memo".into()),
                total: None,
                void_reason: None,
                created_at: None,
                updated_at: None,
                lines,
                is_adjusting: false,
                reverses_transaction_id: None,
                reversal_transaction_id: None,
                reverse_on: None,
            },
            version: RecordVersion::Fingerprint([7; 32]),
        }
    }

    #[test]
    fn simple_rows_open_in_the_row_from_the_transaction_itself() {
        let v = txn(
            TxnType::Expense,
            vec![line(0, 6100, "40.25", "0"), line(1, 1010, "0", "40.25")],
        );
        let Seed::Edit {
            editing,
            draft,
            lines,
        } = seed(&v, acct(1010), AccountClass::Bank)
        else {
            panic!("edit");
        };
        assert_eq!(editing.kind, EntryKind::Check);
        assert_eq!(editing.side, Side::Payment);
        assert_eq!(editing.version, RecordVersion::Fingerprint([7; 32]));
        assert_eq!(draft.payment, Some(money("40.25")));
        assert_eq!(draft.number, "4500");
        assert_eq!(draft.account, Some(acct(6100)));
        assert_eq!(lines.len(), 1);
        // A split check keeps every line for the Splits panel.
        let v = txn(
            TxnType::Expense,
            vec![
                line(0, 6100, "40.00", "0"),
                line(1, 6350, "0.25", "0"),
                line(2, 1010, "0", "40.25"),
            ],
        );
        let Seed::Edit { lines, .. } = seed(&v, acct(1010), AccountClass::Bank) else {
            panic!("edit");
        };
        assert_eq!(lines.len(), 2);
        // A transfer seen from the receiving account is on the deposit side.
        let v = txn(
            TxnType::Transfer,
            vec![line(0, 1020, "10", "0"), line(1, 1010, "0", "10")],
        );
        let Seed::Edit { editing, .. } = seed(&v, acct(1020), AccountClass::Bank) else {
            panic!("edit");
        };
        assert_eq!(
            (editing.kind, editing.side),
            (EntryKind::Transfer, Side::Deposit)
        );
    }

    #[test]
    fn a_customer_payment_opens_in_receive_payment() {
        let payment = txn(
            TxnType::CustomerPayment,
            vec![line(0, 1010, "10", "0"), line(1, 1200, "0", "10")],
        );
        assert_eq!(
            form_for(&payment.record, &|_| None),
            Some(crate::screens::receive_payment::KIND)
        );
    }

    #[test]
    fn other_kinds_are_read_only() {
        let payment = txn(
            TxnType::CustomerPayment,
            vec![line(0, 1010, "10", "0"), line(1, 1200, "0", "10")],
        );
        assert!(
            matches!(seed(&payment, acct(1010), AccountClass::Bank), Seed::ReadOnly(l) if l.len() == 2)
        );
        let multi_deposit = txn(
            TxnType::Deposit,
            vec![
                line(0, 1010, "15", "0"),
                line(1, 4000, "0", "10"),
                line(2, 4000, "0", "5"),
            ],
        );
        assert!(matches!(
            seed(&multi_deposit, acct(1010), AccountClass::Bank),
            Seed::ReadOnly(_)
        ));
        let mut void = txn(
            TxnType::Expense,
            vec![line(0, 6100, "1", "0"), line(1, 1010, "0", "1")],
        );
        void.record.status = TxnStatus::Void;
        let Seed::ReadOnly(lines) = seed(&void, acct(1010), AccountClass::Bank) else {
            panic!("read only");
        };
        assert_eq!(lines[0].account, "6100 \u{b7} A6100");
    }
}
