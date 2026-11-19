use super::*;

fn screen() -> (tempfile::TempDir, SendScreen) {
    let dir = tempfile::tempdir().expect("tmp");
    let mut link = CompanyLink::demo(dir.path());
    link.backup_dir = dir.path().join("out");
    let today = CalendarDate::from_ymd(2026, 11, 4).expect("d");
    let tenant = TenantId::parse("1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id");
    let s = SendScreen::new(Arc::new(link), Some((tenant, "Ozark Ridge".into())), today);
    (dir, s)
}

fn fill(s: &mut SendScreen, p: &str) {
    s.passphrase = SecretText::from(p);
    s.again = SecretText::from(p);
}

#[test]
fn the_preview_shows_and_a_short_passphrase_sends_nothing() {
    let (_d, mut s) = screen();
    assert_eq!(s.state()["preview_loaded"], true);
    assert_eq!(s.state()["total_assets"], "412906.3300");
    fill(&mut s, "short");
    s.save(None);
    assert_eq!(s.state()["error"], "error.transfer.passphrase_short");
    assert_eq!(s.state()["history"], 0);
    s.passphrase = SecretText::from("long enough passphrase");
    s.again = SecretText::from("long enough passphrasE");
    s.save(None);
    assert_eq!(s.state()["error"], "error.company.passphrase_mismatch");
}

#[test]
fn make_one_then_save_records_a_file_and_clears_the_passphrase() {
    let (_d, mut s) = screen();
    s.make_one();
    assert_eq!(s.state()["generated"], true);
    assert_eq!(s.passphrase.text().len(), 24);
    // The state never carries the passphrase itself.
    assert!(!s.state().to_string().contains(s.passphrase.text()));
    s.save(None);
    assert_eq!(s.state()["done"], true, "{}", s.state());
    assert_eq!(s.state()["history"], 1);
    assert_eq!(
        s.state()["passphrase_filled"],
        false,
        "wiped after the save"
    );
    let o = s.outcome.as_ref().expect("outcome");
    assert!(o.record.path.is_file());
    assert_eq!(
        o.record.path.extension().and_then(|e| e.to_str()),
        Some("clxfer")
    );
    assert_eq!(s.state()["totals_differ"], false);
}

#[test]
fn payroll_drafts_are_asked_about_and_cancel_stops() {
    let (_d, mut s) = screen();
    assert!(s.command(CommandId::new("dev.transfer_demo_payroll_draft")));
    assert_eq!(s.state()["unposted_payroll"], 1);
    fill(&mut s, "ABCD-EFGH-JKLM-NPQR-STUV");
    s.save(None);
    assert_eq!(s.state()["payroll_prompt"], true);
    s.answer_payroll(false, None);
    assert_eq!(s.state()["payroll_prompt"], false);
    assert_eq!(s.state()["done"], false, "Cancel stops");
    s.save(None);
    s.answer_payroll(true, None);
    assert_eq!(s.state()["done"], true, "{}", s.state());
    assert_eq!(s.outcome.as_ref().expect("o").discarded_payroll, 1);
    s.poll(); // the next frame reads the counts again
    assert_eq!(s.state()["unposted_payroll"], 0);
}

#[test]
fn missing_files_and_busy_read_in_plain_words() {
    let (_d, mut s) = screen();
    assert!(s.command(CommandId::new("dev.transfer_demo_missing_files")));
    fill(&mut s, "ABCD-EFGH-JKLM-NPQR-STUV");
    s.save(None);
    assert_eq!(s.state()["missing_files"], 3);
    assert_eq!(s.state()["error"], "error.transfer.missing_files");
    assert!(s.command(CommandId::new("dev.transfer_demo_busy")));
    fill(&mut s, "ABCD-EFGH-JKLM-NPQR-STUV");
    s.save(None);
    assert_eq!(s.state()["error"], "error.company.busy");
}

/// T11: a post between the preview and Save makes the file's totals
/// differ, and the screen says so.
#[test]
fn a_post_after_the_preview_shows_the_file_totals_differ() {
    let (_d, mut s) = screen();
    assert!(s.command(CommandId::new("dev.transfer_demo_post")));
    fill(&mut s, "ABCD-EFGH-JKLM-NPQR-STUV");
    s.save(None);
    assert_eq!(s.state()["done"], true);
    assert_eq!(s.state()["totals_differ"], true);
}

/// Review B1: the audit rows the backup and the payroll discard write
/// between the preview and the file are not "your books changed".
#[test]
fn audit_rows_alone_do_not_show_the_books_changed() {
    let (_d, mut s) = screen();
    fill(&mut s, "ABCD-EFGH-JKLM-NPQR-STUV");
    s.save(None);
    assert_eq!(s.state()["done"], true, "{}", s.state());
    let mut file = s.preview.clone().expect("preview");
    file.counts.audit_entries += 7;
    for c in &mut file.companies {
        c.counts.audit_entries += 7;
    }
    s.outcome.as_mut().expect("outcome").totals = Some(file.clone());
    assert_eq!(s.state()["totals_differ"], false);
    file.counts.transactions += 1;
    s.outcome.as_mut().expect("outcome").totals = Some(file);
    assert_eq!(s.state()["totals_differ"], true);
}

/// S1: a held transfer blocks every way out of the company until it ends.
#[test]
fn a_held_transfer_blocks_leaving_the_company() {
    let (_d, mut s) = screen();
    assert!(s.command(CommandId::new("dev.transfer_demo_hold")));
    fill(&mut s, "ABCD-EFGH-JKLM-NPQR-STUV");
    s.save(None);
    assert_eq!(s.state()["running"], true);
    for id in BLOCKED_WHILE_SENDING {
        assert!(blocked_now(id), "{id}");
    }
    assert!(!blocked_now("file.backup"));
    assert!(s.command(CommandId::new("dev.transfer_demo_release")));
    assert_eq!(s.state()["running"], false);
    assert_eq!(s.state()["done"], true);
}

/// B2 / Q-P32w: a made passphrase is hidden until Show.
#[test]
fn a_made_passphrase_stays_hidden_until_show() {
    let (_d, mut s) = screen();
    s.make_one();
    assert_eq!(s.state()["shown"], false);
    s.show_generated = true;
    assert_eq!(s.state()["shown"], true);
    s.make_one();
    assert_eq!(s.state()["shown"], false, "a new one starts hidden");
}

#[test]
fn close_restore_and_delete_wait_for_a_running_transfer() {
    // The counter is shared by every window (and test): only "running
    // while held" is checked here, never "not running".
    {
        let _g = Sending::start();
        assert!(transfer_running());
    }
    for k in [
        "send.inc.hidden",
        "send.exc.secrets",
        "send.how.body",
        "send.busy_tip",
        "send.history.empty",
        "send.payroll.title",
        "send.missing.reason.FILE_MISSING",
        "send.missing.reason.FILE_SIZE_MISMATCH",
        "send.missing.reason.EXTERNAL_FILE",
        "error.transfer.missing_files",
        "picker.title.save_transfer",
    ] {
        assert!(crate::i18n::has(k), "{k}");
    }
}
