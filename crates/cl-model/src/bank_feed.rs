//! Bank Feed review state (P26, DESIGN.md "Bank Feed"; no egui, no I/O).
//!
//! - [`Why`]: the badge a row carries (Rule, Match, Transfer, AI, Needs
//!   you, …) from what the engine reported, with no AI badge at all while
//!   AI is off or offline (P26 acceptance; Q-P26i: "AI · high" at a
//!   confidence of at least 0.80).
//! - [`ReviewList`]: the rows on screen, the cursor (J/K, Up/Down), the
//!   selection (Space, the check boxes) and what a key asks for
//!   ([`FeedAction`]). Rows that changed under the owner leave the
//!   selection ([`ReviewList::drop_rows`], plan §5 LAN).
//! - [`plan_accept`]: what "Accept selected" sends: ready rows with an
//!   account, matches with one candidate both ways (Q-P26q), never
//!   transfers (they post through their own route), sorted by id.
//! - [`SplitDraft`] and [`DepositDraft`]: the remainder and the deposit
//!   difference as the owner types, exact and display-only (the engine
//!   re-checks every total).
//! - [`RuleDirection`]: "Money out" is `amount_sign` 1 in the rule AST.
//!
//! Amounts are [`Money`] in the **feed sign** (positive = money out); the
//! screen shows −feed ([`display_amount`]).

use std::collections::BTreeSet;

use cl_core::Money;

/// A confidence of at least this many basis points is "AI · high" (0.80,
/// Q-P26i).
pub const AI_HIGH_BP: u16 = 8_000;
/// Rows per page and per bucket (plan §5 "Large data").
pub const PAGE: u16 = 500;

/// The amount the screen shows: money in positive (−feed).
#[must_use]
pub fn display_amount(feed: Money) -> Money {
    feed.checked_neg().unwrap_or(feed)
}

/// True when `m` has no more than 2 decimals (a split is possible).
#[must_use]
pub fn whole_cents(m: Money) -> bool {
    m.to_ten_thousandths() % 100 == 0
}

/// A waiting row's review bucket.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bucket {
    /// Ready to accept.
    Ready,
    /// Needs the owner.
    Review,
    /// Has a match signal.
    Matches,
}

/// The segment control over the waiting rows (Q-P26e: mockup wording).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Segment {
    /// Every waiting row.
    #[default]
    All,
    /// Ready.
    Ready,
    /// Needs review.
    Review,
    /// Matches.
    Matches,
}

impl Segment {
    /// The four segments, in mockup order.
    pub const ALL: [Segment; 4] = [
        Segment::All,
        Segment::Ready,
        Segment::Review,
        Segment::Matches,
    ];

    /// The bucket this segment lists (`None` for All).
    #[must_use]
    pub const fn bucket(self) -> Option<Bucket> {
        match self {
            Segment::All => None,
            Segment::Ready => Some(Bucket::Ready),
            Segment::Review => Some(Bucket::Review),
            Segment::Matches => Some(Bucket::Matches),
        }
    }

    /// The i18n key of its label.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Segment::All => "bank_feed.segment.all",
            Segment::Ready => "bank_feed.segment.ready",
            Segment::Review => "bank_feed.segment.review",
            Segment::Matches => "bank_feed.segment.matches",
        }
    }

    /// The control channel's word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Segment::All => "all",
            Segment::Ready => "ready",
            Segment::Review => "review",
            Segment::Matches => "matches",
        }
    }
}

/// Which rows the list shows (Q-P26h: the Show selector).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Show {
    /// Waiting for the owner (the segments apply).
    #[default]
    Waiting,
    /// In the books (the "Entry voided" chip applies).
    Accepted,
    /// Left out.
    Excluded,
}

impl Show {
    /// The three, in order.
    pub const ALL: [Show; 3] = [Show::Waiting, Show::Accepted, Show::Excluded];

    /// The i18n key of its label.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Show::Waiting => "bank_feed.show.waiting",
            Show::Accepted => "bank_feed.show.accepted",
            Show::Excluded => "bank_feed.show.excluded",
        }
    }

    /// The control channel's word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Show::Waiting => "waiting",
            Show::Accepted => "accepted",
            Show::Excluded => "excluded",
        }
    }
}

/// Why a row is proposed the way it is (the "Why" column badge).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Why {
    /// A rule staged it.
    Rule,
    /// It matches an entry already in the books.
    Match,
    /// It looks like one side of a transfer between own accounts.
    Transfer,
    /// AI suggested it; `high` at a confidence of at least 0.80.
    Ai {
        /// Confident.
        high: bool,
    },
    /// Past entries for the same payee suggested it (no AI).
    History,
    /// It may be a row already imported.
    Duplicate,
    /// The owner must choose.
    NeedsYou,
    /// The linked entry was voided in the register (Q-P26u).
    EntryVoided,
    /// The linked entry's amount or bank account was edited since it was
    /// accepted (Q-PR5a): out of matching until reviewed again.
    EntryChanged,
    /// A receipt in the review queue matches this bank line (A05, Q-A05a):
    /// offered in the detail panel, never applied by itself.
    Receipt,
    /// Already accepted (Show › Accepted; P26 QA-8).
    Accepted,
    /// Excluded (Show › Excluded).
    Excluded,
}

impl Why {
    /// The i18n key of the badge.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Why::Rule => "bank_feed.why.rule",
            Why::Match => "bank_feed.why.match",
            Why::Transfer => "bank_feed.why.transfer",
            Why::Ai { high: true } => "bank_feed.why.ai_high",
            Why::Ai { high: false } => "bank_feed.why.ai_check",
            Why::History => "bank_feed.why.history",
            Why::Duplicate => "bank_feed.why.duplicate",
            Why::NeedsYou => "bank_feed.why.needs_you",
            Why::EntryVoided => "bank_feed.why.entry_voided",
            Why::EntryChanged => "bank_feed.why.entry_changed",
            Why::Receipt => "bank_feed.why.receipt",
            Why::Accepted => "bank_feed.why.accepted",
            Why::Excluded => "bank_feed.why.excluded",
        }
    }

    /// The control channel's word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Why::Rule => "rule",
            Why::Match => "match",
            Why::Transfer => "transfer",
            Why::Ai { high: true } => "ai_high",
            Why::Ai { high: false } => "ai_check",
            Why::History => "history",
            Why::Duplicate => "duplicate",
            Why::NeedsYou => "needs_you",
            Why::EntryVoided => "entry_voided",
            Why::EntryChanged => "entry_changed",
            Why::Receipt => "receipt",
            Why::Accepted => "accepted",
            Why::Excluded => "excluded",
        }
    }

    /// An AI badge.
    #[must_use]
    pub const fn is_ai(self) -> bool {
        matches!(self, Why::Ai { .. })
    }
}

/// Where a suggestion came from, as the engine's `matchType` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A rule.
    Rule,
    /// AI.
    Ai,
    /// Past entries (history, exact or fuzzy name, a check image).
    History,
    /// The owner, on another screen.
    Manual,
    /// Nothing suggested.
    None,
}

impl Source {
    /// Reads the engine's `matchType` (unknown words are history).
    #[must_use]
    pub fn parse(match_type: Option<&str>) -> Source {
        match match_type {
            None => Source::None,
            Some("rule") => Source::Rule,
            Some("ai") => Source::Ai,
            Some("manual") => Source::Manual,
            Some(_) => Source::History,
        }
    }
}

/// What the badge is decided from.
#[allow(
    clippy::struct_excessive_bools,
    reason = "flags read straight from the engine's row"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WhyInput {
    /// The waiting row's bucket (`None` once accepted or excluded).
    pub bucket: Option<Bucket>,
    /// The suggestion's source.
    pub source: Source,
    /// A rule staged it (the engine's `suggestedRule`).
    pub has_rule: bool,
    /// Its confidence in basis points (0..=10 000).
    pub confidence_bp: Option<u16>,
    /// Flagged as a possible duplicate.
    pub possible_duplicate: bool,
    /// Candidate journal lines.
    pub match_candidates: u32,
    /// A transfer partner was found (or the row reads as a transfer).
    pub transfer: bool,
    /// An account is suggested or staged.
    pub has_account: bool,
    /// The linked entry was voided.
    pub entry_voided: bool,
    /// The linked entry was edited since it was accepted (Q-PR5a).
    pub entry_changed: bool,
    /// A receipt in the review queue matches it (A05).
    pub has_receipt: bool,
}

/// The badge for a row. `ai_on`: AI is set up and reachable; when it
/// isn't, no AI badge ever shows (an older AI suggestion reads as past
/// entries).
#[must_use]
pub fn why(i: WhyInput, ai_on: bool) -> Why {
    if i.entry_voided {
        return Why::EntryVoided;
    }
    if i.entry_changed {
        return Why::EntryChanged;
    }
    if i.possible_duplicate && i.bucket.is_some() {
        return Why::Duplicate;
    }
    if i.transfer {
        return Why::Transfer;
    }
    if i.bucket == Some(Bucket::Matches) || i.match_candidates > 0 {
        return Why::Match;
    }
    // After Match: an entry already in the books is matched (the receipt
    // goes with it), never posted again. Before "Needs you" (A05 QA-1): a
    // line with a receipt says so in the list even without an account (in
    // a new company that is every line); the panel asks for the account.
    if i.has_receipt {
        return Why::Receipt;
    }
    if !i.has_account {
        return Why::NeedsYou;
    }
    if i.has_rule || i.source == Source::Rule {
        return Why::Rule;
    }
    match i.source {
        Source::Ai if ai_on => Why::Ai {
            high: i.confidence_bp.is_some_and(|c| c >= AI_HIGH_BP),
        },
        Source::Ai | Source::History | Source::Manual => Why::History,
        Source::Rule => Why::Rule,
        Source::None => Why::NeedsYou,
    }
}

/// What a key or button on the list asks the screen to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedAction<Id> {
    /// Accept the row (A).
    Accept(Id),
    /// Open the split editor on the row (S).
    Split(Id),
    /// Make a rule from the row (R).
    Rule(Id),
    /// Accept the selected rows (Ctrl+Enter), in list order.
    AcceptSelected(Vec<Id>),
}

/// The rows on screen, the cursor and the selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewList<Id> {
    rows: Vec<Id>,
    cursor: Option<usize>,
    selected: BTreeSet<Id>,
    /// What the last key asked for (taken by the screen each frame).
    pub action: Option<FeedAction<Id>>,
}

impl<Id> Default for ReviewList<Id> {
    fn default() -> ReviewList<Id> {
        ReviewList {
            rows: Vec::new(),
            cursor: None,
            selected: BTreeSet::new(),
            action: None,
        }
    }
}

impl<Id: Copy + Ord> ReviewList<Id> {
    /// New rows (a reload). The cursor stays on its row when the row is
    /// still there, else on the same place; the selection keeps only rows
    /// still listed.
    pub fn set_rows(&mut self, rows: Vec<Id>) {
        let at = self.current();
        let place = self.cursor;
        self.selected.retain(|id| rows.contains(id));
        self.rows = rows;
        self.cursor = if self.rows.is_empty() {
            None
        } else if let Some(i) = at.and_then(|id| self.rows.iter().position(|r| *r == id)) {
            Some(i)
        } else {
            Some(place.unwrap_or(0).min(self.rows.len() - 1))
        };
    }

    /// The rows, in order.
    #[must_use]
    pub fn rows(&self) -> &[Id] {
        &self.rows
    }

    /// The cursor's row.
    #[must_use]
    pub fn current(&self) -> Option<Id> {
        self.cursor.and_then(|i| self.rows.get(i).copied())
    }

    /// The cursor's index.
    #[must_use]
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// Puts the cursor on `id`.
    pub fn focus(&mut self, id: Id) {
        if let Some(i) = self.rows.iter().position(|r| *r == id) {
            self.cursor = Some(i);
        }
    }

    /// J / Down.
    pub fn next(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.cursor = Some(self.cursor.map_or(0, |i| (i + 1).min(self.rows.len() - 1)));
    }

    /// K / Up.
    pub fn prev(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.cursor = Some(self.cursor.map_or(0, |i| i.saturating_sub(1)));
    }

    /// Space: selects or clears the cursor's row.
    pub fn toggle_current(&mut self) {
        if let Some(id) = self.current() {
            self.toggle(id);
        }
    }

    /// A row's check box.
    pub fn toggle(&mut self, id: Id) {
        if !self.selected.remove(&id) && self.rows.contains(&id) {
            self.selected.insert(id);
        }
    }

    /// The header check box: every row, or none when all are selected.
    pub fn toggle_all(&mut self) {
        if !self.rows.is_empty() && self.rows.iter().all(|r| self.selected.contains(r)) {
            self.selected.clear();
        } else {
            self.selected = self.rows.iter().copied().collect();
        }
    }

    /// Clears the selection.
    pub fn clear_selection(&mut self) {
        self.selected.clear();
    }

    /// Selected rows, in list order.
    #[must_use]
    pub fn selected(&self) -> Vec<Id> {
        self.rows
            .iter()
            .copied()
            .filter(|r| self.selected.contains(r))
            .collect()
    }

    /// True when `id` is selected.
    #[must_use]
    pub fn is_selected(&self, id: Id) -> bool {
        self.selected.contains(&id)
    }

    /// Rows that changed under the owner (another computer, a 409) leave
    /// the selection; the screen reads them again.
    pub fn drop_rows(&mut self, ids: &[Id]) {
        for id in ids {
            self.selected.remove(id);
        }
    }

    /// Runs a `feed.*` command; `false` for any other.
    pub fn command(&mut self, id: &str) -> bool {
        match id {
            "feed.next" | "feed.next_arrow" => self.next(),
            "feed.prev" | "feed.prev_arrow" => self.prev(),
            "feed.select" => self.toggle_current(),
            "feed.accept" => self.action = self.current().map(FeedAction::Accept),
            "feed.split" => self.action = self.current().map(FeedAction::Split),
            "feed.rule" => self.action = self.current().map(FeedAction::Rule),
            "feed.accept_selected" => {
                let s = self.selected();
                // Nothing selected: the cursor's row (one Ctrl+Enter, one row).
                let s = if s.is_empty() {
                    self.current().into_iter().collect()
                } else {
                    s
                };
                self.action = (!s.is_empty()).then_some(FeedAction::AcceptSelected(s));
            }
            _ => return false,
        }
        true
    }
}

/// One selected row as "Accept selected" sees it.
#[allow(
    clippy::struct_excessive_bools,
    reason = "flags read straight from the engine's row"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanRow<Id> {
    /// The row.
    pub id: Id,
    /// Its bucket (`None` when it isn't waiting).
    pub bucket: Option<Bucket>,
    /// An account is suggested or staged.
    pub has_account: bool,
    /// Candidate journal lines.
    pub match_candidates: u32,
    /// The single candidate line has only this row as its candidate.
    pub mutual: bool,
    /// A transfer (posted through its own route).
    pub transfer: bool,
    /// A possible duplicate waiting for a decision.
    pub possible_duplicate: bool,
}

/// Why a selected row isn't in the accept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Skip {
    /// It isn't waiting any more.
    NotWaiting,
    /// It needs the owner (no account, low confidence).
    NeedsReview,
    /// A possible duplicate: Keep both or Remove first.
    Duplicate,
    /// More than one candidate, or the candidate fits another row too.
    AmbiguousMatch,
    /// A transfer: accept it on its own.
    Transfer,
}

/// What "Accept selected" sends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AcceptPlan<Id> {
    /// Rows to post with their staged account, sorted by id.
    pub categorize: Vec<Id>,
    /// Rows to match to their one candidate, sorted by id.
    pub matches: Vec<Id>,
    /// Rows left for the owner, with why, in list order.
    pub skipped: Vec<(Id, Skip)>,
}

impl<Id> AcceptPlan<Id> {
    /// Rows the accept sends.
    #[must_use]
    pub fn len(&self) -> usize {
        self.categorize.len() + self.matches.len()
    }

    /// Nothing to send.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Plans "Accept selected" (Q-P26q): ready rows with an account post;
/// matches post only with one candidate both ways; transfers, duplicates
/// and rows needing review are left for the owner. Ids are sorted so two
/// computers lock rows in the same order.
#[must_use]
pub fn plan_accept<Id: Copy + Ord>(rows: &[PlanRow<Id>]) -> AcceptPlan<Id> {
    let mut plan = AcceptPlan {
        categorize: Vec::new(),
        matches: Vec::new(),
        skipped: Vec::new(),
    };
    for r in rows {
        let skip = match r.bucket {
            None => Some(Skip::NotWaiting),
            Some(_) if r.possible_duplicate => Some(Skip::Duplicate),
            Some(_) if r.transfer => Some(Skip::Transfer),
            Some(Bucket::Matches) => {
                if r.match_candidates == 1 && r.mutual {
                    plan.matches.push(r.id);
                    None
                } else {
                    Some(Skip::AmbiguousMatch)
                }
            }
            Some(Bucket::Ready) if r.has_account => {
                plan.categorize.push(r.id);
                None
            }
            Some(Bucket::Ready | Bucket::Review) => Some(Skip::NeedsReview),
        };
        if let Some(s) = skip {
            plan.skipped.push((r.id, s));
        }
    }
    plan.categorize.sort();
    plan.matches.sort();
    plan
}

/// The split editor's lines and what is left to place (display only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitDraft<A> {
    /// The bank amount being split (positive).
    pub total: Money,
    /// Each line's account and amount, as typed.
    pub lines: Vec<(Option<A>, Option<Money>)>,
}

impl<A> SplitDraft<A> {
    /// A split of `feed` (either sign) into two empty lines; `None` when
    /// the amount has more than 2 decimals (it can't be split).
    #[must_use]
    pub fn new(feed: Money) -> Option<SplitDraft<A>> {
        whole_cents(feed).then(|| SplitDraft {
            total: feed.abs(),
            lines: vec![(None, None), (None, None)],
        })
    }

    /// Total − typed lines (`None` if a sum overflows).
    #[must_use]
    pub fn remainder(&self) -> Option<Money> {
        let typed = Money::sum(self.lines.iter().filter_map(|(_, m)| *m)).ok()?;
        self.total.checked_sub(typed).ok()
    }

    /// Every line has an account and a positive whole-cent amount, there
    /// are 2 to 50 lines and nothing is left.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        (2..=50).contains(&self.lines.len())
            && self
                .lines
                .iter()
                .all(|(a, m)| a.is_some() && m.is_some_and(|m| m.is_positive() && whole_cents(m)))
            && self.remainder().is_some_and(Money::is_zero)
    }
}

/// A deposit match: chosen payments less fee lines against the bank
/// amount (Q-P26t). Display only: the engine reads the live payment
/// amounts and checks again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepositDraft<P, A> {
    /// The bank deposit (positive).
    pub bank: Money,
    /// The chosen payments and their amounts.
    pub payments: Vec<(P, Money)>,
    /// Fee lines: account and amount as typed.
    pub fees: Vec<(Option<A>, Option<Money>)>,
}

impl<P, A> DepositDraft<P, A> {
    /// Payments total.
    #[must_use]
    pub fn payments_total(&self) -> Option<Money> {
        Money::sum(self.payments.iter().map(|(_, m)| *m)).ok()
    }

    /// Fees total.
    #[must_use]
    pub fn fees_total(&self) -> Option<Money> {
        Money::sum(self.fees.iter().filter_map(|(_, m)| *m)).ok()
    }

    /// Bank − (payments − fees); Accept is enabled at zero.
    #[must_use]
    pub fn difference(&self) -> Option<Money> {
        let net = self
            .payments_total()?
            .checked_sub(self.fees_total()?)
            .ok()?;
        self.bank.checked_sub(net).ok()
    }

    /// Payments chosen (1 to 100), every fee line has an account and a
    /// positive whole-cent amount (at most 10), and the difference is zero.
    #[must_use]
    pub fn is_balanced(&self) -> bool {
        (1..=100).contains(&self.payments.len())
            && self.fees.len() <= 10
            && self
                .fees
                .iter()
                .all(|(a, m)| a.is_some() && m.is_some_and(|m| m.is_positive() && whole_cents(m)))
            && self.difference().is_some_and(Money::is_zero)
    }
}

/// A rule's direction (the rule AST's `amount_sign`, feed sign).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleDirection {
    /// Money out (a positive feed amount).
    MoneyOut,
    /// Money in.
    MoneyIn,
    /// Either.
    Either,
}

impl RuleDirection {
    /// `amount_sign` for the rule AST: money out is 1 (the feed sign).
    #[must_use]
    pub const fn amount_sign(self) -> Option<i8> {
        match self {
            RuleDirection::MoneyOut => Some(1),
            RuleDirection::MoneyIn => Some(-1),
            RuleDirection::Either => None,
        }
    }

    /// The direction of a feed amount.
    #[must_use]
    pub fn of(feed: Money) -> RuleDirection {
        if feed.is_positive() {
            RuleDirection::MoneyOut
        } else {
            RuleDirection::MoneyIn
        }
    }
}

#[cfg(test)]
#[path = "bank_feed_tests.rs"]
mod tests;
