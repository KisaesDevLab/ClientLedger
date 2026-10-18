use super::*;

fn cid(n: u32) -> ContactId {
    ContactId::parse(&format!("c0000000-0000-4000-8000-{n:012}")).expect("id")
}

fn aid(n: u32) -> AccountId {
    AccountId::parse(&format!("a0000000-0000-4000-8000-{n:012}")).expect("id")
}

fn entry(n: u32, roles: Roles, name: &str, active: bool) -> ContactEntry {
    ContactEntry {
        id: cid(n),
        roles,
        name: name.to_string(),
        company: None,
        email: None,
        phone: None,
        active,
    }
}

fn sample() -> ContactList {
    let mut galena = entry(4, Roles::Vendor, "Galena Gravel", true);
    galena.email = Some("orders@galena.example".into());
    ContactList::new(vec![
        entry(1, Roles::Client, "riverbend HOA", true),
        entry(2, Roles::Both, "Pine Ridge Storage", true),
        entry(3, Roles::Client, "Answering Co", false),
        galena,
    ])
}

fn names(list: &ContactList, rows: &[usize]) -> Vec<String> {
    rows.iter()
        .map(|i| list.entries()[*i].name.clone())
        .collect()
}

#[test]
fn roles_read_and_write_the_engine_values() {
    for r in [Roles::Client, Roles::Vendor, Roles::Both] {
        assert_eq!(Roles::from_wire(r.wire()), Some(r));
    }
    assert_eq!(Roles::Client.wire(), "customer");
    assert_eq!(Roles::from_wire("employee"), None);
    assert!(Roles::Both.has(Role::Client) && Roles::Both.has(Role::Vendor));
    assert!(!Roles::Client.has(Role::Vendor));
    assert_eq!(Roles::Client.with(Role::Vendor, true), Some(Roles::Both));
    assert_eq!(Roles::Both.with(Role::Client, false), Some(Roles::Vendor));
    assert_eq!(
        Roles::Vendor.with(Role::Vendor, false),
        None,
        "needs a role"
    );
    assert_eq!(Roles::only(Role::Vendor), Roles::Vendor);
}

#[test]
fn a_contact_with_both_roles_is_in_both_centers() {
    let list = sample();
    let f = ContactFilter::default();
    let clients = names(&list, &list.visible(Role::Client, &f));
    let vendors = names(&list, &list.visible(Role::Vendor, &f));
    assert_eq!(clients, ["Pine Ridge Storage", "riverbend HOA"]);
    assert_eq!(vendors, ["Galena Gravel", "Pine Ridge Storage"]);
    assert_eq!(list.in_role(Role::Client), 3);
    assert_eq!(list.in_role(Role::Vendor), 2);
}

#[test]
fn filters_hide_inactive_and_search_every_column() {
    let list = sample();
    let all = ContactFilter {
        show_inactive: true,
        ..ContactFilter::default()
    };
    assert_eq!(
        names(&list, &list.visible(Role::Client, &all)),
        ["Answering Co", "Pine Ridge Storage", "riverbend HOA"],
        "case-insensitive name order"
    );
    let by_email = ContactFilter {
        search: " GALENA.example ".into(),
        ..ContactFilter::default()
    };
    assert_eq!(
        names(&list, &list.visible(Role::Vendor, &by_email)),
        ["Galena Gravel"]
    );
    assert_eq!(list.visible(Role::Client, &by_email), Vec::<usize>::new());
}

#[test]
fn names_are_unique_across_both_lists() {
    let list = sample();
    assert_eq!(list.name_owner("  Galena gravel ", None), Some(cid(4)));
    assert_eq!(list.name_owner("Galena Gravel", Some(cid(4))), None);
    assert_eq!(
        list.name_owner("answering co", None),
        None,
        "an inactive contact does not hold its name"
    );
    assert_eq!(list.name_owner(" ", None), None);
}

#[test]
fn merge_targets_are_other_active_contacts_of_the_role() {
    let list = sample();
    let t: Vec<ContactId> = list
        .merge_targets(Role::Client, cid(1))
        .iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(t, [cid(2)], "inactive and vendors are left out");
}

#[test]
fn terms_keep_unknown_text_and_round_trip_the_fixed_list() {
    for t in Terms::ALL {
        let c = TermsChoice::from_stored(Some(t.wire()));
        assert_eq!(c, TermsChoice::Fixed(t));
        assert_eq!(c.stored().as_deref(), Some(t.wire()));
    }
    assert_eq!(TermsChoice::from_stored(None), TermsChoice::None);
    assert_eq!(TermsChoice::from_stored(Some("  ")), TermsChoice::None);
    let other = TermsChoice::from_stored(Some("2% 10 Net 30"));
    assert_eq!(other.stored().as_deref(), Some("2% 10 Net 30"));
    assert_eq!(TermsChoice::None.stored(), None);
}

#[test]
fn email_and_tax_id_checks() {
    for ok in ["a@b.co", "billing@ozark-ridge.example.com"] {
        assert!(looks_like_email(ok), "{ok}");
    }
    for bad in [
        "", "a", "a@b", "@b.co", "a@b.", "a b@c.co", "a@@b.co", "a@.co",
    ] {
        assert!(!looks_like_email(bad), "{bad}");
    }
    assert_eq!(tax_id_digits("12-3456789").as_deref(), Some("123456789"));
    assert_eq!(tax_id_digits("123 45 6789").as_deref(), Some("123456789"));
    assert_eq!(tax_id_digits("12-345678"), None);
    assert_eq!(tax_id_digits("12-34567890"), None);
    assert_eq!(tax_id_digits("12-34567B9"), None);
    assert_eq!(
        masked_tax_id("12-3456789").as_deref(),
        Some("\u{2022}\u{2022}\u{2022}-\u{2022}\u{2022}-6789")
    );
    assert_eq!(masked_tax_id("12"), None);
}

#[test]
fn check_trims_and_drops_vendor_fields_for_a_client() {
    let list = sample();
    let mut d = ContactDraft::new(Role::Client);
    d.name = "  Table Rock Dental ".into();
    d.email = " office@trd.example ".into();
    d.billing.city = " Branson ".into();
    d.terms = TermsChoice::Fixed(Terms::Net30);
    d.expense_account = Some(aid(1));
    d.tax_id = "not checked".into();
    d.track_1099 = true;
    let c = check(&d, &list, None, |_| false).expect("valid");
    assert_eq!(c.name, "Table Rock Dental");
    assert_eq!(c.email.as_deref(), Some("office@trd.example"));
    assert_eq!(c.billing[2].as_deref(), Some("Branson"));
    assert_eq!(c.billing[0], None);
    assert_eq!(c.terms.as_deref(), Some("net_30"));
    assert_eq!(c.expense_account, None, "a client has no expense account");
    assert_eq!(c.tax_id, None);
    assert!(!c.track_1099);
}

#[test]
fn check_keeps_vendor_fields_and_finds_every_problem() {
    let list = sample();
    let mut d = ContactDraft::new(Role::Vendor);
    d.name = "Crane Creek Tree".into();
    d.expense_account = Some(aid(7));
    d.tax_id = "12-3456789".into();
    d.tax_id_kind = Some(TaxIdKind::Ein);
    d.track_1099 = true;
    d.vendor_account = "ACCT-0042".into();
    let c = check(&d, &list, None, |a| a == aid(7)).expect("valid");
    assert_eq!(c.expense_account, Some(aid(7)));
    assert_eq!(c.tax_id.as_deref(), Some("12-3456789"));
    assert_eq!(c.tax_id_kind, Some(TaxIdKind::Ein));
    assert!(c.track_1099);

    let mut bad = ContactDraft::new(Role::Vendor);
    bad.name = "pine ridge storage".into();
    bad.company = "x".repeat(NAME_MAX + 1);
    bad.last = "y".repeat(PERSON_MAX + 1);
    bad.email = "nope".into();
    bad.phone = "5".repeat(PHONE_MAX + 1);
    bad.billing.zip = "1".repeat(ZIP_MAX + 1);
    bad.shipping.state = "s".repeat(STATE_MAX + 1);
    bad.terms = TermsChoice::Other("t".repeat(TERMS_MAX + 1));
    bad.notes = "n".repeat(NOTES_MAX + 1);
    bad.expense_account = Some(aid(9));
    bad.vendor_account = "v".repeat(VENDOR_ACCOUNT_MAX + 1);
    bad.tax_id = "123".into();
    let problems = check(&bad, &list, None, |_| false).expect_err("invalid");
    assert_eq!(
        problems,
        [
            ContactProblem::NameTaken,
            ContactProblem::CompanyTooLong,
            ContactProblem::PersonTooLong,
            ContactProblem::EmailInvalid,
            ContactProblem::PhoneTooLong,
            ContactProblem::BillingTooLong,
            ContactProblem::ShippingTooLong,
            ContactProblem::TermsTooLong,
            ContactProblem::NotesTooLong,
            ContactProblem::ExpenseAccountInvalid,
            ContactProblem::VendorAccountTooLong,
            ContactProblem::TaxIdInvalid,
        ]
    );
    for p in &problems {
        assert!(p.key().as_str().starts_with("contacts.problem."));
        let _ = p.field();
    }
    let mut empty = ContactDraft::new(Role::Client);
    empty.name = " ".into();
    assert_eq!(
        check(&empty, &list, None, |_| true),
        Err(vec![ContactProblem::NameMissing])
    );
    empty.name = "\u{1F33F}".repeat(NAME_MAX / 2 + 1);
    assert_eq!(
        check(&empty, &list, None, |_| true),
        Err(vec![ContactProblem::NameTooLong]),
        "counted as the engine counts"
    );
}

#[test]
fn renaming_keeps_its_own_name_and_ship_to_copies_billing() {
    let list = sample();
    let mut d = ContactDraft::new(Role::Vendor);
    d.name = "Galena Gravel".into();
    assert!(check(&d, &list, Some(cid(4)), |_| true).is_ok());
    d.billing.line1 = "1 Quarry Rd".into();
    d.ship_to_billing();
    assert_eq!(d.shipping.line1, "1 Quarry Rd");
    assert!(!d.shipping.is_blank());
    assert!(AddressDraft::default().is_blank());
}

/// P11 acceptance: 20k-record lists filter under 100 ms.
#[test]
fn twenty_thousand_contacts_filter_quickly() {
    let entries: Vec<ContactEntry> = (0..20_000u32)
        .map(|n| {
            let roles = match n % 3 {
                0 => Roles::Client,
                1 => Roles::Vendor,
                _ => Roles::Both,
            };
            let mut e = entry(n, roles, &format!("Contact {n:05}"), n % 7 != 0);
            e.email = Some(format!("c{n}@example.test"));
            e
        })
        .collect();
    let list = ContactList::new(entries);
    let filter = ContactFilter {
        show_inactive: false,
        search: "contact 19".into(),
    };
    let start = std::time::Instant::now();
    let rows = list.visible(Role::Client, &filter);
    let all = list.visible(Role::Vendor, &ContactFilter::default());
    let took = start.elapsed();
    assert!(!rows.is_empty() && !all.is_empty());
    assert!(took.as_millis() < 100, "filtering took {took:?}");
}

#[test]
fn removing_a_tax_id_only_counts_for_a_vendor_with_nothing_typed() {
    let list = sample();
    let mut d = ContactDraft::new(Role::Vendor);
    d.name = "Bull Shoals Docks".into();
    d.clear_tax_id = true;
    assert!(check(&d, &list, None, |_| true).expect("ok").clear_tax_id);
    d.tax_id = "12-3456789".into();
    let typed = check(&d, &list, None, |_| true).expect("ok");
    assert!(!typed.clear_tax_id, "a new one replaces it");
    d.roles = Roles::Client;
    assert!(!check(&d, &list, None, |_| true).expect("ok").clear_tax_id);
}

/// B1: `Debug` never prints a tax ID, only whether there is one.
#[test]
fn debug_hides_the_tax_id() {
    let list = sample();
    let mut d = ContactDraft::new(Role::Vendor);
    d.name = "Canary Tree".into();
    d.tax_id = "98-7654321".into();
    let shown = format!("{d:?}");
    assert!(!shown.contains("7654321"), "{shown}");
    assert!(shown.contains("has_tax_id: true"), "{shown}");
    let c = check(&d, &list, None, |_| true).expect("valid");
    let shown = format!("{c:?} {c:#?}");
    assert!(!shown.contains("7654321"), "{shown}");
    assert!(shown.contains("has_tax_id: true"), "{shown}");
}
