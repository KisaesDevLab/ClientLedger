//! Item List screen tests (headless, demo link).

use std::sync::Arc;

use cl_api::{ApiError, ConflictReason, UserText};
use cl_core::ids::TenantId;
use cl_model::CommandId;
use cl_model::forms::SaveIntent;
use cl_model::lists::items::{ItemKind, ItemProblem};
use eframe::egui::{self, Pos2, RawInput, Rect, vec2};

use super::{FormPanel, ItemsScreen};
use crate::company::CompanyLink;
use crate::company::demo::account_id;
use crate::company::demo::lists::item_id;
use crate::shell::Shell;
use crate::windows::Screen;

const TENANT: &str = "d0000000-0000-4000-8000-000000000001";

struct Rig {
    _dir: tempfile::TempDir,
    ctx: egui::Context,
    link: Arc<CompanyLink>,
    s: ItemsScreen,
}

impl Rig {
    fn new() -> Rig {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let id = TenantId::parse(TENANT).expect("id");
        let ctx = egui::Context::default();
        Shell::install(&ctx);
        cl_grid::sequencer::install(&ctx);
        let s = ItemsScreen::new(Arc::clone(&link), Some((id, "Ozark".into())));
        let mut rig = Rig {
            _dir: dir,
            ctx,
            link,
            s,
        };
        rig.frames(3);
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

    fn form(&mut self) -> &mut FormPanel {
        self.s.panel.as_mut().expect("no form open")
    }

    fn state(&self, key: &str) -> serde_json::Value {
        self.s.state()[key].clone()
    }

    fn cmd(&mut self, id: &'static str) -> bool {
        let r = self.s.command(CommandId::new(id));
        self.frames(2);
        r
    }

    fn select(&mut self, n: u32) {
        self.s.place_cursor(Some(item_id(n)));
        self.frames(2);
        assert_eq!(self.s.selected(), Some(item_id(n)));
    }
}

#[test]
fn loads_the_demo_items_without_the_inactive_one() {
    let r = Rig::new();
    assert_eq!(r.state("screen"), "items");
    assert_eq!(r.state("loaded"), true);
    assert_eq!(r.state("rows"), 8);
    assert_eq!(r.state("total"), 9);
    // The demo engine keeps the sales kinds (U-39a) and price levels (U-40).
    assert_eq!(r.state("kinds_available"), 5);
    assert_eq!(r.state("price_levels_available"), true);
    assert_eq!(r.state("panel"), "none");
    assert_eq!(r.s.title(), "Item List");
    assert!(!r.s.dirty());
    // Prices are the engine's exact strings.
    let labels: Vec<String> = r.s.model.rows().iter().map(|x| x.label.clone()).collect();
    assert!(
        labels.iter().any(|l| l == "Design Consultation, 0.045"),
        "{labels:?}"
    );
    assert!(labels.iter().any(|l| l == "Weekly Mowing, 45.00"));
}

#[test]
fn create_with_save_and_close_then_edit() {
    let mut r = Rig::new();
    assert!(r.cmd("list.new"));
    assert_eq!(r.state("panel"), "new");
    {
        let f = r.form();
        f.form
            .edit(super::F_NAME, |d| d.name = "Leaf Removal".into());
        f.form.edit(super::F_PRICE, |d| d.price = "120.5".into());
    }
    assert!(r.s.dirty());
    assert!(r.s.save_form(SaveIntent::SaveAndClose));
    r.frames(2);
    assert_eq!(r.state("panel"), "none");
    assert_eq!(r.state("note"), "saved");
    assert_eq!(r.state("rows"), 9); // with the demo subtotal and discount (U-39a)
    assert_eq!(r.state("selected_price"), "120.5000");
    assert_eq!(r.state("selected_active"), true);
    let made = r.s.selected().expect("selected");
    let demo = r.link.demo_state().expect("demo").items_mut().clone();
    let saved = demo.iter().find(|i| i.id == made).expect("kept");
    assert_eq!(saved.fields.income_account_id, account_id(4000));
    assert!(saved.fields.is_taxable);

    assert!(r.cmd("edit.edit_transaction"));
    assert_eq!(r.state("panel"), "edit");
    r.form().form.edit(super::F_TAXABLE, |d| d.taxable = false);
    assert!(r.cmd("form.save_close"));
    let demo = r.link.demo_state().expect("demo").items_mut().clone();
    assert!(
        !demo
            .iter()
            .find(|i| i.id == made)
            .expect("kept")
            .fields
            .is_taxable
    );
}

#[test]
fn save_and_new_keeps_the_income_account_and_problems_focus_their_field() {
    let mut r = Rig::new();
    r.cmd("list.new");
    r.form()
        .form
        .edit(super::F_NAME, |d| d.name = "weekly mowing".into());
    r.form()
        .form
        .edit(super::F_PRICE, |d| d.price = "-4".into());
    assert!(!r.s.save_form(SaveIntent::SaveAndNew));
    assert_eq!(
        r.form().problems,
        vec![ItemProblem::NameTaken, ItemProblem::PriceNegative]
    );
    assert_eq!(r.state("problem"), "name_taken");
    r.form()
        .form
        .edit(super::F_NAME, |d| d.name = "Gutter Cleaning".into());
    r.form()
        .form
        .edit(super::F_PRICE, |d| d.price = "40/3".into());
    assert!(r.s.save_form(SaveIntent::SaveAndNew));
    r.frames(2);
    assert_eq!(r.state("panel"), "new");
    assert_eq!(
        r.form().form.current().income_account,
        Some(account_id(4000))
    );
    let demo = r.link.demo_state().expect("demo").items_mut().clone();
    let g = demo
        .iter()
        .find(|i| i.fields.name == "Gutter Cleaning")
        .expect("saved");
    assert_eq!(
        g.fields.unit_price.map(cl_core::Money::to_wire).as_deref(),
        Some("13.3333"),
        "a calculated price keeps four decimals, not cents"
    );
    // A discount item (U-39a): a percent and an amount off together are
    // refused; the percent alone saves.
    r.form()
        .form
        .edit(super::F_KIND, |d| d.kind = ItemKind::Discount);
    r.form()
        .form
        .edit(super::F_NAME, |d| d.name = "Promo".into());
    r.form().form.edit(super::F_PRICE, |d| d.price = "5".into());
    r.form()
        .form
        .edit(super::F_DISCOUNT, |d| d.discount = "10".into());
    assert!(!r.s.save_form(SaveIntent::SaveAndClose));
    assert_eq!(r.form().problems, vec![ItemProblem::DiscountBoth]);
    r.form().form.edit(super::F_PRICE, |d| d.price.clear());
    assert!(r.s.save_form(SaveIntent::SaveAndClose));
    let saved = r.link.demo_state().expect("demo").items_mut().clone();
    let promo = saved
        .iter()
        .find(|i| i.fields.name == "Promo")
        .expect("saved");
    assert_eq!(promo.fields.item_type, cl_api::ItemType::Discount);
}

#[test]
fn esc_reverts_then_closes_then_clears_the_search() {
    let mut r = Rig::new();
    r.select(1);
    r.cmd("edit.edit_transaction");
    r.form().form.edit(super::F_NAME, |d| d.name.push('!'));
    assert_eq!(r.state("dirty"), true);
    assert!(r.cmd("form.revert"));
    assert_eq!(r.state("dirty"), false);
    assert_eq!(r.state("panel"), "edit");
    assert!(r.cmd("form.revert"));
    assert_eq!(r.state("panel"), "none");
    r.s.filter.search = "mulch".into();
    r.s.refilter();
    r.frames(1);
    assert_eq!(r.state("rows"), 1);
    assert!(r.cmd("form.revert"));
    assert_eq!(r.state("rows"), 8);
    assert!(
        !r.cmd("form.revert"),
        "nothing left: the shell closes the window"
    );
}

#[test]
fn make_inactive_and_active_again() {
    let mut r = Rig::new();
    r.select(2);
    r.cmd("list.toggle_active");
    assert_eq!(r.state("note"), "made_inactive");
    assert_eq!(r.state("rows"), 7);
    r.cmd("list.show_inactive");
    assert_eq!(r.state("show_inactive"), true);
    assert_eq!(r.s.selected(), Some(item_id(2)), "the cursor comes back");
    assert_eq!(r.state("selected_active"), false);
    r.cmd("list.toggle_active");
    assert_eq!(r.state("note"), "made_active");
    assert_eq!(r.state("selected_active"), true);
    r.cmd("list.show_inactive");
    assert_eq!(r.state("rows"), 8);
}

#[test]
fn warnings_without_a_selection_and_with_a_dirty_form() {
    let mut r = Rig::new();
    r.s.open_edit(None);
    assert_eq!(r.state("warning"), "select_first");
    r.cmd("list.new");
    r.form().form.edit(super::F_NAME, |d| d.name = "x".into());
    r.cmd("list.new");
    assert_eq!(r.state("warning"), "finish_first");
    assert!(r.cmd("edit.undo"));
    assert!(r.cmd("edit.redo"));
    assert!(r.cmd("edit.find"));
    assert!(r.cmd("form.next_record") || !r.cmd("form.prev_record"));
    assert!(!r.cmd("no.such.command"));
}

#[test]
fn an_engine_name_conflict_reads_like_the_local_check() {
    let mut r = Rig::new();
    r.cmd("list.new");
    r.form().save_failed(ApiError::Conflict {
        reason: ConflictReason::AlreadyExists,
        current_version: None,
        code: None,
        message: UserText::default(),
    });
    assert_eq!(r.form().problems, vec![ItemProblem::NameTaken]);
    r.form().save_failed(ApiError::NotFound { code: None });
    assert!(r.form().error.is_some());
}
