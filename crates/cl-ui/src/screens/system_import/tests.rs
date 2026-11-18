//! Import from another program against the demo engine: each source's
//! files import as one batch (date asked first, sent with each upload),
//! tie out to 0.00, and a re-import is refused.

use std::sync::Arc;

use super::*;
use crate::company::CompanyLink;
use cl_model::imports::wizard::Step;

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 18).expect("date")
}

fn link() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    {
        let mut d = link.demo_state().expect("demo");
        let open = d.open.expect("open");
        if let Some(c) = d.companies.iter_mut().find(|c| c.tenant_id == open) {
            c.closed_through = None;
        }
    }
    (dir, link)
}

fn run(s: &mut ImportScreen) {
    s.next(); // program chosen
    assert_eq!(s.wizard.step, Step::Dates);
    s.date_text = "06/30/2026".into();
    s.opening_text = "12/31/2025".into();
    s.sync_dates();
    assert!(s.can_next(), "{}", s.state());
    s.next();
    assert_eq!(s.wizard.step, Step::Files);
    assert!(s.command(CommandId::new("dev.imports_demo_file_all")));
    s.poll();
    assert!(s.can_next(), "{}", s.state());
    s.next();
    assert_eq!(s.wizard.step, Step::Mapping);
    s.next();
    assert_eq!(s.wizard.step, Step::Import);
    s.next();
    for _ in 0..6 {
        s.poll();
    }
}

#[test]
fn each_program_imports_and_ties_out() {
    for source in SOURCES {
        let (_d, link) = link();
        let mut s = ImportScreen::new(Arc::clone(&link), today(), SOURCES[0]);
        s.poll();
        s.wizard.set_source(source);
        run(&mut s);
        assert!(s.wizard.imported, "{source:?}: {}", s.state());
        assert_eq!(s.wizard.step, Step::TieOut, "{source:?}");
        s.poll();
        assert_eq!(
            s.tieout.as_ref().map(|t| t.all_zero),
            Some(true),
            "{source:?}"
        );
        assert_eq!(s.state()["screen"], KIND);
        assert!(!source.requires_empty_company());
    }
}

#[test]
fn a_reimport_of_the_same_files_is_refused() {
    let (_d, link) = link();
    let mut s = ImportScreen::new(Arc::clone(&link), today(), SOURCES[0]);
    s.poll();
    run(&mut s);
    assert!(s.wizard.imported);
    drop(s);
    let mut again = ImportScreen::new(Arc::clone(&link), today(), SOURCES[0]);
    again.poll();
    again.next();
    again.date_text = "06/30/2026".into();
    again.sync_dates();
    again.next();
    assert!(again.command(CommandId::new("dev.imports_demo_file_all")));
    again.poll();
    assert!(
        again.uploads.values().flatten().any(|f| f.refusal.is_some()),
        "{}",
        again.state()
    );
    assert!(!again.can_next());
}
