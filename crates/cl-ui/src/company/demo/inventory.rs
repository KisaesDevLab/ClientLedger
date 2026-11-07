//! The demo engine's average costing (P24 plan §2.2, §3 "Demo parity"),
//! held to `fixtures/inventory/golden.json` (every case not marked
//! `engineOnly`).
//!
//! - **Full replay per item**: every change replays the item's movements in
//!   position order (date, then entry order), so the result depends on the
//!   documents only, never on the order they were entered (I6).
//! - Relief on the true value, `round2(T × q / Q)` (Q-P24c); excess and
//!   empty-stock inflows at the last positive average, else the purchase
//!   cost snapshot (Q-P24d); corrections on the next inflow after negative
//!   stock; a residual when a row leaves nothing on hand but value.
//! - Rounding is half away from zero
//!   (`round_dp_with_strategy(_, MidpointAwayFromZero)`), negatives
//!   included; never banker's rounding (golden case N).
//! - Closed periods (owner 2026-11-06): any inventory document dated on or
//!   before the closing date is refused (`INVENTORY_PERIOD_CLOSED`), even
//!   with the closing-date password, so nothing is ever restated.
//!
//! Money is exact: `Money`, `Quantity`, `UnitCost` and `Decimal` only.

use std::collections::BTreeMap;

use cl_core::ids::ItemId;
use cl_core::{CalendarDate, Money, Quantity, UnitCost};
use rust_decimal::{Decimal, RoundingStrategy};

/// What a movement does to an item (P24 §2.1 `kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveKind {
    /// A bill, check or card charge line: `quantity` in at `amount`.
    Receipt {
        /// Units received (above zero).
        quantity: Quantity,
        /// The line amount.
        amount: Money,
    },
    /// A vendor credit line: `quantity` out at `amount` (N1).
    VendorReturn {
        /// Units returned (above zero).
        quantity: Quantity,
        /// The line amount.
        amount: Money,
    },
    /// An invoice or sales receipt line: `quantity` out at cost.
    Sale {
        /// Units sold (above zero).
        quantity: Quantity,
    },
    /// A credit memo line: `quantity` back in at the current average
    /// (Q-P24e).
    CustomerReturn {
        /// Units returned (above zero).
        quantity: Quantity,
    },
    /// An adjustment of quantity only (value at cost).
    AdjustQuantity {
        /// Signed units.
        change: Quantity,
    },
    /// An adjustment of value only (refused while nothing is on hand).
    AdjustValue {
        /// Signed value.
        change: Money,
    },
    /// An adjustment of both: `change` units at the given `value`.
    AdjustBoth {
        /// Signed units.
        change: Quantity,
        /// Signed value.
        value: Money,
    },
}

/// One movement: a document's effect on one item (several lines of one
/// part on one document are aggregated by the caller, S4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Movement {
    /// The document (demo key).
    pub doc: String,
    /// The part.
    pub item: ItemId,
    /// Its date.
    pub date: CalendarDate,
    /// Entry order (kept by an edit: the engine's `created_at`).
    pub seq: u64,
    /// What it does.
    pub kind: MoveKind,
    /// The item's purchase cost when the line was posted (Q-P24d).
    pub fallback_cost: Option<UnitCost>,
    /// A credit memo's linked sale (Q-P24e/u): its units come back at that
    /// sale's cost by cumulative difference.
    pub return_of: Option<String>,
}

/// One row after a replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The document.
    pub doc: String,
    /// The part.
    pub item: ItemId,
    /// The date.
    pub date: CalendarDate,
    /// Entry order.
    pub seq: u64,
    /// Signed effect on the asset account before any correction or
    /// residual (a receipt positive, a sale's relief negative).
    pub value: Money,
    /// The negative-stock correction C (posted Dr COGS / Cr asset when
    /// positive; the asset moves by `-C`).
    pub correction: Money,
    /// The residual moved to COGS (the asset moves by `-residual`).
    pub residual: Money,
    /// On hand after the row.
    pub on_hand: Quantity,
    /// Value after the row (what the GL holds).
    pub asset_value: Money,
    /// The true last positive average after the row.
    pub last_cost: Option<UnitCost>,
    /// What the movement does (the warnings read its direction).
    pub kind: MoveKind,
    /// A linked return's sale (the row's place inside its document).
    pub link: Option<String>,
    /// A sale's `correction_share` (PR-4 S5): the corrections attributed to
    /// the units it sold below 0 (zero on every other row).
    pub correction_share: Money,
}

/// Why the demo costing could not compute (an amount out of range).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CostError {
    /// An amount or quantity left its column's range.
    Overflow,
}

fn round(d: Decimal, dp: u32) -> Decimal {
    d.round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero)
}

fn money(d: Decimal) -> Result<Money, CostError> {
    Money::try_from(round(d, 2)).map_err(|_| CostError::Overflow)
}

/// `round2(v × a / b)`.
fn ratio(v: Money, a: Quantity, b: Quantity) -> Result<Money, CostError> {
    let num = v
        .as_decimal()
        .checked_mul(a.as_decimal())
        .ok_or(CostError::Overflow)?;
    money(num.checked_div(b.as_decimal()).ok_or(CostError::Overflow)?)
}

fn add(a: Money, b: Money) -> Result<Money, CostError> {
    a.checked_add(b).map_err(|_| CostError::Overflow)
}

fn sub(a: Money, b: Money) -> Result<Money, CostError> {
    a.checked_sub(b).map_err(|_| CostError::Overflow)
}

fn qadd(a: Quantity, b: Quantity) -> Result<Quantity, CostError> {
    a.checked_add(b).map_err(|_| CostError::Overflow)
}

/// The running state of one item.
#[derive(Clone, Copy, Debug, Default)]
struct State {
    q: Quantity,
    t: Money,
    l: Option<UnitCost>,
}

impl State {
    /// The cost per unit used beyond what is on hand (Q-P24d).
    fn unit_cost(&self, fallback: Option<UnitCost>) -> UnitCost {
        self.l
            .filter(|l| l.is_positive())
            .or(fallback)
            .unwrap_or(UnitCost::ZERO)
    }

    /// Value of `q` units out at cost (`q` above zero).
    fn relief(&self, q: Quantity, fallback: Option<UnitCost>) -> Result<Money, CostError> {
        if self.q.is_positive() && q <= self.q {
            return ratio(self.t, q, self.q);
        }
        let held = if self.q.is_positive() {
            self.t
        } else {
            Money::ZERO
        };
        let on = if self.q.is_positive() {
            self.q
        } else {
            Quantity::ZERO
        };
        let excess = q.checked_sub(on).map_err(|_| CostError::Overflow)?;
        add(
            held,
            self.unit_cost(fallback)
                .value_of(excess)
                .map_err(|_| CostError::Overflow)?,
        )
    }

    /// Value of `r` units in at cost.
    fn inflow_at_cost(&self, r: Quantity, fallback: Option<UnitCost>) -> Result<Money, CostError> {
        if self.q.is_positive() {
            ratio(self.t, r, self.q)
        } else {
            self.unit_cost(fallback)
                .value_of(r)
                .map_err(|_| CostError::Overflow)
        }
    }

    /// The correction of an inflow of `r` units worth `a` while on hand
    /// is below zero.
    fn correction(&self, r: Quantity, a: Money) -> Result<Money, CostError> {
        if !self.q.is_negative() {
            return Ok(Money::ZERO);
        }
        let short = self.q.checked_neg().map_err(|_| CostError::Overflow)?;
        let k = if r < short { r } else { short };
        let neg_t = self.t.checked_neg().map_err(|_| CostError::Overflow)?;
        sub(ratio(a, k, r)?, ratio(neg_t, k, short)?)
    }

    /// Takes one inflow of `r` units worth `a`.
    fn inflow(&mut self, r: Quantity, a: Money) -> Result<(Money, Money), CostError> {
        let c = self.correction(r, a)?;
        self.t = sub(add(self.t, a)?, c)?;
        self.q = qadd(self.q, r)?;
        Ok((a, c))
    }

    /// Takes one outflow of `q` units worth `v` (relief or a given value).
    fn outflow(&mut self, q: Quantity, v: Money) -> Result<Money, CostError> {
        self.t = sub(self.t, v)?;
        self.q = self.q.checked_sub(q).map_err(|_| CostError::Overflow)?;
        v.checked_neg().map_err(|_| CostError::Overflow)
    }

    fn apply(&mut self, m: &Movement) -> Result<(Money, Money), CostError> {
        let fb = m.fallback_cost;
        match m.kind {
            MoveKind::Receipt { quantity, amount } => self.inflow(quantity, amount),
            MoveKind::CustomerReturn { quantity } => {
                let a = self.inflow_at_cost(quantity, fb)?;
                self.inflow(quantity, a)
            }
            MoveKind::Sale { quantity } => {
                let v = self.relief(quantity, fb)?;
                Ok((self.outflow(quantity, v)?, Money::ZERO))
            }
            MoveKind::VendorReturn { quantity, amount } => {
                Ok((self.outflow(quantity, amount)?, Money::ZERO))
            }
            MoveKind::AdjustQuantity { change } if change.is_negative() => {
                let q = change.abs();
                let v = self.relief(q, fb)?;
                Ok((self.outflow(q, v)?, Money::ZERO))
            }
            MoveKind::AdjustQuantity { change } => {
                let a = self.inflow_at_cost(change, fb)?;
                self.inflow(change, a)
            }
            MoveKind::AdjustValue { change } => {
                self.t = add(self.t, change)?;
                Ok((change, Money::ZERO))
            }
            MoveKind::AdjustBoth { change, value } if change.is_negative() => {
                let v = value.checked_neg().map_err(|_| CostError::Overflow)?;
                Ok((self.outflow(change.abs(), v)?, Money::ZERO))
            }
            MoveKind::AdjustBoth { change, value } => self.inflow(change, value),
        }
    }
}

/// A sale's place: its date and entry order.
type SalePos = (CalendarDate, u64);

/// A row's position key (see [`position`]).
type PosKey<'m> = (CalendarDate, u64, &'m str, Option<SalePos>, Option<&'m str>);

/// Position order, as the engine's `positionAsc` (PR-4 B1, RB1): date,
/// then entry order, then the document key; inside one document the
/// linked sale's position (date, entry order; none first, so the unlinked
/// row leads), then the sale's key. The row id the engine ends on never
/// decides inside one item: a document has one row per item and linked
/// sale (the caller aggregates its lines, S4). `sales` holds each sale's
/// position.
fn position<'m>(m: &'m Movement, sales: &BTreeMap<&str, SalePos>) -> PosKey<'m> {
    let sale = m.return_of.as_deref();
    let at = sale.and_then(|s| sales.get(s).copied());
    (m.date, m.seq, m.doc.as_str(), at, sale)
}

/// A sale linked returns come back from: its relief `R` for `S` units
/// (plus the corrections of units it sold below 0, PR-4 S5), the units
/// returned so far `N` and the value they brought back `B`.
#[derive(Clone, Copy, Debug, Default)]
struct Sold {
    relief: Money,
    units: Quantity,
    returned: Quantity,
    brought: Money,
    /// The corrections attributed to it (its `correction_share`).
    share: Money,
    /// Its row in the replay's output.
    row: usize,
}

impl Sold {
    /// The next linked return of `q` units: `max(0, round2(R × (N + q) / S)
    /// − B)` (Q-P24e; the cumulative difference while R does not change;
    /// never below 0, PR-4 RS2: R can fall under what earlier returns
    /// brought back when its short units are cured at a lower cost).
    fn take(&mut self, q: Quantity) -> Result<Money, CostError> {
        let next = qadd(self.returned, q)?;
        let v = if self.units.is_positive() {
            let v = sub(ratio(self.relief, next, self.units)?, self.brought)?;
            if v.is_negative() { Money::ZERO } else { v }
        } else {
            Money::ZERO
        };
        self.returned = next;
        self.brought = add(self.brought, v)?;
        Ok(v)
    }
}

/// The units a movement takes out (sales, vendor returns, adjustments
/// down) and brings in.
fn flows(k: MoveKind) -> (Quantity, Quantity) {
    match k {
        MoveKind::Sale { quantity } | MoveKind::VendorReturn { quantity, .. } => {
            (quantity, Quantity::ZERO)
        }
        MoveKind::Receipt { quantity, .. } | MoveKind::CustomerReturn { quantity } => {
            (Quantity::ZERO, quantity)
        }
        MoveKind::AdjustQuantity { change } | MoveKind::AdjustBoth { change, .. } => {
            if change.is_negative() {
                (change.abs(), Quantity::ZERO)
            } else {
                (Quantity::ZERO, change)
            }
        }
        MoveKind::AdjustValue { .. } => (Quantity::ZERO, Quantity::ZERO),
    }
}

/// The first-short, first-cured queue of units sold below 0 (PR-4 S5):
/// each entry is the sale (none for another outflow) and its units still
/// short.
#[derive(Default)]
struct Short(Vec<(Option<String>, Quantity)>);

impl Short {
    /// An outflow of `q` units with `on` on hand before it.
    fn join(&mut self, sale: Option<&str>, q: Quantity, on: Quantity) -> Result<(), CostError> {
        let held = if on.is_positive() { on } else { Quantity::ZERO };
        let excess = q.checked_sub(held).map_err(|_| CostError::Overflow)?;
        if excess.is_positive() {
            self.0.push((sale.map(str::to_string), excess));
        }
        Ok(())
    }

    /// An inflow curing `k` units with correction `c`: a linked return
    /// first takes its own sale's short units (`own`, PR-4 RS2), then the
    /// oldest; each sale taken gets `round2(C × K_i / k) − round2(C ×
    /// K_(i−1) / k)` (`K_i` the units taken so far; a zero share is left
    /// out, as the engine's `trackShort`).
    fn cure(
        &mut self,
        k: Quantity,
        c: Money,
        own: Option<&str>,
    ) -> Result<Vec<(String, Money)>, CostError> {
        let mut out = Vec::new();
        let mut taken = Quantity::ZERO;
        let mut given = Money::ZERO;
        let mut take = |e: &mut (Option<String>, Quantity)| -> Result<(), CostError> {
            let left = k.checked_sub(taken).map_err(|_| CostError::Overflow)?;
            let u = if e.1 < left { e.1 } else { left };
            if !u.is_positive() {
                return Ok(());
            }
            taken = qadd(taken, u)?;
            let up_to = ratio(c, taken, k)?;
            let share = sub(up_to, given)?;
            given = up_to;
            if let Some(sale) = &e.0
                && !share.is_zero()
            {
                out.push((sale.clone(), share));
            }
            e.1 = e.1.checked_sub(u).map_err(|_| CostError::Overflow)?;
            Ok(())
        };
        if let Some(own) = own {
            for e in &mut self.0 {
                if e.0.as_deref() == Some(own) {
                    take(e)?;
                }
            }
        }
        for e in &mut self.0 {
            take(e)?;
        }
        self.0.retain(|e| e.1.is_positive());
        Ok(out)
    }
}

/// Replays every item's movements from zero, in position order. Rows come
/// back per item, each item's rows in position order.
///
/// # Errors
/// [`CostError::Overflow`].
pub fn replay(moves: &[Movement]) -> Result<Vec<Row>, CostError> {
    let mut by_item: BTreeMap<String, Vec<&Movement>> = BTreeMap::new();
    for m in moves {
        by_item.entry(m.item.to_string()).or_default().push(m);
    }
    let sales: BTreeMap<&str, SalePos> = moves
        .iter()
        .filter(|m| matches!(m.kind, MoveKind::Sale { .. }))
        .map(|m| (m.doc.as_str(), (m.date, m.seq)))
        .collect();
    let mut out = Vec::with_capacity(moves.len());
    for list in by_item.values_mut() {
        // Stable: rows of one document and link keep their line order.
        list.sort_by(|a, b| position(a, &sales).cmp(&position(b, &sales)));
        let mut s = State::default();
        let mut sold: BTreeMap<&str, Sold> = BTreeMap::new();
        let mut short = Short::default();
        for m in list.iter() {
            let on_before = s.q;
            let (out_q, in_q) = flows(m.kind);
            let linked = match (m.kind, m.return_of.as_deref()) {
                (MoveKind::CustomerReturn { quantity }, Some(sale)) => {
                    sold.get_mut(sale).map(|x| (quantity, x))
                }
                _ => None,
            };
            let is_linked = linked.is_some();
            let (value, correction) = match linked {
                Some((quantity, x)) => {
                    let a = x.take(quantity)?;
                    s.inflow(quantity, a)?
                }
                None => s.apply(m)?,
            };
            if let MoveKind::Sale { quantity } = m.kind {
                sold.insert(
                    m.doc.as_str(),
                    Sold {
                        relief: value.checked_neg().map_err(|_| CostError::Overflow)?,
                        units: quantity,
                        row: out.len(),
                        ..Sold::default()
                    },
                );
            }
            // PR-4 S5: units sold below 0 and the corrections that cure
            // them, attributed to their sales.
            if out_q.is_positive() {
                let sale = matches!(m.kind, MoveKind::Sale { .. }).then_some(m.doc.as_str());
                short.join(sale, out_q, on_before)?;
            }
            if in_q.is_positive() && on_before.is_negative() {
                let short_q = on_before.abs();
                let k = if in_q < short_q { in_q } else { short_q };
                let own = m.return_of.as_deref().filter(|_| is_linked);
                for (sale, share) in short.cure(k, correction, own)? {
                    if let Some(x) = sold.get_mut(sale.as_str()) {
                        x.relief = add(x.relief, share)?;
                        x.share = add(x.share, share)?;
                    }
                }
            }
            let mut residual = Money::ZERO;
            if s.q.is_zero() && !s.t.is_zero() {
                residual = s.t;
                s.t = Money::ZERO;
            }
            if s.q.is_positive() && s.t.is_positive() {
                s.l = UnitCost::average(s.t, s.q).map_err(|_| CostError::Overflow)?;
            }
            out.push(Row {
                doc: m.doc.clone(),
                item: m.item,
                date: m.date,
                seq: m.seq,
                value,
                correction,
                residual,
                on_hand: s.q,
                asset_value: s.t,
                last_cost: s.l,
                kind: m.kind,
                link: m.return_of.clone(),
                correction_share: Money::ZERO,
            });
        }
        for x in sold.values() {
            if let Some(r) = out.get_mut(x.row) {
                r.correction_share = x.share;
            }
        }
    }
    Ok(out)
}

/// `NEGATIVE_STOCK` over the rows at or after `from` (the walk's start),
/// as the engine's `negativeWarning` (PR-4 S1, RS1), at most one per item:
/// the first walked row that
/// - is the posting `trigger`'s own outflow ending below 0 (each sale while
///   already below 0 warns again; its own inflows never warn);
/// - or is an earlier posting's row now below 0 whose stored on hand
///   before this posting (`old`) was 0 or more (voiding the receipt that
///   cured a shortfall warns);
/// - or is an earlier value-only adjustment that had stock (stored on hand
///   above 0) and now sits at 0 or below.
#[must_use]
pub fn negative_stock(
    rows: &[Row],
    from: (CalendarDate, u64),
    old: &[Row],
    trigger: &str,
) -> Vec<cl_api::NegativeStock> {
    let mut out: Vec<cl_api::NegativeStock> = Vec::new();
    for r in rows {
        if (r.date, r.seq) < from || out.iter().any(|w| w.item_id == r.item) {
            continue;
        }
        let fresh = if r.doc == trigger {
            flows(r.kind).0.is_positive() && r.on_hand.is_negative()
        } else {
            // A row keeps its document, item and linked sale.
            let stored = old
                .iter()
                .find(|o| o.doc == r.doc && o.item == r.item && o.link == r.link)
                .map_or(Quantity::ZERO, |o| o.on_hand);
            (r.on_hand.is_negative() && !stored.is_negative())
                || (matches!(r.kind, MoveKind::AdjustValue { .. })
                    && !r.on_hand.is_positive()
                    && stored.is_positive())
        };
        if fresh {
            out.push(cl_api::NegativeStock {
                item_id: r.item,
                date: r.date,
                on_hand: r.on_hand,
            });
        }
    }
    out
}

/// Why the demo engine refused an inventory write (the engine's codes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DemoInventoryError {
    /// Dated on or before the closing date (`INVENTORY_PERIOD_CLOSED`;
    /// refused even with the closing-date password, owner 2026-11-06).
    PeriodClosed,
    /// A value-only adjustment with nothing on hand at its date
    /// (`INVENTORY_NO_STOCK`, Q-P24s).
    NoStock,
    /// On hand moved since the count sheet was read (`STALE`).
    Stale(Vec<ItemId>),
    /// No such document.
    NotFound,
    /// An amount left its range.
    Cost(CostError),
    /// A linked return dated before its sale (`RETURN_BEFORE_SALE`).
    ReturnBeforeSale,
    /// Linked returns over what the sale sold (`RETURN_EXCEEDS_SALE`).
    ReturnExceedsSale,
    /// A sale with linked returns voided, deleted or cut below them
    /// (`SALE_HAS_RETURNS`, Q-P24x).
    SaleHasReturns,
}

impl From<CostError> for DemoInventoryError {
    fn from(e: CostError) -> Self {
        DemoInventoryError::Cost(e)
    }
}

impl std::fmt::Display for DemoInventoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for DemoInventoryError {}

impl std::fmt::Display for CostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("an amount is out of range")
    }
}

impl std::error::Error for CostError {}

impl DemoInventoryError {
    /// The engine code the demo answers with.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            DemoInventoryError::PeriodClosed => "INVENTORY_PERIOD_CLOSED",
            DemoInventoryError::NoStock => "INVENTORY_NO_STOCK",
            DemoInventoryError::Stale(_) => "STALE",
            DemoInventoryError::NotFound => "NOT_FOUND",
            DemoInventoryError::Cost(_) => "VALIDATION_ERROR",
            DemoInventoryError::ReturnBeforeSale => "RETURN_BEFORE_SALE",
            DemoInventoryError::ReturnExceedsSale => "RETURN_EXCEEDS_SALE",
            DemoInventoryError::SaleHasReturns => "SALE_HAS_RETURNS",
        }
    }
}

/// One part's movement on a document as a write sends it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocLine {
    /// The part.
    pub item: ItemId,
    /// What it does.
    pub kind: MoveKind,
    /// The item's purchase cost now (the fallback snapshot).
    pub fallback_cost: Option<UnitCost>,
    /// A credit memo line's linked sale (its demo key).
    pub return_of: Option<String>,
}

/// The engine's linked-return rules over a ledger (U-158): each linked
/// return is of a sale of the same item that comes before it in position
/// order (date, then entry order: a credit memo entered before its sale on
/// the same date is refused too, review S2), and the returns linked to a
/// sale never pass what it sold (Q-P24x: a sale edited below them, moved
/// after one, or losing the item is refused too).
fn check_links(moves: &[Movement], doc: &str) -> Result<(), DemoInventoryError> {
    // (sale, item) -> (returned so far, the first return's position).
    type Linked<'a> = BTreeMap<(&'a str, String), (Quantity, (CalendarDate, u64))>;
    let mut linked: Linked = BTreeMap::new();
    for m in moves {
        let (Some(sale), MoveKind::CustomerReturn { quantity }) = (m.return_of.as_deref(), m.kind)
        else {
            continue;
        };
        let at = (m.date, m.seq);
        let e = linked
            .entry((sale, m.item.to_string()))
            .or_insert((Quantity::ZERO, at));
        e.0 = qadd(e.0, quantity)?;
        e.1 = e.1.min(at);
    }
    for ((sale, item), (returned, first)) in linked {
        let sold = moves.iter().find(|m| {
            m.doc == sale && m.item.to_string() == item && matches!(m.kind, MoveKind::Sale { .. })
        });
        // The sale being written breaks its returns (409); a return asking
        // for more than was sold, or before it, is the return's fault (400).
        let sale_side = sale == doc;
        let Some(sold) = sold else {
            return Err(if sale_side {
                DemoInventoryError::SaleHasReturns
            } else {
                DemoInventoryError::ReturnExceedsSale
            });
        };
        if (sold.date, sold.seq) > first {
            return Err(if sale_side {
                DemoInventoryError::SaleHasReturns
            } else {
                DemoInventoryError::ReturnBeforeSale
            });
        }
        if let MoveKind::Sale { quantity } = sold.kind
            && returned > quantity
        {
            return Err(if sale_side {
                DemoInventoryError::SaleHasReturns
            } else {
                DemoInventoryError::ReturnExceedsSale
            });
        }
    }
    Ok(())
}

/// The demo engine's inventory ledger: every live movement, the closing
/// date and the entry counter.
#[derive(Clone, Debug, Default)]
pub struct DemoInventory {
    moves: Vec<Movement>,
    seq: u64,
    /// The company's closing date (inventory writes on or before it are
    /// refused).
    pub closing_date: Option<CalendarDate>,
    /// The demo company tracks inventory (`inventoryEnabled`; off like a
    /// new engine company until Preferences turns it on, Q-P24j).
    pub enabled: bool,
    /// Each document's kind, for the valuation detail (WP-2).
    pub doc_types: BTreeMap<String, cl_api::TxnType>,
    /// The warnings of the last write (the screens read them once).
    pub last_warnings: Vec<cl_api::NegativeStock>,
    /// Adjustments recorded in the demo (counts and openings included).
    pub adjustments: Vec<cl_api::AdjustmentRecord>,
    /// The sample parts were added (once, on the first switch-on).
    pub seeded: bool,
}

impl DemoInventory {
    fn check_open(&self, date: CalendarDate) -> Result<(), DemoInventoryError> {
        match self.closing_date {
            Some(c) if date <= c => Err(DemoInventoryError::PeriodClosed),
            _ => Ok(()),
        }
    }

    /// The rows after a full replay.
    ///
    /// # Errors
    /// [`CostError::Overflow`].
    pub fn rows(&self) -> Result<Vec<Row>, CostError> {
        replay(&self.moves)
    }

    /// The date of `doc`'s movements (`None` without any).
    #[must_use]
    pub fn date_of(&self, doc: &str) -> Option<CalendarDate> {
        self.moves.iter().find(|m| m.doc == doc).map(|m| m.date)
    }

    /// Every live movement (the valuation detail reads them).
    #[must_use]
    pub fn moves(&self) -> &[Movement] {
        &self.moves
    }

    /// Whether `doc` has movements.
    #[must_use]
    pub fn has(&self, doc: &str) -> bool {
        self.moves.iter().any(|m| m.doc == doc)
    }

    /// Posts a new document's lines; answers the `NEGATIVE_STOCK` warnings.
    ///
    /// # Errors
    /// [`DemoInventoryError::PeriodClosed`], [`DemoInventoryError::NoStock`]
    /// (nothing changes), [`DemoInventoryError::Cost`].
    pub fn post(
        &mut self,
        doc: &str,
        date: CalendarDate,
        lines: &[DocLine],
    ) -> Result<Vec<cl_api::NegativeStock>, DemoInventoryError> {
        self.check_open(date)?;
        self.seq += 1;
        let seq = self.seq;
        self.replace(doc, date, seq, lines, (date, seq))
    }

    /// Replaces a document's lines (an edit keeps its entry order); the
    /// walk starts at the earlier of its old and new positions.
    ///
    /// # Errors
    /// As [`DemoInventory::post`]; [`DemoInventoryError::NotFound`]; an
    /// old date on or before the closing date is refused too.
    pub fn edit(
        &mut self,
        doc: &str,
        date: CalendarDate,
        lines: &[DocLine],
    ) -> Result<Vec<cl_api::NegativeStock>, DemoInventoryError> {
        let old = self
            .moves
            .iter()
            .filter(|m| m.doc == doc)
            .map(|m| (m.date, m.seq))
            .min()
            .ok_or(DemoInventoryError::NotFound)?;
        self.check_open(old.0)?;
        self.check_open(date)?;
        let start = old.min((date, old.1));
        self.replace(doc, date, old.1, lines, start)
    }

    /// The linked-return rules for `doc` written with `lines` on `date`,
    /// checked before anything is saved (nothing changes).
    ///
    /// # Errors
    /// `ReturnBeforeSale`, `ReturnExceedsSale`, `SaleHasReturns`.
    pub fn check_write(
        &self,
        doc: &str,
        date: CalendarDate,
        lines: &[DocLine],
    ) -> Result<(), DemoInventoryError> {
        // An edit keeps its entry order (as `replace` will); a new
        // document comes after everything.
        let seq = self
            .moves
            .iter()
            .filter(|m| m.doc == doc)
            .map(|m| m.seq)
            .min()
            .unwrap_or(u64::MAX);
        let mut next: Vec<Movement> = self
            .moves
            .iter()
            .filter(|m| m.doc != doc)
            .cloned()
            .collect();
        for l in lines {
            next.push(Movement {
                doc: doc.to_string(),
                item: l.item,
                date,
                seq,
                kind: l.kind,
                fallback_cost: l.fallback_cost,
                return_of: l.return_of.clone(),
            });
        }
        check_links(&next, doc)
    }

    /// Voids or deletes a document: its movements go; later rows are
    /// recomputed.
    ///
    /// # Errors
    /// [`DemoInventoryError::NotFound`], [`DemoInventoryError::PeriodClosed`].
    pub fn remove(&mut self, doc: &str) -> Result<Vec<cl_api::NegativeStock>, DemoInventoryError> {
        let old = self
            .moves
            .iter()
            .filter(|m| m.doc == doc)
            .map(|m| (m.date, m.seq))
            .min()
            .ok_or(DemoInventoryError::NotFound)?;
        self.check_open(old.0)?;
        // Q-P24x: a sale with linked returns stays until they go.
        if self
            .moves
            .iter()
            .any(|m| m.return_of.as_deref() == Some(doc))
        {
            return Err(DemoInventoryError::SaleHasReturns);
        }
        let before = self.rows()?;
        self.moves.retain(|m| m.doc != doc);
        let rows = self.rows()?;
        Ok(negative_stock(&rows, old, &before, doc))
    }

    fn replace(
        &mut self,
        doc: &str,
        date: CalendarDate,
        seq: u64,
        lines: &[DocLine],
        start: (CalendarDate, u64),
    ) -> Result<Vec<cl_api::NegativeStock>, DemoInventoryError> {
        let mut next: Vec<Movement> = self
            .moves
            .iter()
            .filter(|m| m.doc != doc)
            .cloned()
            .collect();
        for l in lines {
            next.push(Movement {
                doc: doc.to_string(),
                item: l.item,
                date,
                seq,
                kind: l.kind,
                fallback_cost: l.fallback_cost,
                return_of: l.return_of.clone(),
            });
        }
        check_links(&next, doc)?;
        let rows = replay(&next)?;
        // A value-only change needs stock on hand just before it (Q-P24s).
        for l in lines {
            if matches!(l.kind, MoveKind::AdjustValue { .. }) {
                let before = rows
                    .iter()
                    .rfind(|r| {
                        r.item == l.item && (r.date, r.seq, r.doc.as_str()) < (date, seq, doc)
                    })
                    .map_or(Quantity::ZERO, |r| r.on_hand);
                if !before.is_positive() {
                    return Err(DemoInventoryError::NoStock);
                }
            }
        }
        let before = self.rows()?;
        self.moves = next;
        Ok(negative_stock(&rows, start, &before, doc))
    }

    /// An item's on hand, value and average after every posting (Q-P24l).
    ///
    /// # Errors
    /// [`CostError::Overflow`].
    pub fn stock(&self, item: ItemId) -> Result<cl_api::ItemStock, CostError> {
        self.stock_as_of(item, None)
    }

    /// The same as of `date` (every row dated on or before it).
    ///
    /// # Errors
    /// [`CostError::Overflow`].
    pub fn stock_as_of(
        &self,
        item: ItemId,
        date: Option<CalendarDate>,
    ) -> Result<cl_api::ItemStock, CostError> {
        let rows = self.rows()?;
        let last = rows
            .iter()
            .rfind(|r| r.item == item && date.is_none_or(|d| r.date <= d));
        Ok(cl_api::ItemStock {
            quantity_on_hand: Some(last.map_or(Quantity::ZERO, |r| r.on_hand)),
            average_cost: last.and_then(|r| r.last_cost),
            asset_value: Some(last.map_or(Money::ZERO, |r| r.asset_value)),
            unit_prices: Vec::new(),
        })
    }

    /// Posts a count (Q-P24g): each line's `sheet_on_hand` must still be on
    /// hand as of `as_of`, else `Stale` listing the items; one adjustment
    /// `doc` for the non-zero differences (none when every count matched).
    ///
    /// # Errors
    /// [`DemoInventoryError::Stale`], as [`DemoInventory::post`].
    pub fn post_count(
        &mut self,
        doc: &str,
        as_of: CalendarDate,
        lines: &[cl_api::CountLine],
    ) -> Result<(Vec<DocLine>, Vec<cl_api::NegativeStock>), DemoInventoryError> {
        self.check_open(as_of)?;
        let mut stale = Vec::new();
        let mut changes = Vec::new();
        for l in lines {
            let on = self
                .stock_as_of(l.item_id, Some(as_of))?
                .quantity_on_hand
                .unwrap_or(Quantity::ZERO);
            if on != l.sheet_on_hand {
                stale.push(l.item_id);
                continue;
            }
            let change = l
                .counted_quantity
                .checked_sub(on)
                .map_err(|_| CostError::Overflow)?;
            if !change.is_zero() {
                changes.push(DocLine {
                    item: l.item_id,
                    kind: MoveKind::AdjustQuantity { change },
                    fallback_cost: None,
                    return_of: None,
                });
            }
        }
        if !stale.is_empty() {
            return Err(DemoInventoryError::Stale(stale));
        }
        if changes.is_empty() {
            return Ok((changes, Vec::new()));
        }
        let warnings = self.post(doc, as_of, &changes)?;
        Ok((changes, warnings))
    }
}

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;
