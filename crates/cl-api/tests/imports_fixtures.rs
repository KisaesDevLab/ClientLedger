//! P31: the import fixtures agree with themselves and with their
//! `EXPECTED.toml` (plan §3). Every set is re-summed here in `rust_decimal`
//! with readers of its own (`support/imports_expect.rs`), independent of
//! cl-api and the engine:
//!
//! - opening trial balance + journals = reference trial balance, account by
//!   account (income statement accounts closed into retained earnings at
//!   each fiscal year end);
//! - A/R = the open invoices, A/P = the unpaid bills, Inventory Asset = the
//!   valuation;
//! - each altered set differs on exactly the accounts its expectation names;
//! - every feature plan §3 lists is present, QBD files are Latin-1, and no
//!   real-looking contact data is in any file.

#[path = "support/imports_expect.rs"]
mod imports_expect;

use std::collections::BTreeMap;

use imports_expect::{
    Balances, Expected, add, amount, amounts, ap_gl, ap_tb, as_of, bytes, debits, dec, diffs,
    expected, fiscal_year_start, generic_gl, generic_tb, latin1, leg_debits, net, net_income,
    qbd_account_types, qbd_journal, qbd_journal_total, qbd_list_balances, qbd_open_items, qbd_tb,
    qbd_total_debit, qbd_valuation, qbo_gl, qbo_numbers, qbo_tb, read_csv, roll_forward, section,
    set_dir, text,
};
use rust_decimal::Decimal;

const QBD: &str = "qbd/shoal-creek";

fn value<'a>(e: &'a Expected, sec: &str, key: &str) -> &'a str {
    section(e, sec)
        .get(key)
        .unwrap_or_else(|| panic!("[{sec}] {key} missing"))
}

/// The books of the Shoal Creek set as of the conversion date, from the
/// opening trial balance and both journals (journal-2025-2026 may be
/// replaced by an altered file).
fn shoal_creek_books(journal_2025: &str) -> Balances {
    let e = expected(QBD);
    let types = qbd_account_types(&text(QBD, "account-list.csv"));
    let mut legs = qbd_journal(&text(QBD, "journal-2024.csv"));
    legs.extend(qbd_journal(&latin1(
        &std::fs::read(journal_2025).expect("journal"),
    )));
    roll_forward(
        &qbd_tb(&text(QBD, "tb-opening-2023-12-31.csv")),
        value(&e, "company", "opening_tb_date"),
        &legs,
        value(&e, "company", "conversion_date"),
        &types,
        "3900",
        value(&e, "company", "fiscal_year_start_month")
            .parse()
            .expect("month"),
    )
}

fn base_journal_2025() -> String {
    set_dir(QBD)
        .join("journal-2025-2026.csv")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn shoal_creek_rolls_forward_to_its_reference_trial_balance() {
    let e = expected(QBD);
    let reference = qbd_tb(&text(QBD, "tb-2026-10-27.csv"));
    assert_eq!(reference, amounts(&e, "reference_tb"), "file vs EXPECTED");
    let books = shoal_creek_books(&base_journal_2025());
    assert_eq!(
        diffs(&books, &reference),
        Balances::new(),
        "opening + journals = reference"
    );
    let t = section(&e, "totals");
    let opening = text(QBD, "tb-opening-2023-12-31.csv");
    assert_eq!(debits(&qbd_tb(&opening)), dec(&t["opening_tb_debits"]));
    assert_eq!(qbd_total_debit(&opening), dec(&t["opening_tb_debits"]));
    assert_eq!(debits(&reference), dec(&t["reference_tb_debits"]));
    assert_eq!(
        qbd_total_debit(&text(QBD, "tb-2026-10-27.csv")),
        dec(&t["reference_tb_debits"])
    );
    for stem in ["journal-2024", "journal-2025-2026"] {
        let j = text(QBD, &format!("{stem}.csv"));
        let legs = qbd_journal(&j);
        let want = dec(&t[&format!("{stem}_debits")]);
        assert_eq!(leg_debits(&legs), want, "{stem}");
        assert_eq!(qbd_journal_total(&j), want, "{stem} TOTAL row");
        assert_eq!(net(&legs), Decimal::ZERO, "{stem}: every entry balances");
    }
}

#[test]
fn shoal_creek_net_income_and_retained_earnings_add_up() {
    let e = expected(QBD);
    let t = section(&e, "totals");
    let types = qbd_account_types(&text(QBD, "account-list.csv"));
    let opening = qbd_tb(&text(QBD, "tb-opening-2023-12-31.csv"));
    let ni_2023: Decimal = -opening
        .iter()
        .filter(|(k, _)| {
            types
                .get(*k)
                .is_some_and(|t| imports_expect::is_income_statement(t))
        })
        .map(|(_, v)| *v)
        .sum::<Decimal>();
    assert_eq!(ni_2023, dec(&t["net_income_2023"]));
    let mut legs = qbd_journal(&text(QBD, "journal-2024.csv"));
    legs.extend(qbd_journal(&text(QBD, "journal-2025-2026.csv")));
    let ni_2024 = net_income(&legs, &types, "2024-01-01", "2024-12-31");
    let ni_2025 = net_income(&legs, &types, "2025-01-01", "2025-12-31");
    assert_eq!(ni_2024, dec(&t["net_income_2024"]));
    assert_eq!(ni_2025, dec(&t["net_income_2025"]));
    let re = -opening["3900"] + ni_2023 + ni_2024 + ni_2025;
    assert_eq!(-amounts(&e, "reference_tb")["3900"], re);
}

#[test]
fn open_items_and_valuation_equal_their_control_accounts() {
    let e = expected(QBD);
    let reference = amounts(&e, "reference_tb");
    let t = section(&e, "totals");
    let ar = qbd_open_items(&text(QBD, "open-invoices.csv"));
    let ap = qbd_open_items(&text(QBD, "unpaid-bills.csv"));
    let sum = |l: &[imports_expect::OpenItem]| l.iter().map(|i| i.open).sum::<Decimal>();
    assert_eq!(sum(&ar), dec(&t["open_invoices"]));
    assert_eq!(sum(&ar), reference["1200"], "A/R = open invoices");
    assert_eq!(sum(&ap), dec(&t["unpaid_bills"]));
    assert_eq!(-sum(&ap), reference["2000"], "A/P = unpaid bills");
    let by_key = |l: &[imports_expect::OpenItem]| -> BTreeMap<String, Decimal> {
        l.iter().map(|i| (i.key.clone(), i.open)).collect()
    };
    assert_eq!(by_key(&ar), amounts(&e, "open_invoices"));
    assert_eq!(by_key(&ap), amounts(&e, "unpaid_bills"));
    let valuation = qbd_valuation(&text(QBD, "inventory-valuation.csv"));
    assert_eq!(valuation, amounts(&e, "inventory_valuation"));
    let v: Decimal = valuation.values().copied().sum();
    assert_eq!(v, dec(&t["inventory_valuation"]));
    assert_eq!(v, reference["1300"], "Inventory Asset = valuation");
    // The lists' own balances say the same (top-level names only).
    let customers = qbd_list_balances(&text(QBD, "customer-list.csv"), "Customer");
    assert_eq!(customers.values().copied().sum::<Decimal>(), sum(&ar));
    let vendors = qbd_list_balances(&text(QBD, "vendor-list.csv"), "Vendor");
    assert_eq!(vendors.values().copied().sum::<Decimal>(), sum(&ap));
}

/// Engine spike: every open item maps to a document; the unapplied
/// payment ("Payment" on Open Invoices) is a credit memo, with a negative
/// open balance like the QBD credit memo.
#[test]
fn open_items_map_to_documents_and_payments_are_credit_memos() {
    let e = expected(QBD);
    let want = section(&e, "open_item_documents");
    let mut items = qbd_open_items(&text(QBD, "open-invoices.csv"));
    items.extend(qbd_open_items(&text(QBD, "unpaid-bills.csv")));
    assert_eq!(items.len(), want.len());
    for i in &items {
        let doc = match i.kind.to_ascii_lowercase().as_str() {
            "invoice" => "invoice",
            "credit memo" | "payment" => "credit_memo",
            "bill" => "bill",
            "credit" => "vendor_credit",
            other => panic!("unknown open-item type {other}"),
        };
        assert_eq!(want[&i.key], doc, "{}", i.key);
        if matches!(doc, "credit_memo" | "vendor_credit") {
            assert!(i.open < Decimal::ZERO, "{} is a credit", i.key);
        }
    }
    assert!(items.iter().any(|i| i.kind == "Payment"));
}

/// S5: the reference trial balance, both agings and the valuation are dated
/// the conversion date; the opening trial balance the day before history
/// starts; no journal row is after the conversion date or before history.
#[test]
fn shoal_creek_dates_line_up() {
    let e = expected(QBD);
    let conversion = value(&e, "company", "conversion_date");
    for f in [
        "tb-2026-10-27.csv",
        "open-invoices.csv",
        "unpaid-bills.csv",
        "inventory-valuation.csv",
    ] {
        assert_eq!(as_of(&text(QBD, f)), conversion, "{f}");
    }
    assert_eq!(
        as_of(&text(QBD, "tb-opening-2023-12-31.csv")),
        value(&e, "company", "opening_tb_date")
    );
    let from = value(&e, "company", "history_from_two_years");
    // Q-142 default: the fiscal year two years before the conversion's.
    let fy = fiscal_year_start(conversion, 1);
    let two_back = format!("{}{}", fy[..4].parse::<u32>().expect("y") - 2, &fy[4..]);
    assert_eq!(from, two_back);
    for stem in ["journal-2024", "journal-2025-2026"] {
        for l in qbd_journal(&text(QBD, &format!("{stem}.csv"))) {
            assert!(
                l.date.as_str() >= from && l.date.as_str() <= conversion,
                "{stem}: {}",
                l.date
            );
        }
    }
}

#[test]
fn qbd_files_are_latin1_with_middle_dot_accounts() {
    for f in [
        "account-list.csv",
        "customer-list.csv",
        "journal-2025-2026.csv",
        "open-invoices.csv",
    ] {
        let b = bytes(QBD, f);
        assert!(
            std::str::from_utf8(&b).is_err(),
            "{f} must be Latin-1, not UTF-8"
        );
    }
    let accounts = bytes(QBD, "account-list.csv");
    assert!(accounts.contains(&0xB7), "middle dot separator");
    assert!(
        bytes(QBD, "customer-list.csv").contains(&0xE9),
        "é as one Latin-1 byte"
    );
    let gl = bytes("imports/qbd-gl", "journal.csv");
    assert!(std::str::from_utf8(&gl).is_err() && gl.contains(&0xB7));
}

#[test]
fn shoal_creek_covers_every_listed_feature() {
    let e = expected(QBD);
    let all: String = section(&e, "files")
        .keys()
        .map(|stem| text(QBD, &format!("{stem}.csv")))
        .collect();
    for (feature, needle) in section(&e, "features") {
        assert!(all.contains(needle.as_str()), "{feature}: {needle:?}");
    }
    let customers = text(QBD, "customer-list.csv");
    let vendors = text(QBD, "vendor-list.csv");
    let both = value(&e, "features", "both_lists");
    assert!(customers.contains(both) && vendors.contains(both));
    let inactive = value(&e, "features", "inactive_with_open_item");
    assert!(
        read_csv(&customers)
            .iter()
            .any(|r| r.get(1).is_some_and(|s| s == "Inactive")
                && r.get(2).is_some_and(|s| s == inactive))
    );
    assert!(text(QBD, "open-invoices.csv").contains(inactive));
    // The partial payment leaves 1005 open for less than it was invoiced.
    let invoiced: Decimal = qbd_journal(&text(QBD, "journal-2025-2026.csv"))
        .iter()
        .filter(|l| l.date == "2026-03-15" && l.account == "1200")
        .map(|l| l.amount)
        .sum();
    let open = amounts(&e, "open_invoices")["1005"];
    assert!(open > Decimal::ZERO && open < invoiced);
    // Every file the set lists exists, and the expectation names each kind.
    for (stem, kind) in section(&e, "files") {
        assert!(set_dir(QBD).join(format!("{stem}.csv")).is_file(), "{stem}");
        assert_ne!(kind.as_str(), "");
    }
}

/// Plan line 208: the altered sets differ on exactly the accounts named.
#[test]
fn altered_repointed_moves_one_line_between_two_accounts() {
    let set = "qbd/shoal-creek-altered/repointed";
    let e = expected(set);
    let reference = qbd_tb(&text(QBD, "tb-2026-10-27.csv"));
    let journal = set_dir(set).join("journal-2025-2026.csv");
    let books = shoal_creek_books(&journal.to_string_lossy());
    assert_eq!(diffs(&books, &reference), amounts(&e, "diffs"));
    // The altered journal still balances and still totals the same.
    let j = latin1(&std::fs::read(&journal).expect("journal"));
    assert_eq!(net(&qbd_journal(&j)), Decimal::ZERO);
    assert_eq!(
        qbd_journal_total(&j),
        dec(&section(&expected(QBD), "totals")["journal-2025-2026_debits"])
    );
    for (k, v) in section(&e, "balance_rows") {
        assert_eq!(dec(v), Decimal::ZERO, "{k}");
    }
}

#[test]
fn altered_open_item_shows_on_receivables_and_its_history_account() {
    let set = "qbd/shoal-creek-altered/open-item";
    let e = expected(set);
    let reference = qbd_tb(&text(QBD, "tb-2026-10-27.csv"));
    let history = shoal_creek_books(&base_journal_2025());
    let items = qbd_open_items(&latin1(&bytes(set, "open-invoices.csv")));
    let documents: Decimal = items.iter().map(|i| i.open).sum();
    // The books' A/R is the open documents; the journal's A/R activity sits
    // on "Receivables before conversion" less the documents' offsets.
    let mut books = history.clone();
    let journal_ar = books.remove("1200").unwrap_or_default();
    add(&mut books, "1200", documents);
    assert_eq!(diffs(&books, &reference), amounts(&e, "diffs"));
    let history_ar = journal_ar - documents;
    assert_eq!(
        history_ar,
        dec(value(&e, "balance_rows", "history_ar")),
        "history_ar"
    );
    let base: BTreeMap<String, Decimal> = qbd_open_items(&text(QBD, "open-invoices.csv"))
        .into_iter()
        .map(|i| (i.key, i.open))
        .collect();
    for (key, want) in section(&e, "items") {
        // The same file is on both sides of an item row, so it stays 0.00.
        assert_eq!(dec(want), Decimal::ZERO, "{key}");
        let altered = items.iter().find(|i| &i.key == key).expect("item").open;
        assert_ne!(altered, base[key], "{key} was changed");
    }
}

#[test]
fn the_overlap_journal_overlaps_the_base_journal() {
    let base = qbd_journal(&text(QBD, "journal-2025-2026.csv"));
    let overlap = qbd_journal(&latin1(&bytes(
        "qbd/shoal-creek-altered",
        "journal-overlap.csv",
    )));
    let (lo, hi) = (
        base.iter().map(|l| l.date.clone()).min().expect("min"),
        base.iter().map(|l| l.date.clone()).max().expect("max"),
    );
    assert_ne!(overlap.len(), 0);
    assert!(overlap.iter().all(|l| l.date >= lo && l.date <= hi));
    assert_eq!(net(&overlap), Decimal::ZERO);
}

/// The books of a Bluebonnet set from its opening TB and GL.
fn other_system(set: &str) -> (Expected, Balances, Balances) {
    let e = expected(set);
    let src = value(&e, "company", "source").to_string();
    let read = |f: &str| text(set, f);
    let (opening, legs, reference) = match src.as_str() {
        "accounting_power" => (
            ap_tb(&read("tb-opening.csv"), "Adjusted Balance"),
            ap_gl(&read("gl.csv")),
            ap_tb(&read("tb-reference.csv"), "Adjusted Balance"),
        ),
        "quickbooks_online" => {
            let n = qbo_numbers(&read("coa.csv"));
            (
                qbo_tb(&read("tb-opening.csv"), &n),
                qbo_gl(&read("gl.csv"), &n),
                qbo_tb(&read("tb-reference.csv"), &n),
            )
        }
        "quickbooks_desktop" => (
            qbd_tb(&read("tb-opening.csv")),
            qbd_journal(&read("journal.csv")),
            qbd_tb(&read("tb-reference.csv")),
        ),
        "generic" => (
            generic_tb(&read("tb-opening.csv")),
            generic_gl(&read("gl.csv")),
            generic_tb(&read("tb-reference.csv")),
        ),
        other => panic!("source {other}"),
    };
    let t = section(&e, "totals");
    assert_eq!(debits(&opening), dec(&t["opening_tb_debits"]), "{set}");
    assert_eq!(debits(&reference), dec(&t["reference_tb_debits"]), "{set}");
    assert_eq!(leg_debits(&legs), dec(&t["gl_debits"]), "{set}");
    assert_eq!(net(&legs), Decimal::ZERO, "{set}: the GL balances");
    let mut books = opening;
    for l in &legs {
        assert!(l.date.as_str() <= value(&e, "company", "conversion_date"));
        assert!(l.date.as_str() > value(&e, "company", "opening_tb_date"));
        add(&mut books, &l.account, l.amount);
    }
    (e, books, reference)
}

#[test]
fn every_other_system_set_ties_to_its_reference_trial_balance() {
    for set in [
        "imports/accounting-power",
        "imports/quickbooks-online",
        "imports/qbd-gl",
        "imports/generic",
    ] {
        let (e, books, reference) = other_system(set);
        assert_eq!(reference, amounts(&e, "reference_tb"), "{set}: file");
        assert_eq!(diffs(&books, &reference), Balances::new(), "{set}");
        for stem in section(&e, "files").keys() {
            assert!(
                std::fs::read_dir(set_dir(set)).expect("dir").any(|f| f
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    == format!("{stem}.csv")),
                "{set}: {stem}"
            );
        }
        let both = value(&e, "features", "both_lists");
        let files: String = section(&e, "files")
            .keys()
            .map(|s| text(set, &format!("{s}.csv")))
            .collect();
        assert!(
            files.matches(both).count() >= 2,
            "{set}: {both} on both lists"
        );
    }
}

#[test]
fn generic_altered_differs_on_two_accounts() {
    let e = expected("imports/generic-altered");
    let base = "imports/generic";
    let mut books = generic_tb(&text(base, "tb-opening.csv"));
    let legs = generic_gl(&text("imports/generic-altered", "gl.csv"));
    assert_eq!(net(&legs), Decimal::ZERO, "still balances");
    for l in &legs {
        add(&mut books, &l.account, l.amount);
    }
    let reference = generic_tb(&text(base, "tb-reference.csv"));
    assert_eq!(diffs(&books, &reference), amounts(&e, "diffs"));
}
#[test]
fn spreadsheet_transactions_balance_and_the_bad_rows_are_bad() {
    let set = "imports/transactions";
    let e = expected(set);
    let rows = read_csv(&text(set, "good.csv"));
    let h = &rows[0];
    let c = |n: &str| h.iter().position(|x| x == n).expect(n);
    let (ig, it, inum, iamt, idr, icr) = (
        c("group_id"),
        c("type"),
        c("number"),
        c("line_amount"),
        c("line_debit"),
        c("line_credit"),
    );
    let mut docs: BTreeMap<String, (Decimal, Decimal, Decimal)> = BTreeMap::new();
    for r in &rows[1..] {
        let key = format!("{}:{}", r[it], r[inum]);
        let d = docs.entry(key).or_default();
        d.0 += amount(&r[iamt]);
        d.1 += amount(&r[idr]);
        d.2 += amount(&r[icr]);
        assert_ne!(r[ig].as_str(), "");
    }
    let good = section(&e, "good");
    assert_eq!(docs.len().to_string(), good["documents"]);
    for (key, (amt, dr, cr)) in &docs {
        let want = dec(&good[key]);
        if key.starts_with("journal_entry") {
            assert_eq!(dr, cr, "{key} balances");
            assert_eq!(*dr, want, "{key}");
        } else {
            assert_eq!(*amt, want, "{key}");
        }
    }
    let accounts: Vec<String> = read_csv(&text("imports/generic", "coa.csv"))[1..]
        .iter()
        .map(|r| r[0].clone())
        .collect();
    let bad = read_csv(&text(set, "bad-rows.csv"));
    let group = |g: &str| -> Vec<&Vec<String>> { bad[1..].iter().filter(|r| r[ig] == g).collect() };
    for (g, code) in section(&e, "bad") {
        let rows = group(g);
        assert!(!rows.is_empty(), "{g}");
        let holds = match code.as_str() {
            "IMPORT_UNBALANCED" => {
                rows.iter().map(|r| amount(&r[idr])).sum::<Decimal>()
                    != rows.iter().map(|r| amount(&r[icr])).sum::<Decimal>()
            }
            "IMPORT_UNKNOWN_REF" => rows
                .iter()
                .any(|r| !accounts.contains(&r[c("line_account")])),
            "IMPORT_MIXED_GROUP" => rows.iter().any(|r| r[it] != rows[0][it]),
            "IMPORT_AMOUNT_INVALID" => rows
                .iter()
                .any(|r| r[iamt].trim().is_empty() && r[idr].trim().is_empty()),
            other => panic!("code {other}"),
        };
        assert!(holds, "{g} is not {code}");
    }
}

/// Synthetic only (NN10): no real-looking addresses or numbers.
#[test]
fn no_real_contact_data_in_any_fixture() {
    let mut files = Vec::new();
    for root in ["qbd", "imports"] {
        let mut stack = vec![imports_expect::fixtures_root().join(root)];
        while let Some(d) = stack.pop() {
            for f in std::fs::read_dir(&d).expect("dir") {
                let p = f.expect("entry").path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    files.push(p);
                }
            }
        }
    }
    assert!(files.len() > 40, "found {}", files.len());
    for p in files {
        let t = latin1(&std::fs::read(&p).expect("read"));
        for word in t.split(|c: char| c.is_whitespace() || c == ',' || c == '"') {
            if word.contains('@') {
                assert!(word.ends_with(".example"), "{}: {word}", p.display());
            }
            let digits: String = word.chars().filter(char::is_ascii_digit).collect();
            if word.len() == 8 && word.as_bytes()[3] == b'-' && digits.len() == 7 {
                assert!(word.starts_with("555-"), "{}: {word}", p.display());
            }
        }
        assert!(
            std::fs::metadata(&p).expect("meta").len() < 10 * 1024 * 1024,
            "{}",
            p.display()
        );
    }
}
