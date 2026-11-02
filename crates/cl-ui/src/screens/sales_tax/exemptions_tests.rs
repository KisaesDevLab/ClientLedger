//! Tax Exemption Certificates over the demo engine.

use super::*;
use crate::company::demo::DemoState;

fn screen() -> (tempfile::TempDir, ExemptionsScreen) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let s = ExemptionsScreen::new(link, DemoState::today());
    (dir, s)
}

#[test]
fn a_certificate_saves_lists_expiring_and_deactivates() {
    let (_d, mut s) = screen();
    assert_eq!(s.state()["certificates"], 0);
    let client = s.clients.first().map(|c| c.id).expect("a client");
    s.set_client(Some(client));
    s.new_entry();
    s.save();
    assert_eq!(
        s.state()["problem"],
        "tax_exemptions.problem.number_required"
    );
    if let Some((f, _)) = s.editor.as_mut() {
        f.number = "RS-1001".into();
        f.state = "Missouri".into();
    }
    s.save();
    assert_eq!(s.state()["problem"], "tax_exemptions.problem.bad_state");
    if let Some((f, _)) = s.editor.as_mut() {
        f.state = "mo".into();
        f.expires_on = CalendarDate::from_ymd(2026, 12, 15).ok();
    }
    s.save();
    let v = s.state();
    assert_eq!(v["saved"], 1, "{v}");
    assert_eq!(v["certificates"], 1);
    assert_eq!(v["expiring"], 1, "within 60 days of 11/04");
    assert_eq!(s.list[0].state.as_deref(), Some("MO"));
    let id = s.list[0].id;
    s.open(id);
    s.deactivate();
    assert_eq!(s.state()["certificates"], 0);
    s.show_inactive = true;
    s.refresh();
    assert_eq!(s.state()["inactive"], 1, "deactivated, never deleted");
    let text = s.state().to_string();
    assert!(
        !text.contains("RS-1001"),
        "no certificate numbers in the state"
    );
}

#[test]
fn a_certificate_keeps_its_file() {
    let (_d, mut s) = screen();
    let client = s.clients.first().map(|c| c.id).expect("a client");
    s.new_entry();
    s.upload_file("cert.pdf".into(), b"%PDF-1.4\n%%EOF\n".to_vec());
    assert_eq!(
        s.state()["problem"],
        "tax_exemptions.attach.client_first",
        "attached to the client"
    );
    s.set_client(Some(client));
    s.new_entry();
    s.upload_file("cert.txt".into(), b"plain text".to_vec());
    assert_eq!(s.state()["file"], false, "checked before sending");
    s.upload_file("cert.pdf".into(), b"%PDF-1.4\n%%EOF\n".to_vec());
    assert_eq!(s.state()["file"], true, "{}", s.state());
    if let Some((f, _)) = s.editor.as_mut() {
        f.number = "RS-2001".into();
        f.state = "MO".into();
    }
    s.save();
    assert!(s.list[0].attachment_id.is_some(), "saved with the file");
    let id = s.list[0].id;
    s.open(id);
    assert_eq!(s.state()["file"], true, "reopened with its file");
}
