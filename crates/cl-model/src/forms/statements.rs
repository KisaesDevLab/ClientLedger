//! Statements' form state (P22 D3): the statement date, layout, period
//! start, the batch filters (Q-176), the clients picked and the message,
//! and the run plan: the selection in runs of at most 200 clients, each
//! with its own save attempt (one idempotency key per run, reused on a
//! retry; P22 plan §5). No egui and no money: the amounts due, aging and
//! statement lines are the engine's.

use std::collections::BTreeSet;
use std::fmt;

use cl_core::CalendarDate;
use cl_core::ids::ContactId;

use crate::text::TextKey;

/// Clients per run the desktop sends (the engine takes up to 500).
pub const RUN_CHUNK: usize = 200;

/// The statement layouts (Q-175, Q-P22h).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum StatementFormatChoice {
    /// Each open item (the default).
    #[default]
    OpenItem,
    /// The opening balance and the period's activity.
    BalanceForward,
    /// The period's activity, the previous balance in the header.
    Transaction,
}

impl StatementFormatChoice {
    /// The engine's `format`.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            StatementFormatChoice::OpenItem => "open_item",
            StatementFormatChoice::BalanceForward => "balance_forward",
            StatementFormatChoice::Transaction => "transaction",
        }
    }

    /// The label's text key.
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            StatementFormatChoice::OpenItem => TextKey("statements.format.open_item"),
            StatementFormatChoice::BalanceForward => TextKey("statements.format.balance_forward"),
            StatementFormatChoice::Transaction => TextKey("statements.format.transaction"),
        }
    }
}

/// One run to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunChunk {
    /// Its place in the plan (0-based).
    pub index: usize,
    /// The save attempt this chunk belongs to: the UI keys it as
    /// (attempt, index), so a retry of the same chunk reuses its key.
    pub attempt: u64,
    /// The clients (1-200), in id order.
    pub clients: Vec<ContactId>,
}

/// The runs a Print, PDF or Email sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunPlan {
    /// The statement date.
    pub as_of: CalendarDate,
    /// The layout.
    pub format: StatementFormatChoice,
    /// The period start (`None` for open item).
    pub start: Option<CalendarDate>,
    /// The runs.
    pub chunks: Vec<RunChunk>,
}

/// Why statements can't be issued yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StatementsProblem {
    /// No statement date.
    NoDate,
    /// No client picked.
    NoClients,
    /// The period starts after the statement date.
    StartAfterAsOf,
}

impl StatementsProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            StatementsProblem::NoDate => TextKey("statements.problem.no_date"),
            StatementsProblem::NoClients => TextKey("statements.problem.no_clients"),
            StatementsProblem::StartAfterAsOf => TextKey("statements.problem.start_after_as_of"),
        }
    }
}

/// The Statements window's choices.
#[derive(Clone, PartialEq, Eq)]
pub struct StatementsForm {
    /// The statement date.
    pub as_of: Option<CalendarDate>,
    /// The layout (open item by default).
    pub format: StatementFormatChoice,
    /// A typed period start (`None`: the first of the as-of month).
    pub start: Option<CalendarDate>,
    /// Only clients with something at least this many days overdue.
    pub min_days_overdue: u32,
    /// Only clients with a balance (on by default, Q-176).
    pub open_only: bool,
    /// The message printed on every statement (owner data).
    pub message: String,
    selected: BTreeSet<ContactId>,
    attempt: u64,
}

impl Default for StatementsForm {
    fn default() -> Self {
        StatementsForm {
            as_of: None,
            format: StatementFormatChoice::OpenItem,
            start: None,
            min_days_overdue: 0,
            open_only: true,
            message: String::new(),
            selected: BTreeSet::new(),
            attempt: 0,
        }
    }
}

impl fmt::Debug for StatementsForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StatementsForm")
            .field("as_of", &self.as_of)
            .field("format", &self.format)
            .field("start", &self.start)
            .field("min_days_overdue", &self.min_days_overdue)
            .field("open_only", &self.open_only)
            .field("selected", &self.selected.len())
            .finish_non_exhaustive()
    }
}

impl StatementsForm {
    /// Statements as of `as_of`.
    #[must_use]
    pub fn new(as_of: Option<CalendarDate>) -> StatementsForm {
        StatementsForm {
            as_of,
            ..StatementsForm::default()
        }
    }

    /// The start date field shows (not for open item).
    #[must_use]
    pub fn shows_start(&self) -> bool {
        self.format != StatementFormatChoice::OpenItem
    }

    /// The period start sent: `None` for open item, else the typed date or
    /// the first of the as-of month.
    #[must_use]
    pub fn effective_start(&self) -> Option<CalendarDate> {
        if !self.shows_start() {
            return None;
        }
        self.start
            .or_else(|| self.as_of.map(CalendarDate::start_of_month))
    }

    /// Picks or unpicks a client.
    pub fn toggle(&mut self, client: ContactId) {
        if !self.selected.remove(&client) {
            self.selected.insert(client);
        }
        self.attempt += 1;
    }

    /// Picks exactly `clients` (Select all of the listed rows).
    pub fn select_only(&mut self, clients: &[ContactId]) {
        self.selected = clients.iter().copied().collect();
        self.attempt += 1;
    }

    /// True when `client` is picked.
    #[must_use]
    pub fn is_selected(&self, client: ContactId) -> bool {
        self.selected.contains(&client)
    }

    /// How many are picked.
    #[must_use]
    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }

    /// Marks a changed date, layout or start (a new attempt).
    pub fn touched(&mut self) {
        self.attempt += 1;
    }

    /// The runs to send: the picked clients in id order, at most
    /// [`RUN_CHUNK`] per run.
    ///
    /// # Errors
    /// Every [`StatementsProblem`] found.
    pub fn run_plan(&self) -> Result<RunPlan, Vec<StatementsProblem>> {
        let mut p = Vec::new();
        if self.as_of.is_none() {
            p.push(StatementsProblem::NoDate);
        }
        if self.selected.is_empty() {
            p.push(StatementsProblem::NoClients);
        }
        if let (Some(s), Some(a)) = (self.effective_start(), self.as_of)
            && s > a
        {
            p.push(StatementsProblem::StartAfterAsOf);
        }
        let Some(as_of) = self.as_of.filter(|_| p.is_empty()) else {
            return Err(p);
        };
        let all: Vec<ContactId> = self.selected.iter().copied().collect();
        Ok(RunPlan {
            as_of,
            format: self.format,
            start: self.effective_start(),
            chunks: all
                .chunks(RUN_CHUNK)
                .enumerate()
                .map(|(index, c)| RunChunk {
                    index,
                    attempt: self.attempt,
                    clients: c.to_vec(),
                })
                .collect(),
        })
    }
}

#[cfg(test)]
#[path = "statements_tests.rs"]
mod tests;
