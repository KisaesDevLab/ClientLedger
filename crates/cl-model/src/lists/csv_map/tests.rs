use super::*;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_owned()).collect()
}

fn acct(n: u8) -> AccountId {
    AccountId::parse(&format!("2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{n:02}")).expect("id")
}

#[test]
fn guesses_from_headers_once_each() {
    let m = guess(
        CsvList::Accounts,
        &s(&["Account Number", "Name", "Type", "Detail Type", "Balance"]),
    );
    assert_eq!(
        m,
        vec![
            Some(Target::AccountNumber),
            Some(Target::AccountName),
            Some(Target::AccountType),
            Some(Target::DetailType),
            None
        ]
    );
    let m = guess(
        CsvList::Contacts,
        &s(&["Display Name", "Customer", "E-mail", "Phone"]),
    );
    assert_eq!(
        m,
        vec![
            Some(Target::ContactName),
            None,
            Some(Target::Email),
            Some(Target::Phone)
        ]
    );
    // The engine's own export headers map back.
    let m = guess(
        CsvList::Items,
        &s(&[
            "Name",
            "Description",
            "Unit Price",
            "Income Account ID",
            "Taxable",
            "Active",
        ]),
    );
    assert_eq!(m.iter().flatten().count(), 5);
}

#[test]
fn mapping_problems() {
    let m = vec![Some(Target::AccountName), Some(Target::AccountName)];
    let p = check_mapping(CsvList::Accounts, &m, 3);
    assert_eq!(
        p,
        vec![
            MappingProblem::Twice(Target::AccountName),
            MappingProblem::Missing(Target::AccountType)
        ]
    );
    assert_eq!(
        check_mapping(CsvList::Contacts, &[Some(Target::ContactName)], 0),
        vec![MappingProblem::NoRows]
    );
    assert_eq!(
        check_mapping(
            CsvList::Accounts,
            &[Some(Target::AccountName), Some(Target::AccountType)],
            2001
        ),
        vec![MappingProblem::TooManyRows(2000)]
    );
    assert_eq!(
        check_mapping(CsvList::Items, &[Some(Target::ItemName)], 1),
        vec![]
    );
    for p in [
        MappingProblem::Missing(Target::Email),
        MappingProblem::Twice(Target::Email),
        MappingProblem::TooManyRows(1),
        MappingProblem::NoRows,
    ] {
        assert!(p.key().as_str().starts_with("csv.problem."));
    }
    for l in CsvList::ALL {
        assert!(l.label().as_str().starts_with("csv.list."));
        assert_ne!(l.key(), "");
        for t in l.targets() {
            assert!(t.label().as_str().starts_with("csv.field."));
        }
    }
}

#[test]
fn account_types_from_many_spellings() {
    assert_eq!(
        parse_account_type("revenue"),
        Some((AccountKind::Income, None))
    );
    assert_eq!(
        parse_account_type("Other Expense"),
        Some((AccountKind::OtherExpense, None))
    );
    assert_eq!(
        parse_account_type("other_revenue"),
        Some((AccountKind::OtherIncome, None))
    );
    assert_eq!(
        parse_account_type("Income"),
        Some((AccountKind::Income, None))
    );
    assert_eq!(parse_account_type("COGS"), Some((AccountKind::Cogs, None)));
    assert_eq!(
        parse_account_type("Bank"),
        Some((AccountKind::Asset, Some("bank")))
    );
    assert_eq!(
        parse_account_type("Credit Card"),
        Some((AccountKind::Liability, Some("credit_card")))
    );
    assert_eq!(
        parse_account_type("Long Term Liability"),
        Some((AccountKind::Liability, Some("long_term_liability")))
    );
    assert_eq!(parse_account_type("Spaceship"), None);
}

#[test]
fn account_rows() {
    let m = guess(
        CsvList::Accounts,
        &s(&["Number", "Name", "Type", "Detail Type"]),
    );
    let long_num = "9".repeat(21);
    let rows = vec![
        (2, s(&["10100", "Operating Checking", "Bank", ""])),
        (3, s(&["", "Fuel", "Expense", "Utilities"])),
        (4, s(&["", "Odd detail", "Expense", "Something else"])),
        (5, s(&["", "", "Expense", ""])),
        (6, s(&["", "Moon", "Spaceship", ""])),
        (7, s(&[long_num.as_str(), "Long", "Asset", ""])),
    ];
    let b = build(CsvList::Accounts, &m, &rows, None, None);
    assert_eq!(b.rows.len(), 3);
    assert_eq!(
        b.rows[0].1,
        Mapped::Account(AccountRow {
            number: Some("10100".into()),
            name: "Operating Checking".into(),
            kind: AccountKind::Asset,
            detail_type: Some("bank".into()),
        })
    );
    match &b.rows[1].1 {
        Mapped::Account(a) => assert_eq!(a.detail_type.as_deref(), Some("utilities")),
        other => panic!("{other:?}"),
    }
    match &b.rows[2].1 {
        Mapped::Account(a) => assert_eq!(a.detail_type.as_deref(), Some("Something else")),
        other => panic!("{other:?}"),
    }
    let problems: Vec<_> = b.issues.iter().map(|i| (i.line, i.problem)).collect();
    assert_eq!(
        problems,
        vec![
            (5, RowProblem::NameMissing),
            (6, RowProblem::TypeUnknown),
            (7, RowProblem::TooLong(Target::AccountNumber)),
        ]
    );
}

#[test]
fn contact_rows() {
    let m = guess(
        CsvList::Contacts,
        &s(&["Name", "Company", "Email", "Phone"]),
    );
    let rows = vec![
        (
            2,
            s(&["Ada Park", "Park Homes", "ada@example.com", "555-0100"]),
        ),
        (3, s(&["Bo", "", "not-an-email", ""])),
        (4, s(&["Cy", "", "", &"1".repeat(31)])),
        (5, s(&["Di"])),
    ];
    let b = build(CsvList::Contacts, &m, &rows, None, None);
    assert_eq!(b.rows.len(), 2);
    assert_eq!(
        b.rows[1].1,
        Mapped::Contact(ContactRow {
            name: "Di".into(),
            company: None,
            email: None,
            phone: None
        })
    );
    assert_eq!(b.issues[0].problem, RowProblem::EmailInvalid);
    assert_eq!(b.issues[1].problem, RowProblem::TooLong(Target::Phone));
    assert!(!email_ok("a@b"));
    assert!(!email_ok("a b@c.d"));
    assert!(email_ok("a@b.co"));
}

#[test]
fn item_rows_exact_prices_and_accounts() {
    let income = acct(1);
    let other = acct(2);
    let resolve = |c: &str| match c {
        "4000" | "Service income" => Some(other),
        _ => None,
    };
    let taken = |n: &str| n.eq_ignore_ascii_case("mowing");
    let l = ItemLookups {
        income_account: &resolve,
        default_income: Some(income),
        name_taken: &taken,
    };
    assert!(format!("{l:?}").contains("default_income"));
    let m = guess(
        CsvList::Items,
        &s(&["Item", "Description", "Price", "Account", "Taxable"]),
    );
    let rows = vec![
        (2, s(&["Edging", "Beds", "$1,234.5678", "4000", "No"])),
        (3, s(&["Trim", "", "", "", ""])),
        (4, s(&["MOWING", "", "", "", ""])),
        (5, s(&["Bad price", "", "12.34567", "", ""])),
        (6, s(&["Negative", "", "-1.00", "", ""])),
        (7, s(&["Elsewhere", "", "", "Nowhere", ""])),
        (8, s(&["Maybe", "", "", "", "perhaps"])),
        (9, s(&["trim", "", "", "", ""])),
    ];
    let b = build(CsvList::Items, &m, &rows, Some(&l), None);
    assert_eq!(b.rows.len(), 2);
    assert_eq!(
        b.rows[0].1,
        Mapped::Item(ItemRow {
            name: "Edging".into(),
            description: Some("Beds".into()),
            price: Some(Money::parse_wire("1234.5678").expect("m")),
            income_account: other,
            taxable: false,
        })
    );
    match &b.rows[1].1 {
        Mapped::Item(i) => {
            assert_eq!(i.income_account, income);
            assert!(i.taxable);
            assert_eq!(i.price, None);
        }
        other => panic!("{other:?}"),
    }
    let problems: Vec<_> = b.issues.iter().map(|i| i.problem).collect();
    assert_eq!(
        problems,
        vec![
            RowProblem::NameTaken,
            RowProblem::PriceInvalid,
            RowProblem::PriceNegative,
            RowProblem::IncomeAccountUnknown,
            RowProblem::TaxableInvalid,
            RowProblem::NameTaken,
        ]
    );
    // No default and no column: every row says so.
    let none = ItemLookups {
        income_account: &resolve,
        default_income: None,
        name_taken: &|_| false,
    };
    let b = build(
        CsvList::Items,
        &[Some(Target::ItemName)],
        &[(2, s(&["X"]))],
        Some(&none),
        None,
    );
    assert_eq!(b.issues[0].problem, RowProblem::IncomeAccountMissing);
    let b = build(
        CsvList::Items,
        &[Some(Target::ItemName)],
        &[(2, s(&["X"]))],
        None,
        None,
    );
    assert_eq!(b.issues[0].problem, RowProblem::IncomeAccountMissing);
}

/// S2: a contact name already on an active contact, or twice in the file,
/// is left out as taken.
#[test]
fn contact_names_already_used_are_taken() {
    let m = guess(CsvList::Contacts, &s(&["Name"]));
    let rows = vec![
        (2, s(&["Ada Park"])),
        (3, s(&["ada park"])),
        (4, s(&["Pine Ridge Storage"])),
        (5, s(&["New Name"])),
    ];
    let active = ["pine ridge storage".to_string()];
    let taken = |n: &str| active.contains(&n.to_lowercase());
    let b = build(CsvList::Contacts, &m, &rows, None, Some(&taken));
    let ok: Vec<usize> = b.rows.iter().map(|(l, _)| *l).collect();
    assert_eq!(ok, [2, 5]);
    let issues: Vec<(usize, RowProblem)> = b.issues.iter().map(|i| (i.line, i.problem)).collect();
    assert_eq!(
        issues,
        [(3, RowProblem::NameTaken), (4, RowProblem::NameTaken)]
    );
}

#[test]
fn every_row_problem_has_text() {
    for p in [
        RowProblem::NameMissing,
        RowProblem::TooLong(Target::Email),
        RowProblem::TypeUnknown,
        RowProblem::PriceInvalid,
        RowProblem::PriceNegative,
        RowProblem::IncomeAccountUnknown,
        RowProblem::IncomeAccountMissing,
        RowProblem::EmailInvalid,
        RowProblem::TaxableInvalid,
        RowProblem::NameTaken,
    ] {
        assert!(p.key().as_str().starts_with("csv.row."));
    }
}
