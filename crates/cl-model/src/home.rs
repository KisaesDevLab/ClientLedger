//! Home's model (P16, D10): the four task lanes, the Needs attention kinds,
//! the backup reminder rule (Q-217) and the setup checklist (Q-63).
//!
//! Nothing here holds an amount: lane footers name the figure the screen
//! shows (an engine string, or a dash), and attention items are counts.
//! Labels, shortcut chips and "arrives in Pxx" come from
//! [`crate::commands::find`] at draw time, so the registry stays the one
//! source.

use cl_core::Timestamp;

/// A lane on Home.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LaneId {
    /// Money in: invoices, payments, sales receipts, deposits.
    MoneyIn,
    /// Money out: bills, checks, card charges.
    MoneyOut,
    /// Banking: the bank feed, registers, reconcile, transfers.
    Banking,
    /// Close and report.
    CloseReport,
}

impl LaneId {
    /// Stable id (`state()`, AccessKit ids).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            LaneId::MoneyIn => "money_in",
            LaneId::MoneyOut => "money_out",
            LaneId::Banking => "banking",
            LaneId::CloseReport => "close_report",
        }
    }
}

/// The figure at the foot of a lane (an engine figure, or a dash while the
/// engine cannot report it exactly).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LaneFooter {
    /// What clients owe (U-76 receivables).
    ClientsOwe,
    /// Bills due within the company's window (Q-169, U-76 payables).
    BillsDue,
    /// The latest finished reconciliation (account and statement date).
    LastReconciled,
    /// Sales tax owed (the sales tax report, P28).
    SalesTaxOwed,
}

/// One lane: a title and four command ids, in the mockup's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lane {
    /// Which lane.
    pub id: LaneId,
    /// English title (the UI passes it through i18n).
    pub title: &'static str,
    /// Command ids of the actions, top to bottom.
    pub actions: [&'static str; 4],
    /// The figure at the foot.
    pub footer: LaneFooter,
}

/// The four lanes (D10, `docs/design/mockups/Home.html`).
pub const LANES: [Lane; 4] = [
    Lane {
        id: LaneId::MoneyIn,
        title: "Money in",
        actions: [
            "clients.create_invoice",
            "clients.receive_payment",
            "clients.sales_receipt",
            "banking.deposit",
        ],
        footer: LaneFooter::ClientsOwe,
    },
    Lane {
        id: LaneId::MoneyOut,
        title: "Money out",
        actions: [
            "vendors.enter_bill",
            "vendors.pay_bills",
            "vendors.write_check",
            "vendors.card_charge",
        ],
        footer: LaneFooter::BillsDue,
    },
    Lane {
        id: LaneId::Banking,
        title: "Banking",
        actions: [
            "banking.feed",
            "banking.register",
            "banking.reconcile",
            "banking.transfer",
        ],
        footer: LaneFooter::LastReconciled,
    },
    Lane {
        id: LaneId::CloseReport,
        title: "Close & report",
        actions: [
            "company.journal_entry",
            "reports.profit_loss",
            "company.send_to_accountant",
            "company.closing_date",
        ],
        footer: LaneFooter::SalesTaxOwed,
    },
];

/// A kind of Needs attention line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AttentionKind {
    /// Bank-feed items waiting.
    BankWaiting,
    /// A reconciliation started and not finished.
    ReconcileUnfinished,
    /// Invoices past due.
    InvoicesOverdue,
    /// Bills overdue or due within `days` days (Q-169).
    BillsDue {
        /// The company's window.
        days: u16,
    },
    /// Payroll imports not posted.
    PayrollUnposted,
    /// Open to-dos (Q-130; U-37).
    ToDos,
    /// Items at or below their reorder point (Q-149; U-78).
    Reorder,
    /// The weekly health check (Q-171; U-12, P34).
    HealthCheck,
    /// No backup for a week (Q-217).
    Backup,
}

impl AttentionKind {
    /// Every kind in display order, with the company's bills window.
    #[must_use]
    pub const fn all(bills_due_days: u16) -> [AttentionKind; 9] {
        [
            AttentionKind::BankWaiting,
            AttentionKind::ReconcileUnfinished,
            AttentionKind::InvoicesOverdue,
            AttentionKind::BillsDue {
                days: bills_due_days,
            },
            AttentionKind::PayrollUnposted,
            AttentionKind::ToDos,
            AttentionKind::Reorder,
            AttentionKind::HealthCheck,
            AttentionKind::Backup,
        ]
    }

    /// Stable id (`state()` keys).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            AttentionKind::BankWaiting => "bank_waiting",
            AttentionKind::ReconcileUnfinished => "reconcile_unfinished",
            AttentionKind::InvoicesOverdue => "invoices_overdue",
            AttentionKind::BillsDue { .. } => "bills_due",
            AttentionKind::PayrollUnposted => "payroll_unposted",
            AttentionKind::ToDos => "todos",
            AttentionKind::Reorder => "reorder",
            AttentionKind::HealthCheck => "health_check",
            AttentionKind::Backup => "backup",
        }
    }

    /// Whether this build can count it at all. To-dos (U-37), reorder
    /// alerts (U-78) and the health check (U-12) arrive later and share one
    /// muted line on Home.
    #[must_use]
    pub const fn supported(self) -> bool {
        !matches!(
            self,
            AttentionKind::ToDos | AttentionKind::Reorder | AttentionKind::HealthCheck
        )
    }

    /// The command its link opens (a placeholder window until its screen
    /// arrives), if it has one.
    #[must_use]
    pub const fn opens(self) -> Option<&'static str> {
        match self {
            AttentionKind::BankWaiting => Some("banking.feed"),
            AttentionKind::ReconcileUnfinished => Some("banking.reconcile"),
            AttentionKind::InvoicesOverdue => Some("reports.center"),
            AttentionKind::BillsDue { .. } => Some("vendors.pay_bills"),
            AttentionKind::PayrollUnposted => Some("payroll.import"),
            AttentionKind::Backup => Some("file.backup"),
            AttentionKind::ToDos | AttentionKind::Reorder | AttentionKind::HealthCheck => None,
        }
    }
}

/// One Needs attention line: a count, never an amount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttentionItem {
    /// What it is about.
    pub kind: AttentionKind,
    /// How many (`None` while unknown: still loading, failed, or the engine
    /// cannot count it). The backup reminder counts 1 when due.
    pub count: Option<u64>,
    /// Whether the count can be read on this engine and build.
    pub available: bool,
}

impl AttentionItem {
    /// A counted line.
    #[must_use]
    pub const fn counted(kind: AttentionKind, count: u64) -> AttentionItem {
        AttentionItem {
            kind,
            count: Some(count),
            available: kind.supported(),
        }
    }

    /// A line whose count is not known.
    #[must_use]
    pub const fn unknown(kind: AttentionKind, available: bool) -> AttentionItem {
        AttentionItem {
            kind,
            count: None,
            available: available && kind.supported(),
        }
    }

    /// The backup reminder: 1 when a backup is due, else 0.
    #[must_use]
    pub const fn backup(due: bool) -> AttentionItem {
        AttentionItem::counted(AttentionKind::Backup, if due { 1 } else { 0 })
    }

    /// Whether the line asks for the owner (available and above zero).
    #[must_use]
    pub fn needs_attention(&self) -> bool {
        self.available && self.count.is_some_and(|c| c > 0)
    }
}

/// The backup reminder window (Q-217).
pub const BACKUP_DAYS_DEFAULT: u16 = 7;

/// Whether a backup is due: never backed up, or the last one is `days`
/// days old or older. A last backup in the future (a clock change) is not
/// due.
#[must_use]
pub fn backup_due(last: Option<Timestamp>, now: Timestamp, days: u16) -> bool {
    let Some(last) = last else {
        return true;
    };
    let window = i64::from(days) * 86_400_000;
    now.unix_millis().saturating_sub(last.unix_millis()) >= window
}

/// One step of the setup checklist (Q-63).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChecklistItem {
    /// Company name and address.
    CompanyInfo,
    /// A bank account in the chart of accounts.
    BankAccount,
    /// Opening balances entered.
    OpeningBalances,
    /// A first transaction.
    FirstTransaction,
    /// A first bank file imported.
    BankFile,
    /// Email for invoices set up.
    Email,
    /// A first backup.
    FirstBackup,
}

impl ChecklistItem {
    /// Every step, in order.
    pub const ALL: [ChecklistItem; 7] = [
        ChecklistItem::CompanyInfo,
        ChecklistItem::BankAccount,
        ChecklistItem::OpeningBalances,
        ChecklistItem::FirstTransaction,
        ChecklistItem::BankFile,
        ChecklistItem::Email,
        ChecklistItem::FirstBackup,
    ];

    /// Stable id (`state()`, AccessKit ids).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            ChecklistItem::CompanyInfo => "company_info",
            ChecklistItem::BankAccount => "bank_account",
            ChecklistItem::OpeningBalances => "opening_balances",
            ChecklistItem::FirstTransaction => "first_transaction",
            ChecklistItem::BankFile => "bank_file",
            ChecklistItem::Email => "email",
            ChecklistItem::FirstBackup => "first_backup",
        }
    }

    /// The command its link opens.
    #[must_use]
    pub const fn opens(self) -> &'static str {
        match self {
            ChecklistItem::CompanyInfo | ChecklistItem::Email => "edit.preferences",
            ChecklistItem::BankAccount | ChecklistItem::OpeningBalances => {
                "lists.chart_of_accounts"
            }
            ChecklistItem::FirstTransaction => "vendors.write_check",
            ChecklistItem::BankFile => "banking.import_file",
            ChecklistItem::FirstBackup => "file.backup",
        }
    }
}

/// The setup checklist: its steps and which are done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checklist {
    items: Vec<(ChecklistItem, bool)>,
}

impl Default for Checklist {
    fn default() -> Self {
        Checklist::new(&ChecklistItem::ALL)
    }
}

impl Checklist {
    /// A checklist of `items` (duplicates dropped, order kept), none done.
    /// Home drops `OpeningBalances` when the engine has no signal for it
    /// (Q-P16j).
    #[must_use]
    pub fn new(items: &[ChecklistItem]) -> Checklist {
        let mut out: Vec<(ChecklistItem, bool)> = Vec::with_capacity(items.len());
        for &i in items {
            if !out.iter().any(|(x, _)| *x == i) {
                out.push((i, false));
            }
        }
        Checklist { items: out }
    }

    /// Ticks or clears a step (a step not on the list is ignored).
    pub fn set(&mut self, item: ChecklistItem, done: bool) {
        if let Some(slot) = self.items.iter_mut().find(|(i, _)| *i == item) {
            slot.1 = done;
        }
    }

    /// Whether `item` is on the list and done.
    #[must_use]
    pub fn is_done(&self, item: ChecklistItem) -> bool {
        self.items.iter().any(|(i, d)| *i == item && *d)
    }

    /// The steps with their state, in order.
    pub fn items(&self) -> impl Iterator<Item = (ChecklistItem, bool)> + '_ {
        self.items.iter().copied()
    }

    /// Steps done.
    #[must_use]
    pub fn done_count(&self) -> usize {
        self.items.iter().filter(|(_, d)| *d).count()
    }

    /// Steps in all.
    #[must_use]
    pub fn total(&self) -> usize {
        self.items.len()
    }

    /// The first step not done ("Set up email for invoices").
    #[must_use]
    pub fn first_open(&self) -> Option<ChecklistItem> {
        self.items.iter().find(|(_, d)| !*d).map(|(i, _)| *i)
    }

    /// Every step done.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.first_open().is_none()
    }

    /// Whether Home shows it: not dismissed and not complete.
    #[must_use]
    pub fn visible(&self, dismissed: bool) -> bool {
        !dismissed && !self.complete()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{COMMANDS, find};

    #[test]
    fn every_lane_action_is_a_registered_window() {
        let mut seen = std::collections::HashSet::new();
        for lane in &LANES {
            assert!(seen.insert(lane.id.key()), "lane ids are unique");
            for id in lane.actions {
                let c = find(id).unwrap_or_else(|| panic!("{id} is not in COMMANDS"));
                assert!(c.opens, "{id} opens a window (a screen or a placeholder)");
                // Built now, or a placeholder that says when it arrives.
                assert!(c.arrives.is_none_or(|p| p.starts_with('P')), "{id}");
            }
        }
        assert_eq!(
            LANES.map(|l| l.id.key()),
            ["money_in", "money_out", "banking", "close_report"]
        );
        // The mockup's shortcut chips come from the registry.
        assert!(
            find("vendors.write_check")
                .is_some_and(|c| c.shortcut.is_some() && c.arrives.is_none())
        );
        assert!(find("clients.create_invoice").is_some_and(|c| c.arrives == Some("P20")));
        assert!(COMMANDS.len() > 16);
    }

    #[test]
    fn attention_links_and_keys() {
        let all = AttentionKind::all(14);
        let mut keys = std::collections::HashSet::new();
        for k in all {
            assert!(keys.insert(k.key()), "{} once", k.key());
            if let Some(id) = k.opens() {
                assert!(find(id).is_some_and(|c| c.opens), "{id}");
            }
            assert_eq!(k.opens().is_none(), !k.supported(), "{k:?}");
        }
        assert_eq!(all[3], AttentionKind::BillsDue { days: 14 });
        assert!(!AttentionKind::ToDos.supported());
        assert!(!AttentionKind::Reorder.supported());
        assert!(!AttentionKind::HealthCheck.supported());

        let bank = AttentionItem::counted(AttentionKind::BankWaiting, 37);
        assert!(bank.needs_attention());
        assert!(!AttentionItem::counted(AttentionKind::BankWaiting, 0).needs_attention());
        let todo = AttentionItem::counted(AttentionKind::ToDos, 3);
        assert!(
            !todo.available && !todo.needs_attention(),
            "later items never ask"
        );
        let loading = AttentionItem::unknown(AttentionKind::InvoicesOverdue, true);
        assert!(loading.available && !loading.needs_attention());
        assert!(!AttentionItem::unknown(AttentionKind::Reorder, true).available);
        assert!(AttentionItem::backup(true).needs_attention());
        assert_eq!(AttentionItem::backup(false).count, Some(0));
    }

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(ms).expect("ts")
    }

    #[test]
    fn backup_is_due_after_seven_whole_days() {
        const H: i64 = 3_600_000;
        let now = at(1_793_000_000_000);
        assert!(
            backup_due(None, now, BACKUP_DAYS_DEFAULT),
            "never backed up"
        );
        let ago = |hours: i64| Some(at(now.unix_millis() - hours * H));
        assert!(!backup_due(ago(6 * 24 + 23), now, 7), "6 days 23 hours");
        assert!(backup_due(ago(7 * 24), now, 7), "exactly 7 days");
        assert!(backup_due(ago(30 * 24), now, 7));
        assert!(!backup_due(ago(0), now, 7), "just now");
        assert!(
            !backup_due(ago(-5), now, 7),
            "a backup in the future is not due"
        );
        assert!(backup_due(ago(0), now, 0), "a zero window is always due");
        assert!(
            !backup_due(ago(24 * 365), now, u16::MAX),
            "the widest window"
        );
    }

    #[test]
    fn checklist_counts_and_first_open() {
        let mut c = Checklist::default();
        assert_eq!((c.done_count(), c.total()), (0, 7));
        assert_eq!(c.first_open(), Some(ChecklistItem::CompanyInfo));
        for i in ChecklistItem::ALL {
            if i != ChecklistItem::Email {
                c.set(i, true);
            }
        }
        assert_eq!(c.done_count(), 6, "6 of 7 done");
        assert_eq!(c.first_open(), Some(ChecklistItem::Email));
        assert!(c.visible(false) && !c.visible(true), "dismissed hides it");
        c.set(ChecklistItem::Email, true);
        assert!(c.complete() && !c.visible(false), "done hides it");
        c.set(ChecklistItem::BankFile, false);
        assert!(!c.is_done(ChecklistItem::BankFile));
        assert_eq!(c.first_open(), Some(ChecklistItem::BankFile));

        // Without an opening-balance signal the list has six steps (Q-P16j).
        let six: Vec<_> = ChecklistItem::ALL
            .into_iter()
            .filter(|i| *i != ChecklistItem::OpeningBalances)
            .collect();
        let mut s = Checklist::new(&six);
        s.set(ChecklistItem::OpeningBalances, true);
        assert_eq!(
            (s.done_count(), s.total()),
            (0, 6),
            "an absent step is ignored"
        );
        assert!(!s.is_done(ChecklistItem::OpeningBalances));
        let dup = Checklist::new(&[ChecklistItem::Email, ChecklistItem::Email]);
        assert_eq!(dup.total(), 1);
        assert_eq!(
            dup.items().collect::<Vec<_>>(),
            [(ChecklistItem::Email, false)]
        );
        for i in ChecklistItem::ALL {
            assert!(find(i.opens()).is_some_and(|c| c.opens), "{}", i.key());
        }
        let keys: std::collections::HashSet<_> =
            ChecklistItem::ALL.iter().map(|i| i.key()).collect();
        assert_eq!(keys.len(), 7);
    }
}
