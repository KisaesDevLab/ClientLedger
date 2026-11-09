//! Pay Bills opened from a receipt (PR-A1 S1, Q-PRA1a): Review Documents'
//! "Pay this bill" for a receipt that matches an unpaid bill.
//!
//! - **Open** ([`PayBillsScreen::open_from_receipt`]): the payment date is
//!   the receipt's, every bill is listed (the bill may not be due yet),
//!   and once the accounts and the list are read the bill is ticked and
//!   the receipt's card or bank account picked (a card pays by card, a
//!   bank account by check). A bill no longer in the list (paid
//!   meanwhile) is not ticked and the screen says so; the receipt stays
//!   in Review Documents.
//! - **After the pay**: the payment that paid the bill (the vendor's) gets
//!   the receipt through `POST /document-drafts/{id}/attach` (a bill
//!   payment takes files), one key per pay. The receipt's original then
//!   moves as for any attach (the inbox's `handled_by_attachment`). A
//!   refusal leaves the receipt in Review Documents and says so; nothing
//!   is paid twice (the pay already went through on its own key).
//!
//! Nothing here logs; `Debug` shows ids only.

use std::fmt;

use cl_api::{
    ApiError, AttachmentId, CreatedPayment, DocumentDraftId, IdempotencyKey, OrphanLabel,
};
use cl_core::CalendarDate;
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_model::register::AccountClass;

use super::{Method, PayBillsScreen};
use crate::company::Task;
use crate::screens::company::form::DateInput;

/// What "Pay this bill" hands Pay Bills.
#[derive(Clone, PartialEq, Eq)]
pub struct PayBillPrefill {
    /// The bill.
    pub bill: TransactionId,
    /// Its vendor (the payment to file the receipt on is the vendor's).
    pub vendor: Option<ContactId>,
    /// The receipt's card or bank account.
    pub from: Option<AccountId>,
    /// The receipt's date.
    pub date: CalendarDate,
    /// The receipt.
    pub receipt: DocumentDraftId,
    /// Its change token as read (`updatedAt`).
    pub expected_updated_at: String,
    /// Its file (the inbox moves its original once filed).
    pub attachment: Option<AttachmentId>,
}

impl fmt::Debug for PayBillPrefill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayBillPrefill")
            .field("bill", &self.bill)
            .field("receipt", &self.receipt)
            .finish_non_exhaustive()
    }
}

/// Where the receipt stands.
#[derive(Default)]
pub(crate) struct ReceiptPay {
    /// Waiting for the accounts and the list to tick the bill.
    pub waiting: Option<PayBillPrefill>,
    /// The bill is ticked: the receipt goes on the payment that pays it.
    pub armed: Option<PayBillPrefill>,
    task: Option<Task<TransactionId>>,
    /// The receipt is on the payment (control channel).
    pub filed: Option<TransactionId>,
    /// The original moves next frame (the inbox needs the window's
    /// context).
    pub handled: Option<AttachmentId>,
}

impl PayBillsScreen {
    /// Opens on the bill a receipt pays (Q-PRA1a).
    pub(crate) fn open_from_receipt(&mut self, p: PayBillPrefill) {
        self.date = DateInput::new(p.date);
        self.show_all = true;
        self.form.show = cl_model::forms::bill_payment::ShowBills::All;
        self.receipt = ReceiptPay {
            waiting: Some(p),
            ..ReceiptPay::default()
        };
        self.notice = None;
        self.load_list();
        self.apply_receipt();
    }

    /// Ticks the bill once the accounts and the list are read.
    pub(crate) fn apply_receipt(&mut self) {
        if self.receipt.waiting.is_none()
            || !self.loaded
            || !self.list_loaded
            || self.list_task.is_some()
        {
            return;
        }
        let Some(p) = self.receipt.waiting.take() else {
            return;
        };
        if let Some(from) = p.from
            && let Some(a) = self.accounts.iter().find(|a| a.id == from)
        {
            match crate::screens::register::class_of(a) {
                Some(AccountClass::CreditCard) => {
                    self.method = Method::Card;
                    self.card = Some(from);
                }
                Some(AccountClass::Bank) => {
                    self.method = Method::Check;
                    self.bank = Some(from);
                }
                _ => {}
            }
            self.account_changed();
        }
        if self.form.all_rows().iter().any(|r| r.bill == p.bill) {
            self.form.set_selected(p.bill, true);
            self.revision += 1;
            self.notice = Some("paybills.receipt.ready");
            self.receipt.armed = Some(p);
        } else {
            // Paid (or voided) since the receipt was matched.
            self.notice = Some("paybills.receipt.not_open");
        }
    }

    /// The pay went through: the receipt goes on the payment of its bill's
    /// vendor, when the bill was in the pay.
    pub(crate) fn file_receipt(&mut self, paid: &[TransactionId], payments: &[CreatedPayment]) {
        let Some(p) = self.receipt.armed.take() else {
            return;
        };
        if !paid.contains(&p.bill) {
            // Unticked before paying: the receipt waits for its own pay.
            self.receipt.armed = Some(p);
            return;
        }
        let payment = match p.vendor {
            Some(v) => payments.iter().find(|x| x.contact_id == Some(v)),
            None if payments.len() == 1 => payments.first(),
            None => None,
        };
        let Some(payment) = payment.map(|x| x.id) else {
            self.notice = Some("paybills.receipt.not_filed");
            return;
        };
        let key = IdempotencyKey::new();
        let (id, expected) = (p.receipt, p.expected_updated_at.clone());
        if self.link.is_demo() {
            let out = match self.link.demo_state() {
                Some(mut d) => match d.open {
                    Some(tenant) => crate::company::demo::drafts::attach(
                        &mut d, tenant, id, payment, &expected, key,
                    )
                    .map(|a| a.transaction),
                    None => Err(ApiError::NoCompanyOpen),
                },
                None => Err(ApiError::NoCompanyOpen),
            };
            self.receipt_answer(out.ok(), Some(p));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.notice = Some("paybills.receipt.not_filed");
            return;
        };
        let label = OrphanLabel {
            what: "orphan.draft_attach",
            key: Some(key.clone()),
        };
        match group.write(&session, label, move |s| async move {
            s.document_drafts()
                .attach(id, payment, &expected, key)
                .await
                .map(|a| a.transaction)
        }) {
            Ok(t) => {
                self.receipt.task = Some(Task::Pending(t));
                self.receipt.armed = Some(p);
            }
            Err(_) => self.notice = Some("paybills.receipt.not_filed"),
        }
    }

    /// The attach answered.
    pub(crate) fn poll_receipt(&mut self) {
        let Some(r) = self.receipt.task.as_mut().and_then(Task::try_take) else {
            return;
        };
        self.receipt.task = None;
        let p = self.receipt.armed.take();
        self.receipt_answer(r.ok(), p);
    }

    fn receipt_answer(&mut self, r: Option<TransactionId>, p: Option<PayBillPrefill>) {
        match r {
            Some(t) => {
                self.receipt.filed = Some(t);
                self.receipt.handled = p.and_then(|p| p.attachment);
                self.notice = Some("paybills.receipt.filed");
            }
            // The pay stands; the receipt stays in Review Documents (it
            // can be attached there to the payment).
            None => self.notice = Some("paybills.receipt.not_filed"),
        }
    }

    /// The attach is in flight.
    pub(crate) fn receipt_busy(&self) -> bool {
        self.receipt.task.is_some()
    }
}
