use cl_core::fiscal::FiscalYearSpec;
use cl_core::ids::{AccountId, ContactId, TagId};
use cl_core::{CalendarDate, Money};
use proptest::prelude::*;

use super::*;
use crate::lists::accounts::AccountKind;

fn d(y: i32, m: u32, day: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, day).unwrap()
}

fn calendar() -> FiscalYearSpec {
    FiscalYearSpec::Calendar { start_month: 1 }
}

fn july() -> FiscalYearSpec {
    FiscalYearSpec::Calendar { start_month: 7 }
}

fn r(start: CalendarDate, end: CalendarDate) -> DateRange {
    DateRange { start, end }
}

fn money(s: &str) -> Money {
    Money::parse_wire(s).unwrap()
}

fn acct(n: u32) -> AccountId {
    AccountId::parse(&format!("a0000000-0000-4000-8000-{n:012x}")).unwrap()
}

fn contact(n: u32) -> ContactId {
    ContactId::parse(&format!("c0000000-0000-4000-8000-{n:012x}")).unwrap()
}

fn tag(n: u32) -> TagId {
    TagId::parse(&format!("70000000-0000-4000-8000-{n:012x}")).unwrap()
}

// ----- ranges -------------------------------------------------------------

#[test]
fn last_90_days_matches_the_plan_example() {
    // "No transactions from 07/21/2026 to 10/19/2026."
    assert_eq!(
        RangePreset::Last90Days.resolve(d(2026, 10, 19), &calendar()),
        r(d(2026, 7, 21), d(2026, 10, 19))
    );
    assert_eq!(RangePreset::default(), RangePreset::Last90Days);
}

#[test]
fn month_presets_cover_whole_months_and_leap_years() {
    let today = d(2024, 3, 15);
    assert_eq!(
        RangePreset::ThisMonth.resolve(today, &calendar()),
        r(d(2024, 3, 1), d(2024, 3, 31))
    );
    assert_eq!(
        RangePreset::LastMonth.resolve(today, &calendar()),
        r(d(2024, 2, 1), d(2024, 2, 29)),
        "leap February"
    );
    assert_eq!(
        RangePreset::LastMonth.resolve(d(2026, 3, 31), &calendar()),
        r(d(2026, 2, 1), d(2026, 2, 28))
    );
    assert_eq!(
        RangePreset::LastMonth.resolve(d(2026, 1, 1), &calendar()),
        r(d(2025, 12, 1), d(2025, 12, 31)),
        "across the year end"
    );
}

#[test]
fn quarter_and_year_presets_follow_the_fiscal_year() {
    let today = d(2026, 10, 19);
    assert_eq!(
        RangePreset::ThisQuarter.resolve(today, &calendar()),
        r(d(2026, 10, 1), d(2026, 12, 31))
    );
    assert_eq!(
        RangePreset::ThisYear.resolve(today, &calendar()),
        r(d(2026, 1, 1), d(2026, 12, 31))
    );
    assert_eq!(
        RangePreset::LastYear.resolve(today, &calendar()),
        r(d(2025, 1, 1), d(2025, 12, 31))
    );
    // A July fiscal year: FY Jul 2026 - Jun 2027, second quarter Oct-Dec.
    assert_eq!(
        RangePreset::ThisQuarter.resolve(today, &july()),
        r(d(2026, 10, 1), d(2026, 12, 31))
    );
    assert_eq!(
        RangePreset::ThisYear.resolve(today, &july()),
        r(d(2026, 7, 1), d(2027, 6, 30))
    );
    assert_eq!(
        RangePreset::LastYear.resolve(today, &july()),
        r(d(2025, 7, 1), d(2026, 6, 30))
    );
    // An April fiscal year in May: first quarter Apr-Jun.
    let april = FiscalYearSpec::Calendar { start_month: 4 };
    assert_eq!(
        RangePreset::ThisQuarter.resolve(d(2026, 5, 2), &april),
        r(d(2026, 4, 1), d(2026, 6, 30))
    );
}

#[test]
fn all_dates_and_custom() {
    assert_eq!(
        RangePreset::AllDates.resolve(d(2026, 10, 19), &calendar()),
        r(CalendarDate::min(), CalendarDate::max())
    );
    let c = RangePreset::custom(d(2026, 1, 5), d(2026, 2, 6)).unwrap();
    assert_eq!(
        c.resolve(d(2030, 1, 1), &calendar()),
        r(d(2026, 1, 5), d(2026, 2, 6))
    );
    assert_eq!(
        RangePreset::custom(d(2026, 2, 7), d(2026, 2, 6)),
        Err(RangeProblem::FromAfterTo)
    );
    assert!(RangePreset::custom(d(2026, 2, 6), d(2026, 2, 6)).is_ok());
    // A hand-built backwards custom range is still read the right way round.
    let back = RangePreset::Custom {
        from: d(2026, 3, 1),
        to: d(2026, 2, 1),
    };
    assert_eq!(
        back.resolve(d(2026, 1, 1), &calendar()),
        r(d(2026, 2, 1), d(2026, 3, 1))
    );
}

#[test]
fn ranges_at_the_edges_of_the_supported_dates_do_not_fail() {
    let first = CalendarDate::min();
    assert_eq!(
        RangePreset::Last90Days.resolve(first, &calendar()).start,
        first
    );
    assert_eq!(
        RangePreset::LastMonth.resolve(first, &calendar()),
        r(first, d(1900, 1, 31))
    );
    let last = CalendarDate::max();
    assert_eq!(
        RangePreset::ThisMonth.resolve(last, &calendar()),
        r(d(2199, 12, 1), last)
    );
    for p in RangePreset::PRESETS {
        for today in [first, last, d(2026, 2, 28)] {
            for spec in [calendar(), july()] {
                let got = p.resolve(today, &spec);
                assert!(got.start <= got.end, "{p:?} {today}");
            }
        }
    }
}

#[test]
fn every_preset_has_a_label_and_a_remembered_form() {
    let mut labels = std::collections::HashSet::new();
    for p in RangePreset::PRESETS {
        assert!(labels.insert(p.label()));
        assert_eq!(RangePreset::from_pref(&p.to_pref()), Some(p));
    }
    let c = RangePreset::custom(d(2026, 1, 5), d(2026, 2, 6)).unwrap();
    assert_eq!(c.to_pref(), "custom:2026-01-05..2026-02-06");
    assert_eq!(RangePreset::from_pref(&c.to_pref()), Some(c));
    for bad in [
        "",
        "next_decade",
        "custom:",
        "custom:2026-01-05",
        "custom:2026-02-06..2026-01-05",
        "custom:2026-13-01..2026-12-31",
    ] {
        assert_eq!(RangePreset::from_pref(bad), None, "{bad}");
    }
}

// ----- sort and filters ---------------------------------------------------

#[test]
fn sort_choices_map_to_the_engine() {
    assert_eq!(SortChoice::default(), SortChoice::DateOldest);
    let wire: Vec<_> = SortChoice::ALL
        .iter()
        .map(|s| (s.sort_by_wire(), s.descending(), s.is_date()))
        .collect();
    assert_eq!(
        wire,
        [
            ("date", false, true),
            ("date", true, true),
            ("ref_no", false, false),
            ("amount", false, false),
            ("type", false, false),
        ]
    );
    let labels: std::collections::HashSet<_> = SortChoice::ALL.iter().map(|s| s.label()).collect();
    assert_eq!(labels.len(), SortChoice::ALL.len());
}

#[test]
fn filters_activity_and_checks() {
    let mut f = Filters::default();
    assert!(!f.is_active());
    f.payee = Some("   ".into());
    assert!(!f.is_active(), "a blank payee is no filter");
    f.payee = Some(" Acme ".into());
    assert_eq!(f.payee_text(), Some("Acme"));
    assert!(f.is_active());
    assert!(!f.needs_register_exact());
    f.uncleared = true;
    assert!(f.needs_register_exact());
    f.clear();
    assert_eq!(f, Filters::default());
    f.tag = Some(tag(1));
    assert!(f.is_active() && f.needs_register_exact());
    f.clear();
    f.min = Some(money("10.00"));
    assert!(f.is_active());
    assert_eq!(f.check(), Ok(()));
    f.max = Some(money("9.99"));
    assert_eq!(f.check(), Err(FilterProblem::MinAboveMax));
    f.max = Some(money("10"));
    assert_eq!(f.check(), Ok(()), "equal ends are fine");
    f.min = None;
    assert!(f.is_active());
}

#[test]
fn filter_debug_shows_no_payee_or_amounts() {
    let f = Filters {
        payee: Some("Secret Payee".into()),
        min: Some(money("1234.56")),
        ..Filters::default()
    };
    let s = format!("{f:?}");
    assert!(!s.contains("Secret") && !s.contains("1234"), "{s}");
}

// ----- prefs --------------------------------------------------------------

#[test]
fn prefs_round_trip() {
    for p in [
        RegisterPrefs::default(),
        RegisterPrefs {
            range: RangePreset::ThisMonth,
            sort: SortChoice::Amount,
            one_line: true,
        },
        RegisterPrefs {
            range: RangePreset::custom(d(2026, 1, 1), d(2026, 3, 31)).unwrap(),
            sort: SortChoice::DateNewest,
            one_line: false,
        },
    ] {
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"v\":1"), "{json}");
        let back: RegisterPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }
    let json = serde_json::to_string(&RegisterPrefs {
        range: RangePreset::ThisMonth,
        ..RegisterPrefs::default()
    })
    .unwrap();
    assert!(json.contains("this_month"), "stored relative: {json}");
}

#[test]
fn prefs_read_leniently() {
    let p: RegisterPrefs = serde_json::from_str(
        r#"{"v":7,"range":"next_decade","sort":"by_mood","one_line":true,"zoom":3}"#,
    )
    .unwrap();
    assert_eq!(
        p,
        RegisterPrefs {
            one_line: true,
            ..RegisterPrefs::default()
        }
    );
    let p: RegisterPrefs = serde_json::from_str("{}").unwrap();
    assert_eq!(p, RegisterPrefs::default());
    let p: RegisterPrefs = serde_json::from_str(r#"{"sort":"type"}"#).unwrap();
    assert_eq!(p.sort, SortChoice::Type);
}

// ----- balances, classes, labels -------------------------------------------

#[test]
fn balance_mode_hides_only_filtered_balances_before_r1() {
    assert_eq!(BalanceMode::choose(false, false), BalanceMode::Engine);
    assert_eq!(BalanceMode::choose(false, true), BalanceMode::Engine);
    assert_eq!(BalanceMode::choose(true, true), BalanceMode::Engine);
    assert_eq!(
        BalanceMode::choose(true, false),
        BalanceMode::HiddenFiltered
    );
    assert_eq!(BalanceMode::Engine.tooltip(), None);
    assert!(BalanceMode::HiddenFiltered.tooltip().is_some());
}

#[test]
fn account_classes() {
    use AccountClass as C;
    let cases = [
        (AccountKind::Asset, Some("bank"), C::Bank),
        (
            AccountKind::Asset,
            Some("accounts_receivable"),
            C::Receivable,
        ),
        (AccountKind::Asset, Some("fixed_asset"), C::OtherAsset),
        (AccountKind::Asset, None, C::OtherAsset),
        (AccountKind::Liability, Some("credit_card"), C::CreditCard),
        (AccountKind::Liability, Some("accounts_payable"), C::Payable),
        (
            AccountKind::Liability,
            Some("long_term_liability"),
            C::OtherLiability,
        ),
        (AccountKind::Equity, Some("owners_equity"), C::Equity),
        (AccountKind::Expense, Some("bank"), C::IncomeOrExpense),
        (AccountKind::Income, None, C::IncomeOrExpense),
        (AccountKind::Cogs, None, C::IncomeOrExpense),
        (AccountKind::OtherIncome, None, C::IncomeOrExpense),
        (AccountKind::OtherExpense, None, C::IncomeOrExpense),
    ];
    for (kind, detail, want) in cases {
        assert_eq!(AccountClass::of(kind, detail), want, "{kind:?} {detail:?}");
    }
    assert!(!C::IncomeOrExpense.has_register());
    assert!(C::Equity.has_register() && C::Receivable.has_register());
    assert!(C::Receivable.needs_name() && C::Payable.needs_name());
    assert!(!C::Bank.needs_name());
}

#[test]
fn column_labels_by_class() {
    let l = |c| {
        let x = ColumnLabels::for_class(c);
        (x.payment.as_str(), x.deposit.as_str())
    };
    assert_eq!(
        l(AccountClass::Bank),
        ("register.col.payment", "register.col.deposit")
    );
    assert_eq!(
        l(AccountClass::CreditCard),
        ("register.col.charge", "register.col.card_payment")
    );
    for c in [AccountClass::Receivable, AccountClass::OtherAsset] {
        assert_eq!(l(c), ("register.col.decrease", "register.col.increase"));
    }
    for c in [
        AccountClass::Payable,
        AccountClass::OtherLiability,
        AccountClass::Equity,
    ] {
        assert_eq!(l(c), ("register.col.increase", "register.col.decrease"));
    }
}

#[test]
fn display_sign_flips_only_credit_normal_stored_balances() {
    use super::BalanceSource::{AccountBalance, Register};
    const ALL: [AccountClass; 7] = [
        AccountClass::Bank,
        AccountClass::Receivable,
        AccountClass::OtherAsset,
        AccountClass::CreditCard,
        AccountClass::Payable,
        AccountClass::OtherLiability,
        AccountClass::Equity,
    ];
    let v = money("-1084.6600");
    // Register figures come in the account's natural sign: never flipped.
    for c in ALL {
        let s = DisplaySign::for_source(c, Register);
        assert_eq!(s, DisplaySign::AsSent, "{c:?}");
        assert_eq!(s.apply(v).unwrap(), v);
    }
    for c in [
        AccountClass::Bank,
        AccountClass::Receivable,
        AccountClass::OtherAsset,
    ] {
        assert_eq!(
            DisplaySign::for_source(c, AccountBalance),
            DisplaySign::AsSent
        );
    }
    // The stored balance is debit-positive: a card owing 1,084.66 stores
    // -1084.66 and shows 1,084.66 (Q-241).
    for c in [
        AccountClass::CreditCard,
        AccountClass::Payable,
        AccountClass::OtherLiability,
        AccountClass::Equity,
    ] {
        let s = DisplaySign::for_source(c, AccountBalance);
        assert_eq!(s, DisplaySign::Negated);
        assert!(s.negates());
        assert_eq!(s.apply(v).unwrap(), money("1084.66"));
        assert_eq!(s.apply(v).unwrap().to_wire(), money("1084.6600").to_wire());
        assert_eq!(s.apply(s.apply(v).unwrap()).unwrap(), v, "a pure sign flip");
    }
    assert_eq!(
        DisplaySign::Negated.apply(Money::ZERO).unwrap(),
        Money::ZERO
    );
}

#[test]
fn stored_balances_flip_for_every_credit_normal_kind() {
    use AccountKind as K;
    for k in [K::Liability, K::Equity, K::Income, K::OtherIncome] {
        assert_eq!(
            DisplaySign::for_stored_balance(k),
            DisplaySign::Negated,
            "{k:?}"
        );
    }
    for k in [K::Asset, K::Cogs, K::Expense, K::OtherExpense] {
        assert_eq!(
            DisplaySign::for_stored_balance(k),
            DisplaySign::AsSent,
            "{k:?}"
        );
    }
    // The register classes agree with the kind rule.
    for (kind, detail) in [
        (K::Asset, Some("bank")),
        (K::Asset, None),
        (K::Liability, Some("credit_card")),
        (K::Liability, Some("accounts_payable")),
        (K::Liability, None),
        (K::Equity, None),
    ] {
        assert_eq!(
            DisplaySign::for_source(
                AccountClass::of(kind, detail),
                BalanceSource::AccountBalance
            ),
            DisplaySign::for_stored_balance(kind),
            "{kind:?} {detail:?}"
        );
    }
}

#[test]
fn engine_fields_land_in_the_column_of_their_side() {
    // Debit-normal: the engine's `payment` is the credit (first column).
    for c in [
        AccountClass::Bank,
        AccountClass::Receivable,
        AccountClass::OtherAsset,
    ] {
        assert_eq!(c.engine_payment_side(), Side::Payment, "{c:?}");
    }
    // Credit-normal: the engine's `payment` is the debit (second column),
    // so a card charge (a credit, sent as `deposit`) lands under Charge.
    for c in [
        AccountClass::CreditCard,
        AccountClass::Payable,
        AccountClass::OtherLiability,
        AccountClass::Equity,
    ] {
        assert_eq!(c.engine_payment_side(), Side::Deposit, "{c:?}");
    }
    let card = ColumnLabels::for_class(AccountClass::CreditCard);
    assert_eq!(card.payment.as_str(), "register.col.charge", "first column");
}

// ----- entry kinds ----------------------------------------------------------

const ALL_TAGS: [EntryKindTag; 4] = [
    EntryKindTag::Expense,
    EntryKindTag::Deposit,
    EntryKindTag::Transfer,
    EntryKindTag::JournalEntry,
];

const CLASSES: [AccountClass; 8] = [
    AccountClass::Bank,
    AccountClass::CreditCard,
    AccountClass::Receivable,
    AccountClass::Payable,
    AccountClass::OtherAsset,
    AccountClass::OtherLiability,
    AccountClass::Equity,
    AccountClass::IncomeOrExpense,
];

#[test]
fn entry_tags_read_the_engine_values() {
    for t in ALL_TAGS {
        assert_eq!(EntryKindTag::from_wire(t.wire()), Some(t));
    }
    assert_eq!(EntryKindTag::from_wire("check"), None);
    assert_eq!(EntryKindTag::from_wire(""), None);
}

/// The expected kind with every entry type allowed (Q-P12g).
fn expected(this: AccountClass, other: AccountClass, side: Side) -> EntryKind {
    use AccountClass as C;
    match (this, side) {
        (C::Bank | C::CreditCard, _) if matches!(other, C::Bank | C::CreditCard) => {
            EntryKind::Transfer
        }
        (C::Bank | C::CreditCard, _) if matches!(other, C::Receivable | C::Payable) => {
            EntryKind::JournalEntry
        }
        (C::Bank, Side::Payment) => EntryKind::Check,
        (C::Bank, Side::Deposit) => EntryKind::Deposit,
        (C::CreditCard, Side::Payment) => EntryKind::CardCharge,
        _ => EntryKind::JournalEntry,
    }
}

#[test]
fn infer_kind_table() {
    // Every class x side x other class x allowed subset.
    for this in CLASSES {
        for other in CLASSES {
            for side in [Side::Payment, Side::Deposit] {
                let want = expected(this, other, side);
                for mask in 0u8..16 {
                    let allowed: Vec<_> = ALL_TAGS
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| mask & (1 << i) != 0)
                        .map(|(_, t)| *t)
                        .collect();
                    let got = infer_kind(this, Some(other), side, &allowed);
                    if allowed.contains(&want.needs()) {
                        assert_eq!(got, Ok(want), "{this:?} {other:?} {side:?} {allowed:?}");
                    } else {
                        assert_eq!(
                            got,
                            Err(EntryProblem::KindNotAllowed),
                            "{this:?} {other:?} {side:?} {allowed:?}"
                        );
                    }
                }
            }
        }
        assert_eq!(
            infer_kind(this, None, Side::Payment, &ALL_TAGS),
            Err(EntryProblem::AccountMissing)
        );
    }
}

#[test]
fn infer_kind_with_the_engine_lists() {
    // `getAllowedEntryTypes` in the engine's register service.
    let bank = [
        EntryKindTag::Expense,
        EntryKindTag::Deposit,
        EntryKindTag::Transfer,
        EntryKindTag::JournalEntry,
    ];
    let card = [
        EntryKindTag::Expense,
        EntryKindTag::Transfer,
        EntryKindTag::JournalEntry,
    ];
    let e = AccountClass::IncomeOrExpense;
    assert_eq!(
        infer_kind(AccountClass::Bank, Some(e), Side::Payment, &bank),
        Ok(EntryKind::Check)
    );
    assert_eq!(
        infer_kind(AccountClass::Bank, Some(e), Side::Deposit, &bank),
        Ok(EntryKind::Deposit)
    );
    assert_eq!(
        infer_kind(AccountClass::CreditCard, Some(e), Side::Payment, &card),
        Ok(EntryKind::CardCharge)
    );
    assert_eq!(
        infer_kind(
            AccountClass::CreditCard,
            Some(AccountClass::Bank),
            Side::Deposit,
            &card
        ),
        Ok(EntryKind::Transfer)
    );
    assert!(is_view_only(&[]), "A/R gets no entry types");
    assert!(!is_view_only(&card));
}

#[test]
fn entry_kinds_have_distinct_labels() {
    let kinds = [
        EntryKind::Check,
        EntryKind::Deposit,
        EntryKind::Transfer,
        EntryKind::CardCharge,
        EntryKind::JournalEntry,
    ];
    let labels: std::collections::HashSet<_> = kinds.iter().map(|k| k.label()).collect();
    assert_eq!(labels.len(), kinds.len());
}

// ----- entry checks ---------------------------------------------------------

fn bank_ctx(other: Option<AccountClass>) -> EntryContext<'static> {
    EntryContext {
        this_account: acct(1),
        this_class: AccountClass::Bank,
        other_class: other,
        allowed: &ALL_TAGS,
    }
}

fn check_draft() -> EntryDraft {
    EntryDraft {
        date: Some(d(2026, 10, 19)),
        number: " 4418 ".into(),
        payee: Some(contact(1)),
        account: Some(acct(2)),
        memo: "  ".into(),
        payment: Some(money("125.40")),
        deposit: None,
        tags: vec![tag(1)],
    }
}

#[test]
fn a_good_check_passes_trimmed() {
    let got = check(
        &check_draft(),
        &bank_ctx(Some(AccountClass::IncomeOrExpense)),
    )
    .unwrap();
    assert_eq!(got.kind, EntryKind::Check);
    assert_eq!(got.side, Side::Payment);
    assert_eq!(got.amount, money("125.40"));
    assert_eq!(
        got.amount.to_wire(),
        money("125.40").to_wire(),
        "exact as typed"
    );
    assert_eq!(got.number.as_deref(), Some("4418"));
    assert_eq!(got.memo, None);
    assert_eq!(got.tag, Some(tag(1)));
    assert_eq!(got.account, acct(2));
    let s = format!("{got:?}");
    assert!(!s.contains("125") && !s.contains("4418"), "{s}");
}

#[test]
fn entry_problems() {
    let ctx = bank_ctx(Some(AccountClass::IncomeOrExpense));
    let problems =
        |draft: &EntryDraft, ctx: &EntryContext<'_>| check(draft, ctx).err().unwrap_or_default();

    let mut x = check_draft();
    x.date = None;
    assert_eq!(problems(&x, &ctx), [EntryProblem::DateMissing]);

    let mut x = check_draft();
    x.payment = None;
    assert_eq!(problems(&x, &ctx), [EntryProblem::AmountMissing]);

    let mut x = check_draft();
    x.deposit = Some(money("1"));
    assert_eq!(problems(&x, &ctx), [EntryProblem::BothSides]);

    for bad in ["0", "0.00", "-5.00"] {
        let mut x = check_draft();
        x.payment = Some(money(bad));
        assert_eq!(
            problems(&x, &ctx),
            [EntryProblem::AmountNotPositive],
            "{bad}"
        );
    }

    let mut x = check_draft();
    x.account = None;
    assert_eq!(
        problems(&x, &bank_ctx(None)),
        [EntryProblem::AccountMissing]
    );

    let mut x = check_draft();
    x.account = Some(acct(1));
    assert_eq!(
        problems(&x, &bank_ctx(Some(AccountClass::Bank))),
        [EntryProblem::SameAccount],
        "a transfer within the same register is refused"
    );

    assert_eq!(
        problems(&check_draft(), &bank_ctx(None)),
        [EntryProblem::AccountGone]
    );

    let mut x = check_draft();
    x.tags.push(tag(2));
    assert_eq!(problems(&x, &ctx), [EntryProblem::TooManyTags]);

    let only_deposits = [EntryKindTag::Deposit];
    let narrow = EntryContext {
        allowed: &only_deposits,
        ..ctx
    };
    assert_eq!(
        problems(&check_draft(), &narrow),
        [EntryProblem::KindNotAllowed]
    );

    // Several at once, all reported.
    let x = EntryDraft {
        number: "1".into(),
        ..EntryDraft::default()
    };
    assert_eq!(
        problems(&x, &bank_ctx(None)),
        [
            EntryProblem::DateMissing,
            EntryProblem::AccountMissing,
            EntryProblem::AmountMissing
        ]
    );
}

#[test]
fn receivables_and_payables_need_a_name() {
    // A bank line against A/R needs a name and becomes a journal entry.
    let mut x = check_draft();
    x.payee = None;
    let ar = bank_ctx(Some(AccountClass::Receivable));
    assert_eq!(check(&x, &ar).err(), Some(vec![EntryProblem::NameRequired]));
    x.payee = Some(contact(3));
    assert_eq!(check(&x, &ar).map(|c| c.kind), Ok(EntryKind::JournalEntry));

    // The A/P register itself (Q-P12p).
    let ap_ctx = EntryContext {
        this_account: acct(9),
        this_class: AccountClass::Payable,
        other_class: Some(AccountClass::IncomeOrExpense),
        allowed: &[EntryKindTag::JournalEntry],
    };
    let mut x = check_draft();
    x.payee = None;
    assert_eq!(
        check(&x, &ap_ctx).err(),
        Some(vec![EntryProblem::NameRequired])
    );
    x.payee = Some(contact(4));
    assert_eq!(
        check(&x, &ap_ctx).map(|c| c.kind),
        Ok(EntryKind::JournalEntry)
    );

    // A/R against A/P: both lines on named accounts is allowed (Q-206,
    // owner 2026-10-25); the name is still required.
    let both = EntryContext {
        other_class: Some(AccountClass::Receivable),
        ..ap_ctx
    };
    assert_eq!(
        check(&x, &both).map(|c| c.kind),
        Ok(EntryKind::JournalEntry)
    );
    x.payee = None;
    assert_eq!(
        check(&x, &both).err(),
        Some(vec![EntryProblem::NameRequired])
    );
}

#[test]
fn problems_name_their_fields() {
    let all = [
        (EntryProblem::DateMissing, EntryField::Date),
        (EntryProblem::AmountMissing, EntryField::Amount),
        (EntryProblem::BothSides, EntryField::Amount),
        (EntryProblem::AmountNotPositive, EntryField::Amount),
        (EntryProblem::AccountMissing, EntryField::Account),
        (EntryProblem::AccountGone, EntryField::Account),
        (EntryProblem::SameAccount, EntryField::Account),
        (EntryProblem::NameRequired, EntryField::Payee),
        (EntryProblem::TooManyTags, EntryField::Tag),
        (EntryProblem::KindNotAllowed, EntryField::Account),
    ];
    let mut keys = std::collections::HashSet::new();
    for (p, f) in all {
        assert_eq!(p.field(), f, "{p:?}");
        assert!(keys.insert(p.key()), "{p:?}");
        assert!(p.key().as_str().starts_with("register.entry.problem."));
    }
}

#[test]
fn draft_helpers_and_redacted_debug() {
    let mut x = check_draft();
    assert!(!x.is_empty());
    assert_eq!(x.side(), Some(Side::Payment));
    x.clear_keep_date();
    assert_eq!(x.date, Some(d(2026, 10, 19)));
    assert!(x.is_empty(), "only the date is kept");
    assert_eq!(x.side(), None);
    x.deposit = Some(money("3"));
    assert_eq!(x.side(), Some(Side::Deposit));
    let mut y = check_draft();
    y.memo = "Private memo".into();
    let s = format!("{y:?}");
    assert!(
        !s.contains("Private") && !s.contains("125") && !s.contains("4418"),
        "{s}"
    );
}

// ----- the page window --------------------------------------------------------

type Row = (u64, u64);

fn page_rows(version: u64, total: u64, page: PageNo) -> Vec<Row> {
    (page.first_row()..total.min(page.first_row() + PAGE))
        .map(|i| (version, i))
        .collect()
}

#[test]
fn first_request_learns_the_size_then_fetches_visible_pages() {
    let mut w: WindowCache<Row> = WindowCache::new(1);
    assert_eq!(w.row(0), Some(RowView::Loading));
    let first = w.wanted(0..30);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].page, PageNo(0));
    assert_eq!(first[0].page.engine_page(), 1);
    assert!(
        w.wanted(5000..5030).is_empty(),
        "page one is still in flight"
    );
    assert_eq!(
        w.accept(first[0], 1000, page_rows(0, 1000, PageNo(0))),
        Reply::Taken
    );
    assert_eq!(w.total_rows(), Some(1000));
    assert_eq!(w.total_pages(), Some(5));
    assert_eq!(w.last_page(), Some(PageNo(4)));
    assert_eq!(w.row(0), Some(RowView::Ready(&(0, 0))));
    assert_eq!(w.row(1000), None);

    // Jump to the bottom (newest at the bottom): the last page, then the one
    // before it.
    let reqs = w.wanted(980..1000);
    let pages: Vec<_> = reqs.iter().map(|r| r.page).collect();
    assert_eq!(pages, [PageNo(4), PageNo(3)]);
    assert_eq!(w.row(990), Some(RowView::Loading));
    for q in reqs {
        w.accept(q, 1000, page_rows(0, 1000, q.page));
    }
    assert_eq!(w.row(999), Some(RowView::Ready(&(0, 999))));
    assert!(w.wanted(980..1000).is_empty(), "everything wanted is held");

    // A view across two pages asks both and one either side.
    let pages: Vec<_> = w.wanted(390..420).iter().map(|r| r.page).collect();
    assert_eq!(pages, [PageNo(1), PageNo(2)]);
}

#[test]
fn invalidate_keeps_old_rows_as_stale_and_drops_late_replies() {
    let mut w: WindowCache<Row> = WindowCache::new(1);
    let q = w.wanted(0..10)[0];
    w.accept(q, 300, page_rows(0, 300, q.page));
    let next = w.wanted(0..10);
    assert_eq!(next.len(), 1, "page two as prefetch");
    w.invalidate();
    assert!(!w.is_current(&next[0]));
    assert_eq!(
        w.accept(next[0], 300, page_rows(0, 300, PageNo(1))),
        Reply::Dropped
    );
    assert_eq!(w.row(3), Some(RowView::Stale(&(0, 3))), "no flash to empty");
    assert_eq!(w.row(250), Some(RowView::Loading));
    let again = w.wanted(0..10);
    assert_eq!(
        again.iter().map(|r| r.page).collect::<Vec<_>>(),
        [PageNo(0), PageNo(1)]
    );
    assert!(again.iter().all(|r| r.generation == w.generation()));
    w.accept(again[0], 301, page_rows(1, 301, PageNo(0)));
    assert_eq!(w.row(3), Some(RowView::Ready(&(1, 3))));
    assert_eq!(w.total_rows(), Some(301));
}

#[test]
fn scrolling_away_drops_the_old_request() {
    let mut w: WindowCache<Row> = WindowCache::new(1);
    let q = w.wanted(0..10)[0];
    w.accept(q, 100_000, page_rows(0, 100_000, q.page));
    let far = w.wanted(50_000..50_030);
    assert!(far.iter().all(|r| w.is_current(r)));
    let farther = w.wanted(90_000..90_030);
    assert!(far.iter().all(|r| !w.is_current(r)), "left the viewport");
    assert_eq!(
        w.accept(far[0], 100_000, page_rows(0, 100_000, far[0].page)),
        Reply::Dropped
    );
    assert!(farther.iter().all(|r| w.is_current(r)));
}

#[test]
fn failures_show_and_retry() {
    let mut w: WindowCache<Row> = WindowCache::new(1);
    let q = w.wanted(0..10)[0];
    assert_eq!(w.fail(q), Reply::Taken);
    assert_eq!(w.row(0), Some(RowView::Failed));
    assert!(w.wanted(0..10).is_empty(), "no retry loop");
    w.retry(PageNo(0));
    let q = w.wanted(0..10)[0];
    assert_eq!(w.fail(q), Reply::Taken);
    assert_eq!(w.fail(q), Reply::Dropped, "only once");
    w.invalidate();
    assert_eq!(w.row(0), Some(RowView::Loading), "a refresh retries");
}

#[test]
fn a_new_query_forgets_everything_and_a_smaller_total_clamps() {
    let mut w: WindowCache<Row> = WindowCache::new(1);
    assert!(!w.set_query(1));
    let q = w.wanted(0..10)[0];
    w.accept(q, 450, page_rows(0, 450, q.page));
    for q in w.wanted(400..450) {
        w.accept(q, 450, page_rows(0, 450, q.page));
    }
    assert_eq!(w.clamp_row(449), 449);
    w.invalidate();
    let reqs = w.wanted(400..450);
    // The register shrank to 120 lines: page three is gone, the anchor moves.
    let first = reqs.iter().find(|r| r.page == PageNo(2)).copied().unwrap();
    w.accept(first, 120, Vec::new());
    assert_eq!(w.total_pages(), Some(1));
    assert_eq!(w.row(449), None);
    assert_eq!(w.clamp_row(449), 119);
    assert!(w.cached_pages() <= 1);

    assert!(w.set_query(2));
    assert_eq!(w.query_fp(), 2);
    assert_eq!(w.total_rows(), None);
    assert_eq!(w.cached_pages(), 0);
    assert_eq!(w.in_flight(), 0);
    assert_eq!(w.clamp_row(10), 0);
}

#[test]
fn an_empty_register_is_asked_once_per_generation() {
    let mut w: WindowCache<Row> = WindowCache::new(1);
    let q = w.wanted(0..30)[0];
    w.accept(q, 0, Vec::new());
    assert_eq!(w.total_pages(), Some(0));
    assert_eq!(w.last_page(), None);
    assert_eq!(w.row(0), None);
    assert_eq!(w.wanted(0..30).len(), 0);
    w.invalidate();
    let again = w.wanted(0..30);
    assert_eq!(
        again.iter().map(|r| r.page).collect::<Vec<_>>(),
        [PageNo(0)]
    );
}

#[test]
fn memory_is_capped() {
    let total = 200 * PAGE;
    let mut w: WindowCache<Row> = WindowCache::new(1);
    let q = w.wanted(0..10)[0];
    w.accept(q, total, page_rows(0, total, q.page));
    for p in 0..200 {
        for q in w.wanted(p * PAGE..p * PAGE + 30) {
            w.accept(q, total, page_rows(0, total, q.page));
        }
        assert!(w.cached_pages() <= MAX_PAGES);
    }
    // The most recent pages are the ones kept.
    assert_eq!(w.row(total - 1), Some(RowView::Ready(&(0, total - 1))));
    assert_eq!(w.row(0), Some(RowView::Loading));
}

#[derive(Debug, Clone)]
enum Op {
    Scroll(u64),
    Deliver(usize),
    Fail(usize),
    Change(u64),
    Query,
    Retry(u64),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0u64..20_000).prop_map(Op::Scroll),
        5 => any::<usize>().prop_map(Op::Deliver),
        1 => any::<usize>().prop_map(Op::Fail),
        1 => (0u64..20_000).prop_map(Op::Change),
        1 => Just(Op::Query),
        1 => (0u64..100).prop_map(Op::Retry),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// §6.1: the window never shows a row the fake engine did not send for
    /// the current data, never asks twice or past the end, and stays within
    /// its memory cap.
    #[test]
    fn window_cache_property(start in 0u64..20_000, ops in prop::collection::vec(op(), 1..60)) {
        // The fake engine: data version and size. Each row is (version, index).
        let mut version = 0u64;
        let mut total = start;
        let mut fp = 1u64;
        let mut w: WindowCache<Row> = WindowCache::new(fp);
        // Requests handed out, with the engine version they were asked under.
        let mut out: Vec<(PageRequest, u64)> = Vec::new();
        for op in ops {
            match op {
                Op::Scroll(at) => {
                    let reqs = w.wanted(at..at + 40);
                    let mut seen = std::collections::HashSet::new();
                    for q in &reqs {
                        prop_assert!(seen.insert(q.page), "asked twice in one call");
                        prop_assert!(
                            !out.iter().any(|(o, _)| o.page == q.page && w.is_current(o)),
                            "asked a page already in flight"
                        );
                        prop_assert_eq!(q.generation, w.generation());
                        if let Some(pages) = w.total_pages() {
                            prop_assert!(q.page.0 < pages.max(1), "past the end");
                        }
                    }
                    out.extend(reqs.into_iter().map(|q| (q, version)));
                }
                Op::Deliver(i) if !out.is_empty() => {
                    let (q, v) = out.remove(i % out.len());
                    let was_current = w.is_current(&q);
                    let got = w.accept(q, total, page_rows(v, total, q.page));
                    prop_assert_eq!(got == Reply::Taken, was_current);
                    if was_current {
                        prop_assert_eq!(v, version, "a current request is for current data");
                    }
                }
                Op::Fail(i) if !out.is_empty() => {
                    let (q, _) = out.remove(i % out.len());
                    let was_current = w.is_current(&q);
                    prop_assert_eq!(w.fail(q) == Reply::Taken, was_current);
                }
                Op::Deliver(_) | Op::Fail(_) => {}
                Op::Change(new_total) => {
                    version += 1;
                    total = new_total;
                    w.invalidate();
                }
                Op::Query => {
                    version += 1;
                    fp += 1;
                    prop_assert!(w.set_query(fp));
                }
                Op::Retry(p) => w.retry(PageNo(p)),
            }
            prop_assert!(w.cached_pages() <= MAX_PAGES);
            // Every Ready row is the engine's row for the current data.
            let known = w.total_rows().unwrap_or(0);
            for p in 0..known.div_ceil(PAGE) {
                for i in [p * PAGE, p * PAGE + 7, p * PAGE + PAGE - 1] {
                    match w.row(i) {
                        Some(RowView::Ready(row)) => {
                            prop_assert_eq!(*row, (version, i));
                            prop_assert!(i < total);
                        }
                        Some(RowView::Stale(row)) => prop_assert!(row.0 < version),
                        Some(RowView::Loading | RowView::Failed) | None => {}
                    }
                }
            }
        }
    }
}
