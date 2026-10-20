//! Go To (Ctrl+F, P12 WP7): find a line by number, payee, memo or exact
//! amount within the shown dates and filters, with Find Next and Find
//! Previous.
//!
//! The register only holds the pages on screen, so Go To asks the engine:
//!
//! - **Date sort.** The matches come from the shown query plus the search
//!   (`search=` text, or `min_amount = max_amount =` the typed amount),
//!   sorted by date away from the cursor. For the nearest match's date the
//!   engine's row count of the shown query up to the day before (or after,
//!   newest first) is that date's first row; the lines of that date are then
//!   read page by page to find the match's row.
//! - **Other sorts.** The shown query's pages are read from the cursor on,
//!   at most [`MAX_PAGES`] of them; then "Not found in the shown dates."
//!
//! Every match is checked here as well, on the line's own fields (an exact
//! amount compares `Money`; a display-only amount compares its text). No
//! amount is ever added up.

use std::collections::VecDeque;

use cl_api::{RegisterLine, RegisterPage, RegisterQuery, RegisterSort, SortDir};
use cl_core::ids::JournalLineId;
use cl_core::{CalendarDate, Money};
use cl_grid::core::viewport::RowAlign;
use cl_grid::{CellPos, GridHeight, ModelRef, NoLookups};
use cl_model::register::PAGE;
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::RegisterScreen;
use super::filters::{Choice, combo};
use super::view::COL_DATE;
use crate::company::Task;
use crate::i18n::t;
use crate::theme::Tokens;

/// Pages Go To reads before it gives up.
pub const MAX_PAGES: u32 = 50;

/// Rows per page Go To asks for.
const PER_PAGE: u16 = 200;

/// Where to look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GotoField {
    /// Number, payee or memo.
    #[default]
    Any,
    /// The number.
    Number,
    /// The payee.
    Payee,
    /// The memo.
    Memo,
    /// The payment or deposit, exactly.
    Amount,
}

impl GotoField {
    /// Every choice in the "Look in" list.
    pub const ALL: [GotoField; 5] = [
        GotoField::Any,
        GotoField::Number,
        GotoField::Payee,
        GotoField::Memo,
        GotoField::Amount,
    ];

    /// The i18n key of its label.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            GotoField::Any => "register.goto.field.any",
            GotoField::Number => "register.goto.field.number",
            GotoField::Payee => "register.goto.field.payee",
            GotoField::Memo => "register.goto.field.memo",
            GotoField::Amount => "register.goto.field.amount",
        }
    }

    /// The name the control channel reports.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            GotoField::Any => "any",
            GotoField::Number => "number",
            GotoField::Payee => "payee",
            GotoField::Memo => "memo",
            GotoField::Amount => "amount",
        }
    }
}

/// What is looked for. `Debug` shows neither the text nor the amount.
#[derive(Clone, PartialEq, Eq)]
pub enum Needle {
    /// Text in a field (lower case; matched case-insensitively).
    Text {
        /// Which field.
        field: GotoField,
        /// The text, lower case.
        lower: String,
    },
    /// An exact payment or deposit.
    Amount(Money),
}

impl std::fmt::Debug for Needle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Needle::Text { field, .. } => write!(f, "Text({field:?})"),
            Needle::Amount(_) => f.write_str("Amount(..)"),
        }
    }
}

impl Needle {
    /// The needle for typed text.
    ///
    /// # Errors
    /// The i18n key of the owner message (nothing typed, or an amount that
    /// isn't one).
    pub fn new(field: GotoField, typed: &str) -> Result<Needle, &'static str> {
        let typed = typed.trim();
        if typed.is_empty() {
            return Err("register.goto.problem.empty");
        }
        if field == GotoField::Amount {
            return match super::filters::parse_bound(typed)? {
                Some(m) => Ok(Needle::Amount(m)),
                None => Err("register.goto.problem.empty"),
            };
        }
        Ok(Needle::Text {
            field,
            lower: typed.to_lowercase(),
        })
    }

    /// True when `l` is a match.
    #[must_use]
    pub fn matches(&self, l: &RegisterLine) -> bool {
        match self {
            Needle::Text { field, lower } => {
                let has = |v: Option<&str>| v.is_some_and(|v| v.to_lowercase().contains(lower));
                let check = l.check_number.map(|n| n.to_string());
                let number = has(l.number.as_deref()) || has(check.as_deref());
                match field {
                    GotoField::Number => number,
                    GotoField::Payee => has(l.payee.as_deref()),
                    GotoField::Memo => has(l.memo.as_deref()),
                    GotoField::Any | GotoField::Amount => {
                        number || has(l.payee.as_deref()) || has(l.memo.as_deref())
                    }
                }
            }
            Needle::Amount(m) => {
                let want = format!("{:.2}", m.as_decimal());
                [l.payment.as_ref(), l.deposit.as_ref()]
                    .into_iter()
                    .flatten()
                    .any(|a| match a.exact() {
                        Some(v) => v == *m,
                        // Display-only text: compared as text, never parsed.
                        None => a.display(false).is_some_and(|t| t == want),
                    })
            }
        }
    }

    /// The shown query narrowed to the matches (`None` when nothing in the
    /// shown amount range can match).
    fn narrow(&self, main: &RegisterQuery) -> Option<RegisterQuery> {
        let mut q = main.clone();
        match self {
            Needle::Text { lower, .. } => q.search = Some(lower.clone()),
            Needle::Amount(m) => {
                if main.min.is_some_and(|lo| *m < lo) || main.max.is_some_and(|hi| *m > hi) {
                    return None;
                }
                q.min = Some(*m);
                q.max = Some(*m);
            }
        }
        Some(q)
    }
}

/// Find Next or Find Previous.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// Down the register.
    Next,
    /// Up the register.
    Previous,
}

/// What the search wants next.
#[derive(Debug)]
pub enum Action {
    /// Read this page (from 1) of this query.
    Fetch {
        /// The query.
        query: RegisterQuery,
        /// The page, from 1.
        page: u32,
        /// Rows per page.
        per_page: u16,
    },
    /// The match is the row at `index` of the shown query.
    Found {
        /// Row index in the shown register.
        index: u64,
        /// Its journal line.
        line: JournalLineId,
    },
    /// No match in the shown dates.
    NotFound,
}

#[derive(Debug, Clone, Copy)]
enum Phase {
    /// Reading page `page` of the matches.
    Matches { page: u32 },
    /// Counting the shown rows before `date`.
    Offset { date: CalendarDate },
    /// Reading page `page` of the shown query for the lines of `date`,
    /// from row `at` on.
    Locate {
        date: CalendarDate,
        page: u32,
        at: u64,
    },
    /// Reading page `page` of the shown query (sorts other than date).
    Scan { page: u32 },
}

/// One Find Next / Find Previous in progress.
#[derive(Debug)]
pub struct Search {
    needle: Needle,
    dir: Dir,
    /// The cursor's row index, if any.
    from: Option<u64>,
    main: RegisterQuery,
    matches: Option<RegisterQuery>,
    /// Matches waiting, by date in search order.
    groups: VecDeque<(CalendarDate, Vec<JournalLineId>)>,
    /// The next page of matches, when there is one.
    more_matches: Option<u32>,
    /// Rows found for the date being located.
    hits: Vec<(u64, JournalLineId)>,
    pages_read: u32,
    phase: Phase,
}

fn date_sorted(q: &RegisterQuery) -> bool {
    q.sort == RegisterSort::Date
}

impl Search {
    /// Starts a search of the shown query `main` from the cursor row
    /// (`from`, dated `from_date`); `total` is the shown row count.
    #[must_use]
    pub fn start(
        needle: Needle,
        dir: Dir,
        main: RegisterQuery,
        from: Option<u64>,
        from_date: Option<CalendarDate>,
        total: u64,
    ) -> (Search, Action) {
        let mut s = Search {
            needle,
            dir,
            from,
            main,
            matches: None,
            groups: VecDeque::new(),
            more_matches: None,
            hits: Vec::new(),
            pages_read: 0,
            phase: Phase::Scan { page: 1 },
        };
        if date_sorted(&s.main) {
            let Some(mut q) = s.needle.narrow(&s.main) else {
                return (s, Action::NotFound);
            };
            let forward = s.forward_in_date();
            if let Some(d) = from_date {
                if forward {
                    q.start = q.start.max(d);
                } else {
                    q.end = q.end.min(d);
                }
            }
            q.dir = if forward { SortDir::Asc } else { SortDir::Desc };
            if q.start > q.end {
                return (s, Action::NotFound);
            }
            s.matches = Some(q.clone());
            s.phase = Phase::Matches { page: 1 };
            let a = Action::Fetch {
                query: q,
                page: 1,
                per_page: PER_PAGE,
            };
            return (s, a);
        }
        let start = match (dir, from) {
            (Dir::Next, None) => Some(0),
            (Dir::Next, Some(c)) => c.checked_add(1),
            (Dir::Previous, None) => total.checked_sub(1),
            (Dir::Previous, Some(c)) => c.checked_sub(1),
        };
        let Some(start) = start.filter(|s| *s < total) else {
            return (s, Action::NotFound);
        };
        let page = page_of(start);
        s.phase = Phase::Scan { page };
        let a = s.fetch_main(page);
        (s, a)
    }

    /// True when the search runs towards later dates.
    fn forward_in_date(&self) -> bool {
        (self.main.dir == SortDir::Asc) == (self.dir == Dir::Next)
    }

    fn fetch_main(&self, page: u32) -> Action {
        Action::Fetch {
            query: self.main.clone(),
            page,
            per_page: PER_PAGE,
        }
    }

    /// A page asked for has arrived.
    pub fn on_page(&mut self, page: &RegisterPage) -> Action {
        self.pages_read += 1;
        match self.phase {
            Phase::Matches { page: n } => {
                for l in page.lines.iter().filter(|l| self.needle.matches(l)) {
                    match self.groups.back_mut() {
                        Some((d, ids)) if *d == l.date => ids.push(l.line_id),
                        _ => self.groups.push_back((l.date, vec![l.line_id])),
                    }
                }
                let read = u64::from(n) * u64::from(PER_PAGE);
                self.more_matches = (read < page.total_rows).then_some(n + 1);
                self.next_group()
            }
            Phase::Offset { date } => self.locate(date, page.total_rows),
            Phase::Locate { date, page: n, at } => {
                let first = u64::from(n - 1) * u64::from(PER_PAGE);
                let ids = self
                    .groups
                    .front()
                    .map(|(_, ids)| ids.clone())
                    .unwrap_or_default();
                let mut past = false;
                for (i, l) in page.lines.iter().enumerate() {
                    let index = first + i as u64;
                    if index < at {
                        continue;
                    }
                    if l.date != date {
                        past = true;
                        break;
                    }
                    if ids.contains(&l.line_id) {
                        self.hits.push((index, l.line_id));
                    }
                }
                let more = first + (page.lines.len() as u64) < page.total_rows;
                if !past && more && self.hits.len() < ids.len() && self.pages_read < MAX_PAGES {
                    self.phase = Phase::Locate {
                        date,
                        page: n + 1,
                        at,
                    };
                    return self.fetch_main(n + 1);
                }
                if let Some((index, line)) = self.pick() {
                    return Action::Found { index, line };
                }
                self.groups.pop_front();
                self.hits.clear();
                self.next_group()
            }
            Phase::Scan { page: n } => {
                let first = u64::from(n - 1) * u64::from(PER_PAGE);
                let hit = |(i, l): (usize, &RegisterLine)| {
                    let index = first + i as u64;
                    let after = match (self.dir, self.from) {
                        (_, None) => true,
                        (Dir::Next, Some(c)) => index > c,
                        (Dir::Previous, Some(c)) => index < c,
                    };
                    (after && self.needle.matches(l)).then_some(Action::Found {
                        index,
                        line: l.line_id,
                    })
                };
                let found = match self.dir {
                    Dir::Next => page.lines.iter().enumerate().find_map(hit),
                    Dir::Previous => page.lines.iter().enumerate().rev().find_map(hit),
                };
                if let Some(a) = found {
                    return a;
                }
                if self.pages_read >= MAX_PAGES {
                    return Action::NotFound;
                }
                let next = match self.dir {
                    Dir::Next => {
                        let end = first + page.lines.len() as u64;
                        (end < page.total_rows && !page.lines.is_empty()).then_some(n + 1)
                    }
                    Dir::Previous => n.checked_sub(1).filter(|p| *p >= 1),
                };
                match next {
                    Some(p) => {
                        self.phase = Phase::Scan { page: p };
                        self.fetch_main(p)
                    }
                    None => Action::NotFound,
                }
            }
        }
    }

    /// The nearest located row in the search direction.
    fn pick(&self) -> Option<(u64, JournalLineId)> {
        let ok = |i: u64| match (self.dir, self.from) {
            (_, None) => true,
            (Dir::Next, Some(c)) => i > c,
            (Dir::Previous, Some(c)) => i < c,
        };
        let it = self.hits.iter().copied().filter(|(i, _)| ok(*i));
        match self.dir {
            Dir::Next => it.min_by_key(|(i, _)| *i),
            Dir::Previous => it.max_by_key(|(i, _)| *i),
        }
    }

    /// Locates the next date of matches, or reads more matches.
    fn next_group(&mut self) -> Action {
        if let Some(date) = self.groups.front().map(|(d, _)| *d) {
            // The shown rows that come before `date` in the shown order.
            let mut count = self.main.clone();
            let before = if self.main.dir == SortDir::Asc {
                date.add_days(-1).ok().map(|d| {
                    count.end = d;
                    d >= count.start
                })
            } else {
                date.add_days(1).ok().map(|d| {
                    count.start = d;
                    d <= count.end
                })
            };
            if before != Some(true) {
                return self.locate(date, 0);
            }
            self.phase = Phase::Offset { date };
            return Action::Fetch {
                query: count,
                page: 1,
                per_page: 1,
            };
        }
        match (self.more_matches, &self.matches) {
            (Some(p), Some(q)) if self.pages_read < MAX_PAGES => {
                self.phase = Phase::Matches { page: p };
                Action::Fetch {
                    query: q.clone(),
                    page: p,
                    per_page: PER_PAGE,
                }
            }
            _ => Action::NotFound,
        }
    }

    fn locate(&mut self, date: CalendarDate, at: u64) -> Action {
        let page = page_of(at);
        self.phase = Phase::Locate { date, page, at };
        self.fetch_main(page)
    }
}

/// The page (from 1) holding row `index`.
fn page_of(index: u64) -> u32 {
    u32::try_from(index / PAGE + 1).unwrap_or(u32::MAX)
}

/// The Go To bar under the toolbar.
#[derive(Debug, Default)]
pub struct GotoBar {
    /// Shown.
    pub open: bool,
    /// Where to look.
    pub field: GotoField,
    /// What to look for, as typed (owner data).
    pub text: String,
    /// Put the keyboard in the text field next frame.
    focus: bool,
    /// The status line (i18n key): not found, a problem, or searching.
    pub status: Option<&'static str>,
    /// Matches found so far in this window.
    pub found: u32,
    /// How the last search ended: `found` or `not_found`.
    pub last: Option<&'static str>,
    search: Option<Search>,
    task: Option<Task<RegisterPage>>,
    /// The match to scroll to once its row is loaded.
    pub jump: Option<(u64, JournalLineId)>,
}

impl GotoBar {
    /// Stops a search in progress (the shown rows changed).
    pub fn cancel(&mut self) {
        if self.search.take().is_some() {
            self.status = None;
        }
        self.task = None;
        self.jump = None;
    }

    /// True while a search waits for the engine.
    #[must_use]
    pub fn searching(&self) -> bool {
        self.search.is_some()
    }

    /// What the control channel reports (never the typed text).
    #[must_use]
    pub fn state(&self) -> Value {
        json!({
            "open": self.open,
            "field": self.field.name(),
            "searching": self.searching(),
            "status": self.status.map(|k| k.rsplit('.').next().unwrap_or(k)),
            "found": self.found,
            "last": self.last,
            "jump_pending": self.jump.is_some(),
        })
    }
}

impl RegisterScreen {
    /// Ctrl+F: shows the bar with the keyboard in it.
    pub(crate) fn open_goto(&mut self) {
        self.goto.open = true;
        self.goto.focus = true;
    }

    /// Close: the bar goes away and forgets what was typed.
    pub(crate) fn close_goto(&mut self) {
        self.goto.cancel();
        self.goto = GotoBar {
            found: self.goto.found,
            field: self.goto.field,
            ..GotoBar::default()
        };
    }

    /// Find Next / Find Previous from the cursor row.
    pub(crate) fn find(&mut self, dir: Dir) {
        self.goto.cancel();
        self.goto.last = None;
        let needle = match Needle::new(self.goto.field, &self.goto.text) {
            Ok(n) => n,
            Err(p) => {
                self.goto.status = Some(p);
                return;
            }
        };
        let (Some(main), Some(total)) = (self.query.clone(), self.model.total()) else {
            return;
        };
        let cursor = self.grid.core().cursor().map(|c| c.row);
        let from = cursor.and_then(|k| cl_grid::RowModel::index_of(&self.model, k));
        let from_date = cursor
            .and_then(|k| self.model.row_for_key(k))
            .map(|r| r.date);
        let (search, action) =
            Search::start(needle, dir, main, from.map(|i| i as u64), from_date, total);
        self.goto.search = Some(search);
        self.goto.status = Some("register.goto.searching");
        self.goto_act(action);
        // The demo answers at once.
        self.drive_goto();
    }

    fn goto_act(&mut self, action: Action) {
        match action {
            Action::Fetch {
                query,
                page,
                per_page,
            } => {
                let task = self
                    .account
                    .and_then(|a| self.fetch_raw(a, &query, page, per_page));
                match task {
                    Some(t) => self.goto.task = Some(t),
                    None => self.goto_failed(),
                }
            }
            Action::Found { index, line } => {
                self.goto.search = None;
                self.goto.status = None;
                self.goto.found += 1;
                self.goto.last = Some("found");
                self.goto.jump = Some((index, line));
            }
            Action::NotFound => {
                self.goto.search = None;
                self.goto.last = Some("not_found");
                self.goto.status = Some("register.goto.not_found");
            }
        }
    }

    fn goto_failed(&mut self) {
        self.goto.search = None;
        self.goto.task = None;
        self.goto.status = Some("register.goto.failed");
    }

    /// Feeds arrived pages to the search (each frame).
    pub(super) fn drive_goto(&mut self) {
        // Bounded: a search reads at most MAX_PAGES pages and a few counts.
        for _ in 0..(MAX_PAGES + 8) {
            let Some(result) = self.goto.task.as_mut().and_then(Task::try_take) else {
                return;
            };
            self.goto.task = None;
            let Some(search) = self.goto.search.as_mut() else {
                return;
            };
            let Ok(page) = result else {
                self.goto_failed();
                return;
            };
            let action = search.on_page(&page);
            self.goto_act(action);
        }
    }

    /// Once the match's row is loaded, scroll to it and put the cursor on
    /// it (the keyboard goes to the register).
    pub(super) fn finish_goto(&mut self) {
        let Some((index, line)) = self.goto.jump else {
            return;
        };
        let Some(i) = usize::try_from(index).ok() else {
            self.goto.jump = None;
            return;
        };
        if self.model.total().is_some_and(|t| index >= t) {
            self.goto.jump = None;
            return;
        }
        let Some(key) = cl_grid::RowModel::row_key(&self.model, i) else {
            return;
        };
        // The row moved since the search (someone wrote meanwhile): go to
        // the line where it is now when it is loaded.
        let key = self
            .model
            .key_of(line)
            .filter(|k| cl_grid::RowModel::index_of(&self.model, *k).is_some())
            .unwrap_or(key);
        let label = String::new();
        let env = self
            .parts
            .env(&NoLookups, &label, GridHeight::Rows { min: 1, max: 1 });
        self.grid
            .scroll_to(key, RowAlign::Nearest, &ModelRef::Browse(&mut self.model));
        self.grid.focus(
            CellPos {
                row: key,
                col: COL_DATE,
            },
            ModelRef::Browse(&mut self.model),
            &env,
        );
        self.entry.focus = None;
        self.goto.jump = None;
    }

    /// The Go To bar.
    pub(super) fn goto_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if !self.goto.open {
            return;
        }
        let mut action: Option<Option<Dir>> = None;
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::Label::new(RichText::new(t("register.goto.title")).strong())
                    .selectable(false),
            );
            let choices: Vec<Choice> = GotoField::ALL
                .iter()
                .map(|f| Choice {
                    text: t(f.key()),
                    caption: false,
                })
                .collect();
            let current = GotoField::ALL
                .iter()
                .position(|f| *f == self.goto.field)
                .unwrap_or(0);
            if let Some(i) = combo(
                ui,
                "register/goto-field",
                &t("register.goto.look_in"),
                &choices,
                current,
            ) && let Some(f) = GotoField::ALL.get(i)
            {
                self.goto.field = *f;
            }
            let l = ui.add(egui::Label::new(t("register.goto.find")).selectable(false));
            let r = ui
                .add_sized(
                    [180.0, 22.0],
                    egui::TextEdit::singleline(&mut self.goto.text),
                )
                .labelled_by(l.id);
            if std::mem::take(&mut self.goto.focus) {
                r.request_focus();
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                action = Some(Some(Dir::Next));
            }
            if ui.button(t("register.goto.next")).clicked() {
                action = Some(Some(Dir::Next));
            }
            if ui.button(t("register.goto.previous")).clicked() {
                action = Some(Some(Dir::Previous));
            }
            if ui.button(t("register.goto.close")).clicked() {
                action = Some(None);
            }
            if let Some(s) = self.goto.status {
                let color = if s == "register.goto.searching" {
                    tokens.text_secondary
                } else {
                    tokens.warn_text
                };
                ui.label(RichText::new(t(s)).color(color));
            }
        });
        match action {
            Some(Some(dir)) => self.find(dir),
            Some(None) => self.close_goto(),
            None => {}
        }
        if self.goto.searching() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::company::demo::register::{DemoLedger, page};
    use cl_core::ids::AccountId;

    fn chart() -> Vec<cl_api::Account> {
        crate::company::demo::chart()
    }

    fn operating(chart: &[cl_api::Account]) -> AccountId {
        chart
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some("1010"))
            .map(|a| a.id)
            .expect("1010")
    }

    type Cache = std::collections::HashMap<String, RegisterPage>;

    /// A demo engine page, remembered (building the demo ledger is slow).
    fn cached(cache: &mut Cache, q: &RegisterQuery, n: u32, per: u16) -> RegisterPage {
        let key = format!(
            "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{n}|{per}",
            q.start,
            q.end,
            q.search,
            q.min.map(Money::to_wire),
            q.max.map(Money::to_wire),
            q.sort,
            q.dir
        );
        cache
            .entry(key)
            .or_insert_with(|| {
                let chart = chart();
                let acct = operating(&chart);
                page(&DemoLedger::default(), &chart, acct, q, n, per).expect("page")
            })
            .clone()
    }

    /// Runs a search against the demo engine; returns the answer and the
    /// requests made.
    fn run(
        cache: &mut Cache,
        needle: Needle,
        dir: Dir,
        main: &RegisterQuery,
        from: Option<u64>,
    ) -> (Option<(u64, JournalLineId)>, u32) {
        let total = cached(cache, main, 1, 1).total_rows;
        let from_date = from.map(|i| {
            let p = cached(cache, main, page_of(i), 200);
            p.lines[usize::try_from(i % 200).expect("i")].date
        });
        let (mut s, mut a) = Search::start(needle, dir, main.clone(), from, from_date, total);
        let mut calls = 0;
        loop {
            match a {
                Action::Fetch {
                    query,
                    page: n,
                    per_page,
                } => {
                    calls += 1;
                    let p = cached(cache, &query, n, per_page);
                    a = s.on_page(&p);
                }
                Action::Found { index, line } => return (Some((index, line)), calls),
                Action::NotFound => return (None, calls),
            }
        }
    }

    /// Every row of the shown query, in order.
    fn rows(main: &RegisterQuery) -> Vec<RegisterLine> {
        let chart = chart();
        let acct = operating(&chart);
        let ledger = DemoLedger::default();
        let mut out = Vec::new();
        for n in 1.. {
            let p = page(&ledger, &chart, acct, main, n, 200).expect("page");
            let done = p.lines.len() < 200;
            out.extend(p.lines);
            if done {
                break;
            }
        }
        out
    }

    /// The nearest match by brute force.
    fn brute(lines: &[RegisterLine], n: &Needle, dir: Dir, from: Option<u64>) -> Option<u64> {
        let idx =
            (0..lines.len() as u64).filter(|i| n.matches(&lines[usize::try_from(*i).expect("i")]));
        match (dir, from) {
            (Dir::Next, None) => idx.min(),
            (Dir::Next, Some(c)) => idx.filter(|i| *i > c).min(),
            (Dir::Previous, None) => idx.max(),
            (Dir::Previous, Some(c)) => idx.filter(|i| *i < c).max(),
        }
    }

    fn all_dates() -> RegisterQuery {
        RegisterQuery::new(CalendarDate::min(), CalendarDate::max())
    }

    #[test]
    fn find_next_and_previous_agree_with_a_full_scan_in_every_sort() {
        let mut cache = Cache::new();
        let mut queries = Vec::new();
        for (sort, dir) in [
            (RegisterSort::Date, SortDir::Asc),
            (RegisterSort::Date, SortDir::Desc),
            (RegisterSort::Amount, SortDir::Asc),
            (RegisterSort::Number, SortDir::Desc),
        ] {
            let mut q = all_dates();
            q.sort = sort;
            q.dir = dir;
            queries.push(q);
        }
        for main in &queries {
            let lines = rows(main);
            assert!(lines.len() > 300, "more than a page");
            // A payee, a number and an amount from the shown rows.
            let payee = lines.iter().find_map(|l| l.payee.clone()).expect("payee");
            let number = lines
                .iter()
                .find_map(|l| l.number.clone().filter(|n| n.len() > 2))
                .expect("number");
            let amount = lines
                .iter()
                .rev()
                .find_map(|l| l.payment.as_ref().and_then(cl_api::RegisterAmount::exact))
                .expect("amount");
            let needles = [
                Needle::new(GotoField::Payee, &payee).expect("n"),
                Needle::new(GotoField::Number, &number).expect("n"),
                Needle::new(GotoField::Any, &payee[..3]).expect("n"),
                Needle::Amount(amount),
            ];
            let n = lines.len() as u64;
            for needle in &needles {
                for dir in [Dir::Next, Dir::Previous] {
                    for from in [None, Some(0), Some(n / 2), Some(n - 1)] {
                        let want = brute(&lines, needle, dir, from);
                        let (got, calls) = run(&mut cache, needle.clone(), dir, main, from);
                        assert_eq!(
                            got.map(|(i, _)| i),
                            want,
                            "{needle:?} {dir:?} from {from:?} sort {:?}/{:?}",
                            main.sort,
                            main.dir
                        );
                        if let Some((i, line)) = got {
                            assert_eq!(lines[usize::try_from(i).expect("i")].line_id, line);
                        }
                        assert!(calls <= MAX_PAGES + 2, "{calls}");
                    }
                }
            }
        }
    }

    #[test]
    fn nothing_found_says_so() {
        let main = all_dates();
        let n = Needle::new(GotoField::Memo, "no such memo anywhere").expect("n");
        assert_eq!(run(&mut Cache::new(), n, Dir::Next, &main, None).0, None);
        // An amount outside the shown amount range cannot match.
        let mut filtered = all_dates();
        filtered.max = Some(Money::parse_wire("10").expect("m"));
        let a = Needle::Amount(Money::parse_wire("11").expect("m"));
        let (s, first) = Search::start(a, Dir::Next, filtered, None, None, 5);
        assert!(matches!(first, Action::NotFound));
        assert!(
            format!("{s:?}").contains("Amount(..)"),
            "no digits in Debug"
        );
    }

    #[test]
    fn typed_needles_are_checked() {
        assert_eq!(
            Needle::new(GotoField::Payee, "  "),
            Err("register.goto.problem.empty")
        );
        assert_eq!(
            Needle::new(GotoField::Amount, "abc"),
            Err("grid.amount.invalid")
        );
        assert_eq!(
            Needle::new(GotoField::Amount, "1,084.66"),
            Ok(Needle::Amount(Money::parse_wire("1084.66").expect("m")))
        );
        assert_eq!(
            Needle::new(GotoField::Memo, "Fuel"),
            Ok(Needle::Text {
                field: GotoField::Memo,
                lower: "fuel".into()
            })
        );
    }

    #[test]
    fn display_only_amounts_match_as_text() {
        let body = include_str!("../../../../../fixtures/engine/register-legacy.json");
        let page = cl_api::resources::register::page_from_json(body.as_bytes()).expect("page");
        let l = page
            .lines
            .iter()
            .find(|l| l.payment.as_ref().and_then(|a| a.display(false)).is_some())
            .expect("a payment");
        let text = l
            .payment
            .as_ref()
            .and_then(|a| a.display(false))
            .expect("t");
        let n = Needle::new(GotoField::Amount, &text).expect("n");
        assert!(n.matches(l));
        let other = Needle::Amount(Money::parse_wire("0.01").expect("m"));
        assert!(!other.matches(l) || text == "0.01");
    }
}
