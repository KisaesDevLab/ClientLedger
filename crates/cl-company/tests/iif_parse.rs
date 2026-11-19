//! The IIF file read back by a strict reader (P32: "IIF validated by a
//! parser test"), plus the golden file `fixtures/iif/sample.iif`.
//!
//! The reader checks what QuickBooks Desktop needs of the text: CRLF line
//! ends only, Windows-1252, a header row before each record kind with the
//! same number of fields as its rows, `TRNS`/`SPL`/`ENDTRNS` blocks that
//! sum to `0.00`, and every account and name a block uses defined in the
//! lists. Set `CL_BLESS=1` to rewrite the golden file.

#![allow(
    clippy::doc_markdown,
    clippy::too_many_lines,
    reason = "product names; one strict reader and one sample company"
)]

use std::collections::{BTreeMap, BTreeSet};

use cl_company::iif::encode::cp1252_char;
use cl_company::iif::{
    Books, ContactKind, Options, SourceAccount, SourceContact, SourceItem, SourceLine, SourceTxn,
    plan,
};
use cl_core::{CalendarDate, Money};
use rust_decimal::Decimal;

/// What the reader found.
#[derive(Debug, Default)]
struct Parsed {
    accounts: BTreeSet<String>,
    names: BTreeSet<String>,
    items: BTreeSet<String>,
    blocks: Vec<Vec<(String, String, Decimal)>>,
}

fn read_iif(bytes: &[u8]) -> Result<Parsed, String> {
    let text: String = bytes
        .iter()
        .map(|&b| cp1252_char(b).ok_or(format!("byte {b:#04x} is not Windows-1252 text")))
        .collect::<Result<_, _>>()?;
    if !text.ends_with("\r\n") {
        return Err("the file must end with CRLF".into());
    }
    let body = &text[..text.len() - 2];
    let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut p = Parsed::default();
    let mut open: Option<Vec<(String, String, Decimal)>> = None;
    for (n, line) in body.split("\r\n").enumerate() {
        if line.contains('\n') || line.contains('\r') {
            return Err(format!("line {n}: bare line break"));
        }
        if line.contains('"') {
            return Err(format!("line {n}: a quote"));
        }
        let fields: Vec<&str> = line.split('\t').collect();
        let kind = fields[0];
        if let Some(h) = kind.strip_prefix('!') {
            headers.insert(
                h.to_string(),
                fields[1..].iter().map(|s| (*s).to_string()).collect(),
            );
            continue;
        }
        let head = headers
            .get(kind)
            .ok_or(format!("line {n}: {kind} before its header"))?;
        if fields.len() - 1 != head.len() {
            return Err(format!(
                "line {n}: {} fields, header has {}",
                fields.len() - 1,
                head.len()
            ));
        }
        let get = |name: &str| -> String {
            head.iter()
                .position(|h| h == name)
                .map(|i| fields[i + 1].to_string())
                .unwrap_or_default()
        };
        match kind {
            "ACCNT" => {
                let name = get("NAME");
                if let Some((parent, _)) = name.rsplit_once(':')
                    && !p.accounts.contains(&parent.to_lowercase())
                {
                    return Err(format!("line {n}: sub-account before its parent"));
                }
                if !p.accounts.insert(name.to_lowercase()) {
                    return Err(format!("line {n}: account defined twice"));
                }
            }
            "CUST" | "VEND" => {
                if !p.names.insert(get("NAME").to_lowercase()) {
                    return Err(format!("line {n}: name defined twice"));
                }
            }
            "INVITEM" => {
                if !p.accounts.contains(&get("ACCNT").to_lowercase()) {
                    return Err(format!("line {n}: item account undefined"));
                }
                if !p.items.insert(get("NAME").to_lowercase()) {
                    return Err(format!("line {n}: item defined twice"));
                }
            }
            "TRNS" | "SPL" => {
                if (kind == "TRNS") == open.is_some() {
                    return Err(format!("line {n}: {kind} out of place"));
                }
                let amount_text = get("AMOUNT");
                let (_, frac) = amount_text
                    .split_once('.')
                    .ok_or(format!("line {n}: amount"))?;
                if frac.len() != 2 || amount_text == "-0.00" {
                    return Err(format!("line {n}: amount {amount_text}"));
                }
                let amount: Decimal = amount_text
                    .parse()
                    .map_err(|_| format!("line {n}: amount"))?;
                let date = get("DATE");
                if date.len() != 10 || &date[2..3] != "/" || &date[5..6] != "/" {
                    return Err(format!("line {n}: date {date}"));
                }
                if get("TRNSTYPE") != "GENERAL JOURNAL" {
                    return Err(format!("line {n}: type"));
                }
                open.get_or_insert_with(Vec::new)
                    .push((get("ACCNT"), get("NAME"), amount));
            }
            "ENDTRNS" => {
                let block = open
                    .take()
                    .ok_or(format!("line {n}: ENDTRNS without TRNS"))?;
                let sum: Decimal = block.iter().map(|l| l.2).sum();
                if !sum.is_zero() {
                    return Err(format!("line {n}: block sums to {sum}"));
                }
                for (a, name, _) in &block {
                    if !p.accounts.is_empty() && !p.accounts.contains(&a.to_lowercase()) {
                        return Err(format!("line {n}: account undefined"));
                    }
                    if !name.is_empty()
                        && !p.names.is_empty()
                        && !p.names.contains(&name.to_lowercase())
                    {
                        return Err(format!("line {n}: name undefined"));
                    }
                }
                p.blocks.push(block);
            }
            other => return Err(format!("line {n}: unknown record {other}")),
        }
    }
    if open.is_some() {
        return Err("a block without ENDTRNS".into());
    }
    Ok(p)
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn d(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("date")
}

fn acct(id: &str, name: &str, ty: &str, detail: &str, parent: Option<&str>) -> SourceAccount {
    SourceAccount {
        id: id.into(),
        name: name.into(),
        number: None,
        parent_id: parent.map(str::to_string),
        account_type: ty.into(),
        detail_type: Some(detail.into()),
        system_tag: None,
        description: None,
    }
}

fn ln(id: &str, account: &str, amount: &str) -> SourceLine {
    SourceLine {
        line_id: id.into(),
        account_id: account.into(),
        contact_id: None,
        amount: m(amount),
        memo: None,
    }
}

/// The synthetic sample company of the golden file (no real data).
fn sample() -> Books {
    let txn =
        |id: &str, date: &str, ty: &str, contact: Option<&str>, memo: &str, lines| SourceTxn {
            id: id.into(),
            date: d(date),
            txn_type: ty.into(),
            number: Some(id.trim_start_matches('t').into()),
            memo: Some(memo.into()),
            contact_id: contact.map(str::to_string),
            lines,
        };
    Books {
        accounts: vec![
            acct("bank", "Operating Checking", "asset", "bank", None),
            SourceAccount {
                system_tag: Some("accounts_receivable".into()),
                number: Some("1200".into()),
                ..acct(
                    "ar",
                    "Accounts Receivable",
                    "asset",
                    "accounts_receivable",
                    None,
                )
            },
            acct(
                "ap",
                "Accounts Payable",
                "liability",
                "accounts_payable",
                None,
            ),
            acct("card", "Company Card", "liability", "credit_card", None),
            acct("eq", "Owner Equity", "equity", "owners_equity", None),
            acct("inc", "Lawn Care", "revenue", "service", None),
            acct("exp", "Supplies", "expense", "office_supplies", None),
            acct("fuel", "Fuel", "expense", "travel", Some("exp")),
        ],
        contacts: vec![
            SourceContact {
                id: "c1".into(),
                name: "Maple Street HOA".into(),
                kind: ContactKind::Client,
            },
            SourceContact {
                id: "v1".into(),
                name: "Green Supply \u{2013} Joplin".into(),
                kind: ContactKind::Vendor,
            },
            SourceContact {
                id: "b1".into(),
                name: "Ridge Partners".into(),
                kind: ContactKind::Both,
            },
        ],
        items: vec![SourceItem {
            id: "i1".into(),
            name: "Mowing".into(),
            item_type: "service".into(),
            description: Some("Weekly mowing".into()),
            income_account_id: Some("inc".into()),
        }],
        txns: vec![
            txn(
                "t1001",
                "2026-01-02",
                "journal_entry",
                None,
                "Opening cash",
                vec![ln("1", "bank", "5000.00"), ln("2", "eq", "-5000.00")],
            ),
            txn(
                "t1002",
                "2026-01-15",
                "invoice",
                Some("c1"),
                "January mowing",
                vec![ln("3", "ar", "1250.00"), ln("4", "inc", "-1250.00")],
            ),
            txn(
                "t1003",
                "2026-01-20",
                "bill",
                Some("v1"),
                "Mulch \u{201C}premium\u{201D}",
                vec![
                    ln("5", "exp", "312.4949"),
                    ln("6", "fuel", "87.5051"),
                    ln("7", "ap", "-400.00"),
                ],
            ),
            txn(
                "t1004",
                "2026-02-01",
                "customer_payment",
                Some("b1"),
                "",
                vec![
                    ln("8", "bank", "300.00"),
                    SourceLine {
                        contact_id: Some("b1".into()),
                        ..ln("9", "ar", "-300.00")
                    },
                ],
            ),
            txn(
                "t1005",
                "2026-02-03",
                "expense",
                None,
                "Fuel",
                vec![ln("10", "fuel", "45.10"), ln("11", "card", "-45.10")],
            ),
        ],
    }
}

#[test]
fn the_sample_file_reads_back_strictly_and_matches_the_golden_file() {
    let p = plan(&sample(), Options::default()).expect("plan");
    let bytes = p.to_bytes();
    let parsed = read_iif(&bytes).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(parsed.blocks.len(), 5);
    assert_eq!(parsed.accounts.len(), 8);
    assert_eq!(
        parsed.names.len(),
        4,
        "the both-contact is a client and a vendor"
    );
    assert!(parsed.accounts.contains("supplies:fuel"));
    // The en dash and curly quotes are Windows-1252 bytes, not `?`.
    assert!(bytes.contains(&0x96) && bytes.contains(&0x93));
    assert_eq!(p.unmappable_chars, 0);

    let golden =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/iif/sample.iif");
    if std::env::var("CL_BLESS").as_deref() == Ok("1") {
        std::fs::write(&golden, &bytes).expect("bless");
    }
    let want = std::fs::read(&golden).expect("golden file (CL_BLESS=1 writes it)");
    assert!(
        want == bytes,
        "the file changed; review it and run with CL_BLESS=1"
    );
}

#[test]
fn the_reader_refuses_broken_files() {
    for bad in [
        &b"!CUST\tNAME\r\nCUST\tA\tB\r\n"[..],
        b"CUST\tA\r\n",
        b"!CUST\tNAME\r\nCUST\tA\n",
        b"!CUST\tNAME\r\nCUST\t\"A\"\r\n",
        b"!CUST\tNAME\r\nCUST\tA\x81\r\n",
    ] {
        assert!(read_iif(bad).is_err(), "{bad:?}");
    }
    let unbalanced = "!TRNS\tTRNSID\tTRNSTYPE\tDATE\tACCNT\tNAME\tAMOUNT\tDOCNUM\tMEMO\r\n\
        !SPL\tSPLID\tTRNSTYPE\tDATE\tACCNT\tNAME\tAMOUNT\tDOCNUM\tMEMO\r\n!ENDTRNS\r\n\
        TRNS\t\tGENERAL JOURNAL\t01/02/2026\tA\t\t1.00\t\t\r\n\
        SPL\t\tGENERAL JOURNAL\t01/02/2026\tB\t\t-0.99\t\t\r\nENDTRNS\r\n";
    assert!(
        read_iif(unbalanced.as_bytes())
            .unwrap_err()
            .contains("sums to")
    );
}
