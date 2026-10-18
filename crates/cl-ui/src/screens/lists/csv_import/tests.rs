//! Import or Export a List tests (headless, demo link).

use std::sync::Arc;

use cl_api::ContactType;
use cl_core::Money;
use cl_core::ids::TenantId;
use cl_model::lists::csv_map::Target;
use eframe::egui::{self, Pos2, RawInput, Rect, vec2};

use super::{CsvScreen, ListChoice, Skip};
use crate::company::CompanyLink;
use crate::shell::Shell;
use crate::windows::Screen;

const TENANT: &str = "d0000000-0000-4000-8000-000000000001";

struct Rig {
    dir: tempfile::TempDir,
    ctx: egui::Context,
    link: Arc<CompanyLink>,
    s: CsvScreen,
}

impl Rig {
    fn new() -> Rig {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let id = TenantId::parse(TENANT).expect("id");
        let ctx = egui::Context::default();
        Shell::install(&ctx);
        let mut s = CsvScreen::new(Arc::clone(&link), Some((id, "Ozark".into())));
        s.folder = dir.path().join("exports").display().to_string();
        let mut rig = Rig { dir, ctx, link, s };
        rig.frames(2);
        rig
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 820.0))),
                ..RawInput::default()
            };
            let s = &mut self.s;
            let mut out = self.ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
            });
            out.textures_delta.clear();
        }
    }

    fn state(&self, key: &str) -> serde_json::Value {
        self.s.state()[key].clone()
    }

    fn file(&self, name: &str, text: &str) -> String {
        let p = self.dir.path().join(name);
        std::fs::write(&p, text).expect("write");
        p.display().to_string()
    }

    fn read(&mut self, name: &str, text: &str) {
        self.s.file = self.file(name, text);
        self.s.read_file();
        self.frames(2);
    }
}

#[test]
fn exports_every_list_to_a_new_file() {
    let mut r = Rig::new();
    assert_eq!(r.state("screen"), "import_export");
    for (list, head) in [
        (ListChoice::Accounts, "Account Number,Name,Type"),
        (ListChoice::Contacts, "Display Name,Type,Company"),
        (ListChoice::Items, "Name,Description,Unit Price"),
        (ListChoice::Tags, "Name,Group,Description"),
    ] {
        r.s.set_list(list);
        r.s.start_export();
        r.frames(1);
        assert_eq!(r.state("exported"), true, "{list:?}");
        let path = r.s.exported.clone().expect("path");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.starts_with('\u{FEFF}'), "BOM for spreadsheets");
        assert!(text[3..].starts_with(head), "{text:.60}");
        assert!(text.lines().count() > 1, "{list:?} has rows");
    }
    // The tags carry their group; a second export doesn't overwrite.
    let tags = std::fs::read_to_string(r.s.exported.clone().expect("p")).expect("read");
    assert!(tags.contains("Route"));
    r.s.start_export();
    let again = r.s.exported.clone().expect("p");
    assert!(again.display().to_string().ends_with("tags (2).csv"));
    // A folder that can't be made shows a plain message.
    let blocker = r.file("not-a-folder", "x");
    r.s.folder = blocker;
    r.s.start_export();
    r.frames(1);
    assert_eq!(r.state("export_error"), true);
    assert_eq!(r.state("exported"), false);
}

#[test]
fn imports_accounts_and_skips_numbers_already_used() {
    let mut r = Rig::new();
    r.read(
        "chart.csv",
        "Number,Name,Type,Detail Type\n77001,Truck fuel,Expense,Utilities\n,Grant income,Other Income,\n77002,,Expense,\n",
    );
    assert_eq!(r.state("file_read"), true);
    assert_eq!(r.state("columns"), 4);
    assert_eq!(r.state("mapped"), 4);
    assert_eq!(r.state("ready"), 2);
    assert_eq!(r.state("left_out"), 1);
    assert_eq!(r.state("can_import"), true);
    r.s.start_import();
    r.frames(1);
    assert_eq!(r.state("imported"), 2);
    assert_eq!(r.state("skipped"), 1);
    let has = r.link.demo_state().is_some_and(|d| {
        d.chart.as_ref().is_some_and(|c| {
            c.iter()
                .any(|a| a.fields.account_number.as_deref() == Some("77001"))
        })
    });
    assert!(has);
    // The same file again: the numbered account is already there.
    r.read("chart2.csv", "Number,Name,Type\n77001,Truck fuel,Expense\n");
    r.s.start_import();
    r.frames(1);
    assert_eq!(r.state("imported"), 0);
    let o = r.s.outcome.clone().expect("outcome");
    assert_eq!(o.skipped, vec![(2, Skip::Exists)]);
}

#[test]
fn mapping_changes_and_problems() {
    let mut r = Rig::new();
    r.read("chart.csv", "Name,Kind\nFuel,Expense\n");
    assert_eq!(r.state("mapped"), 1, "Kind isn't a known header");
    assert_eq!(r.state("problems"), 1);
    assert_eq!(r.state("can_import"), false);
    r.s.set_mapping(1, Some(Target::AccountType));
    assert_eq!(r.state("problems"), 0);
    assert_eq!(r.state("ready"), 1);
    r.s.set_mapping(1, Some(Target::AccountName));
    assert_eq!(r.state("problems"), 2, "twice and missing");
    r.frames(2);
    // Switching the list maps the same file afresh.
    r.s.set_list(ListChoice::Contacts);
    assert_eq!(r.state("list"), "contacts");
    assert_eq!(r.state("mapped"), 1);
    assert_eq!(r.state("ready"), 1);
    r.s.set_list(ListChoice::Tags);
    r.frames(2);
    assert_eq!(r.state("can_import"), false);
    assert_eq!(r.state("ready"), 0);
}

#[test]
fn imports_contacts_with_the_chosen_role() {
    let mut r = Rig::new();
    r.s.set_list(ListChoice::Contacts);
    r.read(
        "people.csv",
        "Display Name,Company,Email,Phone\nRiver Supply,River Supply LLC,orders@example.com,555-0100\nBad Mail,,nope,\n",
    );
    assert_eq!(r.state("ready"), 1);
    r.s.role = ContactType::Vendor;
    r.frames(2);
    r.s.start_import();
    r.frames(1);
    assert_eq!(r.state("imported"), 1);
    let vendor = r.link.demo_state().and_then(|mut d| {
        d.contacts_mut()
            .iter()
            .find(|c| c.fields.display_name == "River Supply")
            .map(|c| c.fields.contact_type)
    });
    assert_eq!(vendor, Some(ContactType::Vendor));
}

/// S2: a name an active contact already has is left out, the file's own
/// duplicates too, and the names are read again after an import, whether
/// it worked or failed.
#[test]
fn contact_import_leaves_out_names_already_used() {
    let mut r = Rig::new();
    r.s.set_list(ListChoice::Contacts);
    r.frames(2);
    assert!(r.s.lookups.is_some(), "contact names read");
    r.read(
        "people.csv",
        "Display Name\nCrane Creek Tree Service\nNew Person\nnew person\n",
    );
    assert_eq!(r.state("ready"), 1);
    assert_eq!(r.state("left_out"), 2);
    r.s.start_import();
    r.frames(2);
    assert_eq!(r.state("imported"), 1);
    assert!(r.s.lookups.is_some(), "read again after the import");
    r.read("people2.csv", "Display Name\nNEW PERSON\n");
    assert_eq!(r.state("ready"), 0, "now taken");

    // A failed import: no second import until the names are read again.
    r.read("people3.csv", "Display Name\nAnother Person\n");
    assert_eq!(r.state("ready"), 1);
    r.s.import_finished(Err(cl_api::ApiError::NotFound { code: None }));
    assert!(r.s.lookups.is_none());
    assert!(!r.s.can_import(), "blocked until the names are read");
    r.frames(2);
    assert!(r.s.lookups.is_some());
    assert!(r.s.can_import());
}

#[test]
fn imports_items_with_exact_prices() {
    let mut r = Rig::new();
    r.s.set_list(ListChoice::Items);
    r.frames(2);
    assert!(r.s.lookups.is_some());
    assert!(r.s.default_income.is_some(), "the first income account");
    r.read(
        "items.csv",
        "Item,Description,Price,Taxable\nStump grinding,Per hour,\"$1,045.0450\",No\nFree thing,,-1,\n",
    );
    assert_eq!(r.state("ready"), 1);
    assert_eq!(r.state("left_out"), 1);
    r.frames(2);
    r.s.start_import();
    r.frames(2);
    assert_eq!(r.state("imported"), 1);
    let price = r.link.demo_state().and_then(|mut d| {
        d.items_mut()
            .iter()
            .find(|i| i.fields.name == "Stump grinding")
            .and_then(|i| i.fields.unit_price)
    });
    assert_eq!(price, Some(Money::parse_wire("1045.0450").expect("m")));
    // The name is now taken: the same row is left out next time.
    r.read("items2.csv", "Item\nstump grinding\n");
    assert_eq!(r.state("ready"), 0);
    assert_eq!(r.state("left_out"), 1);
}

#[test]
fn unreadable_files_say_why() {
    let mut r = Rig::new();
    r.s.file = r.dir.path().join("missing.csv").display().to_string();
    r.s.read_file();
    assert_eq!(r.state("read_error"), true);
    r.read("empty.csv", "\n\n");
    assert_eq!(r.state("read_error"), true);
    r.read("quote.csv", "Name\n\"open\n");
    assert_eq!(r.state("read_error"), true);
    r.read("header.csv", "Name,Type\n");
    assert_eq!(r.state("read_error"), false);
    assert_eq!(r.state("problems"), 1, "no rows below the header");
    r.frames(2);
    assert_ne!(r.s.title(), "");
}
