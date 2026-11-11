//! The demo engine's reports (P28 WP5): the statements and the ledger
//! answered from the sample's postings (with the demo's own changes) in the
//! engine's exact JSON shapes, then read by the same adapters as the live
//! engine (`run::adapt_exact`, `list_run::adapt`, `adapters::bva::table`).
//! The demo plays the engine here, so it adds up and works out the
//! percents the engine would send (1 dp, half away from zero); the window
//! still never adds anything up. Synthetic.

use std::collections::{BTreeMap, BTreeSet};

use cl_api::{Account, AccountType, ApiError, LocalValidation};
use cl_core::CalendarDate;
use cl_core::ids::{AccountId, TagGroupId, TagId, TransactionId};
use cl_core::report::{Basis, Compare, DateRange, Extra, ReportDates, ReportRequest, ReportTable};
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Value, json};

use super::DemoState;

/// One posted line the reports read.
#[derive(Clone, Debug)]
pub(crate) struct Post {
    pub(crate) date: CalendarDate,
    pub(crate) account: AccountId,
    pub(crate) debit: Decimal,
    pub(crate) credit: Decimal,
    pub(crate) tag: Option<TagId>,
    pub(crate) txn: TransactionId,
    pub(crate) txn_type: String,
    pub(crate) number: String,
    pub(crate) contact: Option<String>,
    pub(crate) memo: String,
    pub(crate) line: usize,
}

/// The demo company's books: its chart and every posted line in date order.
pub(crate) struct Books {
    pub(crate) chart: Vec<Account>,
    pub(crate) posts: Vec<Post>,
}

/// The route tags (`Route` group) the demo puts on income lines that have
/// no tag of their own, so the tag split and the tag columns have something
/// to show (Q-P28af): North, South, Lake Area by transaction number.
fn route_tag(id: u32) -> TagId {
    super::lists::tag_id(id % 3 + 1)
}

fn wire_type(kind: &str) -> String {
    serde_json::to_value(super::register::txn_type(kind))
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn txn_id(n: u32) -> TransactionId {
    TransactionId::parse(&format!("d0000000-0000-4000-8000-b{n:011}"))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

/// The engine's sections of the P&L, in order.
const PL_SECTIONS: [(&str, &[AccountType]); 5] = [
    ("revenue", &[AccountType::Revenue]),
    ("cogs", &[AccountType::Cogs]),
    ("expenses", &[AccountType::Expense]),
    ("otherRevenue", &[AccountType::OtherRevenue]),
    ("otherExpenses", &[AccountType::OtherExpense]),
];

/// Debit-normal types (the engine's `isDebitNormal`).
pub(crate) fn debit_normal(t: AccountType) -> bool {
    matches!(
        t,
        AccountType::Asset | AccountType::Cogs | AccountType::Expense | AccountType::OtherExpense
    )
}

fn is_pl(t: AccountType) -> bool {
    matches!(
        t,
        AccountType::Revenue
            | AccountType::OtherRevenue
            | AccountType::Cogs
            | AccountType::Expense
            | AccountType::OtherExpense
    )
}

/// A line's amount in its account's natural sign.
fn natural(t: AccountType, debit: Decimal, credit: Decimal) -> Decimal {
    if debit_normal(t) {
        debit - credit
    } else {
        credit - debit
    }
}

/// The engine's exact string (4 dp).
fn ex(d: Decimal) -> String {
    format!("{:.4}", d.round_dp(4))
}

/// The engine's `percent1`: part / base x 100 at 1 dp, half away from
/// zero; `None` when the base is zero.
pub(crate) fn percent1(part: Decimal, base: Decimal) -> Option<String> {
    if base.is_zero() {
        return None;
    }
    let p = (part / base * Decimal::ONE_HUNDRED)
        .round_dp_with_strategy(1, RoundingStrategy::MidpointAwayFromZero);
    Some(if p.is_zero() {
        "0.0".to_string()
    } else {
        format!("{p:.1}")
    })
}

/// Which lines a request keeps: the report's tag filter and the
/// ledger drill's "no tag of this group".
#[derive(Clone, Debug, Default)]
struct Lens {
    tags: Vec<TagId>,
    untagged_in: Option<BTreeSet<TagId>>,
    /// Lines with no tag at all (an untagged cell's drill, S6).
    untagged: bool,
    /// The ledger's transaction types (the engine's names).
    types: Vec<String>,
    /// Line sizes |debit - credit| at least / at most (the demo's stand-in
    /// for the engine's amount filter).
    amount_min: Option<Decimal>,
    amount_max: Option<Decimal>,
}

impl Lens {
    fn of(req: &ReportRequest) -> Lens {
        Lens {
            tags: req.filters.tag_ids.clone(),
            untagged_in: req.filters.untagged_in_group.map(group_tags),
            untagged: req.filters.untagged,
            types: req.filters.txn_types.clone(),
            amount_min: req.filters.amount_min,
            amount_max: req.filters.amount_max,
        }
    }

    fn keeps(&self, p: &Post) -> bool {
        if !self.tags.is_empty() && !p.tag.is_some_and(|t| self.tags.contains(&t)) {
            return false;
        }
        if self.untagged && p.tag.is_some() {
            return false;
        }
        if !self.types.is_empty() && !self.types.contains(&p.txn_type) {
            return false;
        }
        if self.amount_min.is_some() || self.amount_max.is_some() {
            let size = (p.debit - p.credit).abs();
            if self.amount_min.is_some_and(|m| size < m)
                || self.amount_max.is_some_and(|m| size > m)
            {
                return false;
            }
        }
        match &self.untagged_in {
            Some(g) => !p.tag.is_some_and(|t| g.contains(&t)),
            None => true,
        }
    }
}

fn group_tags(g: TagGroupId) -> BTreeSet<TagId> {
    super::lists::tags()
        .into_iter()
        .filter(|t| t.fields.group_id == Some(g))
        .map(|t| t.id)
        .collect()
}

impl Books {
    /// The demo's chart and postings (voided transactions left out).
    pub(crate) fn load(d: &DemoState) -> Books {
        let mut chart = d.chart.clone().unwrap_or_else(super::chart);
        let system = cl_company::Sample::load().company.system_accounts;
        // The fixture's opening balances sit on an equity account the demo
        // chart doesn't list; the reports show it so the books balance.
        if !chart
            .iter()
            .any(|a| a.fields.account_number.as_deref() == Some(system.opening_balances.as_str()))
        {
            let n: u32 = system.opening_balances.parse().unwrap_or(30000);
            chart.push(Account {
                id: super::account_id(n),
                fields: cl_api::AccountFields {
                    name: "Opening Balance Equity".into(),
                    account_number: Some(system.opening_balances.clone()),
                    account_type: AccountType::Equity,
                    detail_type: None,
                    description: None,
                    parent_id: None,
                },
                is_active: true,
                is_system: true,
                system_tag: Some("opening_balance_equity".into()),
                balance: None,
                created_at: None,
                updated_at: None,
                card_last4: None,
            });
        }
        let mut by_number: BTreeMap<&str, &Account> = chart
            .iter()
            .filter_map(|a| a.fields.account_number.as_deref().map(|n| (n, a)))
            .collect();
        // The fixture's receivables number is the chart's Accounts Receivable.
        if let Some(ar) = chart
            .iter()
            .find(|a| a.system_tag.as_deref() == Some("accounts_receivable"))
        {
            by_number
                .entry(system.accounts_receivable.as_str())
                .or_insert(ar);
        }
        let named_tags: BTreeMap<String, TagId> = super::lists::tags()
            .into_iter()
            .map(|t| (t.fields.name, t.id))
            .collect();
        let mut posts = Vec::new();
        let mut txns = d.ledger.transactions();
        txns.retain(|t| !d.ledger.is_voided(t.id));
        txns.sort_by(|a, b| a.date.cmp(&b.date).then(a.id.cmp(&b.id)));
        for t in txns {
            for (i, l) in t.lines.iter().enumerate() {
                let Some(a) = by_number.get(l.account.as_str()) else {
                    continue;
                };
                let own = l.tag.as_ref().and_then(|n| named_tags.get(n).copied());
                let tag = own.or_else(|| {
                    (t.kind == "invoice" && a.fields.account_type == AccountType::Revenue)
                        .then(|| route_tag(t.id))
                });
                posts.push(Post {
                    date: t.date,
                    account: a.id,
                    debit: l.debit.as_decimal(),
                    credit: l.credit.as_decimal(),
                    tag,
                    txn: txn_id(t.id),
                    txn_type: wire_type(&t.kind),
                    number: t.number.clone(),
                    contact: t.contact.clone(),
                    memo: l.memo.clone().unwrap_or_else(|| t.memo.clone()),
                    line: i,
                });
            }
        }
        Books { chart, posts }
    }

    /// The natural-sign total of `account` over `from..=to`, every line.
    pub(crate) fn natural_total(
        &self,
        account: &Account,
        from: CalendarDate,
        to: CalendarDate,
    ) -> Decimal {
        self.net(account, Some(from), to, &|_| true)
    }

    fn account(&self, id: AccountId) -> Option<&Account> {
        self.chart.iter().find(|a| a.id == id)
    }

    /// The natural-sign total of `account` over `from..=to` (`None`: from
    /// the first entry) of the lines `keep` takes.
    fn net(
        &self,
        account: &Account,
        from: Option<CalendarDate>,
        to: CalendarDate,
        keep: &dyn Fn(&Post) -> bool,
    ) -> Decimal {
        self.posts
            .iter()
            .filter(|p| p.account == account.id && p.date <= to && from.is_none_or(|f| p.date >= f))
            .filter(|p| keep(p))
            .map(|p| natural(account.fields.account_type, p.debit, p.credit))
            .sum()
    }

    /// The accounts of `types` placed as the engine's account tree: those
    /// `shown` keeps plus their parents, parents first, in number order;
    /// with depth and whether each has children.
    pub(crate) fn tree(
        &self,
        types: &[AccountType],
        shown: &dyn Fn(&Account) -> bool,
    ) -> Vec<(&Account, usize, bool)> {
        let of_type: Vec<&Account> = self
            .chart
            .iter()
            .filter(|a| types.contains(&a.fields.account_type))
            .collect();
        let mut keep: BTreeSet<AccountId> = BTreeSet::new();
        for a in of_type.iter().filter(|a| shown(a)) {
            let mut cur = Some(*a);
            while let Some(c) = cur {
                if !keep.insert(c.id) {
                    break;
                }
                cur = c.fields.parent_id.and_then(|p| self.account(p));
            }
        }
        let key = |a: &Account| {
            (
                a.fields.account_number.clone().unwrap_or_default(),
                a.fields.name.clone(),
            )
        };
        let mut placed = Vec::new();
        let mut roots: Vec<&Account> = self
            .chart
            .iter()
            .filter(|a| keep.contains(&a.id))
            .filter(|a| a.fields.parent_id.is_none_or(|p| !keep.contains(&p)))
            .collect();
        roots.sort_by_key(|a| key(a));
        for r in roots {
            walk(self, &keep, r, 0, &key, &mut placed);
        }
        placed
    }

    /// Every descendant of `id` (itself included).
    pub(crate) fn subtree(&self, id: AccountId) -> Vec<AccountId> {
        let mut out = vec![id];
        let mut i = 0;
        while let Some(cur) = out.get(i).copied() {
            out.extend(
                self.chart
                    .iter()
                    .filter(|a| a.fields.parent_id == Some(cur))
                    .map(|a| a.id),
            );
            i += 1;
        }
        out
    }
}

/// Places `a` and its kept children, depth first in number order.
fn walk<'a>(
    b: &'a Books,
    keep: &BTreeSet<AccountId>,
    a: &'a Account,
    depth: usize,
    key: &dyn Fn(&Account) -> (String, String),
    out: &mut Vec<(&'a Account, usize, bool)>,
) {
    let mut kids: Vec<&Account> = b
        .chart
        .iter()
        .filter(|c| c.fields.parent_id == Some(a.id) && keep.contains(&c.id))
        .collect();
    kids.sort_by_key(|c| key(c));
    out.push((a, depth, !kids.is_empty()));
    for k in kids {
        walk(b, keep, k, depth + 1, key, out);
    }
}

fn meta(req: &ReportRequest, from: Option<CalendarDate>, to: CalendarDate) -> Value {
    let basis = req.basis.unwrap_or(Basis::Accrual).as_str();
    match from {
        Some(f) => json!({"exact": true, "basis": basis, "cashBasisRules": null,
            "reportSchemaVersion": "2", "from": f.to_wire(), "to": to.to_wire()}),
        None => json!({"exact": true, "basis": basis, "cashBasisRules": null,
            "reportSchemaVersion": "2", "asOf": to.to_wire()}),
    }
}

fn entry_base(a: &Account, depth: usize) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert("accountId".into(), json!(a.id.to_string()));
    m.insert("accountNumber".into(), json!(a.fields.account_number));
    m.insert("name".into(), json!(a.fields.name));
    m.insert(
        "parentAccountId".into(),
        json!(a.fields.parent_id.map(|p| p.to_string())),
    );
    m.insert("depth".into(), json!(depth));
    m
}

/// The fiscal year's first day for `d` (the demo company's year starts in
/// January) unless the request names it.
fn year_start(req: &ReportRequest, d: CalendarDate) -> CalendarDate {
    req.fiscal_year_start
        .filter(|f| *f <= d)
        .unwrap_or_else(|| d.start_of_year())
}

// --- Profit & Loss ---------------------------------------------------------

/// `/reports/profit-loss?exact=1` (no columns).
fn profit_loss(b: &Books, req: &ReportRequest, r: DateRange) -> Value {
    let lens = Lens::of(req);
    let keep = |p: &Post| lens.keeps(p);
    let mut out = serde_json::Map::new();
    let mut totals: BTreeMap<&str, Decimal> = BTreeMap::new();
    for (key, types) in PL_SECTIONS {
        let own = |a: &Account| b.net(a, Some(r.from), r.to, &keep);
        let placed = b.tree(types, &|a| !own(a).is_zero());
        let mut entries = Vec::new();
        let mut total = Decimal::ZERO;
        for (a, depth, kids) in &placed {
            let amount = own(a);
            total += amount;
            let mut e = entry_base(a, *depth);
            e.insert("amountExact".into(), json!(ex(amount)));
            if *kids {
                let rollup: Decimal = b
                    .subtree(a.id)
                    .into_iter()
                    .filter_map(|id| b.account(id))
                    .map(own)
                    .sum();
                e.insert("rollupExact".into(), json!(ex(rollup)));
            }
            entries.push(Value::Object(e));
        }
        totals.insert(key, total);
        out.insert(key.into(), Value::Array(entries));
    }
    let section_total = |k: &str| totals.get(k).copied().unwrap_or_default();
    let has_cogs = out
        .get("cogs")
        .and_then(Value::as_array)
        .is_some_and(|v| !v.is_empty());
    let has_other = ["otherRevenue", "otherExpenses"].iter().any(|k| {
        out.get(*k)
            .and_then(Value::as_array)
            .is_some_and(|v| !v.is_empty())
    });
    let gross = section_total("revenue") - section_total("cogs");
    let operating = gross - section_total("expenses");
    let net = section_total("revenue") + section_total("otherRevenue")
        - section_total("cogs")
        - section_total("expenses")
        - section_total("otherExpenses");
    for (k, total_key) in [
        ("revenue", "totalRevenueExact"),
        ("cogs", "totalCogsExact"),
        ("expenses", "totalExpensesExact"),
        ("otherRevenue", "totalOtherRevenueExact"),
        ("otherExpenses", "totalOtherExpensesExact"),
    ] {
        out.insert(total_key.into(), json!(ex(section_total(k))));
    }
    out.insert(
        "grossProfitExact".into(),
        if has_cogs {
            json!(ex(gross))
        } else {
            Value::Null
        },
    );
    out.insert(
        "operatingIncomeExact".into(),
        if has_cogs || has_other {
            json!(ex(operating))
        } else {
            Value::Null
        },
    );
    out.insert("netIncomeExact".into(), json!(ex(net)));
    out.insert("meta".into(), meta(req, Some(r.from), r.to));
    out.insert("startDate".into(), json!(r.from.to_wire()));
    out.insert("endDate".into(), json!(r.to.to_wire()));
    Value::Object(out)
}

/// Where a column's values come from (the engine's `ColDef.src`).
#[derive(Clone, Debug)]
enum Src {
    Sums(DateRange),
    SumOf(Vec<usize>),
    /// Only the lines of one tag of the compare group / not of the group.
    Tag(DateRange, Option<TagId>),
    Change(usize, usize),
    Pct(usize, usize),
}

#[derive(Clone, Debug)]
struct Col {
    head: Value,
    src: Src,
}

fn prior_range(r: DateRange, year: bool) -> Option<DateRange> {
    if year {
        return Some(DateRange {
            from: r.from.add_months(-12).ok()?,
            to: r.to.add_months(-12).ok()?,
        });
    }
    let len = r.from.days_until(r.to) + 1;
    Some(DateRange {
        from: r.from.add_days(-len).ok()?,
        to: r.from.add_days(-1).ok()?,
    })
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn fmt_day(d: CalendarDate) -> String {
    let m = MONTHS
        .get(usize::try_from(d.month()).unwrap_or(1).saturating_sub(1))
        .copied()
        .unwrap_or("");
    format!("{m} {}, {}", d.day(), d.year())
}

/// The engine's `rangeCaption` (Budget vs. Actual's columns too).
pub(crate) fn caption_of(r: DateRange) -> String {
    range_caption(r)
}

/// The engine's `rangeCaption`.
fn range_caption(r: DateRange) -> String {
    if r.from.day() == 1
        && r.from.start_of_month() == r.to.start_of_month()
        && r.to == r.to.end_of_month()
    {
        let m = MONTHS
            .get(
                usize::try_from(r.from.month())
                    .unwrap_or(1)
                    .saturating_sub(1),
            )
            .copied()
            .unwrap_or("");
        return format!("{m} {}", r.from.year());
    }
    if r.from == r.from.start_of_year()
        && r.to == r.to.end_of_year()
        && r.from.year() == r.to.year()
    {
        return r.from.year().to_string();
    }
    format!("{} - {}", fmt_day(r.from), fmt_day(r.to))
}

fn ranges(req: &ReportRequest) -> Option<(Vec<DateRange>, bool)> {
    match &req.dates {
        ReportDates::Ranges(v) if !v.is_empty() => Some((v.clone(), true)),
        ReportDates::Range(r) => Some((vec![*r], false)),
        _ => None,
    }
}

/// The engine's column plan for the P&L (`report-columns.service.ts`).
fn pl_columns(req: &ReportRequest) -> Option<Vec<Col>> {
    let (rs, period_columns) = ranges(req)?;
    let main = DateRange {
        from: rs.first()?.from,
        to: rs.last()?.to,
    };
    let prior = if req.extras.contains(&Extra::PriorYear) {
        Some(true)
    } else if req.extras.contains(&Extra::PriorPeriod) {
        Some(false)
    } else {
        None
    };
    let drill = |r: DateRange| json!({"start_date": r.from.to_wire(), "end_date": r.to.to_wire()});
    let head = |key: &str, caption: &str, kind: &str, r: Option<DateRange>, d: Value| {
        json!({"key": key, "caption": caption, "kind": kind, "valueType": "amount",
            "from": r.map(|r| r.from.to_wire()), "to": r.map(|r| r.to.to_wire()), "drill": d})
    };
    let mut cols: Vec<Col> = Vec::new();
    // The engine's rule: a period's prior is its `prior_ranges` entry when
    // sent (B3), else the day count; the YTD column always the day count.
    let add_prior = |cols: &mut Vec<Col>,
                     base: usize,
                     r: DateRange,
                     explicit: Option<DateRange>|
     -> Option<()> {
        let year = prior?;
        let pr = match explicit {
            Some(e) if !year => e,
            _ => prior_range(r, year)?,
        };
        let key = cols.get(base)?.head.get("key")?.as_str()?.to_string();
        let mut h = head(
            &format!("{key}_prior"),
            if year { "Prior year" } else { "Prior period" },
            "prior",
            Some(pr),
            drill(pr),
        );
        h["baseKey"] = json!(key);
        let p = cols.len();
        cols.push(Col {
            head: h,
            src: Src::Sums(pr),
        });
        let mut c = head(
            &format!("{key}_change"),
            "$ Change",
            "change",
            None,
            Value::Null,
        );
        c["baseKey"] = json!(key);
        cols.push(Col {
            head: c,
            src: Src::Change(base, p),
        });
        let mut pc = head(
            &format!("{key}_pct_change"),
            "% Change",
            "pct_change",
            None,
            Value::Null,
        );
        pc["baseKey"] = json!(key);
        pc["valueType"] = json!("percent");
        cols.push(Col {
            head: pc,
            src: Src::Pct(base, p),
        });
        Some(())
    };
    if let Compare::ByTag { tag_group_id } = req.columns {
        for t in super::lists::tags()
            .into_iter()
            .filter(|t| t.fields.group_id == Some(tag_group_id) && t.is_active)
        {
            let caption = match &t.fields.tag_number {
                Some(n) => format!("{n} {}", t.fields.name),
                None => t.fields.name.clone(),
            };
            cols.push(Col {
                head: head(
                    &format!("t:{}", t.id),
                    &caption,
                    "tag",
                    Some(main),
                    json!({"tag_id": t.id.to_string()}),
                ),
                src: Src::Tag(main, Some(t.id)),
            });
        }
        cols.push(Col {
            head: head(
                "no_tag",
                "No tag in group",
                "no_tag",
                Some(main),
                json!({"tag_scope": "untagged_in_group", "tag_group_id": tag_group_id.to_string()}),
            ),
            src: Src::Tag(main, None),
        });
        cols.push(Col {
            head: head("total", "Total", "total", Some(main), drill(main)),
            src: Src::Sums(main),
        });
        return Some(cols);
    }
    if req.columns != Compare::None {
        return None;
    }
    let mut periods = Vec::new();
    for (i, r) in rs.iter().enumerate() {
        let idx = cols.len();
        periods.push(idx);
        let (key, caption, kind) = if period_columns {
            (format!("p{i}"), range_caption(*r), "period")
        } else {
            ("total".to_string(), "Total".to_string(), "total")
        };
        cols.push(Col {
            head: head(&key, &caption, kind, Some(*r), drill(*r)),
            src: Src::Sums(*r),
        });
        let explicit = req.prior_ranges.as_ref().and_then(|p| p.get(i)).copied();
        add_prior(&mut cols, idx, *r, explicit);
    }
    if period_columns && rs.len() > 1 {
        cols.push(Col {
            head: head("total", "Total", "total", Some(main), drill(main)),
            src: Src::SumOf(periods),
        });
    }
    if req.extras.contains(&Extra::Ytd) {
        let r = DateRange {
            from: year_start(req, main.to),
            to: main.to,
        };
        let idx = cols.len();
        cols.push(Col {
            head: head("ytd", "YTD", "ytd", Some(r), drill(r)),
            src: Src::Sums(r),
        });
        add_prior(&mut cols, idx, r, None);
    }
    Some(cols)
}

/// One row's values: amounts, `$ change` and `% change` (the engine's
/// `rederive`).
fn values(cols: &[Col], sums: &dyn Fn(&Src) -> Decimal) -> Vec<Option<Decimal>> {
    let mut v: Vec<Option<Decimal>> = cols
        .iter()
        .map(|c| match &c.src {
            Src::Sums(_) | Src::Tag(..) => Some(sums(&c.src)),
            _ => None,
        })
        .collect();
    rederive(cols, &mut v);
    v
}

fn rederive(cols: &[Col], v: &mut [Option<Decimal>]) {
    for (i, c) in cols.iter().enumerate() {
        let at = |j: usize, v: &[Option<Decimal>]| v.get(j).copied().flatten().unwrap_or_default();
        let x = match &c.src {
            Src::SumOf(ix) => Some(ix.iter().map(|j| at(*j, v)).sum()),
            Src::Change(cur, p) => Some(at(*cur, v) - at(*p, v)),
            _ => continue,
        };
        if let Some(slot) = v.get_mut(i) {
            *slot = x;
        }
    }
}

fn add(cols: &[Col], a: &[Option<Decimal>], b: &[Option<Decimal>]) -> Vec<Option<Decimal>> {
    let mut v: Vec<Option<Decimal>> = cols
        .iter()
        .enumerate()
        .map(|(i, c)| match c.src {
            Src::Pct(..) => None,
            _ => Some(
                a.get(i).copied().flatten().unwrap_or_default()
                    + b.get(i).copied().flatten().unwrap_or_default(),
            ),
        })
        .collect();
    rederive(cols, &mut v);
    v
}

fn wire(cols: &[Col], v: &[Option<Decimal>]) -> Value {
    Value::Array(
        cols.iter()
            .enumerate()
            .map(|(i, c)| {
                let x = v.get(i).copied().flatten().unwrap_or_default();
                match c.src {
                    Src::Pct(cur, p) => {
                        let cur = v.get(cur).copied().flatten().unwrap_or_default();
                        let p = v.get(p).copied().flatten().unwrap_or_default();
                        percent1(cur - p, p.abs()).map_or(Value::Null, Value::String)
                    }
                    _ => json!(ex(x)),
                }
            })
            .collect(),
    )
}

fn pct_of_income(cols: &[Col], v: &[Option<Decimal>], income: &[Option<Decimal>]) -> Value {
    Value::Array(
        cols.iter()
            .enumerate()
            .map(|(i, c)| match c.src {
                Src::Pct(..) | Src::Change(..) => Value::Null,
                _ => percent1(
                    v.get(i).copied().flatten().unwrap_or_default(),
                    income.get(i).copied().flatten().unwrap_or_default(),
                )
                .map_or(Value::Null, Value::String),
            })
            .collect(),
    )
}

fn any_non_zero(cols: &[Col], v: &[Option<Decimal>]) -> bool {
    cols.iter().enumerate().any(|(i, c)| {
        !matches!(c.src, Src::Pct(..) | Src::Change(..))
            && !v.get(i).copied().flatten().unwrap_or_default().is_zero()
    })
}

/// `/reports/profit-loss?exact=1` in the column layout (U-201).
fn profit_loss_columns(b: &Books, req: &ReportRequest, cols: &[Col]) -> Option<Value> {
    type Rows = Vec<(serde_json::Map<String, Value>, Vec<Option<Decimal>>)>;
    let (rs, _) = ranges(req)?;
    let main = DateRange {
        from: rs.first()?.from,
        to: rs.last()?.to,
    };
    let lens = Lens::of(req);
    let split = req.tag_split.map(|g| {
        let tags: Vec<cl_api::Tag> = super::lists::tags()
            .into_iter()
            .filter(|t| t.fields.group_id == Some(g) && t.is_active)
            .collect();
        (g, tags)
    });
    let group_of_compare = match req.columns {
        Compare::ByTag { tag_group_id } => Some(group_tags(tag_group_id)),
        _ => None,
    };
    let row_values = |a: &Account, pick: &dyn Fn(&Post) -> bool| {
        values(cols, &|src| match src {
            Src::Sums(r) => b.net(a, Some(r.from), r.to, &|p| lens.keeps(p) && pick(p)),
            Src::Tag(r, Some(t)) => b.net(a, Some(r.from), r.to, &|p| {
                lens.keeps(p) && pick(p) && p.tag == Some(*t)
            }),
            Src::Tag(r, None) => b.net(a, Some(r.from), r.to, &|p| {
                lens.keeps(p)
                    && pick(p)
                    && !p
                        .tag
                        .is_some_and(|t| group_of_compare.as_ref().is_some_and(|g| g.contains(&t)))
            }),
            _ => Decimal::ZERO,
        })
    };
    let zero: Vec<Option<Decimal>> = add(cols, &[], &[]);
    let mut sections: Vec<(&str, Rows, Vec<Option<Decimal>>)> = Vec::new();
    for (key, types) in PL_SECTIONS {
        let own: BTreeMap<AccountId, Vec<Option<Decimal>>> = b
            .chart
            .iter()
            .filter(|a| types.contains(&a.fields.account_type))
            .map(|a| (a.id, row_values(a, &|_| true)))
            .filter(|(_, v)| any_non_zero(cols, v))
            .collect();
        let placed = b.tree(types, &|a| own.contains_key(&a.id));
        let mut total = zero.clone();
        for v in own.values() {
            total = add(cols, &total, v);
        }
        let mut rows = Vec::new();
        for (a, depth, kids) in &placed {
            let mine = own.get(&a.id).cloned().unwrap_or_else(|| zero.clone());
            let mut base = entry_base(a, *depth);
            if *kids {
                let mut r = zero.clone();
                for id in b.subtree(a.id) {
                    if let Some(v) = own.get(&id) {
                        r = add(cols, &r, v);
                    }
                }
                base.insert("rollupValuesExact".into(), wire(cols, &r));
            }
            let Some((g, tags)) = &split else {
                let mut e = base;
                e.insert("rowKind".into(), json!("account"));
                e.insert("valuesExact".into(), wire(cols, &mine));
                e.insert("drill".into(), json!({"account_ids": a.id.to_string()}));
                rows.push((e, mine));
                continue;
            };
            let in_group: BTreeSet<TagId> = tags.iter().map(|t| t.id).collect();
            let untagged = if own.contains_key(&a.id) {
                row_values(a, &|p| !p.tag.is_some_and(|t| in_group.contains(&t)))
            } else {
                zero.clone()
            };
            let mut e = base.clone();
            e.insert("rowKind".into(), json!("account"));
            for k in ["tagId", "tagNumber", "tagName"] {
                e.insert(k.into(), Value::Null);
            }
            e.insert("valuesExact".into(), wire(cols, &untagged));
            e.insert(
                "drill".into(),
                json!({"account_ids": a.id.to_string(), "tag_scope": "untagged_in_group",
                    "tag_group_id": g.to_string()}),
            );
            rows.push((e, untagged));
            if own.contains_key(&a.id) {
                for t in tags {
                    let v = row_values(a, &|p| p.tag == Some(t.id));
                    if !any_non_zero(cols, &v) {
                        continue;
                    }
                    let mut e = base.clone();
                    e.remove("rollupValuesExact");
                    e.insert("rowKind".into(), json!("tag"));
                    e.insert("tagId".into(), json!(t.id.to_string()));
                    e.insert("tagNumber".into(), json!(t.fields.tag_number));
                    e.insert("tagName".into(), json!(t.fields.name));
                    e.insert("valuesExact".into(), wire(cols, &v));
                    e.insert(
                        "drill".into(),
                        json!({"account_ids": a.id.to_string(), "tag_id": t.id.to_string()}),
                    );
                    rows.push((e, v));
                }
            }
            let mut e = base;
            e.remove("rollupValuesExact");
            e.insert("rowKind".into(), json!("account_total"));
            for k in ["tagId", "tagNumber", "tagName"] {
                e.insert(k.into(), Value::Null);
            }
            e.insert("valuesExact".into(), wire(cols, &mine));
            e.insert("drill".into(), json!({"account_ids": a.id.to_string()}));
            rows.push((e, mine));
        }
        sections.push((key, rows, total));
    }
    let total_of = |k: &str| {
        sections
            .iter()
            .find(|s| s.0 == k)
            .map_or_else(|| zero.clone(), |s| s.2.clone())
    };
    let combine = |f: &dyn Fn(usize) -> Decimal| {
        let mut v: Vec<Option<Decimal>> = cols
            .iter()
            .enumerate()
            .map(|(i, c)| match c.src {
                Src::Pct(..) => None,
                _ => Some(f(i)),
            })
            .collect();
        rederive(cols, &mut v);
        v
    };
    let at = |v: &[Option<Decimal>], i: usize| v.get(i).copied().flatten().unwrap_or_default();
    let (income, cost, spent, other_rev, other_exp) = (
        total_of("revenue"),
        total_of("cogs"),
        total_of("expenses"),
        total_of("otherRevenue"),
        total_of("otherExpenses"),
    );
    let has = |k: &str| sections.iter().any(|s| s.0 == k && !s.1.is_empty());
    let has_cogs = has("cogs");
    let has_other = has("otherRevenue") || has("otherExpenses");
    let gross = combine(&|i| at(&income, i) - at(&cost, i));
    let operating = if has_cogs {
        combine(&|i| at(&gross, i) - at(&spent, i))
    } else {
        combine(&|i| at(&income, i) - at(&spent, i))
    };
    let net = combine(&|i| {
        at(&income, i) + at(&other_rev, i) - at(&cost, i) - at(&spent, i) - at(&other_exp, i)
    });
    let total_rows: [(&str, Option<&Vec<Option<Decimal>>>); 8] = [
        ("totalRevenue", Some(&income)),
        ("totalCogs", Some(&cost)),
        ("totalExpenses", Some(&spent)),
        ("totalOtherRevenue", Some(&other_rev)),
        ("totalOtherExpenses", Some(&other_exp)),
        ("grossProfit", has_cogs.then_some(&gross)),
        (
            "operatingIncome",
            (has_cogs || has_other).then_some(&operating),
        ),
        ("netIncome", Some(&net)),
    ];
    let totals: serde_json::Map<String, Value> = total_rows
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.map_or(Value::Null, |v| wire(cols, v))))
        .collect();
    let pct = req.extras.contains(&Extra::PctOfIncome);
    let mut out = serde_json::Map::new();
    for (key, rows, _) in sections {
        let rows: Vec<Value> = rows
            .into_iter()
            .map(|(mut e, v)| {
                if pct {
                    e.insert("pctOfIncomeExact".into(), pct_of_income(cols, &v, &income));
                }
                Value::Object(e)
            })
            .collect();
        out.insert(key.into(), Value::Array(rows));
    }
    if pct {
        let p: serde_json::Map<String, Value> = total_rows
            .iter()
            .map(|(k, v)| {
                let p = v.map_or(Value::Null, |v| pct_of_income(cols, v, &income));
                ((*k).to_string(), p)
            })
            .collect();
        out.insert("pctOfIncomeTotalsExact".into(), Value::Object(p));
    }
    out.insert("totalsExact".into(), Value::Object(totals));
    out.insert("layout".into(), json!("columns"));
    out.insert(
        "columns".into(),
        Value::Array(cols.iter().map(|c| c.head.clone()).collect()),
    );
    out.insert(
        "tagSplit".into(),
        split.as_ref().map_or(Value::Null, |(g, tags)| {
            json!({"tagGroupId": g.to_string(), "tags": tags.iter().map(|t| json!({
                "tagId": t.id.to_string(), "tagNumber": t.fields.tag_number, "tagName": t.fields.name,
            })).collect::<Vec<_>>()})
        }),
    );
    out.insert(
        "drillTarget".into(),
        json!({"path": "/reports/general-ledger", "amount": "netChangeExact",
            "params": {"basis": req.basis.unwrap_or(Basis::Accrual).as_str()}}),
    );
    out.insert("meta".into(), meta(req, Some(main.from), main.to));
    out.insert("startDate".into(), json!(main.from.to_wire()));
    out.insert("endDate".into(), json!(main.to.to_wire()));
    Some(Value::Object(out))
}

// --- Balance Sheet, Trial Balance -------------------------------------------

const BS_SECTIONS: [(&str, AccountType, &str); 3] = [
    ("assets", AccountType::Asset, "totalAssetsExact"),
    (
        "liabilities",
        AccountType::Liability,
        "totalLiabilitiesExact",
    ),
    ("equity", AccountType::Equity, "totalEquityExact"),
];

/// Net income over `from..=to` (`None`: from the first entry).
fn net_income(b: &Books, from: Option<CalendarDate>, to: CalendarDate, lens: &Lens) -> Decimal {
    b.chart
        .iter()
        .filter(|a| is_pl(a.fields.account_type))
        .map(|a| {
            let n = b.net(a, from, to, &|p| lens.keeps(p));
            if debit_normal(a.fields.account_type) {
                -n
            } else {
                n
            }
        })
        .sum()
}

/// `/reports/balance-sheet?exact=1` as of `to`.
fn balance_sheet(b: &Books, req: &ReportRequest, to: CalendarDate) -> Value {
    let lens = Lens::of(req);
    let fy = year_start(req, to);
    let mut out = serde_json::Map::new();
    let mut totals: Vec<Decimal> = Vec::new();
    for (key, t, total_key) in BS_SECTIONS {
        let bal = |a: &Account| b.net(a, None, to, &|p| lens.keeps(p));
        let placed = b.tree(&[t], &|a| !bal(a).is_zero());
        let mut entries = Vec::new();
        let mut total = Decimal::ZERO;
        for (a, depth, kids) in &placed {
            let v = bal(a);
            total += v;
            let mut e = entry_base(a, *depth);
            e.insert("balanceExact".into(), json!(ex(v)));
            if *kids {
                let r: Decimal = b
                    .subtree(a.id)
                    .into_iter()
                    .filter_map(|id| b.account(id))
                    .map(bal)
                    .sum();
                e.insert("rollupExact".into(), json!(ex(r)));
            }
            entries.push(Value::Object(e));
        }
        if t == AccountType::Equity {
            let prior = fy
                .add_days(-1)
                .map_or(Decimal::ZERO, |d| net_income(b, None, d, &lens));
            let current = net_income(b, Some(fy), to, &lens);
            for (name, v) in [
                ("Retained Earnings (Prior Years)", prior),
                ("Net Income (Current Year)", current),
            ] {
                if v.is_zero() && name.starts_with("Retained") {
                    continue;
                }
                total += v;
                entries.push(json!({"accountId": null, "accountNumber": null, "name": name,
                    "parentAccountId": null, "depth": 0, "balanceExact": ex(v), "calculated": true}));
            }
        }
        totals.push(total);
        out.insert(key.into(), Value::Array(entries));
        out.insert(total_key.into(), json!(ex(total)));
    }
    let le: Decimal = totals.iter().skip(1).sum();
    out.insert("totalLiabilitiesAndEquityExact".into(), json!(ex(le)));
    out.insert("meta".into(), meta(req, None, to));
    out.insert("asOfDate".into(), json!(to.to_wire()));
    Value::Object(out)
}

/// `/reports/trial-balance?exact=1` at `to` (income and expenses from the
/// fiscal year's start).
fn trial_balance(b: &Books, req: &ReportRequest, to: CalendarDate) -> Value {
    let lens = Lens::of(req);
    let fy = year_start(req, to);
    let mut rows = Vec::new();
    let (mut dr, mut cr) = (Decimal::ZERO, Decimal::ZERO);
    let mut chart: Vec<&Account> = b.chart.iter().collect();
    chart.sort_by_key(|a| a.fields.account_number.clone().unwrap_or_default());
    for a in chart {
        let from = is_pl(a.fields.account_type).then_some(fy);
        let (d, c) = b
            .posts
            .iter()
            .filter(|p| p.account == a.id && p.date <= to && from.is_none_or(|f| p.date >= f))
            .filter(|p| lens.keeps(p))
            .fold((Decimal::ZERO, Decimal::ZERO), |(d, c), p| {
                (d + p.debit, c + p.credit)
            });
        let net = d - c;
        if net.is_zero() {
            continue;
        }
        let (debit, credit) = if net.is_sign_positive() {
            (net, Decimal::ZERO)
        } else {
            (Decimal::ZERO, -net)
        };
        dr += debit;
        cr += credit;
        rows.push(
            json!({"accountId": a.id.to_string(), "id": a.id.to_string(),
            "account_number": a.fields.account_number, "name": a.fields.name,
            "account_type": a.fields.account_type.as_str(), "depth": 0, "parentAccountId": null,
            "debitExact": ex(debit), "creditExact": ex(credit), "netExact": ex(net)}),
        );
    }
    json!({"data": rows, "totalDebitsExact": ex(dr), "totalCreditsExact": ex(cr),
        "meta": meta(req, Some(fy.min(to)), to), "startDate": fy.min(to).to_wire(), "endDate": to.to_wire()})
}

// --- General Ledger ----------------------------------------------------------

/// `/reports/general-ledger?exact=1` over `r` (the account, tag and
/// "no tag of the group" filters a drill carries).
fn general_ledger(b: &Books, req: &ReportRequest, r: DateRange) -> Value {
    let lens = Lens::of(req);
    let picked = &req.filters.account_ids;
    let mut chart: Vec<&Account> = b
        .chart
        .iter()
        .filter(|a| picked.is_empty() || picked.contains(&a.id))
        .collect();
    chart.sort_by_key(|a| a.fields.account_number.clone().unwrap_or_default());
    let fy = year_start(req, r.from);
    let (mut td, mut tc) = (Decimal::ZERO, Decimal::ZERO);
    let mut count = 0usize;
    let mut accounts = Vec::new();
    for a in chart {
        let t = a.fields.account_type;
        let begin_from = is_pl(t).then_some(fy);
        let begin = r.from.add_days(-1).map_or(Decimal::ZERO, |end| {
            if begin_from.is_some_and(|f| f > end) {
                Decimal::ZERO
            } else {
                b.net(a, begin_from, end, &|p| lens.keeps(p))
            }
        });
        let lines: Vec<&Post> = b
            .posts
            .iter()
            .filter(|p| p.account == a.id && p.date >= r.from && p.date <= r.to && lens.keeps(p))
            .collect();
        if picked.is_empty() && lines.is_empty() && begin.is_zero() {
            continue;
        }
        let mut running = begin;
        let (mut pd, mut pc) = (Decimal::ZERO, Decimal::ZERO);
        let tags = super::lists::tags();
        let rows: Vec<Value> = lines
            .iter()
            .map(|p| {
                running += natural(t, p.debit, p.credit);
                pd += p.debit;
                pc += p.credit;
                let tag_name = p
                    .tag
                    .and_then(|id| tags.iter().find(|x| x.id == id))
                    .map(|x| x.fields.name.clone());
                json!({"lineId": format!("{}:{}", p.txn, p.line), "transactionId": p.txn.to_string(),
                    "date": p.date.to_wire(), "txnType": p.txn_type, "txnNumber": p.number,
                    "contactName": p.contact, "description": p.memo,
                    "tagId": p.tag.map(|x| x.to_string()), "tagName": tag_name,
                    "debitExact": ex(p.debit), "creditExact": ex(p.credit),
                    "runningBalanceExact": ex(running)})
            })
            .collect();
        count += rows.len();
        td += pd;
        tc += pc;
        accounts.push(
            json!({"accountId": a.id.to_string(), "id": a.id.to_string(),
            "accountNumber": a.fields.account_number, "name": a.fields.name,
            "accountType": t.as_str(),
            "normalBalance": if debit_normal(t) { "debit" } else { "credit" },
            "beginningBalanceExact": ex(begin), "periodDebitsExact": ex(pd),
            "periodCreditsExact": ex(pc), "endingBalanceExact": ex(running),
            "netChangeExact": ex(running - begin), "lines": rows}),
        );
    }
    json!({"accounts": accounts, "totalDebitsExact": ex(td), "totalCreditsExact": ex(tc),
        "rowCount": count, "truncated": false, "meta": meta(req, Some(r.from), r.to),
        "startDate": r.from.to_wire(), "endDate": r.to.to_wire(), "fiscalYearStart": fy.to_wire()})
}

/// The demo's answer to a statement or ledger request, through the live
/// adapters. `Local(Unsupported)` for an option the demo doesn't play.
pub(crate) fn statement(d: &DemoState, req: &ReportRequest) -> Result<ReportTable, ApiError> {
    use cl_api::resources::reports::run::adapt_exact;
    use cl_core::report::Source;
    let unsupported = ApiError::Local(LocalValidation::Unsupported);
    let span = req
        .dates
        .span()
        .ok_or(ApiError::Local(LocalValidation::BadRange))?;
    // The demo's lines carry no client: it can't play the "No client" or
    // "All others" ledgers (S6), so those cells draw no link.
    if req.filters.no_contact || req.filters.other_contacts_than.is_some() {
        return Err(unsupported);
    }
    let b = Books::load(d);
    let columns = matches!(req.dates, ReportDates::Ranges(_))
        || !req.extras.is_empty()
        || req.columns != Compare::None
        || req.tag_split.is_some();
    let v = match req.source {
        Source::ProfitLoss if columns => {
            let cols = pl_columns(req).ok_or(unsupported)?;
            profit_loss_columns(&b, req, &cols).ok_or(ApiError::Internal)?
        }
        Source::ProfitLoss => profit_loss(&b, req, span),
        Source::BalanceSheet if !columns => balance_sheet(&b, req, span.to),
        Source::TrialBalance if !columns => trial_balance(&b, req, span.to),
        Source::GeneralLedger if !columns => general_ledger(&b, req, span),
        _ => return Err(unsupported),
    };
    let mut table = adapt_exact(req, &v)?;
    // What `Reports::run` adds from the chart's roles (B2, S5).
    use cl_api::resources::reports::adapters::statements;
    if statements::wants_roles(req, &v) {
        let roles = statements::AccountRoles::of(&b.chart);
        statements::finish_balances(req, &v, &mut table, Some(&roles));
    }
    Ok(table)
}

#[cfg(test)]
#[path = "reports_tests.rs"]
mod tests;
