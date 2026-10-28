use std::collections::BTreeSet;

use cl_core::ids::TagGroupId;
use proptest::prelude::*;

use super::*;
use crate::forms::FormState;
use crate::lists::tags::{GroupEntry, TagEntry};

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn uuid(n: u64) -> String {
    format!("00000000-0000-0000-0000-{n:012}")
}

fn account(n: u64) -> AccountId {
    AccountId::parse(&uuid(n)).expect("id")
}

fn contact(n: u64) -> ContactId {
    ContactId::parse(&uuid(n)).expect("id")
}

fn tag(n: u64) -> TagId {
    TagId::parse(&uuid(n)).expect("id")
}

fn group(n: u64) -> TagGroupId {
    TagGroupId::parse(&uuid(n)).expect("id")
}

fn date() -> CalendarDate {
    CalendarDate::from_ymd(2026, 10, 20).expect("date")
}

/// Groups: "Project" (one per line, order 1) with tags 11 "Install", 12
/// "Maintenance"; "Crew" (one per line, order 2) with 21 "Crew A"; tag 31
/// "Promo" ungrouped.
fn tags() -> TagList {
    let g = |id, name: &str, order| GroupEntry {
        id: group(id),
        name: name.into(),
        description: None,
        single: true,
        required_for: BTreeSet::new(),
        order,
    };
    let t = |id, name: &str, grp: Option<u64>| TagEntry {
        id: tag(id),
        number: None,
        name: name.into(),
        group: grp.map(group),
        color: None,
        description: None,
        active: true,
        usage: None,
    };
    TagList::new(
        vec![g(2, "Crew", 2), g(1, "Project", 1)],
        vec![
            t(31, "Promo", None),
            t(21, "Crew A", Some(2)),
            t(12, "Maintenance", Some(1)),
            t(11, "Install", Some(1)),
        ],
    )
}

fn line(keys: &mut RowKeyGen, acct: u64, amount: Option<&str>) -> FormLine {
    FormLine {
        account: Some(account(acct)),
        amount: amount.map(money),
        ..FormLine::blank(keys.next_key())
    }
}

/// A valid check for 1,286.40 on two lines.
fn valid(keys: &mut RowKeyGen) -> CheckForm {
    let mut f = CheckForm::new(
        Some(account(1010)),
        Some(date()),
        NumberField::Proposed(4418),
        keys,
    );
    f.payee = Some(contact(7));
    f.payee_name = " Greenway Nursery ".into();
    f.amount = Some(money("1286.40"));
    f.lines = FormLines::from_lines(
        vec![
            line(keys, 5110, Some("1200.00")),
            line(keys, 6350, Some("86.40")),
        ],
        keys,
    );
    f
}

fn run(f: &CheckForm) -> Result<CheckedCheck, Vec<CheckProblem>> {
    let list = tags();
    check(
        f,
        &CheckContext {
            tags: &list,
            editing: false,
        },
    )
}

#[test]
fn a_saved_check_keeps_a_number_unless_printed_later() {
    let list = tags();
    let editing = CheckContext {
        tags: &list,
        editing: true,
    };
    let mut f = valid(&mut RowKeyGen::new());
    f.number = NumberField::Typed("  ".into());
    assert!(run(&f).is_ok(), "a new check: the engine numbers it");
    assert_eq!(
        check(&f, &editing).expect_err("refused"),
        vec![CheckProblem::NumberInvalid]
    );
    f.number = NumberField::Typed("4418".into());
    assert!(check(&f, &editing).is_ok());
    f.number = NumberField::ToPrint {
        kept: String::new(),
        proposed: false,
    };
    f.print_later = true;
    assert!(check(&f, &editing).is_ok(), "printed later: no number");
}

fn problems(f: &CheckForm) -> Vec<CheckProblem> {
    run(f).expect_err("refused")
}

#[test]
fn a_valid_check_passes_trimmed() {
    let mut keys = RowKeyGen::new();
    let mut f = valid(&mut keys);
    f.memo = "  fall mulch ".into();
    f.address = "\n12 Elm St\nSpringfield  ".into();
    let c = run(&f).expect("valid");
    assert_eq!(c.number, Some(4418));
    assert_eq!(c.amount, money("1286.40"));
    assert_eq!(c.payee_name, "Greenway Nursery");
    assert_eq!(c.memo.as_deref(), Some("fall mulch"));
    assert_eq!(c.address.as_deref(), Some("12 Elm St\nSpringfield"));
    assert_eq!(c.lines.len(), 2, "the blank entry line is not sent");
    assert_eq!(c.lines[1].amount, money("86.40"));
    assert!(!c.print_later);
}

#[test]
fn every_problem() {
    let mut keys = RowKeyGen::new();
    let base = valid(&mut keys);
    let case = |edit: &dyn Fn(&mut CheckForm)| {
        let mut f = base.clone();
        edit(&mut f);
        problems(&f)
    };
    assert_eq!(case(&|f| f.bank = None), vec![CheckProblem::BankMissing]);
    assert_eq!(case(&|f| f.date = None), vec![CheckProblem::DateMissing]);
    assert_eq!(case(&|f| f.payee = None), vec![CheckProblem::PayeeMissing]);
    assert_eq!(
        case(&|f| f.amount = None),
        vec![CheckProblem::AmountMissing],
        "no amount: no lines comparison either"
    );
    let zero = case(&|f| f.amount = Some(Money::ZERO));
    assert_eq!(zero[0], CheckProblem::AmountNotPositive);
    assert!(matches!(zero[1], CheckProblem::LinesDisagree { .. }));
    assert_eq!(
        case(&|f| f.lines.lines[1].amount = Some(money("86.39"))),
        vec![CheckProblem::LinesDisagree {
            difference: money("0.01")
        }]
    );
    assert_eq!(
        case(&|f| f.lines.lines[1].amount = Some(money("86.41"))),
        vec![CheckProblem::LinesDisagree {
            difference: money("-0.01")
        }]
    );
    let k0 = base.lines.lines[0].key;
    let k1 = base.lines.lines[1].key;
    assert_eq!(
        case(&|f| f.lines.lines[0].account = None),
        vec![CheckProblem::LineAccountMissing(k0)]
    );
    assert_eq!(
        case(&|f| {
            f.lines.lines[1].amount = None;
            f.amount = Some(money("1200"));
        }),
        vec![CheckProblem::LineAmountNotPositive(k1)]
    );
    assert_eq!(
        case(&|f| {
            f.lines.lines[1].amount = Some(money("-86.40"));
            f.amount = Some(money("1113.60"));
        }),
        vec![CheckProblem::LineAmountNotPositive(k1)]
    );
    assert_eq!(
        case(&|f| f.lines.lines[0].billable = true),
        vec![CheckProblem::BillableNeedsCustomer(k0)]
    );
    assert!(
        run(&{
            let mut f = base.clone();
            f.lines.lines[0].billable = true;
            f.lines.lines[0].customer = Some(contact(9));
            f
        })
        .is_ok()
    );
}

#[test]
fn every_problem_number_address_and_size() {
    let mut keys = RowKeyGen::new();
    let base = valid(&mut keys);
    let case = |edit: &dyn Fn(&mut CheckForm)| {
        let mut f = base.clone();
        edit(&mut f);
        problems(&f)
    };
    for bad in ["44a", "-1", "0", "1.5", "2147483648", "99999999999"] {
        assert_eq!(
            case(&|f| f.number = NumberField::Typed(bad.into())),
            vec![CheckProblem::NumberInvalid],
            "{bad}"
        );
    }
    assert_eq!(
        case(&|f| f.number = NumberField::Proposed(0)),
        vec![CheckProblem::NumberInvalid]
    );
    assert_eq!(
        case(&|f| f.number = NumberField::ToPrint {
            kept: String::new(),
            proposed: false
        }),
        vec![CheckProblem::NumberInvalid],
        "To print without Print later"
    );
    assert_eq!(
        case(&|f| f.print_later = true),
        vec![CheckProblem::NumberWithPrintLater]
    );
    assert_eq!(
        case(&|f| f.address = "x".repeat(ADDRESS_MAX + 1)),
        vec![CheckProblem::AddressTooLong]
    );
    assert!(
        run(&{
            let mut f = base.clone();
            f.address = "é".repeat(ADDRESS_MAX);
            f
        })
        .is_ok(),
        "the limit counts characters"
    );
    assert_eq!(
        case(&|f| {
            f.amount = Some(money("1000000000000.00"));
            f.lines.lines[0].amount = Some(money("999999999913.60"));
        }),
        vec![CheckProblem::AmountTooLarge]
    );
    let many = case(&|f| {
        let mut k = RowKeyGen::after(f.lines.lines.iter().map(|l| l.key));
        let lines = (0..=MAX_LINES)
            .map(|_| line(&mut k, 5110, Some("1")))
            .collect();
        f.lines = FormLines::from_lines(lines, &mut k);
        f.amount = Some(money("501"));
    });
    assert_eq!(many, vec![CheckProblem::TooManyLines]);
    // Several at once, in field order.
    let all = case(&|f| {
        f.bank = None;
        f.date = None;
        f.payee = None;
        f.amount = None;
    });
    assert_eq!(
        all,
        vec![
            CheckProblem::BankMissing,
            CheckProblem::DateMissing,
            CheckProblem::PayeeMissing,
            CheckProblem::AmountMissing
        ]
    );
}

#[test]
fn no_lines_leaves_the_whole_amount_to_assign() {
    let mut keys = RowKeyGen::new();
    let mut f = valid(&mut keys);
    f.lines = FormLines::from_lines(Vec::new(), &mut keys);
    assert_eq!(
        problems(&f),
        vec![CheckProblem::LinesDisagree {
            difference: money("1286.40")
        }]
    );
}

#[test]
fn problems_name_their_field_and_message() {
    let k = RowKey(3);
    let all = [
        (CheckProblem::BankMissing, CheckField::Bank),
        (CheckProblem::DateMissing, CheckField::Date),
        (CheckProblem::PayeeMissing, CheckField::Payee),
        (CheckProblem::AmountMissing, CheckField::Amount),
        (CheckProblem::AmountNotPositive, CheckField::Amount),
        (
            CheckProblem::LinesDisagree {
                difference: Money::ZERO,
            },
            CheckField::Lines,
        ),
        (
            CheckProblem::LineAccountMissing(k),
            CheckField::LineAccount(k),
        ),
        (
            CheckProblem::LineAmountNotPositive(k),
            CheckField::LineAmount(k),
        ),
        (
            CheckProblem::BillableNeedsCustomer(k),
            CheckField::LineCustomer(k),
        ),
        (CheckProblem::NumberInvalid, CheckField::Number),
        (CheckProblem::NumberWithPrintLater, CheckField::Number),
        (CheckProblem::AddressTooLong, CheckField::Address),
        (CheckProblem::AmountTooLarge, CheckField::Amount),
        (CheckProblem::TooManyLines, CheckField::Lines),
    ];
    let mut keys = BTreeSet::new();
    for (p, field) in all {
        assert_eq!(p.field(), field, "{p:?}");
        assert!(p.key().0.starts_with("check.problem."), "{p:?}");
        keys.insert(p.key().0);
    }
    assert_eq!(keys.len(), all.len(), "one message per problem");
}

#[test]
fn propose_number_matrix() {
    assert_eq!(propose_number(None, None), 1001);
    assert_eq!(propose_number(Some(4418), None), 4418);
    assert_eq!(propose_number(None, Some(4417)), 4418);
    assert_eq!(propose_number(Some(4418), Some(4417)), 4418);
    assert_eq!(propose_number(Some(4400), Some(4417)), 4418, "highest wins");
    assert_eq!(propose_number(Some(5000), Some(4417)), 5000, "engine wins");
    assert_eq!(propose_number(None, Some(u32::MAX)), u32::MAX, "saturates");
}

#[test]
fn number_parse() {
    assert_eq!(parse_number(""), Ok(None));
    assert_eq!(parse_number("  "), Ok(None));
    assert_eq!(parse_number(" 4418 "), Ok(Some(4418)));
    assert_eq!(parse_number("007"), Ok(Some(7)));
    assert_eq!(parse_number("2147483647"), Ok(Some(MAX_CHECK_NUMBER)));
    assert_eq!(parse_number("+1"), Err(CheckProblem::NumberInvalid));
    assert_eq!(parse_number("1 000"), Err(CheckProblem::NumberInvalid));
}

#[test]
fn print_later_round_trip() {
    let mut keys = RowKeyGen::new();
    let mut f = valid(&mut keys);
    f.set_print_later(true);
    assert_eq!(f.number.text(), None, "shows To print");
    let c = run(&f).expect("print later is valid without a number");
    assert_eq!((c.number, c.print_later), (None, true));
    f.set_print_later(true);
    assert!(f.print_later, "ticking twice changes nothing");
    f.set_print_later(false);
    assert_eq!(f.number, NumberField::Proposed(4418), "proposed comes back");
    f.number = NumberField::Typed("12".into());
    f.set_print_later(true);
    assert_eq!(
        f.number,
        NumberField::ToPrint {
            kept: "12".into(),
            proposed: false
        }
    );
    f.set_print_later(false);
    assert_eq!(
        f.number,
        NumberField::Typed("12".into()),
        "typed comes back"
    );
    f.number = NumberField::Typed(String::new());
    f.print_later = true;
    assert!(run(&f).is_ok(), "an empty typed number with Print later");
}

#[test]
fn single_line_follows_the_header() {
    let mut keys = RowKeyGen::new();
    let mut f = CheckForm::new(
        Some(account(1010)),
        Some(date()),
        NumberField::Proposed(1),
        &mut keys,
    );
    // No line: nothing to follow.
    f.set_amount(Some(money("10")));
    assert_eq!(f.lines.used().count(), 0);
    // A single line with no amount takes the header.
    f.lines = FormLines::from_lines(vec![line(&mut keys, 5110, None)], &mut keys);
    f.set_amount(Some(money("25.00")));
    assert_eq!(f.lines.lines[0].amount, Some(money("25.00")));
    // ... and keeps following while it equals the previous header.
    f.set_amount(Some(money("30.00")));
    assert_eq!(f.lines.lines[0].amount, Some(money("30.00")));
    // An amount the owner changed on the line stays.
    f.lines.lines[0].amount = Some(money("12.00"));
    f.set_amount(Some(money("40.00")));
    assert_eq!(f.lines.lines[0].amount, Some(money("12.00")));
    // Two used lines: nothing follows.
    f.lines = FormLines::from_lines(
        vec![line(&mut keys, 5110, None), line(&mut keys, 6350, None)],
        &mut keys,
    );
    f.set_amount(Some(money("50.00")));
    assert!(f.lines.lines.iter().all(|l| l.amount.is_none()));
    // A line that only had the amount clears with the header and stays
    // a single entry line.
    f.lines = FormLines::from_lines(
        vec![FormLine {
            amount: Some(money("50.00")),
            ..FormLine::blank(RowKey(0))
        }],
        &mut keys,
    );
    f.set_amount(None);
    assert_eq!(f.lines.lines.len(), 1);
    assert!(f.lines.lines[0].is_blank());
}

#[test]
fn blank_header_takes_the_lines_total_on_leaving_the_grid() {
    let mut keys = RowKeyGen::new();
    let mut f = CheckForm::new(
        Some(account(1010)),
        Some(date()),
        NumberField::Proposed(1),
        &mut keys,
    );
    f.leave_lines();
    assert_eq!(f.amount, None, "no typed line amounts");
    f.lines = FormLines::from_lines(
        vec![
            line(&mut keys, 5110, Some("1200.00")),
            line(&mut keys, 6350, Some("86.40")),
        ],
        &mut keys,
    );
    f.leave_lines();
    assert_eq!(f.amount, Some(money("1286.40")));
    assert!(f.amount_from_lines);
    // It keeps following the lines while flagged.
    f.lines.lines[1].amount = Some(money("86.41"));
    f.leave_lines();
    assert_eq!(f.amount, Some(money("1286.41")));
    // Typing the header clears the flag; then nothing changes on its own.
    f.set_amount(Some(money("1300.00")));
    assert!(!f.amount_from_lines);
    f.leave_lines();
    assert_eq!(f.amount, Some(money("1300.00")));
    assert_eq!(
        f.totals_status(),
        Some(TotalsStatus::Remaining(money("13.59")))
    );
    // A flagged amount clears when the lines lose their amounts.
    let mut g = CheckForm::new(None, None, NumberField::Proposed(1), &mut keys);
    g.lines = FormLines::from_lines(vec![line(&mut keys, 1, Some("5"))], &mut keys);
    g.leave_lines();
    g.lines.lines[0].amount = None;
    g.leave_lines();
    assert_eq!((g.amount, g.amount_from_lines), (None, false));
}

#[test]
fn totals_status_is_exact() {
    let mut keys = RowKeyGen::new();
    let mut f = valid(&mut keys);
    assert_eq!(f.totals_status(), Some(TotalsStatus::Balanced));
    f.amount = Some(money("1286.65"));
    assert_eq!(
        f.totals_status(),
        Some(TotalsStatus::Remaining(money("0.25")))
    );
    f.amount = Some(money("1286.15"));
    assert_eq!(f.totals_status(), Some(TotalsStatus::Over(money("0.25"))));
    f.amount = None;
    assert_eq!(f.totals_status(), None);
}

#[test]
fn prefill_copies_everything_but_amounts() {
    let mut keys = RowKeyGen::new();
    let mut last = valid(&mut keys);
    last.memo = "Fall mulch".into();
    last.address = "12 Elm St".into();
    last.header_tags = vec![tag(21)];
    last.lines.lines[0].customer = Some(contact(9));
    last.lines.lines[0].billable = true;
    last.lines.lines[0].tag = Some(tag(11));
    last.lines.lines[0].memo = "mulch".into();
    let mut f = CheckForm::new(
        Some(account(1010)),
        Some(date()),
        NumberField::Proposed(4419),
        &mut keys,
    );
    f.payee = Some(contact(7));
    f.amount = Some(money("99.00"));
    assert!(f.can_prefill());
    let mut state = FormState::new(f);
    assert!(state.apply_prefill(1, &last));
    let v = state.current();
    assert_eq!(
        v.amount,
        Some(money("99.00")),
        "the check amount is untouched"
    );
    assert_eq!(v.memo, "Fall mulch");
    assert_eq!(v.address, "12 Elm St");
    assert_eq!(v.header_tags, vec![tag(21)]);
    assert_eq!(
        v.number,
        NumberField::Proposed(4419),
        "the number is not copied"
    );
    let used: Vec<&FormLine> = v.lines.used().collect();
    assert_eq!(used.len(), 2);
    assert!(v.lines.lines.iter().all(|l| l.amount.is_none()));
    assert_eq!(used[0].account, Some(account(5110)));
    assert_eq!(used[0].customer, Some(contact(9)));
    assert!(used[0].billable);
    assert_eq!(used[0].tag, Some(tag(11)));
    assert_eq!(used[0].memo, "mulch");
    assert!(
        v.lines.lines.last().is_some_and(FormLine::is_blank),
        "entry line kept"
    );
    let keys_seen: BTreeSet<RowKey> = v.lines.lines.iter().map(|l| l.key).collect();
    assert_eq!(keys_seen.len(), v.lines.lines.len(), "keys stay unique");
    assert!(!v.can_prefill());
    assert!(state.undo(), "one undo step");
    assert_eq!(state.current().lines.used().count(), 0);
}

#[test]
fn vendor_default_fills_only_an_untouched_first_line() {
    let mut keys = RowKeyGen::new();
    let mut f = CheckForm::new(None, None, NumberField::Proposed(1), &mut keys);
    f.amount = Some(money("45.00"));
    assert!(f.apply_vendor_default(account(5110), Some(tag(11)), &mut keys));
    assert_eq!(f.lines.lines[0].account, Some(account(5110)));
    assert_eq!(f.lines.lines[0].tag, Some(tag(11)));
    assert_eq!(
        f.lines.lines[0].amount,
        Some(money("45.00")),
        "the typed check amount follows onto the single line"
    );
    assert_eq!(f.lines.lines.len(), 2, "a new entry line");
    assert!(!f.apply_vendor_default(account(6350), None, &mut keys));
    assert_eq!(f.lines.lines[0].account, Some(account(5110)));
    let mut g = CheckForm::new(None, None, NumberField::Proposed(1), &mut keys);
    g.lines.lines.clear();
    assert!(g.apply_vendor_default(account(6350), None, &mut keys));
    assert_eq!(
        g.lines.lines[0].amount, None,
        "no check amount, no line amount"
    );
    assert_eq!(g.lines.lines.len(), 2);
}

#[test]
fn tag_defaults_one_per_line_in_group_order() {
    let list = tags();
    // The line keeps its own tag.
    assert_eq!(
        line_tag_default(&[tag(21)], Some(tag(12)), &list),
        Some(tag(12))
    );
    // Otherwise the first header tag in group order (Project before Crew).
    assert_eq!(
        line_tag_default(&[tag(21), tag(11)], None, &list),
        Some(tag(11))
    );
    assert_eq!(
        line_tag_default(&[tag(31), tag(21)], None, &list),
        Some(tag(21))
    );
    assert_eq!(line_tag_default(&[], None, &list), None);
    // Saved lines carry exactly that tag.
    let mut keys = RowKeyGen::new();
    let mut f = valid(&mut keys);
    f.header_tags = vec![tag(21), tag(11)];
    f.lines.lines[1].tag = Some(tag(12));
    let c = run(&f).expect("valid");
    assert_eq!(c.lines[0].tag, Some(tag(11)));
    assert_eq!(c.lines[1].tag, Some(tag(12)));
    assert_eq!(c.header_tags, vec![tag(21), tag(11)]);
}

#[test]
fn debug_hides_owner_data() {
    let mut keys = RowKeyGen::new();
    let mut f = valid(&mut keys);
    f.memo = "secret memo".into();
    f.draft_attachment = Some(AttachmentRef {
        id: "a1".into(),
        name: "receipt-greenway.pdf".into(),
    });
    let checked = run(&f).expect("valid");
    for s in [format!("{f:?}"), format!("{checked:?}")] {
        for leak in ["1286", "Greenway", "secret", "receipt", "4418"] {
            assert!(!s.contains(leak), "{leak} in {s}");
        }
    }
}

fn arb_cents() -> impl Strategy<Value = i64> {
    prop_oneof![1_i64..1_000_i64, 1_i64..100_000_000_i64]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// `check()` passes iff the typed lines add up to the check amount
    /// exactly, and reports the exact difference otherwise.
    #[test]
    fn ok_iff_lines_equal_the_amount(
        line_cents in prop::collection::vec(arb_cents(), 0..8),
        header_cents in prop::option::of(arb_cents()),
        bump in -3_i64..=3_i64,
    ) {
        let mut keys = RowKeyGen::new();
        let mut f = valid(&mut keys);
        let lines = line_cents
            .iter()
            .map(|c| FormLine {
                amount: Some(Money::from_cents(*c).expect("cents")),
                ..line(&mut keys, 5110, None)
            })
            .collect();
        f.lines = FormLines::from_lines(lines, &mut keys);
        let sum: i64 = line_cents.iter().sum();
        let header = header_cents.unwrap_or(sum).saturating_add(bump).max(1);
        f.amount = Some(Money::from_cents(header).expect("cents"));
        let result = run(&f);
        if header == sum {
            prop_assert!(result.is_ok(), "{:?}", result);
        } else {
            let p = result.expect_err("disagree");
            let want = Money::from_cents(header - sum).expect("cents");
            prop_assert_eq!(p, vec![CheckProblem::LinesDisagree { difference: want }]);
        }
        // A one-cent change on any line always refuses.
        if header == sum && !line_cents.is_empty() {
            for (i, cents) in line_cents.iter().enumerate() {
                for d in [-1_i64, 1] {
                    let mut g = f.clone();
                    let c = cents + d;
                    g.lines.lines[i].amount = Some(Money::from_cents(c).expect("cents"));
                    prop_assert!(run(&g).is_err());
                }
            }
        }
    }

    /// Prefill never sets an amount and never touches the check amount.
    #[test]
    fn prefill_copies_no_amount(
        last_cents in prop::collection::vec(prop::option::of(arb_cents()), 0..6),
        last_header in prop::option::of(arb_cents()),
        header in prop::option::of(arb_cents()),
    ) {
        let mut keys = RowKeyGen::new();
        let mut last = CheckForm::new(None, None, NumberField::Proposed(1), &mut keys);
        last.amount = last_header.map(|c| Money::from_cents(c).expect("cents"));
        let seeds = last_cents
            .iter()
            .map(|c| FormLine {
                amount: c.map(|c| Money::from_cents(c).expect("cents")),
                memo: "m".into(),
                ..FormLine::blank(RowKey(0))
            })
            .collect();
        last.lines = FormLines::from_lines(seeds, &mut keys);
        let mut f = CheckForm::new(None, None, NumberField::Proposed(2), &mut keys);
        f.amount = header.map(|c| Money::from_cents(c).expect("cents"));
        let before = f.amount;
        f.prefill_from_last(&last);
        prop_assert!(f.lines.lines.iter().all(|l| l.amount.is_none()));
        prop_assert_eq!(f.amount, before);
        prop_assert_eq!(f.lines.used().count(), last_cents.len());
    }
}

#[test]
fn a_draft_fills_empty_fields_and_blocks_save_until_accepted() {
    use crate::forms::doc_read::{Confidence, DocReading, ReadField, ReadLine, ReadTarget};
    let mut keys = RowKeyGen::new();
    let reading = DocReading {
        vendor: Some(contact(7)),
        bill_date: Some(date()),
        total: Some(money("86.40")),
        paid_from: Some(account(1010)),
        check_number: Some(" 4420 ".into()),
        lines: vec![ReadLine {
            description: "Mulch".into(),
            amount: Some(money("86.40")),
            account: Some(account(6350)),
            confidence: Some(Confidence::Low),
        }],
        ..DocReading::default()
    };
    let mut f = CheckForm::new(None, None, NumberField::Proposed(4418), &mut keys);
    let filled = apply(&reading, &mut f, &mut keys);
    assert_eq!(
        filled[..5],
        [
            ReadTarget::PaidFrom,
            ReadTarget::CheckNumber,
            ReadTarget::Date,
            ReadTarget::Payee,
            ReadTarget::Total
        ]
    );
    assert_eq!(filled.len(), 6);
    assert_eq!(f.number, NumberField::Typed("4420".into()));
    assert_eq!(f.amount, Some(money("86.40")));
    let line = f.lines.used().next().expect("a line").key;
    assert_eq!(
        f.read.state(ReadTarget::Line(line)),
        Some(ReadField::Read {
            level: Confidence::Low
        })
    );
    assert_eq!(
        run(&f).expect_err("not accepted"),
        vec![CheckProblem::ReadNotAccepted]
    );
    assert_eq!(
        CheckProblem::ReadNotAccepted.field(),
        CheckField::ReadBanner
    );
    assert_eq!(
        CheckProblem::ReadNotAccepted.key(),
        TextKey("check.problem.read_not_accepted")
    );
    f.read.accept_all();
    let c = run(&f).expect("accepted");
    assert_eq!(c.number, Some(4420));
    // A typed number, a check to print and the owner's values stay.
    let mut typed = valid(&mut keys);
    typed.number = NumberField::Typed("17".into());
    assert_eq!(apply(&reading, &mut typed, &mut keys), Vec::new());
    assert_eq!(typed.number, NumberField::Typed("17".into()));
    let mut later = CheckForm::new(None, None, NumberField::Proposed(1), &mut keys);
    later.set_print_later(true);
    apply(&reading, &mut later, &mut keys);
    assert!(later.read.state(ReadTarget::CheckNumber).is_none());
    assert!(matches!(later.number, NumberField::ToPrint { .. }));
}
