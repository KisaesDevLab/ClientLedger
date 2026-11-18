//! The guided spot-check list (Q-144), saved with the conversion record.
//!
//! After the tie-out the owner compares a few things with `QuickBooks`:
//! each bank balance, the three largest open invoices and bills, sales tax
//! and payroll liabilities, net income year to date, inventory, and five
//! sample transactions picked from the batch. The sample is seeded by the
//! batch id with fixed integer arithmetic, so the same batch always shows
//! the same five (no `rand`, no `DefaultHasher`).
//!
//! Amounts are only compared to rank open items (never added up).

use cl_core::Money;

/// What an item asks the owner to check. Text: `imports.spot.<key>`, with
/// the item's `subject` (an account, document or transaction) filled in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A bank or card balance.
    Bank,
    /// One of the largest open invoices.
    OpenInvoice,
    /// One of the largest unpaid bills.
    UnpaidBill,
    /// Sales tax owed.
    SalesTax,
    /// Payroll liabilities.
    Payroll,
    /// Net income for the year to date.
    NetIncome,
    /// Inventory on hand and its value.
    Inventory,
    /// A sample transaction.
    Sample,
}

impl Kind {
    /// A stable key.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Kind::Bank => "bank",
            Kind::OpenInvoice => "ar",
            Kind::UnpaidBill => "ap",
            Kind::SalesTax => "sales_tax",
            Kind::Payroll => "payroll",
            Kind::NetIncome => "net_income",
            Kind::Inventory => "inventory",
            Kind::Sample => "sample",
        }
    }
}

/// An account to check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountRef {
    /// Stable id (the account id).
    pub id: String,
    /// Its name.
    pub name: String,
}

/// An open invoice or bill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenDoc {
    /// The source file row (stable key).
    pub source_row: u64,
    /// Document number.
    pub number: String,
    /// Customer or vendor.
    pub name: String,
    /// Open balance (engine's).
    pub open: Money,
}

/// A transaction the batch created (the engine's list), a sample candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Txn {
    /// Its id.
    pub id: String,
    /// The engine's type (`invoice`, `bill`, `journal_entry`, …): saved in
    /// the key so the sample opens in its form from a reopened list.
    pub txn_type: String,
    /// What the item names (number, date, name).
    pub subject: String,
}

/// What the generator reads.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Input {
    /// Bank and card accounts.
    pub banks: Vec<AccountRef>,
    /// Open invoices.
    pub open_invoices: Vec<OpenDoc>,
    /// Unpaid bills.
    pub unpaid_bills: Vec<OpenDoc>,
    /// The sales tax liability account, when there is one.
    pub sales_tax: Option<AccountRef>,
    /// The payroll liabilities account, when there is one.
    pub payroll: Option<AccountRef>,
    /// The company has inventory.
    pub inventory: bool,
    /// The batch's transactions (any order).
    pub transactions: Vec<Txn>,
    /// The batch id (seeds the sample).
    pub batch_id: u128,
}

/// One item of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Stable key (saved; `bank:<id>`, `ar:<row>`, `sample:<id>`, …).
    pub key: String,
    /// What to check.
    pub kind: Kind,
    /// The account, document or transaction named in the text.
    pub subject: Option<String>,
    /// Ticked.
    pub done: bool,
    /// The owner's note.
    pub note: String,
}

fn item(kind: Kind, id: &str, subject: Option<String>) -> Item {
    Item {
        key: if id.is_empty() {
            kind.key().to_string()
        } else {
            format!("{}:{id}", kind.key())
        },
        kind,
        subject,
        done: false,
        note: String::new(),
    }
}

/// How many open items of each side and how many samples.
pub const LARGEST: usize = 3;
/// Sample transactions.
pub const SAMPLES: usize = 5;

/// The three largest by open balance (ties by row, for a stable list).
fn largest(docs: &[OpenDoc]) -> Vec<&OpenDoc> {
    let mut v: Vec<&OpenDoc> = docs.iter().filter(|d| d.open.is_positive()).collect();
    v.sort_by(|a, b| b.open.cmp(&a.open).then(a.source_row.cmp(&b.source_row)));
    v.truncate(LARGEST);
    v
}

/// splitmix64: a fixed, documented integer mix (stable across releases).
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Up to [`SAMPLES`] transactions picked by `seed` from `ids` (sorted
/// first, so the order the engine listed them in does not matter).
#[must_use]
pub fn sample(ids: &[String], seed: u128) -> Vec<String> {
    let mut pool: Vec<String> = ids.to_vec();
    pool.sort();
    pool.dedup();
    // Fold the 128-bit id into 64 bits (truncation intended).
    #[allow(clippy::cast_possible_truncation, reason = "folding the halves")]
    let mut state = (seed as u64) ^ ((seed >> 64) as u64);
    let mut out = Vec::new();
    while out.len() < SAMPLES && !pool.is_empty() {
        state = mix(state);
        let len = u64::try_from(pool.len()).unwrap_or(u64::MAX);
        let i = usize::try_from(state % len).unwrap_or(0);
        out.push(pool.swap_remove(i));
    }
    out
}

/// The list for a batch.
#[must_use]
pub fn generate(input: &Input) -> Vec<Item> {
    let mut out = Vec::new();
    for b in &input.banks {
        out.push(item(Kind::Bank, &b.id, Some(b.name.clone())));
    }
    for d in largest(&input.open_invoices) {
        out.push(item(
            Kind::OpenInvoice,
            &d.source_row.to_string(),
            Some(format!("{} {}", d.number, d.name)),
        ));
    }
    for d in largest(&input.unpaid_bills) {
        out.push(item(
            Kind::UnpaidBill,
            &d.source_row.to_string(),
            Some(format!("{} {}", d.number, d.name)),
        ));
    }
    if let Some(a) = &input.sales_tax {
        out.push(item(Kind::SalesTax, &a.id, Some(a.name.clone())));
    }
    if let Some(a) = &input.payroll {
        out.push(item(Kind::Payroll, &a.id, Some(a.name.clone())));
    }
    out.push(item(Kind::NetIncome, "", None));
    if input.inventory {
        out.push(item(Kind::Inventory, "", None));
    }
    let ids: Vec<String> = input.transactions.iter().map(|t| t.id.clone()).collect();
    for id in sample(&ids, input.batch_id) {
        if let Some(t) = input.transactions.iter().find(|t| t.id == id) {
            out.push(item(
                Kind::Sample,
                &format!("{}:{}", t.txn_type, t.id),
                Some(t.subject.clone()),
            ));
        }
    }
    out
}

/// A sample item's transaction (`sample:<type>:<id>` keys): its type and
/// id, to open it in its form.
#[must_use]
pub fn sample_target(key: &str) -> Option<(&str, &str)> {
    let rest = key.strip_prefix("sample:")?;
    let (kind, id) = rest.split_once(':')?;
    (!kind.is_empty() && !id.is_empty()).then_some((kind, id))
}

/// Keeps the owner's ticks and notes from a saved list for the same keys.
#[must_use]
pub fn merge(generated: Vec<Item>, saved: &[(String, bool, String)]) -> Vec<Item> {
    generated
        .into_iter()
        .map(|mut i| {
            if let Some((_, done, note)) = saved.iter().find(|(k, ..)| *k == i.key) {
                i.done = *done;
                i.note.clone_from(note);
            }
            i
        })
        .collect()
}

/// Every item is ticked.
#[must_use]
pub fn complete(items: &[Item]) -> bool {
    items.iter().all(|i| i.done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(row: u64, n: &str, open: &str) -> OpenDoc {
        OpenDoc {
            source_row: row,
            number: n.into(),
            name: "Name".into(),
            open: Money::parse_wire(open).expect("money"),
        }
    }

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("txn-{i:03}")).collect()
    }

    fn txns(n: usize) -> Vec<Txn> {
        ids(n)
            .into_iter()
            .map(|id| Txn {
                subject: format!("Invoice {id}"),
                id,
                txn_type: "invoice".into(),
            })
            .collect()
    }

    fn input() -> Input {
        Input {
            banks: vec![AccountRef {
                id: "a1".into(),
                name: "Checking".into(),
            }],
            open_invoices: vec![
                doc(5, "1005", "3500.00"),
                doc(6, "1004", "2345.67"),
                doc(7, "CM-7", "-1234.00"),
                doc(8, "1009", "50.00"),
                doc(9, "1010", "99.00"),
            ],
            unpaid_bills: vec![doc(5, "F-131", "389.44"), doc(6, "VC-9", "-50.00")],
            sales_tax: Some(AccountRef {
                id: "a2".into(),
                name: "Sales Tax Payable".into(),
            }),
            payroll: None,
            inventory: true,
            transactions: txns(40),
            batch_id: 0x4b0c_4e8e_6f6e_4c1d_8a37_6c1f_4d8e_9b03,
        }
    }

    #[test]
    fn the_list_names_what_to_check() {
        let items = generate(&input());
        let keys: Vec<&str> = items.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(
            &keys[..6],
            ["bank:a1", "ar:5", "ar:6", "ar:9", "ap:5", "sales_tax:a2"]
        );
        assert!(keys.contains(&"net_income") && keys.contains(&"inventory"));
        assert!(!keys.iter().any(|k| k.starts_with("payroll")));
        assert_eq!(
            items.iter().filter(|i| i.kind == Kind::Sample).count(),
            SAMPLES
        );
        let mut unique = keys.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), keys.len());
        assert!(!complete(&items));
        let s = items.iter().find(|i| i.kind == Kind::Sample).expect("sample");
        let (kind, id) = sample_target(&s.key).expect("target");
        assert_eq!(kind, "invoice");
        assert!(id.starts_with("txn-"));
        assert_eq!(s.subject.as_deref(), Some(format!("Invoice {id}").as_str()));
        assert_eq!(sample_target("bank:a1"), None);
        assert_eq!(sample_target("sample:invoice:"), None);
    }

    #[test]
    fn the_sample_is_the_same_for_the_same_batch() {
        let a = sample(&ids(40), 7);
        let mut shuffled = ids(40);
        shuffled.reverse();
        assert_eq!(a, sample(&shuffled, 7), "order of the engine's list");
        assert_ne!(a, sample(&ids(40), 8), "another batch, another sample");
        assert_eq!(a.len(), SAMPLES);
        let mut d = a.clone();
        d.sort();
        d.dedup();
        assert_eq!(d.len(), SAMPLES, "no repeats");
        assert_eq!(sample(&ids(3), 7).len(), 3);
        assert_eq!(sample(&[], 7).len(), 0);
        // Pinned: a change to the mix would change saved lists.
        assert_eq!(mix(0), 0xE220_A839_7B1D_CDAF);
    }

    #[test]
    fn saved_ticks_and_notes_survive_a_rebuild() {
        let items = generate(&input());
        let saved = vec![
            ("bank:a1".to_string(), true, "Matches".to_string()),
            ("gone".to_string(), true, String::new()),
        ];
        let merged = merge(items, &saved);
        assert!(merged[0].done);
        assert_eq!(merged[0].note, "Matches");
        assert!(!merged[1].done);
        let all: Vec<Item> = merged
            .into_iter()
            .map(|mut i| {
                i.done = true;
                i
            })
            .collect();
        assert!(complete(&all));
    }
}
