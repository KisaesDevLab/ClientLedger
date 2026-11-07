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
    s.next();
    assert_eq!(s.result.as_ref().expect("result").counts.inserted, 0);
    assert_eq!(f.link.demo_state().expect("demo").bank_feed.waiting(), 3);
}

#[test]
fn undo_takes_the_rows_out_and_they_come_back() {
    let f = fixture();
    let mut s = screen(&f);
    previewed(&mut s, &f);
    s.next();
    s.undo(false);
    assert_eq!(s.undone.map(|u| u.removed), Some(3));
    assert_eq!(f.link.demo_state().expect("demo").bank_feed.waiting(), 0);
    s.undo(false);
    assert_eq!(s.state()["undo_trouble"], "bankimport.undo.already");
    s.start_over(false);
    previewed(&mut s, &f);
    assert_eq!(s.preview.as_ref().expect("preview").counts.new, 3);
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

#[test]
fn every_text_key_is_in_the_catalog() {
    for k in [
        "bankimport.subtitle",
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
        "Date,Description,Amount\n11/03/2026,COFFEE ROASTERS,-4.50\n",
    );
    s.next();
    s.pick_account(bank_account(&s));
    s.set_flipped(s.flipped);
    s.next();
    s.poll();
    let p = s.preview.as_ref().expect("preview");
    assert_eq!(p.counts.possible_duplicate, 1, "{:?}", p.rows);
    assert_eq!(flow::rows_to_send(p, &s.choices), 1);
    assert!(p.rows[0].existing_imported_at.is_some());
    s.next();
    assert_eq!(s.result.as_ref().expect("sent").counts.inserted, 1);
}
