//! Import Bank File against the demo feed (P25): the real parsers, the
//! demo's copy of the engine keys. The scenario `import_bank_file.yaml`
//! drives the drawn window.

use std::sync::Arc;

use super::*;
use crate::company::CompanyLink;

const OFX: &str = "OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\nSECURITY:NONE\nENCODING:USASCII\n\
CHARSET:1252\nCOMPRESSION:NONE\nOLDFILEUID:NONE\nNEWFILEUID:NONE\n\n<OFX>\n<SIGNONMSGSRSV1><SONRS>\
<STATUS><CODE>0<SEVERITY>INFO</STATUS><DTSERVER>20261103<LANGUAGE>ENG</SONRS></SIGNONMSGSRSV1>\n\
<BANKMSGSRSV1><STMTTRNRS><TRNUID>1<STATUS><CODE>0<SEVERITY>INFO</STATUS>\n<STMTRS><CURDEF>USD\n\
<BANKACCTFROM><BANKID>123456789<ACCTID>000012342208<ACCTTYPE>CHECKING</BANKACCTFROM>\n\
<BANKTRANLIST><DTSTART>20261028<DTEND>20261103\n\
<STMTTRN><TRNTYPE>DEBIT<DTPOSTED>20261031<TRNAMT>-186.40<FITID>A1<NAME>MONETT TIRE AND LUBE</STMTTRN>\n\
<STMTTRN><TRNTYPE>CREDIT<DTPOSTED>20261101<TRNAMT>8450.00<FITID>A2<NAME>ACH CREDIT HOA</STMTTRN>\n\
<STMTTRN><TRNTYPE>CHECK<DTPOSTED>20261102<TRNAMT>-925.00<FITID>A3<CHECKNUM>4412<NAME>CHECK</STMTTRN>\n\
</BANKTRANLIST><LEDGERBAL><BALAMT>92431.07<DTASOF>20261103</LEDGERBAL></STMTRS></STMTTRNRS>\
</BANKMSGSRSV1></OFX>\n";

const CSV: &str = "Date,Description,Amount\n11/01/2026,COFFEE ROASTERS,-4.50\n11/02/2026,DEPOSIT,120.00\n11/02/2026,ZERO,0.00\n";

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

struct Fixture {
    _dir: tempfile::TempDir,
    files: tempfile::TempDir,
    link: Arc<CompanyLink>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    Fixture {
        _dir: dir,
        files: tempfile::tempdir().expect("files"),
        link,
    }
}

fn screen(f: &Fixture) -> ImportBankFileScreen {
    let mut s = ImportBankFileScreen::new(Arc::clone(&f.link), None, today());
    s.poll();
    s
}

/// Reads `text` as the file `name`, waiting for the worker.
fn read(s: &mut ImportBankFileScreen, f: &Fixture, name: &str, text: &str) {
    let path = f.files.path().join(name);
    std::fs::write(&path, text).expect("write");
    s.read(path, None);
    for _ in 0..500 {
        s.poll();
        if s.reading.is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the file was never read");
}

/// Waits for the demo's preparation of the import just sent (Q-P25v).
fn prepared(s: &mut ImportBankFileScreen) {
    for _ in 0..300 {
        s.poll();
        if !s.preparing() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("never prepared");
}

fn card_account(s: &ImportBankFileScreen) -> AccountId {
    s.targets
        .iter()
        .find(|t| t.card)
        .expect("a card account")
        .id
}

fn bank_account(s: &ImportBankFileScreen) -> AccountId {
    s.targets
        .iter()
        .find(|t| !t.card)
        .expect("a bank account")
        .id
}

/// Steps 1 to 3 for the OFX file into the first bank account.
fn previewed(s: &mut ImportBankFileScreen, f: &Fixture) {
    read(s, f, "operating.qfx", OFX);
    assert_eq!(s.state()["step"], "choose");
    assert!(s.can_next(), "{:?}", s.read_error);
    s.next();
    assert_eq!(s.step, Step::Link);
    s.pick_account(bank_account(s));
    if s.sign_needs_confirm() {
        s.set_flipped(s.flipped);
    }
    s.next();
    assert_eq!(s.step, Step::Check);
    s.poll();
}

#[test]
fn the_accounts_are_the_bank_and_card_accounts() {
    let f = fixture();
    let s = screen(&f);
    assert!(!s.targets.is_empty());
    assert!(s.targets.iter().any(|t| !t.card));
    assert_eq!(s.state()["screen"], "import_bank_file");
    assert!(!s.can_next(), "no file yet");
}

#[test]
fn a_file_previews_sends_and_a_second_import_adds_nothing() {
    let f = fixture();
    let mut s = screen(&f);
    previewed(&mut s, &f);
    let p = s.preview.as_ref().expect("preview");
    assert_eq!(
        (p.counts.in_file, p.counts.new, p.counts.duplicate),
        (3, 3, 0)
    );
    assert_eq!(flow::rows_to_send(p, &s.choices), 3);
    assert!(p.balance_check.is_none(), "not available in demo");
    s.next();
    assert_eq!(s.step, Step::Done);
    assert_eq!(s.sent, 1);
    let r = s.result.as_ref().expect("result");
    assert_eq!(r.counts.inserted, 3);
    assert!(r.batch.is_some());
    assert_eq!(f.link.demo_state().expect("demo").bank_feed.waiting(), 3);

    // The same file again: nothing new.
    s.start_over(false);
    assert_eq!(s.step, Step::Choose);
    previewed(&mut s, &f);
    let p = s.preview.as_ref().expect("preview");
    assert_eq!((p.counts.new, p.counts.duplicate), (0, 3));
    assert!(p.same_file_imported_at.is_some());
    assert_eq!(flow::rows_to_send(p, &s.choices), 0);
    assert!(
        s.remembered_flip.is_some(),
        "the account's last import sets the sign"
    );
    // UI review 8: nothing to add or replace closes Send.
    assert!(!s.can_next(), "nothing to send");
    s.next();
    assert_eq!(s.step, Step::Check);
    assert!(s.result.is_none());
    assert_eq!(f.link.demo_state().expect("demo").bank_feed.waiting(), 3);
}

/// A row the bank changed after it was imported: Replace waiting item
/// opens Send with nothing added (an update); Add as new adds it; the
/// rail's badge is told the feed's new count.
#[test]
fn a_changed_row_replaces_or_is_added() {
    let f = fixture();
    let mut s = screen(&f);
    previewed(&mut s, &f);
    s.next();
    prepared(&mut s);
    let _ = f.link.drain();
    s.start_over(false);
    let changed = OFX.replace(
        "<DTPOSTED>20261031<TRNAMT>-186.40<FITID>A1",
        "<DTPOSTED>20261101<TRNAMT>-196.40<FITID>A1",
    );
    read(&mut s, &f, "operating2.qfx", &changed);
    s.next();
    s.pick_account(bank_account(&s));
    s.next();
    s.poll();
    let p = s.preview.clone().expect("preview");
    assert_eq!((p.counts.changed, p.counts.duplicate), (1, 2));
    let line = p
        .rows
        .iter()
        .find(|r| r.status == ImportRowStatus::Changed)
        .expect("changed")
        .line;
    // R2-3: what it was (the waiting item, feed sign) and the new date.
    let row = p.rows.iter().find(|r| r.line == line).expect("row");
    assert_eq!(
        row.existing_amount,
        Some(cl_core::Money::parse_wire("186.40").expect("m"))
    );
    assert_eq!(row.existing_date, CalendarDate::from_ymd(2026, 10, 31).ok());
    // Q-P25af: no default; Send waits for an answer.
    assert!(!s.can_next(), "nothing chosen yet");
    assert_eq!(s.state()["changed_unanswered"], 1);
    s.choose(line, changed_skip());
    s.poll();
    assert_eq!(s.unanswered_changed(), 0);
    assert_eq!(s.replacing(), 0);
    assert!(!s.can_next(), "skipped and nothing new: nothing to send");
    s.choose(line, changed_choice(ChangedAction::Replace));
    s.poll();
    assert_eq!(s.replacing(), 1);
    assert!(s.can_next(), "an update opens Send");
    // Replace only: nothing added, the waiting item updated; undo puts
    // it back.
    s.next();
    let sent = s.result.clone().expect("result");
    assert_eq!(sent.counts.inserted, 0);
    assert!(sent.rows.iter().any(|x| x.replaced));
    prepared(&mut s);
    s.undo(false);
    assert_eq!(s.undone.map(|u| u.restored), Some(1));
    assert!(crate::i18n::has("bankimport.done.undone_restored.one"));
    s.start_over(false);
    read(&mut s, &f, "operating2.qfx", &changed);
    s.next();
    s.next();
    s.poll();
    assert_eq!(s.preview.as_ref().expect("preview").counts.changed, 1);
    s.choose(line, changed_choice(ChangedAction::Add));
    s.poll();
    assert_eq!(s.replacing(), 0);
    assert_eq!(
        flow::rows_to_send(s.preview.as_ref().expect("preview"), &s.choices),
        1
    );
    let _ = f.link.drain();
    s.next();
    assert_eq!(s.result.as_ref().expect("result").counts.inserted, 1);
    assert_eq!(f.link.demo_state().expect("demo").bank_feed.waiting(), 4);
    s.poll();
    let counts: Vec<u32> = f
        .link
        .drain()
        .into_iter()
        .filter_map(|e| match e {
            crate::company::CompanyEvent::BankFeedCount(n) => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(counts, vec![37 + 4], "the sample's 37 and these 4");
}

#[test]
fn undo_takes_the_rows_out_and_they_come_back() {
    let f = fixture();
    let mut s = screen(&f);
    previewed(&mut s, &f);
    s.next();
    // Q-P25v: preparing first; once prepared, Undo takes the rows out.
    assert_eq!(s.state()["preparing"], true);
    prepared(&mut s);
    s.undo(false);
    assert_eq!(s.undone.map(|u| u.removed), Some(3));
    assert_eq!(s.state()["result"]["post_processing"], "done");
    assert_eq!(f.link.demo_state().expect("demo").bank_feed.waiting(), 0);
    s.undo(false);
    assert_eq!(s.state()["undo_trouble"], "bankimport.undo.already");
    s.start_over(false);
    previewed(&mut s, &f);
    assert_eq!(s.preview.as_ref().expect("preview").counts.new, 3);
}

/// PR #234 (Q-P25ad): while the suggestions are prepared the wizard
/// counts them and Undo stays open; an undo then stops them and says so.
#[test]
fn undo_while_preparing_stops_the_suggestions() {
    let f = fixture();
    let mut s = screen(&f);
    assert!(s.command(CommandId::new("dev.demo_bank_import_slow")));
    previewed(&mut s, &f);
    s.next();
    assert_eq!(s.state()["progress_caps"], true);
    assert_eq!(s.state()["preparing"], true);
    // The first check reads the count.
    s.prep_next = Some(std::time::Instant::now());
    s.poll();
    assert_eq!(s.state()["prep_progress"][1], 3);
    s.undo(false);
    assert_eq!(s.undone.map(|u| u.removed), Some(3));
    assert_eq!(s.state()["result"]["post_processing"], "cancelled");
    assert_eq!(s.state()["preparing"], false);
    assert_eq!(s.state()["undo_trouble"], Value::Null);
    assert_eq!(f.link.demo_state().expect("demo").bank_feed.waiting(), 0);
    // Later checks never bring "preparing" back.
    s.prep_next = Some(std::time::Instant::now());
    s.poll();
    assert_eq!(s.state()["result"]["post_processing"], "cancelled");
    assert!(
        crate::i18n::t("bankimport.done.prep_cancelled")
            .contains("Import undone; suggestions stopped")
    );
}

/// P25 follow-up 6: a wizard opened again sees an earlier import still
/// being prepared, with its count, until it is undone or done.
#[test]
fn a_reopened_wizard_rechecks_an_earlier_import() {
    let f = fixture();
    let mut s = screen(&f);
    assert_eq!(s.state()["earlier"], Value::Null);
    assert!(s.command(CommandId::new("dev.demo_bank_import_slow")));
    previewed(&mut s, &f);
    s.next();
    let batch = s.result.as_ref().and_then(|r| r.batch).expect("batch");
    drop(s);
    let mut again = screen(&f);
    assert_eq!(again.state()["earlier"]["post_processing"], "scheduled");
    assert_eq!(again.state()["earlier"]["progress"][1], 3);
    assert!(again.earlier_next.is_some(), "checked again");
    // Undone elsewhere: the note goes at the next check.
    f.link
        .demo_state()
        .expect("demo")
        .bank_feed
        .undo(batch)
        .expect("undo");
    again.earlier_next = Some(std::time::Instant::now());
    again.poll();
    assert_eq!(again.state()["earlier"], Value::Null);
    assert!(again.earlier_next.is_none());
    // Nothing preparing: no note.
    let fresh = screen(&f);
    assert_eq!(fresh.state()["earlier"], Value::Null);
}

#[test]
fn a_frozen_body_keeps_its_key_until_it_changes() {
    let f = fixture();
    let mut s = screen(&f);
    previewed(&mut s, &f);
    let req = s.request().expect("request");
    let first = s.key_for(&req);
    assert_eq!(
        s.key_for(&req),
        first,
        "a retry of the same body reuses the key"
    );
    s.set_flipped(!s.flipped);
    let changed = s.request().expect("request");
    assert_ne!(changed, req);
    assert_ne!(s.key_for(&changed), first, "a changed body gets a new key");
}

#[test]
fn rows_are_sent_in_the_feed_sign_with_the_holders_balance() {
    let f = fixture();
    let mut s = screen(&f);
    previewed(&mut s, &f);
    let req = s.request().expect("request");
    assert_eq!(req.rows.len(), 3);
    // Money out is positive on the wire (feed sign).
    assert_eq!(
        cl_api::wire_decimal(req.rows[0].amount).as_deref(),
        Ok("186.40")
    );
    assert_eq!(
        cl_api::wire_decimal(req.rows[1].amount).as_deref(),
        Ok("-8450.00")
    );
    assert_eq!(req.rows[2].check_number, Some(4412));
    assert_eq!(
        req.file_ending_balance
            .and_then(|b| cl_api::wire_decimal(b).ok())
            .as_deref(),
        Some("92431.07")
    );
    assert_eq!(req.format, cl_api::BankFileFormat::Ofx);
    assert_eq!(req.file_name, "operating.qfx");
}

/// QA-3: the card fixture in the issuer's signs (flipped) and in the
/// holder's signs (as is) sends the same rows and the same holder-signed
/// balance, -429.12; the balance check then reads in one sign, the amount
/// owed: Bank says 429.12, Books will say X, Difference = 429.12 - X.
#[test]
fn a_card_file_sends_one_balance_in_either_sign_convention() {
    let mut sent = Vec::new();
    for (file, flip) in [
        ("card_positive_issuer.qfx", true),
        ("card_ccstmt.qfx", false),
    ] {
        let f = fixture();
        let mut s = screen(&f);
        let card = card_account(&s);
        into_account(&mut s, bank_fixture(file), card);
        assert!(s.sign_needs_confirm(), "{file}: a card asks once");
        assert_eq!(s.flipped, flip, "{file}: the suggestion");
        s.set_flipped(s.flipped);
        let req = s.request().expect("request");
        let amounts: Vec<String> = req
            .rows
            .iter()
            .map(|r| cl_api::wire_decimal(r.amount).expect("amount"))
            .collect();
        let balance = req
            .file_ending_balance
            .and_then(|b| cl_api::wire_decimal(b).ok());
        assert_eq!(balance.as_deref(), Some("-429.12"), "{file}");
        // Feed sign: a charge (money out) is positive.
        assert_eq!(amounts[0], "45.67", "{file}");
        assert_eq!(amounts[3], "-500.00", "{file}");
        sent.push(amounts);
    }
    assert_eq!(sent[0], sent[1], "the same rows either way");

    // The engine's answer for this file (U-06, amounts owed): a fresh card
    // account owes 0.00; the rows net -70.78 owed (525.00 of credits,
    // 454.22 of charges), so the books will say -70.78 and the difference
    // is 429.12 - (-70.78) = 499.90. QA's run: the card owed 98.00 on
    // 10/31, books 27.22, difference 429.12 - 27.22 = 401.90.
    let m = |s: &str| cl_core::Money::parse_wire(s).expect("money");
    for (projected, difference) in [("-70.78", "499.90"), ("27.22", "401.90")] {
        let check = cl_api::ImportBalanceCheck {
            file_balance: m("-429.12"),
            file_date: CalendarDate::from_ymd(2026, 10, 31).expect("date"),
            books_at_date: m("0.00"),
            pending_total: m("0.00"),
            new_total: m("0.00"),
            likely_in_books_count: 0,
            likely_in_books_total: m("0.00"),
            projected: m(projected),
            difference: m(difference),
        };
        let fig = flow::balance_figures(&check, true);
        assert_eq!(fig.bank, m("429.12"), "owed, as the books say it");
        assert_eq!(fig.books, m(projected));
        assert_eq!(
            fig.bank.checked_sub(fig.books).expect("sub"),
            fig.difference,
            "Difference = Bank says - Books will say"
        );
        // A bank account's balance is shown as the file has it.
        assert_eq!(flow::balance_figures(&check, false).bank, m("-429.12"));
    }
}

#[test]
fn a_csv_shows_its_columns_and_skips_zero_rows() {
    let f = fixture();
    let mut s = screen(&f);
    read(&mut s, &f, "bank.csv", CSV);
    assert!(s.csv_mapping.is_some(), "{:?}", s.read_error);
    let st = s.state();
    assert_eq!(st["csv"], true);
    assert_eq!(st["rows"], 2);
    assert_eq!(st["skipped"], 1);
}

#[test]
fn a_broken_file_names_its_line_and_never_moves_on() {
    let f = fixture();
    let mut s = screen(&f);
    read(
        &mut s,
        &f,
        "bad.ofx",
        "OFXHEADER:100\nDATA:OFXSGML\nVERSION:102\n\n<!DOCTYPE x>\n<OFX>\n",
    );
    let e = s.state()["read_error"].clone();
    assert!(e.is_object(), "{e}");
    assert!(!s.can_next());
    read(&mut s, &f, "empty.csv", "");
    assert!(s.read_error.is_some());
}

#[test]
fn the_state_has_no_owner_data() {
    let f = fixture();
    let mut s = screen(&f);
    previewed(&mut s, &f);
    let text = s.state().to_string();
    for secret in ["operating", "2208", "MONETT", "186.40", "92431"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
}

#[test]
fn alt_n_and_alt_b_move_between_steps() {
    let f = fixture();
    let mut s = screen(&f);
    read(&mut s, &f, "operating.qfx", OFX);
    assert!(s.command(CommandId::new("form.save_new")));
    assert_eq!(s.step, Step::Link);
    assert!(s.command(CommandId::new("form.step_back")));
    assert_eq!(s.step, Step::Choose);
    assert_eq!(s.scope(), cl_model::Scope::Form);
}

/// Q-P25ae: Esc goes back one step; on step 1 the window closes (the
/// shell asks first while a chosen file is not sent).
#[test]
fn esc_goes_back_then_closes_asking_while_a_file_is_chosen() {
    let f = fixture();
    let mut s = screen(&f);
    assert!(
        !s.command(CommandId::new("form.revert")),
        "nothing chosen: close"
    );
    assert!(!s.dirty());
    previewed(&mut s, &f);
    assert!(s.dirty(), "a file chosen and not sent");
    assert!(s.command(CommandId::new("form.revert")));
    assert_eq!(s.step, Step::Link);
    assert!(s.command(CommandId::new("form.revert")));
    assert_eq!(s.step, Step::Choose);
    assert!(
        !s.command(CommandId::new("form.revert")),
        "step 1: the shell closes"
    );
    s.next();
    s.next();
    s.poll();
    s.next();
    assert_eq!(s.step, Step::Done);
    assert!(!s.dirty(), "sent: closes without asking");
    assert!(!s.command(CommandId::new("form.revert")));
}

#[test]
fn every_text_key_is_in_the_catalog() {
    for k in [
        "bankimport.subtitle",
        "bankimport.changed.heading",
        "bankimport.changed.heading.one",
        "bankimport.changed.hint",
        "bankimport.changed.was",
        "bankimport.changed.now",
        "bankimport.changed.skip",
        "bankimport.changed.replace_off",
        "bankimport.changed.left",
        "bankimport.changed.chosen_replace",
        "bankimport.changed.chosen_add",
        "bankimport.changed.chosen_skip",
        "bankimport.balance.bank_says_owe",
        "bankimport.balance.books_will_owe",
        "bankimport.balance.books_pending",
        "bankimport.balance.books_pending_owe",
        "bankimport.balance.books_now",
        "bankimport.balance.books_now_owe",
        "bankimport.status.possible_on",
        "bankimport.linked_elsewhere",
        "bankimport.undo.used",
        "picker.title.bank_file",
        "orphan.bank_import",
    ] {
        assert!(crate::i18n::has(k), "{k}");
    }
}

/// Q-P25c (owner 2026-11-07): a look-alike row is added with a flag, not
/// skipped; the demo applies the same rule as the engine.
#[test]
fn a_look_alike_is_added_with_a_flag() {
    let f = fixture();
    let mut s = screen(&f);
    read(
        &mut s,
        &f,
        "a.csv",
        "Date,Description,Amount\n11/01/2026,COFFEE ROASTERS 0031,-4.50\n",
    );
    s.next();
    s.pick_account(bank_account(&s));
    s.set_flipped(s.flipped);
    s.next();
    s.poll();
    s.next();
    assert_eq!(s.result.as_ref().expect("sent").counts.inserted, 1);
    s.start_over(false);
    read(
        &mut s,
        &f,
        "b.csv",
        // The file's range (11/01-11/03) holds the 11/01 row (Q-P25c).
        "Date,Description,Amount\n11/01/2026,BAKERY,-3.00\n11/03/2026,COFFEE ROASTERS,-4.50\n",
    );
    s.next();
    s.pick_account(bank_account(&s));
    s.set_flipped(s.flipped);
    s.next();
    s.poll();
    let p = s.preview.as_ref().expect("preview");
    assert_eq!(p.counts.possible_duplicate, 1, "{:?}", p.rows);
    assert_eq!(flow::rows_to_send(p, &s.choices), 2);
    assert!(p.rows[1].existing_imported_at.is_some());
    s.next();
    assert_eq!(s.result.as_ref().expect("sent").counts.inserted, 2);
}

fn bank_fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/bank")
        .join(name)
}

fn read_path(s: &mut ImportBankFileScreen, path: std::path::PathBuf) {
    s.read(path, None);
    for _ in 0..1000 {
        s.poll();
        if s.reading.is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the file was never read");
}

/// The mockup's story: the prior download, then the 11/03 file shows 41
/// transactions, 37 new and 4 already imported (scenario `import_bank_file`).
#[test]
fn the_mockup_story_on_the_real_fixtures() {
    let f = fixture();
    let mut s = screen(&f);
    for (file, expect_new) in [
        ("operating_prior.qfx", None),
        ("operating_2026-11-03.qfx", Some(37)),
    ] {
        read_path(&mut s, bank_fixture(file));
        assert!(s.read_error.is_none(), "{file}: {:?}", s.read_error);
        s.next();
        s.pick_account(bank_account(&s));
        if s.sign_needs_confirm() {
            s.set_flipped(s.flipped);
        }
        s.next();
        s.poll();
        let p = s.preview.as_ref().expect("preview");
        if let Some(n) = expect_new {
            assert_eq!(
                (p.counts.in_file, p.counts.new, p.counts.duplicate),
                (41, n, 4),
                "{file}"
            );
        }
        s.next();
        assert_eq!(s.step, Step::Done, "{file}");
        s.start_over(false);
    }
}

/// Steps 1 and 2 of `path` into `account`.
fn into_account(s: &mut ImportBankFileScreen, path: std::path::PathBuf, account: AccountId) {
    read_path(s, path);
    assert!(s.read_error.is_none(), "{:?}", s.read_error);
    s.next();
    s.pick_account(account);
    wait_reading(s);
}

fn wait_reading(s: &mut ImportBankFileScreen) {
    for _ in 0..500 {
        s.poll();
        if s.reading.is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("never read");
}

/// Review DB1: a card CSV (charges positive) into a card account asks
/// for the sign with the flip suggested; the answer is kept for the next
/// file of that account and format (Q-P25t).
#[test]
fn a_card_csv_into_a_card_account_asks_once() {
    let f = fixture();
    let mut s = screen(&f);
    let card = card_account(&s);
    into_account(&mut s, bank_fixture("card_positive_charges.csv"), card);
    assert_eq!(s.state()["card_account"], true);
    assert!(
        s.sign_needs_confirm(),
        "a card account asks on its first import"
    );
    assert!(s.flipped, "charges are positive: the flip is suggested");
    assert!(!s.can_next());
    s.set_flipped(s.flipped);
    assert!(s.can_next());
    s.next();
    s.poll();
    let req = s.request().expect("request");
    let charge = req.rows.iter().find(|r| r.line == 2).expect("line 2");
    assert_eq!(
        cl_api::wire_decimal(charge.amount).as_deref(),
        Ok("45.67"),
        "a charge is money out: positive on the wire"
    );
    s.next();
    assert_eq!(s.step, Step::Done);
    s.start_over(false);
    into_account(&mut s, bank_fixture("card_positive_charges.csv"), card);
    assert_eq!(s.remembered_flip, Some(true));
    assert!(!s.sign_needs_confirm());
    assert!(s.flipped);
    // The same file into a bank account: no card rule, nothing to ask.
    s.start_over(false);
    let bank = bank_account(&s);
    into_account(&mut s, bank_fixture("card_positive_charges.csv"), bank);
    assert!(!s.sign_needs_confirm());
    assert!(!s.flipped);
}

/// Review DB2: after the account's last batch set the columns, the
/// owner's change stays (it is not put back by the history).
#[test]
fn an_owner_mapping_survives_the_history() {
    let f = fixture();
    let mut s = screen(&f);
    let bank = bank_account(&s);
    into_account(&mut s, bank_fixture("dates_ambiguous.csv"), bank);
    s.set_flipped(s.flipped);
    s.next();
    s.poll();
    s.next();
    assert_eq!(s.step, Step::Done);
    s.start_over(false);
    into_account(&mut s, bank_fixture("dates_ambiguous.csv"), bank);
    assert_eq!(s.state()["csv_source"], "batch");
    let mut m = s.csv_mapping.clone().expect("mapping");
    m.date_order = cl_import::DateOrder::Dmy;
    s.set_mapping(m.clone());
    wait_reading(&mut s);
    assert_eq!(s.csv_mapping.as_ref(), Some(&m), "the owner's columns stay");
    assert_eq!(s.state()["csv_source"], "owner");
    assert_eq!(s.state()["csv_date_order"], "dmy");
    for _ in 0..5 {
        s.poll();
    }
    assert_eq!(s.csv_mapping.as_ref(), Some(&m));
}

/// Review DB2: a remembered layout whose columns were reordered (same
/// count) is not applied; the columns are guessed again.
#[test]
fn a_remembered_layout_that_does_not_fit_is_not_used() {
    let f = fixture();
    let mut s = screen(&f);
    let bank = bank_account(&s);
    read(
        &mut s,
        &f,
        "a.csv",
        "Date,Description,Debit,Credit\n11/01/2026,COFFEE,4.50,\n11/02/2026,DEPOSIT,,120.00\n",
    );
    s.next();
    s.pick_account(bank);
    s.set_flipped(s.flipped);
    s.next();
    s.poll();
    s.next();
    assert_eq!(s.step, Step::Done);
    s.start_over(false);
    read(
        &mut s,
        &f,
        "b.csv",
        "Date,Description,Credit,Debit\n11/03/2026,COFFEE,,4.50\n11/04/2026,DEPOSIT,120.00,\n",
    );
    s.next();
    s.pick_account(bank);
    wait_reading(&mut s);
    assert_eq!(s.state()["csv_source"], "guess");
    // QA-6: the owner is told the columns changed since last time.
    assert_eq!(s.state()["layout_changed"], true);
    let money_out = s.current_section().expect("section").rows[0].file_amount;
    assert!(money_out.is_sign_negative(), "debit stays money out");
}

/// QA-4: the flip remembered from the issuer-signed QFX is not applied to
/// a holder-signed file of the same card; the evidence wins and the owner
/// is asked again, and the 8 rows are then already imported, not changed.
#[test]
fn a_remembered_flip_the_file_contradicts_is_asked_again() {
    let f = fixture();
    let mut s = screen(&f);
    let card = card_account(&s);
    into_account(&mut s, bank_fixture("card_positive_issuer.qfx"), card);
    assert!(s.flipped);
    s.set_flipped(true);
    s.next();
    s.poll();
    s.next();
    assert_eq!(s.step, Step::Done);
    prepared(&mut s);
    s.start_over(false);

    read_path(&mut s, bank_fixture("card_ccstmt.qfx"));
    s.next();
    assert_eq!(s.account, Some(card), "linked by the card number");
    let st = s.state();
    assert_eq!(st["remembered_flip"], true);
    assert_eq!(st["sign_differs"], true, "{st}");
    assert_eq!(st["sign_needs_confirm"], true);
    assert!(!s.flipped, "the file's evidence is the default");
    assert!(!s.can_next());
    s.set_flipped(s.flipped);
    s.next();
    s.poll();
    let p = s.preview.as_ref().expect("preview");
    assert_eq!((p.counts.duplicate, p.counts.changed), (8, 0));

    // The same issuer file again: the remembered flip agrees, no question.
    s.start_over(false);
    read_path(&mut s, bank_fixture("card_positive_issuer.qfx"));
    s.next();
    assert_eq!(s.state()["sign_differs"], false);
    assert!(!s.sign_needs_confirm());
    assert!(s.flipped);
}

/// QA-5: a checking section into a card account (and a card file into a
/// bank account) warns; a CSV never does.
#[test]
fn the_file_and_account_kinds_are_compared() {
    let f = fixture();
    let mut s = screen(&f);
    let (card, bank) = (card_account(&s), bank_account(&s));
    read_path(&mut s, bank_fixture("multi_account.ofx"));
    s.next();
    s.pick_section(0);
    s.pick_account(card);
    assert_eq!(s.type_mismatch(), Some("bankimport.type.bank_file"));
    assert_eq!(s.state()["type_mismatch"], true);
    s.pick_account(bank);
    assert_eq!(s.type_mismatch(), None);
    s.pick_section(2);
    s.pick_account(bank);
    assert_eq!(s.type_mismatch(), Some("bankimport.type.card_file"));
    for k in ["bankimport.type.bank_file", "bankimport.type.card_file"] {
        assert!(crate::i18n::has(k), "{k}");
    }
    assert_eq!(
        flow::type_mismatch(cl_import::AccountKind::Unknown, true),
        None
    );
    assert_eq!(
        flow::type_mismatch(cl_import::AccountKind::CreditLine, false),
        None
    );
}

/// QA-7 and R2-5: a balance column can be chosen by hand (here one the
/// guess doesn't know), and a layout using one column twice says why.
#[test]
fn a_balance_column_is_mapped_by_hand() {
    let f = fixture();
    let mut s = screen(&f);
    read(
        &mut s,
        &f,
        "bal.csv",
        "Date,Description,Amount,Avail
11/01/2026,COFFEE,-4.50,95.50
11/02/2026,DEPOSIT,120.00,215.50
",
    );
    let mut m = s.csv_mapping.clone().expect("csv");
    assert_eq!(m.balance, None, "not guessed");
    assert!(s.current_section().expect("s").ledger_balance.is_none());
    m.balance = Some(3);
    s.set_mapping(m.clone());
    wait_reading(&mut s);
    let b = s
        .current_section()
        .expect("s")
        .ledger_balance
        .expect("balance");
    assert_eq!(b.amount, rust_decimal::Decimal::new(21550, 2));
    assert_eq!(flow::mapping_clash(&m), None);
    m.date = 2;
    assert_eq!(flow::mapping_clash(&m), Some("bankimport.csv.same_date"));
    m.date = 0;
    m.amount = cl_import::AmountColumns::DebitCredit {
        debit: 2,
        credit: 2,
    };
    assert_eq!(flow::mapping_clash(&m), Some("bankimport.csv.same_in_out"));
    for k in [
        "bankimport.csv.same_date",
        "bankimport.csv.same_in_out",
        "bankimport.csv.same_indicator",
        "bankimport.csv.changed",
        "bankimport.csv.balance",
        "bankimport.sign.remembered_differs",
    ] {
        assert!(crate::i18n::has(k), "{k}");
    }
}

/// QA-8: back to step 1 and another file replaces the first, and an
/// answer still on its way for the first file is not applied.
#[test]
fn another_file_after_going_back_replaces_the_first() {
    let f = fixture();
    let mut s = screen(&f);
    read(&mut s, &f, "first.csv", CSV);
    s.next();
    s.pick_account(bank_account(&s));
    s.back();
    assert_eq!(s.step, Step::Choose);
    read(&mut s, &f, "operating.qfx", OFX);
    let l = s.loaded.as_ref().expect("loaded");
    assert_eq!(l.file_name, "operating.qfx");
    assert_eq!(s.state()["file"]["format"], "ofx");
    assert_eq!(s.account, None, "the link starts over");
    assert!(s.history_task.is_none());
}

/// Q-P25aa (owner 2026-11-07): after a restore the company keeps its
/// links (the engine holds the key, Q-P25u): the account links itself.
#[test]
fn after_a_restore_the_account_links_itself() {
    let f = fixture();
    let mut s = screen(&f);
    previewed(&mut s, &f);
    s.next();
    assert_eq!(s.step, Step::Done);
    assert!(s.command(CommandId::new("dev.demo_bank_import_restore")));
    assert_eq!(s.step, Step::Choose);
    read(&mut s, &f, "operating.qfx", OFX);
    s.next();
    assert_eq!(s.state()["linked"], "auto");
    assert!(s.account.is_some());
    assert!(!s.sign_needs_confirm());
}

/// Q-P25aa: a second computer has no layouts of its own; the company's
/// latest batch brings the owner's columns.
#[test]
fn a_second_computer_reuses_the_companys_csv_layout() {
    let f = fixture();
    let mut s = screen(&f);
    let bank = bank_account(&s);
    into_account(&mut s, bank_fixture("dates_ambiguous.csv"), bank);
    let mut m = s.csv_mapping.clone().expect("mapping");
    m.date_order = cl_import::DateOrder::Dmy;
    s.set_mapping(m.clone());
    wait_reading(&mut s);
    s.set_flipped(s.flipped);
    s.next();
    s.poll();
    s.next();
    assert_eq!(s.step, Step::Done);
    assert!(s.command(CommandId::new("dev.bank_import_forget_local")));
    let mut other = screen(&f);
    into_account(&mut other, bank_fixture("dates_ambiguous.csv"), bank);
    assert_eq!(other.state()["csv_source"], "batch");
    assert_eq!(other.csv_mapping.as_ref(), Some(&m));
}

/// Q-P25v: the result waits for the preparation, then says it is done;
/// the window may be closed meanwhile.
#[test]
fn the_result_waits_for_the_preparation() {
    let f = fixture();
    let mut s = screen(&f);
    previewed(&mut s, &f);
    s.next();
    assert_eq!(s.state()["result"]["post_processing"], "scheduled");
    assert!(s.busy(), "keeps repainting while preparing");
    prepared(&mut s);
    assert_eq!(s.state()["result"]["post_processing"], "done");
    assert!(!s.busy());
}

/// The demo refuses a commit of a file linked to another account (the
/// engine's 409) until the owner relinks it.
#[test]
fn a_file_linked_elsewhere_is_refused_until_relinked() {
    let f = fixture();
    let mut s = screen(&f);
    previewed(&mut s, &f);
    s.next();
    s.start_over(false);
    read(&mut s, &f, "operating.qfx", OFX);
    s.next();
    let other = s
        .targets
        .iter()
        .find(|t| Some(t.id) != s.account)
        .expect("another account")
        .id;
    s.pick_account(other);
    if s.sign_needs_confirm() {
        s.set_flipped(s.flipped);
    }
    s.next();
    s.poll();
    assert_eq!(s.state()["preview"]["linked_elsewhere"], true);
    assert!(!s.can_next());
    s.send();
    assert_eq!(s.state()["send_trouble"], "bankimport.linked_elsewhere");
    s.relink = true;
    s.send();
    assert_eq!(s.step, Step::Done);
}

/// Q-P25ac: past the 120 s wait the owner is told the preparation goes on,
/// and it is still checked, so Undo opens once it is done; counts are
/// grouped by thousands (R2-7).
#[test]
fn after_the_wait_gives_up_the_preparation_is_still_checked() {
    let f = fixture();
    let mut s = screen(&f);
    previewed(&mut s, &f);
    s.next();
    let now = std::time::Instant::now();
    s.prep_started = now.checked_sub(PREPARING_BUDGET + std::time::Duration::from_secs(1));
    s.prep_next = Some(now);
    s.poll();
    assert_eq!(s.state()["prep_gave_up"], true);
    assert!(s.preparing() || s.state()["result"]["post_processing"] == "done");
    // The demo's preparation ends after 800 ms; the slow check finds it.
    std::thread::sleep(std::time::Duration::from_millis(900));
    s.prep_next = Some(std::time::Instant::now());
    s.poll();
    assert_eq!(s.state()["result"]["post_processing"], "done");
    assert!(!s.preparing(), "Undo opens");
    assert_eq!(crate::i18n::count_text(20_000), "20,000");
    assert_eq!(crate::i18n::count_text(999), "999");
    assert_eq!(crate::i18n::count_text(1_234_567), "1,234,567");
    assert_eq!(
        crate::i18n::t_count("bankimport.done.sent_all", 20_000, &[]),
        "Sent 20,000 to the Bank Feed."
    );
    for k in ["bankimport.done.preparing_of", "bankimport.done.prep_done"] {
        assert!(crate::i18n::has(k), "{k}");
    }
}

/// U-175: checking, savings, cash and petty cash accounts are offered next
/// to `bank` and cards; other assets are not.
#[test]
fn every_bank_register_type_is_offered() {
    let f = fixture();
    let mut s = screen(&f);
    assert!(s.all_bank_types(), "the demo takes them all");
    let template = crate::company::demo::chart()
        .into_iter()
        .find(|a| a.fields.account_type == cl_api::AccountType::Asset)
        .expect("an asset");
    let accounts: Vec<Account> = [
        "bank",
        "checking",
        "savings",
        "cash",
        "petty_cash",
        "prepaid_expenses",
    ]
    .into_iter()
    .enumerate()
    .map(|(i, d)| {
        let mut a = template.clone();
        a.id = format!("3b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{i:02}")
            .parse()
            .expect("id");
        a.fields.account_number = Some(format!("10{i}0"));
        a.fields.name = d.to_string();
        a.fields.detail_type = Some(d.to_string());
        a.is_active = true;
        a
    })
    .collect();
    s.take_targets(accounts);
    let names: Vec<&str> = s.targets.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names.len(), 5, "{names:?}");
    assert!(names.iter().all(|n| !n.contains("prepaid")));
    assert!(s.targets.iter().all(|t| !t.card));
}

/// The dark run after a light one (scenario `import_bank_file`): a reset
/// feed reuses its row keys; the review forgets the old ones, so the rows
/// sent next reach the Bank Feed and its badge.
#[test]
fn a_reset_feed_sends_its_rows_again() {
    let f = fixture();
    let send_prior = |f: &Fixture| {
        let mut demo = f.link.demo_state().expect("demo");
        let before = crate::company::demo::feed_review::waiting(&mut demo);
        drop(demo);
        let mut s = screen(f);
        previewed(&mut s, f);
        s.next();
        assert_eq!(s.result.as_ref().expect("result").counts.inserted, 3);
        prepared(&mut s);
        let mut demo = f.link.demo_state().expect("demo");
        crate::company::demo::feed_review::waiting(&mut demo) - before
    };
    assert_eq!(send_prior(&f), 3);
    let mut s = screen(&f);
    assert!(s.command(CommandId::new("dev.demo_bank_import_reset")));
    let mut demo = f.link.demo_state().expect("demo");
    assert_eq!(crate::company::demo::feed_review::waiting(&mut demo), 37);
    drop(demo);
    assert_eq!(send_prior(&f), 3, "the same keys again");
}
