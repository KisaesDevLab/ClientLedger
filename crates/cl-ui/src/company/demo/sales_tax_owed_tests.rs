//! Tests of the demo owed/pay/exemption/recovery engine, held to the
//! non-engineOnly parts of goldens 7-10 and 12-14
//! (`fixtures/sales_tax/golden.json`).

#![allow(clippy::many_single_char_names)]

use super::*;
use crate::company::demo::sales_tax::tests::{Seed, d, day, golden, lines, run_step, same, seed};
use cl_api::{
    ApplicationRequest, ApplyInvoice, ApplyRequest, BounceRequest, NewCustomerPayment, NewInvoice,
    PayAdjustment, PayLine, PaymentPreviewRequest, SaveApplication,
};
use serde_json::Value;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn item(s: &Seed, name: &str) -> SalesTaxItemId {
    super::super::sales_tax::items(&mut s.state.clone(), true)
        .into_iter()
        .find(|i| i.name == name)
        .map(|i| i.id)
        .expect("item")
}

fn agency(s: &mut Seed, name: &str) -> SalesTaxAgencyId {
    super::super::sales_tax::agencies(&mut s.state, true)
        .into_iter()
        .find(|a| a.vendor_name == name)
        .map(|a| a.id)
        .expect("agency")
}

/// Write-offs up to 200.00 (the golden's settings).
fn allow_write_offs(s: &mut Seed) {
    super::super::receivables::seed(&mut s.state);
    s.state.receivables.settings.write_off_threshold = Some(m("200.00"));
    s.state.receivables.settings.write_off_account = Some(super::super::account_id(
        super::super::receivables::BAD_DEBTS,
    ));
    s.state.receivables.settings.discount_account = Some(super::super::account_id(
        super::super::receivables::SALES_DISCOUNTS,
    ));
}

/// Invoice `name` saved for `client` (ids kept by name).
fn invoice(
    s: &mut Seed,
    names: &mut BTreeMap<String, TransactionId>,
    req: &Value,
) -> TransactionId {
    let client = s.clients[req["client"].as_str().expect("client")];
    let new = NewInvoice::new(
        client,
        day(req["date"].as_str().expect("date")),
        lines(s, req),
    );
    let saved = super::super::invoices::create(&mut s.state, &new, None).expect("invoice");
    names.insert(
        req["name"].as_str().expect("name").to_string(),
        saved.record.txn.id,
    );
    saved.record.txn.id
}

/// A payment as Receive Payment sends it (each row's token from the
/// preview).
fn receive(
    s: &mut Seed,
    names: &mut BTreeMap<String, TransactionId>,
    req: &Value,
    hold: bool,
) -> TransactionId {
    let client = s.clients[req["client"].as_str().expect("client")];
    let clearing = super::super::receivables::clearing(&mut s.state).expect("clearing");
    let apps: Vec<(TransactionId, Money, Option<Money>, Option<Money>)> = req["applications"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| {
            (
                names[a["invoice"].as_str().expect("inv")],
                m(a["amount"].as_str().expect("amt")),
                a["discount"].as_str().map(m),
                a["writeOff"].as_str().map(m),
            )
        })
        .collect();
    let date = day(req["date"].as_str().expect("date"));
    let p = super::super::receivables::preview(
        &mut s.state,
        &PaymentPreviewRequest {
            customer_id: client,
            date,
            amount: Some(m(req["amount"].as_str().expect("amount"))),
            deposit_to: Some(clearing),
            applications: Some(
                apps.iter()
                    .map(|(i, a, disc, wo)| ApplicationRequest {
                        invoice_id: *i,
                        amount: Some(*a),
                        discount_amount: *disc,
                        write_off_amount: *wo,
                    })
                    .collect(),
            ),
            credit_applications: Vec::new(),
            hold_as_deposit: hold,
            editing_payment_id: None,
        },
    )
    .expect("preview");
    let new = NewCustomerPayment {
        customer_id: client,
        date,
        amount: p.amount,
        deposit_to: clearing,
        payment_method: Some("check".into()),
        ref_no: None,
        memo: None,
        applications: apps
            .iter()
            .map(|(i, a, disc, wo)| SaveApplication {
                invoice_id: *i,
                amount: *a,
                expected_balance_due: p.invoice(*i).expect("row").balance_due_exact.clone(),
                discount_amount: *disc,
                write_off_amount: *wo,
            })
            .collect(),
        credit_applications: Vec::new(),
        write_off_account_id: None,
        discount_account_id: None,
        hold_as_deposit: hold,
        bank_feed_item_id: None,
    };
    let rec = super::super::receivables::receive(&mut s.state, &new, None, None).expect("paid");
    names.insert(req["name"].as_str().expect("name").to_string(), rec.id);
    rec.id
}

fn apply_to(s: &mut Seed, names: &BTreeMap<String, TransactionId>, source: &str, req: &Value) {
    let src = names[source];
    let rec = super::super::receivables::get(&mut s.state, src).expect("rec");
    super::super::receivables::apply(
        &mut s.state,
        src,
        &ApplyRequest {
            date: day(req["date"].as_str().expect("date")),
            applications: vec![ApplyInvoice {
                invoice_id: names[req["invoice"].as_str().expect("inv")],
                amount: Some(m(req["amount"].as_str().expect("amt"))),
                expected_balance_due: None,
            }],
            expected_updated_at: Some(rec.updated_at),
        },
        None,
    )
    .expect("applied");
}

fn basis(req: &Value) -> SalesTaxBasis {
    if req["basis"] == "cash" {
        SalesTaxBasis::Cash
    } else {
        SalesTaxBasis::Accrual
    }
}

fn check_liability(s: &mut Seed, req: &Value, want: &Value, what: &str) {
    let r = liability(
        &mut s.state,
        cl_api::LiabilityQuery {
            start_date: day(req["startDate"].as_str().expect("start")),
            end_date: day(req["endDate"].as_str().expect("end")),
            basis: Some(basis(req)),
            agency_id: None,
        },
    )
    .expect("liability");
    for w in want["items"].as_array().into_iter().flatten() {
        let name = w["item"].as_str().expect("item");
        let it = r
            .agencies
            .iter()
            .flat_map(|a| a.items.iter())
            .find(|i| i.name == name)
            .unwrap_or_else(|| panic!("{what}: no row for {name}"));
        let what = format!("{what} {name}");
        same(it.taxable_sales.as_decimal(), &w["taxableSales"], &what);
        same(it.tax.collected.as_decimal(), &w["collected"], &what);
        same(it.tax.discounts.as_decimal(), &w["discounts"], &what);
        same(it.tax.bad_debts.as_decimal(), &w["badDebts"], &what);
        same(it.tax.recoveries.as_decimal(), &w["recoveries"], &what);
        same(it.tax.adjustments.as_decimal(), &w["adjustments"], &what);
        same(it.tax.paid.as_decimal(), &w["paid"], &what);
        same(it.tax.owed_at_end.as_decimal(), &w["owedAtEnd"], &what);
        for (code, amount) in w["nonTaxable"].as_object().into_iter().flatten() {
            let got = it
                .non_taxable
                .iter()
                .find(|n| &n.code == code)
                .map_or(Decimal::ZERO, |n| n.amount.as_decimal());
            same(got, amount, &what);
        }
    }
    let collected: Decimal = r
        .agencies
        .iter()
        .map(|a| a.totals.collected.as_decimal())
        .sum();
    same(collected + r.unassigned.as_decimal(), &want["total"], what);
    if let Some(rd) = want["roundingDifference"].as_str() {
        let got = r.rounding_difference.expect("rounding").as_decimal();
        assert_eq!(got, d(rd), "{what} rounding");
    }
    if r.basis == SalesTaxBasis::Cash {
        assert!(r.hides_bad_debts());
    }
}

fn case(g: &Value, id: i64) -> &Value {
    g["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|c| c["id"] == id)
        .expect("case")
}

fn pay_request(s: &mut Seed, req: &Value) -> PaySalesTaxRequest {
    let a = agency(s, req["agency"].as_str().expect("agency"));
    let bank = super::super::receivables::clearing(&mut s.state).expect("bank");
    super::super::receivables::seed(&mut s.state);
    let income = super::super::account_id(super::super::receivables::SALES_DISCOUNTS);
    PaySalesTaxRequest {
        agency_id: a,
        through_date: day(req["throughDate"].as_str().expect("through")),
        date: day(req["date"].as_str().expect("date")),
        bank_account_id: bank,
        method: if req["method"] == "electronic" {
            SalesTaxPayMethod::Electronic
        } else {
            SalesTaxPayMethod::Check
        },
        lines: req["lines"].as_array().map(|ls| {
            ls.iter()
                .map(|l| PayLine {
                    tax_item_id: item(s, l["item"].as_str().expect("item")),
                    amount: m(l["amount"].as_str().expect("amount")),
                    expected_owed: None,
                })
                .collect()
        }),
        adjustments: req["adjustments"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| PayAdjustment {
                tax_item_id: item(s, x["item"].as_str().expect("item")),
                account_id: income,
                amount: m(x["amount"].as_str().expect("amount")),
                reason: x["reason"].as_str().expect("reason").into(),
            })
            .collect(),
        print_later: req["printLater"].as_bool().unwrap_or(false),
        check_number: None,
        memo: None,
        expected_owed: None,
    }
}

fn owed_of(s: &mut Seed, agency_name: &str) -> SalesTaxOwed {
    let a = agency(s, agency_name);
    owed(&mut s.state, day("2026-11-30"), None, Some(a)).expect("owed")
}

#[test]
fn the_demo_engine_runs_the_golden_chain_through_case_10() {
    let g = golden();
    let mut s = seed(&g);
    for id in 1..=6 {
        for step in case(&g, id)["steps"].as_array().expect("steps") {
            run_step(&mut s, id, step);
        }
    }
    let mut names: BTreeMap<String, TransactionId> = BTreeMap::new();
    for (n, inv) in &s.state.invoices.stored {
        if let Some(num) = inv.record["txnNumber"].as_str() {
            names.insert(num.to_string(), *n);
        }
    }
    // INV-E and INV-G as the chain made them (cases 1 and 6).
    let by_total = |s: &Seed, t: &str| {
        s.state
            .invoices
            .stored
            .iter()
            .find(|(_, x)| x.record["total"].as_str().map(d) == Some(d(t)))
            .map(|(id, _)| *id)
            .expect("invoice")
    };
    names.insert("INV-E".into(), by_total(&s, "1088.75"));
    names.insert("INV-G".into(), by_total(&s, "200.00"));
    // Case 8 (its non-engineOnly steps), then 7.
    for id in [8, 7] {
        for step in case(&g, id)["steps"].as_array().expect("steps") {
            if step["engineOnly"] == true {
                continue;
            }
            let req = &step["request"];
            let what = format!("case {id} {}", step["op"]);
            match step["op"].as_str().expect("op") {
                "receivePayment" => {
                    receive(&mut s, &mut names, req, false);
                }
                "receiveDeposit" => {
                    receive(&mut s, &mut names, req, true);
                }
                "applyCredit" => apply_to(&mut s, &names, req["payment"].as_str().expect("p"), req),
                "drawDeposit" => apply_to(&mut s, &names, req["deposit"].as_str().expect("d"), req),
                "invoice" => {
                    invoice(&mut s, &mut names, req);
                }
                "liability" => check_liability(&mut s, req, &step["expect"], &what),
                // The refund of PMT-R moves Unapplied receipts only (no tax).
                "refund" => {}
                other => panic!("unexpected {other}"),
            }
        }
    }
    // Case 9: Pay Sales Tax.
    let state_agency = "Sample State Revenue Dept.";
    let city_agency = "Sample City Revenue Office";
    let mut paid_id = None;
    for step in case(&g, 9)["steps"].as_array().expect("steps") {
        let req = &step["request"];
        let want = &step["expect"];
        let what = format!("case 9 {}", step["op"]);
        match step["op"].as_str().expect("op") {
            "payPreview" => {
                let r = pay_request(&mut s, req);
                let p = preview(&mut s.state, &r).expect("preview");
                same(p.total.as_decimal(), &want["total"], &what);
                for w in want["lines"].as_array().into_iter().flatten() {
                    let id = item(&s, w["item"].as_str().expect("item"));
                    let l = p
                        .lines
                        .iter()
                        .find(|l| l.tax_item_id == Some(id))
                        .expect("line");
                    same(l.owed.as_decimal(), &w["owed"], &what);
                    same(l.adjustment.as_decimal(), &w["adjustment"], &what);
                    same(l.amount.as_decimal(), &w["amount"], &what);
                }
            }
            "pay" if req["replay"] == true => {
                let r = pay_request(
                    &mut s,
                    &json!({"agency": state_agency, "throughDate": "2026-11-30",
                        "date": "2026-12-10", "method": "check"}),
                );
                let again = create(&mut s.state, &r, None, Some("pay-1")).expect("replay");
                assert_eq!(Some(again.id), paid_id, "{what}");
            }
            "pay" => {
                let mut r = pay_request(&mut s, req);
                let p = preview(&mut s.state, &r).expect("preview");
                // The screen posts the owner's amounts (or the preview's)
                // with each line's expectedOwed and the owed total copied.
                r.expected_owed = Some(p.owed);
                r.lines = Some(
                    p.lines
                        .iter()
                        .filter_map(|l| {
                            let typed = r.lines.as_ref().and_then(|ls| {
                                ls.iter().find(|x| Some(x.tax_item_id) == l.tax_item_id)
                            });
                            Some(PayLine {
                                tax_item_id: l.tax_item_id?,
                                amount: typed.map_or(l.amount, |t| t.amount),
                                expected_owed: Some(l.expected_owed),
                            })
                        })
                        .collect(),
                );
                if req["name"] == "PAY-STALE" {
                    for l in r.lines.iter_mut().flatten() {
                        l.expected_owed = Some(m("70.00"));
                    }
                }
                let key = req["key"].as_str();
                match create(&mut s.state, &r, None, key) {
                    Ok(done) => {
                        assert!(want["error"].is_null(), "{what}: expected an error");
                        if req["name"] == "PAY-1" {
                            paid_id = Some(done.id);
                            assert_eq!(done.print_status.as_deref(), Some("queue"));
                            let rec = get(&s.state, done.id).expect("record");
                            same(rec.total.as_decimal(), &json!("210.42"), &what);
                        }
                    }
                    Err(e) => {
                        assert!(!want["error"].is_null(), "{what}: {e:?}");
                        assert_eq!(
                            cl_api::pay_sales_tax_refusal(&e),
                            Some(cl_api::PaySalesTaxRefusal::OwedChanged),
                            "{what}"
                        );
                    }
                }
            }
            "owed" => {
                let o = owed_of(&mut s, req["agency"].as_str().expect("agency"));
                same(o.total.as_decimal(), &want["total"], &what);
                for w in want["items"].as_array().into_iter().flatten() {
                    let id = item(&s, w["item"].as_str().expect("item"));
                    let got = o
                        .agencies
                        .iter()
                        .flat_map(|a| a.items.iter())
                        .find(|i| i.tax_item_id == Some(id))
                        .expect("item");
                    same(got.owed.as_decimal(), &w["owed"], &what);
                }
            }
            "check" => {
                let stp = stp(&mut s.state).expect("stp");
                let bank = super::super::receivables::clearing(&mut s.state).expect("bank");
                let chart = super::super::receivables::chart(&mut s.state);
                let id = s
                    .state
                    .ledger
                    .save_lines(
                        &chart,
                        &DemoJournal {
                            kind: "check",
                            date: day(req["date"].as_str().expect("date")),
                            number: String::new(),
                            contact: None,
                            memo: String::new(),
                            lines: vec![
                                DemoLine {
                                    account: stp,
                                    debit: m("24.02"),
                                    credit: Money::ZERO,
                                    memo: None,
                                },
                                DemoLine {
                                    account: bank,
                                    debit: Money::ZERO,
                                    credit: m("24.02"),
                                    memo: None,
                                },
                            ],
                        },
                        None,
                    )
                    .expect("check");
                names.insert("CHK-CITY".into(), id);
            }
            "link" => {
                let a = agency(&mut s, city_agency);
                let city = item(&s, "City");
                let r = LinkPaymentRequest {
                    transaction_id: names["CHK-CITY"],
                    agency_id: a,
                    lines: vec![cl_api::LinkLine {
                        tax_item_id: city,
                        amount: m("24.02"),
                    }],
                };
                // Not in the golden file: link, unlink (owed again), link.
                link(&mut s.state, &r, None, Some("link-0")).expect("linked");
                same(
                    owed_of(&mut s, city_agency).total.as_decimal(),
                    &json!("0.00"),
                    &what,
                );
                let u = unlink(&mut s.state, names["CHK-CITY"], None, Some("unlink-0"))
                    .expect("unlinked");
                assert_eq!(u.unlinked, 1);
                same(
                    owed_of(&mut s, city_agency).total.as_decimal(),
                    &json!("24.02"),
                    &what,
                );
                link(&mut s.state, &r, None, req["key"].as_str()).expect("linked");
                let rec = get(&s.state, names["CHK-CITY"]).expect("record");
                assert!(rec.linked && rec.stub_lines.len() == 1, "{what}");
            }
            "liability" => {
                let r = liability(
                    &mut s.state,
                    cl_api::LiabilityQuery {
                        start_date: day(req["startDate"].as_str().expect("s")),
                        end_date: day(req["endDate"].as_str().expect("e")),
                        basis: Some(basis(req)),
                        agency_id: None,
                    },
                )
                .expect("liability");
                same(
                    r.other_entries.expect("company-wide").as_decimal(),
                    &want["otherEntries"],
                    &what,
                );
            }
            "void" => {
                let id = paid_id.expect("paid");
                void(&mut s.state, id, None).expect("void");
                let bank = super::super::receivables::clearing(&mut s.state).expect("bank");
                assert_eq!(print_queue(&mut s.state, bank, &|_| false).len(), 0);
            }
            "checkQueue" => {}
            other => panic!("unexpected {other}"),
        }
    }
    // Case 10: exemption certificates.
    let mut warned = 0;
    for step in case(&g, 10)["steps"].as_array().expect("steps") {
        let req = &step["request"];
        let want = &step["expect"];
        match step["op"].as_str().expect("op") {
            "exemption" => {
                let client = s.clients[req["client"].as_str().expect("c")];
                save_exemption(
                    &mut s.state,
                    None,
                    &TaxExemptionDraft {
                        contact_id: client,
                        number: req["number"].as_str().expect("n").into(),
                        state: req["state"].as_str().map(str::to_string),
                        expires_on: req["expiresOn"].as_str().map(day),
                        attachment_id: None,
                        note: None,
                    },
                    None,
                )
                .expect("saved");
            }
            "invoicePreview" => {
                let client = s.clients[req["client"].as_str().expect("c")];
                let got = exemption_problem(&mut s.state, client, day("2026-11-20"));
                let wanted = want["warnings"]
                    .as_array()
                    .expect("w")
                    .first()
                    .and_then(Value::as_str);
                assert_eq!(got, wanted, "case 10 {}", req["client"]);
                // The invoice preview carries it.
                let new = NewInvoice::new(client, day("2026-11-20"), lines(&s, req));
                let p = super::super::invoices::preview(&mut s.state, &new, None).expect("p");
                assert_eq!(
                    p.warnings.first().map(|x| x.code.as_str()),
                    wanted,
                    "in warnings"
                );
                warned += 1;
            }
            "exemptionsExpiring" => {
                let names: Vec<String> = expiring(&mut s.state, day("2026-11-04"), 60)
                    .into_iter()
                    .map(|e| e.contact_name)
                    .collect();
                let want: Vec<&str> = want["clients"]
                    .as_array()
                    .expect("c")
                    .iter()
                    .filter_map(Value::as_str)
                    .collect();
                assert_eq!(names, want);
            }
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!(warned, 4);
}

/// Runs a SSUTA case (12-14) from a fresh seed.
fn run_ssuta(id: i64, ssuta: bool) -> Seed {
    let g = golden();
    let mut s = seed(&g);
    allow_write_offs(&mut s);
    s.state.sales_tax.ssuta = ssuta;
    let mut names: BTreeMap<String, TransactionId> = BTreeMap::new();
    let c = case(&g, id);
    let steps = if ssuta {
        c["steps"].clone()
    } else {
        c["variants"][0]["steps"].clone()
    };
    for step in steps.as_array().expect("steps") {
        if step["engineOnly"] == true {
            continue;
        }
        let req = &step["request"];
        let want = &step["expect"];
        let what = format!("case {id} {}", step["op"]);
        match step["op"].as_str().expect("op") {
            "invoice" => {
                invoice(&mut s, &mut names, req);
            }
            "receivePayment" => {
                // The preview's discountTax / writeOffTax (U-153).
                for a in req["applications"].as_array().into_iter().flatten() {
                    let inv = a["invoice"].as_str().expect("inv");
                    let z = |k: &str| a[k].as_str().map_or(Decimal::ZERO, d);
                    let parts = preview_tax_parts(
                        &s.state,
                        names[inv],
                        z("amount"),
                        z("discount"),
                        z("writeOff"),
                    );
                    if !ssuta {
                        assert_eq!(parts, None, "{what}: switch off");
                    } else if let Some(want) = want["discountTax"][inv].as_str() {
                        assert_eq!(parts.map(|p| p.0), Some(d(want)), "{what}");
                    }
                }
                receive(&mut s, &mut names, req, false);
            }
            "liability" => check_liability(&mut s, req, want, &what),
            "recovery" => {
                let r = recover(
                    &mut s.state,
                    &NewRecovery {
                        invoice_id: names[req["invoice"].as_str().expect("inv")],
                        date: day(req["date"].as_str().expect("date")),
                        amount: m(req["amount"].as_str().expect("amount")),
                        memo: None,
                    },
                    None,
                    req["key"].as_str(),
                )
                .expect("recovered");
                same(
                    r.recovered_tax.expect("tax").as_decimal(),
                    &want["recoveredTax"],
                    &what,
                );
                same(
                    r.invoice_balance_due.as_decimal(),
                    &want["invoiceBalanceDue"],
                    &what,
                );
            }
            "bounce" => {
                let p = names[req["payment"].as_str().expect("p")];
                let rec = super::super::receivables::get(&mut s.state, p).expect("rec");
                let out = super::super::receivables::bounce(
                    &mut s.state,
                    p,
                    &BounceRequest {
                        date: day(req["date"].as_str().expect("date")),
                        bank_fee_amount: None,
                        bank_fee_account_id: None,
                        bank_fee_bank_account_id: None,
                        memo: None,
                        expected_updated_at: rec.updated_at,
                    },
                    None,
                    None,
                );
                if want["error"].is_null() {
                    out.expect("bounced");
                } else {
                    let e = out.expect_err("refused");
                    let code = match &e {
                        ApiError::Conflict { code, .. } => {
                            code.as_ref().map(|c| c.as_str().to_string())
                        }
                        _ => None,
                    };
                    assert_eq!(code.as_deref(), want["error"]["code"].as_str(), "{what}");
                }
            }
            other => panic!("unexpected {other}"),
        }
    }
    s
}

#[test]
fn ssuta_discount_case_12_on_and_off() {
    run_ssuta(12, true);
    run_ssuta(12, false);
}

#[test]
fn ssuta_bad_debt_and_recovery_case_13() {
    let mut s = run_ssuta(13, true);
    // A second recovery past the write-off is refused with what is left.
    let inv = recoverable(&s.state, None);
    assert_eq!(inv.len(), 1);
    assert_eq!(inv[0].3, m("67.75"));
    let e = recover(
        &mut s.state,
        &NewRecovery {
            invoice_id: inv[0].0,
            date: day("2026-12-28"),
            amount: m("70.00"),
            memo: None,
        },
        None,
        None,
    )
    .expect_err("too much");
    assert_eq!(cl_api::recovery_available(&e), Some(m("67.75")));
}

#[test]
fn ssuta_bounce_case_14() {
    run_ssuta(14, true);
}

#[test]
fn cash_running_total_reaches_the_document_tax_when_fully_paid() {
    // Σ cash shares over a full settlement = T (the property of §6.3, on a
    // few uneven splits).
    for parts in [
        vec!["0.01", "1088.74"],
        vec!["362.92", "362.92", "362.91"],
        vec!["100.00", "200.00", "300.00", "488.75"],
    ] {
        let total = d("1088.75");
        let mut p = Decimal::ZERO;
        let mut sum = Decimal::ZERO;
        for x in parts {
            let x = d(x);
            sum += share(d("88.75"), p + x, p, total);
            p += x;
        }
        assert_eq!(sum, d("88.75"));
    }
}

#[test]
fn ssuta_split_matches_the_golden_shares() {
    let s = |c: &str, disc: &str, w: &str| Settlement {
        date: day("2026-11-08"),
        cash: d(c),
        discount: d(disc),
        write_off: d(w),
    };
    assert_eq!(
        ssuta_split(
            d("35.50"),
            s("426.79", "8.71", "0"),
            Decimal::ZERO,
            d("435.50")
        ),
        (d("0.71"), Decimal::ZERO)
    );
    assert_eq!(
        ssuta_split(
            d("17.75"),
            s("100.00", "0", "117.75"),
            Decimal::ZERO,
            d("217.75")
        ),
        (Decimal::ZERO, d("9.60"))
    );
    // A bounce's mirror is the same split, negated.
    assert_eq!(
        ssuta_split(
            d("35.50"),
            s("-426.79", "-8.71", "0"),
            d("435.50"),
            d("435.50")
        ),
        (d("-0.71"), Decimal::ZERO)
    );
}
