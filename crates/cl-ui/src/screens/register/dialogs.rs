//! Closed periods, Void, Delete and their conflicts in the register (P12
//! WP8, S5; non-negotiable 8).
//!
//! - **Closed period.** A change that touches a date on or before the
//!   closing date (an edit of a locked row, a date moved into the closed
//!   period, a void or delete of a locked row), or an engine 423, starts
//!   the closing-date password flow (`cl_model::closing::ClosingFlow`). The
//!   password is typed into a masked, zeroizing field, handed to the one
//!   save it is for (`closingPassword`), and dropped when the engine
//!   answers (Q-P12j: asked for every change, never kept). A wrong password
//!   keeps the dialog open; too many tries is a plain message. Without
//!   `closing-password-v1` the dialog only explains that closed-period
//!   transactions can't be changed yet.
//! - **Void** asks first (the reason is optional) and then voids.
//! - **Delete** (R7, `txn-delete-v1`) asks first, saying what happens, and
//!   only for the kinds the engine deletes (Q-245); otherwise, and without
//!   the capability, it offers Void instead. A reconciled transaction gets
//!   a plain message.
//! - **Conflicts.** Void and Delete read the transaction when they ask; if
//!   it changed before the owner confirms, nothing is sent and the owner
//!   is offered Reload.
//! - **Bills, vendor credits and bill payments** (P17 `WP2b`, review B1)
//!   never go through the generic `POST /transactions/{id}/void` or
//!   `DELETE /transactions/{id}`: on the pinned engine those do no A/P
//!   checks, so a paid bill or a bill payment voided there keeps its
//!   payment links and A/P and the documents disagree. Void goes to the
//!   document's own route ([`VoidRoute`]): a bill or vendor credit void is
//!   refused by the engine once payments or applications exist (shown in
//!   plain words), and a bill payment void takes its applications off the
//!   bills and credits it paid. Delete is refused with an offer to void.
//!   Those routes take no closing-date password, so a closed-period A/P
//!   row is explained instead. The routing stays the same once the pin
//!   carries `ap-edit-v1`, which also guards the generic routes.
//!
//! Nothing here is logged; the control-channel state carries flags only.

use cl_api::{
    ApiError, Capabilities, ClearState, ClosedPeriodAuthorization, ConflictReason, DeleteOptions,
    IdempotencyKey, OrphanLabel, PostingOptions, RecordVersion, Session, Transaction, TxnStatus,
    TxnType, Versioned,
};
use cl_core::ids::TransactionId;
use cl_core::{CalendarDate, Secret};
use cl_model::CommandId;
use cl_model::closing::is_locked;
use eframe::egui::{self, RichText, Ui};

use super::RegisterScreen;
use crate::company::Task;
use crate::forms::closing_prompt::{ClosingClick, ClosingPrompt};
use crate::i18n::t;
use crate::screens::company::form;
use crate::theme::Tokens;

/// Void or Delete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActionKind {
    /// Void (amounts become zero; it stays listed).
    Void,
    /// Soft delete (R7).
    Delete,
}

impl ActionKind {
    fn name(self) -> &'static str {
        match self {
            ActionKind::Void => "void",
            ActionKind::Delete => "delete",
        }
    }
}

/// Where a row's Void goes, by the kind of transaction (P17 `WP2b`, review
/// B1). Accounts Payable documents use their own engine routes, never the
/// generic ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VoidRoute {
    /// `POST /transactions/{id}/void` (and Delete, where the kind allows).
    Generic,
    /// `POST /bills/{id}/void`: refused once payments are applied.
    Bill,
    /// `POST /vendor-credits/{id}/void`: refused once applied to bills.
    VendorCredit,
    /// `POST /bill-payments/{id}/void`: takes its applications off the
    /// bills and credits it paid.
    BillPayment,
}

impl VoidRoute {
    /// The route for a transaction of kind `t`.
    pub(super) const fn of(t: TxnType) -> VoidRoute {
        match t {
            TxnType::Bill => VoidRoute::Bill,
            TxnType::VendorCredit => VoidRoute::VendorCredit,
            TxnType::BillPayment => VoidRoute::BillPayment,
            _ => VoidRoute::Generic,
        }
    }

    /// An Accounts Payable document (bill, vendor credit, bill payment).
    pub(super) const fn is_ap(self) -> bool {
        !matches!(self, VoidRoute::Generic)
    }

    /// Why Delete is refused for this kind (it is offered Void instead);
    /// `None` when the generic rules decide.
    pub(super) const fn delete_refused(self) -> Option<&'static str> {
        match self {
            VoidRoute::Generic => None,
            VoidRoute::Bill | VoidRoute::VendorCredit => Some("register.ap.delete_document"),
            VoidRoute::BillPayment => Some("register.ap.delete_payment"),
        }
    }

    /// The plain message for the engine's refusal of this void (payments
    /// or applications exist, or a finished reconciliation).
    pub(super) const fn refused(self) -> Option<&'static str> {
        match self {
            VoidRoute::Generic => None,
            VoidRoute::Bill => Some("register.ap.void_bill_paid"),
            VoidRoute::VendorCredit => Some("register.ap.void_credit_applied"),
            VoidRoute::BillPayment => Some("register.ap.void_payment_refused"),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            VoidRoute::Generic => "generic",
            VoidRoute::Bill => "bill",
            VoidRoute::VendorCredit => "vendor_credit",
            VoidRoute::BillPayment => "bill_payment",
        }
    }
}

/// One row action as the write queue sends it (no `Debug`: it holds the
/// owner's reason and maybe the closing-date password).
pub(super) struct RowWrite {
    pub(super) kind: ActionKind,
    pub(super) route: VoidRoute,
    pub(super) id: TransactionId,
    /// The version the owner was asked at (refused when it moved).
    pub(super) version: Option<RecordVersion>,
    /// Delete's reason (optional).
    pub(super) reason: Option<String>,
    /// Void's reason (always one).
    pub(super) void_reason: String,
    /// Only ever for the generic routes.
    pub(super) password: Option<Secret>,
    pub(super) key: IdempotencyKey,
}

/// Sends one row action: an A/P document's void to its own route, anything
/// else to the generic void or delete. An A/P Delete, or a password for an
/// A/P route, is refused here with nothing sent (the screen never builds
/// one; this keeps the generic routes unreachable for A/P documents).
///
/// # Errors
/// `Conflict { Stale }` when the transaction changed since the owner was
/// asked; `Local(InvalidValue)` for the refused combinations; any
/// [`ApiError`] from the engine.
pub(super) async fn row_write(s: Session, w: RowWrite) -> Result<(), ApiError> {
    if w.route.is_ap() && (w.kind == ActionKind::Delete || w.password.is_some()) {
        return Err(ApiError::Local(cl_api::LocalValidation::InvalidValue));
    }
    let t = s.transactions();
    // Refuse a change made elsewhere since the owner was asked.
    // Best-effort until U-17 (`record-version-v1`): the engine's void
    // and delete take no version, so this read-then-send leaves a
    // short window in which another window's edit is not caught.
    // With U-17 the version goes in `If-Match` and the engine checks
    // it atomically.
    if let Some(expected) = w.version {
        let now = t.get(w.id).await?;
        if now.version != expected {
            return Err(ApiError::Conflict {
                reason: ConflictReason::Stale,
                current_version: Some(now.version),
                code: None,
                message: cl_api::UserText::default(),
            });
        }
    }
    let ap = s.ap_voids();
    match (w.kind, w.route) {
        (ActionKind::Void, VoidRoute::Generic) => {
            t.void(w.id, &w.void_reason, &with_password(w.password), w.key)
                .await?;
        }
        (ActionKind::Void, VoidRoute::Bill) => ap.bill(w.id, &w.void_reason, w.key).await?,
        (ActionKind::Void, VoidRoute::VendorCredit) => {
            ap.vendor_credit(w.id, &w.void_reason, w.key).await?;
        }
        (ActionKind::Void, VoidRoute::BillPayment) => {
            ap.bill_payment(w.id, &w.void_reason, w.key).await?;
        }
        (ActionKind::Delete, _) => {
            let opts = DeleteOptions {
                reason: w.reason,
                closing_password: w.password,
            };
            t.delete(w.id, &opts, w.key).await?;
        }
    }
    Ok(())
}

/// A change waiting for the closing-date password (no `Debug`: it holds
/// the owner's reason text).
#[derive(Clone)]
pub(super) enum Pending {
    /// The entry row's save.
    Save,
    /// A void or delete of `id`, read at `version`.
    Row {
        kind: ActionKind,
        /// Where a void goes (the kind as read).
        route: VoidRoute,
        id: TransactionId,
        date: CalendarDate,
        version: Option<RecordVersion>,
        reason: Option<String>,
    },
}

/// The transaction a Void or Delete is about, as read for it.
pub(super) struct Target {
    kind: ActionKind,
    id: TransactionId,
    date: CalendarDate,
    read: Option<Task<Versioned<Transaction>>>,
    /// Its version and kind once read.
    record: Option<(RecordVersion, TxnType, TxnStatus)>,
    reason: String,
}

/// What the row-action panel shows.
pub(super) enum Panel {
    None,
    /// Reading the transaction, then asking to confirm.
    Confirm(Box<Target>),
    /// Delete can't be used for this one: offer Void.
    OfferVoid {
        id: TransactionId,
        date: CalendarDate,
        why: &'static str,
    },
    /// A plain message with OK.
    Notice(&'static str),
}

/// The register's closing-date flow and row actions.
pub(super) struct RowActions {
    pub(super) panel: Panel,
    /// The closing-date password prompt (shared with Write Check).
    pub(super) closing: ClosingPrompt<Pending>,
    /// A void or delete on its way.
    task: Option<(Pending, Task<()>)>,
    /// The last row action's failure.
    pub(super) error: Option<ApiError>,
    /// What Reload re-reads after a conflict.
    reload: Option<(ActionKind, TransactionId, CalendarDate)>,
    /// The last row action that went through.
    done: Option<&'static str>,
    /// The route of the last row action sent (P17 `WP2b`).
    route: Option<VoidRoute>,
    count: u32,
    /// Tests: `(closing-password-v1, txn-delete-v1)` instead of the link's.
    #[cfg(test)]
    pub(super) caps: Option<(bool, bool)>,
    /// Tests: the row under the cursor.
    #[cfg(test)]
    pub(super) cursor: Option<cl_grid::RowKey>,
}

impl Default for RowActions {
    fn default() -> RowActions {
        RowActions {
            panel: Panel::None,
            closing: ClosingPrompt::default(),
            task: None,
            error: None,
            reload: None,
            done: None,
            route: None,
            count: 0,
            #[cfg(test)]
            caps: None,
            #[cfg(test)]
            cursor: None,
        }
    }
}

impl std::fmt::Debug for RowActions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowActions")
            .field("closing", &self.closing)
            .finish_non_exhaustive()
    }
}

fn with_password(p: Option<Secret>) -> PostingOptions {
    PostingOptions {
        closed_period: p.map(ClosedPeriodAuthorization::with_password),
    }
}

impl RegisterScreen {
    /// The engine takes a closing-date password (U-15). The demo plays an
    /// engine that does.
    pub(super) fn closing_supported(&self) -> bool {
        #[cfg(test)]
        if let Some((c, _)) = self.actions.caps {
            return c;
        }
        self.link.is_demo() || self.link.capabilities().has(Capabilities::CLOSING_PASSWORD)
    }

    /// The engine deletes transactions (R7).
    pub(super) fn delete_supported(&self) -> bool {
        #[cfg(test)]
        if let Some((_, d)) = self.actions.caps {
            return d;
        }
        self.link.is_demo() || self.link.capabilities().has(Capabilities::TXN_DELETE)
    }

    /// The register row under the cursor.
    fn cursor_row(&self) -> Option<super::view::RegRow> {
        #[cfg(test)]
        if let Some(key) = self.actions.cursor {
            return self.model.row_for_key(key).cloned();
        }
        let key = self.grid.core().cursor()?.row;
        self.model.row_for_key(key).cloned()
    }

    // ------------------------------------------------------------ closing

    /// Starts the password flow for `pending` (a locked change found here).
    pub(super) fn ask_closing(&mut self, pending: Pending) {
        let supported = self.closing_supported();
        self.actions
            .closing
            .ask(pending, supported, self.model.closing);
    }

    /// A locked row can't be changed with this engine: explain.
    pub(super) fn closing_unavailable(&mut self) {
        let supported = self.closing_supported();
        self.actions
            .closing
            .unavailable(supported, self.model.closing);
    }

    /// Feeds a failed change to the flow. True when the dialog took it
    /// (nothing else should show it).
    pub(super) fn closing_failed(&mut self, e: &ApiError, pending: Pending) -> bool {
        let supported = self.closing_supported();
        self.actions
            .closing
            .failed(e, pending, supported, self.model.closing)
    }

    /// The change the password was for went through.
    pub(super) fn closing_saved(&mut self) {
        self.actions.closing.saved();
    }

    /// The minutes left before another password may be tried, while the
    /// engine's attempt limit is in force.
    #[cfg(test)]
    pub(super) fn closing_wait_minutes(&self) -> Option<u64> {
        self.actions.closing.wait_minutes()
    }

    /// Change: send the pending change with the typed password.
    pub(super) fn submit_closing(&mut self) {
        let Some((pending, secret)) = self.actions.closing.submit() else {
            return;
        };
        match pending {
            Pending::Save => {
                self.record_with(Some(secret));
            }
            Pending::Row {
                kind,
                route,
                id,
                date,
                version,
                reason,
            } => self.send_row_action(kind, route, id, date, version, reason, Some(secret)),
        }
        // Nothing went out (the entry needs fixing, or no session): the
        // password is dropped and the dialog closes.
        if self.entry.task.is_none() && self.actions.task.is_none() {
            self.actions.closing.nothing_sent();
        }
    }

    /// Cancel (or OK on the "not available yet" message): nothing changes.
    pub(super) fn cancel_closing(&mut self) {
        self.actions.closing.cancel();
    }

    // -------------------------------------------------------- void, delete

    /// Void Transaction on the row under the cursor.
    pub(super) fn start_void(&mut self) -> bool {
        let Some(row) = self.cursor_row() else {
            self.actions.panel = Panel::Notice("register.row_action.pick_row");
            return true;
        };
        if row.void {
            self.actions.panel = Panel::Notice("register.void.already");
            return true;
        }
        self.open_confirm(ActionKind::Void, row.txn_id, row.date);
        true
    }

    /// Ctrl+D on the row under the cursor: delete, or the offer to void.
    pub(super) fn start_delete(&mut self) -> bool {
        let Some(row) = self.cursor_row() else {
            self.actions.panel = Panel::Notice("register.row_action.pick_row");
            return true;
        };
        let (id, date) = (row.txn_id, row.date);
        self.actions.panel = if let Some(why) = VoidRoute::of(row.txn_type).delete_refused() {
            // A bill, vendor credit or bill payment is never deleted here
            // (P17 WP2b): offer its own void.
            Panel::OfferVoid { id, date, why }
        } else if !self.delete_supported() {
            Panel::OfferVoid {
                id,
                date,
                why: "register.delete.unavailable",
            }
        } else if !row.txn_type.is_deletable() {
            Panel::OfferVoid {
                id,
                date,
                why: "register.delete.unsupported",
            }
        } else if row.cleared == ClearState::Reconciled {
            Panel::Notice("register.delete.reconciled")
        } else {
            self.open_confirm(ActionKind::Delete, id, date);
            return true;
        };
        true
    }

    /// Reads the transaction (its version and kind) and asks to confirm.
    pub(super) fn open_confirm(&mut self, kind: ActionKind, id: TransactionId, date: CalendarDate) {
        self.actions.error = None;
        self.actions.reload = None;
        let read = if let Some(mut demo) = self.link.demo_state() {
            let chart = demo
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            let contacts = demo.contacts_mut().clone();
            let got = demo.ledger.get(&chart, &contacts, id);
            drop(demo);
            got.map(Task::ready)
        } else {
            match (self.link.session(), self.group.as_ref()) {
                (Some(session), Some(group)) => group
                    .read(
                        &session,
                        move |s| async move { s.transactions().get(id).await },
                    )
                    .ok()
                    .map(Task::Pending),
                _ => None,
            }
        };
        self.actions.panel = Panel::Confirm(Box::new(Target {
            kind,
            id,
            date,
            read,
            record: None,
            reason: String::new(),
        }));
        self.poll_actions();
    }

    /// Takes finished reads and writes.
    pub(super) fn poll_actions(&mut self) {
        if let Panel::Confirm(target) = &mut self.actions.panel
            && let Some(task) = target.read.as_mut()
            && let Some(result) = task.try_take()
        {
            target.read = None;
            let kind = target.kind;
            match result {
                Ok(v) => {
                    let r = &v.record;
                    target.date = r.txn_date;
                    target.record = Some((v.version, r.txn_type, r.status));
                    let (id, date) = (target.id, target.date);
                    let ap_delete = VoidRoute::of(r.txn_type).delete_refused();
                    if r.status == TxnStatus::Void && kind == ActionKind::Void {
                        self.actions.panel = Panel::Notice("register.void.already");
                    } else if let Some(why) = ap_delete.filter(|_| kind == ActionKind::Delete) {
                        // The kind as read decides (P17 WP2b).
                        self.actions.panel = Panel::OfferVoid { id, date, why };
                    } else if kind == ActionKind::Delete && !r.txn_type.is_deletable() {
                        self.actions.panel = Panel::OfferVoid {
                            id,
                            date,
                            why: "register.delete.unsupported",
                        };
                    }
                }
                Err(e) => {
                    self.actions.panel = Panel::None;
                    self.actions.error = Some(e);
                }
            }
        }
        if let Some((_, task)) = self.actions.task.as_mut()
            && let Some(result) = task.try_take()
        {
            let Some((pending, _)) = self.actions.task.take() else {
                return;
            };
            match result {
                Ok(()) => self.row_action_done(&pending),
                Err(e) => self.row_action_failed(&pending, e),
            }
        }
    }

    /// The owner confirmed: send it (the password flow first when the row
    /// is in the closed period).
    pub(super) fn confirm(&mut self) {
        let Panel::Confirm(target) = &self.actions.panel else {
            return;
        };
        let Some((version, txn_type, _)) = target.record else {
            return;
        };
        let reason = Some(target.reason.trim().to_string()).filter(|r| !r.is_empty());
        let (kind, id, date) = (target.kind, target.id, target.date);
        self.actions.panel = Panel::None;
        // The kind as read picks the route (P17 WP2b).
        let route = VoidRoute::of(txn_type);
        self.send_row_action(kind, route, id, date, Some(version), reason, None);
    }

    #[allow(clippy::too_many_arguments, reason = "one row action, unpacked")]
    fn send_row_action(
        &mut self,
        kind: ActionKind,
        route: VoidRoute,
        id: TransactionId,
        date: CalendarDate,
        version: Option<RecordVersion>,
        reason: Option<String>,
        password: Option<Secret>,
    ) {
        let pending = Pending::Row {
            kind,
            route,
            id,
            date,
            version,
            reason: reason.clone(),
        };
        if route.is_ap() {
            // P17 WP2b: never the generic routes for an A/P document.
            if let Some(why) = route
                .delete_refused()
                .filter(|_| kind == ActionKind::Delete)
            {
                self.actions.panel = Panel::Notice(why);
                return;
            }
            // Their own routes take no closing-date password yet.
            if password.is_some() || is_locked(date, self.model.closing) {
                self.actions.panel = Panel::Notice("register.ap.closed");
                return;
            }
        }
        if password.is_none() && is_locked(date, self.model.closing) {
            if self.closing_supported() {
                self.ask_closing(pending);
            } else {
                self.closing_unavailable();
            }
            return;
        }
        self.actions.error = None;
        self.actions.route = Some(route);
        if self.link.is_demo() {
            let result = self.demo_row_action(kind, id, date, password.as_ref());
            match result {
                Ok(()) => self.row_action_done(&pending),
                Err(e) => self.row_action_failed(&pending, e),
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: match kind {
                ActionKind::Void => "orphan.register_void",
                ActionKind::Delete => "orphan.register_delete",
            },
            key: Some(key.clone()),
        };
        let void_reason = reason
            .clone()
            .unwrap_or_else(|| t("register.void.default_reason"));
        let w = RowWrite {
            kind,
            route,
            id,
            version,
            reason,
            void_reason,
            password,
            key,
        };
        let written = group.write(&session, label, move |s| row_write(s, w));
        match written {
            Ok(p) => self.actions.task = Some((pending, Task::Pending(p))),
            Err(_) => self.row_action_failed(
                &pending,
                ApiError::Local(cl_api::LocalValidation::InvalidValue),
            ),
        }
    }

    fn demo_row_action(
        &self,
        kind: ActionKind,
        id: TransactionId,
        date: CalendarDate,
        password: Option<&Secret>,
    ) -> Result<(), ApiError> {
        crate::company::demo::register::check_closing(self.model.closing, &[date], password)?;
        let mut demo = self
            .link
            .demo_state()
            .ok_or(ApiError::Local(cl_api::LocalValidation::InvalidValue))?;
        match kind {
            ActionKind::Void => demo.ledger.void(id),
            ActionKind::Delete => demo.ledger.delete(id),
        }
    }

    fn row_action_done(&mut self, pending: &Pending) {
        self.closing_saved();
        if let Pending::Row { kind, .. } = pending {
            self.actions.done = Some(kind.name());
        }
        self.actions.count += 1;
        self.actions.error = None;
        self.actions.reload = None;
        self.after_own_write();
    }

    pub(super) fn row_action_failed(&mut self, pending: &Pending, e: ApiError) {
        // P17 WP2b: an A/P document's own route refused it. Its closed
        // period has no password path yet, and the engine's refusal (payments
        // or applications exist, a finished reconciliation) is said plainly.
        if let Pending::Row { route, .. } = pending
            && route.is_ap()
        {
            let notice = match &e {
                ApiError::ClosedPeriod(_) => Some("register.ap.closed"),
                ApiError::Validation(_) => route.refused(),
                _ => None,
            };
            if let Some(key) = notice {
                self.actions.error = None;
                self.actions.reload = None;
                self.actions.panel = Panel::Notice(key);
                return;
            }
        }
        if self.closing_failed(&e, pending.clone()) {
            return;
        }
        if let Pending::Row { kind, id, date, .. } = pending {
            self.actions.reload = matches!(
                e,
                ApiError::Conflict {
                    reason: ConflictReason::Stale,
                    ..
                }
            )
            .then_some((*kind, *id, *date));
        }
        if matches!(
            e,
            ApiError::OutcomeUnknown { .. } | ApiError::NotFound { .. }
        ) {
            // Show what the books have now.
            self.ledger_changed();
        }
        self.actions.error = Some(e);
    }

    /// Reload after a conflict: read the row again and ask again.
    pub(super) fn reload_row_action(&mut self) {
        let Some((kind, id, date)) = self.actions.reload.take() else {
            return;
        };
        self.ledger_changed();
        self.open_confirm(kind, id, date);
    }

    /// Esc closes the dialog or panel; `None` when neither is up.
    pub(super) fn actions_command(&mut self, id: CommandId) -> Option<bool> {
        if self.actions.closing.dialog_open() {
            return self.actions.closing.command(id);
        }
        if !matches!(self.actions.panel, Panel::None) && id.as_str() == "form.revert" {
            self.actions.panel = Panel::None;
            return Some(true);
        }
        None
    }

    // ------------------------------------------------------------- drawing

    /// The closing-date dialog and the row-action panel (above the grid).
    pub(super) fn actions_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        match self.actions.closing.ui(ui, tokens) {
            ClosingClick::Submit => self.submit_closing(),
            ClosingClick::Cancel => self.cancel_closing(),
            ClosingClick::None => {}
        }
        self.panel_ui(ui, tokens);
        if let Some(e) = &self.actions.error {
            form::inline_error(ui, tokens, e);
            if self.actions.reload.is_some() {
                let mut reload = false;
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(t("register.edit.changed_elsewhere")).color(tokens.warn_text),
                    );
                    reload = ui.button(t("register.edit.reload")).clicked();
                });
                if reload {
                    self.reload_row_action();
                }
            }
        }
        if let Some(done) = self.actions.done
            && self.actions.error.is_none()
            && matches!(self.actions.panel, Panel::None)
            && !self.actions.closing.dialog_open()
        {
            let key = if done == "void" {
                "register.void.done"
            } else {
                "register.delete.done"
            };
            ui.add(
                egui::Label::new(
                    RichText::new(t(key))
                        .size(12.0)
                        .color(tokens.text_secondary),
                )
                .selectable(false),
            );
        }
        if self.actions.task.is_some() {
            form::progress_line(ui, tokens, &t("register.row_action.working"));
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn panel_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        enum Click {
            None,
            Close,
            Confirm,
            Void(TransactionId, CalendarDate),
        }
        let mut click = Click::None;
        match &mut self.actions.panel {
            Panel::None => return,
            Panel::Notice(key) => {
                let key: &str = key;
                form::card(ui, tokens, |ui| {
                    ui.add(egui::Label::new(t(key)).wrap());
                    if ui.button(t("register.row_action.ok")).clicked() {
                        click = Click::Close;
                    }
                });
            }
            Panel::OfferVoid { id, date, why } => {
                let (id, date, why): (TransactionId, CalendarDate, &str) = (*id, *date, why);
                form::card(ui, tokens, |ui| {
                    ui.add(egui::Label::new(t(why)).wrap());
                    ui.horizontal(|ui| {
                        if ui.button(t("register.void.open")).clicked() {
                            click = Click::Void(id, date);
                        }
                        if ui.button(t("register.row_action.cancel")).clicked() {
                            click = Click::Close;
                        }
                    });
                });
            }
            Panel::Confirm(target) => {
                let (title, body, reason, confirm) = match target.kind {
                    ActionKind::Void => (
                        "register.void.title",
                        "register.void.body",
                        "register.void.reason",
                        "register.void.confirm",
                    ),
                    ActionKind::Delete => (
                        "register.delete.title",
                        "register.delete.body",
                        "register.delete.reason",
                        "register.delete.confirm",
                    ),
                };
                let ready = target.record.is_some();
                // A bill payment's void also reopens the bills it paid.
                let body = match target.record {
                    Some((_, TxnType::BillPayment, _)) if target.kind == ActionKind::Void => {
                        "register.ap.void_payment_body"
                    }
                    _ => body,
                };
                form::card(ui, tokens, |ui| {
                    form::heading(ui, tokens, &t(title));
                    ui.add(egui::Label::new(t(body)).wrap());
                    if !ready {
                        form::progress_line(ui, tokens, &t("register.row_action.reading"));
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(50));
                        return;
                    }
                    form::field(ui, &t(reason), &mut target.reason);
                    ui.horizontal(|ui| {
                        if ui.button(t(confirm)).clicked() {
                            click = Click::Confirm;
                        }
                        if ui.button(t("register.row_action.cancel")).clicked() {
                            click = Click::Close;
                        }
                    });
                });
            }
        }
        match click {
            Click::None => {}
            Click::Close => self.actions.panel = Panel::None,
            Click::Confirm => self.confirm(),
            Click::Void(id, date) => self.open_confirm(ActionKind::Void, id, date),
        }
    }

    /// The control-channel state (flags only; never the password, reasons,
    /// names or amounts).
    pub(super) fn actions_state(&self) -> serde_json::Value {
        let a = &self.actions;
        let panel = match &a.panel {
            Panel::None => "none",
            Panel::Confirm(t) if t.kind == ActionKind::Void => "confirm_void",
            Panel::Confirm(_) => "confirm_delete",
            Panel::OfferVoid { .. } => "offer_void",
            Panel::Notice(_) => "notice",
        };
        let notice = match &a.panel {
            Panel::Notice(k) | Panel::OfferVoid { why: k, .. } => Some(*k),
            _ => None,
        };
        serde_json::json!({
            "panel": panel,
            "notice": notice,
            "closing": format!("{:?}", a.closing.stage()).to_lowercase(),
            "closing_attempt": a.closing.attempt(),
            "closing_wait_minutes": a.closing.wait_minutes(),
            "closing_supported": self.closing_supported(),
            "delete_supported": self.delete_supported(),
            "pending": a.closing.pending().map(|p| match p {
                Pending::Save => "save",
                Pending::Row { kind, .. } => kind.name(),
            }),
            "working": a.task.is_some(),
            "error": a.error.is_some(),
            "stale": a.reload.is_some(),
            "last": a.done,
            "route": a.route.map(VoidRoute::name),
            "count": a.count,
        })
    }
}
